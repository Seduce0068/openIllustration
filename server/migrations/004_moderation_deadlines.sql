-- ADR-0004: 7-day voting window, explicit conclusion time and appeal linkage.
-- 空の deadline_at は created_at + 7 days として扱う（既存行の救済）。
ALTER TABLE reports ADD COLUMN deadline_at DATETIME;
ALTER TABLE reports ADD COLUMN concluded_at DATETIME;
ALTER TABLE reports ADD COLUMN appeal_of INTEGER;
UPDATE reports SET deadline_at = datetime(created_at, '+7 days') WHERE deadline_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_reports_deadline_at ON reports(deadline_at);
CREATE INDEX IF NOT EXISTS idx_reports_concluded_at ON reports(concluded_at);
CREATE INDEX IF NOT EXISTS idx_reports_appeal_of ON reports(appeal_of);
