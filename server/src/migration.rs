// Migration runner for OpenIllustration
// Applies versioned SQL migrations from server/migrations/
//
// Conventions:
// - Filenames must be strictly `NNN_name.sql`: `NNN` is exactly three digits and
//   `name` is one or more lowercase letters/digits/underscores
//   (e.g. `001_initial_schema.sql`). Every `.sql` file is validated before any
//   migration is applied; invalid names, gaps and duplicates abort the run.
// - Migrations are applied in one transaction per file and recorded in the
//   `_migrations` table together with a SHA-256 checksum of the file content.
// - Applied migrations must not be edited; edits are detected at startup and
//   abort boot (see `verify_migrations`).
// - Statements are split by a SQL-aware scanner that honours single/double-quoted
//   literals and `--` line comments, so `;` / `--` inside literals are safe.
//   Block comments and dollar-quoted strings are not interpreted.

use crate::AppError;
use libsql::Connection;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

const MIGRATIONS_DIR: &str = "migrations";
const MIGRATIONS_TABLE: &str = "_migrations";

/// Fixed diagnostics for `MIGRATIONS_DIR` validation. The raw value is never
/// echoed: invalid configuration is rejected fail-closed without leaking it.
const MIGRATIONS_DIR_NOT_UTF8: &str =
    "MIGRATIONS_DIR must be valid UTF-8; refusing to run migrations";
const MIGRATIONS_DIR_NOT_ABSOLUTE: &str =
    "MIGRATIONS_DIR must be an absolute path; refusing to run migrations";
const MIGRATIONS_DIR_NOT_FOUND: &str =
    "MIGRATIONS_DIR does not point to an existing directory; refusing to run migrations";
const MIGRATIONS_DIR_NOT_READABLE: &str =
    "MIGRATIONS_DIR directory is not readable; refusing to run migrations";

/// Development back-compat search order (unchanged from the original runner):
/// `./migrations` (cwd) → `$CARGO_MANIFEST_DIR/migrations` →
/// `<compile-time crate dir>/migrations`. First existing candidate wins.
fn default_migrations_dir() -> PathBuf {
    // 1. Relative to the current directory. Works when the server runs from
    //    `server/` (cargo run -p server in server dir, cargo test, deployed layout
    //    with binary + migrations directory together).
    let path = Path::new(MIGRATIONS_DIR);
    if path.exists() {
        return path.to_path_buf();
    }
    // 2. Runtime manifest dir (dev / build tooling convenience).
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let path = Path::new(&manifest_dir).join(MIGRATIONS_DIR);
        if path.exists() {
            return path;
        }
    }
    // 3. Compile-time crate directory (dev convenience: cargo run from the
    //    workspace root still finds server/migrations/).
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        let path = Path::new(manifest_dir).join(MIGRATIONS_DIR);
        if path.exists() {
            return path;
        }
    }
    // Fallback: return the default (run_migrations_from skips a missing dir,
    // verify_migrations_from rejects it fail-closed).
    warn!(
        "Migrations directory not found, tried: ./migrations, $CARGO_MANIFEST_DIR/migrations, <crate>/migrations"
    );
    Path::new(MIGRATIONS_DIR).to_path_buf()
}

/// Validate an explicit `MIGRATIONS_DIR` override, fail-closed.
///
/// Checks, in order: absolute path, existing directory, readability, and the
/// same strict `.sql` enumeration / filename / 001..N sequence validation used
/// by `run_migrations` and `verify_migrations`. Any failure is an error — an
/// invalid override never silently falls back to the development defaults.
fn validate_migrations_dir_override(value: &str) -> Result<PathBuf, AppError> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(AppError::Internal(MIGRATIONS_DIR_NOT_ABSOLUTE.to_string()));
    }
    if !path.is_dir() {
        return Err(AppError::Internal(MIGRATIONS_DIR_NOT_FOUND.to_string()));
    }
    // read_dir はディレクトリの読み取り可能性を実際に検査する。
    // 失敗時は OS 由来の文言を出さず固定文言へ変換する。
    if fs::read_dir(path).is_err() {
        return Err(AppError::Internal(MIGRATIONS_DIR_NOT_READABLE.to_string()));
    }
    // .sql 列挙・ファイル名・001 からの連番を共有の厳密検証で担保する。
    // ここで拒否しないと不正なセットが run / verify をすり抜ける。
    collect_migration_files(path)?;
    Ok(path.to_path_buf())
}

/// Resolve the migrations directory from the supplied `MIGRATIONS_DIR` value.
///
/// `Some(value)` takes highest priority and is validated fail-closed;
/// `None` keeps the development back-compat search order. The value is passed
/// in rather than read here so tests never mutate the process environment
/// (`std::env::set_var` is `unsafe` in edition 2024).
fn resolve_migrations_dir(migrations_dir: Option<String>) -> Result<PathBuf, AppError> {
    match migrations_dir {
        Some(value) => validate_migrations_dir_override(&value),
        None => Ok(default_migrations_dir()),
    }
}

/// Read `MIGRATIONS_DIR` from the runtime environment and resolve the path.
fn get_migrations_dir() -> Result<PathBuf, AppError> {
    let value = match std::env::var_os("MIGRATIONS_DIR") {
        // 非 UTF-8 値は黙って既定へフォールバックせず固定文言で拒否する。
        Some(raw) => Some(
            raw.to_str()
                .ok_or_else(|| AppError::Internal(MIGRATIONS_DIR_NOT_UTF8.to_string()))?
                .to_string(),
        ),
        None => None,
    };
    resolve_migrations_dir(value)
}

/// Initialize the migrations tracking table
async fn ensure_migrations_table(db: &Connection) -> Result<(), AppError> {
    db.execute(
        &format!(
            "CREATE TABLE IF NOT EXISTS {} (
                version TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                checksum TEXT NOT NULL
            )",
            MIGRATIONS_TABLE
        ),
        (),
    )
    .await?;
    Ok(())
}

/// Get list of applied migration versions
async fn get_applied_versions(db: &Connection) -> Result<Vec<String>, AppError> {
    let mut rows = db
        .query(
            &format!("SELECT version FROM {} ORDER BY version", MIGRATIONS_TABLE),
            (),
        )
        .await?;
    let mut versions = Vec::new();
    while let Some(row) = rows.next().await? {
        versions.push(row.get(0)?);
    }
    Ok(versions)
}

/// Calculate SHA-256 checksum of migration content
fn calculate_checksum(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// A migration file that passed strict filename validation.
#[derive(Debug, Clone)]
struct MigrationFile {
    /// Numeric value of the `NNN` prefix (used for sequence validation).
    version: u32,
    /// Zero-padded token as written in the filename, e.g. `"001"`.
    version_token: String,
    /// Full file name, used in diagnostics.
    filename: String,
    /// `NNN_name` (filename without `.sql`); kept as the `_migrations.name` value
    /// for compatibility with databases migrated by earlier versions.
    name: String,
    path: PathBuf,
}

fn invalid_migration_filename(filename: &str) -> AppError {
    AppError::Internal(format!(
        "Invalid migration filename '{filename}': expected NNN_name.sql (NNN = exactly three digits, name = one or more lowercase letters/digits/underscores)"
    ))
}

/// Strictly parse `NNN_name.sql`.
///
/// Accepting an unvalidated name would record an arbitrary value (e.g. `notes`)
/// as `_migrations.version` and let `verify_migrations` accept it as well, so
/// anything outside the documented format is rejected here (fail-closed).
fn parse_migration_filename(filename: &str) -> Result<(u32, &str, &str), AppError> {
    let reject = || invalid_migration_filename(filename);
    let stem = filename.strip_suffix(".sql").ok_or_else(&reject)?;
    let (version_token, name) = stem.split_once('_').ok_or_else(&reject)?;

    if version_token.len() != 3
        || !version_token.bytes().all(|b| b.is_ascii_digit())
        || name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err(reject());
    }

    let version = version_token.parse::<u32>().map_err(|_| reject())?;
    Ok((version, version_token, stem))
}

/// Enumerate all `.sql` files, validate their names and version sequence, and
/// return them ordered by numeric version.
fn collect_migration_files(migrations_path: &Path) -> Result<Vec<MigrationFile>, AppError> {
    let entries = fs::read_dir(migrations_path)
        .map_err(|e| AppError::Internal(format!("Failed to read migrations directory: {}", e)))?;

    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| {
            AppError::Internal(format!("Failed to read migrations directory entry: {}", e))
        })?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("sql") {
            continue;
        }
        let filename = entry.file_name().to_string_lossy().to_string();
        let (version, version_token, name) = parse_migration_filename(&filename)?;
        let version_token = version_token.to_string();
        let name = name.to_string();
        files.push(MigrationFile {
            version,
            version_token,
            filename,
            name,
            path,
        });
    }

    // 空ディレクトリを「001 が無い」として fail-closed で拒否する。
    if files.is_empty() {
        return Err(AppError::Internal(EMPTY_MIGRATIONS_ERROR.to_string()));
    }

    files.sort_by_key(|file| file.version);
    validate_migration_sequence(&files)?;
    Ok(files)
}

/// 空の migrations ディレクトリを拒否する単一情報源（最低 001 が必要）。
const EMPTY_MIGRATIONS_ERROR: &str =
    "No migration files found: at least migration 001 is required; refusing to run migrations";

/// Versions must start at 001 and increase by exactly one, with no duplicates.
fn validate_migration_sequence(files: &[MigrationFile]) -> Result<(), AppError> {
    if files.is_empty() {
        return Err(AppError::Internal(EMPTY_MIGRATIONS_ERROR.to_string()));
    }
    let mut expected = 1u32;
    let mut previous: Option<&MigrationFile> = None;

    for file in files {
        if let Some(prev) = previous
            && prev.version == file.version
        {
            return Err(AppError::Internal(format!(
                "Duplicate migration version '{}' in '{}' and '{}'; refusing to run migrations",
                file.version_token, prev.filename, file.filename
            )));
        }
        if file.version != expected {
            return Err(AppError::Internal(format!(
                "Non-contiguous migration versions: expected {:03} but found '{}'; migration versions must start at 001 with no gaps",
                expected, file.filename
            )));
        }
        previous = Some(file);
        expected = expected.saturating_add(1);
    }

    Ok(())
}

/// Split migration SQL into statements for execution.
///
/// A single character scan recognises single/double-quoted literals (with SQL's
/// doubled-quote escapes) and `--` line comments. `;` and `--` inside a quoted
/// literal are preserved; comment text is discarded before splitting.
///
/// LIMITATION: block comments (`/* ... */`) and dollar-quoted strings (`$$...$$`)
/// are not interpreted, so migrations must avoid them.
pub(crate) fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = sql.char_indices().peekable();

    while let Some((_, ch)) = chars.next() {
        match quote {
            Some(q) => {
                current.push(ch);
                if ch == q {
                    // SQL escapes a quote inside a literal by doubling it.
                    match chars.peek() {
                        Some(&(_, next)) if next == q => {
                            current.push(next);
                            chars.next();
                        }
                        _ => quote = None,
                    }
                }
            }
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    current.push(ch);
                }
                '-' => {
                    if matches!(chars.peek(), Some(&(_, '-'))) {
                        // Discard the rest of the `--` line comment but keep
                        // the newline so following lines are parsed normally.
                        chars.next();
                        for (_, comment_ch) in chars.by_ref() {
                            if comment_ch == '\n' {
                                current.push('\n');
                                break;
                            }
                        }
                    } else {
                        current.push(ch);
                    }
                }
                ';' => {
                    let statement = current.trim().to_owned();
                    if !statement.is_empty() {
                        statements.push(statement);
                    }
                    current.clear();
                }
                _ => current.push(ch),
            },
        }
    }

    let tail = current.trim();
    if !tail.is_empty() {
        statements.push(tail.to_owned());
    }

    statements
}

/// Apply a single migration file
async fn apply_migration(
    db: &Connection,
    version: &str,
    name: &str,
    sql: &str,
) -> Result<(), AppError> {
    let checksum = calculate_checksum(sql);

    // ADR-0013 P4: リモートDBでは instance.lock が強制されないため、
    // write_lock を取得してから BEGIN し、適用済み判定も tx 内で再検証する。
    // ALTER/DROP は冪等でないため、ロック外の判定だけで「未適用」と決めつけると
    // 複数インスタンスの同時起動で二重適用になる（checksum 再読はロック後）。
    let (_write_guard, tx) = crate::begin_write_tx(db).await?;

    let mut rows = tx
        .query(
            &format!(
                "SELECT checksum FROM {} WHERE version = ?",
                MIGRATIONS_TABLE
            ),
            [version],
        )
        .await?;

    if let Some(row) = rows.next().await? {
        let existing_checksum: String = row.get(0)?;
        if existing_checksum == checksum {
            drop(rows);
            tx.commit().await?;
            info!("Migration {} ({}) already applied, skipping", version, name);
            return Ok(());
        } else {
            drop(rows);
            return Err(AppError::Internal(format!(
                "Migration {} ({}) checksum mismatch: file checksum {}, applied checksum {}",
                version, name, checksum, existing_checksum
            )));
        }
    }
    drop(rows);

    info!("Applying migration {}: {}", version, name);

    // SQL-aware splitter: `;` / `--` inside quoted literals stay intact and
    // `--` comment text is removed before splitting.
    // LIMITATION: block comments and dollar quoting are not parsed (see function).
    for statement in split_statements(sql) {
        tx.execute(statement.as_str(), ()).await?;
    }

    // Record migration as applied
    // 上位の write_lock で直列化済みのため、適用済み判定→INSERT の競合は起きない。
    // INSERT ... ON CONFLICT DO NOTHING は保険（checksum 不一致は上で fail-closed）。
    tx.execute(
        &format!(
            "INSERT INTO {} (version, name, checksum) VALUES (?, ?, ?)
             ON CONFLICT(version) DO NOTHING",
            MIGRATIONS_TABLE
        ),
        (version, name, checksum),
    )
    .await?;

    tx.commit().await?;
    info!("Migration {} applied successfully", version);
    Ok(())
}

/// Run all pending migrations from the default migrations directory
pub async fn run_migrations(db: &Connection) -> Result<(), AppError> {
    let migrations_path = get_migrations_dir()?;
    run_migrations_from(db, &migrations_path).await
}

/// Run all pending migrations from an explicit directory (testable entry point)
pub async fn run_migrations_from(db: &Connection, migrations_path: &Path) -> Result<(), AppError> {
    ensure_migrations_table(db).await?;
    let applied = get_applied_versions(db).await?;

    if !migrations_path.exists() {
        warn!(
            "Migrations directory not found at {:?}, skipping",
            migrations_path
        );
        return Ok(());
    }

    info!("Running migrations from {:?}", migrations_path);

    // Fail-closed: validate every filename and the whole version sequence before
    // applying anything, so an unknown/non-numeric file can never become a
    // `_migrations.version` value that later passes verification.
    let files = collect_migration_files(migrations_path)?;
    // collect が拒否するはずだが、公開入口でも明示的に空を拒否する（多層防御）。
    if files.is_empty() {
        return Err(AppError::Internal(EMPTY_MIGRATIONS_ERROR.to_string()));
    }

    for file in files {
        if applied.contains(&file.version_token) {
            info!("Migration {} already applied, skipping", file.version_token);
            continue; // Already applied
        }

        info!("Applying migration: {}", file.filename);
        let sql = fs::read_to_string(&file.path)
            .map_err(|e| AppError::Internal(format!("Failed to read migration file: {}", e)))?;
        apply_migration(db, &file.version_token, &file.name, &sql).await?;
    }

    Ok(())
}

/// Verify all migrations match expected checksums (for CI/CD)
pub async fn verify_migrations(db: &Connection) -> Result<(), AppError> {
    let migrations_path = get_migrations_dir()?;
    verify_migrations_from(db, &migrations_path).await
}

/// Verify checksums against an explicit directory (testable entry point)
pub async fn verify_migrations_from(
    db: &Connection,
    migrations_path: &Path,
) -> Result<(), AppError> {
    ensure_migrations_table(db).await?;

    // ディレクトリ自体が無い場合は「適用済みバージョンに対応するファイルが無い」
    // 検査をすり抜けてしまうため、fail-closed で起動を拒否する。
    if !migrations_path.exists() {
        return Err(AppError::Internal(format!(
            "Migrations directory not found: {}; refusing to start without migration files",
            migrations_path.display()
        )));
    }

    // run_migrations と同じ厳密検証を共有する。ここでも検証しないと、
    // run 側をすり抜けた不正ファイルが verify を通過してしまう。
    let files = collect_migration_files(migrations_path)?;
    // collect が拒否するはずだが、公開入口でも明示的に空を拒否する（多層防御）。
    if files.is_empty() {
        return Err(AppError::Internal(EMPTY_MIGRATIONS_ERROR.to_string()));
    }

    let file_versions: HashSet<String> = files
        .iter()
        .map(|file| file.version_token.clone())
        .collect();

    // 適用済みだがファイルが無いバージョンは、古いバイナリ/誤った migrations ディレクトリ。
    // 想定より新しいスキーマに対して起動すると後続の SQL が失敗するため早期に拒否する。
    // （削除された migration の検出はこのチェックと上記の連続性チェックが担う。）
    let applied = get_applied_versions(db).await?;
    if let Some(unknown) = applied
        .iter()
        .find(|version| !file_versions.contains(*version))
    {
        return Err(AppError::Internal(format!(
            "Applied migration {unknown} has no matching file; refusing to start with an older or mismatched migration set"
        )));
    }

    for file in files {
        let sql = fs::read_to_string(&file.path)
            .map_err(|e| AppError::Internal(format!("Failed to read migration file: {}", e)))?;
        let expected_checksum = calculate_checksum(&sql);

        let mut rows = db
            .query(
                &format!(
                    "SELECT checksum FROM {} WHERE version = ?",
                    MIGRATIONS_TABLE
                ),
                [file.version_token.as_str()],
            )
            .await?;

        let stored = match rows.next().await? {
            Some(row) => Some(row.get::<String>(0)?),
            None => None,
        };
        // read-then-next-query の順序を統一するため、次のファイルへ進む前にカーソルを閉じる。
        drop(rows);
        if let Some(stored_checksum) = stored
            && stored_checksum != expected_checksum
        {
            return Err(AppError::Internal(format!(
                "Migration {} checksum mismatch: file checksum {}, applied checksum {}",
                file.version_token, expected_checksum, stored_checksum
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsql::Builder;

    fn temp_migrations_dir(tag: &str) -> PathBuf {
        let base = std::env::var("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let dir = base.join(format!("openillustration_migration_test_{}", tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_migration(dir: &Path, version: u32, name: &str, sql: &str) {
        fs::write(dir.join(format!("{version:03}_{name}.sql")), sql).unwrap();
    }

    fn write_contiguous_migrations(dir: &Path, count: u32) {
        for version in 1..=count {
            write_migration(
                dir,
                version,
                "migration",
                &format!("CREATE TABLE migration_{version} (id INTEGER PRIMARY KEY);"),
            );
        }
    }

    async fn memory_db() -> Connection {
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap()
    }

    #[test]
    fn migration_filename_parser_accepts_only_documented_format() {
        let (version, token, name) = parse_migration_filename("001_initial_schema.sql").unwrap();
        assert_eq!(version, 1);
        assert_eq!(token, "001");
        assert_eq!(name, "001_initial_schema");

        for bad in [
            "notes.sql",
            "01_a.sql",
            "0001_a.sql",
            "001_.sql",
            "001_a-b.sql",
            "001_A.sql",
            "001_a.txt",
            "abc_a.sql",
        ] {
            assert!(
                parse_migration_filename(bad).is_err(),
                "{bad} must be rejected"
            );
        }
    }

    /// 敵対的レビューで指摘されたケース: 数値でないファイル名が version として
    /// 記録され、verify も通過してしまう問題を fail-closed で拒否する。
    #[tokio::test]
    async fn rejects_invalid_migration_filename_in_run_and_verify() {
        let dir = temp_migrations_dir("badname");
        write_migration(
            &dir,
            1,
            "initial",
            "CREATE TABLE initial_t (id INTEGER PRIMARY KEY);",
        );
        fs::write(
            dir.join("notes.sql"),
            "CREATE TABLE notes (id INTEGER PRIMARY KEY);",
        )
        .unwrap();

        let db = memory_db().await;
        let run_err = run_migrations_from(&db, &dir).await.unwrap_err();
        assert!(
            run_err.to_string().contains("Invalid migration filename")
                && run_err.to_string().contains("notes.sql"),
            "unexpected error: {run_err}"
        );

        let verify_err = verify_migrations_from(&db, &dir).await.unwrap_err();
        assert!(
            verify_err
                .to_string()
                .contains("Invalid migration filename"),
            "verify must also reject the invalid filename, got: {verify_err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// 追加された 999_future.sql が連続性違反として検出されること。
    #[tokio::test]
    async fn rejects_added_non_contiguous_migration_version() {
        let dir = temp_migrations_dir("future");
        write_contiguous_migrations(&dir, 13);

        let db = memory_db().await;
        run_migrations_from(&db, &dir).await.unwrap();
        verify_migrations_from(&db, &dir).await.unwrap();

        write_migration(
            &dir,
            999,
            "future",
            "CREATE TABLE future_t (id INTEGER PRIMARY KEY);",
        );

        let run_err = run_migrations_from(&db, &dir).await.unwrap_err();
        assert!(
            run_err
                .to_string()
                .contains("Non-contiguous migration versions")
                && run_err.to_string().contains("expected 014"),
            "unexpected error: {run_err}"
        );
        let verify_err = verify_migrations_from(&db, &dir).await.unwrap_err();
        assert!(
            verify_err
                .to_string()
                .contains("Non-contiguous migration versions"),
            "verify must also reject the gap, got: {verify_err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// 同一バージョンの重複（013_dup.sql 等）が検出されること。
    #[tokio::test]
    async fn rejects_duplicate_migration_version() {
        let dir = temp_migrations_dir("duplicate");
        write_contiguous_migrations(&dir, 13);

        let db = memory_db().await;
        run_migrations_from(&db, &dir).await.unwrap();

        write_migration(
            &dir,
            13,
            "dup",
            "CREATE TABLE dup_t (id INTEGER PRIMARY KEY);",
        );

        let run_err = run_migrations_from(&db, &dir).await.unwrap_err();
        assert!(
            run_err
                .to_string()
                .contains("Duplicate migration version '013'"),
            "unexpected error: {run_err}"
        );
        let verify_err = verify_migrations_from(&db, &dir).await.unwrap_err();
        assert!(
            verify_err
                .to_string()
                .contains("Duplicate migration version '013'"),
            "verify must also reject duplicates, got: {verify_err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// 連続した migration セット（001〜013 相当）は従来どおり適用できること。
    #[tokio::test]
    async fn accepts_full_contiguous_migration_set() {
        let dir = temp_migrations_dir("fullset");
        write_contiguous_migrations(&dir, 13);

        let db = memory_db().await;
        run_migrations_from(&db, &dir).await.unwrap();
        verify_migrations_from(&db, &dir).await.unwrap();
        assert_eq!(get_applied_versions(&db).await.unwrap().len(), 13);

        // --- MIGRATIONS_DIR override の境界検証 ---
        // 環境変数を汚さず resolve_migrations_dir へ値を注入して検証する。
        // 1. 未設定 = 従来の既定探索（開発後方互換）。
        let unset = resolve_migrations_dir(None).unwrap();
        assert_eq!(unset, default_migrations_dir());
        assert!(unset.ends_with(MIGRATIONS_DIR));

        // 2. 有効な絶対パスは解決され、連番セットも検証を通過する。
        let dir_value = dir.to_str().unwrap().to_string();
        let resolved = resolve_migrations_dir(Some(dir_value)).unwrap();
        assert_eq!(resolved, dir);

        // 3. 存在しないパスは固定文言で拒否する（既定へフォールバックしない）。
        let missing = dir.join("does_not_exist");
        let missing_value = missing.to_str().unwrap().to_string();
        let err = resolve_migrations_dir(Some(missing_value)).unwrap_err();
        assert!(
            err.to_string().contains("MIGRATIONS_DIR")
                && err.to_string().contains("existing directory"),
            "unexpected error: {err}"
        );

        // 4. 相対パスは絶対パス要件違反として拒否する。
        let relative = MIGRATIONS_DIR.to_string();
        let err = resolve_migrations_dir(Some(relative)).unwrap_err();
        assert!(
            err.to_string().contains("absolute path"),
            "unexpected error: {err}"
        );

        // 5. 001 から連続しないセットは override 解決時点で fail-closed に拒否する。
        let gapped = temp_migrations_dir("override_gap");
        write_migration(
            &gapped,
            1,
            "initial",
            "CREATE TABLE gap1 (id INTEGER PRIMARY KEY);",
        );
        write_migration(
            &gapped,
            3,
            "later",
            "CREATE TABLE gap3 (id INTEGER PRIMARY KEY);",
        );
        let gapped_value = gapped.to_str().unwrap().to_string();
        let err = resolve_migrations_dir(Some(gapped_value)).unwrap_err();
        assert!(
            err.to_string()
                .contains("Non-contiguous migration versions"),
            "unexpected error: {err}"
        );

        let _ = fs::remove_dir_all(&gapped);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 空の migrations ディレクトリ（001 が無い）は run / verify ともに拒否されること。
    #[tokio::test]
    async fn rejects_empty_migrations_directory() {
        let dir = temp_migrations_dir("empty");
        let db = memory_db().await;

        let run_err = run_migrations_from(&db, &dir).await.unwrap_err();
        assert!(
            run_err.to_string().contains("at least migration 001"),
            "unexpected error: {run_err}"
        );
        let verify_err = verify_migrations_from(&db, &dir).await.unwrap_err();
        assert!(
            verify_err.to_string().contains("at least migration 001"),
            "unexpected error: {verify_err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn applies_pending_migration_and_creates_tables() {
        let dir = temp_migrations_dir("apply");
        fs::write(
            dir.join("001_test.sql"),
            "-- comment line\nCREATE TABLE test_table (id INTEGER PRIMARY KEY, name TEXT);\nCREATE INDEX idx_test_name ON test_table(name);",
        )
        .unwrap();

        let db = Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap();
        run_migrations_from(&db, &dir).await.unwrap();

        // Table created
        db.execute("INSERT INTO test_table (id, name) VALUES (1, 'a')", ())
            .await
            .unwrap();

        // Recorded as applied
        let applied = get_applied_versions(&db).await.unwrap();
        assert_eq!(applied, vec!["001".to_string()]);

        // Idempotent second run
        run_migrations_from(&db, &dir).await.unwrap();
        let applied = get_applied_versions(&db).await.unwrap();
        assert_eq!(applied.len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn detects_modified_migration_checksum() {
        let dir = temp_migrations_dir("checksum");
        let file = dir.join("001_test.sql");
        fs::write(&file, "CREATE TABLE first_table (id INTEGER PRIMARY KEY);").unwrap();

        let db = Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap();
        run_migrations_from(&db, &dir).await.unwrap();

        // Modify the migration after it was applied
        fs::write(
            &file,
            "CREATE TABLE first_table (id INTEGER PRIMARY KEY, extra TEXT);",
        )
        .unwrap();

        // Same checksum table exists; apply_migration must reject the changed file
        let err = apply_migration(
            &db,
            "001",
            "001_test",
            "CREATE TABLE first_table (id INTEGER PRIMARY KEY, extra TEXT);",
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// 既存DBの段階的アップグレード（001→003 適用後に 004 を追加）を再現する。
    #[tokio::test]
    async fn upgrades_existing_database_with_new_migration() {
        let dir = temp_migrations_dir("upgrade");
        fs::write(
            dir.join("001_initial.sql"),
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);",
        )
        .unwrap();
        fs::write(
            dir.join("002_index.sql"),
            "CREATE INDEX idx_items_name ON items(name);",
        )
        .unwrap();
        fs::write(
            dir.join("003_column.sql"),
            "ALTER TABLE items ADD COLUMN note TEXT;",
        )
        .unwrap();

        let db = Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap();
        run_migrations_from(&db, &dir).await.unwrap();
        db.execute("INSERT INTO items (id, name) VALUES (1, 'old')", ())
            .await
            .unwrap();
        assert_eq!(
            get_applied_versions(&db).await.unwrap(),
            vec!["001".to_string(), "002".to_string(), "003".to_string()]
        );

        // 新しいマイグレーションを追加して再起動相当の処理を実行
        fs::write(
            dir.join("004_deadline.sql"),
            "ALTER TABLE items ADD COLUMN deadline_at DATETIME;\nUPDATE items SET deadline_at = datetime('now', '+7 days') WHERE deadline_at IS NULL;",
        )
        .unwrap();
        run_migrations_from(&db, &dir).await.unwrap();
        verify_migrations_from(&db, &dir).await.unwrap();

        let mut rows = db
            .query("SELECT name, note, deadline_at FROM items WHERE id = 1", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let name: String = row.get(0).unwrap();
        let note: Option<String> = row.get(1).unwrap();
        let deadline: Option<String> = row.get(2).unwrap();
        assert_eq!(name, "old", "existing data must survive the upgrade");
        assert!(note.is_none());
        assert!(deadline.is_some(), "backfill must populate the new column");
        assert_eq!(get_applied_versions(&db).await.unwrap().len(), 4);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn verify_migrations_rejects_missing_applied_version() {
        let dir = temp_migrations_dir("missing");
        fs::write(
            dir.join("001_initial.sql"),
            "CREATE TABLE v1_table (id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        fs::write(
            dir.join("002_later.sql"),
            "ALTER TABLE v1_table ADD COLUMN later TEXT;",
        )
        .unwrap();

        let db = Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap();
        run_migrations_from(&db, &dir).await.unwrap();

        // 002 のファイルを失った状態（古いデプロイイメージ等）を再現
        fs::remove_file(dir.join("002_later.sql")).unwrap();
        let err = verify_migrations_from(&db, &dir).await.unwrap_err();
        assert!(
            err.to_string().contains("no matching file"),
            "unexpected error: {err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn verify_migrations_accepts_matching_checksums() {
        let dir = temp_migrations_dir("verify");
        fs::write(
            dir.join("001_test.sql"),
            "CREATE TABLE verify_table (id INTEGER PRIMARY KEY);",
        )
        .unwrap();

        let db = Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap();
        run_migrations_from(&db, &dir).await.unwrap();

        // Matching checksums pass
        verify_migrations_from(&db, &dir).await.unwrap();

        // Modified file is detected as drift
        fs::write(
            dir.join("001_test.sql"),
            "CREATE TABLE verify_table (id INTEGER PRIMARY KEY, extra TEXT);",
        )
        .unwrap();
        let err = verify_migrations_from(&db, &dir).await.unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"));

        // ディレクトリを丸ごと失った場合も fail-closed で拒否する
        // （「適用済みファイル欠落」検査をすり抜けない）。
        fs::remove_dir_all(&dir).unwrap();
        let err = verify_migrations_from(&db, &dir).await.unwrap_err();
        assert!(
            err.to_string().contains("Migrations directory not found"),
            "unexpected error: {err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
