-- ログアウト済みセッション ID (sid) を永続化する（再起動後も失効を維持）。
-- 起動時にメモリへロードし、認証時の判定は in-memory で行う。
CREATE TABLE IF NOT EXISTS revoked_sessions (
    sid TEXT PRIMARY KEY,
    revoked_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
