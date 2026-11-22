use axum::{
    Json,
    extract::{ConnectInfo, Request},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
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

/// IPv4-mapped IPv6（`::ffff:a.b.c.d`）と IPv4-compatible IPv6（`::a.b.c.d`）
/// を IPv4 として扱う。dual-stack で bind した場合、IPv4 接続のピアは mapped
/// 形式で届く。compatible 形式を放置すると IPv4 拒否ゲート（SEC-01）を
/// `::192.0.2.1` のような表記ですり抜けられるため、mapped と同じく正規化する。
fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .or_else(|| ipv4_compatible(v6))
            .map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// IPv4-compatible IPv6（`::a.b.c.d`）を IPv4 として解釈する。
///
/// `::`（unspecified）と `::1`（loopback）は形式上は `::0.0.0.0` /
/// `::0.0.0.1` と読めるが、正規の IPv6 アドレスとして保持する。これらを
/// IPv4 化すると IPv6 ループバックの許可（既存運用）が壊れるためである。
fn ipv4_compatible(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    if v6 == Ipv6Addr::UNSPECIFIED || v6 == Ipv6Addr::LOCALHOST {
        return None;
    }
    let segments = v6.segments();
    // compatible 形式は上位 6 セグメントがすべて 0（mapped はセグメント 5 が 0xffff）。
    if segments[..6] != [0; 6] {
        return None;
    }
    let bits = (u32::from(segments[6]) << 16) | u32::from(segments[7]);
    Some(Ipv4Addr::from(bits))
}

/// 接続元 IP を決定する。`trust_forwarded` が true のときに限り
/// `X-Forwarded-For` を採用し、解決できない場合は None を返して呼び出し側で
/// 拒否する（fail-closed）。false のときはヘッダー全体を無視し、ピアアドレスを
/// 使う。クライアントが注入できる値は信頼済みプロキシ経由でも安易に使わない
/// （NET-02）。
pub fn client_ip_from_request(
    headers: &HeaderMap,
    connect_ip: Option<IpAddr>,
    trust_forwarded: bool,
) -> Option<IpAddr> {
    if trust_forwarded {
        return forwarded_client_ip(headers, connect_ip);
    }
    connect_ip.map(canonical_ip)
}

/// 信頼済みプロキシ経由のクライアント IP を解決する。
///
/// 2 つの XFF 運用を区別する（NET-02）。
///
/// - 追記型: 末尾が接続元ピア（信頼プロキシ自身）と一致する。末尾のピア
///   エントリを除いたチェーンの最左値を元のクライアントとして採用する。
///   単一ホップではこれは「末尾を除いた直前の値」と一致し、複数ホップでも
///   本来のクライアントを取り出せる。
/// - 上書き型: 末尾がピアと一致しない。従来どおり末尾を採用する。
///
/// カンマ区切り・前後の空白は正規化し、1 つでも IP として解釈できない
/// エントリがある場合は None（fail-closed）とする。**非 UTF-8 も None** とし、
/// 読み取れた値だけを残す fail-open を避ける（NET-02）。
fn forwarded_client_ip(headers: &HeaderMap, connect_ip: Option<IpAddr>) -> Option<IpAddr> {
    // 非UTF-8もNone: 1 つでも to_str() が失敗したら全体を None として拒否する。
    let values: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .map(|value| value.to_str().ok())
        .collect::<Option<Vec<_>>>()?;
    let mut entries: Vec<IpAddr> = values
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(|entry| entry.trim().parse::<IpAddr>().ok().map(canonical_ip))
        .collect::<Option<Vec<_>>>()?;
    let peer = connect_ip.map(canonical_ip);
    let is_peer = |ip: IpAddr| peer.is_some_and(|peer| peer == ip);

    if entries.last().copied().is_some_and(is_peer) {
        // 追記型: 末尾のピア（複数ホップで連続し得る）をすべて除き、
        // 残ったチェーンの最左値＝元のクライアントを採用する。
        while entries.last().copied().is_some_and(is_peer) {
            entries.pop();
        }
        return entries.first().copied();
    }

    // 上書き型: 従来どおり末尾を採用。
    entries.last().copied()
}

pub fn is_ipv4(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(_))
}

/// `ALLOW_IPV4_LOCALHOST=1` のとき 127.0.0.0/8 のみ IPv4 を許可（ローカル開発用）。
///
/// この関数はリクエストごとに呼ばれるため、不正値をここで起動拒否できない。
/// 受理値は共通 `bool_env_state`（`1`/`true` と `0`/`false` のみ）へ統一し、
/// `Valid(true)` のときだけ IPv4 を許可する。未設定・受理値外・非 UTF-8 は
/// いずれも `false`（fail-closed）として IPv4 を一切許可しない。
/// 不正値は `run()` が起動時に警告し、`doctor` も `ipv4_localhost` カテゴリで警告する
/// （「起動時警告＋per-request fail-closed」の二段構え）。
pub fn ipv4_permitted(ip: IpAddr) -> bool {
    if matches!(
        common::env::bool_env_state(common::env::read_env("ALLOW_IPV4_LOCALHOST")),
        common::env::BoolEnvState::Valid(true)
    ) && let IpAddr::V4(v4) = ip
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

    /// 非 UTF-8 の生バイト（0xff）を含む XFF ヘッダを作る（fail-closed 検証用）。
    fn xff_raw_headers(values: &[&[u8]]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(
                "x-forwarded-for",
                HeaderValue::from_bytes(value).expect("raw header value"),
            );
        }
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

    /// NET-02: 追記型 XFF では末尾のピア（信頼プロキシ自身）を除き、
    /// 元のクライアントを採用する。`client, proxy1, peer` なら client。
    #[test]
    fn trusted_forwarded_header_drops_trailing_peer_and_uses_original_client() {
        let peer: IpAddr = "2001:db8::1".parse().unwrap();
        let client: IpAddr = "2001:db8::a".parse().unwrap();
        let proxy1: IpAddr = "2001:db8::b".parse().unwrap();

        // 3 ホップ: 末尾がピアなので、その直前だけでなく元のクライアントを採用。
        let headers = xff_headers(&format!("{client}, {proxy1}, {peer}"));
        assert_eq!(
            client_ip_from_request(&headers, Some(peer), true),
            Some(client)
        );

        // 単一ホップ: 「末尾を除いた直前の値」＝元のクライアント。
        let headers = xff_headers(&format!("{client}, {peer}"));
        assert_eq!(
            client_ip_from_request(&headers, Some(peer), true),
            Some(client)
        );

        // ピアが末尾に連続して追記されても元のクライアントを採用。
        let headers = xff_headers(&format!("{client}, {peer}, {peer}"));
        assert_eq!(
            client_ip_from_request(&headers, Some(peer), true),
            Some(client)
        );

        // 空白・IPv4/IPv6 混在・mapped ピアにも対応する。
        let peer: IpAddr = "::ffff:203.0.113.1".parse().unwrap();
        let client: IpAddr = "2001:db8::c".parse().unwrap();
        let headers = xff_headers(&format!("  {client} , 203.0.113.9 , 203.0.113.1 "));
        assert_eq!(
            client_ip_from_request(&headers, Some(peer), true),
            Some(client)
        );
    }

    /// NET-02: 上書き型 XFF（末尾がピアでない）では従来どおり末尾を採用する。
    #[test]
    fn trusted_forwarded_header_overwrite_style_uses_last_entry() {
        let peer: IpAddr = "2001:db8::1".parse().unwrap();
        let last: IpAddr = "2001:db8::2".parse().unwrap();
        let headers = xff_headers(&format!("203.0.113.5, {last}"));
        assert_eq!(
            client_ip_from_request(&headers, Some(peer), true),
            Some(last)
        );

        // 追記型と誤判定しないよう、途中にピアが現れても末尾を採用する。
        let headers = xff_headers(&format!("203.0.113.5, {peer}, {last}"));
        assert_eq!(
            client_ip_from_request(&headers, Some(peer), true),
            Some(last)
        );
    }

    /// NET-02: 信頼プロキシ未設定時は複数値の XFF 全体が無視される。
    #[test]
    fn forwarded_header_ignored_when_proxy_untrusted_multi_value() {
        let peer: IpAddr = "2001:db8::9".parse().unwrap();
        let headers = xff_headers("203.0.113.1, 2001:db8::2, 2001:db8::1");
        assert_eq!(
            client_ip_from_request(&headers, Some(peer), false),
            Some(peer)
        );
    }

    /// 不正なエントリが混ざる XFF は fail-closed とする。
    #[test]
    fn trusted_forwarded_header_with_invalid_entry_is_rejected() {
        let peer: IpAddr = "2001:db8::1".parse().unwrap();
        for value in [
            "2001:db8::a, not-an-ip, 2001:db8::1",
            "2001:db8::a,,2001:db8::1",
            "2001:db8::a, 2001:db8::1, not-an-ip",
        ] {
            assert_eq!(
                client_ip_from_request(&xff_headers(value), Some(peer), true),
                None,
                "{value} must be rejected"
            );
        }

        // 非 UTF-8 が混在すると、有効な値があっても解決しない（非UTF-8もNone）。
        let mixed = xff_raw_headers(&[b"2001:db8::a", &[0xff]]);
        assert_eq!(
            client_ip_from_request(&mixed, Some(peer), true),
            None,
            "non-UTF-8 value mixed with a valid value must be rejected"
        );

        // 非 UTF-8 のみでも解決しない。
        let non_utf8_only = xff_raw_headers(&[&[0xff]]);
        assert_eq!(
            client_ip_from_request(&non_utf8_only, Some(peer), true),
            None,
            "non-UTF-8-only value must be rejected"
        );

        // 有効な値のみなら解決できる（fail-open への回帰防止）。
        let valid_only = xff_raw_headers(&[b"2001:db8::a"]);
        assert_eq!(
            client_ip_from_request(&valid_only, Some(peer), true),
            Some("2001:db8::a".parse::<IpAddr>().unwrap()),
            "valid UTF-8 value must still resolve"
        );
    }

    /// NET-02: 信頼済みプロキシで `X-Forwarded-For` を解決できない場合は拒否（fail-closed）
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

    /// SEC-01: IPv4-compatible IPv6（`::a.b.c.d`）は IPv4 として正規化し、
    /// mapped 形式と同じく拒否する。`::` / `::1` は IPv6 のまま保持する。
    #[test]
    fn ipv4_compatible_form_is_canonicalized_to_ipv4() {
        let compatible: IpAddr = "::192.0.2.1".parse().unwrap();
        let v4: IpAddr = "192.0.2.1".parse().unwrap();
        assert_eq!(canonical_ip(compatible), v4);
        assert_eq!(
            client_ip_from_request(&HeaderMap::new(), Some(compatible), false),
            Some(v4)
        );
        assert_eq!(
            client_ip_from_request(&xff_headers("::192.0.2.1"), None, true),
            Some(v4)
        );

        // 正規の IPv6 アドレス（unspecified / loopback / 通常のグローバル）は維持。
        for original in ["::", "::1", "2001:db8::1"] {
            let ip: IpAddr = original.parse().unwrap();
            assert_eq!(canonical_ip(ip), ip, "{original} must stay IPv6");
        }
    }

    /// SEC-01: IPv4-compatible 形式のクライアントは IPv4 ゲートで拒否される。
    #[test]
    fn evaluate_access_rejects_ipv4_compatible_form() {
        let v6_peer: IpAddr = "2001:db8::9".parse().unwrap();
        assert_eq!(
            evaluate_access(&HeaderMap::new(), "::192.0.2.1".parse().unwrap(), false),
            AccessDecision::Ipv4Forbidden
        );
        assert_eq!(
            evaluate_access(&xff_headers("::192.0.2.1"), v6_peer, true),
            AccessDecision::Ipv4Forbidden
        );
        // 上書き型 XFF の末尾が compatible 形式でも同样的に拒否する。
        assert_eq!(
            evaluate_access(&xff_headers("2001:db8::a, ::192.0.2.1"), v6_peer, true),
            AccessDecision::Ipv4Forbidden
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

    /// CIDR 検証を `OnceLock` 非依存の純粋関数として直接検証する（NET-02）。
    /// IPv4/IPv6 の正規形は受理し、全アドレス範囲・幅超過・不正形式は拒否する。
    #[test]
    fn classify_range_accepts_canonical_forms_and_rejects_unsafe_prefixes() {
        // 前後の空白は trim され、IPv4 / IPv6 ともに正規形で受理される。
        assert_eq!(
            classify_range(" 203.0.113.7/24 "),
            Ok(("203.0.113.7".parse::<IpAddr>().unwrap(), 24))
        );
        assert_eq!(
            classify_range("2001:0db8:0000::/32"),
            Ok(("2001:db8::".parse::<IpAddr>().unwrap(), 32))
        );
        // プレフィックス省略（IP 単体）はアドレス幅いっぱいになる。
        assert_eq!(
            classify_range("127.0.0.1"),
            Ok(("127.0.0.1".parse::<IpAddr>().unwrap(), 32))
        );
        assert_eq!(
            classify_range("::1"),
            Ok(("::1".parse::<IpAddr>().unwrap(), 128))
        );
        // IPv4-mapped IPv6 は IPv4 へ正規化され、幅も 32 に切り替わる。
        assert_eq!(
            classify_range("::ffff:10.1.2.3/8"),
            Ok(("10.1.2.3".parse::<IpAddr>().unwrap(), 8))
        );

        // fail-closed: 全アドレス範囲・幅超過・不正形式はすべて Err。
        for invalid in [
            "0.0.0.0/0",
            "::/0",
            "10.0.0.0/33",
            "2001:db8::/129",
            "10.0.0.0/x",
            "not-an-ip/8",
        ] {
            assert!(
                classify_range(invalid).is_err(),
                "{invalid} must be rejected as an unsafe CIDR"
            );
        }
    }

    /// `validated_ranges` が正規形の安全な CIDR だけを通すことを確認する。
    #[test]
    fn validated_ranges_keeps_only_safe_entries() {
        let entries = parse_ranges(
            " 203.0.113.0/24 , ::ffff:10.0.0.0/8 , 0.0.0.0/0 , ::/0 , bogus , 10.0.0.0/33 , 2001:db8::/32 ",
        );
        assert_eq!(
            validated_ranges(entries),
            vec!["203.0.113.0/24", "::ffff:10.0.0.0/8", "2001:db8::/32"]
        );
    }
}
