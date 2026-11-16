//! Digital Agency OIDC authentication (ADR-0002).
//!
//! The BFF starts an authorization code + PKCE flow, keeps `state`, `nonce`,
//! and `code_verifier` server-side, and consumes them once at the callback.
//! ID tokens are verified against the provider's JWKS with ES256.

use crate::{AppError, AppState, Claims, VerifyResponse, ensure_user_not_banned};
use axum::{
    Json,
    extract::{Form, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration as StdDuration, Instant},
};
use uuid::Uuid;

const PENDING_TTL: StdDuration = StdDuration::from_secs(10 * 60);
const JWKS_TTL: StdDuration = StdDuration::from_secs(5 * 60);
const SESSION_TTL_SECS: i64 = 24 * 60 * 60;
/// リフレッシュトークンの有効期間（30日）。
const REFRESH_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// リフレッシュトークンの SHA-256 ハッシュ。DB にはハッシュのみ保存する。
/// トークン本体は UUIDv4（128bit 乱数）を base64url で表現する。
fn hash_refresh_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// 新しいリフレッシュトークンを生成し、指定ファミリへ登録する。
async fn issue_refresh_token(
    state: &AppState,
    sub: &str,
    family_id: &str,
) -> Result<String, AppError> {
    let token = URL_SAFE_NO_PAD.encode(Uuid::new_v4().as_bytes());
    let token_hash = hash_refresh_token(&token);
    let expires_at = (Utc::now() + Duration::seconds(REFRESH_TTL_SECS)).to_rfc3339();
    state
        .db
        .execute(
            "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at) VALUES (?, ?, ?, ?)",
            (token_hash, sub.to_string(), family_id.to_string(), expires_at),
        )
        .await?;
    Ok(token)
}

/// HttpOnly のリフレッシュトークン Cookie を構築する。
fn refresh_cookie(token: &str, secure: bool) -> Result<HeaderValue, AppError> {
    let value = format!(
        "refresh_token={token}; HttpOnly; Path=/auth/refresh; SameSite=Strict; Max-Age={REFRESH_TTL_SECS}{}",
        if secure { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&value)
        .map_err(|_| AppError::Internal("invalid refresh cookie".to_string()))
}

/// ファミリ全体を失効させる（盗用検知時）。
async fn revoke_refresh_family(state: &AppState, family_id: &str) -> Result<(), AppError> {
    state
        .db
        .execute(
            "UPDATE refresh_tokens SET used_at = datetime('now') WHERE family_id = ? AND used_at IS NULL",
            [family_id],
        )
        .await?;
    Ok(())
}

#[derive(Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub scopes: String,
    pub cookie_secure: bool,
    pub clock_skew_secs: u64,
    /// 要求する保証レベル（acr）。未指定なら検証しない（開発用）。
    pub required_acr: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// 要求する認証方法（amr）。未指定なら検証しない（開発用）。
    pub required_amr: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// 認証からの最大経過時間（秒）。設定時は認証要求に max_age を付与し、
    /// ID トークンの auth_time を検証する（未設定なら検証しない）。
    pub max_auth_age_secs: std::sync::Arc<std::sync::Mutex<Option<u64>>>,
}

struct PendingAuth {
    nonce: String,
    code_verifier: String,
    created_at: Instant,
}

pub struct OidcState {
    config: OidcConfig,
    pending: Mutex<HashMap<String, PendingAuth>>,
    jwks_cache: Mutex<Option<(JwkSet, Instant)>>,
}

impl OidcState {
    pub fn new(config: OidcConfig) -> Self {
        Self {
            config,
            pending: Mutex::new(HashMap::new()),
            jwks_cache: Mutex::new(None),
        }
    }

    /// カンマ区切りの設定値を空要素を除いて分割する。
    fn split_config_list(value: &str) -> Vec<String> {
        value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// 必須の環境変数が揃っている場合だけ有効化する。
    /// 不完全な設定で中途半端に起動しないよう fail-closed にする。
    pub fn from_env() -> Option<Arc<Self>> {
        let issuer = std::env::var("OIDC_ISSUER").ok()?;
        let client_id = std::env::var("OIDC_CLIENT_ID").ok()?;
        let client_secret = std::env::var("OIDC_CLIENT_SECRET").ok()?;
        let redirect_uri = std::env::var("OIDC_REDIRECT_URI").ok()?;
        let authorization_endpoint = std::env::var("OIDC_AUTHORIZATION_ENDPOINT").ok()?;
        let token_endpoint = std::env::var("OIDC_TOKEN_ENDPOINT").ok()?;
        let jwks_uri = std::env::var("OIDC_JWKS_URI").ok()?;
        let scopes = std::env::var("OIDC_SCOPES").unwrap_or_else(|_| "openid".to_string());
        let cookie_secure = std::env::var("OIDC_COOKIE_SECURE")
            .map(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
            .unwrap_or(true);
        let clock_skew_secs = std::env::var("OIDC_CLOCK_SKEW_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(60);
        let required_acr = std::env::var("OIDC_REQUIRED_ACR")
            .map(|value| Self::split_config_list(&value))
            .unwrap_or_default();
        let required_amr = std::env::var("OIDC_REQUIRED_AMR")
            .map(|value| Self::split_config_list(&value))
            .unwrap_or_default();
        let max_auth_age_secs = std::env::var("OIDC_MAX_AUTH_AGE_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| *seconds > 0);

        Some(Arc::new(Self::new(OidcConfig {
            issuer,
            client_id,
            client_secret,
            redirect_uri,
            authorization_endpoint,
            token_endpoint,
            jwks_uri,
            scopes,
            cookie_secure,
            clock_skew_secs,
            required_acr: std::sync::Arc::new(std::sync::Mutex::new(required_acr)),
            required_amr: std::sync::Arc::new(std::sync::Mutex::new(required_amr)),
            max_auth_age_secs: std::sync::Arc::new(std::sync::Mutex::new(max_auth_age_secs)),
        })))
    }

    /// テスト用: 要求する保証レベルを設定する（本番は環境変数から読む）。
    #[cfg(test)]
    pub fn set_required_assurance(&self, acr: Vec<String>, amr: Vec<String>) {
        *self.config.required_acr.lock().unwrap() = acr;
        *self.config.required_amr.lock().unwrap() = amr;
    }

    /// テスト用: 認証からの最大経過時間を設定する（本番は環境変数から読む）。
    #[cfg(test)]
    pub fn set_max_auth_age(&self, seconds: Option<u64>) {
        *self.config.max_auth_age_secs.lock().unwrap() = seconds;
    }

    fn take_pending(&self, state: &str) -> Result<PendingAuth, AppError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| AppError::Internal("OIDC pending store is poisoned".to_string()))?;
        let now = Instant::now();
        pending.retain(|_, auth| now.duration_since(auth.created_at) < PENDING_TTL);
        pending
            .remove(state)
            .ok_or_else(|| AppError::BadRequest("invalid or expired state".to_string()))
    }

    fn cached_jwks(&self) -> Option<JwkSet> {
        let cache = self.jwks_cache.lock().ok()?;
        cache.as_ref().and_then(|(jwks, fetched_at)| {
            if fetched_at.elapsed() < JWKS_TTL {
                Some(jwks.clone())
            } else {
                None
            }
        })
    }

    fn store_jwks(&self, jwks: JwkSet) {
        if let Ok(mut cache) = self.jwks_cache.lock() {
            *cache = Some((jwks, Instant::now()));
        }
    }

    #[cfg(test)]
    pub(crate) fn pending_nonce(&self, state: &str) -> Option<String> {
        self.pending
            .lock()
            .ok()?
            .get(state)
            .map(|auth| auth.nonce.clone())
    }
}

#[derive(Serialize)]
pub struct StartResponse {
    pub authorization_url: String,
    pub state: String,
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: String,
    pub state: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    id_token: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct IdTokenClaims {
    iss: String,
    #[serde(deserialize_with = "deserialize_audience")]
    aud: Vec<String>,
    exp: usize,
    iat: usize,
    nbf: Option<usize>,
    sub: String,
    nonce: Option<String>,
    at_hash: Option<String>,
    acr: Option<String>,
    amr: Option<Vec<String>>,
    auth_time: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct LogoutTokenClaims {
    iss: String,
    #[serde(deserialize_with = "deserialize_audience")]
    aud: Vec<String>,
    exp: usize,
    iat: usize,
    sub: Option<String>,
    sid: Option<String>,
    events: serde_json::Value,
}

#[derive(Deserialize)]
pub struct LogoutTokenRequest {
    pub logout_token: String,
}

fn deserialize_audience<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Audience {
        Single(String),
        Multiple(Vec<String>),
    }

    Ok(match Audience::deserialize(deserializer)? {
        Audience::Single(value) => vec![value],
        Audience::Multiple(values) => values,
    })
}

fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn verify_at_hash(access_token: &str, expected: &str) -> bool {
    let digest = Sha256::digest(access_token.as_bytes());
    URL_SAFE_NO_PAD.encode(&digest[..16]) == expected
}

async fn fetch_jwks(
    http_client: &reqwest::Client,
    config: &OidcConfig,
) -> Result<JwkSet, AppError> {
    let response = http_client
        .get(&config.jwks_uri)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("JWKS request failed: {e}")))?;
    if !response.status().is_success() {
        return Err(AppError::Internal(
            "JWKS endpoint returned an error".to_string(),
        ));
    }
    response
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("JWKS decode failed: {e}")))
}

async fn exchange_code(
    http_client: &reqwest::Client,
    config: &OidcConfig,
    code: &str,
    code_verifier: &str,
) -> Result<TokenResponse, AppError> {
    let response = http_client
        .post(&config.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", config.redirect_uri.as_str()),
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("token request failed: {e}")))?;
    if !response.status().is_success() {
        return Err(AppError::Unauthorized(
            "token endpoint rejected the authorization code".to_string(),
        ));
    }
    response
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("token response decode failed: {e}")))
}

async fn verification_key(
    http_client: &reqwest::Client,
    oidc: &OidcState,
    kid: &str,
) -> Result<DecodingKey, AppError> {
    let jwks = match oidc.cached_jwks() {
        Some(jwks) => jwks,
        None => {
            let jwks = fetch_jwks(http_client, &oidc.config).await?;
            oidc.store_jwks(jwks.clone());
            jwks
        }
    };

    let jwk = match jwks.find(kid).cloned() {
        Some(jwk) => jwk,
        None => {
            // 鍵ローテーション直後は未知の kid が来るため、一度だけ再取得する。
            let jwks = fetch_jwks(http_client, &oidc.config).await?;
            oidc.store_jwks(jwks.clone());
            jwks.find(kid)
                .cloned()
                .ok_or_else(|| AppError::Unauthorized("unknown signing key".to_string()))?
        }
    };

    DecodingKey::from_jwk(&jwk)
        .map_err(|_| AppError::Unauthorized("invalid signing key".to_string()))
}

async fn verify_id_token(
    http_client: &reqwest::Client,
    oidc: &OidcState,
    id_token: &str,
    expected_nonce: &str,
) -> Result<IdTokenClaims, AppError> {
    let header = decode_header(id_token)
        .map_err(|_| AppError::Unauthorized("invalid ID token header".to_string()))?;
    let kid = header
        .kid
        .ok_or_else(|| AppError::Unauthorized("ID token has no kid".to_string()))?;

    let key = verification_key(http_client, oidc, &kid).await?;

    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_issuer(std::slice::from_ref(&oidc.config.issuer));
    validation.set_audience(std::slice::from_ref(&oidc.config.client_id));
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.leeway = oidc.config.clock_skew_secs;

    let data = decode::<IdTokenClaims>(id_token, &key, &validation)
        .map_err(|_| AppError::Unauthorized("ID token validation failed".to_string()))?;
    let claims = data.claims;

    let now = Utc::now().timestamp() as usize;
    if claims.iat > now + oidc.config.clock_skew_secs as usize {
        return Err(AppError::Unauthorized(
            "ID token was issued in the future".to_string(),
        ));
    }
    if claims.nonce.as_deref() != Some(expected_nonce) {
        return Err(AppError::Unauthorized(
            "ID token nonce mismatch".to_string(),
        ));
    }
    if claims.iss != oidc.config.issuer {
        return Err(AppError::Unauthorized(
            "ID token issuer mismatch".to_string(),
        ));
    }

    // 保証レベル (acr) と認証方法 (amr) は設定時のみ検証する。
    // 要求値があるのにクレームが欠ける場合は本番有効化しない（fail-closed）。
    let required_acr = oidc
        .config
        .required_acr
        .lock()
        .map_err(|_| AppError::Internal("assurance config is poisoned".to_string()))?;
    if !required_acr.is_empty() {
        let actual = claims.acr.as_deref();
        if !required_acr
            .iter()
            .any(|required| actual == Some(required.as_str()))
        {
            return Err(AppError::Unauthorized(
                "ID token acr does not satisfy the required assurance level".to_string(),
            ));
        }
    }
    drop(required_acr);
    let required_amr = oidc
        .config
        .required_amr
        .lock()
        .map_err(|_| AppError::Internal("assurance config is poisoned".to_string()))?;
    if !required_amr.is_empty() {
        let actual = claims.amr.as_deref().unwrap_or_default();
        if !required_amr
            .iter()
            .all(|required| actual.iter().any(|value| value == required))
        {
            return Err(AppError::Unauthorized(
                "ID token amr does not satisfy the required authentication methods".to_string(),
            ));
        }
    }
    drop(required_amr);

    // 認証からの経過時間 (max_age) を検証する。要求時に auth_time が
    // 欠落している場合は本番有効化しない（fail-closed）。
    let max_age = oidc
        .config
        .max_auth_age_secs
        .lock()
        .map_err(|_| AppError::Internal("assurance config is poisoned".to_string()))?;
    if let Some(max_age_secs) = *max_age {
        let auth_time = claims
            .auth_time
            .ok_or_else(|| AppError::Unauthorized("ID token is missing auth_time".to_string()))?;
        let earliest = now
            .saturating_sub(max_age_secs as usize)
            .saturating_sub(oidc.config.clock_skew_secs as usize);
        if auth_time < earliest || auth_time > now + oidc.config.clock_skew_secs as usize {
            return Err(AppError::Unauthorized(
                "ID token auth_time is outside the allowed window".to_string(),
            ));
        }
    }

    Ok(claims)
}

async fn verify_logout_token(
    http_client: &reqwest::Client,
    oidc: &OidcState,
    logout_token: &str,
) -> Result<LogoutTokenClaims, AppError> {
    let header = decode_header(logout_token)
        .map_err(|_| AppError::Unauthorized("invalid logout token header".to_string()))?;
    let kid = header
        .kid
        .ok_or_else(|| AppError::Unauthorized("logout token has no kid".to_string()))?;
    let key = verification_key(http_client, oidc, &kid).await?;

    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_issuer(std::slice::from_ref(&oidc.config.issuer));
    validation.set_audience(std::slice::from_ref(&oidc.config.client_id));
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);
    validation.leeway = oidc.config.clock_skew_secs;

    let data = decode::<LogoutTokenClaims>(logout_token, &key, &validation)
        .map_err(|_| AppError::Unauthorized("logout token validation failed".to_string()))?;
    let claims = data.claims;

    if claims
        .events
        .get("http://schemas.openid.net/event/backchannel-logout")
        .is_none()
    {
        return Err(AppError::Unauthorized(
            "logout token is missing the backchannel event".to_string(),
        ));
    }
    if claims.sid.is_none() && claims.sub.is_none() {
        return Err(AppError::Unauthorized(
            "logout token has neither sid nor sub".to_string(),
        ));
    }

    Ok(claims)
}

pub async fn start(State(state): State<Arc<AppState>>) -> Result<Json<StartResponse>, AppError> {
    let oidc = state
        .oidc
        .as_ref()
        .ok_or_else(|| AppError::NotImplemented("OIDC is not configured".to_string()))?;

    let auth_state = random_token();
    let nonce = random_token();
    let code_verifier = random_token();
    let challenge = code_challenge(&code_verifier);

    oidc.pending
        .lock()
        .map_err(|_| AppError::Internal("OIDC pending store is poisoned".to_string()))?
        .insert(
            auth_state.clone(),
            PendingAuth {
                nonce: nonce.clone(),
                code_verifier,
                created_at: Instant::now(),
            },
        );

    let mut url = reqwest::Url::parse(&oidc.config.authorization_endpoint)
        .map_err(|_| AppError::Internal("invalid authorization endpoint".to_string()))?;
    let max_age = oidc
        .config
        .max_auth_age_secs
        .lock()
        .map_err(|_| AppError::Internal("assurance config is poisoned".to_string()))?;
    if let Some(max_age_secs) = *max_age {
        url.query_pairs_mut()
            .append_pair("max_age", &max_age_secs.to_string());
    }
    drop(max_age);
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &oidc.config.client_id)
        .append_pair("redirect_uri", &oidc.config.redirect_uri)
        .append_pair("scope", &oidc.config.scopes)
        .append_pair("state", &auth_state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");

    Ok(Json(StartResponse {
        authorization_url: url.to_string(),
        state: auth_state,
    }))
}

pub async fn callback(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, AppError> {
    let oidc = state
        .oidc
        .as_ref()
        .ok_or_else(|| AppError::NotImplemented("OIDC is not configured".to_string()))?;

    let pending = oidc.take_pending(&query.state)?;
    let token = exchange_code(
        &state.http_client,
        &oidc.config,
        &query.code,
        &pending.code_verifier,
    )
    .await?;
    let id_claims =
        verify_id_token(&state.http_client, oidc, &token.id_token, &pending.nonce).await?;

    let at_hash = id_claims
        .at_hash
        .as_deref()
        .ok_or_else(|| AppError::Unauthorized("ID token has no at_hash".to_string()))?;
    if !verify_at_hash(&token.access_token, at_hash) {
        return Err(AppError::Unauthorized("at_hash mismatch".to_string()));
    }

    let user_hash = id_claims.sub;
    state
        .db
        .execute(
            "INSERT OR IGNORE INTO users (user_hash) VALUES (?)",
            [user_hash.clone()],
        )
        .await?;

    let now = Utc::now();
    let now_ts = now.timestamp() as usize;
    let expiration = now
        .checked_add_signed(Duration::seconds(SESSION_TTL_SECS))
        .expect("valid timestamp")
        .timestamp() as usize;
    let iat = {
        let mut revoked_subs = state
            .revoked_subs_before
            .lock()
            .map_err(|_| crate::AppError::Internal("session store is poisoned".to_string()))?;
        let issued_at = match revoked_subs.get(&user_hash).copied() {
            Some(threshold) => std::cmp::max(now_ts, threshold.saturating_add(1)),
            None => now_ts,
        };
        // 新しい認証セッションの発行により、過去セッション失効の閾値は不要になる。
        revoked_subs.remove(&user_hash);
        issued_at
    };
    // 再認証で旧セッション拒否の役割を終えた閾値を DB からも削除する。
    state
        .db
        .execute(
            "DELETE FROM revoked_subs WHERE sub = ?",
            [user_hash.clone()],
        )
        .await?;
    let session_claims = Claims {
        sub: user_hash.clone(),
        exp: expiration,
        sid: Some(Uuid::new_v4().to_string()),
        iat: Some(iat),
    };
    let session_token = jsonwebtoken::encode(
        &state.jwt_keys.header(),
        &session_claims,
        state.jwt_keys.encoding_key(),
    )
    .map_err(|e| AppError::Internal(format!("JWT generation failed: {e}")))?;

    let family_id = Uuid::new_v4().to_string();
    let refresh_token = issue_refresh_token(&state, &user_hash, &family_id).await?;

    let cookie = format!(
        "session={session_token}; HttpOnly; Path=/; SameSite=Lax; Max-Age={SESSION_TTL_SECS}{}",
        if oidc.config.cookie_secure {
            "; Secure"
        } else {
            ""
        }
    );

    let mut response = Json(VerifyResponse {
        authorized: true,
        user_hash: Some(user_hash),
        token: Some(session_token),
    })
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie)
            .map_err(|_| AppError::Internal("invalid session cookie".to_string()))?,
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        refresh_cookie(&refresh_token, oidc.config.cookie_secure)?,
    );
    Ok(response)
}

/// リフレッシュトークンを回転させ、新しいセッション JWT とリフレッシュトークンを発行する。
/// 既に使用済みのトークンが再提示された場合は盗用とみなし、ファミリ全体を失効する。
pub async fn refresh(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let token = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|part| {
                let (name, value) = part.trim().split_once('=')?;
                (name == "refresh_token").then_some(value)
            })
        })
        .ok_or_else(|| AppError::Unauthorized("refresh token is missing".to_string()))?;

    let token_hash = hash_refresh_token(token);
    let row = {
        let mut rows = state
            .db
            .query(
                "SELECT sub, family_id, expires_at, used_at FROM refresh_tokens WHERE token_hash = ?",
                [token_hash.clone()],
            )
            .await?;
        rows.next()
            .await?
            .map(
                |row| -> Result<(String, String, String, Option<String>), AppError> {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                },
            )
            .transpose()?
    };
    let (sub, family_id, expires_at, used_at) =
        row.ok_or_else(|| AppError::Unauthorized("unknown refresh token".to_string()))?;

    if used_at.is_some() {
        // 既に消費済みのトークンの再利用は盗用の兆候。ファミリを全失効する。
        revoke_refresh_family(&state, &family_id).await?;
        return Err(AppError::Unauthorized(
            "refresh token reuse detected; token family revoked".to_string(),
        ));
    }
    if expires_at <= Utc::now().to_rfc3339() {
        return Err(AppError::Unauthorized("refresh token expired".to_string()));
    }

    // 消費済みへ更新（Concurrent な再利用は UPDATE の影響件数で検出する）。
    let updated = state
        .db
        .execute(
            "UPDATE refresh_tokens SET used_at = datetime('now') WHERE token_hash = ? AND used_at IS NULL",
            [token_hash],
        )
        .await?;
    if updated == 0 {
        revoke_refresh_family(&state, &family_id).await?;
        return Err(AppError::Unauthorized(
            "refresh token reuse detected; token family revoked".to_string(),
        ));
    }
    ensure_user_not_banned(&state.db, &sub).await?;

    let now_ts = Utc::now().timestamp() as usize;
    let expiration = (Utc::now() + Duration::seconds(SESSION_TTL_SECS)).timestamp() as usize;
    let iat = {
        let revoked_subs = state
            .revoked_subs_before
            .lock()
            .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?;
        match revoked_subs.get(&sub).copied() {
            Some(threshold) => std::cmp::max(now_ts, threshold.saturating_add(1)),
            None => now_ts,
        }
    };
    let session_claims = Claims {
        sub: sub.clone(),
        exp: expiration,
        sid: Some(Uuid::new_v4().to_string()),
        iat: Some(iat),
    };
    let session_token = jsonwebtoken::encode(
        &state.jwt_keys.header(),
        &session_claims,
        state.jwt_keys.encoding_key(),
    )
    .map_err(|e| AppError::Internal(format!("JWT generation failed: {e}")))?;
    let new_refresh = issue_refresh_token(&state, &sub, &family_id).await?;

    let secure = state
        .oidc
        .as_ref()
        .map(|oidc| oidc.config.cookie_secure)
        .unwrap_or(true);
    let mut response = Json(VerifyResponse {
        authorized: true,
        user_hash: Some(sub),
        token: Some(session_token),
    })
    .into_response();
    response
        .headers_mut()
        .append(header::SET_COOKIE, refresh_cookie(&new_refresh, secure)?);
    Ok(response)
}

/// OIDC Back-Channel Logout。`sid` を失効させ、該当セッションを拒否する。
pub async fn backchannel_logout(
    State(state): State<Arc<AppState>>,
    Form(payload): Form<LogoutTokenRequest>,
) -> Result<StatusCode, AppError> {
    let oidc = state
        .oidc
        .as_ref()
        .ok_or_else(|| AppError::NotImplemented("OIDC is not configured".to_string()))?;
    let claims = verify_logout_token(&state.http_client, oidc, &payload.logout_token).await?;

    if let Some(sid) = claims.sid {
        crate::revoke_session(&state, &sid).await?;
    }

    if let Some(sub) = claims.sub {
        let revoked_at = Utc::now().timestamp() as usize;
        state
            .revoked_subs_before
            .lock()
            .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?
            .insert(sub.clone(), revoked_at);
        // プロセス再起動後も失効を維持するため DB にも記録する。
        state
            .db
            .execute(
                "INSERT INTO revoked_subs (sub, revoked_at) VALUES (?, ?)
                 ON CONFLICT(sub) DO UPDATE SET revoked_at = excluded.revoked_at,
                     updated_at = CURRENT_TIMESTAMP",
                (sub, revoked_at as i64),
            )
            .await?;
    }

    Ok(StatusCode::OK)
}
