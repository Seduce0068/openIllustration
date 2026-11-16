use anyhow::{Context, anyhow};
use common::{
    AuditSnapshotEntry, SignedAuditSnapshot, compute_audit_chain_digest, sign_audit_snapshot,
};
use libsql::Builder;

#[derive(Debug, Default)]
struct Args {
    database_url: Option<String>,
    database_token: Option<String>,
    signing_key: Option<String>,
    output: Option<String>,
    help: bool,
}

fn print_help() {
    eprintln!(
        "Usage: sign_audit_export --signing-key PEM_PATH --output PATH [--database-url URL] [--database-token TOKEN]\n\
         \n\
         Creates a signed snapshot of content_audit_log. The signing key is a\n\
         PKCS#8 RSA private key PEM file. Never pass the key itself on the CLI.\n\
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
            "--database-token" => {
                args.database_token = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("--database-token requires a value"))?,
                );
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
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    Ok(args)
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
    if std::path::Path::new(&output).exists() {
        return Err(anyhow!("snapshot output already exists: {output}"));
    }
    let private_key_pem =
        std::fs::read_to_string(&signing_key_path).context("failed to read signing key")?;

    let database_url = args
        .database_url
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .unwrap_or_else(|| "file:local.db".to_string());
    let database_token = args
        .database_token
        .or_else(|| std::env::var("DATABASE_TOKEN").ok())
        .unwrap_or_default();

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
            reason: row.get(5)?,
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

    let mut snapshot = SignedAuditSnapshot {
        version: 2,
        entries,
        merkle_root: digest.root_hash,
        signature: String::new(),
    };
    snapshot.signature =
        sign_audit_snapshot(&snapshot, &private_key_pem).map_err(|error| anyhow!("{error}"))?;
    let json = serde_json::to_string_pretty(&snapshot).context("failed to serialize snapshot")?;
    std::fs::write(&output, json).context("failed to write snapshot")?;
    println!(
        "audit_export_signed: output={output} entries={}",
        snapshot.entry_count()
    );
    Ok(())
}
