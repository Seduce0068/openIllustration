-- ADR-0004: 運用者の BAN / UNBAN / ピン解除判断を記録する運用ログ。
-- これは改ざん検知付きの第三者検証可能監査（DEC-04）ではない。
CREATE TABLE IF NOT EXISTS governance_audit_log (
    log_id INTEGER PRIMARY KEY,
    action TEXT NOT NULL CHECK(action IN ('BAN', 'UNBAN', 'UNPIN')),
    target TEXT NOT NULL,
    actor_hash TEXT NOT NULL,
    reason TEXT NOT NULL,
    details TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_governance_audit_log_target ON governance_audit_log(target);
CREATE INDEX IF NOT EXISTS idx_governance_audit_log_created_at ON governance_audit_log(created_at DESC);
