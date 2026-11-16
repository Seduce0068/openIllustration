-- sub 単位のセッション失効閾値を永続化する（再起動後も失効を維持）。
CREATE TABLE IF NOT EXISTS revoked_subs (
    sub TEXT PRIMARY KEY,
    revoked_at INTEGER NOT NULL,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
