use anyhow::{Context, anyhow};
use common::{
    AuditSnapshotEntry, GovernanceAuditEntry, SignedAuditSnapshot, compute_audit_chain_digest,
    mask_reason, sign_audit_snapshot,
};
use libsql::Builder;
use std::{fs::OpenOptions, io::Write};

#[derive(Debug, Default)]
struct Args {
    database_url: Option<String>,
    signing_key: Option<String>,
    output: Option<String>,
    include_reasons: bool,
    help: bool,
}

fn print_help() {
    eprintln!(
        "Usage: sign_audit_export --signing-key PEM_PATH --output PATH [--database-url URL] [--include-reasons]\n\
         \n\
         Creates a signed snapshot of content_audit_log and governance_audit_log. The signing key is a\n\
         PKCS#8 RSA private key PEM file. Never pass the key itself on the CLI.\n\
         The PEM must be owner-only (0600/0400) and its directory must not be group/other writable\n\
         (0700 recommended). The remote libSQL token is read only from the DATABASE_TOKEN\n\
         environment variable; --database-token is intentionally not accepted (argv is public).\n\
         Reasons/details are redacted by default; --include-reasons exposes them (opt-in).\n\
         Defaults: DATABASE_URL (or file:local.db), DATABASE_TOKEN (or empty).\n\
         Exit code: 0 on success, 1 on error."
    );
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => args.help = true,
            "--database-url" => {
                args.database_url = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("--database-url requires a value"))?,
                );
            }
            // 認証トークンを argv（ps 等で公開される）に載せないため、環境変数のみ受け付ける。
            "--database-token" => {
                return Err(anyhow!(
                    "--database-token is not accepted; set DATABASE_TOKEN in the environment instead"
                ));
            }
            "--signing-key" => {
                args.signing_key = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("--signing-key requires a value"))?,
                );
            }
            "--output" => {
                args.output = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("--output requires a value"))?,
                );
            }
            "--include-reasons" => args.include_reasons = true,
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

/// 署名鍵の権限を検査する。PEM が 0644 等で露出していないことを保証する。
/// ファイルは owner-only（0600/0400）を要求し、親ディレクトリが group/other
/// 書き込み可能な場合は拒否、0700 でない場合は警告する。
fn check_signing_key_permissions(path: &str) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        // canonicalize してシンボリックリンク先の実ファイル/ディレクトリを検査する。
        let resolved = std::fs::canonicalize(path)
            .with_context(|| format!("failed to resolve signing key: {path}"))?;
        let mode = std::fs::metadata(&resolved)
            .with_context(|| format!("failed to stat signing key: {path}"))?
            .mode()
            & 0o777;
        if mode & 0o077 != 0 {
            return Err(anyhow!(
                "signing key {path} has permissions {mode:04o}; require owner-only 0600 (chmod 600)"
            ));
        }

        if let Some(parent) = resolved.parent() {
            let parent_mode = std::fs::metadata(parent)
                .with_context(|| {
                    format!("failed to stat signing key directory: {}", parent.display())
                })?
                .mode()
                & 0o777;
            if parent_mode & 0o022 != 0 {
                return Err(anyhow!(
                    "signing key directory {} has permissions {parent_mode:04o}; group/other writable is not allowed",
                    parent.display()
                ));
            }
            if parent_mode & 0o055 != 0 {
                eprintln!(
                    "warning: signing key directory {} has permissions {parent_mode:04o}; 0700 is recommended",
                    parent.display()
                );
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    if args.help {
        print_help();
        return Ok(());
    }
    let signing_key_path = args
        .signing_key
        .ok_or_else(|| anyhow!("--signing-key is required"))?;
    let output = args.output.ok_or_else(|| anyhow!("--output is required"))?;
    check_signing_key_permissions(&signing_key_path)?;
    let private_key_pem =
        std::fs::read_to_string(&signing_key_path).context("failed to read signing key")?;

    let database_url = args
        .database_url
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .unwrap_or_else(|| "file:local.db".to_string());
    // argv はプロセス一覧に露出するため、トークンは環境変数からのみ読む。
    let database_token = std::env::var("DATABASE_TOKEN").unwrap_or_default();

    let db = if database_url.starts_with("file:") || !database_url.contains("://") {
        let path = database_url.strip_prefix("file:").unwrap_or(&database_url);
        Builder::new_local(path).build().await?.connect()?
    } else {
        Builder::new_remote(database_url, database_token)
            .build()
            .await?
            .connect()?
    };

    let mut rows = db
        .query(
            "SELECT log_id, cid, old_status, new_status, changed_by, reason, previous_log_hash, entry_hash \
             FROM content_audit_log ORDER BY log_id ASC",
            (),
        )
        .await
        .context("failed to query audit log")?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next().await.context("failed to read audit row")? {
        entries.push(AuditSnapshotEntry {
            log_id: row.get(0)?,
            cid: row.get(1)?,
            old_status: row.get(2)?,
            new_status: row.get(3)?,
            changed_by: row.get(4)?,
            reason: mask_reason(&row.get::<String>(5)?, args.include_reasons),
            previous_log_hash: row.get(6)?,
            entry_hash: row.get(7)?,
        });
    }
    let digest = compute_audit_chain_digest(
        &entries
            .iter()
            .map(|entry| (entry.log_id, entry.entry_hash.clone()))
            .collect::<Vec<_>>(),
    )
    .map_err(anyhow::Error::msg)?;

    // ADR-0004 §10: BAN / UNBAN / ピン解除の運用判断も署名対象に含める。
    let mut gov_rows = db
        .query(
            "SELECT log_id, action, target, actor_hash, reason, details, created_at \
             FROM governance_audit_log ORDER BY log_id ASC",
            (),
        )
        .await
        .context("failed to query governance audit log")?;
    let mut governance_entries = Vec::new();
    while let Some(row) = gov_rows
        .next()
        .await
        .context("failed to read governance row")?
    {
        governance_entries.push(GovernanceAuditEntry {
            log_id: row.get(0)?,
            action: row.get(1)?,
            target: row.get(2)?,
            actor_hash: row.get(3)?,
            reason: mask_reason(&row.get::<String>(4)?, args.include_reasons),
            details: row
                .get::<Option<String>>(5)?
                .map(|details| mask_reason(&details, args.include_reasons)),
            created_at: row.get(6)?,
        });
    }

    let mut snapshot = SignedAuditSnapshot {
        version: common::AUDIT_SNAPSHOT_VERSION,
        reasons_masked: !args.include_reasons,
        entries,
        governance_entries,
        merkle_root: digest.root_hash,
        signature: String::new(),
    };
    snapshot.signature =
        sign_audit_snapshot(&snapshot, &private_key_pem).map_err(|error| anyhow!("{error}"))?;
    let json = serde_json::to_string_pretty(&snapshot).context("failed to serialize snapshot")?;
    // create_new で存在チェックと生成を原子的に行い、TOCTOU で既存ファイルを
    // 上書きしないようにする。
    let mut output_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .context("failed to create snapshot output (it must not already exist)")?;
    output_file
        .write_all(json.as_bytes())
        .context("failed to write snapshot")?;
    println!(
        "audit_export_signed: output={output} entries={}",
        snapshot.entry_count()
    );
    Ok(())
}
