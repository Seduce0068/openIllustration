// Migration runner for OpenIllustration
// Applies versioned SQL migrations from server/migrations/
//
// Conventions:
// - Filenames: `NNN_name.sql` with zero-padded, monotonically increasing numbers
//   (e.g. `001_initial_schema.sql`). Ordering is lexicographic on the filename.
// - Migrations are applied in one transaction per file and recorded in the
//   `_migrations` table together with a SHA-256 checksum of the file content.
// - Applied migrations must not be edited; edits are detected at startup and
//   abort boot (see `verify_migrations`).
// - Statements are split naively on `;` after stripping `--` line comments.
//   Avoid semicolons and comment markers inside string literals.

use crate::AppError;
use libsql::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

const MIGRATIONS_DIR: &str = "migrations";
const MIGRATIONS_TABLE: &str = "_migrations";

/// Get the migrations directory path, checking multiple locations
fn get_migrations_dir() -> PathBuf {
    // 1. Relative to the current directory. Works when the server runs from
    //    `server/` (cargo run -p server in server dir, cargo test, deployed layout
    //    with binary + migrations directory together).
    let path = Path::new(MIGRATIONS_DIR);
    if path.exists() {
        return path.to_path_buf();
    }
    // 2. Runtime override (useful for deployment without a colocated migrations dir).
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
    // Fallback: return the default (will log warning if not found)
    warn!(
        "Migrations directory not found, tried: ./migrations, $CARGO_MANIFEST_DIR/migrations, <crate>/migrations"
    );
    Path::new(MIGRATIONS_DIR).to_path_buf()
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

/// Apply a single migration file
async fn apply_migration(
    db: &Connection,
    version: &str,
    name: &str,
    sql: &str,
) -> Result<(), AppError> {
    let checksum = calculate_checksum(sql);

    // Check if already applied with same checksum
    let mut rows = db
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
            info!("Migration {} ({}) already applied, skipping", version, name);
            return Ok(());
        } else {
            return Err(AppError::Internal(format!(
                "Migration {} ({}) checksum mismatch: file checksum {}, applied checksum {}",
                version, name, checksum, existing_checksum
            )));
        }
    }

    info!("Applying migration {}: {}", version, name);

    // Execute migration SQL in a transaction
    let tx = db.transaction().await?;

    // Statement splitter: strips `--` line comments, then splits on `;`.
    // LIMITATION: string literals containing `--` or `;` are not supported.
    // Keep migrations free of semicolons/comment markers inside string literals.
    let sql_no_comments: String = sql
        .lines()
        .map(|line| {
            if let Some(idx) = line.find("--") {
                &line[..idx]
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let statements: Vec<&str> = sql_no_comments
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    for statement in statements {
        tx.execute(statement, ()).await?;
    }

    // Record migration as applied
    tx.execute(
        &format!(
            "INSERT INTO {} (version, name, checksum) VALUES (?, ?, ?)",
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
    let migrations_path = get_migrations_dir();
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

    let entries = fs::read_dir(migrations_path)
        .map_err(|e| AppError::Internal(format!("Failed to read migrations directory: {}", e)))?;
    let mut entries: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();

    // Sort by filename (version prefix)
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let filename = entry.file_name().to_string_lossy().to_string();
        // Expect format: NNN_name.sql
        let version = filename.split('_').next().unwrap_or("").to_string();
        if version.is_empty() {
            warn!(
                "Skipping migration with invalid filename format: {}",
                filename
            );
            continue;
        }

        if applied.contains(&version) {
            info!("Migration {} already applied, skipping", version);
            continue; // Already applied
        }

        info!("Applying migration: {}", filename);
        let sql = fs::read_to_string(entry.path())
            .map_err(|e| AppError::Internal(format!("Failed to read migration file: {}", e)))?;
        let name = filename.trim_end_matches(".sql").to_string();
        apply_migration(db, &version, &name, &sql).await?;
    }

    Ok(())
}

/// Verify all migrations match expected checksums (for CI/CD)
pub async fn verify_migrations(db: &Connection) -> Result<(), AppError> {
    let migrations_path = get_migrations_dir();
    verify_migrations_from(db, &migrations_path).await
}

/// Verify checksums against an explicit directory (testable entry point)
pub async fn verify_migrations_from(
    db: &Connection,
    migrations_path: &Path,
) -> Result<(), AppError> {
    ensure_migrations_table(db).await?;

    if !migrations_path.exists() {
        return Ok(());
    }

    let entries = fs::read_dir(migrations_path)
        .map_err(|e| AppError::Internal(format!("Failed to read migrations directory: {}", e)))?;
    let mut entries: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();

    entries.sort_by_key(|e| e.file_name());

    let file_versions: std::collections::HashSet<String> = entries
        .iter()
        .filter_map(|entry| {
            let filename = entry.file_name().to_string_lossy().to_string();
            let version = filename.split('_').next().unwrap_or("").to_string();
            (!version.is_empty()).then_some(version)
        })
        .collect();

    // 適用済みだがファイルが無いバージョンは、古いバイナリ/誤った migrations ディレクトリ。
    // 想定より新しいスキーマに対して起動すると後続の SQL が失敗するため早期に拒否する。
    let applied = get_applied_versions(db).await?;
    if let Some(unknown) = applied
        .iter()
        .find(|version| !file_versions.contains(*version))
    {
        return Err(AppError::Internal(format!(
            "Applied migration {unknown} has no matching file; refusing to start with an older or mismatched migration set"
        )));
    }

    for entry in entries {
        let filename = entry.file_name().to_string_lossy().to_string();
        let version = filename.split('_').next().unwrap_or("").to_string();
        if version.is_empty() {
            continue;
        }

        let sql = fs::read_to_string(entry.path())
            .map_err(|e| AppError::Internal(format!("Failed to read migration file: {}", e)))?;
        let expected_checksum = calculate_checksum(&sql);

        let mut rows = db
            .query(
                &format!(
                    "SELECT checksum FROM {} WHERE version = ?",
                    MIGRATIONS_TABLE
                ),
                [version.as_str()],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            let stored_checksum: String = row.get(0)?;
            if stored_checksum != expected_checksum {
                return Err(AppError::Internal(format!(
                    "Migration {} checksum mismatch: file checksum {}, applied checksum {}",
                    version, expected_checksum, stored_checksum
                )));
            }
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

        let _ = fs::remove_dir_all(&dir);
    }
}
