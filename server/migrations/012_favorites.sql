-- R5-02: お気に入り / ブックマーク。
-- ユーザー識別は JWT の Claims.sub のみ。リクエストから ID は受け取らない。
-- user_hash / cid の組は一意とし、重複追加は INSERT OR IGNORE で冪等にする。
-- ユーザーまたはコンテンツが削除されたら、お気に入りも CASCADE で削除する。

CREATE TABLE IF NOT EXISTS favorites (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_hash TEXT NOT NULL REFERENCES users(user_hash) ON DELETE CASCADE,
    cid TEXT NOT NULL REFERENCES contents(cid) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- UNIQUE(user_hash, cid): 同じユーザーが同じコンテンツを複数回登録できないようにする。
CREATE UNIQUE INDEX IF NOT EXISTS idx_favorites_user_cid ON favorites(user_hash, cid);

-- お気に入り一覧（本人分・新着順）の参照用インデックス。
CREATE INDEX IF NOT EXISTS idx_favorites_user_created ON favorites(user_hash, created_at DESC);
