use crate::{
    AppError, AppState, ContentFilter, ContentMeta, ContentQuery, DEFAULT_CONTENT_LIMIT,
    FavoriteResponse, MAX_CONTENT_DESCRIPTION_BYTES, MAX_CONTENT_LIMIT, MAX_CONTENT_TAG_BYTES,
    MAX_CONTENT_TAGS, MAX_CONTENT_TITLE_BYTES, ModAuditCountRow, ModConclusionCountRow,
    ModGovernanceCountRow, ModStatsMonthly, ModStatsResponse, ModStatsTotals,
    PublicModStatsMonthly, PublicModStatsResponse, PublicModStatsTotals, VerifyRequest,
    VerifyResponse, allowed_origins, build_content_query, build_mod_stats, build_public_mod_stats,
    build_router, ensure_user_not_banned, escape_like, parse_content_query, parse_pagination,
    parse_tags_field,
    payments::{MAX_ORDER_JPY, PaymentOrder, PaymentVerification, PaymentVerifier},
    process_report_conclusion, sha256_hex, today_for_policy, update_content_status, validate_age,
    validate_age_at, validate_content_metadata, validate_jwt_secret, validate_upload_content,
    validate_upload_content_type,
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
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use tower::ServiceExt;

/// プロセス環境変数を扱うテストの直列化に使う。
/// 書き手（`test_set_env` / `test_remove_env`）の呼び出し側は必ずこのロックを保持し、
/// 読み手は `lock_env_and_read` を通して値を取得する。
static ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// `ENV_LOCK` を取得して環境変数を読み、取得したガードを返す。
/// 読み手も書き手と同じロックを保持したまま値を使い続けられるようにする。
/// 既にガードを保持している場合はこの関数を呼ばないこと（デッドロックする）。
async fn lock_env_and_read(key: &str) -> (tokio::sync::MutexGuard<'_, ()>, Option<String>) {
    let guard = ENV_LOCK.lock().await;
    let value = std::env::var(key).ok();
    (guard, value)
}

/// テスト専用の環境変数設定（呼び出し前に `ENV_LOCK` を保持すること）。
///
/// # Safety
/// Rust 2024 では `set_var` / `remove_var` が unsafe で、呼び出し側は
/// 「他スレッドが同時に環境変数を読まないこと」を保証する必要がある。
/// 書き手の呼び出し箇所はすべて `ENV_LOCK` を保持して直列化しており、
/// テストが直接読む場合も `lock_env_and_read` で同じロックを取得する。
/// `EXPOSE_INTERNAL_ERRORS` は起動時に解釈して `AppState` へ注入する方式へ移行済みで、
/// このロックと無関係に並列テストできる。残る per-request 読込の
/// `OIDC_COOKIE_SECURE` 等はこのロックを通らないため完全な直列化は保証できない。
fn test_set_env(key: &str, value: &str) {
    // SAFETY: 呼び出し側が ENV_LOCK を保持している（上記コメント参照）。
    unsafe { std::env::set_var(key, value) };
}

/// テスト専用の環境変数削除（呼び出し前に `ENV_LOCK` を保持すること）。
fn test_remove_env(key: &str) {
    // SAFETY: 同上。呼び出し側が ENV_LOCK を保持している。
    unsafe { std::env::remove_var(key) };
}

/// テスト用の HS256 秘密。HS256 の要件どおり 32 バイト以上にする。
const TEST_JWT_SECRET: &str = "test_secret_for_openillustration_tests";

/// テスト用の HS256 鍵。
fn test_jwt_keys() -> Arc<crate::jwt::JwtKeys> {
    Arc::new(crate::jwt::JwtKeys::from_hmac_secret(TEST_JWT_SECRET).expect("test_secret is valid"))
}

/// テスト用 HTTP クライアント。本番と同じ `crate::build_http_client` 経由で
/// `Policy::none()` を適用するため、3xx の Location 先へは到達しない。
fn test_http_client() -> reqwest::Client {
    crate::build_http_client(Duration::from_secs(30)).expect("build test HTTP client")
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g', 'Illustration', 'test_user', 'mock_checksum', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![HeaderValue::from_static("http://localhost:8080")],
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g', 'Illustration', 'test_user', 'mock_checksum', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
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

/// 検証中に注文の期限を過ぎさせ、verify 後の再判定を発火させる verifier。
/// 時刻は進めず、トランザクション外の I/O 中に `expires_at` を過去へ書き換える。
struct ExpiryAdvancingVerifier {
    db: std::sync::OnceLock<Arc<libsql::Connection>>,
    verification: PaymentVerification,
}

#[async_trait]
impl PaymentVerifier for ExpiryAdvancingVerifier {
    async fn verify(&self, order: &PaymentOrder) -> Result<PaymentVerification, AppError> {
        let db = self.db.get().ok_or_else(|| {
            AppError::Internal("ExpiryAdvancingVerifier has no database".to_string())
        })?;
        db.execute(
            "UPDATE payment_orders SET expires_at = ? WHERE order_id = ?",
            (
                "2000-01-01T00:00:00+00:00".to_string(),
                order.order_id.clone(),
            ),
        )
        .await?;
        Ok(self.verification.clone())
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z', 'Novel', 'test_user', 'abc', 'Approved', 1000)",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z', 'Novel', 'test_user', 'abc', 'Approved', 1000)",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
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
    /// 設定時は ID トークンの nbf を絶対 UNIX 時刻で上書きする（R1-19 nbf 検証用）。
    nbf_override: Arc<std::sync::Mutex<Option<usize>>>,
    /// 設定時は ID トークンから nbf クレームを省略する（Option 扱いの確認用）。
    omit_nbf: Arc<std::sync::Mutex<bool>>,
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
        let nbf_override = Arc::new(std::sync::Mutex::new(None));
        let omit_nbf = Arc::new(std::sync::Mutex::new(false));
        let auth_time = Arc::new(std::sync::Mutex::new(None));
        let state = Arc::new(MockOidcState {
            base_url: base_url.clone(),
            client_id: "test-client".to_string(),
            nonce: nonce.clone(),
            at_hash_override: at_hash_override.clone(),
            forged_key_der: forged_key_der.clone(),
            omit_kid: omit_kid.clone(),
            nbf_override: nbf_override.clone(),
            omit_nbf: omit_nbf.clone(),
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
            nbf_override,
            omit_nbf,
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
        self.logout_token_for_sub_with_times(sub, now, None)
    }

    /// iat / nbf を明示して logout token を署名する（新鮮性・再送冪等テスト用）。
    /// `nbf` が None のときは nbf クレームを付与しない。
    fn logout_token_for_sub_with_times(&self, sub: &str, iat: usize, nbf: Option<usize>) -> String {
        let now = chrono::Utc::now().timestamp() as usize;
        let exp = iat.max(now) + 300;
        let mut claims = json!({
            "iss": self.state.base_url,
            "aud": self.state.client_id,
            "exp": exp,
            "iat": iat,
            "jti": uuid::Uuid::new_v4().to_string(),
            "sub": sub,
            "events": {"http://schemas.openid.net/event/backchannel-logout": {}},
        });
        if let (Some(base), Some(nbf)) = (claims.as_object_mut(), nbf) {
            base.insert("nbf".to_string(), json!(nbf));
        }
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
    /// 設定時は ID トークンの nbf を絶対 UNIX 時刻で上書きする（R1-19 nbf 検証用）。
    nbf_override: Arc<std::sync::Mutex<Option<usize>>>,
    /// 設定時は ID トークンから nbf クレームを省略する（Option 扱いの確認用）。
    omit_nbf: Arc<std::sync::Mutex<bool>>,
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
    // R1-19: nbf を差し替える／省略して、検証の有無を観測できるようにする。
    if *state.omit_nbf.lock().unwrap() {
        claims.as_object_mut().unwrap().remove("nbf");
    } else if let Some(nbf) = *state.nbf_override.lock().unwrap() {
        claims["nbf"] = json!(nbf);
    }
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
    setup_app_with_oidc_age_policy_and_cookie_secure(age_policy, false).await
}

/// `OIDC_COOKIE_SECURE` 相当の設定を差し替えて OIDC アプリを組み立てる。
async fn setup_app_with_oidc_age_policy_and_cookie_secure(
    age_policy: common::age::AgePolicy,
    cookie_secure: bool,
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
        cookie_secure,
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
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![HeaderValue::from_static("http://localhost:8080")],
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

    // R2-ENV: read_env 分類を数値パーサ入力と固定警告へ変換する（生バイト非保持）。
    let utf8 = common::env::EnvValue::Utf8("42".to_string());
    let (input, warning) = crate::numeric_env_value("RATE_LIMIT_PER_MIN", &utf8);
    assert_eq!(input, Some("42"));
    assert_eq!(warning, None);
    assert_eq!(crate::rate_limit_per_min(input), 42);

    let missing = common::env::EnvValue::Missing;
    let (input, warning) = crate::numeric_env_value("RATE_LIMIT_PER_MIN", &missing);
    assert_eq!(input, None);
    assert_eq!(warning, None);
    assert_eq!(crate::rate_limit_per_min(input), 60);

    let non_utf8 = common::env::EnvValue::NonUtf8;
    let (input, warning) = crate::numeric_env_value("RATE_LIMIT_PER_MIN", &non_utf8);
    assert_eq!(input, None);
    assert_eq!(crate::rate_limit_per_min(input), 60);
    // 固定文言はキー名のみで値・生バイトを含まない。
    assert_eq!(
        warning.as_deref(),
        Some("RATE_LIMIT_PER_MIN is not valid UTF-8; using default")
    );

    // P1-ENV: 範囲付き分類は「正規化後の値が入力と一致しない」場合に固定文言警告を出す。
    // 未設定・範囲内・下限・上限は警告なし、範囲外・タイポ・非 UTF-8 は警告あり。
    fn classify(env: &common::env::EnvValue) -> (Option<&str>, Option<String>) {
        crate::numeric_env_value_in_range("RATE_LIMIT_PER_MIN", env, 1u32, 600)
    }
    assert_eq!(classify(&common::env::EnvValue::Missing), (None, None));
    for valid in ["1", "60", "600"] {
        let env = common::env::EnvValue::Utf8(valid.to_string());
        let (input, warning) = classify(&env);
        assert_eq!(input, Some(valid));
        assert_eq!(warning, None, "value={valid}");
    }
    for invalid in ["0", "601", "abc", "01", " 60 "] {
        let env = common::env::EnvValue::Utf8(invalid.to_string());
        let (input, warning) = classify(&env);
        assert_eq!(input, Some(invalid));
        assert_eq!(
            warning.as_deref(),
            Some("RATE_LIMIT_PER_MIN is invalid or outside 1..=600; using default"),
            "value={invalid:?}"
        );
    }
    let non_utf8 = common::env::EnvValue::NonUtf8;
    let (input, warning) = classify(&non_utf8);
    assert_eq!(input, None);
    assert_eq!(
        warning.as_deref(),
        Some("RATE_LIMIT_PER_MIN is not valid UTF-8; using default")
    );
}

/// P1-BIND: BIND_ADDR / PORT の起動時解決。既定・有効値・不正値・特権ポートを検証する。
#[test]
fn test_bind_addr_port_resolution_boundaries() {
    use common::env::EnvValue;

    // 未設定は現行の `[::]:3000`。
    assert_eq!(
        crate::resolve_bind_addr(EnvValue::Missing, EnvValue::Missing).unwrap(),
        SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 3000))
    );
    // IPv4 / IPv6 の受理値と下限ポート 1024。
    assert_eq!(
        crate::resolve_bind_addr(
            EnvValue::Utf8("127.0.0.1".to_string()),
            EnvValue::Utf8("8080".to_string())
        )
        .unwrap(),
        SocketAddr::from(([127, 0, 0, 1], 8080))
    );
    assert_eq!(
        crate::resolve_bind_addr(
            EnvValue::Utf8("::1".to_string()),
            EnvValue::Utf8("1024".to_string())
        )
        .unwrap(),
        SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 1024)
    );

    // 不正 IP・非 UTF-8 は固定文言で拒否（値はエコーしない）。
    for bad in [
        EnvValue::Utf8("not-an-ip".to_string()),
        EnvValue::Utf8("127.0.0.1:3000".to_string()),
        EnvValue::NonUtf8,
    ] {
        assert_eq!(
            crate::resolve_bind_addr(bad, EnvValue::Missing),
            Err(crate::BIND_ADDR_INVALID)
        );
    }
    // 不正・特権（<1024）・範囲外・非 UTF-8 ポートも固定文言で拒否。
    for bad in [
        EnvValue::Utf8("abc".to_string()),
        EnvValue::Utf8("0".to_string()),
        EnvValue::Utf8("80".to_string()),
        EnvValue::Utf8("65536".to_string()),
        EnvValue::NonUtf8,
    ] {
        assert_eq!(
            crate::resolve_bind_addr(EnvValue::Missing, bad),
            Err(crate::PORT_INVALID)
        );
    }
    assert!(!crate::BIND_ADDR_INVALID.contains("not-an-ip"));
    assert!(!crate::PORT_INVALID.contains("65536"));
}

/// P1-ENV: RUST_LOG と数値 env の起動時警告境界（既定・範囲内・範囲外・非 UTF-8）。
#[test]
fn test_startup_env_warning_boundaries() {
    use common::env::EnvValue;

    // RUST_LOG: 未設定 / 有効フィルタは警告なし、非 UTF-8・空・不正フィルタは固定文言。
    assert_eq!(crate::rust_log_filter(&EnvValue::Missing).1, None);
    assert_eq!(
        crate::rust_log_filter(&EnvValue::Utf8("info".to_string())).1,
        None
    );
    for bad in [
        EnvValue::NonUtf8,
        EnvValue::Utf8(String::new()),
        EnvValue::Utf8("server=not-a-level".to_string()),
    ] {
        assert_eq!(
            crate::rust_log_filter(&bad).1,
            Some(crate::RUST_LOG_INVALID_WARNING)
        );
    }
    assert_eq!(
        crate::RUST_LOG_INVALID_WARNING,
        "RUST_LOG is not a valid filter directive; using the default filter (info; value redacted)"
    );

    // 7 つの数値 env の範囲外がすべて固定文言警告になる（値・生バイトは非エコー）。
    macro_rules! out_of_range {
        ($name:expr, $value:expr, $min:expr, $max:expr) => {{
            let env = $value;
            let (input, warning) = crate::numeric_env_value_in_range($name, &env, $min, $max);
            assert!(input.is_some(), "{}", $name);
            let expected = format!(
                "{} is invalid or outside {}..={}; using default",
                $name, $min, $max
            );
            assert_eq!(warning.as_deref(), Some(expected.as_str()), "{}", $name);
            assert!(
                !warning.unwrap().contains("999999999"),
                "{} must not echo the value",
                $name
            );
        }};
    }
    out_of_range!(
        "RATE_LIMIT_PER_MIN",
        EnvValue::Utf8("999999999".to_string()),
        1u32,
        600u32
    );
    out_of_range!(
        "MAX_CONCURRENT_REQUESTS",
        EnvValue::Utf8("999999999".to_string()),
        1usize,
        256usize
    );
    out_of_range!(
        "IPFS_TIMEOUT_SECS",
        EnvValue::Utf8("999999999".to_string()),
        1u64,
        300u64
    );
    out_of_range!(
        "IPFS_MAX_ATTEMPTS",
        EnvValue::Utf8("999999999".to_string()),
        1usize,
        5usize
    );
    out_of_range!(
        "OIDC_HTTP_TIMEOUT_SECS",
        EnvValue::Utf8("999999999".to_string()),
        1u64,
        60u64
    );
    out_of_range!(
        "MODERATION_FINALIZE_INTERVAL_SECS",
        EnvValue::Utf8("999999999".to_string()),
        30u64,
        3600u64
    );
    out_of_range!(
        "GRACEFUL_DRAIN_TIMEOUT_SECS",
        EnvValue::Utf8("999999999".to_string()),
        1u64,
        300u64
    );

    // 未設定・境界値・範囲内は警告なし（既定値フォールバックの判定と一致）。
    assert_eq!(
        crate::numeric_env_value_in_range(
            "GRACEFUL_DRAIN_TIMEOUT_SECS",
            &EnvValue::Missing,
            1u64,
            300u64
        ),
        (None, None)
    );
    for valid in ["1", "30", "300"] {
        let value = EnvValue::Utf8(valid.to_string());
        let (input, warning) =
            crate::numeric_env_value_in_range("GRACEFUL_DRAIN_TIMEOUT_SECS", &value, 1u64, 300u64);
        assert_eq!(input, Some(valid));
        assert_eq!(warning, None, "value={valid}");
    }
}

/// R4-02: `MAX_CONCURRENT_REQUESTS` の境界値。
/// 実装は `.filter(|n| (1..=256).contains(n)).unwrap_or(DEFAULT)` なので、
/// 範囲外の値は上限 256 へクランプせず既定値 16 に倒れる（安全側の既定）。
#[test]
fn test_max_concurrent_requests_defaults_and_bounds() {
    // 未設定と有効範囲内の値。
    assert_eq!(
        crate::max_concurrent_requests(None),
        16,
        "未設定は既定値 16"
    );
    assert_eq!(
        crate::max_concurrent_requests(Some("1")),
        1,
        "下限 1 は有効"
    );
    assert_eq!(
        crate::max_concurrent_requests(Some("16")),
        16,
        "既定値そのもの"
    );
    assert_eq!(
        crate::max_concurrent_requests(Some("256")),
        256,
        "上限 256 は有効"
    );

    // 不正値・範囲外はすべて既定値 16 にフォールバックする。
    assert_eq!(crate::max_concurrent_requests(Some("0")), 16, "0 は範囲外");
    assert_eq!(
        crate::max_concurrent_requests(Some("-1")),
        16,
        "負数はパース失敗"
    );
    assert_eq!(
        crate::max_concurrent_requests(Some("bad")),
        16,
        "非数値はパース失敗"
    );
    assert_eq!(
        crate::max_concurrent_requests(Some("")),
        16,
        "空文字はパース失敗"
    );
    assert_eq!(
        crate::max_concurrent_requests(Some("257")),
        16,
        "上限超過はクランプではなく既定値に倒れる"
    );
    assert_eq!(
        crate::max_concurrent_requests(Some("999999999999999999999999")),
        16,
        "極端な過大値（usize オーバーフロー）も既定値に倒れる"
    );
}

/// CORS プリフライト（OPTIONS）はレート制限のクォータを消費しない。
#[tokio::test]
async fn test_rate_limit_ignores_preflight() {
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
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: Some(Arc::new(crate::RateLimitState {
            limiter: governor::RateLimiter::keyed(governor::Quota::per_minute(
                NonZeroU32::new(1).unwrap(),
            )),
            concurrency: Arc::new(tokio::sync::Semaphore::new(16)),
        })),
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let app = build_router(state);
    let peer = test_ipv6_peer();

    // 3 回の OPTIONS はクォータを消費せず、その後の GET が通る。
    for _ in 0..3 {
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::options("/metrics/client-events")
                    .body(Body::empty())
                    .unwrap(),
                peer,
            ))
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }
    let response = app
        .oneshot(with_peer(
            Request::get("/no-such-path").body(Body::empty()).unwrap(),
            peer,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: Some(Arc::new(crate::RateLimitState {
            limiter: governor::RateLimiter::keyed(governor::Quota::per_minute(
                NonZeroU32::new(2).unwrap(),
            )),
            concurrency: Arc::new(tokio::sync::Semaphore::new(16)),
        })),
        pinata_jwt: None,
        allowed_origins: vec![],
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

/// ネットワーク判定だけを通す app（信頼プロキシあり）。
fn trusted_network_app() -> Router {
    Router::new()
        .fallback(|| async { StatusCode::OK })
        .layer(axum::middleware::from_fn(trusted_network_middleware))
}

/// ネットワーク判定だけを通す app（信頼プロキシなし）。
fn untrusted_network_app() -> Router {
    Router::new()
        .fallback(|| async { StatusCode::OK })
        .layer(axum::middleware::from_fn(untrusted_network_middleware))
}

async fn rate_limit_test_state(per_minute: u32) -> Arc<AppState> {
    rate_limit_test_state_with_concurrency(per_minute, 16).await
}

/// 同時実行上限を明示してレート制限テスト用の状態を作る。
async fn rate_limit_test_state_with_concurrency(
    per_minute: u32,
    concurrency: usize,
) -> Arc<AppState> {
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
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: Some(Arc::new(crate::RateLimitState {
            limiter: governor::RateLimiter::keyed(governor::Quota::per_minute(
                NonZeroU32::new(per_minute).unwrap(),
            )),
            concurrency: Arc::new(tokio::sync::Semaphore::new(concurrency)),
        })),
        pinata_jwt: None,
        allowed_origins: vec![],
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

/// R4-02: 同時実行上限 1 の動作を実リクエストで検証する。
///
/// 1 個目を遅延ハンドラで保持している間は 2 個目が 429 になり、
/// 1 個目を解放して permit が返却されると再リクエストが通る。
/// レート制限（IP 単位クォータ）は十分大きくして同時実行上限だけを切り分ける。
#[tokio::test]
async fn test_concurrent_request_limit_rejects_second_and_releases_permit() {
    // ハンドラの開始通知と、1 個目を保持するための通知。
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let handler_entered = Arc::clone(&entered);
    let handler_release = Arc::clone(&release);

    let state = rate_limit_test_state_with_concurrency(100, 1).await;
    // permit の取得／返却を直接観測するためのハンドル（layer に渡す前に複製）。
    let concurrency = Arc::clone(&state.rate_limit.as_ref().unwrap().concurrency);
    let app = Router::new()
        .route(
            "/hold",
            axum::routing::get(move || {
                let entered = Arc::clone(&handler_entered);
                let release = Arc::clone(&handler_release);
                async move {
                    entered.notify_one();
                    release.notified().await;
                    StatusCode::OK
                }
            }),
        )
        .route("/fast", axum::routing::get(|| async { StatusCode::OK }))
        .layer(axum::middleware::from_fn_with_state(
            state,
            crate::rate_limit_middleware,
        ));

    let peer = test_ipv6_peer();
    let hold_request = || Request::get("/hold").body(Body::empty()).unwrap();
    let fast_request = || Request::get("/fast").body(Body::empty()).unwrap();

    // 1 個目を別タスクで開始し、ハンドラ到達（permit 取得済み）まで待つ。
    let first = tokio::spawn({
        let app = app.clone();
        async move {
            app.oneshot(with_peer(hold_request(), peer))
                .await
                .unwrap()
                .status()
        }
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("1 個目のリクエストがハンドラに到達しなかった");
    assert_eq!(
        concurrency.available_permits(),
        0,
        "1 個目が唯一の permit を保持していること"
    );

    // 2 個目は /fast（即座に応答する）へ送る。permit が無いため 429 になる。
    // /fast を使うことで、失敗分岐が外れた mutation ではハングせず 200 となり即検知できる。
    let second = app
        .clone()
        .oneshot(with_peer(fast_request(), peer))
        .await
        .unwrap()
        .status();
    assert_eq!(
        second,
        StatusCode::TOO_MANY_REQUESTS,
        "同時実行上限を超えた 2 個目は 429 で拒否されること"
    );

    // 1 個目を解放して完了を待つ。
    release.notify_one();
    assert_eq!(first.await.unwrap(), StatusCode::OK);
    assert_eq!(
        concurrency.available_permits(),
        1,
        "1 個目の完了で permit が返却されること"
    );

    // permit が返却されているので、次のリクエストは通る。
    let third = app
        .oneshot(with_peer(fast_request(), peer))
        .await
        .unwrap()
        .status();
    assert_eq!(
        third,
        StatusCode::OK,
        "permit 解放後は再リクエストが通ること"
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

/// 敵対的レビュー: 明示設定必須なのに未設定の場合、callback / refresh も 403 にする。
/// 信頼属性必須モードだけで設定チェックしていた抜け穴の回帰テスト。
#[tokio::test]
async fn test_require_explicit_configuration_blocks_oidc_callback_and_refresh() {
    let policy = common::age::AgePolicy {
        require_explicit_configuration: true,
        ..common::age::AgePolicy::default()
    };
    let (app, oidc, mock, _db) = setup_app_with_oidc_and_age_policy(policy).await;

    // callback は state を消費する前に共通ゲートで拒否される。
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

    // refresh もクッキーより先に同じ共通ゲートで拒否される。
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

    // auth_verify も同じ共通ゲートに載る。require=1 未充足 + trusted_mode +
    // 有効な verified_claims でも、クレーム検証 / nonce 消費より前に 403 になる。
    let gated_policy = common::age::AgePolicy {
        require_explicit_configuration: true,
        ..trusted_age_policy()
    };
    let (gated_app, _gated_db) = setup_verified_age_app(gated_policy).await;
    let nonce = crate::issue_mock_nonce_for_test();
    let (status, _) = post_verify(&gated_app, verified_verify_body(&nonce, "jpki-test", "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
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

/// SSRF 検証用のローカルシナリオ。
/// ゲートウェイは 302 + `Location: <リダイレクト先>` を返し、リダイレクト先は
/// 内部サービス（例: 169.254.169.254）に見立てたローカルサーバー。
/// `target_hits` でリダイレクトを追従したかどうかを観測する。
struct RedirectScenario {
    gateway_template: String,
    target_hits: Arc<AtomicUsize>,
    _gateway_handle: tokio::task::JoinHandle<()>,
    _target_handle: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct RedirectTargetState {
    hits: Arc<AtomicUsize>,
    body: Vec<u8>,
}

impl RedirectScenario {
    /// `target_body` はリダイレクト先が返す本文。CID の checksum と一致させれば、
    /// リダイレクトを追従した場合にだけ検証成功（200）になる。
    async fn start(target_body: Vec<u8>) -> Self {
        let target_hits = Arc::new(AtomicUsize::new(0));
        let target_state = RedirectTargetState {
            hits: Arc::clone(&target_hits),
            body: target_body,
        };
        let target_listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind redirect target");
        let target_addr = target_listener
            .local_addr()
            .expect("redirect target address");
        let target_app = Router::new()
            .route("/secret", axum::routing::get(redirect_target_handler))
            .with_state(target_state);
        let target_handle = tokio::spawn(async move {
            axum::serve(target_listener, target_app)
                .await
                .expect("redirect target server");
        });

        let location = Arc::new(format!("http://{target_addr}/secret"));
        let gateway_listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind redirect gateway");
        let gateway_addr = gateway_listener
            .local_addr()
            .expect("redirect gateway address");
        let gateway_app = Router::new()
            .route("/{*path}", axum::routing::get(redirect_gateway_handler))
            .with_state(location);
        let gateway_handle = tokio::spawn(async move {
            axum::serve(gateway_listener, gateway_app)
                .await
                .expect("redirect gateway server");
        });

        Self {
            gateway_template: format!("http://{gateway_addr}/{{cid}}"),
            target_hits,
            _gateway_handle: gateway_handle,
            _target_handle: target_handle,
        }
    }
}

impl Drop for RedirectScenario {
    fn drop(&mut self) {
        self._gateway_handle.abort();
        self._target_handle.abort();
    }
}

async fn redirect_gateway_handler(
    axum::extract::State(location): axum::extract::State<Arc<String>>,
) -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location.as_str())
        .body(Body::empty())
        .expect("redirect response")
}

async fn redirect_target_handler(
    axum::extract::State(state): axum::extract::State<RedirectTargetState>,
) -> Response {
    state.hits.fetch_add(1, Ordering::SeqCst);
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(state.body.clone()))
        .expect("redirect target response")
}

fn verify_body(birth_date: &str, country: &str) -> String {
    serde_json::to_string(&VerifyRequest {
        presentation_token: "mock_token".to_string(),
        nonce: Some(crate::issue_mock_nonce_for_test()),
        public_key_der: None,
        birth_date: Some(birth_date.to_string()),
        country_code: Some(country.to_string()),
        proof_of_possession: None,
        verified_claims: None,
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
        &[0u8; 256],
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
    assert_security_headers(&response);
}

/// 全レスポンスに必須のセキュリティヘッダが付いていることを検証する。
fn assert_security_headers(response: &Response) {
    assert_eq!(response.headers()["X-Content-Type-Options"], "nosniff");
    assert_eq!(response.headers()["X-Frame-Options"], "DENY");
    assert_eq!(
        response.headers()["Content-Security-Policy"],
        "default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
    // R2-SERVER: 機微 API のレスポンスは保存しない（/health /ready /metrics は別途除外）。
    assert_eq!(
        response.headers()["cache-control"],
        "no-store",
        "sensitive API response must not be cached"
    );
}

/// SEC-01: CORS プリフライト（OPTIONS）にもセキュリティヘッダを付与する。
/// CorsLayer は OPTIONS を短絡して内側のミドルウェアを通さないため、
/// 外側の security_headers が無いとヘッダが欠落する。
#[tokio::test]
async fn test_security_headers_on_cors_preflight() {
    let app = setup_app().await;
    let response = app
        .oneshot(with_peer(
            Request::options("/auth/login")
                .header(header::ORIGIN, "http://localhost:8080")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    // tower-http のプリフライトは内側を通らず短絡する（200 または 204）。
    assert!(
        response.status().is_success(),
        "preflight must be short-circuited by CorsLayer: {}",
        response.status()
    );
    assert_security_headers(&response);
}

/// CORS: `remove_favorite` は DELETE ルート。クロスオリジンのプリフライトで
/// DELETE が許可され、許可メソッド一覧に含まれることを検証する。
#[tokio::test]
async fn test_cors_preflight_allows_delete() {
    let app = setup_app().await;
    let response = app
        .oneshot(with_peer(
            Request::options("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g/favorite")
                .header(header::ORIGIN, "http://localhost:8080")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "DELETE")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "DELETE preflight must be short-circuited by CorsLayer: {}",
        response.status()
    );
    let allow_methods = response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_METHODS)
        .expect("preflight must advertise allowed methods")
        .to_str()
        .unwrap();
    assert!(
        allow_methods.contains("DELETE"),
        "allowed methods must include DELETE: {allow_methods}"
    );
    assert!(allow_methods.contains("GET"), "{allow_methods}");
    assert!(allow_methods.contains("POST"), "{allow_methods}");
    assert!(
        !allow_methods.contains("PUT") && !allow_methods.contains("PATCH"),
        "only GET/POST/DELETE must be allowed: {allow_methods}"
    );
}

/// R2-SERVER: クロスオリジンの通常レスポンスで x-request-id が露出され、
/// JS から相関 ID を読めることを検証する（許可オリジンは明示リストのまま）。
#[tokio::test]
async fn test_cors_exposes_request_id_header() {
    let app = setup_app().await;
    let response = app
        .oneshot(with_peer(
            Request::get("/health")
                .header(header::ORIGIN, "http://localhost:8080")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let exposed = response
        .headers()
        .get(header::ACCESS_CONTROL_EXPOSE_HEADERS)
        .expect("cross-origin response must expose x-request-id")
        .to_str()
        .unwrap();
    assert!(
        exposed
            .split(',')
            .any(|name| name.trim().eq_ignore_ascii_case("x-request-id")),
        "unexpected Access-Control-Expose-Headers: {exposed}"
    );
    assert!(
        response.headers().contains_key("x-request-id"),
        "exposed header must actually be present"
    );
}

/// SEC-01: レート制限の 429 レスポンスにもセキュリティヘッダを付与する。
#[tokio::test]
async fn test_security_headers_on_rate_limited_response() {
    let state = rate_limit_test_state(1).await;
    let app = build_router(state);
    let peer = test_ipv6_peer();
    let request = || {
        with_peer(
            Request::get("/no-such-path").body(Body::empty()).unwrap(),
            peer,
        )
    };
    assert_eq!(
        app.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let response = app.oneshot(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_security_headers(&response);
}

/// SEC-01: IPFS ゲートウェイ未設定の 500 レスポンスにもセキュリティヘッダを付与する。
#[tokio::test]
async fn test_security_headers_on_internal_error_response() {
    let (app, db) = setup_app_with_options(Vec::new(), Duration::from_secs(1), true).await;
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'Illustration', 'test_user', 'unused', 'Approved');",
        (),
    )
    .await
    .unwrap();
    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T")
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
    assert_security_headers(&response);
}

/// SEC-01: 本文なしの 204 レスポンスにもセキュリティヘッダを付与する。
#[tokio::test]
async fn test_security_headers_on_no_content_response() {
    let app = setup_app().await;
    let response = app
        .oneshot(client_event_request(Body::from(
            r#"{"event":"neutralized_url"}"#,
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_security_headers(&response);
}

// ---- リクエスト相関 ID（x-request-id）----

/// OBS-01: 全レスポンスへ x-request-id が付与され、リクエストごとに異なる v4 UUID になる。
#[tokio::test]
async fn test_request_id_header_is_unique_uuid() {
    let app = setup_app().await;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..3 {
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::get("/health").body(Body::empty()).unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let raw = response
            .headers()
            .get("x-request-id")
            .expect("every response must carry x-request-id")
            .to_str()
            .unwrap();
        let id = uuid::Uuid::parse_str(raw).expect("x-request-id must be a UUID");
        assert_eq!(id.get_version_num(), 4, "x-request-id must be v4");
        assert_eq!(id.to_string(), raw, "x-request-id must be a canonical UUID");
        assert!(seen.insert(id), "x-request-id must differ per request");
    }
    assert_eq!(seen.len(), 3);
}

/// OBS-01: エラー応答・CORS プリフライトにも x-request-id が付き、
/// 既存のセキュリティヘッダが壊れていないことを検証する。
#[tokio::test]
async fn test_request_id_header_on_error_and_preflight_responses() {
    let app = setup_app().await;

    // 公開 IPv4 は IP ゲートで 403 に短絡する。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/content").body(Body::empty()).unwrap(),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        response.headers().contains_key("x-request-id"),
        "403 response must carry x-request-id"
    );
    assert_security_headers(&response);

    // OPTIONS は CorsLayer で短絡するが、相関 ID とセキュリティヘッダは付与される。
    let response = app
        .oneshot(with_peer(
            Request::options("/auth/login")
                .header(header::ORIGIN, "http://localhost:8080")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "preflight must be short-circuited by CorsLayer: {}",
        response.status()
    );
    assert!(
        response.headers().contains_key("x-request-id"),
        "preflight response must carry x-request-id"
    );
    assert_security_headers(&response);
}

/// OBS-02: 生成した ID がリクエスト extensions にも格納される。
#[tokio::test]
async fn test_request_id_stored_in_request_extensions() {
    async fn echo_request_id(
        axum::extract::Extension(crate::RequestId(id)): axum::extract::Extension<crate::RequestId>,
    ) -> String {
        id.to_string()
    }

    let app = Router::new()
        .route("/echo-id", axum::routing::get(echo_request_id))
        .layer(axum::middleware::from_fn(crate::request_id_and_access_log));

    let response = app
        .oneshot(Request::get("/echo-id").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let header_id = response
        .headers()
        .get("x-request-id")
        .expect("x-request-id must be present")
        .to_str()
        .unwrap()
        .to_owned();
    let body = to_bytes(response.into_body(), 64).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&body), header_id);
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

    // ADR-0013 P3: /metrics のみ IPv4 監視ネットワークからスクレイプ可能。
    // /health は 403 のまま（上のアサーション）で、除去対象が /metrics だけであることを固定する。
    let app = setup_app().await;
    let metrics = app
        .oneshot(with_peer(
            Request::get("/metrics").body(Body::empty()).unwrap(),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
        ))
        .await
        .unwrap();
    assert_eq!(
        metrics.status(),
        StatusCode::OK,
        "/metrics must remain scrapeable from IPv4 monitoring network"
    );
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

/// NET-02: 追記型 XFF（末尾が信頼プロキシ自身）では元のクライアントを採用する。
/// 末尾のピアを採用する回帰では IPv6 ピアとして許可されてしまうため 403 で検証する。
#[tokio::test]
async fn test_trusted_proxy_append_style_xff_uses_original_client() {
    let app = trusted_network_app();
    let mut req = Request::get("/no-such-path").body(Body::empty()).unwrap();
    req.headers_mut().insert(
        "x-forwarded-for",
        format!("203.0.113.5, 2001:db8::1234, {RATE_LIMIT_PROXY}")
            .parse::<HeaderValue>()
            .unwrap(),
    );
    let response = app.oneshot(with_peer(req, RATE_LIMIT_PROXY)).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// NET-02: 上書き型 XFF（末尾がピアでない）は従来どおり末尾を採用する。
#[tokio::test]
async fn test_trusted_proxy_overwrite_style_xff_uses_last_entry() {
    let app = trusted_network_app();
    let mut req = Request::get("/no-such-path").body(Body::empty()).unwrap();
    req.headers_mut().insert(
        "x-forwarded-for",
        "203.0.113.5, 2001:db8::1234"
            .parse::<HeaderValue>()
            .unwrap(),
    );
    let response = app.oneshot(with_peer(req, RATE_LIMIT_PROXY)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// NET-02: 信頼プロキシ未設定時は複数値の XFF 全体を無視する。
#[tokio::test]
async fn test_untrusted_proxy_ignores_multi_value_xff() {
    let app = untrusted_network_app();
    let mut req = Request::get("/no-such-path").body(Body::empty()).unwrap();
    req.headers_mut().insert(
        "x-forwarded-for",
        format!("203.0.113.1, 2001:db8::2, {RATE_LIMIT_PROXY}")
            .parse::<HeaderValue>()
            .unwrap(),
    );
    let response = app
        .oneshot(with_peer(req, RATE_LIMIT_UNTRUSTED_PEER))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// SEC-01: IPv4-compatible IPv6（`::a.b.c.d`）は IPv4 として拒否する。
#[tokio::test]
async fn test_ipv4_compatible_peer_is_rejected() {
    let app = setup_app().await;
    let req = with_peer(
        Request::get("/health").body(Body::empty()).unwrap(),
        "::192.0.2.1".parse().unwrap(),
    );
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// SEC-01: 信頼プロキシ経由の XFF に IPv4-compatible 形式が入っても拒否する。
#[tokio::test]
async fn test_ipv4_compatible_forwarded_client_is_rejected() {
    let app = trusted_network_app();
    let mut req = Request::get("/no-such-path").body(Body::empty()).unwrap();
    req.headers_mut().insert(
        "x-forwarded-for",
        "::192.0.2.1".parse::<HeaderValue>().unwrap(),
    );
    let response = app.oneshot(with_peer(req, RATE_LIMIT_PROXY)).await.unwrap();
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
    assert_eq!(origins.len(), 1);
    assert_eq!(origins[0], "https://ok.example.com");
    assert!(
        origins.iter().all(|origin| origin != "null"),
        "`null` must never enter ALLOWED_ORIGINS"
    );
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
    // ALLOWED_ORIGINS の本番判定は client と同じ loopback 代替表現も拒否する。
    assert!(crate::is_production_https_origin("https://api.example.org"));
    for bad in [
        "https://localhost.",
        "https://2130706433",
        "https://0x7f000001",
        "https://[::]",
        "https://127.0.0.1:8443",
        "http://api.example.org",
    ] {
        assert!(
            !crate::is_production_https_origin(bad),
            "loopback alternative must be rejected"
        );
    }

    // モック認証が有効な開発環境では警告のみ（既存の開発フローを壊さない）。
    assert!(
        crate::production_config_errors(true, false, common::env::EnvValue::Missing).is_empty()
    );

    // R2-STARTUP: ALLOW_MOCK_AUTH=1 と本番相当設定の併用は、doctor の
    // mock_auth_with_production_config と同じ共有ヘルパで run() の起動ゲートへ渡される。
    // これによりモック経由で任意 user_hash の JWT が発行される状態を防ぐ。
    const MOCK_PROD_ERROR: &str = "ALLOW_MOCK_AUTH is enabled together with production-like settings; do not deploy this configuration";
    let empty_lookup = |_name: &str| Err(std::env::VarError::NotPresent);
    // モック単独（本番相当設定なし）は従来どおり許可。
    assert_eq!(
        crate::mock_auth_with_production_config_error(true, empty_lookup),
        None
    );
    // 本番相当（allow_mock_auth=false）では併用判定自体は発動しない。
    assert_eq!(
        crate::mock_auth_with_production_config_error(false, |name: &str| {
            if name == "JWT_RSA_PRIVATE_KEY_PEM" {
                Ok("dummy".to_string())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        }),
        None
    );
    // OIDC 必須 7 変数のいずれかが存在すれば固定文言で拒否（値はエコーしない）。
    for name in [
        "OIDC_ISSUER",
        "OIDC_CLIENT_ID",
        "OIDC_CLIENT_SECRET",
        "OIDC_REDIRECT_URI",
        "OIDC_AUTHORIZATION_ENDPOINT",
        "OIDC_TOKEN_ENDPOINT",
        "OIDC_JWKS_URI",
    ] {
        let lookup = |key: &str| {
            if key == name {
                Ok("https://issuer.example".to_string())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        };
        assert_eq!(
            crate::mock_auth_with_production_config_error(true, lookup),
            Some(MOCK_PROD_ERROR),
            "OIDC variable {name} must trigger startup refusal"
        );
    }
    // 実値の PINATA_JWT / JWT_RSA_PRIVATE_KEY_PEM も単独で拒否。
    for (name, value) in [
        ("PINATA_JWT", "real-production-jwt"),
        ("JWT_RSA_PRIVATE_KEY_PEM", "-----BEGIN PRIVATE KEY-----"),
    ] {
        let lookup = |key: &str| {
            if key == name {
                Ok(value.to_string())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        };
        assert_eq!(
            crate::mock_auth_with_production_config_error(true, lookup),
            Some(MOCK_PROD_ERROR),
            "{name} must trigger startup refusal"
        );
        assert!(
            !MOCK_PROD_ERROR.contains(value),
            "secret must not be echoed"
        );
    }
    // 開発用 PINATA_JWT 単独は本番相当設定ではないため拒否しない。
    let mock_jwt_lookup = |key: &str| {
        if key == "PINATA_JWT" {
            Ok("mock_jwt".to_string())
        } else {
            Err(std::env::VarError::NotPresent)
        }
    };
    assert_eq!(
        crate::mock_auth_with_production_config_error(true, mock_jwt_lookup),
        None
    );

    // 本番相当では不足を列挙する。
    let errors = crate::production_config_errors(false, false, common::env::EnvValue::Missing);
    assert_eq!(errors.len(), 2);
    assert!(errors.iter().any(|e| e.contains("OIDC")));
    assert!(errors.iter().any(|e| e.contains("PINATA_JWT")));

    assert_eq!(
        crate::production_config_errors(
            false,
            true,
            common::env::EnvValue::Utf8("real-jwt".to_string())
        ),
        Vec::<&str>::new()
    );
    assert!(
        !crate::production_config_errors(false, true, common::env::EnvValue::Utf8(String::new()))
            .is_empty()
    );
    assert!(
        !crate::production_config_errors(
            false,
            true,
            common::env::EnvValue::Utf8("mock_jwt".to_string())
        )
        .is_empty(),
        "the development mock JWT must not be accepted in production"
    );
    // 前後の空白付きモック値も開発用として拒否（値はエコーしない）。
    for mock in [" mock_jwt ", "\tmock_jwt"] {
        assert_eq!(
            crate::production_config_errors(
                false,
                true,
                common::env::EnvValue::Utf8(mock.to_string())
            ),
            vec!["PINATA_JWT is the development mock value"],
            "mock={mock:?} must be rejected without echoing the value"
        );
    }

    // 空白・タブ・改行のみの値も空とみなし fail-closed。固定文言のみで値はエコーしない。
    for blank in [" ", "\t", "\n", "\r\n", " \t\n "] {
        assert_eq!(
            crate::production_config_errors(
                false,
                true,
                common::env::EnvValue::Utf8(blank.to_string())
            ),
            vec!["PINATA_JWT is empty"],
            "blank={blank:?} must be rejected without echoing the value"
        );
    }
    // 非 UTF-8 は common::env 分類器の NonUtf8 として区別され、
    // run() / doctor と同じ固定文言「PINATA_JWT is empty」になる（値はエコーしない）。
    let non_utf8 = common::env::classify(Err::<String, _>(std::env::VarError::NotUnicode(
        std::ffi::OsString::from_vec(vec![0xff]),
    )));
    assert_eq!(non_utf8, common::env::EnvValue::NonUtf8);
    assert_eq!(
        crate::production_config_errors(false, true, non_utf8),
        vec!["PINATA_JWT is empty"]
    );

    // R2-ENV: LOG_FORMAT は未設定 / json / text（大小文字不問）のみ受理し、
    // タイポ・非 UTF-8 は共通の固定文言へ倒す（値・生バイトはエコーしない）。
    assert_eq!(
        crate::log_format_state(common::env::EnvValue::Missing),
        crate::LogFormatState::Text
    );
    for value in ["json", "JSON", "text", "TeXt"] {
        assert_eq!(
            crate::log_format_state(common::env::EnvValue::Utf8(value.to_string())),
            if value.eq_ignore_ascii_case("json") {
                crate::LogFormatState::Json
            } else {
                crate::LogFormatState::Text
            },
            "value={value}"
        );
    }
    for value in ["jsonn", "xml", ""] {
        assert_eq!(
            crate::log_format_state(common::env::EnvValue::Utf8(value.to_string())),
            crate::LogFormatState::Invalid,
            "value={value:?}"
        );
    }
    assert_eq!(
        crate::log_format_state(common::env::EnvValue::NonUtf8),
        crate::LogFormatState::Invalid
    );
    assert_eq!(
        crate::LOG_FORMAT_INVALID_WARNING,
        "LOG_FORMAT must be json or text; using text (invalid value)"
    );

    // 実値は従来どおり警告なし（前後の空白があっても非空なら許可）。
    assert!(
        crate::production_config_errors(
            false,
            true,
            common::env::EnvValue::Utf8("  real-jwt  ".to_string())
        )
        .is_empty()
    );

    // R2-PINATA: 共有ヘルパ pinata_jwt_production_error の直接アサーション。
    // 3 固定文言・非エコー・実値の受理を production_config_errors と同じ基準で検証する。
    assert_eq!(
        crate::pinata_jwt_production_error(&common::env::EnvValue::Missing),
        Some("PINATA_JWT is not configured")
    );
    assert_eq!(
        crate::pinata_jwt_production_error(&common::env::EnvValue::NonUtf8),
        Some("PINATA_JWT is empty")
    );
    for blank in [" ", "\t", "\n", " \t\n "] {
        assert_eq!(
            crate::pinata_jwt_production_error(&common::env::EnvValue::Utf8(blank.to_string())),
            Some("PINATA_JWT is empty"),
            "blank={blank:?} must be rejected without echoing the value"
        );
    }
    for mock in ["mock_jwt", " mock_jwt ", "\tmock_jwt"] {
        assert_eq!(
            crate::pinata_jwt_production_error(&common::env::EnvValue::Utf8(mock.to_string())),
            Some("PINATA_JWT is the development mock value"),
            "mock={mock:?} must be rejected without echoing the value"
        );
    }
    assert_eq!(
        crate::pinata_jwt_production_error(&common::env::EnvValue::Utf8("real-jwt".to_string())),
        None
    );
    // pinata_jwt_production_like は共有判定の is_none() に委譲済み（実値のみ受理）。
    assert!(crate::pinata_jwt_production_like(
        common::env::EnvValue::Utf8("real-jwt".to_string())
    ));
    assert!(!crate::pinata_jwt_production_like(
        common::env::EnvValue::Utf8(" mock_jwt ".to_string())
    ));

    // P1-PINATA: 起動時解決は doctor と同じ判定で、実トークンのみ Some を返す。
    // 未設定・非 UTF-8・空白のみ・モック値は None（モック経路）へ倒す（値はエコーしない）。
    assert_eq!(
        crate::resolve_pinata_jwt(common::env::EnvValue::Missing),
        None
    );
    assert_eq!(
        crate::resolve_pinata_jwt(common::env::EnvValue::NonUtf8),
        None
    );
    for rejected in ["", " ", "\tmock_jwt\n", "mock_jwt"] {
        assert_eq!(
            crate::resolve_pinata_jwt(common::env::EnvValue::Utf8(rejected.to_string())),
            None,
            "rejected={rejected:?}"
        );
    }
    assert_eq!(
        crate::resolve_pinata_jwt(common::env::EnvValue::Utf8("real-jwt".to_string())),
        Some("real-jwt".to_string())
    );

    // R2-DBURL-SERVER: 本番相当では DATABASE_URL が必須（既定なし、値はエコーしない）。
    assert_eq!(
        crate::production_database_url_error(false, common::env::EnvValue::Missing),
        Some("DATABASE_URL is not configured (required in production)")
    );
    assert_eq!(
        crate::production_database_url_error(false, common::env::EnvValue::NonUtf8),
        Some("DATABASE_URL is not valid UTF-8")
    );
    for blank in [" ", "\t", "\n", " \t\n "] {
        assert_eq!(
            crate::production_database_url_error(
                false,
                common::env::EnvValue::Utf8(blank.to_string())
            ),
            Some("DATABASE_URL is empty"),
            "blank={blank:?} must be rejected without echoing the value"
        );
    }
    assert_eq!(
        crate::production_database_url_error(
            false,
            common::env::EnvValue::Utf8("file:local.db".to_string())
        ),
        Some("DATABASE_URL must be an absolute local file path in production"),
        "the old relative default must be rejected in production"
    );
    assert_eq!(
        crate::production_database_url_error(
            false,
            common::env::EnvValue::Utf8("  file:local.db  ".to_string())
        ),
        Some("DATABASE_URL must be an absolute local file path in production"),
        "the old relative default with spaces must be rejected in production"
    );
    assert_eq!(
        crate::production_database_url_error(
            false,
            common::env::EnvValue::Utf8("file:./data.db".to_string())
        ),
        Some("DATABASE_URL must be an absolute local file path in production"),
        "any relative file path must be rejected in production"
    );
    // R2-DENY-IGNORE: 本番相当ではリモート DSN をゲートで拒否する（fail-closed）。
    // 値やトークンはエコーせず、固定文言のみを返す。
    for remote in [
        "libsql://db.example.invalid:443/main?token=dummy-token",
        "https://db.example.invalid/main",
        "http://db.example.invalid/main",
        "wss://db.example.invalid/main",
        "foo://db.example.invalid/main",
        "  libsql://db.example.invalid:443/main  ",
    ] {
        assert_eq!(
            crate::production_database_url_error(
                false,
                common::env::EnvValue::Utf8(remote.to_string())
            ),
            Some("DATABASE_URL remote DSN is not permitted in production"),
            "remote DSN must be rejected in production: {remote:?}"
        );
    }
    // 絶対パスの file: URL は本番でも受理。前後空白付きも trim 後の判定で受理し、
    // run() 側も同じ trim を適用して接続 URL へ空白が残らないことを検証する。
    for raw in [
        "file:/var/lib/openillustration/local.db",
        "  file:/var/lib/openillustration/local.db  ",
    ] {
        assert_eq!(
            crate::production_database_url_error(
                false,
                common::env::EnvValue::Utf8(raw.to_string())
            ),
            None,
            "absolute path must be accepted: {raw:?}"
        );
        assert_eq!(raw.trim(), "file:/var/lib/openillustration/local.db");
    }
    // 開発（mock 有効）では従来どおり全て受理（未設定・旧既定も許容）。
    for value in [
        common::env::EnvValue::Missing,
        common::env::EnvValue::NonUtf8,
        common::env::EnvValue::Utf8("file:local.db".to_string()),
    ] {
        assert_eq!(crate::production_database_url_error(true, value), None);
    }

    // R1-CF: AUDIT_EXPECT_ROOT を起動ゲートへ昇格。共有ヘルパの判定を直接検証する。
    const AUDIT_ROOT_ERROR: &str =
        "AUDIT_EXPECT_ROOT is required in production (a hex Merkle root)";
    assert_eq!(
        crate::production_audit_expect_root_error(false, common::env::EnvValue::Missing),
        Some(AUDIT_ROOT_ERROR)
    );
    let non_utf8_root = common::env::classify(Err::<String, _>(std::env::VarError::NotUnicode(
        std::ffi::OsString::from_vec(vec![0xff]),
    )));
    assert_eq!(non_utf8_root, common::env::EnvValue::NonUtf8);
    assert_eq!(
        crate::production_audit_expect_root_error(false, non_utf8_root),
        Some(AUDIT_ROOT_ERROR)
    );
    // ADR-0013 P2: 64hex 固定へ厳密化。1文字・奇数長・63桁・65桁・非hex を拒否する。
    let short_63 = "a".repeat(63);
    let long_65 = "a".repeat(65);
    let non_hex_64 = "g".repeat(64);
    for invalid in [
        "",
        " \t ",
        "deadbeefcafebabe-nothex-ZZZZ",
        "  ABCDEF0123456789  ",
        "a",
        "abc",
        short_63.as_str(),
        long_65.as_str(),
        non_hex_64.as_str(),
    ] {
        assert_eq!(
            crate::production_audit_expect_root_error(
                false,
                common::env::EnvValue::Utf8(invalid.to_string())
            ),
            Some(AUDIT_ROOT_ERROR),
            "invalid root must be rejected without echoing: {invalid:?}"
        );
        assert!(!crate::is_hex_merkle_root(invalid));
    }
    // 64桁のみ受理（小文字・大文字・混在を許容）。
    let valid_lower = "ab".repeat(32);
    let valid_upper = "AB".repeat(32);
    let valid_mixed = format!("{}{}", "a".repeat(32), "B".repeat(32));
    for valid in [
        valid_lower.as_str(),
        valid_upper.as_str(),
        valid_mixed.as_str(),
    ] {
        assert!(crate::is_hex_merkle_root(valid));
        assert_eq!(
            crate::production_audit_expect_root_error(
                false,
                common::env::EnvValue::Utf8(valid.to_string())
            ),
            None
        );
    }
    // 開発（mock 有効）では従来どおり全て受理。
    for value in [
        common::env::EnvValue::Missing,
        common::env::EnvValue::NonUtf8,
        common::env::EnvValue::Utf8(String::new()),
    ] {
        assert_eq!(crate::production_audit_expect_root_error(true, value), None);
    }

    // ADR-0013 P2: DB シンボリックパスの境界（本番拒否 / 開発許容 / ロックは実パス共有）。
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let as_str = |path: &std::path::Path| path.to_string_lossy().into_owned();
        let dir = std::env::temp_dir().join(format!(
            "openillustration_p2_dbsymlink_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir for db symlink test");
        let real_db = dir.join("real.db");
        std::fs::write(&real_db, b"db").expect("seed real db");
        let alias_db = dir.join("alias.db");
        symlink(&real_db, &alias_db).expect("create db symlink");

        // 本番相当（mock 無効）では symlink パスを固定文言で拒否（値・パスは非エコー）。
        assert_eq!(
            crate::production_database_symlink_error(false, &as_str(&alias_db)),
            Some(crate::DATABASE_SYMLINK_REJECTED)
        );
        assert!(!crate::DATABASE_SYMLINK_REJECTED.contains("alias"));
        // 通常ファイル・存在しないパスは symlink ではないので受理。
        assert_eq!(
            crate::production_database_symlink_error(false, &as_str(&real_db)),
            None
        );
        assert_eq!(
            crate::production_database_symlink_error(false, &as_str(&dir.join("missing.db"))),
            None
        );
        // 開発（mock 有効）では従来どおり symlink も受理。
        assert_eq!(
            crate::production_database_symlink_error(true, &as_str(&alias_db)),
            None
        );

        // canonicalize 後の実パスからロックを導出するため、別 symlink 経由でも同一ロック。
        let lock_from_alias = crate::database_lock_path(&as_str(&alias_db));
        let lock_from_real = crate::database_lock_path(&as_str(&real_db));
        assert_eq!(lock_from_alias, lock_from_real);
        assert!(lock_from_alias.ends_with("real.db.instance.lock"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // R2-AGE-ORIGINS: ALLOWED_ORIGINS を本番起動ゲートへ昇格。共有ヘルパの直接検証。
    const ORIGINS_ERROR: &str = "ALLOWED_ORIGINS has no valid http(s) origin (unset, empty, or all entries invalid) while mock auth is disabled; all CORS origins are rejected";
    let non_utf8_origins = || {
        common::env::classify(Err::<String, _>(std::env::VarError::NotUnicode(
            std::ffi::OsString::from_vec(vec![0xff]),
        )))
    };
    // 開発（mock 有効）では値の有無を問わず受理（後方互換）。
    for value in [
        common::env::EnvValue::Missing,
        non_utf8_origins(),
        common::env::EnvValue::Utf8(String::new()),
        common::env::EnvValue::Utf8("localhost:8080, null".to_string()),
    ] {
        assert_eq!(crate::production_allowed_origins_error(true, value), None);
    }
    // 本番相当では未設定・非 UTF-8・空・全不正を固定文言で拒否（値・生バイトはエコーしない）。
    for value in [
        common::env::EnvValue::Missing,
        non_utf8_origins(),
        common::env::EnvValue::Utf8(String::new()),
        common::env::EnvValue::Utf8(" , , ".to_string()),
        common::env::EnvValue::Utf8("localhost:8080, null, ftp://x".to_string()),
    ] {
        assert_eq!(
            crate::production_allowed_origins_error(false, value),
            Some(ORIGINS_ERROR)
        );
    }
    // 本番相当は https かつオリジンのみ。http / userinfo / path / query /
    // loopback は client の is_https_non_local_origin と同じ基準で拒否する。
    for invalid in [
        "http://example.jp",
        "https://user@example.jp",
        "https://example.jp/v1",
        "https://example.jp/?key=1",
        "https://example.jp#top",
        "https://127.0.0.1:8443",
        "https://[::1]:8443",
        "https://0.0.0.0",
        "https://LOCALHOST",
        "http://localhost:8080, null",
    ] {
        assert_eq!(
            crate::production_allowed_origins_error(
                false,
                common::env::EnvValue::Utf8(invalid.to_string())
            ),
            Some(ORIGINS_ERROR),
            "invalid={invalid:?}"
        );
    }
    // 有効な https オリジンが1つでもあれば受理（不正要素が混じっても捨てて残す）。
    for valid in [
        "https://example.jp",
        "https://api.example.org:8443",
        "http://localhost:8080, https://example.jp",
        "https://example.jp/, null",
    ] {
        assert_eq!(
            crate::production_allowed_origins_error(
                false,
                common::env::EnvValue::Utf8(valid.to_string())
            ),
            None,
            "valid={valid:?}"
        );
    }
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
    // パース失敗する userinfo 付き URL でも認証情報とクエリを除去する。
    let redacted = crate::redact_url("https://user:pass@exa mple.com/path?token=x");
    assert!(!redacted.contains("user:pass"));
    assert!(!redacted.contains("token=x"));
    assert_eq!(redacted, "https://exa mple.com/path");
    // userinfo 内に '?' / '#' を含むパース失敗 URL でも authority を先に切り出すため、
    // userinfo の一部がクエリ/フラグメントとして残存しない。
    assert_eq!(
        crate::redact_url("https://user:pa?ss@exa mple.com/path"),
        "https://exa mple.com/path"
    );
    assert_eq!(
        crate::redact_url("https://user:p#ss@exa mple.com/path?token=x"),
        "https://exa mple.com/path"
    );
    // パース失敗時も authority 内に '@' が無ければ userinfo 除去を行わず、
    // パス内の '@' は残す。
    assert_eq!(
        crate::redact_url("https://exa mple.com/path@item?token=x"),
        "https://exa mple.com/path@item"
    );
    // file スキームでは set_username/set_password が Err を返すため、
    // 第93ラウンドの fallback 分岐へ入り、userinfo/query が除去される。
    let file_redacted = crate::redact_url("file://user:pass@host/path?token=secret");
    assert!(!file_redacted.contains("user:pass"), "{file_redacted}");
    assert!(!file_redacted.contains("token=secret"), "{file_redacted}");
}

/// ALLOWED_ORIGINS の警告用マスクはオリジン値そのものを漏らさない。
#[test]
fn test_redact_origin_does_not_leak_value() {
    let origin = "https://secret-origin.example.com:8443/path?token=secret";
    let masked = crate::redact_origin(origin);
    assert!(!masked.contains(origin));
    assert!(!masked.contains("secret-origin.example.com"));
    assert!(!masked.contains("8443"));
    assert!(!masked.contains("token=secret"));
    assert_eq!(masked, crate::redact_origin("https://another.invalid"));
}

/// CID は厳密形式のみ受理し、パス注入文字列と旧ダミー CID を拒否する。
#[test]
fn test_cid_validation_rejects_path_characters() {
    // 有効な CIDv0 / CIDv1 は受理する。
    assert!(crate::is_safe_cid(
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5AXQfD"
    ));
    assert!(crate::is_safe_cid(
        "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
    ));
    // 旧ダミー CID は base58 長・文字集合が不正なため拒否する。
    assert!(!crate::is_safe_cid("QmTestCID"));
    assert!(!crate::is_safe_cid("QmValid"));
    assert!(!crate::is_safe_cid("QmPayTest"));
    assert!(!crate::is_safe_cid(
        "QmNoSuchContent000000000000000000000000000000"
    ));
    // `/`, `..`, `?`, `#` などの注入文字列は厳密形式検証に包含されて拒否される。
    assert!(!crate::is_safe_cid("../etc/passwd"));
    assert!(!crate::is_safe_cid("Qm?x=1"));
    assert!(!crate::is_safe_cid("Qm#fragment"));
    assert!(!crate::is_safe_cid("Qm/extra"));
    assert!(!crate::is_safe_cid(""));
    assert!(!crate::is_safe_cid(&"a".repeat(129)));
}

/// 開発用モックの pin_to_ipfs が発行する CID が厳密検証を通過することを担保する。
#[test]
fn test_mock_pin_cid_is_valid_and_unique() {
    let first = crate::mock_cid_for_data(b"hello");
    let second = crate::mock_cid_for_data(b"world");
    assert_eq!(first.len(), 46);
    assert!(
        crate::is_safe_cid(&first),
        "mock CID must be a valid CIDv0: {first}"
    );
    assert!(
        crate::is_safe_cid(&second),
        "mock CID must be a valid CIDv0: {second}"
    );
    assert_ne!(first, second, "different data must produce different CIDs");
    assert_eq!(
        first,
        crate::mock_cid_for_data(b"hello"),
        "same data must produce the same CID"
    );
}

/// PINATA_JWT は起動時に `AppState` へ確定し、リクエスト毎の `read_env` は廃止された。
/// プロセス環境に実トークン風の値が残っていても、`AppState` が None ならモック経路を
/// 通り外部 Pinata API へ送信しないことを検証する（値はエコーしない）。
#[tokio::test]
async fn test_pin_to_ipfs_accepts_whitespace_decorated_mock_jwt() {
    let _env_guard = ENV_LOCK.lock().await;

    // モック判定が壊れて外部へ送ろうとしても、到達不能なローカルプロキシで
    // 即座に失敗させ、テストが外部 Pinata API へ到達しないことを保証する。
    let mut state = (*rate_limit_test_state(1000).await).clone();
    state.http_client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::https("http://127.0.0.1:1").expect("valid proxy URL"))
        .build()
        .expect("build no-network HTTP client");
    // 起動時解決の結果（None = モック経路）だけが参照される。
    state.pinata_jwt = None;

    let data = b"whitespace-decorated-mock-jwt";
    let expected = crate::mock_cid_for_data(data);

    // 既存の完全一致 "mock_jwt" も壊さず、空白・タブ・改行装飾でもモック CID を返す。
    for jwt in ["mock_jwt", " mock_jwt ", "\tmock_jwt\n"] {
        assert_eq!(
            crate::resolve_pinata_jwt(common::env::EnvValue::Utf8(jwt.to_string())),
            None,
            "mock JWT {jwt:?} must not be resolved as a production token"
        );
        test_set_env("PINATA_JWT", jwt);
        let cid = crate::pin_to_ipfs(&state, data.to_vec(), "test.txt")
            .await
            .unwrap_or_else(|e| panic!("mock JWT {jwt:?} must not hit Pinata API: {e}"));
        assert_eq!(cid, expected, "mock JWT {jwt:?} must return the mock CID");
    }

    // 実トークン風の値を環境へ置いても、AppState が None ならモック経路のまま。
    test_set_env("PINATA_JWT", "real-jwt-in-environment");
    let cid = crate::pin_to_ipfs(&state, data.to_vec(), "test.txt")
        .await
        .expect("environment must not override the startup-resolved value");
    assert_eq!(cid, expected);

    test_remove_env("PINATA_JWT");
}

/// Pinata の外部レスポンスが不正な `IpfsHash` を返した場合、保存前に拒否する。
/// mutation で `is_safe_cid` 検証を外すと不正ケースが通って失敗する。
#[test]
fn test_pinata_response_rejects_invalid_ipfs_hash() {
    let valid = crate::mock_cid_for_data(b"valid");
    assert_eq!(
        crate::cid_from_pinata_response(&json!({ "IpfsHash": valid.clone() })).unwrap(),
        valid
    );

    for bad in ["QmTestCID", "../etc/passwd", "Qm?x=1", ""] {
        let err = crate::cid_from_pinata_response(&json!({ "IpfsHash": bad }))
            .expect_err("invalid CID from Pinata must be rejected");
        assert!(
            matches!(&err, AppError::Internal(_)),
            "unexpected error: {err:?}"
        );
    }
    // 欠落・非文字列も Internal（HTTP 側のエラーと同じ扱い）。
    assert!(matches!(
        crate::cid_from_pinata_response(&json!({})),
        Err(AppError::Internal(_))
    ));
    assert!(matches!(
        crate::cid_from_pinata_response(&json!({ "IpfsHash": 42 })),
        Err(AppError::Internal(_))
    ));
}

/// SEC-04: 500系エラーの内部詳細は既定で隠し、注入値が真のときだけ公開する。
/// `EXPOSE_INTERNAL_ERRORS` は起動時に一度だけ解釈して `AppState` へ保持し、
/// リクエストではその注入値だけを参照する（環境変数書き換え・`ENV_LOCK` 依存を廃止）。
#[tokio::test]
async fn test_internal_error_details_visibility() {
    async fn error_body() -> (StatusCode, String) {
        let response = AppError::Internal("no such table: users".to_string()).into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    // 注入スコープ外（ミドルウェア未適用）は安全側の false。
    let (status, text) = error_body().await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        !text.contains("no such table"),
        "details must be hidden: {text}"
    );
    assert!(
        text.contains("Internal server error"),
        "generic message expected: {text}"
    );

    // AppState から注入された値と同じ真偽値で状態を切り替える（環境変数は触らない）。
    for expose in [false, true] {
        let (status, text) = crate::EXPOSE_INTERNAL_ERRORS
            .scope(expose, error_body())
            .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            text.contains("no such table"),
            expose,
            "injected expose={expose} must control visibility: {text}"
        );
        assert_eq!(
            text.contains("Internal server error"),
            !expose,
            "generic message must appear only when masked: {text}"
        );
    }

    // internal_error_body は注入値だけを参照する（per-request の env 読みは存在しない）。
    assert_eq!(
        crate::internal_error_body("injected-detail".to_string(), true),
        "injected-detail"
    );
    assert_eq!(
        crate::internal_error_body("injected-detail".to_string(), false),
        "Internal server error"
    );

    // 起動時検証側の分類: 非 UTF-8 は Invalid で、Valid(true) として扱われない。
    let non_utf8 = common::env::classify(Err::<String, _>(std::env::VarError::NotUnicode(
        std::ffi::OsString::from_vec(vec![0xff]),
    )));
    assert_eq!(non_utf8, common::env::EnvValue::NonUtf8);
    assert_eq!(
        common::env::bool_env_state(non_utf8),
        common::env::BoolEnvState::Invalid
    );
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
        "/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
        "/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g/audit",
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

/// auth_verify 発行セッション（モック経路）にも sid が付与され、logout で即失効すること。
#[tokio::test]
async fn test_auth_verify_issued_session_sid_and_logout_revokes_it() {
    let app = setup_app().await;
    let response = app
        .clone()
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
    let token = body.token.expect("auth_verify must issue a JWT");
    let claims = jsonwebtoken::decode::<Claims>(
        &token,
        &jsonwebtoken::DecodingKey::from_secret(TEST_JWT_SECRET.as_bytes()).unwrap(),
        &jsonwebtoken::Validation::default(),
    )
    .unwrap()
    .claims;
    assert!(
        claims.sid.is_some(),
        "auth_verify-issued JWT must carry a sid so /auth/logout can revoke it"
    );

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
        StatusCode::UNAUTHORIZED,
        "the logged-out auth_verify JWT must be rejected"
    );
}

/// 同じ sid で logout を二重呼び出ししても 401 ではなく 200 を返す（冪等）。
#[tokio::test]
async fn test_logout_is_idempotent_for_revoked_sid() {
    let app = setup_app().await;
    let response = app
        .clone()
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
    let token = body.token.expect("auth_verify must issue a JWT");

    // 2 回目の logout は既に失効済みの sid を送る。AuthExtractor の 401 に
    // ならず、ハンドラの冪等分岐で 200 が返ることを確認する。
    for attempt in 1..=2 {
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
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "logout attempt {attempt} must be idempotent for an already-revoked sid"
        );
    }

    // 二重 logout 後も失効自体は維持されている。
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
        proof_of_possession: None,
        verified_claims: None,
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

// ---- JPKI 所有証明（PoP）のテスト ----
//
// 補足: jpki.rs の `mod tests` 内にある `rsa_signing_key()` / `named_params()` は
// モジュール私的で sister module の `tests` から参照できない。そのため同じ
// 決定的 RNG (`TestRng`) と rcgen による CA 署名 RSA 2048 リーフで
// jpki.rs のフィクスチャを再現する。
// 純粋関数4件に加え、auth_verify のモック分岐を通すハンドラ経由4件を下記に置く。
// 結線はモック経路のみで、実証明書経路 (verify_presentation) は変更しない。

/// CA 署名の RSA 2048 リーフ証明書を作り、`verify_presentation` 経由で
/// リーフの SPKI 公開鍵 DER を取り出す。署名用の秘密鍵も一緒に返す。
fn pop_test_ca_signed_leaf() -> (Vec<u8>, PrivateKey) {
    use rsa::pkcs8::EncodePrivateKey;

    let mut ca_rng = TestRng::new();
    let ca_private = PrivateKey::new(&mut ca_rng, 2048).expect("generate CA RSA 2048 key");
    let ca_pkcs8 = ca_private.to_pkcs8_der().expect("CA PKCS#8 DER");
    let ca_keypair = rcgen::KeyPair::try_from(ca_pkcs8.as_bytes()).expect("CA keypair");
    let mut ca_params =
        rcgen::CertificateParams::new(vec!["pop-ca.jpki.test".to_string()]).expect("CA params");
    let mut ca_dn = rcgen::DistinguishedName::new();
    ca_dn.push(rcgen::DnType::CommonName, "pop-ca");
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = rcgen::CertifiedIssuer::self_signed(ca_params, ca_keypair).expect("self-signed CA");

    let mut leaf_rng = TestRng::new();
    let leaf_private = PrivateKey::new(&mut leaf_rng, 2048).expect("generate leaf RSA 2048 key");
    let leaf_pkcs8 = leaf_private.to_pkcs8_der().expect("leaf PKCS#8 DER");
    let leaf_keypair = rcgen::KeyPair::try_from(leaf_pkcs8.as_bytes()).expect("leaf keypair");
    let mut leaf_params =
        rcgen::CertificateParams::new(vec!["pop-leaf.jpki.test".to_string()]).expect("leaf params");
    let mut leaf_dn = rcgen::DistinguishedName::new();
    leaf_dn.push(rcgen::DnType::CommonName, "pop-leaf");
    leaf_params.distinguished_name = leaf_dn;
    let leaf = leaf_params
        .signed_by(&leaf_keypair, &ca)
        .expect("CA-signed leaf");

    let cert_b64 = base64::engine::general_purpose::STANDARD.encode(leaf.der());
    let public_der = crate::jpki::verify_presentation(&cert_b64, None, false)
        .expect("CA-signed RSA 2048 leaf must be accepted");
    (public_der, leaf_private)
}

/// 秘密鍵で nonce に PKCS#1 v1.5 + SHA-256 署名する（テスト専用）。
fn pop_sign(private_key: &PrivateKey, nonce: &str) -> Vec<u8> {
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};
    let signing_key = SigningKey::<Sha256>::new(private_key.clone());
    signing_key.sign(nonce.as_bytes()).to_bytes().to_vec()
}

/// 正しい nonce + 署名で `Ok(())`。
#[test]
fn test_proof_of_possession_valid_signature() {
    let (public_der, private_key) = pop_test_ca_signed_leaf();
    let nonce = "nonce-r2-jpki-pop-001";
    let signature = pop_sign(&private_key, nonce);
    assert!(crate::jpki::verify_proof_of_possession(&public_der, nonce, &signature).is_ok());
}

/// 改ざん nonce では `Err`（署名は元 nonce のまま）。
#[test]
fn test_proof_of_possession_tampered_nonce_is_rejected() {
    let (public_der, private_key) = pop_test_ca_signed_leaf();
    let nonce = "nonce-r2-jpki-pop-002";
    let signature = pop_sign(&private_key, nonce);
    let err = crate::jpki::verify_proof_of_possession(&public_der, "tampered-nonce", &signature)
        .expect_err("tampered nonce must fail");
    assert!(
        matches!(&err, AppError::Unauthorized(msg) if msg.contains("signature is invalid")),
        "unexpected error: {err:?}"
    );
}

/// 別鍵で署名した値は `Err`。
#[test]
fn test_proof_of_possession_other_key_signature_is_rejected() {
    let (public_der, _leaf_private) = pop_test_ca_signed_leaf();
    let (_other_public_der, other_private) = pop_test_ca_signed_leaf();
    let nonce = "nonce-r2-jpki-pop-003";
    let signature = pop_sign(&other_private, nonce);
    let err = crate::jpki::verify_proof_of_possession(&public_der, nonce, &signature)
        .expect_err("signature from another key must fail");
    assert!(
        matches!(&err, AppError::Unauthorized(_)),
        "unexpected error: {err:?}"
    );
}

/// `ProofOfPossession` は nonce / signature を保持し、未知フィールドを拒否する。
#[test]
fn test_proof_of_possession_envelope_denies_unknown_fields() {
    use crate::jpki::ProofOfPossession;
    let valid = r#"{"nonce":"n","signature":"c2ln"}"#;
    let pop: ProofOfPossession = serde_json::from_str(valid).expect("valid PoP envelope");
    assert_eq!(pop.nonce, "n");
    assert_eq!(pop.signature, "c2ln");

    let unknown = r#"{"nonce":"n","signature":"c2ln","extra":1}"#;
    assert!(
        serde_json::from_str::<ProofOfPossession>(unknown).is_err(),
        "unknown fields must be denied"
    );
}

// ---- auth_verify のモック分岐を通す PoP 回帰テスト（4件・opt-in） ----

/// PoP envelope を組み立てる。署名は base64(STANDARD) で符号化する。
fn pop_envelope(nonce: &str, signature: &[u8]) -> crate::jpki::ProofOfPossession {
    crate::jpki::ProofOfPossession {
        nonce: nonce.to_string(),
        signature: base64::engine::general_purpose::STANDARD.encode(signature),
    }
}

/// ハンドラへ送る JSON を組み立てる。VerifyRequest の PoP フィールドは
/// Serialize 対象外のため、envelope はここで明示的に挿入する。
fn pop_verify_body(
    nonce: &str,
    public_key_der: Option<Vec<u8>>,
    proof_of_possession: Option<crate::jpki::ProofOfPossession>,
) -> String {
    let mut body = json!({
        "presentation_token": "mock_token",
        "nonce": nonce,
        "public_key_der": public_key_der,
        "birth_date": "2000-01-01",
        "country_code": "JP",
    });
    if let Some(pop) = proof_of_possession {
        body["proof_of_possession"] = json!({
            "nonce": pop.nonce,
            "signature": pop.signature,
        });
    }
    body.to_string()
}

/// 正しい nonce に対する署名を添えた PoP は 200。
#[tokio::test]
async fn test_auth_verify_pop_valid_signature_is_accepted() {
    let app = setup_app().await;
    let (public_der, private_key) = pop_test_ca_signed_leaf();
    let nonce = crate::issue_mock_nonce_for_test();
    let signature = pop_sign(&private_key, &nonce);
    let body = pop_verify_body(
        &nonce,
        Some(public_der),
        Some(pop_envelope(&nonce, &signature)),
    );
    assert_eq!(post_verify(&app, body).await.0, StatusCode::OK);
}

/// 署名対象 nonce の改ざん、および pop.nonce の改ざんは 401。
#[tokio::test]
async fn test_auth_verify_pop_tampered_nonce_is_rejected() {
    let app = setup_app().await;
    let (public_der, private_key) = pop_test_ca_signed_leaf();

    // (1) 署名は別文字列に対して作られ、pop.nonce は発行 nonce のまま。
    let nonce = crate::issue_mock_nonce_for_test();
    let signature = pop_sign(&private_key, "tampered-nonce");
    let body = pop_verify_body(
        &nonce,
        Some(public_der.clone()),
        Some(pop_envelope(&nonce, &signature)),
    );
    assert_eq!(post_verify(&app, body).await.0, StatusCode::UNAUTHORIZED);

    // (2) 署名は発行 nonce に対して正しいが、pop.nonce が改ざんされている。
    let nonce = crate::issue_mock_nonce_for_test();
    let signature = pop_sign(&private_key, &nonce);
    let body = pop_verify_body(
        &nonce,
        Some(public_der),
        Some(pop_envelope("tampered-nonce", &signature)),
    );
    assert_eq!(post_verify(&app, body).await.0, StatusCode::UNAUTHORIZED);
}

/// 別の秘密鍵で署名した PoP は 401。
#[tokio::test]
async fn test_auth_verify_pop_other_key_signature_is_rejected() {
    let app = setup_app().await;
    let (public_der, _leaf_private) = pop_test_ca_signed_leaf();
    let (_other_public_der, other_private) = pop_test_ca_signed_leaf();
    let nonce = crate::issue_mock_nonce_for_test();
    let signature = pop_sign(&other_private, &nonce);
    let body = pop_verify_body(
        &nonce,
        Some(public_der),
        Some(pop_envelope(&nonce, &signature)),
    );
    assert_eq!(post_verify(&app, body).await.0, StatusCode::UNAUTHORIZED);
}

/// PoP 省略時は従来どおり受理する（opt-in・後方互換）。
#[tokio::test]
async fn test_auth_verify_pop_is_opt_in() {
    let app = setup_app().await;
    let nonce = crate::issue_mock_nonce_for_test();
    let body = pop_verify_body(&nonce, None, None);
    assert_eq!(post_verify(&app, body).await.0, StatusCode::OK);
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
        "target_cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
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
        .query("SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g'", ())
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
        "target_cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
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
            "SELECT COUNT(*) FROM reports WHERE target_cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g'",
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
        "target_cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111S",
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

    let first = post_report(
        &app,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
        "first report",
    )
    .await;
    assert_eq!(
        first.status(),
        StatusCode::OK,
        "first report must be accepted"
    );

    let second = post_report(
        &app,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
        "second report",
    )
    .await;
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("already under review"),
        "client must receive the duplicate-report rejection: {text}"
    );

    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM reports WHERE target_cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g'",
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
        let response = post_report(
            &app,
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
            reason,
        )
        .await;
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

    let accepted = post_report(
        &app,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
        "  legit concern  ",
    )
    .await;
    assert_eq!(
        accepted.status(),
        StatusCode::OK,
        "valid reason must be accepted"
    );

    let mut rows = db
        .query(
            "SELECT reason FROM reports WHERE target_cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g'",
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
        body.extend_from_slice(&[0u8; 256]);
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
    // ID-06: 短絡 413 も外側ミドルウェアのセキュリティヘッダ/相関IDを維持する。
    assert_security_headers(&response);
    assert!(!response.headers()["x-request-id"].is_empty());
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
    body.extend_from_slice(&[0u8; 256]);
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
    signature_part.extend_from_slice(&[0u8; 256]);
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
        "a mismatched owner must not insert content (only QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g exists)"
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111o', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111o', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
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
        let juror_hash = sha256_hex(&public_der);
        db.execute(
            "INSERT INTO users (user_hash, public_key_der) VALUES (?, ?)",
            (juror_hash.clone(), public_der),
        )
        .await
        .unwrap();

        let message = crate::moderation_vote_message_v2(1, "Reject", &juror_hash);
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
        .query("SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111o'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status, "Reported",
        "no early conclusion before the voting deadline"
    );

    force_conclude(&db, 1).await;

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111o'", ())
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111h', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111h', 'test')",
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
        .query("SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111h'", ())
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111J', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111J', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
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
    let juror_hash = sha256_hex(&public_der);
    db.execute(
        "INSERT INTO users (user_hash, public_key_der) VALUES (?, ?)",
        (juror_hash.clone(), public_der),
    )
    .await
    .unwrap();

    let message = crate::moderation_vote_message_v2(1, "Approve", &juror_hash);
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
    let juror_hash = sha256_hex(&public_der);
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

/// 与えられた署名対象メッセージを RSA 秘密鍵で署名するテストヘルパー。
fn sign_vote_message(message: &str, private_key: &PrivateKey) -> Vec<u8> {
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};

    let signing_key = SigningKey::<Sha256>::new(private_key.clone());
    signing_key.sign(message.as_bytes()).to_vec()
}

/// 任意の署名バイト列で投票リクエストを送信し、ステータスコードを返す。
/// 署名対象メッセージを差し替える再生テストから使う。
async fn send_vote_with_signature(
    app: &Router,
    report_id: i64,
    result: &str,
    juror_hash: &str,
    signature: Vec<u8>,
) -> StatusCode {
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

/// v2 メッセージに正しく署名した投票リクエストを送信する。
async fn send_signed_vote(
    app: &Router,
    report_id: i64,
    result: &str,
    juror_hash: &str,
    private_key: &PrivateKey,
) -> StatusCode {
    let message = crate::moderation_vote_message_v2(report_id, result, juror_hash);
    let signature = sign_vote_message(&message, private_key);
    send_vote_with_signature(app, report_id, result, juror_hash, signature).await
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111E', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111E', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
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
            "SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111E'",
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
            "SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111E'",
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
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let app = build_router(shared_state);

    let (juror_hash, key) = insert_juror(&db).await;
    assert_eq!(
        send_signed_vote(&app, 9999, "Approve", &juror_hash, &key).await,
        StatusCode::NOT_FOUND,
        "unknown report must return 404"
    );
}

/// v2 投票署名メッセージは report_id / result / juror_hash のすべてに依存し、
/// ドメイン分離子を含む（転用防止の基盤）。
#[test]
fn test_moderation_vote_message_v2_binds_all_fields() {
    let a = crate::moderation_vote_message_v2(1, "Approve", "juror_a");
    assert_ne!(
        a,
        crate::moderation_vote_message_v2(2, "Approve", "juror_a")
    );
    assert_ne!(a, crate::moderation_vote_message_v2(1, "Reject", "juror_a"));
    assert_ne!(
        a,
        crate::moderation_vote_message_v2(1, "Approve", "juror_b")
    );
    assert_eq!(
        a,
        crate::moderation_vote_message_v2(1, "Approve", "juror_a")
    );

    let parsed: serde_json::Value = serde_json::from_str(&a).unwrap();
    assert_eq!(parsed["domain"], "moderation-vote-v2");
    assert_eq!(parsed["report_id"], 1);
    assert_eq!(parsed["result"], "Approve");
    assert_eq!(parsed["juror_hash"], "juror_a");

    // 旧 v1 形式とは必ず異なる（アップロード署名等とのクロスプロトコル分離）。
    #[allow(deprecated)]
    let legacy = crate::moderation_vote_message(1, "Approve");
    assert_ne!(a, legacy);
}

/// v2 投票署名テスト用の共通セットアップ（content + report + app）。
async fn setup_vote_signature_test() -> (Router, libsql::Connection) {
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111E', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111E', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let app = build_router(shared_state);
    (app, db)
}

/// v2 メッセージに正しく署名した投票は受理され、票が記録される。
#[tokio::test]
async fn test_vote_v2_signature_accepted() {
    let (app, db) = setup_vote_signature_test().await;
    let (juror_hash, key) = insert_juror(&db).await;

    assert_eq!(
        send_signed_vote(&app, 1, "Approve", &juror_hash, &key).await,
        StatusCode::OK,
        "v2 署名の投票は受理されること"
    );

    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1, "v2 署名の投票が記録されること");
}

/// 旧 v1 形式で署名した投票は 401 で拒否され、票を記録しない。
#[tokio::test]
async fn test_vote_rejects_legacy_v1_signature() {
    let (app, db) = setup_vote_signature_test().await;
    let (juror_hash, key) = insert_juror(&db).await;

    #[allow(deprecated)]
    let legacy_message = crate::moderation_vote_message(1, "Approve");
    let signature = sign_vote_message(&legacy_message, &key);
    assert_eq!(
        send_vote_with_signature(&app, 1, "Approve", &juror_hash, signature).await,
        StatusCode::UNAUTHORIZED,
        "旧形式の署名は拒否されること"
    );

    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 0, "旧形式の署名では票が記録されないこと");
}

/// 別の juror_hash 向けに署名したメッセージは転用できない。
#[tokio::test]
async fn test_vote_rejects_signature_bound_to_other_juror() {
    let (app, db) = setup_vote_signature_test().await;
    let (juror_a, _key_a) = insert_juror(&db).await;
    let (juror_b, key_b) = insert_juror(&db).await;

    // B の秘密鍵で署名するが、署名対象メッセージは A の juror_hash に束縛されている。
    let forged_message = crate::moderation_vote_message_v2(1, "Approve", &juror_a);
    let signature = sign_vote_message(&forged_message, &key_b);
    assert_eq!(
        send_vote_with_signature(&app, 1, "Approve", &juror_b, signature).await,
        StatusCode::UNAUTHORIZED,
        "別 juror_hash に束縛された署名は転用できないこと"
    );

    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 0, "転用署名では票が記録されないこと");
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1118', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1118', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
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

/// ALLOW_MOCK_AUTH=1 でも空署名・不正長の全ゼロ署名はモックとして受理しない。
#[tokio::test]
async fn test_vote_rejects_empty_and_wrong_length_zero_signatures() {
    let (app, db) = setup_mod_app().await;
    let (juror_hash, _key) = insert_juror(&db).await;
    let token = generate_token(TEST_JWT_SECRET, &juror_hash).await;

    for (name, signature, expected) in [
        ("empty", Vec::new(), StatusCode::BAD_REQUEST),
        ("64-byte zero", vec![0u8; 64], StatusCode::UNAUTHORIZED),
        ("66-byte zero", vec![0u8; 66], StatusCode::UNAUTHORIZED),
    ] {
        let vote_body = json!({
            "report_id": 1,
            "juror_hash": juror_hash,
            "result": "Approve",
            "signature": signature,
        });
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::post("/mod/vote")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(vote_body.to_string()))
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "case={name}");
    }

    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        count, 0,
        "malformed signatures must not be recorded as votes"
    );
}

/// モック署名は 256 バイトの全ゼロ列のみ受理し、既存の投票フローを維持する。
#[tokio::test]
async fn test_vote_accepts_exact_mock_signature_length() {
    let (app, db) = setup_mod_app().await;
    let (juror_hash, _key) = insert_juror(&db).await;
    let vote_body = json!({
        "report_id": 1,
        "juror_hash": juror_hash,
        "result": "Approve",
        "signature": vec![0u8; crate::MOCK_SIGNATURE_LEN],
    });

    let response = app
        .oneshot(with_peer(
            Request::post("/mod/vote")
                .header("content-type", "application/json")
                .header(
                    "authorization",
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
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "256-byte zero signature is the only accepted mock signature"
    );

    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1, "mock vote must still be recorded");
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1117', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1117', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111d', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111d', 'test')",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let app = build_router(shared_state);

    let (juror_hash, key) = insert_juror(&db).await;

    // juror_hash 名義で正しく署名し、トークンの sub のみを別の値にする。
    // 投票者のなりすまし（他人の名義での投票）は署名が正しくても拒否される。
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};

    let message = crate::moderation_vote_message_v2(1, "Approve", &juror_hash);
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
            Request::post("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111e/support")
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
            Request::post("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111e/support")
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
/// DB の checksum と一致しない場合、コンテンツを返さず 502 で拒否する。
/// 認証切れの 401 と区別し、本文には固定文言のみを返す。
#[tokio::test]
async fn test_content_checksum_verification_fails_on_tamper() {
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: b"tampered content".to_vec(),
        delay: Duration::ZERO,
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111f".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(1), true).await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111f', 'Novel', 'test_user', 'correct_hash_123', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111f")
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
        StatusCode::BAD_GATEWAY,
        "checksum mismatch must be reported as an upstream failure, not an auth error"
    );
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("Upstream content failed integrity verification"),
        "rejection must come from integrity verification: {body}"
    );
    assert!(
        !body.contains("Integrity check failed"),
        "the old 401 auth error message must not be reused: {body}"
    );
    assert!(
        !body.contains("tampered content"),
        "gateway body must not be returned: {body}"
    );
    assert!(
        !body.contains("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111f"),
        "raw CID must not leak into the error body: {body}"
    );
    assert!(
        !body.contains("correct_hash_123"),
        "raw checksum must not leak into the error body: {body}"
    );
}

/// CONTENT-02: ストリーム途中で切断された応答は不完全なデータとして返さない。
#[tokio::test]
async fn test_get_content_rejects_truncated_stream() {
    let body = b"partial content".to_vec();
    let checksum = sha256_hex(&body);
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111j".to_string()),
        inflated_content_length: Some(body.len() as u64 + 100),
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(1), true).await;
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111j', 'Novel', 'test_user', ?, 'Approved');",
        [checksum.clone()],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111j")
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
    let checksum = sha256_hex(&body);
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111n".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(1), true).await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111n', 'Illustration', 'test_user', ?, 'Approved');",
        [checksum.clone()],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111n")
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
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Y".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(1), true).await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Y', 'Illustration', 'test_user', 'unused', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Y")
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
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111i".to_string()),
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111i', 'Illustration', 'test_user', 'unused', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111i")
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
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111K".to_string()),
        inflated_content_length: None,
    })
    .await;
    let body = b"fallback content".to_vec();
    let checksum = sha256_hex(&body);
    let healthy = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111K".to_string()),
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111K', 'Illustration', 'test_user', ?, 'Approved');",
        [checksum],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111K")
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

/// CONTENT-02: 先頭ゲートウェイがストリーム途中で切断（chunk() エラー）しても、
/// 次のゲートウェイへフェイルオーバーして完全な本文を返す。
#[tokio::test]
async fn test_get_content_chunk_error_falls_back_to_next_gateway() {
    let cid = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111K";
    let truncated = b"partial content from broken gateway".to_vec();
    let failing = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: truncated.clone(),
        delay: Duration::ZERO,
        expected_cid: Some(cid.to_string()),
        // 実際より長い Content-Length を宣言して chunk() をエラーにする。
        inflated_content_length: Some(truncated.len() as u64 + 100),
    })
    .await;
    let body = b"content from healthy gateway".to_vec();
    let checksum = sha256_hex(&body);
    let healthy = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some(cid.to_string()),
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
             VALUES (?, 'Illustration', 'test_user', ?, 'Approved');",
        [cid, checksum.as_str()],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get(format!("/content/{cid}"))
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
    assert!(
        !bytes
            .windows(truncated.len())
            .any(|window| window == truncated.as_slice()),
        "truncated gateway body must not leak: {}",
        String::from_utf8_lossy(&bytes)
    );
}

/// CONTENT-02: 全ゲートウェイがストリーム途中で切断した場合は 500 を返し、
/// 部分データを完全なコンテンツとして返さない（fail-closed）。
#[tokio::test]
async fn test_get_content_all_gateways_chunk_error_returns_error() {
    let cid = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1115";
    let secret = b"secret partial body";
    let first = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: secret.to_vec(),
        delay: Duration::ZERO,
        expected_cid: Some(cid.to_string()),
        inflated_content_length: Some(secret.len() as u64 + 100),
    })
    .await;
    let second = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: secret.to_vec(),
        delay: Duration::ZERO,
        expected_cid: Some(cid.to_string()),
        inflated_content_length: Some(secret.len() as u64 + 100),
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
             VALUES (?, 'Illustration', 'test_user', 'unused', 'Approved');",
        [cid],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get(format!("/content/{cid}"))
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
        !body.contains("secret partial body"),
        "gateway body leaked: {body}"
    );
}

/// SSRF-REDIRECT: ゲートウェイが 302 で内部サービスへリダイレクトしても
/// 追従せず、リダイレクト先へ到達しないことを検証する。
/// リダイレクトを追従すれば checksum が一致して 200 になるよう仕掛ける。
#[tokio::test]
async fn test_get_content_does_not_follow_gateway_redirect() {
    let target_body = b"metadata from internal service".to_vec();
    let scenario = RedirectScenario::start(target_body.clone()).await;
    let checksum = sha256_hex(&target_body);
    let cid = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111K";
    let (app, db) = setup_app_with_options(
        vec![scenario.gateway_template.clone()],
        Duration::from_secs(1),
        true,
    )
    .await;
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES (?, 'Illustration', 'test_user', ?, 'Approved');",
        [cid, checksum.as_str()],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get(format!("/content/{cid}"))
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
        StatusCode::INTERNAL_SERVER_ERROR,
        "3xx must be rejected instead of being followed"
    );
    assert_eq!(
        scenario.target_hits.load(Ordering::SeqCst),
        0,
        "the redirect target must never be reached"
    );
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("metadata from internal service"),
        "redirect target body must not leak: {text}"
    );
}

/// SSRF-REDIRECT: 302 を返すゲートウェイは無視し、次のゲートウェイへ
/// フェイルオーバーする（3xx の Location 先へは接続しない）。
#[tokio::test]
async fn test_get_content_redirect_falls_back_to_next_gateway() {
    let scenario = RedirectScenario::start(b"internal metadata".to_vec()).await;
    let body = b"content from healthy gateway".to_vec();
    let checksum = sha256_hex(&body);
    let healthy = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111f".to_string()),
        inflated_content_length: None,
    })
    .await;
    let cid = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111f";
    let (app, db) = setup_app_with_options(
        vec![scenario.gateway_template.clone(), healthy.template.clone()],
        Duration::from_secs(1),
        true,
    )
    .await;
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES (?, 'Illustration', 'test_user', ?, 'Approved');",
        [cid, checksum.as_str()],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get(format!("/content/{cid}"))
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
    assert_eq!(
        scenario.target_hits.load(Ordering::SeqCst),
        0,
        "failover must not contact the redirect target"
    );
}

/// CONTENT-02: 10 MiB ちょうどの応答は受理する（境界の off-by-one 検出）。
#[tokio::test]
async fn test_get_content_accepts_exact_size_limit() {
    let body = vec![0u8; 10 * 1024 * 1024];
    let checksum = sha256_hex(&body);
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111A".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(5), true).await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111A', 'Illustration', 'test_user', ?, 'Approved');",
        [checksum],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111A")
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111U', 'Illustration', 'test_user', 'unused', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111U")
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
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1115".to_string()),
        inflated_content_length: None,
    })
    .await;
    let second = MockGateway::start(MockResponse {
        status: StatusCode::BAD_GATEWAY,
        body: b"another secret".to_vec(),
        delay: Duration::ZERO,
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1115".to_string()),
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1115', 'Illustration', 'test_user', 'unused', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1115")
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
    let checksum = sha256_hex(&body);
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body: body.clone(),
        delay: Duration::ZERO,
        expected_cid: Some("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111W".to_string()),
        inflated_content_length: None,
    })
    .await;
    let (app, db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(1), true).await;

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111W', 'Novel', 'test_user', ?, 'Approved');",
        [checksum],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111W")
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
        &[0u8; 256],
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

    // public_key_der が NULL のモック作成ユーザーが実署名を提示しても
    // NULL デコードで 500 にならず、既存の 401 経路へ載る。
    let null_key_user = "null_key_user";
    db.execute(
        "INSERT INTO users (user_hash, public_key_der) VALUES (?, NULL)",
        [null_key_user],
    )
    .await
    .unwrap();
    let null_key_token = generate_token(TEST_JWT_SECRET, null_key_user).await;
    let boundary = "auth01-null-public-key";
    let body = multipart_upload_body_full(
        boundary,
        "upload.jxl",
        "image/jxl",
        &[0xFF, 0x0A, 0x01, 0x00],
        None,
        null_key_user,
        &[0x11u8; 256],
    );
    let (status, response_body) = send_upload(&app, &null_key_token, boundary, body).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "NULL public_key_der must yield 401, not 500: {response_body}"
    );
    assert!(
        response_body.contains("Invalid digital signature"),
        "NULL public_key_der must ride the existing 401 path: {response_body}"
    );
}

/// ALLOW_MOCK_AUTH=1 でも空署名・不正長の全ゼロ署名はアップロードで受理しない。
#[tokio::test]
async fn test_upload_rejects_empty_and_wrong_length_zero_signatures() {
    let (app, db) = setup_app_with_db().await;
    // 不正長署名はモックを通らず RSA 検証へ進むため、公開鍵を登録しておく。
    use rsa::pkcs8::DecodePrivateKey;
    let private_key = PrivateKey::from_pkcs8_pem(&test_rsa_private_pem()).unwrap();
    let public_der = private_key
        .to_public_key()
        .to_public_key_der()
        .unwrap()
        .to_vec();
    db.execute(
        "UPDATE users SET public_key_der = ? WHERE user_hash = 'test_user'",
        [public_der],
    )
    .await
    .unwrap();
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    for (len, expected) in [
        (0usize, StatusCode::BAD_REQUEST),
        (64, StatusCode::UNAUTHORIZED),
        (66, StatusCode::UNAUTHORIZED),
    ] {
        let boundary = format!("signature-length-{len}");
        let body = multipart_upload_body_full(
            &boundary,
            "upload.jxl",
            "image/jxl",
            &[0xFF, 0x0A, 0x01, 0x00],
            None,
            "test_user",
            &vec![0u8; len],
        );
        let (status, response_body) = send_upload(&app, &token, &boundary, body).await;
        assert_eq!(status, expected, "signature length {len}: {response_body}");
    }
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1116', 'Illustration', 'user1', 'hash1', 'Approved')",
        (),
    )
    .await
    .unwrap();

    let tx = db.transaction().await.unwrap();

    update_content_status(
        &tx,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1116",
        "Reported",
        "user2",
        "First report",
    )
    .await
    .unwrap();
    update_content_status(
        &tx,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1116",
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
                        "cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z",
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

    let message = crate::payments::wallet_bind_message_v2(&order_id, 137, &wallet, receiving, 1000);
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
                        "cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z",
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
                        "cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z",
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

    let message = crate::payments::wallet_bind_message_v2(&order_id, 137, &wallet, receiving, 1000);
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
                        "cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z",
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

/// 回帰固定: 同一 ppid / cid / 受取先 / 金額の注文は正当に複数作成できる。
///
/// 部分 UNIQUE (ppid, cid, recipient_address, amount_wei) は却下済み。根拠:
/// 同じコンテンツを同じ金額で複数回購入するのは正当なユースケースであり、
/// 注文の同一性は order_id（UUID）で識別される。signature bind も order_id 単位で
/// 行われるため、部分 UNIQUE は 2 回目以降の正当な注文を作成不能にする。
/// 既存の UNIQUE (chain_id, tx_hash, log_index) はオンチェーン送金の二重計上を
/// 防ぐものであり、注文作成回数を制限するものではない。
/// payments.rs は担当外のため、却下根拠はこのテスト側に記録する。
#[tokio::test]
async fn test_duplicate_ppid_cid_recipient_amount_orders_are_allowed() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let (first_id, first_amount) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    let (second_id, second_amount) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    assert_ne!(
        first_id, second_id,
        "each order must have a distinct order_id"
    );
    assert_eq!(first_amount, second_amount);

    let mut rows = db
        .query(
            "SELECT order_id, ppid, cid, recipient_address, amount_wei
               FROM payment_orders WHERE order_id IN (?, ?)
              ORDER BY order_id",
            (first_id.clone(), second_id.clone()),
        )
        .await
        .unwrap();
    let mut stored = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        let order_id: String = row.get(0).unwrap();
        let ppid: String = row.get(1).unwrap();
        let cid: String = row.get(2).unwrap();
        let recipient: String = row.get(3).unwrap();
        let amount: String = row.get(4).unwrap();
        stored.push((order_id, ppid, cid, recipient, amount));
    }
    assert_eq!(stored.len(), 2, "both legitimate orders must be stored");
    let keys: Vec<_> = stored
        .iter()
        .map(|(_, ppid, cid, recipient, amount)| {
            (
                ppid.as_str(),
                cid.as_str(),
                recipient.as_str(),
                amount.as_str(),
            )
        })
        .collect();
    let expected = keys[0];
    assert!(
        keys.iter().all(|key| *key == expected),
        "all four fields must match"
    );
    assert_eq!(keys[0].0, "test_user", "ppid is the token sub");
    assert_eq!(keys[0].1, "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z");
    assert_eq!(keys[0].2, receiving);
    assert_eq!(keys[0].3, first_amount.as_str());
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z', 'Novel', 'test_user', 'abc', 'Approved', 1000)",
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
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let app = build_router(shared_state);
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z', 'Novel', 'test_user', 'abc', 'Approved', 1000)",
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
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let app = build_router(shared_state);
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

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

/// MISMATCH は terminal 状態。再検証で DETECTED / FINALIZED へ進まないことを固定する。
#[tokio::test]
async fn test_payment_verify_mismatch_is_terminal() {
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z', 'Novel', 'test_user', 'abc', 'Approved', 1000)",
        (),
    )
    .await
    .unwrap();
    let verifier = Arc::new(MockVerifier::default());
    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xmismatch-reverify".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei: "1000000000000000000000".to_string(),
    });
    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let app = build_router(shared_state);
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

    // 他ノードの確定済み不一致判定を模して MISMATCH にする。
    db.execute(
        "UPDATE payment_orders SET status = ? WHERE order_id = ?",
        (crate::payments::STATUS_MISMATCH, order_id.clone()),
    )
    .await
    .unwrap();

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert!(
        response.status() == StatusCode::CONFLICT || response.status() == StatusCode::BAD_REQUEST,
        "MISMATCH must not be reverted to DETECTED/FINALIZED"
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
        "MISMATCH must remain terminal after re-verification"
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
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;
    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// テスト用にウォレット所有証明を記録する（verify の必須条件）。
#[allow(clippy::too_many_arguments)]
async fn bind_payment_order(
    app: &Router,
    token: &str,
    order_id: &str,
    wallet_key: &SigningKey,
    chain_id: i64,
    recipient_address: &str,
    amount_jpy: u64,
) {
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let message = crate::payments::wallet_bind_message_v2(
        order_id,
        chain_id,
        &wallet,
        recipient_address,
        amount_jpy,
    );
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
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

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

/// MISMATCH マーキングも CAS 検査対象であり、検証中に status が外部書き換えされた
/// 場合は 409 を返し、書き換え後の値を上書きしない。
#[tokio::test]
async fn test_payment_verify_mismatch_cas_conflict() {
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z', 'Novel', 'test_user', 'abc', 'Approved', 1000)",
        (),
    )
    .await
    .unwrap();
    let verification = PaymentVerification {
        chain_id: 137,
        tx_hash: "0xmismatchrace".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        // 注文と金額が不一致 → MISMATCH マーキング経路へ入る。
        amount_wei: "1".to_string(),
    };
    // RacingVerifier は load_order の後に status を外部書き換えするため、
    // 読み取った古い status を期待する MISMATCH 更新は 0 件になる。
    let verifier = Arc::new(RacingVerifier {
        db: db.clone(),
        verification,
    });

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let app = build_router(shared_state);
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("order status changed during mismatch marking"),
        "client must receive the mismatch CAS conflict: {text}"
    );

    // 外部書き換え値（MISMATCH）がそのまま残り、400 にも化けないことを確認する。
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
        "externally rewritten status must remain untouched"
    );
}

/// 回帰: verifier が返す amount_wei に先頭ゼロが付いても、DB 保存値と数値として
/// 等しければ送金一致として受理される（文字列表記差の誤判定を防ぐ）。
#[tokio::test]
async fn test_payment_verify_accepts_leading_zero_amount_wei() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, _db) = setup_app_with_payments(receiving, verifier.clone()).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, amount_wei) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

    // 同じ値の先頭ゼロ付き表記。文字列比較なら不一致だが数値比較では等価。
    let padded_amount = format!("00{amount_wei}");
    assert_ne!(padded_amount, amount_wei);
    assert_eq!(
        padded_amount.parse::<u128>().unwrap(),
        amount_wei.parse::<u128>().unwrap()
    );
    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xabc".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei: padded_amount,
    });

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(response.status(), StatusCode::OK);
    let order: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        order["status"], "FINALIZED",
        "numerically equal amount must not be marked as MISMATCH"
    );
}

/// 回帰: verifier が返す amount_wei が非数値なら Internal で拒否し、
/// MISMATCH 遷移も行わない（fail-closed）。
#[tokio::test]
async fn test_payment_verify_rejects_non_numeric_amount_wei_as_internal() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier.clone()).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _amount_wei) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xabc".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei: "not-a-number".to_string(),
    });

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(
        response.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "unparsable amount_wei must be handled as Internal (fail-closed)"
    );

    // パース失敗は不一致判定ではないため、注文を MISMATCH に進めない。
    let mut rows = db
        .query(
            "SELECT status FROM payment_orders WHERE order_id = ?",
            [order_id.clone()],
        )
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(status, crate::payments::STATUS_PENDING);
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
    bind_payment_order(&app, &token, &order_a, &wallet_key, 137, receiving, 1000).await;
    let (order_b, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_b, &wallet_key, 137, receiving, 1000).await;

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

/// JPYC: UNIQUE 違反以外の DB エラーは「送金の再利用」として 400 にせず 500 にする。
#[tokio::test]
async fn test_payment_verify_maps_non_unique_db_error_to_internal() {
    // 内部エラーのマスクは AppState の注入値（既定 false）で決まり、
    // 環境変数を触らず並列実行しても競合しない。

    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier.clone()).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, amount_wei) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

    *verifier.verification.lock().unwrap() = Some(PaymentVerification {
        chain_id: 137,
        tx_hash: "0xdef".to_string(),
        log_index: 0,
        block_number: 100,
        confirmations: 256,
        from: wallet.clone(),
        to: receiving.to_string(),
        amount_wei,
    });

    // verify の CAS 更新だけを意図的に失敗させる。紐付け更新より後にトリガを張るため、
    // 準備段階は通常どおり成功する。RAISE(ABORT) は UNIQUE 違反ではない DB エラーとなる。
    db.execute(
        &format!(
            "CREATE TRIGGER fail_verify_update BEFORE UPDATE ON payment_orders
             WHEN NEW.order_id = '{order_id}'
             BEGIN
                 SELECT RAISE(ABORT, 'injected database failure');
             END"
        ),
        (),
    )
    .await
    .unwrap();

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(
        response.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "non-UNIQUE DB errors must not be reported as transaction reuse"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("Internal server error"),
        "internal details must be masked by default: {text}"
    );
    assert!(
        !text.contains("transaction is already used by another order"),
        "must not map non-UNIQUE DB errors to the reuse message: {text}"
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
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

    db.execute(
        "UPDATE payment_orders SET expires_at = ? WHERE order_id = ?",
        ("2000-01-01T00:00:00+00:00".to_string(), order_id.clone()),
    )
    .await
    .unwrap();

    let message = crate::payments::wallet_bind_message_v2(&order_id, 137, &wallet, receiving, 1000);
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

/// 盲署名対策: 注文IDのみを署名対象にした旧形式の署名は受理されない。
#[tokio::test]
async fn test_bind_wallet_rejects_legacy_order_only_signature() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;

    // 旧形式（注文IDのみ）で署名する。金額・送金先・チェーンが確認できないため拒否される。
    let message = format!("openIllustration payment order {order_id}");
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
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // 競合ケース: 他ライターが status を PENDING 以外へ進めた場合は 409 で拒否し、
    // wallet_bound を書き換えない（verify_order と同じ単一ライター契約）。
    db.execute(
        "UPDATE payment_orders SET status = ? WHERE order_id = ?",
        (crate::payments::STATUS_MISMATCH, order_id.clone()),
    )
    .await
    .unwrap();
    let message = crate::payments::wallet_bind_message_v2(&order_id, 137, &wallet, receiving, 1000);
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
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("order status changed during wallet binding"),
        "conflict must use the fixed 409 wording: {text}"
    );
    let mut rows = db
        .query(
            "SELECT wallet_bound FROM payment_orders WHERE order_id = ?",
            [order_id],
        )
        .await
        .unwrap();
    let bound: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(bound, 0, "conflicting bind must not stamp wallet_bound");
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
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

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

/// 決済: 検証 I/O の間に期限を過ぎた注文は FINALIZED へ進まず 400 で拒否される。
/// 読み取った古い status を期待する CAS の前に、tx 内の最新 expires_at で再判定する。
#[tokio::test]
async fn test_payment_verify_rechecks_expiry_after_verification() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(ExpiryAdvancingVerifier {
        db: std::sync::OnceLock::new(),
        verification: PaymentVerification {
            chain_id: 137,
            tx_hash: "0xtoctou".to_string(),
            log_index: 0,
            block_number: 100,
            confirmations: 256,
            from: wallet.clone(),
            to: receiving.to_string(),
            amount_wei: "1000000000000000000000".to_string(),
        },
    });
    let (app, db) = setup_app_with_payments(receiving, verifier.clone()).await;
    verifier
        .db
        .set(Arc::clone(&db))
        .expect("test database must be set once");
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

    let response = verify_payment_order(&app, &token, &order_id).await;
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "expiry during verification must not reach FINALIZED"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("order has expired"),
        "rejection must come from the post-verification expiry recheck"
    );

    let mut rows = db
        .query(
            "SELECT status, tx_hash, confirmations FROM payment_orders WHERE order_id = ?",
            [order_id.clone()],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let status: String = row.get(0).unwrap();
    let tx_hash: Option<String> = row.get(1).unwrap();
    let confirmations: i64 = row.get(2).unwrap();
    assert_eq!(status, crate::payments::STATUS_EXPIRED);
    assert!(
        tx_hash.is_none() && confirmations == 0,
        "the expired order must not receive the verified transfer"
    );
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
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

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

/// テスト用: 外部ライターが write_lock と BEGIN を保持した状態を作る。
/// `started` を受け取った時点でロックは保持されており、`release` を送ると
/// COMMIT してロックを解放する。tx 外書き込みの mutation を検出するために使う。
fn spawn_lock_holding_writer(
    db: Arc<libsql::Connection>,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        // 本番の書き込みと同じ順序: write_lock を取ってから BEGIN する。
        let guard = crate::write_lock(&db).await.unwrap().lock_owned().await;
        let tx = db.transaction().await.unwrap();
        tx.execute("INSERT INTO users (user_hash) VALUES ('lock_holder')", ())
            .await
            .unwrap();
        started_tx.send(()).unwrap();
        release_rx.await.unwrap();
        tx.commit().await.unwrap();
        drop(guard);
    });
    (started_rx, release_tx, writer)
}

/// テスト用: tracing の event message を収集する Layer。
/// 周期 tick がロック待ちをスキップしたときの warn を検出するために使う。
#[derive(Clone, Default)]
struct LogMessages(Arc<std::sync::Mutex<Vec<String>>>);

impl LogMessages {
    fn contains(&self, needle: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .iter()
            .any(|message| message.contains(needle))
    }

    /// 起動時/周期の同名ログを区別するため、完全一致でも探せるようにする。
    fn contains_exact(&self, message: &str) -> bool {
        self.0.lock().unwrap().iter().any(|entry| entry == message)
    }
}

struct MessageVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            use std::fmt::Write as _;
            let _ = write!(self.0, "{value:?}");
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogMessages {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut message = String::new();
        event.record(&mut MessageVisitor(&mut message));
        self.0.lock().unwrap().push(message);
    }
}

/// テスト用: write_lock と BEGIN を保持したまま期限切れ通報を作るライター。
/// 周期 tick の finalize が write_lock 待ちになる状況を作るために使う。
fn spawn_report_lock_holding_writer(
    db: Arc<libsql::Connection>,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        let guard = crate::write_lock(&db).await.unwrap().lock_owned().await;
        let tx = db.transaction().await.unwrap();
        tx.execute(
            "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'Novel', 'owner', 'abc', 'Reported')",
            (),
        )
        .await
        .unwrap();
        tx.execute(
            "INSERT INTO reports (report_id, target_cid, reason, deadline_at)
             VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'due', datetime('now', '-1 day'))",
            (),
        )
        .await
        .unwrap();
        started_tx.send(()).unwrap();
        release_rx.await.unwrap();
        tx.commit().await.unwrap();
        drop(guard);
    });
    (started_rx, release_tx, writer)
}

/// 期限切れ注文への bind は write_lock 経由で EXPIRED 更新し、競合下でも 500 にならない。
/// mutation で EXPIRED 更新を tx 外の `db.execute` に戻すと、ロック保持中に
/// リクエストが完了してしまうため失敗する。
#[tokio::test]
async fn test_expired_bind_waits_for_write_transaction() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;

    // status は PENDING のまま期限だけを過去にする。
    db.execute(
        "UPDATE payment_orders SET expires_at = ? WHERE order_id = ?",
        ("2000-01-01T00:00:00+00:00".to_string(), order_id.clone()),
    )
    .await
    .unwrap();

    // 実フローに合わせて有効な v2 署名を作る（期限チェックは署名検証より前）。
    let message = crate::payments::wallet_bind_message_v2(&order_id, 137, &wallet, receiving, 1000);
    let mut hasher = Keccak256::new();
    hasher.update(format!("\x19Ethereum Signed Message:\n{}", message.len()).as_bytes());
    hasher.update(message.as_bytes());
    let digest = hasher.finalize();
    let (signature, recovery_id) = wallet_key.sign_prehash_recoverable(&digest);
    let mut sig_bytes = signature.to_bytes().to_vec();
    sig_bytes.push(recovery_id.to_byte());
    let signature = format!("0x{}", hex::encode(sig_bytes));

    let (started, release, writer) = spawn_lock_holding_writer(Arc::clone(&db));
    started.await.unwrap();

    let app_for_task = app.clone();
    let token_for_task = token.clone();
    let order_id_for_task = order_id.clone();
    let mut bind_task = tokio::spawn(async move {
        app_for_task
            .oneshot(with_peer(
                Request::post(format!("/payments/orders/{order_id_for_task}/bind"))
                    .header("authorization", format!("Bearer {token_for_task}"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "signature": signature }).to_string()))
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap()
    });
    tokio::task::yield_now().await;

    // tx 外の生 execute なら write_lock をバイパスしてここで完了してしまう。
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut bind_task)
            .await
            .is_err(),
        "期限切れ bind は書き込みトランザクションの完了を待つべき"
    );

    release.send(()).unwrap();
    writer.await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(5), &mut bind_task)
        .await
        .expect("bind must finish after the write transaction commits")
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("order has expired"),
        "rejection must come from elapsed expiry: {text}"
    );

    // EXPIRED 更新がコミットされていること。
    let mut rows = db
        .query(
            "SELECT status FROM payment_orders WHERE order_id = ?",
            [order_id],
        )
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(status, crate::payments::STATUS_EXPIRED);
}

/// 期限切れ注文への verify も write_lock 経由で EXPIRED 更新し、競合下で 500 にならない。
#[tokio::test]
async fn test_expired_verify_waits_for_write_transaction() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let wallet_key = SigningKey::from_slice(&[0x11u8; 32]).expect("valid k256 test scalar");
    let wallet = crate::payments::address_from_verifying_key(wallet_key.verifying_key());
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let (order_id, _) = create_payment_order(&app, &token, &wallet, 137, 1000).await;
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

    db.execute(
        "UPDATE payment_orders SET expires_at = ? WHERE order_id = ?",
        ("2000-01-01T00:00:00+00:00".to_string(), order_id.clone()),
    )
    .await
    .unwrap();

    let (started, release, writer) = spawn_lock_holding_writer(Arc::clone(&db));
    started.await.unwrap();

    let app_for_task = app.clone();
    let token_for_task = token.clone();
    let order_id_for_task = order_id.clone();
    let mut verify_task = tokio::spawn(async move {
        verify_payment_order(&app_for_task, &token_for_task, &order_id_for_task).await
    });
    tokio::task::yield_now().await;

    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut verify_task)
            .await
            .is_err(),
        "期限切れ verify は書き込みトランザクションの完了を待つべき"
    );

    release.send(()).unwrap();
    writer.await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(5), &mut verify_task)
        .await
        .expect("verify must finish after the write transaction commits")
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("order has expired"),
        "rejection must come from elapsed expiry: {text}"
    );

    let mut rows = db
        .query(
            "SELECT status FROM payment_orders WHERE order_id = ?",
            [order_id],
        )
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(status, crate::payments::STATUS_EXPIRED);
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
    bind_payment_order(&app, &token, &order_id, &wallet_key, 137, receiving, 1000).await;

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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // R2-SESSION-COOKIE: session Cookie は発行されず、SET_COOKIE は refresh と
    // state 削除の2本だけ。prefix で選んで属性を検証する。
    let cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_string)
        .collect();
    assert_eq!(
        cookies.len(),
        2,
        "callback must set exactly refresh + state deletion cookies: {cookies:?}"
    );
    let refresh = cookies
        .iter()
        .find(|cookie| cookie.starts_with("refresh_token="))
        .expect("callback must set a refresh_token cookie");
    assert!(refresh.contains("HttpOnly"), "{refresh}");
    assert!(refresh.contains("SameSite=Strict"), "{refresh}");
    assert!(refresh.contains("Path=/auth"), "{refresh}");
    let state_deletion = cookies
        .iter()
        .find(|cookie| cookie.starts_with("oidc_state=;"))
        .expect("callback must clear the oidc_state cookie");
    assert!(state_deletion.contains("HttpOnly"), "{state_deletion}");
    assert!(state_deletion.contains("SameSite=Lax"), "{state_deletion}");
    assert!(state_deletion.contains("Max-Age=0"), "{state_deletion}");
    assert!(
        cookies.iter().all(|cookie| !cookie.starts_with("session=")),
        "session cookie must not be issued: {cookies:?}"
    );

    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["authorized"], true);
    assert_eq!(body["user_hash"], "test-ppid");
    assert!(body["token"].as_str().unwrap().len() > 20);
}

/// ログイン CSRF 対策: start が JSON の state と同じ値を oidc_state Cookie に
/// HttpOnly / Path=/auth / SameSite=Lax / Max-Age=600 で格納すること。
#[tokio::test]
async fn test_oidc_start_sets_state_cookie() {
    let (app, _oidc, _mock, _db) = setup_app_with_oidc().await;

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
    let cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_string)
        .collect();
    let state_cookie = cookies
        .iter()
        .find(|cookie| cookie.starts_with("oidc_state="))
        .expect("start must set an oidc_state cookie");

    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let state = started["state"].as_str().expect("start JSON keeps state");
    assert!(
        state_cookie.starts_with(&format!("oidc_state={state};")),
        "cookie value must equal the JSON state: {state_cookie}"
    );
    assert!(state_cookie.contains("HttpOnly"), "{state_cookie}");
    assert!(state_cookie.contains("Path=/auth"), "{state_cookie}");
    assert!(state_cookie.contains("SameSite=Lax"), "{state_cookie}");
    assert!(state_cookie.contains("Max-Age=600"), "{state_cookie}");
    // mock（cookie_secure=false）では `__Host-` を付けず既存名を維持する。
    // `__Host-` は Secure が必須なので、付けると Cookie が保存されない。
    assert!(
        !cookies
            .iter()
            .any(|cookie| cookie.starts_with("__Host-oidc_state="))
    );
    // テスト構成は cookie_secure=false なので Secure は付かない。
    assert!(!state_cookie.contains("; Secure"), "{state_cookie}");
}

/// 本番相当（cookie_secure=true）では `__Host-oidc_state` / Path=/ / Secure を使い、
/// start→callback が成立すること。削除 Cookie も同名・同 Path になること。
#[tokio::test]
async fn test_oidc_state_cookie_secure_follows_config() {
    let policy = common::age::AgePolicy::default();
    let (app, oidc, mock, _db) =
        setup_app_with_oidc_age_policy_and_cookie_secure(policy, true).await;

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
    let cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_string)
        .collect();
    let state_cookie = cookies
        .iter()
        .find(|cookie| cookie.starts_with("__Host-oidc_state="))
        .expect("secure start must set an __Host-oidc_state cookie")
        .to_string();
    assert!(
        !cookies
            .iter()
            .any(|cookie| cookie.starts_with("oidc_state=")),
        "secure 時は旧名の Cookie を出さない: {cookies:?}"
    );
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let auth_state = started["state"].as_str().expect("start JSON keeps state");
    assert!(
        state_cookie.starts_with(&format!("__Host-oidc_state={auth_state};")),
        "cookie value must equal the JSON state: {state_cookie}"
    );
    assert!(state_cookie.contains("; Secure"), "{state_cookie}");
    assert!(state_cookie.contains("HttpOnly"), "{state_cookie}");
    assert!(state_cookie.contains("Path=/;"), "{state_cookie}");
    assert!(!state_cookie.contains("Path=/auth"), "{state_cookie}");
    assert!(
        !state_cookie.to_ascii_lowercase().contains("domain="),
        "{state_cookie}"
    );

    let nonce = oidc.pending_nonce(auth_state).expect("pending nonce");
    *mock.nonce.lock().unwrap() = Some(nonce);

    // `__Host-oidc_state` を送り返すと callback が成立する（start と同名）。
    let response = app
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .header(header::COOKIE, format!("__Host-oidc_state={auth_state}"))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_string)
        .collect();
    let deletion = cookies
        .iter()
        .find(|cookie| cookie.starts_with("__Host-oidc_state="))
        .expect("callback must clear the state cookie")
        .to_string();
    assert!(deletion.starts_with("__Host-oidc_state=;"), "{deletion}");
    assert!(deletion.contains("Max-Age=0"), "{deletion}");
    assert!(deletion.contains("; Secure"), "{deletion}");
    assert!(deletion.contains("Path=/;"), "{deletion}");
    assert!(!deletion.contains("Path=/auth"), "{deletion}");

    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["authorized"], true);
}

/// state のすり替えを防ぐ double-submit 検証。
/// Cookie 欠落 / 不一致は 401（値をエコーしない）、一致時は従来どおり成功し、
/// 検証後に state Cookie を Max-Age=0 で削除する。
#[tokio::test]
async fn test_oidc_callback_binds_state_to_cookie() {
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
    let auth_state = started["state"].as_str().unwrap().to_string();
    let nonce = oidc.pending_nonce(&auth_state).expect("pending nonce");
    *mock.nonce.lock().unwrap() = Some(nonce);

    let uri = format!("/auth/oidc/callback?code=test-code&state={auth_state}");

    // Cookie 欠落は 401。固定文言のみで state 値はエコーしない。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get(uri.as_str()).body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("OIDC state mismatch"), "{text}");
    assert!(
        !text.contains(auth_state.as_str()),
        "state 値をエコーしない: {text}"
    );

    // 不一致 Cookie も 401。エラー文言は同じで、値はエコーしない。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get(uri.as_str())
                .header(header::COOKIE, "oidc_state=attacker-controlled-state")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("OIDC state mismatch"), "{text}");
    assert!(
        !text.contains("attacker-controlled-state"),
        "Cookie 値をエコーしない: {text}"
    );

    // 検証前に拒否しているので pending はまだ消費されていない。
    assert!(oidc.pending_nonce(&auth_state).is_some());

    // 一致 Cookie は従来どおり成功し、state Cookie が削除される。
    let response = app
        .oneshot(with_peer(
            Request::get(uri.as_str())
                .header(header::COOKIE, format!("oidc_state={auth_state}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_string)
        .collect();
    let deletion = cookies
        .iter()
        .find(|cookie| cookie.starts_with("oidc_state="))
        .expect("callback must clear the state cookie");
    assert!(deletion.starts_with("oidc_state=;"), "{deletion}");
    assert!(deletion.contains("Max-Age=0"), "{deletion}");
    assert!(deletion.contains("HttpOnly"), "{deletion}");
    assert!(deletion.contains("Path=/auth"), "{deletion}");
    assert!(deletion.contains("SameSite=Lax"), "{deletion}");

    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["authorized"], true);
    assert!(oidc.pending_nonce(&auth_state).is_none());
}

/// デコード不能なバイト列を含む state Cookie は fail-closed で 401。
/// 不正バイトはエコーせず固定文言 "OIDC state mismatch" のみを返す。
#[tokio::test]
async fn test_oidc_callback_rejects_undecodable_state_cookie() {
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
    let auth_state = started["state"].as_str().unwrap().to_string();
    let nonce = oidc.pending_nonce(&auth_state).expect("pending nonce");
    *mock.nonce.lock().unwrap() = Some(nonce);

    let uri = format!("/auth/oidc/callback?code=test-code&state={auth_state}");

    // \xff は HeaderValue としては有効な obs-text だが UTF-8 としてはデコード不能。
    // state_from_cookie の to_str 失敗経路で検証失敗扱いになることを確認する。
    let bad_cookie = HeaderValue::from_bytes(b"oidc_state=\xff").expect("raw cookie header");
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get(uri.as_str())
                .header(header::COOKIE, bad_cookie)
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("OIDC state mismatch"), "{text}");
    assert!(
        !text.contains(auth_state.as_str()),
        "state 値をエコーしない: {text}"
    );

    // 検証前に拒否しているので pending は消費されず、正規フローは成立する。
    assert!(oidc.pending_nonce(&auth_state).is_some());
    let response = app
        .oneshot(with_peer(
            Request::get(uri.as_str())
                .header(header::COOKIE, format!("oidc_state={auth_state}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// state Cookie が同一名で重複した場合は攻撃の兆候として 401 にする。
/// 攻撃者値が先に来て query.state と一致しても受け付けない（旧 find_map の先勝ち回帰）。
#[tokio::test]
async fn test_oidc_callback_rejects_duplicate_state_cookies() {
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
    let auth_state = started["state"].as_str().unwrap().to_string();
    let nonce = oidc.pending_nonce(&auth_state).expect("pending nonce");
    *mock.nonce.lock().unwrap() = Some(nonce);

    const ATTACKER: &str = "attacker-controlled-state";
    let uri = format!("/auth/oidc/callback?code=test-code&state={ATTACKER}");

    // 単一ヘッダ内で同一名が重複（攻撃者値が先）。旧 find_map なら先勝ちで
    // query.state と一致して検証を通ってしまうため、401 になることを検証する。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get(uri.as_str())
                .header(
                    header::COOKIE,
                    format!("oidc_state={ATTACKER}; oidc_state={auth_state}"),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("OIDC state mismatch"), "{text}");
    assert!(!text.contains(ATTACKER), "state 値をエコーしない: {text}");
    assert!(
        !text.contains(auth_state.as_str()),
        "state 値をエコーしない: {text}"
    );

    // 別々の Cookie ヘッダに分かれていても重複なら拒否する。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get(uri.as_str())
                .header(header::COOKIE, format!("oidc_state={ATTACKER}"))
                .header(header::COOKIE, format!("oidc_state={auth_state}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // 検証前に拒否しているので pending は消費されず、正規フローは成立する。
    assert!(oidc.pending_nonce(&auth_state).is_some());
    let response = app
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(oidc.pending_nonce(&auth_state).is_none());
}

/// BAN 済み subject では OIDC callback がセッションを発行しない。
/// `refresh` と同じく `ensure_user_not_banned` の固定文言 ("User is banned") で
/// 403 を返し、INSERT / JWT 発行前に拒否されることを検証する。
#[tokio::test]
async fn test_oidc_callback_rejects_banned_user() {
    for (kind, ban_until) in [("perm", "NULL"), ("timed", "datetime('now', '+1 day')")] {
        let (app, oidc, mock, db) = setup_app_with_oidc().await;

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

        // mock_token が発行する ID トークンの sub は "test-ppid"。
        db.execute(
            &format!(
                "INSERT INTO users (user_hash, is_banned, ban_until) \
                 VALUES ('test-ppid', TRUE, {ban_until})"
            ),
            (),
        )
        .await
        .unwrap();

        let response = app
            .clone()
            .oneshot(with_peer(
                Request::get(format!(
                    "/auth/oidc/callback?code=test-code&state={auth_state}"
                ))
                .header(header::COOKIE, format!("oidc_state={auth_state}"))
                .body(Body::empty())
                .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{kind}: BAN 済み subject は 403 で拒否されること"
        );

        // セッション JWT / refresh token の Cookie が付いてはならない。
        let cookies: Vec<String> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(str::to_string)
            .collect();
        assert!(
            cookies.iter().all(|cookie| !cookie.contains("session=")),
            "{kind}: session JWT が発行されてはならない: {cookies:?}"
        );
        assert!(
            cookies
                .iter()
                .all(|cookie| !cookie.contains("refresh_token=")),
            "{kind}: refresh token が発行されてはならない: {cookies:?}"
        );

        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(
            body["error"].as_str(),
            Some("User is banned"),
            "{kind}: BAN 理由は refresh と同じ固定文言であること"
        );

        // トークン発行前に拒否されているため refresh_tokens は空のまま。
        let mut rows = db
            .query("SELECT COUNT(*) FROM refresh_tokens", ())
            .await
            .unwrap();
        let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(count, 0, "{kind}: refresh token が保存されてはならない");
    }
}

/// R1-23: pending は上限まで登録でき、上限超過は 429 で拒否される。
#[tokio::test]
async fn test_oidc_pending_limit_rejects_excess_registration() {
    let (_app, oidc, _mock, _db) = setup_app_with_oidc().await;
    let max = crate::oidc::MAX_PENDING_AUTH;

    for i in 0..max {
        let state = format!("pending-{i}");
        oidc.register_pending(state.clone(), crate::oidc::OidcState::test_pending(&state))
            .expect("registration up to the limit must succeed");
    }
    assert_eq!(oidc.pending_count(), max);

    let error = oidc
        .register_pending(
            "overflow".to_string(),
            crate::oidc::OidcState::test_pending("overflow"),
        )
        .expect_err("registration beyond the limit must fail");
    assert!(
        matches!(error, AppError::TooManyRequests(_)),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        oidc.pending_count(),
        max,
        "rejected registration must not be stored"
    );
}

/// R1-23: 上限に達した状態で `/auth/oidc/start` が 429 を返す。
#[tokio::test]
async fn test_oidc_start_returns_429_when_pending_full() {
    let (app, oidc, _mock, _db) = setup_app_with_oidc().await;
    let max = crate::oidc::MAX_PENDING_AUTH;
    for i in 0..max {
        let state = format!("pending-{i}");
        oidc.register_pending(state.clone(), crate::oidc::OidcState::test_pending(&state))
            .expect("registration up to the limit must succeed");
    }

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
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("too many pending OIDC authorizations")
    );
    assert_eq!(
        oidc.pending_count(),
        max,
        "a rejected start must not add another pending entry"
    );
}

/// R1-23: 期限切れエントリは登録時に掃除され、上限に空きができる。
#[tokio::test]
async fn test_oidc_pending_limit_purges_expired_entries() {
    let (_app, oidc, _mock, _db) = setup_app_with_oidc().await;
    let max = crate::oidc::MAX_PENDING_AUTH;
    for i in 0..max {
        let state = format!("pending-{i}");
        oidc.register_pending(state.clone(), crate::oidc::OidcState::test_pending(&state))
            .expect("registration up to the limit must succeed");
    }

    let expired = "pending-0";
    oidc.expire_pending(expired);
    assert!(
        oidc.pending_nonce(expired).is_some(),
        "the helper only rewinds created_at"
    );

    oidc.register_pending(
        "fresh".to_string(),
        crate::oidc::OidcState::test_pending("fresh"),
    )
    .expect("an expired entry must free capacity");
    assert_eq!(oidc.pending_count(), max);
    assert!(
        oidc.pending_nonce(expired).is_none(),
        "expired entry must be purged"
    );
    assert_eq!(oidc.pending_nonce("fresh").as_deref(), Some("fresh"));
}

/// 起動時/周期 sweep: 期限切れ pending を件数付きで回収し、map を空にする。
#[tokio::test]
async fn test_oidc_sweep_expired_pending_empties_store() {
    let (_app, oidc, _mock, _db) = setup_app_with_oidc().await;
    let states: Vec<String> = (0..3).map(|i| format!("stale-{i}")).collect();
    // register_pending 自体が期限切れを掃除するため、全件登録してから期限切れにする。
    for state in &states {
        oidc.register_pending(state.clone(), crate::oidc::OidcState::test_pending(state))
            .expect("registration must succeed");
    }
    for state in &states {
        oidc.expire_pending(state);
    }
    assert_eq!(oidc.pending_count(), states.len());

    assert_eq!(oidc.sweep_expired_pending(), states.len());
    assert_eq!(oidc.pending_count(), 0);
    for state in &states {
        assert!(oidc.pending_nonce(state).is_none());
    }
    assert_eq!(oidc.sweep_expired_pending(), 0, "sweep must be idempotent");

    // 回収後は新規登録が可能で、フロー用の pending として残る。
    oidc.register_pending(
        "after-sweep".to_string(),
        crate::oidc::OidcState::test_pending("after-sweep"),
    )
    .expect("registration after sweep must succeed");
    assert_eq!(oidc.pending_count(), 1);
    assert_eq!(
        oidc.pending_nonce("after-sweep").as_deref(),
        Some("after-sweep")
    );
}

/// sweep は期限切れだけを回収し、生きている pending を使う OIDC フローは壊さない。
#[tokio::test]
async fn test_oidc_sweep_keeps_start_callback_flow() {
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
    let auth_state = started["state"].as_str().unwrap().to_string();

    // register_pending は登録時に期限切れを掃除するため、start 後に期限切れを仕込む。
    oidc.register_pending(
        "stale".to_string(),
        crate::oidc::OidcState::test_pending("stale"),
    )
    .expect("registration must succeed");
    oidc.expire_pending("stale");
    assert_eq!(oidc.pending_count(), 2);

    assert_eq!(oidc.sweep_expired_pending(), 1);
    assert_eq!(oidc.pending_count(), 1);
    let nonce = oidc
        .pending_nonce(&auth_state)
        .expect("sweep must keep fresh pending");
    *mock.nonce.lock().unwrap() = Some(nonce);

    let response = app
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(oidc.pending_nonce(&auth_state).is_none());
    assert_eq!(oidc.pending_count(), 0);
}

/// 起動時 purge 群に OIDC pending sweep が組み込まれていることを検証する。
/// ここを外すと期限切れ pending が register/take まで残るため timeout で失敗する。
#[tokio::test]
async fn test_moderation_finalizer_sweeps_expired_oidc_pending_on_startup() {
    let (_app, oidc, _mock, db) = setup_app_with_oidc().await;
    oidc.register_pending(
        "stale-startup".to_string(),
        crate::oidc::OidcState::test_pending("stale-startup"),
    )
    .expect("registration must succeed");
    oidc.register_pending(
        "fresh-startup".to_string(),
        crate::oidc::OidcState::test_pending("fresh-startup"),
    )
    .expect("registration must succeed");
    // 両方登録してから期限切れにする（登録時に purge が走るため）。
    oidc.expire_pending("stale-startup");
    assert_eq!(oidc.pending_count(), 2);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let finalizer = tokio::spawn(crate::run_moderation_finalizer(
        db,
        Some(oidc.clone()),
        Duration::from_secs(3600),
        shutdown_rx,
        Arc::new(crate::Metrics::default()),
        None,
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        while oidc.pending_nonce("stale-startup").is_some() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("startup purge must sweep expired OIDC pending");
    assert_eq!(
        oidc.pending_count(),
        1,
        "fresh pending must survive startup purge"
    );
    assert_eq!(
        oidc.pending_nonce("fresh-startup").as_deref(),
        Some("fresh-startup")
    );

    shutdown_tx.send(()).expect("shutdown signal");
    finalizer.await.expect("finalizer task must stop cleanly");
}

/// finalizer の tick / 処理件数 / 最終成功時刻カウンタが実際に進むことを検証する。
#[tokio::test]
async fn test_finalizer_metrics_increment() {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    // 起動時回収で 1 件処理されるよう期限切れトークンを置く。
    db.execute(
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
             VALUES ('expired-metric', 'u', 'f', '2000-01-01T00:00:00+00:00')",
        (),
    )
    .await
    .unwrap();

    let metrics = Arc::new(crate::Metrics::default());
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    // R2-SERVER: 周期 tick のレート制限キー回収をこのテストで固定する。
    let rate_limit = Arc::new(crate::RateLimitState {
        limiter: governor::RateLimiter::keyed(governor::Quota::per_minute(
            std::num::NonZeroU32::new(60).unwrap(),
        )),
        concurrency: Arc::new(tokio::sync::Semaphore::new(1)),
    });
    let finalizer = tokio::spawn(crate::run_moderation_finalizer(
        db,
        None,
        Duration::from_millis(20),
        shutdown_rx,
        Arc::clone(&metrics),
        Some(rate_limit),
    ));

    tokio::time::timeout(Duration::from_secs(5), async {
        while metrics.finalizer_ticks.load(Ordering::Relaxed) == 0
            || metrics.finalizer_processed.load(Ordering::Relaxed) == 0
            || metrics.rate_limiter_sweeps.load(Ordering::Relaxed) == 0
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("finalizer counters must advance");

    shutdown_tx.send(()).expect("shutdown signal");
    finalizer.await.expect("finalizer task must stop cleanly");

    assert!(metrics.finalizer_ticks.load(Ordering::Relaxed) >= 1);
    assert!(metrics.finalizer_processed.load(Ordering::Relaxed) >= 1);
    assert!(metrics.finalizer_last_success_secs.load(Ordering::Relaxed) > 0);
    // キー回収 sweep が回ったことと、キー数ゲージが更新されたことを固定する。
    assert!(metrics.rate_limiter_sweeps.load(Ordering::Relaxed) >= 1);
}

/// R1-23: 上限近くまで埋まっていても OIDC start/callback フローは成立する。
#[tokio::test]
async fn test_oidc_pending_limit_keeps_start_callback_flow() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    let max = crate::oidc::MAX_PENDING_AUTH;
    for i in 0..max - 1 {
        let state = format!("pending-{i}");
        oidc.register_pending(state.clone(), crate::oidc::OidcState::test_pending(&state))
            .expect("registration up to the limit must succeed");
    }

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
    assert_eq!(oidc.pending_count(), max);

    let nonce = oidc.pending_nonce(&auth_state).expect("pending nonce");
    *mock.nonce.lock().unwrap() = Some(nonce);
    let response = app
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={auth_state}"
            ))
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        oidc.pending_nonce(&auth_state).is_none(),
        "callback must consume the pending entry"
    );
    assert_eq!(oidc.pending_count(), max - 1);
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
    let state_cookie = format!("oidc_state={auth_state}");
    let first = app
        .clone()
        .oneshot(with_peer(
            Request::get(uri.as_str())
                .header(header::COOKIE, state_cookie.as_str())
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .oneshot(with_peer(
            Request::get(uri.as_str())
                .header(header::COOKIE, state_cookie.as_str())
                .body(Body::empty())
                .unwrap(),
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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
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

/// R1-19 / ADR-0002: 未来の nbf を持つ ID トークンは 401 で拒否される。
/// clock skew (60 秒) を十分に超える値を設定し、validate_nbf が効くことを確認する。
#[tokio::test]
async fn test_oidc_rejects_future_nbf() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    let now = chrono::Utc::now().timestamp() as usize;
    *mock.nbf_override.lock().unwrap() = Some(now + 3600);

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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
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
        "client must receive the nbf rejection: {text}"
    );
}

/// R1-19: nbf は Option なので、クレーム欠落時も従来どおり受理される。
#[tokio::test]
async fn test_oidc_accepts_missing_nbf() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    *mock.omit_nbf.lock().unwrap() = true;

    let body = oidc_login(&app, oidc.as_ref(), &mock).await;
    assert_eq!(body["authorized"], true);
}

/// R1-19: clock skew (60 秒) 内の未来 nbf は受理される（既存の有効トークンも壊れない）。
#[tokio::test]
async fn test_oidc_accepts_nbf_within_clock_skew() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    let now = chrono::Utc::now().timestamp() as usize;
    *mock.nbf_override.lock().unwrap() = Some(now + 30);

    let body = oidc_login(&app, oidc.as_ref(), &mock).await;
    assert_eq!(body["authorized"], true);
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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
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
            .header(header::COOKIE, format!("oidc_state={auth_state}"))
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

    // R2-REMOTE-REVOKE: 失効判定は DB 正本。メモリが空の第2 AppState（同一 DB）でも
    // 同じ JWT は 401 になる（他インスタンスの logout が即時に反映される）。
    let second_state = Arc::new(AppState {
        db: db.clone(),
        http_client: test_http_client(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: false,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        oidc: None,
        // メモリは意図的に空。DB の revoked_sessions だけに失効が残っている。
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let second_app = crate::build_router(second_state);
    let response = second_app
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

/// R2-SERVER: 失効済み sub の JWT は /auth/logout でも 401 となり、
/// sid の失効書き込みを行わない（sid 側の冪等性は従来どおり維持）。
#[tokio::test]
async fn test_logout_rejects_revoked_sub() {
    let (app, db) = setup_app_with_db().await;
    let sid = "logout-revoked-sub-sid";
    let claims = Claims {
        sub: "test_user".to_string(),
        exp: 10000000000,
        sid: Some(sid.to_string()),
        iat: None,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_ref()).unwrap(),
    )
    .unwrap();
    // iat=None は 0 として扱われるため、閾値 1 で確実に失効と判定される。
    db.execute(
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES ('test_user', 1)",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::post("/auth/logout")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let mut rows = db
        .query(
            "SELECT 1 FROM revoked_sessions WHERE sid = ?",
            [sid.to_string()],
        )
        .await
        .unwrap();
    assert!(
        rows.next().await.unwrap().is_none(),
        "revoked sub must not write a sid on logout"
    );
}

/// R1-18: ログアウト時に refresh_token Cookie を Max-Age=0 で削除する。
/// DB 側のファミリ失効だけではブラウザに 30 日残ってしまう。
#[tokio::test]
async fn test_logout_clears_refresh_cookie() {
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    const SENT_REFRESH: &str = "sent_refresh_token_must_not_be_echoed";

    let response = app
        .oneshot(with_peer(
            Request::post("/auth/logout")
                .header("authorization", format!("Bearer {token}"))
                .header(header::COOKIE, format!("refresh_token={SENT_REFRESH}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_owned)
        .collect();
    let deletion = cookies
        .iter()
        .find(|cookie| cookie.starts_with("refresh_token="))
        .expect("logout must set a refresh_token deletion cookie");
    assert!(
        deletion.starts_with("refresh_token=;"),
        "deletion cookie must have an empty value: {deletion}"
    );
    assert!(deletion.contains("Path=/auth;"), "{deletion}");
    assert!(deletion.contains("HttpOnly"), "{deletion}");
    assert!(deletion.contains("SameSite=Strict"), "{deletion}");
    assert!(deletion.contains("Max-Age=0"), "{deletion}");
    assert!(
        !deletion.contains(SENT_REFRESH),
        "the presented token must not be echoed: {deletion}"
    );
    // R2-SESSION-COOKIE: session Cookie は発行されないため logout の削除対象に含まない。
    assert!(
        cookies.iter().all(|cookie| !cookie.starts_with("session=")),
        "logout must not set a session cookie: {cookies:?}"
    );

    let body = to_bytes(response.into_body(), 64).await.unwrap();
    assert_eq!(body.as_ref(), b"true", "既存の Json(true) 本文を維持する");
}

/// R2: OIDC 構成の `cookie_secure` が削除 Cookie の Secure 属性を決める。
/// 発行側 refresh_cookie と同じ設定を単一ソースとして共有することを検証する。
#[tokio::test]
async fn test_logout_deletion_cookie_follows_oidc_cookie_secure() {
    for cookie_secure in [false, true] {
        let (app, _oidc, _mock, _db) = setup_app_with_oidc_age_policy_and_cookie_secure(
            common::age::AgePolicy::default(),
            cookie_secure,
        )
        .await;
        let token = generate_token(TEST_JWT_SECRET, "test_user").await;
        let response = app
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
        let cookie = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find(|value| value.starts_with("refresh_token="))
            .expect("logout must set a refresh_token deletion cookie")
            .to_owned();
        assert_eq!(
            cookie.contains("; Secure"),
            cookie_secure,
            "cookie_secure={cookie_secure}: {cookie}"
        );
    }
}

/// R2: OIDC 未構成（mock 認証）では env に関わらず cookie_secure 既定 true を使い、
/// 削除 Cookie に Secure を付与する（fail-closed / 直接 env 再読込の廃止）。
#[tokio::test]
async fn test_logout_deletion_cookie_secure_by_default_without_oidc() {
    // 旧実装はこの env を直接読んでいたが、OIDC 未構成では無視される。
    let (_env_guard, previous) = lock_env_and_read("OIDC_COOKIE_SECURE").await;
    test_set_env("OIDC_COOKIE_SECURE", "0");
    let app = setup_app().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let response = app
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
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("refresh_token="))
        .expect("logout must set a refresh_token deletion cookie")
        .to_owned();
    assert!(
        cookie.contains("; Secure"),
        "default must be fail-closed: {cookie}"
    );

    match previous {
        Some(value) => test_set_env("OIDC_COOKIE_SECURE", &value),
        None => test_remove_env("OIDC_COOKIE_SECURE"),
    }
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
                .header(header::COOKIE, "oidc_state=unknown-state")
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
        // R2-AUTHZ: 仕様上 sub は任意だが、本システムは refresh 失効を sub に
        // 依存するため sid-only の logout token を 401 で拒否する。
        (
            "sid_only_without_sub",
            json!({
                "sid": "some-sid",
                "events": {"http://schemas.openid.net/event/backchannel-logout": {}},
            }),
        ),
        (
            "non_empty_events",
            json!({
                "sid": "some-sid",
                "events": {"http://schemas.openid.net/event/backchannel-logout": {"foo": "bar"}},
            }),
        ),
        (
            "events_not_an_object",
            json!({
                "sid": "some-sid",
                "events": "http://schemas.openid.net/event/backchannel-logout",
            }),
        ),
        (
            "nonce_present",
            json!({
                "sid": "some-sid",
                "events": {"http://schemas.openid.net/event/backchannel-logout": {}},
                "nonce": "some-nonce",
            }),
        ),
        (
            "nonce_present_as_number",
            json!({
                "sid": "some-sid",
                "events": {"http://schemas.openid.net/event/backchannel-logout": {}},
                "nonce": 42,
            }),
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

    // 専用ヘルパーが生成する sid-only トークン（sub 無し）も401で拒否する。
    let logout_token = mock.logout_token("sid-only");
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
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
    // R2-AUTHZ: sub 無しの sid-only logout token は拒否されるため、
    // セッション失効の回帰では sid と sub の両方を付与する。
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

    // R2-AUTH item1 回帰: refresh token 発行をトリガで失敗させ、発行失敗時に
    // 旧閾値が DB から消えない（旧 JWT が復活しない）ことを固定する。
    // 旧順序（DB 削除 → 発行）ではここで count が 0 になり失敗する。
    db.execute(
        "CREATE TRIGGER fail_refresh_issue BEFORE INSERT ON refresh_tokens
         BEGIN SELECT RAISE(ABORT, 'injected refresh issuance failure'); END",
        (),
    )
    .await
    .unwrap();

    let start = app
        .clone()
        .oneshot(with_peer(
            Request::get("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::OK);
    let started: serde_json::Value =
        serde_json::from_slice(&to_bytes(start.into_body(), 4096).await.unwrap()).unwrap();
    let failed_state = started["state"].as_str().unwrap().to_string();
    let failed_nonce = oidc.pending_nonce(&failed_state).expect("pending nonce");
    *mock.nonce.lock().unwrap() = Some(failed_nonce);
    let failed = app
        .clone()
        .oneshot(with_peer(
            Request::get(format!(
                "/auth/oidc/callback?code=test-code&state={failed_state}"
            ))
            .header(header::COOKIE, format!("oidc_state={failed_state}"))
            .body(Body::empty())
            .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(
        failed.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "injected refresh-token issuance failure must fail the callback"
    );
    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM revoked_subs WHERE sub = ?",
            [claims.sub.clone()],
        )
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        count, 1,
        "issuance failure must keep the old revocation threshold in DB"
    );

    db.execute("DROP TRIGGER fail_refresh_issue", ())
        .await
        .unwrap();

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

/// R2-LOGOUT-TOKEN: 同一 logout token の再送は revoked_at を変えず、再認証セッションを維持する。
#[tokio::test]
async fn test_backchannel_logout_replay_keeps_revoked_at_and_reauthenticated_session() {
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
    let issued_at = chrono::Utc::now().timestamp() as usize;
    let logout_token = mock.logout_token_for_sub_with_times(&claims.sub, issued_at, None);

    for _ in 0..2 {
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
    }

    let read_revoked_at = || async {
        let mut rows = db
            .query(
                "SELECT revoked_at FROM revoked_subs WHERE sub = ?",
                [claims.sub.clone()],
            )
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    };
    let first_threshold = read_revoked_at().await;
    assert_eq!(first_threshold, issued_at as i64);

    let new_body = oidc_login(&app, oidc.as_ref(), &mock).await;
    let new_token = new_body["token"].as_str().unwrap().to_string();
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
    assert_eq!(read_revoked_at().await, first_threshold);

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

/// R2-LOGOUT-TOKEN: 未来の iat / nbf を持つ logout token は401で拒否する。
#[tokio::test]
async fn test_backchannel_logout_rejects_future_iat_and_nbf() {
    let (app, _oidc, mock, _db) = setup_app_with_oidc().await;
    let now = chrono::Utc::now().timestamp() as usize;
    for logout_token in [
        mock.logout_token_for_sub_with_times("future-iat", now + 600, None),
        mock.logout_token_for_sub_with_times("future-nbf", now, Some(now + 600)),
    ] {
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
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

/// R2-LOGOUT-TOKEN: 古い iat の再送は DB の失効閾値を前進させない。
#[tokio::test]
async fn test_backchannel_logout_stale_iat_does_not_advance_threshold() {
    let (app, _oidc, mock, db) = setup_app_with_oidc().await;
    let now = chrono::Utc::now().timestamp() as usize;
    let fresh = mock.logout_token_for_sub_with_times("stale-sub", now, None);
    let stale = mock.logout_token_for_sub_with_times("stale-sub", now - 120, None);
    for logout_token in [fresh, stale] {
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
    }
    let mut rows = db
        .query(
            "SELECT revoked_at FROM revoked_subs WHERE sub = 'stale-sub'",
            (),
        )
        .await
        .unwrap();
    let threshold: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(threshold, now as i64);
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
    // 全ゼロは回収不能な送金先なので拒否する。
    assert!(!crate::payments::is_valid_eth_address(
        "0x0000000000000000000000000000000000000000"
    ));
}

/// JPYC: 円→Wei 変換の境界。0 / 上限超え / i64・u64 境界を拒否し、上限値まで正確に変換する。
#[test]
fn test_jpy_to_wei_bounds() {
    // 1 円 = 10^18 wei（JPYC_DECIMALS = 18）。
    assert_eq!(
        crate::payments::jpy_to_wei(1).unwrap(),
        10u128.pow(18).to_string()
    );
    // 1,000,000 円 = 10^24 wei。u64::MAX (≈1.8e19) を確実に超えるため u128 演算が必要。
    let max_wei = crate::payments::jpy_to_wei(MAX_ORDER_JPY).unwrap();
    assert_eq!(
        max_wei,
        (u128::from(MAX_ORDER_JPY) * 10u128.pow(18)).to_string()
    );
    assert_eq!(max_wei.len(), 25);

    // 0 と上限超えは拒否する。i64::MAX / u64::MAX も乗算前に拒否し、オーバーフローしない。
    let expected_error = format!("amount_jpy must be between 1 and {MAX_ORDER_JPY}");
    for invalid in [0u64, MAX_ORDER_JPY + 1, i64::MAX as u64, u64::MAX] {
        let err = crate::payments::jpy_to_wei(invalid).unwrap_err();
        assert!(
            matches!(&err, AppError::BadRequest(msg) if msg == &expected_error),
            "unexpected error for {invalid}: {err}"
        );
    }
}

/// JPYC: DB 保存形式の wei 文字列から円建て金額を導出する境界。
/// 10^18 スケールで割り切れる値のみ受理し、商が u64 を超える値は拒否する。
#[test]
fn test_amount_jpy_from_wei_bounds() {
    const WEI_PER_JPY: u128 = 1_000_000_000_000_000_000;

    // 成功: 1 円 = 10^18 wei。注文上限分の wei も上限の円へ戻る。
    assert_eq!(
        crate::payments::amount_jpy_from_wei(&WEI_PER_JPY.to_string()).unwrap(),
        1u64
    );
    let max_wei = (u128::from(MAX_ORDER_JPY) * WEI_PER_JPY).to_string();
    assert_eq!(
        crate::payments::amount_jpy_from_wei(&max_wei).unwrap(),
        MAX_ORDER_JPY
    );
    // この関数が検証するのは u64 範囲まで。注文上限の検証は呼び出し側の責務。
    let u64_max_wei = (u128::from(u64::MAX) * WEI_PER_JPY).to_string();
    assert_eq!(
        crate::payments::amount_jpy_from_wei(&u64_max_wei).unwrap(),
        u64::MAX
    );

    // 拒否: 商が u64 を超える (u64::MAX + 1 円分)。u128 の範囲内で検証する。
    let over_u64_wei = (u128::from(u64::MAX) + 1) * WEI_PER_JPY;
    let err = crate::payments::amount_jpy_from_wei(&over_u64_wei.to_string()).unwrap_err();
    assert!(
        matches!(&err, AppError::Internal(msg) if msg.contains("exceeds u64 JPY")),
        "unexpected error for {over_u64_wei}: {err}"
    );

    // 拒否: 0 wei と 10^18 スケールで 0 になる先頭ゼロ付き文字列。
    // jpy_to_wei(0) と同じく「注文金額は正」という契約に揃え、Internal で fail-closed にする。
    for zero in ["0", "000000000000000000"] {
        let err = crate::payments::amount_jpy_from_wei(zero).unwrap_err();
        assert!(
            matches!(&err, AppError::Internal(msg) if msg == "amount_wei is zero"),
            "unexpected error for {zero:?}: {err}"
        );
    }

    // 拒否: 数値として解析できない文字列 (空文字・非数字・負数・小数・空白混入・16進表記)
    // と u128::MAX を超える桁数 (parse::<u128> のオーバーフローも fail-closed で Internal)。
    for invalid in ["", "abc", "-1", "1.5", " 1", "0x1"]
        .iter()
        .map(|s| s.to_string())
        .chain(std::iter::once(format!("{}0", u128::MAX)))
    {
        let err = crate::payments::amount_jpy_from_wei(&invalid).unwrap_err();
        assert!(
            matches!(&err, AppError::Internal(msg) if msg.contains("invalid amount_wei")),
            "unexpected error for {invalid:?}: {err}"
        );
    }
}

/// JPYC: `checked_div(scale).filter(|jpy| jpy * scale == wei)` の回帰。
/// 割り切れない wei を切り捨てて整数円として受理しないことを固定する。
#[test]
fn test_amount_jpy_from_wei_rejects_indivisible_wei() {
    const WEI_PER_JPY: u128 = 1_000_000_000_000_000_000;

    // 1 wei は 0 円、scale/2 は半円、scale±1 も整数円にならない。
    // 注文上限分に 1 wei を足した値も同じく拒否する。
    for wei in [
        1u128,
        WEI_PER_JPY / 2,
        WEI_PER_JPY - 1,
        WEI_PER_JPY + 1,
        u128::from(MAX_ORDER_JPY) * WEI_PER_JPY + 1,
    ] {
        let err = crate::payments::amount_jpy_from_wei(&wei.to_string()).unwrap_err();
        assert!(
            matches!(&err, AppError::Internal(msg) if msg.contains("not a whole JPY amount")),
            "wei {wei} must be rejected as indivisible: {err}"
        );
    }
}

/// JPYC: 対応 4 チェーンの必要確認数と、未対応チェーンの i64::MAX 分岐。
#[test]
fn test_required_confirmations_by_chain() {
    assert_eq!(crate::payments::required_confirmations(1), 32);
    assert_eq!(crate::payments::required_confirmations(137), 256);
    assert_eq!(crate::payments::required_confirmations(43114), 12);
    assert_eq!(crate::payments::required_confirmations(8217), 12);

    // 未対応チェーンは i64::MAX を返し、どんな確認数でも FINALIZED に到達できないようにする。
    for unsupported in [0i64, 2, 10, -1, i64::MAX] {
        assert_eq!(
            crate::payments::required_confirmations(unsupported),
            i64::MAX
        );
    }
}

/// JPYC: 署名復元の入力エラー（非 hex / 65 バイト以外 / 不正 recovery id / 復元失敗）。
#[test]
fn test_recover_ethereum_address_rejects_invalid_signatures() {
    let message = b"openIllustration payment order boundary";

    // 非 hex は hex::decode で拒否する（長さガードを通すため 0x 込み 132 文字）。
    let non_hex = format!("0x{}", "zz".repeat(65));
    let err = crate::payments::recover_ethereum_address(message, &non_hex).unwrap_err();
    assert!(
        matches!(&err, AppError::BadRequest(msg) if msg == "signature must be hex"),
        "unexpected error: {err}"
    );

    // 64 バイト（130 文字）は長さガードを通り、65 バイト検査で拒否する。
    let short = format!("0x{}", "11".repeat(64));
    let err = crate::payments::recover_ethereum_address(message, &short).unwrap_err();
    assert!(
        matches!(&err, AppError::BadRequest(msg) if msg == "signature must be 65 bytes"),
        "unexpected error: {err}"
    );

    // 66 バイト（134 文字）は hex::decode 前の長さガードで拒否する。
    let long = format!("0x{}", "11".repeat(66));
    let err = crate::payments::recover_ethereum_address(message, &long).unwrap_err();
    assert!(
        matches!(&err, AppError::BadRequest(msg) if msg == "signature hex must be 130 or 132 characters"),
        "unexpected error: {err}"
    );

    // recovery id が 0/1/27/28 以外は拒否する。
    let mut signature = vec![0x11u8; 65];
    for v in [2u8, 26, 29, 255] {
        signature[64] = v;
        let encoded = format!("0x{}", hex::encode(&signature));
        let err = crate::payments::recover_ethereum_address(message, &encoded).unwrap_err();
        assert!(
            matches!(&err, AppError::BadRequest(msg) if msg == "invalid signature recovery id"),
            "unexpected error: {err}"
        );
    }

    // r=5 は secp256k1 曲線上に存在しない x 座標なので復元に失敗する（s=1 は有効なスカラー）。
    let mut signature = [0u8; 65];
    signature[31] = 5;
    signature[63] = 1;
    signature[64] = 0;
    let encoded = format!("0x{}", hex::encode(signature));
    let err = crate::payments::recover_ethereum_address(message, &encoded).unwrap_err();
    assert!(
        matches!(&err, AppError::Unauthorized(msg) if msg == "signature recovery failed"),
        "unexpected error: {err}"
    );
}

/// JPYC: hex::decode 前に署名文字列長を検査し、130/132 文字以外を早期 BadRequest で拒否する。
#[test]
fn test_recover_ethereum_address_rejects_oversized_hex_before_decode() {
    let message = b"openIllustration payment order oversized signature";
    let guard_error = "signature hex must be 130 or 132 characters";

    for len in [8, 129, 131, 133, 1_000_000] {
        let candidate = "a".repeat(len);
        let err = crate::payments::recover_ethereum_address(message, &candidate).unwrap_err();
        assert!(
            matches!(&err, AppError::BadRequest(msg) if msg == guard_error),
            "length {len} must be rejected by the pre-decode guard, got {err}"
        );
    }

    // 0x 付きも「文字列全体の長さ」で判定されるため、同じく早期拒否される。
    let oversized = format!("0x{}", "ab".repeat(100_000));
    let err = crate::payments::recover_ethereum_address(message, &oversized).unwrap_err();
    assert!(
        matches!(&err, AppError::BadRequest(msg) if msg == guard_error),
        "unexpected error: {err}"
    );

    // 対照: 130 / 132 文字はガードを通り、65 バイトの復元処理まで進む。
    // 0xaa は recovery id として不正なので "invalid signature recovery id" が返る。
    for candidate in ["a".repeat(130), format!("0x{}", "a".repeat(130))] {
        let err = crate::payments::recover_ethereum_address(message, &candidate).unwrap_err();
        assert!(
            matches!(&err, AppError::BadRequest(msg) if msg == "invalid signature recovery id"),
            "length {} must pass the hex-length guard, got {err}",
            candidate.len()
        );
    }
}

/// JPYC: 0x プレフィックス無しの署名と 27/28 形式の recovery id も同じアドレスへ復元する。
#[test]
fn test_recover_ethereum_address_accepts_hex_variants() {
    let message = b"openIllustration payment order signature variants";
    let wallet_key = SigningKey::from_slice(&[0x42u8; 32]).expect("valid k256 test scalar");
    let expected = crate::payments::address_from_verifying_key(wallet_key.verifying_key());

    let mut hasher = Keccak256::new();
    hasher.update(format!("\x19Ethereum Signed Message:\n{}", message.len()).as_bytes());
    hasher.update(message);
    let digest = hasher.finalize();
    let (signature, recovery_id) = wallet_key.sign_prehash_recoverable(&digest);
    let signature_bytes = signature.to_bytes();

    for recovery_byte in [recovery_id.to_byte(), recovery_id.to_byte() + 27] {
        let mut raw = signature_bytes.to_vec();
        raw.push(recovery_byte);
        let encoded = hex::encode(&raw);
        // 0x 付き／無しの両方を受け付ける。
        for candidate in [encoded.as_str(), format!("0x{encoded}").as_str()] {
            let recovered = crate::payments::recover_ethereum_address(message, candidate).unwrap();
            assert_eq!(recovered, expected);
        }
    }
}

/// JPYC: expires_at == now の境界も期限切れとして扱う（<= 判定）。
#[test]
fn test_order_expiry_boundary_is_inclusive() {
    let now = "2026-01-01T00:00:00+00:00";
    assert!(crate::payments::is_order_expired(now, now));
    assert!(crate::payments::is_order_expired(
        "2025-12-31T23:59:59+00:00",
        now
    ));
    assert!(!crate::payments::is_order_expired(
        "2026-01-01T00:00:01+00:00",
        now
    ));
}

/// JPYC: 小数秒の桁数（0/3/6/9）やタイムゾーン表記が違っても RFC3339 の
/// 時刻として比較し、辞書順比較のような前後逆転を起こさない。
#[test]
fn test_order_expiry_parses_rfc3339_fractional_digits() {
    // 同一時刻を小数秒ありで表したもの。辞書順では ".000+00:00" が "+00:00" より
    // 後に来る（'.' > '+'）ため文字列比較なら有効と誤判定されるが、
    // 境界 <= により期限切れが正しい。3/6/9 桁でも同様。
    for expires_at in [
        "2026-01-01T00:00:00.000+00:00",
        "2026-01-01T00:00:00.000000+00:00",
        "2026-01-01T00:00:00.000000000+00:00",
    ] {
        assert!(
            crate::payments::is_order_expired(expires_at, "2026-01-01T00:00:00+00:00"),
            "{expires_at} は now と同一時刻なので期限切れ"
        );
    }

    // 3 桁の `.000Z` は 9 桁の `.000000001+00:00` より 1ns 前なので期限切れ。
    // 辞書順では 'Z'(0x5A) > '0'(0x30) のため前後が逆転する。
    assert!(crate::payments::is_order_expired(
        "2026-01-01T00:00:00.000Z",
        "2026-01-01T00:00:00.000000001+00:00",
    ));

    // 9 桁の未来時刻（境界より 1ns 後）は有効。
    assert!(!crate::payments::is_order_expired(
        "2026-01-01T00:00:00.000000001+00:00",
        "2026-01-01T00:00:00+00:00",
    ));

    // タイムゾーン表記違い：+09:00 の 09:00 は UTC の 00:00 と同一時刻。
    assert!(crate::payments::is_order_expired(
        "2026-01-01T09:00:00+09:00",
        "2026-01-01T00:00:00+00:00",
    ));

    // パースできない値は fail-closed（期限切れ扱い）。now 側も同様。
    for (expires_at, now) in [
        ("not-a-rfc3339-date", "2026-01-01T00:00:00+00:00"),
        ("2026-01-01T00:00:00+00:00", "not-a-rfc3339-date"),
    ] {
        assert!(
            crate::payments::is_order_expired(expires_at, now),
            "パース失敗は fail-closed: {expires_at} / {now}"
        );
    }
}

/// JPYC: non-hardened パス m/44'/60'/0'/0/{index} の上限 2^31-1 を超える値は
/// 受取アドレス再利用を防ぐため拒否する。
#[test]
fn test_derivation_index_bounds() {
    assert_eq!(crate::payments::derivation_index_to_u32(0).unwrap(), 0u32);
    assert!(crate::payments::derivation_index_to_u32(-1).is_err());
    // 上限 2^31-1 (=2147483647) は許可。
    let max_non_hardened = (1i64 << 31) - 1;
    assert_eq!(
        crate::payments::derivation_index_to_u32(max_non_hardened).unwrap(),
        max_non_hardened as u32
    );
    // 上限 +1 (=2^31) は拒否。
    assert!(crate::payments::derivation_index_to_u32(1i64 << 31).is_err());
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
                            "cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z",
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

/// JPYC: 注文作成リクエストを送り、状態コードと JSON 本文を返す（テスト専用）。
/// 検証対象は並行注文の受取先・index なので、コンテンツと金額は常に同じ値を使う。
async fn post_order_request(
    app: &Router,
    token: &str,
    chain_id: i64,
    wallet_address: &str,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/payments/orders")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "chain_id": chain_id,
                        "wallet_address": wallet_address,
                        "cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z",
                        "amount_jpy": 1000,
                    })
                    .to_string(),
                ))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let status = response.status();
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    (status, body)
}

/// テスト用: 保存済み注文の `(derivation_index, recipient_address)` を index 順で返す。
/// NULL（固定受取アドレス）は SQLite の並び順で先に来る。
async fn stored_order_derivations(db: &libsql::Connection) -> Vec<(Option<i64>, String)> {
    let mut rows = db
        .query(
            "SELECT derivation_index, recipient_address FROM payment_orders
              ORDER BY derivation_index, order_id",
            (),
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        let index: Option<i64> = row.get(0).unwrap();
        let address: String = row.get(1).unwrap();
        out.push((index, address));
    }
    out
}

/// R1-26: 同一 xprv・同一 DB への並行注文は連番の derivation_index と、
/// 異なる受取先アドレスを得る（書き込みトランザクション内の原子的予約）。
#[tokio::test]
async fn test_parallel_orders_derive_sequential_indices_and_unique_addresses() {
    let xprv = "xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi";
    let (app, db) = setup_app_with_hd_payments(xprv).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let tasks: Vec<_> = (0..2)
        .map(|_| {
            let app = app.clone();
            let token = token.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                post_order_request(
                    &app,
                    &token,
                    137,
                    "0x1111111111111111111111111111111111111111",
                )
                .await
            })
        })
        .collect();

    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    for (status, body) in &results {
        assert_eq!(*status, StatusCode::OK, "並行注文が失敗した: {body}");
    }

    let mut returned: Vec<String> = results
        .iter()
        .map(|(_, body)| body["recipient_address"].as_str().unwrap().to_string())
        .collect();
    returned.sort();
    assert_ne!(returned[0], returned[1], "受取先アドレスが重複した");

    let stored = stored_order_derivations(&db).await;
    assert_eq!(stored.len(), 2);
    assert_eq!(
        stored.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        vec![Some(0), Some(1)],
        "derivation_index は 0,1 の連番であること"
    );
    let mut addresses: Vec<String> = stored.iter().map(|(_, a)| a.clone()).collect();
    addresses.sort();
    assert_eq!(addresses, returned, "応答と保存済みの受取先が一致すること");
}

/// R1-26: 同じ内容の並行注文は UNIQUE 違反で失敗せず、全件成功する。
#[tokio::test]
async fn test_identical_parallel_orders_all_succeed() {
    const CONCURRENCY: usize = 5;
    let xprv = "xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi";
    let (app, db) = setup_app_with_hd_payments(xprv).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(CONCURRENCY));
    let tasks: Vec<_> = (0..CONCURRENCY)
        .map(|_| {
            let app = app.clone();
            let token = token.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                post_order_request(
                    &app,
                    &token,
                    137,
                    "0x1111111111111111111111111111111111111111",
                )
                .await
            })
        })
        .collect();

    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    for (status, body) in &results {
        assert_eq!(*status, StatusCode::OK, "UNIQUE 違反で失敗した: {body}");
    }

    let stored = stored_order_derivations(&db).await;
    assert_eq!(stored.len(), CONCURRENCY);
    assert_eq!(
        stored.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        (0..CONCURRENCY as i64).map(Some).collect::<Vec<_>>(),
        "全注文に異なる連番が割り当てられること"
    );
    let unique: std::collections::HashSet<&String> = stored.iter().map(|(_, a)| a).collect();
    assert_eq!(unique.len(), CONCURRENCY, "受取先アドレスは全件異なること");
}

/// R1-26 回帰: 固定受取アドレス設定では derivation_index=NULL のまま複数注文できる。
#[tokio::test]
async fn test_fixed_receiving_address_keeps_derivation_index_null() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    for _ in 0..3 {
        let (status, body) = post_order_request(&app, &token, 137, receiving).await;
        assert_eq!(status, StatusCode::OK, "固定アドレス注文が失敗した: {body}");
        assert_eq!(body["recipient_address"].as_str().unwrap(), receiving);
    }

    let stored = stored_order_derivations(&db).await;
    assert_eq!(stored.len(), 3);
    assert!(
        stored
            .iter()
            .all(|(index, address)| index.is_none() && address == receiving),
        "固定受取アドレスでは index が NULL のまま再利用されること"
    );
}

/// 固定受取アドレス設定でも並行注文が全件成功し、order が全件保存される。
#[tokio::test]
async fn test_fixed_receiving_address_parallel_orders_all_saved() {
    const CONCURRENCY: usize = 5;
    let receiving = "0x1111111111111111111111111111111111111111";
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(CONCURRENCY));
    let tasks: Vec<_> = (0..CONCURRENCY)
        .map(|_| {
            let app = app.clone();
            let token = token.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                post_order_request(
                    &app,
                    &token,
                    137,
                    "0x1111111111111111111111111111111111111111",
                )
                .await
            })
        })
        .collect();

    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    for (status, body) in &results {
        assert_eq!(*status, StatusCode::OK, "並行注文が失敗した: {body}");
        assert_eq!(body["recipient_address"].as_str().unwrap(), receiving);
    }

    let stored = stored_order_derivations(&db).await;
    assert_eq!(stored.len(), CONCURRENCY, "全注文が保存されること");
    assert!(
        stored
            .iter()
            .all(|(index, address)| index.is_none() && address == receiving),
        "固定受取アドレスでは index が NULL のまま再利用されること"
    );
}

/// 固定アドレス分岐の INSERT も HD 分岐と同じく write_lock を通ることを検証する。
/// mutation で `insert_payment_order(&state.db, ...)` に戻すと、ロック保持中に
/// リクエストが完了してしまうため失敗する。
#[tokio::test]
async fn test_fixed_address_order_waits_for_write_transaction() {
    let receiving = "0x1111111111111111111111111111111111111111";
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) = setup_app_with_payments(receiving, verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let (started, release, writer) = spawn_lock_holding_writer(Arc::clone(&db));
    started.await.unwrap();

    let app_for_task = app.clone();
    let token_for_task = token.clone();
    let mut order_task = tokio::spawn(async move {
        post_order_request(&app_for_task, &token_for_task, 137, receiving).await
    });
    tokio::task::yield_now().await;

    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut order_task)
            .await
            .is_err(),
        "固定アドレス注文は書き込みトランザクションの完了を待つべき"
    );

    release.send(()).unwrap();
    writer.await.unwrap();

    let (status, body) = tokio::time::timeout(Duration::from_secs(5), &mut order_task)
        .await
        .expect("order must finish after the write transaction commits")
        .unwrap();
    assert_eq!(status, StatusCode::OK, "ロック解放後は成功すること: {body}");
    assert_eq!(body["recipient_address"].as_str().unwrap(), receiving);

    let stored = stored_order_derivations(&db).await;
    assert_eq!(stored.len(), 1, "注文が 1 件保存されること");
    assert!(
        stored
            .iter()
            .all(|(index, address)| index.is_none() && address == receiving)
    );
}

/// R1-26: 不正 chain_id / wallet_address で拒否した注文は derivation_index を消費しない。
#[tokio::test]
async fn test_rejected_orders_do_not_consume_derivation_index() {
    let xprv = "xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi";
    let (app, db) = setup_app_with_hd_payments(xprv).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let wallet = "0x1111111111111111111111111111111111111111";

    let (status, _) = post_order_request(&app, &token, 999_999, wallet).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "不正 chain_id は拒否される"
    );
    let (status, _) = post_order_request(&app, &token, 137, "0xnot-an-address").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "不正 wallet_address は拒否される"
    );

    for _ in 0..2 {
        let (status, body) = post_order_request(&app, &token, 137, wallet).await;
        assert_eq!(status, StatusCode::OK, "正しい注文が失敗した: {body}");
    }

    let stored = stored_order_derivations(&db).await;
    assert_eq!(stored.len(), 2, "拒否された注文は保存されない");
    assert_eq!(
        stored.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        vec![Some(0), Some(1)],
        "拒否注文で index が飛ばないこと"
    );
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
    // セッション JWT の nbf 検証も OIDC 側と同じく明示有効（jsonwebtoken 既定は false）。
    assert!(
        keys.validation().validate_nbf,
        "session JWT must validate nbf"
    );
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
    // 引数 kid は公開鍵から導出した値と一致しなければならない（R2-SERVER）。
    let old_kid = crate::jwt::JwtKeys::kid_from_public_pem(&old_public).expect("derive old kid");
    let mut keys =
        crate::jwt::JwtKeys::from_rsa_private_key_pem(&new_pem).expect("new signing key");
    keys.add_verification_key(&old_kid, &old_public)
        .expect("register old public key");

    let old_keys = crate::jwt::JwtKeys::from_rsa_private_key_pem(&old_pem).expect("old key");
    let claims = Claims {
        sub: "during_rotation".to_string(),
        exp: 10000000000,
        sid: None,
        iat: None,
    };
    let mut header = old_keys.header();
    header.kid = Some(old_kid);
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

/// RS256: 公開 PEM を HMAC 秘密として署名した HS256 トークンを、
/// `decode()` の alg 不一致分岐で直接拒否する。
/// 既存の混同テストは RSA 署名鍵で HS256 署名しようとして署名自体が失敗し、
/// `decode()` の分岐を通っていなかったため、ここで明示的に通す。
#[test]
fn test_jwt_rejects_hs256_token_signed_with_public_pem() {
    let pem = test_rsa_private_pem();
    let public_pem = test_rsa_public_pem(&pem);
    let keys = crate::jwt::JwtKeys::from_rsa_private_key_pem(&pem).expect("valid RSA key");
    let claims = Claims {
        sub: "attacker".to_string(),
        exp: 10000000000,
        sid: None,
        iat: None,
    };

    // 公開鍵は誰でも入手できるため、HMAC 秘密として使える。
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some(keys.header().kid.clone().expect("RS256 header kid"));
    let hmac_key = EncodingKey::from_secret(public_pem.as_bytes()).expect("HS256 encoding key");
    let token = encode(&header, &claims, &hmac_key).expect("sign HS256 with public PEM");

    let error = keys
        .decode::<Claims>(&token)
        .expect_err("RS256 verifier must reject an HS256 token");
    assert!(
        error.contains("unexpected JWT algorithm"),
        "the explicit algorithm check must reject before key selection: {error}"
    );
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

/// `JWT_RSA_VERIFICATION_KEYS`（`jwt_keys_from_env` の verification_keys_json）
/// に旧公開鍵を登録すると、旧 kid 付きトークンが検証できる。
#[test]
fn test_jwt_keys_from_env_verification_keys_json_accepts_old_kid() {
    let new_pem = test_rsa_private_pem();
    let old_pem = test_rsa_private_pem();
    let old_public = test_rsa_public_pem(&old_pem);
    // kid は公開鍵から導出した値と一致させる（R2-SERVERの整合検査）。
    let old_kid = crate::jwt::JwtKeys::kid_from_public_pem(&old_public).expect("derive old kid");
    // 実コードと同じ kid -> SPKI 公開鍵 PEM の JSON オブジェクト。
    let map = std::collections::HashMap::from([(old_kid.clone(), old_public)]);
    let json = serde_json::to_string(&map).expect("serialize verification keys");

    let keys = crate::jwt_keys_from_env(Some(new_pem), None, Some(&json), false)
        .expect("register old public key from JSON");
    assert_eq!(keys.algorithm_name(), "RS256");

    let old_keys = crate::jwt::JwtKeys::from_rsa_private_key_pem(&old_pem).expect("old key");
    let claims = Claims {
        sub: "rotated_user".to_string(),
        exp: 10000000000,
        sid: None,
        iat: None,
    };
    let mut header = old_keys.header();
    header.kid = Some(old_kid);
    let token = encode(&header, &claims, old_keys.encoding_key()).expect("sign with old key");
    let decoded: Claims = keys.decode(&token).expect("old kid must verify").claims;
    assert_eq!(decoded.sub, "rotated_user");
}

/// verification_keys_json: 不正な JSON / オブジェクト以外は起動時に拒否する。
#[test]
fn test_jwt_keys_from_env_verification_keys_json_rejects_invalid_json() {
    let error =
        crate::jwt_keys_from_env(Some(test_rsa_private_pem()), None, Some("{not-json"), false)
            .err()
            .expect("malformed JSON must be rejected");
    assert!(
        error.contains("must be a kid->PEM JSON object"),
        "got: {error}"
    );

    let error = crate::jwt_keys_from_env(Some(test_rsa_private_pem()), None, Some("[]"), false)
        .err()
        .expect("non-object JSON must be rejected");
    assert!(
        error.contains("must be a kid->PEM JSON object"),
        "got: {error}"
    );
}

/// verification_keys_json: 空 kid と 129 文字 kid は拒否する（1..=128）。
#[test]
fn test_jwt_keys_from_env_verification_keys_json_rejects_invalid_kid() {
    let public_pem = test_rsa_public_pem(&test_rsa_private_pem());
    for (kid, label) in [(String::new(), "empty"), ("k".repeat(129), "too long")] {
        let map = std::collections::HashMap::from([(kid, public_pem.clone())]);
        let json = serde_json::to_string(&map).expect("serialize kid->PEM map");
        let error =
            crate::jwt_keys_from_env(Some(test_rsa_private_pem()), None, Some(&json), false)
                .err()
                .expect("invalid kid must be rejected");
        assert!(
            error.contains("kid must be 1..=128 characters"),
            "{label}: {error}"
        );
    }
    // 長さは有効でも導出値と違う kid も拒否する（R2-SERVER）。
    let map = std::collections::HashMap::from([("different".to_string(), public_pem)]);
    let json = serde_json::to_string(&map).expect("serialize kid->PEM map");
    let error = crate::jwt_keys_from_env(Some(test_rsa_private_pem()), None, Some(&json), false)
        .err()
        .expect("mismatched kid must be rejected");
    assert!(error.contains("kid mismatch"), "{error}");
}

/// verification_keys_json: 不正な PEM は kid 付きエラーで拒否する。
#[test]
fn test_jwt_keys_from_env_verification_keys_json_rejects_invalid_pem() {
    let json = json!({ "0123456789abcdef0123456789abcdef": "not a PEM" }).to_string();
    let error = crate::jwt_keys_from_env(Some(test_rsa_private_pem()), None, Some(&json), false)
        .err()
        .expect("invalid PEM must be rejected");
    assert!(
        error.contains("JWT_RSA_VERIFICATION_KEYS[0123456789abcdef0123456789abcdef]"),
        "{error}"
    );
    assert!(error.contains("invalid RSA public key PEM"), "{error}");
}

/// verification_keys_json は RS256 専用。HS256 選択時は起動を拒否する。
#[test]
fn test_jwt_keys_from_env_rejects_verification_keys_json_with_hmac() {
    let error =
        crate::jwt_keys_from_env(None, Some(TEST_JWT_SECRET.to_string()), Some("{}"), false)
            .err()
            .expect("RSA verification keys are meaningless with HS256");
    assert!(error.contains("only valid with RS256"), "got: {error}");
}

/// `add_verification_key` の kid 長境界（1 / 128 受理、129 / 空 拒否）。
#[test]
fn test_jwt_add_verification_key_kid_bounds() {
    let signing_pem = test_rsa_private_pem();
    let old_public = test_rsa_public_pem(&test_rsa_private_pem());
    // kid は公開鍵から導出した値のみ受理する（導出値は SHA-256 先頭16バイトの hex）。
    let old_kid = crate::jwt::JwtKeys::kid_from_public_pem(&old_public).expect("derive old kid");
    let mut keys =
        crate::jwt::JwtKeys::from_rsa_private_key_pem(&signing_pem).expect("signing key");

    assert!(
        keys.add_verification_key(&old_kid, &old_public).is_ok(),
        "derived kid"
    );
    // 1 文字 kid は長さ范围是内だが、導出値と一致しないため拒否する。
    let error = keys
        .add_verification_key("k", &old_public)
        .expect_err("mismatched kid");
    assert!(error.contains("kid mismatch"), "got: {error}");
    let error = keys
        .add_verification_key(&"k".repeat(128), &old_public)
        .expect_err("128 char kid must not match the derived kid");
    assert!(error.contains("kid mismatch"), "got: {error}");

    let error = keys
        .add_verification_key(&"k".repeat(129), &old_public)
        .expect_err("129 char kid");
    assert!(error.contains("1..=128"), "got: {error}");
    let error = keys
        .add_verification_key("", &old_public)
        .expect_err("empty kid");
    assert!(error.contains("1..=128"), "got: {error}");
}

/// `add_verification_key` は重複 kid と不正 PEM を拒否する。
#[test]
fn test_jwt_add_verification_key_rejects_duplicate_and_invalid_pem() {
    let signing_pem = test_rsa_private_pem();
    let old_public = test_rsa_public_pem(&test_rsa_private_pem());
    let old_kid = crate::jwt::JwtKeys::kid_from_public_pem(&old_public).expect("derive old kid");
    let mut keys =
        crate::jwt::JwtKeys::from_rsa_private_key_pem(&signing_pem).expect("signing key");

    keys.add_verification_key(&old_kid, &old_public)
        .expect("register once");
    let error = keys
        .add_verification_key(&old_kid, &old_public)
        .expect_err("duplicate kid must be rejected");
    assert!(
        error.contains("duplicate verification key kid"),
        "got: {error}"
    );

    let error = keys
        .add_verification_key("bad", "not a PEM")
        .expect_err("invalid PEM must be rejected");
    assert!(error.contains("invalid RSA public key PEM"), "got: {error}");
}

/// `add_verification_key` は 1024 bit の弱い公開鍵を拒否する。
#[test]
fn test_jwt_add_verification_key_rejects_weak_key() {
    let mut rng = TestRng::new();
    let weak = PrivateKey::new(&mut rng, 1024).expect("generate weak RSA key");
    let weak_public = weak
        .to_public_key()
        .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
        .expect("encode weak public key")
        .to_string();
    let weak_kid = crate::jwt::JwtKeys::kid_from_public_pem(&weak_public).expect("derive weak kid");

    let mut keys = crate::jwt::JwtKeys::from_rsa_private_key_pem(&test_rsa_private_pem())
        .expect("signing key");
    let error = keys
        .add_verification_key(&weak_kid, &weak_public)
        .expect_err("1024-bit verification key must be rejected");
    assert!(error.contains("at least 2048 bits"), "got: {error}");
    assert!(error.contains("1024 bits"), "got: {error}");
}

/// `JWT_RSA_VERIFICATION_KEYS` に 1024 bit 公開鍵を設定すると起動に失敗する。
#[test]
fn test_jwt_keys_from_env_verification_keys_json_rejects_weak_key() {
    let mut rng = TestRng::new();
    let weak = PrivateKey::new(&mut rng, 1024).expect("generate weak RSA key");
    let weak_public = weak
        .to_public_key()
        .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
        .expect("encode weak public key")
        .to_string();
    let weak_kid = crate::jwt::JwtKeys::kid_from_public_pem(&weak_public).expect("derive weak kid");
    let map = std::collections::HashMap::from([(weak_kid.clone(), weak_public)]);
    let json = serde_json::to_string(&map).expect("serialize kid->PEM map");

    let error = crate::jwt_keys_from_env(Some(test_rsa_private_pem()), None, Some(&json), false)
        .err()
        .expect("weak verification key must fail startup");
    assert!(
        error.contains(&format!("JWT_RSA_VERIFICATION_KEYS[{weak_kid}]")),
        "got: {error}"
    );
    assert!(error.contains("at least 2048 bits"), "got: {error}");
}

/// `JWT_REQUIRE_KID` の env 解析（起動時ロジックを純粋関数化して検証）。
#[test]
fn test_jwt_require_kid_from_env_values() {
    assert!(!crate::jwt_require_kid_from_env(None), "unset is false");
    assert!(
        !crate::jwt_require_kid_from_env(Some("0")),
        "\"0\" is false"
    );
    assert!(
        !crate::jwt_require_kid_from_env(Some("false")),
        "\"false\" is false"
    );
    assert!(
        !crate::jwt_require_kid_from_env(Some("FALSE")),
        "comparison is case-insensitive"
    );
    assert!(crate::jwt_require_kid_from_env(Some("1")), "\"1\" is true");
    assert!(
        crate::jwt_require_kid_from_env(Some("true")),
        "\"true\" is true via the shared parse_bool_env"
    );
    assert!(
        crate::jwt_require_kid_from_env(Some(" TRUE ")),
        "trim and lowercase are preserved by the shared parser"
    );
    // 受理値外は fail-closed で false（起動時は bool_env_at_startup が拒否する）。
    for invalid in ["yes", "no", "", "maybe"] {
        assert!(
            !crate::jwt_require_kid_from_env(Some(invalid)),
            "invalid value must fail closed: {invalid:?}"
        );
    }

    // 非 UTF-8 は classify で NonUtf8 → bool_env_state が Invalid。
    // 起動時は bool_env_at_startup が固定文言で拒否し、生バイトは文言へ出ない。
    let non_utf8 = common::env::classify(Err::<String, _>(std::env::VarError::NotUnicode(
        std::ffi::OsString::from_vec(vec![0xff]),
    )));
    assert_eq!(non_utf8, common::env::EnvValue::NonUtf8);
    assert_eq!(
        common::env::bool_env_state(non_utf8.clone()),
        common::env::BoolEnvState::Invalid
    );
    assert_eq!(format!("{non_utf8:?}"), "NonUtf8");
}

/// RS256 選択時に短い JWT_SECRET が警告対象であること（警告条件を関数化して検証）。
#[test]
fn test_jwt_short_secret_warning_condition() {
    assert!(crate::should_warn_short_jwt_secret(Some("short")));
    assert!(!crate::should_warn_short_jwt_secret(Some(TEST_JWT_SECRET)));
    assert!(!crate::should_warn_short_jwt_secret(None));

    // 警告が出ても鍵選択は変わらず、RS256 が使われる（既存挙動の維持）。
    let keys = crate::jwt_keys_from_env(
        Some(test_rsa_private_pem()),
        Some("short".to_string()),
        None,
        false,
    )
    .expect("short JWT_SECRET does not block RS256 startup");
    assert_eq!(keys.algorithm_name(), "RS256");
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
            "configuration_satisfied",
            "country_minimums",
            "default_minimum_age",
            "enforcement_mode",
            "notes",
            "require_explicit_configuration",
            "settings_always_public",
            "target_regions",
            "trusted_issuers",
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

    // 非 UTF-8 の AGE_* 値は「未設定」ではなく Invalid として起動拒否する。
    // VarError::NotUnicode の生バイト（0xff）は固定文言へ変換され、エコーされない。
    let err = common::age::AgePolicy::from_env_with(|name: &str| match name {
        "AGE_REQUIRE_EXPLICIT_CONFIGURATION" => Err(std::env::VarError::NotUnicode(
            std::ffi::OsString::from_vec(vec![0xff]),
        )),
        _ => Err(std::env::VarError::NotPresent),
    })
    .expect_err("non-UTF-8 age flag must be rejected");
    assert_eq!(
        err,
        "AGE_REQUIRE_EXPLICIT_CONFIGURATION must be a boolean (1/0/true/false)"
    );
    assert!(!err.contains('\u{fffd}'), "生バイトを文字列化しない: {err}");
}

/// DEC-01: 署名付き年齢属性の adapter を mock 経路で検証するためのアプリ。
async fn setup_verified_age_app(
    age_policy: common::age::AgePolicy,
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

    let state = Arc::new(AppState {
        db: db.clone(),
        http_client: test_http_client(),
        jwt_keys: test_jwt_keys(),
        allow_mock_auth: true,
        ipfs_gateways: Vec::new(),
        ipfs_timeout: Duration::from_secs(15),
        ipfs_max_attempts: 1,
        payment_receiving_address: None,
        payment_receiving_xprv: None,
        payment_verifier: None,
        // OIDC 未設定のため audience のフォールバックは空文字列。
        oidc: None,
        revoked_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        revoked_subs_before: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        age_policy,
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    (build_router(state), db)
}

fn trusted_age_policy() -> common::age::AgePolicy {
    common::age::AgePolicy {
        enforcement_mode: common::age::AgeEnforcementMode::TrustedAttributeRequired,
        trusted_issuers: vec!["jpki-test".to_string()],
        ..common::age::AgePolicy::default()
    }
}

/// 検証済み属性クレームを含む `/auth/verify` の本文。
fn verified_verify_body(nonce: &str, issuer: &str, audience: &str) -> String {
    serde_json::json!({
        "presentation_token": "mock_token",
        "nonce": nonce,
        "public_key_der": null,
        // 自己申告は成人として不十分な値。信頼属性経路では使われないことを検証する。
        "birth_date": "2010-01-01",
        "country_code": "JP",
        "verified_claims": {
            "birthdate": "2000-01-01",
            "country": "JP",
            "audience": audience,
            "attribute": {
                "issuer": issuer,
                "signed_subject": "age-over-20",
                "nonce": nonce
            }
        }
    })
    .to_string()
}

async fn post_verify(app: &Router, body: String) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// DEC-01-01: 正常系。検証済み属性は自己申告の生年月日より優先され、成人として受理される。
#[tokio::test]
async fn test_verified_age_accepts_valid_signed_attribute_in_mock_path() {
    let (app, _db) = setup_verified_age_app(trusted_age_policy()).await;
    let nonce = crate::issue_mock_nonce_for_test();
    let (status, body) = post_verify(&app, verified_verify_body(&nonce, "jpki-test", "")).await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["authorized"], true);

    // R2-USERHASH: issuer + signed_subject 由来の 64-hex で、旧 birthdate:country とは異なる。
    let user_hash = json["user_hash"]
        .as_str()
        .expect("user_hash must be issued on the verified path");
    assert_eq!(
        user_hash.len(),
        64,
        "user_hash must be a 64-character sha256 hex digest"
    );
    assert!(
        user_hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "user_hash must be hex only"
    );
    let legacy_hash = sha256_hex("2000-01-01:JP".as_bytes());
    assert_ne!(
        user_hash, legacy_hash,
        "legacy birthdate:country hash must not be reused"
    );

    // signed_subject を変えた2回目（別 nonce）で hash が変わる（pairwise 性）。
    let nonce2 = crate::issue_mock_nonce_for_test();
    let mut payload: serde_json::Value =
        serde_json::from_str(&verified_verify_body(&nonce2, "jpki-test", "")).unwrap();
    payload["verified_claims"]["attribute"]["signed_subject"] =
        serde_json::json!("age-over-20-pairwise");
    let (status, body2) = post_verify(&app, payload.to_string()).await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {body2}");
    let json2: serde_json::Value = serde_json::from_str(&body2).unwrap();
    assert_ne!(
        json2["user_hash"].as_str(),
        Some(user_hash),
        "a different signed_subject must derive a different user_hash"
    );
}

/// DEC-01-02: 自己申告の混入拒否。自己申告フィールドは検証済み属性へ昇格しない。
#[tokio::test]
async fn test_verified_age_rejects_self_reported_fields_and_missing_claims() {
    let (app, _db) = setup_verified_age_app(trusted_age_policy()).await;

    // 信頼属性必須モードで verified_claims が無ければ、成人の自己申告でも 403。
    let (status, _) = post_verify(&app, verify_body("1990-01-01", "JP")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // verified_claims へ自己申告の birth_date / country_code を混ぜた JSON は
    // deny_unknown_fields の serde エラーとして axum が 422 を返す。
    let nonce = crate::issue_mock_nonce_for_test();
    let mut payload: serde_json::Value =
        serde_json::from_str(&verified_verify_body(&nonce, "jpki-test", "")).unwrap();
    payload["verified_claims"]["birth_date"] = serde_json::json!("2000-01-01");
    payload["verified_claims"]["country_code"] = serde_json::json!("JP");
    let (status, _) = post_verify(&app, payload.to_string()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// DEC-01-03: nonce 再利用拒否。既存 `MOCK_NONCES.remove()` の単回消費を再利用する。
#[tokio::test]
async fn test_verified_age_rejects_reused_nonce() {
    let (app, _db) = setup_verified_age_app(trusted_age_policy()).await;
    let nonce = crate::issue_mock_nonce_for_test();
    let body = verified_verify_body(&nonce, "jpki-test", "");
    let (status, _) = post_verify(&app, body.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = post_verify(&app, body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a consumed nonce must not be reusable"
    );

    // 年齢拒否（403）経路でも nonce が消費済みとなることを固定する。
    // nonce 消費が年齢判定より後ろへ戻ると、この再提示が 200 になり回帰を検出できる。
    let underage_nonce = crate::issue_mock_nonce_for_test();
    let underage_body = verified_verify_body(&underage_nonce, "jpki-test", "").replace(
        "\"birthdate\":\"2000-01-01\"",
        "\"birthdate\":\"2010-01-01\"",
    );
    let (status, _) = post_verify(&app, underage_body).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "underage must be rejected");
    let (status, _) =
        post_verify(&app, verified_verify_body(&underage_nonce, "jpki-test", "")).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "nonce must be consumed even when age verification rejects the request"
    );
}

/// DEC-01-04: 未許可 issuer 拒否。
#[tokio::test]
async fn test_verified_age_rejects_untrusted_issuer() {
    let (app, _db) = setup_verified_age_app(trusted_age_policy()).await;
    let nonce = crate::issue_mock_nonce_for_test();
    let (status, _) = post_verify(&app, verified_verify_body(&nonce, "attacker.example", "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// DEC-01-05: audience 不一致拒否。OIDC 未設定時のフォールバックは空文字列。
#[tokio::test]
async fn test_verified_age_rejects_audience_mismatch() {
    let (app, _db) = setup_verified_age_app(trusted_age_policy()).await;
    let nonce = crate::issue_mock_nonce_for_test();
    let (status, _) = post_verify(
        &app,
        verified_verify_body(&nonce, "jpki-test", "other-client"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// DEC-01-06: allow_mock_auth 無効では verified_claims を受理しない。
#[tokio::test]
async fn test_verified_age_claims_rejected_when_mock_auth_disabled() {
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
        db: db.clone(),
        http_client: test_http_client(),
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
        age_policy: trusted_age_policy(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let app = build_router(state);
    let nonce = crate::issue_mock_nonce_for_test();
    let (status, _) = post_verify(&app, verified_verify_body(&nonce, "jpki-test", "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// DEC-01-07: R1 item9。既定モードは自己申告のままで、信頼属性必須へ自動昇格しない。
#[tokio::test]
async fn test_verified_age_default_mode_keeps_self_reported() {
    let (app, _db) = setup_verified_age_app(common::age::AgePolicy::default()).await;
    let (status, body) = post_verify(&app, verify_body("1990-01-01", "JP")).await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");
}

/// TZ 解決の純粋関数テスト。UTC 16:00 は JST では翌日 01:00 になる。
#[test]
fn test_today_for_policy_uses_configured_local_timezone() {
    let now = chrono::DateTime::parse_from_rfc3339("2024-01-01T16:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let utc_day = chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
    let jst_next_day = chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();

    assert_eq!(today_for_policy(now, None), utc_day);
    assert_eq!(today_for_policy(now, Some("UTC")), utc_day);
    assert_eq!(today_for_policy(now, Some("Etc/GMT+5")), utc_day);
    assert_eq!(today_for_policy(now, Some("Etc/GMT-9")), jst_next_day);
    assert_eq!(today_for_policy(now, Some(" Asia/Tokyo ")), jst_next_day);
    assert_eq!(today_for_policy(now, Some("Asia/Tokyo")), jst_next_day);
    // 未対応の TZ 名は現行どおり UTC にフォールバックする。
    assert_eq!(today_for_policy(now, Some("America/New_York")), utc_day);
    // 範囲外・数値なしの Etc/GMT はオフセット解決不可として UTC へフォールバックする。
    // doctor と同じ共有関数を直接検証し、境界（±25 / 数値なし）の差分が無いことを確認する。
    assert_eq!(crate::policy_utc_offset_hours(Some("Asia/Tokyo")), Some(9));
    assert_eq!(crate::policy_utc_offset_hours(Some("Etc/GMT-9")), Some(9));
    assert_eq!(crate::policy_utc_offset_hours(Some("Etc/GMT+5")), Some(-5));
    assert_eq!(crate::policy_utc_offset_hours(Some("Etc/GMT-25")), None);
    assert_eq!(crate::policy_utc_offset_hours(Some("Etc/GMT+25")), None);
    assert_eq!(crate::policy_utc_offset_hours(Some("Etc/GMT")), None);
    assert_eq!(today_for_policy(now, Some("Etc/GMT-25")), utc_day);
    assert_eq!(today_for_policy(now, Some("Etc/GMT+25")), utc_day);
    assert_eq!(today_for_policy(now, Some("Etc/GMT")), utc_day);
    // TZ 読込は common::env::read_env ベースの configured_tz に一本化されている。
    // プロセス環境を汚さず、read_env の分類と同じ結果になることを確認する。
    let configured = crate::configured_tz();
    let expected = match common::env::read_env("TZ") {
        common::env::EnvValue::Utf8(value) => Some(value),
        common::env::EnvValue::Missing | common::env::EnvValue::NonUtf8 => None,
    };
    assert_eq!(configured, expected);
}

/// 年齢境界がローカルの「今日」で判定されることの回帰テスト。
#[test]
fn test_validate_age_at_uses_local_today_for_age_boundary() {
    let now = chrono::DateTime::parse_from_rfc3339("2024-01-01T16:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let policy = common::age::AgePolicy::default(); // 既定の最低年齢は 21 歳

    // 2003-01-02 生まれは JST の 2024-01-02 では 21 歳、UTC の 2024-01-01 では 20 歳。
    assert!(
        validate_age_at(&policy, "2003-01-02", "JP", now, Some("Asia/Tokyo")).is_ok(),
        "JST の翌日基準では成人として受理されること"
    );
    assert!(
        validate_age_at(&policy, "2003-01-02", "JP", now, Some("Etc/GMT-9")).is_ok(),
        "Etc/GMT-9 (UTC+9) でも同じ境界になること"
    );
    assert!(matches!(
        validate_age_at(&policy, "2003-01-02", "JP", now, Some("UTC")),
        Err(AppError::Forbidden(_))
    ));
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
            Request::get("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111V")
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
        // 厳密形式検証を通過する有効な CIDv0 をステータスごとに使う。
        let cid = if status == "Reported" {
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111q"
        } else {
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111r"
        };
        db.execute(
            "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES (?, 'Illustration', 'test_user', 'unused', ?)",
            (cid, status),
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

    // IPFS I/O の間に status が Reported へ更新された場合も、取得開始時の
    // Approved 判定を使い回してバイトを返さない（レスポンス直前の再検証）。
    let body = b"mid-flight reported".to_vec();
    let checksum = sha256_hex(&body);
    let changed_cid = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111s";
    let gateway = MockGateway::start(MockResponse {
        status: StatusCode::OK,
        body,
        delay: Duration::from_millis(800),
        expected_cid: Some(changed_cid.to_string()),
        inflated_content_length: None,
    })
    .await;
    let (delayed_app, delayed_db) =
        setup_app_with_options(vec![gateway.template.clone()], Duration::from_secs(3), true).await;
    delayed_db
        .execute(
            "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES (?, 'Illustration', 'test_user', ?, 'Approved')",
            [changed_cid.to_string(), checksum],
        )
        .await
        .unwrap();
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let updater_db = Arc::clone(&delayed_db);
    let updater_cid = changed_cid.to_string();
    let updater = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        updater_db
            .execute(
                "UPDATE contents SET status = 'Reported' WHERE cid = ?",
                [updater_cid],
            )
            .await
            .unwrap();
    });
    let response = delayed_app
        .oneshot(with_peer(
            Request::get(format!("/content/{changed_cid}"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    updater.await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "status changed during IPFS I/O must not be served with the stale Approved decision"
    );
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

/// 定足数は単一定数へ集約され、値は従来どおり 3（同数 Rejected ルールは不変）。
#[test]
fn test_quorum_votes_constant() {
    assert_eq!(crate::QUORUM_VOTES, 3);
}

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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'Illustration', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'test')",
        (),
    )
    .await
    .unwrap();

    let state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    (build_router(state), db)
}

/// 認証済み運用者（sub=operator）で finalize エンドポイントを呼ぶ
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
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    assert_eq!(send_finalize(&app, 1).await, StatusCode::OK);

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status, "Reported",
        "finalize must not conclude before the deadline"
    );

    expire_report(&db, 1).await;

    // 手動 finalize は HTTP ハンドラのみ運用者限定。未登録 sub は 403 となり、
    // 期限到達後でも通報を確定できない（自動確定はスケジューラの別経路）。
    let forbidden = app
        .clone()
        .oneshot(with_peer(
            Request::post("/mod/reports/1/finalize")
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        generate_token(TEST_JWT_SECRET, "unregistered").await
                    ),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status, "Reported",
        "unregistered sub must not conclude an expired report"
    );

    assert_eq!(send_finalize(&app, 1).await, StatusCode::OK);

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T'", ())
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
        .query("SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        status, "Approved",
        "quorum-not-met reports must restore content"
    );

    let mut rows = db
        .query(
            "SELECT reason FROM content_audit_log WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T' ORDER BY log_id DESC LIMIT 1",
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
        send_report(
            &app,
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T",
            "Appeal: new evidence",
            // R2-AUTHZ: 異議は元通報者またはコンテンツ所有者のみ。
            // 本フィクスチャのコンテンツ所有者は "owner" なのでその sub で申し立てる。
            "owner"
        )
        .await,
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
            "SELECT reason FROM content_audit_log WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T' ORDER BY log_id DESC LIMIT 1",
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

/// R2-AUTHZ: 元通報者でもコンテンツ所有者でもない主体の異議は 403 で拒否する。
#[tokio::test]
async fn test_appeal_by_unauthorized_user_is_forbidden() {
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
        send_report(
            &app,
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T",
            "Appeal by a stranger",
            "stranger"
        )
        .await,
        StatusCode::FORBIDDEN,
        "only the original reporter or content owner may appeal"
    );

    let mut rows = db.query("SELECT COUNT(*) FROM reports", ()).await.unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1, "a rejected appeal must not create a report row");
}

/// R2-AUTHZ: 同一 CID の異議は 30 日以内に 1 回までとする。
#[tokio::test]
async fn test_second_appeal_within_30_days_is_rejected() {
    let (app, db) = setup_mod_app().await;
    // report 1 を確定させる。
    for _ in 0..3 {
        let (juror, key) = insert_juror(&db).await;
        assert_eq!(
            send_signed_vote(&app, 1, "Reject", &juror, &key).await,
            StatusCode::OK
        );
    }
    force_conclude(&db, 1).await;

    // 1 回目の異議（所有者による申し立て）は受理される。
    assert_eq!(
        send_report(
            &app,
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T",
            "First appeal",
            "owner"
        )
        .await,
        StatusCode::OK
    );

    // 異議（report 2）を確定させ、再度の異議を試みる。
    for _ in 0..3 {
        let (juror, key) = insert_juror(&db).await;
        assert_eq!(
            send_signed_vote(&app, 2, "Reject", &juror, &key).await,
            StatusCode::OK
        );
    }
    force_conclude(&db, 2).await;

    assert_eq!(
        send_report(
            &app,
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T",
            "Second appeal",
            "owner"
        )
        .await,
        StatusCode::FORBIDDEN,
        "a second appeal for the same CID within 30 days must be rejected"
    );

    let mut rows = db.query("SELECT COUNT(*) FROM reports", ()).await.unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        count, 2,
        "the cooled-down appeal must not create a report row"
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
        "UPDATE contents SET status = 'Rejected' WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T'",
        (),
    )
    .await
    .unwrap();

    assert_eq!(
        send_report(
            &app,
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T",
            "Fresh report",
            "reporter"
        )
        .await,
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

/// BAN で BAN 以前に発行済みの JWT / 未使用 refresh token が即失効し、
/// sub の revoked_subs 閾値が in-memory と DB の両方へ記録される。
#[tokio::test]
async fn test_ban_revokes_existing_sessions_and_refresh_tokens() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('target')", ())
        .await
        .unwrap();

    // BAN 前に発行された有効な JWT（24h）。
    let issued_at = chrono::Utc::now().timestamp() as usize;
    let target_token = encode(
        &Header::default(),
        &Claims {
            sub: "target".to_string(),
            exp: issued_at + 24 * 3600,
            sid: None,
            iat: Some(issued_at),
        },
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()).unwrap(),
    )
    .unwrap();

    // BAN 前は API を利用できる。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/content")
                .header("authorization", format!("Bearer {target_token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "BAN 前の JWT は有効");

    // BAN 前に発行された未使用 refresh token。
    let refresh = "target_unused_refresh_before_ban";
    db.execute(
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
         VALUES (?, 'target', 'target-family', '2099-01-01T00:00:00+00:00')",
        [hex::encode(Sha2Sha256::digest(refresh.as_bytes()))],
    )
    .await
    .unwrap();

    assert_eq!(
        send_mod_action(
            &app,
            "/mod/operators/target/ban",
            "operator",
            json!({"reason": "immediate revocation", "duration_days": 30}),
        )
        .await,
        StatusCode::OK
    );

    // 既存 JWT は失効閾値により拒否される（in-memory 記録の検証）。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/content")
                .header("authorization", format!("Bearer {target_token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "BAN 以前の JWT は即座に拒否される"
    );

    // 未使用 refresh token が DB 上で消費されている。
    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM refresh_tokens WHERE sub = 'target' AND used_at IS NULL",
            (),
        )
        .await
        .unwrap();
    let active: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(active, 0, "BAN で未使用 refresh token を消費する");
    let (status, _, _) = post_refresh(&app, refresh).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "BAN 以前の refresh token から延長できない"
    );

    // 再起動後も効く DB の失効閾値が記録されている。
    let mut rows = db
        .query(
            "SELECT revoked_at FROM revoked_subs WHERE sub = 'target'",
            (),
        )
        .await
        .unwrap();
    let row = rows
        .next()
        .await
        .unwrap()
        .expect("ban_user must record a revoked_subs threshold");
    let recorded: i64 = row.get(0).unwrap();
    let now = chrono::Utc::now().timestamp();
    assert!(
        (now - recorded).abs() <= 120,
        "revoked_at must be ~now: {recorded}"
    );
}

/// 回帰: ban_user はメモリ閾値の更新より前に DB 側の失効をコミットする。
/// メモリ lock が poison で失敗しても DB の閾値が残り、再起動後も失効が維持される。
#[tokio::test]
async fn test_ban_commits_db_threshold_before_memory_update() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('target')", ())
        .await
        .unwrap();

    let revoked_subs_before: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>> =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let poison_target = Arc::clone(&revoked_subs_before);
    let _ = std::thread::spawn(move || {
        let _guard = poison_target.lock().unwrap();
        panic!("poison revoked_subs_before");
    })
    .join();
    assert!(revoked_subs_before.is_poisoned());

    let state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: test_http_client(),
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
        revoked_subs_before,
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::from(["operator".to_string()])),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![HeaderValue::from_static("http://localhost:8080")],
    });
    // AuthClaims extractor は revoked_subs_before ではなく DB を読むため、
    // poison でハンドラへの到達が塞がることはない。ここでは ban_user を直接
    // 呼び、ban_user 自身がコミット後のメモリ更新で poison 失敗することと、
    // DB 側の閾値が先にコミット済みであることを検証する。
    let result = crate::ban_user(
        State(Arc::clone(&state)),
        crate::AuthClaims(Claims {
            sub: "operator".to_string(),
            exp: (chrono::Utc::now().timestamp() + 3600) as usize,
            sid: None,
            iat: Some(chrono::Utc::now().timestamp() as usize),
        }),
        Path("target".to_string()),
        Json(crate::BanRequest {
            reason: "restart window regression".to_string(),
            duration_days: None,
        }),
    )
    .await;

    // ADR-0013 P4: メモリ更新の poison は警告へ格下げし、DB 閾値のコミットと
    // 未使用リフレッシュトークン消費を必ず完了する（back-channel と同契約）。
    assert!(
        result.is_ok(),
        "poisoned cache must not block the committed revocation and token cleanup"
    );

    let mut rows = db
        .query("SELECT COUNT(*) FROM revoked_subs WHERE sub = 'target'", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        count, 1,
        "DB threshold must be committed before the in-memory update"
    );
}

/// ban_user のメモリ失効閾値が古い値で退行しないことを固定する。
/// 退行すると /auth/refresh が小さい iat を採番し、発行直後の JWT が失効する。
#[tokio::test]
async fn test_ban_memory_threshold_is_monotonic() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('target')", ())
        .await
        .unwrap();
    let before: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>> =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let state = Arc::new(AppState {
        db: Arc::new(db.clone()),
        http_client: test_http_client(),
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
        revoked_subs_before: Arc::clone(&before),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::from(["operator".to_string()])),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let claims = || {
        crate::AuthClaims(Claims {
            sub: "operator".to_string(),
            exp: (chrono::Utc::now().timestamp() + 3600) as usize,
            sid: None,
            iat: Some(chrono::Utc::now().timestamp() as usize),
        })
    };
    // 1回目: 大きい閾値を書き込む。
    before
        .lock()
        .unwrap()
        .insert("target".to_string(), 2_000_000_000);
    let _ = crate::ban_user(
        State(Arc::clone(&state)),
        claims(),
        Path("target".to_string()),
        Json(crate::BanRequest {
            reason: "first ban".to_string(),
            duration_days: None,
        }),
    )
    .await
    .unwrap();
    // 2回目: 現在時刻由来の小さい revoked_at でもメモリ閾値は退行しない。
    let result = crate::ban_user(
        State(Arc::clone(&state)),
        claims(),
        Path("target".to_string()),
        Json(crate::BanRequest {
            reason: "second ban".to_string(),
            duration_days: None,
        }),
    )
    .await;
    // DB 側 UPSERT が古い閾値を拒否するため、2回目はエラーになり得る。
    // いずれにせよメモリ閾値は最大値を保持しなければならない。
    let _ = result;
    let threshold = *before.lock().unwrap().get("target").unwrap();
    assert_eq!(
        threshold, 2_000_000_000,
        "in-memory revocation threshold must not regress"
    );
}

/// シグナル受信前は drain_timeout を超えてもサーバーが生存し続けることを固定する。
/// 旧実装 `timeout(drain_timeout, server)` へ戻すとこのテストが失敗する。
#[tokio::test]
async fn test_server_drain_timeout_applies_only_after_shutdown_signal() {
    let (_tx, rx) = tokio::sync::watch::channel(());
    let short_timeout = Duration::from_millis(20);

    // シグナル無し: 実際のサーバーは無期限に動く。ここでは 100ms の future を
    // 20ms の timeout と競わせ、シグナルが無ければ timeout が効かないことを検証する。
    let server = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok::<(), std::io::Error>(())
    };
    let mut pinned = Box::pin(server);
    let start = std::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_millis(200),
        crate::await_server_with_drain_timeout(pinned.as_mut(), rx, short_timeout),
    )
    .await
    .expect("outer guard must not fire");
    assert!(result.is_ok(), "no signal => no drain timeout");
    assert!(
        start.elapsed() >= Duration::from_millis(80),
        "server future must have been awaited to completion"
    );

    // シグナル有り: drain_timeout が適用され、server が完了しなければ timeout になる。
    let (tx2, rx2) = tokio::sync::watch::channel(());
    tx2.send(()).unwrap();
    let hanging = async { std::future::pending::<Result<(), std::io::Error>>().await };
    let mut pinned2 = Box::pin(hanging);
    let result = crate::await_server_with_drain_timeout(pinned2.as_mut(), rx2, short_timeout).await;
    assert!(result.is_err(), "signal received => drain timeout applies");
    assert_eq!(result.unwrap_err(), "drain timed out");
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
    let before_epoch = chrono::Utc::now().timestamp();

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

    // 行不存在フォールバック: is_banned=TRUE なのに revoked_subs 行が無い場合でも
    // UPDATE の影響件数を検査して同じ tx で閾値行を INSERT し、旧 JWT の復活を防ぐ。
    let mut rows = db
        .query(
            "SELECT revoked_at FROM revoked_subs WHERE sub = 'target'",
            (),
        )
        .await
        .unwrap();
    let row = rows
        .next()
        .await
        .unwrap()
        .expect("is_banned=TRUE で revoked_subs 行が無い場合も UNBAN が閾値行を作成すること");
    let revoked_at: i64 = row.get(0).unwrap();
    assert!(
        revoked_at >= before_epoch,
        "フォールバック INSERT は UNBAN 時刻以降の epoch を保持すること"
    );

    // 再送冪等性: 同一 UNBAN をもう一度送っても 200、監査は1行のまま、
    // revoked_subs の閾値は前進しない（再認証済みの有効セッションを守る）。
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
    let audit_count: i64 = db
        .query(
            "SELECT COUNT(*) FROM governance_audit_log WHERE action = 'UNBAN' AND target = 'target'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(
        audit_count, 1,
        "replayed UNBAN must not duplicate the audit row"
    );
    let current_epoch: i64 = db
        .query(
            "SELECT revoked_at FROM revoked_subs WHERE sub = 'target'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(
        current_epoch, revoked_at,
        "replayed UNBAN must not advance the revocation threshold"
    );
}

/// UNBAN は BAN 由来の revoked_subs 閾値を削除せず、sub epoch を現在時刻へ進める。
/// 旧 JWT（iat <= 旧閾値）は失効のまま残り、refresh も再認証まで拒否される。
/// back-channel logout 由来の閾値も消さずに保持する（DB 正本を検証）。
/// 関数名は既存契約の名前を維持したまま、oracle を epoch 前進へ更新している。
#[tokio::test]
async fn test_unban_clears_revoked_subs_threshold() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    db.execute(
        "INSERT INTO users (user_hash, is_banned) VALUES ('target', TRUE)",
        (),
    )
    .await
    .unwrap();
    let old_threshold = chrono::Utc::now().timestamp();
    db.execute(
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES ('target', ?)",
        [old_threshold],
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
            "SELECT revoked_at FROM revoked_subs WHERE sub = 'target'",
            (),
        )
        .await
        .unwrap();
    let epoch: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert!(
        epoch >= old_threshold,
        "UNBAN で失効閾値が削除されず epoch が進むこと"
    );
    assert!(
        crate::is_sub_revoked(&db, "target", Some(old_threshold as usize))
            .await
            .unwrap(),
        "旧 JWT の iat は UNBAN 後も失効したままであること"
    );
}

// ---- BAN 期限（ban_until）: duration_days から期限を設定し、判定側で時刻比較する ----

/// 期限付き BAN の陪審員で投票APIを呼ぶ。`None` は ban_until=NULL（無期限）。
async fn vote_status_with_ban_deadline(deadline_modifier: Option<&str>) -> StatusCode {
    let (app, db) = setup_mod_app().await;
    let (juror, key) = insert_juror(&db).await;
    match deadline_modifier {
        Some(modifier) => {
            db.execute(
                "UPDATE users SET is_banned = TRUE, ban_until = datetime('now', ?) WHERE user_hash = ?",
                [modifier.to_string(), juror.clone()],
            )
            .await
            .unwrap();
        }
        None => {
            db.execute(
                "UPDATE users SET is_banned = TRUE, ban_until = NULL WHERE user_hash = ?",
                [juror.clone()],
            )
            .await
            .unwrap();
        }
    }
    send_signed_vote(&app, 1, "Reject", &juror, &key).await
}

/// BAN 期限: duration_days=1 で ban_until が約1日後に設定される（DB確認）。
#[tokio::test]
async fn test_ban_sets_ban_until_for_duration() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('target')", ())
        .await
        .unwrap();

    assert_eq!(
        send_mod_action(
            &app,
            "/mod/operators/target/ban",
            "operator",
            json!({"reason": "timed ban", "duration_days": 1}),
        )
        .await,
        StatusCode::OK
    );

    let mut rows = db
        .query(
            "SELECT is_banned, banned_at, ban_until FROM users WHERE user_hash = 'target'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let banned: bool = row.get(0).unwrap();
    let banned_at: Option<String> = row.get(1).unwrap();
    let ban_until: Option<String> = row.get(2).unwrap();
    assert!(banned, "BAN 対象は is_banned=TRUE のまま");
    assert!(banned_at.is_some(), "banned_at は従来どおり設定される");

    let stored = ban_until.expect("duration_days 指定時は ban_until が設定される");
    let parsed = chrono::NaiveDateTime::parse_from_str(&stored, "%Y-%m-%d %H:%M:%S")
        .unwrap_or_else(|error| {
            panic!("ban_until は SQLite datetime() 形式であるべき: {stored}: {error}")
        });
    let expected = chrono::Utc::now().naive_utc() + chrono::Duration::days(1);
    assert!(
        (parsed - expected).num_seconds().abs() <= 120,
        "ban_until は約1日後であるべき: {stored}"
    );

    // 期限切れの期間付き BAN は is_banned=TRUE のまま残る。再 BAN が CAS で
    // 無効化されず、ban_until が未来へ更新され監査行が1件増えることを固定する。
    db.execute(
        "UPDATE users SET ban_until = datetime('now', '-1 day') WHERE user_hash = 'target'",
        (),
    )
    .await
    .unwrap();
    assert_eq!(
        send_mod_action(
            &app,
            "/mod/operators/target/ban",
            "operator",
            json!({"reason": "re-ban after expiry", "duration_days": 2}),
        )
        .await,
        StatusCode::OK
    );
    let mut rows = db
        .query("SELECT ban_until FROM users WHERE user_hash = 'target'", ())
        .await
        .unwrap();
    let renewed: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    let renewed = chrono::NaiveDateTime::parse_from_str(&renewed, "%Y-%m-%d %H:%M:%S").unwrap();
    assert!(
        renewed > chrono::Utc::now().naive_utc(),
        "expired ban must be renewed into the future: {renewed}"
    );
    let audit_count: i64 = db
        .query(
            "SELECT COUNT(*) FROM governance_audit_log WHERE target = 'target' AND action = 'BAN'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(audit_count, 2, "re-ban after expiry must be audited");
}

/// BAN 期限: 期限切れは is_banned=TRUE のままだが API 利用は妨げない。
#[tokio::test]
async fn test_expired_ban_does_not_block_user() {
    assert_eq!(
        vote_status_with_ban_deadline(Some("-1 day")).await,
        StatusCode::OK,
        "期限切れ BAN は判定上のみ解除される"
    );
}

/// BAN 期限: 未来の ban_until は従来どおり 403 で拒否される。
#[tokio::test]
async fn test_future_ban_until_blocks_user() {
    assert_eq!(
        vote_status_with_ban_deadline(Some("+1 day")).await,
        StatusCode::FORBIDDEN,
        "未来の ban_until を持つ BAN は有効"
    );
}

/// BAN 期限: ban_until=NULL（無期限）の BAN は今も 403（回帰）。
#[tokio::test]
async fn test_indefinite_ban_still_blocks_user() {
    assert_eq!(
        vote_status_with_ban_deadline(None).await,
        StatusCode::FORBIDDEN,
        "ban_until=NULL は無期限 BAN として扱われる"
    );
}

/// BAN 期限: UNBAN で ban_until が NULL に戻る。
#[tokio::test]
async fn test_unban_clears_ban_until() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    db.execute("INSERT INTO users (user_hash) VALUES ('target')", ())
        .await
        .unwrap();

    assert_eq!(
        send_mod_action(
            &app,
            "/mod/operators/target/ban",
            "operator",
            json!({"reason": "timed ban", "duration_days": 30}),
        )
        .await,
        StatusCode::OK
    );
    let mut rows = db
        .query("SELECT ban_until FROM users WHERE user_hash = 'target'", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let before: Option<String> = row.get(0).unwrap();
    assert!(before.is_some(), "BAN 時に ban_until が設定される");

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
            "SELECT is_banned, banned_at, ban_until FROM users WHERE user_hash = 'target'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let banned: bool = row.get(0).unwrap();
    let banned_at: Option<String> = row.get(1).unwrap();
    let ban_until: Option<String> = row.get(2).unwrap();
    assert!(!banned);
    assert!(banned_at.is_none());
    assert!(ban_until.is_none(), "UNBAN で ban_until もクリアされる");
}

// ---- BAN 済み Juror の票を定足数集計から除外する（敵対的レビュー指摘の修正） ----

#[derive(Debug)]
struct ConclusionOutcome {
    status: String,
    reason: String,
    stored_votes: i64,
}

/// 投票後に Juror を BAN して確定させ、結果・監査理由・保存済み票数を返す。
/// `ban_until_modifier`: None = 無期限 BAN, "+1 day" = 有効な期限付き, "-1 day" = 期限切れ。
async fn conclude_after_banning_juror(
    vote_results: &[&str],
    banned_index: usize,
    ban_until_modifier: Option<&str>,
) -> ConclusionOutcome {
    let (app, db) = setup_mod_app().await;
    let mut jurors = Vec::new();
    for result in vote_results {
        let (juror, key) = insert_juror(&db).await;
        assert_eq!(
            send_signed_vote(&app, 1, result, &juror, &key).await,
            StatusCode::OK,
            "vote must be accepted before the juror is banned"
        );
        jurors.push(juror);
    }

    let banned = &jurors[banned_index];
    match ban_until_modifier {
        Some(modifier) => {
            db.execute(
                "UPDATE users SET is_banned = TRUE, ban_until = datetime('now', ?) WHERE user_hash = ?",
                [modifier.to_string(), banned.clone()],
            )
            .await
            .unwrap();
        }
        None => {
            db.execute(
                "UPDATE users SET is_banned = TRUE, ban_until = NULL WHERE user_hash = ?",
                [banned.clone()],
            )
            .await
            .unwrap();
        }
    }

    force_conclude(&db, 1).await;

    let mut rows = db
        .query(
            "SELECT reason FROM content_audit_log
             WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T'
             ORDER BY log_id DESC LIMIT 1",
            (),
        )
        .await
        .unwrap();
    let reason: String = rows.next().await.unwrap().unwrap().get(0).unwrap();

    let mut rows = db
        .query(
            "SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T'",
            (),
        )
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();

    let mut rows = db
        .query("SELECT COUNT(*) FROM votes WHERE report_id = 1", ())
        .await
        .unwrap();
    let stored_votes: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();

    ConclusionOutcome {
        status,
        reason,
        stored_votes,
    }
}

#[tokio::test]
async fn test_vote_after_ban_excluded_from_conclusion_tally() {
    // 投票後に無期限 BAN された Juror の票は集計から除外される。
    // BAN 前なら 3票（Reject 2 > Approve 1）で Rejected になるが、
    // 除外後は 2票で定足数未達 → Approved に復帰する。
    let outcome = conclude_after_banning_juror(&["Approve", "Reject", "Reject"], 1, None).await;
    assert_eq!(outcome.stored_votes, 3, "票自体は DB に保存されていること");
    assert_eq!(
        outcome.status, "Approved",
        "BAN 済み票を除くと定足数未達になること"
    );
    assert!(
        outcome.reason.contains("quorum not met: 2 votes (need 3)"),
        "BAN 済み票が集計から除外されていること: {}",
        outcome.reason
    );
}

#[tokio::test]
async fn test_three_votes_with_one_banned_fails_quorum() {
    // 3票中1票が有効期限内の BAN（未来の ban_until）→ 実効2票で定足数未達。
    let outcome =
        conclude_after_banning_juror(&["Reject", "Reject", "Reject"], 2, Some("+1 day")).await;
    assert_eq!(outcome.stored_votes, 3, "3票とも保存されていること");
    assert_eq!(
        outcome.status, "Approved",
        "定足数未達なら Approved に復帰すること"
    );
    assert!(
        outcome.reason.contains("quorum not met: 2 votes (need 3)"),
        "有効な期限付き BAN も除外されること: {}",
        outcome.reason
    );
}

#[tokio::test]
async fn test_expired_ban_vote_still_counted_in_conclusion() {
    // ban_until が過去の BAN は判定上解除済み → 3票すべて集計され Rejected。
    let outcome =
        conclude_after_banning_juror(&["Reject", "Reject", "Reject"], 0, Some("-1 day")).await;
    assert_eq!(outcome.stored_votes, 3);
    assert_eq!(
        outcome.status, "Rejected",
        "期限切れ BAN の票は引き続き集計されること"
    );
    assert!(
        outcome.reason.contains("3 Reject"),
        "全3票が集計されていること: {}",
        outcome.reason
    );
    assert!(
        !outcome.reason.contains("quorum not met"),
        "期限切れ BAN で定足数未達になってはいけない: {}",
        outcome.reason
    );
}

#[tokio::test]
async fn test_mod_stats_quorum_excludes_banned_juror_votes() {
    // /mod/stats の定足数判定も process_report_conclusion と同じ基準に揃える。
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    let mut jurors = Vec::new();
    for _ in 0..3 {
        let (juror, key) = insert_juror(&db).await;
        assert_eq!(
            send_signed_vote(&app, 1, "Reject", &juror, &key).await,
            StatusCode::OK
        );
        jurors.push(juror);
    }
    db.execute(
        "UPDATE users SET is_banned = TRUE, ban_until = NULL WHERE user_hash = ?",
        [jurors[0].clone()],
    )
    .await
    .unwrap();
    // 月次集計は concluded_at を見るため、確定済みとしてマークする。
    db.execute(
        "UPDATE reports SET concluded_at = '2026-01-01 00:00:00' WHERE report_id = 1",
        (),
    )
    .await
    .unwrap();

    let response = send_mod_stats_request(&app, Some("operator")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let stats: ModStatsResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(stats.totals.concluded, 1);
    assert_eq!(
        stats.totals.quorum_not_met, 1,
        "BAN 済み票を除いた2票は定足数未達として集計されること"
    );
}

#[tokio::test]
async fn test_unpin_records_decision_with_audit() {
    let (app_no_op, _) = setup_mod_app().await;
    assert_eq!(
        send_mod_action(
            &app_no_op,
            "/mod/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T/unpin",
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
            "/mod/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T/unpin",
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
            "/mod/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111k/unpin",
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
    assert_eq!(target, "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T");
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
    // ADR-0013 P4: REQUEST_TIMEOUT_SECS は drain と同じ既定・範囲を共有する。
    assert_eq!(crate::request_timeout_secs(None), 30);
    assert_eq!(crate::request_timeout_secs(Some("1")), 1);
    assert_eq!(crate::request_timeout_secs(Some("300")), 300);
    assert_eq!(
        crate::request_timeout_secs(Some("0")),
        30,
        "zero must fall back"
    );
    assert_eq!(
        crate::request_timeout_secs(Some("301")),
        30,
        "over the maximum must fall back"
    );
    assert_eq!(crate::request_timeout_secs(Some("bad")), 30);
    assert_eq!(
        crate::request_timeout_secs_from_env(common::env::EnvValue::Missing),
        30
    );
    assert_eq!(
        crate::request_timeout_duration(Some("5")),
        Duration::from_secs(5)
    );
}

#[tokio::test]
async fn test_finalize_due_reports_skips_before_deadline() {
    let (_app, db) = setup_mod_app().await;
    let count = crate::finalize_due_reports(&db).await.unwrap();
    assert_eq!(count, 0, "a fresh report must not be auto-finalized");

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T'", ())
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
        .query("SELECT status FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T'", ())
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111c', 'Novel', 'owner', 'def', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason, deadline_at)
             VALUES (2, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111c', 'second', datetime('now', '-1 day'))",
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
                let lock = crate::write_lock(&db).await.unwrap();
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111F', 'Novel', 'owner', 'abc', 'Reported')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason, deadline_at)
             VALUES (2, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111F', 'due', datetime('now', '-1 day'))",
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
    assert_eq!(crate::parse_instance_lock(&contents).0, std::process::id());

    // 読取失敗（存在しない先を指す壊れた symlink）は stale とみなさず拒否（fail-closed）。
    // root では chmod 000 が効かないため symlink で読取エラーを再現する。
    std::fs::remove_file(&lock).unwrap();
    std::os::unix::fs::symlink("missing-target", &lock).unwrap();
    assert!(crate::acquire_instance_lock(lock.to_str().unwrap()).is_err());
    // 読取失敗時にロックを削除していないこと（symlink が残っていること）を確認する。
    assert!(std::fs::symlink_metadata(&lock).is_ok());
    assert_eq!(
        std::fs::read_link(&lock).unwrap(),
        std::path::Path::new("missing-target")
    );

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
    assert_eq!(crate::parse_instance_lock(&contents).0, std::process::id());

    let _ = std::fs::remove_dir_all(&dir);
}

/// ロックファイルの Newtype パース（旧形式 PID のみ／新形式 PID+starttime）。
#[test]
fn test_instance_lock_parse_formats() {
    assert_eq!(crate::parse_instance_lock("123\n"), (123, None));
    assert_eq!(crate::parse_instance_lock("123\n456\n"), (123, Some(456)));
    assert_eq!(crate::parse_instance_lock("bad\n"), (0, None));
    assert_eq!(crate::parse_instance_lock(""), (0, None));
}

/// 生存 PID だが starttime が異なるロックは stale として回収できる（PID 再利用対策）。
#[test]
fn test_instance_lock_rejects_reused_pid_with_different_starttime() {
    let dir = std::env::temp_dir().join(format!(
        "openillustration_reused_pid_lock_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("instance.db.instance.lock");

    let starttime = crate::read_proc_starttime(std::process::id()).unwrap_or(1);
    std::fs::write(
        &lock,
        format!("{}\n{}\n", std::process::id(), starttime.wrapping_add(1)),
    )
    .unwrap();
    assert!(
        crate::acquire_instance_lock(lock.to_str().unwrap()).is_ok(),
        "a mismatched starttime must be treated as stale"
    );

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
    assert_eq!(purged, Some(1), "only the expired token must be purged");

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

/// refresh purge は SQLite の datetime() で正規化して比較するため、
/// タイムゾーン表記の差で辞書順が逆転しても正しく回収／保持される。
#[tokio::test]
async fn test_purge_expired_refresh_tokens_normalizes_timezone() {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;

    let now = chrono::Utc::now();
    let east = chrono::FixedOffset::east_opt(9 * 3600).unwrap();
    let west = chrono::FixedOffset::west_opt(9 * 3600).unwrap();

    // 実時刻は 2 時間前（期限切れ）だが +09:00 表記の壁時計は now より後に来るため、
    // 文字列比較なら回収されない。
    let expired = (now - chrono::Duration::hours(2))
        .with_timezone(&east)
        .to_rfc3339();
    // 実時刻は 2 時間後（有効）だが -09:00 表記の壁時計は now より前に来るため、
    // 文字列比較なら誤って回収される。
    let valid = (now + chrono::Duration::hours(2))
        .with_timezone(&west)
        .to_rfc3339();

    for (token_hash, expires_at) in [
        ("expired-tz", expired.as_str()),
        ("valid-tz", valid.as_str()),
    ] {
        db.execute(
            "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
             VALUES (?, 'u', 'f', ?)",
            (token_hash, expires_at),
        )
        .await
        .unwrap();
    }

    let purged = crate::purge_expired_refresh_tokens(&db).await.unwrap();
    assert_eq!(
        purged,
        Some(1),
        "only the actually expired token must be purged"
    );

    let mut rows = db
        .query("SELECT token_hash FROM refresh_tokens", ())
        .await
        .unwrap();
    let mut remaining = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        remaining.push(row.get::<String>(0).unwrap());
    }
    assert_eq!(remaining, vec!["valid-tz"]);
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
    // 保持期間は refresh TTL（30 日）+ マージンへ延長済み。
    db.execute(
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES ('old', ?)",
        [now - 30 * 24 * 60 * 60 - 3600],
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
    assert_eq!(purged, Some(1), "only the stale revocation must be purged");

    let mut rows = db.query("SELECT sub FROM revoked_subs", ()).await.unwrap();
    let mut remaining = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        remaining.push(row.get::<String>(0).unwrap());
    }
    assert_eq!(remaining, vec!["recent"]);
}

/// revoked_subs の保持期間はリフレッシュトークン TTL（30 日）+ マージン。
/// セッション TTL（24h）を超えても refresh が有効な間は閾値を残し、
/// 30 日を超えた古い閾値だけを回収する境界を検証する。
#[tokio::test]
async fn test_purge_stale_revoked_subs_keeps_refresh_window() {
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
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES ('within_refresh', ?)",
        [now - 25 * 60 * 60],
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES ('beyond_retention', ?)",
        [now - 31 * 24 * 60 * 60],
    )
    .await
    .unwrap();

    let purged = crate::purge_stale_revoked_subs(&db).await.unwrap();
    assert_eq!(purged, Some(1), "30 日を超えた閾値だけ回収されること");

    let mut rows = db.query("SELECT sub FROM revoked_subs", ()).await.unwrap();
    let mut remaining = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        remaining.push(row.get::<String>(0).unwrap());
    }
    assert_eq!(remaining, vec!["within_refresh"]);
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

    // メモリの positive-only キャッシュにも同じ sid を入れておき、purge 成功時に
    // 同じカットオフの sid だけが反映的に削除されることを固定する。
    let cache = Arc::new(std::sync::Mutex::new(std::collections::HashSet::from([
        "old".to_string(),
        "recent".to_string(),
        "other-instance".to_string(),
    ])));
    let purged = crate::purge_stale_revoked_sessions_with_cache(&db, None, Some(cache.as_ref()))
        .await
        .unwrap();
    assert_eq!(purged, Some(1), "only the stale revocation must be purged");

    let mut rows = db
        .query("SELECT sid FROM revoked_sessions", ())
        .await
        .unwrap();
    let mut remaining = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        remaining.push(row.get::<String>(0).unwrap());
    }
    assert_eq!(remaining, vec!["recent"]);

    let cached = cache.lock().unwrap();
    assert!(
        !cached.contains("old"),
        "purge した sid はメモリからも消える"
    );
    assert!(cached.contains("recent"), "生存 sid はメモリに残る");
    assert!(
        cached.contains("other-instance"),
        "DB の削除対象ではない sid は維持する"
    );
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
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
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

/// expires_at は文字列ではなく RFC3339 の時刻として比較する。
/// 小数秒の桁数や `Z` / `+00:00` の違いで辞書順が逆転しても誤判定しないこと。
#[test]
fn test_refresh_token_expired_parses_rfc3339() {
    fn at(value: &str) -> chrono::DateTime<chrono::Utc> {
        value.parse::<chrono::DateTime<chrono::Utc>>().unwrap()
    }

    // 同一時刻・小数秒表現違いでも <= により期限切れ（辞書順では "…00+00:00"
    // より "…00.000+00:00" が後に来るため、文字列比較なら誤って有効と判定される）。
    assert!(crate::oidc::refresh_token_expired(
        "2026-01-01T00:00:00.000+00:00",
        at("2026-01-01T00:00:00+00:00")
    ));

    // `Z` は数字より ASCII コードが大きいため辞書順が逆転する例。
    // 時刻としては 1ns 前なので期限切れが正しい。
    assert!(crate::oidc::refresh_token_expired(
        "2026-01-01T00:00:00.000Z",
        at("2026-01-01T00:00:00.000000001+00:00")
    ));

    // 小数秒を含む未来時刻は有効。境界より前（= まだ期限切れでない）も有効。
    assert!(!crate::oidc::refresh_token_expired(
        "2026-01-01T00:00:01.123456789+00:00",
        at("2026-01-01T00:00:00+00:00")
    ));
    assert!(!crate::oidc::refresh_token_expired(
        "2026-01-01T00:00:00.000000001+00:00",
        at("2026-01-01T00:00:00+00:00")
    ));

    // パースできない値は fail-closed（期限切れ扱い）。
    assert!(crate::oidc::refresh_token_expired(
        "not-a-rfc3339-date",
        at("2026-01-01T00:00:00+00:00")
    ));
    assert!(crate::oidc::refresh_token_expired(
        "",
        at("2026-01-01T00:00:00+00:00")
    ));
}

/// R2 契約: 発行処理は `parse_from_rfc3339` 可能な RFC3339 を `expires_at` へ書く。
///
/// migration 006 は `expires_at DATETIME` と宣言するが、SQLite の DATETIME は
/// 型 affinity であり、実際の格納値は `chrono::to_rfc3339()` の TEXT である。
/// マイグレーションの型宣言から SQL datetime 形式を推測してはならない。
/// マイグレーション自体は SHA-256 チェックサム保護のため編集しない。
#[tokio::test]
async fn test_issued_refresh_token_expires_at_is_rfc3339() {
    let (app, oidc, mock, db) = setup_app_with_oidc().await;
    let (_body, refresh) = oidc_login_with_refresh(&app, &oidc, &mock).await;
    let hash = hex::encode(Sha2Sha256::digest(refresh.as_bytes()));

    let mut rows = db
        .query(
            "SELECT expires_at FROM refresh_tokens WHERE token_hash = ?",
            [hash],
        )
        .await
        .unwrap();
    let row = rows
        .next()
        .await
        .unwrap()
        .expect("issued refresh token must be stored");
    let expires_at: String = row.get(0).unwrap();
    let parsed = chrono::DateTime::parse_from_rfc3339(&expires_at)
        .unwrap_or_else(|e| panic!("issued expires_at must be RFC3339: {expires_at}: {e}"));
    assert!(
        parsed.with_timezone(&chrono::Utc) > chrono::Utc::now(),
        "newly issued refresh token must not be expired"
    );
}

/// 期限切れの refresh token（小数秒付き）は /auth/refresh で拒否される。
#[tokio::test]
async fn test_refresh_rejects_expired_token_with_fractional_seconds() {
    let (app, _oidc, _mock, db) = setup_app_with_oidc().await;
    let token = "expired-fractional-token";
    let hash = hex::encode(Sha2Sha256::digest(token.as_bytes()));
    db.execute(
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
         VALUES (?, ?, ?, '2020-01-01T00:00:00.123456789+00:00')",
        (hash, "expired-user", "expired-family"),
    )
    .await
    .unwrap();

    let (status, _, body) = post_refresh(&app, token).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an expired refresh token must be rejected: {body}"
    );
}

/// refresh_token Cookie が同一名で重複した場合は攻撃の兆候として 401 にする。
/// 旧 find_map の先勝ちだと攻撃者値が先に来たときに正規値へすり替えられる。
#[tokio::test]
async fn test_refresh_rejects_duplicate_refresh_cookies() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    let (_login, refresh) = oidc_login_with_refresh(&app, &oidc, &mock).await;

    const ATTACKER: &str = "attacker-controlled-refresh";

    // 単一ヘッダ内で同一名が重複。順序に関わらず 401 になることを検証する。
    for cookies in [
        format!("refresh_token={ATTACKER}; refresh_token={refresh}"),
        format!("refresh_token={refresh}; refresh_token={ATTACKER}"),
    ] {
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::post("/auth/refresh")
                    .header(header::COOKIE, cookies.as_str())
                    .header(header::ORIGIN, "http://localhost:8080")
                    .body(Body::empty())
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains(ATTACKER), "token 値をエコーしない: {text}");
        assert!(
            !text.contains(refresh.as_str()),
            "token 値をエコーしない: {text}"
        );
    }

    // 重複を拒否しただけなので正規トークンは未消費のまま、通常どおり成功する。
    let (status, _, _) = post_refresh(&app, &refresh).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "重複拒否で正規トークンは消費されない"
    );
}

/// refresh_token Cookie が別々の Cookie ヘッダに分かれて重複しても 401 にする。
/// 旧コードは headers.get() で先頭ヘッダしか見ていなかった。
#[tokio::test]
async fn test_refresh_rejects_duplicate_refresh_cookie_headers() {
    let (app, oidc, mock, _db) = setup_app_with_oidc().await;
    let (_login, refresh) = oidc_login_with_refresh(&app, &oidc, &mock).await;

    // 正規値を先頭ヘッダ、攻撃者値を第2ヘッダに置く。旧コードは先頭のみを
    // 見て 200 になっていたが、全ヘッダを走査して 401 になることを検証する。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/refresh")
                .header(header::COOKIE, format!("refresh_token={refresh}"))
                .header(header::COOKIE, "refresh_token=attacker-controlled-refresh")
                .header(header::ORIGIN, "http://localhost:8080")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// /auth/refresh へリクエストし、(status, _new_refresh_cookie, body) を返す。
async fn post_refresh(app: &Router, token: &str) -> (StatusCode, String, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/refresh")
                .header(header::COOKIE, format!("refresh_token={token}"))
                .header(header::ORIGIN, "http://localhost:8080")
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

    // R2-REFRESH-ORIGIN: 許可外 Origin / cross-site は Cookie より先に 401。
    // どちらも拒否段階なので refresh トークンは消費されず、後の成功経路に影響しない。
    for (origin, fetch_site) in [
        ("https://evil.example.com", None),
        ("http://localhost:8080", Some("cross-site")),
    ] {
        let mut request = Request::post("/auth/refresh")
            .header(header::COOKIE, format!("refresh_token={refresh}"))
            .header(header::ORIGIN, origin);
        if let Some(site) = fetch_site {
            request = request.header("sec-fetch-site", site);
        }
        let response = app
            .clone()
            .oneshot(with_peer(
                request.body(Body::empty()).unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/refresh")
                .header(header::COOKIE, format!("refresh_token={refresh}"))
                .header(header::ORIGIN, "http://localhost:8080")
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

    // 原子回転: 提示した旧トークンは消費済み、新トークンは未使用で登録される。
    let old_hash = hex::encode(Sha2Sha256::digest(refresh.as_bytes()));
    let new_hash = hex::encode(Sha2Sha256::digest(new_refresh.as_bytes()));
    for (hash, expected_consumed, label) in [
        (old_hash.as_str(), true, "old"),
        (new_hash.as_str(), false, "new"),
    ] {
        let mut rows = db
            .query(
                "SELECT used_at FROM refresh_tokens WHERE token_hash = ?",
                [hash],
            )
            .await
            .unwrap();
        let used_at: Option<String> = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(
            used_at.is_some(),
            expected_consumed,
            "{label} refresh token used_at consumption state"
        );
    }
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

    // R2-AUTH item2: JWT の sub と Cookie の refresh_tokens.sub が不一致なら
    // ファミリを更新せず 401 を返す。別 sub / 別 family の行を仕込んで検証する。
    let foreign = "foreign-sub-refresh-token";
    db.execute(
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
         VALUES (?, ?, ?, ?)",
        (
            sha256_hex(foreign.as_bytes()),
            "foreign-sub".to_string(),
            "foreign-family".to_string(),
            (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339(),
        ),
    )
    .await
    .unwrap();
    let mismatch = app
        .clone()
        .oneshot(with_peer(
            Request::post("/auth/logout")
                .header("authorization", format!("Bearer {token}"))
                .header(header::COOKIE, format!("refresh_token={foreign}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(
        mismatch.status(),
        StatusCode::UNAUTHORIZED,
        "refresh token with a different sub must not be accepted for logout"
    );
    // ADR-0013 P4 item5: sub 不一致の 401 でも refresh_token 削除 Cookie を付与する。
    let mismatch_cookies: Vec<String> = mismatch
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok().map(str::to_owned))
        .collect();
    let deletion = mismatch_cookies
        .iter()
        .find(|cookie| cookie.starts_with("refresh_token="))
        .expect("sub mismatch must still delete the refresh cookie");
    assert!(deletion.starts_with("refresh_token=;"), "{deletion}");
    assert!(deletion.contains("Max-Age=0"), "{deletion}");
    assert!(
        !deletion.contains(foreign),
        "the mismatched token must not be echoed"
    );
    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM refresh_tokens WHERE family_id = ? AND used_at IS NULL",
            ["foreign-family".to_string()],
        )
        .await
        .unwrap();
    let active: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        active, 1,
        "mismatched sub must not revoke the foreign family"
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
        active, 2,
        "the real family must also stay active on mismatch"
    );

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
            "SELECT COUNT(*) FROM refresh_tokens WHERE sub = ? AND used_at IS NULL",
            [login["user_hash"].as_str().unwrap().to_string()],
        )
        .await
        .unwrap();
    let active: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(active, 0, "logout must revoke the matching family");
    // 不一致側のファミリは最後まで未消費のまま（更新されなかった証拠）。
    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM refresh_tokens WHERE family_id = ? AND used_at IS NULL",
            ["foreign-family".to_string()],
        )
        .await
        .unwrap();
    let active: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(active, 1, "mismatched family must remain untouched");
}

/// back-channel logout の sub 失効は、失効前に発行された未使用 refresh token
/// も DB 上で消費するため、そのトークンからの refresh は 401 になる。
#[tokio::test]
async fn test_backchannel_logout_consumes_unused_refresh_tokens() {
    let (app, oidc, mock, db) = setup_app_with_oidc().await;
    let (login, refresh) = oidc_login_with_refresh(&app, &oidc, &mock).await;
    let sub = login["user_hash"].as_str().unwrap().to_string();

    let claims = jsonwebtoken::decode::<common::Claims>(
        login["token"].as_str().unwrap(),
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

    // 未使用トークンが DB に残っていないこと（再起動後も効く永続状態）。
    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM refresh_tokens WHERE sub = ? AND used_at IS NULL",
            [sub.clone()],
        )
        .await
        .unwrap();
    let active: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        active, 0,
        "logout must consume every unused token for the sub"
    );

    let (status, _, _) = post_refresh(&app, &refresh).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a token issued before logout must not refresh the session"
    );
}

/// 回帰: backchannel_logout の sub 失効は refresh 消費より前に DB 閾値をコミットする。
/// refresh 側の書き込みが失敗しても DB の閾値は残り、再起動後も失効が維持される。
#[tokio::test]
async fn test_backchannel_logout_commits_db_threshold_before_refresh_revocation() {
    let (app, oidc, mock, db) = setup_app_with_oidc().await;
    let (login, _refresh) = oidc_login_with_refresh(&app, &oidc, &mock).await;
    let claims = jsonwebtoken::decode::<common::Claims>(
        login["token"].as_str().unwrap(),
        &jsonwebtoken::DecodingKey::from_secret(TEST_JWT_SECRET.as_bytes()).unwrap(),
        &jsonwebtoken::Validation::default(),
    )
    .unwrap()
    .claims;
    let sub = claims.sub.clone();
    let logout_token = mock.logout_token_for_sub(&sub);

    // refresh 消費を失敗させ、DB 閾値が先に確定していることを確認する。
    db.execute("DROP TABLE refresh_tokens", ()).await.unwrap();

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
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let mut rows = db
        .query("SELECT COUNT(*) FROM revoked_subs WHERE sub = ?", [sub])
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        count, 1,
        "DB threshold must be committed before refresh revocation"
    );
}

/// 再起動相当（in-memory の失効閾値が空）でも、DB の sub 失効閾値が残っていれば
/// refresh を拒否する。未使用トークンが競合で残った場合の多層防御。
#[tokio::test]
async fn test_refresh_rejects_revoked_sub_with_empty_in_memory_state() {
    let (app, oidc, mock, db) = setup_app_with_oidc().await;
    let (login, _refresh) = oidc_login_with_refresh(&app, &oidc, &mock).await;
    let claims = jsonwebtoken::decode::<common::Claims>(
        login["token"].as_str().unwrap(),
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

    // 失効と競合して未使用のまま残ったトークンを模擬する。
    let escaped = "escaped-refresh-token";
    let hash = hex::encode(Sha2Sha256::digest(escaped.as_bytes()));
    db.execute(
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
         VALUES (?, ?, ?, '2999-01-01T00:00:00Z')",
        (hash, claims.sub.clone(), "escaped-family"),
    )
    .await
    .unwrap();

    // プロセス再起動相当: メモリの失効表は空だが DB は同じものを共有する。
    let state = Arc::new(AppState {
        db: db.clone(),
        http_client: test_http_client(),
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
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![HeaderValue::from_static("http://localhost:8080")],
    });
    let restarted = build_router(state);

    let (status, _, _) = post_refresh(&restarted, escaped).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "DB-persisted sub revocation must block refresh after restart"
    );
}

#[tokio::test]
async fn test_refresh_rejects_missing_cookie() {
    let (app, _oidc, _mock, _db) = setup_app_with_oidc().await;
    let response = app
        .oneshot(with_peer(
            Request::post("/auth/refresh")
                .header(header::ORIGIN, "http://localhost:8080")
                .body(Body::empty())
                .unwrap(),
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
                "cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z",
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

/// price が NULL の Approved 行は一覧と同じく 0 として扱い、
/// 価格不正（購入不可）として 400 を返す。生 price を読むと NULL の
/// i64 変換失敗で 500 になるため、その回帰を検証する。
#[tokio::test]
async fn test_order_rejects_null_price_as_bad_request() {
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) =
        setup_app_with_payments("0x1111111111111111111111111111111111111111", verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let cid = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z";

    // fixture の Approved 行を NULL price に変更する（一覧側の NULL ケースと同じ状態）。
    db.execute("UPDATE contents SET price = NULL WHERE cid = ?", [cid])
        .await
        .unwrap();

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
                        "cid": cid,
                        "amount_jpy": 1000,
                    })
                    .to_string(),
                ))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    let status = response.status();
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "NULL price must be rejected as a bad request, not 500: {body}"
    );
    assert_eq!(body["error"], "amount_jpy must match the content price");
}

#[tokio::test]
async fn test_order_rejects_unknown_or_non_approved_content() {
    let verifier = Arc::new(MockVerifier::default());
    let (app, db) =
        setup_app_with_payments("0x1111111111111111111111111111111111111111", verifier).await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status, price)
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111b', 'Novel', 'test_user', 'abc', 'Reported', 1000)",
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
    unknown["cid"] = json!("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111m");
    assert_eq!(
        post_order_json(&app, &token, unknown).await,
        StatusCode::NOT_FOUND
    );

    let mut reported = base;
    reported["cid"] = json!("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111b");
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
                        "cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z",
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
    assert_eq!(
        order["cid"],
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111Z"
    );
}

// ---- メトリクス ----

fn read_metric(body: &str, name: &str) -> u64 {
    body.lines()
        .find_map(|line| line.strip_prefix(name).map(str::trim))
        .unwrap_or_else(|| panic!("metric {name} not found"))
        .parse()
        .unwrap()
}

/// `/metrics` から HTTP カウンタ（requests, errors）を一組で読み取る。
/// `/metrics` 自体は observe_metrics の除外対象なので、この読み取りはカウンタを増やさない。
async fn read_http_metrics(app: &Router) -> (u64, u64) {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/metrics").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(response.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    (
        read_metric(&body, "openillustration_http_requests_total"),
        read_metric(&body, "openillustration_http_errors_total"),
    )
}

/// 現在の中立化 URL カウンタ値を `/metrics` から読み取る。
async fn read_neutralized_metric(app: &Router) -> u64 {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/metrics").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(response.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    read_metric(&body, "openillustration_neutralized_urls_total")
}

async fn read_ui_error_metric(app: &Router) -> u64 {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/metrics").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();
    read_metric(&body, "openillustration_client_ui_errors_total")
}

/// 中立化 URL イベントを 1 件送信する（既定 count を使う）。
fn client_event_request(body: Body) -> Request<Body> {
    with_peer(
        Request::post("/metrics/client-events")
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .unwrap(),
        test_ipv6_peer(),
    )
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
    let neutralized_before = read_metric(&before_body, "openillustration_neutralized_urls_total");

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

    // クライアントイベントを送り、専用カウンタが1リクエストにつき1だけ進むことを確認する。
    let event = app
        .clone()
        .oneshot(client_event_request(Body::from(
            r#"{"event":"neutralized_url","count":3}"#,
        )))
        .await
        .unwrap();
    assert_eq!(event.status(), StatusCode::NO_CONTENT);

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
    let neutralized_after = read_metric(&after_body, "openillustration_neutralized_urls_total");
    // 注入されたカウンタは他テストと共有されないため、正確な差分を検証できる。
    // 増えるのは /no-such-path と /metrics/client-events の 2 件だけ
    // （前後の /metrics スクレイプは除外される）。
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
    assert_eq!(
        neutralized_after,
        neutralized_before + 1,
        "neutralized URL counter must advance by one per accepted event"
    );
}

/// 監視用の 3 パスは observe_metrics から除外され、requests/errors が増えない。
#[tokio::test]
async fn test_metrics_exclude_health_ready_and_metrics_paths() {
    let app = setup_app().await;
    let (requests_before, errors_before) = read_http_metrics(&app).await;

    for path in ["/health", "/ready", "/metrics"] {
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::get(path).body(Body::empty()).unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path} must be healthy");
        let _ = to_bytes(response.into_body(), 8192).await.unwrap();
    }

    let (requests_after, errors_after) = read_http_metrics(&app).await;
    assert_eq!(
        requests_after, requests_before,
        "monitoring paths must not increment requests"
    );
    assert_eq!(
        errors_after, errors_before,
        "monitoring paths must not increment errors"
    );
}

/// 監視パスでも GET 以外（例: POST /metrics）はカウンタへ計上される。
/// 除外は GET スクレイプだけを対象にし、非 GET のエラーを取りこぼさないための回帰テスト。
#[tokio::test]
async fn test_metrics_count_non_get_requests_to_monitoring_paths() {
    let app = setup_app().await;
    let (requests_before, errors_before) = read_http_metrics(&app).await;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/metrics").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    // /metrics は get(metrics) のみ。POST は 405 となり errors にも計上される。
    assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "POST /metrics must not be treated as a monitoring scrape"
    );
    let _ = to_bytes(response.into_body(), 8192).await.unwrap();

    let (requests_after, errors_after) = read_http_metrics(&app).await;
    assert_eq!(
        requests_after,
        requests_before + 1,
        "non-GET requests to monitoring paths must increment requests"
    );
    assert_eq!(
        errors_after,
        errors_before + 1,
        "non-GET error responses on monitoring paths must increment errors"
    );
}

/// finalizer/purge カウンタが HELP/TYPE 付きで /metrics に公開される。
#[tokio::test]
async fn test_metrics_expose_finalizer_counters() {
    let app = setup_app().await;
    let response = app
        .oneshot(with_peer(
            Request::get("/metrics").body(Body::empty()).unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // ADR-0013 P4: /metrics にも nosniff / X-Frame-Options / CSP を付与する。
    // no-store は監視パス対象外のため、ここでは3ヘッダのみを個別検証する。
    assert_eq!(response.headers()["X-Content-Type-Options"], "nosniff");
    assert_eq!(response.headers()["X-Frame-Options"], "DENY");
    assert_eq!(
        response.headers()["Content-Security-Policy"],
        "default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
    // R2-SERVER: 監視用パスは no-store の付与対象外（スクレイプを阻害しない）。
    assert!(
        !response.headers().contains_key(header::CACHE_CONTROL),
        "monitoring endpoint must not be forced to no-store"
    );
    let body =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();

    for (name, metric_type) in [
        ("openillustration_finalizer_ticks_total", "counter"),
        ("openillustration_finalizer_processed_total", "counter"),
        ("openillustration_finalizer_lock_busy_total", "counter"),
        ("openillustration_finalizer_failures_total", "counter"),
        (
            "openillustration_finalizer_last_success_timestamp_seconds",
            "gauge",
        ),
    ] {
        assert!(
            body.contains(&format!("# HELP {name} ")),
            "missing HELP line for {name}"
        );
        assert!(
            body.contains(&format!("# TYPE {name} {metric_type}")),
            "missing TYPE line for {name}"
        );
        let _ = read_metric(&body, name);
    }
}

// ---- observe_metrics は最外側で全レスポンスを捕捉する ----

/// レート制限の 429 は内側で短絡するが、最外側の observe_metrics に計上される。
#[tokio::test]
async fn test_metrics_count_rate_limited_response() {
    let app = build_router(rate_limit_test_state(1).await);
    let peer = test_ipv6_peer();

    // 先にクォータを使い切り、計測開始直後から 429 が返る状態にする。
    let first = app
        .clone()
        .oneshot(with_peer(
            Request::get("/no-such-path").body(Body::empty()).unwrap(),
            peer,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::NOT_FOUND);

    let (requests_before, errors_before) = read_http_metrics(&app).await;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/no-such-path").body(Body::empty()).unwrap(),
            peer,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    let (requests_after, errors_after) = read_http_metrics(&app).await;
    // 429 の 1 件だけが増える（前後の GET /metrics は除外される）。
    assert_eq!(requests_after, requests_before + 1);
    assert_eq!(errors_after, errors_before + 1);
}

/// IP ゲートの 403 は内側で短絡するが、最外側の observe_metrics に計上される。
/// 監視パスは除外されるため、トリガーには通常のパスを使う。
#[tokio::test]
async fn test_metrics_count_ipv4_forbidden_response() {
    let _env_guard = ENV_LOCK.lock().await;
    test_remove_env("ALLOW_IPV4_LOCALHOST");
    let app = setup_app().await;
    let (requests_before, errors_before) = read_http_metrics(&app).await;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/no-such-path").body(Body::empty()).unwrap(),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let (requests_after, errors_after) = read_http_metrics(&app).await;
    assert_eq!(requests_after, requests_before + 1);
    assert_eq!(errors_after, errors_before + 1);
}

/// サイズ拒否の 413 は内側で短絡するが、最外側の observe_metrics に計上される。
#[tokio::test]
async fn test_metrics_count_oversized_body_rejection() {
    let app = setup_app().await;
    let (requests_before, errors_before) = read_http_metrics(&app).await;

    // Content-Length だけを 10 MiB 超に宣言すれば reject_oversized_body が 413 を返す。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/content/upload")
                .header(header::CONTENT_LENGTH, (10 * 1024 * 1024 + 1).to_string())
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let (requests_after, errors_after) = read_http_metrics(&app).await;
    // 413 の 1 件だけが増える（前後の GET /metrics は除外される）。
    assert_eq!(requests_after, requests_before + 1);
    assert_eq!(errors_after, errors_before + 1);
}

/// CORS プリフライトの短絡レスポンスも最外側の observe_metrics に計上される。
#[tokio::test]
async fn test_metrics_count_cors_preflight() {
    let app = setup_app().await;
    let (requests_before, errors_before) = read_http_metrics(&app).await;

    let response = app
        .clone()
        .oneshot(with_peer(
            Request::options("/auth/login")
                .header(header::ORIGIN, "http://localhost:8080")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "preflight must be short-circuited by CorsLayer: {}",
        response.status()
    );

    let (requests_after, errors_after) = read_http_metrics(&app).await;
    // プリフライトの 1 件だけが増える（前後の GET /metrics は除外される）。
    assert_eq!(requests_after, requests_before + 1);
    assert_eq!(errors_after, errors_before);
}

// ---- クライアントイベント集計（URL 本文を送らず件数だけを集計する） ----

/// 正常系: count 指定 / 省略 / 上限値がそれぞれ正確に加算される。
#[tokio::test]
async fn test_client_events_neutralized_url_increments_counter() {
    let app = setup_app().await;
    let before = read_neutralized_metric(&app).await;

    let response = app
        .clone()
        .oneshot(client_event_request(Body::from(
            r#"{"event":"neutralized_url","count":7}"#,
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        to_bytes(response.into_body(), 16).await.unwrap().is_empty(),
        "204 response must have no body"
    );
    // count の値にかかわらず、1 リクエストにつきカウンタは 1 だけ進む。
    assert_eq!(read_neutralized_metric(&app).await, before + 1);

    // ADR-0013 P4: ui_error も受け付け、専用カウンタのみが進む（機微情報は送らない）。
    let ui_before = read_ui_error_metric(&app).await;
    let response = app
        .clone()
        .oneshot(client_event_request(Body::from(
            r#"{"event":"ui_error","count":3}"#,
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(read_ui_error_metric(&app).await, ui_before + 1);
    assert_eq!(read_neutralized_metric(&app).await, before + 1);

    // count 省略時は 1。
    let response = app
        .clone()
        .oneshot(client_event_request(Body::from(
            r#"{"event":"neutralized_url"}"#,
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(read_neutralized_metric(&app).await, before + 2);

    // 上限 1000 も受け付ける。
    let response = app
        .clone()
        .oneshot(client_event_request(Body::from(
            r#"{"event":"neutralized_url","count":1000}"#,
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(read_neutralized_metric(&app).await, before + 3);

    // 加算は 1 固定: 同じリクエストを N 回送れば before + 3 + N になる。
    for n in 1..=3u64 {
        let response = app
            .clone()
            .oneshot(client_event_request(Body::from(
                r#"{"event":"neutralized_url","count":1000}"#,
            )))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(read_neutralized_metric(&app).await, before + 3 + n);
    }
}

/// 異常系: 不正な event / count / JSON は 400 で、カウンタを一切動かさない。
#[tokio::test]
async fn test_client_events_rejects_invalid_payloads() {
    let app = setup_app().await;
    let before = read_neutralized_metric(&app).await;

    let invalid_bodies = [
        r#"{"event":"other_event","count":1}"#,
        r#"{"event":"neutralized_url ","count":1}"#,
        r#"{"event":"neutralized_url","count":0}"#,
        r#"{"event":"neutralized_url","count":1001}"#,
        r#"{"event":"neutralized_url","count":-1}"#,
        r#"{"event":"neutralized_url","count":1.5}"#,
        r#"{"event":"neutralized_url","count":"1"}"#,
        r#"{"event":"neutralized_url","count":null}"#,
        r#"{"event":123,"count":1}"#,
        r#"{"event":"neutralized_url","count":1,"url":"https://example.com/secret"}"#,
        r#"{"event":"neutralized_url","count":1,"ip":"203.0.113.1"}"#,
        r#"{"event":"neutralized_url","count":1,"user_agent":"ua"}"#,
        r#"{"event":"ui_error","count":1,"url":"https://example.com/secret"}"#,
        r#"{"event":"ui_error","count":1,"ip":"203.0.113.1"}"#,
        r#"{}"#,
        r#"[]"#,
        r#"not json"#,
    ];
    for body in invalid_bodies {
        let response = app
            .clone()
            .oneshot(client_event_request(Body::from(body)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "body: {body}");
        assert!(
            to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .is_empty(),
            "error response must have no body: {body}"
        );
    }
    // どの不正入力も加算してはならない（絶対値ではなく差分で検証）。
    assert_eq!(read_neutralized_metric(&app).await, before);
}

/// 1 KiB を超えるボディは 400 で、境界値の 1024 バイトは受け付ける。
#[tokio::test]
async fn test_client_events_rejects_oversized_body() {
    let app = setup_app().await;
    let before = read_neutralized_metric(&app).await;

    // JSON の末尾空白でパディングする（未知キーは deny_unknown_fields で拒否される）。
    let base = br#"{"event":"neutralized_url","count":1}"#;
    let padding = 1024 - base.len();

    let mut exact_body = base.to_vec();
    exact_body.extend(vec![b' '; padding]);
    assert_eq!(exact_body.len(), 1024);
    let response = app
        .clone()
        .oneshot(client_event_request(Body::from(exact_body)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(read_neutralized_metric(&app).await, before + 1);

    // Content-Length なしの 1025 バイト（to_bytes 側の上限で拒否される経路）。
    let mut too_large = base.to_vec();
    too_large.extend(vec![b' '; padding + 1]);
    assert_eq!(too_large.len(), 1025);
    let response = app
        .clone()
        .oneshot(client_event_request(Body::from(too_large)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // ID-06: 1 KiB 超過の短絡 400 もセキュリティヘッダ/相関IDを維持する。
    assert_security_headers(&response);
    assert!(!response.headers()["x-request-id"].is_empty());
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(read_neutralized_metric(&app).await, before + 1);

    // Content-Length が 10 MiB 超の宣言でも 413 ではなく 400（本文なし）。
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::post("/metrics/client-events")
                .header(header::CONTENT_LENGTH, "10485761")
                .body(Body::from(r#"{"event":"neutralized_url"}"#))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(read_neutralized_metric(&app).await, before + 1);
}

/// このエンドポイントも既存の IP 単位レート制限の対象（除外されていない）。
#[tokio::test]
async fn test_client_events_is_rate_limited() {
    let app = build_router(rate_limit_test_state(2).await);
    let peer = test_ipv6_peer();

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(with_peer(
                Request::post("/metrics/client-events")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"event":"neutralized_url"}"#))
                    .unwrap(),
                peer,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    let response = app
        .oneshot(with_peer(
            Request::post("/metrics/client-events")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"event":"neutralized_url"}"#))
                .unwrap(),
            peer,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
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
fn test_parse_pagination_defaults_and_bounds() {
    // 未指定は安全側の既定値（limit=50 / offset=0）。
    assert_eq!(parse_pagination(None, None), (DEFAULT_CONTENT_LIMIT, 0));

    // 有効境界（1..=100 / offset>=0）はそのまま通す。
    assert_eq!(parse_pagination(Some("1"), Some("7")), (1, 7));
    assert_eq!(parse_pagination(Some("100"), Some("0")), (100, 0));

    // 範囲外・非数値の limit は 400 にせず既定 50 へフォールバックする。
    for limit in ["0", "101", "-1", "abc", ""] {
        assert_eq!(
            parse_pagination(Some(limit), Some("0")),
            (DEFAULT_CONTENT_LIMIT, 0),
            "limit={limit:?}"
        );
    }

    // 負・非数値の offset は 0 とする。
    for offset in ["-1", "abc", ""] {
        assert_eq!(
            parse_pagination(Some("50"), Some(offset)),
            (DEFAULT_CONTENT_LIMIT, 0),
            "offset={offset:?}"
        );
    }
}

#[test]
fn test_parse_pagination_trims_and_matches_content_query() {
    // 前後の空白は trim してから解析する。
    assert_eq!(parse_pagination(Some(" 2 "), Some(" 3 ")), (2, 3));

    // parse_content_query が共通ヘルパへ委譲していることを確認する。
    let limit = Some(" 2 ");
    let offset = Some(" 3 ");
    let query = ContentQuery {
        q: None,
        tag: None,
        content_type: None,
        limit: limit.map(str::to_string),
        offset: offset.map(str::to_string),
        favorite_only: None,
    };
    let filter = parse_content_query(&query);
    assert_eq!(
        (filter.limit, filter.offset),
        parse_pagination(limit, offset)
    );
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

/// 実リポジトリの migration セット（001〜014）が新しい厳密検証を通過し、
/// 適用と検証の双方が成功することの回帰テスト。
#[tokio::test]
async fn test_migration_set_applies_and_verifies_cleanly() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();

    crate::migration::run_migrations(&db).await.unwrap();
    crate::migration::verify_migrations(&db).await.unwrap();

    let mut rows = db
        .query("SELECT version FROM _migrations ORDER BY version", ())
        .await
        .unwrap();
    let mut versions = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        let version: String = row.get(0).unwrap();
        versions.push(version);
    }
    assert!(!versions.is_empty(), "migrations must have been applied");
    assert_eq!(versions[0], "001");
    for (index, version) in versions.iter().enumerate() {
        assert_eq!(
            version.parse::<u32>().unwrap(),
            index as u32 + 1,
            "applied versions must be contiguous: {versions:?}"
        );
    }
}

/// 文字列リテラル内の `;` と `--` が文分割されず、値として保持されること。
#[tokio::test]
async fn test_migration_splitter_keeps_semicolons_and_dashes_in_literals() {
    let sql = "CREATE TABLE literal_t (id INTEGER PRIMARY KEY, note TEXT);\n\
               INSERT INTO literal_t (id, note) \
               VALUES (1, 'a; b -- not a comment');";
    let statements = crate::migration::split_statements(sql);
    assert_eq!(statements.len(), 2, "statements: {statements:?}");
    assert!(statements[1].contains("'a; b -- not a comment'"));

    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    for statement in &statements {
        db.execute(statement.as_str(), ()).await.unwrap();
    }

    let mut rows = db
        .query("SELECT note FROM literal_t WHERE id = 1", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let note: String = row.get(0).unwrap();
    assert_eq!(note, "a; b -- not a comment");
}

/// 行コメント内の `;` が文分割されないこと。
#[test]
fn test_migration_splitter_ignores_semicolons_in_line_comments() {
    let sql = "-- note; with a semicolon\n\
               CREATE TABLE comment_t (id INTEGER); -- trailing; comment\n\
               INSERT INTO comment_t (id) VALUES (1);";
    let statements = crate::migration::split_statements(sql);
    assert_eq!(statements.len(), 2);
    assert_eq!(
        statements[0].as_str(),
        "CREATE TABLE comment_t (id INTEGER)"
    );
    assert_eq!(
        statements[1].as_str(),
        "INSERT INTO comment_t (id) VALUES (1)"
    );
}

/// migration 014 の実ファイルを読み込む（006 はチェックサム保護のため変更しない）。
fn migration_014_sql() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("migrations")
        .join("014_refresh_tokens_expires_at_text.sql");
    std::fs::read_to_string(path).expect("migration 014 must exist in server/migrations")
}

/// 006 相当の旧 refresh_tokens DDL。実 migration 006 は checksum 保護のため編集しない。
const MIGRATION_014_OLD_REFRESH_TOKENS_DDL: &str = "CREATE TABLE IF NOT EXISTS refresh_tokens (\
     token_hash TEXT PRIMARY KEY, \
     sub TEXT NOT NULL, \
     family_id TEXT NOT NULL, \
     expires_at DATETIME NOT NULL, \
     used_at DATETIME, \
     created_at DATETIME DEFAULT CURRENT_TIMESTAMP\
     );";

/// 001〜013 を合成（006 は旧 refresh_tokens）+ 実 014 の一時 migration セットを作る。
fn migration_014_temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("openillustration_migration_014_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for version in 1..=13 {
        let sql = if version == 6 {
            MIGRATION_014_OLD_REFRESH_TOKENS_DDL.to_owned()
        } else {
            format!("CREATE TABLE synthetic_{version} (id INTEGER PRIMARY KEY);")
        };
        std::fs::write(dir.join(format!("{version:03}_migration.sql")), sql).unwrap();
    }
    std::fs::write(
        dir.join("014_refresh_tokens_expires_at_text.sql"),
        migration_014_sql(),
    )
    .unwrap();
    dir
}

/// 実 migration セット適用後に expires_at が TEXT と宣言されること（014 の型修正）。
#[tokio::test]
async fn test_migration_014_declares_expires_at_as_text() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    crate::migration::run_migrations(&db).await.unwrap();

    for (column, expected) in [
        ("expires_at", "TEXT"),
        ("used_at", "DATETIME"),
        ("created_at", "DATETIME"),
    ] {
        let mut rows = db
            .query(
                "SELECT type FROM pragma_table_info('refresh_tokens') WHERE name = ?",
                [column],
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let declared: String = row.get(0).unwrap();
        assert_eq!(declared, expected, "unexpected declared type for {column}");
    }

    let mut rows = db
        .query("SELECT COUNT(*) FROM _migrations WHERE version = '014'", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let applied: i64 = row.get(0).unwrap();
    assert_eq!(applied, 1, "migration 014 must be recorded as applied");
}

/// 006 相当の DB に保存した小数秒付き RFC3339 が 014 の再作成後も保持されること。
#[tokio::test]
async fn test_migration_014_preserves_rfc3339_expires_at() {
    let dir = migration_014_temp_dir("preserve");
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();

    let original = "2026-12-01T00:00:00.123456789+00:00";
    db.execute(MIGRATION_014_OLD_REFRESH_TOKENS_DDL, ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at) \
         VALUES ('hash', 'sub', 'family', ?)",
        [original],
    )
    .await
    .unwrap();

    crate::migration::run_migrations_from(&db, &dir)
        .await
        .unwrap();

    let mut rows = db
        .query(
            "SELECT expires_at FROM refresh_tokens WHERE token_hash = 'hash'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let stored: String = row.get(0).unwrap();
    assert_eq!(
        stored, original,
        "RFC3339 value must survive the table rebuild"
    );
    chrono::DateTime::parse_from_rfc3339(&stored)
        .unwrap_or_else(|e| panic!("preserved expires_at must be RFC3339: {stored}: {e}"));

    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' \
             AND name IN ('idx_refresh_tokens_family', 'idx_refresh_tokens_sub')",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let indexes: i64 = row.get(0).unwrap();
    assert_eq!(indexes, 2, "both refresh_tokens indexes must be recreated");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 014 を書き換えると checksum mismatch で検出されること（実ファイルは変更しない）。
#[tokio::test]
async fn test_migration_014_rewrite_detects_checksum_mismatch() {
    let dir = migration_014_temp_dir("checksum");
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    crate::migration::run_migrations_from(&db, &dir)
        .await
        .unwrap();
    crate::migration::verify_migrations_from(&db, &dir)
        .await
        .unwrap();

    let mut rewritten = migration_014_sql();
    rewritten.push_str("\n-- rewritten for checksum drift test\n");
    std::fs::write(
        dir.join("014_refresh_tokens_expires_at_text.sql"),
        rewritten,
    )
    .unwrap();

    let err = crate::migration::verify_migrations_from(&db, &dir)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("Migration 014 checksum mismatch"),
        "rewritten 014 must be detected: {err}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 本番起動と同じ順序（FK 有効化 → migration）でローカル DB を組み立て、
/// 参照先不存在の INSERT が拒否されることと既存の正常系が壊れないことを検証する。
#[tokio::test]
async fn test_foreign_keys_enabled_and_enforced_on_local_db() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();

    // 本番 run() と同じヘルパを migration 前に呼ぶ。
    crate::enable_foreign_keys(&db).await.unwrap();

    // PRAGMA foreign_keys が実際に 1 になっていること。
    {
        let mut rows = db.query("PRAGMA foreign_keys", ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let enabled: i64 = row.get(0).unwrap();
        assert_eq!(
            enabled, 1,
            "foreign_keys must be ON after enable_foreign_keys"
        );
    }

    init_schema(&db).await;

    // 既存の users / contents / favorites 正常系が壊れないこと。
    db.execute("INSERT INTO users (user_hash) VALUES ('fk_user')", ())
        .await
        .unwrap();
    db.execute(
        "INSERT INTO contents (cid, checksum, owner_hash) \
         VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111F', 'sum', 'fk_user')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO favorites (user_hash, cid) \
         VALUES ('fk_user', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111F')",
        (),
    )
    .await
    .unwrap();

    // 参照先が存在しない INSERT は FK により拒否される。
    assert!(
        db.execute(
            "INSERT INTO favorites (user_hash, cid) \
             VALUES ('missing_user', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111F')",
            (),
        )
        .await
        .is_err(),
        "favorites INSERT with a missing user must be rejected by FK"
    );
    assert!(
        db.execute(
            "INSERT INTO contents (cid, checksum, owner_hash) \
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A112F', 'sum', 'missing_user')",
            (),
        )
        .await
        .is_err(),
        "contents INSERT with a missing owner must be rejected by FK"
    );

    // ON DELETE CASCADE（012_favorites）が本番と同じ FK 有効化で効くこと。
    // CASCADE が無効なら参照中の contents 削除自体が FK 違反で失敗する。
    db.execute(
        "DELETE FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111F'",
        (),
    )
    .await
    .unwrap();
    {
        let mut rows = db
            .query(
                "SELECT COUNT(*) FROM favorites WHERE user_hash = 'fk_user'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let count: i64 = row.get(0).unwrap();
        assert_eq!(count, 0, "favorites must be removed by ON DELETE CASCADE");
    }

    // 正常系の行は残っており、孤立行は検出されないこと。
    let violations = crate::find_foreign_key_violations(&db).await.unwrap();
    assert!(
        violations.is_empty(),
        "clean DB must have no FK violations: {violations:?}"
    );
}

/// FK 無効な既存 DB に孤立行がある場合に `foreign_key_check` が検出できることを検証する。
/// 本番相当（mock 無効）では検出結果を fail-closed で起動拒否へ使い、
/// 開発（mock 有効）では起動を止めず warn のみとする（ADR-0010）。
#[tokio::test]
async fn test_foreign_key_check_detects_orphan_rows() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;

    // この :memory: 接続では FK は既定で無効（init_schema は migration のみで
    // PRAGMA を実行しない）。孤立行を作るため明示的に OFF を指定する。
    // 接続はテスト専用なので他テストへ波及しない。
    db.execute("PRAGMA foreign_keys = OFF", ()).await.unwrap();
    db.execute(
        "INSERT INTO favorites (user_hash, cid) VALUES ('orphan_user', 'orphan_cid')",
        (),
    )
    .await
    .unwrap();

    let violations = crate::find_foreign_key_violations(&db).await.unwrap();
    assert!(
        violations.iter().any(|value| value.contains("favorites")),
        "orphan favorites row must be reported: {violations:?}"
    );
}

// --- 起動時 FK fail-closed の純粋判定（ADR-0010） --------------------------

#[test]
fn test_foreign_key_startup_error_mock_always_none() {
    let violations = vec!["favorites(rowid=Some(1)) -> users".to_string()];
    // mock 有効なら孤立行・検査失敗にかかわらず常に None（warn のみ、起動は止めない）。
    assert!(crate::foreign_key_startup_error(true, &violations, false).is_none());
    assert!(crate::foreign_key_startup_error(true, &violations, true).is_none());
    assert!(crate::foreign_key_startup_error(true, &[], true).is_none());
    assert!(crate::foreign_key_startup_error(true, &[], false).is_none());
}

#[test]
fn test_foreign_key_startup_error_production_violations_are_fatal() {
    let violations = vec![
        "favorites(rowid=Some(7)) -> users".to_string(),
        "contents(rowid=Some(9)) -> users".to_string(),
    ];
    let message = crate::foreign_key_startup_error(false, &violations, false)
        .expect("本番相当では孤立行で起動を拒否する");
    assert!(message.contains('2'), "件数を含むこと: {message}");
    assert!(message.contains("favorites"), "table を含むこと: {message}");
    assert!(
        message.contains("rowid=Some(7)"),
        "rowid を含むこと: {message}"
    );
    assert!(message.contains("users"), "parent を含むこと: {message}");
    // 構造体のデバッグ表現や設定値は含めない（件数と table/rowid/parent のみ）。
    assert!(
        !message.contains("violations="),
        "内部表現を出さない: {message}"
    );
}

#[test]
fn test_foreign_key_startup_error_check_failure_is_fatal_in_production() {
    let message = crate::foreign_key_startup_error(false, &[], true)
        .expect("本番相当では検査失敗でも起動を拒否する");
    assert!(
        message.contains("foreign key check"),
        "固定文言であること: {message}"
    );
    assert!(
        !message.contains("failed to run"),
        "検査エラーの本文（設定値を含む可能性）はエコーしない: {message}"
    );
}

#[test]
fn test_foreign_key_startup_error_clean_production_is_none() {
    assert!(crate::foreign_key_startup_error(false, &[], false).is_none());
}

/// 既存ヘルパが組み立てるアプリの users / contents が参照整合性を保ち、
/// favorites の正常系が壊れないことを回帰検証する（全体テスト非破壊）。
#[tokio::test]
async fn test_existing_setup_app_with_db_rows_are_fk_clean() {
    let (_app, db) = setup_app_with_db().await;

    let violations = crate::find_foreign_key_violations(&db).await.unwrap();
    assert!(
        violations.is_empty(),
        "setup_app_with_db must not create orphan rows: {violations:?}"
    );

    // 既存ヘルパが挿入した test_user / content へのお気に入り登録は成功する。
    db.execute(
        "INSERT INTO favorites (user_hash, cid) \
         VALUES ('test_user', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g')",
        (),
    )
    .await
    .unwrap();
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
        "INSERT INTO contents (cid, type, checksum) VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111R', 'Novel', 'legacy_sum')",
        (),
    )
    .await
    .unwrap();

    crate::migration::run_migrations(&db).await.unwrap();

    let mut rows = db
        .query(
            "SELECT title, description, tags FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111R'",
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
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111B",
        "Illustration",
        "Cat illustration",
        "A cute cat",
        "fantasy,art",
        "2026-01-04 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111H",
        "Novel",
        "Dog novel",
        "A brave story",
        "dog,novel",
        "2026-01-03 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111C",
        "Illustration",
        "Landscape",
        "Nice colors",
        "cat,fantasy",
        "2026-01-02 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111D",
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
    assert_eq!(
        listed_cids(&items),
        vec!["QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111B"]
    );

    // tag はカンマ区切りの各タグの前方一致（先頭・カンマ後ともに該当）。
    let (_, items) = get_content_list(&app, &token, "?tag=cat").await;
    assert_eq!(
        listed_cids(&items),
        vec![
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111C",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111D"
        ]
    );

    // type は完全一致。未知の値は 400 ではなく 0 件。
    let (_, items) = get_content_list(&app, &token, "?type=Novel").await;
    assert_eq!(
        listed_cids(&items),
        vec![
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111H",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111D"
        ]
    );
    let (status, items) = get_content_list(&app, &token, "?type=Unknown").await;
    assert_eq!(status, StatusCode::OK);
    assert!(items.is_empty());

    // 複数条件は AND。既定の並び順（新着順）も維持される。
    let (_, items) = get_content_list(&app, &token, "?type=Novel&tag=cat").await;
    assert_eq!(
        listed_cids(&items),
        vec!["QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111D"]
    );

    // 追加メタデータが既存5項目と一緒に JSON で返る。
    let (_, items) = get_content_list(&app, &token, "?q=Landscape").await;
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0]["cid"],
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111C"
    );
    assert_eq!(items[0]["title"], "Landscape");
    assert_eq!(items[0]["description"], "Nice colors");
    assert_eq!(items[0]["tags"], serde_json::json!(["cat", "fantasy"]));
    assert_eq!(items[0]["type"], "Illustration");
    assert_eq!(items[0]["status"], "Approved");
    assert_eq!(items[0]["price"], 0);
}

/// price が NULL の Approved 行があっても一覧は 500 にならず、
/// 購入不可を表す 0 として返る（エクスポート側の Option<i64> と経路を揃える）。
#[tokio::test]
async fn test_list_contents_null_price_is_returned_as_zero() {
    let (app, db) = setup_app_with_db().await;
    db.execute("DELETE FROM contents", ()).await.unwrap();
    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status, price, title, description, tags, created_at)
         VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111B',
                 'Illustration', 'test_user', 'sum', 'Approved', NULL,
                 'Null price', 'no price', '', '2026-01-05 00:00:00')",
        (),
    )
    .await
    .unwrap();
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    let (status, items) = get_content_list(&app, &token, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0]["cid"],
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111B"
    );
    assert_eq!(items[0]["price"], 0);
}

#[tokio::test]
async fn test_list_contents_limit_boundaries_and_offset() {
    let (app, db) = setup_app_with_db().await;
    db.execute("DELETE FROM contents", ()).await.unwrap();
    // 有効な CIDv0（Qm + base58 44 文字）を day から生成する。
    let cid_of = |day: u32| format!("Qm{}{day:03}", "1".repeat(41));
    for day in 1..=5 {
        insert_content_meta(
            &db,
            &cid_of(day),
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
    assert_eq!(listed_cids(&items), vec![cid_of(3), cid_of(2)]);
    let (_, items) = get_content_list(&app, &token, "?offset=10").await;
    assert!(items.is_empty());
}

#[tokio::test]
async fn test_like_wildcards_are_literal() {
    let (app, db) = setup_app_with_db().await;
    db.execute("DELETE FROM contents", ()).await.unwrap();
    insert_content_meta(
        &db,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111a",
        "Illustration",
        "plain title",
        "plain description",
        "plain",
        "2026-01-02 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111p",
        "Illustration",
        "50%_done",
        "note",
        "%_tag",
        "2026-01-01 00:00:00",
    )
    .await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;

    // エスケープされていれば "%" / "_" はリテラルとして扱われ、QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111p だけに一致する。
    for query in ["%", "_"] {
        let (_, items) =
            get_content_list(&app, &token, &format!("?q={}", percent_encode(query))).await;
        assert_eq!(
            listed_cids(&items),
            vec!["QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111p"],
            "query={query}"
        );
    }
    let (_, items) = get_content_list(&app, &token, &format!("?tag={}", percent_encode("%"))).await;
    assert_eq!(
        listed_cids(&items),
        vec!["QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111p"]
    );

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
        &[0u8; 256],
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
        let (status, body) = post_favorite(
            &app,
            &token,
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "response: {body}");
        assert_eq!(body, json!({"is_favorite": true}));
        let parsed: FavoriteResponse = serde_json::from_value(body).unwrap();
        assert!(parsed.is_favorite);
        assert_eq!(
            favorite_count(
                &db,
                "test_user",
                "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g"
            )
            .await,
            1
        );
    }

    // 未登録の削除も冪等で、行が0件のまま成功する。
    for _ in 0..2 {
        let (status, body) = delete_favorite(
            &app,
            &token,
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "response: {body}");
        assert_eq!(body, json!({"is_favorite": false}));
        let parsed: FavoriteResponse = serde_json::from_value(body).unwrap();
        assert!(!parsed.is_favorite);
        assert_eq!(
            favorite_count(
                &db,
                "test_user",
                "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g"
            )
            .await,
            0
        );
    }

    // 削除後に再追加できる。
    let (status, _) = post_favorite(
        &app,
        &token,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        favorite_count(
            &db,
            "test_user",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g"
        )
        .await,
        1
    );
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
                    .uri("/content/QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g/favorite")
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
         VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111b', 'Illustration', 'test_user', 'sum', 'Reported')",
        (),
    )
    .await
    .unwrap();
    let (status, _) = post_favorite(
        &app,
        &token,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111b",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        favorite_count(
            &db,
            "test_user",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111b"
        )
        .await,
        0
    );
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
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response: {response}");
    assert_eq!(
        favorite_count(
            &db,
            "test_user",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g"
        )
        .await,
        0
    );
    assert_eq!(
        favorite_count(
            &db,
            "attacker",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g"
        )
        .await,
        1
    );

    // 攻撃者の削除も攻撃者の行にだけ作用する。
    let (status, _) = delete_favorite(
        &app,
        &attacker_token,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        favorite_count(
            &db,
            "test_user",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g"
        )
        .await,
        0
    );
    assert_eq!(
        favorite_count(
            &db,
            "attacker",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g"
        )
        .await,
        0
    );

    // 被害者本人の追加は被害者の行だけを作る。
    let (status, _) = post_favorite(
        &app,
        &victim_token,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        favorite_count(
            &db,
            "test_user",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g"
        )
        .await,
        1
    );
    assert_eq!(
        favorite_count(
            &db,
            "attacker",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g"
        )
        .await,
        0
    );
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
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111N",
        "Illustration",
        "mine",
        "",
        "",
        "2026-01-04 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111P",
        "Illustration",
        "other",
        "",
        "",
        "2026-01-03 00:00:00",
    )
    .await;
    insert_content_meta(
        &db,
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111a",
        "Illustration",
        "plain",
        "",
        "",
        "2026-01-02 00:00:00",
    )
    .await;
    db.execute(
        "INSERT INTO favorites (user_hash, cid) VALUES ('test_user', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111N')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO favorites (user_hash, cid) VALUES ('attacker', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111P')",
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
        vec![
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111N",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111P",
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111a"
        ]
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
        assert_eq!(
            listed_cids(&items),
            vec!["QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111N"],
            "query={query}"
        );
    }
    let (_, items) = get_content_list(&app, &attacker_token, "?favorite_only=1").await;
    assert_eq!(
        listed_cids(&items),
        vec!["QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111P"]
    );

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
    assert_eq!(
        listed_cids(&items),
        vec!["QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111N"]
    );
    let (_, items) = get_content_list(&app, &token, "?favorite_only=1&offset=1").await;
    assert!(items.is_empty());

    // Approved 限定は維持。Reported に変えたコンテンツはお気に入り一覧から消える。
    db.execute(
        "UPDATE contents SET status = 'Reported' WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111N'",
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
        "cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111R",
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
        "cid": "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111G",
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
        "INSERT INTO contents (cid, checksum) VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111L', 'sum')",
        (),
    )
    .await
    .unwrap();

    // UNIQUE(user_hash, cid): 同じ組の2回目の INSERT は失敗する。
    db.execute(
        "INSERT INTO favorites (user_hash, cid) VALUES ('u1', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111L')",
        (),
    )
    .await
    .unwrap();
    assert!(
        db.execute(
            "INSERT INTO favorites (user_hash, cid) VALUES ('u1', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111L')",
            (),
        )
        .await
        .is_err(),
        "duplicate favorite must violate UNIQUE(user_hash, cid)"
    );
    db.execute(
        "INSERT OR IGNORE INTO favorites (user_hash, cid) VALUES ('u1', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111L')",
        (),
    )
    .await
    .unwrap();
    assert_eq!(
        favorite_count(&db, "u1", "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111L").await,
        1
    );
    // 別ユーザーなら同じ CID を登録できる。
    db.execute(
        "INSERT INTO favorites (user_hash, cid) VALUES ('u2', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111L')",
        (),
    )
    .await
    .unwrap();
    assert_eq!(
        favorite_count(&db, "u2", "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111L").await,
        1
    );

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
    db.execute(
        "DELETE FROM contents WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111L'",
        (),
    )
    .await
    .unwrap();
    assert_eq!(
        favorite_count(&db, "u1", "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111L").await,
        0
    );
    assert_eq!(
        favorite_count(&db, "u2", "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111L").await,
        0
    );

    // ユーザー削除でも CASCADE する（別のコンテンツで確認）。
    db.execute(
        "INSERT INTO contents (cid, checksum) VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111M', 'sum')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO favorites (user_hash, cid) VALUES ('u1', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111M')",
        (),
    )
    .await
    .unwrap();
    db.execute("DELETE FROM users WHERE user_hash = 'u1'", ())
        .await
        .unwrap();
    assert_eq!(
        favorite_count(&db, "u1", "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111M").await,
        0
    );
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
         VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'Approved', 'Reported', 'SECRET_USER_HASH_SENTINEL',
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
         VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'Approved', 'Reported', 'reporter', 'report reason',
                 'hash1', '2025-12-31 23:59:59')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO content_audit_log
             (cid, old_status, new_status, changed_by, reason, entry_hash, created_at)
         VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'Reported', 'Rejected', 'SYSTEM_GOVERNANCE', 'tie',
                 'hash2', '2026-01-01 00:00:00')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO content_audit_log
             (cid, old_status, new_status, changed_by, reason, entry_hash, created_at)
         VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'Rejected', 'Approved', 'SYSTEM_GOVERNANCE', 'quorum not met',
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
         VALUES ('UNPIN', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'operator', 'unpin reason', '2026-01-01 00:00:00')",
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
         VALUES (2, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'secret2', '2026-01-01 00:00:00')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (3, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'secret3')",
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

// ---- R2-PUBLISHED-SERVER / ADR-0008 段階2: 公開集計ルート ----

/// 認証付きで `/mod/stats/public` を呼ぶ（`None` なら認証ヘッダなし）。
async fn send_public_mod_stats_request(app: &Router, sub: Option<&str>) -> Response {
    let mut request = Request::get("/mod/stats/public");
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

#[tokio::test]
async fn test_public_mod_stats_requires_authentication() {
    // 未認証は AuthClaims extractor により 401。
    let (app, _db) = setup_mod_app_with_operators(&["operator"]).await;
    let response = send_public_mod_stats_request(&app, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_public_mod_stats_allows_general_sub() {
    // 運用者未設定・未登録の一般 sub でも 200（require_moderation_operator を呼ばない）。
    let (app, _db) = setup_mod_app().await;
    let response = send_public_mod_stats_request(&app, Some("general_user")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let parsed: PublicModStatsResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed.totals, PublicModStatsTotals::default());
    assert!(parsed.monthly.is_empty());
}

#[tokio::test]
async fn test_public_mod_stats_allows_operator_sub() {
    // 運用者 sub も同じ契約で 200（特権ルートではない）。
    let (app, _db) = setup_mod_app_with_operators(&["operator"]).await;
    let response = send_public_mod_stats_request(&app, Some("operator")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let parsed: PublicModStatsResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed.totals, PublicModStatsTotals::default());
}

#[tokio::test]
async fn test_public_mod_stats_response_contains_no_forbidden_fields() {
    let (app, db) = setup_mod_app_with_operators(&["operator"]).await;
    db.execute(
        "INSERT INTO content_audit_log
             (cid, old_status, new_status, changed_by, reason, entry_hash, created_at)
         VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111T', 'Approved', 'Reported', 'SECRET_USER_HASH_SENTINEL',
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
    db.execute(
        "UPDATE reports SET reason = 'SECRET_REASON_SENTINEL' WHERE report_id = 1",
        (),
    )
    .await
    .unwrap();

    let response = send_public_mod_stats_request(&app, Some("general_user")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();

    // 移送してはならない値・キーがレスポンス文字列に一切現れないこと。
    for sentinel in [
        "SECRET_REASON_SENTINEL",
        "SECRET_CID_SENTINEL",
        "SECRET_USER_HASH_SENTINEL",
        "reason",
        "cid",
        "user_hash",
        "bans",
        "unbans",
        "unpins",
    ] {
        assert!(
            !body.contains(sentinel),
            "public response must not contain {sentinel:?}: {body}"
        );
    }

    // ホワイトリストでキー集合を固定（余分なフィールドがないこと）。
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let root = value.as_object().unwrap();
    let mut root_keys: Vec<&String> = root.keys().collect();
    root_keys.sort();
    // ADR-0013 P4: server_now は鮮度判定用のサーバー時刻（機微情報なし）。
    assert_eq!(
        root_keys,
        vec!["generated_at", "monthly", "server_now", "totals"]
    );
    assert!(
        root["server_now"].as_str().is_some_and(
            |value| value.contains('T') && (value.ends_with('Z') || value.ends_with("+00:00"))
        ),
        "server_now must be RFC3339 UTC"
    );

    let mut total_keys: Vec<&String> = root["totals"].as_object().unwrap().keys().collect();
    total_keys.sort();
    assert_eq!(
        total_keys,
        vec![
            "approved",
            "concluded",
            "quorum_not_met",
            "rejected",
            "reports",
        ]
    );

    let monthly = root["monthly"].as_array().unwrap();
    // 月数はフィクスチャの集計結果に依存するため、キー集合のみを検証する。
    assert!(!monthly.is_empty());
    for entry in monthly {
        let mut keys: Vec<&String> = entry.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["approved", "concluded", "month", "rejected", "reports"],
            "public monthly must not contain quorum_not_met or moderation actions"
        );
    }

    let parsed: PublicModStatsResponse = serde_json::from_slice(&bytes).unwrap();
    assert!(chrono::DateTime::parse_from_rfc3339(&parsed.generated_at).is_ok());
    assert_eq!(parsed.totals.reports, 1);
    assert_eq!(parsed.totals.approved, 0);
}

#[test]
fn test_build_public_mod_stats_matches_source() {
    // build_mod_stats の結果をそのまま縮約し、公開項目の値が元と同値であること。
    let source = build_mod_stats(
        STATS_GENERATED_AT,
        &[
            audit_count("2025-12", "Reported", 2),
            audit_count("2026-01", "Approved", 3),
            audit_count("2026-01", "Rejected", 1),
        ],
        &[
            governance_count("2025-12", "BAN", 4),
            governance_count("2026-01", "UNBAN", 5),
            governance_count("2026-01", "UNPIN", 6),
        ],
        &[conclusion_count("2026-01", true, 1)],
    );
    let public = build_public_mod_stats(&source);

    assert_eq!(public.generated_at, source.generated_at);
    assert_eq!(public.generated_at, STATS_GENERATED_AT);
    assert_eq!(
        public.totals,
        PublicModStatsTotals {
            reports: source.totals.reports,
            concluded: source.totals.concluded,
            approved: source.totals.approved,
            rejected: source.totals.rejected,
            quorum_not_met: source.totals.quorum_not_met,
        }
    );
    assert_eq!(public.totals.reports, 2);
    assert_eq!(public.totals.concluded, 1);
    assert_eq!(public.totals.approved, 2); // 3 - 定足数未達1
    assert_eq!(public.totals.rejected, 1);
    assert_eq!(public.totals.quorum_not_met, 1);
    assert_eq!(
        public.monthly,
        vec![
            PublicModStatsMonthly {
                month: "2025-12".to_string(),
                reports: 2,
                ..Default::default()
            },
            PublicModStatsMonthly {
                month: "2026-01".to_string(),
                approved: 2,
                rejected: 1,
                concluded: 1,
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
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1114', 'Illustration', 'alice', 'checksum-alice', 'Approved', 100,
                     '2026-01-01T00:00:00Z', 'alice title', 'alice desc', 'art,alice')",
        "INSERT INTO contents (cid, type, owner_hash, checksum, status, price, created_at, title, description, tags)
             VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1119', 'Novel', 'bob', 'checksum-bob', 'Approved', 200,
                     '2026-01-02T00:00:00Z', 'bob title', 'bob desc', 'novel,bob')",
        "INSERT INTO favorites (user_hash, cid) VALUES ('alice', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1114')",
        "INSERT INTO favorites (user_hash, cid) VALUES ('bob', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1119')",
        "INSERT INTO transactions (tx_id, cid, sender_hash, receiver_hash, amount, tx_hash)
             VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1114', 'alice', 'bob', 500, 'tx-alice-to-bob')",
        "INSERT INTO transactions (tx_id, cid, sender_hash, receiver_hash, amount, tx_hash)
             VALUES (2, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1119', 'bob', 'alice', 700, 'tx-bob-to-alice')",
        "INSERT INTO transactions (tx_id, cid, sender_hash, receiver_hash, amount, tx_hash)
             VALUES (3, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1119', 'bob', 'bob', 900, 'tx-bob-only')",
        "INSERT INTO signatures (signature_id, target_id, signer_hash, signature_blob, algorithm)
             VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1114', 'alice', X'aa', 'RSASSA-PKCS1-v1_5')",
        "INSERT INTO signatures (signature_id, target_id, signer_hash, signature_blob, algorithm)
             VALUES (2, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1119', 'bob', X'bb', 'RSASSA-PKCS1-v1_5')",
        "INSERT INTO content_audit_log (log_id, cid, old_status, new_status, changed_by, reason, entry_hash)
             VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1114', NULL, 'Approved', 'alice', 'alice secret reason', 'hash-a1')",
        "INSERT INTO content_audit_log (log_id, cid, old_status, new_status, changed_by, reason, entry_hash)
             VALUES (2, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1119', NULL, 'Approved', 'bob', 'bob secret reason', 'hash-b1')",
        "INSERT INTO reports (report_id, target_cid, reason)
             VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1114', 'alice private report reason')",
        "INSERT INTO reports (report_id, target_cid, reason)
             VALUES (2, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1119', 'bob private report reason')",
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
             VALUES (3, 'UNPIN', 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1114', 'alice', 'self reason', NULL)",
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
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    build_router(shared_state)
}

/// `GET /me/export` を送信する。`sub = None` は Authorization ヘッダ無し。
async fn me_export_request(app: &Router, sub: Option<&str>) -> Response {
    let mut builder = Request::get("/me/export");
    if let Some(sub) = sub {
        // DB正本の revoked_subs には alice=1000/bob=2000 がシードされているため、
        // それより未来の iat を付与して「失効前のセッション」として扱わせる。
        // 失効判定は DB を正本とするため、JWT だけを操作すれば他テストへ波及しない。
        let claims = Claims {
            sub: sub.to_string(),
            exp: 10000000000,
            sid: None,
            iat: Some(10_000_000),
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(TEST_JWT_SECRET.as_ref()).unwrap(),
        )
        .unwrap();
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
    assert_eq!(
        export["contents"][0]["cid"],
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1114"
    );
    assert_eq!(export["favorites"].as_array().unwrap().len(), 1);
    assert_eq!(
        export["favorites"][0]["cid"],
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1114"
    );
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
            .all(|row| row["cid"] != "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1119")
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
    assert_eq!(
        bob["contents"][0]["cid"],
        "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1119"
    );
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
            .all(|row| row["cid"] != "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A1114")
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

    // 回帰: GET /transactions は claims.sub でスコープし、本人が sender / receiver
    // である行だけを返す（bob 同士の tx-bob-only は漏えいしない）。
    let alice_claims = Claims {
        sub: "alice".to_string(),
        exp: 10000000000,
        sid: None,
        iat: Some(10_000_000),
    };
    let alice_token = encode(
        &Header::default(),
        &alice_claims,
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_ref()).unwrap(),
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::get("/transactions")
                .header("authorization", format!("Bearer {alice_token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(rows.len(), 2, "alice は本人が絡む2件のみ見える");
    assert!(
        rows.iter()
            .all(|row| row["sender_hash"] == "alice" || row["receiver_hash"] == "alice"),
        "他ユーザー間の取引が返ってはならない"
    );
    assert!(
        rows.iter().all(|row| row["tx_hash"] != "tx-bob-only"),
        "bob 同士の取引は漏えいしない"
    );
}

/// R5-04: データが無いユーザーでは 12 配列すべてが空になる。
#[tokio::test]
async fn me_export_empty_user_has_empty_arrays() {
    // setup_app は test_user / QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g のみを投入する。別 sub には行が無い。
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

/// BAN 期限: /me/export の users 行に ban_until が含まれる。
#[tokio::test]
async fn me_export_includes_ban_until() {
    let (app, db) = setup_app_with_db().await;
    let stored = "2027-01-02 03:04:05";
    db.execute(
        "UPDATE users SET is_banned = TRUE, ban_until = ? WHERE user_hash = 'test_user'",
        [stored.to_string()],
    )
    .await
    .unwrap();

    let export = me_export_body(&app, "test_user").await;
    let users = export["users"].as_array().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["user_hash"], "test_user");
    assert_eq!(users[0]["is_banned"], true);
    assert_eq!(
        users[0]["ban_until"], stored,
        "ban_until は SQLite datetime() 形式の文字列でエクスポートされる"
    );
}

/// 一時 SQLite ファイル（ドロップ時に削除）。別接続からの書き込みを伴う
/// スナップショット検証テストで、エクスポート用と書き込み用の
/// 2 つの Connection を同じ DB に向けるために使う。
struct TempDbPath(std::path::PathBuf);

impl Drop for TempDbPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        for suffix in ["-journal", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", self.0.display(), suffix));
        }
    }
}

/// 12 表の読み取りが単一の読み取りトランザクション内で行われることを検証する。
/// `collect_export_snapshot` の `debug_assert!(!db.is_autocommit())` が
/// `me_export` 経由で発火しないことを確認する（cargo test は debug ビルド）。
#[tokio::test]
async fn me_export_queries_run_inside_read_transaction() {
    let app = setup_me_export_app().await;
    let export = me_export_body(&app, "alice").await;
    assert_eq!(export["version"], 1);

    // `me_export` から `db.transaction()` / `tx.commit()` を外す mutation を
    // かけると、autocommit 状態で collect_export_snapshot が呼ばれ、
    // debug_assert! が失敗してこのテストが落ちる。
}

/// エクスポート実行中に別接続から BAN 書き込みを試みても、
/// エクスポート結果が書き込み前の単一時点スナップショットになることを検証する。
///
/// 書き込み接続は `BEGIN IMMEDIATE` で BAN 更新と監査ログ挿入を保持したまま
/// エクスポートを実行する。未コミットの書き込みは別接続から見えないため、
/// エクスポートは書き込み前の状態を一貫して返す。全12クエリが同じ読み取り
/// トランザクションに所属することは
/// `me_export_queries_run_inside_read_transaction` の debug_assert が担保する。
#[tokio::test]
async fn me_export_snapshot_consistent_under_concurrent_ban_write() {
    static DB_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = TempDbPath(std::env::temp_dir().join(format!(
        "oi-export-snapshot-{}-{}.db",
        std::process::id(),
        DB_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    let _ = std::fs::remove_file(&path.0);
    let path_str = path.0.to_string_lossy().into_owned();

    let db_export = Arc::new(
        Builder::new_local(&path_str)
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    let db_writer = Builder::new_local(&path_str)
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db_export).await;
    db_export
        .execute(
            "INSERT INTO users (user_hash, public_key_der, is_banned) VALUES ('alice', X'0102', 0)",
            (),
        )
        .await
        .unwrap();

    // 別接続で BAN を開始する（まだ COMMIT しない）。
    db_writer.execute("BEGIN IMMEDIATE", ()).await.unwrap();
    db_writer
        .execute(
            "UPDATE users SET is_banned = 1, ban_until = '2027-01-01T00:00:00Z' \
             WHERE user_hash = 'alice'",
            (),
        )
        .await
        .unwrap();
    db_writer
        .execute(
            "INSERT INTO governance_audit_log \
             (log_id, action, target, actor_hash, reason, details) \
             VALUES (99, 'BAN', 'alice', 'operator', 'ban reason', 'ban details')",
            (),
        )
        .await
        .unwrap();

    let shared_state = Arc::new(AppState {
        db: db_export.clone(),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let app = build_router(shared_state);

    let app_for_task = app.clone();
    let export_task =
        tokio::spawn(async move { me_export_request(&app_for_task, Some("alice")).await });

    // エクスポート中、書き込み接続は BAN 更新と監査ログ挿入を未コミットのまま
    // 保持する。トランザクション内の全クエリが同じスナップショットを読むことは
    // me_export_queries_run_inside_read_transaction の debug_assert が担保する。
    // ここでは、別接続の未コミット書き込みに惑わされず書き込み前の値が
    // 一貫して返ることを検証する。
    let response = export_task.await.unwrap();

    // エクスポート完了後に書き込みを確定する。
    db_writer.execute("COMMIT", ()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 10 * 1024 * 1024)
        .await
        .unwrap();
    let export: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // エクスポートは書き込み前のスナップショットを返す。
    let users = export["users"].as_array().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(
        users[0]["is_banned"], false,
        "BAN 更新前の users 行が見えること"
    );
    let audit = export["governance_audit_log"].as_array().unwrap();
    assert!(
        audit.iter().all(|row| row["action"] != "BAN"),
        "BAN 更新前の governance_audit_log が見えること: {audit:?}"
    );
}

/// 共有 Connection 上で書き込みトランザクションが BEGIN を保持している間、
/// `me_export` が write_lock を待ってから BEGIN し、500 にならずに成功することを検証する。
///
/// 旧実装は write_lock なしで `db.transaction()` を呼ぶため、並行する書き込みの
/// BEGIN と衝突して "cannot start a transaction within a transaction" の 500 になる。
/// mutation で `begin_write_tx(&state.db)` を素の `db.transaction()` に戻すと、
/// このテストは「書き込み完了前に完了してしまう」または 500 で失敗する。
#[tokio::test]
async fn me_export_waits_for_concurrent_write_transaction() {
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
        "INSERT INTO users (user_hash, public_key_der, is_banned) VALUES ('alice', X'0102', 0)",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: Arc::clone(&db),
        http_client: test_http_client(),
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
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![],
    });
    let app = build_router(shared_state);

    // 書き込み側が BEGIN を保持したことの通知と、COMMIT を許可するチャネル。
    let (write_started_tx, write_started_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let (write_done_tx, write_done_rx) = tokio::sync::oneshot::channel::<()>();

    let writer_db = Arc::clone(&db);
    let writer = tokio::spawn(async move {
        // 本番の書き込みと同じ順序: write_lock を取ってから BEGIN する。
        let guard = crate::write_lock(&writer_db)
            .await
            .unwrap()
            .lock_owned()
            .await;
        let tx = writer_db.transaction().await.unwrap();
        tx.execute(
            "INSERT INTO users (user_hash) VALUES ('concurrent_writer')",
            (),
        )
        .await
        .unwrap();
        write_started_tx.send(()).unwrap();
        // 書き込みトランザクションを意図的に保持する。
        release_rx.await.unwrap();
        tx.commit().await.unwrap();
        drop(guard);
        write_done_tx.send(()).unwrap();
    });

    // 書き込みが共有 Connection 上で BEGIN を保持するまで待つ。
    write_started_rx.await.unwrap();

    let app_for_task = app.clone();
    let mut export_task =
        tokio::spawn(async move { me_export_request(&app_for_task, Some("alice")).await });
    // エクスポート側が BEGIN を試みる機会を確実に与える。
    tokio::task::yield_now().await;

    // 書き込みが終わる前にエクスポートが完了してはいけない。
    // write_lock を取得しない旧実装では、ここで即座に 500 レスポンスが返る。
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut export_task)
            .await
            .is_err(),
        "me_export は書き込みトランザクションの完了を待つべき"
    );

    release_tx.send(()).unwrap();
    write_done_rx.await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(5), &mut export_task)
        .await
        .expect("me_export must finish after the write transaction commits")
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "write_lock 待機後は 500 にならず成功すること"
    );
    writer.await.unwrap();
}

// ---------------------------------------------------------------------------
// R5-04: GET /mod/reports（通報理由の要約化）
// ---------------------------------------------------------------------------

/// 認証付き一覧は要約を返し、DB の全文や末尾センチネルを漏らさない。
#[tokio::test]
async fn list_reports_returns_summarized_reason() {
    let (app, db) = setup_app_with_db().await;
    db.execute(
        "UPDATE contents SET status = 'Reported' WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g'",
        (),
    )
    .await
    .unwrap();

    let full_reason = format!(
        "SENTINEL_HEAD user@example.com {} _TAIL_SENTINEL",
        "x".repeat(130)
    );
    db.execute(
        "INSERT INTO reports (report_id, target_cid, reason) VALUES (1, 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g', ?)",
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

/// GET /content/{cid}/audit の reason は要約化し、ハッシュ検証用の entry_hash は
/// DB の生 reason から計算された値をそのまま返す（表示のみマスク）。
#[tokio::test]
async fn test_get_content_audit_masks_reason_but_preserves_entry_hash() {
    let (app, db) = setup_app_with_db().await;
    let cid = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g";
    let raw_reason = "contact user@example.com https://example.com/a code 1234";
    let previous_log_hash = "GENESIS_HASH";

    let entry = common::AuditLogEntry {
        log_id: None,
        cid: cid.to_string(),
        old_status: "Approved".to_string(),
        new_status: "Reported".to_string(),
        changed_by: "reporter".to_string(),
        reason: raw_reason.to_string(),
        previous_log_hash: previous_log_hash.to_string(),
    };
    let expected_hash = entry.calculate_hash_v2();
    db.execute(
        "INSERT INTO content_audit_log
             (cid, old_status, new_status, changed_by, reason, previous_log_hash, entry_hash)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        (
            cid,
            "Approved",
            "Reported",
            "reporter",
            raw_reason,
            previous_log_hash,
            expected_hash.clone(),
        ),
    )
    .await
    .unwrap();

    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let response = app
        .oneshot(with_peer(
            Request::get(format!("/content/{cid}/audit"))
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
    assert!(!text.contains(raw_reason), "全文が漏れた: {text}");
    assert!(
        !text.contains("user@example.com"),
        "メールアドレスが漏れた: {text}"
    );
    assert!(!text.contains("example.com"), "URL が漏れた: {text}");
    assert!(!text.contains("1234"), "4 桁数字が漏れた: {text}");

    let logs: Vec<crate::AuditLogResponse> = serde_json::from_slice(&body).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(
        logs[0].reason.as_deref(),
        Some(common::summarize_reason(raw_reason).as_str()),
        "レスポンスは summarize_reason の結果であること"
    );
    assert!(
        logs[0]
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("[redacted]"))
    );

    // レスポンスの entry_hash はマスク影響を受けず、生 reason から再計算した値と一致する。
    assert_eq!(logs[0].entry_hash, expected_hash);
    let mut rows = db
        .query("SELECT reason, entry_hash FROM content_audit_log", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let stored_raw: String = row.get(0).unwrap();
    let stored_hash: String = row.get(1).unwrap();
    assert_eq!(stored_raw, raw_reason, "DB には全文が残ること");
    let verify_entry = common::AuditLogEntry {
        reason: stored_raw,
        ..entry
    };
    assert_eq!(verify_entry.calculate_hash_v2(), stored_hash);
    assert_eq!(stored_hash, logs[0].entry_hash);
}

/// content_audit_log の NULL 許容列（cid / old_status / new_status / changed_by /
/// reason / previous_log_hash）が `Option<String>` として読め、NULL 行でも
/// 共通ロジックによる監査連鎖検証が完了することの回帰。
/// sign_audit_export / verify_audit_log の実 CLI ではなく共通ロジック経由で確認する。
#[tokio::test]
async fn test_content_audit_nullable_columns_read_as_option_and_verify() {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    init_schema(&db).await;

    // entry_hash は NOT NULL なので 64 hex を与え、それ以外の 6 列はすべて NULL にする。
    db.execute(
        "INSERT INTO content_audit_log
             (log_id, cid, old_status, new_status, changed_by, reason, previous_log_hash, entry_hash)
         VALUES (1, NULL, NULL, NULL, NULL, NULL, NULL,
                 '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef')",
        (),
    )
    .await
    .unwrap();

    let mut rows = db
        .query(
            "SELECT cid, old_status, new_status, changed_by, reason, previous_log_hash, entry_hash
             FROM content_audit_log ORDER BY log_id",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();

    // NULL 列は Option<String> として読め、すべて None になる（読み取り側の回帰オラクル）。
    let cid: Option<String> = row.get(0).unwrap();
    let old_status: Option<String> = row.get(1).unwrap();
    let new_status: Option<String> = row.get(2).unwrap();
    let changed_by: Option<String> = row.get(3).unwrap();
    let reason: Option<String> = row.get(4).unwrap();
    let previous_log_hash: Option<String> = row.get(5).unwrap();
    assert!(cid.is_none());
    assert!(old_status.is_none());
    assert!(new_status.is_none());
    assert!(changed_by.is_none());
    assert!(reason.is_none());
    assert!(previous_log_hash.is_none());

    // 共通ロジックへは未設定を空文字として渡し、NULL 行でも検証が完了することを確認する。
    let entry = common::AuditLogEntry {
        log_id: Some(1),
        cid: cid.unwrap_or_default(),
        old_status: old_status.unwrap_or_default(),
        new_status: new_status.unwrap_or_default(),
        changed_by: changed_by.unwrap_or_default(),
        reason: reason.unwrap_or_default(),
        previous_log_hash: previous_log_hash.unwrap_or_default(),
    };
    assert_eq!(entry.calculate_hash().len(), 64);
    assert_eq!(entry.calculate_hash_v2().len(), 64);

    let stored_entry_hash: String = row.get(6).unwrap();
    let digest = common::compute_audit_chain_digest(&[(1, stored_entry_hash)]).unwrap();
    assert_eq!(digest.count, 1);
    assert_eq!(digest.first_log_id, Some(1));
    assert_eq!(digest.last_log_id, Some(1));
    assert_eq!(digest.root_hash.len(), 64);
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
         VALUES ('QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111X', 'Illustration', 'test_user', 'mock_checksum', 'Approved')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "UPDATE contents SET status = 'Reported' WHERE cid = 'QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g'",
        (),
    )
    .await
    .unwrap();
    for (id, cid, reason, created_at) in [
        (
            1,
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
            "old reason",
            "2026-01-01 00:00:00",
        ),
        (
            2,
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111X",
            "should not appear",
            "2026-01-02 00:00:00",
        ),
        (
            3,
            "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g",
            "new reason",
            "2026-01-03 00:00:00",
        ),
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

/// R2: GET /mod/reports は MAX_REPORT_LIST 件で切り、全行 Vec 化を防ぐ。
/// SQL の LIMIT を外すと 501 件返ってこのテストが失敗するため、mutation 検出にもなる。
#[tokio::test]
async fn list_reports_caps_at_max_report_list() {
    assert_eq!(crate::MAX_REPORT_LIST, 500);
    let (app, db) = setup_app_with_db().await;
    let cid = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g";
    db.execute(
        "UPDATE contents SET status = 'Reported' WHERE cid = ?",
        [cid.to_string()],
    )
    .await
    .unwrap();

    // 上限 + 1 件投入。created_at は id と単調増加させ、newest-first も検証する。
    let tx = db.transaction().await.unwrap();
    for id in 1..=crate::MAX_REPORT_LIST + 1 {
        let created_at = format!("2026-01-01 00:{:02}:{:02}", id / 60, id % 60);
        tx.execute(
            "INSERT INTO reports (report_id, target_cid, reason, created_at)
             VALUES (?, ?, ?, ?)",
            libsql::params![id, cid.to_string(), format!("reason {id}"), created_at],
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

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
        items.len(),
        crate::MAX_REPORT_LIST as usize,
        "LIMIT が効かず全件返っている"
    );
    // 接続順・レスポンス順（newest first）は従来どおり維持。
    // 501 件投入して 500 件返るため、先頭は最新の 501、末尾は 2 になる。
    assert_eq!(items.first().unwrap().report_id, crate::MAX_REPORT_LIST + 1);
    assert_eq!(
        items[0].reason,
        format!("reason {}", crate::MAX_REPORT_LIST + 1)
    );
}

/// R2: GET /content/{cid}/audit は MAX_AUDIT_LIST 件で切り、全行 Vec 化を防ぐ。
/// 既存の entry_hash / previous_log_hash 連鎖検証（test_audit_log_chain_integrity 等）は
/// DB を直接読むため、この LIMIT 追加の影響を受けず壊れない。
#[tokio::test]
async fn get_content_audit_caps_at_max_audit_list() {
    assert_eq!(crate::MAX_AUDIT_LIST, 500);
    let (app, db) = setup_app_with_db().await;
    let cid = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g";

    let tx = db.transaction().await.unwrap();
    for id in 1..=crate::MAX_AUDIT_LIST + 1 {
        tx.execute(
            "INSERT INTO content_audit_log
                 (log_id, cid, old_status, new_status, changed_by, reason, entry_hash)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            libsql::params![
                id,
                cid.to_string(),
                "Approved".to_string(),
                "Reported".to_string(),
                "reporter".to_string(),
                format!("reason {id}"),
                format!("{id:064x}"),
            ],
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let response = app
        .oneshot(with_peer(
            Request::get(format!("/content/{cid}/audit"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let logs: Vec<crate::AuditLogResponse> = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        logs.len(),
        crate::MAX_AUDIT_LIST as usize,
        "LIMIT が効かず全件返っている"
    );
    // 最新 MAX_AUDIT_LIST 件を新しい順（log_id DESC）で返す。
    // 501 件投入したため最古の log_id 1 は切り捨てられ、2〜501 が返る。
    assert_eq!(logs.first().unwrap().log_id, crate::MAX_AUDIT_LIST + 1);
    assert_eq!(logs.last().unwrap().log_id, 2);
    // 全体が降順（新しい順）であること。ORDER BY を ASC に戻すと失敗する。
    let ids: Vec<i64> = logs.iter().map(|log| log.log_id).collect();
    let expected: Vec<i64> = (2..=crate::MAX_AUDIT_LIST + 1).rev().collect();
    assert_eq!(ids, expected, "最新 500 件が新しい順に返ること");
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

/// 文字列マップを `std::env::var` と同じ Result 契約で読む共通ヘルパ。
///
/// `non_utf8` に変数名を入れると `Err(VarError::NotUnicode(...))` を注入する。
/// 注入した生バイトは診断・エラー文言へ一切渡さず、固定文言のみを検証する。
fn result_lookup<'a>(
    env: &'a std::collections::BTreeMap<String, String>,
    non_utf8: &'a [&'a str],
) -> impl Fn(&str) -> Result<String, std::env::VarError> + 'a {
    move |name| {
        if non_utf8.contains(&name) {
            Err(std::env::VarError::NotUnicode(
                std::ffi::OsString::from_vec(vec![0xff]),
            ))
        } else {
            env.get(name).cloned().ok_or(std::env::VarError::NotPresent)
        }
    }
}

/// 必須 7 変数が揃えば OIDC が有効化され、値がそのまま設定へ反映される。
#[test]
fn test_oidc_from_env_with_all_required_present() {
    let env = oidc_required_env();
    let state = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
        .expect("valid optional OIDC values")
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
            crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
                .expect("no invalid optional values")
                .is_none(),
            "missing {missing} must disable OIDC (fail-closed)"
        );
    }
}

/// 必須変数が Some でも trim 後に空文字なら Err（設定値はエコーしない）。
/// 空の client_id / client_secret では認証が黙って機能不全になるため fail-closed。
#[test]
fn test_oidc_from_env_with_empty_required_values_rejected() {
    const EMPTY_VALUE_ERROR: &str = "OIDC configuration values must not be empty";
    for key in ["OIDC_CLIENT_ID", "OIDC_CLIENT_SECRET"] {
        for raw in ["", "   ", "\t\n"] {
            let mut env = oidc_required_env();
            env.insert(key.to_string(), raw.to_string());
            let err = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
                .err()
                .unwrap_or_else(|| panic!("empty {key} (raw={raw:?}) must be rejected"));
            // 固定文言なので設定値（秘密情報を含む）は一切含まれない。
            assert_eq!(err, EMPTY_VALUE_ERROR, "key={key}, raw={raw:?}");
        }
    }

    // 有値なら有効化される（空文字検証が正常値を誤って拒否しないこと）。
    let env = oidc_required_env();
    let state = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
        .expect("valid OIDC configuration")
        .expect("all required variables present");
    assert_eq!(state.config().client_id, "client-123");
    assert_eq!(state.config().client_secret, "secret-456");

    // 未設定（None）が 1 つでもあれば、別の変数が空でも Ok(None) 契約を維持する。
    let mut env = oidc_required_env();
    env.insert("OIDC_CLIENT_SECRET".to_string(), String::new());
    env.remove("OIDC_JWKS_URI");
    assert!(
        crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
            .expect("empty value with a missing required variable must not be an Err")
            .is_none(),
        "missing required variable must disable OIDC (Ok(None))"
    );

    // 非 UTF-8 の必須値は「未設定」ではなく不正値として起動拒否する。
    // 生バイトは固定文言へ変換され、エラーには一切現れない。
    const NON_UTF8_ERROR: &str = "OIDC configuration values must be valid UTF-8";
    let env = oidc_required_env();
    let err = crate::oidc::OidcState::from_env_with(result_lookup(&env, &["OIDC_CLIENT_SECRET"]))
        .err()
        .expect("non-UTF-8 required value must be rejected");
    assert_eq!(err, NON_UTF8_ERROR);
    assert!(!err.contains('\u{fffd}'), "生バイトを文字列化しない: {err}");
}

/// ClockSkew: 既定 60、1..=300 のみ受け付け、範囲外・不正値は既定へ戻す。
#[test]
fn test_oidc_from_env_with_clock_skew_boundaries() {
    let assert_skew = |value: Option<&str>, expected: u64| {
        let mut env = oidc_required_env();
        if let Some(value) = value {
            env.insert("OIDC_CLOCK_SKEW_SECS".to_string(), value.to_string());
        }
        let state = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
            .expect("valid OIDC configuration")
            .expect("required variables are present");
        assert_eq!(
            state.config().clock_skew_secs,
            expected,
            "OIDC_CLOCK_SKEW_SECS={value:?}"
        );
    };
    assert_skew(None, 60);
    assert_skew(Some("1"), 1);
    assert_skew(Some("60"), 60);
    assert_skew(Some("300"), 300);
    // 範囲外（0 / 301 / u64::MAX）は既定値へフォールバック。
    assert_skew(Some("0"), 60);
    assert_skew(Some("301"), 60);
    assert_skew(Some("18446744073709551615"), 60);
    // u64 に収まらない / 負数 / 空 / 非数値は既定値へフォールバック。
    assert_skew(Some("18446744073709551616"), 60);
    assert_skew(Some("-1"), 60);
    assert_skew(Some(""), 60);
    assert_skew(Some("abc"), 60);
}

/// max_auth_age: 正の整数は保持、0 / 負 / 非数値 / オーバーフローは Err（起動拒否）。
/// 不正値を捨てて「検証しない」にすると auth_time / max_age 検証が黙って無効化されるため、
/// fail-closed にする（R2 / OIDC assurance）。
#[test]
fn test_oidc_from_env_with_max_auth_age_boundaries() {
    let parse_age = |value: Option<&str>| {
        let mut env = oidc_required_env();
        if let Some(value) = value {
            env.insert("OIDC_MAX_AUTH_AGE_SECS".to_string(), value.to_string());
        }
        crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
    };

    // 未設定は「検証しない」（後方互換）。
    assert_eq!(
        *parse_age(None)
            .expect("valid OIDC configuration")
            .expect("required variables are present")
            .config()
            .max_auth_age_secs
            .lock()
            .unwrap(),
        None
    );

    // 正の整数（境界 1 / 通常値）はそのまま保持する。
    // u64::MAX は MAX_AUTH_AGE_MAX_SECS 超過のため下の拒否側へ移す。
    for (raw, expected) in [("1", 1), ("900", 900)] {
        let state = parse_age(Some(raw))
            .expect("valid OIDC configuration")
            .expect("required variables are present");
        assert_eq!(
            *state.config().max_auth_age_secs.lock().unwrap(),
            Some(expected),
            "OIDC_MAX_AUTH_AGE_SECS={raw}"
        );
    }

    // 不正値と上限超過は Err。None にフォールバックして検証を黙って無効化してはならない。
    for raw in [
        "0",
        "-1",
        "abc",
        "",
        "18446744073709551615", // u64::MAX: 上限 2592000 秒を超過
        "18446744073709551616", // オーバーフロー
    ] {
        let err = parse_age(Some(raw))
            .err()
            .expect("invalid OIDC_MAX_AUTH_AGE_SECS must be rejected");
        assert!(
            err.contains("OIDC_MAX_AUTH_AGE_SECS"),
            "error must identify the invalid variable: {err}"
        );
    }

    // 非 UTF-8 も「未設定」ではなく不正値として起動拒否し、auth_time/max_age 検証を
    // 黙って無効化しない。生バイトや置換文字は固定文言へ一切含まれない。
    let env = oidc_required_env();
    let err =
        crate::oidc::OidcState::from_env_with(result_lookup(&env, &["OIDC_MAX_AUTH_AGE_SECS"]))
            .err()
            .expect("non-UTF-8 OIDC_MAX_AUTH_AGE_SECS must be rejected");
    assert!(
        err.contains("must be valid UTF-8"),
        "unexpected error: {err}"
    );
    assert!(
        !err.contains('\u{fffd}'),
        "replacement char must not leak: {err}"
    );
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
            let state = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
                .expect("valid OIDC configuration")
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

/// オプション値の既定と cookie_secure の真偽判定（1/true と 0/false のみ、他は拒否）。
#[test]
fn test_oidc_from_env_with_optional_defaults_and_cookie_secure() {
    let assert_cookie = |value: Option<&str>, expected: bool| {
        let mut env = oidc_required_env();
        env.insert("OIDC_SCOPES".to_string(), "openid profile".to_string());
        if let Some(value) = value {
            env.insert("OIDC_COOKIE_SECURE".to_string(), value.to_string());
        }
        let state = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
            .expect("valid OIDC configuration")
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
    // 受理値外（空文字）は fail-closed で起動拒否。固定文言で値はエコーしない。
    const INVALID: &str = "OIDC_COOKIE_SECURE must be one of 1/true/0/false";
    for raw in ["", "yes", "no"] {
        let mut env = oidc_required_env();
        env.insert("OIDC_COOKIE_SECURE".to_string(), raw.to_string());
        let error = match crate::oidc::OidcState::from_env_with(result_lookup(&env, &[])) {
            Err(error) => error,
            Ok(_) => panic!("invalid OIDC_COOKIE_SECURE must be rejected: raw={raw:?}"),
        };
        // 固定文言との完全一致により、入力値がエコーされていないことを検証する。
        assert_eq!(error, INVALID, "raw={raw:?}");
    }
}

/// OIDC エンドポイント URL: https 必須、mock 時のみ localhost の http を許容。
/// エラー文言は固定で設定値を含まない（情報漏洩を防ぐ）。
#[test]
fn test_oidc_from_env_with_endpoint_url_scheme_validation() {
    const ENDPOINT_KEYS: [&str; 5] = [
        "OIDC_ISSUER",
        "OIDC_REDIRECT_URI",
        "OIDC_AUTHORIZATION_ENDPOINT",
        "OIDC_TOKEN_ENDPOINT",
        "OIDC_JWKS_URI",
    ];
    const INVALID_URL_ERROR: &str = "OIDC endpoints must be https (http is allowed only for localhost when ALLOW_MOCK_AUTH is enabled)";

    // 有効な https（4 エンドポイント + redirect_uri）は有効化される。
    let env = oidc_required_env();
    assert!(
        crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
            .expect("all endpoints are https")
            .is_some(),
        "valid https endpoints must enable OIDC"
    );

    // 本番相当（ALLOW_MOCK_AUTH 未設定）では http はどの変数でも拒否。
    for key in ENDPOINT_KEYS {
        let mut env = oidc_required_env();
        env.insert(
            key.to_string(),
            "http://issuer.example.jp/endpoint".to_string(),
        );
        let err = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
            .err()
            .expect("http endpoint must be rejected in production mode");
        assert_eq!(err, INVALID_URL_ERROR);
        assert!(
            !err.contains("issuer.example.jp"),
            "error must not echo the value"
        );
    }

    // mock 有効（1 / true / TRUE）なら localhost 系の http のみ許容。
    for flag in ["1", "true", "TRUE"] {
        for host in ["localhost", "127.0.0.1", "[::1]"] {
            let mut env = oidc_required_env();
            for key in ENDPOINT_KEYS {
                env.insert(key.to_string(), format!("http://{host}/endpoint"));
            }
            env.insert("ALLOW_MOCK_AUTH".to_string(), flag.to_string());
            assert!(
                crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
                    .expect("localhost http is allowed in mock mode")
                    .is_some(),
                "ALLOW_MOCK_AUTH={flag}, host={host} must be accepted"
            );
        }
    }

    // mock が有効でもリモートの http は拒否（値をエコーしない）。
    let mut env = oidc_required_env();
    env.insert("ALLOW_MOCK_AUTH".to_string(), "1".to_string());
    env.insert(
        "OIDC_ISSUER".to_string(),
        "http://remote.example.jp/issuer".to_string(),
    );
    let err = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
        .err()
        .expect("remote http must be rejected even in mock mode");
    assert_eq!(err, INVALID_URL_ERROR);
    assert!(
        !err.contains("remote.example.jp"),
        "error must not echo the value"
    );

    // 受理値の偽（0 / false）では localhost の http も拒否。
    for flag in ["0", "false"] {
        let mut env = oidc_required_env();
        env.insert("ALLOW_MOCK_AUTH".to_string(), flag.to_string());
        for key in ENDPOINT_KEYS {
            env.insert(key.to_string(), "http://localhost/endpoint".to_string());
        }
        let err = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
            .err()
            .expect("ALLOW_MOCK_AUTH={flag} must not allow http");
        assert_eq!(err, INVALID_URL_ERROR);
    }

    // 受理値外（空 / yes）は共通 parse_bool_env で拒否し、固定文言の起動エラーを返す。
    const INVALID_BOOL: &str = "ALLOW_MOCK_AUTH must be one of 1/true/0/false";
    for flag in ["", "yes"] {
        let mut env = oidc_required_env();
        env.insert("ALLOW_MOCK_AUTH".to_string(), flag.to_string());
        for key in ENDPOINT_KEYS {
            env.insert(key.to_string(), "http://localhost/endpoint".to_string());
        }
        let err = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
            .err()
            .expect("invalid ALLOW_MOCK_AUTH must be rejected");
        assert_eq!(err, INVALID_BOOL, "flag={flag:?}");
    }

    // パース不能・不正スキームも拒否し、設定値をエコーしない。
    for raw in ["not a url", "", "ftp://localhost/issuer", "https://"] {
        let mut env = oidc_required_env();
        env.insert("OIDC_ISSUER".to_string(), raw.to_string());
        let err = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
            .err()
            .expect("invalid URL must be rejected");
        assert_eq!(err, INVALID_URL_ERROR);
        if !raw.is_empty() {
            assert!(!err.contains(raw), "error must not echo the invalid value");
        }
    }
}

/// 抽出した is_valid_oidc_endpoint の直接境界テスト。
/// doctor と from_env_with が同じ判定を共有していることを固定する。
#[test]
fn test_is_valid_oidc_endpoint_host_boundaries() {
    // https は mock 有無にかかわらず有効。
    for allow_mock_auth in [false, true] {
        assert!(crate::oidc::is_valid_oidc_endpoint(
            "https://issuer.example.jp/issuer",
            allow_mock_auth
        ));
    }

    // 本番では http は不可。
    assert!(!crate::oidc::is_valid_oidc_endpoint(
        "http://issuer.example.jp/issuer",
        false
    ));

    // mock 時は localhost 系のみ http を許容。
    for host in ["localhost", "127.0.0.1", "[::1]", "::1"] {
        let url = if host == "::1" {
            "http://[::1]/endpoint".to_string()
        } else {
            format!("http://{host}/endpoint")
        };
        assert!(
            crate::oidc::is_valid_oidc_endpoint(&url, true),
            "host={host}"
        );
    }

    // mock 時でもリモート http と不正 URL は不可。
    assert!(!crate::oidc::is_valid_oidc_endpoint(
        "http://remote.example.jp/issuer",
        true
    ));
    for raw in ["not a url", "", "ftp://localhost/issuer", "https://"] {
        assert!(!crate::oidc::is_valid_oidc_endpoint(raw, true), "raw={raw}");
    }

    // R2-AUTH: userinfo を含む URL はスキーム・mock 設定にかかわらず拒否する。
    // username のみ / username+password / password のみの3形を固定する。
    for raw in [
        "https://user@issuer.example.jp/issuer",
        "https://user:pass@issuer.example.jp/issuer",
        "https://:pass@issuer.example.jp/issuer",
        "http://user@localhost/issuer",
        "http://user:pass@127.0.0.1/issuer",
    ] {
        for allow_mock_auth in [false, true] {
            assert!(
                !crate::oidc::is_valid_oidc_endpoint(raw, allow_mock_auth),
                "userinfo must be rejected: raw={raw}, mock={allow_mock_auth}"
            );
        }
    }
}

// --- 共通 OIDC バリデータ（validate_oidc_config_with） -----------------------

/// 必須変数の有無・空値・ハードエラーを `OidcState` なしで判定できる。
#[test]
fn test_validate_oidc_config_with_required_and_hard_errors() {
    let env = oidc_required_env();
    let result: crate::OidcConfigValidation =
        crate::validate_oidc_config_with(result_lookup(&env, &[]));
    assert!(result.enabled);
    assert!(result.required_present);
    assert!(result.hard_errors.is_empty());
    assert!(result.cookie_secure);
    assert_eq!(result.clock_skew_secs, 60);
    assert_eq!(result.max_auth_age_secs, None);
    assert_eq!(result.acr_amr_present, (false, false));

    // 必須変数が欠けると enabled=false（from_env_with の Ok(None) と同じ条件）。
    let mut partial = oidc_required_env();
    partial.remove("OIDC_JWKS_URI");
    let result = crate::validate_oidc_config_with(result_lookup(&partial, &[]));
    assert!(!result.enabled);
    assert!(!result.required_present);

    // Some でも trim 後に空文字なら required_present=false（production 警告へ渡す値）。
    // 7 変数はすべて設定されているため enabled は真のまま。
    let mut empty = oidc_required_env();
    empty.insert("OIDC_CLIENT_SECRET".to_string(), String::new());
    let result = crate::validate_oidc_config_with(result_lookup(&empty, &[]));
    assert!(result.enabled);
    assert!(!result.required_present);
    assert_eq!(
        result.hard_errors,
        vec!["OIDC configuration values must not be empty"]
    );

    // 空文字と欠落が同時にある場合は Ok(None) 側が優先される。
    let mut empty_and_missing = oidc_required_env();
    empty_and_missing.insert("OIDC_CLIENT_SECRET".to_string(), String::new());
    empty_and_missing.remove("OIDC_JWKS_URI");
    let result = crate::validate_oidc_config_with(result_lookup(&empty_and_missing, &[]));
    assert!(!result.enabled);
    assert!(!result.required_present);
    assert!(result.hard_errors.is_empty());

    // 非 UTF-8 の必須値は値が存在するため enabled=true の不正値として扱い、
    // 固定文言をハードエラーへ積む（生バイトは保持・露出しない）。
    const NON_UTF8_ERROR: &str = "OIDC configuration values must be valid UTF-8";
    let result = crate::validate_oidc_config_with(result_lookup(&env, &["OIDC_CLIENT_SECRET"]));
    assert!(result.enabled, "値が存在するため enabled は真");
    assert!(!result.required_present, "不正値は充足扱いにしない");
    assert_eq!(result.hard_errors, vec![NON_UTF8_ERROR]);

    // 保証レベル強制（acr/amr/scopes）と clock skew の NonUtf8 も
    // 「未設定」へ倒さず固定文言で起動拒否する（fail-open 防止）。
    for name in [
        "OIDC_REQUIRED_ACR",
        "OIDC_REQUIRED_AMR",
        "OIDC_SCOPES",
        "OIDC_CLOCK_SKEW_SECS",
    ] {
        let validated =
            crate::validate_oidc_config_with(result_lookup(&env, &["OIDC_CLIENT_SECRET", name]));
        let errors = format!("{:?}", validated.hard_errors);
        assert!(
            validated.hard_errors.contains(&NON_UTF8_ERROR),
            "{name} の NonUtf8 が harg_errors に積まれない: {errors}"
        );
        assert!(!errors.contains("0xff"), "生バイトが露出: {errors}");
    }
}

/// ハードエラーは固定文言のみで、秘密値も設定値も保持・露出しない。
#[test]
fn test_validate_oidc_config_with_errors_are_fixed_and_secret_free() {
    const OIDC_SECRET: &str = "oidc-super-secret-value-1234567890-abcdef";
    const REMOTE_URL: &str = "http://remote-secret.example.jp/issuer";
    let mut env = oidc_required_env();
    env.insert("OIDC_CLIENT_SECRET".to_string(), OIDC_SECRET.to_string());
    env.insert("OIDC_ISSUER".to_string(), REMOTE_URL.to_string());
    env.insert("OIDC_MAX_AUTH_AGE_SECS".to_string(), "0".to_string());
    let result = crate::validate_oidc_config_with(result_lookup(&env, &[]));

    assert_eq!(
        result.hard_errors,
        vec![
            "OIDC endpoints must be https (http is allowed only for localhost when ALLOW_MOCK_AUTH is enabled)",
            "OIDC_MAX_AUTH_AGE_SECS must be greater than 0",
        ]
    );
    let debug = format!("{result:?}");
    assert!(!debug.contains(OIDC_SECRET), "秘密値が露出: {debug}");
    assert!(!debug.contains(REMOTE_URL), "設定値が露出: {debug}");
    for error in &result.hard_errors {
        assert!(!error.contains(OIDC_SECRET));
        assert!(!error.contains(REMOTE_URL));
    }

    // R2-AUTH: userinfo を含む URL も同じ固定文言で起動拒否になる。
    // 秘密情報（userinfo の値）は一切エコーしない。
    const USERINFO_SECRET: &str = "embedded-user-secret";
    let mut userinfo_env = oidc_required_env();
    userinfo_env.insert(
        "OIDC_ISSUER".to_string(),
        format!("https://{USERINFO_SECRET}@issuer.example.jp/issuer"),
    );
    let userinfo_result = crate::validate_oidc_config_with(result_lookup(&userinfo_env, &[]));
    assert_eq!(
        userinfo_result.hard_errors,
        vec![
            "OIDC endpoints must be https (http is allowed only for localhost when ALLOW_MOCK_AUTH is enabled)"
        ],
        "userinfo rejection must reuse the fixed startup/doctor message"
    );
    let debug = format!("{userinfo_result:?}");
    assert!(
        !debug.contains(USERINFO_SECRET),
        "userinfo was echoed: {debug}"
    );
}

/// オプション値の実効値が from_env_with が組み立てる設定と一致する。
#[test]
fn test_validate_oidc_config_with_optional_values_match_from_env_with() {
    let mut env = oidc_required_env();
    env.insert("OIDC_COOKIE_SECURE".to_string(), "false".to_string());
    env.insert("OIDC_CLOCK_SKEW_SECS".to_string(), "120".to_string());
    env.insert("OIDC_MAX_AUTH_AGE_SECS".to_string(), "900".to_string());
    env.insert(
        "OIDC_REQUIRED_ACR".to_string(),
        " urn:acr:1, , urn:acr:2 ".to_string(),
    );
    env.insert("OIDC_REQUIRED_AMR".to_string(), " pwd, otp ".to_string());

    let result = crate::validate_oidc_config_with(result_lookup(&env, &[]));
    assert!(!result.cookie_secure);
    assert_eq!(result.clock_skew_secs, 120);
    assert_eq!(result.max_auth_age_secs, Some(900));
    assert_eq!(result.acr_amr_present, (true, true));
    assert!(result.hard_errors.is_empty());

    let state = crate::oidc::OidcState::from_env_with(result_lookup(&env, &[]))
        .expect("valid OIDC configuration")
        .expect("required variables are present");
    assert_eq!(state.config().cookie_secure, result.cookie_secure);
    assert_eq!(state.config().clock_skew_secs, result.clock_skew_secs);
    assert_eq!(
        *state.config().max_auth_age_secs.lock().unwrap(),
        result.max_auth_age_secs
    );
    assert_eq!(state.config().required_acr.lock().unwrap().len(), 2);
    assert_eq!(state.config().required_amr.lock().unwrap().len(), 2);

    // 範囲外の clock_skew は既定へフォールバックし、生値との差で doctor が警告する。
    let mut out_of_range = oidc_required_env();
    out_of_range.insert("OIDC_CLOCK_SKEW_SECS".to_string(), "301".to_string());
    let result = crate::validate_oidc_config_with(result_lookup(&out_of_range, &[]));
    assert_eq!(result.clock_skew_secs, crate::DEFAULT_CLOCK_SKEW_SECS);
    assert!(result.hard_errors.is_empty());
}

// ---- proptest: 入力境界と仕様不変条件の網羅 ----
//
// fuzz_test.rs の年齢プロパティに続き、CID 検証・金額変換・通報理由要約の
// 「任意入力で panic しない」ことと「仕様から導ける独立な判定」を検証する。
// 実行時間を抑えるため各プロパティの cases は 128 に設定している。
mod property_tests {
    use crate::payments::{MAX_ORDER_JPY, jpy_to_wei};
    use proptest::prelude::*;

    /// CIDv0 の base58btc アルファベット（仕様上の定義をテスト側で持つ）。
    const BASE58BTC: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    /// CIDv1 の base32lower アルファベット（パディング無し）。
    const BASE32LOWER: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    const KNOWN_CIDV0: &str = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5AXQfD";
    const KNOWN_CIDV1: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// 任意の u64 で panic せず、0 と上限超えは Err、
        /// 範囲内は 10^18 スケールの独立参照値と一致する。
        #[test]
        fn prop_jpy_to_wei_boundaries_and_reference(
            amount in prop_oneof![
                Just(0u64),
                Just(MAX_ORDER_JPY),
                Just(MAX_ORDER_JPY + 1),
                Just(u64::MAX),
                any::<u64>(),
            ]
        ) {
            let result = jpy_to_wei(amount);
            if amount == 0 || amount > MAX_ORDER_JPY {
                prop_assert!(result.is_err(), "amount {amount} must be rejected");
            } else {
                // ADR-0003: JPYC は 10^18 最小単位。範囲内なら u128 で溢れない。
                const WEI_PER_JPY: u128 = 1_000_000_000_000_000_000;
                let expected = (u128::from(amount) * WEI_PER_JPY).to_string();
                prop_assert_eq!(result.expect("in-range amount"), expected);
            }
        }

        /// 任意の文字列（ASCII／Unicode 混入・長さ境界を含む）で panic せず、
        /// 受理された値は CID 仕様の文字集合と形式を満たす。
        #[test]
        fn prop_is_valid_cid_only_accepts_canonical_ascii(
            input in prop_oneof![
                Just(KNOWN_CIDV0.to_string()),
                Just(KNOWN_CIDV1.to_string()),
                Just(String::new()),
                Just("🍣".repeat(50)),
                Just("a".repeat(129)),
                any::<String>(),
            ]
        ) {
            let accepted = common::is_valid_cid(&input);

            // 空・非 ASCII・128 バイト超は実装の上限ガードで必ず拒否される。
            if input.is_empty() || !input.is_ascii() || input.len() > 128 {
                prop_assert!(
                    !accepted,
                    "empty / non-ASCII / oversized input must be rejected: {input:?}"
                );
            }

            // 受理側の独立 oracle: v0 は Qm + base58btc 44 文字（計 46）、
            // v1 は b + base32lower（1 文字以上）。それ以外は受理しない。
            if accepted {
                let v0 = input.len() == 46
                    && input.starts_with("Qm")
                    && input
                        .as_bytes()[2..]
                        .iter()
                        .all(|byte| BASE58BTC.contains(byte));
                let v1 = input.starts_with('b')
                    && input.len() > 1
                    && input
                        .as_bytes()[1..]
                        .iter()
                        .all(|byte| BASE32LOWER.contains(byte));
                prop_assert!(v0 || v1, "unexpected accepted CID shape: {input:?}");
            }

            // 既知の正規 CID を落とす always-false 実装を検出する。
            if input == KNOWN_CIDV0 || input == KNOWN_CIDV1 {
                prop_assert!(accepted, "known valid CID must be accepted: {input:?}");
            }
        }

        /// 任意の Unicode 文字列で panic せず、結果は 120 Unicode 文字以下、
        /// 空文字の対応と冪等性を保つ。
        #[test]
        fn prop_summarize_reason_is_bounded_and_idempotent(input in any::<String>()) {
            let summarized = common::summarize_reason(&input);
            prop_assert!(
                summarized.chars().count() <= 120,
                "summary must fit in 120 chars: {input:?}"
            );
            prop_assert_eq!(
                summarized.is_empty(),
                input.is_empty(),
                "emptiness must be preserved: {:?}",
                input
            );
            prop_assert_eq!(
                common::summarize_reason(&summarized),
                summarized,
                "summarization must be idempotent: {:?}",
                input
            );
        }

        /// PII を含まない文字列は 120 文字まで等価に保たれ、
        /// 超過時は先頭 119 文字 + `…` に切り詰められる（Unicode 境界を守る）。
        #[test]
        fn prop_summarize_reason_truncates_safe_text_at_char_boundary(
            chars in prop::collection::vec(
                prop_oneof![
                    Just('あ'),
                    Just('知'),
                    Just('A'),
                    Just('b'),
                    Just(' '),
                    Just('\n'),
                    Just('🍣'),
                    Just('。'),
                ],
                0..=160usize
            )
        ) {
            // '@' / ':' / '.' / 数字を含まないため PII 置換は発生しない。
            let input: String = chars.into_iter().collect();
            let expected = if input.chars().count() <= 120 {
                input.clone()
            } else {
                let mut truncated: String = input.chars().take(119).collect();
                truncated.push('…');
                truncated
            };
            prop_assert_eq!(common::summarize_reason(&input), expected);
        }
    }
}

// ---- write_lock 回帰: 共有 Connection への autocommit 書き込みを禁止する ----

/// 書き込み tx が write_lock を保持している間、お気に入り追加は完了しない。
/// ロック外の autocommit INSERT だと他 tx の BEGIN に取り込まれ、COMMIT/ROLLBACK
/// のタイミングが外部ライターに握られるため、この待機が回帰を検出する。
#[tokio::test]
async fn test_favorite_write_waits_for_write_lock() {
    let (app, db) = setup_app_with_db().await;
    let token = generate_token(TEST_JWT_SECRET, "test_user").await;
    let cid = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g";

    let (started, release, writer) = spawn_lock_holding_writer(Arc::clone(&db));
    started.await.unwrap();

    let app_for_task = app.clone();
    let token_for_task = token.clone();
    let cid_for_task = cid.to_string();
    let mut favorite_task =
        tokio::spawn(
            async move { post_favorite(&app_for_task, &token_for_task, &cid_for_task).await },
        );
    tokio::task::yield_now().await;

    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut favorite_task)
            .await
            .is_err(),
        "favorite INSERT must wait for the in-flight write transaction"
    );

    release.send(()).unwrap();
    writer.await.unwrap();

    let (status, body) = tokio::time::timeout(Duration::from_secs(5), &mut favorite_task)
        .await
        .expect("favorite must finish after the write transaction commits")
        .unwrap();
    assert_eq!(status, StatusCode::OK, "response: {body}");
    assert_eq!(body, json!({"is_favorite": true}));
    assert_eq!(favorite_count(&db, "test_user", cid).await, 1);
}

/// `/auth/logout` の sid 失効書き込みも write_lock を取得し、
/// 他 tx の完了後に成功する（ロック保持中に 200 を返さない）。
#[tokio::test]
async fn test_logout_write_waits_for_write_lock() {
    let (app, db) = setup_app_with_db().await;
    let sid = "sid-wait-for-write-lock";
    let claims = Claims {
        sub: "test_user".to_string(),
        exp: 10000000000,
        sid: Some(sid.to_string()),
        iat: None,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_ref()).unwrap(),
    )
    .unwrap();

    let (started, release, writer) = spawn_lock_holding_writer(Arc::clone(&db));
    started.await.unwrap();

    let mut logout_task = tokio::spawn(async move {
        app.clone()
            .oneshot(with_peer(
                Request::post("/auth/logout")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap()
    });
    tokio::task::yield_now().await;

    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut logout_task)
            .await
            .is_err(),
        "logout must wait for the in-flight write transaction"
    );

    release.send(()).unwrap();
    writer.await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(5), &mut logout_task)
        .await
        .expect("logout must finish after the write transaction commits")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut rows = db
        .query(
            "SELECT sid FROM revoked_sessions WHERE sid = ?",
            [sid.to_string()],
        )
        .await
        .unwrap();
    assert!(
        rows.next().await.unwrap().is_some(),
        "revoked sid must be committed after the lock is released"
    );
}

/// back-channel logout の DB 更新（sid / sub / refresh token）も tx 内で行われ、
/// 外部 HTTP 検証はロック取得より前に完了している。
#[tokio::test]
async fn test_backchannel_logout_write_waits_for_write_lock() {
    let (app, oidc, mock, db) = setup_app_with_oidc().await;
    let body = oidc_login(&app, oidc.as_ref(), &mock).await;
    let claims = jsonwebtoken::decode::<common::Claims>(
        body["token"].as_str().unwrap(),
        &jsonwebtoken::DecodingKey::from_secret(TEST_JWT_SECRET.as_bytes()).unwrap(),
        &jsonwebtoken::Validation::default(),
    )
    .unwrap()
    .claims;
    let sid = claims.sid.expect("session id");
    let logout_token = mock.logout_token_for_sid_and_sub(&sid, &claims.sub);

    let (started, release, writer) = spawn_lock_holding_writer(Arc::clone(&db));
    started.await.unwrap();

    let mut logout_task = tokio::spawn(async move {
        app.clone()
            .oneshot(with_peer(
                Request::post("/auth/backchannel-logout")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("logout_token={logout_token}")))
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap()
    });
    tokio::task::yield_now().await;

    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut logout_task)
            .await
            .is_err(),
        "back-channel logout must wait for the in-flight write transaction"
    );

    release.send(()).unwrap();
    writer.await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(5), &mut logout_task)
        .await
        .expect("back-channel logout must finish after the write transaction commits")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut rows = db
        .query(
            "SELECT sid FROM revoked_sessions WHERE sid = ?",
            [sid.clone()],
        )
        .await
        .unwrap();
    assert!(rows.next().await.unwrap().is_some());
    let mut rows = db
        .query(
            "SELECT sub FROM revoked_subs WHERE sub = ?",
            [claims.sub.clone()],
        )
        .await
        .unwrap();
    assert!(rows.next().await.unwrap().is_some());
}

/// 期限切れレコードの purge（3 種類すべて）も write_lock を取得する。
/// ロックを外すと他 tx の BEGIN 中に DELETE が取り込まれ、ここで完了してしまう。
#[tokio::test]
async fn test_purge_waits_for_write_lock() {
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
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
         VALUES ('expired', 'u', 'f', '2000-01-01T00:00:00+00:00')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO revoked_sessions (sid, revoked_at) VALUES ('old', '2000-01-01 00:00:00')",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES ('old', ?)",
        [chrono::Utc::now().timestamp() - 30 * 24 * 60 * 60 - 3600],
    )
    .await
    .unwrap();

    let (started, release, writer) = spawn_lock_holding_writer(Arc::clone(&db));
    started.await.unwrap();

    let db_refresh = Arc::clone(&db);
    let db_sessions = Arc::clone(&db);
    let db_subs = Arc::clone(&db);
    let mut refresh_task =
        tokio::spawn(async move { crate::purge_expired_refresh_tokens(&db_refresh).await });
    let mut sessions_task =
        tokio::spawn(async move { crate::purge_stale_revoked_sessions(&db_sessions).await });
    let mut subs_task =
        tokio::spawn(async move { crate::purge_stale_revoked_subs(&db_subs).await });
    tokio::task::yield_now().await;

    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut refresh_task)
            .await
            .is_err(),
        "refresh token purge must wait for the in-flight write transaction"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut sessions_task)
            .await
            .is_err(),
        "revoked session purge must wait for the in-flight write transaction"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut subs_task)
            .await
            .is_err(),
        "revoked sub purge must wait for the in-flight write transaction"
    );

    release.send(()).unwrap();
    writer.await.unwrap();

    let purged = tokio::time::timeout(Duration::from_secs(5), &mut refresh_task)
        .await
        .expect("refresh token purge must finish after the write transaction commits")
        .unwrap()
        .unwrap();
    assert_eq!(purged, Some(1));
    let purged = tokio::time::timeout(Duration::from_secs(5), &mut sessions_task)
        .await
        .expect("revoked session purge must finish after the write transaction commits")
        .unwrap()
        .unwrap();
    assert_eq!(purged, Some(1));
    let purged = tokio::time::timeout(Duration::from_secs(5), &mut subs_task)
        .await
        .expect("revoked sub purge must finish after the write transaction commits")
        .unwrap()
        .unwrap();
    assert_eq!(purged, Some(1));
}

/// 周期 purge は write_lock の取得だけを PURGE_LOCK_WAIT で待ち、
/// ビジーなら SQL を実行せずスキップする（Ok(None)）。
/// mutation でロック取得側の timeout を外すと完了せず失敗する。
#[tokio::test]
async fn test_periodic_purge_skips_without_running_sql_when_lock_is_busy() {
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
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
         VALUES ('expired', 'u', 'f', '2000-01-01T00:00:00+00:00')",
        (),
    )
    .await
    .unwrap();

    let (started, release, writer) = spawn_lock_holding_writer(Arc::clone(&db));
    started.await.unwrap();

    let outcome = tokio::time::timeout(Duration::from_secs(5), async {
        crate::purge_expired_refresh_tokens(&db).await
    })
    .await
    .expect("periodic purge must give up on the write lock within PURGE_LOCK_WAIT")
    .unwrap();
    assert_eq!(
        outcome, None,
        "a busy write lock must be skipped without running SQL"
    );

    release.send(()).unwrap();
    writer.await.unwrap();

    // ロックが空けば今度は削除まで完了する（SQL はタイムアウトなしで走る）。
    assert_eq!(
        crate::purge_expired_refresh_tokens(&db).await.unwrap(),
        Some(1)
    );
}

/// 周期 tick の finalize が write_lock 取得でタイムアウトしたときだけ
/// "write lock is busy" の warn を出し、次回 tick に回して shutdown を遅らせない。
/// 2 段構造では SQL の遅延を busy と誤報しないため、warn 文言まで確認する。
/// mutation でロック取得側の timeout を外すとスキップ warn が出ず失敗する。
#[tokio::test]
async fn test_periodic_finalize_lock_wait_is_bounded() {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;
    // contents.owner_hash は users(user_hash) を参照するため、所有者を用意する。
    db.execute("INSERT INTO users (user_hash) VALUES ('owner')", ())
        .await
        .unwrap();

    let logs = LogMessages::default();
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let metrics = Arc::new(crate::Metrics::default());
    let subscriber = tracing_subscriber::layer::SubscriberExt::with(
        tracing_subscriber::registry(),
        logs.clone(),
    );
    // current_thread ランタイムの spawn タスクも同じスレッドで poll されるため、
    // スレッドローカルの default subscriber で finalize の warn を拾える。
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);

    {
        let finalizer = tokio::spawn(crate::run_moderation_finalizer(
            Arc::clone(&db),
            None,
            Duration::from_millis(100),
            shutdown_rx,
            Arc::clone(&metrics),
            None,
        ));
        // 起動時の purge/finalize（ここでは対象 0 件）が終わるのを待つ。
        tokio::time::sleep(Duration::from_millis(200)).await;

        // ライター取得前にスナップショットを取り、差分で finalize 側の
        // インクリメントまで検証できるようにする。
        let lock_busy_before = metrics.finalizer_lock_busy.load(Ordering::Relaxed);

        // ライターが write_lock を保持したまま期限切れ通報を作る。
        let (started, release, writer) = spawn_report_lock_holding_writer(Arc::clone(&db));
        started.await.unwrap();

        // 周期 purge 群（3 × PURGE_LOCK_WAIT）のあと finalize も
        // ロック取得だけ PURGE_LOCK_WAIT で諦め、busy の warn が出るまで待つ。
        tokio::time::timeout(Duration::from_secs(12), async {
            while !logs.contains("Moderation finalization skipped: write lock is busy") {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("periodic finalization must give up on the write lock");
        // 1 tick で purge 3 件 + finalize 1 件 = 4 件の増分を確認する。
        // finalize 側の fetch_add を外すと 3 件で止まるためこのアサートが失敗する。
        let lock_busy_delta = metrics
            .finalizer_lock_busy
            .load(Ordering::Relaxed)
            .saturating_sub(lock_busy_before);
        assert!(
            lock_busy_delta >= 4,
            "3 purge skips plus 1 finalize skip must each increment lock_busy (delta={lock_busy_delta})"
        );

        // ロックが保持されたままでも、finalize が PURGE_LOCK_WAIT を超えて
        // 待たないこと（上の warn）を確認できれば、shutdown 分岐は遅延なく選ばれる。
        // tick 分岐が進行中の場合はその分岐が終わってから次回ループで停止するため、
        // ここでは最終的に停止することを確認する。
        shutdown_tx.send(()).expect("shutdown signal");
        tokio::time::timeout(Duration::from_secs(20), finalizer)
            .await
            .expect("shutdown must not wait for the write lock to be released")
            .expect("finalizer must stop cleanly");

        release.send(()).unwrap();
        writer.await.unwrap();
    }
}

/// 認証時のユーザー登録 INSERT も write_lock を取得してから実行する。
#[tokio::test]
async fn test_auth_verify_write_waits_for_write_lock() {
    let (app, db) = setup_app_with_db().await;
    let body = verify_body("2000-01-01", "JP");

    let (started, release, writer) = spawn_lock_holding_writer(Arc::clone(&db));
    started.await.unwrap();

    // 後続の BAN 回帰アサーションでも app を使うため、spawn には clone を渡す。
    let spawn_app = app.clone();
    let mut verify_task = tokio::spawn(async move {
        spawn_app
            .oneshot(with_peer(
                Request::post("/auth/verify")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
                test_ipv6_peer(),
            ))
            .await
            .unwrap()
    });
    tokio::task::yield_now().await;

    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut verify_task)
            .await
            .is_err(),
        "auth verify must wait for the in-flight write transaction"
    );

    release.send(()).unwrap();
    writer.await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(5), &mut verify_task)
        .await
        .expect("auth verify must finish after the write transaction commits")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // BAN 原子化の契约回归: BAN 済みユーザーが 403 で拒否されることを確認する。
    // この顺序では事前 ensure_user_not_banned が先に拒否するため、write tx 内再読
    // （auth_verify の BAN 原子化）そのものは别途コードレビューで担保する。
    // 新規 test 関数は追加せず、既存の auth_verify テスト内にアサーションを追加する。
    let banned_hash = sha256_hex(b"2000-01-01:JP");
    db.execute(
        "INSERT INTO users (user_hash, is_banned) VALUES (?, TRUE) \
         ON CONFLICT(user_hash) DO UPDATE SET is_banned = TRUE, ban_until = NULL",
        [banned_hash],
    )
    .await
    .unwrap();
    let banned_response = app
        .oneshot(with_peer(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(verify_body("2000-01-01", "JP")))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(banned_response.status(), StatusCode::FORBIDDEN);
    let banned_body = to_bytes(banned_response.into_body(), 4096).await.unwrap();
    assert!(String::from_utf8_lossy(&banned_body).contains("User is banned"));
}

/// `/auth/verify` の再認証で in-memory の失効閾値を消しても DB に残ると、
/// 再起動後に refresh が塞がる。OIDC callback と同じ順序で
/// `delete_superseded_revocation` を呼ぶことを担保する回帰テスト。
/// mutation で DB 削除を外すと refresh が 401 になり失敗する。
#[tokio::test]
async fn test_auth_verify_clears_db_revocation_before_restart() {
    let db = Arc::new(
        Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap(),
    );
    init_schema(&db).await;

    // public_key_der 無しのモック認証では user_hash = sha256("birth_date:country_code")。
    let birth_date = "2000-01-01";
    let country_code = "JP";
    let sub = sha256_hex(format!("{birth_date}:{country_code}").as_bytes());
    let old_threshold = chrono::Utc::now().timestamp() as usize;

    // back-channel logout 相当: 閾値を DB と in-memory の両方へ記録する。
    db.execute(
        "INSERT INTO revoked_subs (sub, revoked_at) VALUES (?, ?)",
        (sub.clone(), old_threshold as i64),
    )
    .await
    .unwrap();
    let revoked_subs_before: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>> =
        Arc::new(std::sync::Mutex::new(
            [(sub.clone(), old_threshold)].into_iter().collect(),
        ));
    let state = Arc::new(AppState {
        db: Arc::clone(&db),
        http_client: test_http_client(),
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
        revoked_subs_before: Arc::clone(&revoked_subs_before),
        age_policy: common::age::AgePolicy::default(),
        moderation_operators: Arc::new(std::collections::HashSet::new()),
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        metrics: Arc::new(crate::Metrics::default()),
        expose_internal_errors: false,
        rate_limit: None,
        pinata_jwt: None,
        allowed_origins: vec![HeaderValue::from_static("http://localhost:8080")],
    });

    // 再認証: 新しい iat を発行し、メモリの旧閾値を remove する。
    let request = VerifyRequest {
        presentation_token: "mock_token".to_string(),
        nonce: Some(crate::issue_mock_nonce_for_test()),
        public_key_der: None,
        birth_date: Some(birth_date.to_string()),
        country_code: Some(country_code.to_string()),
        proof_of_possession: None,
        verified_claims: None,
    };
    let verified = crate::auth_verify(State(Arc::clone(&state)), Json(request))
        .await
        .unwrap();
    assert!(verified.authorized);
    assert!(revoked_subs_before.lock().unwrap().is_empty());

    // 再起動後を模し、再認証で発行した未使用の refresh token を用意する。
    // refresh の失効判定は in-memory ではなく DB を見るため、DB 削除漏れは 401 になる。
    let refresh_token = "post_reauthentication_refresh_token";
    let hash = hex::encode(Sha2Sha256::digest(refresh_token.as_bytes()));
    db.execute(
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
         VALUES (?, ?, ?, ?)",
        (
            hash,
            sub.clone(),
            "family".to_string(),
            (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339(),
        ),
    )
    .await
    .unwrap();

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("refresh_token={refresh_token}")).unwrap(),
    );
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("http://localhost:8080"),
    );
    let response = crate::oidc::refresh(State(Arc::clone(&state)), headers)
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "re-authentication must clear the DB threshold so refresh survives a restart"
    );

    let mut rows = db
        .query("SELECT 1 FROM revoked_subs WHERE sub = ?", [sub])
        .await
        .unwrap();
    assert!(
        rows.next().await.unwrap().is_none(),
        "stale revocation threshold must be deleted from DB too"
    );
}

/// 起動時の write_lock 待ちに上限があり、ビジーでも warn を出して
/// 起動処理（周期ループへの移行）を続行することを検証する。
/// mutation で上限を None（無期限）に戻すと 5 秒の timeout で失敗する。
#[tokio::test]
async fn test_startup_lock_wait_is_bounded_and_startup_continues() {
    assert_eq!(
        crate::STARTUP_LOCK_WAIT,
        Duration::from_secs(30),
        "production startup lock wait must be capped"
    );

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
    // ロックが空けば周期 tick で削除される期限切れトークン。
    db.execute(
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at)
         VALUES ('startup-expired', 'u', 'f', '2000-01-01T00:00:00+00:00')",
        (),
    )
    .await
    .unwrap();

    // 起動中に write_lock を長保持するライター。
    let (started, release, writer) = spawn_lock_holding_writer(Arc::clone(&db));
    started.await.unwrap();

    let logs = LogMessages::default();
    let subscriber = tracing_subscriber::layer::SubscriberExt::with(
        tracing_subscriber::registry(),
        logs.clone(),
    );
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let finalizer = tokio::spawn(crate::run_moderation_finalizer_with_startup_wait(
        Arc::clone(&db),
        None,
        Duration::from_millis(100),
        Duration::from_millis(300), // テスト用の短い上限
        shutdown_rx,
        Arc::new(crate::Metrics::default()),
        None,
    ));

    // 4 つの起動時回収すべてが上限で打ち切られ、warn が出ること。
    // 本番上限の 30 秒を待つようなら 5 秒の timeout で失敗する。
    tokio::time::timeout(Duration::from_secs(5), async {
        while !logs.contains("Moderation finalization skipped on startup: write lock is busy") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("startup lock waits must be aborted at the configured limit");
    assert!(
        logs.contains("Refresh token purge skipped on startup: write lock is busy"),
        "startup purge timeout must emit a busy warning"
    );

    // ロックを解放すると周期ループへ進み、保留だった回収が走る。
    release.send(()).unwrap();
    writer.await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !logs.contains_exact("Purged 1 expired refresh token(s)") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("finalizer must continue to periodic ticks after startup lock timeout");

    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), finalizer)
        .await
        .expect("finalizer must stop cleanly")
        .expect("finalizer task must not panic");
}

/// 周期 finalize はロック取得後に SELECT/COMMIT をタイムアウトなしで
/// 件数分すべて完了させることを検証する。
///
/// ライターは起動時刻からの固定 sleep を使わず、finalize が write_lock 待ちへ
/// 入ったことをメトリクスとログで同期してからチャネルで解放される。完了条件は
/// ログ文字列ではなく DB の `concluded_at` 更新件数で判定する。
/// mutation でロック取得＋SQL 全体を PURGE_LOCK_WAIT で包む旧実装に戻すと、
/// 待ちの期限直前に解放された残り予算では SQL が中途中断され全件完了できない。
#[tokio::test]
async fn test_periodic_finalize_completes_all_reports_after_lock_acquired() {
    const REPORT_COUNT: usize = 10000;
    // PURGE_LOCK_WAIT(2s) の期限直前に解放するための残り予算。
    // 起動時刻ではなく「finalize がロック待ちへ入った時点」から計測する。
    const RELEASE_MARGIN: Duration = Duration::from_millis(1800);

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
    db.execute(
        &format!(
            "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             SELECT 'QmBulk' || printf('%05d', x), 'Novel', 'owner', 'abc', 'Reported'
             FROM (WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM cnt WHERE x < {REPORT_COUNT}) SELECT x FROM cnt)"
        ),
        (),
    )
    .await
    .unwrap();
    db.execute(
        &format!(
            "INSERT INTO reports (report_id, target_cid, reason, deadline_at)
             SELECT x, 'QmBulk' || printf('%05d', x), 'bulk', datetime('now', '-1 day')
             FROM (WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM cnt WHERE x < {REPORT_COUNT}) SELECT x FROM cnt)"
        ),
        (),
    )
    .await
    .unwrap();

    let logs = LogMessages::default();
    let subscriber = tracing_subscriber::layer::SubscriberExt::with(
        tracing_subscriber::registry(),
        logs.clone(),
    );
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let metrics = Arc::new(crate::Metrics::default());

    // ライターは固定 sleep ではなく、テストが同期チャネルで送る解放通知を待つ。
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let db_writer = Arc::clone(&db);
    let writer = tokio::spawn(async move {
        let guard = crate::write_lock(&db_writer)
            .await
            .unwrap()
            .lock_owned()
            .await;
        started_tx.send(()).unwrap();
        release_rx.await.unwrap();
        drop(guard);
    });
    started_rx.await.unwrap();

    let finalizer = tokio::spawn(crate::run_moderation_finalizer_with_startup_wait(
        Arc::clone(&db),
        None,
        Duration::from_millis(50),
        Duration::from_millis(100), // 起動時 finalize はビジーでスキップさせる
        shutdown_rx,
        Arc::clone(&metrics),
        None,
    ));

    // 起動時回収（purge 3 件 + finalize 1 件）が上限で打ち切られた時点を同期点にする。
    tokio::time::timeout(Duration::from_secs(5), async {
        while !logs.contains("Moderation finalization skipped on startup: write lock is busy") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("startup cleanup must skip while the writer holds the lock");
    let lock_busy_after_startup = metrics.finalizer_lock_busy.load(Ordering::Relaxed);

    // 周期 purge 3 件がそれぞれ PURGE_LOCK_WAIT でスキップした直後に、
    // 周期 finalize が同じ write_lock の待ちへ入る。
    tokio::time::timeout(Duration::from_secs(12), async {
        while metrics.finalizer_lock_busy.load(Ordering::Relaxed) < lock_busy_after_startup + 3 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("periodic purges must skip before finalize waits for the lock");
    // finalize が待ちキューへ入るのを確実にしてから、期限直前までロックを保持する。
    tokio::task::yield_now().await;
    tokio::time::sleep(RELEASE_MARGIN).await;
    release_tx.send(()).unwrap();

    // 完了条件はログではなく DB の concluded_at 更新件数。
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let mut rows = db
                .query(
                    "SELECT COUNT(*) FROM reports WHERE concluded_at IS NOT NULL",
                    (),
                )
                .await
                .unwrap();
            let concluded: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
            if concluded == REPORT_COUNT as i64 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("periodic finalize must conclude every due report after acquiring the lock");

    let mut rows = db
        .query(
            "SELECT COUNT(*) FROM reports WHERE concluded_at IS NULL",
            (),
        )
        .await
        .unwrap();
    let pending: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(pending, 0, "all due reports must be concluded in one tick");
    assert!(
        !logs.contains("Moderation finalization skipped: write lock is busy"),
        "finalize must acquire the lock and run SQL, not skip it"
    );

    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), finalizer)
        .await
        .expect("finalizer must stop cleanly")
        .expect("finalizer task must not panic");
    writer.await.unwrap();
}

/// ADR-0013 P4 item10: write_lock の識別子が接続ポインタではなく canonicalize 済み DB パスであること。
#[tokio::test]
async fn test_write_lock_key_uses_canonical_db_path() {
    let dir = std::env::temp_dir();
    let file_name = format!("dsh-write-lock-{}.db", uuid::Uuid::new_v4());
    let file = dir.join(&file_name);
    let plain = file.to_string_lossy().into_owned();
    // `./` を挟んだ別表記でも canonicalize 後は同じキーになる。
    let dotted = format!("{}/./{}", dir.to_string_lossy(), file_name);
    let db_a = Builder::new_local(plain)
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    let db_b = Builder::new_local(dotted)
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();

    let lock_a = crate::write_lock(&db_a).await.unwrap();
    let lock_b = crate::write_lock(&db_b).await.unwrap();
    assert!(
        Arc::ptr_eq(&lock_a, &lock_b),
        "two connections to the same canonical DB path must share one write lock"
    );

    // 接続破棄相当の回収後は古い Arc と別のロックが作られる。
    crate::reclaim_db_write_lock(&db_a);
    crate::reclaim_db_write_lock(&db_b);
    let recreated = crate::write_lock(&db_a).await.unwrap();
    assert!(
        !Arc::ptr_eq(&lock_a, &recreated),
        "reclaimed lock entry must not be reused"
    );

    crate::reclaim_db_write_lock(&db_a);
    drop(lock_a);
    drop(lock_b);
    drop(recreated);
    drop(db_a);
    drop(db_b);
    let _ = std::fs::remove_file(&file);
}
