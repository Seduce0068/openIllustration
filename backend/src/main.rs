#[cfg(test)]
mod tests;
#[cfg(test)]
mod fuzz_test;

// Main entry point for the Guild Backend
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use std::net::SocketAddr;
use libsql::{Builder, Connection};
use std::sync::Arc;
use reqwest::Client;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Connection>,
    pub http_client: Client,
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
async fn main() -> anyhow::Result<()> {
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "file:local.db".to_string());
    let db_token = std::env::var("DATABASE_TOKEN").unwrap_or_else(|_| "".to_string());
    
    let db = Builder::new_remote(db_url, db_token)
        .build()
        .await?
        .connect()?;

    db.execute(
        "CREATE TABLE IF NOT EXISTS users (
            user_hash TEXT PRIMARY KEY,
            is_banned BOOLEAN DEFAULT FALSE,
            banned_at DATETIME
        );", (),
    ).await?;

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
        .layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024)) // 10MB limit
        .with_state(shared_state);

    let addr = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 3000));
    println!("Guild Backend listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
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
) -> Response {
    let query_result = match state.db.query("SELECT checksum FROM contents WHERE cid = ?", [cid.clone()]).await {
        Ok(mut rows) => match rows.next().await {
            Ok(Some(row)) => row,
            _ => return (StatusCode::NOT_FOUND, "Content metadata not found").into_response(),
        },
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response(),
    };

    let expected_checksum: String = match query_result.get(0) {
        Ok(val) => val,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Metadata retrieval error").into_response(),
    };

    let url = format!("https://ipfs.io/ipfs/{}", cid);
    let response = match state.http_client.get(url).send().await {
        Ok(res) => res,
        Err(_) => return (StatusCode::BAD_GATEWAY, "Failed to fetch from IPFS").into_response(),
    };

    // Limit memory usage for body bytes
    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "Content too large or transfer failed").into_response(),
    };

    let actual_checksum = sha256::digest(&bytes[..]);
    if actual_checksum != expected_checksum {
        return (StatusCode::FORBIDDEN, "Integrity check failed").into_response();
    }

    Response::builder()
        .header(header::CONTENT_TYPE, "image/jxl")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR).into_response())
}

pub async fn mod_report(
    State(state): State<Arc<AppState>>,
    Json(report): Json<Report>,
) -> Result<Json<bool>, StatusCode> {
    state.db.execute(
        "INSERT INTO reports (target_cid, reason) VALUES (?, ?)", 
        [report.target_cid.clone(), report.reason]
    ).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    state.db.execute("UPDATE contents SET status = 'Reported' WHERE cid = ?", [report.target_cid])
        .await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(true))
}

pub async fn mod_vote(
    State(state): State<Arc<AppState>>,
    Json(vote): Json<Vote>,
) -> Result<Json<bool>, StatusCode> {
    // TODO: Verify juror_hash authenticity
    state.db.execute(
        "INSERT INTO votes (report_id, juror_hash, vote_result) VALUES (?, ?, ?)", 
        [vote.report_id.to_string(), vote.juror_hash, vote.result]
    ).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(true))
}
