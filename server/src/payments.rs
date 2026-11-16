//! JPYC payment orders and settlement verification (ADR-0003).

use crate::{AppError, AppState, AuthClaims, is_safe_cid};
use async_trait::async_trait;
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{Duration, Utc};
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
pub const MAX_ORDER_JPY: u64 = 1_000_000;

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
    pub amount_jpy: u64,
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

/// 0x 形式の 20 バイトアドレスか。チェックサムは別途検証する。
pub fn is_valid_eth_address(address: &str) -> bool {
    let Some(hex) = address.strip_prefix("0x") else {
        return false;
    };
    hex.len() == 40 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn normalize_address(address: &str) -> String {
    address.to_ascii_lowercase()
}

/// 注文IDに対するウォレット所有証明のメッセージ。
pub fn wallet_bind_message(order_id: &str) -> String {
    format!("openIllustration payment order {order_id}")
}

/// 円建て金額を JPYC の最小単位（10^18）へ変換する。
pub fn jpy_to_wei(amount_jpy: u64) -> Result<String, AppError> {
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

pub async fn create_order(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Json(payload): Json<CreateOrderRequest>,
) -> Result<Json<CreateOrderResponse>, AppError> {
    if !is_safe_cid(&payload.cid) {
        return Err(AppError::BadRequest("Invalid CID".to_string()));
    }
    // 提示価格と支払額を照合する。存在しない/レビュー中のコンテンツは注文できない。
    let mut content_rows = state
        .db
        .query(
            "SELECT price, status FROM contents WHERE cid = ?",
            [payload.cid.clone()],
        )
        .await?;
    let (content_price, content_status): (i64, String) = match content_rows.next().await? {
        Some(row) => (row.get(0)?, row.get(1)?),
        None => return Err(AppError::NotFound),
    };
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

    let (recipient_address, derivation_index) = if let Some(xprv) = &state.payment_receiving_xprv {
        let mut rows = state
            .db
            .query(
                "SELECT COALESCE(MAX(derivation_index), -1) + 1 FROM payment_orders",
                (),
            )
            .await?;
        let index: i64 = match rows.next().await? {
            Some(row) => row.get(0)?,
            None => 0,
        };
        (derive_receiving_address(xprv, index as u32)?, Some(index))
    } else if let Some(address) = &state.payment_receiving_address {
        (normalize_address(address), None)
    } else {
        return Err(AppError::NotImplemented(
            "JPYC receiving address is not configured".to_string(),
        ));
    };

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

    state
        .db
        .execute(
            "INSERT INTO payment_orders
                (order_id, ppid, cid, chain_id, wallet_address, recipient_address,
                 amount_wei, status, expires_at, derivation_index)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                order_id.clone(),
                claims.sub.clone(),
                payload.cid.clone(),
                payload.chain_id,
                wallet_address,
                recipient_address.clone(),
                amount_wei.clone(),
                STATUS_PENDING,
                expires_at.clone(),
                derivation_index,
            ),
        )
        .await?;

    Ok(Json(CreateOrderResponse {
        order_id,
        chain_id: payload.chain_id,
        recipient_address,
        amount_wei,
        expires_at,
    }))
}

pub async fn get_order(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Path(order_id): Path<String>,
) -> Result<Json<PaymentOrder>, AppError> {
    Ok(Json(load_order(&state, &order_id, &claims.sub).await?))
}

/// 期限切れ注文を EXPIRED へ更新して拒否する（更新系エンドポイント共通）。
async fn reject_if_expired(state: &AppState, order: &PaymentOrder) -> Result<(), AppError> {
    if order.expires_at <= chrono::Utc::now().to_rfc3339() {
        state
            .db
            .execute(
                "UPDATE payment_orders SET status = ? WHERE order_id = ? AND status NOT IN (?, ?)",
                (
                    STATUS_EXPIRED,
                    order.order_id.as_str(),
                    STATUS_FINALIZED,
                    STATUS_EXPIRED,
                ),
            )
            .await?;
        return Err(AppError::BadRequest("order has expired".to_string()));
    }
    Ok(())
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
    reject_if_expired(state.as_ref(), &order).await?;
    if order.status != STATUS_PENDING {
        return Err(AppError::BadRequest(
            "order is not awaiting wallet binding".to_string(),
        ));
    }

    let message = wallet_bind_message(&order.order_id);
    let recovered = recover_ethereum_address(message.as_bytes(), &payload.signature)?;
    if normalize_address(&recovered) != normalize_address(&order.wallet_address) {
        return Err(AppError::Unauthorized(
            "signature does not match the order wallet".to_string(),
        ));
    }

    Ok(Json(true))
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
    reject_if_expired(state.as_ref(), &order).await?;

    let verifier = state.payment_verifier.clone().ok_or_else(|| {
        AppError::NotImplemented("chain verification is not configured".to_string())
    })?;
    let verification = verifier.verify(&order).await?;

    if verification.chain_id != order.chain_id
        || normalize_address(&verification.from) != order.wallet_address
        || normalize_address(&verification.to) != order.recipient_address
        || verification.amount_wei != order.amount_wei
    {
        state
            .db
            .execute(
                "UPDATE payment_orders SET status = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE order_id = ?",
                (STATUS_MISMATCH, order_id.clone()),
            )
            .await?;
        return Err(AppError::BadRequest(
            "transfer does not match the order".to_string(),
        ));
    }

    let next_status = if verification.confirmations >= required_confirmations(order.chain_id) {
        STATUS_FINALIZED
    } else {
        STATUS_CONFIRMING
    };

    let tx = state
        .db
        .transaction()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction start failed: {}", e)))?;

    let update = tx
        .execute(
            "UPDATE payment_orders
             SET status = ?, tx_hash = ?, log_index = ?, block_number = ?,
                 confirmations = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE order_id = ?",
            (
                STATUS_DETECTED,
                verification.tx_hash.clone(),
                verification.log_index,
                verification.block_number,
                verification.confirmations,
                order_id.clone(),
            ),
        )
        .await;

    if let Err(error) = update {
        // UNIQUE(chain_id, tx_hash, log_index) は同一送金の再利用を拒否する。
        warn!("payment verification rejected: {}", error);
        return Err(AppError::BadRequest(
            "transaction is already used by another order".to_string(),
        ));
    }

    tx.execute(
        "UPDATE payment_orders
         SET status = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE order_id = ?",
        (next_status, order_id.clone()),
    )
    .await?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;

    Ok(Json(load_order(&state, &order_id, &claims.sub).await?))
}
