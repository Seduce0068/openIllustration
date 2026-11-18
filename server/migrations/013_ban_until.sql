-- BAN の期限（ban_until）を users に永続化する。
-- 既存行は NULL のまま（後方互換）。NULL=無期限、期限切れでも is_banned は TRUE のまま、判定側で時刻比較する。
-- 時刻は SQLite の datetime() と比較できる TEXT（ISO 8601 形式）で保存する。

ALTER TABLE users ADD COLUMN ban_until TEXT;

-- 期限付き BAN の一括回収・判定側の時刻比較で使うインデックス。
CREATE INDEX IF NOT EXISTS idx_users_ban_until ON users(ban_until);
