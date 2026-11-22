use anyhow::{Context, anyhow};
use common::env::read_env;
use common::instance_lock::{instance_lock_owner_alive, parse_instance_lock};
use libsql::Builder;
use server::cli_args;
use server::resolve_database_url;
use std::path::Path;

#[derive(Debug, Default)]
struct Args {
    database_url: Option<String>,
    output: Option<String>,
    help: bool,
}

const HELP: &str = "Usage: backup_db --output PATH [--database-url URL]\n\
     \n\
     Creates a consistent SQLite snapshot of a local libSQL database using\n\
     VACUUM INTO, then verifies tables and the migration ledger.\n\
     The output file must not already exist (no overwrite).\n\
     The remote libSQL token is read only from the DATABASE_TOKEN environment\n\
     variable (argv is public, so secrets are never accepted there).\n\
     DATABASE_URL is required: pass --database-url or set DATABASE_URL; no default is used.\n\
     DATABASE_TOKEN is read only from the environment (or left empty).\n\
     Exit code: 0 on success, 1 on error.";

fn print_help() {
    eprintln!("{HELP}");
}

/// argv の解釈だけを行う純粋関数。環境変数・プロセス引数を読まないため、
/// テストから直接呼べる。認証トークンは argv で受け付けない。
fn parse_args_from<I>(items: I) -> anyhow::Result<Args>
where
    I: IntoIterator<Item = String>,
{
    cli_args::parse_args_from(items, parse_str_args)
}

/// 受け付けるフラグの定義は bin 側に残す。値取り・秘密値拒否・unknown 固定文言は
/// `server::cli_args` の共有実装を呼ぶだけ。
fn parse_str_args(items: &[String]) -> anyhow::Result<Args> {
    cli_args::parse_str_args_with(items, Args::default(), |args, arg, it| match arg {
        "-h" | "--help" => {
            args.help = true;
            Ok(true)
        }
        "--database-url" => {
            args.database_url = Some(cli_args::take_value(it, "--database-url")?.to_string());
            Ok(true)
        }
        "--output" => {
            args.output = Some(cli_args::take_value(it, "--output")?.to_string());
            Ok(true)
        }
        _ => Ok(false),
    })
}

fn parse_args() -> anyhow::Result<Args> {
    parse_args_from(std::env::args())
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

/// `DATABASE_URL` がローカル DB を指す場合、そのファイルパスを返す。
/// `open_db` と同じ判定（remote DB はインスタンスロックの対象外）。
fn local_db_path(database_url: &str) -> Option<&str> {
    if database_url.starts_with("file:") || !database_url.contains("://") {
        Some(database_url.strip_prefix("file:").unwrap_or(database_url))
    } else {
        None
    }
}

/// `<db>.instance.lock` が生存中のサーバーに保持されていないか検査する。
/// ロックが無い、または明らかに stale（PID 不存在）なら続行を許可する。
/// ロックの読み取りや生存判定が不能な場合は拒否する（fail-closed）。
fn ensure_no_live_instance(db_path: &str) -> anyhow::Result<()> {
    let lock_path = format!("{db_path}.instance.lock");
    let lock = Path::new(&lock_path);
    match std::fs::symlink_metadata(lock) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot inspect instance lock: {lock_path}"));
        }
    }
    let contents = std::fs::read_to_string(lock)
        .with_context(|| format!("cannot read instance lock: {lock_path}"))?;
    let (pid, starttime) = parse_instance_lock(&contents);
    if instance_lock_owner_alive(pid, starttime) {
        anyhow::bail!(
            "refusing backup: a live server instance holds the instance lock \
             ({lock_path}, pid {pid}); stop the server or wait until startup/migration has finished"
        );
    }
    Ok(())
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

    // 暗黙の file:local.db 既定は撤廃。--database-url または DATABASE_URL の
    // 明示設定がなければ起動しない（deploy 設定例は既に明示済み）。
    let database_url = resolve_database_url(args.database_url, read_env("DATABASE_URL"))
        .map_err(|error| anyhow!(error.to_string()))?;
    // argv はプロセス一覧に露出するため、トークンは環境変数からのみ読む。
    let database_token = std::env::var("DATABASE_TOKEN").unwrap_or_default();

    // サーバー起動時（特にマイグレーション実行中）は VACUUM INTO と競合し、
    // BUSY や旧スナップショットになる恐れがある。サーバーが <db>.instance.lock
    // を保持していないことを確認してからバックアップを開始する。
    if let Some(local_path) = local_db_path(&database_url) {
        ensure_no_live_instance(local_path)?;
    }
    // 出力先 DB が別のサーバーの DB として使われている場合も拒否する。
    ensure_no_live_instance(&output)?;

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

// read_proc_starttime はテスト専用（本番経路は common の instance_lock_owner_alive が参照）。
#[cfg(test)]
use common::instance_lock::read_proc_starttime;

#[cfg(test)]
mod tests {
    use super::{
        HELP, ensure_no_live_instance, local_db_path, parse_args_from, parse_instance_lock,
        read_proc_starttime, resolve_database_url,
    };
    use common::env::EnvValue;
    use server::DatabaseUrlError;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "backup-db-lock-test-{}-{nanos}-{seq}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("create temp dir");
            Self(path)
        }

        fn db(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_lock(db_path: &str, contents: &str) {
        std::fs::write(format!("{db_path}.instance.lock"), contents).expect("write lock");
    }

    /// 実在しない PID を返す（stale ロックのテスト用）。
    fn unused_pid() -> u32 {
        (1..u32::MAX)
            .rev()
            .find(|pid| !Path::new(&format!("/proc/{pid}")).exists())
            .expect("find an unused pid")
    }

    // --- parse_args_from: 認証トークンを argv で受け付けない ---

    #[test]
    fn database_token_argument_is_rejected_without_leaking_value() {
        const SECRET: &str = "TOP_SECRET_DATABASE_TOKEN_VALUE";
        let error = parse_args_from(["backup_db", "--database-token", SECRET].map(String::from))
            .expect_err("--database-token must never be accepted on argv");
        let message = error.to_string();
        assert!(message.contains("DATABASE_TOKEN"), "{message}");
        assert!(!message.contains(SECRET), "token value leaked: {message}");
    }

    #[test]
    fn database_token_argument_without_value_is_also_rejected() {
        let error = parse_args_from(["backup_db", "--database-token"].map(String::from))
            .expect_err("--database-token must be rejected even without a value");
        assert!(error.to_string().contains("DATABASE_TOKEN"), "{error:#}");
    }

    #[test]
    fn option_like_value_is_rejected_without_echo() {
        const DUMMY: &str = "SECRET_VALUE_MUST_NOT_APPEAR";
        for flag in ["--database-url", "--output"] {
            let argv = ["backup_db", flag, "--database-token", DUMMY];
            let error = parse_args_from(argv.map(String::from))
                .expect_err("option-like token must be rejected as a value");
            let message = error.to_string();
            let expected = format!("{flag} requires a value");
            assert!(message.contains(expected.as_str()), "{message}");
            assert!(!message.contains(DUMMY), "dummy value leaked: {message}");
            assert!(
                !message.contains("--database-token"),
                "token flag leaked: {message}"
            );
        }
    }

    #[test]
    fn documented_arguments_are_still_parsed() {
        let args = parse_args_from(
            [
                "backup_db",
                "--output",
                "backup.db",
                "--database-url",
                "file:app.db",
                "-h",
            ]
            .map(String::from),
        )
        .expect("documented flags must parse");
        assert_eq!(args.output.as_deref(), Some("backup.db"));
        assert_eq!(args.database_url.as_deref(), Some("file:app.db"));
        assert!(args.help);
    }

    #[test]
    fn help_text_does_not_advertise_database_token() {
        assert!(!HELP.contains("--database-token"), "{HELP}");
        assert!(HELP.contains("DATABASE_TOKEN"), "{HELP}");
    }

    // --- instance lock ---

    #[test]
    fn local_db_path_matches_open_db_selection() {
        assert_eq!(local_db_path("file:app.db"), Some("app.db"));
        assert_eq!(local_db_path("app.db"), Some("app.db"));
        assert_eq!(local_db_path("libsql://example.invalid/app.db"), None);
    }

    #[test]
    fn parse_instance_lock_reads_new_and_legacy_formats() {
        assert_eq!(parse_instance_lock("12345\n67890\n"), (12345, Some(67890)));
        assert_eq!(
            parse_instance_lock(" 12345 \n 67890 \n"),
            (12345, Some(67890))
        );
        assert_eq!(parse_instance_lock("12345\n"), (12345, None));
        assert_eq!(parse_instance_lock("not-a-pid\n"), (0, None));
    }

    #[test]
    fn backup_is_allowed_without_lock_file() {
        let dir = TempDir::new();
        assert!(ensure_no_live_instance(&dir.db("app.db")).is_ok());
    }

    #[test]
    fn backup_is_rejected_when_live_server_holds_lock() {
        let dir = TempDir::new();
        let db = dir.db("app.db");
        let pid = std::process::id();
        let contents = match read_proc_starttime(pid) {
            Some(starttime) => format!("{pid}\n{starttime}\n"),
            // /proc が読めない環境でも fail-closed で拒否される。
            None => format!("{pid}\n"),
        };
        write_lock(&db, &contents);
        let error = ensure_no_live_instance(&db).expect_err("live lock must be rejected");
        assert!(error.to_string().contains("refusing backup"), "{error:#}");
    }

    #[test]
    fn backup_is_rejected_for_legacy_pid_only_lock() {
        let dir = TempDir::new();
        let db = dir.db("app.db");
        write_lock(&db, &format!("{}\n", std::process::id()));
        assert!(ensure_no_live_instance(&db).is_err());
    }

    #[test]
    fn backup_is_allowed_when_lock_owner_is_stale() {
        // /proc が読めない環境では判定不能のため fail-closed で拒否される。
        if read_proc_starttime(std::process::id()).is_none() {
            return;
        }
        let dir = TempDir::new();
        let db = dir.db("app.db");
        let stale = unused_pid();
        write_lock(&db, &format!("{stale}\n"));
        assert!(ensure_no_live_instance(&db).is_ok());
        write_lock(&db, &format!("{stale}\n123456789\n"));
        assert!(ensure_no_live_instance(&db).is_ok());
    }

    #[test]
    fn backup_is_allowed_for_unparsable_lock() {
        if read_proc_starttime(std::process::id()).is_none() {
            return;
        }
        let dir = TempDir::new();
        let db = dir.db("app.db");
        write_lock(&db, "not-a-pid\n");
        assert!(ensure_no_live_instance(&db).is_ok());
    }

    // --- resolve_database_url: 暗黙既定値の撤廃 ---

    #[test]
    fn database_url_is_required_when_no_source_is_set() {
        let error = resolve_database_url(None, EnvValue::Missing)
            .expect_err("no implicit database URL may be used");
        assert_eq!(error.to_string(), DatabaseUrlError::Required.to_string());
    }

    #[test]
    fn database_url_from_cli_takes_priority_over_environment() {
        let url = resolve_database_url(
            Some("libsql://cli.example.invalid/app.db".to_string()),
            EnvValue::Utf8("libsql://env.example.invalid/app.db".to_string()),
        )
        .expect("CLI value must take priority over the environment");
        assert_eq!(url, "libsql://cli.example.invalid/app.db");
    }

    #[test]
    fn database_url_from_environment_is_accepted_without_cli() {
        let url = resolve_database_url(
            None,
            EnvValue::Utf8("libsql://env.example.invalid/app.db".to_string()),
        )
        .expect("environment value must be accepted");
        assert_eq!(url, "libsql://env.example.invalid/app.db");
    }

    #[test]
    fn blank_database_url_is_rejected() {
        for (cli, env) in [
            (Some(String::new()), EnvValue::Utf8("  \t\n ".to_string())),
            (Some("  \t\n ".to_string()), EnvValue::Missing),
            (None, EnvValue::Utf8(String::new())),
        ] {
            let error =
                resolve_database_url(cli, env).expect_err("blank database URL must be rejected");
            assert_eq!(error.to_string(), DatabaseUrlError::Required.to_string());
        }
    }

    #[test]
    fn database_url_error_does_not_echo_values() {
        const CLI_BLANK: &str = "  \t ";
        const ENV_BLANK: &str = "\n  ";
        let from_none = resolve_database_url(None, EnvValue::Missing)
            .expect_err("missing URL must be rejected")
            .to_string();
        let from_blank =
            resolve_database_url(Some(CLI_BLANK.into()), EnvValue::Utf8(ENV_BLANK.into()))
                .expect_err("blank URLs must be rejected")
                .to_string();
        // 入力値の有無にかかわらず同じ固定文言で、値や旧既定値をエコーしない。
        assert_eq!(from_blank, from_none);
        assert_eq!(from_none, DatabaseUrlError::Required.to_string());
        assert!(!from_none.contains("file:local.db"), "{from_none}");
        assert!(!from_none.contains("libsql://"), "{from_none}");
    }
}
