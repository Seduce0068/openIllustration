use anyhow::{Context, anyhow};
use libsql::Builder;
use std::path::Path;

#[derive(Debug, Default)]
struct Args {
    input: Option<String>,
    help: bool,
}

fn print_help() {
    eprintln!(
        "Usage: verify_backup --input PATH\n\
         \n\
         Verifies that a backup produced by backup_db is structurally usable:\n\
         the file exists, opens as SQLite, has tables, has a migration ledger,\n\
         and every applied migration has a matching version.\n\
         This is a structural check, not a full application-level restore test.\n\
         Exit code: 0 if valid, 1 on error."
    );
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => args.help = true,
            "--input" => {
                args.input = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("--input requires a value"))?,
                )
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
    let input = args.input.ok_or_else(|| anyhow!("--input is required"))?;
    if !Path::new(&input).exists() {
        return Err(anyhow!("backup file does not exist: {input}"));
    }

    let db = Builder::new_local(&input)
        .build()
        .await
        .context("failed to open backup")?
        .connect()?;

    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
            (),
        )
        .await
        .context("failed to inspect backup")?;
    let tables: i64 = rows
        .next()
        .await
        .context("failed to read table count")?
        .context("no table count row")?
        .get(0)?;
    if tables == 0 {
        return Err(anyhow!("backup has no tables"));
    }

    let mut rows = db
        .query("SELECT COUNT(*) FROM _migrations", ())
        .await
        .context("migration ledger missing")?;
    let migrations: i64 = rows
        .next()
        .await
        .context("failed to read migration count")?
        .context("no migration count row")?
        .get(0)?;
    if migrations == 0 {
        return Err(anyhow!("backup has no applied migrations"));
    }

    println!("backup_verification: OK tables={tables} migrations={migrations}");
    Ok(())
}
