use axum::{
    Json,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use libsql::Connection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{AppError, AppState, AuthClaims, begin_write_tx};

// ---------------------------------------------------------------------------
// R5-04: データ主体による自身のデータのエクスポート（GET /me/export）
//
// 削除権（/me/erase）は対象外。全クエリで Claims.sub をバインド渡しにし、
// 他人の行が混入しないようにする。セッション / トークン識別子・署名鍵は
// レスポンス型に存在しないため、JSON へ出力されない。
// ---------------------------------------------------------------------------

/// `GET /me/export` のトップレベルレスポンス。
///
/// `revoked_sessions`（sid を含む）は対象外の 12 表を格納する。
/// `sub` は user_hash / ppid と同一値。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataExport {
    /// スキーマバージョン（固定値 1）。
    pub version: u32,
    /// 生成時刻（RFC 3339、UTC）。
    pub generated_at: String,
    /// データ主体の識別子（user_hash = ppid）。
    pub sub: String,
    pub users: Vec<ExportedUser>,
    pub contents: Vec<ExportedContent>,
    pub favorites: Vec<ExportedFavorite>,
    pub transactions: Vec<ExportedTransaction>,
    pub signatures: Vec<ExportedSignature>,
    pub content_audit_log: Vec<ExportedContentAudit>,
    pub reports: Vec<ExportedReport>,
    pub votes: Vec<ExportedVote>,
    pub payment_orders: Vec<ExportedPaymentOrder>,
    pub governance_audit_log: Vec<ExportedGovernanceAudit>,
    /// token_hash / family_id は含めない（sub / 有効期限 / 使用時刻のみ）。
    pub refresh_tokens: Vec<ExportedRefreshToken>,
    pub revoked_subs: Vec<ExportedRevokedSub>,
}

/// users 行のエクスポート表現。公開鍵 DER は秘密鍵ではないため含める。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedUser {
    pub user_hash: String,
    pub public_key_der: Option<Vec<u8>>,
    pub is_banned: Option<bool>,
    pub banned_at: Option<String>,
    /// BAN の期限（SQLite datetime 形式の文字列）。NULL は無期限。
    pub ban_until: Option<String>,
}

/// contents 行のエクスポート表現。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedContent {
    pub cid: String,
    #[serde(rename = "type")]
    pub content_type: Option<String>,
    pub owner_hash: Option<String>,
    pub checksum: String,
    pub status: Option<String>,
    pub price: Option<i64>,
    pub created_at: Option<String>,
    pub title: String,
    pub description: String,
    pub tags: String,
}

/// favorites 行のエクスポート表現。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedFavorite {
    pub id: i64,
    pub user_hash: String,
    pub cid: String,
    pub created_at: Option<String>,
}

/// transactions 行のエクスポート表現。送信者 / 受信者の両方が本人である行を含む。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedTransaction {
    pub tx_id: i64,
    pub cid: Option<String>,
    pub sender_hash: Option<String>,
    pub receiver_hash: Option<String>,
    pub amount: i64,
    pub tx_hash: String,
    pub created_at: Option<String>,
}

/// signatures 行のエクスポート表現。署名データ自体は公開情報であり、
/// 署名鍵（秘密鍵）はどこにも保存していない。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedSignature {
    pub signature_id: i64,
    pub target_id: String,
    pub signer_hash: String,
    pub signature_blob: Vec<u8>,
    pub algorithm: String,
    pub created_at: Option<String>,
}

/// content_audit_log 行のエクスポート表現。`reason` は既定でマスクする。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedContentAudit {
    pub log_id: i64,
    pub cid: Option<String>,
    pub old_status: Option<String>,
    pub new_status: Option<String>,
    pub changed_by: Option<String>,
    pub reason: Option<String>,
    pub previous_log_hash: Option<String>,
    pub entry_hash: String,
    pub created_at: Option<String>,
}

/// reports 行のエクスポート表現。`reason` は既定でマスクする。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedReport {
    pub report_id: i64,
    pub target_cid: Option<String>,
    pub reason: Option<String>,
    pub created_at: Option<String>,
    pub deadline_at: Option<String>,
    pub concluded_at: Option<String>,
    pub appeal_of: Option<i64>,
}

/// votes 行のエクスポート表現。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedVote {
    pub vote_id: i64,
    pub report_id: Option<i64>,
    pub voter_hash: Option<String>,
    pub vote_result: Option<String>,
    pub voted_at: Option<String>,
}

/// payment_orders 行のエクスポート表現。xprv / JWT 等の秘密情報は保持しない。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedPaymentOrder {
    pub order_id: String,
    pub ppid: String,
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
    pub updated_at: String,
    pub cid: Option<String>,
    pub wallet_bound: i64,
    pub derivation_index: Option<i64>,
}

/// governance_audit_log 行のエクスポート表現。
/// `reason` / `details` は既定でマスクする。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedGovernanceAudit {
    pub log_id: i64,
    pub action: String,
    pub target: String,
    pub actor_hash: String,
    pub reason: String,
    pub details: Option<String>,
    pub created_at: Option<String>,
}

/// refresh_tokens 行のエクスポート表現。
/// token_hash（主キー）と family_id は意図的に除外する。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedRefreshToken {
    pub sub: String,
    pub expires_at: String,
    pub used_at: Option<String>,
    pub created_at: Option<String>,
}

/// revoked_subs 行のエクスポート表現。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedRevokedSub {
    pub sub: String,
    pub revoked_at: i64,
    pub updated_at: Option<String>,
}

/// `reason` / `details` を公開用にマスクする（空文字はそのまま空文字）。
fn mask_export_text(value: Option<String>) -> Option<String> {
    value.map(|text| common::mask_reason(&text, false))
}

/// 自分の users 行を読み出す。
async fn export_users(db: &Connection, sub: &str) -> Result<Vec<ExportedUser>, AppError> {
    let mut rows = db
        .query(
            "SELECT user_hash, public_key_der, is_banned, banned_at, ban_until
             FROM users WHERE user_hash = ?",
            libsql::params![sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedUser {
            user_hash: row.get(0)?,
            public_key_der: row.get(1)?,
            is_banned: row.get(2)?,
            banned_at: row.get(3)?,
            ban_until: row.get(4)?,
        });
    }
    Ok(items)
}

/// 自分の contents 行を読み出す。
async fn export_contents(db: &Connection, sub: &str) -> Result<Vec<ExportedContent>, AppError> {
    let mut rows = db
        .query(
            "SELECT cid, \"type\", owner_hash, checksum, status, price, created_at,
                    title, description, tags
             FROM contents WHERE owner_hash = ? ORDER BY created_at, cid",
            libsql::params![sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedContent {
            cid: row.get(0)?,
            content_type: row.get(1)?,
            owner_hash: row.get(2)?,
            checksum: row.get(3)?,
            status: row.get(4)?,
            price: row.get(5)?,
            created_at: row.get(6)?,
            title: row.get(7)?,
            description: row.get(8)?,
            tags: row.get(9)?,
        });
    }
    Ok(items)
}

/// 自分の favorites 行を読み出す。
async fn export_favorites(db: &Connection, sub: &str) -> Result<Vec<ExportedFavorite>, AppError> {
    let mut rows = db
        .query(
            "SELECT id, user_hash, cid, created_at
             FROM favorites WHERE user_hash = ? ORDER BY id",
            libsql::params![sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedFavorite {
            id: row.get(0)?,
            user_hash: row.get(1)?,
            cid: row.get(2)?,
            created_at: row.get(3)?,
        });
    }
    Ok(items)
}

/// 自分が送信者または受信者である transactions 行を読み出す。
async fn export_transactions(
    db: &Connection,
    sub: &str,
) -> Result<Vec<ExportedTransaction>, AppError> {
    let mut rows = db
        .query(
            "SELECT tx_id, cid, sender_hash, receiver_hash, amount, tx_hash, created_at
             FROM transactions WHERE sender_hash = ? OR receiver_hash = ? ORDER BY tx_id",
            libsql::params![sub.to_string(), sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedTransaction {
            tx_id: row.get(0)?,
            cid: row.get(1)?,
            sender_hash: row.get(2)?,
            receiver_hash: row.get(3)?,
            amount: row.get(4)?,
            tx_hash: row.get(5)?,
            created_at: row.get(6)?,
        });
    }
    Ok(items)
}

/// 自分が署名した signatures 行を読み出す（他人の署名は含めない）。
async fn export_signatures(db: &Connection, sub: &str) -> Result<Vec<ExportedSignature>, AppError> {
    let mut rows = db
        .query(
            "SELECT signature_id, target_id, signer_hash, signature_blob, algorithm, created_at
             FROM signatures WHERE signer_hash = ? ORDER BY signature_id",
            libsql::params![sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedSignature {
            signature_id: row.get(0)?,
            target_id: row.get(1)?,
            signer_hash: row.get(2)?,
            signature_blob: row.get(3)?,
            algorithm: row.get(4)?,
            created_at: row.get(5)?,
        });
    }
    Ok(items)
}

/// 自分が変更した、または自分のコンテンツに対する content_audit_log 行を読み出す。
async fn export_content_audit(
    db: &Connection,
    sub: &str,
) -> Result<Vec<ExportedContentAudit>, AppError> {
    let mut rows = db
        .query(
            "SELECT log_id, cid, old_status, new_status, changed_by, reason,
                    previous_log_hash, entry_hash, created_at
             FROM content_audit_log
             WHERE changed_by = ?
                OR cid IN (SELECT cid FROM contents WHERE owner_hash = ?)
             ORDER BY log_id",
            libsql::params![sub.to_string(), sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedContentAudit {
            log_id: row.get(0)?,
            cid: row.get(1)?,
            old_status: row.get(2)?,
            new_status: row.get(3)?,
            changed_by: row.get(4)?,
            reason: mask_export_text(row.get(5)?),
            previous_log_hash: row.get(6)?,
            entry_hash: row.get(7)?,
            created_at: row.get(8)?,
        });
    }
    Ok(items)
}

/// 自分のコンテンツに対する reports 行を読み出す。
/// reports 自体に user_hash は無いため、target_cid 経由で絞り込む。
async fn export_reports(db: &Connection, sub: &str) -> Result<Vec<ExportedReport>, AppError> {
    let mut rows = db
        .query(
            "SELECT r.report_id, r.target_cid, r.reason, r.created_at,
                    r.deadline_at, r.concluded_at, r.appeal_of
             FROM reports r
             JOIN contents c ON c.cid = r.target_cid
             WHERE c.owner_hash = ? ORDER BY r.report_id",
            libsql::params![sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedReport {
            report_id: row.get(0)?,
            target_cid: row.get(1)?,
            reason: mask_export_text(row.get(2)?),
            created_at: row.get(3)?,
            deadline_at: row.get(4)?,
            concluded_at: row.get(5)?,
            appeal_of: row.get(6)?,
        });
    }
    Ok(items)
}

/// 自分が投票した votes 行を読み出す。
async fn export_votes(db: &Connection, sub: &str) -> Result<Vec<ExportedVote>, AppError> {
    let mut rows = db
        .query(
            "SELECT vote_id, report_id, voter_hash, vote_result, voted_at
             FROM votes WHERE voter_hash = ? ORDER BY vote_id",
            libsql::params![sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedVote {
            vote_id: row.get(0)?,
            report_id: row.get(1)?,
            voter_hash: row.get(2)?,
            vote_result: row.get(3)?,
            voted_at: row.get(4)?,
        });
    }
    Ok(items)
}

/// 自分の payment_orders 行を読み出す。
async fn export_payment_orders(
    db: &Connection,
    sub: &str,
) -> Result<Vec<ExportedPaymentOrder>, AppError> {
    let mut rows = db
        .query(
            "SELECT order_id, ppid, chain_id, wallet_address, recipient_address, amount_wei,
                    status, tx_hash, log_index, block_number, confirmations, expires_at,
                    created_at, updated_at, cid, wallet_bound, derivation_index
             FROM payment_orders WHERE ppid = ? ORDER BY created_at, order_id",
            libsql::params![sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedPaymentOrder {
            order_id: row.get(0)?,
            ppid: row.get(1)?,
            chain_id: row.get(2)?,
            wallet_address: row.get(3)?,
            recipient_address: row.get(4)?,
            amount_wei: row.get(5)?,
            status: row.get(6)?,
            tx_hash: row.get(7)?,
            log_index: row.get(8)?,
            block_number: row.get(9)?,
            confirmations: row.get(10)?,
            expires_at: row.get(11)?,
            created_at: row.get(12)?,
            updated_at: row.get(13)?,
            cid: row.get(14)?,
            wallet_bound: row.get(15)?,
            derivation_index: row.get(16)?,
        });
    }
    Ok(items)
}

/// 自分の governance_audit_log 行を読み出す（対象または操作者）。
async fn export_governance_audit(
    db: &Connection,
    sub: &str,
) -> Result<Vec<ExportedGovernanceAudit>, AppError> {
    let mut rows = db
        .query(
            "SELECT log_id, action, target, actor_hash, reason, details, created_at
             FROM governance_audit_log
             WHERE target = ? OR actor_hash = ? ORDER BY log_id",
            libsql::params![sub.to_string(), sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedGovernanceAudit {
            log_id: row.get(0)?,
            action: row.get(1)?,
            target: row.get(2)?,
            actor_hash: row.get(3)?,
            reason: common::mask_reason(&row.get::<String>(4)?, false),
            details: mask_export_text(row.get(5)?),
            created_at: row.get(6)?,
        });
    }
    Ok(items)
}

/// 自分の refresh_tokens 行を読み出す（token_hash / family_id は返さない）。
async fn export_refresh_tokens(
    db: &Connection,
    sub: &str,
) -> Result<Vec<ExportedRefreshToken>, AppError> {
    let mut rows = db
        .query(
            "SELECT sub, expires_at, used_at, created_at
             FROM refresh_tokens WHERE sub = ? ORDER BY created_at, expires_at",
            libsql::params![sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedRefreshToken {
            sub: row.get(0)?,
            expires_at: row.get(1)?,
            used_at: row.get(2)?,
            created_at: row.get(3)?,
        });
    }
    Ok(items)
}

/// 自分の revoked_subs 行を読み出す。
async fn export_revoked_subs(
    db: &Connection,
    sub: &str,
) -> Result<Vec<ExportedRevokedSub>, AppError> {
    let mut rows = db
        .query(
            "SELECT sub, revoked_at, updated_at FROM revoked_subs WHERE sub = ?",
            libsql::params![sub.to_string()],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ExportedRevokedSub {
            sub: row.get(0)?,
            revoked_at: row.get(1)?,
            updated_at: row.get(2)?,
        });
    }
    Ok(items)
}

/// 12 表を単一のスナップショットで読み切る。
///
/// `db` には読み取りトランザクション内の接続（`BEGIN DEFERRED` 開始後）を渡す。
/// 全クエリが同じトランザクションに所属することをデバッグビルドで検証する
/// （`cargo test` では `debug_assert!` が有効）。
async fn collect_export_snapshot(
    db: &Connection,
    sub: &str,
    generated_at: String,
) -> Result<DataExport, AppError> {
    debug_assert!(
        !db.is_autocommit(),
        "export queries must run inside a read transaction"
    );
    Ok(DataExport {
        version: 1,
        generated_at,
        sub: sub.to_string(),
        users: export_users(db, sub).await?,
        contents: export_contents(db, sub).await?,
        favorites: export_favorites(db, sub).await?,
        transactions: export_transactions(db, sub).await?,
        signatures: export_signatures(db, sub).await?,
        content_audit_log: export_content_audit(db, sub).await?,
        reports: export_reports(db, sub).await?,
        votes: export_votes(db, sub).await?,
        payment_orders: export_payment_orders(db, sub).await?,
        governance_audit_log: export_governance_audit(db, sub).await?,
        refresh_tokens: export_refresh_tokens(db, sub).await?,
        revoked_subs: export_revoked_subs(db, sub).await?,
    })
}

/// R5-04: `GET /me/export`。認証済みのデータ主体が自身の全データを JSON で取得する。
///
/// レスポンス本文はログへ出力しない。ダウンロード用途のため
/// `Content-Disposition: attachment` と `Cache-Control: no-store` を付与する。
pub async fn me_export(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
) -> Result<Response, AppError> {
    let sub = claims.sub.as_str();

    // 12 表の読み取りを単一のトランザクション（libsql の
    // `Connection::transaction()` = BEGIN DEFERRED）で包む。読み取り専用でも
    // 共有 Connection は同時に 1 つの BEGIN しか保持できないため、
    // 書き込みと同じ write_lock を取得して BEGIN を直列化する。
    // これによりアップロード等の書き込みトランザクションと並行しても
    // "cannot start a transaction within a transaction" による 500 にならず、
    // 書き込み完了を待ってからエクスポートが開始される。
    // ガードは tx より後にドロップされるため、コミット／ロールバック完了まで
    // ロックが保持される。
    let (_write_guard, tx) = begin_write_tx(&state.db).await?;

    // トランザクション開始直後の時刻をメタデータとし、各テーブル読取時刻とのずれを防ぐ。
    let generated_at = chrono::Utc::now().to_rfc3339();
    let export = collect_export_snapshot(&tx, sub, generated_at).await?;

    // 読み取りのみだが明示的にコミットしてトランザクションを終了する。
    // 途中のクエリがエラーを返して `?` で早期 return した場合、
    // libsql::Transaction の Drop が内側の local::Transaction
    // （DropBehavior::Rollback）を破棄し、ROLLBACK が実行される。
    // tx の終了後に _write_guard がドロップされ write_lock が解放されるため、
    // 中途半端なトランザクションや共有ロックは残らない。
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;

    let mut response = Json(export).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"openillustration-export.json\""),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}
