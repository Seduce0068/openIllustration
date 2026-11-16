-- 注文を対象コンテンツへ紐づけ、提示価格との照合を可能にする。
ALTER TABLE payment_orders ADD COLUMN cid TEXT REFERENCES contents(cid);
CREATE INDEX IF NOT EXISTS idx_payment_orders_cid ON payment_orders(cid);
