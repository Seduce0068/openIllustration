-- Payment orders for JPYC settlement (ADR-0003).
-- Amounts are stored as decimal strings in the token's smallest unit
-- so that 10^18-scale values never pass through SQLite integers.

CREATE TABLE IF NOT EXISTS payment_orders (
    order_id TEXT PRIMARY KEY,
    ppid TEXT NOT NULL,
    chain_id INTEGER NOT NULL,
    wallet_address TEXT NOT NULL,
    recipient_address TEXT NOT NULL,
    amount_wei TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN (
        'PENDING', 'DETECTED', 'CONFIRMING', 'FINALIZED',
        'FAILED', 'MISMATCH', 'EXPIRED'
    )),
    tx_hash TEXT,
    log_index INTEGER,
    block_number INTEGER,
    confirmations INTEGER NOT NULL DEFAULT 0,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (chain_id, tx_hash, log_index)
);

CREATE INDEX IF NOT EXISTS idx_payment_orders_ppid
    ON payment_orders(ppid);
CREATE INDEX IF NOT EXISTS idx_payment_orders_status
    ON payment_orders(status);
CREATE INDEX IF NOT EXISTS idx_payment_orders_expires_at
    ON payment_orders(expires_at);
