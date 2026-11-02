// Guild Backend library
mod jpki;
mod migration;
mod network;

use axum::{
    Json, Router,
    body::Body,
    extract::{FromRef, Multipart, Path, State},
    http::{HeaderValue, StatusCode, header},
    middleware::from_fn,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use common::Claims;
use libsql::{Builder, Connection};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tracing::warn;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Connection>,
    pub http_client: Client,
    pub jwt_secret: String,
    /// 開発用: mock_token およびゼロ署名を許可
    pub allow_mock_auth: bool,
    /// IPFS ゲートウェイの URL テンプレート。`{cid}` を CID で置換する。
    /// 空の場合は取得を試みず、常に失敗する。
    pub ipfs_gateways: Vec<String>,
    /// 各 IPFS ゲートウェイへのリクエストタイムアウト。
    pub ipfs_timeout: Duration,
}

#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] libsql::Error),
    #[error("Internal server error: {0}")]
    Internal(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Not found")]
    NotFound,
    #[error("Bad request: {0}")]
    BadRequest(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
    #[error("Not implemented: {0}")]
    NotImplemented(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::Database(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                internal_error_body(format!("DB Error: {}", e)),
            ),
            AppError::Internal(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, internal_error_body(msg))
            }
            AppError::Io(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                internal_error_body(format!("IO Error: {}", e)),
            ),
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            AppError::NotFound => (StatusCode::NOT_FOUND, "Resource not found".to_string()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            AppError::NotImplemented(msg) => (StatusCode::NOT_IMPLEMENTED, msg),
        };

        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

/// 内部エラーの詳細をクライアントへ公開するか。未設定時は一般化メッセージを返す。
/// 開発時は `EXPOSE_INTERNAL_ERRORS=1` で詳細を確認できる。
fn expose_internal_errors() -> bool {
    std::env::var("EXPOSE_INTERNAL_ERRORS")
        .map(|value| value == "1")
        .unwrap_or(false)
}

/// 500系レスポンスのボディを決定する。詳細は常にログへ出し、公開はフラグに従う。
fn internal_error_body(details: String) -> String {
    tracing::error!("Internal error: {}", details);
    if expose_internal_errors() {
        details
    } else {
        "Internal server error".to_string()
    }
}

use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};

// Newtype wrapper to satisfy Orphan Rules for Axum extractor
pub struct AuthClaims(pub Claims);

impl std::ops::Deref for AuthClaims {
    type Target = Claims;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// Axum extractor for JWT validation
impl<S> axum::extract::FromRequestParts<S> for AuthClaims
where
    S: Send + Sync,
    Arc<AppState>: axum::extract::FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let state = Arc::<AppState>::from_ref(state);

        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(AppError::Unauthorized(
                "Missing Authorization header".to_string(),
            ))?;

        if !auth_header.starts_with("Bearer ") {
            return Err(AppError::Unauthorized(
                "Invalid Authorization format".to_string(),
            ));
        }

        let token = &auth_header[7..];

        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(state.jwt_secret.as_ref()),
            &Validation::default(),
        )
        .map_err(|e| AppError::Unauthorized(format!("Invalid token: {}", e)))?;

        Ok(AuthClaims(token_data.claims))
    }
}

#[derive(Serialize, Deserialize)]
pub struct Oid4vpRequest {
    pub nonce: String,
    pub presentation_definition_uri: String,
}

// End of imports

#[derive(Serialize, Deserialize)]
pub struct VerifyRequest {
    pub presentation_token: String,
    pub public_key_der: Option<Vec<u8>>,
    /// VC から取得した生年月日 (YYYY-MM-DD)
    pub birth_date: Option<String>,
    /// ISO 3166-1 alpha-2 国コード
    pub country_code: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct VerifyResponse {
    pub authorized: bool,
    pub user_hash: Option<String>,
    pub token: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ContentMeta {
    pub cid: String,
    pub checksum: String,
    #[serde(rename = "type")]
    pub content_type: String,
    pub status: String,
    pub price: i64,
}

#[derive(Serialize, Deserialize)]
pub struct TransactionResponse {
    pub tx_id: i64,
    pub cid: String,
    pub sender_hash: String,
    pub receiver_hash: String,
    pub amount: i64,
    pub tx_hash: String,
    pub created_at: String,
}

// Governance structures
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub report_id: i64,
    pub target_cid: String,
    pub reason: String,
    pub status: String, // 'Pending', 'Approved', 'Rejected'
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Vote {
    pub report_id: i64,
    pub juror_hash: String,
    pub result: String, // 'Approve', 'Reject'
    pub signature: Vec<u8>,
}

/// `RUST_LOG` でフィルタ可能なログ subscriber を初期化する。未設定時は INFO。
/// タイムスタンプは RFC 3339 (UTC)。
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
        .init();
}

pub async fn run() -> anyhow::Result<()> {
    init_tracing();

    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "file:local.db".to_string());
    let db_token = std::env::var("DATABASE_TOKEN").unwrap_or_else(|_| "".to_string());

    let db = if db_url.starts_with("file:") || !db_url.contains("://") {
        let path = db_url.strip_prefix("file:").unwrap_or(&db_url);
        Builder::new_local(path).build().await?.connect()?
    } else {
        Builder::new_remote(db_url, db_token)
            .build()
            .await?
            .connect()?
    };

    // Run database migrations
    migration::run_migrations(&db).await?;
    // Detect checksum drift in already-applied migrations
    migration::verify_migrations(&db).await?;

    let jwt_secret = std::env::var("JWT_SECRET")
        .map_err(|_| anyhow::anyhow!("JWT_SECRET must be set (at least 32 random bytes)"))?;
    validate_jwt_secret(&jwt_secret).map_err(|e| anyhow::anyhow!(e))?;

    let ipfs_gateways_env = std::env::var("IPFS_GATEWAYS").ok();
    let ipfs_gateways = ipfs_gateways(ipfs_gateways_env.as_deref());
    if ipfs_gateways.is_empty() {
        warn!("IPFS_GATEWAYS is empty; content retrieval will fail");
    } else {
        tracing::info!(
            "IPFS_GATEWAYS: {} gateway(s) configured",
            ipfs_gateways.len()
        );
    }
    let ipfs_timeout_env = std::env::var("IPFS_TIMEOUT_SECS").ok();
    let ipfs_timeout = ipfs_timeout(ipfs_timeout_env.as_deref());

    // TRUSTED_PROXY_CIDRS を起動時に評価する（不正な要素は警告され無視される）。
    let trusted_proxy_ranges = network::trusted_proxy_ranges();
    if std::env::var_os("TRUSTED_PROXY_CIDRS").is_some() {
        tracing::info!(
            "TRUSTED_PROXY_CIDRS: {} valid range(s) enabled",
            trusted_proxy_ranges.len()
        );
    } else {
        tracing::info!("TRUSTED_PROXY_CIDRS: not configured");
    }

    let shared_state = Arc::new(AppState {
        db: Arc::new(db),
        http_client: Client::new(),
        jwt_secret,
        allow_mock_auth: std::env::var("ALLOW_MOCK_AUTH")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false),
        ipfs_gateways,
        ipfs_timeout,
    });

    let app = build_router(shared_state);

    let addr = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 3000));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Guild Backend listening on {}", addr);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

/// SIGINT / SIGTERM を待つ。受信後は `axum::serve` が処理中リクエストの完了を待って停止する。
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        } else {
            // SIGTERM を購読できない場合は ctrl-c のみで停止する。
            std::future::pending::<()>().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("Shutdown signal received; draining in-flight requests");
}

#[derive(Serialize, Deserialize)]
pub struct AuditLogResponse {
    pub log_id: i64,
    pub cid: String,
    pub old_status: Option<String>,
    pub new_status: String,
    pub changed_by: String,
    pub reason: Option<String>,
    pub created_at: String,
    pub entry_hash: String,
}

/// 開発用の既定CORSオリジン。本番では `ALLOWED_ORIGINS` で上書きする。
const DEV_ALLOWED_ORIGINS: [&str; 3] = [
    "http://localhost:8080",
    "http://127.0.0.1:8080",
    "http://[::1]:8080",
];

/// ブラウザの `Origin` は `scheme://host[:port]` 形式か `null` のみ送られる。
/// scheme のない設定値はどのリクエストにも一致しないため、設定ミスとして無視する。
fn is_origin_shape(origin: &str) -> bool {
    origin == "null" || origin.starts_with("http://") || origin.starts_with("https://")
}

/// Parse the `ALLOWED_ORIGINS` environment value (comma-separated) into
/// header values. Falls back to the local development origins when unset;
/// invalid entries are ignored, and an empty result rejects all origins.
/// Trailing slashes are removed because browsers never send them in `Origin`.
pub fn allowed_origins(env_value: Option<&str>) -> Vec<HeaderValue> {
    match env_value {
        Some(value) => value
            .split(',')
            .filter_map(|origin| {
                let origin = origin.trim().trim_end_matches('/');
                if origin.is_empty() {
                    return None;
                }
                if !is_origin_shape(origin) {
                    warn!(
                        "Ignoring ALLOWED_ORIGINS entry without an http(s) scheme: {:?}",
                        origin
                    );
                    return None;
                }
                match origin.parse::<HeaderValue>() {
                    Ok(parsed) => Some(parsed),
                    Err(_) => {
                        warn!("Ignoring invalid ALLOWED_ORIGINS entry: {:?}", origin);
                        None
                    }
                }
            })
            .collect(),
        None => DEV_ALLOWED_ORIGINS
            .iter()
            .map(|origin| origin.parse().expect("valid dev origin"))
            .collect(),
    }
}

/// HS256 の鍵はハッシュ出力（32バイト）以上の長さを必須とする（RFC 7518 §3.2）。
/// 短い鍵は総当たり・辞書攻撃で偽造トークンを作られるため、起動時に拒否する。
pub fn validate_jwt_secret(secret: &str) -> Result<(), String> {
    if secret.len() < 32 {
        Err(format!(
            "JWT_SECRET must be at least 32 bytes for HS256 (got {} bytes)",
            secret.len()
        ))
    } else {
        Ok(())
    }
}

/// 既定の IPFS ゲートウェイ。`{cid}` が CID に置換される。
const DEFAULT_IPFS_GATEWAYS: [&str; 3] = [
    "https://ipfs.io/ipfs/{cid}",
    "https://gateway.pinata.cloud/ipfs/{cid}",
    "https://{cid}.ipfs.dweb.link",
];

const DEFAULT_IPFS_TIMEOUT_SECS: u64 = 15;

/// `IPFS_GATEWAYS`（カンマ区切り）を解析する。
/// 未設定時は既定のゲートウェイを使う。空要素、`{cid}` を含まない値、
/// http(s) 以外の URL テンプレートは設定ミスとして警告し、無視する。
pub fn ipfs_gateways(env_value: Option<&str>) -> Vec<String> {
    match env_value {
        Some(value) => value
            .split(',')
            .map(str::trim)
            .filter(|gateway| !gateway.is_empty())
            .filter(|gateway| {
                if !gateway.contains("{cid}") {
                    warn!(
                        "Ignoring IPFS_GATEWAYS entry without a {{cid}} placeholder: {:?}",
                        gateway
                    );
                    return false;
                }
                // `{cid}` を有効な CIDv1 に置換して URL として検証する。
                let probe = gateway.replace(
                    "{cid}",
                    "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
                );
                match reqwest::Url::parse(&probe) {
                    Ok(url) if matches!(url.scheme(), "http" | "https") => true,
                    _ => {
                        warn!(
                            "Ignoring IPFS_GATEWAYS entry that is not an http(s) URL template: {:?}",
                            gateway
                        );
                        false
                    }
                }
            })
            .map(str::to_string)
            .collect(),
        None => DEFAULT_IPFS_GATEWAYS
            .iter()
            .map(|gateway| (*gateway).to_string())
            .collect(),
    }
}

/// `IPFS_TIMEOUT_SECS` を解析する。未設定・0・不正値・上限超過は既定の 15 秒。
/// 上限は、設定ミスでリクエストが実質無期限にハングするのを防ぐ。
pub fn ipfs_timeout(env_value: Option<&str>) -> Duration {
    const MAX_IPFS_TIMEOUT_SECS: u64 = 300;

    env_value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| (1..=MAX_IPFS_TIMEOUT_SECS).contains(secs))
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(DEFAULT_IPFS_TIMEOUT_SECS))
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let allowed_origins_env = std::env::var("ALLOWED_ORIGINS").ok();
    Router::new()
        .route("/auth/login", post(auth_login))
        .route("/auth/verify", post(auth_verify))
        .route("/content", get(list_contents))
        .route("/content/upload", post(upload_content))
        .route("/transactions", get(list_transactions))
        .route("/mod/report", post(mod_report))
        .route("/mod/reports", get(list_reports))
        .route("/mod/vote", post(mod_vote))
        .route("/content/{cid}", get(get_content))
        .route("/content/{cid}/audit", get(get_content_audit))
        .route("/content/{cid}/support", post(support_creator))
        .route("/health", get(|| async { "ok" }))
        .layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024))
        .layer(
            CorsLayer::new()
                .allow_origin(allowed_origins(allowed_origins_env.as_deref()))
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]),
        )
        .layer(from_fn(network::ipv6_only_middleware))
        .with_state(state)
}

#[derive(Serialize, Deserialize)]
pub struct ReportResponse {
    pub report_id: i64,
    pub target_cid: String,
    pub reason: String,
    pub created_at: String,
    pub status: String,
}

pub async fn list_reports(
    State(state): State<Arc<AppState>>,
    _claims: AuthClaims,
) -> Result<Json<Vec<ReportResponse>>, AppError> {
    let mut rows = state
        .db
        .query(
            "SELECT r.report_id, r.target_cid, r.reason, r.created_at, c.status 
             FROM reports r
             JOIN contents c ON r.target_cid = c.cid
             WHERE c.status = 'Reported'
             ORDER BY r.created_at DESC",
            (),
        )
        .await?;

    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ReportResponse {
            report_id: row.get(0)?,
            target_cid: row.get(1)?,
            reason: row.get(2)?,
            created_at: row.get(3)?,
            status: row.get(4)?,
        });
    }
    Ok(Json(items))
}

pub async fn list_transactions(
    State(state): State<Arc<AppState>>,
    _claims: AuthClaims,
) -> Result<Json<Vec<TransactionResponse>>, AppError> {
    let mut rows = state
        .db
        .query(
            "SELECT tx_id, cid, sender_hash, receiver_hash, amount, tx_hash, created_at 
             FROM transactions ORDER BY created_at DESC LIMIT 100",
            (),
        )
        .await?;

    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(TransactionResponse {
            tx_id: row.get(0)?,
            cid: row.get(1)?,
            sender_hash: row.get(2)?,
            receiver_hash: row.get(3)?,
            amount: row.get(4)?,
            tx_hash: row.get(5)?,
            created_at: row.get(6)?,
        });
    }
    Ok(Json(items))
}

pub async fn get_content_audit(
    State(state): State<Arc<AppState>>,
    _claims: AuthClaims,
    Path(cid): Path<String>,
) -> Result<Json<Vec<AuditLogResponse>>, AppError> {
    let mut rows = state
        .db
        .query(
            "SELECT log_id, cid, old_status, new_status, changed_by, reason, created_at, entry_hash 
             FROM content_audit_log WHERE cid = ? ORDER BY log_id ASC",
            [cid],
        )
        .await?;

    let mut logs = Vec::new();
    while let Some(row) = rows.next().await? {
        logs.push(AuditLogResponse {
            log_id: row.get(0)?,
            cid: row.get(1)?,
            old_status: row.get(2)?,
            new_status: row.get(3)?,
            changed_by: row.get(4)?,
            reason: row.get(5)?,
            created_at: row.get(6)?,
            entry_hash: row.get(7)?,
        });
    }
    Ok(Json(logs))
}
pub async fn auth_login() -> Json<Oid4vpRequest> {
    let nonce = Uuid::new_v4().to_string();
    let presentation_definition_uri =
        "https://guild.example.com/api/presentation-definition".to_string();

    Json(Oid4vpRequest {
        nonce,
        presentation_definition_uri,
    })
}

pub async fn auth_verify(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<VerifyRequest>,
) -> Result<Json<VerifyResponse>, AppError> {
    let birth_date = payload
        .birth_date
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("birth_date is required".to_string()))?;
    let country_code = payload
        .country_code
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("country_code is required".to_string()))?;
    validate_age(birth_date, country_code)?;

    let verified_key = if payload.presentation_token == "mock_token" && state.allow_mock_auth {
        payload.public_key_der.clone()
    } else {
        Some(jpki::verify_presentation(
            &payload.presentation_token,
            payload.public_key_der.clone(),
            state.allow_mock_auth,
        )?)
    };

    let user_hash = if let Some(ref der) = verified_key {
        sha256::digest(&der[..])
    } else {
        sha256::digest(format!("{}:{}", birth_date, country_code).as_bytes())
    };

    ensure_user_not_banned(&state.db, &user_hash).await?;

    if let Some(der) = verified_key {
        state.db.execute(
            "INSERT INTO users (user_hash, public_key_der) VALUES (?, ?) ON CONFLICT(user_hash) DO UPDATE SET public_key_der = excluded.public_key_der",
            (user_hash.clone(), der)
        ).await?;
    } else {
        state
            .db
            .execute(
                "INSERT OR IGNORE INTO users (user_hash) VALUES (?)",
                [user_hash.clone()],
            )
            .await?;
    }

    let expiration = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::hours(24))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = Claims {
        sub: user_hash.clone(),
        exp: expiration,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_ref()),
    )
    .map_err(|e| AppError::Internal(format!("JWT generation failed: {}", e)))?;

    Ok(Json(VerifyResponse {
        authorized: true,
        user_hash: Some(user_hash),
        token: Some(token),
    }))
}

/// ログへ出す URL から userinfo と query を除去する。
/// ゲートウェイ URL に認証情報やトークンが含まれても記録しない。
pub fn redact_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.set_query(None);
            parsed.to_string()
        }
        Err(_) => url.split('?').next().unwrap_or(url).to_string(),
    }
}

/// `reqwest::Error` の詳細には URL が含まれ得るため、分類だけを返す。
fn describe_reqwest_error(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection failed"
    } else if error.is_body() {
        "body error"
    } else if error.is_decode() {
        "decode error"
    } else {
        "request failed"
    }
}

pub async fn get_content(
    State(state): State<Arc<AppState>>,
    _claims: AuthClaims,
    Path(cid): Path<String>,
) -> Result<Response, AppError> {
    let mut rows = state
        .db
        .query(
            "SELECT checksum, type FROM contents WHERE cid = ?",
            [cid.clone()],
        )
        .await?;

    let (expected_checksum, content_kind): (String, String) = if let Some(row) = rows.next().await?
    {
        (row.get(0)?, row.get(1)?)
    } else {
        return Err(AppError::NotFound);
    };

    // 保存済みのコンテンツ種別に応じた MIME を返す。
    let response_content_type = if content_kind == "Novel" {
        "text/markdown; charset=utf-8"
    } else {
        "image/jxl"
    };

    // ゲートウェイは設定順に試す。すべて失敗した場合は 500 を返し、
    // 部分的なデータを完全なコンテンツとして返さない。
    let mut last_error = String::new();

    if state.ipfs_gateways.is_empty() {
        last_error = "no IPFS gateways configured".to_string();
    }

    for gateway_template in &state.ipfs_gateways {
        let gateway_url = gateway_template.replace("{cid}", &cid);
        match state
            .http_client
            .get(&gateway_url)
            .timeout(state.ipfs_timeout)
            .send()
            .await
        {
            Ok(mut response) if response.status().is_success() => {
                const MAX_CONTENT_BYTES: usize = 10 * 1024 * 1024;
                if response
                    .content_length()
                    .is_some_and(|length| length > MAX_CONTENT_BYTES as u64)
                {
                    return Err(AppError::BadRequest(
                        "IPFS content exceeds size limit".to_string(),
                    ));
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|e| AppError::Internal(format!("Failed to read response body: {e}")))?
                {
                    if bytes.len().saturating_add(chunk.len()) > MAX_CONTENT_BYTES {
                        return Err(AppError::BadRequest(
                            "IPFS content exceeds size limit".to_string(),
                        ));
                    }
                    bytes.extend_from_slice(&chunk);
                }

                let actual_checksum = sha256::digest(&bytes[..]);
                if actual_checksum != expected_checksum {
                    return Err(AppError::Unauthorized("Integrity check failed".to_string()));
                }

                return Response::builder()
                    .header(header::CONTENT_TYPE, response_content_type)
                    .header("X-Forensic-Checksum", expected_checksum)
                    .body(Body::from(bytes))
                    .map_err(|e| AppError::Internal(format!("Response build error: {}", e)));
            }
            Ok(response) => {
                last_error = format!("{}: HTTP {}", redact_url(&gateway_url), response.status());
            }
            Err(e) => {
                last_error = format!(
                    "{}: {}",
                    redact_url(&gateway_url),
                    describe_reqwest_error(&e)
                );
            }
        }
    }

    Err(AppError::Internal(format!(
        "All IPFS gateways failed. Last error: {}",
        last_error
    )))
}

pub async fn list_contents(
    State(state): State<Arc<AppState>>,
    _claims: AuthClaims,
) -> Result<Json<Vec<ContentMeta>>, AppError> {
    let mut rows = state
        .db
        .query(
            "SELECT cid, checksum, type, status, price FROM contents WHERE status = 'Approved' ORDER BY created_at DESC LIMIT 50",
            (),
        )
        .await?;

    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ContentMeta {
            cid: row.get(0)?,
            checksum: row.get(1)?,
            content_type: row.get(2)?,
            status: row.get(3)?,
            price: row.get(4)?,
        });
    }
    Ok(Json(items))
}

/// 支援APIは無効化している。模擬支援記録はチェーン送金の検証を欠き支払証明に
/// ならないため、JPYC送金検証の実装まで常に 501 を返す。
/// 認証は維持し、未認証アクセスには従来どおり 401 を返す。
pub async fn support_creator(_claims: AuthClaims) -> Result<Json<String>, AppError> {
    Err(AppError::NotImplemented(
        "Support API is disabled until JPYC settlement is implemented".to_string(),
    ))
}

pub async fn update_content_status(
    tx: &libsql::Transaction,
    cid: &str,
    new_status: &str,
    changed_by: &str,
    reason: &str,
) -> Result<(), AppError> {
    // 1. Get current status and latest log hash for chaining
    let mut rows = tx
        .query("SELECT status FROM contents WHERE cid = ?", [cid])
        .await?;
    let old_status: String = if let Some(row) = rows.next().await? {
        row.get(0)?
    } else {
        return Err(AppError::NotFound);
    };

    let mut log_rows = tx
        .query(
            "SELECT entry_hash FROM content_audit_log ORDER BY log_id DESC LIMIT 1",
            (),
        )
        .await?;
    let previous_log_hash = if let Some(row) = log_rows.next().await? {
        row.get(0)?
    } else {
        "GENESIS_HASH".to_string()
    };

    let entry = common::AuditLogEntry {
        log_id: None,
        cid: cid.to_string(),
        old_status: old_status.clone(),
        new_status: new_status.to_string(),
        changed_by: changed_by.to_string(),
        reason: reason.to_string(),
        previous_log_hash: previous_log_hash.clone(),
    };
    let entry_hash = entry.calculate_hash();

    // 2. Perform updates
    tx.execute(
        "UPDATE contents SET status = ? WHERE cid = ?",
        [new_status, cid],
    )
    .await?;

    tx.execute(
        "INSERT INTO content_audit_log (cid, old_status, new_status, changed_by, reason, previous_log_hash, entry_hash) VALUES (?, ?, ?, ?, ?, ?, ?)",
        (cid, old_status, new_status, changed_by, reason, previous_log_hash, entry_hash),
    ).await?;

    Ok(())
}

pub async fn mod_report(
    State(state): State<Arc<AppState>>,
    _claims: AuthClaims,
    Json(report): Json<Report>,
) -> Result<Json<bool>, AppError> {
    let tx = state
        .db
        .transaction()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction start failed: {}", e)))?;

    // The target must exist and must not already be under review. Duplicate
    // reports while a review is pending would create parallel report rows
    // that all resolve against the same content status.
    let mut rows = tx
        .query(
            "SELECT status FROM contents WHERE cid = ?",
            [report.target_cid.clone()],
        )
        .await?;
    let current_status: String = match rows.next().await? {
        Some(row) => row.get(0)?,
        None => return Err(AppError::NotFound),
    };
    if current_status == "Reported" {
        return Err(AppError::BadRequest(
            "Content is already under review".to_string(),
        ));
    }

    tx.execute(
        "INSERT INTO reports (target_cid, reason) VALUES (?, ?)",
        [report.target_cid.clone(), report.reason.clone()],
    )
    .await
    .map_err(|e| AppError::Internal(format!("DB Insert failed: {}", e)))?;

    // Use forensic helper to update status and log it
    update_content_status(
        &tx,
        &report.target_cid,
        "Reported",
        &_claims.sub, // The reporter's hash
        &format!("User report: {}", report.reason),
    )
    .await?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;

    Ok(Json(true))
}

pub async fn process_report_conclusion(
    tx: &libsql::Transaction,
    report_id: i64,
) -> Result<(), AppError> {
    // 1. Tally votes
    let mut rows = tx
        .query(
            "SELECT vote_result, COUNT(*) FROM votes WHERE report_id = ? GROUP BY vote_result",
            [report_id],
        )
        .await?;

    let mut approve_count = 0;
    let mut reject_count = 0;

    while let Some(row) = rows.next().await? {
        let result: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        if result == "Approve" {
            approve_count = count;
        } else if result == "Reject" {
            reject_count = count;
        }
    }

    // 2. Define threshold (Example: Conclusion after 3 total votes)
    let total_votes = approve_count + reject_count;
    if total_votes >= 3 {
        let final_status = if reject_count > approve_count {
            "Rejected"
        } else {
            "Approved"
        };

        // Get target CID
        let mut report_rows = tx
            .query(
                "SELECT target_cid FROM reports WHERE report_id = ?",
                [report_id],
            )
            .await?;
        if let Some(row) = report_rows.next().await? {
            let cid: String = row.get(0)?;
            update_content_status(
                tx,
                &cid,
                final_status,
                "SYSTEM_GOVERNANCE",
                &format!(
                    "Consensus reached: {} Approve, {} Reject",
                    approve_count, reject_count
                ),
            )
            .await?;
        }
    }

    Ok(())
}

pub async fn mod_vote(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Json(vote): Json<Vote>,
) -> Result<Json<bool>, AppError> {
    if vote.report_id <= 0 || !matches!(vote.result.as_str(), "Approve" | "Reject") {
        return Err(AppError::BadRequest("Invalid vote".to_string()));
    }
    // 0. Security check: Token sub must match juror_hash
    if claims.sub != vote.juror_hash {
        return Err(AppError::Unauthorized("Token sub mismatch".to_string()));
    }

    // 1. Verify Signature (Auditability)
    let message = json!( {
        "report_id": vote.report_id,
        "result": vote.result
    })
    .to_string();

    let mock_sig = state.allow_mock_auth && vote.signature.iter().all(|&b| b == 0);

    if !mock_sig
        && !verify_user_signature(
            &state,
            &vote.juror_hash,
            message.as_bytes(),
            &vote.signature,
        )
        .await?
    {
        return Err(AppError::Unauthorized(
            "Invalid digital signature".to_string(),
        ));
    }

    let tx = state
        .db
        .transaction()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction start failed: {}", e)))?;

    // Report must exist and its target content must still be under review.
    // Votes on concluded reports are rejected so that re-tallying can never
    // flip an already-decided status.
    let mut report_rows = tx
        .query(
            "SELECT c.status FROM reports r
             JOIN contents c ON r.target_cid = c.cid
             WHERE r.report_id = ?",
            [vote.report_id],
        )
        .await?;
    let target_status: String = match report_rows.next().await? {
        Some(row) => row.get(0)?,
        None => return Err(AppError::NotFound),
    };
    if target_status != "Reported" {
        return Err(AppError::BadRequest(
            "Report is already concluded".to_string(),
        ));
    }

    let mut existing_vote = tx
        .query(
            "SELECT 1 FROM votes WHERE report_id = ? AND voter_hash = ? LIMIT 1",
            (vote.report_id, vote.juror_hash.clone()),
        )
        .await?;
    if existing_vote.next().await?.is_some() {
        return Err(AppError::BadRequest("Juror has already voted".to_string()));
    }

    // 2. Insert vote record
    tx.execute(
        "INSERT INTO votes (report_id, voter_hash, vote_result) VALUES (?, ?, ?)",
        [
            vote.report_id.to_string(),
            vote.juror_hash.clone(),
            vote.result,
        ],
    )
    .await
    .map_err(|e| AppError::Internal(format!("DB Insert failed: {}", e)))?;

    // 3. Insert signature for auditability
    tx.execute(
        "INSERT INTO signatures (target_id, signer_hash, signature_blob, algorithm) VALUES (?, ?, ?, ?)",
        (format!("vote:{}", vote.report_id), vote.juror_hash, vote.signature, "JPKI-RSASSA-PKCS1-v1_5")
    ).await
        .map_err(|e| AppError::Internal(format!("Signature Insert failed: {}", e)))?;

    // 4. Trigger consensus processing
    process_report_conclusion(&tx, vote.report_id).await?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;

    Ok(Json(true))
}

use serde_json::json;

pub async fn verify_user_signature(
    state: &AppState,
    user_hash: &str,
    data: &[u8],
    signature: &[u8],
) -> Result<bool, AppError> {
    let mut rows = state
        .db
        .query(
            "SELECT public_key_der FROM users WHERE user_hash = ?",
            [user_hash],
        )
        .await?;
    let der: Vec<u8> = if let Some(row) = rows.next().await? {
        row.get(0)?
    } else {
        return Ok(false);
    };

    match common::verify_rsa_signature(&der, data, signature) {
        Ok(()) => Ok(true),
        Err(e) => {
            // Keep the crypto detail in logs; the client sees a generic 401.
            warn!("Signature verification failed: {}", e);
            Err(AppError::Unauthorized(
                "Invalid digital signature".to_string(),
            ))
        }
    }
}

pub async fn pin_to_ipfs(
    state: &AppState,
    data: Vec<u8>,
    filename: &str,
) -> Result<String, AppError> {
    let pinata_jwt = std::env::var("PINATA_JWT").unwrap_or_else(|_| "mock_jwt".to_string());

    if pinata_jwt == "mock_jwt" {
        let checksum = sha256::digest(&data[..]);
        return Ok(format!("QmMockPinata{}", &checksum[0..10]));
    }

    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(data).file_name(filename.to_string()),
    );

    let res = state
        .http_client
        .post("https://api.pinata.cloud/pinning/pinFileToIPFS")
        .bearer_auth(pinata_jwt)
        .multipart(form)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Pinata request failed: {}", e)))?;

    if !res.status().is_success() {
        return Err(AppError::Internal(format!(
            "Pinata upload failed: {}",
            res.status()
        )));
    }

    let json: serde_json::Value = res
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("Invalid Pinata response: {}", e)))?;

    let cid = json["IpfsHash"]
        .as_str()
        .ok_or_else(|| AppError::Internal("CID not found in Pinata response".to_string()))?;

    Ok(cid.to_string())
}

pub async fn upload_content(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    mut multipart: Multipart,
) -> Result<Json<String>, AppError> {
    let mut data = None;
    let mut content_type = None;
    let mut owner_hash = None;
    let mut signature = None;
    // 価格は `contents.price`（SQLite INTEGER）と `Content::price` と同じ64bit整数で扱う。
    // 型を省略すると i32 にフォールバックし、文書化された整数値を不正として拒否する。
    let mut price: i64 = 0;
    let mut price_seen = false;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {}", e)))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if !matches!(name.as_str(), "file" | "owner_hash" | "signature" | "price") {
            return Err(AppError::BadRequest(format!("Unexpected field: {name}")));
        }
        match name.as_str() {
            "file" => {
                if data.is_some() {
                    return Err(AppError::BadRequest("Duplicate file field".to_string()));
                }
                content_type = field.content_type().map(|s| s.to_string());
                data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::Internal(format!("Failed to read field: {}", e)))?,
                );
            }
            "owner_hash" => {
                if owner_hash.is_some() {
                    return Err(AppError::BadRequest(
                        "Duplicate owner_hash field".to_string(),
                    ));
                }
                owner_hash = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Invalid owner_hash: {}", e)))?,
                );
            }
            "signature" => {
                if signature.is_some() {
                    return Err(AppError::BadRequest(
                        "Duplicate signature field".to_string(),
                    ));
                }
                signature = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Invalid signature: {}", e)))?,
                );
            }
            "price" => {
                if price_seen {
                    return Err(AppError::BadRequest("Duplicate price field".to_string()));
                }
                price_seen = true;
                price = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Invalid price: {e}")))?
                    .parse()
                    .map_err(|_| AppError::BadRequest("price must be an integer".to_string()))?;
                if price < 0 {
                    return Err(AppError::BadRequest(
                        "price must not be negative".to_string(),
                    ));
                }
            }
            _ => {}
        }
    }

    let data = data.ok_or(AppError::BadRequest("Missing file".to_string()))?;
    let owner_hash = owner_hash.ok_or(AppError::BadRequest("Missing owner_hash".to_string()))?;
    let signature = signature.ok_or(AppError::BadRequest("Missing signature".to_string()))?;

    // 0. Security check: Token sub must match owner_hash
    if claims.sub != owner_hash {
        return Err(AppError::Unauthorized("Token sub mismatch".to_string()));
    }

    ensure_user_not_banned(&state.db, &owner_hash).await?;

    let content_kind = validate_upload_content_type(content_type.as_deref())?;
    validate_upload_content(content_kind, &data)?;

    // 1. Verify Signature (dev mock: all-zero signature when ALLOW_MOCK_AUTH)
    let mock_sig = state.allow_mock_auth && signature.iter().all(|&b| b == 0);
    if !mock_sig && !verify_user_signature(&state, &owner_hash, &data, &signature).await? {
        return Err(AppError::Unauthorized(
            "Invalid digital signature".to_string(),
        ));
    }

    let checksum = sha256::digest(&data[..]);

    // 2. Pin to IPFS with a filename that matches the declared kind
    let filename = if content_kind == "Novel" {
        "upload.md"
    } else {
        "upload.jxl"
    };
    let cid = pin_to_ipfs(&state, data.to_vec(), filename).await?;

    let tx = state
        .db
        .transaction()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction start failed: {}", e)))?;

    // The same bytes resolve to the same CID and contents is keyed by CID,
    // so a re-upload would otherwise surface as a primary-key 500.
    let mut existing = tx
        .query("SELECT 1 FROM contents WHERE cid = ?", [cid.clone()])
        .await?;
    if existing.next().await?.is_some() {
        return Err(AppError::BadRequest("Content already exists".to_string()));
    }

    tx.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, price) VALUES (?, ?, ?, ?, ?)",
        (
            cid.clone(),
            content_kind.to_string(),
            owner_hash.clone(),
            checksum,
            price,
        ),
    )
    .await
    .map_err(|e| AppError::Internal(format!("DB Insert failed: {}", e)))?;

    tx.execute(
        "INSERT INTO signatures (target_id, signer_hash, signature_blob, algorithm) VALUES (?, ?, ?, ?)",
        (cid.clone(), owner_hash, signature.to_vec(), "JPKI-RSASSA-PKCS1-v1_5")
    ).await
        .map_err(|e| AppError::Internal(format!("Signature Insert failed: {}", e)))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;

    Ok(Json(cid))
}

pub async fn ensure_user_not_banned(db: &Connection, user_hash: &str) -> Result<(), AppError> {
    let mut rows = db
        .query(
            "SELECT is_banned FROM users WHERE user_hash = ?",
            [user_hash],
        )
        .await?;
    if let Some(row) = rows.next().await? {
        let banned: bool = row.get(0)?;
        if banned {
            return Err(AppError::Forbidden("User is banned".to_string()));
        }
    }
    Ok(())
}

pub fn validate_age(birth_date: &str, country_code: &str) -> Result<(), AppError> {
    let today = chrono::Utc::now().date_naive();
    common::age::is_age_allowed(birth_date, country_code, today).map_err(|e| match e {
        common::age::AgeValidationError::UnderAge {
            country,
            age,
            threshold,
        } => AppError::Forbidden(format!(
            "Age verification failed for {country}: age {age}, required {threshold}"
        )),
        common::age::AgeValidationError::InvalidBirthDate(msg) => {
            AppError::BadRequest(format!("Invalid birth_date: {msg}"))
        }
    })
}

/// INT-02: JPEG XL / Markdown のみ許可
pub fn validate_upload_content_type(content_type: Option<&str>) -> Result<&'static str, AppError> {
    let media_type = content_type
        .and_then(|value| value.split(';').next())
        .unwrap_or("")
        .trim();
    if media_type.eq_ignore_ascii_case("image/jxl") {
        return Ok("Illustration");
    }
    if media_type.eq_ignore_ascii_case("text/markdown") {
        return Ok("Novel");
    }
    Err(AppError::BadRequest(
        "Unsupported content type; use image/jxl or text/markdown".to_string(),
    ))
}

/// Checks that uploaded bytes match the allowed JPEG XL or UTF-8 Markdown format.
pub fn validate_upload_content(content_kind: &str, data: &[u8]) -> Result<(), AppError> {
    match content_kind {
        "Illustration"
            if data.starts_with(&[0xFF, 0x0A]) || data.starts_with(b"\0\0\0\x0CJXL \r\n\x87\n") =>
        {
            Ok(())
        }
        "Illustration" => Err(AppError::BadRequest(
            "Invalid JPEG XL codestream signature".to_string(),
        )),
        "Novel" if std::str::from_utf8(data).is_ok() => Ok(()),
        "Novel" => Err(AppError::BadRequest(
            "Markdown content must be UTF-8".to_string(),
        )),
        _ => Err(AppError::BadRequest("Unsupported content kind".to_string())),
    }
}

#[cfg(test)]
mod fuzz_test;
#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests;
