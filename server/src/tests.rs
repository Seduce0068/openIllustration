use crate::{
    AppState, VerifyRequest, VerifyResponse, build_router, ensure_user_not_banned,
    update_content_status, validate_age, validate_upload_content, validate_upload_content_type,
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
    crate::migration::run_migrations(db).await.unwrap();
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
    let token = generate_token("test_secret", "test_user").await;
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
                    format!("Bearer {}", generate_token("test_secret", "reporter").await),
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
                        generate_token("test_secret", "reporter2").await
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
                    format!("Bearer {}", generate_token("test_secret", "reporter").await),
                )
                .body(Body::from(report_body.to_string()))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
    let token = generate_token("test_secret", "test_user").await;
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
                .body(Body::from(body))
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// 同一コンテンツの再アップロードは 400（CID主キー衝突を500にしない）
#[tokio::test]
async fn test_upload_duplicate_content_rejected() {
    let app = setup_app().await;
    let token = generate_token("test_secret", "test_user").await;
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
        jwt_secret: "test_secret".to_string(),
        allow_mock_auth: true,
    });
    let app = build_router(shared_state);

    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};

    for _ in 0..3 {
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
        jwt_secret: "test_secret".to_string(),
        allow_mock_auth: true,
    });
    let app = build_router(shared_state);

    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};

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
                        format!("Bearer {}", generate_token("test_secret", &token_sub).await),
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
async fn insert_juror(db: &libsql::Connection) -> (String, RsaPrivateKey) {
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
    (juror_hash, private_key)
}

/// 署名付きの投票リクエストを送信し、ステータスコードを返す
async fn send_signed_vote(
    app: &Router,
    report_id: i64,
    result: &str,
    juror_hash: &str,
    private_key: &RsaPrivateKey,
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
                    format!("Bearer {}", generate_token("test_secret", juror_hash).await),
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
        jwt_secret: "test_secret".to_string(),
        allow_mock_auth: true,
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
        jwt_secret: "test_secret".to_string(),
        allow_mock_auth: true,
    });
    let app = build_router(shared_state);

    let (juror_hash, key) = insert_juror(&db).await;
    assert_eq!(
        send_signed_vote(&app, 9999, "Approve", &juror_hash, &key).await,
        StatusCode::NOT_FOUND,
        "unknown report must return 404"
    );
}

/// INT-01: IPFS改ざん検知（サーバー側）
#[tokio::test]
async fn test_content_checksum_verification_fails_on_tamper() {
    let (app, db) = setup_app_with_db().await;

    // Insert required user first
    db.execute("INSERT INTO users (user_hash) VALUES ('owner')", ())
        .await
        .unwrap();

    db.execute(
        "INSERT INTO contents (cid, type, owner_hash, checksum, status)
             VALUES ('QmTampered', 'Novel', 'owner', 'correct_hash_123', 'Approved');",
        (),
    )
    .await
    .unwrap();

    let response = app
        .oneshot(with_peer(
            Request::get("/content/QmTampered")
                .header(
                    "authorization",
                    format!("Bearer {}", generate_token("test_secret", "tester").await),
                )
                .body(Body::empty())
                .unwrap(),
            test_ipv6_peer(),
        ))
        .await
        .unwrap();

    // IPFS ゲートウェイが利用不可の場合は 500 になる可能性があるため、
    // 401 (改ざん検知) または 500 (ゲートウェイエラー) のいずれかを許容
    assert!(
        response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR,
        "Expected 401 or 500, got: {}",
        response.status()
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
