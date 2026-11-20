// Guild Backend library
mod jpki;
mod jwt;
mod migration;
mod network;
mod oidc;
mod payments;

// 設定診断 CLI（bin/doctor.rs）と時刻検証で同じ定数を参照するための再エクスポート。
pub use oidc::{
    CLOCK_SKEW_MAX_SECS, CLOCK_SKEW_MIN_SECS, DEFAULT_CLOCK_SKEW_SECS, is_valid_oidc_endpoint,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, FromRef, MatchedPath, Multipart, Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    middleware::from_fn,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use common::Claims;
use libsql::{Builder, Connection};
use reqwest::Client;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{Span, warn};
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
    /// 周期 finalizer の tick 実行回数。
    pub finalizer_ticks: std::sync::atomic::AtomicU64,
    /// finalizer / 周期 purge が回収・確定したレコード件数。値や CID は保持しない。
    pub finalizer_processed: std::sync::atomic::AtomicU64,
    /// write_lock がビジーで回収をスキップした回数。
    pub finalizer_lock_busy: std::sync::atomic::AtomicU64,
    /// purge / 自動確定が失敗した回数。
    pub finalizer_failures: std::sync::atomic::AtomicU64,
    /// 直近の回収処理が成功した時刻（Unix秒）。未実行なら 0。
    pub finalizer_last_success_secs: std::sync::atomic::AtomicU64,
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
/// フレーム内へ埋め込むことを防ぐ X-Frame-Options（DENY）、および
/// API レスポンスへ実害のない CSP（既定全拒否・base-uri / form-action /
/// frame-ancestors を 'none' に固定）を付与する。HTML 配信の CSP は
/// 配信側担当とし、ここでは API レスポンスにのみ適用する。
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
    if !headers.contains_key("Content-Security-Policy") {
        headers.insert(
            "Content-Security-Policy",
            HeaderValue::from_static(
                "default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            ),
        );
    }
    response
}

/// リクエストごとの相関 ID。`Request::extensions` と task-local の両方に格納する。
#[derive(Clone, Copy, Debug)]
pub(crate) struct RequestId(pub(crate) Uuid);

tokio::task_local! {
    /// 現在処理中のリクエスト相関 ID。Internal エラーログから参照する。
    static REQUEST_ID: Uuid;
}

/// 相関 ID のレスポンスヘッダ名（大小文字を問わず比較される）。
const REQUEST_ID_HEADER: &str = "x-request-id";

/// 全レスポンスへ `x-request-id` を付与し、機微値を含まないアクセスログを JSON で出力する。
///
/// ログ項目は method / （テンプレート化した）path / status / duration_ms / request_id に限定し、
/// 生クエリ・生 CID・トークンは一切記録しない。`observe_metrics` の内側・CorsLayer の
/// 外側に置くため、プリフライトや内側ミドルウェアの短絡レスポンスにもヘッダが付く。
async fn request_id_and_access_log(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let request_id = Uuid::new_v4();
    let method = request.method().clone();
    // ルーティング済みならテンプレートパス（例: /content/{cid}）を使い、生 CID を出さない。
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_else(|| request.uri().path().to_owned());

    request.extensions_mut().insert(RequestId(request_id));
    // extensions に格納した値をヘッダの正規ソースとして使う（task-local との不一致を検出）。
    let header_request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|value| value.0)
        .unwrap_or(request_id);
    debug_assert_eq!(header_request_id, request_id);

    let started = Instant::now();
    let mut response = REQUEST_ID
        .scope(request_id, async { next.run(request).await })
        .await;
    let status = response.status();
    tracing::info!(
        method = %method,
        path = %path,
        status = u64::from(status.as_u16()),
        duration_ms = started.elapsed().as_millis() as u64,
        request_id = %request_id,
        "access"
    );
    response.headers_mut().insert(
        REQUEST_ID_HEADER,
        HeaderValue::from_str(&header_request_id.to_string())
            .expect("UUID string is always a valid header value"),
    );
    response
}

/// TraceLayer の `on_response` コールバック。スパンの status のみを記録し、
/// 追加のログイベントは出さない（アクセスログは上記ミドルウェアが担当）。
fn record_response_status(response: &axum::response::Response, _latency: Duration, span: &Span) {
    span.record("status", u64::from(response.status().as_u16()));
}

/// リクエスト数と 4xx/5xx 応答数を記録するミドルウェア。
/// Router の最も外側に積むことで、CorsLayer が短絡する OPTIONS 204 や
/// 内側のミドルウェアが返す 429 / 403 / 413 も取りこぼさず計上する。
async fn observe_metrics(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // 監視用の /health, /ready, /metrics は GET スクレイプだけを除外する。
    // 非 GET（例: POST /metrics の 405）はスクレイプではないため requests / errors へ残す。
    // /ready の 503（readiness 障害）も GET スクレイプとして除外され errors から消えるが、
    // readiness の状態は /ready のレスポンス自体で監視する（新規メトリクスは追加しない）。
    if request.method() == axum::http::Method::GET
        && matches!(request.uri().path(), "/health" | "/ready" | "/metrics")
    {
        return next.run(request).await;
    }
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
         openillustration_neutralized_urls_total {}\n\
         # HELP openillustration_finalizer_ticks_total Total moderation finalizer tick executions.\n\
         # TYPE openillustration_finalizer_ticks_total counter\n\
         openillustration_finalizer_ticks_total {}\n\
         # HELP openillustration_finalizer_processed_total Total records processed by the finalizer and periodic purges.\n\
         # TYPE openillustration_finalizer_processed_total counter\n\
         openillustration_finalizer_processed_total {}\n\
         # HELP openillustration_finalizer_lock_busy_total Total purge/finalize runs skipped due to a busy write lock.\n\
         # TYPE openillustration_finalizer_lock_busy_total counter\n\
         openillustration_finalizer_lock_busy_total {}\n\
         # HELP openillustration_finalizer_failures_total Total failed purge/finalize operations.\n\
         # TYPE openillustration_finalizer_failures_total counter\n\
         openillustration_finalizer_failures_total {}\n\
         # HELP openillustration_finalizer_last_success_timestamp_seconds Unix timestamp of the last successful finalizer/purge run.\n\
         # TYPE openillustration_finalizer_last_success_timestamp_seconds gauge\n\
         openillustration_finalizer_last_success_timestamp_seconds {}\n",
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
        state
            .metrics
            .finalizer_ticks
            .load(std::sync::atomic::Ordering::Relaxed),
        state
            .metrics
            .finalizer_processed
            .load(std::sync::atomic::Ordering::Relaxed),
        state
            .metrics
            .finalizer_lock_busy
            .load(std::sync::atomic::Ordering::Relaxed),
        state
            .metrics
            .finalizer_failures
            .load(std::sync::atomic::Ordering::Relaxed),
        state
            .metrics
            .finalizer_last_success_secs
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

/// ロックを保持したままトランザクションを開始する（libsql::Error をそのまま返す版）。
///
/// すべての書き込みはこの関数を通す。共有 `Connection` で `BEGIN` を保持できるのは
/// 1 つだけなので、`BEGIN` の前に `write_lock` を取得し、コミット後にガードを
/// ドロップしてロックを解放する。ガードを先に落とすと次の書き込みが
/// このトランザクションへ取り込まれるため、順序を逆転させないこと。
async fn begin_write_tx_sql(
    db: &Connection,
) -> Result<(tokio::sync::OwnedMutexGuard<()>, libsql::Transaction), libsql::Error> {
    let guard = write_lock(db).lock_owned().await;
    begin_write_tx_sql_with_guard(db, guard).await
}

/// 取得済みの write_lock ガードを保持したまま `BEGIN` する。
/// 周期 purge では「ロック取得だけをタイムアウト付きで待ち、SQL は完了まで待つ」ために使う。
async fn begin_write_tx_sql_with_guard(
    db: &Connection,
    guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<(tokio::sync::OwnedMutexGuard<()>, libsql::Transaction), libsql::Error> {
    let tx = db.transaction().await?;
    Ok((guard, tx))
}

/// purge / 自動確定の共通 write_lock 取得。
/// `Some(wait)` のときだけロック取得をタイムアウトし、ビジーなら `Ok(None)` でスキップを返す。
/// `None` は呼び出し側が上限を設けない場合（公開 API の通し実行）に限り完了まで待つ。
/// SQL 実行はこの関数の外側でタイムアウトなしで行う。
async fn acquire_purge_lock(
    db: &Connection,
    wait: Option<Duration>,
) -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, libsql::Error> {
    let lock = write_lock(db);
    let guard = match wait {
        Some(wait) => match tokio::time::timeout(wait, lock.lock_owned()).await {
            Ok(guard) => guard,
            Err(_) => return Ok(None),
        },
        None => lock.lock_owned().await,
    };
    Ok(Some(guard))
}

/// `begin_write_tx_sql` を `AppError` 側の API へ変換したラッパー。
/// ガードは戻り値に保持させ、コミット後にドロップする。
async fn begin_write_tx(
    db: &Connection,
) -> Result<(tokio::sync::OwnedMutexGuard<()>, libsql::Transaction), AppError> {
    begin_write_tx_sql(db)
        .await
        .map_err(|e| AppError::Internal(format!("Transaction start failed: {}", e)))
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
    /// 上流（IPFS ゲートウェイ等）の応答が完全性検証に失敗した場合。
    /// 認証エラー（401）と混同しないよう、固定文言の 502 として返す。
    #[error("Upstream content failed integrity verification")]
    BadGateway,
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
            AppError::BadGateway => (
                StatusCode::BAD_GATEWAY,
                "Upstream content failed integrity verification".to_string(),
            ),
        };

        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

/// 内部エラーの詳細をクライアントへ公開するか。未設定時は一般化メッセージを返す。
/// 開発時は `EXPOSE_INTERNAL_ERRORS=1` または `true`（大文字小文字不問）で詳細を確認できる。
fn expose_internal_errors() -> bool {
    std::env::var("EXPOSE_INTERNAL_ERRORS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 500系レスポンスのボディを決定する。詳細は常にログへ出し、公開はフラグに従う。
fn internal_error_body(details: String) -> String {
    // ミドルウェアが張った task-local から相関 ID を取る。
    // 直接呼び出された場合などスコープ外では ID なしでログする。
    match REQUEST_ID.try_with(|id| *id) {
        Ok(id) => tracing::error!(request_id = %id, "Internal error: {}", details),
        Err(_) => tracing::error!("Internal error: {}", details),
    }
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
        // 空白・タブ・改行のみの値も空とみなし fail-closed（値はエコーしない）。
        Some(v) if v.trim().is_empty() => errors.push("PINATA_JWT is empty"),
        // 前後の空白付きモック値も開発用として拒否（値はエコーしない）。
        Some(v) if v.trim() == "mock_jwt" => {
            errors.push("PINATA_JWT is the development mock value")
        }
        Some(_) => {}
    }
    errors
}

/// 本番・テスト共通の HTTP クライアントを組み立てる。
///
/// `Policy::none()` で全リダイレクトを拒否する。IPFS ゲートウェイ（および
/// OIDC エンドポイント）が 3xx でクラウドメタデータ等の内部アドレスへ
/// リダイレクトしても、CID 検証やゲートウェイ URL 検証をすり抜けて
/// 内部へ到達しないようにするための必須設定である。
/// 3xx は追従されず、そのままレスポンスとして呼び出し側へ返る。
pub(crate) fn build_http_client(timeout: Duration) -> reqwest::Result<Client> {
    Client::builder()
        .timeout(timeout)
        .redirect(Policy::none())
        .build()
}

/// SQLite/libSQL の外部キー制約をコネクション単位で有効化する。
///
/// `PRAGMA foreign_keys` はコネクションごとの設定で、トランザクション中は変更が
/// 無効になる。そのため migration 実行前に呼び出すこと。リモート DB では
/// 接続単位で効かないため、呼び出し側で warn を出す（`run()` を参照）。
pub(crate) async fn enable_foreign_keys(db: &Connection) -> anyhow::Result<()> {
    db.execute("PRAGMA foreign_keys = ON", ())
        .await
        .map_err(|e| anyhow::anyhow!("failed to enable SQLite foreign keys: {e}"))?;
    Ok(())
}

/// 参照先が存在しない外部キー（孤立行）を検出し、可読な文字列で返す。
///
/// 既存 DB に孤立行があっても起動は止めず、呼び出し側で warn のために使う。
pub(crate) async fn find_foreign_key_violations(db: &Connection) -> anyhow::Result<Vec<String>> {
    let mut rows = db
        .query("PRAGMA foreign_key_check", ())
        .await
        .map_err(|e| anyhow::anyhow!("failed to run PRAGMA foreign_key_check: {e}"))?;
    let mut violations = Vec::new();
    while let Some(row) = rows.next().await? {
        let table: String = row.get(0)?;
        let rowid: Option<i64> = row.get(1)?;
        let parent: String = row.get(2)?;
        violations.push(format!("{table}(rowid={rowid:?}) -> {parent}"));
    }
    Ok(violations)
}

pub async fn run() -> anyhow::Result<()> {
    init_tracing();

    let allow_mock_auth = std::env::var("ALLOW_MOCK_AUTH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    // `from_env()` は不正な OIDC 設定で panic するため、同じ環境変数読み取りを行う
    // `from_env_with()` を直接呼び、Err を anyhow エラーとして起動失敗を伝播させる。
    let oidc = oidc::OidcState::from_env_with(|name| std::env::var(name).ok())
        .map_err(|message| anyhow::anyhow!("invalid OIDC configuration: {message}"))?;
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
        let connection = Builder::new_local(path).build().await?.connect()?;
        // SQLite/libSQL の FK 制約はコネクション単位。migration より前に有効化する。
        // これにより 012_favorites の ON DELETE CASCADE などが本番でも効く。
        enable_foreign_keys(&connection).await?;
        connection
    } else {
        // リモート DB では PRAGMA がコネクション単位で効かないため、書き込み側で
        // 参照整合性を担保する必要がある（単一ライター警告とは別に明示する）。
        warn!(
            "remote DB configured: `PRAGMA foreign_keys = ON` is per-connection and not enforced; ensure referential integrity at the writer"
        );
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

    // FK 有効化後に孤立行が残っていないか検査する。既存 DB に孤立行があっても
    // 起動は止めず、明示的な warn で是正を促す（fail-closed は将来フェーズで検討）。
    match find_foreign_key_violations(&db).await {
        Ok(violations) if !violations.is_empty() => {
            warn!(
                "foreign key check found {} orphaned row(s): {}",
                violations.len(),
                violations.join("; ")
            );
        }
        Ok(_) => {}
        Err(e) => {
            warn!("foreign key check could not be completed: {e}");
        }
    }

    let verification_keys_env = std::env::var("JWT_RSA_VERIFICATION_KEYS").ok();
    let require_kid_env = std::env::var("JWT_REQUIRE_KID").ok();
    let require_kid = jwt_require_kid_from_env(require_kid_env.as_deref());
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
        http_client: build_http_client(oidc_http_timeout(
            std::env::var("OIDC_HTTP_TIMEOUT_SECS").ok().as_deref(),
        ))
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
    let worker_metrics = Arc::clone(&shared_state.metrics);
    // finalizer の JoinHandle 結果を検査する際に使う Metrics（shared_state は build_router へ move される）。
    let shutdown_metrics = Arc::clone(&shared_state.metrics);
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
        worker_metrics,
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
    match finalizer.await {
        Ok(()) => {}
        Err(error) => {
            tracing::error!("Moderation finalizer task failed: {}", error);
            shutdown_metrics
                .finalizer_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    // ロックファイルを削除して次の起動を許可する。
    if let Some(path) = instance_lock {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            // 既に削除済み（別プロセスが回収した等）は正常系として扱う。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(
                    "Failed to remove instance lock file {}: kind={:?}, error={}",
                    path,
                    error.kind(),
                    error
                );
            }
        }
    }
    Ok(())
}

/// SIGINT / SIGTERM を待つ。受信後は `axum::serve` が処理中リクエストの完了を待って停止する。
async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            // Ctrl-C を購読できない場合は SIGTERM のみで停止する（SIGTERM 側と対称）。
            tracing::warn!("Failed to register Ctrl-C signal handler; waiting for SIGTERM only");
            std::future::pending::<()>().await;
        }
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

/// 起動時警告へ出す ALLOWED_ORIGINS エントリのマスク表現。
/// IPFS 用の `redact_url` と同じ方針で生の設定値をログへ残さず、オリジン値そのものは
/// 出力しない（区分だけを返す）。
fn redact_origin(origin: &str) -> &'static str {
    if origin == "null" {
        "null-origin"
    } else if origin.starts_with("https://") {
        "https-origin"
    } else if origin.starts_with("http://") {
        "http-origin"
    } else {
        "no-scheme-entry"
    }
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
                        "Ignoring ALLOWED_ORIGINS entry without an http(s) scheme ({}; value redacted)",
                        redact_origin(origin)
                    );
                    return None;
                }
                match origin.parse::<HeaderValue>() {
                    Ok(parsed) => Some(parsed),
                    Err(_) => {
                        warn!(
                            "Ignoring invalid ALLOWED_ORIGINS entry ({}; value redacted)",
                            redact_origin(origin)
                        );
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

/// `JWT_REQUIRE_KID` の解釈。未設定・`"0"`・`"false"`（大小文字無視）のみ
/// false で、それ以外は true とする（従来の起動時挙動をそのまま抽出）。
pub fn jwt_require_kid_from_env(value: Option<&str>) -> bool {
    value.is_some_and(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
}

/// RS256 選択時に短い `JWT_SECRET` を警告すべきか。
/// ログを捕捉せずに警告条件を検証できるよう、判定だけを純粋関数に切り出す。
pub fn should_warn_short_jwt_secret(secret: Option<&str>) -> bool {
    secret.is_some_and(|secret| jwt::JwtKeys::from_hmac_secret(secret).is_err())
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
            if should_warn_short_jwt_secret(Some(&secret)) {
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
                        "Ignoring IPFS_GATEWAYS entry without a {{cid}} placeholder (value redacted)"
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
                            "Ignoring IPFS_GATEWAYS entry that is not an http(s) URL template (value redacted)"
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
/// ロック取得は完了まで待つ（呼び出し側で上限を設けたい場合は `finalize_due_reports_with_wait` を使う）。
pub async fn finalize_due_reports(db: &Connection) -> Result<usize, AppError> {
    let guard = write_lock(db).lock_owned().await;
    finalize_due_reports_with_guard(db, guard).await
}

/// 取得済みの write_lock ガードで期限切れ通報を確定する。
/// 一件ごとの BEGIN/COMMIT はガードを保持したまま回し、SQL はタイムアウトなしで完了まで待つ。
async fn finalize_due_reports_with_guard(
    db: &Connection,
    guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<usize, AppError> {
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
    let mut guard = guard;
    for report_id in report_ids {
        let (next_guard, tx) = begin_write_tx_sql_with_guard(db, guard).await?;
        guard = next_guard;
        process_report_conclusion(&tx, report_id).await?;
        tx.commit().await?;
        finalized += 1;
        // 多数の通報をまとめて確定する間も executor を譲り、停止通知や
        // タイマーが進めるようにする（ロックガードは保持したまま）。
        tokio::task::yield_now().await;
    }
    Ok(finalized)
}

/// purge と同じ 2 段構造で自動確定する。
/// `lock_wait` は write_lock 取得だけに適用し、取得後の SQL は完了まで待つ。
/// `Ok(None)` はロックがビジーでスキップしたことを表す（SELECT や COMMIT の遅延ではない）。
async fn finalize_due_reports_with_wait(
    db: &Connection,
    lock_wait: Option<Duration>,
) -> Result<Option<usize>, AppError> {
    let Some(guard) = acquire_purge_lock(db, lock_wait).await? else {
        return Ok(None);
    };
    finalize_due_reports_with_guard(db, guard).await.map(Some)
}

/// 周期タスクが書き込みロックの取得で待ち続けると、後続の書き込みも
/// ロック待ちのキューに並んで詰まるため、周期回収だけは短時間で諦める。
const PURGE_LOCK_WAIT: Duration = Duration::from_secs(2);

/// 起動時の回収も無期限に write_lock を待つと、同じプロセスのロック保持中に
/// `finalizer.await` が止まり起動・停止が固まるため上限を設ける。
/// タイムアウト時は warn を出して回収をスキップし起動を続行する。
const STARTUP_LOCK_WAIT: Duration = Duration::from_secs(30);

/// 期限切れリフレッシュトークンを削除する。期限切れレコードは認証に使えず、
/// 保持しても盗用検知の意味がないため、定期的に回収してDBの増大を防ぐ。
/// `lock_wait` は write_lock 取得だけに適用し、取得後の SQL は完了まで待つ。
/// `Ok(None)` はロックがビジーでスキップしたことを表す。
async fn purge_expired_refresh_tokens_with_wait(
    db: &Connection,
    lock_wait: Option<Duration>,
) -> Result<Option<u64>, libsql::Error> {
    let Some(guard) = acquire_purge_lock(db, lock_wait).await? else {
        return Ok(None);
    };
    // `expires_at` は chrono の to_rfc3339()（小数秒 0/3/6/9 桁・タイムゾーン表記は
    // Z / +HH:MM）で保存されるため、文字列の辞書順比較では表記の差で前後が逆転する。
    // ここでは SQLite の datetime() で両辺を UTC の SQL datetime に正規化して比較する。
    // Rust 側で全行を読んでパースする方式もあり得るが、書き込みロック保持中に
    // 全走査するのはコストが大きく、既存の ban 判定などと同じ SQL 側正規化を採用した
    // （lib.rs の ensure_user_not_banned と同じ方針）。秒未満の切り捨ては単調なので
    // 実際に期限切れのトークンが取り残されることはなく、高々 1 秒早く回収されるだけ。
    let now = chrono::Utc::now().to_rfc3339();
    let (_write_guard, tx) = begin_write_tx_sql_with_guard(db, guard).await?;
    tx.execute(
        "DELETE FROM refresh_tokens WHERE datetime(expires_at) <= datetime(?)",
        [now],
    )
    .await?;
    let deleted = tx.changes();
    tx.commit().await?;
    Ok(Some(deleted))
}

/// 周期用: ロック取得だけ `PURGE_LOCK_WAIT` を待ち、ビジーならスキップする。
async fn purge_expired_refresh_tokens(db: &Connection) -> Result<Option<u64>, libsql::Error> {
    purge_expired_refresh_tokens_with_wait(db, Some(PURGE_LOCK_WAIT)).await
}

/// 古い失効 sid を回収する。失効sid を持つ JWT は最大 SESSION_TTL_SECS で
/// 期限切れになるため、それより古いレコードを削除しても失効は復活しない。
/// リフレッシュ時は常に新しい sid が発行されるため再利用の心配もない。
async fn purge_stale_revoked_sessions_with_wait(
    db: &Connection,
    lock_wait: Option<Duration>,
) -> Result<Option<u64>, libsql::Error> {
    let Some(guard) = acquire_purge_lock(db, lock_wait).await? else {
        return Ok(None);
    };
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(oidc::SESSION_TTL_SECS + 60);
    let (_write_guard, tx) = begin_write_tx_sql_with_guard(db, guard).await?;
    tx.execute(
        "DELETE FROM revoked_sessions WHERE revoked_at <= ?",
        [cutoff.format("%Y-%m-%d %H:%M:%S").to_string()],
    )
    .await?;
    let deleted = tx.changes();
    tx.commit().await?;
    Ok(Some(deleted))
}

/// 周期用: ロック取得だけ `PURGE_LOCK_WAIT` を待ち、ビジーならスキップする。
async fn purge_stale_revoked_sessions(db: &Connection) -> Result<Option<u64>, libsql::Error> {
    purge_stale_revoked_sessions_with_wait(db, Some(PURGE_LOCK_WAIT)).await
}

/// 古い sub 失効閾値を回収する。閾値より前に発行された JWT は最大
/// SESSION_TTL_SECS で期限切れになるため、それより古い閾値は不要になる。
/// 新しい JWT は常に現在時刻より後ろの iat を持つため、失効は復活しない。
async fn purge_stale_revoked_subs_with_wait(
    db: &Connection,
    lock_wait: Option<Duration>,
) -> Result<Option<u64>, libsql::Error> {
    let Some(guard) = acquire_purge_lock(db, lock_wait).await? else {
        return Ok(None);
    };
    let cutoff = chrono::Utc::now().timestamp() - oidc::SESSION_TTL_SECS - 60;
    let (_write_guard, tx) = begin_write_tx_sql_with_guard(db, guard).await?;
    tx.execute("DELETE FROM revoked_subs WHERE revoked_at <= ?", [cutoff])
        .await?;
    let deleted = tx.changes();
    tx.commit().await?;
    Ok(Some(deleted))
}

/// 周期用: ロック取得だけ `PURGE_LOCK_WAIT` を待ち、ビジーならスキップする。
async fn purge_stale_revoked_subs(db: &Connection) -> Result<Option<u64>, libsql::Error> {
    purge_stale_revoked_subs_with_wait(db, Some(PURGE_LOCK_WAIT)).await
}

/// 1 回の回収処理が成功したことを finalizer メトリクスへ反映する。
/// 件数だけを加算し、レコードの値や CID は一切保持しない。
fn note_finalizer_success(metrics: &Metrics, processed: u64) {
    if processed > 0 {
        metrics
            .finalizer_processed
            .fetch_add(processed, std::sync::atomic::Ordering::Relaxed);
    }
    metrics.finalizer_last_success_secs.store(
        chrono::Utc::now().timestamp().max(0) as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// 起動中に動作する自動確定ワーカー。停止シグナルで終了する。
async fn run_moderation_finalizer(
    db: Arc<Connection>,
    oidc: Option<Arc<oidc::OidcState>>,
    interval: Duration,
    shutdown: tokio::sync::watch::Receiver<()>,
    metrics: Arc<Metrics>,
) {
    run_moderation_finalizer_with_startup_wait(
        db,
        oidc,
        interval,
        STARTUP_LOCK_WAIT,
        shutdown,
        metrics,
    )
    .await;
}

/// 本体。起動時ロック待ちの上限を差し替えられるようにしてある（テストは短い値で呼ぶ）。
async fn run_moderation_finalizer_with_startup_wait(
    db: Arc<Connection>,
    oidc: Option<Arc<oidc::OidcState>>,
    interval: Duration,
    startup_lock_wait: Duration,
    mut shutdown: tokio::sync::watch::Receiver<()>,
    metrics: Arc<Metrics>,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // 起動直後の即時 tick は消費する

    // 起動前に既に停止通知が出ていれば、ロック取得を試さずに終了する。
    if shutdown.borrow().has_changed() {
        tracing::info!("Moderation finalizer stopped before startup cleanup");
        return;
    }

    // 起動時回収は上限付きで待つ。停止通知が来たら待機を打ち切って起動を続行する。
    // 専用 clone の changed() を使うことで、周期ループが見る shutdown を消費しない。
    let mut startup_shutdown = shutdown.clone();
    tokio::select! {
        _ = startup_shutdown.changed() => {
            tracing::info!("Moderation finalizer stopped during startup cleanup");
            return;
        }
        _ = run_startup_cleanup(
            db.as_ref(),
            oidc.as_ref(),
            startup_lock_wait,
            metrics.as_ref(),
        ) => {}
    }

    // tick 中のロック待ちでも停止通知があれば即座に打ち切る。
    // 専用 clone を使い、外側のループが見る shutdown を消費しないようにする。
    let mut tick_shutdown = shutdown.clone();
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                metrics
                    .finalizer_ticks
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tokio::select! {
                    _ = run_periodic_cleanup(db.as_ref(), oidc.as_ref(), metrics.as_ref()) => {}
                    _ = tick_shutdown.changed() => {
                        tracing::info!("Moderation finalizer stopped");
                        break;
                    }
                }
            }
            _ = shutdown.changed() => {
                tracing::info!("Moderation finalizer stopped");
                break;
            }
        }
    }
}

/// 1 tick 分の周期回収。purge 群は既に 2 段構造で、
/// 自動確定もここで同じ 2 段構造にする。
async fn run_periodic_cleanup(
    db: &Connection,
    oidc: Option<&Arc<oidc::OidcState>>,
    metrics: &Metrics,
) {
    // 周期実行は purge 関数側で write_lock 取得だけを短時間待ち、
    // 取得できたら SQL はタイムアウトなしで完了させる（外側からは timeout で包まない）。
    match purge_expired_refresh_tokens(db).await {
        Ok(Some(0)) => note_finalizer_success(metrics, 0),
        Ok(Some(count)) => {
            note_finalizer_success(metrics, count);
            tracing::info!("Purged {} expired refresh token(s)", count);
        }
        Ok(None) => {
            metrics
                .finalizer_lock_busy
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Err(error) => {
            metrics
                .finalizer_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Refresh token purge failed: {}", error);
        }
    }
    match purge_stale_revoked_sessions(db).await {
        Ok(Some(0)) => note_finalizer_success(metrics, 0),
        Ok(Some(count)) => {
            note_finalizer_success(metrics, count);
            tracing::info!("Purged {} stale revoked session(s)", count);
        }
        Ok(None) => {
            metrics
                .finalizer_lock_busy
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Err(error) => {
            metrics
                .finalizer_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Revoked session purge failed: {}", error);
        }
    }
    match purge_stale_revoked_subs(db).await {
        Ok(Some(0)) => note_finalizer_success(metrics, 0),
        Ok(Some(count)) => {
            note_finalizer_success(metrics, count);
            tracing::info!("Purged {} stale revoked sub(s)", count);
        }
        Ok(None) => {
            metrics
                .finalizer_lock_busy
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Err(error) => {
            metrics
                .finalizer_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Revoked sub purge failed: {}", error);
        }
    }
    // 認証リクエストが来ないままだと register/take の掃除が走らないため、
    // 周期 tick でも期限切れ pending を回収する。
    if let Some(oidc) = oidc {
        let count = oidc.sweep_expired_pending();
        if count > 0 {
            tracing::info!("Purged {} expired OIDC pending authorization(s)", count);
        }
        note_finalizer_success(metrics, count as u64);
    }
    // 周期の自動確定も purge 群と同じ 2 段構造:
    // ロック取得だけ PURGE_LOCK_WAIT で待ち、取得後は SELECT/COMMIT を
    // タイムアウトなしで件数分すべて完了させる。
    // "busy" はロック取得を諦めたときだけ警告し、SQL の遅延を誤報しない。
    match finalize_due_reports_with_wait(db, Some(PURGE_LOCK_WAIT)).await {
        Ok(Some(0)) => note_finalizer_success(metrics, 0),
        Ok(Some(count)) => {
            note_finalizer_success(metrics, count as u64);
            tracing::info!("Auto-finalized {} moderation report(s)", count);
        }
        Ok(None) => {
            metrics
                .finalizer_lock_busy
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Moderation finalization skipped: write lock is busy");
        }
        Err(error) => {
            metrics
                .finalizer_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Moderation finalization failed: {}", error);
        }
    }
}

/// 起動時だけの回収。ロック取得は `lock_wait` で上限を設け、
/// ビジーなら warn を出して次へ進み、起動を止めない。
async fn run_startup_cleanup(
    db: &Connection,
    oidc: Option<&Arc<oidc::OidcState>>,
    lock_wait: Duration,
    metrics: &Metrics,
) {
    // 起動時に一度、期限切れリフレッシュトークンを回収する。
    match purge_expired_refresh_tokens_with_wait(db, Some(lock_wait)).await {
        Ok(Some(0)) => note_finalizer_success(metrics, 0),
        Ok(Some(count)) => {
            note_finalizer_success(metrics, count);
            tracing::info!("Purged {} expired refresh token(s) on startup", count);
        }
        Ok(None) => {
            metrics
                .finalizer_lock_busy
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Refresh token purge skipped on startup: write lock is busy")
        }
        Err(error) => {
            metrics
                .finalizer_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Refresh token purge failed on startup: {}", error);
        }
    }
    match purge_stale_revoked_sessions_with_wait(db, Some(lock_wait)).await {
        Ok(Some(0)) => note_finalizer_success(metrics, 0),
        Ok(Some(count)) => {
            note_finalizer_success(metrics, count);
            tracing::info!("Purged {} stale revoked session(s) on startup", count);
        }
        Ok(None) => {
            metrics
                .finalizer_lock_busy
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Revoked session purge skipped on startup: write lock is busy")
        }
        Err(error) => {
            metrics
                .finalizer_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Revoked session purge failed on startup: {}", error);
        }
    }
    match purge_stale_revoked_subs_with_wait(db, Some(lock_wait)).await {
        Ok(Some(0)) => note_finalizer_success(metrics, 0),
        Ok(Some(count)) => {
            note_finalizer_success(metrics, count);
            tracing::info!("Purged {} stale revoked sub(s) on startup", count);
        }
        Ok(None) => {
            metrics
                .finalizer_lock_busy
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Revoked sub purge skipped on startup: write lock is busy")
        }
        Err(error) => {
            metrics
                .finalizer_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Revoked sub purge failed on startup: {}", error);
        }
    }
    // pending はメモリ上にあり再起動時は空だが、register/take 以外の経路でも
    // 期限切れを回収できるよう起動時にも sweep しておく。
    if let Some(oidc) = oidc {
        let count = oidc.sweep_expired_pending();
        if count > 0 {
            tracing::info!(
                "Purged {} expired OIDC pending authorization(s) on startup",
                count
            );
        }
        note_finalizer_success(metrics, count as u64);
    }

    // 最初の tick まで待つと再起動直後の期限切れ通報が最大 interval のあいだ
    // 未確定のまま残るため、起動時にも一度だけ確定処理を走らせる。
    // ロック待ちは上限付きで、取得後は件数分を完了まで処理する。
    match finalize_due_reports_with_wait(db, Some(lock_wait)).await {
        Ok(Some(0)) => note_finalizer_success(metrics, 0),
        Ok(Some(count)) => {
            note_finalizer_success(metrics, count as u64);
            tracing::info!("Auto-finalized {} moderation report(s) on startup", count)
        }
        Ok(None) => {
            metrics
                .finalizer_lock_busy
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Moderation finalization skipped on startup: write lock is busy")
        }
        Err(error) => {
            metrics
                .finalizer_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("Moderation finalization failed on startup: {}", error);
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

    // TraceLayer のスパン属性は method / path / status のみ。path はルーティングの
    // テンプレートパスを使い、生 CID やクエリは積まない。既定のイベントは
    // request_id_and_access_log 側へ一本化するため無効化する（OPTIONS も通過する）。
    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(|request: &axum::extract::Request| {
            let path = request
                .extensions()
                .get::<MatchedPath>()
                .map(|matched| matched.as_str())
                .unwrap_or_else(|| request.uri().path());
            tracing::info_span!(
                "http_request",
                method = %request.method(),
                path = %path,
                status = tracing::field::Empty,
            )
        })
        .on_request(())
        .on_response(record_response_status)
        .on_failure(())
        .on_body_chunk(())
        .on_eos(());

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
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::DELETE,
                ])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]),
        )
        // CORS プリフライトは CorsLayer で短絡し内側の security_headers を通らないため、
        // CorsLayer の外側にもう一段置く。これで 429/403/500/204 に加え
        // プリフライト応答にも nosniff / DENY が付く。二重付与でも同値のため結果は変わらない。
        .layer(from_fn(security_headers))
        // 相関 ID 付与とアクセスログ。CorsLayer の外側なのでプリフライトや
        // 429 / 403 / 413 の短絡レスポンスにも x-request-id が付く。
        .layer(from_fn(request_id_and_access_log))
        // リクエストスパン（属性は method/path/status のみ）。observe_metrics の内側。
        .layer(trace_layer)
        // メトリクスは最も外側で観測する。OPTIONS 204 や 429 / 403 / 413 は
        // 内側のミドルウェアで短絡するため、ここで初めて全レスポンスを捕捉できる。
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            observe_metrics,
        ))
        .with_state(state)
}

/// GET /mod/reports の取得上限。全行を Vec 化しないよう SQL の LIMIT にバインドする。
pub const MAX_REPORT_LIST: i64 = 500;
/// GET /content/{cid}/audit の取得上限。全行を Vec 化しないよう SQL の LIMIT にバインドする。
pub const MAX_AUDIT_LIST: i64 = 500;

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
             ORDER BY r.created_at DESC
             LIMIT ?",
            libsql::params![MAX_REPORT_LIST],
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

    // 全行を Vec 化しないよう取得件数を MAX_AUDIT_LIST に制限し、
    // 最新 500 件を新しい順（log_id DESC）で返す。
    // 古い行は API からは参照できないため、完全な検証には署名付きエクスポート / DB を使用する。
    // なお本レスポンスは previous_log_hash を含まず reason もマスクされるため、
    // レスポンス単体でのハッシュ連鎖検証は成立しない。
    let mut rows = state
        .db
        .query(
            "SELECT log_id, cid, old_status, new_status, changed_by, reason, created_at, entry_hash 
             FROM content_audit_log WHERE cid = ? ORDER BY log_id DESC
             LIMIT ?",
            libsql::params![cid, MAX_AUDIT_LIST],
        )
        .await?;

    let mut logs = Vec::new();
    while let Some(row) = rows.next().await? {
        // DB には全文を保持したまま、表示用のレスポンスには要約だけを返す。
        // ハッシュ検証に使う entry_hash は DB の生値をそのまま返し、変更しない。
        let reason: Option<String> = row.get(5)?;
        logs.push(AuditLogResponse {
            log_id: row.get(0)?,
            cid: row.get(1)?,
            old_status: row.get(2)?,
            new_status: row.get(3)?,
            changed_by: row.get(4)?,
            reason: reason.map(|text| common::summarize_reason(&text)),
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

    // 共有 Connection への INSERT は write_lock 配下の tx で行う。
    // ロック外の autocommit 書き込みは他トランザクションの BEGIN に取り込まれ、
    // その ROLLBACK でユーザー登録だけ失われる可能性がある。
    let (_write_guard, tx) = begin_write_tx(&state.db).await?;
    if let Some(der) = verified_key {
        tx.execute(
            "INSERT INTO users (user_hash, public_key_der) VALUES (?, ?) ON CONFLICT(user_hash) DO UPDATE SET public_key_der = excluded.public_key_der",
            (user_hash.clone(), der)
        ).await?;
    } else {
        tx.execute(
            "INSERT OR IGNORE INTO users (user_hash) VALUES (?)",
            [user_hash.clone()],
        )
        .await?;
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    // 下流の delete_superseded_revocation が同じ write_lock を取得するため、
    // ここで明示的に解放しないと自己デッドロックする。
    drop(_write_guard);

    let now = chrono::Utc::now();
    let now_ts = now.timestamp() as usize;
    let expiration = now
        .checked_add_signed(chrono::Duration::hours(24))
        .expect("valid timestamp")
        .timestamp() as usize;

    let (iat, old_threshold) = {
        let mut revoked_subs = state
            .revoked_subs_before
            .lock()
            .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?;
        // 新しい認証セッションの発行により、過去セッション失効の閾値は不要になる。
        let old = revoked_subs.remove(&user_hash);
        let issued_at = match old {
            Some(threshold) => std::cmp::max(now_ts, threshold.saturating_add(1)),
            None => now_ts,
        };
        (issued_at, old)
    };
    // OIDC callback と同じ順序で、メモリ削除の後に DB の旧閾値も削除する。
    // これを省くと再起動後に refresh が古い閾値で塞がる。
    if let Some(old_threshold) = old_threshold {
        oidc::delete_superseded_revocation(&state.db, &user_hash, old_threshold as i64).await?;
    }

    let claims = Claims {
        sub: user_hash.clone(),
        exp: expiration,
        // JPKI 発行セッションにも sid を付与し、/auth/logout が
        // revoke_session を呼べるようにする（refresh は発行しないため
        // Cookie ファミリ失効は不要）。
        sid: Some(Uuid::new_v4().to_string()),
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
    // 書き込みは write_lock 配下の tx に閉じ込める。ロックを取らずに
    // autocommit で INSERT すると、並行する他 tx の BEGIN に取り込まれる。
    let (_write_guard, tx) = begin_write_tx(&state.db).await?;
    tx.execute(
        "INSERT OR IGNORE INTO revoked_sessions (sid) VALUES (?)",
        [sid.to_string()],
    )
    .await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    state
        .revoked_sessions
        .lock()
        .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?
        .insert(sid.to_string());
    Ok(())
}

/// sid が失効済みかをメモリと DB の両方で確認する（logout の冪等判定用）。
async fn is_session_revoked(state: &Arc<AppState>, sid: &str) -> Result<bool, AppError> {
    if state
        .revoked_sessions
        .lock()
        .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?
        .contains(sid)
    {
        return Ok(true);
    }
    let mut rows = state
        .db
        .query(
            "SELECT 1 FROM revoked_sessions WHERE sid = ? LIMIT 1",
            [sid.to_string()],
        )
        .await?;
    Ok(rows.next().await?.is_some())
}

/// セッションを失効させる。以後、同じ `sid` の JWT は拒否される。
/// 二重呼び出しは冪等: 既に失効済みの sid でも 200 を返す。
pub async fn logout(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Response, AppError> {
    // AuthClaims 抽出器は失効済み sid を 401 で弾くため、logout では署名・期限
    // のみを検証し、失効判定は下の冪等分岐で行う。検証ロジックは抽出器と同じ
    // jwt_keys.decode を使うため、改ざん・期限切れトークンは従来どおり拒否される。
    let claims = {
        let auth_header = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| AppError::Unauthorized("Missing Authorization header".to_string()))?;
        if !auth_header.starts_with("Bearer ") {
            return Err(AppError::Unauthorized(
                "Invalid Authorization format".to_string(),
            ));
        }
        state
            .jwt_keys
            .decode::<Claims>(&auth_header[7..])
            .map_err(|e| AppError::Unauthorized(format!("Invalid token: {}", e)))?
            .claims
    };
    if let Some(sid) = &claims.sid {
        // メモリ/DB の両方を確認し、既に失効済みなら二重 logout として成功扱い。
        // revoke_session は INSERT OR IGNORE なので再記録自体は安全だが、
        // ここでは不要な再書き込みを避けて冪等に 200 を返す。
        if !is_session_revoked(&state, sid).await? {
            revoke_session(&state, sid).await?;
        }
    }
    // リフレッシュトークンが添付されていれば、そのファミリを失効させる。
    // state Cookie と同じ基準で重複を拒否する（先勝ちで攻撃者の値を選ばない）。
    if let Some(token) = oidc::refresh_cookie_from_headers(&headers) {
        let token_hash = sha256::digest(token.as_bytes());
        // 読み取りカーソルを先に閉じてから BEGIN する。
        let family_id = {
            let mut rows = state
                .db
                .query(
                    "SELECT family_id FROM refresh_tokens WHERE token_hash = ?",
                    [token_hash],
                )
                .await?;
            rows.next()
                .await?
                .map(|row| -> Result<String, AppError> { Ok(row.get(0)?) })
                .transpose()?
        };
        if let Some(family_id) = family_id {
            let (_write_guard, tx) = begin_write_tx(&state.db).await?;
            tx.execute(
                "UPDATE refresh_tokens SET used_at = datetime('now') WHERE family_id = ? AND used_at IS NULL",
                [family_id],
            )
            .await?;
            tx.commit()
                .await
                .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
        }
    }
    // DB 側のファミリ失効だけではブラウザの Cookie が 30 日残るため、
    // 同じ Path/SameSite 属性の空 Cookie で即時削除する。
    // Secure 属性は発行側 refresh_cookie と同じ OIDC 設定を単一ソースとして共有する。
    // OIDC 未構成（mock 認証等）では Secure を付与する（fail-closed）。
    let secure = state
        .oidc
        .as_ref()
        .map(|oidc| oidc.cookie_secure())
        .unwrap_or(true);
    let cookie = oidc::delete_refresh_cookie(secure)?;
    let mut response = Json(true).into_response();
    response.headers_mut().append(header::SET_COOKIE, cookie);
    Ok(response)
}

/// ログへ出す URL から userinfo と query / fragment を除去する。
/// ゲートウェイ URL に認証情報やトークンが含まれても記録しない。
/// パース失敗時のフォールバックは `scheme://` 形のみ authority を走査し、
/// スキームレス URL（`//` 始まりを含む）は authority 対象外で userinfo を除去しない。
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
        Err(_) => {
            // パース失敗時も userinfo とクエリ/フラグメントを除去する。
            // reqwest::Url に頼らず std の文字列操作だけで扱うため、
            // シェル/URL の特殊文字・小数点・パーセントエンコードに影響されない。
            // 先に authority（scheme:// 直後から最初の '/' まで）だけを切り出して userinfo を
            // 除去し、その後にクエリ/フラグメントを除去する。順序を逆にすると userinfo 内の
            // '?' / '#' がクエリ開始と誤判定され、userinfo の一部が残存する。
            let redacted = if let Some(scheme_end) = url.find("://") {
                let authority_start = scheme_end + 3;
                let rest = &url[authority_start..];
                // authority 内に '@' が無ければ URL 全体をそのまま残す。
                let authority_end = rest.find('/').unwrap_or(rest.len());
                let authority = &rest[..authority_end];
                if let Some(at) = authority.rfind('@') {
                    let mut s = String::with_capacity(url.len());
                    s.push_str(&url[..authority_start]);
                    s.push_str(&rest[at + 1..]);
                    s
                } else {
                    url.to_string()
                }
            } else {
                url.to_string()
            };
            redacted
                .split(['?', '#'])
                .next()
                .unwrap_or(&redacted)
                .to_string()
        }
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

    // ゲートウェイは設定順に試す。ネットワークエラー（接続失敗・タイムアウト）は
    // 各ゲートウェイを最大 ipfs_max_attempts 回まで再試行する。
    // 2xx 以外の HTTP 応答（3xx を含む）は次のゲートウェイへフェイルオーバーする。
    // HTTP クライアントは Policy::none() でリダイレクトを追従しないため、
    // 3xx の Location 先へは接続せず、3xx はエラーとして扱う。
    // サイズ超過・チェックサム不一致は即拒否する。
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
            // ストリーム途中の切断（chunk() のエラー）は即 500 とせず、接続エラーと
            // 同様に last_error へ記録して次のゲートウェイへフェイルオーバーする。
            let mut chunk_error = None;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if bytes.len().saturating_add(chunk.len()) > MAX_CONTENT_BYTES {
                            return Err(AppError::BadRequest(
                                "IPFS content exceeds size limit".to_string(),
                            ));
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        chunk_error = Some(describe_reqwest_error(&e));
                        break;
                    }
                }
            }
            if let Some(error) = chunk_error {
                last_error = format!(
                    "{}: Failed to read response body: {}",
                    redact_url(&gateway_url),
                    error
                );
                continue;
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
                return Err(AppError::BadGateway);
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
            // 3xx を含む 2xx 以外は次のゲートウェイへ切り替える。
            // リダイレクトは無効化済みなので Location 先へは接続しない。
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

/// `limit` / `offset` を安全側の既定値へ正規化する純粋関数。
/// 範囲外・非数値の `limit` は 400 にせず既定 50、負・非数値の `offset` は 0 とする。
pub fn parse_pagination(limit: Option<&str>, offset: Option<&str>) -> (i64, i64) {
    let limit = limit
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| (1..=MAX_CONTENT_LIMIT).contains(value))
        .unwrap_or(DEFAULT_CONTENT_LIMIT);
    let offset = offset
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(0);
    (limit, offset)
}

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
    let (limit, offset) = parse_pagination(query.limit.as_deref(), query.offset.as_deref());
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
        // price は外部データ取込等で NULL になり得る。一覧は i64 で返すため
        // COALESCE で 0（購入不可）に読み替え、エクスポート側の Option<i64> と
        // 経路を揃える。JOIN / WHERE / ORDER には影響しない。
        "SELECT cid, checksum, type, status, COALESCE(price, 0) AS price, title, description, tags, \
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
    // 読み取りは従来どおり。カーソルを閉じてから BEGIN する。
    let exists = {
        let mut rows = state
            .db
            .query(
                "SELECT 1 FROM contents WHERE cid = ? AND status = 'Approved'",
                [cid.to_string()],
            )
            .await
            .map_err(|_| AppError::Internal("favorite lookup failed".to_string()))?;
        rows.next()
            .await
            .map_err(|_| AppError::Internal("favorite lookup failed".to_string()))?
            .is_some()
    };
    if !exists {
        // 存在しない / Approved でないコンテンツの有無を識別子で区別しない。
        return Err(AppError::NotFound);
    }
    // 書き込みは write_lock 配下の tx に載せる。ロック取得・BEGIN・COMMIT の
    // いずれが失敗しても、DB 詳細を出さず既存と同じ固定文言で返す。
    let write_result: Result<(), AppError> = async {
        let (_write_guard, tx) = begin_write_tx(&state.db).await?;
        if add {
            tx.execute(
                "INSERT OR IGNORE INTO favorites (user_hash, cid) VALUES (?, ?)",
                (sub.to_string(), cid.to_string()),
            )
            .await?;
        } else {
            tx.execute(
                "DELETE FROM favorites WHERE user_hash = ? AND cid = ?",
                (sub.to_string(), cid.to_string()),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    .await;
    write_result.map_err(|_| AppError::Internal("favorite update failed".to_string()))?;
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

/// ADR-0004: 確定に必要な実効票数（定足数）。
/// 確定処理・理由文字列・統計クエリの3箇所で単一の値として使う。
const QUORUM_VOTES: i64 = 3;

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
    let (final_status, reason) = if total_votes >= QUORUM_VOTES {
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
                "ADR-0004 quorum not met: {} votes (need {}); report dismissed",
                total_votes, QUORUM_VOTES
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
    tx: &libsql::Transaction,
    action: &str,
    target: &str,
    actor: &str,
    reason: &str,
    details: Option<String>,
) -> Result<(), AppError> {
    // 必ず呼び出し側の書き込み tx に載せる（生 Connection は受け取らない）。
    tx.execute(
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

    // BAN 以前に発行された JWT は 24h 有効なまま残るため、sub の失効閾値を
    // 現在時刻で記録して AuthClaims の検証で即座に拒否する。OIDC の
    // back-channel logout と同じ順序・同じヘルパを使う。
    // 再起動窓を塞ぐため、閾値は users 更新と同じ tx で先にコミットする。
    let revoked_at = chrono::Utc::now().timestamp() as usize;

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
    // プロセス再起動後も失効を維持するため、DB 側の閾値を users 更新と同じ
    // トランザクションで確定する。より新しい閾値だけを反映し、古い値で
    // 上書きされないようにする。
    tx.execute(
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES (?, ?)
         ON CONFLICT(sub) DO UPDATE SET revoked_at = excluded.revoked_at,
             updated_at = CURRENT_TIMESTAMP
         WHERE revoked_subs.revoked_at < excluded.revoked_at",
        (user_hash.to_string(), revoked_at as i64),
    )
    .await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    // 下流の revoke_refresh_tokens_for_sub が同じ write_lock を取得するため、
    // ここで明示的に解放しないと自己デッドロックする。
    drop(_write_guard);

    // コミット後にメモリの失効閾値を更新する。DB 側が先に確定しているため、
    // メモリ更新が失敗しても再起動後は DB の閾値で失効が維持される。
    state
        .revoked_subs_before
        .lock()
        .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?
        .insert(user_hash.to_string(), revoked_at);
    // 失効前に発行された未使用リフレッシュトークンを消費し、refresh からの
    // 迂回（iat = 閾値+1 の JWT 発行）を防ぐ。DB 側の失効が先に確定しているため、
    // 中途失敗で JWT が有効なまま残る窓を縮める。
    oidc::revoke_refresh_tokens_for_sub(&state, user_hash).await?;
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

/// モデレーション投票の署名対象メッセージ（旧 v1 形式）。
/// 互換性のため残すが、`mod_vote` は v2 のみ受理するためこの形式では投票できない。
#[deprecated(
    note = "v1 vote messages are no longer accepted by mod_vote; use moderation_vote_message_v2"
)]
pub fn moderation_vote_message(report_id: i64, result: &str) -> String {
    json!({"report_id": report_id, "result": result}).to_string()
}

/// モデレーション投票の署名対象メッセージ（v2）。
/// ドメイン分離子と投票者ハッシュを含め、同じ公開鍵で署名したアップロード署名
/// など他プロトコルの署名を投票へ転用（クロスプロトコル再生）できないようにする。
pub fn moderation_vote_message_v2(report_id: i64, result: &str, juror_hash: &str) -> String {
    json!({
        "domain": "moderation-vote-v2",
        "report_id": report_id,
        "result": result,
        "juror_hash": juror_hash
    })
    .to_string()
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
    // v2 のみ受理する。旧形式の署名や他プロトコル由来の署名はここで拒否される。
    let message = moderation_vote_message_v2(vote.report_id, &vote.result, &vote.juror_hash);

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

/// Pinata レスポンスの `IpfsHash` を取り出し、CID 形式を厳密検証する。
/// 外部レスポンスは信頼できないため、`pin_to_ipfs` は必ずこの関数を通す。
fn cid_from_pinata_response(json: &serde_json::Value) -> Result<String, AppError> {
    let cid = json["IpfsHash"]
        .as_str()
        .ok_or_else(|| AppError::Internal("CID not found in Pinata response".to_string()))?;
    if !is_safe_cid(cid) {
        return Err(AppError::Internal(
            "invalid CID in Pinata response".to_string(),
        ));
    }
    Ok(cid.to_string())
}

pub async fn pin_to_ipfs(
    state: &AppState,
    data: Vec<u8>,
    filename: &str,
) -> Result<String, AppError> {
    let pinata_jwt = std::env::var("PINATA_JWT").unwrap_or_else(|_| "mock_jwt".to_string());

    // production_config_errors / doctor と統一し、前後の空白装飾付きモック値も
    // モック経路へ流す（外部 Pinata API への送信を避ける）。
    if pinata_jwt.trim() == "mock_jwt" {
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

    // 外部レスポンスの IpfsHash は保存前に厳密検証する（不正なら Internal）。
    cid_from_pinata_response(&json)
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
                        CASE WHEN COALESCE(vc.vote_count, 0) < ? THEN 1 ELSE 0 END AS quorum_not_met,
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
            [QUORUM_VOTES],
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

/// `TZ` が固定オフセット形式のときに使うオフセットを解釈する。
///
/// 外部依存（chrono-tz）を追加せず、`Etc/GMT±N` のみを FixedOffset に変換する。
/// POSIX の `Etc/GMT` は符号が直観と逆で、`Etc/GMT-9` が UTC+9（JST）を表す。
fn policy_fixed_offset(tz: Option<&str>) -> Option<chrono::FixedOffset> {
    let tz = tz?.trim();
    if tz.eq_ignore_ascii_case("Asia/Tokyo") {
        // 例外的に日本の固定オフセット +9 として扱う。日本に夏時間はなく、
        // オフセットが変化しないため固定値でも「今日」の境界は安全に求められる。
        return chrono::FixedOffset::east_opt(9 * 3600);
    }
    let hours = tz.strip_prefix("Etc/GMT")?.parse::<i32>().ok()?;
    // POSIX 規約: Etc/GMT-N = UTC+N, Etc/GMT+N = UTC-N。
    chrono::FixedOffset::east_opt(-hours * 3600)
}

/// 年齢判定の基準日（ローカル TZ での「今日」）を求める純粋関数。
///
/// 既知の固定オフセットが解決できない場合は現在どおり UTC の日付を返す
/// （未対応 TZ で年齢境界を意図せず早めない fail-safe な既定）。
pub fn today_for_policy(now: chrono::DateTime<chrono::Utc>, tz: Option<&str>) -> chrono::NaiveDate {
    match policy_fixed_offset(tz) {
        Some(offset) => now.with_timezone(&offset).date_naive(),
        None => now.date_naive(),
    }
}

fn age_validation_error_to_app_error(e: common::age::AgeValidationError) -> AppError {
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
}

/// 設定由来の年齢ゲート（require_explicit_configuration / enforcement_mode）だけを判定する。
///
/// OIDC の callback / refresh は実際の生年月日を保持しないため、ここでは
/// `is_age_allowed_with_policy` を「有効な大人の生年月日」で呼び出して設定エラー
/// のみを取り出し、年齢数値の判定は `/auth/verify` の `validate_age` に委ねる。
/// `is_age_allowed_with_policy` は設定チェックを最初に行うため、設定エラーの
/// 検出は生年月日の内容に依存しない。すべてのセッション発行経路がこの
/// 共通ゲートを通ることで、fail-closed 設定のバイパスを防ぐ。
pub(crate) fn ensure_age_policy_configured(
    policy: &common::age::AgePolicy,
) -> Result<(), AppError> {
    const CONFIG_PROBE_BIRTH_DATE: &str = "1900-01-01";
    const CONFIG_PROBE_TODAY: chrono::NaiveDate =
        chrono::NaiveDate::from_ymd_opt(2000, 1, 1).expect("valid probe date");

    match common::age::is_age_allowed_with_policy(
        policy,
        CONFIG_PROBE_BIRTH_DATE,
        "JP",
        CONFIG_PROBE_TODAY,
    ) {
        Err(
            e @ (common::age::AgeValidationError::ConfigurationRequired
            | common::age::AgeValidationError::TrustedAttributeRequired),
        ) => Err(age_validation_error_to_app_error(e)),
        // Ok / UnderAge / InvalidBirthDate: 設定ゲートは通過している。
        _ => Ok(()),
    }
}

/// 基準時刻と TZ を明示した年齢検証（テストと `validate_age` の共通実装）。
pub(crate) fn validate_age_at(
    policy: &common::age::AgePolicy,
    birth_date: &str,
    country_code: &str,
    now: chrono::DateTime<chrono::Utc>,
    tz: Option<&str>,
) -> Result<(), AppError> {
    let today = today_for_policy(now, tz);
    common::age::is_age_allowed_with_policy(policy, birth_date, country_code, today)
        .map_err(age_validation_error_to_app_error)
}

pub fn validate_age(
    policy: &common::age::AgePolicy,
    birth_date: &str,
    country_code: &str,
) -> Result<(), AppError> {
    let tz = std::env::var("TZ").ok();
    validate_age_at(
        policy,
        birth_date,
        country_code,
        chrono::Utc::now(),
        tz.as_deref(),
    )
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

/// 12 表を単一のスナップショットで読み切る。
///
/// `db` には読み取りトランザクション内の接続（`BEGIN DEFERRED` 開始後）を渡す。
/// 全クエリが同じトランザクションに所属することをデバッグビルドで検証する
/// （`cargo test` では `debug_assert!` が有効）。
async fn collect_export_snapshot(db: &Connection, sub: &str) -> Result<DataExport, AppError> {
    debug_assert!(
        !db.is_autocommit(),
        "export queries must run inside a read transaction"
    );
    Ok(DataExport {
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

    let export = collect_export_snapshot(&tx, sub).await?;

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

#[cfg(test)]
mod fuzz_test;
#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests;
