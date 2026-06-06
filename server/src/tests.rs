use crate::{
    AppState, VerifyRequest, VerifyResponse, build_router, ensure_user_not_banned, validate_age,
    validate_upload_content_type,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

fn test_ipv6_peer() -> IpAddr {
    IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1))
}
use common::Claims;
use jsonwebtoken::{EncodingKey, Header, encode};
use libsql::Builder;
use rsa::{RsaPrivateKey, pkcs8::EncodePublicKey};
use serde_json::json;
use sha2::Sha256;
use std::sync::Arc;
use tower::ServiceExt;

async fn generate_token(secret: &str, sub: &str) -> String {
    let claims = Claims {
        sub: sub.to_string(),
        exp: 10000000000,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_ref()),
    )
    .unwrap()
}

async fn init_schema(db: &libsql::Connection) {
    db.execute(
        "CREATE TABLE users (
                user_hash TEXT PRIMARY KEY,
                public_key_der BLOB,
                is_banned BOOLEAN DEFAULT FALSE,
                banned_at DATETIME
            );",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "CREATE TABLE contents (
                cid TEXT PRIMARY KEY,
                type TEXT,
                owner_hash TEXT,
                checksum TEXT,
                status TEXT DEFAULT 'Approved',
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "CREATE TABLE content_audit_log (
                log_id INTEGER PRIMARY KEY,
                cid TEXT,
                old_status TEXT,
                new_status TEXT,
                changed_by TEXT,
                reason TEXT,
                previous_log_hash TEXT,
                entry_hash TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "CREATE TABLE reports (
                report_id INTEGER PRIMARY KEY AUTOINCREMENT,
                target_cid TEXT,
                reason TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "CREATE TABLE votes (
                vote_id INTEGER PRIMARY KEY AUTOINCREMENT,
                report_id INTEGER,
                voter_hash TEXT,
                vote_result TEXT,
                voted_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        (),
    )
    .await
    .unwrap();
    db.execute(
        "CREATE TABLE signatures (
                signature_id INTEGER PRIMARY KEY AUTOINCREMENT,
                target_id TEXT,
                signer_hash TEXT,
                signature_blob BLOB,
                algorithm TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        (),
    )
    .await
    .unwrap();
}

async fn setup_app_with_db() -> (Router, Arc<libsql::Connection>) {
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
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmTestCID', 'Illustration', 'test_user', 'mock_checksum', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let shared_state = Arc::new(AppState {
        db: db.clone(),
        http_client: reqwest::Client::new(),
        jwt_secret: "test_secret".to_string(),
        allow_mock_auth: true,
    });

    let router = build_router(shared_state);
    (router, db)
}

async fn setup_app() -> Router {
    setup_app_with_db().await.0
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

/// SEC-01: パブリック IPv4 は 403
#[tokio::test]
async fn test_ipv4_public_address_forbidden() {
    unsafe { std::env::remove_var("ALLOW_IPV4_LOCALHOST") };
    let app = setup_app().await;
    let req = with_peer(
        Request::get("/health").body(Body::empty()).unwrap(),
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
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
                    format!("Bearer {}", generate_token("test_secret", "reporter").await),
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

/// INT-02: zip は拒否
#[test]
fn test_reject_zip_content_type() {
    assert!(validate_upload_content_type(Some("application/zip")).is_err());
    assert!(validate_upload_content_type(Some("image/jxl")).is_ok());
    assert!(validate_upload_content_type(Some("text/markdown")).is_ok());
}

#[test]
fn test_age_validation_unit() {
    assert!(validate_age("2000-01-01", "JP").is_ok());
    assert!(validate_age("2015-01-01", "JP").is_err());
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

    let mut rng = rand::thread_rng();
    let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
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
        jwt_secret: "test_secret".to_string(),
        allow_mock_auth: true,
    });
    let app = build_router(shared_state);

    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};

    for _ in 0..3 {
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
                            generate_token("test_secret", &juror_hash).await
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

    let mut rows = db
        .query("SELECT status FROM contents WHERE cid = 'QmVoteTest'", ())
        .await
        .unwrap();
    let status: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(status, "Rejected");
}
