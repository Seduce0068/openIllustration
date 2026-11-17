use crate::{
    AppError, AppState, ContentFilter, ContentMeta, ContentQuery, DEFAULT_CONTENT_LIMIT,
    FavoriteResponse, MAX_CONTENT_DESCRIPTION_BYTES, MAX_CONTENT_LIMIT, MAX_CONTENT_TAG_BYTES,
    MAX_CONTENT_TAGS, MAX_CONTENT_TITLE_BYTES, ModAuditCountRow, ModConclusionCountRow,
    ModGovernanceCountRow, ModStatsMonthly, ModStatsResponse, ModStatsTotals, VerifyRequest,
    VerifyResponse, allowed_origins, build_content_query, build_mod_stats, build_router,
    ensure_user_not_banned, escape_like, parse_content_query, parse_tags_field,
    payments::{MAX_ORDER_JPY, PaymentOrder, PaymentVerification, PaymentVerifier},
    process_report_conclusion, update_content_status, validate_age, validate_content_metadata,
    validate_jwt_secret, validate_upload_content, validate_upload_content_type,
};
use async_trait::async_trait;
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{ConnectInfo, Form, Path, State},
    http::{HeaderValue, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use k256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use serde::Deserialize;
use sha2::Digest as _;
use sha2::Sha256 as Sha2Sha256;
use sha3::{Digest, Keccak256};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// テスト専用の決定的RNG。RSA 鍵生成のテストにだけ使う。
struct TestRng(u64);

impl TestRng {
    fn new() -> Self {
        // 同一プロセス内で連続して生成しても別の鍵になるようカウンタを混ぜる。
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(0x9E37_79B9_7F4A_7C15 ^ n.wrapping_mul(0x2545_F491_4F6C_DD1D))
    }

    fn next_u64_inner(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

impl rsa::rand_core::TryRng for TestRng {
    type Error = rsa::rand_core::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok((self.next_u64_inner() >> 32) as u32)
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        Ok(self.next_u64_inner())
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        for chunk in dst.chunks_mut(8) {
            let value = self.next_u64_inner().to_le_bytes();
            chunk.copy_from_slice(&value[..chunk.len()]);
        }
        Ok(())
    }
}

impl rsa::rand_core::TryCryptoRng for TestRng {}

fn test_ipv6_peer() -> IpAddr {
    IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1))
}
use common::Claims;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use libsql::Builder;
use rsa::sha2::Sha256;
use rsa::{PrivateKey, pkcs8::EncodePublicKey};
use serde_json::json;
use std::sync::{Arc, LazyLock};
use tower::ServiceExt;

/// プロセス環境変数を変更するテストの直列化に使う。
static ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// テスト専用の環境変数設定（呼び出し前に `ENV_LOCK` を保持すること）。
///
/// # Safety
/// Rust 2024 では `set_var` / `remove_var` が unsafe だが、これは「他スレッドが
/// 同時に環境変数を読まないこと」の保証を呼び出し側が負う API である。
/// 本関数は crate 内のテストからしか呼ばれず、すべての呼び出し箇所で
/// `ENV_LOCK` を保持して直列化しているため、並行読み取りは発生しない。
/// また対象はテスト固有の環境変数（`ALLOW_IPV4_LOCALHOST` /
/// `EXPOSE_INTERNAL_ERRORS`）のみで、本番コードパスでは呼ばれない。
fn test_set_env(key: &str, value: &str) {
    // SAFETY: 上記のとおり ENV_LOCK により並行アクセスが排除されている。
    unsafe { std::env::set_var(key, value) };
}

/// テスト専用の環境変数削除（呼び出し前に `ENV_LOCK` を保持すること）。
fn test_remove_env(key: &str) {
    // SAFETY: 同上。ENV_LOCK により並行アクセスが排除されている。
    unsafe { std::env::remove_var(key) };
}

/// テスト用の HS256 秘密。HS256 の要件どおり 32 バイト以上にする。
const TEST_JWT_SECRET: &str = "test_secret_for_openillustration_tests";

/// テスト用の HS256 鍵。
fn test_jwt_keys() -> Arc<crate::jwt::JwtKeys> {
    Arc::new(crate::jwt::JwtKeys::from_hmac_secret(TEST_JWT_SECRET).expect("test_secret is valid"))
}

async fn generate_token(secret: &str, sub: &str) -> String {
    let claims = Claims {
        sub: sub.to_string(),
        exp: 10000000000,
        sid: None,
        iat: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_ref()).unwrap(),
    )
    .unwrap()
}

async fn init_schema(db: &libsql::Connection) {
    crate::migration::run_migrations(db).await.unwrap();
}

async fn setup_app_with_db() -> (Router, Arc<libsql::Connection>) {
    setup_app_with_options(Vec::new(), Duration::from_secs(15), true).await
}

/// テスト用アプリを構築する。IPFS ゲートウェイは既定で無効にし、
/// 外部ネットワークへ接続しない。取得を検証するテストだけが
/// ローカルモックの URL テンプレートを渡す。
async fn setup_app_with_options(
    ipfs_gateways: Vec<String>,
    ipfs_timeout: Duration,
    allow_mock_auth: bool,
) -> (Router, Arc<libsql::Connection>) {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    // Insert test user first (required for FK constraint)
    db.execute("INSERT INTO users (user_hash) VALUES ('test_user')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmTestCID', 'Illustration', 'test_user', 'mock_checksum', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth,
        ipfs_gateways,
        ipfs_timeout,
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });

    let router = build_router(shared_state);
    (router, db)
}

/// 指定した JWT 鍵でサーバーを構築する。RS256 経路の検証に使う。
async fn setup_app_with_jwt_keys(
    jwt_keys: Arc<crate::jwt::JwtKeys>,
) -> (Router, Arc<libsql::Connection>) {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('test_user')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmTestCID', 'Illustration', 'test_user', 'mock_checksum', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: reqwest::Client::new(),
        jwt_keys,
        allow_mock_auth: false,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    (build_router(shared_state), db)
}

#[derive(Default)]
struct MockVerifier {
    verification: std::sync::Mutex<Option<PaymentVerification>>,
}

#[async_trait]
impl PaymentVerifier for MockVerifier {
    async fn verify(&self, _order: &PaymentOrder) -> Result<PaymentVerification, AppError> {
        self.verification
            .lock()
            .expect("verification lock")
            .clone()
            .ok_or_else(|| AppError::Internal("verification not configured".to_string()))
    }
}

/// 検証中に注文状態を書き換える verifier（CAS の回帰テスト用）。
struct RacingVerifier {
    db: Arc<libsql::Connection>,
    verification: PaymentVerification,
}

#[async_trait]
impl PaymentVerifier for RacingVerifier {
    async fn verify(&self, order: &PaymentOrder) -> Result<PaymentVerification, AppError> {
        // load_order の後に状態を変更し、CAS が無いと上書きされる状況を再現する。
        self.db
            .execute(
                "UPDATE payment_orders SET status = ? WHERE order_id = ?",
                (crate::payments::STATUS_MISMATCH, order.order_id.clone()),
            )
            .await?;
        Ok(self.verification.clone())
    }
}

/// 2 段目 CAS の回帰テスト用 verifier。
///
/// `verify()` 自体は状態を書き換えず（1 段目の CAS を必ず成功させるため）、
/// 代わりに DB トリガを仕込む。トリガは 1 段目が `DETECTED` へ更新された
/// 直後に発火して status を書き換えるため、2 段目の `WHERE status = 'DETECTED'`
/// は 0 件更新になる。これは「他ノードが DETECTED を書き換えた」状況を
/// 1 段目と 2 段目の間だけに閉じて再現するものである。
struct Stage2RacingVerifier {
    db: Arc<libsql::Connection>,
    verification: PaymentVerification,
}

#[async_trait]
impl PaymentVerifier for Stage2RacingVerifier {
    async fn verify(&self, _order: &PaymentOrder) -> Result<PaymentVerification, AppError> {
        let sql = format!(
            "CREATE TRIGGER IF NOT EXISTS stage2_race_rewrite
             AFTER UPDATE OF status ON payment_orders
             WHEN NEW.status = '{detected}' AND OLD.status <> '{detected}'
             BEGIN
                 UPDATE payment_orders
                    SET status = '{mismatch}'
                  WHERE order_id = NEW.order_id;
             END;",
            detected = crate::payments::STATUS_DETECTED,
            mismatch = crate::payments::STATUS_MISMATCH,
        );
        // 書き換え後の status は DETECTED ではないのでトリガは再帰しない。
        self.db.execute(&sql, ()).await?;
        Ok(self.verification.clone())
    }
}

async fn setup_app_with_payments(
    receiving_address: &str,
    verifier: Arc<dyn PaymentVerifier>,
) -> (Router, Arc<libsql::Connection>) {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('test_user')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status, price)
             VALUES ('QmPayTest', 'Novel', 'test_user', 'abc', 'Approved', 1000)",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: Some(receiving_address.to_string()),
        payment_receiving_xprv: None,
        payment_verifier: Some(verifier),
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });

    let router = build_router(shared_state);
    (router, db)
}

async fn setup_app_with_hd_payments(xprv: &str) -> (Router, Arc<libsql::Connection>) {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('test_user')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status, price)
             VALUES ('QmPayTest', 'Novel', 'test_user', 'abc', 'Approved', 1000)",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: Some(xprv.to_string()),
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });

    (build_router(shared_state), db)
}

async fn setup_app() -> Router {
    setup_app_with_db().await.0
}

struct MockOidcServer {
    base_url: String,
    nonce: Arc<std::sync::Mutex<Option<String>>>,
    /// 設定時は at_hash 検証の異常系テストのために ID トークンへ埋め込む値を差し替える。
    at_hash_override: Arc<std::sync::Mutex<Option<String>>>,
    /// 設定時は ID トークンを JWKS 未登録の鍵で署名する（署名検証の異常系テスト用）。
    forged_key_der: Arc<std::sync::Mutex<Option<Vec<u8>>>>,
    /// 設定時は ID トークンヘッダから kid を省略する（kid 必須検査のテスト用）。
    omit_kid: Arc<std::sync::Mutex<bool>>,
    /// 設定時は auth_time クレームを発行する（max_age 検証用）。
    auth_time: AuthTimeOverride,
    state: Arc<MockOidcState>,
    handle: tokio::task::JoinHandle<()>,
}

impl MockOidcServer {
    async fn start() -> Self {
        let signing_key =
            p256::ecdsa::SigningKey::from_slice(&[0x01u8; 32]).expect("valid p256 test scalar");
        let der = signing_key.to_pkcs8_der().unwrap();
        let encoding_key = EncodingKey::from_ec_der(der.as_bytes());
        let point = signing_key.verifying_key().to_sec1_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().unwrap());
        let y = URL_SAFE_NO_PAD.encode(point.y().unwrap());

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind mock OIDC server");
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");
        let nonce = Arc::new(std::sync::Mutex::new(None));
        let at_hash_override = Arc::new(std::sync::Mutex::new(None));
        let forged_key_der = Arc::new(std::sync::Mutex::new(None));
        let omit_kid = Arc::new(std::sync::Mutex::new(false));
        let auth_time = Arc::new(std::sync::Mutex::new(None));
        let state = Arc::new(MockOidcState {
            base_url: base_url.clone(),
            client_id: "test-client".to_string(),
            nonce: nonce.clone(),
            at_hash_override: at_hash_override.clone(),
            forged_key_der: forged_key_der.clone(),
            omit_kid: omit_kid.clone(),
            assurance: Arc::new(std::sync::Mutex::new(None)),
            auth_time: auth_time.clone(),
            encoding_key,
            jwks: json!({
                "keys": [{
                    "kty": "EC",
                    "crv": "P-256",
                    "kid": "test-kid",
                    "use": "sig",
                    "alg": "ES256",
                    "x": x,
                    "y": y,
                }]
            }),
        });
        let app = Router::new()
            .route("/jwks", axum::routing::get(mock_jwks))
            .route("/token", axum::routing::post(mock_token))
            .with_state(state.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock OIDC server");
        });

        Self {
            base_url,
            nonce,
            at_hash_override,
            forged_key_der,
            omit_kid,
            auth_time,
            state,
            handle,
        }
    }

    /// 指定した追加クレームで logout token を署名する（異常系テスト用）。
    fn logout_token_with_claims(&self, extra: serde_json::Value) -> String {
        let now = chrono::Utc::now().timestamp() as usize;
        let mut claims = json!({
            "iss": self.state.base_url,
            "aud": self.state.client_id,
            "exp": now + 300,
            "iat": now,
            "jti": uuid::Uuid::new_v4().to_string(),
        });
        if let (Some(base), Some(extra)) = (claims.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                base.insert(key.clone(), value.clone());
            }
        }
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("test-kid".to_string());
        encode(&header, &claims, &self.state.encoding_key).unwrap()
    }

    fn logout_token(&self, sid: &str) -> String {
        self.logout_token_with_claims(json!({
            "sid": sid,
            "events": {"http://schemas.openid.net/event/backchannel-logout": {}},
        }))
    }

    fn logout_token_for_sub(&self, sub: &str) -> String {
        let now = chrono::Utc::now().timestamp() as usize;
        let claims = json!({
            "iss": self.state.base_url,
            "aud": self.state.client_id,
            "exp": now + 300,
            "iat": now,
            "jti": uuid::Uuid::new_v4().to_string(),
            "sub": sub,
            "events": {"http://schemas.openid.net/event/backchannel-logout": {}},
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("test-kid".to_string());
        encode(&header, &claims, &self.state.encoding_key).unwrap()
    }

    fn logout_token_for_sid_and_sub(&self, sid: &str, sub: &str) -> String {
        let now = chrono::Utc::now().timestamp() as usize;
        let claims = json!({
            "iss": self.state.base_url,
            "aud": self.state.client_id,
            "exp": now + 300,
            "iat": now,
            "jti": uuid::Uuid::new_v4().to_string(),
            "sid": sid,
            "sub": sub,
            "events": {"http://schemas.openid.net/event/backchannel-logout": {}},
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("test-kid".to_string());
        encode(&header, &claims, &self.state.encoding_key).unwrap()
    }
}

impl Drop for MockOidcServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

type AssuranceOverride = Arc<std::sync::Mutex<Option<(String, Vec<String>)>>>;
type AuthTimeOverride = Arc<std::sync::Mutex<Option<usize>>>;

struct MockOidcState {
    base_url: String,
    client_id: String,
    nonce: Arc<std::sync::Mutex<Option<String>>>,
    at_hash_override: Arc<std::sync::Mutex<Option<String>>>,
    forged_key_der: Arc<std::sync::Mutex<Option<Vec<u8>>>>,
    omit_kid: Arc<std::sync::Mutex<bool>>,
    /// Some の場合のみ acr/amr クレームを発行する（保証レベル検証用）。
    assurance: AssuranceOverride,
    /// Some の場合のみ auth_time クレームを発行する（max_age 検証用）。
    auth_time: AuthTimeOverride,
    encoding_key: EncodingKey,
    jwks: serde_json::Value,
}

async fn mock_jwks(State(state): State<Arc<MockOidcState>>) -> Json<serde_json::Value> {
    Json(state.jwks.clone())
}

#[derive(Deserialize)]
struct MockTokenRequest {
    code: String,
    code_verifier: String,
}

async fn mock_token(
    State(state): State<Arc<MockOidcState>>,
    Form(payload): Form<MockTokenRequest>,
) -> Json<serde_json::Value> {
    assert!(!payload.code.is_empty());
    assert!(
        payload.code_verifier.len() >= 43,
        "PKCE verifier is too short"
    );

    let nonce = state.nonce.lock().unwrap().clone().unwrap_or_default();
    let now = chrono::Utc::now().timestamp() as usize;
    let access_token = "test_access_token";
    let digest = Sha2Sha256::digest(access_token.as_bytes());
    let at_hash = state
        .at_hash_override
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| URL_SAFE_NO_PAD.encode(&digest[..16]));
    let claims = json!({
        "iss": state.base_url,
        "aud": state.client_id,
        "exp": now + 300,
        "iat": now,
        "nbf": now - 10,
        "sub": "test-ppid",
        "nonce": nonce,
        "at_hash": at_hash,
    });
    let mut claims = claims;
    if let Some((acr, amr)) = state.assurance.lock().unwrap().clone() {
        claims["acr"] = json!(acr);
        claims["amr"] = json!(amr);
    }
    if let Some(auth_time) = *state.auth_time.lock().unwrap() {
        claims["auth_time"] = json!(auth_time);
    }
    let mut header = Header::new(Algorithm::ES256);
    if !*state.omit_kid.lock().unwrap() {
        header.kid = Some("test-kid".to_string());
    }
    let forged_key = state
        .forged_key_der
        .lock()
        .unwrap()
        .clone()
        .map(|der| EncodingKey::from_ec_der(&der));
    let id_token = match &forged_key {
        Some(key) => encode(&header, &claims, key).unwrap(),
        None => encode(&header, &claims, &state.encoding_key).unwrap(),
    };

    Json(json!({
        "access_token": access_token,
        "id_token": id_token,
        "token_type": "Bearer",
        "expires_in": 3600,
    }))
}

async fn setup_app_with_oidc() -> (
    Router,
    Arc<crate::oidc::OidcState>,
    MockOidcServer,
    Arc<libsql::Connection>,
) {
    setup_app_with_oidc_and_age_policy(common::age::AgePolicy::default()).await
}

async fn setup_app_with_oidc_and_age_policy(
    age_policy: common::age::AgePolicy,
) -> (
    Router,
    Arc<crate::oidc::OidcState>,
    MockOidcServer,
    Arc<libsql::Connection>,
) {
    let mock = MockOidcServer::start().await;
    let config = crate::oidc::OidcConfig {
        issuer: mock.base_url.clone(),
        client_id: "test-client".to_string(),
        client_secret: "test-secret".to_string(),
        redirect_uri: "http://localhost/callback".to_string(),
        authorization_endpoint: format!("{}/authorize", mock.base_url),
        token_endpoint: format!("{}/token", mock.base_url),
        jwks_uri: format!("{}/jwks", mock.base_url),
        scopes: "openid".to_string(),
        cookie_secure: false,
        clock_skew_secs: 60,
        required_acr: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        required_amr: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        max_auth_age_secs: std::sync::Arc::new(std::sync::Mutex::new(None)),
    };
    let oidc = Arc::new(crate::oidc::OidcState::new(config));

    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: false,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: Some(oidc.clone()),
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy,
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });

    (build_router(shared_state), oidc, mock, db)
}

/// R4-02: レート制限設定の境界値。
#[test]
fn test_rate_limit_configuration() {
    assert_eq!(crate::rate_limit_per_min(None), 60);
    assert_eq!(crate::rate_limit_per_min(Some("2")), 2);
    assert_eq!(crate::rate_limit_per_min(Some("0")), 60);
    assert_eq!(crate::rate_limit_per_min(Some("601")), 60);
    assert_eq!(crate::rate_limit_per_min(Some("bad")), 60);
    assert_eq!(crate::max_concurrent_requests(None), 16);
    assert_eq!(crate::max_concurrent_requests(Some("1")), 1);
    assert_eq!(crate::max_concurrent_requests(Some("0")), 16);
    assert_eq!(crate::max_concurrent_requests(Some("257")), 16);
    assert_eq!(crate::max_concurrent_requests(Some("bad")), 16);
}

/// R4-02: IP 単位のレート制限が 429 で拒否すること。
#[tokio::test]
async fn test_rate_limit_rejects_excess_requests() {
    use std::num::NonZeroU32;
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    let state = Arc::new(AppState {
        db,
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: false,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: Some(Arc::new(crate::RateLimitState {
            limiter: governor::RateLimiter::keyed(governor::Quota::per_minute(
                NonZeroU32::new(2).unwrap(),
            )),
            concurrency: Arc::new(tokio::sync::Semaphore::new(16)),
        })),
    });
    let app = build_router(state);

    // 2 回は成功（404 でもレート制限は通過）、3 回目で 429。
    let peer = test_ipv6_peer();
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::get("/no-such-path").body(Body::empty()).unwrap(),
                peer,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/no-such-path").body(Body::empty()).unwrap(),
            peer,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    // 監視用エンドポイントは制限対象外。
    let health = app
        .oneshot(with_peer(
            Request::get("/health").body(Body::empty()).unwrap(),
            peer,
        ))
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
}

// ---- レート制限キーが XFF 解決 IP と一致することの検証 ----
// TRUSTED_PROXY_CIDRS は OnceLock なのでテストでは環境変数を触らず、
// network の実処理へ明示的な信頼範囲を渡すラッパーを使う。

const RATE_LIMIT_PROXY: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));
const RATE_LIMIT_CLIENT_A: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0xa));
const RATE_LIMIT_CLIENT_B: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0xb));
const RATE_LIMIT_UNTRUSTED_PEER: IpAddr =
    IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 9));

/// テスト用: 指定プロキシを信頼する network ミドルウェア。
async fn trusted_network_middleware(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    crate::network::resolve_client_ip_and_run(
        &[RATE_LIMIT_PROXY.to_string()],
        ConnectInfo(peer),
        request,
        next,
    )
    .await
}

/// テスト用: 信頼プロキシなし（XFF を無視する）network ミドルウェア。
async fn untrusted_network_middleware(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    crate::network::resolve_client_ip_and_run(&[], ConnectInfo(peer), request, next).await
}

async fn rate_limit_test_state(per_minute: u32) -> Arc<AppState> {
    use std::num::NonZeroU32;
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    Arc::new(AppState {
        db,
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: false,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: Some(Arc::new(crate::RateLimitState {
            limiter: governor::RateLimiter::keyed(governor::Quota::per_minute(
                NonZeroU32::new(per_minute).unwrap(),
            )),
            concurrency: Arc::new(tokio::sync::Semaphore::new(16)),
        })),
    })
}

async fn rate_limit_request(app: Router, peer: IpAddr, forwarded: Option<IpAddr>) -> StatusCode {
    let mut req = Request::get("/no-such-path").body(Body::empty()).unwrap();
    if let Some(client) = forwarded {
        req.headers_mut().insert(
            "x-forwarded-for",
            client.to_string().parse::<HeaderValue>().unwrap(),
        );
    }
    app.oneshot(with_peer(req, peer)).await.unwrap().status()
}

/// レート制限キーは extensions の解決済み IP を優先し、無ければピアを使う。
#[test]
fn test_rate_limit_key_prefers_resolved_client_ip() {
    let peer = test_ipv6_peer();
    let resolved: IpAddr = "2001:db8::a".parse().unwrap();
    let mut req = Request::get("/no-such-path").body(Body::empty()).unwrap();
    assert_eq!(crate::rate_limit_key(peer, &req), peer.to_string());
    req.extensions_mut().insert(resolved);
    assert_eq!(crate::rate_limit_key(peer, &req), resolved.to_string());
}

/// 信頼プロキシ配下では XFF 解決済みクライアント IP ごとに独立バケットを持つ。
/// クライアント A の 2 回目までは成功し 3 回目が 429 でも、
/// 別バケットのクライアント B は 404 のままである。
#[tokio::test]
async fn test_rate_limit_uses_resolved_client_ip_under_trusted_proxy() {
    let state = rate_limit_test_state(2).await;
    let app = Router::new()
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(axum::middleware::from_fn_with_state(
            state,
            crate::rate_limit_middleware,
        ))
        .layer(axum::middleware::from_fn(trusted_network_middleware));

    // A: 2 回通過、3 回目で 429。
    assert_eq!(
        rate_limit_request(app.clone(), RATE_LIMIT_PROXY, Some(RATE_LIMIT_CLIENT_A)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        rate_limit_request(app.clone(), RATE_LIMIT_PROXY, Some(RATE_LIMIT_CLIENT_A)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        rate_limit_request(app.clone(), RATE_LIMIT_PROXY, Some(RATE_LIMIT_CLIENT_A)).await,
        StatusCode::TOO_MANY_REQUESTS
    );

    // B は独立バケットなので A の 429 の影響を受けない。
    assert_eq!(
        rate_limit_request(app, RATE_LIMIT_PROXY, Some(RATE_LIMIT_CLIENT_B)).await,
        StatusCode::NOT_FOUND
    );
}

/// 回帰: XFF 未設定／信頼プロキシ外ではピア IP がキーになる。
#[tokio::test]
async fn test_rate_limit_key_falls_back_to_peer_without_trusted_proxy() {
    let build_app = |state: Arc<AppState>| {
        Router::new()
            .fallback(|| async { StatusCode::NOT_FOUND })
            .layer(axum::middleware::from_fn_with_state(
                state,
                crate::rate_limit_middleware,
            ))
            .layer(axum::middleware::from_fn(untrusted_network_middleware))
    };

    // 信頼プロキシ外からの XFF は無視され、ピア IP バケットで制限される。
    let app = build_app(rate_limit_test_state(2).await);
    for _ in 0..2 {
        assert_eq!(
            rate_limit_request(
                app.clone(),
                RATE_LIMIT_UNTRUSTED_PEER,
                Some(RATE_LIMIT_CLIENT_B)
            )
            .await,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        rate_limit_request(app, RATE_LIMIT_UNTRUSTED_PEER, Some(RATE_LIMIT_CLIENT_B)).await,
        StatusCode::TOO_MANY_REQUESTS
    );

    // XFF 未設定でも同じくピア IP がキーになる（独立した limiter で確認）。
    let app = build_app(rate_limit_test_state(2).await);
    for _ in 0..2 {
        assert_eq!(
            rate_limit_request(app.clone(), RATE_LIMIT_UNTRUSTED_PEER, None).await,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        rate_limit_request(app, RATE_LIMIT_UNTRUSTED_PEER, None).await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

/// R1-17: 信頼属性必須モードでは OIDC callback / refresh を年齢ゲートで拒否する。
#[tokio::test]
async fn test_trusted_age_mode_blocks_oidc_callback_and_refresh() {
    let policy = common::age::AgePolicy {
        enforcement_mode: common::age::AgeEnforcementMode::TrustedAttributeRequired,
        ..common::age::AgePolicy::default()
    };
    let (app, oidc, mock, _db) = setup_app_with_oidc_and_age_policy(policy).await;

    // callback は state を消費する前に拒否される。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/callback?code=test-code&state=any-state")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // refresh もクッキーより先に拒否される。
    let response = app
        .oneshot(with_peer(
            Request::post("/auth/refresh")
                .header(header::COOKIE, "refresh_token=anything")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // 参照を残さないためのダミー利用（ mock / oidc は拒否経路では未使用）。
    let _ = (oidc, mock);
}

async fn oidc_login(
    app: &Router,
    oidc: &crate::oidc::OidcState,
    mock: &MockOidcServer,
) -> serde_json::Value {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let auth_state = started["state"].as_str().unwrap().to_string();
    let nonce = oidc.pending_nonce(&auth_state).expect("pending nonce");
    *mock.nonce.lock().unwrap() = Some(nonce);

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap()
}

/// OIDC ログインを行い、レスポンス本文とリフレッシュトークン Cookie を返す。
async fn oidc_login_with_refresh(
    app: &Router,
    oidc: &crate::oidc::OidcState,
    mock: &MockOidcServer,
) -> (serde_json::Value, String) {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let auth_state = started["state"].as_str().unwrap().to_string();
    let nonce = oidc.pending_nonce(&auth_state).expect("pending nonce");
    *mock.nonce.lock().unwrap() = Some(nonce);

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let refresh = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|part| part.trim().strip_prefix("refresh_token="))
        .unwrap_or_default()
        .to_string();
    assert!(!refresh.is_empty(), "refresh_token cookie must be set");
    let body =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    (body, refresh)
}

/// テスト用のローカル IPFS ゲートウェイ。
/// 外部ネットワークに依存せず、応答の状態・遅延・宣言長を制御できる。
struct MockGateway {
    template: String,
    handle: tokio::task::JoinHandle<()>,
}

impl MockGateway {
    async fn start(response: MockResponse) -> Self {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind mock gateway");
        let addr = listener.local_addr().expect("mock gateway address");
        let app = Router::new()
            .route("/{*path}", axum::routing::get(mock_gateway_handler))
            .with_state(Arc::new(response));
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock gateway server");
        });
        Self {
            template: format!("http://{addr}/{{cid}}"),
            handle,
        }
    }
}

impl Drop for MockGateway {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[derive(Clone, Default)]
struct MockResponse {
    status: StatusCode,
    body: Vec<u8>,
    delay: Duration,
    /// 要求パスがこの CID と一致しない場合は 404 を返す。
    /// `{cid}` 置換の欠落をテストで検出するために使う。
    expected_cid: Option<String>,
    /// Some の場合、実際のボディより大きな Content-Length を宣言して
    /// ストリーム途中の切断を再現する（CONTENT-02）。
    #[allow(dead_code)]
    inflated_content_length: Option<u64>,
}

async fn mock_gateway_handler(
    axum::extract::State(response): axum::extract::State<Arc<MockResponse>>,
    Path(path): Path<String>,
) -> Response {
    if let Some(expected) = &response.expected_cid
        && path.trim_start_matches('/') != expected
    {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .expect("mock not found response");
    }
    if !response.delay.is_zero() {
        tokio::time::sleep(response.delay).await;
    }
    let mut builder = Response::builder().status(response.status);
    if let Some(length) = response.inflated_content_length {
        builder = builder.header(header::CONTENT_LENGTH, length.to_string());
    }
    builder
        .body(Body::from(response.body.clone()))
        .expect("mock response")
}

fn verify_body(birth_date: &str, country: &str) -> String {
    serde_json::to_string(&VerifyRequest {
        presentation_token: "mock_token".to_string(),
        nonce: Some(crate::issue_mock_nonce_for_test()),
        public_key_der: None,
        birth_date: Some(birth_date.to_string()),
        country_code: Some(country.to_string()),
    })
    .unwrap()
}

fn with_peer(req: Request<Body>, ip: IpAddr) -> Request<Body> {
    let mut req = req;
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(ip, 3000)));
    req
}

fn multipart_upload_request(token: &str, boundary: &str, body: Vec<u8>) -> Request<Body> {
    Request::post("/content/upload")
        .header("authorization", format!("Bearer {token}"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

/// `/content/upload` の multipart 本文を組み立てる。
/// `price` 以外は常に正常値にし、400 の原因を価格検証へ限定できるようにする。
/// 署名は全ゼロ（`ALLOW_MOCK_AUTH` のモック署名）を使う。
fn multipart_upload_body(boundary: &str, file_bytes: &[u8], price: Option<&str>) -> Vec<u8> {
    multipart_upload_body_as(boundary, "upload.jxl", "image/jxl", file_bytes, price)
}

/// 許可外 MIME の検証用に、宣言するファイル名と Content-Type を指定する。
fn multipart_upload_body_as(
    boundary: &str,
    filename: &str,
    content_type: &str,
    file_bytes: &[u8],
    price: Option<&str>,
) -> Vec<u8> {
    multipart_upload_body_full(
        boundary,
        filename,
        content_type,
        file_bytes,
        price,
        "test_user",
        &[0u8; 16],
    )
}

/// multipart の全フィールドを指定して本文を組み立てる。
/// 署名検証を無効化した状態でゼロ署名・未知ユーザーを再現するために使う。
#[allow(clippy::too_many_arguments)]
fn multipart_upload_body_full(
    boundary: &str,
    filename: &str,
    content_type: &str,
    file_bytes: &[u8],
    price: Option<&str>,
    owner_hash: &str,
    signature: &[u8],
) -> Vec<u8> {
    let mut body = Vec::new();
    if let Some(price) = price {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"price\"\r\n\r\n{price}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file_bytes);
    body.extend_from_slice(
        format!(
            "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"owner_hash\"\r\n\r\n{owner_hash}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"signature\"\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(signature);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// アップロード1回分の応答（状態コードと本文）を返す。
async fn send_upload(
    app: &Router,
    token: &str,
    boundary: &str,
    body: Vec<u8>,
) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(with_peer(
            multipart_upload_request(token, boundary, body),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// SEC-01: 全レスポンス（エラー含む）へセキュリティヘッダを付与する。
#[tokio::test]
async fn test_security_headers_on_error_response() {
    let app = setup_app().await;
    let response = app
        .oneshot(with_peer(
            Request::get("/content").body(Body::empty()).unwrap(),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.headers()["X-Content-Type-Options"], "nosniff");
    assert_eq!(response.headers()["X-Frame-Options"], "DENY");
}

/// SEC-01: パブリック IPv4 は 403
#[tokio::test]
async fn test_ipv4_public_address_forbidden() {
    let _env_guard = ENV_LOCK.lock().await;
    test_remove_env("ALLOW_IPV4_LOCALHOST");
    let app = setup_app().await;
    let req = with_peer(
        Request::get("/health").body(Body::empty()).unwrap(),
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
    );
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// NET-02: クライアント注入の X-Forwarded-For では IPv4 拒否を迂回できない。
/// `TRUSTED_PROXY_CIDRS` 未設定時はピアアドレスのみを判定に使う。
#[tokio::test]
async fn test_spoofed_forwarded_header_cannot_bypass_ipv4_gate() {
    let app = setup_app().await;
    let mut req = Request::get("/health").body(Body::empty()).unwrap();
    req.headers_mut().insert(
        "x-forwarded-for",
        "2001:db8::1".parse::<HeaderValue>().unwrap(),
    );
    let req = with_peer(req, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)));
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// NET-02: IPv4-mapped IPv6（dual-stack での IPv4 ピア表現）も IPv4 として拒否する
#[tokio::test]
async fn test_ipv4_mapped_peer_is_rejected() {
    let app = setup_app().await;
    let req = with_peer(
        Request::get("/health").body(Body::empty()).unwrap(),
        "::ffff:203.0.113.1".parse().unwrap(),
    );
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_ipv6_address_allowed() {
    let app = setup_app().await;
    let req = with_peer(
        Request::get("/health").body(Body::empty()).unwrap(),
        "2001:db8::1".parse().unwrap(),
    );
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// SEC-03: ALLOWED_ORIGINS 未設定時は開発用オリジンのみ許可
#[test]
fn test_allowed_origins_defaults_to_dev_origins() {
    let origins = allowed_origins(None, true);
    assert_eq!(origins.len(), 3);
}

/// SEC-03: 本番相当（mock 無効）で未設定なら全オリジン拒否（fail-closed）
#[test]
fn test_allowed_origins_without_mock_or_config_rejects_all() {
    let origins = allowed_origins(None, false);
    assert!(
        origins.is_empty(),
        "unset ALLOWED_ORIGINS with mock auth disabled must reject every origin"
    );
}

/// SEC-03: ALLOWED_ORIGINS がカンマ区切りで反映される
#[test]
fn test_allowed_origins_parses_env_value() {
    let origins = allowed_origins(Some("https://example.com, https://app.example.com"), true);
    assert_eq!(origins.len(), 2);
    assert!(origins.iter().any(|o| o == "https://app.example.com"));
}

/// SEC-03: 不正なエントリは無視する（制御文字はヘッダ値にできない）
#[test]
fn test_allowed_origins_ignores_invalid_entries() {
    let origins = allowed_origins(Some("bad\norigin, https://ok.example.com"), true);
    assert_eq!(origins.len(), 1);
    assert_eq!(origins[0], "https://ok.example.com");
}

/// SEC-03: scheme のないエントリは無視する（ブラウザの Origin と一致しないため）
#[test]
fn test_allowed_origins_requires_scheme() {
    let origins = allowed_origins(Some("localhost:8080, https://ok.example.com, null"), true);
    assert_eq!(origins.len(), 2);
    assert_eq!(origins[0], "https://ok.example.com");
    assert_eq!(origins[1], "null");
}

/// SEC-03: 末尾スラッシュはブラウザが送らないため除去する
#[test]
fn test_allowed_origins_trims_trailing_slash() {
    let origins = allowed_origins(
        Some("https://ok.example.com/, http://localhost:3000//"),
        true,
    );
    assert_eq!(origins.len(), 2);
    assert_eq!(origins[0], "https://ok.example.com");
    assert_eq!(origins[1], "http://localhost:3000");
}

/// SEC-05: HS256 は32バイト未満の JWT 秘密鍵を拒否する（RFC 7518 §3.2）
#[test]
fn test_jwt_secret_minimum_length() {
    assert!(validate_jwt_secret(&"a".repeat(32)).is_ok());
    assert!(validate_jwt_secret("short_secret").is_err());
}

/// R4-01 / R4-04: 本番相当では OIDC と実 PINATA_JWT が必須。
#[test]
fn test_production_config_fail_closed() {
    // モック認証が有効な開発環境では警告のみ（既存の開発フローを壊さない）。
    assert!(crate::production_config_errors(true, false, None).is_empty());

    // 本番相当では不足を列挙する。
    let errors = crate::production_config_errors(false, false, None);
    assert_eq!(errors.len(), 2);
    assert!(errors.iter().any(|e| e.contains("OIDC")));
    assert!(errors.iter().any(|e| e.contains("PINATA_JWT")));

    assert_eq!(
        crate::production_config_errors(false, true, Some("real-jwt")),
        Vec::<&str>::new()
    );
    assert!(!crate::production_config_errors(false, true, Some("")).is_empty());
    assert!(
        !crate::production_config_errors(false, true, Some("mock_jwt")).is_empty(),
        "the development mock JWT must not be accepted in production"
    );
}

/// T-01: IPFS ゲートウェイ設定の既定値と無効要素の除外。
#[test]
fn test_ipfs_gateway_configuration() {
    let defaults = crate::ipfs_gateways(None);
    assert_eq!(defaults.len(), 3);
    assert!(defaults.iter().all(|gateway| gateway.contains("{cid}")));

    let parsed = crate::ipfs_gateways(Some(
        "http://a.example/{cid}, ,http://b.example/no-placeholder,http://c.example/{cid}",
    ));
    assert_eq!(
        parsed,
        vec!["http://a.example/{cid}", "http://c.example/{cid}"]
    );

    assert!(crate::ipfs_gateways(Some("")).is_empty());
    assert!(crate::ipfs_gateways(Some("ftp://a.example/{cid}")).is_empty());
    assert!(crate::ipfs_gateways(Some("file:///tmp/{cid}")).is_empty());
}

/// T-01: IPFS タイムアウトの既定値と不正値のフォールバック。
#[test]
fn test_ipfs_timeout_configuration() {
    assert_eq!(crate::ipfs_timeout(None), Duration::from_secs(15));
    assert_eq!(crate::ipfs_timeout(Some("3")), Duration::from_secs(3));
    assert_eq!(crate::ipfs_timeout(Some("0")), Duration::from_secs(15));
    assert_eq!(crate::ipfs_timeout(Some("bad")), Duration::from_secs(15));
    assert_eq!(crate::ipfs_timeout(Some("301")), Duration::from_secs(15));
    assert_eq!(crate::ipfs_timeout(Some("300")), Duration::from_secs(300));
}

/// R4-03: OIDC HTTP タイムアウトは 1〜60 秒の範囲で設定する。
#[test]
fn test_oidc_http_timeout_configuration() {
    assert_eq!(crate::oidc_http_timeout(None), Duration::from_secs(10));
    assert_eq!(
        crate::oidc_http_timeout(Some("30")),
        Duration::from_secs(30)
    );
    assert_eq!(crate::oidc_http_timeout(Some("1")), Duration::from_secs(1));
    assert_eq!(
        crate::oidc_http_timeout(Some("60")),
        Duration::from_secs(60)
    );
    assert_eq!(crate::oidc_http_timeout(Some("0")), Duration::from_secs(10));
    assert_eq!(
        crate::oidc_http_timeout(Some("61")),
        Duration::from_secs(10)
    );
    assert_eq!(
        crate::oidc_http_timeout(Some("bad")),
        Duration::from_secs(10)
    );
}

#[test]
fn test_ipfs_max_attempts_configuration() {
    assert_eq!(crate::ipfs_max_attempts(None), 1);
    assert_eq!(crate::ipfs_max_attempts(Some("3")), 3);
    assert_eq!(crate::ipfs_max_attempts(Some("0")), 1);
    assert_eq!(crate::ipfs_max_attempts(Some("6")), 1);
    assert_eq!(crate::ipfs_max_attempts(Some("bad")), 1);
    assert_eq!(crate::ipfs_max_attempts(Some("5")), 5);
}

/// ログ用 URL から userinfo と query を除去する。
#[test]
fn test_redact_url_removes_credentials_and_query() {
    assert_eq!(
        crate::redact_url("https://user:pass@example.com/path?token=secret"),
        "https://example.com/path"
    );
    assert_eq!(
        crate::redact_url("https://example.com/path?token=secret"),
        "https://example.com/path"
    );
    assert_eq!(crate::redact_url("not a url?token=secret"), "not a url");
    // フラグメント側の秘密も除去する（パース成功・失敗の両経路）。
    assert_eq!(
        crate::redact_url("https://example.com/path?token=secret#frag=secret"),
        "https://example.com/path"
    );
    assert_eq!(
        crate::redact_url("https://example.com/path#frag=secret"),
        "https://example.com/path"
    );
    assert_eq!(crate::redact_url("not a url#token=secret"), "not a url");
}

/// CID の URL パス注入を拒否する。
#[test]
fn test_cid_validation_rejects_path_characters() {
    assert!(crate::is_safe_cid("QmTestCID"));
    assert!(crate::is_safe_cid(
        "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
    ));
    assert!(!crate::is_safe_cid("../etc/passwd"));
    assert!(!crate::is_safe_cid("Qm?x=1"));
    assert!(!crate::is_safe_cid("Qm#fragment"));
    assert!(!crate::is_safe_cid("Qm/extra"));
    assert!(!crate::is_safe_cid(""));
    assert!(!crate::is_safe_cid(&"a".repeat(129)));
}

/// SEC-04: 500系エラーの内部詳細は既定で隠し、EXPOSE_INTERNAL_ERRORS=1 で公開する
#[tokio::test]
async fn test_internal_error_details_visibility() {
    let _env_guard = ENV_LOCK.lock().await;
    test_remove_env("EXPOSE_INTERNAL_ERRORS");

    // Default: generic message without internal details
    let hidden = AppError::Internal("no such table: users".to_string()).into_response();
    assert_eq!(hidden.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = to_bytes(hidden.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        !text.contains("no such table"),
        "details must be hidden: {text}"
    );
    assert!(text.contains("Internal server error"));

    // Opt-in: details visible for debugging
    test_set_env("EXPOSE_INTERNAL_ERRORS", "1");
    let exposed = AppError::Internal("no such table: users".to_string()).into_response();
    let body = to_bytes(exposed.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("no such table"),
        "details must be visible: {text}"
    );

    test_remove_env("EXPOSE_INTERNAL_ERRORS");
}

#[tokio::test]
async fn test_auth_login_returns_nonce() {
    let app = setup_app().await;
    let response = app
        .oneshot(with_peer(
            Request::post("/auth/login").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// R1-21: モック認証が無効な構成では /auth/login を公開しない。
#[tokio::test]
async fn test_auth_login_requires_mock_auth() {
    let (app, _db) = setup_app_with_options(Vec::new(), Duration::from_secs(15), false).await;
    let response = app
        .oneshot(with_peer(
            Request::post("/auth/login").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn test_read_routes_require_authentication() {
    let paths = [
        "/content",
        "/content/QmTestCID",
        "/content/QmTestCID/audit",
        "/transactions",
        "/mod/reports",
    ];

    for path in paths {
        let app = setup_app().await;
        let response = app
            .oneshot(with_peer(
                Request::get(path).body(Body::empty()).unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "Unauthenticated request to {path} must be rejected"
        );
    }
}

#[tokio::test]
async fn test_authenticated_user_can_list_contents() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let response = app
        .oneshot(with_peer(
            Request::get("/content")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_auth_verify_authorized_with_token() {
    let app = setup_app().await;
    let response = app
        .oneshot(with_peer(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(verify_body("2000-01-01", "JP")))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let body: VerifyResponse = serde_json::from_slice(&body_bytes).unwrap();
    assert!(body.authorized);
    assert!(body.token.is_some());
}

/// 開発用モック認証では nonce が必須で、再利用は拒否される。
#[tokio::test]
async fn test_mock_auth_nonce_is_single_use() {
    let app = setup_app().await;

    // /auth/login が発行した nonce を取得する。
    let login = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/login").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(login.into_body(), 4096).await.unwrap()).unwrap();
    let nonce = started["nonce"].as_str().unwrap().to_string();

    let body = serde_json::to_string(&VerifyRequest {
        presentation_token: "mock_token".to_string(),
        nonce: Some(nonce.clone()),
        public_key_der: None,
        birth_date: Some("2000-01-01".to_string()),
        country_code: Some("JP".to_string()),
    })
    .unwrap();
    let send = |body: String| {
        let app = app.clone();
        async move {
            app.oneshot(with_peer(
                Request::post("/auth/verify")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap()
            .status()
        }
    };

    // 1回目は成功（mock_token + public_key_der 無しは nonce 経由で通る）。
    assert_eq!(send(body.clone()).await, StatusCode::OK);
    // 同じ nonce の再利用は 400。
    assert_eq!(send(body).await, StatusCode::BAD_REQUEST);
}

/// nonce 欠落のリクエストは 400。
#[tokio::test]
async fn test_mock_auth_requires_nonce() {
    let app = setup_app().await;
    let body = json!({
        "presentation_token": "mock_token",
        "birth_date": "2000-01-01",
        "country_code": "JP",
    });
    let response = app
        .oneshot(with_peer(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// SEC-03: 18歳未満は 403
#[tokio::test]
async fn test_auth_verify_underage_forbidden() {
    let app = setup_app().await;
    let response = app
        .oneshot(with_peer(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(verify_body("2015-01-01", "JP")))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// birth_date 欠落の認証要求は 400
#[tokio::test]
async fn test_auth_verify_missing_birth_date_rejected() {
    let app = setup_app().await;
    let body = json!({
        "presentation_token": "mock_token",
        "public_key_der": null,
        "country_code": "JP"
    });

    let response = app
        .oneshot(with_peer(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("birth_date is required"),
        "client must receive the missing-field rejection: {text}"
    );
}

/// country_code 欠落の認証要求は 400
#[tokio::test]
async fn test_auth_verify_missing_country_code_rejected() {
    let app = setup_app().await;
    let body = json!({
        "presentation_token": "mock_token",
        "public_key_der": null,
        "birth_date": "2000-01-01"
    });

    let response = app
        .oneshot(with_peer(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("country_code is required"),
        "client must receive the missing-field rejection: {text}"
    );
}

/// 解析不能な birth_date は署名検証より先に 400 で拒否される
#[tokio::test]
async fn test_auth_verify_rejects_malformed_birth_date() {
    let app = setup_app().await;

    let response = app
        .oneshot(with_peer(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(verify_body("not-a-date", "JP")))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("Invalid birth_date"),
        "client must receive the malformed-date rejection: {text}"
    );
}

/// AUTH-02: 期限切れ JWT と exp 欠落トークンは 401 で拒否される。
#[tokio::test]
async fn test_expired_and_missing_exp_tokens_rejected() {
    let app = setup_app().await;

    // 期限切れ (exp が過去) のトークンを手組みで署名する。
    let expired = encode(
        &Header::default(),
        &json!({"sub": "test_user", "exp": 1u64}),
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()).unwrap(),
    )
    .unwrap();
    // exp 欠落トークン（Claims の必須フィールドが無い）。
    let missing_exp = encode(
        &Header::default(),
        &json!({"sub": "test_user"}),
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()).unwrap(),
    )
    .unwrap();

    for (name, token) in [("expired", expired), ("missing_exp", missing_exp)] {
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::get("/content")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "case={name}");
    }
}

/// SEC-02: 無効な JPKI 署名でのアクセス拒否
#[tokio::test]
async fn test_invalid_jpki_signature_rejected() {
    let app = setup_app().await;
    let invalid_body = json!({
        "presentation_token": "invalid_signature_token",
        "public_key_der": null,
        "birth_date": "2000-01-01",
        "country_code": "JP"
    });

    let response = app
        .oneshot(with_peer(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(invalid_body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    // 無効な署名は 401 または 400 で拒否される
    assert!(
        response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::BAD_REQUEST,
        "Invalid JPKI signature should be rejected"
    );
}

/// GOV-03: BAN ユーザーは再認証不可
#[tokio::test]
async fn test_banned_user_blocked_directly() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;
    db.execute(
        "INSERT INTO users (user_hash, is_banned) VALUES ('banned_hash', TRUE)",
        (),
    )
    .await
    .unwrap();
    let result = ensure_user_not_banned(&db, "banned_hash").await;
    assert!(result.is_err());
}

/// GOV-01: 報告後に Reported へ遷移
#[tokio::test]
async fn test_mod_report_sets_reported_status() {
    let (app, db) = setup_app_with_db().await;
    let report_body = json!({
        "report_id": 0,
        "target_cid": "QmTestCID",
        "reason": "Illegal content",
        "status": "Pending"
    });

    let response = app
        .oneshot(with_peer(
            Request::post("/mod/report")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, "reporter").await
                    ),
                )
                .body(Body::from(report_body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmTestCID'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(status, "Reported");
}

/// レビュー中の重複通報は拒否し、通報行を増やさない
#[tokio::test]
async fn test_duplicate_report_rejected() {
    let (app, db) = setup_app_with_db().await;
    let report_body = json!({
        "report_id": 0,
        "target_cid": "QmTestCID",
        "reason": "First report",
        "status": "Pending"
    });

    let first = app
        .clone()
        .oneshot(with_peer(
            Request::post("/mod/report")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, "reporter").await
                    ),
                )
                .body(Body::from(report_body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .oneshot(with_peer(
            Request::post("/mod/report")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, "reporter2").await
                    ),
                )
                .body(Body::from(report_body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(
        second.status(),
        StatusCode::BAD_REQUEST,
        "duplicate report while under review must be rejected"
    );

    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM reports WHERE target_cid = 'QmTestCID'",
            (),
        )
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1, "only the first report must be recorded");
}

/// 存在しないコンテンツへの通報は 404
#[tokio::test]
async fn test_report_missing_content_returns_404() {
    let (app, _db) = setup_app_with_db().await;
    let report_body = json!({
        "report_id": 0,
        "target_cid": "QmMissingContent",
        "reason": "No such content",
        "status": "Pending"
    });

    let response = app
        .oneshot(with_peer(
            Request::post("/mod/report")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, "reporter").await
                    ),
                )
                .body(Body::from(report_body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// 不正なCIDの通報は 400
#[tokio::test]
async fn test_report_rejects_invalid_cid() {
    let (app, _db) = setup_app_with_db().await;
    let report_body = json!({
        "report_id": 0,
        "target_cid": "bad-cid",
        "reason": "Invalid CID",
        "status": "Pending"
    });

    let response = app
        .oneshot(with_peer(
            Request::post("/mod/report")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, "reporter").await
                    ),
                )
                .body(Body::from(report_body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// 通報リクエストを送信し、レスポンスを返す
async fn post_report(app: &Router, cid: &str, reason: &str) -> Response {
    let body = json!({
        "report_id": 0,
        "target_cid": cid,
        "reason": reason,
        "status": "Pending"
    });
    app.clone()
        .oneshot(with_peer(
            Request::post("/mod/report")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, "reporter").await
                    ),
                )
                .body(Body::from(body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap()
}

/// 審査中コンテンツへの二重通報は 400 で拒否され、reports は 1 件のまま
#[tokio::test]
async fn test_report_duplicate_rejected() {
    let (app, db) = setup_app_with_db().await;

    let first = post_report(&app, "QmTestCID", "first report").await;
    assert_eq!(
        first.status(),
        StatusCode::OK,
        "first report must be accepted"
    );

    let second = post_report(&app, "QmTestCID", "second report").await;
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("already under review"),
        "client must receive the duplicate-report rejection: {text}"
    );

    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM reports WHERE target_cid = 'QmTestCID'",
            (),
        )
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1, "duplicate report must not create a second row");
}

/// 空または過大な reason の通報は 400 で拒否され、有効な reason は前後空白を除去して保存する
#[tokio::test]
async fn test_report_reason_validated() {
    let (app, db) = setup_app_with_db().await;

    for reason in ["", "   ", &"x".repeat(2001)] {
        let response = post_report(&app, "QmTestCID", reason).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "reason {reason:?} must be rejected"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("Invalid report reason"),
            "client must receive the reason rejection: {text}"
        );
    }

    let accepted = post_report(&app, "QmTestCID", "  legit concern  ").await;
    assert_eq!(
        accepted.status(),
        StatusCode::OK,
        "valid reason must be accepted"
    );

    let mut rows = db
        .query(
            "SELECT reason FROM reports WHERE target_cid = 'QmTestCID'",
            (),
        )
        .await
        .unwrap();
    let stored: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(stored, "legit concern", "stored reason must be trimmed");
}

/// INT-02: zip は拒否
#[test]
fn test_reject_zip_content_type() {
    assert!(validate_upload_content_type(Some("application/zip")).is_err());
    assert!(validate_upload_content_type(Some("image/jxl")).is_ok());
    assert!(validate_upload_content_type(Some("text/markdown")).is_ok());
    assert!(validate_upload_content_type(Some("image/png")).is_err());
    assert!(validate_upload_content_type(Some("text/html")).is_err());
    assert!(validate_upload_content_type(Some("application/x-image/jxl")).is_err());
    assert!(validate_upload_content_type(Some("text/markdown; charset=utf-8")).is_ok());
}

#[test]
fn test_upload_bytes_match_declared_content_kind() {
    assert!(validate_upload_content("Illustration", &[0xff, 0x0a, 0x01]).is_ok());
    assert!(validate_upload_content("Illustration", b"not an image").is_err());
    assert!(validate_upload_content("Novel", b"# title\n").is_ok());
    assert!(validate_upload_content("Novel", &[0xff, 0xfe]).is_err());
}

#[tokio::test]
async fn test_upload_rejects_unknown_multipart_fields() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let boundary = "test-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"unexpected\"\r\n\r\nvalue\r\n--{boundary}--\r\n"
    );
    let response = app
        .oneshot(with_peer(
            Request::post("/content/upload")
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body.into_bytes()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// CONTENT-01: 実 RSA 鍵で署名したアップロードが受理され、改ざんが拒否される。
#[tokio::test]
async fn test_upload_accepts_real_rsa_signature() {
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::{DecodePrivateKey, EncodePublicKey};
    use rsa::signature::{SignatureEncoding, Signer};

    let private_pem = test_rsa_private_pem();
    let private_key = PrivateKey::from_pkcs8_pem(&private_pem).unwrap();
    let public_der = private_key
        .to_public_key()
        .to_public_key_der()
        .unwrap()
        .to_vec();

    let (app, db) = setup_app_with_db().await;
    db.execute(
        "UPDATE users SET public_key_der = ? WHERE user_hash = 'test_user'",
        [public_der],
    )
    .await
    .unwrap();

    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let file_bytes = vec![0xFF, 0x0A, 0x01, 0x42];
    let signing_key = SigningKey::<Sha256>::new(private_key);
    let signature = signing_key.sign(&file_bytes).to_bytes().to_vec();

    let boundary = "real-rsa-signature";
    let body = multipart_upload_body_full(
        boundary,
        "upload.jxl",
        "image/jxl",
        &file_bytes,
        Some("0"),
        "test_user",
        &signature,
    );
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/content/upload")
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a valid real signature must pass"
    );

    // 同じ署名を別バイト列で使い回すと拒否される。
    let tampered_bytes = vec![0xFF, 0x0A, 0x01, 0x43];
    let boundary = "reused-signature";
    let body = multipart_upload_body_full(
        boundary,
        "upload.jxl",
        "image/jxl",
        &tampered_bytes,
        Some("0"),
        "test_user",
        &signature,
    );
    let response = app
        .oneshot(with_peer(
            Request::post("/content/upload")
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// CONTENT-01: 同一フィールドの重複は ambiguous な解釈を避けるため 400 で拒否する。
#[tokio::test]
async fn test_upload_rejects_duplicate_fields() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    // price を2回送る（片方だけなら正常な本文）。
    let duplicate_price = |boundary: &str| -> Vec<u8> {
        let mut body = Vec::new();
        for price in ["0", "100"] {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"price\"\r\n\r\n{price}\r\n"
                )
                .as_bytes(),
            );
        }
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"upload.jxl\"\r\nContent-Type: image/jxl\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&[0xFF, 0x0A, 0x00, 0x01]);
        body.extend_from_slice(
            format!(
                "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"owner_hash\"\r\n\r\ntest_user\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"signature\"\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&[0u8; 16]);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        body
    };

    let boundary = "duplicate-price";
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/content/upload")
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(duplicate_price(boundary)))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("Duplicate price field"),
        "rejection must name the duplicated field: {}",
        String::from_utf8_lossy(&body)
    );
}

/// CONTENT-01: file / owner_hash / signature の重複も 400 で拒否する。
#[tokio::test]
async fn test_upload_rejects_duplicate_other_fields() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    // 重複させたいフィールド名と、拒否メッセージに含まれる語。
    let cases = [
        ("file", "Duplicate file field"),
        ("owner_hash", "Duplicate owner_hash field"),
        ("signature", "Duplicate signature field"),
    ];

    for (duplicate, expected) in cases {
        let boundary = "dup-other";
        let mut body = Vec::new();
        let part = |name: &str, value: &str| {
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
        };
        // 対象フィールドを2回、それ以外を1回ずつ送る。
        for _ in 0..2 {
            if duplicate == "file" {
                body.extend_from_slice(
                    format!(
                        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"upload.jxl\"\r\nContent-Type: image/jxl\r\n\r\n"
                    )
                    .as_bytes(),
                );
                body.extend_from_slice(&[0xFF, 0x0A, 0x00, 0x01]);
                body.extend_from_slice(b"\r\n");
            } else {
                body.extend_from_slice(part(duplicate, "x").as_bytes());
            }
        }
        if duplicate != "file" {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"upload.jxl\"\r\nContent-Type: image/jxl\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(&[0xFF, 0x0A, 0x00, 0x01]);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(part("owner_hash", "test_user").as_bytes());
        body.extend_from_slice(part("signature", "sig").as_bytes());
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let response = app
            .clone()
            .oneshot(with_peer(
                Request::post("/content/upload")
                    .header("authorization", format!("Bearer {token}"))
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "field={duplicate}"
        );
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains(expected), "field={duplicate}: {text}");
    }
}

/// CONTENT-01: 負価格・非整数・64bit範囲外の価格は保存前に400で拒否する。
/// 価格以外は正常な multipart 本文で送信し、400 が価格検証そのものに
/// 由来することを保証する（価格検証を外すと200で保存される）。
#[tokio::test]
async fn test_upload_rejects_invalid_price() {
    let (app, db) = setup_app_with_db().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let invalid_prices = [
        ("-1", "price must not be negative"),
        ("1.5", "price must be an integer"),
        ("not-an-integer", "price must be an integer"),
        // 64bit整数の範囲外は切り捨てずに拒否する。
        ("9223372036854775808", "price must be an integer"),
        ("-9223372036854775809", "price must be an integer"),
        ("-9223372036854775808", "price must not be negative"),
        // 注文上限超过は注文不可能な価格として拒否する。
        ("1000001", "price must not exceed 1000000"),
        ("9223372036854775807", "price must not exceed 1000000"),
    ];
    for (index, (price, expected_error)) in invalid_prices.into_iter().enumerate() {
        let boundary = format!("invalid-price-{index}");
        let body = multipart_upload_body(&boundary, &[0xFF, 0x0A, 0x00, index as u8], Some(price));
        let (status, response_body) = send_upload(&app, &token, &boundary, body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "price {price:?} must be rejected"
        );
        assert!(
            response_body.contains(expected_error),
            "price {price:?} must fail on price validation, got {response_body}"
        );
    }

    // 拒否は外部保存より前に起きる（fixture の1件のみが残る）。
    let mut rows = db.query("SELECT COUNT(*) FROM contents", ()).await.unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1, "rejected uploads must not be persisted");
}

/// CONTENT-01: 許可外 MIME は400で拒否する。本文は有効な JPEG XL シグネチャにし、
/// 400 が Content-Type 検査に由来することを保証する（検査を外すと200で保存される）。
#[tokio::test]
async fn test_upload_rejects_unsupported_media_type() {
    let (app, db) = setup_app_with_db().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let boundary = "unsupported-media-type";
    let body = multipart_upload_body_as(
        boundary,
        "payload.zip",
        "application/zip",
        &[0xFF, 0x0A, 0x01, 0x02, 0x03],
        None,
    );

    let (status, response_body) = send_upload(&app, &token, boundary, body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "application/zip must be rejected, got {response_body}"
    );
    assert!(
        response_body.contains("Unsupported content type"),
        "application/zip must fail on content type validation, got {response_body}"
    );

    let mut rows = db.query("SELECT COUNT(*) FROM contents", ()).await.unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1, "rejected uploads must not be persisted");
}

/// CONTENT-01: アップロード本文が 10 MiB を超える場合は 413 で拒否される。
#[tokio::test]
async fn test_upload_rejects_body_over_10mib() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let boundary = "oversize-body";
    let mut file_bytes = vec![0xFF, 0x0A];
    file_bytes.resize(10 * 1024 * 1024 + 1, 0x00);
    let body = multipart_upload_body(boundary, &file_bytes, Some("0"));

    let response = app
        .oneshot(with_peer(
            Request::post("/content/upload")
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header("content-length", body.len().to_string())
                .body(Body::from(body))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    // Content-Length 検査ミドルウェアが Multipart extractor より先に 413 を返す。
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "bodies over 10 MiB must be rejected: {}",
        String::from_utf8_lossy(&body)
    );
}

/// CONTENT-01: 正常な価格境界（未指定=既定値、明示的な下限0、注文上限）は
/// 受理され、指定した値がそのまま保存される。
#[tokio::test]
async fn test_upload_accepts_valid_price_boundaries() {
    let cases: [(Option<&str>, i64); 3] = [
        (None, 0),
        (Some("0"), 0),
        (Some("1000000"), MAX_ORDER_JPY as i64),
    ];

    for (index, (price, expected)) in cases.into_iter().enumerate() {
        let (app, db) = setup_app_with_db().await;
        let token = generate_token(TEST_JWT_SECRET, "test_user").await;
        let boundary = format!("valid-price-{index}");
        let body = multipart_upload_body(&boundary, &[0xFF, 0x0A, 0x01, index as u8], price);
        let (status, response_body) = send_upload(&app, &token, &boundary, body).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "price {price:?} must be accepted, got {response_body}"
        );

        let cid: String = serde_json::from_str(&response_body).unwrap();
        let mut rows = db
            .query("SELECT price FROM contents WHERE cid = ?", [cid])
            .await
            .unwrap();
        let stored: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(
            stored, expected,
            "price {price:?} must be stored unmodified"
        );
    }
}

/// 同一コンテンツの再アップロードは 400（CID主キー衝突を500にしない）
#[tokio::test]
async fn test_upload_duplicate_content_rejected() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let boundary = "dup-boundary";

    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.jxl\"\r\nContent-Type: image/jxl\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&[0xFF, 0x0A, 0x01, 0x02, 0x03]);
    body.extend_from_slice(
        format!(
            "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"owner_hash\"\r\n\r\ntest_user\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"signature\"\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&[0u8; 16]);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let send = |app: Router| {
        let body = body.clone();
        let token = token.clone();
        async move {
            app.oneshot(with_peer(
                Request::post("/content/upload")
                    .header("authorization", format!("Bearer {token}"))
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap()
        }
    };

    let first = send(app.clone()).await;
    assert_eq!(
        first.status(),
        StatusCode::OK,
        "first upload should succeed"
    );

    let second = send(app.clone()).await;
    assert_eq!(
        second.status(),
        StatusCode::BAD_REQUEST,
        "duplicate upload must be rejected"
    );
}

/// 必須フィールド（file / owner_hash / signature）の欠落は 400 で拒否される
#[tokio::test]
async fn test_upload_rejects_missing_required_fields() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let boundary = "missing-required-fields";

    let file_part = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.jxl\"\r\nContent-Type: image/jxl\r\n\r\n\r\n"
    )
    .as_bytes()
    .to_vec();
    let owner_part = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"owner_hash\"\r\n\r\ntest_user\r\n"
    )
    .as_bytes()
    .to_vec();
    let mut signature_part =
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"signature\"\r\n\r\n")
            .as_bytes()
            .to_vec();
    signature_part.extend_from_slice(&[0u8; 16]);
    signature_part.extend_from_slice(b"\r\n");

    let build = |parts: [&[u8]; 2]| -> Vec<u8> {
        let mut body = Vec::new();
        for part in parts {
            body.extend_from_slice(part);
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        body
    };

    let cases = [
        (
            build([&owner_part, &signature_part]),
            "Missing file",
            "file must be required",
        ),
        (
            build([&file_part, &signature_part]),
            "Missing owner_hash",
            "owner_hash must be required",
        ),
        (
            build([&file_part, &owner_part]),
            "Missing signature",
            "signature must be required",
        ),
    ];
    for (body, expected, context) in cases {
        let (status, response_body) = send_upload(&app, &token, boundary, body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{context}: {response_body}"
        );
        assert!(
            response_body.contains(expected),
            "{context}: expected {expected:?} in {response_body}"
        );
    }
}

/// トークンの sub と owner_hash が不一致のアップロードは 401 で拒否される
#[tokio::test]
async fn test_upload_rejects_owner_mismatch() {
    let (app, db) = setup_app_with_db().await;
    // トークンは別の sub で発行し、owner_hash は test_user のまま送る。
    let token = generate_token(TEST_JWT_SECRET, "someone_else").await;
    let boundary = "owner-mismatch";
    let body = multipart_upload_body(boundary, &[0xFF, 0x0A, 0x01, 0x02], None);

    let (status, response_body) = send_upload(&app, &token, boundary, body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        response_body.contains("Token sub mismatch"),
        "client must receive the sub mismatch rejection: {response_body}"
    );

    let mut rows = db.query("SELECT COUNT(*) FROM contents", ()).await.unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        count, 1,
        "a mismatched owner must not insert content (only QmTestCID exists)"
    );
}

/// BAN 済みユーザーのアップロードは 403 で拒否される
#[tokio::test]
async fn test_upload_rejects_banned_user() {
    let (app, db) = setup_app_with_db().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    db.execute(
        "UPDATE users SET is_banned = TRUE WHERE user_hash = 'test_user'",
        (),
    )
    .await
    .unwrap();

    let boundary = "banned-user";
    let body = multipart_upload_body(boundary, &[0xFF, 0x0A, 0x01, 0x03], None);

    let (status, response_body) = send_upload(&app, &token, boundary, body).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        response_body.contains("User is banned"),
        "client must receive the ban rejection: {response_body}"
    );
}

#[test]
fn test_age_validation_unit() {
    let policy = common::age::AgePolicy::default();
    assert!(validate_age(&policy, "2000-01-01", "JP").is_ok());
    assert!(validate_age(&policy, "2015-01-01", "JP").is_err());
}

/// GOV-02: 3票でコンセンサス → Rejected
#[tokio::test]
async fn test_vote_consensus_rejects_content() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;

    // Insert required users first
    db.execute("INSERT INTO users (user_hash) VALUES ('owner')", ())
        .await
        .unwrap();
    db.execute("INSERT INTO users (user_hash) VALUES ('juror1')", ())
        .await
        .unwrap();
    db.execute("INSERT INTO users (user_hash) VALUES ('juror2')", ())
        .await
        .unwrap();
    db.execute("INSERT INTO users (user_hash) VALUES ('juror3')", ())
        .await
        .unwrap();

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmVoteTest', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmVoteTest', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    let app = build_router(shared_state);

    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};

    for _ in 0..3 {
        let mut rng = TestRng::new();
        let private_key = PrivateKey::new(&mut rng, 2048).unwrap();
        let public_der = private_key
            .to_public_key()
            .to_public_key_der()
            .unwrap()
            .to_vec();
        let juror_hash = sha256::digest(&public_der);
        db.execute(
            "INSERT INTO users (user_hash, public_key_der) VALUES (?, ?)",
            (juror_hash.clone(), public_der),
        )
        .await
        .unwrap();

        let message = json!({"report_id": 1, "result": "Reject"}).to_string();
        let signing_key = SigningKey::<Sha256>::new(private_key.clone());
        let signature = signing_key.sign(message.as_bytes()).to_vec();

        let vote_body = json!({
            "report_id": 1,
            "juror_hash": juror_hash,
            "result": "Reject",
            "signature": signature
        });

        let response = app
            .clone()
            .oneshot(with_peer(
                Request::post("/mod/vote")
                    .header("content-type", "application/json")
                    .header(
                        "Authorization",
                        format!(
                            "Bearer {}",
                            generate_token(TEST_JWT_SECRET, &juror_hash).await
                        ),
                    )
                    .body(Body::from(vote_body.to_string()))
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "vote should succeed");
    }

    // ADR-0004: 3票でも投票期間中は確定しない（即時確定の廃止）
    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmVoteTest'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status, "Reported",
        "no early conclusion before the voting deadline"
    );

    force_conclude(&db, 1).await;

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmVoteTest'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(status, "Rejected");
}

/// ADR-0004: 同数（Approve 2 / Reject 2）は Rejected に倒れる。
/// 現行実装の即時確定（3票）では到達しないため、確定ロジックを直接検証する。
#[tokio::test]
async fn test_consensus_tie_resolves_to_rejected() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;

    for user in ["owner", "juror1", "juror2", "juror3", "juror4"] {
        db.execute("INSERT INTO users (user_hash) VALUES (?)", [user])
            .await
            .unwrap();
    }
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmTieTest', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmTieTest', 'test')",
        (),
    )
    .await
    .unwrap();

    expire_report(&db, 1).await;
    let tx = db.transaction().await.unwrap();
    for (voter, result) in [
        ("juror1", "Approve"),
        ("juror2", "Approve"),
        ("juror3", "Reject"),
        ("juror4", "Reject"),
    ] {
        tx.execute(
            "INSERT INTO votes (report_id, voter_hash, vote_result) VALUES (1, ?, ?)",
            (voter, result),
        )
        .await
        .unwrap();
    }
    process_report_conclusion(&tx, 1).await.unwrap();
    tx.commit().await.unwrap();

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmTieTest'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status, "Rejected",
        "a tie must resolve to Rejected per ADR-0004"
    );
}

/// MOD-01: 同一ユーザーの同一通報への再投票は拒否
#[tokio::test]
async fn test_duplicate_vote_rejected() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;

    db.execute("INSERT INTO users (user_hash) VALUES ('owner')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmDupVote', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmDupVote', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    let app = build_router(shared_state);

    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};

    let mut rng = TestRng::new();
    let private_key = PrivateKey::new(&mut rng, 2048).unwrap();
    let public_der = private_key
        .to_public_key()
        .to_public_key_der()
        .unwrap()
        .to_vec();
    let juror_hash = sha256::digest(&public_der);
    db.execute(
        "INSERT INTO users (user_hash, public_key_der) VALUES (?, ?)",
        (juror_hash.clone(), public_der),
    )
    .await
    .unwrap();

    let message = json!({"report_id": 1, "result": "Approve"}).to_string();
    let signing_key = SigningKey::<Sha256>::new(private_key.clone());
    let signature = signing_key.sign(message.as_bytes()).to_vec();
    let vote_body = json!({
        "report_id": 1,
        "juror_hash": juror_hash,
        "result": "Approve",
        "signature": signature
    });

    let send_vote = |app: Router| {
        let body = vote_body.to_string();
        let token_sub = juror_hash.clone();
        async move {
            app.oneshot(with_peer(
                Request::post("/mod/vote")
                    .header("content-type", "application/json")
                    .header(
                        "Authorization",
                        format!(
                            "Bearer {}",
                            generate_token(TEST_JWT_SECRET, &token_sub).await
                        ),
                    )
                    .body(Body::from(body))
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap()
        }
    };

    let first = send_vote(app.clone()).await;
    assert_eq!(first.status(), StatusCode::OK, "first vote should succeed");

    let second = send_vote(app.clone()).await;
    assert_eq!(
        second.status(),
        StatusCode::BAD_REQUEST,
        "duplicate vote must be rejected"
    );

    // DB-level unique index also guards against duplicates
    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1, "exactly one vote must be stored");
}

/// 陪審員としてユーザーを登録し、投票署名用のRSA秘密鍵を返す
async fn insert_juror(db: &libsql::Connection) -> (String, PrivateKey) {
    let mut rng = TestRng::new();
    let private_key = PrivateKey::new(&mut rng, 2048).unwrap();
    let public_der = private_key
        .to_public_key()
        .to_public_key_der()
        .unwrap()
        .to_vec();
    let juror_hash = sha256::digest(&public_der);
    db.execute(
        "INSERT INTO users (user_hash, public_key_der) VALUES (?, ?)",
        (juror_hash.clone(), public_der),
    )
    .await
    .unwrap();
    (juror_hash, private_key)
}

/// 投票期限を過ぎた状態にする（ADR-0004 の期限ベース確定を test で再現）
async fn expire_report(db: &libsql::Connection, report_id: i64) {
    db.execute(
        "UPDATE reports SET deadline_at = datetime('now', '-1 day') WHERE report_id = ?",
        [report_id],
    )
    .await
    .unwrap();
}

/// 期限を過ぎた通報を確定処理へ流す（本番では finalize エンドポイント/スケジューラが担う）
async fn force_conclude(db: &libsql::Connection, report_id: i64) {
    expire_report(db, report_id).await;
    let tx = db.transaction().await.unwrap();
    process_report_conclusion(&tx, report_id).await.unwrap();
    tx.commit().await.unwrap();
}

/// 署名付きの投票リクエストを送信し、ステータスコードを返す
async fn send_signed_vote(
    app: &Router,
    report_id: i64,
    result: &str,
    juror_hash: &str,
    private_key: &PrivateKey,
) -> StatusCode {
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};

    let message = crate::moderation_vote_message(report_id, result);
    let signing_key = SigningKey::<Sha256>::new(private_key.clone());
    let signature = signing_key.sign(message.as_bytes()).to_vec();
    let vote_body = json!({
        "report_id": report_id,
        "juror_hash": juror_hash,
        "result": result,
        "signature": signature
    });

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/mod/vote")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, juror_hash).await
                    ),
                )
                .body(Body::from(vote_body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    response.status()
}

/// MOD-02: 確定後の追加投票は拒否され、確定状態は変わらない
#[tokio::test]
async fn test_vote_after_conclusion_rejected() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;

    db.execute("INSERT INTO users (user_hash) VALUES ('owner')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmConcludedVote', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmConcludedVote', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    let app = build_router(shared_state);

    // Three Reject votes conclude the report
    for _ in 0..3 {
        let (juror_hash, key) = insert_juror(&db).await;
        assert_eq!(
            send_signed_vote(&app, 1, "Reject", &juror_hash, &key).await,
            StatusCode::OK,
            "votes before conclusion should succeed"
        );
    }

    // ADR-0004: 投票期間終了後に確定する（3票到達時点では確定しない）
    force_conclude(&db, 1).await;

    let mut rows = db
        .query(
            "SELECT status FROM contents WHERE cid = 'QmConcludedVote'",
            (),
        )
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(status, "Rejected");

    // A later Approve vote must be rejected and must not flip the status
    let (late_juror, late_key) = insert_juror(&db).await;
    assert_eq!(
        send_signed_vote(&app, 1, "Approve", &late_juror, &late_key).await,
        StatusCode::BAD_REQUEST,
        "votes after conclusion must be rejected"
    );

    let mut rows = db
        .query(
            "SELECT status FROM contents WHERE cid = 'QmConcludedVote'",
            (),
        )
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(status, "Rejected", "concluded status must not change");

    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 3, "the late vote must not be recorded");
}

/// 存在しない report_id への投票は 404
#[tokio::test]
async fn test_vote_for_missing_report_returns_404() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    let app = build_router(shared_state);

    let (juror_hash, key) = insert_juror(&db).await;
    assert_eq!(
        send_signed_vote(&app, 9999, "Approve", &juror_hash, &key).await,
        StatusCode::NOT_FOUND,
        "unknown report must return 404"
    );
}

/// 投票署名メッセージは report_id と result の両方に依存する（転用防止の基盤）。
#[test]
fn test_moderation_vote_message_binds_report_and_result() {
    let a = crate::moderation_vote_message(1, "Approve");
    assert_ne!(a, crate::moderation_vote_message(2, "Approve"));
    assert_ne!(a, crate::moderation_vote_message(1, "Reject"));
    assert_eq!(a, crate::moderation_vote_message(1, "Approve"));
}

/// 無効署名の投票は 401 で拒否し、票を記録しない
#[tokio::test]
async fn test_vote_with_invalid_signature_rejected() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;

    db.execute("INSERT INTO users (user_hash) VALUES ('owner')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmBadSig', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmBadSig', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    let app = build_router(shared_state);

    let (juror_hash, _key) = insert_juror(&db).await;
    let vote_body = json!({
        "report_id": 1,
        "juror_hash": juror_hash,
        "result": "Approve",
        "signature": vec![0xABu8; 256]
    });

    let response = app
        .oneshot(with_peer(
            Request::post("/mod/vote")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, &juror_hash).await
                    ),
                )
                .body(Body::from(vote_body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("Invalid digital signature"),
        "client must receive a generic 401: {text}"
    );

    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 0, "a failed signature must not record a vote");
}

/// 不正な投票ペイロード（report_id <= 0、未知の result）は 400 で拒否され、票を記録しない
#[tokio::test]
async fn test_vote_with_invalid_payload_rejected() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;

    db.execute("INSERT INTO users (user_hash) VALUES ('owner')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmBadPayload', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmBadPayload', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    let app = build_router(shared_state);

    let (juror_hash, key) = insert_juror(&db).await;

    assert_eq!(
        send_signed_vote(&app, 0, "Approve", &juror_hash, &key).await,
        StatusCode::BAD_REQUEST,
        "non-positive report_id must be rejected"
    );
    assert_eq!(
        send_signed_vote(&app, 1, "Maybe", &juror_hash, &key).await,
        StatusCode::BAD_REQUEST,
        "unknown result value must be rejected"
    );

    let mut rows = db.query("SELECT COUNT(*) FROM votes", ()).await.unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 0, "invalid payloads must not record votes");
}

/// トークンの sub が vote.juror_hash と不一致の投票は 401 で拒否され、票を記録しない
#[tokio::test]
async fn test_vote_with_sub_mismatch_rejected() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;

    db.execute("INSERT INTO users (user_hash) VALUES ('owner')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmSubMismatch', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmSubMismatch', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    let app = build_router(shared_state);

    let (juror_hash, key) = insert_juror(&db).await;

    // juror_hash 名義で正しく署名し、トークンの sub のみを別の値にする。
    // 投票者のなりすまし（他人の名義での投票）は署名が正しくても拒否される。
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};

    let message = json!({"report_id": 1, "result": "Approve"}).to_string();
    let signing_key = SigningKey::<Sha256>::new(key);
    let signature = signing_key.sign(message.as_bytes()).to_vec();
    let vote_body = json!({
        "report_id": 1,
        "juror_hash": juror_hash,
        "result": "Approve",
        "signature": signature
    });

    let response = app
        .oneshot(with_peer(
            Request::post("/mod/vote")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, "attacker_sub").await
                    ),
                )
                .body(Body::from(vote_body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("Token sub mismatch"),
        "client must receive the sub mismatch rejection: {text}"
    );

    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 0, "a mismatched sub must not record a vote");
}

/// ECO-02: 模擬支援APIは無効化しており、認証済みでも 501 を返す
#[tokio::test]
async fn test_support_api_disabled_returns_501() {
    let (app, db) = setup_app_with_db().await;
    let body = json!({ "amount": 100 }).to_string();

    // 未認証は 401 のまま
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/content/QmSupportTest123/support")
                .header("content-type", "application/json")
                .body(Body::from(body.clone()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // 認証済みでも 501 を返し、模擬取引を記録しない
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/content/QmSupportTest123/support")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, "test_user").await
                    ),
                )
                .body(Body::from(body))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

    let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&response_body);
    assert!(
        text.contains("JPYC"),
        "response must point at the missing JPYC settlement: {text}"
    );

    let mut rows = db
        .query("SELECT COUNT(*) FROM transactions", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 0, "the disabled API must not record transactions");

    // 認証済みでも不正CIDは 400
    let response = app
        .oneshot(with_peer(
            Request::post("/content/bad-cid/support")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, "test_user").await
                    ),
                )
                .body(Body::from("{\"amount\":100}"))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// INT-01 / CONTENT-03: IPFS から取得したデータの SHA-256 が
/// DB の checksum と一致しない場合、コンテンツを返さず 401 で拒否する。
#[tokio::test]
async fn test_content_checksum_verification_fails_on_tamper() {
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: b"tampered content".to_vec(),
        delay: Duration::ZERO,
        expected_cid: Some("QmTampered".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(1), true).await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmTampered', 'Novel', 'test_user', 'correct_hash_123', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmTampered")
                .header(
                    "authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, "tester").await),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "checksum mismatch must reject the content"
    );
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("Integrity check failed"),
        "rejection must come from integrity verification: {body}"
    );
    assert!(
        !body.contains("tampered content"),
        "gateway body must not be returned: {body}"
    );
}

/// CONTENT-02: ストリーム途中で切断された応答は不完全なデータとして返さない。
#[tokio::test]
async fn test_get_content_rejects_truncated_stream() {
    let body = b"partial content".to_vec();
    let checksum = sha256::digest(&body);
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmTruncated".to_string()),
        inflated_content_length: Some(body.len() as u64 + 100),
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(1), true).await;
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmTruncated', 'Novel', 'test_user', ?, 'Approved');",
        [checksum.clone()],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmTruncated")
                .header(
                    "authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, "tester").await),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert!(
        response.status().is_server_error() || response.status().is_client_error(),
        "a truncated stream must not be returned as valid content: {}",
        response.status()
    );
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("partial content"),
        "partial body must not leak: {text}"
    );
}

/// CONTENT-03: checksum が一致する場合だけ、完全なデータを返す。
#[tokio::test]
async fn test_get_content_returns_verified_bytes() {
    let body = b"valid jxl codestream".to_vec();
    let checksum = sha256::digest(&body);
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmValid".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(1), true).await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmValid', 'Illustration', 'test_user', ?, 'Approved');",
        [checksum.clone()],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmValid")
                .header(
                    "authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, "tester").await),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jxl");
    assert_eq!(response.headers()["X-Content-Type-Options"], "nosniff");
    assert_eq!(response.headers()["X-Frame-Options"], "DENY");
    assert_eq!(response.headers()["x-forensic-checksum"], checksum.as_str());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], &body[..]);
}

/// CONTENT-02: 10 MiB を超える応答は 400 で拒否する。
#[tokio::test]
async fn test_get_content_rejects_oversize_body() {
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: vec![0u8; 10 * 1024 * 1024 + 1],
        delay: Duration::ZERO,
        expected_cid: Some("QmOversize".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(1), true).await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmOversize', 'Illustration', 'test_user', 'unused', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmOversize")
                .header(
                    "authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, "tester").await),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("IPFS content exceeds size limit"),
        "rejection must come from the size limit: {body}"
    );
}

/// CONTENT-02: ゲートウェイが応答しない場合は 500 を返し、
/// 部分データを完全なコンテンツとして返さない。
#[tokio::test]
async fn test_get_content_times_out_gateway() {
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: b"late".to_vec(),
        delay: Duration::from_secs(5),
        expected_cid: Some("QmTimeout".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) = setup_app_with_options(
        vec![gateway.template.clone()],
        Duration::from_millis(100),
        true,
    )
    .await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmTimeout', 'Illustration', 'test_user', 'unused', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmTimeout")
                .header(
                    "authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, "tester").await),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("Internal server error"),
        "internal details must stay hidden: {body}"
    );
    assert!(
        !body.contains("late"),
        "gateway body must not be returned: {body}"
    );
}

/// CONTENT-02: 先頭のゲートウェイが失敗しても、次のゲートウェイで成功すれば返す。
#[tokio::test]
async fn test_get_content_falls_back_to_next_gateway() {
    let failing = MockGateway::start(MockResponse {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        body: Vec::new(),
        delay: Duration::ZERO,
        expected_cid: Some("QmFallback".to_string()),
        inflated_content_length: None,
    })
    .await;
    let body = b"fallback content".to_vec();
    let checksum = sha256::digest(&body);
    let healthy = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmFallback".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) = setup_app_with_options(
        vec![failing.template.clone(), healthy.template.clone()],
        Duration::from_secs(1),
        true,
    )
    .await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmFallback', 'Illustration', 'test_user', ?, 'Approved');",
        [checksum],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmFallback")
                .header(
                    "authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, "tester").await),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], &body[..]);
}

/// CONTENT-02: 10 MiB ちょうどの応答は受理する（境界の off-by-one 検出）。
#[tokio::test]
async fn test_get_content_accepts_exact_size_limit() {
    let body = vec![0u8; 10 * 1024 * 1024];
    let checksum = sha256::digest(&body);
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmBoundary".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(5), true).await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmBoundary', 'Illustration', 'test_user', ?, 'Approved');",
        [checksum],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmBoundary")
                .header(
                    "authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, "tester").await),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(bytes.len(), body.len());
}

/// CONTENT-02: ゲートウェイが未設定の場合は 500 を返す。
#[tokio::test]
async fn test_get_content_without_gateways_returns_500() {
    let (app, db) = setup_app_with_options(Vec::new(), Duration::from_secs(1), true).await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmNoGateway', 'Illustration', 'test_user', 'unused', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmNoGateway")
                .header(
                    "authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, "tester").await),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// CONTENT-02: 全ゲートウェイが失敗した場合は 500 を返し、
/// ゲートウェイの応答本文を漏らさない。
#[tokio::test]
async fn test_get_content_all_gateways_fail() {
    let first = MockGateway::start(MockResponse {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        body: b"secret gateway body".to_vec(),
        delay: Duration::ZERO,
        expected_cid: Some("QmAllFail".to_string()),
        inflated_content_length: None,
    })
    .await;
    let second = MockGateway::start(MockResponse {
        status: StatusCode::BAD_GATEWAY,
        body: b"another secret".to_vec(),
        delay: Duration::ZERO,
        expected_cid: Some("QmAllFail".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) = setup_app_with_options(
        vec![first.template.clone(), second.template.clone()],
        Duration::from_secs(1),
        true,
    )
    .await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmAllFail', 'Illustration', 'test_user', 'unused', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmAllFail")
                .header(
                    "authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, "tester").await),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(body.contains("Internal server error"), "{body}");
    assert!(
        !body.contains("secret gateway body"),
        "gateway body leaked: {body}"
    );
    assert!(
        !body.contains("another secret"),
        "gateway body leaked: {body}"
    );
}

/// CONTENT-03: Novel は Markdown の Content-Type を返す。
#[tokio::test]
async fn test_get_content_novel_returns_markdown_mime() {
    let body = b"novel markdown".to_vec();
    let checksum = sha256::digest(&body);
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmNovel".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(1), true).await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmNovel', 'Novel', 'test_user', ?, 'Approved');",
        [checksum],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmNovel")
                .header(
                    "authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, "tester").await),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/markdown; charset=utf-8"
    );
}

/// AUTH-01: ALLOW_MOCK_AUTH が無効な場合、mock_token を受理しない。
#[tokio::test]
async fn test_mock_token_rejected_when_mock_auth_disabled() {
    let (app, _db) = setup_app_with_options(Vec::new(), Duration::from_secs(1), false).await;

    let response = app
        .oneshot(with_peer(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(verify_body("1990-01-01", "JP")))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// AUTH-01: ALLOW_MOCK_AUTH が無効な場合、ゼロ署名の投稿を拒否する。
/// トークン主体と owner_hash を一致させ、署名検証まで到達させる。
#[tokio::test]
async fn test_zero_signature_rejected_when_mock_auth_disabled() {
    let (app, db) = setup_app_with_options(Vec::new(), Duration::from_secs(1), false).await;
    let (owner_hash, _private_key) = insert_juror(&db).await;
    let token = generate_token(TEST_JWT_SECRET, &owner_hash).await;
    let boundary = "auth01-zero-signature";
    let body = multipart_upload_body_full(
        boundary,
        "upload.jxl",
        "image/jxl",
        &[0xFF, 0x0A, 0x01, 0x00],
        None,
        &owner_hash,
        &[0u8; 16],
    );

    let (status, response_body) = send_upload(&app, &token, boundary, body).await;

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "zero signature must be rejected: {response_body}"
    );
    assert!(
        response_body.contains("Invalid digital signature"),
        "rejection must come from signature verification: {response_body}"
    );
}

/// Audit log chain integrity test
#[tokio::test]
async fn test_audit_log_chain_integrity() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;

    // Insert required users first
    db.execute("INSERT INTO users (user_hash) VALUES ('user1')", ())
        .await
        .unwrap();
    db.execute("INSERT INTO users (user_hash) VALUES ('user2')", ())
        .await
        .unwrap();

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmAuditTest', 'Illustration', 'user1', 'hash1', 'Approved')",
        (),
    )
    .await
    .unwrap();

    let tx = db.transaction().await.unwrap();

    update_content_status(&tx, "QmAuditTest", "Reported", "user2", "First report")
        .await
        .unwrap();
    update_content_status(
        &tx,
        "QmAuditTest",
        "Rejected",
        "SYSTEM",
        "Consensus reached",
    )
    .await
    .unwrap();

    tx.commit().await.unwrap();

    let mut rows = db
        .query(
            "SELECT entry_hash, previous_log_hash FROM content_audit_log ORDER BY log_id",
            (),
        )
        .await
        .unwrap();

    let mut prev_hash = "GENESIS_HASH".to_string();
    while let Some(row) = rows.next().await.unwrap() {
        let stored_prev: String = row.get(1).unwrap();
        assert_eq!(stored_prev, prev_hash, "Audit chain broken");
        prev_hash = row.get(0).unwrap();
    }
}

/// JPYC: 注文作成、ウォレット所有証明、送金検証、確定。
#[tokio::test]
async fn test_payment_order_lifecycle() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, _db) = setup_app_with_payments(receiving, verifier.clone()).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/payments/orders")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "chain_id": 137,
                        "wallet_address": wallet,
                        "cid": "QmPayTest",
                        "amount_jpy": 1000,
                    })
                    .to_string(),
                ))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let created: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(created["amount_wei"], "1000000000000000000000");
    assert_eq!(created["recipient_address"], receiving);
    let order_id = created["order_id"].as_str().unwrap().to_string();

    let message = crate::payments::wallet_bind_message(&order_id);
    let mut hasher = Keccak256::new();
    hasher.update(format!("\x19Ethereum Signed Message:\n{}", message.len()).as_bytes());
    hasher.update(message.as_bytes());
    let digest = hasher.finalize();
    let (signature, recovery_id) = wallet_key.sign_prehash_recoverable(&digest);
    let mut sig_bytes = signature.to_bytes().to_vec();
    sig_bytes.push(recovery_id.to_byte());

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post(format!("/payments/orders/{order_id}/bind"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "signature": format!("0x{}", hex::encode(sig_bytes)) }).to_string(),
                ))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xabc".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei: created["amount_wei"].as_str().unwrap().to_string(),
    });

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post(format!("/payments/orders/{order_id}/verify"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let order: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(order["status"], "FINALIZED");
    assert_eq!(order["tx_hash"], "0xabc");
    assert_eq!(order["confirmations"], 256);
}

/// JPYC: 未対応チェーンは注文作成を拒否する。
#[tokio::test]
async fn test_payment_order_rejects_unsupported_chain() {
    let verifier = Arc::new(MockVerifier::default());
    let (app, _db) =
        setup_app_with_payments("0x1111111111111111111111111111111111111111", verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let response = app
        .oneshot(with_peer(
            Request::post("/payments/orders")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "chain_id": 999,
                        "wallet_address": "0x1111111111111111111111111111111111111111",
                        "cid": "QmPayTest",
                        "amount_jpy": 1000,
                    })
                    .to_string(),
                ))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// JPYC: 注文のウォレットと異なる署名は拒否する。
#[tokio::test]
async fn test_payment_order_rejects_wrong_wallet_signature() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let other_key = SigningKey::from_slice(&[0x22u8; 32]).expect("valid k256 test scalar");
    let verifier = Arc::new(MockVerifier::default());
    let (app, _db) = setup_app_with_payments(receiving, verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/payments/orders")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "chain_id": 137,
                        "wallet_address": wallet,
                        "cid": "QmPayTest",
                        "amount_jpy": 1000,
                    })
                    .to_string(),
                ))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let created: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let order_id = created["order_id"].as_str().unwrap().to_string();

    let message = crate::payments::wallet_bind_message(&order_id);
    let mut hasher = Keccak256::new();
    hasher.update(format!("\x19Ethereum Signed Message:\n{}", message.len()).as_bytes());
    hasher.update(message.as_bytes());
    let digest = hasher.finalize();
    let (signature, recovery_id) = other_key.sign_prehash_recoverable(&digest);
    let mut sig_bytes = signature.to_bytes().to_vec();
    sig_bytes.push(recovery_id.to_byte());

    let response = app
        .oneshot(with_peer(
            Request::post(format!("/payments/orders/{order_id}/bind"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "signature": format!("0x{}", hex::encode(sig_bytes)) }).to_string(),
                ))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// テスト用に支払い注文を作成し、order_id と amount_wei を返す
async fn create_payment_order(
    app: &Router,
    token: &str,
    wallet: &str,
    chain_id: i64,
    amount_jpy: u64,
) -> (String, String) {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/payments/orders")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "chain_id": chain_id,
                        "wallet_address": wallet,
                        "cid": "QmPayTest",
                        "amount_jpy": amount_jpy,
                    })
                    .to_string(),
                ))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "order creation must succeed"
    );
    let created: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    (
        created["order_id"].as_str().unwrap().to_string(),
        created["amount_wei"].as_str().unwrap().to_string(),
    )
}

/// R1-25: 検証中に状態が変わった注文は CAS で拒否し、上書きしない。
#[tokio::test]
async fn test_payment_verify_cas_rejects_stale_status() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('test_user')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status, price)
             VALUES ('QmPayTest', 'Novel', 'test_user', 'abc', 'Approved', 1000)",
        (),
    )
    .await
    .unwrap();
    let verification = PaymentVerification {
        chain_id: 137,
        tx_hash: "0xrace".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei: "1000000000000000000000".to_string(),
    };
    let verifier = Arc::new(RacingVerifier {
        db: db.clone(),
        verification,
    });

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: false,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: Some(receiving.to_string()),
        payment_receiving_xprv: None,
        payment_verifier: Some(verifier),
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    let app = build_router(shared_state);
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key).await;

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let mut rows = db
        .query(
            "SELECT status FROM payment_orders WHERE order_id = ?",
            [order_id],
        )
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status,
        crate::payments::STATUS_MISMATCH,
        "stale write must not win"
    );
}

/// 2 段目 CAS: 1 段目の DETECTED 遷移後、2 段目の UPDATE 直前に status が
/// 書き換えられた場合は 409 を返し、2 段目の値で上書きしてはならない。
#[tokio::test]
async fn test_payment_verify_stage2_cas_rejects_race_after_detected() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('test_user')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status, price)
             VALUES ('QmPayTest', 'Novel', 'test_user', 'abc', 'Approved', 1000)",
        (),
    )
    .await
    .unwrap();
    let verification = PaymentVerification {
        chain_id: 137,
        tx_hash: "0xstage2race".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei: "1000000000000000000000".to_string(),
    };
    let verifier = Arc::new(Stage2RacingVerifier {
        db: db.clone(),
        verification,
    });

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: false,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: Some(receiving.to_string()),
        payment_receiving_xprv: None,
        payment_verifier: Some(verifier),
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    let app = build_router(shared_state);
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key).await;

    // 他ノードのコミット済み書き換えを模して、先に status を MISMATCH にする。
    // この値は verify_order のトランザクション外なので 409 後も残る。
    db.execute(
        "UPDATE payment_orders SET status = ? WHERE order_id = ?",
        (crate::payments::STATUS_MISMATCH, order_id.clone()),
    )
    .await
    .unwrap();
    let mut rows = db
        .query(
            "SELECT status FROM payment_orders WHERE order_id = ?",
            [order_id.clone()],
        )
        .await
        .unwrap();
    let before: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        before,
        crate::payments::STATUS_MISMATCH,
        "1 段目の CAS は読み取った status (MISMATCH) を期待するため、ここから始める"
    );

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "second-stage CAS must reject a 0-row update"
    );

    let mut rows = db
        .query(
            "SELECT status FROM payment_orders WHERE order_id = ?",
            [order_id],
        )
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    // 1 段目で DETECTED に遷移した直後にトリガが MISMATCH へ書き換えるため、
    // 2 段目の `WHERE status = 'DETECTED'` は 0 件になる。CAS 失敗時は
    // トランザクション全体がロールバックされ、他ノードが書き換えた値が残る。
    assert_eq!(
        status,
        crate::payments::STATUS_MISMATCH,
        "stage 2 must not overwrite the externally rewritten status (got {status})"
    );
}

/// R1-24: ウォレット所有証明の記録が無い注文は verify を拒否する。
#[tokio::test]
async fn test_payment_verify_requires_wallet_binding() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xunbound".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei: "1000000000000000000000".to_string(),
    });
    let (app, _db) = setup_app_with_payments(receiving, verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("wallet ownership is not bound"));

    // バインド後は受理される（金額は注文と一致させる）。
    bind_payment_order(&app, &token, &order_id, &wallet_key).await;
    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// テスト用にウォレット所有証明を記録する（verify の必須条件）。
async fn bind_payment_order(app: &Router, token: &str, order_id: &str, wallet_key: &SigningKey) {
    let message = crate::payments::wallet_bind_message(order_id);
    let mut hasher = Keccak256::new();
    hasher.update(format!("\x19Ethereum Signed Message:\n{}", message.len()).as_bytes());
    hasher.update(message.as_bytes());
    let digest = hasher.finalize();
    let (signature, recovery_id) = wallet_key.sign_prehash_recoverable(&digest);
    let mut sig_bytes = signature.to_bytes().to_vec();
    sig_bytes.push(recovery_id.to_byte());
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post(format!("/payments/orders/{order_id}/bind"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "signature": format!("0x{}", hex::encode(sig_bytes)) }).to_string(),
                ))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "binding must succeed");
}

/// 支払い注文の verify を実行しレスポンスを返す
async fn verify_payment_order(app: &Router, token: &str, order_id: &str) -> Response {
    app.clone()
        .oneshot(with_peer(
            Request::post(format!("/payments/orders/{order_id}/verify"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap()
}

/// JPYC: 送金内容が注文と一致しない場合は 400 で拒否し、注文を MISMATCH にする。
#[tokio::test]
async fn test_payment_verify_rejects_transfer_mismatch() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier.clone()).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, amount_wei) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key).await;

    // 金額だけが注文と異なる送金検証結果を設定する。
    let wrong_amount = (amount_wei.parse::<u128>().unwrap() + 1).to_string();
    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xabc".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei: wrong_amount,
    });

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("transfer does not match the order"),
        "client must receive the mismatch rejection: {text}"
    );

    let mut rows = db
        .query(
            "SELECT status FROM payment_orders WHERE order_id = ?",
            [order_id.clone()],
        )
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status,
        crate::payments::STATUS_MISMATCH,
        "mismatched order must be marked as MISMATCH"
    );
}

/// JPYC: 同一送金の再利用は拒否される（UNIQUE(chain_id, tx_hash, log_index)）。
#[tokio::test]
async fn test_payment_verify_rejects_reused_transaction() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, _db) = setup_app_with_payments(receiving, verifier.clone()).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_a, amount_wei) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_a, &wallet_key).await;
    let (order_b, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_b, &wallet_key).await;

    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xabc".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei: amount_wei.clone(),
    });

    let first = verify_payment_order(&app, &token, &order_a).await;
    assert_eq!(first.status(), StatusCode::OK, "first use must be accepted");

    let second = verify_payment_order(&app, &token, &order_b).await;
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("transaction is already used by another order"),
        "client must receive the reuse rejection: {text}"
    );
}

/// 決済: 期限切れ注文へのウォレット紐付けを拒否する。
#[tokio::test]
async fn test_bind_wallet_rejects_elapsed_expiry() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _amount) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key).await;

    db.execute(
        "UPDATE payment_orders SET expires_at = ? WHERE order_id = ?",
        ("2000-01-01T00:00:00+00:00".to_string(), order_id.clone()),
    )
    .await
    .unwrap();

    let message = crate::payments::wallet_bind_message(&order_id);
    let mut hasher = Keccak256::new();
    hasher.update(format!("\x19Ethereum Signed Message:\n{}", message.len()).as_bytes());
    hasher.update(message.as_bytes());
    let digest = hasher.finalize();
    let (signature, recovery_id) = wallet_key.sign_prehash_recoverable(&digest);
    let mut sig_bytes = signature.to_bytes().to_vec();
    sig_bytes.push(recovery_id.to_byte());

    let response = app
        .oneshot(with_peer(
            Request::post(format!("/payments/orders/{order_id}/bind"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "signature": format!("0x{}", hex::encode(sig_bytes)) }).to_string(),
                ))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// 決済: status が PENDING のまま expires_at を過ぎた注文を拒否し EXPIRED へ更新する。
#[tokio::test]
async fn test_payment_verify_rejects_elapsed_expiry() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier.clone()).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, amount_wei) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key).await;

    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xabc".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei,
    });

    // status は PENDING のまま、expires_at だけを過去にする。
    db.execute(
        "UPDATE payment_orders SET expires_at = ? WHERE order_id = ?",
        ("2000-01-01T00:00:00+00:00".to_string(), order_id.clone()),
    )
    .await
    .unwrap();

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("order has expired"),
        "rejection must come from elapsed expiry"
    );

    let mut rows = db
        .query(
            "SELECT status FROM payment_orders WHERE order_id = ?",
            [order_id.clone()],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let status: String = row.get(0).unwrap();
    assert_eq!(status, crate::payments::STATUS_EXPIRED);
}

/// 決済: status が EXPIRED の注文は拒否される。
#[tokio::test]
async fn test_payment_verify_rejects_expired_order() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier.clone()).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, amount_wei) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key).await;

    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xabc".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei,
    });

    db.execute(
        "UPDATE payment_orders SET status = ? WHERE order_id = ?",
        (crate::payments::STATUS_EXPIRED, order_id.clone()),
    )
    .await
    .unwrap();

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("order is no longer verifiable"),
        "client must receive the expired rejection: {text}"
    );
}

/// JPYC: 確認数が規定未満の送金は CONFIRMING で保持される。
#[tokio::test]
async fn test_payment_verify_keeps_confirming_until_threshold() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, _db) = setup_app_with_payments(receiving, verifier.clone()).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, amount_wei) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key).await;

    // Polygon (chain 137) の必要確認数は 256。1 不足の状態を再現する。
    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xabc".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 255,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei: amount_wei.clone(),
    });

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(response.status(), StatusCode::OK);
    let order: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        order["status"], "CONFIRMING",
        "order below the confirmation threshold must stay CONFIRMING"
    );
    assert_eq!(order["confirmations"], 255);

    // 2回目: 同じ送金で確認数が進展した場合は再 verify でき、FINALIZED へ進む。
    // 同一注文内の tx_hash 再利用は UNIQUE 制約で拒否されない（別注文との衝突のみ拒否）。
    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xabc".to_string(),
        log_index: 0,
        block_number: 105,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei: amount_wei.clone(),
    });
    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "re-verifying the same transaction as confirmations progress must succeed"
    );
    let order: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(order["status"], "FINALIZED");
    assert_eq!(order["confirmations"], 256);

    // 確定済み注文は再 verify しても状態が変わらない（再編成で確定を戻さない）。
    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xabc".to_string(),
        log_index: 0,
        block_number: 110,
        confirmations: 1,
        from: wallet,
        to: receiving.to_string(),
        amount_wei,
    });
    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(response.status(), StatusCode::OK);
    let order: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        order["status"], "FINALIZED",
        "finalized orders must stay final"
    );
    assert_eq!(
        order["confirmations"], 256,
        "confirmations must not regress"
    );
}

/// ADR-0002: OIDC start と callback で PPID をセッションJWTへ結合する。
/// OIDC: max_age 設定時は認証要求に max_age を付与し、auth_time を検証する。
#[tokio::test]
async fn test_oidc_max_auth_age() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    oidc.set_max_auth_age(Some(300));

    // 認証開始 URL に max_age が含まれる。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert!(
        started["authorization_url"]
            .as_str()
            .unwrap()
            .contains("max_age=300")
    );
    let auth_state = started["state"].as_str().unwrap().to_string();

    // 新鮮な auth_time は受理される。
    let now = chrono::Utc::now().timestamp() as usize;
    *mock.auth_time.lock().unwrap() = Some(now);
    *mock.nonce.lock().unwrap() = Some(oidc.pending_nonce(&auth_state).unwrap());
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 古すぎる auth_time は拒否される。
    let auth_state = {
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::get("/auth/oidc/start")
                    .body(Body::empty())
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        let started: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        started["state"].as_str().unwrap().to_string()
    };
    *mock.auth_time.lock().unwrap() = Some(now - 3600);
    *mock.nonce.lock().unwrap() = Some(oidc.pending_nonce(&auth_state).unwrap());
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // auth_time 欠落も拒否される（fail-closed）。
    let auth_state = {
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::get("/auth/oidc/start")
                    .body(Body::empty())
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        let started: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        started["state"].as_str().unwrap().to_string()
    };
    *mock.auth_time.lock().unwrap() = None;
    *mock.nonce.lock().unwrap() = Some(oidc.pending_nonce(&auth_state).unwrap());
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// OIDC: 要求 acr/amr を満たす ID トークンのみ受理する（fail-closed）。
#[tokio::test]
async fn test_oidc_required_acr_amr() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    *mock.state.assurance.lock().unwrap() = Some((
        "urn:example:loa:2".to_string(),
        vec!["pwd".into(), "otp".into()],
    ));
    oidc.set_required_assurance(
        vec!["urn:example:loa:2".to_string()],
        vec!["pwd".to_string(), "otp".to_string()],
    );

    // 満たす場合は既存フローどおり成功する。
    let body = oidc_login(&app, oidc.as_ref(), &mock).await;
    assert_eq!(body["authorized"], true);

    // 要求 amr を増やすと不足で拒否される。
    oidc.set_required_assurance(
        vec!["urn:example:loa:2".to_string()],
        vec!["pwd".to_string(), "otp".to_string(), "hwk".to_string()],
    );
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let auth_state = started["state"].as_str().unwrap().to_string();
    *mock.nonce.lock().unwrap() = Some(oidc.pending_nonce(&auth_state).unwrap());
    let response = app
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_oidc_start_and_callback() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert!(
        started["authorization_url"]
            .as_str()
            .unwrap()
            .contains("code_challenge_method=S256")
    );
    let auth_state = started["state"].as_str().unwrap().to_string();

    let nonce = oidc.pending_nonce(&auth_state).expect("pending nonce");
    *mock.nonce.lock().unwrap() = Some(nonce);

    let response = app
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("session cookie")
        .to_str()
        .unwrap()
        .to_string();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    assert!(cookie.contains("session="));

    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["authorized"], true);
    assert_eq!(body["user_hash"], "test-ppid");
    assert!(body["token"].as_str().unwrap().len() > 20);
}

/// ADR-0002: state は単回消費であり、再利用は拒否する。
#[tokio::test]
async fn test_oidc_callback_rejects_reused_state() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let auth_state = started["state"].as_str().unwrap().to_string();
    let nonce = oidc.pending_nonce(&auth_state).unwrap();
    *mock.nonce.lock().unwrap() = Some(nonce);

    let uri = format!("/auth/oidc/callback?code=test-code&state={auth_state}");
    let first = app
        .clone()
        .oneshot(with_peer(
            Request::get(uri.as_str()).body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .oneshot(with_peer(
            Request::get(uri.as_str()).body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
}

/// ADR-0002: IDトークンの nonce 不一致を拒否する。
#[tokio::test]
async fn test_oidc_rejects_nonce_mismatch() {
    let (app, _oidc, mock, _db) = setup_app_with_oidc().await;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let auth_state = started["state"].as_str().unwrap().to_string();
    *mock.nonce.lock().unwrap() = Some("wrong-nonce".to_string());

    let response = app
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// at_hash が access_token と一致しない ID トークンは 401 で拒否される
#[tokio::test]
async fn test_oidc_rejects_at_hash_mismatch() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    *mock.at_hash_override.lock().unwrap() = Some("wrong-at-hash".to_string());

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let auth_state = started["state"].as_str().unwrap().to_string();
    // nonce は正当値を埋め込み、at_hash 検証の拒否だけを観測できるようにする。
    let nonce = oidc.pending_nonce(&auth_state).unwrap();
    *mock.nonce.lock().unwrap() = Some(nonce);

    let response = app
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("at_hash mismatch"),
        "client must receive the at_hash rejection: {text}"
    );
}

/// JWKS 未登録の鍵で署名された ID トークンは 401 で拒否される
#[tokio::test]
async fn test_oidc_rejects_id_token_signed_by_unknown_key() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;

    // kid は正規鍵と同じ "test-kid" のまま、署名鍵のみ差し替える。
    let forged = p256::ecdsa::SigningKey::from_slice(&[0x02u8; 32]).unwrap();
    *mock.forged_key_der.lock().unwrap() = Some(forged.to_pkcs8_der().unwrap().as_bytes().to_vec());

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let auth_state = started["state"].as_str().unwrap().to_string();
    let nonce = oidc.pending_nonce(&auth_state).unwrap();
    *mock.nonce.lock().unwrap() = Some(nonce);

    let response = app
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("ID token validation failed"),
        "client must receive the signature rejection: {text}"
    );
}

/// kid を持たない ID トークンは 401 で拒否される
#[tokio::test]
async fn test_oidc_rejects_id_token_without_kid() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    *mock.omit_kid.lock().unwrap() = true;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let auth_state = started["state"].as_str().unwrap().to_string();
    let nonce = oidc.pending_nonce(&auth_state).unwrap();
    *mock.nonce.lock().unwrap() = Some(nonce);

    let response = app
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("ID token has no kid"),
        "client must receive the kid rejection: {text}"
    );
}

/// ADR-0002: ログアウトしたセッションJWTは以後のAPIで拒否される。
#[tokio::test]
async fn test_logout_revokes_session() {
    let (app, oidc, mock, db) = setup_app_with_oidc().await;
    let body = oidc_login(&app, oidc.as_ref(), &mock).await;
    let token = body["token"].as_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/logout")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 失効 sid が DB へ永続化される（再起動後も失効が維持される）。
    let mut rows = db
        .query("SELECT sid FROM revoked_sessions", ())
        .await
        .unwrap();
    assert!(rows.next().await.unwrap().is_some());

    let response = app
        .oneshot(with_peer(
            Request::get("/content")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// sid を持たないトークンのログアウトは 200 で受け付けるが、失効対象がなくセッションは有効のまま
#[tokio::test]
async fn test_logout_without_sid_keeps_session() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/logout")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(with_peer(
            Request::get("/content")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a token without sid has nothing to revoke"
    );
}

/// OIDC 未構成の back-channel logout は 501
#[tokio::test]
async fn test_backchannel_logout_requires_oidc_configuration() {
    let app = setup_app().await;

    let response = app
        .oneshot(with_peer(
            Request::post("/auth/backchannel-logout")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("logout_token=anything"))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("OIDC is not configured"),
        "client must receive the configuration rejection: {text}"
    );
}

/// 未知の state でのコールバックは 400
#[tokio::test]
async fn test_oidc_callback_rejects_unknown_state() {
    let (app, _oidc, _mock, _db) = setup_app_with_oidc().await;

    let response = app
        .oneshot(with_peer(
            Request::get("/auth/oidc/callback?code=test-code&state=unknown-state")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("invalid or expired state"),
        "client must receive the state rejection: {text}"
    );
}

/// OIDC 未構成の callback は 501
#[tokio::test]
async fn test_oidc_callback_requires_oidc_configuration() {
    let app = setup_app().await;

    let response = app
        .oneshot(with_peer(
            Request::get("/auth/oidc/callback?code=test-code&state=some-state")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("OIDC is not configured"),
        "client must receive the configuration rejection: {text}"
    );
}

/// OIDC 未構成の start は 501
#[tokio::test]
async fn test_oidc_start_requires_oidc_configuration() {
    let app = setup_app().await;

    let response = app
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("OIDC is not configured"),
        "client must receive the configuration rejection: {text}"
    );
}

/// 不正な logout token は 401 で拒否される
#[tokio::test]
async fn test_backchannel_logout_rejects_invalid_token() {
    let (app, _oidc, _mock, _db) = setup_app_with_oidc().await;

    let response = app
        .oneshot(with_peer(
            Request::post("/auth/backchannel-logout")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("logout_token=not-a-jwt"))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// ADR-0002: events 欠落 / sid と sub の両方欠落の logout token は拒否される。
#[tokio::test]
async fn test_backchannel_logout_rejects_malformed_claims() {
    let (app, _oidc, mock, _db) = setup_app_with_oidc().await;

    let cases = [
        ("missing_events", json!({"sid": "some-sid"})),
        (
            "missing_sid_and_sub",
            json!({"events": {"http://schemas.openid.net/event/backchannel-logout": {}}}),
        ),
    ];
    for (name, extra) in cases {
        let logout_token = mock.logout_token_with_claims(extra);
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::post("/auth/backchannel-logout")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("logout_token={logout_token}")))
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "case={name}");
    }
}

/// ADR-0002: back-channel logout で sid を失効させる。
#[tokio::test]
async fn test_backchannel_logout_revokes_session() {
    let (app, oidc, mock, db) = setup_app_with_oidc().await;
    let body = oidc_login(&app, oidc.as_ref(), &mock).await;
    let token = body["token"].as_str().unwrap().to_string();

    let claims = jsonwebtoken::decode::<common::Claims>(
        &token,
        &jsonwebtoken::DecodingKey::from_secret(TEST_JWT_SECRET.as_bytes()).unwrap(),
        &jsonwebtoken::Validation::default(),
    )
    .unwrap()
    .claims;
    let sid = claims.sid.expect("session id");
    let logout_token = mock.logout_token(&sid);

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/backchannel-logout")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("logout_token={logout_token}")))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // back-channel logout も DB へ永続化する。
    let mut rows = db
        .query("SELECT sid FROM revoked_sessions WHERE sid = ?", [sid])
        .await
        .unwrap();
    assert!(rows.next().await.unwrap().is_some());

    let response = app
        .oneshot(with_peer(
            Request::get("/content")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// back-channel logout: sub 指定時は当該ユーザーの既存セッションを失効させる。
#[tokio::test]
async fn test_backchannel_logout_revokes_sessions_for_sub() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    let body = oidc_login(&app, oidc.as_ref(), &mock).await;
    let token = body["token"].as_str().unwrap().to_string();

    let claims = jsonwebtoken::decode::<common::Claims>(
        &token,
        &jsonwebtoken::DecodingKey::from_secret(TEST_JWT_SECRET.as_bytes()).unwrap(),
        &jsonwebtoken::Validation::default(),
    )
    .unwrap()
    .claims;
    let logout_token = mock.logout_token_for_sub(&claims.sub);

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/backchannel-logout")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("logout_token={logout_token}")))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(with_peer(
            Request::get("/content")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// back-channel logout: sub 失効後も再認証すれば新しいセッションを利用できる。
#[tokio::test]
async fn test_backchannel_logout_for_sub_allows_reauthentication() {
    let (app, oidc, mock, db) = setup_app_with_oidc().await;
    let body = oidc_login(&app, oidc.as_ref(), &mock).await;
    let old_token = body["token"].as_str().unwrap().to_string();

    let claims = jsonwebtoken::decode::<common::Claims>(
        &old_token,
        &jsonwebtoken::DecodingKey::from_secret(TEST_JWT_SECRET.as_bytes()).unwrap(),
        &jsonwebtoken::Validation::default(),
    )
    .unwrap()
    .claims;
    let logout_token = mock.logout_token_for_sub(&claims.sub);

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/backchannel-logout")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("logout_token={logout_token}")))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 失効閾値は DB にも永続化される。
    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM revoked_subs WHERE sub = ?",
            [claims.sub.clone()],
        )
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1, "revocation must be persisted");

    let new_body = oidc_login(&app, oidc.as_ref(), &mock).await;
    let new_token = new_body["token"].as_str().unwrap().to_string();

    let response = app
        .oneshot(with_peer(
            Request::get("/content")
                .header("authorization", format!("Bearer {new_token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM revoked_subs WHERE sub = ?",
            [claims.sub.clone()],
        )
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        count, 0,
        "re-authentication must clear the persisted threshold"
    );
}

/// back-channel logout: sid と sub の両方が指定された場合も既存セッションを失効させる。
#[tokio::test]
async fn test_backchannel_logout_revokes_session_with_sid_and_sub() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    let body = oidc_login(&app, oidc.as_ref(), &mock).await;
    let token = body["token"].as_str().unwrap().to_string();

    let claims = jsonwebtoken::decode::<common::Claims>(
        &token,
        &jsonwebtoken::DecodingKey::from_secret(TEST_JWT_SECRET.as_bytes()).unwrap(),
        &jsonwebtoken::Validation::default(),
    )
    .unwrap()
    .claims;
    let sid = claims
        .sid
        .clone()
        .expect("OIDC-issued token should have sid");
    let logout_token = mock.logout_token_for_sid_and_sub(&sid, &claims.sub);

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/backchannel-logout")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("logout_token={logout_token}")))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(with_peer(
            Request::get("/content")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// JPYC: EVM アドレスの大小文字は区別せず、混在ケースも小文字化して照合する。
#[test]
fn test_eth_address_normalization_is_case_insensitive() {
    let lower = "0x52908400098527886e0f7030069857d2e4169ee7";
    let mixed = "0x52908400098527886E0F7030069857D2E4169EE7";
    assert!(crate::payments::is_valid_eth_address(lower));
    assert!(crate::payments::is_valid_eth_address(mixed));
    assert_eq!(
        crate::payments::normalize_address(mixed),
        crate::payments::normalize_address(lower)
    );
    // 0x プレフィックス無し／長さ違いは拒否
    assert!(!crate::payments::is_valid_eth_address(
        "52908400098527886e0f7030069857d2e4169ee7"
    ));
    assert!(!crate::payments::is_valid_eth_address("0x1234"));
}

/// JPYC: derivation_index の u32 境界を超える値は受取アドレス再利用を防ぐため拒否。
#[test]
fn test_derivation_index_bounds() {
    assert_eq!(crate::payments::derivation_index_to_u32(0).unwrap(), 0u32);
    assert_eq!(
        crate::payments::derivation_index_to_u32(u32::MAX as i64).unwrap(),
        u32::MAX
    );
    assert!(crate::payments::derivation_index_to_u32(-1).is_err());
    assert!(crate::payments::derivation_index_to_u32(u32::MAX as i64 + 1).is_err());
}

/// JPYC: BIP44派生が決定的で、indexごとに異なるアドレスを返す。
#[test]
fn test_derive_receiving_address() {
    let xprv = "xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi";
    let first = crate::payments::derive_receiving_address(xprv, 0).unwrap();
    let again = crate::payments::derive_receiving_address(xprv, 0).unwrap();
    let second = crate::payments::derive_receiving_address(xprv, 1).unwrap();
    assert_eq!(first, again);
    assert_ne!(first, second);
    assert!(crate::payments::is_valid_eth_address(&first));
    assert!(crate::payments::is_valid_eth_address(&second));
}

/// JPYC: 注文ごとに異なる受取アドレスを派生する。
#[tokio::test]
async fn test_payment_orders_derive_unique_addresses() {
    let xprv = "xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi";
    let (app, _db) = setup_app_with_hd_payments(xprv).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let mut addresses = Vec::new();

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::post("/payments/orders")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "chain_id": 137,
                            "wallet_address": "0x1111111111111111111111111111111111111111",
                            "cid": "QmPayTest",
                        "amount_jpy": 1000,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let created: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        addresses.push(created["recipient_address"].as_str().unwrap().to_string());
    }

    assert_ne!(addresses[0], addresses[1]);
    assert!(crate::payments::is_valid_eth_address(&addresses[0]));
    assert!(crate::payments::is_valid_eth_address(&addresses[1]));
}

// ---------------------------------------------------------------------------
// RS256（RSA 秘密鍵演算）のテスト
// ---------------------------------------------------------------------------

/// テスト用の RSA 秘密鍵（PKCS#8 PEM）を TestRng で生成する。
fn test_rsa_private_pem() -> String {
    use rsa::pkcs8::EncodePrivateKey;
    let mut rng = TestRng::new();
    let private_key = PrivateKey::new(&mut rng, 2048).expect("generate test RSA key");
    private_key
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .expect("encode test RSA key")
        .to_string()
}

/// 秘密鍵 PEM から対応する公開鍵 PEM を導出する（テスト用）。
fn test_rsa_public_pem(private_pem: &str) -> String {
    use rsa::pkcs8::{DecodePrivateKey, EncodePublicKey};
    let key = PrivateKey::from_pkcs8_pem(private_pem).expect("valid private key");
    key.to_public_key()
        .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
        .expect("public key PEM")
        .to_string()
}

/// RS256: 署名ヘッダに kid が入り、未知 kid のトークンは拒否される。
#[test]
fn test_jwt_rs256_kid_verification() {
    let pem = test_rsa_private_pem();
    let keys = crate::jwt::JwtKeys::from_rsa_private_key_pem(&pem).expect("valid RSA key");
    let kid = keys
        .header()
        .kid
        .clone()
        .expect("RS256 header must carry kid");
    assert_eq!(kid.len(), 32, "kid is a 128-bit hex digest");

    let claims = Claims {
        sub: "kid_user".to_string(),
        exp: 10000000000,
        sid: None,
        iat: None,
    };
    let token = encode(&keys.header(), &claims, keys.encoding_key()).expect("sign");
    let decoded: Claims = keys
        .decode(&token)
        .expect("verify with matching kid")
        .claims;
    assert_eq!(decoded.sub, "kid_user");

    let forged = crate::jwt::JwtKeys::from_rsa_private_key_pem(&test_rsa_private_pem())
        .expect("another RSA key");
    let mut header = forged.header();
    header.kid = Some("unknown-kid".to_string());
    let unknown = encode(&header, &claims, forged.encoding_key()).expect("sign with unknown kid");
    let result: Result<jsonwebtoken::TokenData<Claims>, String> = keys.decode(&unknown);
    assert!(
        result.is_err(),
        "unknown kid must be rejected (fail-closed)"
    );
}

/// RS256: ローテーション前に発行された kid 無しトークンも検証できる。
#[test]
fn test_jwt_rs256_accepts_legacy_token_without_kid() {
    let pem = test_rsa_private_pem();
    let keys = crate::jwt::JwtKeys::from_rsa_private_key_pem(&pem).expect("valid RSA key");
    let claims = Claims {
        sub: "legacy".to_string(),
        exp: 10000000000,
        sid: None,
        iat: None,
    };
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = None;
    let token = encode(&header, &claims, keys.encoding_key()).expect("sign without kid");
    let decoded: Claims = keys
        .decode(&token)
        .expect("legacy token must verify")
        .claims;
    assert_eq!(decoded.sub, "legacy");
}

/// RS256: JWT_REQUIRE_KID 有効時は kid 無しトークンを拒否する（fail-closed）。
#[test]
fn test_jwt_require_kid_rejects_legacy_token() {
    let pem = test_rsa_private_pem();
    let mut keys = crate::jwt::JwtKeys::from_rsa_private_key_pem(&pem).expect("valid RSA key");
    keys.set_require_kid(true);
    let claims = Claims {
        sub: "user".to_string(),
        exp: 10000000000,
        sid: None,
        iat: None,
    };

    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = None;
    let legacy = encode(&header, &claims, keys.encoding_key()).expect("sign without kid");
    assert!(
        keys.decode::<Claims>(&legacy).is_err(),
        "kid-less tokens must be rejected when require_kid is enabled"
    );

    let with_kid = encode(&keys.header(), &claims, keys.encoding_key()).expect("sign with kid");
    let decoded: Claims = keys
        .decode(&with_kid)
        .expect("kid-bearing token must verify")
        .claims;
    assert_eq!(decoded.sub, "user");
}

#[test]
fn test_jwt_require_kid_rejects_hmac_configuration() {
    assert!(
        crate::jwt_keys_from_env(None, Some(TEST_JWT_SECRET.to_string()), None, true).is_err(),
        "HS256 issues no kid; requiring kid must be rejected at startup"
    );
}

/// RS256: 追加した旧公開鍵で署名されたトークンを検証できる。
#[test]
fn test_jwt_rs256_additional_verification_key() {
    let new_pem = test_rsa_private_pem();
    let old_pem = test_rsa_private_pem();
    let old_public = test_rsa_public_pem(&old_pem);
    let mut keys =
        crate::jwt::JwtKeys::from_rsa_private_key_pem(&new_pem).expect("new signing key");
    keys.add_verification_key("rotated-old", &old_public)
        .expect("register old public key");

    let old_keys = crate::jwt::JwtKeys::from_rsa_private_key_pem(&old_pem).expect("old key");
    let claims = Claims {
        sub: "during_rotation".to_string(),
        exp: 10000000000,
        sid: None,
        iat: None,
    };
    let mut header = old_keys.header();
    header.kid = Some("rotated-old".to_string());
    let token = encode(&header, &claims, old_keys.encoding_key()).expect("sign with old key");
    let decoded: Claims = keys.decode(&token).expect("old key must verify").claims;
    assert_eq!(decoded.sub, "during_rotation");

    // 署名鍵と同じ kid は登録できない。
    let duplicate = keys.header().kid.clone().unwrap();
    assert!(keys.add_verification_key(&duplicate, &old_public).is_err());
}

/// RS256: 秘密鍵から鍵一式を構築できる。
#[test]
fn test_jwt_keys_from_rsa_private_key() {
    let pem = test_rsa_private_pem();
    let keys = crate::jwt::JwtKeys::from_rsa_private_key_pem(&pem).expect("valid RSA key");
    assert_eq!(keys.algorithm_name(), "RS256");
    assert_eq!(keys.header().alg, jsonwebtoken::Algorithm::RS256);
}

/// RS256: 署名したトークンを同じ鍵で検証できる。
#[test]
fn test_jwt_rs256_roundtrip() {
    let pem = test_rsa_private_pem();
    let keys = crate::jwt::JwtKeys::from_rsa_private_key_pem(&pem).expect("valid RSA key");

    let claims = Claims {
        sub: "rs256_user".to_string(),
        exp: 10000000000,
        sid: Some("rs256_sid".to_string()),
        iat: None,
    };
    let token = encode(&keys.header(), &claims, keys.encoding_key()).expect("sign with RS256");
    let data = jsonwebtoken::decode::<Claims>(&token, keys.decoding_key(), &keys.validation())
        .expect("verify with RS256");
    assert_eq!(data.claims.sub, "rs256_user");
    assert_eq!(data.claims.sid.as_deref(), Some("rs256_sid"));
}

/// RS256: ヘッダの `alg` を HS256 に差し替えたトークンを拒否する。
/// 許可アルゴリズムは起動時に確定するため、混同攻撃が成立しない。
#[test]
fn test_jwt_rejects_algorithm_confusion() {
    let pem = test_rsa_private_pem();
    let keys = crate::jwt::JwtKeys::from_rsa_private_key_pem(&pem).expect("valid RSA key");

    let claims = Claims {
        sub: "attacker".to_string(),
        exp: 10000000000,
        sid: None,
        iat: None,
    };
    // RS256 鍵のまま HS256 を宣言したトークンを作る。
    let mut header = Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = None;
    let forged = encode(&header, &claims, keys.encoding_key());
    match forged {
        // 鍵種別が合わないため署名自体が失敗する。
        Err(_) => {}
        Ok(token) => {
            let result =
                jsonwebtoken::decode::<Claims>(&token, keys.decoding_key(), &keys.validation());
            assert!(result.is_err(), "alg confusion must be rejected");
        }
    }
}

/// RS256: 1024 bit の鍵は鍵長要件を満たさず拒否する。
#[test]
fn test_jwt_rejects_short_rsa_key() {
    use rsa::pkcs8::EncodePrivateKey;
    let mut rng = TestRng::new();
    let weak = PrivateKey::new(&mut rng, 1024).expect("generate weak RSA key");
    let pem = weak
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .expect("encode weak key");
    assert!(crate::jwt::JwtKeys::from_rsa_private_key_pem(&pem).is_err());
}

/// 環境変数の選択: RSA 秘密鍵があれば RS256、無ければ HS256 を使う。
#[test]
fn test_jwt_keys_from_env_prefers_rsa() {
    let pem = test_rsa_private_pem();
    let keys = crate::jwt_keys_from_env(Some(pem), Some(TEST_JWT_SECRET.to_string()), None, false)
        .expect("RSA key wins");
    assert_eq!(keys.algorithm_name(), "RS256");

    let hmac = crate::jwt_keys_from_env(None, Some(TEST_JWT_SECRET.to_string()), None, false)
        .expect("HMAC fallback");
    assert_eq!(hmac.algorithm_name(), "HS256");

    assert!(crate::jwt_keys_from_env(None, None, None, false).is_err());

    // 不正な RSA PEM は、有効な JWT_SECRET があっても黙ってフォールバックしない。
    assert!(
        crate::jwt_keys_from_env(
            Some("not a pem".to_string()),
            Some(TEST_JWT_SECRET.to_string()),
            None,
            false,
        )
        .is_err(),
        "invalid RSA key must not be silently ignored when JWT_SECRET is present"
    );
}

/// RS256 の鍵で署名したセッショントークンが、認証済み API を通る。
#[tokio::test]
async fn test_authenticated_request_with_rs256_token() {
    let pem = test_rsa_private_pem();
    let keys =
        Arc::new(crate::jwt::JwtKeys::from_rsa_private_key_pem(&pem).expect("valid RSA key"));
    let claims = Claims {
        sub: "rs256_user".to_string(),
        exp: 10000000000,
        sid: None,
        iat: None,
    };
    let token = encode(&keys.header(), &claims, keys.encoding_key()).expect("sign with RS256");
    let (app, _db) = setup_app_with_jwt_keys(keys).await;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/content")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    // 未認証なら 401。RS256 トークンが受理された場合は 200。
    assert_eq!(response.status(), StatusCode::OK);
}

/// DEC-02: 年齢判定の有効設定値は認証不要で常時公開される。
#[tokio::test]
async fn test_age_policy_endpoint_is_public() {
    let app = setup_app().await;
    let response = app
        .oneshot(with_peer(
            Request::get("/policy/age").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["settings_always_public"], true);
    assert_eq!(json["default_minimum_age"], 21);
    assert_eq!(
        json["enforcement_mode"], "self_reported_dob_country",
        "default enforcement mode must be self-reported"
    );
    assert!(json["notes"].is_array());
}

#[tokio::test]
async fn test_age_policy_endpoint_exposes_only_non_secret_public_settings() {
    let app = setup_app().await;

    let response = app
        .oneshot(with_peer(
            Request::get("/policy/age").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected response body: {}",
        String::from_utf8_lossy(&body)
    );
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let obj = json
        .as_object()
        .expect("age policy response must be object");

    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "country_minimums",
            "default_minimum_age",
            "enforcement_mode",
            "notes",
            "operator_configured",
            "require_explicit_configuration",
            "settings_always_public",
            "target_regions",
        ],
        "public age policy endpoint must expose only non-secret policy settings"
    );

    // Defense-in-depth: ensure known secret/config-sensitive identifiers never appear.
    let body_text = String::from_utf8(body.to_vec()).unwrap().to_lowercase();
    for forbidden in [
        "jwt_secret",
        "jwt_rsa_private_key_pem",
        "oidc_client_secret",
        "payment_receiving_xprv",
        "pinata_jwt",
        "database_url",
        "db_token",
        "private_key",
        "client_secret",
        "xprv",
    ] {
        assert!(
            !body_text.contains(forbidden),
            "age policy response must not contain secret-like token: {forbidden}"
        );
    }
}

#[test]
fn test_age_policy_fail_closed_modes() {
    let today = chrono::Utc::now().date_naive();
    // 30年前相当の成人日付を使う（現在日基準で十分成人）
    let adult_birth = (today - chrono::Duration::days(365 * 30))
        .format("%Y-%m-%d")
        .to_string();

    let require_config = common::age::AgePolicy {
        require_explicit_configuration: true,
        operator_configured: false,
        ..common::age::AgePolicy::default()
    };
    let err = validate_age(&require_config, &adult_birth, "JP").unwrap_err();
    assert!(matches!(err, AppError::Forbidden(_)));

    let trusted_only = common::age::AgePolicy {
        enforcement_mode: common::age::AgeEnforcementMode::TrustedAttributeRequired,
        ..common::age::AgePolicy::default()
    };
    let err = validate_age(&trusted_only, &adult_birth, "JP").unwrap_err();
    assert!(matches!(err, AppError::Forbidden(_)));
}

#[tokio::test]
async fn test_content_audit_rejects_invalid_cid() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let response = app
        .oneshot(with_peer(
            Request::get("/content/bad-cid/audit")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// 不正な CID でのコンテンツ取得は 400
#[tokio::test]
async fn test_get_content_rejects_invalid_cid() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let response = app
        .oneshot(with_peer(
            Request::get("/content/bad-cid")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// 存在しない有効な CID はゲートウェイへ問い合わせる前に 404
#[tokio::test]
async fn test_get_content_missing_cid_returns_404() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmNoSuchContent000000000000000000000000000000")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// GOV-02 / REQ: 通報中・却下コンテンツは CID を知っていても直接取得できない。
#[tokio::test]
async fn test_get_content_hides_reported_and_rejected() {
    for status in ["Reported", "Rejected"] {
        let (app, db) = setup_app_with_db().await;
        let cid = format!("QmHidden{status}");
        db.execute(
            "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES (?, 'Illustration', 'test_user', 'unused', ?)",
            (cid.clone(), status),
        )
        .await
        .unwrap();
        let token = generate_token(TEST_JWT_SECRET, "test_user").await;
        let response = app
            .oneshot(with_peer(
                Request::get(format!("/content/{cid}"))
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "status={status} must not be directly retrievable"
        );
    }
}

#[tokio::test]
async fn test_payment_order_rejects_invalid_order_id_format() {
    let verifier = Arc::new(MockVerifier::default());
    let (app, _db) =
        setup_app_with_payments("0x1111111111111111111111111111111111111111", verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let response = app
        .oneshot(with_peer(
            Request::get("/payments/orders/not-a-uuid")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---- ADR-0004: 期限ベース確定・定足数・異議・投票資格 ----

/// モデレーション用の最小アプリ（content は Reviewed 中、report 1 が存在）
async fn setup_mod_app() -> (Router, libsql::Connection) {
    setup_mod_app_with_operators(&[]).await
}

/// 運用者（セルフホストの所有者）を設定したモデレーション用アプリ
async fn setup_mod_app_with_operators(operators: &[&str]) -> (Router, libsql::Connection) {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('owner')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmModTest', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmModTest', 'test')",
        (),
    )
    .await
    .unwrap();

    let state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(
            operators
                .iter()
                .map(|value| value.to_string())
                .collect::<std::collections::HashSet<_>>(),
        ),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    (build_router(state), db)
}

/// 認証済みで finalize エンドポイントを呼ぶ
async fn send_finalize(app: &Router, report_id: i64) -> StatusCode {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post(format!("/mod/reports/{report_id}/finalize"))
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, "operator").await
                    ),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    response.status()
}

/// 認証済みで通報を送る
async fn send_report(app: &Router, cid: &str, reason: &str, sub: &str) -> StatusCode {
    let body = json!({
        "report_id": 0,
        "target_cid": cid,
        "reason": reason,
        "status": "Pending"
    });
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/mod/report")
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, sub).await),
                )
                .body(Body::from(body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    response.status()
}

#[tokio::test]
async fn test_finalize_is_noop_before_deadline() {
    let (app, db) = setup_mod_app().await;
    assert_eq!(send_finalize(&app, 1).await, StatusCode::OK);

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmModTest'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status, "Reported",
        "finalize must not conclude before the deadline"
    );

    expire_report(&db, 1).await;
    assert_eq!(send_finalize(&app, 1).await, StatusCode::OK);

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmModTest'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status, "Approved",
        "zero votes must dismiss the report and restore content"
    );
}

#[tokio::test]
async fn test_quorum_not_met_restores_approved_with_audit_reason() {
    let (app, db) = setup_mod_app().await;
    for _ in 0..2 {
        let (juror, key) = insert_juror(&db).await;
        assert_eq!(
            send_signed_vote(&app, 1, "Reject", &juror, &key).await,
            StatusCode::OK
        );
    }
    force_conclude(&db, 1).await;

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmModTest'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status, "Approved",
        "quorum-not-met reports must restore content"
    );

    let mut rows = db
        .query(
            "SELECT reason FROM content_audit_log WHERE cid = 'QmModTest' ORDER BY log_id DESC LIMIT 1",
            (),
        )
        .await
        .unwrap();
    let reason: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert!(
        reason.contains("quorum not met"),
        "audit must record the dismissal reason: {reason}"
    );

    let mut rows = db
        .query("SELECT concluded_at FROM reports WHERE report_id = 1", ())
        .await
        .unwrap();
    let concluded: Option<String> = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert!(concluded.is_some(), "concluded_at must be recorded");
}

#[tokio::test]
async fn test_vote_after_deadline_rejected() {
    let (app, db) = setup_mod_app().await;
    let (juror, key) = insert_juror(&db).await;
    assert_eq!(
        send_signed_vote(&app, 1, "Reject", &juror, &key).await,
        StatusCode::OK
    );

    expire_report(&db, 1).await;
    let (late_juror, late_key) = insert_juror(&db).await;
    assert_eq!(
        send_signed_vote(&app, 1, "Reject", &late_juror, &late_key).await,
        StatusCode::BAD_REQUEST,
        "votes after the deadline must be rejected"
    );

    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1, "the late vote must not be recorded");
}

#[tokio::test]
async fn test_content_owner_cannot_vote() {
    let (app, db) = setup_mod_app().await;
    let mut rng = TestRng::new();
    let owner_key = PrivateKey::new(&mut rng, 2048).unwrap();
    let public_der = owner_key
        .to_public_key()
        .to_public_key_der()
        .unwrap()
        .to_vec();
    db.execute(
        "UPDATE users SET public_key_der = ? WHERE user_hash = 'owner'",
        [public_der],
    )
    .await
    .unwrap();

    assert_eq!(
        send_signed_vote(&app, 1, "Reject", "owner", &owner_key).await,
        StatusCode::BAD_REQUEST,
        "the content submitter must not be allowed to vote"
    );
    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 0, "the owner's vote must not be recorded");
}

#[tokio::test]
async fn test_banned_juror_cannot_vote() {
    let (app, db) = setup_mod_app().await;
    let (juror, key) = insert_juror(&db).await;
    db.execute(
        "UPDATE users SET is_banned = TRUE WHERE user_hash = ?",
        [juror.clone()],
    )
    .await
    .unwrap();

    assert_eq!(
        send_signed_vote(&app, 1, "Reject", &juror, &key).await,
        StatusCode::FORBIDDEN,
        "banned users must not be allowed to vote"
    );
    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 0, "the banned juror's vote must not be recorded");
}

#[tokio::test]
async fn test_appeal_within_30_days_creates_new_report() {
    let (app, db) = setup_mod_app().await;
    for _ in 0..3 {
        let (juror, key) = insert_juror(&db).await;
        assert_eq!(
            send_signed_vote(&app, 1, "Reject", &juror, &key).await,
            StatusCode::OK
        );
    }
    force_conclude(&db, 1).await;

    assert_eq!(
        send_report(&app, "QmModTest", "Appeal: new evidence", "appellant").await,
        StatusCode::OK,
        "a re-report within 30 days must be accepted as an appeal"
    );

    let mut rows = db
        .query(
            "SELECT appeal_of, concluded_at FROM reports ORDER BY report_id",
            (),
        )
        .await
        .unwrap();
    let first = rows.next().await.unwrap().unwrap();
    let first_concluded: Option<String> = first.get(1).unwrap();
    assert!(
        first_concluded.is_some(),
        "original conclusion must be preserved"
    );
    let second = rows.next().await.unwrap().unwrap();
    let appeal_of: Option<i64> = second.get(0).unwrap();
    assert_eq!(
        appeal_of,
        Some(1),
        "the new report must link to the original"
    );

    let mut rows = db
        .query(
            "SELECT reason FROM content_audit_log WHERE cid = 'QmModTest' ORDER BY log_id DESC LIMIT 1",
            (),
        )
        .await
        .unwrap();
    let reason: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert!(
        reason.contains("appeal"),
        "audit must record the appeal: {reason}"
    );
}

#[tokio::test]
async fn test_report_after_appeal_window_is_not_an_appeal() {
    let (app, db) = setup_mod_app().await;
    db.execute(
        "UPDATE reports SET concluded_at = datetime('now', '-31 days') WHERE report_id = 1",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "UPDATE contents SET status = 'Rejected' WHERE cid = 'QmModTest'",
        (),
    )
    .await
    .unwrap();

    assert_eq!(
        send_report(&app, "QmModTest", "Fresh report", "reporter").await,
        StatusCode::OK
    );
    let mut rows = db
        .query(
            "SELECT appeal_of FROM reports ORDER BY report_id DESC LIMIT 1",
            (),
        )
        .await
        .unwrap();
    let appeal_of: Option<i64> = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(appeal_of, None, "re-reports after 30 days are not appeals");
}

// ---- ADR-0004: デプロイごとの運用者と監査 ----

#[test]
fn test_moderation_operators_parser() {
    let parsed = crate::moderation_operators(Some(" alice , bob ,, alice , "));
    assert_eq!(parsed.len(), 2);
    assert!(parsed.contains("alice"));
    assert!(parsed.contains("bob"));
    assert!(crate::moderation_operators(None).is_empty());
    assert!(crate::moderation_operators(Some(" , , ")).is_empty());
}

/// 認証済みで運用操作を呼ぶ
async fn send_mod_action(
    app: &Router,
    path: &str,
    sub: &str,
    body: serde_json::Value,
) -> StatusCode {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post(path)
                .header("content-type", "application/json")
                .header(
                    "Authorization",
                    format!("Bearer {}", generate_token(TEST_JWT_SECRET, sub).await),
                )
                .body(Body::from(body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    response.status()
}

#[tokio::test]
async fn test_ban_requires_configured_operator() {
    let (app, db) = setup_mod_app().await;
    db.execute("INSERT INTO users (user_hash) VALUES ('target')", ())
        .await
        .unwrap();

    let status = send_mod_action(
        &app,
        "/mod/operators/target/ban",
        "operator",
        json!({"reason": "illegal content", "duration_days": 30}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "unconfigured operators must be denied"
    );

    let mut rows = db
        .query("SELECT COUNT(*) FROM governance_audit_log", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        count, 0,
        "denied operations must not be recorded as actions"
    );
}

#[tokio::test]
async fn test_configured_operator_can_ban_with_audit() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('target')", ())
        .await
        .unwrap();

    let status = send_mod_action(
        &app,
        "/mod/operators/target/ban",
        "operator",
        json!({"reason": "  repeated violations  ", "duration_days": 30}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mut rows = db
        .query(
            "SELECT is_banned, banned_at FROM users WHERE user_hash = 'target'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let banned: bool = row.get(0).unwrap();
    let banned_at: Option<String> = row.get(1).unwrap();
    assert!(banned);
    assert!(banned_at.is_some());

    let mut rows = db
        .query(
            "SELECT action, target, actor_hash, reason, details FROM governance_audit_log",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let action: String = row.get(0).unwrap();
    let target: String = row.get(1).unwrap();
    let actor: String = row.get(2).unwrap();
    let reason: String = row.get(3).unwrap();
    let details: Option<String> = row.get(4).unwrap();
    assert_eq!(action, "BAN");
    assert_eq!(target, "target");
    assert_eq!(actor, "operator");
    assert_eq!(reason, "repeated violations", "reason must be trimmed");
    assert!(details.unwrap().contains("30"));
}

#[tokio::test]
async fn test_ban_rejects_self_and_invalid_input() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('target')", ())
        .await
        .unwrap();

    assert_eq!(
        send_mod_action(
            &app,
            "/mod/operators/operator/ban",
            "operator",
            json!({"reason": "self", "duration_days": null}),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "self-ban must be rejected"
    );
    assert_eq!(
        send_mod_action(
            &app,
            "/mod/operators/target/ban",
            "operator",
            json!({"reason": "bad duration", "duration_days": 0}),
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send_mod_action(
            &app,
            "/mod/operators/target/ban",
            "operator",
            json!({"reason": "   ", "duration_days": 1}),
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send_mod_action(
            &app,
            "/mod/operators/missing/ban",
            "operator",
            json!({"reason": "missing", "duration_days": null}),
        )
        .await,
        StatusCode::NOT_FOUND
    );

    let mut rows = db
        .query("SELECT COUNT(*) FROM governance_audit_log", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 0, "rejected operations must not be recorded");
}

#[tokio::test]
async fn test_banned_operator_cannot_ban() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    db.execute(
        "INSERT INTO users (user_hash, is_banned) VALUES ('operator', TRUE), ('target', FALSE)",
        (),
    )
    .await
    .unwrap();

    assert_eq!(
        send_mod_action(
            &app,
            "/mod/operators/target/ban",
            "operator",
            json!({"reason": "illegal", "duration_days": null}),
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn test_unban_restores_user_with_audit() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    db.execute(
        "INSERT INTO users (user_hash, is_banned, banned_at) VALUES ('target', TRUE, datetime('now'))",
        (),
    )
    .await
    .unwrap();

    assert_eq!(
        send_mod_action(
            &app,
            "/mod/operators/target/unban",
            "operator",
            json!({"reason": "appeal accepted"}),
        )
        .await,
        StatusCode::OK
    );

    let mut rows = db
        .query(
            "SELECT is_banned, banned_at FROM users WHERE user_hash = 'target'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let banned: bool = row.get(0).unwrap();
    let banned_at: Option<String> = row.get(1).unwrap();
    assert!(!banned);
    assert!(banned_at.is_none());

    let mut rows = db
        .query("SELECT action, reason FROM governance_audit_log", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let action: String = row.get(0).unwrap();
    let reason: String = row.get(1).unwrap();
    assert_eq!(action, "UNBAN");
    assert_eq!(reason, "appeal accepted");
}

#[tokio::test]
async fn test_unpin_records_decision_with_audit() {
    let (app_no_op, _) = setup_mod_app().await;
    assert_eq!(
        send_mod_action(
            &app_no_op,
            "/mod/content/QmModTest/unpin",
            "operator",
            json!({"reason": "concluded illegal content"}),
        )
        .await,
        StatusCode::FORBIDDEN
    );

    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    assert_eq!(
        send_mod_action(
            &app,
            "/mod/content/QmModTest/unpin",
            "operator",
            json!({"reason": "concluded illegal content"}),
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        send_mod_action(
            &app,
            "/mod/content/not-a-cid/unpin",
            "operator",
            json!({"reason": "bad cid"}),
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send_mod_action(
            &app,
            "/mod/content/QmUnknown/unpin",
            "operator",
            json!({"reason": "missing"}),
        )
        .await,
        StatusCode::NOT_FOUND
    );

    let mut rows = db
        .query(
            "SELECT action, target, actor_hash, reason FROM governance_audit_log",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let action: String = row.get(0).unwrap();
    let target: String = row.get(1).unwrap();
    let actor: String = row.get(2).unwrap();
    let reason: String = row.get(3).unwrap();
    assert_eq!(action, "UNPIN");
    assert_eq!(target, "QmModTest");
    assert_eq!(actor, "operator");
    assert_eq!(reason, "concluded illegal content");
}

// ---- ADR-0004: 期限到達の自動検知（ワーカー共通処理） ----

#[test]
fn test_moderation_finalize_interval_defaults_and_bounds() {
    use std::time::Duration;
    assert_eq!(
        crate::moderation_finalize_interval(None),
        Duration::from_secs(300)
    );
    assert_eq!(
        crate::moderation_finalize_interval(Some("60")),
        Duration::from_secs(60)
    );
    assert_eq!(
        crate::moderation_finalize_interval(Some("29")),
        Duration::from_secs(300),
        "too short must fall back"
    );
    assert_eq!(
        crate::moderation_finalize_interval(Some("3601")),
        Duration::from_secs(300),
        "too long must fall back"
    );
    assert_eq!(
        crate::moderation_finalize_interval(Some("abc")),
        Duration::from_secs(300)
    );
}

#[test]
fn test_graceful_drain_timeout_defaults_and_bounds() {
    use std::time::Duration;
    assert_eq!(crate::graceful_drain_timeout(None), Duration::from_secs(30));
    assert_eq!(
        crate::graceful_drain_timeout(Some("5")),
        Duration::from_secs(5)
    );
    assert_eq!(
        crate::graceful_drain_timeout(Some("0")),
        Duration::from_secs(30),
        "zero must fall back"
    );
    assert_eq!(
        crate::graceful_drain_timeout(Some("301")),
        Duration::from_secs(30),
        "over the maximum must fall back"
    );
    assert_eq!(
        crate::graceful_drain_timeout(Some("bad")),
        Duration::from_secs(30)
    );
}

#[tokio::test]
async fn test_finalize_due_reports_skips_before_deadline() {
    let (_app, db) = setup_mod_app().await;
    let count = crate::finalize_due_reports(&db).await.unwrap();
    assert_eq!(count, 0, "a fresh report must not be auto-finalized");

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmModTest'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(status, "Reported");
}

#[tokio::test]
async fn test_finalize_due_reports_concludes_expired_report() {
    let (_app, db) = setup_mod_app().await;
    expire_report(&db, 1).await;

    let count = crate::finalize_due_reports(&db).await.unwrap();
    assert_eq!(count, 1, "the expired report must be auto-finalized");

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmModTest'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status, "Approved",
        "zero votes means quorum-not-met dismissal"
    );

    let mut rows = db
        .query("SELECT concluded_at FROM reports WHERE report_id = 1", ())
        .await
        .unwrap();
    let concluded: Option<String> = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert!(concluded.is_some(), "conclusion time must be recorded");

    let again = crate::finalize_due_reports(&db).await.unwrap();
    assert_eq!(again, 0, "finalization must be idempotent");
}

#[tokio::test]
async fn test_finalize_due_reports_handles_multiple_reports() {
    let (_app, db) = setup_mod_app().await;
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmSecond', 'Novel', 'owner', 'def', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason, deadline_at)
             VALUES (2, 'QmSecond', 'second', datetime('now', '-1 day'))",
        (),
    )
    .await
    .unwrap();
    expire_report(&db, 1).await;

    let count = crate::finalize_due_reports(&db).await.unwrap();
    assert_eq!(count, 2, "all expired reports must be finalized");
}

// ---- 同一コネクション上のトランザクション競合 ----

#[tokio::test]
async fn concurrent_transactions_on_shared_connection_are_serialized() {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('owner')", ())
        .await
        .unwrap();

    let gate = Arc::new(tokio::sync::Barrier::new(2));
    let tasks: Vec<_> = (0..2)
        .map(|index| {
            let db = Arc::clone(&db);
            let gate = Arc::clone(&gate);
            tokio::spawn(async move {
                gate.wait().await;
                let lock = crate::write_lock(&db);
                let _guard = lock.lock().await;
                let tx = db.transaction().await.unwrap();
                tx.execute(
                    "INSERT INTO users (user_hash) VALUES (?)",
                    [format!("concurrent_{index}")],
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
            })
        })
        .collect();

    for task in tasks {
        task.await.unwrap();
    }

    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM users WHERE user_hash LIKE 'concurrent_%'",
            (),
        )
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 2, "both transactions must succeed without DB errors");
}

#[tokio::test]
async fn concurrent_votes_do_not_conflict_with_finalization() {
    use std::sync::Arc as StdArc;

    let (app, db) = setup_mod_app().await;
    // 投票は期限内に通しつつ、finalize 用には期限切れの別通報を作る。
    // これにより同一 Connection 上で投票トランザクションと確定トランザクションが競合する。
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmConcurrent', 'Novel', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason, deadline_at)
             VALUES (2, 'QmConcurrent', 'due', datetime('now', '-1 day'))",
        (),
    )
    .await
    .unwrap();

    let app = StdArc::new(app);
    let mut jurors = Vec::new();
    for _ in 0..4 {
        jurors.push(insert_juror(&db).await);
    }

    let barrier = StdArc::new(tokio::sync::Barrier::new(5));
    let mut tasks = Vec::new();
    for (juror, key) in jurors {
        let app = StdArc::clone(&app);
        let barrier = StdArc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            send_signed_vote(app.as_ref(), 1, "Reject", &juror, &key).await
        }));
    }
    let db_for_finalize = StdArc::new(db);
    let finalize_db = StdArc::clone(&db_for_finalize);
    let finalize_barrier = StdArc::clone(&barrier);
    tasks.push(tokio::spawn(async move {
        finalize_barrier.wait().await;
        let _ = crate::finalize_due_reports(finalize_db.as_ref())
            .await
            .unwrap();
        StatusCode::OK
    }));

    let mut statuses = Vec::new();
    for task in tasks {
        statuses.push(task.await.unwrap());
    }
    assert!(
        statuses.iter().all(|status| *status == StatusCode::OK),
        "all concurrent operations must succeed: {statuses:?}"
    );
}

#[tokio::test]
async fn concurrent_begin_without_lock_is_rejected() {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let tasks: Vec<_> = (0..2)
        .map(|_| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                db.transaction().await
            })
        })
        .collect();
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    let ok = results.iter().filter(|r| r.is_ok()).count();
    let failed = results.iter().filter(|r| r.is_err()).count();
    assert_eq!(ok, 1, "exactly one BEGIN should succeed");
    assert_eq!(failed, 1, "a second concurrent BEGIN should fail");
}

/// 単一ライター検出: 生存 PID のロックがある場合は取得を拒否する。
#[test]
fn test_instance_lock_rejects_live_pid() {
    let dir = std::env::temp_dir().join(format!(
        "openillustration_instance_lock_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("instance.db.instance.lock");

    // 自分自身の PID を書き込むと生存と判定され、2回目の取得は拒否される。
    std::fs::write(&lock, format!("{}\n", std::process::id())).unwrap();
    assert!(crate::acquire_instance_lock(lock.to_str().unwrap()).is_err());

    // ロックファイルを削除すれば再取得できる。
    std::fs::remove_file(&lock).unwrap();
    assert!(crate::acquire_instance_lock(lock.to_str().unwrap()).is_ok());
    let contents = std::fs::read_to_string(&lock).unwrap();
    assert_eq!(contents.trim().parse::<u32>().unwrap(), std::process::id());

    let _ = std::fs::remove_dir_all(&dir);
}

/// 死んだ PID のロックは stale として回収できる。
#[test]
fn test_instance_lock_recovers_stale_lock() {
    let dir = std::env::temp_dir().join(format!(
        "openillustration_stale_lock_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("instance.db.instance.lock");
    std::fs::write(&lock, "99999999\n").unwrap();

    assert!(crate::acquire_instance_lock(lock.to_str().unwrap()).is_ok());
    let contents = std::fs::read_to_string(&lock).unwrap();
    assert_eq!(contents.trim().parse::<u32>().unwrap(), std::process::id());

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- refresh token cleanup ----

#[tokio::test]
async fn test_purge_expired_refresh_tokens() {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;

    // 有効なトークンと期限切れトークンを挿入する。
    db.execute(
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
             VALUES ('valid', 'u', 'f', '2099-01-01T00:00:00+00:00')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
             VALUES ('expired', 'u', 'f', '2000-01-01T00:00:00+00:00')",
        (),
    )
    .await
    .unwrap();

    let purged = crate::purge_expired_refresh_tokens(&db).await.unwrap();
    assert_eq!(purged, 1, "only the expired token must be purged");

    let mut rows = db
        .query("SELECT token_hash FROM refresh_tokens", ())
        .await
        .unwrap();
    let mut remaining = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        remaining.push(row.get::<String>(0).unwrap());
    }
    assert_eq!(remaining, vec!["valid"]);
}

#[tokio::test]
async fn test_purge_stale_revoked_subs() {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;

    let now = chrono::Utc::now().timestamp();
    db.execute(
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES ('old', ?)",
        [now - 24 * 60 * 60 - 3600],
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES ('recent', ?)",
        [now],
    )
    .await
    .unwrap();

    let purged = crate::purge_stale_revoked_subs(&db).await.unwrap();
    assert_eq!(purged, 1, "only the stale revocation must be purged");

    let mut rows = db.query("SELECT sub FROM revoked_subs", ()).await.unwrap();
    let mut remaining = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        remaining.push(row.get::<String>(0).unwrap());
    }
    assert_eq!(remaining, vec!["recent"]);
}

#[tokio::test]
async fn test_purge_stale_revoked_sessions() {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;

    db.execute(
        "INSERT INTO revoked_sessions (sid, revoked_at) VALUES ('old', '2000-01-01 00:00:00')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO revoked_sessions (sid, revoked_at) VALUES ('recent', datetime('now'))",
        (),
    )
    .await
    .unwrap();

    let purged = crate::purge_stale_revoked_sessions(&db).await.unwrap();
    assert_eq!(purged, 1, "only the stale revocation must be purged");

    let mut rows = db
        .query("SELECT sid FROM revoked_sessions", ())
        .await
        .unwrap();
    let mut remaining = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        remaining.push(row.get::<String>(0).unwrap());
    }
    assert_eq!(remaining, vec!["recent"]);
}

// ---- liveness / readiness ----

#[tokio::test]
async fn test_readiness_ok_and_shutting_down() {
    let (app, _db) = setup_app_with_db().await;

    let live = app
        .clone()
        .oneshot(with_peer(
            Request::get("/health").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(live.status(), StatusCode::OK);

    let ready = app
        .clone()
        .oneshot(with_peer(
            Request::get("/ready").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_readiness_returns_503_while_shutting_down() {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    let shutting_down = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let state = Arc::new(AppState {
        db: db.clone(),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down,
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    let app = build_router(state);

    let response = app
        .oneshot(with_peer(
            Request::get("/ready").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn test_readiness_requires_ipv6_peer() {
    let (app, _db) = setup_app_with_db().await;
    let response = app
        .oneshot(with_peer(
            Request::get("/ready").body(Body::empty()).unwrap(),
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "readiness must not bypass the IP gate"
    );
}

// ---- リフレッシュトークンのローテーションと盗用検知 ----

/// /auth/refresh へリクエストし、(status, _new_refresh_cookie, body) を返す。
async fn post_refresh(app: &Router, token: &str) -> (StatusCode, String, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/refresh")
                .header(header::COOKIE, format!("refresh_token={token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let status = response.status();
    let new_refresh = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|part| part.trim().strip_prefix("refresh_token="))
        .unwrap_or_default()
        .to_string();
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    (status, new_refresh, body)
}

/// R1-18: refresh Cookie の Path は /auth 配下全体を対象にする。
/// /auth/refresh 固定だと /auth/logout へ Cookie が送られず、
/// ログアウト時のリフレッシュファミリ失効が到達しない。
#[tokio::test]
async fn test_refresh_cookie_path_covers_logout() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    let (_login, refresh) = oidc_login_with_refresh(&app, &oidc, &mock).await;
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/refresh")
                .header(header::COOKIE, format!("refresh_token={refresh}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.contains("refresh_token="))
        .expect("refresh_token cookie must be set");
    assert!(
        cookie.contains("Path=/auth;"),
        "cookie must cover /auth/logout as well: {cookie}"
    );
    assert!(
        !cookie.contains("Path=/auth/refresh"),
        "cookie must not be scoped to /auth/refresh only: {cookie}"
    );
}

#[tokio::test]
async fn test_refresh_rotates_token_and_issues_new_session() {
    let (app, oidc, mock, db) = setup_app_with_oidc().await;
    let (login, refresh) = oidc_login_with_refresh(&app, &oidc, &mock).await;
    let sub = login["user_hash"].as_str().unwrap().to_string();

    let (status, new_refresh, body) = post_refresh(&app, &refresh).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["authorized"], true);
    assert!(body["token"].as_str().is_some());
    assert!(
        !new_refresh.is_empty(),
        "a new refresh token must be issued"
    );
    assert_ne!(new_refresh, refresh, "the refresh token must be rotated");

    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM refresh_tokens WHERE sub = ? AND used_at IS NOT NULL",
            [sub],
        )
        .await
        .unwrap();
    let used: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(used, 1, "the presented refresh token must be consumed");
}

#[tokio::test]
async fn test_refresh_reuse_revokes_family() {
    let (app, oidc, mock, db) = setup_app_with_oidc().await;
    let (_login, refresh) = oidc_login_with_refresh(&app, &oidc, &mock).await;

    let (status, new_refresh, _) = post_refresh(&app, &refresh).await;
    assert_eq!(status, StatusCode::OK);

    // 古いトークンの再提示は盗用検知となり、新しいトークンも失効する。
    let (status, _, _) = post_refresh(&app, &refresh).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = post_refresh(&app, &new_refresh).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the whole family must be revoked"
    );

    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM refresh_tokens WHERE used_at IS NULL",
            (),
        )
        .await
        .unwrap();
    let active: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        active, 0,
        "no refresh token in the family may remain active"
    );
}

#[tokio::test]
async fn test_refresh_rejects_unknown_token() {
    let (app, _oidc, _mock, _db) = setup_app_with_oidc().await;
    let (status, _, _) = post_refresh(&app, "not-a-real-token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_logout_revokes_refresh_family() {
    let (app, oidc, mock, db) = setup_app_with_oidc().await;
    let (login, refresh) = oidc_login_with_refresh(&app, &oidc, &mock).await;
    let token = login["token"].as_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/logout")
                .header("authorization", format!("Bearer {token}"))
                .header(header::COOKIE, format!("refresh_token={refresh}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let (status, _, _) = post_refresh(&app, &refresh).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "logout must revoke the family"
    );
    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM refresh_tokens WHERE used_at IS NULL",
            (),
        )
        .await
        .unwrap();
    let active: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(active, 0);
}

#[tokio::test]
async fn test_refresh_rejects_missing_cookie() {
    let (app, _oidc, _mock, _db) = setup_app_with_oidc().await;
    let response = app
        .oneshot(with_peer(
            Request::post("/auth/refresh").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ---- 注文とコンテンツ価格の照合 ----

async fn post_order_json(app: &Router, token: &str, body: serde_json::Value) -> StatusCode {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/payments/orders")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    response.status()
}

#[tokio::test]
async fn test_order_requires_matching_content_price() {
    let verifier = Arc::new(MockVerifier::default());
    let (app, _db) =
        setup_app_with_payments("0x1111111111111111111111111111111111111111", verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    for amount in [999u64, 1001, 1] {
        let status = post_order_json(
            &app,
            &token,
            json!({
                "chain_id": 137,
                "wallet_address": "0x1111111111111111111111111111111111111111",
                "cid": "QmPayTest",
                "amount_jpy": amount,
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "amount {amount} must be rejected"
        );
    }
}

#[tokio::test]
async fn test_order_rejects_unknown_or_non_approved_content() {
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) =
        setup_app_with_payments("0x1111111111111111111111111111111111111111", verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status, price)
             VALUES ('QmReported', 'Novel', 'test_user', 'abc', 'Reported', 1000)",
        (),
    )
    .await
    .unwrap();

    let base = json!({
        "chain_id": 137,
        "wallet_address": "0x1111111111111111111111111111111111111111",
        "amount_jpy": 1000,
    });
    let mut unknown = base.clone();
    unknown["cid"] = json!("QmUnknownContent");
    assert_eq!(
        post_order_json(&app, &token, unknown).await,
        StatusCode::NOT_FOUND
    );

    let mut reported = base;
    reported["cid"] = json!("QmReported");
    assert_eq!(
        post_order_json(&app, &token, reported).await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn test_order_rejects_invalid_cid_before_price_lookup() {
    let verifier = Arc::new(MockVerifier::default());
    let (app, _db) =
        setup_app_with_payments("0x1111111111111111111111111111111111111111", verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let status = post_order_json(
        &app,
        &token,
        json!({
            "chain_id": 137,
            "wallet_address": "0x1111111111111111111111111111111111111111",
            "cid": "not/a/cid",
            "amount_jpy": 1000,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_order_records_content_cid() {
    let verifier = Arc::new(MockVerifier::default());
    let (app, _db) =
        setup_app_with_payments("0x1111111111111111111111111111111111111111", verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/payments/orders")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "chain_id": 137,
                        "wallet_address": "0x1111111111111111111111111111111111111111",
                        "cid": "QmPayTest",
                        "amount_jpy": 1000,
                    })
                    .to_string(),
                ))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let order_id = body["order_id"].as_str().unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get(format!("/payments/orders/{order_id}"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let order: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(order["cid"], "QmPayTest");
}

// ---- メトリクス ----

fn read_metric(body: &str, name: &str) -> u64 {
    body.lines()
        .find_map(|line| line.strip_prefix(name).map(str::trim))
        .unwrap_or_else(|| panic!("metric {name} not found"))
        .parse()
        .unwrap()
}

#[tokio::test]
async fn test_metrics_expose_prometheus_counters() {
    let app = setup_app().await;

    let before = app
        .clone()
        .oneshot(with_peer(
            Request::get("/metrics").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(before.status(), StatusCode::OK);
    let content_type = before.headers()[header::CONTENT_TYPE].to_str().unwrap();
    assert!(content_type.starts_with("text/plain"));
    let before_body =
        String::from_utf8(to_bytes(before.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    let requests_before = read_metric(&before_body, "openillustration_http_requests_total");
    let errors_before = read_metric(&before_body, "openillustration_http_errors_total");

    // 404 を発生させ、エラーカウンタが進むことを確認する。
    let missing = app
        .clone()
        .oneshot(with_peer(
            Request::get("/no-such-path").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let after = app
        .oneshot(with_peer(
            Request::get("/metrics").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let after_body =
        String::from_utf8(to_bytes(after.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    let requests_after = read_metric(&after_body, "openillustration_http_requests_total");
    let errors_after = read_metric(&after_body, "openillustration_http_errors_total");
    // 注入されたカウンタは他テストと共有されないため、正確な差分を検証できる。
    assert_eq!(
        requests_after,
        requests_before + 2,
        "requests must be counted exactly"
    );
    assert_eq!(
        errors_after,
        errors_before + 1,
        "each 4xx response must increment the error counter exactly once"
    );
}

// ---- R5-01: コンテンツメタデータと検索・絞り込み ----

#[test]
fn test_content_query_parser_defaults_and_bounds() {
    let empty = ContentQuery {
        q: None,
        tag: None,
        content_type: None,
        limit: None,
        offset: None,
        favorite_only: None,
    };
    assert_eq!(
        parse_content_query(&empty),
        ContentFilter {
            q: None,
            tag: None,
            content_type: None,
            limit: DEFAULT_CONTENT_LIMIT,
            offset: 0,
            favorite_only: false,
        }
    );

    let make = |limit: &str, offset: &str| ContentQuery {
        q: None,
        tag: None,
        content_type: None,
        limit: Some(limit.to_string()),
        offset: Some(offset.to_string()),
        favorite_only: None,
    };
    for limit in ["1", "100"] {
        assert_eq!(
            parse_content_query(&make(limit, "7")).limit,
            limit.parse::<i64>().unwrap()
        );
    }
    // 範囲外は 400 にせず既定 50 へフォールバックする（安全側）。
    for limit in ["0", "101", "-1", "abc", ""] {
        assert_eq!(
            parse_content_query(&make(limit, "0")).limit,
            DEFAULT_CONTENT_LIMIT,
            "limit={limit:?}"
        );
    }
    assert_eq!(MAX_CONTENT_LIMIT, 100);
    for offset in ["-1", "abc", ""] {
        assert_eq!(
            parse_content_query(&make("50", offset)).offset,
            0,
            "offset={offset:?}"
        );
    }
    assert_eq!(parse_content_query(&make("50", "9")).offset, 9);

    let filled = ContentQuery {
        q: Some("  cat  ".to_string()),
        tag: Some(" fantasy ".to_string()),
        content_type: Some("Novel".to_string()),
        limit: Some("2".to_string()),
        offset: Some("1".to_string()),
        favorite_only: None,
    };
    let parsed = parse_content_query(&filled);
    assert_eq!(parsed.q.as_deref(), Some("cat"));
    assert_eq!(parsed.tag.as_deref(), Some("fantasy"));
    assert_eq!(parsed.content_type.as_deref(), Some("Novel"));
}

#[test]
fn test_content_metadata_validation_bounds() {
    let tags = vec!["tag".to_string()];
    assert!(validate_content_metadata("", "", &[]).is_ok());
    assert!(validate_content_metadata(&"a".repeat(MAX_CONTENT_TITLE_BYTES), "", &tags).is_ok());
    assert!(
        validate_content_metadata(&"a".repeat(MAX_CONTENT_TITLE_BYTES + 1), "", &tags).is_err()
    );
    assert!(
        validate_content_metadata("", &"a".repeat(MAX_CONTENT_DESCRIPTION_BYTES), &tags).is_ok()
    );
    assert!(
        validate_content_metadata("", &"a".repeat(MAX_CONTENT_DESCRIPTION_BYTES + 1), &tags)
            .is_err()
    );

    let max_tags: Vec<String> = (0..MAX_CONTENT_TAGS).map(|i| format!("t{i}")).collect();
    assert!(validate_content_metadata("", "", &max_tags).is_ok());
    let too_many: Vec<String> = (0..MAX_CONTENT_TAGS + 1).map(|i| format!("t{i}")).collect();
    assert!(validate_content_metadata("", "", &too_many).is_err());

    let max_tag = vec!["a".repeat(MAX_CONTENT_TAG_BYTES)];
    assert!(validate_content_metadata("", "", &max_tag).is_ok());
    let long_tag = vec!["a".repeat(MAX_CONTENT_TAG_BYTES + 1)];
    assert!(validate_content_metadata("", "", &long_tag).is_err());
    // 複数バイト文字は文字数ではなくバイト数で検査される。
    let multibyte = "あ".repeat(MAX_CONTENT_TAG_BYTES / 3 + 1);
    assert!(validate_content_metadata("", "", &[multibyte]).is_err());

    assert_eq!(
        parse_tags_field(" cat , fantasy ,, "),
        vec!["cat".to_string(), "fantasy".to_string()]
    );
    assert!(parse_tags_field("").is_empty());
}

#[test]
fn test_like_escaping_keeps_wildcards_literal() {
    assert_eq!(escape_like("plain"), "plain");
    assert_eq!(escape_like("100%_cat\\x"), "100\\%\\_cat\\\\x");
}

#[tokio::test]
async fn test_migration_011_existing_rows_read_as_empty_metadata() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    // 011 適用前（001 直後）の contents を再現し、既存行を挿入する。
    db.execute(
        "CREATE TABLE contents (
            cid TEXT PRIMARY KEY,
            type TEXT CHECK(type IN ('Illustration', 'Novel')),
            owner_hash TEXT,
            checksum TEXT NOT NULL,
            status TEXT CHECK(status IN ('Approved', 'Reported', 'Rejected')) DEFAULT 'Approved',
            price INTEGER DEFAULT 0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, checksum) VALUES ('QmLegacy', 'Novel', 'legacy_sum')",
        (),
    )
    .await
    .unwrap();

    crate::migration::run_migrations(&db).await.unwrap();

    let mut rows = db
        .query(
            "SELECT title, description, tags FROM contents WHERE cid = 'QmLegacy'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let title: String = row.get(0).unwrap();
    let description: String = row.get(1).unwrap();
    let tags: String = row.get(2).unwrap();
    assert_eq!(title, "");
    assert_eq!(description, "");
    assert_eq!(tags, "");
    assert!(parse_tags_field(&tags).is_empty());
}

async fn insert_content_meta(
    db: &libsql::Connection,
    cid: &str,
    kind: &str,
    title: &str,
    description: &str,
    tags: &str,
    created_at: &str,
) {
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status, price, title, description, tags, created_at)
         VALUES (?, ?, 'test_user', 'sum', 'Approved', 0, ?, ?, ?, ?)",
        (cid, kind, title, description, tags, created_at),
    )
    .await
    .unwrap();
}

async fn get_content_list(
    app: &Router,
    token: &str,
    query: &str,
) -> (StatusCode, Vec<serde_json::Value>) {
    let uri = format!("/content{query}");
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get(uri.as_str())
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let items: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    (status, items)
}

fn listed_cids(items: &[serde_json::Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| item["cid"].as_str().unwrap().to_string())
        .collect()
}

/// ASCII 安全文字だけを残し、それ以外を percent-encode する（テスト用）。
fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// q / tag / type の絞り込みと、追加メタデータが JSON で返ることを検証する。
#[tokio::test]
async fn test_list_contents_filters_by_query_tag_and_type() {
    let (app, db) = setup_app_with_db().await;
    db.execute("DELETE FROM contents", ()).await.unwrap();
    insert_content_meta(
        &db,
        "QmCatArt",
        "Illustration",
        "Cat illustration",
        "A cute cat",
        "fantasy,art",
        "2026-01-04 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmDogNovel",
        "Novel",
        "Dog novel",
        "A brave story",
        "dog,novel",
        "2026-01-03 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmCatTag",
        "Illustration",
        "Landscape",
        "Nice colors",
        "cat,fantasy",
        "2026-01-02 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmCatlikeTag",
        "Novel",
        "Poetry",
        "Verses",
        "story,catlike",
        "2026-01-01 00:00:00",
    )
    .await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    // q は title / description の部分一致（tags は検索しない）。
    let (status, items) = get_content_list(&app, &token, "?q=cat").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed_cids(&items), vec!["QmCatArt"]);

    // tag はカンマ区切りの各タグの前方一致（先頭・カンマ後ともに該当）。
    let (_, items) = get_content_list(&app, &token, "?tag=cat").await;
    assert_eq!(listed_cids(&items), vec!["QmCatTag", "QmCatlikeTag"]);

    // type は完全一致。未知の値は 400 ではなく 0 件。
    let (_, items) = get_content_list(&app, &token, "?type=Novel").await;
    assert_eq!(listed_cids(&items), vec!["QmDogNovel", "QmCatlikeTag"]);
    let (status, items) = get_content_list(&app, &token, "?type=Unknown").await;
    assert_eq!(status, StatusCode::OK);
    assert!(items.is_empty());

    // 複数条件は AND。既定の並び順（新着順）も維持される。
    let (_, items) = get_content_list(&app, &token, "?type=Novel&tag=cat").await;
    assert_eq!(listed_cids(&items), vec!["QmCatlikeTag"]);

    // 追加メタデータが既存5項目と一緒に JSON で返る。
    let (_, items) = get_content_list(&app, &token, "?q=Landscape").await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["cid"], "QmCatTag");
    assert_eq!(items[0]["title"], "Landscape");
    assert_eq!(items[0]["description"], "Nice colors");
    assert_eq!(items[0]["tags"], serde_json::json!(["cat", "fantasy"]));
    assert_eq!(items[0]["type"], "Illustration");
    assert_eq!(items[0]["status"], "Approved");
    assert_eq!(items[0]["price"], 0);
}

#[tokio::test]
async fn test_list_contents_limit_boundaries_and_offset() {
    let (app, db) = setup_app_with_db().await;
    db.execute("DELETE FROM contents", ()).await.unwrap();
    for day in 1..=5 {
        insert_content_meta(
            &db,
            &format!("Qm{day:03}"),
            "Illustration",
            &format!("title {day}"),
            "",
            "",
            &format!("2026-01-{day:02} 00:00:00"),
        )
        .await;
    }
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    // 全5件しかないため、範囲外が既定 50 へフォールバックすれば 5 件返る。
    for (query, expected_len) in [
        ("", 5),
        ("?limit=1", 1),
        ("?limit=100", 5),
        ("?limit=0", 5),
        ("?limit=101", 5),
    ] {
        let (status, items) = get_content_list(&app, &token, query).await;
        assert_eq!(status, StatusCode::OK, "query={query}");
        assert_eq!(items.len(), expected_len, "query={query}");
    }

    // 新着順 5,4,3,2,1 の 3 件目以降から 2 件。
    let (_, items) = get_content_list(&app, &token, "?limit=2&offset=2").await;
    assert_eq!(listed_cids(&items), vec!["Qm003", "Qm002"]);
    let (_, items) = get_content_list(&app, &token, "?offset=10").await;
    assert!(items.is_empty());
}

#[tokio::test]
async fn test_like_wildcards_are_literal() {
    let (app, db) = setup_app_with_db().await;
    db.execute("DELETE FROM contents", ()).await.unwrap();
    insert_content_meta(
        &db,
        "QmPlain",
        "Illustration",
        "plain title",
        "plain description",
        "plain",
        "2026-01-02 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmWild",
        "Illustration",
        "50%_done",
        "note",
        "%_tag",
        "2026-01-01 00:00:00",
    )
    .await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    // エスケープされていれば "%" / "_" はリテラルとして扱われ、QmWild だけに一致する。
    for query in ["%", "_"] {
        let (_, items) =
            get_content_list(&app, &token, &format!("?q={}", percent_encode(query))).await;
        assert_eq!(listed_cids(&items), vec!["QmWild"], "query={query}");
    }
    let (_, items) = get_content_list(&app, &token, &format!("?tag={}", percent_encode("%"))).await;
    assert_eq!(listed_cids(&items), vec!["QmWild"]);

    // SQL 断片を入れても全件ヒットせず、リテラル文字列として扱われる。
    let injection = "%' OR 1=1 --";
    let (_, items) =
        get_content_list(&app, &token, &format!("?q={}", percent_encode(injection))).await;
    assert!(
        items.is_empty(),
        "LIKE wildcards must not be interpreted as SQL or patterns"
    );
}

/// title / description / tags を付けた multipart 本文を組み立てる。
/// 必須フィールドは既存ヘルパーに任せ、末尾のクロージングデリミタ直前に差し込む。
fn multipart_upload_body_with_metadata(
    boundary: &str,
    file_bytes: &[u8],
    title: Option<&str>,
    description: Option<&str>,
    tags: Option<&str>,
) -> Vec<u8> {
    let closing = format!("\r\n--{boundary}--\r\n");
    let mut body = multipart_upload_body_full(
        boundary,
        "upload.jxl",
        "image/jxl",
        file_bytes,
        None,
        "test_user",
        &[0u8; 16],
    );
    body.truncate(body.len() - closing.len());
    for (name, value) in [
        ("title", title),
        ("description", description),
        ("tags", tags),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|value| (name, value)))
    {
        // 値の直後の CRLF は次のデリミタの一部なので、ここでは付けない。
        body.extend_from_slice(
            format!(
                "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(closing.as_bytes());
    body
}

#[tokio::test]
async fn test_upload_rejects_oversized_metadata() {
    let (app, db) = setup_app_with_db().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let long_title = "a".repeat(MAX_CONTENT_TITLE_BYTES + 1);
    let long_description = "a".repeat(MAX_CONTENT_DESCRIPTION_BYTES + 1);
    let many_tags = ["t"; MAX_CONTENT_TAGS + 1].join(",");
    let long_tag = "a".repeat(MAX_CONTENT_TAG_BYTES + 1);
    let cases: Vec<(Option<&str>, Option<&str>, Option<&str>)> = vec![
        (Some(&long_title), None, None),
        (None, Some(&long_description), None),
        (None, None, Some(&many_tags)),
        (None, None, Some(&long_tag)),
    ];
    for (index, (title, description, tags)) in cases.into_iter().enumerate() {
        let boundary = format!("oversized-meta-{index}");
        let body = multipart_upload_body_with_metadata(
            &boundary,
            &[0xFF, 0x0A, 0x01, index as u8],
            title,
            description,
            tags,
        );
        let (status, response) = send_upload(&app, &token, &boundary, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case {index}: {response}");
    }

    // 拒否されたアップロードは保存されない（fixture の1件だけが残る）。
    let mut rows = db.query("SELECT COUNT(*) FROM contents", ()).await.unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_upload_accepts_optional_metadata() {
    let (app, db) = setup_app_with_db().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    // 境界値ちょうどは受理される。
    let boundary = "metadata-boundary";
    let body = multipart_upload_body_with_metadata(
        boundary,
        &[0xFF, 0x0A, 0x01],
        Some(&"a".repeat(MAX_CONTENT_TITLE_BYTES)),
        Some(&"b".repeat(MAX_CONTENT_DESCRIPTION_BYTES)),
        Some(
            &(0..MAX_CONTENT_TAGS)
                .map(|i| format!("t{i}"))
                .collect::<Vec<_>>()
                .join(","),
        ),
    );
    let (status, response) = send_upload(&app, &token, boundary, body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "boundary values must be accepted: {response}"
    );

    // 空文字（クライアントが常に送る値）とタグの前後空白も受理される。
    let boundary = "metadata-empty";
    let body = multipart_upload_body_with_metadata(
        boundary,
        &[0xFF, 0x0A, 0x02],
        Some("My cat"),
        Some("A description"),
        Some(" cat , fantasy "),
    );
    let (status, response) = send_upload(&app, &token, boundary, body).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let cid: String = serde_json::from_str(&response).unwrap();
    let mut rows = db
        .query(
            "SELECT title, description, tags FROM contents WHERE cid = ?",
            [cid],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let title: String = row.get(0).unwrap();
    let description: String = row.get(1).unwrap();
    let tags: String = row.get(2).unwrap();
    assert_eq!(title, "My cat");
    assert_eq!(description, "A description");
    assert_eq!(tags, "cat,fantasy");

    let boundary = "metadata-blank";
    let body = multipart_upload_body_with_metadata(
        boundary,
        &[0xFF, 0x0A, 0x03],
        Some(""),
        Some(""),
        Some(""),
    );
    let (status, _) = send_upload(&app, &token, boundary, body).await;
    assert_eq!(status, StatusCode::OK, "empty metadata must be accepted");
}

// ---- R5-02: お気に入り / ブックマーク ----

/// お気に入り操作を送る。ユーザー ID は本文に載せず、認証トークンの sub だけに依存する。
async fn send_favorite_request(
    app: &Router,
    method: axum::http::Method,
    token: &str,
    cid: &str,
    body: Body,
) -> (StatusCode, serde_json::Value) {
    let uri = format!("/content/{cid}/favorite");
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::builder()
                .method(method)
                .uri(uri.as_str())
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, value)
}

async fn post_favorite(app: &Router, token: &str, cid: &str) -> (StatusCode, serde_json::Value) {
    send_favorite_request(app, axum::http::Method::POST, token, cid, Body::empty()).await
}

async fn delete_favorite(app: &Router, token: &str, cid: &str) -> (StatusCode, serde_json::Value) {
    send_favorite_request(app, axum::http::Method::DELETE, token, cid, Body::empty()).await
}

async fn favorite_count(db: &libsql::Connection, sub: &str, cid: &str) -> i64 {
    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM favorites WHERE user_hash = ? AND cid = ?",
            (sub.to_string(), cid.to_string()),
        )
        .await
        .unwrap();
    rows.next().await.unwrap().unwrap().get(0).unwrap()
}

#[tokio::test]
async fn test_favorite_add_and_remove_are_idempotent() {
    let (app, db) = setup_app_with_db().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    // 同じ CID を2回追加しても行は1つだけ（UNIQUE + INSERT OR IGNORE）。
    for _ in 0..2 {
        let (status, body) = post_favorite(&app, &token, "QmTestCID").await;
        assert_eq!(status, StatusCode::OK, "response: {body}");
        assert_eq!(body, json!({"is_favorite": true}));
        let parsed: FavoriteResponse = serde_json::from_value(body).unwrap();
        assert!(parsed.is_favorite);
        assert_eq!(favorite_count(&db, "test_user", "QmTestCID").await, 1);
    }

    // 未登録の削除も冪等で、行が0件のまま成功する。
    for _ in 0..2 {
        let (status, body) = delete_favorite(&app, &token, "QmTestCID").await;
        assert_eq!(status, StatusCode::OK, "response: {body}");
        assert_eq!(body, json!({"is_favorite": false}));
        let parsed: FavoriteResponse = serde_json::from_value(body).unwrap();
        assert!(!parsed.is_favorite);
        assert_eq!(favorite_count(&db, "test_user", "QmTestCID").await, 0);
    }

    // 削除後に再追加できる。
    let (status, _) = post_favorite(&app, &token, "QmTestCID").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(favorite_count(&db, "test_user", "QmTestCID").await, 1);
}

#[tokio::test]
async fn test_favorite_requires_auth_and_valid_approved_cid() {
    let (app, db) = setup_app_with_db().await;

    // 未認証は 401（ POST / DELETE とも）。
    for method in [axum::http::Method::POST, axum::http::Method::DELETE] {
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::builder()
                    .method(method)
                    .uri("/content/QmTestCID/favorite")
                    .body(Body::empty())
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    // 英数字以外の CID は既存の is_safe_cid で拒否される。
    let (status, _) = post_favorite(&app, &token, "bad-cid!").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Approved 以外は対象にできない。
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
         VALUES ('QmReported', 'Illustration', 'test_user', 'sum', 'Reported')",
        (),
    )
    .await
    .unwrap();
    let (status, _) = post_favorite(&app, &token, "QmReported").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(favorite_count(&db, "test_user", "QmReported").await, 0);
}

#[tokio::test]
async fn test_favorite_uses_claims_sub_only() {
    let (app, db) = setup_app_with_db().await;
    // FK のため attacker も users に存在するものとする（OIDC 経由で作成される）。
    db.execute("INSERT INTO users (user_hash) VALUES ('attacker')", ())
        .await
        .unwrap();
    let victim_token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let attacker_token = generate_token(TEST_JWT_SECRET, "attacker").await;

    // 攻撃者のトークン + 本文に被害者の user_hash を入れても無視され、
    // 増えるのは攻撃者自身の行だけ。
    let body = Body::from(json!({"user_hash": "test_user"}).to_string());
    let (status, response) = send_favorite_request(
        &app,
        axum::http::Method::POST,
        &attacker_token,
        "QmTestCID",
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response: {response}");
    assert_eq!(favorite_count(&db, "test_user", "QmTestCID").await, 0);
    assert_eq!(favorite_count(&db, "attacker", "QmTestCID").await, 1);

    // 攻撃者の削除も攻撃者の行にだけ作用する。
    let (status, _) = delete_favorite(&app, &attacker_token, "QmTestCID").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(favorite_count(&db, "test_user", "QmTestCID").await, 0);
    assert_eq!(favorite_count(&db, "attacker", "QmTestCID").await, 0);

    // 被害者本人の追加は被害者の行だけを作る。
    let (status, _) = post_favorite(&app, &victim_token, "QmTestCID").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(favorite_count(&db, "test_user", "QmTestCID").await, 1);
    assert_eq!(favorite_count(&db, "attacker", "QmTestCID").await, 0);
}

#[tokio::test]
async fn test_favorite_only_lists_own_favorites_only() {
    let (app, db) = setup_app_with_db().await;
    db.execute("DELETE FROM contents", ()).await.unwrap();
    db.execute("INSERT INTO users (user_hash) VALUES ('attacker')", ())
        .await
        .unwrap();
    insert_content_meta(
        &db,
        "QmFavMine",
        "Illustration",
        "mine",
        "",
        "",
        "2026-01-04 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmFavOther",
        "Illustration",
        "other",
        "",
        "",
        "2026-01-03 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmPlain",
        "Illustration",
        "plain",
        "",
        "",
        "2026-01-02 00:00:00",
    )
    .await;
    db.execute(
        "INSERT INTO favorites (user_hash, cid) VALUES ('test_user', 'QmFavMine')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO favorites (user_hash, cid) VALUES ('attacker', 'QmFavOther')",
        (),
    )
    .await
    .unwrap();
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let attacker_token = generate_token(TEST_JWT_SECRET, "attacker").await;

    // 絞り込みなし: 全件を新着順で返し、is_favorite は閲覧者視点。
    let (status, items) = get_content_list(&app, &token, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        listed_cids(&items),
        vec!["QmFavMine", "QmFavOther", "QmPlain"]
    );
    assert_eq!(items[0]["is_favorite"], true);
    assert_eq!(items[1]["is_favorite"], false);
    assert_eq!(items[2]["is_favorite"], false);
    let (_, items) = get_content_list(&app, &attacker_token, "").await;
    assert_eq!(items[0]["is_favorite"], false);
    assert_eq!(items[1]["is_favorite"], true);

    // favorite_only は "1" / "true"（大小文字無視・前後空白許容）で本人分のみ。
    for query in [
        "?favorite_only=1",
        "?favorite_only=true",
        "?favorite_only=TRUE",
        "?favorite_only=%201%20",
    ] {
        let (status, items) = get_content_list(&app, &token, query).await;
        assert_eq!(status, StatusCode::OK, "query={query}");
        assert_eq!(listed_cids(&items), vec!["QmFavMine"], "query={query}");
    }
    let (_, items) = get_content_list(&app, &attacker_token, "?favorite_only=1").await;
    assert_eq!(listed_cids(&items), vec!["QmFavOther"]);

    // それ以外の値は絞り込みなし（安全側の既定）。
    for query in [
        "?favorite_only=0",
        "?favorite_only=false",
        "?favorite_only=maybe",
    ] {
        let (_, items) = get_content_list(&app, &token, query).await;
        assert_eq!(items.len(), 3, "query={query}");
    }

    // 既存条件との AND とページング。JOIN の重複で件数が壊れないことを確認する。
    let (_, items) = get_content_list(
        &app,
        &token,
        "?favorite_only=1&q=mine&type=Illustration&limit=1&offset=0",
    )
    .await;
    assert_eq!(listed_cids(&items), vec!["QmFavMine"]);
    let (_, items) = get_content_list(&app, &token, "?favorite_only=1&offset=1").await;
    assert!(items.is_empty());

    // Approved 限定は維持。Reported に変えたコンテンツはお気に入り一覧から消える。
    db.execute(
        "UPDATE contents SET status = 'Reported' WHERE cid = 'QmFavMine'",
        (),
    )
    .await
    .unwrap();
    let (_, items) = get_content_list(&app, &token, "?favorite_only=1").await;
    assert!(items.is_empty());
}

#[test]
fn test_content_meta_backward_compatible_without_is_favorite() {
    let legacy = json!({
        "cid": "QmLegacy",
        "checksum": "sum",
        "type": "Illustration",
        "status": "Approved",
        "price": 0,
        "title": "",
        "description": "",
        "tags": [],
    });
    let meta: ContentMeta = serde_json::from_value(legacy).unwrap();
    assert!(!meta.is_favorite, "旧 JSON は false として読める");

    let mut current = json!({
        "cid": "QmCurrent",
        "checksum": "sum",
        "type": "Novel",
        "status": "Approved",
        "price": 0,
        "title": "title",
        "description": "description",
        "tags": ["cat"],
        "is_favorite": true,
    });
    let meta: ContentMeta = serde_json::from_value(current.clone()).unwrap();
    assert!(meta.is_favorite);
    assert_eq!(serde_json::to_value(&meta).unwrap(), current);

    current["is_favorite"] = json!(false);
    let meta: ContentMeta = serde_json::from_value(current).unwrap();
    assert!(!meta.is_favorite);
}

#[test]
fn test_favorite_only_parser_and_query_builder() {
    let make = |value: Option<&str>| ContentQuery {
        q: None,
        tag: None,
        content_type: None,
        limit: None,
        offset: None,
        favorite_only: value.map(ToString::to_string),
    };
    for truthy in ["1", "true", "TRUE", " true "] {
        assert!(
            parse_content_query(&make(Some(truthy))).favorite_only,
            "truthy={truthy:?}"
        );
    }
    for falsy in [None, Some("0"), Some("false"), Some("yes"), Some("")] {
        assert!(
            !parse_content_query(&make(falsy)).favorite_only,
            "falsy={falsy:?}"
        );
    }

    let (sql_false, params_false) =
        build_content_query(&parse_content_query(&make(None)), "viewer_sub");
    assert_eq!(sql_false.matches("EXISTS").count(), 1);
    assert_eq!(params_false.len(), 3); // SELECT 用 sub + limit + offset
    let filter = parse_content_query(&make(Some("1")));
    let (sql_true, params_true) = build_content_query(&filter, "viewer_sub");
    assert_eq!(sql_true.matches("EXISTS").count(), 2);
    assert_eq!(params_true.len(), 4); // 上記 + favorite_only 用 sub
    assert!(sql_true.contains("FROM favorites"));
    assert!(
        !sql_true.contains("viewer_sub"),
        "sub は SQL 文へ埋め込まずバインドする"
    );
    assert!(
        params_true
            .iter()
            .any(|value| matches!(value, libsql::Value::Text(text) if text == "viewer_sub"))
    );
}

#[tokio::test]
async fn test_migration_012_favorites_unique_and_cascade() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    crate::migration::run_migrations(&db).await.unwrap();
    // libsql の既定に依存せず、CASCADE の検証のため FK を有効化する。
    db.execute("PRAGMA foreign_keys = ON", ()).await.unwrap();
    db.execute("INSERT INTO users (user_hash) VALUES ('u1')", ())
        .await
        .unwrap();
    db.execute("INSERT INTO users (user_hash) VALUES ('u2')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, checksum) VALUES ('QmFav', 'sum')",
        (),
    )
    .await
    .unwrap();

    // UNIQUE(user_hash, cid): 同じ組の2回目の INSERT は失敗する。
    db.execute(
        "INSERT INTO favorites (user_hash, cid) VALUES ('u1', 'QmFav')",
        (),
    )
    .await
    .unwrap();
    assert!(
        db.execute(
            "INSERT INTO favorites (user_hash, cid) VALUES ('u1', 'QmFav')",
            (),
        )
        .await
        .is_err(),
        "duplicate favorite must violate UNIQUE(user_hash, cid)"
    );
    db.execute(
        "INSERT OR IGNORE INTO favorites (user_hash, cid) VALUES ('u1', 'QmFav')",
        (),
    )
    .await
    .unwrap();
    assert_eq!(favorite_count(&db, "u1", "QmFav").await, 1);
    // 別ユーザーなら同じ CID を登録できる。
    db.execute(
        "INSERT INTO favorites (user_hash, cid) VALUES ('u2', 'QmFav')",
        (),
    )
    .await
    .unwrap();
    assert_eq!(favorite_count(&db, "u2", "QmFav").await, 1);

    // 指定のインデックスが作成されている。
    let mut rows = db
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' \
             AND name IN ('idx_favorites_user_cid', 'idx_favorites_user_created')",
            (),
        )
        .await
        .unwrap();
    let mut names = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        names.push(row.get::<String>(0).unwrap());
    }
    names.sort();
    assert_eq!(
        names,
        vec!["idx_favorites_user_cid", "idx_favorites_user_created"]
    );

    // FK CASCADE: コンテンツを削除すると両ユーザーのお気に入りも消える。
    db.execute("DELETE FROM contents WHERE cid = 'QmFav'", ())
        .await
        .unwrap();
    assert_eq!(favorite_count(&db, "u1", "QmFav").await, 0);
    assert_eq!(favorite_count(&db, "u2", "QmFav").await, 0);

    // ユーザー削除でも CASCADE する（別のコンテンツで確認）。
    db.execute(
        "INSERT INTO contents (cid, checksum) VALUES ('QmFav2', 'sum')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO favorites (user_hash, cid) VALUES ('u1', 'QmFav2')",
        (),
    )
    .await
    .unwrap();
    db.execute("DELETE FROM users WHERE user_hash = 'u1'", ())
        .await
        .unwrap();
    assert_eq!(favorite_count(&db, "u1", "QmFav2").await, 0);
}

// ---- R5-09: 透明性ダッシュボード（運用者限定の認証内集計） ----

/// 認証付きで `/mod/stats` を呼ぶ（`None` なら認証ヘッダなし）。
async fn send_mod_stats_request(app: &Router, sub: Option<&str>) -> Response {
    let mut request = Request::get("/mod/stats");
    if let Some(sub) = sub {
        request = request.header(
            "authorization",
            format!("Bearer {}", generate_token(TEST_JWT_SECRET, sub).await),
        );
    }
    app.clone()
        .oneshot(with_peer(
            request.body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap()
}

fn audit_count(month: &str, new_status: &str, count: i64) -> ModAuditCountRow {
    ModAuditCountRow {
        month: month.to_string(),
        new_status: new_status.to_string(),
        count,
    }
}

fn governance_count(month: &str, action: &str, count: i64) -> ModGovernanceCountRow {
    ModGovernanceCountRow {
        month: month.to_string(),
        action: action.to_string(),
        count,
    }
}

fn conclusion_count(month: &str, quorum_not_met: bool, count: i64) -> ModConclusionCountRow {
    ModConclusionCountRow {
        month: month.to_string(),
        quorum_not_met,
        count,
    }
}

const STATS_GENERATED_AT: &str = "2026-11-18T02:00:00+00:00";

#[test]
fn test_build_mod_stats_empty() {
    let stats = build_mod_stats(STATS_GENERATED_AT, &[], &[], &[]);

    assert_eq!(stats.generated_at, STATS_GENERATED_AT);
    assert_eq!(stats.totals, ModStatsTotals::default());
    assert!(stats.monthly.is_empty());
}

#[test]
fn test_build_mod_stats_counts_rejected_status() {
    // ADR-0004 の同数判定（Reject >= Approve を Rejected に倒す）は
    // process_report_conclusion の責務。build_mod_stats は確定済みの監査行を
    // ステータス別に数えるだけなので、実同数は test_consensus_tie_resolves_to_rejected
    // で検証する。ここでは Rejected の監査行が集計から漏れないことを検証する。
    let stats = build_mod_stats(
        STATS_GENERATED_AT,
        &[audit_count("2026-11", "Rejected", 1)],
        &[],
        &[conclusion_count("2026-11", false, 1)],
    );

    assert_eq!(stats.totals.rejected, 1);
    assert_eq!(stats.totals.concluded, 1);
    assert_eq!(stats.totals.quorum_not_met, 0);
    assert_eq!(stats.totals.approved, 0);
    assert_eq!(
        stats.monthly,
        vec![ModStatsMonthly {
            month: "2026-11".to_string(),
            rejected: 1,
            concluded: 1,
            ..Default::default()
        }]
    );
}

#[test]
fn test_build_mod_stats_quorum_not_met_excluded_from_approved() {
    // 定足数未達の確定は new_status='Approved' として監査に残るが、
    // 承認ではなく quorum_not_met として別枠に計上する。
    let stats = build_mod_stats(
        STATS_GENERATED_AT,
        &[audit_count("2026-11", "Approved", 1)],
        &[],
        &[conclusion_count("2026-11", true, 1)],
    );

    assert_eq!(stats.totals.quorum_not_met, 1);
    assert_eq!(stats.totals.approved, 0);
    assert_eq!(stats.totals.concluded, 1);
    assert_eq!(stats.totals.rejected, 0);
    assert_eq!(
        stats.monthly,
        vec![ModStatsMonthly {
            month: "2026-11".to_string(),
            concluded: 1,
            ..Default::default()
        }]
    );
}

#[test]
fn test_build_mod_stats_month_boundary_utc() {
    // 1月31日 23:59:59 と 2月1日 00:00:00 を UTC の YYYY-MM で分ける。
    let stats = build_mod_stats(
        STATS_GENERATED_AT,
        &[
            audit_count("2025-12", "Reported", 1),
            audit_count("2026-01", "Reported", 2),
            audit_count("2026-01", "Approved", 3),
        ],
        &[
            governance_count("2025-12", "BAN", 1),
            governance_count("2026-01", "UNBAN", 1),
            governance_count("2026-01", "UNPIN", 2),
        ],
        &[conclusion_count("2026-01", true, 1)],
    );

    assert_eq!(stats.totals.reports, 3);
    assert_eq!(stats.totals.approved, 2); // 3 - 定足数未達1
    assert_eq!(stats.totals.quorum_not_met, 1);
    assert_eq!(stats.totals.concluded, 1);
    assert_eq!(stats.totals.bans, 1);
    assert_eq!(stats.totals.unbans, 1);
    assert_eq!(stats.totals.unpins, 2);
    assert_eq!(
        stats.monthly,
        vec![
            ModStatsMonthly {
                month: "2025-12".to_string(),
                reports: 1,
                bans: 1,
                ..Default::default()
            },
            ModStatsMonthly {
                month: "2026-01".to_string(),
                reports: 2,
                approved: 2,
                concluded: 1,
                unbans: 1,
                unpins: 2,
                ..Default::default()
            },
        ]
    );
}

#[tokio::test]
async fn test_mod_stats_requires_authentication() {
    let (app, _db) = setup_mod_app_with_operators(&["operator"]).await;
    let response = send_mod_stats_request(&app, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_mod_stats_forbidden_when_operator_not_configured() {
    // 運用者未設定（空集合）はフェイルクローズで 403。
    let (app, _db) = setup_mod_app().await;
    let response = send_mod_stats_request(&app, Some("operator")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_mod_stats_forbidden_when_operator_not_registered() {
    // 設定済み運用者と別の sub は 403。
    let (app, _db) = setup_mod_app_with_operators(&["configured_operator"]).await;
    let response = send_mod_stats_request(&app, Some("operator")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_mod_stats_forbidden_when_operator_banned() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('operator')", ())
        .await
        .unwrap();
    db.execute(
        "UPDATE users SET is_banned = TRUE WHERE user_hash = 'operator'",
        (),
    )
    .await
    .unwrap();

    // BAN 済み運用者は既存の ensure_user_not_banned で拒否される。
    let response = send_mod_stats_request(&app, Some("operator")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_mod_stats_response_contains_no_sensitive_fields() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    db.execute(
        "INSERT INTO content_audit_log
             (cid, old_status, new_status, changed_by, reason, entry_hash, created_at)
         VALUES ('QmModTest', 'Approved', 'Reported', 'SECRET_USER_HASH_SENTINEL',
                 'SECRET_REASON_SENTINEL', 'hash1', '2026-01-31 23:59:59')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO governance_audit_log (action, target, actor_hash, reason, created_at)
         VALUES ('BAN', 'SECRET_CID_SENTINEL', 'SECRET_USER_HASH_SENTINEL',
                 'SECRET_REASON_SENTINEL', '2026-02-01 00:00:00')",
        (),
    )
    .await
    .unwrap();

    let response = send_mod_stats_request(&app, Some("operator")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();

    // 移送してはならない値がレスポンス文字列に一切現れないこと。
    for sentinel in [
        "SECRET_REASON_SENTINEL",
        "SECRET_CID_SENTINEL",
        "SECRET_USER_HASH_SENTINEL",
        "reason",
        "cid",
        "user_hash",
    ] {
        assert!(
            !body.contains(sentinel),
            "response must not contain {sentinel:?}: {body}"
        );
    }

    // ホワイトリストでキー集合を固定（余分なフィールドがないこと）。
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let root = value.as_object().unwrap();
    let mut root_keys: Vec<&String> = root.keys().collect();
    root_keys.sort();
    assert_eq!(
        root_keys,
        vec!["generated_at", "monthly", "totals"],
        "top-level keys must be snake_case and whitelisted"
    );

    let mut total_keys: Vec<&String> = root["totals"].as_object().unwrap().keys().collect();
    total_keys.sort();
    assert_eq!(
        total_keys,
        vec![
            "approved",
            "bans",
            "concluded",
            "quorum_not_met",
            "rejected",
            "reports",
            "unbans",
            "unpins",
        ]
    );

    let monthly = root["monthly"].as_array().unwrap();
    assert_eq!(monthly.len(), 2, "月またぎで 2 エントリになること");
    for entry in monthly {
        let mut keys: Vec<&String> = entry.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "approved",
                "bans",
                "concluded",
                "month",
                "rejected",
                "reports",
                "unbans",
                "unpins",
            ],
            "monthly must not contain quorum_not_met"
        );
    }

    // レスポンス型へも機微フィールドが存在しない（コンパイル時の型保証）。
    let parsed: ModStatsResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed.totals.reports, 1);
    assert_eq!(parsed.totals.bans, 1);
}

#[tokio::test]
async fn test_mod_stats_aggregates_from_real_db() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;

    // 月またぎの監査エントリ（UTC 文字列の先頭7桁で grouping される）。
    db.execute(
        "INSERT INTO content_audit_log
             (cid, old_status, new_status, changed_by, reason, entry_hash, created_at)
         VALUES ('QmModTest', 'Approved', 'Reported', 'reporter', 'report reason',
                 'hash1', '2025-12-31 23:59:59')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO content_audit_log
             (cid, old_status, new_status, changed_by, reason, entry_hash, created_at)
         VALUES ('QmModTest', 'Reported', 'Rejected', 'SYSTEM_GOVERNANCE', 'tie',
                 'hash2', '2026-01-01 00:00:00')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO content_audit_log
             (cid, old_status, new_status, changed_by, reason, entry_hash, created_at)
         VALUES ('QmModTest', 'Rejected', 'Approved', 'SYSTEM_GOVERNANCE', 'quorum not met',
                 'hash3', '2026-01-01 00:00:01')",
        (),
    )
    .await
    .unwrap();

    db.execute(
        "INSERT INTO governance_audit_log (action, target, actor_hash, reason, created_at)
         VALUES ('BAN', 'target1', 'operator', 'ban reason', '2025-12-31 23:59:59')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO governance_audit_log (action, target, actor_hash, reason, created_at)
         VALUES ('UNBAN', 'target1', 'operator', 'unban reason', '2026-01-01 00:00:00')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO governance_audit_log (action, target, actor_hash, reason, created_at)
         VALUES ('UNPIN', 'QmModTest', 'operator', 'unpin reason', '2026-01-01 00:00:00')",
        (),
    )
    .await
    .unwrap();

    // report 1 は定足数充足（4票）の確定。
    db.execute(
        "UPDATE reports SET concluded_at = '2026-01-01 00:00:00' WHERE report_id = 1",
        (),
    )
    .await
    .unwrap();
    // report 2 は定足数未達（2票）の確定。report 3 は未確定で集計対象外。
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason, concluded_at)
         VALUES (2, 'QmModTest', 'secret2', '2026-01-01 00:00:00')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (3, 'QmModTest', 'secret3')",
        (),
    )
    .await
    .unwrap();
    for voter in ["v1", "v2", "v3", "v4", "v5", "v6", "v7"] {
        db.execute(
            "INSERT INTO users (user_hash) VALUES (?)",
            [voter.to_string()],
        )
        .await
        .unwrap();
    }
    for (report_id, voter, result) in [
        (1, "v1", "Approve"),
        (1, "v2", "Approve"),
        (1, "v3", "Reject"),
        (1, "v4", "Reject"),
        (2, "v5", "Approve"),
        (2, "v6", "Reject"),
        (3, "v7", "Approve"),
    ] {
        db.execute(
            "INSERT INTO votes (report_id, voter_hash, vote_result) VALUES (?, ?, ?)",
            (report_id, voter, result),
        )
        .await
        .unwrap();
    }

    let response = send_mod_stats_request(&app, Some("operator")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let stats: ModStatsResponse = serde_json::from_slice(&bytes).unwrap();

    assert!(
        chrono::DateTime::parse_from_rfc3339(&stats.generated_at).is_ok(),
        "generated_at must be RFC3339"
    );
    assert_eq!(stats.totals.reports, 1);
    assert_eq!(stats.totals.concluded, 2);
    assert_eq!(stats.totals.rejected, 1);
    assert_eq!(stats.totals.approved, 0); // Approved監査1件 - 定足数未達1件
    assert_eq!(stats.totals.quorum_not_met, 1);
    assert_eq!(stats.totals.bans, 1);
    assert_eq!(stats.totals.unbans, 1);
    assert_eq!(stats.totals.unpins, 1);
    assert_eq!(
        stats.monthly,
        vec![
            ModStatsMonthly {
                month: "2025-12".to_string(),
                reports: 1,
                bans: 1,
                ..Default::default()
            },
            ModStatsMonthly {
                month: "2026-01".to_string(),
                rejected: 1,
                approved: 0,
                concluded: 2,
                unbans: 1,
                unpins: 1,
                ..Default::default()
            },
        ]
    );
}

// ---------------------------------------------------------------------------
// R5-04: GET /me/export（データのエクスポートのみ。削除権は対象外）
// ---------------------------------------------------------------------------

/// `alice`（データ主体）と `bob`（他ユーザー）の両方に全表のデータを投入したアプリ。
async fn setup_me_export_app() -> Router {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;

    let seed = [
        "INSERT INTO users (user_hash, public_key_der, is_banned) VALUES ('alice', X'0102', 0)",
        "INSERT INTO users (user_hash, public_key_der, is_banned) VALUES ('bob', X'0304', 0)",
        "INSERT INTO contents (cid, type, owner_hash, checksum, status, price, created_at, title, description, tags)
             VALUES ('QmAlice1', 'Illustration', 'alice', 'checksum-alice', 'Approved', 100,
                     '2026-01-01T00:00:00Z', 'alice title', 'alice desc', 'art,alice')",
        "INSERT INTO contents (cid, type, owner_hash, checksum, status, price, created_at, title, description, tags)
             VALUES ('QmBob1', 'Novel', 'bob', 'checksum-bob', 'Approved', 200,
                     '2026-01-02T00:00:00Z', 'bob title', 'bob desc', 'novel,bob')",
        "INSERT INTO favorites (user_hash, cid) VALUES ('alice', 'QmAlice1')",
        "INSERT INTO favorites (user_hash, cid) VALUES ('bob', 'QmBob1')",
        "INSERT INTO transactions (tx_id, cid, sender_hash, receiver_hash, amount, tx_hash)
             VALUES (1, 'QmAlice1', 'alice', 'bob', 500, 'tx-alice-to-bob')",
        "INSERT INTO transactions (tx_id, cid, sender_hash, receiver_hash, amount, tx_hash)
             VALUES (2, 'QmBob1', 'bob', 'alice', 700, 'tx-bob-to-alice')",
        "INSERT INTO transactions (tx_id, cid, sender_hash, receiver_hash, amount, tx_hash)
             VALUES (3, 'QmBob1', 'bob', 'bob', 900, 'tx-bob-only')",
        "INSERT INTO signatures (signature_id, target_id, signer_hash, signature_blob, algorithm)
             VALUES (1, 'QmAlice1', 'alice', X'aa', 'RSASSA-PKCS1-v1_5')",
        "INSERT INTO signatures (signature_id, target_id, signer_hash, signature_blob, algorithm)
             VALUES (2, 'QmBob1', 'bob', X'bb', 'RSASSA-PKCS1-v1_5')",
        "INSERT INTO content_audit_log (log_id, cid, old_status, new_status, changed_by, reason, entry_hash)
             VALUES (1, 'QmAlice1', NULL, 'Approved', 'alice', 'alice secret reason', 'hash-a1')",
        "INSERT INTO content_audit_log (log_id, cid, old_status, new_status, changed_by, reason, entry_hash)
             VALUES (2, 'QmBob1', NULL, 'Approved', 'bob', 'bob secret reason', 'hash-b1')",
        "INSERT INTO reports (report_id, target_cid, reason)
             VALUES (1, 'QmAlice1', 'alice private report reason')",
        "INSERT INTO reports (report_id, target_cid, reason)
             VALUES (2, 'QmBob1', 'bob private report reason')",
        "INSERT INTO votes (vote_id, report_id, voter_hash, vote_result)
             VALUES (1, 1, 'alice', 'Approve')",
        "INSERT INTO votes (vote_id, report_id, voter_hash, vote_result)
             VALUES (2, 1, 'bob', 'Reject')",
        "INSERT INTO votes (vote_id, report_id, voter_hash, vote_result)
             VALUES (3, 2, 'bob', 'Approve')",
        "INSERT INTO payment_orders (order_id, ppid, chain_id, wallet_address, recipient_address,
                                     amount_wei, status, expires_at)
             VALUES ('order-alice', 'alice', 1, '0xAAA', '0xRECIPIENT', '1000000000000000000',
                     'PENDING', '2027-01-01T00:00:00.000Z')",
        "INSERT INTO payment_orders (order_id, ppid, chain_id, wallet_address, recipient_address,
                                     amount_wei, status, expires_at)
             VALUES ('order-bob', 'bob', 1, '0xBBB', '0xRECIPIENT', '2000000000000000000',
                     'PENDING', '2027-01-02T00:00:00.000Z')",
        "INSERT INTO governance_audit_log (log_id, action, target, actor_hash, reason, details)
             VALUES (1, 'BAN', 'alice', 'operator', 'alice ban reason', 'alice ban details')",
        "INSERT INTO governance_audit_log (log_id, action, target, actor_hash, reason, details)
             VALUES (2, 'BAN', 'bob', 'operator', 'bob ban reason', 'bob ban details')",
        "INSERT INTO governance_audit_log (log_id, action, target, actor_hash, reason, details)
             VALUES (3, 'UNPIN', 'QmAlice1', 'alice', 'self reason', NULL)",
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
             VALUES ('hash-alice-secret', 'alice', 'family-alice-secret', '2027-01-01T00:00:00Z')",
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
             VALUES ('hash-bob-secret', 'bob', 'family-bob-secret', '2027-01-02T00:00:00Z')",
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES ('alice', 1000)",
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES ('bob', 2000)",
    ];
    for sql in seed {
        db.execute(sql, ()).await.unwrap();
    }

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: reqwest::Client::new(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        rate_limit: None,
    });
    build_router(shared_state)
}

/// `GET /me/export` を送信する。`sub = None` は Authorization ヘッダ無し。
async fn me_export_request(app: &Router, sub: Option<&str>) -> Response {
    let mut builder = Request::get("/me/export");
    if let Some(sub) = sub {
        let token = generate_token(TEST_JWT_SECRET, sub).await;
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(with_peer(
            builder.body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap()
}

/// 200 応答を JSON として読み出す。
async fn me_export_body(app: &Router, sub: &str) -> serde_json::Value {
    let response = me_export_request(app, Some(sub)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 10 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// R5-04: 2 ユーザーを用意し、自分のデータのみ返る（他ユーザー非漏えい）ことを検証する。
#[tokio::test]
async fn me_export_returns_only_own_data() {
    let app = setup_me_export_app().await;
    let export = me_export_body(&app, "alice").await;

    assert_eq!(export["version"], 1);
    assert_eq!(export["sub"], "alice");
    assert!(
        chrono::DateTime::parse_from_rfc3339(export["generated_at"].as_str().unwrap()).is_ok(),
        "generated_at must be RFC3339"
    );

    assert_eq!(export["users"].as_array().unwrap().len(), 1);
    assert_eq!(export["users"][0]["user_hash"], "alice");
    assert_eq!(export["contents"].as_array().unwrap().len(), 1);
    assert_eq!(export["contents"][0]["cid"], "QmAlice1");
    assert_eq!(export["favorites"].as_array().unwrap().len(), 1);
    assert_eq!(export["favorites"][0]["cid"], "QmAlice1");
    // alice は sender / receiver のどちらかで 2 件だけ（bob 同士の tx は含まない）。
    assert_eq!(export["transactions"].as_array().unwrap().len(), 2);
    assert_eq!(export["signatures"].as_array().unwrap().len(), 1);
    assert_eq!(export["signatures"][0]["signer_hash"], "alice");
    assert_eq!(export["content_audit_log"].as_array().unwrap().len(), 1);
    assert_eq!(export["reports"].as_array().unwrap().len(), 1);
    assert_eq!(export["votes"].as_array().unwrap().len(), 1);
    assert_eq!(export["votes"][0]["voter_hash"], "alice");
    assert_eq!(export["payment_orders"].as_array().unwrap().len(), 1);
    assert_eq!(export["payment_orders"][0]["ppid"], "alice");
    // target = alice と actor_hash = alice の 2 件（bob 宛だけの行は含まない）。
    assert_eq!(export["governance_audit_log"].as_array().unwrap().len(), 2);
    assert_eq!(export["refresh_tokens"].as_array().unwrap().len(), 1);
    assert_eq!(export["refresh_tokens"][0]["sub"], "alice");
    assert_eq!(export["revoked_subs"].as_array().unwrap().len(), 1);
    assert_eq!(export["revoked_subs"][0]["sub"], "alice");

    // 取引の cid には相手側コンテンツの CID が入り得る（alice が受取側の tx2）。
    // それは仕様どおり本人の取引行なので、コンテンツ行としての漏えいは別途検査する。
    assert!(
        export["contents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["cid"] != "QmBob1")
    );
    let raw = export.to_string();
    for marker in [
        "checksum-bob",
        "bob title",
        "tx-bob-only",
        "order-bob",
        "hash-bob-secret",
        "family-bob-secret",
    ] {
        assert!(!raw.contains(marker), "bob-only data leaked: {marker}");
    }

    // 対称性: bob のエクスポートにも alice 専用のデータは混入しない。
    let bob = me_export_body(&app, "bob").await;
    assert_eq!(bob["sub"], "bob");
    assert_eq!(bob["users"][0]["user_hash"], "bob");
    assert_eq!(bob["contents"][0]["cid"], "QmBob1");
    // bob は sender / receiver 双方の条件に一致する 3 件すべてを受け取る。
    assert_eq!(bob["transactions"].as_array().unwrap().len(), 3);
    assert_eq!(bob["signatures"][0]["signer_hash"], "bob");
    assert_eq!(bob["reports"].as_array().unwrap().len(), 1);
    assert_eq!(bob["votes"].as_array().unwrap().len(), 2);
    assert_eq!(bob["payment_orders"][0]["ppid"], "bob");
    assert_eq!(bob["governance_audit_log"].as_array().unwrap().len(), 1);
    assert_eq!(bob["refresh_tokens"][0]["sub"], "bob");
    assert_eq!(bob["revoked_subs"][0]["sub"], "bob");

    assert!(
        bob["contents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["cid"] != "QmAlice1")
    );
    let bob_raw = bob.to_string();
    for marker in [
        "checksum-alice",
        "alice title",
        "order-alice",
        "hash-alice-secret",
        "family-alice-secret",
    ] {
        assert!(
            !bob_raw.contains(marker),
            "alice-only data leaked: {marker}"
        );
    }
}

/// R5-04: データが無いユーザーでは 12 配列すべてが空になる。
#[tokio::test]
async fn me_export_empty_user_has_empty_arrays() {
    // setup_app は test_user / QmTestCID のみを投入する。別 sub には行が無い。
    let app = setup_app().await;
    let export = me_export_body(&app, "empty_user").await;

    assert_eq!(export["version"], 1);
    assert_eq!(export["sub"], "empty_user");
    for key in [
        "users",
        "contents",
        "favorites",
        "transactions",
        "signatures",
        "content_audit_log",
        "reports",
        "votes",
        "payment_orders",
        "governance_audit_log",
        "refresh_tokens",
        "revoked_subs",
    ] {
        assert_eq!(
            export[key].as_array().unwrap().len(),
            0,
            "{key} must be empty for a user with no data"
        );
    }
}

/// R5-04: token_hash / sid / family_id 等が JSON に含まれない。
#[tokio::test]
async fn me_export_excludes_sensitive_fields() {
    let app = setup_me_export_app().await;
    let export = me_export_body(&app, "alice").await;
    let raw = export.to_string();

    for forbidden in [
        "token_hash",
        "family_id",
        "\"sid\"",
        "revoked_sessions",
        "xprv",
        "PINATA_JWT",
        "private_key",
        "signing_key",
    ] {
        assert!(
            !raw.contains(forbidden),
            "export must not contain sensitive field: {forbidden}"
        );
    }

    // refresh_tokens 行は sub / expires_at / used_at / created_at の 4 項目のみ。
    let refresh = export["refresh_tokens"][0].as_object().unwrap();
    let keys: std::collections::BTreeSet<&str> = refresh.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        std::collections::BTreeSet::from(["sub", "expires_at", "used_at", "created_at"])
    );
}

/// R5-04: reason / details が common::mask_reason の既定（マスク）で返る。
#[tokio::test]
async fn me_export_masks_reason_and_details() {
    let app = setup_me_export_app().await;
    let export = me_export_body(&app, "alice").await;
    let raw = export.to_string();

    let audit_reason = export["content_audit_log"][0]["reason"].as_str().unwrap();
    assert_eq!(
        audit_reason,
        format!("[redacted:{} bytes]", "alice secret reason".len())
    );
    let report_reason = export["reports"][0]["reason"].as_str().unwrap();
    assert_eq!(
        report_reason,
        format!("[redacted:{} bytes]", "alice private report reason".len())
    );

    let governance = export["governance_audit_log"].as_array().unwrap();
    let ban = governance
        .iter()
        .find(|row| row["log_id"] == 1)
        .expect("BAN row targeting alice");
    assert_eq!(
        ban["reason"],
        format!("[redacted:{} bytes]", "alice ban reason".len())
    );
    assert_eq!(
        ban["details"],
        format!("[redacted:{} bytes]", "alice ban details".len())
    );

    for secret in [
        "alice secret reason",
        "alice private report reason",
        "alice ban reason",
        "alice ban details",
        "bob secret reason",
        "bob private report reason",
        "bob ban reason",
        "bob ban details",
    ] {
        assert!(!raw.contains(secret), "raw reason leaked: {secret}");
    }
}

/// R5-04: 未認証は 401。
#[tokio::test]
async fn me_export_requires_auth() {
    let app = setup_me_export_app().await;

    let response = me_export_request(&app, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let req = Request::get("/me/export")
        .header("authorization", "Bearer invalid.token.value")
        .body(Body::empty())
        .unwrap();
    let response = app
        .clone()
        .oneshot(with_peer(req, test_ipv6_peer()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// R5-04: Content-Disposition / Cache-Control ヘッダを検証する。
#[tokio::test]
async fn me_export_headers() {
    let app = setup_me_export_app().await;
    let response = me_export_request(&app, Some("alice")).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"openillustration-export.json\""
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
}

// ---------------------------------------------------------------------------
// R5-04: GET /mod/reports（通報理由の要約化）
// ---------------------------------------------------------------------------

/// 認証付き一覧は要約を返し、DB の全文や末尾センチネルを漏らさない。
#[tokio::test]
async fn list_reports_returns_summarized_reason() {
    let (app, db) = setup_app_with_db().await;
    db.execute(
        "UPDATE contents SET status = 'Reported' WHERE cid = 'QmTestCID'",
        (),
    )
    .await
    .unwrap();

    let full_reason = format!(
        "SENTINEL_HEAD user@example.com {} _TAIL_SENTINEL",
        "x".repeat(130)
    );
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmTestCID', ?)",
        [full_reason.clone()],
    )
    .await
    .unwrap();

    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let response = app
        .oneshot(with_peer(
            Request::get("/mod/reports")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains(&full_reason), "全文が漏れた: {text}");
    assert!(
        !text.contains("_TAIL_SENTINEL"),
        "末尾センチネルが漏れた: {text}"
    );

    let items: Vec<crate::ReportResponse> = serde_json::from_slice(&body).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].reason, common::summarize_reason(&full_reason));
    assert_eq!(items[0].reason.chars().count(), 120);
    assert!(items[0].reason.ends_with('…'));
    assert!(items[0].reason.contains("[redacted]"));
    assert!(!items[0].reason.contains("example.com"));

    // 要約はレスポンスだけに適用され、DB には全文が残る（監査エクスポート用）。
    let mut rows = db
        .query("SELECT reason FROM reports WHERE report_id = 1", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), full_reason);
}

/// 未認証の GET /mod/reports は従来どおり 401。
#[tokio::test]
async fn list_reports_requires_auth() {
    let app = setup_app().await;
    let response = app
        .oneshot(with_peer(
            Request::get("/mod/reports").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Reported コンテンツのみ、新着順で返る（既存契約の維持確認）。
#[tokio::test]
async fn list_reports_filters_reported_and_orders_newest_first() {
    let (app, db) = setup_app_with_db().await;
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
         VALUES ('QmOtherCID', 'Illustration', 'test_user', 'mock_checksum', 'Approved')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "UPDATE contents SET status = 'Reported' WHERE cid = 'QmTestCID'",
        (),
    )
    .await
    .unwrap();
    for (id, cid, reason, created_at) in [
        (1, "QmTestCID", "old reason", "2026-01-01 00:00:00"),
        (2, "QmOtherCID", "should not appear", "2026-01-02 00:00:00"),
        (3, "QmTestCID", "new reason", "2026-01-03 00:00:00"),
    ] {
        db.execute(
            "INSERT INTO reports (report_id, target_cid, reason, created_at)
             VALUES (?, ?, ?, ?)",
            (id, cid, reason, created_at),
        )
        .await
        .unwrap();
    }

    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let response = app
        .oneshot(with_peer(
            Request::get("/mod/reports")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let items: Vec<crate::ReportResponse> = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        items.iter().map(|item| item.report_id).collect::<Vec<_>>(),
        vec![3, 1]
    );
    assert_eq!(items[0].reason, "new reason");
    assert_eq!(items[1].reason, "old reason");
}

/// 監査: 再認証時の revoked_subs 削除レース回帰テスト。
///
/// callback はメモリの古い閾値を remove した後、
/// `DELETE FROM revoked_subs WHERE sub = ? AND revoked_at <= old` を実行する。
/// その間に back-channel logout が新しい閾値で UPSERT しても直近の失効が消えないこと、
/// および `revoked_at <= old` 条件を外すと新しい閾値も消えること（mutation 対比）を確認する。
/// `sub` は PRIMARY KEY のため、同じ sub の行は常に 1 行だけである点に注意。
#[tokio::test]
async fn test_reauthentication_delete_keeps_newer_revocation_threshold() {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;

    let old_threshold: i64 = 1_700_000_000;
    let new_threshold: i64 = old_threshold + 100;

    async fn seed(db: &libsql::Connection, sub: &str, old: i64, new: i64) {
        // 1. 古い閾値を永続化する。
        db.execute(
            "INSERT INTO revoked_subs (sub, revoked_at) VALUES (?, ?)",
            (sub.to_string(), old),
        )
        .await
        .unwrap();
        // 2. レースを模擬: 削除の前に back-channel logout が新しい閾値を登録する。
        //    oidc::backchannel_logout と同じ UPSERT を使う。
        db.execute(
            "INSERT INTO revoked_subs (sub, revoked_at) VALUES (?, ?)
             ON CONFLICT(sub) DO UPDATE SET revoked_at = excluded.revoked_at,
                 updated_at = CURRENT_TIMESTAMP",
            (sub.to_string(), new),
        )
        .await
        .unwrap();
    }

    // 本命ケース: callback が呼ぶ本番関数を使う（SQL をテスト内で複製しない）。
    seed(&db, "test_user", old_threshold, new_threshold).await;
    crate::oidc::delete_superseded_revocation(&db, "test_user", old_threshold)
        .await
        .unwrap();

    let mut rows = db
        .query(
            "SELECT revoked_at FROM revoked_subs WHERE sub = ?",
            ["test_user".to_string()],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect(
        "newer revocation threshold must survive the re-authentication delete (race regression)",
    );
    let stored: i64 = row.get(0).unwrap();
    assert_eq!(
        stored, new_threshold,
        "the newer threshold registered during the race must not be overwritten"
    );
    assert!(rows.next().await.unwrap().is_none());

    // mutation 対比: 条件を `WHERE sub = ?` に変えると、新しい閾値も消えることを確認する。
    // これにより本命アサーションが `revoked_at <= old` 条件に依存していることを示す。
    // （oidc.rs は書き込み禁止のため、SQL 条件の一時変更は親が別途実施する。）
    seed(&db, "mutation_control", old_threshold, new_threshold).await;
    db.execute(
        "DELETE FROM revoked_subs WHERE sub = ?",
        ["mutation_control".to_string()],
    )
    .await
    .unwrap();
    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM revoked_subs WHERE sub = ?",
            ["mutation_control".to_string()],
        )
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        count, 0,
        "control: without the revoked_at condition the newer threshold is deleted"
    );
}

// --- OIDC 起動時設定の fail-closed と境界値（from_env_with による依存注入） ---

/// `OidcState::from_env_with` に渡す必須 7 変数のベースライン。
fn oidc_required_env() -> std::collections::BTreeMap<String, String> {
    [
        ("OIDC_ISSUER", "https://issuer.example.jp"),
        ("OIDC_CLIENT_ID", "client-123"),
        ("OIDC_CLIENT_SECRET", "secret-456"),
        ("OIDC_REDIRECT_URI", "https://app.example.jp/auth/callback"),
        (
            "OIDC_AUTHORIZATION_ENDPOINT",
            "https://issuer.example.jp/authorize",
        ),
        ("OIDC_TOKEN_ENDPOINT", "https://issuer.example.jp/token"),
        ("OIDC_JWKS_URI", "https://issuer.example.jp/jwks"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

/// 必須 7 変数が揃えば OIDC が有効化され、値がそのまま設定へ反映される。
#[test]
fn test_oidc_from_env_with_all_required_present() {
    let env = oidc_required_env();
    let state = crate::oidc::OidcState::from_env_with(|name| env.get(name).cloned())
        .expect("all required OIDC variables present must enable the provider");
    let config = state.config();
    assert_eq!(config.issuer, "https://issuer.example.jp");
    assert_eq!(config.client_id, "client-123");
    assert_eq!(config.client_secret, "secret-456");
    assert_eq!(config.redirect_uri, "https://app.example.jp/auth/callback");
    assert_eq!(
        config.authorization_endpoint,
        "https://issuer.example.jp/authorize"
    );
    assert_eq!(config.token_endpoint, "https://issuer.example.jp/token");
    assert_eq!(config.jwks_uri, "https://issuer.example.jp/jwks");
    assert_eq!(config.scopes, "openid", "scopes の既定値は openid");
    assert!(config.cookie_secure, "cookie_secure の既定値は true");
}

/// 必須変数を 1 つでも欠くと None（fail-closed）。7 通りすべてを確認する。
#[test]
fn test_oidc_from_env_with_missing_required_variable_fails_closed() {
    const REQUIRED: [&str; 7] = [
        "OIDC_ISSUER",
        "OIDC_CLIENT_ID",
        "OIDC_CLIENT_SECRET",
        "OIDC_REDIRECT_URI",
        "OIDC_AUTHORIZATION_ENDPOINT",
        "OIDC_TOKEN_ENDPOINT",
        "OIDC_JWKS_URI",
    ];
    for missing in REQUIRED {
        let mut env = oidc_required_env();
        env.remove(missing);
        assert!(
            crate::oidc::OidcState::from_env_with(|name| env.get(name).cloned()).is_none(),
            "missing {missing} must disable OIDC (fail-closed)"
        );
    }
}

/// ClockSkew: 既定 60、境界値は u64 の範囲で受け付け、不正値は既定へ戻す。
#[test]
fn test_oidc_from_env_with_clock_skew_boundaries() {
    let assert_skew = |value: Option<&str>, expected: u64| {
        let mut env = oidc_required_env();
        if let Some(value) = value {
            env.insert("OIDC_CLOCK_SKEW_SECS".to_string(), value.to_string());
        }
        let state = crate::oidc::OidcState::from_env_with(|name| env.get(name).cloned())
            .expect("required variables are present");
        assert_eq!(
            state.config().clock_skew_secs,
            expected,
            "OIDC_CLOCK_SKEW_SECS={value:?}"
        );
    };
    assert_skew(None, 60);
    assert_skew(Some("0"), 0);
    assert_skew(Some("1"), 1);
    assert_skew(Some("300"), 300);
    assert_skew(Some("18446744073709551615"), u64::MAX);
    // u64 に収まらない / 負数 / 空 / 非数値は既定値へフォールバック。
    assert_skew(Some("18446744073709551616"), 60);
    assert_skew(Some("-1"), 60);
    assert_skew(Some(""), 60);
    assert_skew(Some("abc"), 60);
}

/// max_auth_age: 0 と不正値は「検証しない」、正の u64 はそのまま保持する。
#[test]
fn test_oidc_from_env_with_max_auth_age_boundaries() {
    let assert_age = |value: Option<&str>, expected: Option<u64>| {
        let mut env = oidc_required_env();
        if let Some(value) = value {
            env.insert("OIDC_MAX_AUTH_AGE_SECS".to_string(), value.to_string());
        }
        let state = crate::oidc::OidcState::from_env_with(|name| env.get(name).cloned())
            .expect("required variables are present");
        assert_eq!(
            *state.config().max_auth_age_secs.lock().unwrap(),
            expected,
            "OIDC_MAX_AUTH_AGE_SECS={value:?}"
        );
    };
    assert_age(None, None);
    assert_age(Some("0"), None); // 0 秒は「検証しない」扱い
    assert_age(Some("1"), Some(1));
    assert_age(Some("900"), Some(900));
    assert_age(Some("18446744073709551615"), Some(u64::MAX));
    assert_age(Some("18446744073709551616"), None);
    assert_age(Some("-1"), None);
    assert_age(Some("abc"), None);
}

/// required_acr / required_amr: カンマ区切りを trim し空要素を除く。
#[test]
fn test_oidc_from_env_with_required_acr_amr_normalization() {
    let assert_lists =
        |acr: Option<&str>, amr: Option<&str>, expected_acr: &[&str], expected_amr: &[&str]| {
            let mut env = oidc_required_env();
            if let Some(acr) = acr {
                env.insert("OIDC_REQUIRED_ACR".to_string(), acr.to_string());
            }
            if let Some(amr) = amr {
                env.insert("OIDC_REQUIRED_AMR".to_string(), amr.to_string());
            }
            let state = crate::oidc::OidcState::from_env_with(|name| env.get(name).cloned())
                .expect("required variables are present");
            let config = state.config();
            assert_eq!(
                config.required_acr.lock().unwrap().as_slice(),
                expected_acr,
                "OIDC_REQUIRED_ACR={acr:?}"
            );
            assert_eq!(
                config.required_amr.lock().unwrap().as_slice(),
                expected_amr,
                "OIDC_REQUIRED_AMR={amr:?}"
            );
        };
    assert_lists(None, None, &[], &[]);
    assert_lists(Some(""), Some(""), &[], &[]);
    assert_lists(Some("   "), None, &[], &[]);
    assert_lists(
        Some(" urn:acr:1, , urn:acr:2 ,"),
        Some(" pwd , otp "),
        &["urn:acr:1", "urn:acr:2"],
        &["pwd", "otp"],
    );
}

/// オプション値の既定と cookie_secure の真偽判定（0 / false 系のみ偽）。
#[test]
fn test_oidc_from_env_with_optional_defaults_and_cookie_secure() {
    let assert_cookie = |value: Option<&str>, expected: bool| {
        let mut env = oidc_required_env();
        env.insert("OIDC_SCOPES".to_string(), "openid profile".to_string());
        if let Some(value) = value {
            env.insert("OIDC_COOKIE_SECURE".to_string(), value.to_string());
        }
        let state = crate::oidc::OidcState::from_env_with(|name| env.get(name).cloned())
            .expect("required variables are present");
        assert_eq!(state.config().scopes, "openid profile");
        assert_eq!(
            state.config().cookie_secure,
            expected,
            "OIDC_COOKIE_SECURE={value:?}"
        );
    };
    assert_cookie(None, true);
    assert_cookie(Some("0"), false);
    assert_cookie(Some("false"), false);
    assert_cookie(Some("FALSE"), false);
    assert_cookie(Some("1"), true);
    assert_cookie(Some(""), true);
}
