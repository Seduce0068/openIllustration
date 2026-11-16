use anyhow::{Context, anyhow};
use libsql::Builder;
use std::path::Path;

#[derive(Debug, Default)]
struct Args {
    database_url: Option<String>,
    database_token: Option<String>,
    output: Option<String>,
    help: bool,
}

fn print_help() {
    eprintln!(
        "Usage: backup_db --output PATH [--database-url URL] [--database-token TOKEN]\n\
         \n\
         Creates a consistent SQLite snapshot of a local libSQL database using\n\
         VACUUM INTO, then verifies tables and the migration ledger.\n\
         The output file must not already exist (no overwrite).\n\
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
            "--database-token" => {
                args.database_token = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("--database-token requires a value"))?,
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

async fn open_db(
    database_url: String,
    database_token: String,
) -> anyhow::Result<libsql::Connection> {
    if database_url.starts_with("file:") || !database_url.contains("://") {
        let path = database_url.strip_prefix("file:").unwrap_or(&database_url);
        Ok(Builder::new_local(path).build().await?.connect()?)
    } else {
        Ok(Builder::new_remote(database_url, database_token)
            .build()
            .await?
            .connect()?)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    if args.help {
        print_help();
        return Ok(());
    }

    let output = args.output.ok_or_else(|| anyhow!("--output is required"))?;
    let output_path = Path::new(&output);
    if output_path.exists() {
        return Err(anyhow!("backup output already exists: {output}"));
    }
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        return Err(anyhow!(
            "backup output directory does not exist: {}",
            parent.display()
        ));
    }

    // VACUUM INTO accepts a SQL string literal; reject quotes to avoid injection.
    if output.contains('\'') {
        return Err(anyhow!(
            "backup output path must not contain a single quote"
        ));
    }

    let database_url = args
        .database_url
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .unwrap_or_else(|| "file:local.db".to_string());
    let database_token = args
        .database_token
        .or_else(|| std::env::var("DATABASE_TOKEN").ok())
        .unwrap_or_default();

    let db = open_db(database_url, database_token)
        .await
        .context("failed to open database")?;
    db.execute(&format!("VACUUM INTO '{output}'"), ())
        .await
        .context("VACUUM INTO failed")?;

    // Verify the snapshot can be opened and the migration ledger is intact.
    let snapshot = Builder::new_local(&output)
        .build()
        .await
        .context("failed to open backup for verification")?
        .connect()?;
    let mut rows = snapshot
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
            (),
        )
        .await
        .context("failed to inspect backup")?;
    let tables: i64 = rows
        .next()
        .await
        .context("failed to read backup inspection")?
        .context("no inspection row")?
        .get(0)?;
    if tables == 0 {
        return Err(anyhow!("backup has no tables"));
    }

    // マイグレーション台帳がスナップショットへ含まれていることを確認する
    // （コメント/ヘルプの「ledger is intact」を実際に保証する）。
    let mut rows = snapshot
        .query("SELECT COUNT(*) FROM _migrations", ())
        .await
        .context("migration ledger missing in backup")?;
    let migrations: i64 = rows
        .next()
        .await
        .context("failed to read migration ledger")?
        .context("no migration ledger row")?
        .get(0)?;
    if migrations == 0 {
        return Err(anyhow!("backup has no applied migrations"));
    }
    println!("backup_ok: output={output} tables={tables} migrations={migrations}");
    Ok(())
}
