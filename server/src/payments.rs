//! JPYC payment orders and settlement verification (ADR-0003).

use crate::{AppError, AppState, AuthClaims, is_safe_cid};
use async_trait::async_trait;
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Duration, Utc};
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use std::sync::Arc;
use tracing::warn;
use uuid::Uuid;

/// 注文の有効期限（分）。ADR-0003 の既定値。
pub const ORDER_TTL_MINUTES: i64 = 30;
/// JPYC の小数桁。コントラクトの `decimals()` と一致することを本番で確認する。
pub const JPYC_DECIMALS: u32 = 18;
/// 1回の注文上限（円）。ADR-0003 の既定値。
/// 価格は日本円の整数（1〜`MAX_ORDER_JPY`）。
/// チェーン上の送金額は wei（10^18 スケール）で別途保持し、混同しない。
pub type AmountJpy = u64;

pub const MAX_ORDER_JPY: AmountJpy = 1_000_000;

pub const STATUS_PENDING: &str = "PENDING";
pub const STATUS_DETECTED: &str = "DETECTED";
pub const STATUS_CONFIRMING: &str = "CONFIRMING";
pub const STATUS_FINALIZED: &str = "FINALIZED";
pub const STATUS_FAILED: &str = "FAILED";
pub const STATUS_MISMATCH: &str = "MISMATCH";
pub const STATUS_EXPIRED: &str = "EXPIRED";

/// 対応チェーンとJPYCプロキシコントラクト（4チェーン共通）。
pub const ALLOWED_CHAINS: [(i64, &str); 4] = [
    (1, "0xe7c3d8c9a439fede00d2600032d5db0be71c3c29"),
    (137, "0xe7c3d8c9a439fede00d2600032d5db0be71c3c29"),
    (43114, "0xe7c3d8c9a439fede00d2600032d5db0be71c3c29"),
    (8217, "0xe7c3d8c9a439fede00d2600032d5db0be71c3c29"),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentOrder {
    pub order_id: String,
    pub ppid: String,
    pub cid: Option<String>,
    pub chain_id: i64,
    pub wallet_address: String,
    pub recipient_address: String,
    pub amount_wei: String,
    pub status: String,
    pub tx_hash: Option<String>,
    pub log_index: Option<i64>,
    pub block_number: Option<i64>,
    pub confirmations: i64,
    pub expires_at: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateOrderRequest {
    /// 購入対象のコンテンツ CID。提示価格と amount_jpy を照合する。
    pub cid: String,
    pub chain_id: i64,
    pub wallet_address: String,
    /// 日本円建ての金額。1〜1,000,000。
    pub amount_jpy: AmountJpy,
}

#[derive(Debug, Serialize)]
pub struct CreateOrderResponse {
    pub order_id: String,
    pub chain_id: i64,
    pub recipient_address: String,
    pub amount_wei: String,
    pub expires_at: String,
}

#[derive(Debug, Deserialize)]
pub struct BindWalletRequest {
    /// 65 バイトの Ethereum 署名（0x 任意）。
    pub signature: String,
}

#[derive(Debug, Clone)]
pub struct PaymentVerification {
    pub chain_id: i64,
    pub tx_hash: String,
    pub log_index: i64,
    pub block_number: i64,
    pub confirmations: i64,
    pub from: String,
    pub to: String,
    pub amount_wei: String,
}

/// チェーンRPCから送金を取得する境界。テストではモックを注入する。
#[async_trait]
pub trait PaymentVerifier: Send + Sync {
    async fn verify(&self, order: &PaymentOrder) -> Result<PaymentVerification, AppError>;
}

pub fn is_allowed_chain(chain_id: i64) -> bool {
    ALLOWED_CHAINS
        .iter()
        .any(|(allowed, _)| *allowed == chain_id)
}

pub fn required_confirmations(chain_id: i64) -> i64 {
    match chain_id {
        1 => 32,
        43114 => 12,
        137 => 256,
        8217 => 12,
        _ => i64::MAX,
    }
}

/// 0x 形式の 20 バイトアドレスか。EIP-55 のチェックサムは表示用であり、
/// EVM のアドレスは大小文字を区別しないため、照合時は `normalize_address` で
/// 小文字化して比較する（チェックサム不一致は送金先の誤りにならない）。
pub fn is_valid_eth_address(address: &str) -> bool {
    let Some(hex) = address.strip_prefix("0x") else {
        return false;
    };
    // ゼロアドレスは回収不能な送金先なので受け付けない。
    hex.len() == 40 && hex.chars().all(|c| c.is_ascii_hexdigit()) && hex.bytes().any(|b| b != b'0')
}

pub fn normalize_address(address: &str) -> String {
    address.to_ascii_lowercase()
}

/// 注文IDのみを署名対象にする旧形式。
/// 金額・送金先・チェーンを確認できない盲署名リスクがあるため、
/// 新規の署名生成には `wallet_bind_message_v2` を使うこと。
#[deprecated(
    note = "use wallet_bind_message_v2 with chain_id, wallet_address, recipient_address and amount_jpy"
)]
// 旧クライアントが参照してもコンパイルできるよう API 互換のため残す。
#[allow(dead_code)]
pub fn wallet_bind_message(order_id: &str) -> String {
    format!("openIllustration payment order {order_id}")
}

/// ウォレット所有証明のメッセージ（v2）。
/// 署名者が送金先・金額・チェーンを確認できるよう、注文IDに加えて
/// chain_id / wallet_address / recipient_address / amount_jpy を含む。
/// アドレスは EVM の大小文字無視を前提に小文字化し、表示用チェックサムの
/// 違いで署名検証が失敗しないようにする。
pub fn wallet_bind_message_v2(
    order_id: &str,
    chain_id: i64,
    wallet_address: &str,
    recipient_address: &str,
    amount_jpy: AmountJpy,
) -> String {
    format!(
        "openIllustration payment order {order_id} chain {chain_id} wallet {} recipient {} amount_jpy {amount_jpy}",
        normalize_address(wallet_address),
        normalize_address(recipient_address),
    )
}

/// amount_wei の文字列表現を u128 へ正規化する。
/// 先頭ゼロなどの表記差は同じ値として扱うため、金額の比較は必ずこの値で行う。
/// 非数値や u128 の範囲超過（オーバーフロー）はデータ不整合として Internal を返す
/// （fail-closed）。DB の保存形式（文字列）は変更しない。
fn parse_amount_wei(amount_wei: &str, source: &str) -> Result<u128, AppError> {
    amount_wei
        .parse::<u128>()
        .map_err(|_| AppError::Internal(format!("invalid amount_wei in {source}: {amount_wei}")))
}

/// DB に保存された amount_wei から円建て金額を導出する。
/// amount_wei は常に `jpy_to_wei` の 10^18 スケールで保存されるため、
/// 割り切れない値はデータ不整合として拒否する。
/// ゼロも `jpy_to_wei(0)` と同じく「注文金額は正」という契約に反するため拒否する。
pub fn amount_jpy_from_wei(amount_wei: &str) -> Result<AmountJpy, AppError> {
    let wei = parse_amount_wei(amount_wei, "order")?;
    let scale = 10u128
        .checked_pow(JPYC_DECIMALS)
        .ok_or_else(|| AppError::Internal("JPYC decimal scale overflow".to_string()))?;
    let jpy = wei
        .checked_div(scale)
        .filter(|jpy| jpy * scale == wei)
        .ok_or_else(|| {
            AppError::Internal(format!(
                "amount_wei is not a whole JPY amount: {amount_wei}"
            ))
        })?;
    if jpy == 0 {
        return Err(AppError::Internal("amount_wei is zero".to_string()));
    }
    AmountJpy::try_from(jpy)
        .map_err(|_| AppError::Internal(format!("amount_wei exceeds u64 JPY: {amount_wei}")))
}

/// 円建て金額を JPYC の最小単位（10^18）へ変換する。
/// DB の derivation_index (i64) を BIP44 用の u32 へ安全に変換する。
/// 境界を超える値は受取アドレスの再利用を防ぐため拒否する。
pub fn derivation_index_to_u32(index: i64) -> Result<u32, AppError> {
    // non-hardened パス m/44'/60'/0'/0/{index} では index を 0..2^31 に制限する。
    // 上限を超える値は受取アドレスの再利用を防ぐため拒否する（fail-closed）。
    if (0..(1i64 << 31)).contains(&index) {
        Ok(index as u32)
    } else {
        Err(AppError::Internal(format!(
            "derivation index {index} exceeds the supported maximum {}",
            (1u64 << 31) - 1
        )))
    }
}

pub fn jpy_to_wei(amount_jpy: AmountJpy) -> Result<String, AppError> {
    if amount_jpy == 0 || amount_jpy > MAX_ORDER_JPY {
        return Err(AppError::BadRequest(format!(
            "amount_jpy must be between 1 and {MAX_ORDER_JPY}"
        )));
    }
    let wei = u128::from(amount_jpy)
        .checked_mul(10u128.pow(JPYC_DECIMALS))
        .ok_or_else(|| AppError::BadRequest("amount is too large".to_string()))?;
    Ok(wei.to_string())
}

/// xprv から注文用の受取アドレスを BIP44 パスで派生する。
pub fn derive_receiving_address(xprv: &str, index: u32) -> Result<String, AppError> {
    use coins_bip32::xkeys::XPriv;

    let root: XPriv = xprv
        .parse()
        .map_err(|e| AppError::Internal(format!("invalid receiving xprv: {e}")))?;
    let path = format!("m/44'/60'/0'/0/{index}");
    let child = root
        .derive_path(path.as_str())
        .map_err(|e| AppError::Internal(format!("receiving address derivation failed: {e}")))?;
    let signing_key: &coins_bip32::prelude::SigningKey = child.as_ref();
    let secret = signing_key.to_bytes();
    let key = k256::ecdsa::SigningKey::from_slice(&secret[..])
        .map_err(|e| AppError::Internal(format!("derived key is invalid: {e}")))?;
    Ok(address_from_verifying_key(key.verifying_key()))
}

/// EIP-191 `personal_sign` の署名から Ethereum アドレスを復元する。
pub fn recover_ethereum_address(message: &[u8], signature_hex: &str) -> Result<String, AppError> {
    // hex::decode の前に長さを検査し、巨大な入力をデコードしない。
    // 130 = 65 バイト（0x 無し） / 132 = 65 バイト（0x 付き）。
    if signature_hex.len() != 130 && signature_hex.len() != 132 {
        return Err(AppError::BadRequest(
            "signature hex must be 130 or 132 characters".to_string(),
        ));
    }
    let raw = signature_hex.strip_prefix("0x").unwrap_or(signature_hex);
    let signature =
        hex::decode(raw).map_err(|_| AppError::BadRequest("signature must be hex".to_string()))?;
    if signature.len() != 65 {
        return Err(AppError::BadRequest(
            "signature must be 65 bytes".to_string(),
        ));
    }

    let v = match signature[64] {
        0 | 1 => signature[64],
        27 | 28 => signature[64] - 27,
        _ => {
            return Err(AppError::BadRequest(
                "invalid signature recovery id".to_string(),
            ));
        }
    };
    let recovery_id = RecoveryId::from_byte(v)
        .ok_or_else(|| AppError::BadRequest("invalid recovery id".to_string()))?;
    let signature = Signature::from_slice(&signature[..64])
        .map_err(|_| AppError::BadRequest("invalid signature".to_string()))?;

    let mut hasher = Keccak256::new();
    hasher.update(format!("\x19Ethereum Signed Message:\n{}", message.len()).as_bytes());
    hasher.update(message);
    let digest = hasher.finalize();

    let key = VerifyingKey::recover_from_prehash(digest.as_slice(), &signature, recovery_id)
        .map_err(|_| AppError::Unauthorized("signature recovery failed".to_string()))?;
    Ok(address_from_verifying_key(&key))
}

pub fn address_from_verifying_key(key: &VerifyingKey) -> String {
    let point = key.to_sec1_point(false);
    let hash = Keccak256::digest(&point.as_bytes()[1..]);
    format!("0x{}", hex::encode(&hash[12..]))
}

fn order_from_row(row: &libsql::Row) -> Result<PaymentOrder, AppError> {
    Ok(PaymentOrder {
        order_id: row.get(0)?,
        ppid: row.get(1)?,
        cid: row.get(2)?,
        chain_id: row.get(3)?,
        wallet_address: row.get(4)?,
        recipient_address: row.get(5)?,
        amount_wei: row.get(6)?,
        status: row.get(7)?,
        tx_hash: row.get(8)?,
        log_index: row.get(9)?,
        block_number: row.get(10)?,
        confirmations: row.get(11)?,
        expires_at: row.get(12)?,
        created_at: row.get(13)?,
    })
}

async fn load_order(
    state: &AppState,
    order_id: &str,
    ppid: &str,
) -> Result<PaymentOrder, AppError> {
    if Uuid::parse_str(order_id).is_err() {
        return Err(AppError::BadRequest("Invalid order id".to_string()));
    }

    let mut rows = state
        .db
        .query(
            "SELECT order_id, ppid, cid, chain_id, wallet_address, recipient_address,
                    amount_wei, status, tx_hash, log_index, block_number,
                    confirmations, expires_at, created_at
             FROM payment_orders WHERE order_id = ? AND ppid = ?",
            (order_id, ppid),
        )
        .await?;
    match rows.next().await? {
        Some(row) => order_from_row(&row),
        // 他人の注文を存在確認に使われないよう 404 に統一する。
        None => Err(AppError::NotFound),
    }
}

/// insert_payment_order の第1引数が `&libsql::Transaction` であることを型レベルで固定する。
/// シグネチャを `&Connection` に戻すと、FnOnce の境界では deref coercion が効かないため
/// この関数自体がコンパイルできなくなる（将来の `&state.db` 誤用を型で防ぐことの検証）。
#[cfg(test)]
#[allow(dead_code)]
fn assert_insert_payment_order_requires_transaction(tx: &libsql::Transaction) {
    fn accept_transaction<'a, F, Fut>(_f: F, _tx: &'a libsql::Transaction)
    where
        F: FnOnce(
            &'a libsql::Transaction,
            &'a str,
            &'a str,
            &'a str,
            i64,
            &'a str,
            &'a str,
            &'a str,
            &'a str,
            Option<i64>,
        ) -> Fut,
        Fut: std::future::Future<Output = Result<(), crate::AppError>>,
    {
    }
    accept_transaction(insert_payment_order, tx);
}

/// payment_orders へ 1 行挿入する。
/// 呼び出し元の書き込みトランザクション（`&libsql::Transaction`）に渡し、
/// HD 分岐・固定アドレス分岐のどちらも同じ write_lock 経路で INSERT する。
#[allow(clippy::too_many_arguments)]
async fn insert_payment_order(
    db: &libsql::Transaction,
    order_id: &str,
    ppid: &str,
    cid: &str,
    chain_id: i64,
    wallet_address: &str,
    recipient_address: &str,
    amount_wei: &str,
    expires_at: &str,
    derivation_index: Option<i64>,
) -> Result<(), AppError> {
    db.execute(
        "INSERT INTO payment_orders
            (order_id, ppid, cid, chain_id, wallet_address, recipient_address,
             amount_wei, status, expires_at, derivation_index)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (
            order_id,
            ppid,
            cid,
            chain_id,
            wallet_address,
            recipient_address,
            amount_wei,
            STATUS_PENDING,
            expires_at,
            derivation_index,
        ),
    )
    .await?;
    Ok(())
}

/// 書き込み tx 内で contents の価格・状態を再検証する。
/// create_order の事前検証と INSERT の間で status / price が変わる TOCTOU を塞ぎ、
/// 検証と payment_orders INSERT を同一 tx に載せる。エラー文言は事前検証と同じ。
async fn ensure_orderable_content_in_tx(
    tx: &libsql::Transaction,
    cid: &str,
    amount_jpy: u64,
) -> Result<(), AppError> {
    let mut rows = tx
        .query(
            "SELECT COALESCE(price, 0) AS price, status FROM contents WHERE cid = ?",
            [cid.to_string()],
        )
        .await?;
    let (content_price, content_status): (i64, String) = match rows.next().await? {
        Some(row) => (row.get(0)?, row.get(1)?),
        None => return Err(AppError::NotFound),
    };
    if content_status != "Approved" {
        return Err(AppError::BadRequest(
            "Content is not available for purchase".to_string(),
        ));
    }
    if content_price <= 0 || content_price as u64 != amount_jpy {
        return Err(AppError::BadRequest(
            "amount_jpy must match the content price".to_string(),
        ));
    }
    Ok(())
}

pub async fn create_order(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Json(payload): Json<CreateOrderRequest>,
) -> Result<Json<CreateOrderResponse>, AppError> {
    if !is_safe_cid(&payload.cid) {
        return Err(AppError::BadRequest("Invalid CID".to_string()));
    }
    // 提示価格と支払額を照合する。存在しない/レビュー中のコンテンツは注文できない。
    // これは早期 400 応答と index 消費防止のための事前検証であり、INSERT と同一 tx
    // 内でも ensure_orderable_content_in_tx で再検証して TOCTOU を塞ぐ。
    let mut content_rows = state
        .db
        .query(
            "SELECT COALESCE(price, 0) AS price, status FROM contents WHERE cid = ?",
            [payload.cid.clone()],
        )
        .await?;
    let (content_price, content_status): (i64, String) = match content_rows.next().await? {
        Some(row) => (row.get(0)?, row.get(1)?),
        None => return Err(AppError::NotFound),
    };
    drop(content_rows);

    if content_status != "Approved" {
        return Err(AppError::BadRequest(
            "Content is not available for purchase".to_string(),
        ));
    }
    if content_price <= 0 || content_price as u64 != payload.amount_jpy {
        return Err(AppError::BadRequest(
            "amount_jpy must match the content price".to_string(),
        ));
    }

    // index 採番の前に全入力を検証し、無駄な index 消費を防ぐ（R1-26）。
    if !is_allowed_chain(payload.chain_id) {
        return Err(AppError::BadRequest("unsupported chain".to_string()));
    }
    if !is_valid_eth_address(&payload.wallet_address) {
        return Err(AppError::BadRequest(
            "wallet_address must be a 0x-prefixed 20-byte address".to_string(),
        ));
    }
    let amount_wei = jpy_to_wei(payload.amount_jpy)?;

    let order_id = Uuid::new_v4().to_string();
    let expires_at = (Utc::now() + Duration::minutes(ORDER_TTL_MINUTES)).to_rfc3339();
    let wallet_address = normalize_address(&payload.wallet_address);

    if let Some(xprv) = &state.payment_receiving_xprv {
        // 採番・アドレス派生・INSERT を 1 つの書き込みトランザクションで行う。
        // ライトロックが同一プロセスの並行注文を直列化し、UNIQUE(derivation_index) が
        // それをすり抜けた場合の受取アドレス再利用も拒否する。
        let (_write_guard, tx) = crate::begin_write_tx(&state.db).await?;
        // 価格・status の検証を INSERT と同じ tx 内で再確認する。
        ensure_orderable_content_in_tx(&tx, &payload.cid, payload.amount_jpy).await?;
        let index: i64 = {
            let mut rows = tx
                .query(
                    "SELECT COALESCE(MAX(derivation_index), -1) + 1 FROM payment_orders",
                    (),
                )
                .await?;
            match rows.next().await? {
                Some(row) => row.get(0)?,
                None => 0,
            }
        };
        let recipient_address = derive_receiving_address(xprv, derivation_index_to_u32(index)?)?;
        insert_payment_order(
            &tx,
            &order_id,
            &claims.sub,
            &payload.cid,
            payload.chain_id,
            &wallet_address,
            &recipient_address,
            &amount_wei,
            &expires_at,
            Some(index),
        )
        .await?;
        // 失敗時は `?` で早期リターンし、tx の drop でロールバックする（既存パターン）。
        tx.commit().await?;

        Ok(Json(CreateOrderResponse {
            order_id,
            chain_id: payload.chain_id,
            recipient_address,
            amount_wei,
            expires_at,
        }))
    } else if let Some(address) = &state.payment_receiving_address {
        // 固定アドレス分岐も HD 分岐と同じ書き込みトランザクション（write_lock）経由で INSERT する。
        // derivation_index は NULL のまま従前どおり。
        let recipient_address = normalize_address(address);
        let (_write_guard, tx) = crate::begin_write_tx(&state.db).await?;
        // HD 分岐と同じく、検証と INSERT を同じ tx 内で行う。
        ensure_orderable_content_in_tx(&tx, &payload.cid, payload.amount_jpy).await?;
        insert_payment_order(
            &tx,
            &order_id,
            &claims.sub,
            &payload.cid,
            payload.chain_id,
            &wallet_address,
            &recipient_address,
            &amount_wei,
            &expires_at,
            None,
        )
        .await?;
        tx.commit().await?;

        Ok(Json(CreateOrderResponse {
            order_id,
            chain_id: payload.chain_id,
            recipient_address,
            amount_wei,
            expires_at,
        }))
    } else {
        Err(AppError::NotImplemented(
            "JPYC receiving address is not configured".to_string(),
        ))
    }
}

pub async fn get_order(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Path(order_id): Path<String>,
) -> Result<Json<PaymentOrder>, AppError> {
    Ok(Json(load_order(&state, &order_id, &claims.sub).await?))
}

/// 期限切れ判定。RFC3339 文字列を時刻としてパースして比較する。
/// 小数秒の桁数（chrono の `AutoSi` は 0/3/6/9 桁）や `Z` / `+HH:MM` の表記差で
/// 辞書順が逆転しても誤判定しないよう、`oidc::refresh_token_expired` と同じ方式を取る。
/// パース失敗は fail-closed（期限切れ扱い）。`expires_at == now` の境界も期限切れとする
/// 既存契約を維持するため比較は `<=` のまま。
/// テストから境界値を直接渡せるよう、時刻取得と比較を分離している。
pub(crate) fn is_order_expired(expires_at: &str, now: &str) -> bool {
    match (
        DateTime::parse_from_rfc3339(expires_at),
        DateTime::parse_from_rfc3339(now),
    ) {
        (Ok(expires_at), Ok(now)) => expires_at.with_timezone(&Utc) <= now.with_timezone(&Utc),
        // 不正な値を「まだ有効」と扱うと期限切れ注文の決済を通してしまうため fail-closed。
        _ => true,
    }
}

/// 期限切れなら EXPIRED へ更新する（更新系エンドポイント共通）。
/// 更新は呼び出し元の書き込みトランザクション内で行い、コミットは呼び出し元が行う。
/// 期限切れで更新した場合は `true` を返す。呼び出し側はコミット後に既存どおり
/// `AppError::BadRequest("order has expired")`（400）を返す。
/// `expires_at` は呼び出し側が読み取った値を渡す（初回は注文読み込み時、
/// 検証後の再判定ではトランザクション内の最新値）。
async fn reject_if_expired(
    tx: &libsql::Transaction,
    order_id: &str,
    expires_at: &str,
) -> Result<bool, AppError> {
    if is_order_expired(expires_at, &chrono::Utc::now().to_rfc3339()) {
        tx.execute(
            // MISMATCH は確定済みの不一致判定なので EXPIRED で上書きしない。
            "UPDATE payment_orders SET status = ? WHERE order_id = ? AND status NOT IN (?, ?, ?)",
            (
                STATUS_EXPIRED,
                order_id,
                STATUS_FINALIZED,
                STATUS_EXPIRED,
                STATUS_MISMATCH,
            ),
        )
        .await?;
        // MISMATCH は確定済みの不一致判定なので期限切れ扱いにしない。
        let mut mismatch_rows = tx
            .query(
                "SELECT 1 FROM payment_orders WHERE order_id = ? AND status = ?",
                (order_id, STATUS_MISMATCH),
            )
            .await?;
        let is_mismatch = mismatch_rows.next().await?.is_some();
        drop(mismatch_rows);
        return Ok(!is_mismatch);
    }
    Ok(false)
}

pub async fn bind_wallet(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Path(order_id): Path<String>,
    Json(payload): Json<BindWalletRequest>,
) -> Result<Json<bool>, AppError> {
    let order = load_order(&state, &order_id, &claims.sub).await?;
    if order.status == STATUS_EXPIRED || order.status == STATUS_FAILED {
        return Err(AppError::BadRequest(
            "order is no longer verifiable".to_string(),
        ));
    }
    // 期限切れ判定と status 確認は write_lock を取得したトランザクション内で行う。
    // 期限切れなら更新をコミットしてから 400 を返す（旧挙動を維持）。
    let (_write_guard, tx) = crate::begin_write_tx(&state.db).await?;
    // tx 外で読んだ order の status / expires_at は古い可能性があるため、
    // 同じ tx 内で最新値を読み直す（verify_order の期限再判定と同じ契約）。
    let (current_status, current_expires_at) = {
        let mut rows = tx
            .query(
                "SELECT status, expires_at FROM payment_orders WHERE order_id = ?",
                [order.order_id.as_str()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => (row.get::<String>(0)?, row.get::<String>(1)?),
            None => (order.status.clone(), order.expires_at.clone()),
        }
    };
    if reject_if_expired(&tx, &order.order_id, &current_expires_at).await? {
        tx.commit().await?;
        return Err(AppError::BadRequest("order has expired".to_string()));
    }
    if current_status != STATUS_PENDING {
        // 競合時は verify_order の CAS と同じ 409・固定文言で拒否する。
        return Err(AppError::Conflict(
            "order status changed during wallet binding".to_string(),
        ));
    }

    // 盲署名を防ぐため、注文IDだけでなく送金先・金額・チェーンも署名対象に含める。
    let amount_jpy = amount_jpy_from_wei(&order.amount_wei)?;
    let message = wallet_bind_message_v2(
        &order.order_id,
        order.chain_id,
        &order.wallet_address,
        &order.recipient_address,
        amount_jpy,
    );
    let recovered = recover_ethereum_address(message.as_bytes(), &payload.signature)?;
    if normalize_address(&recovered) != normalize_address(&order.wallet_address) {
        return Err(AppError::Unauthorized(
            "signature does not match the order wallet".to_string(),
        ));
    }

    // 所有証明を DB へ記録する。再起動後も verify の必須条件として残る。
    // status = 'PENDING' を CAS 条件にし、0 件更新なら成功応答しない。
    // 期限再判定で取得したトランザクションをそのまま使い、一括でコミットする。
    let update = tx
        .execute(
            "UPDATE payment_orders
             SET wallet_bound = 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE order_id = ? AND status = 'PENDING'",
            [order.order_id.as_str()],
        )
        .await?;
    if tx.changes() != 1 {
        return Err(AppError::Conflict(
            "order status changed during wallet binding".to_string(),
        ));
    }
    debug_assert_eq!(update, 1);
    tx.commit().await?;

    Ok(Json(true))
}

/// ウォレット所有証明が記録済みか。未記録なら決済検証を許可しない。
async fn is_wallet_bound(db: &libsql::Connection, order_id: &str) -> Result<bool, AppError> {
    let mut rows = db
        .query(
            "SELECT wallet_bound FROM payment_orders WHERE order_id = ?",
            [order_id],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(row.get::<i64>(0)? != 0),
        None => Err(AppError::NotFound),
    }
}

/// `payment_orders(chain_id, tx_hash, log_index)` の UNIQUE 違反だけを同一送金の再利用として扱う。
///
/// ローカル DB の `SqliteFailure` 第1引数は extended SQLite コードで、
/// UNIQUE 違反は `SQLITE_CONSTRAINT_UNIQUE` (2067) となる。リモート DB (Hrana) の
/// `RemoteSqliteFailure` は第1引数が基本コード (19)、第2引数が extended コードなので
/// 両方を見る。メッセージ文字列は UNIQUE 判定のフォールバックには使わず、
/// コードが UNIQUE のときだけ違反カラム名を検査して対象インデックスかを確定する
/// （メッセージは判定以外へ渡さず、クライアントへは返さない）。
fn is_unique_constraint_violation(error: &libsql::Error) -> bool {
    const SQLITE_CONSTRAINT_UNIQUE: i32 = 2067;
    const UNIQUE_COLUMNS: [&str; 3] = [
        "payment_orders.chain_id",
        "payment_orders.tx_hash",
        "payment_orders.log_index",
    ];
    let message = match error {
        libsql::Error::SqliteFailure(code, message) if *code == SQLITE_CONSTRAINT_UNIQUE => {
            message.as_str()
        }
        libsql::Error::RemoteSqliteFailure(code, extended_code, message)
            if *code == SQLITE_CONSTRAINT_UNIQUE || *extended_code == SQLITE_CONSTRAINT_UNIQUE =>
        {
            message.as_str()
        }
        _ => return false,
    };
    let Some(columns) = message.strip_prefix("UNIQUE constraint failed:") else {
        return false;
    };
    let violated: Vec<&str> = columns.split(',').map(str::trim).collect();
    violated.len() == UNIQUE_COLUMNS.len()
        && UNIQUE_COLUMNS
            .iter()
            .all(|column| violated.contains(column))
}

pub async fn verify_order(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Path(order_id): Path<String>,
) -> Result<Json<PaymentOrder>, AppError> {
    let order = load_order(&state, &order_id, &claims.sub).await?;
    if order.status == STATUS_FINALIZED {
        return Ok(Json(order));
    }
    if order.status == STATUS_EXPIRED || order.status == STATUS_FAILED {
        return Err(AppError::BadRequest(
            "order is no longer verifiable".to_string(),
        ));
    }
    // status が PENDING のまま expires_at を過ぎた注文も期限切れとして扱う。
    // EXPIRED 更新は write_lock を取得したトランザクション内で行い、期限切れなら
    // コミットしてから 400 を返す（旧挙動を維持）。期限切れでなければ書き込みが
    // ないため tx の drop でロールバックし、以降の検証 I/O の前にはロックを解放する。
    {
        let (_write_guard, tx) = crate::begin_write_tx(&state.db).await?;
        if reject_if_expired(&tx, &order.order_id, &order.expires_at).await? {
            tx.commit().await?;
            return Err(AppError::BadRequest("order has expired".to_string()));
        }
    }

    // 所有証明の記録が無い注文は検証を許可しない（再起動後も維持される）。
    if !is_wallet_bound(&state.db, &order.order_id).await? {
        return Err(AppError::BadRequest(
            "wallet ownership is not bound for this order".to_string(),
        ));
    }

    let verifier = state.payment_verifier.clone().ok_or_else(|| {
        AppError::NotImplemented("chain verification is not configured".to_string())
    })?;
    let verification = verifier.verify(&order).await?;

    // 検証（非同期 I/O）の間に現在時刻が期限を過ぎた、あるいは expires_at が
    // 書き換えられた場合は、読み取った古い status を期待する CAS で FINALIZED へ
    // 進ませない。最新の expires_at を tx 内で読み直して再判定し、期限切れなら
    // EXPIRED 更新をコミットして 400 を返す。期限前ならこの tx をそのまま
    // 不一致判定と CAS 更新へ使う（ロックと BEGIN を再取得しない）。
    let (_write_guard, tx) = crate::begin_write_tx(&state.db).await?;
    let current_expires_at = {
        let mut rows = tx
            .query(
                "SELECT expires_at FROM payment_orders WHERE order_id = ?",
                [order.order_id.as_str()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => row.get::<String>(0)?,
            None => order.expires_at.clone(),
        }
    };
    if reject_if_expired(&tx, &order.order_id, &current_expires_at).await? {
        tx.commit().await?;
        return Err(AppError::BadRequest("order has expired".to_string()));
    }

    // amount_wei は文字列で保存されるため、先頭ゼロなどの表記差で誤判定しないよう
    // 両側を u128 としてパースして数値比較する。パース失敗は Internal（fail-closed）。
    let verified_wei = parse_amount_wei(&verification.amount_wei, "transfer")?;
    let order_wei = parse_amount_wei(&order.amount_wei, "order")?;

    if verification.chain_id != order.chain_id
        || normalize_address(&verification.from) != order.wallet_address
        || normalize_address(&verification.to) != order.recipient_address
        || verified_wei != order_wei
    {
        // 不一致マーキングも CAS 対象。検証中に status が外部書き換えされていた場合は
        // 0 件更新ならロールバックして 400 を返さず、他 CAS と同様に 409 で拒否する。
        let mismatch_update = tx
            .execute(
                "UPDATE payment_orders SET status = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE order_id = ? AND status = ? AND status <> ?",
                (
                    STATUS_MISMATCH,
                    order_id.clone(),
                    order.status.clone(),
                    STATUS_FINALIZED,
                ),
            )
            .await?;
        if tx.changes() != 1 {
            return Err(AppError::Conflict(
                "order status changed during mismatch marking".into(),
            ));
        }
        debug_assert_eq!(mismatch_update, 1);
        tx.commit().await?;
        return Err(AppError::BadRequest(
            "transfer does not match the order".to_string(),
        ));
    }

    let next_status = if verification.confirmations >= required_confirmations(order.chain_id) {
        STATUS_FINALIZED
    } else {
        STATUS_CONFIRMING
    };

    // 検証（非同期 I/O）の間に状態が変わっていないことを CAS で担保する。
    // 期限再判定で取得した書き込みロックと tx を引き続き使い、
    // 同一プロセス内の並行 verify を直列化する。
    let update = tx
        .execute(
            "UPDATE payment_orders
             SET status = ?, tx_hash = ?, log_index = ?, block_number = ?,
                 confirmations = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE order_id = ? AND status = ? AND status <> ? AND status <> 'MISMATCH'",
            (
                STATUS_DETECTED,
                verification.tx_hash.clone(),
                verification.log_index,
                verification.block_number,
                verification.confirmations,
                order_id.clone(),
                order.status.clone(),
                STATUS_FINALIZED,
            ),
        )
        .await;

    if let Err(error) = update {
        // UNIQUE(chain_id, tx_hash, log_index) 違反だけを同一送金の再利用として 400 にする。
        // デッドロック・一時障害・容量不足などの DB エラーを「再利用」と誤報告せず、
        // Internal (500) として伝播させる。詳細は既定のマスク処理に任せる。
        if is_unique_constraint_violation(&error) {
            warn!("payment verification rejected: {}", error);
            return Err(AppError::BadRequest(
                "transaction is already used by another order".to_string(),
            ));
        }
        return Err(AppError::Internal(format!(
            "payment verification update failed: {}",
            error
        )));
    }
    if tx.changes() != 1 {
        return Err(AppError::Conflict(
            "order status changed during verification".to_string(),
        ));
    }

    // 1 段目は読み取った status を CAS 条件に DETECTED へ書き換えるため、
    // CONFIRMING から始まった場合も一度 DETECTED に戻ってから次状態へ進む。
    // 確定済み(FINALIZED)へは 1 段目の `status <> FINALIZED` で既に除外され、
    // 2 段目は DETECTED を期待して next_status を書き込む。
    tx.execute(
        "UPDATE payment_orders
         SET status = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE order_id = ? AND status = ? AND status <> ? AND status <> 'MISMATCH'",
        (
            next_status,
            order_id.clone(),
            STATUS_DETECTED,
            STATUS_FINALIZED,
        ),
    )
    .await?;
    // 2 段目も CAS なので、他ノードが DETECTED を書き換えていた場合は
    // 0 件更新のまま成功応答しないようにする。
    if tx.changes() != 1 {
        return Err(AppError::Conflict(
            "order status changed during finalization".to_string(),
        ));
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;

    Ok(Json(load_order(&state, &order_id, &claims.sub).await?))
}

#[cfg(test)]
mod unique_violation_tests {
    use super::is_unique_constraint_violation;

    const REUSE_COLUMNS: &str = "UNIQUE constraint failed: payment_orders.chain_id, payment_orders.tx_hash, payment_orders.log_index";

    #[test]
    fn local_unique_error_is_detected() {
        let error = libsql::Error::SqliteFailure(2067, REUSE_COLUMNS.to_string());
        assert!(is_unique_constraint_violation(&error));
        // 同じ UNIQUE 違反でも対象インデックスでなければ再利用扱いにしない。
        let other_index = libsql::Error::SqliteFailure(
            2067,
            "UNIQUE constraint failed: payment_orders.derivation_index".to_string(),
        );
        assert!(!is_unique_constraint_violation(&other_index));
        let other = libsql::Error::SqliteFailure(5, "database is locked".to_string());
        assert!(!is_unique_constraint_violation(&other));
    }

    #[test]
    fn remote_unique_error_is_detected_by_code_and_extended_code() {
        let by_extended = libsql::Error::RemoteSqliteFailure(19, 2067, REUSE_COLUMNS.to_string());
        assert!(is_unique_constraint_violation(&by_extended));

        let by_basic = libsql::Error::RemoteSqliteFailure(2067, 0, REUSE_COLUMNS.to_string());
        assert!(is_unique_constraint_violation(&by_basic));

        // メッセージ文字列だけでは UNIQUE 判定しない（文字列フォールバックの廃止）。
        let by_message = libsql::Error::RemoteSqliteFailure(19, 0, REUSE_COLUMNS.to_string());
        assert!(!is_unique_constraint_violation(&by_message));

        let other = libsql::Error::RemoteSqliteFailure(19, 5, "database is locked".to_string());
        assert!(!is_unique_constraint_violation(&other));
    }
}
