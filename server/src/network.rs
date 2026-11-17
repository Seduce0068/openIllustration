use axum::{
    Json,
    extract::{ConnectInfo, Request},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::OnceLock;

/// SEC-01: IPv4 接続を拒否（開発時はループバック IPv4 のみ許可可能）
///
/// クライアント IP は実際のピアアドレスを既定とし、ピアが
/// `TRUSTED_PROXY_CIDRS` に含まれるときだけ `X-Forwarded-For` の末尾値を
/// 採用する。信頼済みピアでヘッダーを解決できない場合は fail-closed
/// として拒否する（NET-02）。
pub async fn ipv6_only_middleware(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    resolve_client_ip_and_run(trusted_proxy_ranges(), ConnectInfo(peer), request, next).await
}

/// アクセス判定と、解決済みクライアント IP の extension 注入をまとめた実処理。
///
/// 本番ミドルウェアは `trusted_proxy_ranges()` を渡す。テストからは信頼済み
/// プロキシ範囲を明示して呼ぶことで、`TRUSTED_PROXY_CIDRS` の `OnceLock` を
/// 汚さずに同じ経路を検証できる（`AccessDecision` は private のまま）。
pub(crate) async fn resolve_client_ip_and_run(
    ranges: &[String],
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    let connect_ip = canonical_ip(peer.ip());
    let trust_forwarded = is_trusted_proxy(connect_ip, ranges);
    match evaluate_access(request.headers(), connect_ip, trust_forwarded) {
        AccessDecision::Allowed(client_ip) => {
            // rate_limit_middleware はこの解決済み IP をキーにする（XFF 解決済み）。
            request.extensions_mut().insert(client_ip);
            next.run(request).await
        }
        AccessDecision::UnverifiedClient => {
            tracing::warn!(
                "Rejected request: client address could not be resolved from the trusted proxy chain"
            );
            forbidden("Client address could not be verified.")
        }
        AccessDecision::Ipv4Forbidden => {
            tracing::debug!("Rejected IPv4 client (SEC-01)");
            forbidden("IPv6-only access required. Legacy IPv4 connections are not supported.")
        }
    }
}

fn forbidden(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": message })),
    )
        .into_response()
}

/// ミドルウェアのアクセス判定結果。
#[derive(Debug, PartialEq, Eq)]
enum AccessDecision {
    /// 許可。判定に使ったクライアント IP。
    Allowed(IpAddr),
    /// 信頼済みプロキシからクライアント IP を解決できない（fail-closed）。
    UnverifiedClient,
    /// IPv4 クライアントであり、許可されていない（SEC-01）。
    Ipv4Forbidden,
}

/// 信頼判定・fail-closed・IPv4 ゲートの結線を環境変数から分離した判定本体。
fn evaluate_access(
    headers: &HeaderMap,
    connect_ip: IpAddr,
    trust_forwarded: bool,
) -> AccessDecision {
    let Some(client_ip) = client_ip_from_request(headers, Some(connect_ip), trust_forwarded) else {
        return AccessDecision::UnverifiedClient;
    };
    if is_ipv4(client_ip) && !ipv4_permitted(client_ip) {
        return AccessDecision::Ipv4Forbidden;
    }
    AccessDecision::Allowed(client_ip)
}

/// 起動時に一度だけ評価される信頼済みプロキシ範囲（`TRUSTED_PROXY_CIDRS`）。
static TRUSTED_RANGES: OnceLock<Vec<String>> = OnceLock::new();

/// `TRUSTED_PROXY_CIDRS`（カンマ区切りの IP / CIDR）で指定された信頼済み
/// プロキシ範囲を返す。未設定時は空となり、`X-Forwarded-For` を信用しない。
/// 不正な要素と全アドレスを含む範囲（プレフィックス長 0）は警告のうえ
/// 無視する。
pub fn trusted_proxy_ranges() -> &'static [String] {
    TRUSTED_RANGES.get_or_init(|| {
        std::env::var("TRUSTED_PROXY_CIDRS")
            .map(|value| validated_ranges(parse_ranges(&value)))
            .unwrap_or_default()
    })
}

fn parse_ranges(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

fn validated_ranges(entries: Vec<String>) -> Vec<String> {
    entries
        .into_iter()
        .filter(|entry| match classify_range(entry) {
            Ok(_) => true,
            Err(reason) => {
                tracing::warn!("Ignoring TRUSTED_PROXY_CIDRS entry '{entry}': {reason}");
                false
            }
        })
        .collect()
}

/// 接続元が信頼済みプロキシ範囲に含まれるか。
pub fn is_trusted_proxy(proxy: IpAddr, ranges: &[String]) -> bool {
    ranges.iter().any(|range| cidr_contains(range, proxy))
}

/// IP 単体または CIDR 表記の範囲を解釈する。プレフィックス長 0（全アドレス）
/// とアドレス幅を超えるプレフィックス長は無効とする。
fn classify_range(entry: &str) -> Result<(IpAddr, u8), &'static str> {
    let (addr, prefix) = match entry.split_once('/') {
        Some((addr, prefix)) => {
            let Ok(prefix) = prefix.trim().parse::<u8>() else {
                return Err("prefix length is not a number");
            };
            (addr, Some(prefix))
        }
        None => (entry, None),
    };
    let Ok(net) = addr.trim().parse::<IpAddr>() else {
        return Err("address is not a valid IP");
    };
    let net = canonical_ip(net);
    let width = match net {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    match prefix {
        None => Ok((net, width)),
        Some(0) => Err("prefix length 0 would trust every address"),
        Some(prefix) if prefix > width => Err("prefix length exceeds address width"),
        Some(prefix) => Ok((net, prefix)),
    }
}

/// `range`（IP 単体または CIDR 表記）が `ip` を含むか。
/// 無効な指定はどのアドレスにも一致しない。
fn cidr_contains(range: &str, ip: IpAddr) -> bool {
    let Ok((net, prefix)) = classify_range(range) else {
        return false;
    };
    let ip = canonical_ip(ip);
    match (net, ip) {
        (IpAddr::V4(net), IpAddr::V4(ip)) => {
            let mask = ipv4_mask(prefix);
            (u32::from(net) & mask) == (u32::from(ip) & mask)
        }
        (IpAddr::V6(net), IpAddr::V6(ip)) => {
            let mask = ipv6_mask(prefix);
            (u128::from(net) & mask) == (u128::from(ip) & mask)
        }
        _ => false,
    }
}

fn ipv4_mask(prefix: u8) -> u32 {
    u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0)
}

fn ipv6_mask(prefix: u8) -> u128 {
    u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0)
}

/// IPv4-mapped IPv6（`::ffff:a.b.c.d`）を IPv4 として扱う。dual-stack で
/// bind した場合、IPv4 接続のピアはこの形式で届くため、正規化しないと
/// IPv4 拒否ゲート（SEC-01）が機能しない。
fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// 接続元 IP を決定する。`trust_forwarded` が true のときに限り
/// `X-Forwarded-For` の末尾値（信頼済みプロキシが追記・上書きした値）を
/// 採用し、解決できない場合は None を返して呼び出し側で拒否する
/// （fail-closed）。false のときはピアアドレスを使う。クライアントが
/// 注入できる先頭値は使用しない（NET-02）。
pub fn client_ip_from_request(
    headers: &HeaderMap,
    connect_ip: Option<IpAddr>,
    trust_forwarded: bool,
) -> Option<IpAddr> {
    if trust_forwarded {
        return last_forwarded_ip(headers).map(canonical_ip);
    }
    connect_ip.map(canonical_ip)
}

/// `X-Forwarded-For` の末尾エントリを IP として解釈する。
/// 欠落・形式不正の場合は None を返し、呼び出し側で拒否する。
fn last_forwarded_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .next_back()
        .and_then(|entry| entry.trim().parse::<IpAddr>().ok())
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
    use axum::http::HeaderValue;

    fn xff_headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", value.parse::<HeaderValue>().unwrap());
        headers
    }

    #[test]
    fn detects_ipv4() {
        assert!(is_ipv4("203.0.113.1".parse().unwrap()));
        assert!(!is_ipv4("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn public_ipv4_not_permitted_without_env() {
        assert!(!ipv4_permitted("203.0.113.1".parse().unwrap()));
    }

    /// NET-02: 信頼済みプロキシ以外からの `X-Forwarded-For` は判定に使わない
    #[test]
    fn forwarded_header_ignored_when_proxy_untrusted() {
        let connect = "203.0.113.9".parse::<IpAddr>().unwrap();
        assert_eq!(
            client_ip_from_request(&xff_headers("2001:db8::1"), Some(connect), false),
            Some(connect)
        );
    }

    /// 信頼済みプロキシが追記した末尾値のみを採用する（先頭の偽装値は無視）
    #[test]
    fn trusted_forwarded_header_uses_last_entry() {
        assert_eq!(
            client_ip_from_request(&xff_headers("192.0.2.10, 2001:db8::2"), None, true),
            Some("2001:db8::2".parse::<IpAddr>().unwrap())
        );
    }

    /// 信頼済みプロキシで `X-Forwarded-For` を解決できない場合は拒否（fail-closed）
    #[test]
    fn trusted_forwarded_header_unresolved_is_rejected() {
        let connect = "2001:db8::ff".parse::<IpAddr>().unwrap();
        assert_eq!(
            client_ip_from_request(&HeaderMap::new(), Some(connect), true),
            None
        );
        assert_eq!(
            client_ip_from_request(&xff_headers("not-an-ip"), Some(connect), true),
            None
        );
        assert_eq!(
            client_ip_from_request(&xff_headers("2001:db8::1, "), Some(connect), true),
            None
        );
    }

    /// dual-stack の IPv4 ピア（`::ffff:a.b.c.d`）は IPv4 として正規化する
    #[test]
    fn mapped_ipv4_peer_and_header_are_canonicalized() {
        let mapped: IpAddr = "::ffff:203.0.113.9".parse().unwrap();
        let v4: IpAddr = "203.0.113.9".parse().unwrap();
        assert_eq!(
            client_ip_from_request(&HeaderMap::new(), Some(mapped), false),
            Some(v4)
        );
        assert_eq!(
            client_ip_from_request(&xff_headers("::ffff:203.0.113.9"), None, true),
            Some(v4)
        );
    }

    /// 信頼判定・fail-closed・IPv4 ゲートの結線（環境非依存）
    #[test]
    fn evaluate_access_decisions() {
        let v6_peer: IpAddr = "2001:db8::9".parse().unwrap();
        let v4_peer: IpAddr = "203.0.113.9".parse().unwrap();

        // 非信頼ピア: XFF は無視し、ピアが IPv6 なら許可
        assert_eq!(
            evaluate_access(&xff_headers("203.0.113.1"), v6_peer, false),
            AccessDecision::Allowed(v6_peer)
        );
        // 非信頼ピア: パブリック IPv4 は拒否
        assert_eq!(
            evaluate_access(&HeaderMap::new(), v4_peer, false),
            AccessDecision::Ipv4Forbidden
        );
        // 信頼済みピア: XFF 解決不能は fail-closed
        assert_eq!(
            evaluate_access(&HeaderMap::new(), v6_peer, true),
            AccessDecision::UnverifiedClient
        );
        assert_eq!(
            evaluate_access(&xff_headers("not-an-ip"), v6_peer, true),
            AccessDecision::UnverifiedClient
        );
        // 信頼済みピア: 末尾の XFF 値で判定（mapped IPv4 は拒否、IPv6 は許可）
        assert_eq!(
            evaluate_access(&xff_headers("::ffff:203.0.113.1"), v6_peer, true),
            AccessDecision::Ipv4Forbidden
        );
        assert_eq!(
            evaluate_access(&xff_headers("2001:db8::5"), v6_peer, true),
            AccessDecision::Allowed("2001:db8::5".parse().unwrap())
        );
    }

    #[test]
    fn trusted_proxy_range_matching() {
        let ranges = parse_ranges(" 127.0.0.1, 10.0.0.0/8 , 2001:db8::/32, ,bogus/999 ");
        assert!(is_trusted_proxy("127.0.0.1".parse().unwrap(), &ranges));
        assert!(is_trusted_proxy("10.1.2.3".parse().unwrap(), &ranges));
        assert!(is_trusted_proxy(
            "2001:db8:ffff::1".parse().unwrap(),
            &ranges
        ));
        assert!(!is_trusted_proxy("203.0.113.1".parse().unwrap(), &ranges));
        assert!(!is_trusted_proxy("2001:db9::1".parse().unwrap(), &ranges));
        // IPv4-mapped は正規化して IPv4 範囲へ一致する
        assert!(is_trusted_proxy(
            "::ffff:10.0.0.1".parse().unwrap(),
            &ranges
        ));
        // 形式不正な要素はどのアドレスにも一致しない
        assert!(!is_trusted_proxy(
            "10.1.2.3".parse().unwrap(),
            &["bogus/999".to_string()]
        ));
    }

    #[test]
    fn cidr_edge_cases() {
        assert!(cidr_contains(
            "203.0.113.7/32",
            "203.0.113.7".parse().unwrap()
        ));
        assert!(!cidr_contains(
            "203.0.113.7/32",
            "203.0.113.8".parse().unwrap()
        ));
        assert!(cidr_contains("::1/128", "::1".parse().unwrap()));
        assert!(!cidr_contains("::1/128", "::2".parse().unwrap()));
        assert!(cidr_contains(
            "::ffff:10.0.0.0/8",
            "10.1.2.3".parse().unwrap()
        ));
        // 無効な指定は一致しない（全アドレス・幅超過・不正形式・ファミリ不一致）
        assert!(!cidr_contains("0.0.0.0/0", "203.0.113.1".parse().unwrap()));
        assert!(!cidr_contains("::/0", "2001:db8::1".parse().unwrap()));
        assert!(!cidr_contains("10.0.0.0/33", "10.0.0.1".parse().unwrap()));
        assert!(!cidr_contains(
            "2001:db8::/129",
            "2001:db8::1".parse().unwrap()
        ));
        assert!(!cidr_contains("bogus", "203.0.113.1".parse().unwrap()));
        assert!(!cidr_contains("10.0.0.0/8", "2001:db8::1".parse().unwrap()));
    }

    /// 全アドレス範囲と不正な要素は警告のうえ除外される
    #[test]
    fn invalid_ranges_are_filtered() {
        let entries = parse_ranges("10.0.0.0/8, 0.0.0.0/0, bogus, 127.0.0.1, ::/0");
        assert_eq!(validated_ranges(entries), vec!["10.0.0.0/8", "127.0.0.1"]);
    }
}
