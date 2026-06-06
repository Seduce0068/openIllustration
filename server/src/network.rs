use axum::{
    Json,
    extract::{ConnectInfo, Request},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// SEC-01: IPv4 接続を拒否（開発時はループバック IPv4 のみ許可可能）
pub async fn ipv6_only_middleware(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let client_ip = client_ip_from_request(request.headers(), Some(peer.ip()));

    if let Some(ip) = client_ip
        && is_ipv4(ip)
        && !ipv4_permitted(ip)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "IPv6-only access required. Legacy IPv4 connections are not supported."
            })),
        )
            .into_response();
    }

    next.run(request).await
}

pub fn client_ip_from_request(headers: &HeaderMap, connect_ip: Option<IpAddr>) -> Option<IpAddr> {
    if let Some(forwarded) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(first) = forwarded.split(',').next()
        && let Ok(ip) = first.trim().parse::<IpAddr>()
    {
        return Some(ip);
    }
    connect_ip
}

pub fn is_ipv4(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(_))
}

/// `ALLOW_IPV4_LOCALHOST=1` のとき 127.0.0.0/8 のみ IPv4 を許可（ローカル開発用）
pub fn ipv4_permitted(ip: IpAddr) -> bool {
    if std::env::var("ALLOW_IPV4_LOCALHOST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        && let IpAddr::V4(v4) = ip
    {
        return v4.is_loopback() || v4 == Ipv4Addr::LOCALHOST;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ipv4() {
        assert!(is_ipv4("203.0.113.1".parse().unwrap()));
        assert!(!is_ipv4("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn public_ipv4_not_permitted_without_env() {
        assert!(!ipv4_permitted("203.0.113.1".parse().unwrap()));
    }
}
