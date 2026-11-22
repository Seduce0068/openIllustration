use anyhow::{Context, anyhow};
use libsql::Builder;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
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
         (WAL/SHM/journal sidecars and a live <db>.instance.lock cause an immediate refusal). By default an existing\n\
         target is never overwritten; --force keeps a timestamped safety copy.\n\
         The safety copy, staging tmp and restored target are created with mode 0600\n\
         (owner-only, plaintext) regardless of umask.\n\
         Exit code: 0 on success, 1 on error."
    );
}

/// 値取りの共通ヘルパ。オプション風トークン（`-` で始まる値）は値として受理せず、
/// 値やトークンをエコーせずに `{flag} requires a value` を返す。
fn take_value<'a>(it: &mut impl Iterator<Item = &'a str>, flag: &str) -> anyhow::Result<&'a str> {
    match it.next() {
        Some(value) if !value.starts_with('-') => Ok(value),
        _ => Err(anyhow!("{flag} requires a value")),
    }
}

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
            "--backup" => {
                args.backup = Some(take_value(&mut it, "--backup")?.to_string());
            }
            "--target" => {
                args.target = Some(take_value(&mut it, "--target")?.to_string());
            }
            "--force" => args.force = true,
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
    Ok(args)
}

fn parse_args() -> anyhow::Result<Args> {
    parse_args_from(std::env::args())
}

/// 必須引数（--backup / --target）の欠落を検証する。
/// argv の解釈（parse_args_from）と分離したことで、必須性の判定を直接テストできる。
/// --help は必須引数なしで許可する（run 側で先に処理される）。
fn validate_args(args: &Args) -> anyhow::Result<()> {
    if args.help {
        return Ok(());
    }
    if args.backup.is_none() {
        return Err(anyhow!("--backup is required"));
    }
    if args.target.is_none() {
        return Err(anyhow!("--target is required"));
    }
    Ok(())
}

fn sidecar_of(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// WAL/SHM/journal が残っていると、検証時に WAL が適用されて通っても
/// stage では `.db` 本体しかコピーされず未適用データが失われる。
/// backup 側・target 側の両方で同じ suffix を拒否する。
fn ensure_no_sidecars(path: &Path, side: &str) -> anyhow::Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = sidecar_of(path, suffix);
        if sidecar.exists() {
            return Err(anyhow!(
                "{side} has a {} sidecar; stop the server before restoring",
                sidecar.display()
            ));
        }
    }
    Ok(())
}

/// ロックファイルの内容から `(PID, プロセス開始時刻)` を取り出す。
/// backup_db / server/src/lib.rs の `parse_instance_lock` と同じ形式。bin から
/// `pub(crate)` ヘルパーは参照できないため、ここで素直に再実装する。
fn parse_instance_lock(contents: &str) -> (u32, Option<u64>) {
    let mut lines = contents.lines();
    let pid = lines
        .next()
        .and_then(|line| line.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let starttime = lines
        .next()
        .and_then(|line| line.trim().parse::<u64>().ok());
    (pid, starttime)
}

/// `/proc/<pid>/stat` の field 22 (starttime) を読む。
/// `/proc` が無い環境・プロセスが消えた場合・形式が読めない場合は `None`。
fn read_proc_starttime(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm (field 2) は空白や ')' を含み得るため、最後の ')' より後だけを解析する。
    let after_comm = stat.rfind(')')?;
    stat.get(after_comm + 1..)?
        .split_whitespace()
        .nth(19)?
        .parse::<u64>()
        .ok()
}

/// ロック所有者が生存中かを判定する。`/proc` が読めず判定不能な場合は
/// 「生存」として扱い、復元を拒否する（fail-closed）。
fn instance_lock_owner_alive(pid: u32, starttime: Option<u64>) -> bool {
    if read_proc_starttime(std::process::id()).is_none() {
        return true;
    }
    match starttime {
        // PID 再利用を防ぐため、開始時刻が一致したときだけ生存とみなす。
        Some(expected) => match read_proc_starttime(pid) {
            Some(actual) => actual == expected,
            // /proc/<pid> はあるのに stat が読めない場合は確認不能なので拒否側へ倒す。
            None => Path::new(&format!("/proc/{pid}")).exists(),
        },
        // 旧形式（PID のみ）は後方互換のため PID の存在だけで判定する。
        None => Path::new(&format!("/proc/{pid}")).exists(),
    }
}

/// `<db>.instance.lock` が生存中のサーバーに保持されていないか検査する。
/// ロックが無い、または明らかに stale（PID 不存在）なら続行を許可する。
/// ロックの読み取りや生存判定が不能な場合は拒否する（fail-closed）。
fn ensure_no_live_instance(db_path: &Path, side: &str) -> anyhow::Result<()> {
    let lock = sidecar_of(db_path, ".instance.lock");
    let lock_display = lock.display();
    match std::fs::symlink_metadata(&lock) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot inspect {side} instance lock: {lock_display}"));
        }
    }
    let contents = std::fs::read_to_string(&lock)
        .with_context(|| format!("cannot read {side} instance lock: {lock_display}"))?;
    let (pid, starttime) = parse_instance_lock(&contents);
    if instance_lock_owner_alive(pid, starttime) {
        anyhow::bail!(
            "refusing restore: a live server instance holds the {side} instance lock \
             ({lock_display}, pid {pid}); stop the server or wait until startup/migration has finished"
        );
    }
    Ok(())
}

/// 復元前の安全検査。既存のサイドカー検査を維持しつつ、生存中の instance lock
/// も拒否する。backup_db と同じ PID + starttime の生存判定を使う。
fn ensure_restore_is_safe(backup: &Path, target: &Path) -> anyhow::Result<()> {
    ensure_no_sidecars(backup, "backup")?;
    ensure_no_sidecars(target, "target")?;
    ensure_no_live_instance(backup, "backup")?;
    ensure_no_live_instance(target, "target")?;
    Ok(())
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

/// 平文 DB のコピーを生成時から 0600 で作る（safety copy / staging tmp 共用）。
///
/// `write(true).create_new(true)` で既存ファイルの上書きを拒否し、`mode(0o600)`
/// で umask に依存せず所有者のみ読み書きできる権限に固定する。生成後に
/// chmod する隙間を作らないため、OpenOptions の時点で権限を指定する。
fn create_safety_copy(src: &Path, dst: &Path, label: &str) -> anyhow::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut source = std::fs::File::open(src)
        .with_context(|| format!("failed to open source for {label}: {}", src.display()))?;
    let mut dest = opts
        .open(dst)
        .with_context(|| format!("failed to create {label}: {}", dst.display()))?;
    std::io::copy(&mut source, &mut dest)
        .with_context(|| format!("failed to write {label}: {}", dst.display()))?;
    Ok(())
}

/// rename 失敗時に safety copy を target へ書き戻す。
/// 復元に成功した場合は元の rename エラーへ「safety copy から復元できた」旨と
/// safety_copy のパスを併記し、失敗した場合は握り潰さずパスと rename エラーを
/// 併せたメッセージを返す（safety copy 作成の fail-closed と対称）。
fn rollback_safety_copy(
    safety_copy: Option<&Path>,
    target: &Path,
    rename_error: std::io::Error,
) -> anyhow::Error {
    let Some(copy) = safety_copy else {
        return anyhow::Error::new(rename_error).context("failed to replace target database");
    };
    match std::fs::copy(copy, target) {
        Ok(_) => {
            // 書き戻し後も 0600 を維持（平文 DB の権限緩和を防ぐ）。
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(chmod_error) =
                    std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o600))
                {
                    return anyhow!(
                        "failed to replace target database ({rename_error}); \
                         safety copy was restored from {} but failed to set 0600: {chmod_error}",
                        copy.display()
                    );
                }
            }
            anyhow::Error::new(rename_error).context(format!(
                "failed to replace target database; \
                 safety copy was restored successfully from {}",
                copy.display()
            ))
        }
        Err(copy_error) => anyhow!(
            "failed to replace target database ({rename_error}); \
             additionally failed to restore safety copy from {}: {copy_error}",
            copy.display()
        ),
    }
}

/// ステージ済みファイルを target へ原子的に置き換える。
/// rename が失敗した場合は一時ファイルを片付け、safety copy の復元を試み、
/// その成否を呼び出し側へ返す。一時ファイルの削除に失敗した場合は握り潰さず、
/// tmp のパスと削除エラーを rename エラー・safety copy の成否へ併記して返す。
fn replace_target(tmp: &Path, target: &Path, safety_copy: Option<&Path>) -> anyhow::Result<()> {
    match std::fs::rename(tmp, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let cleanup = std::fs::remove_file(tmp);
            // rollback_safety_copy が rename エラーと safety copy の成否を
            // 併記した Error を返す。削除失敗はそこへ context として積み、
            // 既存の通知を壊さずに一時ファイルの残置を利用者へ見せる。
            let rollback_error = rollback_safety_copy(safety_copy, target, error);
            let combined_error = match cleanup {
                Ok(()) => rollback_error,
                Err(cleanup_error) => rollback_error.context(format!(
                    "failed to clean up staging file {}: {cleanup_error}",
                    tmp.display()
                )),
            };
            Err(combined_error)
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    if args.help {
        print_help();
        return Ok(());
    }
    validate_args(&args)?;
    let backup = PathBuf::from(args.backup.expect("validated by validate_args"));
    let target = PathBuf::from(args.target.expect("validated by validate_args"));

    if !backup.exists() {
        return Err(anyhow!("backup file does not exist: {}", backup.display()));
    }
    // validate_backup の前に検査する。libsql が WAL を checkpoint して
    // サイドカーを消す前に、元の状態を確認するため。生存中の instance lock
    // もここで拒否する（backup_db と同じ PID + starttime 判定）。
    ensure_restore_is_safe(&backup, &target)?;
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
        // 1 秒以内の再 --force で安全コピーを上書きしないようミリ秒を付与する。
        // 既存の `%Y%m%dT%H%M%SZ` 形式は維持し、時刻の末尾に精度が増えるだけ。
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
        let mut name = target.as_os_str().to_os_string();
        name.push(format!(".pre-restore-{stamp}"));
        let copy = PathBuf::from(name);
        create_safety_copy(&target, &copy, "safety copy")
            .context("failed to create safety copy")?;
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
    create_safety_copy(&backup, &tmp, "staging file").context("failed to stage backup")?;
    replace_target(&tmp, &target, safety_copy.as_deref())?;

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
                "restore-db-lock-test-{}-{nanos}-{seq}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("create temp dir");
            Self(path)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_lock(db_path: &Path, contents: &str) {
        std::fs::write(sidecar_of(db_path, ".instance.lock"), contents).expect("write lock");
    }

    /// 実在しない PID を返す（stale ロックのテスト用）。
    fn unused_pid() -> u32 {
        (1..u32::MAX)
            .rev()
            .find(|pid| !Path::new(&format!("/proc/{pid}")).exists())
            .expect("find an unused pid")
    }

    /// 自プロセスの PID + starttime をロック内容として作る。
    fn live_lock_contents() -> String {
        let pid = std::process::id();
        match read_proc_starttime(pid) {
            Some(starttime) => format!("{pid}\n{starttime}\n"),
            // /proc が読めない環境でも fail-closed で拒否される。
            None => format!("{pid}\n"),
        }
    }

    // --- parse_instance_lock: backup_db と同じ形式 ---

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

    // --- ensure_restore_is_safe: 生存 instance lock の拒否 ---

    #[test]
    fn no_lock_and_no_sidecar_allows_restore() {
        let dir = TempDir::new();
        let backup = dir.path("backup.db");
        let target = dir.path("target.db");
        assert!(ensure_restore_is_safe(&backup, &target).is_ok());
    }

    #[test]
    fn live_target_lock_is_rejected() {
        let dir = TempDir::new();
        let backup = dir.path("backup.db");
        let target = dir.path("target.db");
        write_lock(&target, &live_lock_contents());
        let error = ensure_restore_is_safe(&backup, &target)
            .expect_err("a live target lock must be rejected");
        let message = error.to_string();
        assert!(message.contains("refusing restore"), "{message}");
        assert!(message.contains("target"), "{message}");
    }

    #[test]
    fn live_backup_lock_is_rejected() {
        let dir = TempDir::new();
        let backup = dir.path("backup.db");
        let target = dir.path("target.db");
        write_lock(&backup, &live_lock_contents());
        let error = ensure_restore_is_safe(&backup, &target)
            .expect_err("a live backup lock must be rejected");
        let message = error.to_string();
        assert!(message.contains("refusing restore"), "{message}");
        assert!(message.contains("backup"), "{message}");
    }

    #[test]
    fn legacy_pid_only_lock_is_rejected() {
        let dir = TempDir::new();
        let backup = dir.path("backup.db");
        let target = dir.path("target.db");
        write_lock(&target, &format!("{}\n", std::process::id()));
        assert!(ensure_restore_is_safe(&backup, &target).is_err());
    }

    #[test]
    fn stale_lock_is_allowed() {
        // /proc が読めない環境では判定不能のため fail-closed で拒否される。
        if read_proc_starttime(std::process::id()).is_none() {
            return;
        }
        let dir = TempDir::new();
        let backup = dir.path("backup.db");
        let target = dir.path("target.db");
        let stale = unused_pid();
        write_lock(&target, &format!("{stale}\n"));
        assert!(ensure_restore_is_safe(&backup, &target).is_ok());
        write_lock(&target, &format!("{stale}\n123456789\n"));
        assert!(ensure_restore_is_safe(&backup, &target).is_ok());
    }

    #[test]
    fn sidecar_check_is_still_enforced_by_preflight() {
        let dir = TempDir::new();
        let backup = dir.path("backup.db");
        let target = dir.path("target.db");
        let sidecar = sidecar_of(&target, "-wal");
        std::fs::write(&sidecar, b"x").expect("create sidecar");
        let error =
            ensure_restore_is_safe(&backup, &target).expect_err("sidecar must still be rejected");
        let message = error.to_string();
        assert!(message.contains("sidecar"), "{message}");
        assert!(message.contains("target"), "{message}");
    }

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
        assert_eq!(
            sidecar_of(path, "-journal"),
            PathBuf::from("/tmp/example.db-journal")
        );
    }

    #[test]
    fn sidecars_are_rejected_on_both_sides_with_side_named() {
        let dir = std::env::temp_dir().join(format!("restore-db-sidecars-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let backup = dir.join("backup.db");
        let target = dir.join("target.db");

        assert!(ensure_no_sidecars(&backup, "backup").is_ok());
        assert!(ensure_no_sidecars(&target, "target").is_ok());

        for suffix in ["-wal", "-shm", "-journal"] {
            for (path, side) in [(&backup, "backup"), (&target, "target")] {
                let sidecar = sidecar_of(path, suffix);
                std::fs::write(&sidecar, b"x").unwrap();
                let error = ensure_no_sidecars(path, side).unwrap_err().to_string();
                assert!(error.contains(side), "missing side in: {error}");
                assert!(error.contains(suffix), "missing suffix in: {error}");
                std::fs::remove_file(&sidecar).unwrap();
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
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
    fn database_token_argument_is_rejected_without_leaking_value() {
        const SECRET: &str = "SECRET_VALUE_MUST_NOT_APPEAR";
        let error = parse_args_from(["restore_db", "--database-token", SECRET].map(String::from))
            .expect_err("--database-token must never be accepted on argv");
        let message = error.to_string();
        assert!(message.contains("DATABASE_TOKEN"), "{message}");
        assert!(!message.contains(SECRET), "token value leaked: {message}");
    }

    #[test]
    fn option_like_value_is_rejected_without_echo() {
        const DUMMY: &str = "SECRET_VALUE_MUST_NOT_APPEAR";
        for flag in ["--backup", "--target"] {
            let argv = ["restore_db", flag, "--database-token", DUMMY];
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
    fn parse_args_requires_both_paths() {
        // parse_args_from は argv の解釈だけを行い、必須性は validate_args が判定する。
        // --backup のみ → --target が必須。
        let backup_only = parse_args_from(["restore_db", "--backup", "b"].map(String::from))
            .expect("backup-only args must parse");
        let error = validate_args(&backup_only).expect_err("--target must be required");
        assert!(
            error.to_string().contains("--target is required"),
            "{error:#}"
        );

        // --target のみ → --backup が必須。
        let target_only = parse_args_from(["restore_db", "--target", "t"].map(String::from))
            .expect("target-only args must parse");
        let error = validate_args(&target_only).expect_err("--backup must be required");
        assert!(
            error.to_string().contains("--backup is required"),
            "{error:#}"
        );

        // 両方欠落 → 先に検査される --backup が必須。
        let neither = parse_args_from(["restore_db"].map(String::from))
            .expect("no-path args must still parse");
        let error = validate_args(&neither).expect_err("--backup must be required");
        assert!(
            error.to_string().contains("--backup is required"),
            "{error:#}"
        );

        // 両方揃い → 受理。
        let both =
            parse_args_from(["restore_db", "--backup", "b", "--target", "t"].map(String::from))
                .expect("complete args must parse");
        assert!(validate_args(&both).is_ok());
    }

    // --- rename 失敗時の safety copy 復元エラー伝播 ---

    fn injected_rename_error() -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected rename failure",
        )
    }

    #[test]
    fn safety_copy_restore_failure_reports_path_and_rename_error() {
        // target をディレクトリ化して safety copy の書き戻しを失敗させる。
        let dir = TempDir::new();
        let target = dir.path("target.db");
        std::fs::create_dir(&target).expect("make target a directory to break copy");
        let copy = dir.path("target.db.pre-restore-test");
        std::fs::write(&copy, b"safety").expect("write safety copy");

        let error = rollback_safety_copy(Some(&copy), &target, injected_rename_error());
        let message = format!("{error:#}");
        assert!(
            message.contains("failed to restore safety copy"),
            "{message}"
        );
        assert!(message.contains(&copy.display().to_string()), "{message}");
        // 元の rename エラーも握り潰さずに伝わること。
        assert!(message.contains("injected rename failure"), "{message}");
        assert!(
            message.contains("failed to replace target database"),
            "{message}"
        );
    }

    #[test]
    fn safety_copy_restore_success_reports_restoration_confirmation() {
        let dir = TempDir::new();
        let target = dir.path("target.db");
        std::fs::write(&target, b"old").expect("write target");
        let copy = dir.path("target.db.pre-restore-test");
        std::fs::write(&copy, b"safety").expect("write safety copy");

        let error = rollback_safety_copy(Some(&copy), &target, injected_rename_error());
        let message = format!("{error:#}");
        // 元の rename エラーは保持したまま、復元成功とその出所パスも併記される。
        assert!(
            message.contains("failed to replace target database"),
            "{message}"
        );
        assert!(message.contains("injected rename failure"), "{message}");
        assert!(
            message.contains("safety copy was restored successfully"),
            "{message}"
        );
        assert!(message.contains(&copy.display().to_string()), "{message}");
        assert!(
            !message.contains("failed to restore safety copy"),
            "{message}"
        );
        // safety copy の内容が target へ戻っていること（復元の成否が観測できる）。
        assert_eq!(std::fs::read(&target).unwrap(), b"safety".to_vec());
    }

    #[test]
    fn replace_target_with_directory_target_reports_safety_copy_failure() {
        // 実 rename を失敗させ（target をディレクトリ化）、復元失敗が
        // replace_target の Result まで伝播することを検証する。
        let dir = TempDir::new();
        let tmp = dir.path("stage.tmp");
        std::fs::write(&tmp, b"staged").expect("write staging file");
        let target = dir.path("target.db");
        std::fs::create_dir(&target).expect("make target a directory to break rename");
        let copy = dir.path("copy");
        std::fs::write(&copy, b"safety").expect("write safety copy");

        let error = replace_target(&tmp, &target, Some(&copy)).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("failed to restore safety copy"),
            "{message}"
        );
        assert!(
            !tmp.exists(),
            "staging file must be cleaned up on rename failure"
        );
    }

    #[test]
    fn rename_failure_without_safety_copy_keeps_original_error() {
        // safety_copy=None のときも rename エラー自身(io::Error)が chain に残り、
        // staging ファイルは片付けられること。
        let dir = TempDir::new();
        let tmp = dir.path("stage.tmp");
        std::fs::write(&tmp, b"staged").expect("write staging file");
        let target = dir.path("target.db");
        std::fs::create_dir(&target).expect("make target a directory to break rename");

        let error = replace_target(&tmp, &target, None).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("failed to replace target database"),
            "{message}"
        );
        assert!(
            error.chain().any(|cause| cause.is::<std::io::Error>()),
            "original rename error must be preserved: {message}"
        );
        // 削除は成功しているので cleanup 失敗の context は付かない。
        assert!(
            !message.contains("failed to clean up staging file"),
            "{message}"
        );
        assert!(
            !tmp.exists(),
            "staging file must be cleaned up on rename failure"
        );
    }

    #[test]
    fn replace_target_success_returns_ok_and_removes_staging_path() {
        let dir = TempDir::new();
        let tmp = dir.path("stage.tmp");
        std::fs::write(&tmp, b"staged").expect("write staging file");
        let target = dir.path("target.db");

        assert!(replace_target(&tmp, &target, None).is_ok());
        // rename により staging パスは消え、中身は target へ移る。
        assert!(!tmp.exists(), "staging path must be gone after rename");
        assert_eq!(std::fs::read(&target).unwrap(), b"staged".to_vec());
    }

    #[test]
    fn replace_target_reports_staging_cleanup_failure_alongside_rollback() {
        // staging パスをディレクトリ化して remove_file を失敗させる(unlink は
        // ディレクトリに対して EISDIR)。rename も target を通常ファイルにして
        // 失敗させ、削除失敗が rename エラー・safety copy 成否と併記されることを検証する。
        let dir = TempDir::new();
        let tmp = dir.path("stage.tmp");
        std::fs::create_dir(&tmp).expect("make staging path a directory");
        std::fs::write(tmp.join("child"), b"x").expect("write child file");
        let target = dir.path("target.db");
        std::fs::write(&target, b"old").expect("write target file to force rename failure");
        let copy = dir.path("copy");
        std::fs::write(&copy, b"safety").expect("write safety copy");

        let error = replace_target(&tmp, &target, Some(&copy)).unwrap_err();
        let message = format!("{error:#}");
        // 削除失敗の旨・tmp パス・削除エラーが現れること。
        assert!(
            message.contains("failed to clean up staging file"),
            "{message}"
        );
        assert!(message.contains(&tmp.display().to_string()), "{message}");
        // 既存の rollback_safety_copy 通知が壊れていないこと。
        assert!(
            message.contains("failed to replace target database"),
            "{message}"
        );
        assert!(
            message.contains("safety copy was restored successfully"),
            "{message}"
        );
    }
}
