use anyhow::{Context, anyhow};
use libsql::Builder;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
struct Args {
    backup: Option<String>,
    target: Option<String>,
    force: bool,
    help: bool,
}

fn print_help() {
    eprintln!(
        "Usage: restore_db --backup PATH --target PATH [--force]\n\
         \n\
         Restores a backup produced by backup_db. The server must be stopped\n\
         (WAL/SHM sidecars cause an immediate refusal). By default an existing\n\
         target is never overwritten; --force keeps a timestamped safety copy.\n\
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
            "--backup" => {
                args.backup = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("--backup requires a value"))?,
                );
            }
            "--target" => {
                args.target = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("--target requires a value"))?,
                );
            }
            "--force" => args.force = true,
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

fn parse_args() -> anyhow::Result<Args> {
    parse_args_from(std::env::args())
}

fn sidecar_of(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

async fn validate_backup(path: &Path) -> anyhow::Result<(i64, i64)> {
    let db = Builder::new_local(path)
        .build()
        .await
        .context("failed to open backup")?
        .connect()?;

    let mut rows = db
        .query("PRAGMA integrity_check", ())
        .await
        .context("integrity_check failed")?;
    let result: String = rows
        .next()
        .await
        .context("no integrity_check result")?
        .context("no integrity_check row")?
        .get(0)?;
    if result != "ok" {
        return Err(anyhow!("backup integrity_check failed: {result}"));
    }

    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
            (),
        )
        .await?;
    let tables: i64 = rows.next().await?.context("no table count")?.get(0)?;
    if tables == 0 {
        return Err(anyhow!("backup has no tables"));
    }
    let mut rows = db.query("SELECT COUNT(*) FROM _migrations", ()).await?;
    let migrations: i64 = rows.next().await?.context("no migration count")?.get(0)?;
    if migrations == 0 {
        return Err(anyhow!("backup has no applied migrations"));
    }
    Ok((tables, migrations))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    if args.help {
        print_help();
        return Ok(());
    }
    let backup = PathBuf::from(args.backup.ok_or_else(|| anyhow!("--backup is required"))?);
    let target = PathBuf::from(args.target.ok_or_else(|| anyhow!("--target is required"))?);

    if !backup.exists() {
        return Err(anyhow!("backup file does not exist: {}", backup.display()));
    }
    for suffix in ["-wal", "-shm"] {
        let sidecar = sidecar_of(&target, suffix);
        if sidecar.exists() {
            return Err(anyhow!(
                "target has a {} sidecar; stop the server before restoring",
                sidecar.display()
            ));
        }
    }
    if target.exists() && !args.force {
        return Err(anyhow!(
            "target already exists: {} (pass --force to replace with a safety copy)",
            target.display()
        ));
    }

    let (tables, migrations) = validate_backup(&backup).await?;
    println!("backup_verification: OK tables={tables} migrations={migrations}");

    let parent = target
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        return Err(anyhow!(
            "target directory does not exist: {}",
            parent.display()
        ));
    }

    let safety_copy = if target.exists() {
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        let mut name = target.as_os_str().to_os_string();
        name.push(format!(".pre-restore-{stamp}"));
        let copy = PathBuf::from(name);
        std::fs::copy(&target, &copy).context("failed to create safety copy")?;
        println!("safety_copy: {}", copy.display());
        Some(copy)
    } else {
        None
    };

    let tmp = parent.join(format!(
        ".{}.restore-tmp-{}",
        target.file_name().and_then(|n| n.to_str()).unwrap_or("db"),
        std::process::id()
    ));
    if tmp.exists() {
        return Err(anyhow!("staging file already exists: {}", tmp.display()));
    }
    std::fs::copy(&backup, &tmp).context("failed to stage backup")?;
    if let Err(error) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        if let Some(copy) = safety_copy {
            let _ = std::fs::copy(&copy, &target);
        }
        return Err(error).context("failed to replace target database");
    }

    let (t, m) = validate_backup(&target).await?;
    println!(
        "restore_ok: target={} tables={t} migrations={m}",
        target.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_suffixes_are_appended_to_full_path() {
        let path = Path::new("/tmp/example.db");
        assert_eq!(
            sidecar_of(path, "-wal"),
            PathBuf::from("/tmp/example.db-wal")
        );
        assert_eq!(
            sidecar_of(path, "-shm"),
            PathBuf::from("/tmp/example.db-shm")
        );
    }

    #[test]
    fn parse_args_accepts_force_flag() {
        let args = parse_args_from(
            ["restore_db", "--backup", "b", "--target", "t", "--force"].map(String::from),
        )
        .unwrap();
        assert!(args.force);
        assert_eq!(args.backup.as_deref(), Some("b"));
        assert_eq!(args.target.as_deref(), Some("t"));
    }

    #[test]
    fn parse_args_rejects_unknown_argument() {
        assert!(parse_args_from(["restore_db", "--bogus"].map(String::from)).is_err());
    }

    #[test]
    fn parse_args_requires_both_paths() {
        assert!(
            parse_args_from(["restore_db", "--backup", "b"].map(String::from))
                .unwrap()
                .target
                .is_none()
        );
    }
}
