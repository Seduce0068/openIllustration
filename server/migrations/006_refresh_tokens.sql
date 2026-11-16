-- リフレッシュトークンのローテーションと盗用検知用。
-- トークン本体は保存せず SHA-256 ハッシュのみを保持する。
CREATE TABLE IF NOT EXISTS refresh_tokens (
    token_hash TEXT PRIMARY KEY,
    sub TEXT NOT NULL,
    family_id TEXT NOT NULL,
    expires_at DATETIME NOT NULL,
    used_at DATETIME,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_family ON refresh_tokens(family_id);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_sub ON refresh_tokens(sub);
