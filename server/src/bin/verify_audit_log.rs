use anyhow::{Context, anyhow};
use common::{AuditLogEntry, compute_audit_chain_digest};
use libsql::Builder;

#[derive(Debug, Default)]
struct Args {
    database_url: Option<String>,
    database_token: Option<String>,
    cid: Option<String>,
    expect_root: Option<String>,
    help: bool,
}

fn print_help() {
    eprintln!(
        "Usage: verify_audit_log [--database-url URL] [--database-token TOKEN] [--cid CID] [--expect-root HEX]\n\
         \n\
         Verifies content_audit_log hash-chain integrity (read-only).
         Accepts v2 (length-prefixed) and legacy v1 entry hashes.\n\
         Also checks contiguous log_id and prints a Merkle root over entry hashes.\n\
         --expect-root compares the root against a previously recorded value to detect\n\
         truncation or omission (the root must have been published out of band).\n\
         Defaults: DATABASE_URL (or file:local.db), DATABASE_TOKEN (or empty).\n\
         Exit code: 0 if valid, 1 if invalid or on error."
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
            "--cid" => {
                args.cid = Some(it.next().ok_or_else(|| anyhow!("--cid requires a value"))?);
            }
            "--expect-root" => {
                args.expect_root = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("--expect-root requires a value"))?,
                );
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

async fn open_db(
    database_url: String,
    database_token: String,
) -> anyhow::Result<libsql::Connection> {
    let conn = if database_url.starts_with("file:") || !database_url.contains("://") {
        let path = database_url.strip_prefix("file:").unwrap_or(&database_url);
        Builder::new_local(path).build().await?.connect()?
    } else {
        Builder::new_remote(database_url, database_token)
            .build()
            .await?
            .connect()?
    };
    Ok(conn)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    if args.help {
        print_help();
        return Ok(());
    }

    let database_url = args
        .database_url
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .unwrap_or_else(|| "file:local.db".to_string());
    let database_token = args
        .database_token
        .or_else(|| std::env::var("DATABASE_TOKEN").ok())
        .unwrap_or_else(String::new);

    let db = open_db(database_url, database_token)
        .await
        .context("failed to open database")?;

    let mut rows = db
        .query(
            "SELECT log_id, cid, old_status, new_status, changed_by, reason, previous_log_hash, entry_hash \
             FROM content_audit_log ORDER BY log_id ASC",
            (),
        )
        .await
        .context("failed to query content_audit_log (is the database migrated?)")?;

    let mut expected_prev = "GENESIS_HASH".to_string();
    let mut total: usize = 0;
    let mut shown: usize = 0;
    let mut errors: Vec<String> = Vec::new();
    let mut chain_entries: Vec<(i64, String)> = Vec::new();

    while let Some(row) = rows.next().await.context("failed to read audit log row")? {
        let log_id: i64 = row.get(0).context("log_id")?;
        let cid: String = row.get(1).context("cid")?;
        let old_status: String = row.get(2).context("old_status")?;
        let new_status: String = row.get(3).context("new_status")?;
        let changed_by: String = row.get(4).context("changed_by")?;
        let reason: String = row.get(5).context("reason")?;
        let previous_log_hash: String = row.get(6).context("previous_log_hash")?;
        let entry_hash: String = row.get(7).context("entry_hash")?;

        total += 1;

        let entry = AuditLogEntry {
            log_id: Some(log_id),
            cid: cid.clone(),
            old_status: old_status.clone(),
            new_status: new_status.clone(),
            changed_by: changed_by.clone(),
            reason: reason.clone(),
            previous_log_hash: previous_log_hash.clone(),
        };
        let computed_v1 = entry.calculate_hash();
        let computed_v2 = entry.calculate_hash_v2();
        let hash_ok = computed_v2 == entry_hash;
        let legacy_v1 = computed_v1 == entry_hash;
        let chain_ok = previous_log_hash == expected_prev;

        if !hash_ok && !legacy_v1 {
            errors.push(format!(
                "log_id={log_id} cid={cid}: entry_hash mismatch (stored={entry_hash}, v2={computed_v2}, v1={computed_v1})"
            ));
        }
        if !chain_ok {
            errors.push(format!(
                "log_id={log_id} cid={cid}: previous_log_hash mismatch (stored={previous_log_hash}, expected={expected_prev})"
            ));
        }

        let include = args.cid.as_ref().map(|c| c == &cid).unwrap_or(true);
        if include {
            shown += 1;
            println!(
                "log_id={log_id} cid={cid} {old_status}->{new_status} by={changed_by} reason={reason} hash_ok={hash_ok} legacy_v1={legacy_v1} chain_ok={chain_ok} entry_hash={entry_hash}"
            );
        }

        expected_prev = entry_hash.clone();
        chain_entries.push((log_id, entry_hash));
    }

    if args.cid.is_some() {
        println!("shown={shown} total={total}");
    } else {
        println!("total={total}");
    }

    match compute_audit_chain_digest(&chain_entries) {
        Ok(digest) => {
            println!(
                "audit_chain_digest: count={} first={:?} last={:?} root={}",
                digest.count, digest.first_log_id, digest.last_log_id, digest.root_hash
            );
            if let Some(expected) = args.expect_root.as_ref() {
                if expected.eq_ignore_ascii_case(&digest.root_hash) {
                    println!("audit_root_verification: OK");
                } else {
                    errors.push(format!(
                        "audit root mismatch: expected={expected}, computed={}",
                        digest.root_hash
                    ));
                }
            }
        }
        Err(error) => errors.push(format!("audit chain digest error: {error}")),
    }

    if errors.is_empty() {
        println!("audit_log_verification: OK");
        Ok(())
    } else {
        for e in &errors {
            eprintln!("ERROR: {e}");
        }
        eprintln!("audit_log_verification: FAILED ({} error(s))", errors.len());
        std::process::exit(1);
    }
}
