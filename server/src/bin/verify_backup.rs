use anyhow::{Context, anyhow};
use libsql::Builder;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
struct Args {
    input: Option<String>,
    help: bool,
}

const HELP: &str = "Usage: verify_backup --input PATH\n\
         \n\
         Verifies that a backup produced by backup_db is structurally usable:\n\
         the file exists, opens as SQLite, passes PRAGMA integrity_check,\n\
         has tables, and has a migration ledger.\n\
         This is a structural check, not a full application-level restore test.\n\
         This tool does not connect to a database; no token is used.\n\
         Exit code: 0 if valid, 1 on error.";

fn print_help() {
    eprintln!("{HELP}");
}

/// 値取りの共通ヘルパ。オプション風トークン（`-` で始まる値）は値として受理せず、
/// 値やトークンをエコーせずに `{flag} requires a value` を返す。
fn take_value<'a>(it: &mut impl Iterator<Item = &'a str>, flag: &str) -> anyhow::Result<&'a str> {
    match it.next() {
        Some(value) if !value.starts_with('-') => Ok(value),
        _ => Err(anyhow!("{flag} requires a value")),
    }
}

/// argv の解釈だけを行う純粋関数。環境変数・プロセス引数を読まないため、
/// 一時 DB のパスや不正な引数を直接渡してテストできる。
fn parse_args_from<I>(items: I) -> anyhow::Result<Args>
where
    I: IntoIterator<Item = String>,
{
    // 所有権付き引数を保持してから借用し、値取りヘルパへ &str を渡す。
    let owned: Vec<String> = items.into_iter().collect();
    parse_str_args(&owned)
}

fn parse_str_args(items: &[String]) -> anyhow::Result<Args> {
    let mut args = Args::default();
    let mut it = items.iter().skip(1).map(String::as_str);
    while let Some(arg) = it.next() {
        match arg {
            "-h" | "--help" => args.help = true,
            "--input" => {
                args.input = Some(take_value(&mut it, "--input")?.to_string());
            }
            // argv は ps 等で公開される。トークン値は受け付けず、
            // エラーにも値をエコーしない（秘密値を出力しない）。
            "--database-token" => {
                return Err(anyhow!(
                    "--database-token is not accepted; set DATABASE_TOKEN in the environment instead"
                ));
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    if !args.help && args.input.is_none() {
        return Err(anyhow!("--input is required"));
    }
    Ok(args)
}

fn parse_args() -> anyhow::Result<Args> {
    parse_args_from(std::env::args())
}

/// 検証が受理したバックアップの構造情報。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BackupStats {
    tables: i64,
    migrations: i64,
}

fn sidecar_of(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// restore_db と同じ fail-closed 判定。WAL/SHM/journal が残っていると、
/// 検証時に WAL が適用されて通ってもファイル本体だけを運んだ場合に未適用
/// データが失われる。libsql が WAL を checkpoint する前に検査する必要がある。
fn ensure_no_sidecars(path: &Path) -> anyhow::Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = sidecar_of(path, suffix);
        if sidecar.exists() {
            return Err(anyhow!(
                "backup has a {} sidecar; stop the server before verifying",
                sidecar.display()
            ));
        }
    }
    Ok(())
}

/// ファイル存在確認から SQL 判定までをまとめた検証境界。入力を引数で受け取る
/// ため、実 libsql の一時 DB を渡してテストできる。既存 CLI の判定順と
/// エラー文（stdout の成功行含む）は維持する。
async fn verify_backup_at(path: &Path) -> anyhow::Result<BackupStats> {
    if !path.exists() {
        return Err(anyhow!("backup file does not exist: {}", path.display()));
    }
    // DB を開く前に検査する（restore_db と同じ順番）。
    ensure_no_sidecars(path)?;

    let db = Builder::new_local(path)
        .build()
        .await
        .context("failed to open backup")?
        .connect()?;

    // 構造的な破損を検出する（restore_db と同じ判定）。
    let mut rows = db
        .query("PRAGMA integrity_check", ())
        .await
        .context("integrity_check failed")?;
    let integrity: String = rows
        .next()
        .await
        .context("no integrity_check result")?
        .context("no integrity_check row")?
        .get(0)?;
    if integrity != "ok" {
        return Err(anyhow!("backup integrity_check failed: {integrity}"));
    }

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

    Ok(BackupStats { tables, migrations })
}

/// 検証結果から (stdout に出す行, 終了コード) を決める純粋関数。
/// エラー内容は stdout に出さない（秘密値が混じる可能性があるため）。
/// stderr の "Error: ..." 表示と終了コード 1 は既存どおり anyhow に委譲する。
fn decide_output(result: &anyhow::Result<BackupStats>) -> (Option<String>, i32) {
    match result {
        Ok(stats) => (
            Some(format!(
                "backup_verification: OK tables={} migrations={}",
                stats.tables, stats.migrations
            )),
            0,
        ),
        Err(_) => (None, 1),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    if args.help {
        print_help();
        return Ok(());
    }
    let input = args.input.ok_or_else(|| anyhow!("--input is required"))?;

    let result = verify_backup_at(Path::new(&input)).await;
    let (line, exit_code) = decide_output(&result);
    if let Some(line) = line {
        println!("{line}");
    }
    if exit_code == 0 {
        Ok(())
    } else {
        // Err を返すことで従来どおり anyhow が stderr へ理由を出し、終了コード 1 になる。
        result.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BackupStats, HELP, decide_output, ensure_no_sidecars, parse_args_from, sidecar_of,
        verify_backup_at,
    };
    use libsql::Builder;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    /// mktemp 相当の一意な一時ディレクトリ。Drop で確実に削除する。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "verify-backup-test-{}-{nanos}-{seq}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("create temp dir");
            Self(path)
        }

        fn db(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 実 libsql でバックアップ相当の単一ファイル DB を作る。
    /// backup_db の VACUUM INTO 出力と同じくサイドカーは残さない。
    async fn create_backup(path: &Path, user_table: bool, ledger: bool, migrations: usize) {
        let db = Builder::new_local(path)
            .build()
            .await
            .expect("build temp db")
            .connect()
            .expect("connect temp db");
        // テスト DB は常に DELETE モードとし、WAL サイドカーの有無を明示的に制御する。
        // journal_mode は行を返すため query で受け取る（execute は ExecuteReturnedRows）。
        let mut pragma = db
            .query("PRAGMA journal_mode=DELETE", ())
            .await
            .expect("set journal mode");
        while pragma
            .next()
            .await
            .expect("read journal mode result")
            .is_some()
        {}
        if user_table {
            db.execute(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
                (),
            )
            .await
            .expect("create users table");
            db.execute("INSERT INTO users (name) VALUES ('tester')", ())
                .await
                .expect("insert user");
        }
        if ledger {
            db.execute(
                "CREATE TABLE _migrations (version INTEGER PRIMARY KEY, checksum TEXT NOT NULL)",
                (),
            )
            .await
            .expect("create migration ledger");
            for version in 0..migrations {
                db.execute(
                    &format!(
                        "INSERT INTO _migrations (version, checksum) VALUES ({version}, 'abc')"
                    ),
                    (),
                )
                .await
                .expect("insert migration");
            }
        }
        drop(db);
        for suffix in ["-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(sidecar_of(path, suffix));
        }
    }

    fn error_text(error: &anyhow::Error) -> String {
        format!("{error:#}")
    }

    // --- parse_args_from: 引数パース ---

    #[test]
    fn help_text_does_not_advertise_database_token() {
        assert!(!HELP.contains("--database-token"), "{HELP}");
        assert!(HELP.contains("does not connect to a database"), "{HELP}");
    }

    #[test]
    fn parse_args_accepts_backup_path() {
        let args = parse_args_from(["verify_backup", "--input", "backup.db"].map(String::from))
            .expect("backup path must parse");
        assert_eq!(args.input.as_deref(), Some("backup.db"));
        assert!(!args.help);
    }

    #[test]
    fn parse_args_accepts_help_without_input() {
        for flag in ["-h", "--help"] {
            let args = parse_args_from(["verify_backup", flag].map(String::from))
                .expect("help must not require --input");
            assert!(args.help);
            assert_eq!(args.input, None);
        }
    }

    #[test]
    fn parse_args_rejects_missing_input_value() {
        let error = parse_args_from(["verify_backup", "--input"].map(String::from))
            .expect_err("--input requires a value");
        assert!(
            error_text(&error).contains("--input requires a value"),
            "{error:#}"
        );
    }

    #[test]
    fn parse_args_rejects_missing_required_input() {
        let error =
            parse_args_from(["verify_backup"].map(String::from)).expect_err("--input is required");
        assert!(
            error_text(&error).contains("--input is required"),
            "{error:#}"
        );
    }

    #[test]
    fn parse_args_rejects_unknown_argument() {
        let error = parse_args_from(["verify_backup", "--bogus"].map(String::from))
            .expect_err("unknown argument must be rejected");
        assert!(error_text(&error).contains("unknown argument"), "{error:#}");
    }

    #[test]
    fn option_like_value_is_rejected_without_echo() {
        const DUMMY: &str = "SECRET_VALUE_MUST_NOT_APPEAR";
        let argv = ["verify_backup", "--input", "--database-token", DUMMY];
        let error = parse_args_from(argv.map(String::from))
            .expect_err("option-like token must be rejected as a value");
        let message = error_text(&error);
        assert!(message.contains("--input requires a value"), "{message}");
        assert!(!message.contains(DUMMY), "dummy value leaked: {message}");
        assert!(
            !message.contains("--database-token"),
            "token flag leaked: {message}"
        );
    }

    #[test]
    fn database_token_argument_is_rejected_without_leaking_value() {
        const SECRET: &str = "TOP_SECRET_DATABASE_TOKEN_VALUE";
        let error =
            parse_args_from(["verify_backup", "--database-token", SECRET].map(String::from))
                .expect_err("--database-token must never be accepted on argv");
        let message = error_text(&error);
        assert!(message.contains("DATABASE_TOKEN"), "{message}");
        assert!(!message.contains(SECRET), "token value leaked: {message}");
    }

    // --- decide_output: 出力・終了コード判定 ---

    #[test]
    fn success_output_keeps_existing_format() {
        let result = Ok(BackupStats {
            tables: 13,
            migrations: 10,
        });
        assert_eq!(
            decide_output(&result),
            (
                Some("backup_verification: OK tables=13 migrations=10".to_string()),
                0
            )
        );
    }

    #[test]
    fn failure_has_no_stdout_and_exit_code_one() {
        let result = Err(anyhow::anyhow!("backup has no tables"));
        assert_eq!(decide_output(&result), (None, 1));
    }

    #[test]
    fn failure_never_writes_error_text_to_stdout() {
        const SECRET: &str = "TOP_SECRET_DATABASE_TOKEN_VALUE";
        let result = Err(anyhow::anyhow!("token={SECRET}"));
        let (line, code) = decide_output(&result);
        assert_eq!(line, None, "error details must stay on stderr");
        assert_eq!(code, 1);
    }

    // --- sidecar: restore_db と同等の fail-closed ---

    #[test]
    fn sidecar_suffixes_are_appended_to_full_path() {
        let path = Path::new("/tmp/example.db");
        for suffix in ["-wal", "-shm", "-journal"] {
            assert_eq!(
                sidecar_of(path, suffix),
                PathBuf::from(format!("/tmp/example.db{suffix}"))
            );
        }
    }

    #[tokio::test]
    async fn sidecars_are_rejected_before_opening_the_backup() {
        let dir = TempDir::new();
        let backup = dir.db("backup.db");
        create_backup(&backup, true, true, 2).await;
        assert!(ensure_no_sidecars(&backup).is_ok());

        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = sidecar_of(&backup, suffix);
            std::fs::write(&sidecar, b"x").expect("create sidecar");
            let error = verify_backup_at(&backup)
                .await
                .expect_err("sidecar must be rejected");
            let message = error_text(&error);
            assert!(message.contains("sidecar"), "{message}");
            assert!(message.contains(suffix), "{message}");
            std::fs::remove_file(&sidecar).expect("remove sidecar");
        }
        assert!(verify_backup_at(&backup).await.is_ok());
    }

    // --- verify_backup_at: 実 libsql による構造検証 ---

    #[tokio::test]
    async fn valid_backup_is_accepted() {
        let dir = TempDir::new();
        let backup = dir.db("valid.db");
        create_backup(&backup, true, true, 3).await;

        let stats = verify_backup_at(&backup)
            .await
            .expect("valid backup must be accepted");
        assert_eq!(stats.tables, 2); // users + _migrations
        assert_eq!(stats.migrations, 3);
        let (line, code) = decide_output(&Ok(stats));
        assert_eq!(
            line.as_deref(),
            Some("backup_verification: OK tables=2 migrations=3")
        );
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn empty_database_is_rejected() {
        let dir = TempDir::new();
        let backup = dir.db("empty.db");
        create_backup(&backup, false, false, 0).await;

        let error = verify_backup_at(&backup)
            .await
            .expect_err("empty database must be rejected");
        assert!(
            error_text(&error).contains("backup has no tables"),
            "{error:#}"
        );
        assert_eq!(decide_output(&Err(error)).1, 1);
    }

    #[tokio::test]
    async fn missing_migration_ledger_is_rejected() {
        let dir = TempDir::new();
        let backup = dir.db("no-ledger.db");
        create_backup(&backup, true, false, 0).await;

        let error = verify_backup_at(&backup)
            .await
            .expect_err("missing ledger must be rejected");
        assert!(
            error_text(&error).contains("migration ledger missing"),
            "{error:#}"
        );
        assert_eq!(decide_output(&Err(error)).1, 1);
    }

    #[tokio::test]
    async fn empty_migration_ledger_is_rejected() {
        let dir = TempDir::new();
        let backup = dir.db("empty-ledger.db");
        create_backup(&backup, true, true, 0).await;

        let error = verify_backup_at(&backup)
            .await
            .expect_err("empty ledger must be rejected");
        assert!(
            error_text(&error).contains("backup has no applied migrations"),
            "{error:#}"
        );
        assert_eq!(decide_output(&Err(error)).1, 1);
    }

    #[tokio::test]
    async fn nonexistent_file_is_rejected() {
        let dir = TempDir::new();
        let backup = dir.db("missing.db");

        let error = verify_backup_at(&backup)
            .await
            .expect_err("missing file must be rejected");
        assert!(
            error_text(&error).contains("backup file does not exist"),
            "{error:#}"
        );
        assert_eq!(decide_output(&Err(error)).1, 1);
    }

    #[tokio::test]
    async fn non_sqlite_file_is_rejected() {
        let dir = TempDir::new();
        let backup = dir.db("garbage.db");
        std::fs::write(&backup, b"not a sqlite database").expect("write garbage");

        assert!(
            verify_backup_at(&backup).await.is_err(),
            "non-SQLite file must be rejected"
        );
    }
}
