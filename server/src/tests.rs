use crate::{
    AppError, AppState, VerifyRequest, VerifyResponse, allowed_origins, build_router,
    ensure_user_not_banned,
    payments::{MAX_ORDER_JPY, PaymentOrder, PaymentVerification, PaymentVerifier},
    process_report_conclusion, update_content_status, validate_age, validate_jwt_secret,
    validate_upload_content, validate_upload_content_type,
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
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        payment_receiving_address: Some(receiving_address.to_string()),
        payment_receiving_xprv: None,
        payment_verifier: Some(verifier),
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        payment_receiving_address: None,
        payment_receiving_xprv: Some(xprv.to_string()),
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        let state = Arc::new(MockOidcState {
            base_url: base_url.clone(),
            client_id: "test-client".to_string(),
            nonce: nonce.clone(),
            at_hash_override: at_hash_override.clone(),
            forged_key_der: forged_key_der.clone(),
            omit_kid: omit_kid.clone(),
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
            state,
            handle,
        }
    }

    fn logout_token(&self, sid: &str) -> String {
        let now = chrono::Utc::now().timestamp() as usize;
        let claims = json!({
            "iss": self.state.base_url,
            "aud": self.state.client_id,
            "exp": now + 300,
            "iat": now,
            "jti": uuid::Uuid::new_v4().to_string(),
            "sid": sid,
            "events": {"http://schemas.openid.net/event/backchannel-logout": {}},
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("test-kid".to_string());
        encode(&header, &claims, &self.state.encoding_key).unwrap()
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

struct MockOidcState {
    base_url: String,
    client_id: String,
    nonce: Arc<std::sync::Mutex<Option<String>>>,
    at_hash_override: Arc<std::sync::Mutex<Option<String>>>,
    forged_key_der: Arc<std::sync::Mutex<Option<Vec<u8>>>>,
    omit_kid: Arc<std::sync::Mutex<bool>>,
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
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: Some(oidc.clone()),
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });

    (build_router(shared_state), oidc, mock, db)
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

#[derive(Clone)]
struct MockResponse {
    status: StatusCode,
    body: Vec<u8>,
    delay: Duration,
    /// 要求パスがこの CID と一致しない場合は 404 を返す。
    /// `{cid}` 置換の欠落をテストで検出するために使う。
    expected_cid: Option<String>,
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
    Response::builder()
        .status(response.status)
        .body(Body::from(response.body.clone()))
        .expect("mock response")
}

fn verify_body(birth_date: &str, country: &str) -> String {
    serde_json::to_string(&VerifyRequest {
        presentation_token: "mock_token".to_string(),
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

/// SEC-01: パブリック IPv4 は 403
#[tokio::test]
async fn test_ipv4_public_address_forbidden() {
    let _env_guard = ENV_LOCK.lock().await;
    unsafe { std::env::remove_var("ALLOW_IPV4_LOCALHOST") };
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
    unsafe { std::env::remove_var("EXPOSE_INTERNAL_ERRORS") };

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
    unsafe { std::env::set_var("EXPOSE_INTERNAL_ERRORS", "1") };
    let exposed = AppError::Internal("no such table: users".to_string()).into_response();
    let body = to_bytes(exposed.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("no such table"),
        "details must be visible: {text}"
    );

    unsafe { std::env::remove_var("EXPOSE_INTERNAL_ERRORS") };
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
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

    let message = json!({"report_id": report_id, "result": result}).to_string();
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
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    let app = build_router(shared_state);

    let (juror_hash, key) = insert_juror(&db).await;
    assert_eq!(
        send_signed_vote(&app, 9999, "Approve", &juror_hash, &key).await,
        StatusCode::NOT_FOUND,
        "unknown report must return 404"
    );
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
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
    })
    .await;
    let body = b"fallback content".to_vec();
    let checksum = sha256::digest(&body);
    let healthy = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmFallback".to_string()),
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
    })
    .await;
    let second = MockGateway::start(MockResponse {
        status: StatusCode::BAD_GATEWAY,
        body: b"another secret".to_vec(),
        delay: Duration::ZERO,
        expected_cid: Some("QmAllFail".to_string()),
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
    let (order_b, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;

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

/// JPYC: 期限切れ注文の verify は 400 で拒否される。
#[tokio::test]
async fn test_payment_verify_rejects_expired_order() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier.clone()).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, amount_wei) = create_payment_order(&app, &token, &wallet, 137, 1000).await;

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
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
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

/// ADR-0002: back-channel logout で sid を失効させる。
#[tokio::test]
async fn test_backchannel_logout_revokes_session() {
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
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
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
    let keys = crate::jwt_keys_from_env(Some(pem), Some(TEST_JWT_SECRET.to_string()))
        .expect("RSA key wins");
    assert_eq!(keys.algorithm_name(), "RS256");

    let hmac =
        crate::jwt_keys_from_env(None, Some(TEST_JWT_SECRET.to_string())).expect("HMAC fallback");
    assert_eq!(hmac.algorithm_name(), "HS256");

    assert!(crate::jwt_keys_from_env(None, None).is_err());

    // 不正な RSA PEM は、有効な JWT_SECRET があっても黙ってフォールバックしない。
    assert!(
        crate::jwt_keys_from_env(
            Some("not a pem".to_string()),
            Some(TEST_JWT_SECRET.to_string())
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
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down,
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
