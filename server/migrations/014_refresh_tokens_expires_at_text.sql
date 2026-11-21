-- 014: refresh_tokens.expires_at の型宣言を DATETIME から TEXT へ整合させる。
-- 006 の DATETIME は SQLite の型 affinity であり、実際の格納値は RFC3339 文字列だった。
-- 006 はチェックサム保護のため変更しない。CHECK 制約は追加しない。

CREATE TABLE refresh_tokens_new (
    token_hash TEXT PRIMARY KEY,
    sub TEXT NOT NULL,
    family_id TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used_at DATETIME,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO refresh_tokens_new (token_hash, sub, family_id, expires_at, used_at, created_at)
SELECT token_hash, sub, family_id, expires_at, used_at, created_at FROM refresh_tokens;

DROP TABLE refresh_tokens;

ALTER TABLE refresh_tokens_new RENAME TO refresh_tokens;

CREATE INDEX idx_refresh_tokens_family ON refresh_tokens(family_id);
CREATE INDEX idx_refresh_tokens_sub ON refresh_tokens(sub);
