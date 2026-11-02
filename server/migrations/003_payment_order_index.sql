-- Per-order receiving addresses derived from the configured xprv.
-- The partial unique index allows legacy single-address orders
-- (derivation_index IS NULL) while preventing address reuse.

ALTER TABLE payment_orders ADD COLUMN derivation_index INTEGER;

CREATE UNIQUE INDEX IF NOT EXISTS idx_payment_orders_derivation_index
    ON payment_orders(derivation_index)
    WHERE derivation_index IS NOT NULL;
