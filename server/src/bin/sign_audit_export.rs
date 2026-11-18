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

fn parse_args_from<I>(items: I) -> anyhow::Result<Args>
where
    I: IntoIterator<Item = String>,
{
    let mut args = Args::default();
    let mut it = items.into_iter().skip(1);
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

fn parse_args() -> anyhow::Result<Args> {
    parse_args_from(std::env::args())
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

/// 既存ファイルを決して上書きせずに出力ファイルを生成する（TOCTOU 対策）。
fn create_snapshot_output(path: &str) -> anyhow::Result<std::fs::File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .context("failed to create snapshot output (it must not already exist)")
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
    let mut output_file = create_snapshot_output(&output)?;
    output_file
        .write_all(json.as_bytes())
        .context("failed to write snapshot")?;
    println!(
        "audit_export_signed: output={output} entries={}",
        snapshot.entry_count()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check_signing_key_permissions, create_snapshot_output, parse_args_from};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    /// 一意な一時ディレクトリを表し、Drop で確実に削除するテストヘルパ。
    /// 権限変更はテスト後に戻さなくても、ディレクトリごと削除される。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "sign-audit-export-test-{}-{nanos}-{seq}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("create temp dir");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                    .expect("set temp dir mode");
            }
            Self(path)
        }

        fn child(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn path_str(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    fn write_key(path: &Path, mode: u32) {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)
            .expect("create key file");
        file.write_all(b"dummy pkcs8 key").expect("write key file");
        // umask に依存せず正確なモードにする。
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("set key mode");
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("set path mode");
    }

    // --- check_signing_key_permissions: 鍵ファイル本体 ---

    #[cfg(unix)]
    #[test]
    fn owner_only_key_is_accepted() {
        let dir = TempDir::new();
        let key = dir.child("key.pem");
        write_key(&key, 0o600);
        check_signing_key_permissions(&path_str(&key)).expect("0600 key must be accepted");
    }

    #[cfg(unix)]
    #[test]
    fn world_readable_key_is_rejected() {
        let dir = TempDir::new();
        let key = dir.child("key.pem");
        write_key(&key, 0o644);
        let error =
            check_signing_key_permissions(&path_str(&key)).expect_err("0644 key must be rejected");
        let message = error.to_string();
        assert!(message.contains("permissions"), "{message}");
        assert!(message.contains("0600"), "{message}");
    }

    // --- check_signing_key_permissions: 親ディレクトリ ---

    #[cfg(unix)]
    #[test]
    fn group_or_other_writable_parent_is_rejected() {
        let dir = TempDir::new();
        let parent = dir.child("keys");
        std::fs::create_dir(&parent).expect("create keys dir");
        let key = parent.join("key.pem");
        write_key(&key, 0o600);
        set_mode(&parent, 0o777);
        let error = check_signing_key_permissions(&path_str(&key))
            .expect_err("group/other writable parent must be rejected");
        assert!(
            error.to_string().contains("group/other writable"),
            "{error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_0700_parent_is_only_warned() {
        let dir = TempDir::new();
        let parent = dir.child("keys");
        std::fs::create_dir(&parent).expect("create keys dir");
        let key = parent.join("key.pem");
        write_key(&key, 0o600);
        set_mode(&parent, 0o755);
        // 警告は stderr に出るがテストでは捕捉しない。受理されることだけ確認する。
        check_signing_key_permissions(&path_str(&key)).expect("0755 parent must only warn");
    }

    // --- check_signing_key_permissions: canonicalize ---

    #[cfg(unix)]
    #[test]
    fn symlink_target_permissions_are_checked() {
        let dir = TempDir::new();
        let parent = dir.child("keys");
        std::fs::create_dir(&parent).expect("create keys dir");
        set_mode(&parent, 0o700);
        let real = parent.join("real.pem");
        write_key(&real, 0o644);
        let link = parent.join("link.pem");
        std::os::unix::fs::symlink(&real, &link).expect("create symlink");
        let error = check_signing_key_permissions(&path_str(&link))
            .expect_err("symlink to a 0644 key must be rejected");
        assert!(error.to_string().contains("0600"), "{error:#}");
    }

    // --- parse_args_from: CLI 引数 ---

    #[test]
    fn database_token_argument_is_rejected() {
        let error =
            parse_args_from(["sign_audit_export", "--database-token", "secret"].map(String::from))
                .expect_err("--database-token must never be accepted on argv");
        assert!(error.to_string().contains("DATABASE_TOKEN"), "{error:#}");
    }

    #[test]
    fn valid_arguments_are_parsed() {
        let args = parse_args_from(
            [
                "sign_audit_export",
                "--signing-key",
                "key.pem",
                "--output",
                "snapshot.json",
                "--database-url",
                "file:test.db",
                "--include-reasons",
            ]
            .map(String::from),
        )
        .expect("valid arguments must parse");
        assert_eq!(args.signing_key.as_deref(), Some("key.pem"));
        assert_eq!(args.output.as_deref(), Some("snapshot.json"));
        assert_eq!(args.database_url.as_deref(), Some("file:test.db"));
        assert!(args.include_reasons);
        assert!(!args.help);
    }

    // --- create_snapshot_output: 既存ファイルの上書き拒否 ---

    #[test]
    fn output_refuses_to_overwrite_existing_file() {
        let dir = TempDir::new();
        let output = dir.child("snapshot.json");
        std::fs::write(&output, b"original").expect("seed output");
        let error = create_snapshot_output(&path_str(&output))
            .expect_err("existing output must be refused");
        assert!(
            error.to_string().contains("must not already exist"),
            "{error:#}"
        );
        assert_eq!(std::fs::read(&output).expect("read output"), b"original");
    }

    #[test]
    fn output_creates_new_file() {
        let dir = TempDir::new();
        let output = dir.child("snapshot.json");
        let mut file = create_snapshot_output(&path_str(&output)).expect("create new output");
        file.write_all(b"snapshot").expect("write output");
        drop(file);
        assert_eq!(std::fs::read(&output).expect("read output"), b"snapshot");
    }
}
