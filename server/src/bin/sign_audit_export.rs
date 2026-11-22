use anyhow::{Context, anyhow};
use common::{
    AuditSnapshotEntry, GovernanceAuditEntry, SignedAuditSnapshot, compute_audit_chain_digest,
    env::{EnvValue, bool_env_state, read_env},
    mask_reason, sign_audit_snapshot,
};
use libsql::Builder;
use server::cli_args;
use server::resolve_database_url;
use std::{fs::OpenOptions, io::Write};

#[derive(Debug, Default)]
struct Args {
    database_url: Option<String>,
    signing_key: Option<String>,
    output: Option<String>,
    include_reasons: bool,
    allow_empty: bool,
    help: bool,
}

fn print_help() {
    eprintln!(
        "Usage: sign_audit_export --signing-key PEM_PATH --output PATH [--database-url URL] [--include-reasons] [--allow-empty]\n\
         \n\
         Creates a signed snapshot of content_audit_log and governance_audit_log. The signing key is a\n\
         PKCS#8 RSA private key PEM file. Never pass the key itself on the CLI.\n\
         The PEM must be owner-only (0600/0400) and its directory must not be group/other writable\n\
         (0700 recommended). The remote libSQL token is read only from the DATABASE_TOKEN\n\
         environment variable; --database-token is intentionally not accepted (argv is public).\n\
         Reasons/details are redacted by default; --include-reasons exposes them (opt-in).\n\
         An empty result in both audit logs is rejected unless --allow-empty is passed explicitly.\n\
         DATABASE_URL is required: pass --database-url or set DATABASE_URL; no default is used.\n\
         DATABASE_TOKEN is read only from the environment (or left empty).\n\
         Exit code: 0 on success, 1 on error."
    );
}

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
        "--signing-key" => {
            args.signing_key = Some(cli_args::take_value(it, "--signing-key")?.to_string());
            Ok(true)
        }
        "--output" => {
            args.output = Some(cli_args::take_value(it, "--output")?.to_string());
            Ok(true)
        }
        "--include-reasons" => {
            args.include_reasons = true;
            Ok(true)
        }
        "--allow-empty" => {
            args.allow_empty = true;
            Ok(true)
        }
        _ => Ok(false),
    })
}

fn parse_args() -> anyhow::Result<Args> {
    parse_args_from(std::env::args())
}

/// DATABASE_TOKEN が非 UTF-8 の場合の固定文言。トークン値を一切エコーしない。
const DATABASE_TOKEN_INVALID: &str = "DATABASE_TOKEN must be a valid UTF-8 string";

/// `read_env` の結果から DB トークンを取り出す純粋版。
/// 未設定は空文字、非 UTF-8 は不正として固定文言で拒否し、生バイトを保持しない。
fn database_token_from_env_value(value: EnvValue) -> anyhow::Result<String> {
    match value {
        EnvValue::Utf8(token) => Ok(token),
        EnvValue::Missing => Ok(String::new()),
        EnvValue::NonUtf8 => Err(anyhow!(DATABASE_TOKEN_INVALID)),
    }
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
#[cfg(test)]
fn create_snapshot_output(path: &str) -> anyhow::Result<std::fs::File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .context("failed to create snapshot output (it must not already exist)")
}

/// `restore_db` の staging + safety copy と同じ方式で、スナップショットを
/// 0600 の一時ファイルへ書き、fsync のうえ最終パスへ公開する。
/// 公開は `hard_link` で create_new 相当の原子的処理を行い、既存ファイルを
/// 上書きしない（存在チェックと公開の間の競合ウィンドウをなくす）。
/// hard_link が使えない環境（クロスデバイス等）では rename へフォールバックし、
/// その際も最終パスの存在を再確認して競合を検出する。
/// 公開失敗時は一時ファイルを残し、値や CID をエコーせず固定文言でエラーを返す。
fn publish_snapshot_atomic(path: &str, json: &[u8]) -> anyhow::Result<()> {
    publish_snapshot_atomic_with(path, json, |p: &std::path::Path| std::fs::remove_file(p))
}

/// `remove_staging` を差し替え可能な本体。テストから削除失敗を決定的に注入するために分離する。
fn publish_snapshot_atomic_with<F>(path: &str, json: &[u8], remove_staging: F) -> anyhow::Result<()>
where
    F: FnOnce(&std::path::Path) -> std::io::Result<()>,
{
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let target = std::path::Path::new(path);
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let name = target
        .file_name()
        .ok_or_else(|| anyhow!("snapshot output path has no file name: {path}"))?
        .to_string_lossy();
    // PID だけではクラッシュ残骸と衝突し得るため unix_millis も付ける
    // （restore_db のタイムスタンプ付き一時ファイルと同じ考え方）。
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let tmp = parent.join(format!(".{name}.tmp-{}-{millis}", std::process::id()));

    // ステージングの作成・書き込み・fsync。途中で失敗しても一時ファイルは残す
    // （公開失敗と同様に、値や CID をエコーせず固定文言のエラーを返す）。
    {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let mut file = opts.open(&tmp).with_context(|| {
            format!("failed to create staging snapshot file: {}", tmp.display())
        })?;
        file.write_all(json)
            .with_context(|| format!("failed to write staging snapshot: {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to fsync staging snapshot: {}", tmp.display()))?;
    }

    // hard_link は既存の最終パスに対して EEXIST で失敗するため、
    // 存在チェックと公開の間の TOCTOU ウィンドウが無い（create_new 相当）。
    match std::fs::hard_link(&tmp, target) {
        Ok(()) => {
            // hard_link による公開はここで完了済み。staging 削除に失敗しても公開の
            // 成否は変えず、警告ログへ留めて Ok(()) を返す。誤った「失敗」通知と
            // 再実行拒否（既存出力を競合として拒否する挙動）を防ぐためである。
            // staging パスはログへ出してよいが、スナップショットの値や CID は出さない。
            if let Err(error) = remove_staging(&tmp) {
                eprintln!(
                    "warning: failed to remove staging snapshot after publish ({}): {error}",
                    tmp.display()
                );
            }
            Ok(())
        }
        Err(_) => {
            // hard_link が失敗したら最終パスの存在を再確認して競合を検出する。
            // 既存ファイルがあれば上書きせず拒否する（値や CID はエコーしない）。
            // 無ければクロスデバイス等で hard_link が使えない環境とみなし、
            // rename 直前の存在確認を兼ねて旧 rename へフォールバックする。
            if std::fs::symlink_metadata(target).is_ok() {
                return Err(anyhow!("snapshot output must not already exist: {path}"));
            }
            std::fs::rename(&tmp, target)
                .with_context(|| format!("failed to publish snapshot via rename fallback: {path}"))
        }
    }
}

/// 本番相当（`ALLOW_MOCK_AUTH` 無効）では平文理由のエクスポートを既定で拒否する。
/// 正当な調査用途向けの break-glass として `AUDIT_EXPORT_INCLUDE_REASONS=1` の
/// 明示オプトインを追加で要求する（多層防御）。開発（mock 有効）では従来どおり許可。
/// フラグ値・環境変数の値はエコーせず、固定文言のみを返す。
fn ensure_include_reasons_allowed(
    include_reasons: bool,
    allow_mock_auth: bool,
    explicit_opt_in: bool,
) -> anyhow::Result<()> {
    if !include_reasons || allow_mock_auth || explicit_opt_in {
        return Ok(());
    }
    Err(anyhow!(
        "--include-reasons is not allowed in production without explicit opt-in; keep reasons masked or set AUDIT_EXPORT_INCLUDE_REASONS=1 deliberately"
    ))
}

/// content_audit_log / governance_audit_log が両方とも0件の場合は、
/// 明示オプトイン（`--allow-empty`）が無い限り拒否する。
/// 空ログから `AUDIT_CHAIN_EMPTY` ダミールートが作られるのを呼び出し側で防ぐ
/// （`compute_audit_chain_digest` の空扱いは変更しない）。
fn ensure_audit_not_empty(
    content_count: usize,
    governance_count: usize,
    allow_empty: bool,
) -> anyhow::Result<()> {
    if allow_empty || content_count > 0 || governance_count > 0 {
        return Ok(());
    }
    Err(anyhow!(
        "both audit logs are empty; pass --allow-empty to export an empty snapshot deliberately"
    ))
}

/// 真偽値フラグを共通 `read_env` / `bool_env_state` で読む。
/// 未設定は `false`、非 UTF-8 と不正値は入力値をエコーせず起動拒否（fail-closed）する。
fn bool_flag_from_env(name: &str) -> anyhow::Result<bool> {
    bool_flag_from_env_value(name, read_env(name))
}

/// `read_env` の結果を真偽値へ変換する純粋版（非 UTF-8 注入の検証にも使う）。
/// 受理値 1/true/0/false と trim・小文字化は `bool_env_state` 経由で不変。
fn bool_flag_from_env_value(name: &str, value: EnvValue) -> anyhow::Result<bool> {
    // 判定と受理値列挙の固定文言は共通ヘルパへ集約する。
    // ここでは変数名を付けるだけなので、文言の重複は存在しない。
    let (flag, invalid_message) = common::env::bool_env_result(bool_env_state(value), false, false);
    match invalid_message {
        Some(message) => Err(anyhow!("{name} {message}")),
        None => Ok(flag),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    if args.help {
        print_help();
        return Ok(());
    }
    // 本番相当では平文理由のエクスポートを拒否する。値は判定にだけ使いエコーしない。
    let allow_mock_auth = bool_flag_from_env("ALLOW_MOCK_AUTH")?;
    let explicit_opt_in = bool_flag_from_env("AUDIT_EXPORT_INCLUDE_REASONS")?;
    ensure_include_reasons_allowed(args.include_reasons, allow_mock_auth, explicit_opt_in)?;

    let signing_key_path = args
        .signing_key
        .ok_or_else(|| anyhow!("--signing-key is required"))?;
    let output = args.output.ok_or_else(|| anyhow!("--output is required"))?;
    check_signing_key_permissions(&signing_key_path)?;
    let private_key_pem =
        std::fs::read_to_string(&signing_key_path).context("failed to read signing key")?;

    // 暗黙の file:local.db 既定は撤廃。明示設定がなければ起動しない。
    let database_url = resolve_database_url(args.database_url, read_env("DATABASE_URL"))
        .map_err(|error| anyhow!(error.to_string()))?;
    // argv はプロセス一覧に露出するため、トークンは環境変数からのみ読む。
    let database_token = database_token_from_env_value(read_env("DATABASE_TOKEN"))?;

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

    // 両ログが空なら明示オプトインが無い限り拒否する（空ログの暗黙成功を防ぐ）。
    // 空扱いの判定は呼び出し側で行い、ダミールートの算出より前に置く。
    ensure_audit_not_empty(entries.len(), governance_entries.len(), args.allow_empty)?;

    let digest = compute_audit_chain_digest(
        &entries
            .iter()
            .map(|entry| (entry.log_id, entry.entry_hash.clone()))
            .collect::<Vec<_>>(),
    )
    .map_err(anyhow::Error::msg)?;

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
    // restore_db の staging + safety copy と同じ方式: 0600 一時ファイルへ書き
    // fsync してから hard_link で原子的に公開する。既存の最終パスは上書きせず、
    // hard_link が使えない環境のみ rename フォールバックで存在を再確認する。
    // 公開失敗時は一時ファイルを残し、値や CID はエコーしない。
    publish_snapshot_atomic(&output, json.as_bytes())?;
    println!(
        "audit_export_signed: output={output} entries={}",
        snapshot.entry_count()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DATABASE_TOKEN_INVALID, bool_flag_from_env_value, check_signing_key_permissions,
        create_snapshot_output, database_token_from_env_value, ensure_audit_not_empty,
        ensure_include_reasons_allowed, parse_args_from, publish_snapshot_atomic,
        publish_snapshot_atomic_with, resolve_database_url,
    };
    use common::env::{EnvValue, classify};
    use server::DatabaseUrlError;
    use std::ffi::OsString;
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
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

    /// 非 UTF-8 の生バイトを含む OsString を共通分類器へ注入し、
    /// 生バイトを保持しない `EnvValue::NonUtf8` を作る（非エコー契約の検証用）。
    fn non_utf8_env_value() -> EnvValue {
        classify(Err(std::env::VarError::NotUnicode(OsString::from_vec(
            vec![0xff],
        ))))
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
    fn option_like_value_is_rejected_without_echo() {
        const DUMMY: &str = "SECRET_VALUE_MUST_NOT_APPEAR";
        for flag in ["--signing-key", "--output", "--database-url"] {
            let argv = ["sign_audit_export", flag, "--database-token", DUMMY];
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
        assert!(!args.allow_empty);

        // --allow-empty は空ログの明示オプトインとしてパースされる。
        let with_allow_empty = parse_args_from(
            [
                "sign_audit_export",
                "--signing-key",
                "key.pem",
                "--output",
                "snapshot.json",
                "--allow-empty",
            ]
            .map(String::from),
        )
        .expect("--allow-empty must parse");
        assert!(with_allow_empty.allow_empty);

        // 両ログが空なら明示オプトイン無しで拒否し、片方でもあれば受理する。
        ensure_audit_not_empty(0, 0, false).expect_err("empty logs must be rejected");
        ensure_audit_not_empty(1, 0, false).expect("non-empty content log must be accepted");
        ensure_audit_not_empty(0, 1, false).expect("non-empty governance log must be accepted");
        ensure_audit_not_empty(0, 0, true).expect("--allow-empty must permit empty logs");
    }

    // --- include_reasons: 本番相当での平文理由エクスポート拒否 ---

    #[test]
    fn include_reasons_requires_explicit_opt_in_in_production() {
        let error = ensure_include_reasons_allowed(true, false, false)
            .expect_err("plaintext reasons must be rejected in production without opt-in");
        let message = error.to_string();
        assert!(message.contains("--include-reasons"), "{message}");
        assert!(
            message.contains("AUDIT_EXPORT_INCLUDE_REASONS"),
            "{message}"
        );
        // 明示オプトインがあれば本番でも許可（break-glass）。
        ensure_include_reasons_allowed(true, false, true)
            .expect("explicit opt-in must allow --include-reasons in production");

        // 非 UTF-8 は「未設定」ではなく Invalid として両フラグとも起動拒否する。
        // 注入した生バイト (0xff) は固定文言へ一切エコーされない。
        for name in ["AUDIT_EXPORT_INCLUDE_REASONS", "ALLOW_MOCK_AUTH"] {
            let error = bool_flag_from_env_value(name, non_utf8_env_value())
                .expect_err("non-UTF-8 boolean flag must be rejected")
                .to_string();
            assert!(error.contains(name), "{error}");
            assert!(error.contains("must be one of 1/true/0/false"), "{error}");
            assert!(!error.as_bytes().contains(&0xff), "{error:?}");
        }
    }

    #[test]
    fn include_reasons_is_allowed_with_mock_auth() {
        // 開発（mock 有効）では従来どおり許可。
        ensure_include_reasons_allowed(true, true, false)
            .expect("mock auth (development) must allow --include-reasons");
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
        // 原子的公開経路も既存ファイルを拒否し、内容を保持すること。
        let error = publish_snapshot_atomic(&path_str(&output), b"replacement")
            .expect_err("atomic publish must refuse existing output");
        assert!(
            error.to_string().contains("must not already exist"),
            "{error:#}"
        );
        assert_eq!(std::fs::read(&output).expect("read output"), b"original");
        // 公開失敗時はステージングを残す。命名は .tmp-<pid>-<unix_millis>。
        let staging: Vec<_> = std::fs::read_dir(&dir.0)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".snapshot.json.tmp-"))
            .collect();
        assert_eq!(
            staging.len(),
            1,
            "staging must be kept on failure: {staging:?}"
        );
        assert!(
            staging[0].contains(format!("-{}", std::process::id()).as_str()),
            "staging name must include pid: {}",
            staging[0]
        );
        let millis = staging[0].rsplit('-').next().unwrap_or("");
        assert!(
            !millis.is_empty() && millis.chars().all(|c| c.is_ascii_digit()),
            "staging name must include unix_millis: {}",
            staging[0]
        );
    }

    #[test]
    fn output_creates_new_file() {
        let dir = TempDir::new();
        let output = dir.child("snapshot.json");
        let mut file = create_snapshot_output(&path_str(&output)).expect("create new output");
        file.write_all(b"snapshot").expect("write output");
        drop(file);
        assert_eq!(std::fs::read(&output).expect("read output"), b"snapshot");
        // 原子的公開経路: 一時ファイルは残らず、最終パスに内容が公開されること。
        let published = dir.child("published.json");
        publish_snapshot_atomic(&path_str(&published), b"atomic").expect("publish snapshot");
        assert_eq!(
            std::fs::read(&published).expect("read published"),
            b"atomic"
        );
        let leftovers: Vec<_> = std::fs::read_dir(&dir.0)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("tmp-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "staging file must be gone: {leftovers:?}"
        );

        // hard_link 公開成功後に staging 削除が失敗しても Ok(()) を返すこと。
        // 削除関数を差し替えて決定的に失敗を注入し、公開自体は成功扱いになることを検証する。
        // ログへは staging パスのみを出し、値や CID はエコーしない契約。
        let injected = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected staging removal failure",
        );
        let published_with_failed_cleanup = dir.child("published-with-failed-cleanup.json");
        publish_snapshot_atomic_with(&path_str(&published_with_failed_cleanup), b"atomic", |_| {
            Err(injected)
        })
        .expect("staging removal failure must not fail an already published snapshot");
        assert_eq!(
            std::fs::read(&published_with_failed_cleanup).expect("read published"),
            b"atomic",
            "publication must remain visible despite staging cleanup failure"
        );
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

        // 非 UTF-8 の生バイト (0xff) を注入しても固定文言は変わらず、バイトも現れない。
        // 有効な CLI 引数があっても、非 UTF-8 の環境変数は「未設定」として
        // 飲み込まず固定文言で拒否される（CLI 優先判定の将来の回帰も防ぐ）。
        let from_non_utf8 =
            resolve_database_url(Some("file:valid.db".into()), non_utf8_env_value())
                .expect_err("non-UTF-8 DATABASE_URL must be rejected even with a valid CLI URL")
                .to_string();
        assert_eq!(from_non_utf8, DatabaseUrlError::NotUtf8.to_string());
        assert!(
            !from_non_utf8.as_bytes().contains(&0xff),
            "{from_non_utf8:?}"
        );

        // DATABASE_TOKEN も非 UTF-8 を「未設定」とせず不正として扱い、値をエコーしない。
        let token_error = database_token_from_env_value(non_utf8_env_value())
            .expect_err("non-UTF-8 DATABASE_TOKEN must be rejected")
            .to_string();
        assert_eq!(token_error, DATABASE_TOKEN_INVALID);
        assert!(!token_error.as_bytes().contains(&0xff), "{token_error:?}");
        assert!(!token_error.contains("DATABASE_TOKEN="), "{token_error}");
    }
}
