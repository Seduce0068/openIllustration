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
use chrono::{DateTime, Duration, Utc};
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
/// ログイン CSRF 対策の double-submit に使う Cookie 名と有効期限。
/// `secure=false`（mock）はこの名前を維持する。
const OIDC_STATE_COOKIE: &str = "oidc_state";
/// `__Host-` プレフィックスは Secure / Path=/ / Domain なしが必須。
const HOST_OIDC_STATE_COOKIE: &str = "__Host-oidc_state";
const OIDC_STATE_COOKIE_MAX_AGE: i64 = 10 * 60;
/// 未完了の OIDC 認証（`pending`）の保持上限。
/// 認証開始は未認証で呼べるため、上限を設けないと TTL 到期までエントリが
/// 際限なく溜まりメモリを圧迫する（R1-23）。
pub(crate) const MAX_PENDING_AUTH: usize = 1024;
const JWKS_TTL: StdDuration = StdDuration::from_secs(5 * 60);
pub(crate) const SESSION_TTL_SECS: i64 = 24 * 60 * 60;
/// OIDC_CLOCK_SKEW_SECS の許容範囲（秒）。
/// 0 や極端に大きな値は exp / nbf / auth_time 検証を実質的に無効化するため、
/// 範囲外の設定は既定値へフォールバックする（fail-closed）。
pub const CLOCK_SKEW_MIN_SECS: u64 = 1;
pub const CLOCK_SKEW_MAX_SECS: u64 = 300;
pub const DEFAULT_CLOCK_SKEW_SECS: u64 = 60;
/// OIDC_MAX_AUTH_AGE_SECS の上限（30日）。これを超える値は auth_time / max_age
/// 検証の実効性が失われるため、上限超過は起動拒否（fail-closed）にする。
pub const MAX_AUTH_AGE_MAX_SECS: u64 = 2_592_000;
/// リフレッシュトークンの有効期間（30日）。
const REFRESH_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// OIDC Back-Channel Logout の events クレームに必須のキー。
const BACKCHANNEL_LOGOUT_EVENT: &str = "http://schemas.openid.net/event/backchannel-logout";
/// SSRF 対策: OIDC エンドポイントは https のみ受け付ける。
///
/// 開発用の mock 認証が明示的に有効な場合だけ localhost の http を許容し、
/// 本番相当では http / 不正文字列を拒否する（fail-closed）。
/// 戻り値は真偽のみで、判定対象の値は保持・露出しない。
/// `validate_oidc_config_with` 経由で `from_env_with` と `doctor` が同じ判定を共有する。
pub fn is_valid_oidc_endpoint(value: &str, allow_mock_auth: bool) -> bool {
    match reqwest::Url::parse(value) {
        Ok(url) => {
            if url.scheme() == "https" {
                true
            } else if url.scheme() == "http" && allow_mock_auth {
                matches!(
                    url.host_str()
                        .map(|host| host.to_ascii_lowercase())
                        .as_deref(),
                    Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                )
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

// 起動拒否相当の固定文言。設定値は一切含めず、doctor の警告文と完全一致させる。
const OIDC_ENDPOINT_URL_ERROR: &str = "OIDC endpoints must be https (http is allowed only for localhost when ALLOW_MOCK_AUTH is enabled)";
const OIDC_EMPTY_VALUE_ERROR: &str = "OIDC configuration values must not be empty";
/// 必須値が非 UTF-8 のときの固定文言。生バイト（`OsString`）は保持・エコーしない。
const OIDC_NON_UTF8_ERROR: &str = "OIDC configuration values must be valid UTF-8";
const OIDC_MAX_AUTH_AGE_GREATER_ERROR: &str = "OIDC_MAX_AUTH_AGE_SECS must be greater than 0";
const OIDC_MAX_AUTH_AGE_POSITIVE_ERROR: &str = "OIDC_MAX_AUTH_AGE_SECS must be a positive integer";
/// 上限超過の固定文言。上限値（定数）は示しても設定値は一切エコーしない。
const OIDC_MAX_AUTH_AGE_TOO_LARGE_ERROR: &str =
    "OIDC_MAX_AUTH_AGE_SECS must not exceed 2592000 seconds (30 days)";
/// 真偽値フラグは `common::age::parse_bool_env` の受理値のみ。不正値は値をエコーしない。
const ALLOW_MOCK_AUTH_BOOL_ERROR: &str = "ALLOW_MOCK_AUTH must be one of 1/true/0/false";
const OIDC_COOKIE_SECURE_BOOL_ERROR: &str = "OIDC_COOKIE_SECURE must be one of 1/true/0/false";

/// OIDC 設定の検証結果。秘密情報（`client_secret`）と `OidcState` は一切含めない。
///
/// `from_env_with`（起動時 fail-closed）と `doctor`（設定診断）が同じ判定を共有するための
/// 非秘密の値だけを保持する。ハードエラーは固定文言のみで、設定値はエコーしない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcConfigValidation {
    /// 必須 7 変数がすべて存在し、trim 後に非空か。本番必須設定の判定へ渡す。
    pub required_present: bool,
    /// 必須 7 変数がすべて設定されているか（空文字でも設定あり）。`Ok(None)` の判定に使う。
    pub enabled: bool,
    /// `OIDC_COOKIE_SECURE` の実効値（未設定時は true / fail-closed）。
    pub cookie_secure: bool,
    /// `OIDC_CLOCK_SKEW_SECS` の実効値（範囲外・不正値は既定値）。
    pub clock_skew_secs: u64,
    /// `OIDC_MAX_AUTH_AGE_SECS` の解析値。不正値は None（理由は hard_errors）。
    pub max_auth_age_secs: Option<u64>,
    /// (OIDC_REQUIRED_ACR に有効要素があるか, OIDC_REQUIRED_AMR に有効要素があるか)。
    pub acr_amr_present: (bool, bool),
    /// 起動拒否相当の理由（固定文言のみ）。本番警告へは流さない。
    pub hard_errors: Vec<&'static str>,
}

/// 環境変数の読み取りを注入して OIDC 設定を検証する共通バリデータ。
///
/// `from_env_with` と `doctor` の双方から呼ばれ、必須変数・URL・空値・cookie_secure・
/// clock_skew・max_auth_age・acr/amr の判定を一箇所へ集約する。戻り値には秘密情報を
/// 含めないため、doctor へ `OidcState` や `client_secret` を渡す必要はない。
///
/// `lookup` は `std::env::var` と同じ Result 契約で受け取る。`NotPresent`（未設定）と
/// `NotUnicode`（値はあるが非 UTF-8）を区別し、後者は「未設定」ではなく不正値として
/// 起動拒否・doctor 警告の対象にする（生バイトは一切保持・露出しない）。
pub fn validate_oidc_config_with<F>(lookup: F) -> OidcConfigValidation
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    const REQUIRED: [&str; 7] = [
        "OIDC_ISSUER",
        "OIDC_CLIENT_ID",
        "OIDC_CLIENT_SECRET",
        "OIDC_REDIRECT_URI",
        "OIDC_AUTHORIZATION_ENDPOINT",
        "OIDC_TOKEN_ENDPOINT",
        "OIDC_JWKS_URI",
    ];
    const ENDPOINTS: [&str; 5] = [
        "OIDC_ISSUER",
        "OIDC_REDIRECT_URI",
        "OIDC_AUTHORIZATION_ENDPOINT",
        "OIDC_TOKEN_ENDPOINT",
        "OIDC_JWKS_URI",
    ];

    let required: Vec<Result<String, std::env::VarError>> =
        REQUIRED.iter().map(|name| lookup(name)).collect();
    // 未設定（NotPresent）だけが「存在しない」を意味する。非 UTF-8 は値が存在するので
    // enabled を真にし、hard_errors 側で起動拒否へつなぐ（「未設定」扱いにしない）。
    let enabled = required
        .iter()
        .all(|value| !matches!(value, Err(std::env::VarError::NotPresent)));
    let non_utf8 = required
        .iter()
        .any(|value| matches!(value, Err(std::env::VarError::NotUnicode(_))));
    let required_present = enabled
        && required
            .iter()
            .all(|value| value.as_ref().is_ok_and(|value| !value.trim().is_empty()));
    let mut hard_errors = Vec::new();
    if non_utf8 {
        hard_errors.push(OIDC_NON_UTF8_ERROR);
    }
    // 真偽値は共通 parse_bool_env（1/true と 0/false のみ）に統一する。
    // 不正値・非 UTF-8 は fail-closed で false とし、起動拒否理由を hard_errors へ積む。
    let allow_mock_auth = match lookup("ALLOW_MOCK_AUTH") {
        Ok(value) => match common::age::parse_bool_env(&value) {
            Some(parsed) => parsed,
            None => {
                hard_errors.push(ALLOW_MOCK_AUTH_BOOL_ERROR);
                false
            }
        },
        // 未設定・非 UTF-8 は共に mock 無効（fail-closed）。生バイトは見ない。
        Err(_) => false,
    };

    if enabled {
        // SSRF 対策は is_valid_oidc_endpoint に集約する（必須変数が揃ったときだけ検証）。
        // 非 UTF-8 は上でハードエラー化済みなので、ここではデコードできた値だけを検証する。
        if ENDPOINTS
            .iter()
            .filter_map(|name| lookup(name).ok())
            .any(|value| !is_valid_oidc_endpoint(&value, allow_mock_auth))
        {
            hard_errors.push(OIDC_ENDPOINT_URL_ERROR);
        }
        // 必須 7 変数は Ok でも trim 後に空文字なら設定ミスとして起動を拒否する。
        // URL 検証の空文字は上で先に拒否されるが、配列は全 7 変数を含む。
        if required
            .iter()
            .any(|value| value.as_ref().is_ok_and(|value| value.trim().is_empty()))
        {
            hard_errors.push(OIDC_EMPTY_VALUE_ERROR);
        }
    }

    // 未設定は true（fail-closed）。不正値も Secure を外さず true とし、
    // 起動拒否理由を hard_errors へ積む（値はエコーしない）。
    let cookie_secure = match lookup("OIDC_COOKIE_SECURE") {
        Ok(value) => match common::age::parse_bool_env(&value) {
            Some(parsed) => parsed,
            None => {
                hard_errors.push(OIDC_COOKIE_SECURE_BOOL_ERROR);
                true
            }
        },
        Err(_) => true,
    };
    let clock_skew_secs = lookup("OIDC_CLOCK_SKEW_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| (CLOCK_SKEW_MIN_SECS..=CLOCK_SKEW_MAX_SECS).contains(seconds))
        .unwrap_or(DEFAULT_CLOCK_SKEW_SECS);
    let split_present = |name: &str| {
        lookup(name)
            .ok()
            .map(|value| value.split(',').any(|entry| !entry.trim().is_empty()))
            .unwrap_or(false)
    };
    let acr_amr_present = (
        split_present("OIDC_REQUIRED_ACR"),
        split_present("OIDC_REQUIRED_AMR"),
    );
    let max_auth_age_secs = match lookup("OIDC_MAX_AUTH_AGE_SECS") {
        // 非 UTF-8 は「未設定」ではなく不正値として起動拒否する（auth_time/max_age
        // 検証が黙って無効化される fail-open を防ぐ）。
        Err(std::env::VarError::NotUnicode(_)) => {
            hard_errors.push(OIDC_NON_UTF8_ERROR);
            None
        }
        Err(std::env::VarError::NotPresent) => None,
        Ok(value) => match value.parse::<u64>() {
            Ok(0) => {
                hard_errors.push(OIDC_MAX_AUTH_AGE_GREATER_ERROR);
                None
            }
            // 上限超過も 0 / 非数値と同じく起動拒否（fail-closed）。上限を超える
            // max_age は auth_time 検証を実質的に無効化するため、丸めずに拒否する。
            // 固定文言のみを積み、設定値はエコーしない。
            Ok(seconds) if seconds > MAX_AUTH_AGE_MAX_SECS => {
                hard_errors.push(OIDC_MAX_AUTH_AGE_TOO_LARGE_ERROR);
                None
            }
            Ok(seconds) => Some(seconds),
            // 負数・非数値・オーバーフローはすべて Err（起動拒否）。
            Err(_) => {
                hard_errors.push(OIDC_MAX_AUTH_AGE_POSITIVE_ERROR);
                None
            }
        },
    };

    OidcConfigValidation {
        required_present,
        enabled,
        cookie_secure,
        clock_skew_secs,
        max_auth_age_secs,
        acr_amr_present,
        hard_errors,
    }
}

/// リフレッシュトークンの SHA-256 ハッシュ。DB にはハッシュのみ保存する。
/// トークン本体は UUIDv4（128bit 乱数）を base64url で表現する。
fn hash_refresh_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// DB 保存形式（RFC3339）の `expires_at` を時刻として比較し、期限切れを判定する。
///
/// 文字列比較は小数秒の桁数（chrono の `AutoSi` は 0/3/6/9 桁を選ぶ）や
/// タイムゾーン表記の違いで前後が逆転しうるため、パースして比較する。
/// パース失敗時は fail-closed（期限切れ扱い）とする。
pub(crate) fn refresh_token_expired(expires_at: &str, now: DateTime<Utc>) -> bool {
    match DateTime::parse_from_rfc3339(expires_at) {
        Ok(expires_at) => expires_at.with_timezone(&Utc) <= now,
        Err(_) => true,
    }
}

/// 新しいリフレッシュトークンを生成し、呼び出し側のトランザクションへ INSERT する。
///
/// `refresh` では既に確保した `write_lock` と `BEGIN` を保持したまま呼び出す。
/// 別途 `begin_write_tx` を取るとガードの再取得でデッドロックするため、
/// refresh 経路では必ずこのヘルパを使う（コミットは呼び出し側の責務）。
async fn issue_refresh_token_in_tx(
    tx: &libsql::Transaction,
    sub: &str,
    family_id: &str,
) -> Result<String, AppError> {
    let token = URL_SAFE_NO_PAD.encode(Uuid::new_v4().as_bytes());
    let token_hash = hash_refresh_token(&token);
    let expires_at = (Utc::now() + Duration::seconds(REFRESH_TTL_SECS)).to_rfc3339();
    tx.execute(
        "INSERT INTO refresh_tokens (token_hash, sub, family_id, expires_at) VALUES (?, ?, ?, ?)",
        (
            token_hash,
            sub.to_string(),
            family_id.to_string(),
            expires_at,
        ),
    )
    .await?;
    Ok(token)
}

/// 新しいリフレッシュトークンを生成し、専用トランザクションで登録する（callback 用ラッパー）。
///
/// 既存の単発発行経路はこのラッパーを維持する。`refresh` は `write_lock` を
/// 保持したままのため、こちらではなく `issue_refresh_token_in_tx` を使うこと。
async fn issue_refresh_token(
    state: &AppState,
    sub: &str,
    family_id: &str,
) -> Result<String, AppError> {
    let (write_guard, tx) = crate::begin_write_tx(&state.db).await?;
    let token = issue_refresh_token_in_tx(&tx, sub, family_id).await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    drop(write_guard);
    Ok(token)
}

/// HttpOnly のリフレッシュトークン Cookie を構築する。
fn refresh_cookie(token: &str, secure: bool) -> Result<HeaderValue, AppError> {
    let value = format!(
        // /auth/logout でも Cookie が送られ、ファミリ失効が到達するようにする。
        "refresh_token={token}; HttpOnly; Path=/auth; SameSite=Strict; Max-Age={REFRESH_TTL_SECS}{}",
        if secure { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&value)
        .map_err(|_| AppError::Internal("invalid refresh cookie".to_string()))
}

/// HttpOnly のリフレッシュトークン Cookie を削除する（Max-Age=0）。
///
/// 属性は発行側 `refresh_cookie` と同じ Path/SameSite を使い、`secure` にも
/// 同じ `oidc.config.cookie_secure` を渡す。発行と削除で単一ソースを共有する。
pub fn delete_refresh_cookie(secure: bool) -> Result<HeaderValue, AppError> {
    let value = format!(
        "refresh_token=; Path=/auth; HttpOnly; SameSite=Strict; Max-Age=0{}",
        if secure { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&value)
        .map_err(|_| AppError::Internal("invalid refresh cookie".to_string()))
}

/// state Cookie の名前を `cookie_secure` から選ぶ。
///
/// `__Host-` プレフィックスは Secure 属性・Path=/・Domain なしを要求する。
/// 本番相当（`secure=true`）でのみ採用し、`secure=false`（mock）では
/// `__Host-` を付けない。Secure のない Cookie に `__Host-` を付けると
/// ブラウザが保存を拒否して OIDC フローが壊れるため、常時は使わない。
fn state_cookie_name(secure: bool) -> &'static str {
    if secure {
        HOST_OIDC_STATE_COOKIE
    } else {
        OIDC_STATE_COOKIE
    }
}

/// OIDC state の double-submit Cookie を構築する。
///
/// `state` が空文字のときは検証後の削除用（`Max-Age=0`）Cookie になる。
/// Secure 属性は `oidc.config.cookie_secure` に従う。start 時と callback 時で
/// 同じヘルパを通るため、命名と Path は必ず一致する。
fn state_cookie(state: &str, secure: bool) -> Result<HeaderValue, AppError> {
    let (value, max_age) = if state.is_empty() {
        ("", 0)
    } else {
        (state, OIDC_STATE_COOKIE_MAX_AGE)
    };
    // `__Host-` Cookie は Path=/ かつ Domain なしが必須。旧名は Path=/auth を維持する。
    let path = if secure { "/" } else { "/auth" };
    let cookie = format!(
        "{}={value}; HttpOnly; Path={path}; SameSite=Lax; Max-Age={max_age}{}",
        state_cookie_name(secure),
        if secure { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&cookie)
        .map_err(|_| AppError::Internal("invalid OIDC state cookie".to_string()))
}

/// `Cookie` ヘッダから state Cookie の値を取り出す。
///
/// 同一名 Cookie が2つ以上ある場合は攻撃の兆候として `None` を返す。
/// `find_map` の先勝ちだと攻撃者値が先に来たときにすり替えられるため、
/// 重複は例外的に検証失敗へつなげる。値は比較のためだけに使い、
/// エラーメッセージには含めない。
fn state_from_cookie(headers: &HeaderMap, secure: bool) -> Option<&str> {
    let name = state_cookie_name(secure);
    let mut found: Option<&str> = None;
    for header_value in headers.get_all(header::COOKIE) {
        // デコード不能なヘッダは検証失敗扱い（fail-closed）。
        let Ok(cookies) = header_value.to_str() else {
            return None;
        };
        for part in cookies.split(';') {
            let Some((cookie_name, value)) = part.trim().split_once('=') else {
                continue;
            };
            if cookie_name != name {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some(value);
        }
    }
    found
}

/// `Cookie` ヘッダから refresh_token Cookie の値を取り出す。
///
/// `state_from_cookie` と同じ基準で、同一名 Cookie が2つ以上ある場合は
/// 攻撃の兆候として `None` を返す。先勝ちの `find_map` では攻撃者値が
/// 先に来たときにすり替えられるため、重複は検証失敗へつなげる。
/// `get_all` で全ヘッダを走査し、デコード不能なヘッダも fail-closed で
/// `None` にする。値は比較のためだけに使い、エラーには含めない。
///
/// `lib.rs` の logout 経路からも参照するため `pub` にしている。
pub fn refresh_cookie_from_headers(headers: &HeaderMap) -> Option<&str> {
    let mut found: Option<&str> = None;
    for header_value in headers.get_all(header::COOKIE) {
        // デコード不能なヘッダは検証失敗扱い（fail-closed）。
        let Ok(cookies) = header_value.to_str() else {
            return None;
        };
        for part in cookies.split(';') {
            let Some((cookie_name, value)) = part.trim().split_once('=') else {
                continue;
            };
            if cookie_name != "refresh_token" {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some(value);
        }
    }
    found
}

/// `/auth/refresh` の Origin 検証。Cookie を読む前に呼ぶ。
///
/// `Origin` は `ALLOWED_ORIGINS` のいずれかと完全一致でなければならない。
/// `Sec-Fetch-Site: cross-site` は明示拒否するが、`same-origin` / `same-site` /
/// ヘッダ欠損は許可する（`app.example.com` → `api.example.com` の
/// same-site cross-origin 契約を維持するため）。
fn refresh_origin_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return false;
    };
    if !state
        .allowed_origins
        .iter()
        .any(|allowed| allowed == origin)
    {
        return false;
    }
    !headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
}

/// 再認証で不要になった古い失効閾値だけを DB から削除する。
/// 新しい閾値が back-channel logout で登録された場合は残す。
pub(crate) async fn delete_superseded_revocation(
    db: &libsql::Connection,
    sub: &str,
    old_threshold: i64,
) -> Result<(), AppError> {
    let (write_guard, tx) = crate::begin_write_tx(db).await?;
    tx.execute(
        "DELETE FROM revoked_subs WHERE sub = ? AND revoked_at <= ?",
        libsql::params![sub.to_string(), old_threshold],
    )
    .await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    drop(write_guard);
    Ok(())
}

/// ファミリ全体を失効させる（盗用検知時）。
async fn revoke_refresh_family(state: &AppState, family_id: &str) -> Result<(), AppError> {
    let (write_guard, tx) = crate::begin_write_tx(&state.db).await?;
    tx.execute(
        "UPDATE refresh_tokens SET used_at = datetime('now') WHERE family_id = ? AND used_at IS NULL",
        [family_id],
    )
    .await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    drop(write_guard);
    Ok(())
}

/// 指定 `sub` に紐づく未使用リフレッシュトークンをすべて消費する。
///
/// back-channel logout の `sub` 失効後も古いトークンから refresh できると、
/// 新しい `iat` を付けた JWT で失効閾値を迂回できてしまう。消費状態は DB に
/// 永続化されるため、プロセス再起動後も失効が維持される。
pub(crate) async fn revoke_refresh_tokens_for_sub(
    state: &AppState,
    sub: &str,
) -> Result<(), AppError> {
    let (write_guard, tx) = crate::begin_write_tx(&state.db).await?;
    tx.execute(
        "UPDATE refresh_tokens SET used_at = datetime('now') WHERE sub = ? AND used_at IS NULL",
        [sub],
    )
    .await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    drop(write_guard);
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

pub(crate) struct PendingAuth {
    nonce: String,
    code_verifier: String,
    created_at: Instant,
}

/// TTL を過ぎた pending を一括削除し、削除件数を返す。
/// `take_pending` と `register_pending` の双方から呼び、取出しが無い経路でも掃除されるようにする。
fn purge_expired_pending(pending: &mut HashMap<String, PendingAuth>) -> usize {
    let now = Instant::now();
    let before = pending.len();
    pending.retain(|_, auth| now.saturating_duration_since(auth.created_at) < PENDING_TTL);
    before - pending.len()
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
    /// 不正な値が設定されていた場合は `from_env_with` が返す Err を伝播させ、
    /// 起動を拒否する（auth_time / max_age 検証の黙った無効化を防ぐ）。
    pub fn from_env() -> Option<Arc<Self>> {
        match Self::from_env_with(|name: &str| std::env::var(name)) {
            Ok(state) => state,
            Err(message) => panic!("invalid OIDC configuration: {message}"),
        }
    }

    /// 環境変数の読み取りを注入した `from_env` の本体。
    ///
    /// 本番は `from_env()` が `std::env::var` をそのまま渡す。テストは文字列マップを
    /// Result 契約で返すヘルパを渡すことでプロセス環境を汚さずに、必須変数の欠落と
    /// 各オプションの境界値を並列に検証できる。
    ///
    /// 必須変数の欠落は `Ok(None)`（OIDC 無効）だが、値が存在するのに不正な
    /// 場合は `Err` を返す。非 UTF-8 は「未設定」ではなく不正値として扱い、
    /// `enabled=true` のままハードエラーを優先して起動拒否する（防御的）。
    /// 特に `OIDC_MAX_AUTH_AGE_SECS` は 0 / 負 / 非数値 / オーバーフロー /
    /// 上限（30日）超過を捨てると max_age 検証が黙って無効化されるため、
    /// フォールバックせず起動拒否にする（fail-closed）。
    /// 判定は `validate_oidc_config_with` に集約し、ここでは秘密値の読み出しと
    /// `OidcState` の組み立てだけを行う（doctor へは渡さない）。
    pub(crate) fn from_env_with<F>(lookup: F) -> Result<Option<Arc<Self>>, String>
    where
        F: Fn(&str) -> Result<String, std::env::VarError>,
    {
        let validation = validate_oidc_config_with(&lookup);
        // 不正値（非 UTF-8 を含む）は enabled 判定より先に拒否する。
        // ハードエラーを「未設定」扱いの Ok(None) へ流さないための防御的順序。
        if let Some(error) = validation.hard_errors.first() {
            return Err((*error).to_string());
        }
        if !validation.enabled {
            return Ok(None);
        }

        let (
            issuer,
            client_id,
            client_secret,
            redirect_uri,
            authorization_endpoint,
            token_endpoint,
            jwks_uri,
        ) = match (
            lookup("OIDC_ISSUER").ok(),
            lookup("OIDC_CLIENT_ID").ok(),
            lookup("OIDC_CLIENT_SECRET").ok(),
            lookup("OIDC_REDIRECT_URI").ok(),
            lookup("OIDC_AUTHORIZATION_ENDPOINT").ok(),
            lookup("OIDC_TOKEN_ENDPOINT").ok(),
            lookup("OIDC_JWKS_URI").ok(),
        ) {
            (
                Some(issuer),
                Some(client_id),
                Some(client_secret),
                Some(redirect_uri),
                Some(authorization_endpoint),
                Some(token_endpoint),
                Some(jwks_uri),
            ) => (
                issuer,
                client_id,
                client_secret,
                redirect_uri,
                authorization_endpoint,
                token_endpoint,
                jwks_uri,
            ),
            // validation.enabled が真なら到達しない。fail-closed の保険として残す。
            _ => return Ok(None),
        };
        let scopes = lookup("OIDC_SCOPES")
            .ok()
            .unwrap_or_else(|| "openid".to_string());
        let required_acr = lookup("OIDC_REQUIRED_ACR")
            .ok()
            .map(|value| Self::split_config_list(&value))
            .unwrap_or_default();
        let required_amr = lookup("OIDC_REQUIRED_AMR")
            .ok()
            .map(|value| Self::split_config_list(&value))
            .unwrap_or_default();

        Ok(Some(Arc::new(Self::new(OidcConfig {
            issuer,
            client_id,
            client_secret,
            redirect_uri,
            authorization_endpoint,
            token_endpoint,
            jwks_uri,
            scopes,
            cookie_secure: validation.cookie_secure,
            clock_skew_secs: validation.clock_skew_secs,
            required_acr: std::sync::Arc::new(std::sync::Mutex::new(required_acr)),
            required_amr: std::sync::Arc::new(std::sync::Mutex::new(required_amr)),
            max_auth_age_secs: std::sync::Arc::new(std::sync::Mutex::new(
                validation.max_auth_age_secs,
            )),
        }))))
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

    /// テスト用: `from_env_with` が解析した設定を読み取る（本番コードからは使わない）。
    #[cfg(test)]
    pub(crate) fn config(&self) -> &OidcConfig {
        &self.config
    }

    /// Cookie 発行・削除で共有する `cookie_secure` の単一ソース。
    pub(crate) fn cookie_secure(&self) -> bool {
        self.config.cookie_secure
    }

    /// OIDC client_id。verified_claims の audience フォールバック等で使用する。
    pub(crate) fn client_id(&self) -> &str {
        &self.config.client_id
    }

    /// 期限切れ pending を回収し、削除件数を返す。
    /// `register_pending` / `take_pending` は認証リクエストが来たときだけ掃除するため、
    /// 起動時 purge や周期 tick からも呼んでアイドル中の古いエントリを回収する。
    /// ロック取得に失敗した場合は best-effort として 0 を返す。
    pub(crate) fn sweep_expired_pending(&self) -> usize {
        self.pending
            .lock()
            .map(|mut pending| purge_expired_pending(&mut pending))
            .unwrap_or(0)
    }

    /// 期限切れを掃除したうえで pending を登録する。
    /// 上限に達している場合は 429（TooManyRequests）を返し、新規エントリを増やさない。
    /// 認証開始は未認証で呼べるため、ここがメモリ枯渇に対する境界になる（R1-23）。
    pub(crate) fn register_pending(
        &self,
        auth_state: String,
        pending: PendingAuth,
    ) -> Result<(), AppError> {
        let mut store = self
            .pending
            .lock()
            .map_err(|_| AppError::Internal("OIDC pending store is poisoned".to_string()))?;
        purge_expired_pending(&mut store);
        if store.len() >= MAX_PENDING_AUTH {
            return Err(AppError::TooManyRequests(
                "too many pending OIDC authorizations".to_string(),
            ));
        }
        store.insert(auth_state, pending);
        Ok(())
    }

    fn take_pending(&self, state: &str) -> Result<PendingAuth, AppError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| AppError::Internal("OIDC pending store is poisoned".to_string()))?;
        purge_expired_pending(&mut pending);
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

    /// テスト用: `register_pending` に渡すダミーの pending を生成する。
    #[cfg(test)]
    pub(crate) fn test_pending(nonce: &str) -> PendingAuth {
        PendingAuth {
            nonce: nonce.to_string(),
            code_verifier: "test-code-verifier".to_string(),
            created_at: Instant::now(),
        }
    }

    /// テスト用: 現在保持している pending の数。
    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.lock().map(|store| store.len()).unwrap_or(0)
    }

    /// テスト用: 指定 state の `created_at` を TTL 超過分だけ過去にずらす。
    /// 期限切れエントリが `register_pending` で回収されることを検証する。
    #[cfg(test)]
    pub(crate) fn expire_pending(&self, state: &str) {
        if let Ok(mut store) = self.pending.lock()
            && let Some(auth) = store.get_mut(state)
        {
            auth.created_at -= PENDING_TTL + StdDuration::from_secs(1);
        }
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

/// ID トークンのクレーム。すべてのフィールドが検証で使用する。
/// `exp` / `aud` / `nbf` は `jsonwebtoken::Validation`（leeway と validate_nbf）が、
/// それ以外は `verify_id_token` の明示チェックが読む。
#[derive(Debug, Deserialize)]
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
struct LogoutTokenClaims {
    iss: String,
    #[serde(deserialize_with = "deserialize_audience")]
    aud: Vec<String>,
    exp: usize,
    iat: usize,
    /// nbf は任意クレームとして受理する（必須化しない）。
    nbf: Option<usize>,
    sub: Option<String>,
    sid: Option<String>,
    events: serde_json::Value,
    /// Back-channel logout token には nonce を含めてはならない（仕様違反の検知用）。
    /// 型を Value にして、文字列以外の値が入っていても確実に拒否できるようにする。
    nonce: Option<serde_json::Value>,
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
    // R1-19 / ADR-0002: jsonwebtoken 12 の既定は validate_nbf = false のため明示的に有効化する。
    // leeway には OIDC_CLOCK_SKEW_SECS を流用する。nbf は Option であり必須クレームには
    // 含めないため、クレーム欠落時は従来どおり受理される（値がある場合のみ検証される）。
    validation.validate_nbf = true;

    let data = decode::<IdTokenClaims>(id_token, &key, &validation)
        .map_err(|_| AppError::Unauthorized("ID token validation failed".to_string()))?;
    let claims = data.claims;

    // exp / aud / nbf は上の Validation が検証する。ここでも参照して
    // 「全クレームが検証対象」を明示し、dead_code 警告を避ける（挙動は変えない）。
    let _ = (&claims.exp, &claims.aud, claims.nbf);

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
    // nbf が付いている場合は有効開始時刻として検証する（欠落は受理）。
    validation.validate_nbf = true;

    let data = decode::<LogoutTokenClaims>(logout_token, &key, &validation)
        .map_err(|_| AppError::Unauthorized("logout token validation failed".to_string()))?;
    let claims = data.claims;

    // nbf / iss / aud / exp は Validation が検証し、必須化せずに欠落を受理するため、
    // 構造体側の dead_code 解析を明示的に回避する（値自体は使わない）。
    let _ = (&claims.nbf, &claims.iss, &claims.aud, claims.exp);

    let now = Utc::now().timestamp() as usize;
    if claims.iat > now.saturating_add(oidc.config.clock_skew_secs as usize) {
        return Err(AppError::Unauthorized(
            "logout token iat is in the future".to_string(),
        ));
    }

    // 仕様では events の値は空 object でなければならない。
    // キーの存在だけを見ると {"...": {"foo": "bar"}} のような不正な値を通してしまう。
    if !claims
        .events
        .get(BACKCHANNEL_LOGOUT_EVENT)
        .and_then(serde_json::Value::as_object)
        .is_some_and(|event| event.is_empty())
    {
        return Err(AppError::Unauthorized(
            "logout token has an invalid backchannel event".to_string(),
        ));
    }
    // 署名済みトークンなので、nonce の混入は改ざんではなく発行側の仕様違反として拒否する。
    if claims.nonce.is_some() {
        return Err(AppError::Unauthorized(
            "logout token must not contain a nonce".to_string(),
        ));
    }
    // OIDC Back-Channel Logout 仕様では sub は任意クレーム（sid のみでも可）だが、
    // 本システムのリフレッシュ失効は revoked_subs の sub に依存している。
    // sid のみを受理すると refresh から新しいセッションが発行され、
    // ログアウトしたはずのセッションが復活するため、fail-closed で sub を必須化する。
    if claims.sub.is_none() {
        return Err(AppError::Unauthorized(
            "logout token requires sub".to_string(),
        ));
    }

    Ok(claims)
}

pub async fn start(State(state): State<Arc<AppState>>) -> Result<Response, AppError> {
    let oidc = state
        .oidc
        .as_ref()
        .ok_or_else(|| AppError::NotImplemented("OIDC is not configured".to_string()))?;

    let auth_state = random_token();
    let nonce = random_token();
    let code_verifier = random_token();
    let challenge = code_challenge(&code_verifier);

    oidc.register_pending(
        auth_state.clone(),
        PendingAuth {
            nonce: nonce.clone(),
            code_verifier,
            created_at: Instant::now(),
        },
    )?;

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

    // ログイン CSRF 対策: 生成した state を Cookie にも入れ、callback で
    // query と突き合わせる（double-submit）。JSON 本体は従来どおり返す。
    let cookie = state_cookie(&auth_state, oidc.config.cookie_secure)?;
    let mut response = Json(StartResponse {
        authorization_url: url.to_string(),
        state: auth_state,
    })
    .into_response();
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    Ok(response)
}

/// OIDC 主体の「署名鍵未登録」プレースホルダ（subject 由来の決定論的バイト列）。
/// OIDC は認証のみを提供し署名鍵を配布しないため、`public_key_der` を NULL の
/// まま残すと下流の署名検証が NULL デコードで 500 になる。初回登録時にこの
/// 値を保存して署名鍵未登録を明示する（下流は固定文言の 403/501 で扱う）。
/// 実鍵を登録済みの主体は上書きしない（呼び出し側の `WHERE ... IS NULL`）。
const OIDC_SIGNATURE_FREE_DER_PREFIX: &[u8] = b"oidc-no-signing-key:";

fn oidc_placeholder_public_key(user_hash: &str) -> Vec<u8> {
    let mut marker = OIDC_SIGNATURE_FREE_DER_PREFIX.to_vec();
    marker.extend_from_slice(&Sha256::digest(user_hash.as_bytes()));
    marker
}

pub async fn callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, AppError> {
    let oidc = state
        .oidc
        .as_ref()
        .ok_or_else(|| AppError::NotImplemented("OIDC is not configured".to_string()))?;

    // R1-17: 年齢ゲートの設定チェックは全セッション発行経路で共通化する。
    // 信頼属性必須モード／明示設定必須モードでは、OIDC 経路に検証済み年齢属性が
    // 無いため fail-closed で拒否する（数値判定は auth_verify に委ねる）。
    crate::ensure_age_policy_configured(&state.age_policy)?;

    // DEC-01: 署名付き年齢属性 adapter の境界を設定ゲート直後に配線する。
    // 実証明書／検証済み属性の入力経路は未接続のため、信頼属性必須モードでは
    // ここで fail closed（上記と同じ 403）。既定の自己申告モードでは no-op。
    // 認可フロー（state / PKCE / ID トークン検証）は変更しない。
    crate::verified_age::verify_oidc_age_binding(&state.age_policy)?;

    // ログイン CSRF（state すり替え）対策: Cookie の state と query の state を
    // 比較し、一致しなければ値をエコーせずに拒否する。take_pending より前で
    // 検証し、pending を消費させない。
    let cookie_state = state_from_cookie(&headers, oidc.config.cookie_secure)
        .ok_or_else(|| AppError::Unauthorized("OIDC state mismatch".to_string()))?;
    if cookie_state != query.state.as_str() {
        return Err(AppError::Unauthorized("OIDC state mismatch".to_string()));
    }

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
    // BAN 済み subject にはセッションを発行しない。INSERT / JWT 発行の前に
    // refresh と同じ共通ゲートで拒否する（年齢ゲートの呼び出し順は変更しない）。
    crate::ensure_user_not_banned(&state.db, &user_hash).await?;
    // 外部 HTTP（token exchange / JWKS 検証）はすべて上述で完了している。
    // 書き込みだけを write_lock 配下の tx で行い、HTTP 待ちでロックを保持しない。
    let (write_guard, tx) = crate::begin_write_tx(&state.db).await?;
    // BAN 原子化: 事前チェックから tx 取得の間で BAN がコミットされても
    // セッションを発行しないよう、write_lock 保持下の tx 内で
    // users.is_banned / ban_until を再読し、有効 BAN なら rollback して 403。
    // 判定述語は auth_verify / ensure_user_not_banned と同じ（期限切れ BAN は
    // 通過、NULL は無期限継続）。コミットはこの再読が通った後だけに行う。
    let mut ban_rows = tx
        .query(
            "SELECT 1 FROM users
             WHERE user_hash = ?
               AND is_banned = TRUE
               AND (ban_until IS NULL OR ban_until > datetime('now'))",
            [user_hash.clone()],
        )
        .await?;
    if ban_rows.next().await?.is_some() {
        drop(ban_rows);
        drop(tx); // 明示 commit せず rollback して BAN 決定を漏らさない。
        drop(write_guard);
        return Err(AppError::Forbidden("User is banned".to_string()));
    }
    drop(ban_rows);
    // OIDC は署名鍵を配布しないため、初回登録で public_key_der を NULL にすると
    // 下流の署名検証が NULL デコードで 500 になる。subject 由来のプレースホルダ
    // を保存して署名鍵未登録を明示する。既に実鍵が登録済みの場合は上書きしない。
    tx.execute(
        "INSERT INTO users (user_hash, public_key_der) VALUES (?, ?)
         ON CONFLICT(user_hash) DO UPDATE SET public_key_der = excluded.public_key_der
         WHERE users.public_key_der IS NULL",
        (user_hash.clone(), oidc_placeholder_public_key(&user_hash)),
    )
    .await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    // 後続の delete / refresh token 発行も同じロックを取るため明示解放する。
    drop(write_guard);

    let now = Utc::now();
    let now_ts = now.timestamp() as usize;
    let expiration = now
        .checked_add_signed(Duration::seconds(SESSION_TTL_SECS))
        .expect("valid timestamp")
        .timestamp() as usize;
    let (iat, old_threshold) = {
        let mut revoked_subs = state
            .revoked_subs_before
            .lock()
            .map_err(|_| crate::AppError::Internal("session store is poisoned".to_string()))?;
        let old = revoked_subs.remove(&user_hash);
        let issued_at = match old {
            Some(threshold) => std::cmp::max(now_ts, threshold.saturating_add(1)),
            None => now_ts,
        };
        // 新しい認証セッションの発行により、過去セッション失効の閾値は不要になる。
        (issued_at, old)
    };
    // 再認証で旧セッション拒否の役割を終えた閾値を DB からも削除する。
    // 古い閾値より新しい値が back-channel logout で登録された場合は残す
    // （メモリ削除と DB 削除の間のレースで直近の失効が消えないようにする）。
    if let Some(old_threshold) = old_threshold {
        if let Err(err) =
            delete_superseded_revocation(&state.db, &user_hash, old_threshold as i64).await
        {
            // DB が失敗したら先に remove したメモリを旧閾値へ戻す（auth_verify と同じ）。
            // 並行して新しい閾値が登録済みなら or_insert は上書きしない。
            state
                .revoked_subs_before
                .lock()
                .map_err(|_| crate::AppError::Internal("session store is poisoned".to_string()))?
                .entry(user_hash.clone())
                .or_insert(old_threshold);
            return Err(err);
        }
    }
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

    // R2-SESSION-COOKIE: session JWT は応答本文（VerifyResponse.token）のみで搬送し、
    // session Cookie は発行しない。SET_COOKIE は refresh と state 削除の2本だけ。
    let mut response = Json(VerifyResponse {
        authorized: true,
        user_hash: Some(user_hash),
        token: Some(session_token),
    })
    .into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        refresh_cookie(&refresh_token, oidc.config.cookie_secure)?,
    );
    // 検証に成功した state Cookie は使い捨てなので削除する。
    response.headers_mut().append(
        header::SET_COOKIE,
        state_cookie("", oidc.config.cookie_secure)?,
    );
    Ok(response)
}

/// リフレッシュトークンを回転させ、新しいセッション JWT とリフレッシュトークンを発行する。
/// 既に使用済みのトークンが再提示された場合は盗用とみなし、ファミリ全体を失効する。
pub async fn refresh(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // R1-17: refresh も callback と同じ共通ゲートを通し、年齢判定を迂回できないようにする。
    crate::ensure_age_policy_configured(&state.age_policy)?;
    // R2-REFRESH-ORIGIN: Cookie を読む前に Origin を完全一致で検証する。
    if !refresh_origin_allowed(&state, &headers) {
        return Err(AppError::Unauthorized(
            "refresh origin is not allowed".to_string(),
        ));
    }
    let token = refresh_cookie_from_headers(&headers)
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
    if refresh_token_expired(&expires_at, Utc::now()) {
        return Err(AppError::Unauthorized("refresh token expired".to_string()));
    }

    // sub の失効閾値が DB に残っている間は refresh を許可しない。
    // back-channel logout と refresh が競合して未使用トークンが残っても、
    // 再認証前にセッションを延長できないようにする（再起動後も DB で効く）。
    let sub_revoked = {
        let mut revoked_rows = state
            .db
            .query("SELECT 1 FROM revoked_subs WHERE sub = ?", [sub.clone()])
            .await?;
        revoked_rows.next().await?.is_some()
    };
    if sub_revoked {
        revoke_refresh_tokens_for_sub(&state, &sub).await?;
        return Err(AppError::Unauthorized(
            "session for this subject has been revoked".to_string(),
        ));
    }

    // (a) BAN チェックは BEGIN より前に移動する。共通ゲートでの拒否を
    //     write_lock 保持中に行わない（判定順序は従来どおり fail-closed）。
    ensure_user_not_banned(&state.db, &sub).await?;

    // (b) セッション claims / JWT の事前計算も BEGIN より前に済ませる。
    //     署名や Mutex 取得を tx の外で完了させ、tx 内は DB 操作だけにする。
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

    // (d) 消費 UPDATE と新トークン INSERT を単一 tx で原子的に実行する。
    //     write_lock → BEGIN → UPDATE → changes() 判定 → INSERT(in_tx) → COMMIT。
    //     write_lock が Mutex poison を検出しても panic せず、ここでの `?` により
    //     AppError::Internal として伝播する（begin_write_tx_sql 経由）。
    let (write_guard, tx) = crate::begin_write_tx(&state.db).await?;
    let updated = tx
        .execute(
            "UPDATE refresh_tokens SET used_at = datetime('now') WHERE token_hash = ? AND used_at IS NULL",
            [token_hash],
        )
        .await?;
    if updated == 0 {
        // 競合して既に消費済み。tx をロールバックし、ガードを解放してから
        // 別tx でファミリ失効を行う（ガード保持のまま再取得するとデッドロック）。
        tx.rollback()
            .await
            .map_err(|e| AppError::Internal(format!("Transaction rollback failed: {e}")))?;
        drop(write_guard);
        revoke_refresh_family(&state, &family_id).await?;
        return Err(AppError::Unauthorized(
            "refresh token reuse detected; token family revoked".to_string(),
        ));
    }
    // 既存 tx のまま新トークンを INSERT する。
    // issue_refresh_token（別tx 取得）を呼ぶと write_lock 再取得でデッドロックする。
    let new_refresh = issue_refresh_token_in_tx(&tx, &sub, &family_id).await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    drop(write_guard);

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

/// logout token の `sub` 側失効を処理する。
///
/// `sid` 側（`revoke_session`）と独立に呼び出すため、ここで返ったエラーは
/// caller が集約する。再送・古い `iat` は UPSERT の変更件数で冪等にスキップする。
async fn revoke_sub_from_logout_token(
    state: &Arc<AppState>,
    sub: &str,
    revoked_at: usize,
) -> Result<(), AppError> {
    // 更新時刻は受け取った logout token の iat とする。再送や古いトークンは
    // 閾値を前進させず、冪等に成功応答を返す（DB 側 UPSERT の変更件数で判定）。
    // プロセス再起動後も失効を維持するため DB 側の閾値を先にコミットする。
    // logout token の検証（HTTP / JWKS）はすべて上流で完了済みなので、
    // ロック取得中に外部呼び出しが発生して他書き込みを止めることはない。
    let (write_guard, tx) = crate::begin_write_tx(&state.db).await?;
    let changed = tx
        .execute(
            "INSERT INTO revoked_subs (sub, revoked_at) VALUES (?, ?)
             ON CONFLICT(sub) DO UPDATE SET revoked_at = excluded.revoked_at,
                 updated_at = CURRENT_TIMESTAMP
             WHERE revoked_subs.revoked_at < excluded.revoked_at",
            (sub.to_string(), revoked_at as i64),
        )
        .await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Transaction commit failed: {}", e)))?;
    drop(write_guard);

    // 変更0件 = 同一 iat の再送、または閾値を前進させない古いトークン。
    // この場合はメモリ更新とリフレッシュトークン消費をスキップする。
    if changed == 0 {
        return Ok(());
    }

    state
        .revoked_subs_before
        .lock()
        .map_err(|_| AppError::Internal("session store is poisoned".to_string()))?
        .insert(sub.to_string(), revoked_at);
    // 失効前に発行された未使用リフレッシュトークンを消費し、refresh からの
    // 迂回（iat = 閾値+1 の JWT 発行）を防ぐ。DB 状態なので再起動後も有効。
    revoke_refresh_tokens_for_sub(state, sub).await?;
    Ok(())
}

/// OIDC Back-Channel Logout。`sid` と `sub` の失効を独立に試行し、
/// 片方が失敗してもう一方を必ず実行する。失敗は集約して返す。
pub async fn backchannel_logout(
    State(state): State<Arc<AppState>>,
    Form(payload): Form<LogoutTokenRequest>,
) -> Result<StatusCode, AppError> {
    let oidc = state
        .oidc
        .as_ref()
        .ok_or_else(|| AppError::NotImplemented("OIDC is not configured".to_string()))?;
    let claims = verify_logout_token(&state.http_client, oidc, &payload.logout_token).await?;
    // `claims.sid` / `claims.sub` を move しても読めるよう、Copy な iat を先に退避する。
    let revoked_at = claims.iat;

    let mut failures: Vec<String> = Vec::new();

    // sid 側の DB エラーで sub 側を止めないよう、結果をここで受け止めて次へ進む。
    if let Some(sid) = claims.sid {
        if let Err(error) = crate::revoke_session(&state, &sid).await {
            failures.push(format!("sid revocation failed: {}", error));
        }
    }

    if let Some(sub) = claims.sub {
        if let Err(error) = revoke_sub_from_logout_token(&state, &sub, revoked_at).await {
            failures.push(format!("sub revocation failed: {}", error));
        }
    }

    if !failures.is_empty() {
        return Err(AppError::Internal(format!(
            "back-channel logout partially failed: {}",
            failures.join("; ")
        )));
    }

    Ok(StatusCode::OK)
}
