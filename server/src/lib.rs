// Guild Backend library
mod jpki;
mod jwt;
mod migration;
mod network;
mod oidc;
mod payments;

use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, FromRef, Multipart, Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    middleware::from_fn,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use common::Claims;
use libsql::{Builder, Connection};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tracing::warn;
use uuid::Uuid;

/// `/metrics/client-events` が受け付けるリクエストボディの上限（1 KiB）。
const MAX_CLIENT_EVENT_BODY_BYTES: usize = 1024;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Connection>,
    pub http_client: Client,
    /// JWT の署名・検証鍵。起動時に確定し、ヘッダからは選ばせない。
    pub jwt_keys: Arc<jwt::JwtKeys>,
    /// 開発用: mock_token およびゼロ署名を許可
    pub allow_mock_auth: bool,
    /// IPFS ゲートウェイの URL テンプレート。`{cid}` を CID で置換する。
    /// 空の場合は取得を試みず、常に失敗する。
    pub ipfs_gateways: Vec<String>,
    /// 各 IPFS ゲートウェイへのリクエストタイムアウト。
    pub ipfs_timeout: Duration,
    /// ネットワークエラー時に各ゲートウェイを再試行する最大回数（1〜5）。
    pub ipfs_max_attempts: usize,
    /// JPYC 注文の受取アドレス。未設定時は注文作成を 501 で拒否する。
    pub payment_receiving_address: Option<String>,
    /// BIP44 派生用の xprv。設定時は注文ごとの受取アドレスを派生する。
    pub payment_receiving_xprv: Option<String>,
    /// チェーン上の送金を検証する境界。未設定時は検証 API を 501 で拒否する。
    pub payment_verifier: Option<Arc<dyn payments::PaymentVerifier>>,
    /// OIDC/JPKI 認証の設定。未設定時は OIDC エンドポイントを 501 で拒否する。
    pub oidc: Option<Arc<oidc::OidcState>>,
    /// 失効したセッションID。ログアウト済みの JWT を即時拒否する。
    pub revoked_sessions: Arc<Mutex<HashSet<String>>>,
    /// sub 単位の失効閾値（Unix秒）。`iat <= threshold` のトークンを拒否する。
    /// back-channel logout 等で「その時点以前の全セッション」を失効させる。
    pub revoked_subs_before: Arc<Mutex<HashMap<String, usize>>>,
    /// DEC-02: 年齢判定の有効ポリシー（非秘密。公開APIで常時参照可能にする）。
    pub age_policy: common::age::AgePolicy,
    /// ADR-0004: このデプロイの運用者（JPKI 由来の user_hash）。
    /// セルフホストごとに異なるため環境変数で設定し、空なら特権操作を拒否する。
    pub moderation_operators: Arc<HashSet<String>>,
    /// グレースフル停止中フラグ。readiness が 503 を返すために使う。
    pub shutting_down: Arc<std::sync::atomic::AtomicBool>,
    /// HTTP カウンタ。テストから独立したインスタンスを注入できる。
    pub metrics: Arc<Metrics>,
    /// レート制限状態。未設定時は制限を適用しない（開発・テスト用）。
    pub rate_limit: Option<Arc<RateLimitState>>,
}

/// `Content-Length` が上限を超えるリクエストを拒否する。
/// axum の Multipart extractor はストリームを先に解析し 400 を返すため、
/// その前にヘッダーで拒否して適切なステータスを返す（多層防御の1段目）。
/// `/metrics/client-events` は 1 KiB 上限で、他エンドポイントより厳しい 400 を返す。
async fn reject_oversized_body(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;
    let is_client_events = request.uri().path() == "/metrics/client-events";
    let max = if is_client_events {
        MAX_CLIENT_EVENT_BODY_BYTES as u64
    } else {
        MAX_BODY_BYTES
    };
    if let Some(length) = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        && length > max
    {
        return if is_client_events {
            // 入力を一切エコーしないため、エラーでも本文を返さない。
            StatusCode::BAD_REQUEST.into_response()
        } else {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({"error": "request body exceeds 10 MiB"})),
            )
                .into_response()
        };
    }
    next.run(request).await
}

/// IP 単位のレート制限（GCRA）と同時実行上限の状態。
/// 開発用モック認証時は `None` とし、テストからは `Some` を注入できる。
pub struct RateLimitState {
    pub limiter: governor::RateLimiter<
        String,
        governor::state::keyed::DefaultKeyedStateStore<String>,
        governor::clock::DefaultClock,
    >,
    pub concurrency: Arc<tokio::sync::Semaphore>,
}

/// `RATE_LIMIT_PER_MIN` を解析する。未設定・不正値・範囲外は既定の 60/分。
pub fn rate_limit_per_min(env_value: Option<&str>) -> u32 {
    const DEFAULT: u32 = 60;
    env_value
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|n| (1..=600).contains(n))
        .unwrap_or(DEFAULT)
}

/// `MAX_CONCURRENT_REQUESTS` を解析する。未設定・不正値・範囲外は既定の 16。
pub fn max_concurrent_requests(env_value: Option<&str>) -> usize {
    const DEFAULT: usize = 16;
    env_value
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|n| (1..=256).contains(n))
        .unwrap_or(DEFAULT)
}

/// 軽量メトリクス。テストから注入できるよう `AppState` に保持する。
#[derive(Default)]
pub struct Metrics {
    pub requests: std::sync::atomic::AtomicU64,
    pub errors: std::sync::atomic::AtomicU64,
    /// クライアントが中立化した危険 URL の件数。URL 本文自体は保持しない。
    pub neutralized_urls: std::sync::atomic::AtomicU64,
}

/// レート制限キーを決定する。
///
/// `network::ipv6_only_middleware`（本ミドルウェアより外側）が
/// `X-Forwarded-For` を解決して extensions に格納したクライアント IP を優先する。
/// 未設定時は接続元ピアを使う（network が動かない経路のフォールバック）。
fn rate_limit_key(peer: std::net::IpAddr, request: &axum::extract::Request) -> String {
    request
        .extensions()
        .get::<std::net::IpAddr>()
        .copied()
        .unwrap_or(peer)
        .to_string()
}

/// IP 単位のレート制限と同時実行上限を適用するミドルウェア。
/// 監視用の /health, /ready, /metrics は除外する。
async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let Some(limit) = state.rate_limit.as_ref() else {
        return next.run(request).await;
    };
    // CORS プリフライトはレートを消費しない（CorsLayer はより外側で短絡する）。
    if request.method() == axum::http::Method::OPTIONS {
        return next.run(request).await;
    }
    let path = request.uri().path();
    if matches!(path, "/health" | "/ready" | "/metrics") {
        return next.run(request).await;
    }
    if limit
        .limiter
        .check_key(&rate_limit_key(peer.ip(), &request))
        .is_err()
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate limit exceeded" })),
        )
            .into_response();
    }
    let Ok(permit) = limit.concurrency.clone().try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "too many concurrent requests" })),
        )
            .into_response();
    };
    let response = next.run(request).await;
    drop(permit);
    response
}

/// 全レスポンスへ最小限のセキュリティヘッダを付与する。
/// MIME スニッフィング防止（nosniff）と、ブラウザがユーザー投稿を
/// フレーム内へ埋め込むことを防ぐ（frame-ancestors 'none'）相当の
/// X-Frame-Options。CSP はクライアントのリソース読み込みに影響するため
/// 本番構成確定時に別途設計する。
async fn security_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if !headers.contains_key("X-Content-Type-Options") {
        headers.insert(
            "X-Content-Type-Options",
            HeaderValue::from_static("nosniff"),
        );
    }
    if !headers.contains_key("X-Frame-Options") {
        headers.insert("X-Frame-Options", HeaderValue::from_static("DENY"));
    }
    response
}

/// リクエスト数と 4xx/5xx 応答数を記録するミドルウェア。
/// Router の最も外側に積むことで、CorsLayer が短絡する OPTIONS 204 や
/// 内側のミドルウェアが返す 429 / 403 / 413 も取りこぼさず計上する。
async fn observe_metrics(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    state
        .metrics
        .requests
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let response = next.run(request).await;
    if response.status().is_client_error() || response.status().is_server_error() {
        state
            .metrics
            .errors
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    response
}

/// Prometheus 形式の最小化されたメトリクス（外部クレートなし）。
pub async fn metrics(State(state): State<Arc<AppState>>) -> Response {
    let body = format!(
        "# HELP openillustration_http_requests_total Total HTTP requests.\n\
         # TYPE openillustration_http_requests_total counter\n\
         openillustration_http_requests_total {}\n\
         # HELP openillustration_http_errors_total Total 4xx/5xx HTTP responses.\n\
         # TYPE openillustration_http_errors_total counter\n\
         openillustration_http_errors_total {}\n\
         # HELP openillustration_neutralized_urls_total Total neutralized dangerous URLs reported by clients.\n\
         # TYPE openillustration_neutralized_urls_total counter\n\
         openillustration_neutralized_urls_total {}\n",
        state
            .metrics
            .requests
            .load(std::sync::atomic::Ordering::Relaxed),
        state
            .metrics
            .errors
            .load(std::sync::atomic::Ordering::Relaxed),
        state
            .metrics
            .neutralized_urls
            .load(std::sync::atomic::Ordering::Relaxed),
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
        .into_response()
}

/// `POST /metrics/client-events` のペイロード。
///
/// URL 本文や利用者識別子は受け取らず、`event` と `count` だけを扱う。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientEventPayload {
    event: String,
    #[serde(default = "default_client_event_count")]
    count: u32,
}

fn default_client_event_count() -> u32 {
    1
}

/// クライアントが中立化した危険 URL の件数を集計する。
///
/// 匿名リクエストを受け付け、URL 本文・IP・User-Agent を記録もログ出力もしない。
/// 成功時は本文なしの 204、不正入力時は本文なしの 400 を返す。
pub async fn client_events(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
) -> Response {
    // DefaultBodyLimit(10 MiB) に依存せず、このエンドポイント専用の上限で読む。
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_CLIENT_EVENT_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let payload: ClientEventPayload = match serde_json::from_slice(&bytes) {
        Ok(payload) => payload,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    // 新しい event 種別を増やすときはこのホワイトリストだけを拡張する。
    if payload.event != "neutralized_url" || !(1..=1000).contains(&payload.count) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    state.metrics.neutralized_urls.fetch_add(
        u64::from(payload.count),
        std::sync::atomic::Ordering::Relaxed,
    );
    StatusCode::NO_CONTENT.into_response()
}

/// libSQL のローカル接続は同時に 1 つの `BEGIN` しか保持できない。
/// Arc<Connection> の実体アドレスをキーにしたプロセス内ロックで書き込みを直列化する。
static DB_WRITE_LOCKS: std::sync::LazyLock<
    std::sync::Mutex<HashMap<usize, Arc<tokio::sync::Mutex<()>>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// 指定接続に対応する書き込みロックを返す。
pub fn write_lock(db: &Connection) -> Arc<tokio::sync::Mutex<()>> {
    let key = db as *const Connection as usize;
    let mut locks = DB_WRITE_LOCKS.lock().expect("write lock table poisoned");
    Arc::clone(
        locks
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
    )
}

/// ロックを保持したままトランザクションを開始する。
/// ガードは戻り値に保持させ、コミット後にドロップする。
async fn begin_write_tx(
    db: &Connection,
) -> Result<(tokio::sync::OwnedMutexGuard<()>, libsql::Transaction), AppError> {
    let guard = write_lock(db).lock_owned().await;
    let tx = db
        .transaction()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction start failed: {}", e)))?;
    Ok((guard, tx))
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
    #[error("Conflict: {0}")]
    Conflict(String),
    #[error("Too many requests: {0}")]
    TooManyRequests(String),
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
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            AppError::TooManyRequests(msg) => (StatusCode::TOO_MANY_REQUESTS, msg),
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

use jsonwebtoken::encode;

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

        // 許可アルゴリズムは起動時に確定した 1 つだけ。トークンの
        // ヘッダにある `alg` を見て検証方式を切り替えない。
        let token_data = state
            .jwt_keys
            .decode::<Claims>(token)
            .map_err(|e| AppError::Unauthorized(format!("Invalid token: {}", e)))?;

        if let Some(sid) = &token_data.claims.sid {
            let revoked = state
                .revoked_sessions
                .lock()
                .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?;
            if revoked.contains(sid) {
                return Err(AppError::Unauthorized("Session revoked".to_string()));
            }
        }

        let revoked_subs = state
            .revoked_subs_before
            .lock()
            .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?;
        if let Some(&threshold) = revoked_subs.get(&token_data.claims.sub) {
            let issued_at = token_data.claims.iat.unwrap_or(0);
            if issued_at <= threshold {
                return Err(AppError::Unauthorized("Session revoked".to_string()));
            }
        }

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
    /// 開発用モック認証で `/auth/login` が発行した nonce。単回消費。
    #[serde(default)]
    pub nonce: Option<String>,
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
    /// 既存行は migration 011 の既定値により空文字列になる。
    pub title: String,
    pub description: String,
    /// DB ではカンマ区切りの TEXT。API では文字列配列として返す。
    pub tags: Vec<String>,
    /// 一覧取得時に、現在のユーザーがお気に入り登録しているかを設定する。
    /// 旧 JSON にこの項目が無い場合は serde の既定値 false として読む。
    #[serde(default)]
    pub is_favorite: bool,
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
    let use_json = std::env::var("LOG_FORMAT")
        .map(|value| value.eq_ignore_ascii_case("json"))
        .unwrap_or(false);
    if use_json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
            .with_current_span(false)
            .with_span_list(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
            .init();
    }
}

/// ロックファイルの内容から `(PID, プロセス開始時刻)` を取り出す純粋関数。
///
/// - 新形式: 1行目が PID、2行目が `/proc/<pid>/stat` の field 22 (starttime)。
/// - 旧形式: PID のみ（`starttime` は `None`）。後方互換として扱う。
/// - PID が読めない場合: `(0, None)` を返す。PID 0 は通常のプロセスに割り当てられないため、
///   呼び出し側は stale ロックとして回収できる。
pub(crate) fn parse_instance_lock(contents: &str) -> (u32, Option<u64>) {
    let mut lines = contents.lines();
    let pid = lines
        .next()
        .and_then(|line| line.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let starttime = lines
        .next()
        .and_then(|line| line.trim().parse::<u64>().ok());
    (pid, starttime)
}

/// `/proc/<pid>/stat` の field 22 (starttime: 起動からの clock ticks) を読む純粋関数。
/// `/proc` が無い環境・プロセスが消えた場合・形式が読めない場合は `None`。
pub(crate) fn read_proc_starttime(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm (field 2) は空白や ')' を含み得るため、最後の ')' より後だけを解析する。
    // ')' の直後は field 3 (state) なので field 22 はそこから 19 個先のトークン。
    let after_comm = stat.rfind(')')?;
    stat.get(after_comm + 1..)?
        .split_whitespace()
        .nth(19)?
        .parse::<u64>()
        .ok()
}

/// ロック所有者が生存中かを判定する。
/// `/proc` が読めない環境では判定不能のため「生存」として起動を拒否する（fail-closed）。
fn instance_lock_owner_alive(pid: u32, starttime: Option<u64>) -> bool {
    if read_proc_starttime(std::process::id()).is_none() {
        return true;
    }
    match starttime {
        // PID 再利用を防ぐため、開始時刻が一致したときだけ生存とみなす。
        // /proc/<pid> はあるのに stat が読めない場合は確認不能なので拒否側へ倒す。
        Some(expected) => match read_proc_starttime(pid) {
            Some(actual) => actual == expected,
            None => std::path::Path::new(&format!("/proc/{pid}")).exists(),
        },
        // 旧形式（PID のみ）は後方互換のため PID の存在だけで判定する。
        None => std::path::Path::new(&format!("/proc/{pid}")).exists(),
    }
}

/// PID と starttime を1回の書き込みでロックファイルへ記録する。
fn write_instance_lock(file: &mut std::fs::File) -> anyhow::Result<()> {
    use std::io::Write;

    let pid = std::process::id();
    let contents = match read_proc_starttime(pid) {
        Some(starttime) => format!("{pid}\n{starttime}\n"),
        None => format!("{pid}\n"),
    };
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// 単一ライター用のロックファイルを原子的に取得する。
fn acquire_instance_lock(lock_path: &str) -> anyhow::Result<()> {
    use std::fs::OpenOptions;

    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)
    {
        Ok(mut file) => write_instance_lock(&mut file),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let contents = std::fs::read_to_string(lock_path).unwrap_or_default();
            let (stale_pid, stale_starttime) = parse_instance_lock(&contents);
            if instance_lock_owner_alive(stale_pid, stale_starttime) {
                anyhow::bail!(
                    "another server instance appears to be running (lock: {lock_path}, pid: {stale_pid})"
                );
            }
            // TOCTOU 対策: stale と判定した直後に再読込し、最初に読んだ (PID, starttime)
            // と一致するときだけ削除する。書き換えられていたら他プロセスが先に
            // ロックを取得したとして、こちらは削除せず失敗する。
            let still_stale = std::fs::read_to_string(lock_path)
                .ok()
                .is_some_and(|current| {
                    parse_instance_lock(&current) == (stale_pid, stale_starttime)
                });
            if !still_stale {
                anyhow::bail!("instance lock changed while acquiring (lock: {lock_path}); retry");
            }
            std::fs::remove_file(lock_path)?;
            let mut file = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(lock_path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    anyhow::bail!(
                        "another server instance acquired the lock first (lock: {lock_path})"
                    );
                }
                Err(error) => return Err(error.into()),
            };
            write_instance_lock(&mut file)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod instance_lock_tests {
    use super::{parse_instance_lock, read_proc_starttime};

    #[test]
    fn parse_instance_lock_supports_new_and_legacy_formats() {
        assert_eq!(parse_instance_lock("12345\n67890\n"), (12345, Some(67890)));
        assert_eq!(parse_instance_lock("12345\n"), (12345, None));
        assert_eq!(
            parse_instance_lock(" 12345 \n 67890 \n"),
            (12345, Some(67890))
        );
        assert_eq!(parse_instance_lock("not-a-pid\n"), (0, None));
    }

    #[test]
    fn read_proc_starttime_matches_proc_stat_field_22() {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", std::process::id())) else {
            return; // /proc が無い環境ではスキップ
        };
        // テストバイナリ名に空白や ')' は含まれないため、単純な split でも field 22 が取れる。
        // 独立した取り出し方で starttime のフィールド位置を検証する。
        let expected = stat
            .split_whitespace()
            .nth(21)
            .and_then(|value| value.parse::<u64>().ok());
        assert!(expected.is_some());
        assert_eq!(read_proc_starttime(std::process::id()), expected);
    }
}

/// 本番相当（モック認証無効）で起動する前に必須設定を検査する。
/// 開発用のフォールバックがそのまま本番へ持ち込まれないようにする（fail-closed）。
/// 秘密情報の値は返さず、どの設定が不足しているかだけを返す。
pub fn production_config_errors(
    allow_mock_auth: bool,
    oidc_configured: bool,
    pinata_jwt: Option<&str>,
) -> Vec<&'static str> {
    if allow_mock_auth {
        return Vec::new();
    }
    let mut errors = Vec::new();
    if !oidc_configured {
        errors.push("OIDC is not configured (OIDC_* environment variables are required)");
    }
    match pinata_jwt {
        None => errors.push("PINATA_JWT is not configured"),
        Some("") => errors.push("PINATA_JWT is empty"),
        Some("mock_jwt") => errors.push("PINATA_JWT is the development mock value"),
        Some(_) => {}
    }
    errors
}

pub async fn run() -> anyhow::Result<()> {
    init_tracing();

    let allow_mock_auth = std::env::var("ALLOW_MOCK_AUTH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let oidc = oidc::OidcState::from_env();
    let pinata_jwt = std::env::var("PINATA_JWT").ok();
    let config_errors =
        production_config_errors(allow_mock_auth, oidc.is_some(), pinata_jwt.as_deref());
    if !config_errors.is_empty() {
        return Err(anyhow::anyhow!(
            "Production configuration is incomplete: {}",
            config_errors.join("; ")
        ));
    }

    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "file:local.db".to_string());
    let db_token = std::env::var("DATABASE_TOKEN").unwrap_or_else(|_| "".to_string());

    let local_path = if db_url.starts_with("file:") || !db_url.contains("://") {
        Some(db_url.strip_prefix("file:").unwrap_or(&db_url).to_string())
    } else {
        None
    };

    let db = if let Some(path) = &local_path {
        Builder::new_local(path).build().await?.connect()?
    } else {
        Builder::new_remote(db_url, db_token)
            .build()
            .await?
            .connect()?
    };

    // 単一ライター運用を起動時に強制する。ロックファイルを原子的に作成し、
    // 第二インスタンスの起動を fail-closed で拒否する。
    // リモート DB はこの排他が使えないため警告に留める。
    let instance_lock = if let Some(path) = &local_path {
        let lock_path = format!("{path}.instance.lock");
        acquire_instance_lock(&lock_path)?;
        tracing::info!("single-writer lock acquired: {lock_path}");
        Some(lock_path)
    } else {
        warn!("remote DB configured: single-writer lock is not enforced; ensure only one writer");
        None
    };

    // Run database migrations
    migration::run_migrations(&db).await?;
    // Detect checksum drift in already-applied migrations
    migration::verify_migrations(&db).await?;

    let verification_keys_env = std::env::var("JWT_RSA_VERIFICATION_KEYS").ok();
    let require_kid = std::env::var("JWT_REQUIRE_KID")
        .map(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
        .unwrap_or(false);
    let jwt_keys = jwt_keys_from_env(
        std::env::var("JWT_RSA_PRIVATE_KEY_PEM").ok(),
        std::env::var("JWT_SECRET").ok(),
        verification_keys_env.as_deref(),
        require_kid,
    )
    .map_err(|e| anyhow::anyhow!(e))?;
    tracing::info!("JWT signing algorithm: {}", jwt_keys.algorithm_name());

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
    let ipfs_attempts_env = std::env::var("IPFS_MAX_ATTEMPTS").ok();
    let ipfs_max_attempts = ipfs_max_attempts(ipfs_attempts_env.as_deref());

    let payment_receiving_address = std::env::var("PAYMENT_RECEIVING_ADDRESS")
        .ok()
        .filter(|address| {
            if payments::is_valid_eth_address(address) {
                true
            } else {
                warn!("PAYMENT_RECEIVING_ADDRESS is not a valid 0x address; payment orders are disabled");
                false
            }
        });

    if oidc.is_none() {
        tracing::info!("OIDC is not configured; JPKI OIDC endpoints are disabled");
    }

    let payment_receiving_xprv = std::env::var("PAYMENT_RECEIVING_XPRV").ok().filter(|xprv| {
        if payments::derive_receiving_address(xprv, 0).is_ok() {
            true
        } else {
            warn!("PAYMENT_RECEIVING_XPRV is invalid; HD derivation is disabled");
            false
        }
    });

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

    let age_policy = common::age::AgePolicy::from_env()
        .map_err(|e| anyhow::anyhow!("Invalid age policy configuration: {e}"))?;
    tracing::info!(
        "Age policy: default_minimum_age={}, country_overrides={}, operator_configured={}, require_explicit_configuration={}, enforcement_mode={}, settings_always_public={}",
        age_policy.default_minimum_age,
        age_policy.country_minimums.len(),
        age_policy.operator_configured,
        age_policy.require_explicit_configuration,
        age_policy.enforcement_mode,
        age_policy.settings_always_public
    );

    // 永続化済みの sub 失効閾値を起動時にロードする。
    let revoked_subs_before = {
        let mut rows = db
            .query("SELECT sub, revoked_at FROM revoked_subs", ())
            .await?;
        let mut map = HashMap::new();
        while let Some(row) = rows.next().await? {
            let sub: String = row.get(0)?;
            let revoked_at: i64 = row.get(1)?;
            map.insert(sub, revoked_at as usize);
        }
        Arc::new(Mutex::new(map))
    };

    // 永続化済みの失効 sid を起動時にロードする（再起動でログアウトが無効化されない）。
    let revoked_sessions = {
        let mut rows = db.query("SELECT sid FROM revoked_sessions", ()).await?;
        let mut set = HashSet::new();
        while let Some(row) = rows.next().await? {
            let sid: String = row.get(0)?;
            set.insert(sid);
        }
        Arc::new(Mutex::new(set))
    };

    let moderation_operators_env = std::env::var("MODERATION_OPERATOR_HASHES").ok();
    let moderation_operators = Arc::new(moderation_operators(moderation_operators_env.as_deref()));
    if moderation_operators.is_empty() {
        warn!("MODERATION_OPERATOR_HASHES is not set: BAN/unban/unpin endpoints are disabled");
    }

    // 本番相当（モック認証無効）のみ IP 単位のレート制限と同時実行上限を有効化する。
    let rate_limit = if allow_mock_auth {
        None
    } else {
        use std::num::NonZeroU32;
        let per_min = rate_limit_per_min(std::env::var("RATE_LIMIT_PER_MIN").ok().as_deref());
        let quota = governor::Quota::per_minute(
            NonZeroU32::new(per_min)
                .unwrap_or_else(|| unreachable!("rate_limit_per_min returns a positive value")),
        );
        Some(Arc::new(RateLimitState {
            limiter: governor::RateLimiter::keyed(quota),
            concurrency: Arc::new(tokio::sync::Semaphore::new(max_concurrent_requests(
                std::env::var("MAX_CONCURRENT_REQUESTS").ok().as_deref(),
            ))),
        }))
    };

    let shared_state = Arc::new(AppState {
        db: Arc::new(db),
        http_client: Client::builder()
            .timeout(oidc_http_timeout(
                std::env::var("OIDC_HTTP_TIMEOUT_SECS").ok().as_deref(),
            ))
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {e}"))?,
        jwt_keys: Arc::new(jwt_keys),
        allow_mock_auth,
        ipfs_gateways,
        ipfs_timeout,
        ipfs_max_attempts,
        payment_receiving_address,
        payment_receiving_xprv,
        payment_verifier: None,
        oidc,
        revoked_sessions,
        revoked_subs_before,
        age_policy,
        moderation_operators,
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit,
    });

    let worker_db = Arc::clone(&shared_state.db);
    let worker_oidc = shared_state.oidc.clone();
    let shutdown_flag = Arc::clone(&shared_state.shutting_down);
    let app = build_router(shared_state);

    let finalize_interval_secs = std::env::var("MODERATION_FINALIZE_INTERVAL_SECS").ok();
    let finalize_interval = moderation_finalize_interval(finalize_interval_secs.as_deref());
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let finalizer = tokio::spawn(run_moderation_finalizer(
        worker_db,
        worker_oidc,
        finalize_interval,
        shutdown_rx,
    ));

    let addr = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 3000));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Guild Backend listening on {}", addr);
    let drain_timeout_secs = std::env::var("GRACEFUL_DRAIN_TIMEOUT_SECS").ok();
    let drain_timeout = graceful_drain_timeout(drain_timeout_secs.as_deref());
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        shutdown_flag.store(true, std::sync::atomic::Ordering::Release);
        let _ = shutdown_tx.send(());
    });

    match tokio::time::timeout(drain_timeout, server).await {
        Ok(result) => result?,
        Err(_) => {
            warn!(
                "Graceful shutdown drain timed out after {:?}; closing remaining connections",
                drain_timeout
            );
        }
    }
    let _ = finalizer.await;
    // ロックファイルを削除して次の起動を許可する。
    if let Some(path) = instance_lock {
        let _ = std::fs::remove_file(&path);
    }
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
/// header values. Falls back to the local development origins only when
/// `allow_dev_defaults` is true (development mock auth); otherwise unset
/// config rejects all origins (fail-closed). Invalid entries are ignored,
/// and an empty result rejects all origins. Trailing slashes are removed
/// because browsers never send them in `Origin`.
pub fn allowed_origins(env_value: Option<&str>, allow_dev_defaults: bool) -> Vec<HeaderValue> {
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
        None if allow_dev_defaults => DEV_ALLOWED_ORIGINS
            .iter()
            .map(|origin| origin.parse().expect("valid dev origin"))
            .collect(),
        None => {
            warn!(
                "ALLOWED_ORIGINS is not set and mock auth is disabled: all CORS origins are rejected"
            );
            Vec::new()
        }
    }
}

/// HS256 の鍵はハッシュ出力（32バイト）以上の長さを必須とする（RFC 7518 §3.2）。
/// 短い鍵は総当たり・辞書攻撃で偽造トークンを作られるため、起動時に拒否する。
pub fn validate_jwt_secret(secret: &str) -> Result<(), String> {
    jwt::JwtKeys::from_hmac_secret(secret).map(|_| ())
}

/// 環境変数から JWT の署名鍵を選ぶ。
///
/// `JWT_RSA_PRIVATE_KEY_PEM`（PKCS#8 PEM）が設定されていれば RS256、
/// 未設定なら `JWT_SECRET` による HS256 を使う。どちらも無ければ起動しない。
pub fn jwt_keys_from_env(
    rsa_pem: Option<String>,
    secret: Option<String>,
    verification_keys_json: Option<&str>,
    require_kid: bool,
) -> Result<jwt::JwtKeys, String> {
    let mut keys = match (rsa_pem, secret) {
        (Some(pem), Some(secret)) => {
            // RS256 が選択されるため秘密は使われないが、短い値が残っている
            // 設定ミスは警告する（将来 RSA を外したときに弱鍵が露出する）。
            if jwt::JwtKeys::from_hmac_secret(&secret).is_err() {
                warn!(
                    "JWT_SECRET is set but shorter than 32 bytes; it is ignored while JWT_RSA_PRIVATE_KEY_PEM is configured"
                );
            }
            jwt::JwtKeys::from_rsa_private_key_pem(&pem).map_err(|error| {
                format!("JWT_RSA_PRIVATE_KEY_PEM is invalid: {error}; refusing to fall back to JWT_SECRET")
            })?
        }
        (Some(pem), None) => jwt::JwtKeys::from_rsa_private_key_pem(&pem)
            .map_err(|error| format!("JWT_RSA_PRIVATE_KEY_PEM is invalid: {error}"))?,
        (None, Some(secret)) => jwt::JwtKeys::from_hmac_secret(&secret)?,
        (None, None) => {
            return Err(
            "JWT_SECRET must be set (at least 32 random bytes), or JWT_RSA_PRIVATE_KEY_PEM for RS256"
                .to_string(),
        );
        }
    };

    if let Some(json) = verification_keys_json {
        if keys.algorithm_name() != "RS256" {
            return Err("JWT_RSA_VERIFICATION_KEYS is only valid with RS256".to_string());
        }
        let map: HashMap<String, String> = serde_json::from_str(json).map_err(|e| {
            format!("JWT_RSA_VERIFICATION_KEYS must be a kid->PEM JSON object: {e}")
        })?;
        for (kid, pem) in map {
            keys.add_verification_key(&kid, &pem)
                .map_err(|e| format!("JWT_RSA_VERIFICATION_KEYS[{kid}]: {e}"))?;
        }
    }
    if require_kid {
        if keys.algorithm_name() != "RS256" {
            return Err("JWT_REQUIRE_KID is only valid with RS256".to_string());
        }
        keys.set_require_kid(true);
    }
    Ok(keys)
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

/// `OIDC_HTTP_TIMEOUT_SECS` を解析する。未設定・不正値・範囲外は既定の 10 秒。
/// OIDC の token / JWKS 取得が実質無期限にハングするのを防ぐ。
pub fn oidc_http_timeout(env_value: Option<&str>) -> Duration {
    const DEFAULT_SECS: u64 = 10;
    const MIN_SECS: u64 = 1;
    const MAX_SECS: u64 = 60;

    env_value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| (MIN_SECS..=MAX_SECS).contains(secs))
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_SECS))
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

/// `IPFS_MAX_ATTEMPTS` を解析する。未設定・不正値・範囲外は既定の 1 回。
/// 上限は、設定ミスで1リクエストが長時間ハングするのを防ぐ。
pub fn ipfs_max_attempts(env_value: Option<&str>) -> usize {
    const DEFAULT: usize = 1;
    const MAX: usize = 5;
    env_value
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|attempts| (1..=MAX).contains(attempts))
        .unwrap_or(DEFAULT)
}

/// ADR-0004: カンマ区切りの運用者 user_hash を読み込む。
/// 空要素・重複は除去する。未設定時は空集合とし、特権操作をフェイルクローズする。
pub fn moderation_operators(env_value: Option<&str>) -> HashSet<String> {
    env_value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// グレースフル停止の drain タイムアウト（秒）。範囲外・不正値は既定 30 秒へフォールバック。
pub fn graceful_drain_timeout(env_value: Option<&str>) -> Duration {
    const DEFAULT_SECS: u64 = 30;
    const MIN_SECS: u64 = 1;
    const MAX_SECS: u64 = 300;
    env_value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| (MIN_SECS..=MAX_SECS).contains(secs))
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_SECS))
}

/// ADR-0004: 自動確定ワーカーの周期（秒）。範囲外・不正値は既定 300 秒へフォールバック。
pub fn moderation_finalize_interval(env_value: Option<&str>) -> Duration {
    const DEFAULT_SECS: u64 = 300;
    const MIN_SECS: u64 = 30;
    const MAX_SECS: u64 = 3600;
    env_value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| (MIN_SECS..=MAX_SECS).contains(secs))
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_SECS))
}

/// 期限到達済みの未確定通報をまとめて確定する。自動ワーカーと手動操作の共通処理。
pub async fn finalize_due_reports(db: &Connection) -> Result<usize, AppError> {
    let mut rows = db
        .query(
            "SELECT r.report_id FROM reports r
             JOIN contents c ON r.target_cid = c.cid
             WHERE c.status = 'Reported'
               AND r.concluded_at IS NULL
               AND datetime('now') >= COALESCE(r.deadline_at, datetime(r.created_at, '+7 days'))",
            (),
        )
        .await?;
    let mut report_ids = Vec::new();
    while let Some(row) = rows.next().await? {
        let report_id: i64 = row.get(0)?;
        report_ids.push(report_id);
    }

    let mut finalized = 0;
    for report_id in report_ids {
        let (_write_guard, tx) = begin_write_tx(db).await?;
        process_report_conclusion(&tx, report_id).await?;
        tx.commit().await?;
        finalized += 1;
    }
    Ok(finalized)
}

/// 起動中に動作する自動確定ワーカー。停止シグナルで終了する。
/// 期限切れリフレッシュトークンを削除する。期限切れレコードは認証に使えず、
/// 保持しても盗用検知の意味がないため、定期的に回収してDBの増大を防ぐ。
async fn purge_expired_refresh_tokens(db: &Connection) -> Result<u64, libsql::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    db.execute("DELETE FROM refresh_tokens WHERE expires_at <= ?", [now])
        .await
        .map(|_| db.changes())
}

/// 古い失効 sid を回収する。失効sid を持つ JWT は最大 SESSION_TTL_SECS で
/// 期限切れになるため、それより古いレコードを削除しても失効は復活しない。
/// リフレッシュ時は常に新しい sid が発行されるため再利用の心配もない。
async fn purge_stale_revoked_sessions(db: &Connection) -> Result<u64, libsql::Error> {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(oidc::SESSION_TTL_SECS + 60);
    db.execute(
        "DELETE FROM revoked_sessions WHERE revoked_at <= ?",
        [cutoff.format("%Y-%m-%d %H:%M:%S").to_string()],
    )
    .await
    .map(|_| db.changes())
}

/// 古い sub 失効閾値を回収する。閾値より前に発行された JWT は最大
/// SESSION_TTL_SECS で期限切れになるため、それより古い閾値は不要になる。
/// 新しい JWT は常に現在時刻より後ろの iat を持つため、失効は復活しない。
async fn purge_stale_revoked_subs(db: &Connection) -> Result<u64, libsql::Error> {
    let cutoff = chrono::Utc::now().timestamp() - oidc::SESSION_TTL_SECS - 60;
    db.execute("DELETE FROM revoked_subs WHERE revoked_at <= ?", [cutoff])
        .await
        .map(|_| db.changes())
}

async fn run_moderation_finalizer(
    db: Arc<Connection>,
    oidc: Option<Arc<oidc::OidcState>>,
    interval: Duration,
    mut shutdown: tokio::sync::watch::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // 起動直後の即時 tick は消費する

    // 起動時に一度、期限切れリフレッシュトークンを回収する。
    match purge_expired_refresh_tokens(db.as_ref()).await {
        Ok(0) => {}
        Ok(count) => tracing::info!("Purged {} expired refresh token(s) on startup", count),
        Err(error) => tracing::warn!("Refresh token purge failed on startup: {}", error),
    }
    match purge_stale_revoked_sessions(db.as_ref()).await {
        Ok(0) => {}
        Ok(count) => tracing::info!("Purged {} stale revoked session(s) on startup", count),
        Err(error) => tracing::warn!("Revoked session purge failed on startup: {}", error),
    }
    match purge_stale_revoked_subs(db.as_ref()).await {
        Ok(0) => {}
        Ok(count) => tracing::info!("Purged {} stale revoked sub(s) on startup", count),
        Err(error) => tracing::warn!("Revoked sub purge failed on startup: {}", error),
    }
    // pending はメモリ上にあり再起動時は空だが、register/take 以外の経路でも
    // 期限切れを回収できるよう起動時にも sweep しておく。
    if let Some(oidc) = &oidc {
        let count = oidc.sweep_expired_pending();
        if count > 0 {
            tracing::info!(
                "Purged {} expired OIDC pending authorization(s) on startup",
                count
            );
        }
    }

    // 最初の tick まで待つと再起動直後の期限切れ通報が最大 interval のあいだ
    // 未確定のまま残るため、起動時にも一度だけ確定処理を走らせる。
    // シャットダウンが既に通知されている（watch の初期値から変化した）場合は
    // 停止中の余計な書き込みを避けるため実行しない。
    if !shutdown.borrow().has_changed() {
        match finalize_due_reports(db.as_ref()).await {
            Ok(0) => {}
            Ok(count) => tracing::info!("Auto-finalized {} moderation report(s) on startup", count),
            Err(error) => tracing::warn!("Moderation finalization failed on startup: {}", error),
        }
    }

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match purge_expired_refresh_tokens(db.as_ref()).await {
                    Ok(0) => {}
                    Ok(count) => tracing::info!("Purged {} expired refresh token(s)", count),
                    Err(error) => tracing::warn!("Refresh token purge failed: {}", error),
                }
                match purge_stale_revoked_sessions(db.as_ref()).await {
                    Ok(0) => {}
                    Ok(count) => tracing::info!("Purged {} stale revoked session(s)", count),
                    Err(error) => tracing::warn!("Revoked session purge failed: {}", error),
                }
                match purge_stale_revoked_subs(db.as_ref()).await {
                    Ok(0) => {}
                    Ok(count) => tracing::info!("Purged {} stale revoked sub(s)", count),
                    Err(error) => tracing::warn!("Revoked sub purge failed: {}", error),
                }
                // 認証リクエストが来ないままだと register/take の掃除が走らないため、
                // 周期 tick でも期限切れ pending を回収する。
                if let Some(oidc) = &oidc {
                    let count = oidc.sweep_expired_pending();
                    if count > 0 {
                        tracing::info!("Purged {} expired OIDC pending authorization(s)", count);
                    }
                }
                match finalize_due_reports(db.as_ref()).await {
                    Ok(0) => {}
                    Ok(count) => tracing::info!("Auto-finalized {} moderation report(s)", count),
                    Err(error) => tracing::warn!("Moderation finalization failed: {}", error),
                }
            }
            _ = shutdown.changed() => {
                tracing::info!("Moderation finalizer stopped");
                break;
            }
        }
    }
}

/// readiness: DB が応答し、停止処理中でなければ 200。それ以外は 503。
/// liveness は `/health` のまま（プロセス生存のみ）。
pub async fn readiness(State(state): State<Arc<AppState>>) -> Response {
    if state
        .shutting_down
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "shutting_down"})),
        )
            .into_response();
    }
    match state.db.query("SELECT 1", ()).await {
        Ok(mut rows) => match rows.next().await {
            Ok(Some(_)) => (StatusCode::OK, Json(json!({"status": "ready"}))).into_response(),
            Ok(None) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"status": "db_unavailable"})),
            )
                .into_response(),
            Err(error) => {
                warn!("Readiness DB query failed: {error}");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"status": "db_unavailable"})),
                )
                    .into_response()
            }
        },
        Err(error) => {
            warn!("Readiness DB query failed: {error}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"status": "db_unavailable"})),
            )
                .into_response()
        }
    }
}

/// DEC-02: 有効な年齢判定ポリシー（非秘密設定値）を常時公開する。
pub async fn get_age_policy(State(state): State<Arc<AppState>>) -> Json<common::age::AgePolicy> {
    Json(state.age_policy.public_settings().clone())
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let allowed_origins_env = std::env::var("ALLOWED_ORIGINS").ok();
    Router::new()
        .route("/auth/login", post(auth_login))
        .route("/auth/verify", post(auth_verify))
        .route("/auth/oidc/start", get(oidc::start))
        .route("/auth/oidc/callback", get(oidc::callback))
        .route("/auth/refresh", post(oidc::refresh))
        .route("/auth/logout", post(logout))
        .route("/auth/backchannel-logout", post(oidc::backchannel_logout))
        .route("/content", get(list_contents))
        .route("/content/upload", post(upload_content))
        .route("/transactions", get(list_transactions))
        .route("/mod/report", post(mod_report))
        .route("/mod/reports", get(list_reports))
        .route("/mod/reports/{report_id}/finalize", post(finalize_report))
        .route("/mod/operators/{user_hash}/ban", post(ban_user))
        .route("/mod/operators/{user_hash}/unban", post(unban_user))
        .route("/mod/content/{cid}/unpin", post(record_unpin))
        .route("/mod/vote", post(mod_vote))
        .route("/mod/stats", get(mod_stats))
        .route("/content/{cid}", get(get_content))
        .route("/content/{cid}/audit", get(get_content_audit))
        .route(
            "/content/{cid}/favorite",
            post(add_favorite).delete(remove_favorite),
        )
        .route("/content/{cid}/support", post(support_creator))
        .route("/payments/orders", post(payments::create_order))
        .route("/payments/orders/{order_id}", get(payments::get_order))
        .route(
            "/payments/orders/{order_id}/bind",
            post(payments::bind_wallet),
        )
        .route(
            "/payments/orders/{order_id}/verify",
            post(payments::verify_order),
        )
        .route("/health", get(|| async { "ok" }))
        .route("/ready", get(readiness))
        .route("/metrics", get(metrics))
        .route("/metrics/client-events", post(client_events))
        .route("/policy/age", get(get_age_policy))
        .route("/me/export", get(me_export))
        .layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024))
        .layer(from_fn(reject_oversized_body))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        .layer(from_fn(network::ipv6_only_middleware))
        // IP ゲートの 403 を含む全レスポンスへヘッダーを付与する。
        .layer(from_fn(security_headers))
        // CORS はミドルウェア群の外側に置く。レート超過 429 / IP ゲート 403 の
        // 応答にも ACAO が付き、プリフライトは内側のミドルウェアを通過しない。
        .layer(
            CorsLayer::new()
                .allow_origin(allowed_origins(
                    allowed_origins_env.as_deref(),
                    state.allow_mock_auth,
                ))
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]),
        )
        // CORS プリフライトは CorsLayer で短絡し内側の security_headers を通らないため、
        // CorsLayer の外側にもう一段置く。これで 429/403/500/204 に加え
        // プリフライト応答にも nosniff / DENY が付く。二重付与でも同値のため結果は変わらない。
        .layer(from_fn(security_headers))
        // メトリクスは最も外側で観測する。OPTIONS 204 や 429 / 403 / 413 は
        // 内側のミドルウェアで短絡するため、ここで初めて全レスポンスを捕捉できる。
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            observe_metrics,
        ))
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
        // DB には全文を保持したまま、一覧レスポンスには要約だけを返す。
        let reason: String = row.get(2)?;
        items.push(ReportResponse {
            report_id: row.get(0)?,
            target_cid: row.get(1)?,
            reason: common::summarize_reason(&reason),
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
    if !is_safe_cid(&cid) {
        return Err(AppError::BadRequest("Invalid CID".to_string()));
    }

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
/// 開発用モック認証の nonce を発行から5分間、単回消費で保持する。
/// 本番は OIDC 経路 (`state`/`nonce`) を使うため、ここは開発専用。
static MOCK_NONCES: std::sync::LazyLock<Mutex<HashMap<String, std::time::Instant>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
const MOCK_NONCE_TTL: Duration = Duration::from_secs(300);

/// テスト用: モック nonce を直接発行してストアへ登録する。
#[cfg(test)]
pub fn issue_mock_nonce_for_test() -> String {
    let nonce = Uuid::new_v4().to_string();
    let mut nonces = MOCK_NONCES.lock().expect("nonce store");
    nonces.insert(nonce.clone(), std::time::Instant::now());
    nonce
}

pub async fn auth_login(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Oid4vpRequest>, AppError> {
    // 開発用モック経路を本番相当で公開しない（設計書との整合）。
    if !state.allow_mock_auth {
        return Err(AppError::NotImplemented(
            "mock authentication is disabled".to_string(),
        ));
    }
    let nonce = Uuid::new_v4().to_string();
    {
        let mut nonces = MOCK_NONCES
            .lock()
            .map_err(|_| AppError::Internal("nonce store is poisoned".to_string()))?;
        let now = std::time::Instant::now();
        nonces.retain(|_, issued| now.duration_since(*issued) <= MOCK_NONCE_TTL);
        nonces.insert(nonce.clone(), now);
    }
    let presentation_definition_uri =
        "https://guild.example.com/api/presentation-definition".to_string();

    Ok(Json(Oid4vpRequest {
        nonce,
        presentation_definition_uri,
    }))
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
    validate_age(&state.age_policy, birth_date, country_code)?;

    let verified_key = if payload.presentation_token == "mock_token" && state.allow_mock_auth {
        // 開発用モックでも nonce を必須化し、リプレイを防ぐ。
        let nonce = payload
            .nonce
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("nonce is required".to_string()))?;
        {
            let mut nonces = MOCK_NONCES
                .lock()
                .map_err(|_| AppError::Internal("nonce store is poisoned".to_string()))?;
            let now = std::time::Instant::now();
            nonces.retain(|_, issued| now.duration_since(*issued) <= MOCK_NONCE_TTL);
            match nonces.remove(nonce) {
                Some(_) => {}
                None => {
                    return Err(AppError::BadRequest("invalid or reused nonce".to_string()));
                }
            }
        }
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

    let now = chrono::Utc::now();
    let now_ts = now.timestamp() as usize;
    let expiration = now
        .checked_add_signed(chrono::Duration::hours(24))
        .expect("valid timestamp")
        .timestamp() as usize;

    let iat = {
        let mut revoked_subs = state
            .revoked_subs_before
            .lock()
            .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?;
        let issued_at = match revoked_subs.get(&user_hash).copied() {
            Some(threshold) => std::cmp::max(now_ts, threshold.saturating_add(1)),
            None => now_ts,
        };
        // 新しい認証セッションの発行により、過去セッション失効の閾値は不要になる。
        revoked_subs.remove(&user_hash);
        issued_at
    };

    let claims = Claims {
        sub: user_hash.clone(),
        exp: expiration,
        sid: None,
        iat: Some(iat),
    };

    let token = encode(
        &state.jwt_keys.header(),
        &claims,
        state.jwt_keys.encoding_key(),
    )
    .map_err(|e| AppError::Internal(format!("JWT generation failed: {}", e)))?;

    Ok(Json(VerifyResponse {
        authorized: true,
        user_hash: Some(user_hash),
        token: Some(token),
    }))
}

/// セッション ID を失効させ、DB と in-memory の両方へ記録する。
pub(crate) async fn revoke_session(state: &Arc<AppState>, sid: &str) -> Result<(), AppError> {
    state
        .db
        .execute(
            "INSERT OR IGNORE INTO revoked_sessions (sid) VALUES (?)",
            [sid.to_string()],
        )
        .await?;
    state
        .revoked_sessions
        .lock()
        .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?
        .insert(sid.to_string());
    Ok(())
}

/// セッションを失効させる。以後、同じ `sid` の JWT は拒否される。
pub async fn logout(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    headers: axum::http::HeaderMap,
) -> Result<Response, AppError> {
    if let Some(sid) = &claims.sid {
        revoke_session(&state, sid).await?;
    }
    // リフレッシュトークンが添付されていれば、そのファミリを失効させる。
    if let Some(token) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|part| {
                let (name, value) = part.trim().split_once('=')?;
                (name == "refresh_token").then_some(value)
            })
        })
    {
        let token_hash = sha256::digest(token.as_bytes());
        let mut rows = state
            .db
            .query(
                "SELECT family_id FROM refresh_tokens WHERE token_hash = ?",
                [token_hash],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let family_id: String = row.get(0)?;
            state
                .db
                .execute(
                    "UPDATE refresh_tokens SET used_at = datetime('now') WHERE family_id = ? AND used_at IS NULL",
                    [family_id],
                )
                .await?;
        }
    }
    // DB 側のファミリ失効だけではブラウザの Cookie が 30 日残るため、
    // 同じ Path/SameSite 属性の空 Cookie で即時削除する。
    // Secure 属性は refresh_cookie と同じ OIDC 設定（OIDC_COOKIE_SECURE）に合わせる。
    // 本番の OidcState は from_env でこの値を読み込むため、同一の判定を使う。
    // 未設定なら Secure を付与する（cookie_secure の既定と同じ fail-closed）。
    let secure = std::env::var("OIDC_COOKIE_SECURE")
        .map(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
        .unwrap_or(true);
    let delete_cookie = format!(
        "refresh_token=; Path=/auth; HttpOnly; SameSite=Strict; Max-Age=0{}",
        if secure { "; Secure" } else { "" }
    );
    let cookie = HeaderValue::from_str(&delete_cookie)
        .map_err(|_| AppError::Internal("invalid refresh cookie".to_string()))?;
    let mut response = Json(true).into_response();
    response.headers_mut().append(header::SET_COOKIE, cookie);
    Ok(response)
}

/// ログへ出す URL から userinfo と query を除去する。
/// ゲートウェイ URL に認証情報やトークンが含まれても記録しない。
pub fn redact_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.set_query(None);
            // フラグメントにもトークン等の秘密が入り得るため除去する。
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => url.split(['?', '#']).next().unwrap_or(url).to_string(),
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

/// CID を URL パスへ埋め込む前の安全検査。
///
/// `common::is_valid_cid` による厳密形式検証（CIDv0: `Qm` + base58 44 文字、
/// CIDv1: `b` + base32lower の varint/version/codec/multihash/digest 検証）へ委譲する。
/// 許可される文字集合と全体長が固定されるため、`/`、`..`、`?`、`#` などの
/// パス・クエリ・フラグメント注入を試みる文字列は厳密形式検証に包含されて拒否される。
pub fn is_safe_cid(cid: &str) -> bool {
    common::is_valid_cid(cid)
}

pub async fn get_content(
    State(state): State<Arc<AppState>>,
    _claims: AuthClaims,
    Path(cid): Path<String>,
) -> Result<Response, AppError> {
    if !is_safe_cid(&cid) {
        return Err(AppError::BadRequest("Invalid CID".to_string()));
    }

    // 一覧と同じく Approve 済みのみ配信する。通報中（Reported）/却下（Rejected）は
    // CID を知っていても直接取得できないようにする（暫定非表示の実効化）。
    let mut rows = state
        .db
        .query(
            "SELECT checksum, type FROM contents WHERE cid = ? AND status = 'Approved'",
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

    // ゲートウェイは設定順に試す。ネットワークエラー（接続失敗・タイムアウト）に
    // 限り各ゲートウェイを最大 ipfs_max_attempts 回まで再試行する。
    // HTTP エラー・サイズ超過・チェックサム不一致は再試行せず即拒否する。
    // すべて失敗した場合は 500 を返し、部分的なデータを完全なコンテンツとして返さない。
    let mut last_error = String::new();

    if state.ipfs_gateways.is_empty() {
        last_error = "no IPFS gateways configured".to_string();
    }

    for gateway_template in &state.ipfs_gateways {
        let gateway_url = gateway_template.replace("{cid}", &cid);
        let mut response = None;
        for attempt in 1..=state.ipfs_max_attempts {
            match state
                .http_client
                .get(&gateway_url)
                .timeout(state.ipfs_timeout)
                .send()
                .await
            {
                Ok(res) => {
                    response = Some(res);
                    break;
                }
                Err(error) => {
                    last_error = format!(
                        "{}: {} (attempt {}/{})",
                        redact_url(&gateway_url),
                        describe_reqwest_error(&error),
                        attempt,
                        state.ipfs_max_attempts
                    );
                }
            }
        }
        let Some(mut response) = response else {
            continue;
        };
        if response.status().is_success() {
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
                // 本文・チェックサム値は出さず、どの CID / ゲートウェイで
                // 不整合が起きたかだけを記録する（運用監視用）。
                warn!(
                    "IPFS integrity mismatch: cid={} gateway={}",
                    cid,
                    redact_url(&gateway_url)
                );
                return Err(AppError::Unauthorized("Integrity check failed".to_string()));
            }

            return Response::builder()
                .header(header::CONTENT_TYPE, response_content_type)
                // ユーザー管理バイトの MIME スニッフィングを防ぐ（HTML として
                // 解釈されると XSS になり得る）。
                .header("X-Content-Type-Options", "nosniff")
                .header("X-Forensic-Checksum", expected_checksum)
                .body(Body::from(bytes))
                .map_err(|e| AppError::Internal(format!("Response build error: {}", e)));
        } else {
            last_error = format!("{}: HTTP {}", redact_url(&gateway_url), response.status());
        }
    }

    Err(AppError::Internal(format!(
        "All IPFS gateways failed. Last error: {}",
        last_error
    )))
}

/// `GET /content` の省略可能なクエリ。`limit` / `offset` は生の文字列として受け、
/// 純粋関数 `parse_content_query` で安全側の既定値へ正規化する。
#[derive(Debug, Deserialize)]
pub struct ContentQuery {
    pub q: Option<String>,
    pub tag: Option<String>,
    #[serde(rename = "type")]
    pub content_type: Option<String>,
    pub limit: Option<String>,
    pub offset: Option<String>,
    /// "1" / "true" のとき、本人のお気に入りのみを返す。
    pub favorite_only: Option<String>,
}

/// 正規化済みの検索条件。`limit` / `offset` は SQL のバインド値として使う整数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentFilter {
    pub q: Option<String>,
    pub tag: Option<String>,
    pub content_type: Option<String>,
    pub limit: i64,
    pub offset: i64,
    pub favorite_only: bool,
}

pub const DEFAULT_CONTENT_LIMIT: i64 = 50;
pub const MAX_CONTENT_LIMIT: i64 = 100;

/// 検索クエリを純粋に解析する。範囲外・非数値の `limit` は 400 にせず既定 50
/// （安全側）へフォールバックする。負・非数値の `offset` は 0 とする。
/// `type` は完全一致を保つため前後の空白を除去しない（未知の値は 0 件になる）。
pub fn parse_content_query(query: &ContentQuery) -> ContentFilter {
    let optional_text = |value: Option<&str>| -> Option<String> {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    };
    let limit = query
        .limit
        .as_deref()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| (1..=MAX_CONTENT_LIMIT).contains(value))
        .unwrap_or(DEFAULT_CONTENT_LIMIT);
    let offset = query
        .offset
        .as_deref()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(0);
    // favorite_only は "1" / "true"（大文字小文字・前後空白を無視）だけを真とし、
    // それ以外は安全側の false（絞り込みなし）として扱う。
    let favorite_only = query
        .favorite_only
        .as_deref()
        .is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"));
    ContentFilter {
        q: optional_text(query.q.as_deref()),
        tag: optional_text(query.tag.as_deref()),
        content_type: query
            .content_type
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        limit,
        offset,
        favorite_only,
    }
}

/// LIKE パターン内のワイルドカード（`%` / `_`）とエスケープ文字自身（`\`）を
/// リテラルとして扱う。SQL 側では必ず `ESCAPE '\'` と併用する。
pub fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '%' => escaped.push_str("\\%"),
            '_' => escaped.push_str("\\_"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// `GET /content` の SQL 文とバインド値を組み立てる純粋関数。
/// ユーザー入力は SQL 文へ一切埋め込まず、すべてバインドパラメータで渡す。
/// `viewer_sub` は is_favorite の判定と favorite_only の絞り込みにだけ使い、
/// ログやエラー応答へは出力しない。
pub fn build_content_query(
    filter: &ContentFilter,
    viewer_sub: &str,
) -> (String, Vec<libsql::Value>) {
    // JOIN するとお気に入りの重複行でページング件数が壊れるため、相関 EXISTS
    // で判定する（重複行を作らず、favorite_only のときだけ favorites を参照）。
    let mut sql = String::from(
        "SELECT cid, checksum, type, status, price, title, description, tags, \
         EXISTS (SELECT 1 FROM favorites f WHERE f.cid = contents.cid AND f.user_hash = ?) AS is_favorite \
         FROM contents WHERE status = 'Approved'",
    );
    // SELECT 句のプレースホルダが先に来るため、閲覧者の sub を最初に積む。
    let mut params: Vec<libsql::Value> = vec![libsql::Value::Text(viewer_sub.to_string())];

    if let Some(q) = filter.q.as_deref() {
        sql.push_str(" AND (title LIKE ? ESCAPE '\\' OR description LIKE ? ESCAPE '\\')");
        let pattern = format!("%{}%", escape_like(q));
        params.push(libsql::Value::Text(pattern.clone()));
        params.push(libsql::Value::Text(pattern));
    }
    if let Some(tag) = filter.tag.as_deref() {
        // tags はカンマ区切り。先頭タグと、カンマに続くタグの前方一致を許す。
        sql.push_str(" AND (tags LIKE ? ESCAPE '\\' OR tags LIKE ? ESCAPE '\\')");
        let escaped = escape_like(tag);
        params.push(libsql::Value::Text(format!("{escaped}%")));
        params.push(libsql::Value::Text(format!("%,{escaped}%")));
    }
    if let Some(kind) = filter.content_type.as_deref() {
        sql.push_str(" AND type = ?");
        params.push(libsql::Value::Text(kind.to_string()));
    }
    if filter.favorite_only {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM favorites ff WHERE ff.cid = contents.cid AND ff.user_hash = ?)",
        );
        params.push(libsql::Value::Text(viewer_sub.to_string()));
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ? OFFSET ?");
    params.push(libsql::Value::Integer(filter.limit));
    params.push(libsql::Value::Integer(filter.offset));
    (sql, params)
}

/// 認証必須・Approved 限定・新着順は維持しつつ、省略可能な検索条件を適用する。
/// 各項目の is_favorite は閲覧者（Claims.sub）視点で設定する。
pub async fn list_contents(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Query(query): Query<ContentQuery>,
) -> Result<Json<Vec<ContentMeta>>, AppError> {
    let filter = parse_content_query(&query);
    let (sql, params) = build_content_query(&filter, &claims.sub);
    let mut rows = state.db.query(&sql, params).await?;

    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ContentMeta {
            cid: row.get(0)?,
            checksum: row.get(1)?,
            content_type: row.get(2)?,
            status: row.get(3)?,
            price: row.get(4)?,
            title: row.get(5)?,
            description: row.get(6)?,
            tags: parse_tags_field(&row.get::<String>(7)?),
            is_favorite: row.get::<i64>(8)? != 0,
        });
    }
    Ok(Json(items))
}

/// お気に入り操作のレスポンス。ユーザー識別子は一切含めない。
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FavoriteResponse {
    pub is_favorite: bool,
}

/// お気に入りの追加 / 削除に共通する処理。
/// ユーザー識別は `Claims.sub` のみ。リクエスト本文やクエリの ID は受け取らない。
/// 既存行への再追加・未登録行の削除はどちらも成功として扱う（冪等）。
/// DB 由来のエラー詳細は sub を含み得るため、公開・ログ出力とも固定文言にする。
async fn change_favorite(
    state: &Arc<AppState>,
    sub: &str,
    cid: &str,
    add: bool,
) -> Result<FavoriteResponse, AppError> {
    if !is_safe_cid(cid) {
        return Err(AppError::BadRequest("Invalid CID".to_string()));
    }
    let mut rows = state
        .db
        .query(
            "SELECT 1 FROM contents WHERE cid = ? AND status = 'Approved'",
            [cid.to_string()],
        )
        .await
        .map_err(|_| AppError::Internal("favorite lookup failed".to_string()))?;
    if rows
        .next()
        .await
        .map_err(|_| AppError::Internal("favorite lookup failed".to_string()))?
        .is_none()
    {
        // 存在しない / Approved でないコンテンツの有無を識別子で区別しない。
        return Err(AppError::NotFound);
    }
    if add {
        state
            .db
            .execute(
                "INSERT OR IGNORE INTO favorites (user_hash, cid) VALUES (?, ?)",
                (sub.to_string(), cid.to_string()),
            )
            .await
            .map_err(|_| AppError::Internal("favorite update failed".to_string()))?;
    } else {
        state
            .db
            .execute(
                "DELETE FROM favorites WHERE user_hash = ? AND cid = ?",
                (sub.to_string(), cid.to_string()),
            )
            .await
            .map_err(|_| AppError::Internal("favorite update failed".to_string()))?;
    }
    Ok(FavoriteResponse { is_favorite: add })
}

/// `POST /content/{cid}/favorite`: 認証済みユーザーが自分の sub でお気に入りを追加する。
pub async fn add_favorite(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Path(cid): Path<String>,
) -> Result<Json<FavoriteResponse>, AppError> {
    Ok(Json(
        change_favorite(&state, &claims.sub, &cid, true).await?,
    ))
}

/// `DELETE /content/{cid}/favorite`: 認証済みユーザーが自分の sub のお気に入りを削除する。
pub async fn remove_favorite(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Path(cid): Path<String>,
) -> Result<Json<FavoriteResponse>, AppError> {
    Ok(Json(
        change_favorite(&state, &claims.sub, &cid, false).await?,
    ))
}

/// 支援APIは無効化している。模擬支援記録はチェーン送金の検証を欠き支払証明に
/// ならないため、JPYC送金検証の実装まで常に 501 を返す。
/// 認証は維持し、未認証アクセスには従来どおり 401 を返す。
pub async fn support_creator(
    _claims: AuthClaims,
    Path(cid): Path<String>,
) -> Result<Json<String>, AppError> {
    if !is_safe_cid(&cid) {
        return Err(AppError::BadRequest("Invalid CID".to_string()));
    }
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
    let entry_hash = entry.calculate_hash_v2();

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
    if !is_safe_cid(&report.target_cid) {
        return Err(AppError::BadRequest("Invalid CID".to_string()));
    }
    // Free-text reason guard: reject empty input and oversized payloads
    // before touching the DB. The exact moderation policy is pending
    // ADR-0004; this only enforces basic input hygiene.
    let reason = report.reason.trim();
    if reason.is_empty() || reason.len() > 2000 {
        return Err(AppError::BadRequest("Invalid report reason".to_string()));
    }

    let (_write_guard, tx) = begin_write_tx(&state.db).await?;

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

    // ADR-0004: 確定から30日以内の再通報は異議として受理する。
    // 元の確定を上書きせず、新しい通報行を appeal_of で関連付ける。
    let mut prior_rows = tx
        .query(
            "SELECT report_id FROM reports
             WHERE target_cid = ? AND concluded_at IS NOT NULL
               AND datetime('now') <= datetime(concluded_at, '+30 days')
             ORDER BY concluded_at DESC, report_id DESC LIMIT 1",
            [report.target_cid.clone()],
        )
        .await?;
    let appeal_of: Option<i64> = match prior_rows.next().await? {
        Some(row) => Some(row.get(0)?),
        None => None,
    };

    match appeal_of {
        Some(prior_id) => {
            tx.execute(
                "INSERT INTO reports (target_cid, reason, deadline_at, appeal_of)
                 VALUES (?, ?, datetime('now', '+7 days'), ?)",
                (report.target_cid.clone(), reason.to_string(), prior_id),
            )
            .await
        }
        None => {
            tx.execute(
                "INSERT INTO reports (target_cid, reason, deadline_at)
                 VALUES (?, ?, datetime('now', '+7 days'))",
                (report.target_cid.clone(), reason.to_string()),
            )
            .await
        }
    }
    .map_err(|e| AppError::Internal(format!("DB Insert failed: {}", e)))?;

    let audit_reason = if appeal_of.is_some() {
        format!("ADR-0004 appeal filed: {}", reason)
    } else {
        format!("User report: {}", reason)
    };

    // Use forensic helper to update status and log it
    update_content_status(
        &tx,
        &report.target_cid,
        "Reported",
        &_claims.sub, // The reporter's hash
        &audit_reason,
    )
    .await?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;

    Ok(Json(true))
}

/// 票数集計で「現在 BAN 中の投票者」を除外するための共通述語。
///
/// `v` = votes, `u` = users の別名で使う。判定は `ensure_user_not_banned` と同じ:
/// BAN は `is_banned = TRUE` かつ `ban_until` が NULL（無期限）または未来。
/// 期限切れ（`ban_until <= now`）は判定上 BAN 解除済みなので票を残す。
/// `users` に該当行がない投票者も BAN 情報がないため票を残す（LEFT JOIN と併用）。
const VOTE_NOT_FROM_BANNED_VOTER_SQL: &str = "(u.user_hash IS NULL OR COALESCE(u.is_banned, FALSE) = FALSE OR (u.ban_until IS NOT NULL AND u.ban_until <= datetime('now')))";

/// ADR-0004: 期限到達済みの通報を集計して確定する。
///
/// - 投票期間（7日。`deadline_at` 未設定時は `created_at + 7 days`）が終わる前は
///   何もしない。3票到達による即時確定は行わない。
/// - 定足数（3票）未達なら通報を棄却し、コンテンツを `Approved` に戻す。
/// - 同数は公開継続リスクを避けて `Rejected` とする。
/// - 確定済み・レビュー対象外の通報は冪等に無視する。
/// - 投票後に BAN された Juror の票は定足数に算入しない（期限切れ BAN は算入する）。
pub async fn process_report_conclusion(
    tx: &libsql::Transaction,
    report_id: i64,
) -> Result<(), AppError> {
    let mut report_rows = tx
        .query(
            "SELECT r.target_cid, c.status, r.concluded_at,
                    CASE WHEN datetime('now') >= COALESCE(r.deadline_at, datetime(r.created_at, '+7 days'))
                         THEN 1 ELSE 0 END AS due
             FROM reports r
             JOIN contents c ON r.target_cid = c.cid
             WHERE r.report_id = ?",
            [report_id],
        )
        .await?;

    let (cid, content_status, concluded_at, due) = match report_rows.next().await? {
        Some(row) => {
            let cid: String = row.get(0)?;
            let content_status: String = row.get(1)?;
            let concluded_at: Option<String> = row.get(2)?;
            let due: i64 = row.get(3)?;
            (cid, content_status, concluded_at, due)
        }
        None => return Ok(()),
    };

    if due == 0 || content_status != "Reported" || concluded_at.is_some() {
        return Ok(());
    }

    // 1. Tally votes（BAN 済み Juror の票は除外。期限切れ BAN の票は残す）
    let mut rows = tx
        .query(
            &format!(
                "SELECT v.vote_result, COUNT(*) AS cnt
                 FROM votes v
                 LEFT JOIN users u ON u.user_hash = v.voter_hash
                 WHERE v.report_id = ? AND {VOTE_NOT_FROM_BANNED_VOTER_SQL}
                 GROUP BY v.vote_result"
            ),
            [report_id],
        )
        .await?;

    let mut approve_count: i64 = 0;
    let mut reject_count: i64 = 0;

    while let Some(row) = rows.next().await? {
        let result: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        if result == "Approve" {
            approve_count = count;
        } else if result == "Reject" {
            reject_count = count;
        }
    }

    let total_votes = approve_count + reject_count;
    let (final_status, reason) = if total_votes >= 3 {
        // ADR-0004: 同数は公開継続リスクを避けて Rejected 側に倒す。
        let status = if reject_count >= approve_count {
            "Rejected"
        } else {
            "Approved"
        };
        (
            status,
            format!(
                "ADR-0004 conclusion: {} Approve, {} Reject",
                approve_count, reject_count
            ),
        )
    } else {
        (
            "Approved",
            format!(
                "ADR-0004 quorum not met: {} votes (need 3); report dismissed",
                total_votes
            ),
        )
    };

    update_content_status(tx, &cid, final_status, "SYSTEM_GOVERNANCE", &reason).await?;
    tx.execute(
        "UPDATE reports SET concluded_at = datetime('now') WHERE report_id = ?",
        [report_id],
    )
    .await?;

    Ok(())
}

/// ADR-0004: 期限到達済みの通報を明示的に確定する（運用/スケジューラからの呼び出し用）。
/// 期限内に呼ばれても早期確定は行わず、何も変更しない。冪等。
pub async fn finalize_report(
    State(state): State<Arc<AppState>>,
    _claims: AuthClaims,
    Path(report_id): Path<i64>,
) -> Result<Json<bool>, AppError> {
    if report_id <= 0 {
        return Err(AppError::BadRequest("Invalid report id".to_string()));
    }
    let (_write_guard, tx) = begin_write_tx(&state.db).await?;
    let mut rows = tx
        .query("SELECT 1 FROM reports WHERE report_id = ?", [report_id])
        .await?;
    if rows.next().await?.is_none() {
        return Err(AppError::NotFound);
    }
    process_report_conclusion(&tx, report_id).await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    Ok(Json(true))
}

#[derive(Deserialize)]
pub struct BanRequest {
    pub reason: String,
    pub duration_days: Option<i64>,
}

#[derive(Deserialize)]
pub struct ModerationActionRequest {
    pub reason: String,
}

/// 運用理由の基本検証。前後空白を除去し、空・過大な入力を拒否する。
fn validated_moderation_reason(reason: &str) -> Result<&str, AppError> {
    let trimmed = reason.trim();
    if trimmed.is_empty() || trimmed.len() > 2000 {
        return Err(AppError::BadRequest(
            "Invalid moderation reason".to_string(),
        ));
    }
    Ok(trimmed)
}

/// ADR-0004: 特権操作はこのデプロイで設定された運用者だけが実行できる。
/// 未設定・BAN済みの主体は拒否する（フェイルクローズ）。
async fn require_moderation_operator(state: &AppState, actor: &str) -> Result<(), AppError> {
    if !state.moderation_operators.contains(actor) {
        return Err(AppError::Forbidden(
            "Moderation operator is not configured for this account".to_string(),
        ));
    }
    ensure_user_not_banned(state.db.as_ref(), actor).await?;
    Ok(())
}

/// 運用判断の記録。DEC-04 の第三者検証可能な監査ではなく、運用ログである。
async fn record_governance_action(
    db: &Connection,
    action: &str,
    target: &str,
    actor: &str,
    reason: &str,
    details: Option<String>,
) -> Result<(), AppError> {
    db.execute(
        "INSERT INTO governance_audit_log (action, target, actor_hash, reason, details)
         VALUES (?, ?, ?, ?, ?)",
        (action, target, actor, reason, details),
    )
    .await?;
    Ok(())
}

/// ADR-0004: 自動BANは行わない。運用者が証跡を確認して判断し、根拠と期間を記録する。
pub async fn ban_user(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Path(user_hash): Path<String>,
    Json(body): Json<BanRequest>,
) -> Result<Json<bool>, AppError> {
    require_moderation_operator(&state, &claims.sub).await?;
    let reason = validated_moderation_reason(&body.reason)?;
    let user_hash = user_hash.trim();
    if user_hash.is_empty() {
        return Err(AppError::BadRequest("Invalid user hash".to_string()));
    }
    if let Some(days) = body.duration_days
        && !(1..=3650).contains(&days)
    {
        return Err(AppError::BadRequest("Invalid ban duration".to_string()));
    }
    if user_hash == claims.sub {
        return Err(AppError::BadRequest(
            "Operators cannot ban themselves".to_string(),
        ));
    }

    let (_write_guard, tx) = begin_write_tx(&state.db).await?;
    let mut rows = tx
        .query("SELECT 1 FROM users WHERE user_hash = ?", [user_hash])
        .await?;
    if rows.next().await?.is_none() {
        return Err(AppError::NotFound);
    }
    // 期間付き BAN は ban_until に期限を記録する。NULL は無期限を表す。
    // duration_days は上流で 1..=3650 に検証済みのため文字列連結による SQL 注入はないが、
    // 引数としてバインドして日数を組み立てる。
    if let Some(days) = body.duration_days {
        tx.execute(
            "UPDATE users SET is_banned = TRUE, banned_at = datetime('now'),
                    ban_until = datetime('now', '+' || ? || ' days')
             WHERE user_hash = ?",
            libsql::params![days, user_hash.to_string()],
        )
        .await?;
    } else {
        tx.execute(
            "UPDATE users SET is_banned = TRUE, banned_at = datetime('now'), ban_until = NULL
             WHERE user_hash = ?",
            [user_hash],
        )
        .await?;
    }
    let details = match body.duration_days {
        Some(days) => Some(format!("{{\"duration_days\":{days}}}")),
        None => Some("{\"indefinite\":true}".to_string()),
    };
    record_governance_action(&tx, "BAN", user_hash, &claims.sub, reason, details).await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    Ok(Json(true))
}

/// 誤BANの回復。異議申立て後の運用判断として記録する。
pub async fn unban_user(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Path(user_hash): Path<String>,
    Json(body): Json<ModerationActionRequest>,
) -> Result<Json<bool>, AppError> {
    require_moderation_operator(&state, &claims.sub).await?;
    let reason = validated_moderation_reason(&body.reason)?;
    let user_hash = user_hash.trim();
    if user_hash.is_empty() {
        return Err(AppError::BadRequest("Invalid user hash".to_string()));
    }

    let (_write_guard, tx) = begin_write_tx(&state.db).await?;
    let mut rows = tx
        .query("SELECT 1 FROM users WHERE user_hash = ?", [user_hash])
        .await?;
    if rows.next().await?.is_none() {
        return Err(AppError::NotFound);
    }
    tx.execute(
        "UPDATE users SET is_banned = FALSE, banned_at = NULL, ban_until = NULL WHERE user_hash = ?",
        [user_hash],
    )
    .await?;
    record_governance_action(&tx, "UNBAN", user_hash, &claims.sub, reason, None).await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    Ok(Json(true))
}

/// ADR-0004 §8: 実際の Pinata ピン解除は手動で行う。ここでは解除判断と理由を記録する。
/// オンチェーン・他ゲートウェイからの削除は保証しない。
pub async fn record_unpin(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    Path(cid): Path<String>,
    Json(body): Json<ModerationActionRequest>,
) -> Result<Json<bool>, AppError> {
    require_moderation_operator(&state, &claims.sub).await?;
    let reason = validated_moderation_reason(&body.reason)?;
    if !is_safe_cid(&cid) {
        return Err(AppError::BadRequest("Invalid CID".to_string()));
    }

    let (_write_guard, tx) = begin_write_tx(&state.db).await?;
    let mut rows = tx
        .query("SELECT 1 FROM contents WHERE cid = ?", [cid.clone()])
        .await?;
    if rows.next().await?.is_none() {
        return Err(AppError::NotFound);
    }
    record_governance_action(&tx, "UNPIN", &cid, &claims.sub, reason, None).await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    Ok(Json(true))
}

/// モデレーション投票の署名対象メッセージを生成する。
/// 現行形式（互換性のため変更しない）を一箇所に集約し、将来のドメイン分離を
/// この関数だけで行えるようにする。
pub fn moderation_vote_message(report_id: i64, result: &str) -> String {
    json!({"report_id": report_id, "result": result}).to_string()
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
    // ADR-0004: BAN 済み利用者は投票資格を持たない。
    ensure_user_not_banned(&state.db, &claims.sub).await?;

    // 1. Verify Signature (Auditability)
    let message = moderation_vote_message(vote.report_id, &vote.result);

    // 空署名は長さ検証前にモックとして受理されないよう明示的に拒否する。
    if vote.signature.is_empty() {
        return Err(AppError::BadRequest("Signature is empty".to_string()));
    }
    let mock_sig = state.allow_mock_auth && is_mock_signature(&vote.signature);

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

    let (_write_guard, tx) = begin_write_tx(&state.db).await?;

    // Report must exist, still be under review, and be inside its voting
    // window. Votes on concluded or expired reports are rejected so that a
    // re-tally can never flip an already-decided status.
    let mut report_rows = tx
        .query(
            "SELECT c.status, c.owner_hash,
                    CASE WHEN datetime('now') >= COALESCE(r.deadline_at, datetime(r.created_at, '+7 days'))
                         THEN 1 ELSE 0 END AS expired
             FROM reports r
             JOIN contents c ON r.target_cid = c.cid
             WHERE r.report_id = ?",
            [vote.report_id],
        )
        .await?;
    let (target_status, owner_hash, expired) = match report_rows.next().await? {
        Some(row) => {
            let status: String = row.get(0)?;
            let owner_hash: Option<String> = row.get(1)?;
            let expired: i64 = row.get(2)?;
            (status, owner_hash, expired)
        }
        None => return Err(AppError::NotFound),
    };
    if target_status != "Reported" {
        return Err(AppError::BadRequest(
            "Report is already concluded".to_string(),
        ));
    }
    if expired == 1 {
        return Err(AppError::BadRequest("Voting period has ended".to_string()));
    }
    // ADR-0004: 対象コンテンツの投稿者は投票できない。
    if owner_hash.as_deref() == Some(vote.juror_hash.as_str()) {
        return Err(AppError::BadRequest(
            "Content owner cannot vote".to_string(),
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

/// 開発用モック署名として受理する RSA-2048 署名のバイト長。
/// 空配列や不正長の全ゼロ列が `iter().all(|&b| b == 0)` で真になるため、
/// 長さも明示的に検証する（`ALLOW_MOCK_AUTH=1` のときのみ受理）。
pub(crate) const MOCK_SIGNATURE_LEN: usize = 256;

/// 256 バイトの全ゼロ署名だけを開発用モックとして扱う。
pub(crate) fn is_mock_signature(signature: &[u8]) -> bool {
    signature.len() == MOCK_SIGNATURE_LEN && signature.iter().all(|&b| b == 0)
}

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

/// base58btc エンコード。CIDv0 の生成に使う最小実装（外部依存なし）。
fn base58btc_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let leading_zeros = input.iter().take_while(|&&byte| byte == 0).count();
    let mut bytes = input.to_vec();
    let mut encoded = Vec::new();
    let mut start = leading_zeros;
    while start < bytes.len() {
        let mut carry = 0u32;
        for byte in &mut bytes[start..] {
            let value = carry * 256 + u32::from(*byte);
            *byte = (value / 58) as u8;
            carry = value % 58;
        }
        encoded.push(ALPHABET[carry as usize]);
        while start < bytes.len() && bytes[start] == 0 {
            start += 1;
        }
    }
    let mut result = String::with_capacity(encoded.len() + leading_zeros);
    for _ in 0..leading_zeros {
        result.push('1');
    }
    for byte in encoded.into_iter().rev() {
        result.push(byte as char);
    }
    result
}

/// 開発用モック: データの sha256 から厳密形式を満たす CIDv0 を生成する。
/// multihash = sha2-256(0x12) + 長さ 32(0x20) + digest 32 バイトを base58btc 化する。
/// 同じデータなら同じ CID、異なるデータなら異なる CID になる。
fn mock_cid_for_data(data: &[u8]) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(data);
    let mut multihash = Vec::with_capacity(34);
    multihash.push(0x12);
    multihash.push(0x20);
    multihash.extend_from_slice(&digest[..]);
    base58btc_encode(&multihash)
}

pub async fn pin_to_ipfs(
    state: &AppState,
    data: Vec<u8>,
    filename: &str,
) -> Result<String, AppError> {
    let pinata_jwt = std::env::var("PINATA_JWT").unwrap_or_else(|_| "mock_jwt".to_string());

    if pinata_jwt == "mock_jwt" {
        return Ok(mock_cid_for_data(&data));
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
    // R5-01: 任意メタデータ。未指定時は空文字列として保存する。
    let mut title: Option<String> = None;
    let mut description: Option<String> = None;
    let mut tags: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {}", e)))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if !matches!(
            name.as_str(),
            "file" | "owner_hash" | "signature" | "price" | "title" | "description" | "tags"
        ) {
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
                // 注文側の上限（MAX_ORDER_JPY）と揃え、注文不可能な価格の登録を防ぐ。
                if price > payments::MAX_ORDER_JPY as i64 {
                    return Err(AppError::BadRequest(format!(
                        "price must not exceed {}",
                        payments::MAX_ORDER_JPY
                    )));
                }
            }
            "title" => {
                if title.is_some() {
                    return Err(AppError::BadRequest("Duplicate title field".to_string()));
                }
                title = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Invalid title: {}", e)))?,
                );
            }
            "description" => {
                if description.is_some() {
                    return Err(AppError::BadRequest(
                        "Duplicate description field".to_string(),
                    ));
                }
                description =
                    Some(field.text().await.map_err(|e| {
                        AppError::BadRequest(format!("Invalid description: {}", e))
                    })?);
            }
            "tags" => {
                if tags.is_some() {
                    return Err(AppError::BadRequest("Duplicate tags field".to_string()));
                }
                tags = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Invalid tags: {}", e)))?,
                );
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

    // メタデータは外部へのピン留め前に検証し、超過時は無駄な IPFS 呼び出しを避ける。
    let title = title.unwrap_or_default();
    let description = description.unwrap_or_default();
    let parsed_tags = parse_tags_field(tags.as_deref().unwrap_or(""));
    validate_content_metadata(&title, &description, &parsed_tags)?;
    let stored_tags = parsed_tags.join(",");

    // 1. Verify Signature (dev mock: 256-byte all-zero signature when ALLOW_MOCK_AUTH)
    if signature.is_empty() {
        return Err(AppError::BadRequest("Signature is empty".to_string()));
    }
    let mock_sig = state.allow_mock_auth && is_mock_signature(&signature);
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

    let (_write_guard, tx) = begin_write_tx(&state.db).await?;

    // The same bytes resolve to the same CID and contents is keyed by CID,
    // so a re-upload would otherwise surface as a primary-key 500.
    let mut existing = tx
        .query("SELECT 1 FROM contents WHERE cid = ?", [cid.clone()])
        .await?;
    if existing.next().await?.is_some() {
        return Err(AppError::BadRequest("Content already exists".to_string()));
    }

    tx.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, price, title, description, tags)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        (
            cid.clone(),
            content_kind.to_string(),
            owner_hash.clone(),
            checksum,
            price,
            title,
            description,
            stored_tags,
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
    // 期限付き BAN は ban_until が現在時刻を過ぎた時点で通過させる。
    // NULL は無期限なので従来どおり BAN 継続。時刻比較は SQLite の
    // datetime('now') に任せ、RFC3339 と SQL datetime の書式差に依存しない。
    let mut rows = db
        .query(
            "SELECT is_banned, ban_until FROM users
             WHERE user_hash = ?
               AND is_banned = TRUE
               AND (ban_until IS NULL OR ban_until > datetime('now'))",
            [user_hash],
        )
        .await?;
    if rows.next().await?.is_some() {
        return Err(AppError::Forbidden("User is banned".to_string()));
    }
    Ok(())
}

// ---- R5-09: 透明性ダッシュボード（運用者限定の認証内集計） ----
//
// プライバシー設計: DB からは集計済みのカラム（月 / ステータス / アクション /
// 件数 / 定足数フラグ）だけを読み、CID・reason・user_hash は一切 SELECT しない。
// レスポンス型も同じホワイトリストで、移送可能なフィールドを型レベルで固定する。

/// `content_audit_log` の集計行（新ステータス・月・件数のみ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModAuditCountRow {
    /// UTC の `YYYY-MM`（`substr(created_at, 1, 7)`）。
    pub month: String,
    /// `content_audit_log.new_status` のホワイトリスト値。
    pub new_status: String,
    pub count: i64,
}

/// `governance_audit_log` の集計行（アクション・月・件数のみ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModGovernanceCountRow {
    /// UTC の `YYYY-MM`（`substr(created_at, 1, 7)`）。
    pub month: String,
    /// `governance_audit_log.action` のホワイトリスト値。
    pub action: String,
    pub count: i64,
}

/// 確定通報の集計行（月・定足数未達フラグ・件数のみ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModConclusionCountRow {
    /// UTC の `YYYY-MM`（`substr(concluded_at, 1, 7)`）。
    pub month: String,
    /// 投票数が定足数（3）未達で棄却された確定かどうか。
    pub quorum_not_met: bool,
    pub count: i64,
}

/// 全体集計。 snake_case のフィールド名がそのまま JSON キーになる。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModStatsTotals {
    pub reports: i64,
    pub concluded: i64,
    pub approved: i64,
    pub rejected: i64,
    pub quorum_not_met: i64,
    pub bans: i64,
    pub unbans: i64,
    pub unpins: i64,
}

/// 月次集計。 `quorum_not_met` は共通インターフェース上 totals のみに存在する。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModStatsMonthly {
    pub month: String,
    pub reports: i64,
    pub concluded: i64,
    pub approved: i64,
    pub rejected: i64,
    pub bans: i64,
    pub unbans: i64,
    pub unpins: i64,
}

/// `GET /mod/stats` のレスポンス。機微情報（CID / reason / user_hash）は型に存在しない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModStatsResponse {
    /// RFC 3339（UTC）。
    pub generated_at: String,
    pub totals: ModStatsTotals,
    pub monthly: Vec<ModStatsMonthly>,
}

/// 月次エントリを取得（または既定値で作成）する。
fn mod_stats_month<'a>(
    monthly: &'a mut BTreeMap<String, ModStatsMonthly>,
    month: &str,
) -> &'a mut ModStatsMonthly {
    monthly
        .entry(month.to_string())
        .or_insert_with(|| ModStatsMonthly {
            month: month.to_string(),
            ..Default::default()
        })
}

/// DB 依存を持たない集計ロジック。
///
/// - `new_status` / `action` はホワイトリスト方式で、未知の値は無視する。
/// - 定足数未達の確定は ADR-0004 により `new_status='Approved'` として監査に残る。
///   そのため `approved` から差し引き、`quorum_not_met` として別枠で計上する
///   （同数は `Rejected` なので `rejected` にそのまま計上される）。
/// - 月は入力行の `YYYY-MM` をそのまま使い、UTC 境界の再解釈は行わない。
/// - 月次は月の昇順で返す。
pub fn build_mod_stats(
    generated_at: &str,
    audits: &[ModAuditCountRow],
    governance: &[ModGovernanceCountRow],
    conclusions: &[ModConclusionCountRow],
) -> ModStatsResponse {
    let mut totals = ModStatsTotals::default();
    let mut monthly: BTreeMap<String, ModStatsMonthly> = BTreeMap::new();

    for row in audits {
        let count = row.count.max(0);
        match row.new_status.as_str() {
            "Reported" => {
                totals.reports += count;
                mod_stats_month(&mut monthly, &row.month).reports += count;
            }
            "Approved" => {
                totals.approved += count;
                mod_stats_month(&mut monthly, &row.month).approved += count;
            }
            "Rejected" => {
                totals.rejected += count;
                mod_stats_month(&mut monthly, &row.month).rejected += count;
            }
            _ => {}
        }
    }

    for row in governance {
        let count = row.count.max(0);
        match row.action.as_str() {
            "BAN" => {
                totals.bans += count;
                mod_stats_month(&mut monthly, &row.month).bans += count;
            }
            "UNBAN" => {
                totals.unbans += count;
                mod_stats_month(&mut monthly, &row.month).unbans += count;
            }
            "UNPIN" => {
                totals.unpins += count;
                mod_stats_month(&mut monthly, &row.month).unpins += count;
            }
            _ => {}
        }
    }

    for row in conclusions {
        let count = row.count.max(0);
        totals.concluded += count;
        mod_stats_month(&mut monthly, &row.month).concluded += count;
        if row.quorum_not_met {
            totals.quorum_not_met += count;
            totals.approved = totals.approved.saturating_sub(count);
            let entry = mod_stats_month(&mut monthly, &row.month);
            entry.approved = entry.approved.saturating_sub(count);
        }
    }

    ModStatsResponse {
        generated_at: generated_at.to_string(),
        totals,
        monthly: monthly.into_values().collect(),
    }
}

/// `content_audit_log` から集計行だけを読み出す（CID / reason / changed_by は SELECT しない）。
async fn load_mod_audit_counts(db: &Connection) -> Result<Vec<ModAuditCountRow>, AppError> {
    let mut rows = db
        .query(
            "SELECT substr(created_at, 1, 7) AS month, new_status, COUNT(*) AS cnt
             FROM content_audit_log
             GROUP BY month, new_status",
            (),
        )
        .await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(ModAuditCountRow {
            month: row.get(0)?,
            new_status: row.get(1)?,
            count: row.get(2)?,
        });
    }
    Ok(result)
}

/// `governance_audit_log` から集計行だけを読み出す（target / actor_hash / reason は SELECT しない）。
async fn load_mod_governance_counts(
    db: &Connection,
) -> Result<Vec<ModGovernanceCountRow>, AppError> {
    let mut rows = db
        .query(
            "SELECT substr(created_at, 1, 7) AS month, action, COUNT(*) AS cnt
             FROM governance_audit_log
             GROUP BY month, action",
            (),
        )
        .await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(ModGovernanceCountRow {
            month: row.get(0)?,
            action: row.get(1)?,
            count: row.get(2)?,
        });
    }
    Ok(result)
}

/// 確定通報を月・定足数未達フラグ別に集計する。
/// reason / target_cid / voter_hash は SELECT せず、投票数（COUNT）だけを使う。
/// 定足数判定は `process_report_conclusion` と同じく BAN 中投票者の票を除外する。
async fn load_mod_conclusion_counts(
    db: &Connection,
) -> Result<Vec<ModConclusionCountRow>, AppError> {
    let mut rows = db
        .query(
            &format!(
                "SELECT substr(r.concluded_at, 1, 7) AS month,
                        CASE WHEN COALESCE(vc.vote_count, 0) < 3 THEN 1 ELSE 0 END AS quorum_not_met,
                        COUNT(*) AS cnt
                 FROM reports r
                 LEFT JOIN (
                     SELECT v.report_id, COUNT(*) AS vote_count
                     FROM votes v
                     LEFT JOIN users u ON u.user_hash = v.voter_hash
                     WHERE {VOTE_NOT_FROM_BANNED_VOTER_SQL}
                     GROUP BY v.report_id
                 ) vc ON vc.report_id = r.report_id
                 WHERE r.concluded_at IS NOT NULL
                 GROUP BY month, quorum_not_met"
            ),
            (),
        )
        .await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        let quorum_not_met: i64 = row.get(1)?;
        result.push(ModConclusionCountRow {
            month: row.get(0)?,
            quorum_not_met: quorum_not_met != 0,
            count: row.get(2)?,
        });
    }
    Ok(result)
}

/// R5-09: 透明性ダッシュボードの集計値を返す（公開 API ではない）。
/// 認証済みかつ設定済みの運用者だけが参照でき、未設定・未登録・BAN 済みは 403 とする。
pub async fn mod_stats(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
) -> Result<Json<ModStatsResponse>, AppError> {
    require_moderation_operator(&state, &claims.sub).await?;

    let audits = load_mod_audit_counts(&state.db).await?;
    let governance = load_mod_governance_counts(&state.db).await?;
    let conclusions = load_mod_conclusion_counts(&state.db).await?;

    Ok(Json(build_mod_stats(
        &chrono::Utc::now().to_rfc3339(),
        &audits,
        &governance,
        &conclusions,
    )))
}

pub fn validate_age(
    policy: &common::age::AgePolicy,
    birth_date: &str,
    country_code: &str,
) -> Result<(), AppError> {
    let today = chrono::Utc::now().date_naive();
    common::age::is_age_allowed_with_policy(policy, birth_date, country_code, today).map_err(|e| {
        match e {
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
            common::age::AgeValidationError::ConfigurationRequired => AppError::Forbidden(
                "Age policy requires explicit operator configuration".to_string(),
            ),
            common::age::AgeValidationError::TrustedAttributeRequired => AppError::Forbidden(
                "Trusted age attribute verification is required by policy, but no verified attribute input is available"
                    .to_string(),
            ),
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

/// 任意メタデータの上限。文字数ではなく UTF-8 バイト数で検査する。
pub const MAX_CONTENT_TITLE_BYTES: usize = 200;
pub const MAX_CONTENT_DESCRIPTION_BYTES: usize = 2000;
pub const MAX_CONTENT_TAGS: usize = 10;
pub const MAX_CONTENT_TAG_BYTES: usize = 50;

/// カンマ区切りの `tags` フィールドを分割する。空要素は捨て、前後の空白を除去する。
/// クライアントが空文字を送った場合は空ベクトルになる。
pub fn parse_tags_field(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// title / description / tags の長さ・個数を検証する。超過は 400。
pub fn validate_content_metadata(
    title: &str,
    description: &str,
    tags: &[String],
) -> Result<(), AppError> {
    if title.len() > MAX_CONTENT_TITLE_BYTES {
        return Err(AppError::BadRequest(format!(
            "title must not exceed {MAX_CONTENT_TITLE_BYTES} bytes"
        )));
    }
    if description.len() > MAX_CONTENT_DESCRIPTION_BYTES {
        return Err(AppError::BadRequest(format!(
            "description must not exceed {MAX_CONTENT_DESCRIPTION_BYTES} bytes"
        )));
    }
    if tags.len() > MAX_CONTENT_TAGS {
        return Err(AppError::BadRequest(format!(
            "tags must not contain more than {MAX_CONTENT_TAGS} items"
        )));
    }
    if tags.iter().any(|tag| tag.len() > MAX_CONTENT_TAG_BYTES) {
        return Err(AppError::BadRequest(format!(
            "each tag must not exceed {MAX_CONTENT_TAG_BYTES} bytes"
        )));
    }
    Ok(())
}

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

/// R5-04: `GET /me/export`。認証済みのデータ主体が自身の全データを JSON で取得する。
///
/// レスポンス本文はログへ出力しない。ダウンロード用途のため
/// `Content-Disposition: attachment` と `Cache-Control: no-store` を付与する。
pub async fn me_export(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
) -> Result<Response, AppError> {
    let sub = claims.sub.as_str();
    let db = state.db.as_ref();

    let export = DataExport {
        version: 1,
        generated_at: chrono::Utc::now().to_rfc3339(),
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
    };

    let mut response = Json(export).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"openillustration-export.json\""),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[cfg(test)]
mod fuzz_test;
#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests;
