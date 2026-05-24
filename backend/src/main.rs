mod tests;
mod fuzz_test;

use axum::{extract::{Path, State}, response::{IntoResponse, Response}, routing::post, Json, Router, body::Body, http::{header, StatusCode}};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use std::net::SocketAddr;
use chrono::{Datelike, Local, NaiveDate};
use libsql::{Builder, Connection};
use std::sync::Arc;
use reqwest::Client;

#[derive(Clone)]
struct AppState {
    db: Arc<Connection>,
    http_client: Client,
}

#[derive(Serialize, Deserialize)]
pub struct Oid4vpRequest {
    pub nonce: String,
    pub presentation_definition_uri: String,
}

#[derive(Deserialize, Serialize)]
pub struct VerifyRequest {
    pub presentation_token: String,
}

#[derive(Serialize, Deserialize)]
pub struct VerifyResponse {
    pub authorized: bool,
    pub user_hash: Option<String>,
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
}

#[tokio::main]
async fn main() {
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "file:local.db".to_string());
    let db_token = std::env::var("DATABASE_TOKEN").unwrap_or_else(|_| "".to_string());
    
    let db = Builder::new_remote(db_url, db_token)
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();

    db.execute(
        "CREATE TABLE IF NOT EXISTS users (
            user_hash TEXT PRIMARY KEY,
            is_banned BOOLEAN DEFAULT FALSE,
            banned_at DATETIME
        );", (),
    ).await.unwrap();

    let shared_state = Arc::new(AppState { 
        db: Arc::new(db),
        http_client: Client::new(),
    });

    let app = Router::new()
        .route("/auth/login", post(auth_login))
        .route("/auth/verify", post(auth_verify))
        .route("/mod/report", post(mod_report))
        .route("/mod/vote", post(mod_vote))
        .route("/content/:cid", axum::routing::get(get_content))
        .with_state(shared_state);

    let addr = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 3000));
    println!("Guild Backend listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

pub async fn auth_login() -> Json<Oid4vpRequest> {
    let nonce = Uuid::new_v4().to_string();
    let presentation_definition_uri = "https://guild.example.com/api/presentation-definition".to_string();

    Json(Oid4vpRequest {
        nonce,
        presentation_definition_uri,
    })
}

pub async fn auth_verify(
    State(state): State<Arc<AppState>>,
    Json(_payload): Json<VerifyRequest>
) -> Json<VerifyResponse> {
    let user_hash = "sha256_mock_hash_12345".to_string();
    let _ = state.db.execute("INSERT OR IGNORE INTO users (user_hash) VALUES (?)", [user_hash.clone()]).await;

    Json(VerifyResponse {
        authorized: true,
        user_hash: Some(user_hash),
    })
}

pub async fn get_content(
    State(state): State<Arc<AppState>>,
    Path(cid): Path<String>,
) -> impl IntoResponse {
    let row = state.db.query("SELECT checksum FROM contents WHERE cid = ?", [cid.clone()]).await.unwrap().next().await.unwrap();
    let expected_checksum: String = row.unwrap().get(0).unwrap();

    let url = format!("https://ipfs.io/ipfs/{}", cid);
    let response = state.http_client.get(url).send().await.unwrap();
    let bytes = response.bytes().await.unwrap();

    let actual_checksum = sha256::digest(&bytes[..]);
    if actual_checksum != expected_checksum {
        return (StatusCode::FORBIDDEN, "Integrity check failed").into_response();
    }

    Response::builder()
        .header(header::CONTENT_TYPE, "image/jxl")
        .body(Body::from(bytes))
        .unwrap()
}

pub async fn mod_report(
    State(state): State<Arc<AppState>>,
    Json(report): Json<Report>,
) -> Json<bool> {
    let _ = state.db.execute(
        "INSERT INTO reports (target_cid, reason) VALUES (?, ?)", 
        [report.target_cid.clone(), report.reason]
    ).await;
    let _ = state.db.execute("UPDATE contents SET status = 'Reported' WHERE cid = ?", [report.target_cid]).await;
    Json(true)
}

pub async fn mod_vote(
    State(state): State<Arc<AppState>>,
    Json(vote): Json<Vote>,
) -> Json<bool> {
    let _ = state.db.execute(
        "INSERT INTO votes (report_id, juror_hash, vote_result) VALUES (?, ?, ?)", 
        [vote.report_id.to_string(), vote.juror_hash, vote.result]
    ).await;
    Json(true)
}
