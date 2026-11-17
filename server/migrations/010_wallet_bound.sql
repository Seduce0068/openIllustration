-- R1-24: ウォレット所有証明（bind）を注文状態へ永続化する。
-- 既存注文は既定 0（未バインド）となり、verify の必須条件を満たさない。
ALTER TABLE payment_orders ADD COLUMN wallet_bound INTEGER NOT NULL DEFAULT 0;
