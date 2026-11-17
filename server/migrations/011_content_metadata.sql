-- R5-01: コンテンツのメタデータ（タイトル / 説明 / タグ）。
-- 既存行は空文字列として読めるように NOT NULL DEFAULT '' で追加する。
-- tags はカンマ区切りの TEXT で保存し、前方一致検索は LIKE のバインド
-- パラメータで行う（値は SQL へ埋め込まない）。

ALTER TABLE contents ADD COLUMN title TEXT NOT NULL DEFAULT '';
ALTER TABLE contents ADD COLUMN description TEXT NOT NULL DEFAULT '';
ALTER TABLE contents ADD COLUMN tags TEXT NOT NULL DEFAULT '';

-- tag パラメータの前方一致（`tag%` / `%,tag%`）で使うインデックス。
CREATE INDEX IF NOT EXISTS idx_contents_tags ON contents(tags);
