-- Initial schema for OpenIllustration Guild Backend
-- Generated from server/src/lib.rs run() function

-- Users table: Identity and public key storage
CREATE TABLE IF NOT EXISTS users (
    user_hash TEXT PRIMARY KEY,
    public_key_der BLOB,
    is_banned BOOLEAN DEFAULT FALSE,
    banned_at DATETIME
);

-- Contents table: IPFS-pinned content metadata
CREATE TABLE IF NOT EXISTS contents (
    cid TEXT PRIMARY KEY,
    type TEXT CHECK(type IN ('Illustration', 'Novel')),
    owner_hash TEXT REFERENCES users(user_hash),
    checksum TEXT NOT NULL,
    status TEXT CHECK(status IN ('Approved', 'Reported', 'Rejected')) DEFAULT 'Approved',
    price INTEGER DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Transactions table: P2P support payments (currently simulated)
CREATE TABLE IF NOT EXISTS transactions (
    tx_id INTEGER PRIMARY KEY,
    cid TEXT REFERENCES contents(cid),
    sender_hash TEXT REFERENCES users(user_hash),
    receiver_hash TEXT REFERENCES users(user_hash),
    amount INTEGER NOT NULL,
    tx_hash TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Signatures table: Cryptographic signatures for auditability
CREATE TABLE IF NOT EXISTS signatures (
    signature_id INTEGER PRIMARY KEY,
    target_id TEXT NOT NULL, -- CID, report_id, or vote_id
    signer_hash TEXT NOT NULL,
    signature_blob BLOB NOT NULL,
    algorithm TEXT NOT NULL, -- e.g., 'RSASSA-PKCS1-v1_5'
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Content audit log: Hash-chained state transitions
CREATE TABLE IF NOT EXISTS content_audit_log (
    log_id INTEGER PRIMARY KEY,
    cid TEXT REFERENCES contents(cid),
    old_status TEXT,
    new_status TEXT,
    changed_by TEXT,
    reason TEXT,
    previous_log_hash TEXT,
    entry_hash TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Reports table: Content moderation reports
CREATE TABLE IF NOT EXISTS reports (
    report_id INTEGER PRIMARY KEY,
    target_cid TEXT REFERENCES contents(cid),
    reason TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Votes table: Jury votes on reports
CREATE TABLE IF NOT EXISTS votes (
    vote_id INTEGER PRIMARY KEY,
    report_id INTEGER REFERENCES reports(report_id),
    voter_hash TEXT REFERENCES users(user_hash),
    vote_result TEXT CHECK(vote_result IN ('Approve', 'Reject')),
    voted_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_contents_owner_hash ON contents(owner_hash);
CREATE INDEX IF NOT EXISTS idx_contents_status ON contents(status);
CREATE INDEX IF NOT EXISTS idx_contents_created_at ON contents(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_transactions_cid ON transactions(cid);
CREATE INDEX IF NOT EXISTS idx_transactions_sender_hash ON transactions(sender_hash);
CREATE INDEX IF NOT EXISTS idx_transactions_receiver_hash ON transactions(receiver_hash);
CREATE INDEX IF NOT EXISTS idx_signatures_target_id ON signatures(target_id);
CREATE INDEX IF NOT EXISTS idx_signatures_signer_hash ON signatures(signer_hash);
CREATE INDEX IF NOT EXISTS idx_content_audit_log_cid ON content_audit_log(cid);
CREATE INDEX IF NOT EXISTS idx_reports_target_cid ON reports(target_cid);
CREATE INDEX IF NOT EXISTS idx_votes_report_id ON votes(report_id);
CREATE INDEX IF NOT EXISTS idx_votes_voter_hash ON votes(voter_hash);

-- Unique constraint to prevent duplicate votes (not in current DDL but required per docs)
CREATE UNIQUE INDEX IF NOT EXISTS idx_votes_unique ON votes(report_id, voter_hash);