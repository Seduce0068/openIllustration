#[cfg(test)]
mod tests {
    use crate::{auth_login, auth_verify, VerifyRequest, VerifyResponse, AppState};
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::post,
        Router,
    };
    use tower::ServiceExt;
    use std::sync::Arc;
    use libsql::Builder;

    async fn setup_app() -> Router {
        let db = Builder::new_local(":memory:").build().await.unwrap().connect().unwrap();
        let shared_state = Arc::new(AppState { 
            db: Arc::new(db),
            http_client: reqwest::Client::new(),
        });
        Router::new()
            .route("/auth/login", post(auth_login))
            .route("/auth/verify", post(auth_verify))
            .with_state(shared_state)
    }

    #[tokio::test]
    async fn test_auth_login_returns_nonce() {
        let app = Router::new().route("/auth/login", post(auth_login));
        let response = app
            .oneshot(Request::post("/auth/login").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_verify_authorized() {
        let app = setup_app().await;
        let body = serde_json::to_string(&VerifyRequest {
            presentation_token: "mock_token".to_string(),
        }).unwrap();

        let response = app
            .oneshot(
                Request::post("/auth/verify")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        
        let body_bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        let body: VerifyResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert!(body.authorized);
    }
}
