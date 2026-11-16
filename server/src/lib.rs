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
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tracing::warn;
use uuid::Uuid;

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

/// ロックファイルに記録された PID が生存しているか（Linux の /proc を使用）。
/// /proc が無い環境では stale と判断せず、安全側に倒して拒否する。
fn pid_alive(pid: u32) -> bool {
    let proc_path = format!("/proc/{pid}");
    std::path::Path::new(&proc_path).exists()
}

/// 単一ライター用のロックファイルを原子的に取得する。
fn acquire_instance_lock(lock_path: &str) -> anyhow::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)
    {
        Ok(mut file) => {
            writeln!(file, "{}", std::process::id())?;
            file.sync_all()?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let contents = std::fs::read_to_string(lock_path).unwrap_or_default();
            let stale_pid = contents.trim().parse::<u32>().ok();
            if stale_pid.is_some_and(pid_alive) {
                anyhow::bail!(
                    "another server instance appears to be running (lock: {lock_path}, pid: {})",
                    stale_pid.unwrap()
                );
            }
            // クラッシュ後の残骸。原子的に削除して再取得を試みる。
            std::fs::remove_file(lock_path)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(lock_path)?;
            writeln!(file, "{}", std::process::id())?;
            file.sync_all()?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn run() -> anyhow::Result<()> {
    init_tracing();

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
    let jwt_keys = jwt_keys_from_env(
        std::env::var("JWT_RSA_PRIVATE_KEY_PEM").ok(),
        std::env::var("JWT_SECRET").ok(),
        verification_keys_env.as_deref(),
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

    let oidc = oidc::OidcState::from_env();
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

    let moderation_operators_env = std::env::var("MODERATION_OPERATOR_HASHES").ok();
    let moderation_operators = Arc::new(moderation_operators(moderation_operators_env.as_deref()));
    if moderation_operators.is_empty() {
        warn!("MODERATION_OPERATOR_HASHES is not set: BAN/unban/unpin endpoints are disabled");
    }

    let shared_state = Arc::new(AppState {
        db: Arc::new(db),
        http_client: Client::new(),
        jwt_keys: Arc::new(jwt_keys),
        allow_mock_auth: std::env::var("ALLOW_MOCK_AUTH")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false),
        ipfs_gateways,
        ipfs_timeout,
        payment_receiving_address,
        payment_receiving_xprv,
        payment_verifier: None,
        oidc,
        revoked_sessions: Arc::new(Mutex::new(HashSet::new())),
        revoked_subs_before,
        age_policy,
        moderation_operators,
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });

    let worker_db = Arc::clone(&shared_state.db);
    let shutdown_flag = Arc::clone(&shared_state.shutting_down);
    let app = build_router(shared_state);

    let finalize_interval_secs = std::env::var("MODERATION_FINALIZE_INTERVAL_SECS").ok();
    let finalize_interval = moderation_finalize_interval(finalize_interval_secs.as_deref());
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let finalizer = tokio::spawn(run_moderation_finalizer(
        worker_db,
        finalize_interval,
        shutdown_rx,
    ));

    let addr = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 3000));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Guild Backend listening on {}", addr);
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
async fn run_moderation_finalizer(
    db: Arc<Connection>,
    interval: Duration,
    mut shutdown: tokio::sync::watch::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // 起動直後の即時 tick は消費する
    loop {
        tokio::select! {
            _ = ticker.tick() => {
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
        .route("/content/{cid}", get(get_content))
        .route("/content/{cid}/audit", get(get_content_audit))
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
        .route("/policy/age", get(get_age_policy))
        .layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024))
        .layer(
            CorsLayer::new()
                .allow_origin(allowed_origins(
                    allowed_origins_env.as_deref(),
                    state.allow_mock_auth,
                ))
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

pub async fn auth_login() -> Result<Json<Oid4vpRequest>, AppError> {
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

/// セッションを失効させる。以後、同じ `sid` の JWT は拒否される。
pub async fn logout(
    State(state): State<Arc<AppState>>,
    claims: AuthClaims,
    headers: axum::http::HeaderMap,
) -> Result<Json<bool>, AppError> {
    if let Some(sid) = &claims.sid {
        state
            .revoked_sessions
            .lock()
            .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?
            .insert(sid.clone());
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
    Ok(Json(true))
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

/// CID を URL パスへ埋め込む前の安全検査。
/// 英数字のみを許可し、`/`、`..`、`?`、`#` などのパス・クエリ注入を拒否する。
/// 厳密な CID 仕様の検証は別途行う。
pub fn is_safe_cid(cid: &str) -> bool {
    !cid.is_empty() && cid.len() <= 128 && cid.chars().all(|c| c.is_ascii_alphanumeric())
}

pub async fn get_content(
    State(state): State<Arc<AppState>>,
    _claims: AuthClaims,
    Path(cid): Path<String>,
) -> Result<Response, AppError> {
    if !is_safe_cid(&cid) {
        return Err(AppError::BadRequest("Invalid CID".to_string()));
    }

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

/// ADR-0004: 期限到達済みの通報を集計して確定する。
///
/// - 投票期間（7日。`deadline_at` 未設定時は `created_at + 7 days`）が終わる前は
///   何もしない。3票到達による即時確定は行わない。
/// - 定足数（3票）未達なら通報を棄却し、コンテンツを `Approved` に戻す。
/// - 同数は公開継続リスクを避けて `Rejected` とする。
/// - 確定済み・レビュー対象外の通報は冪等に無視する。
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

    // 1. Tally votes
    let mut rows = tx
        .query(
            "SELECT vote_result, COUNT(*) FROM votes WHERE report_id = ? GROUP BY vote_result",
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
    tx.execute(
        "UPDATE users SET is_banned = TRUE, banned_at = datetime('now') WHERE user_hash = ?",
        [user_hash],
    )
    .await?;
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
        "UPDATE users SET is_banned = FALSE, banned_at = NULL WHERE user_hash = ?",
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
                // 注文側の上限（MAX_ORDER_JPY）と揃え、注文不可能な価格の登録を防ぐ。
                if price > payments::MAX_ORDER_JPY as i64 {
                    return Err(AppError::BadRequest(format!(
                        "price must not exceed {}",
                        payments::MAX_ORDER_JPY
                    )));
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

#[cfg(test)]
mod fuzz_test;
#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests;
