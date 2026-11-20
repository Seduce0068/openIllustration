//! 設定検証 CLI。本番起動前に設定を診断する（R5-06 / R4-01 の事前検証）。
//! 既定は警告のみ。`--strict` を付けたときのみ問題があれば終了コード 1 とする。
//! 秘密情報の値は一切出力しない。

use server::{
    CLOCK_SKEW_MAX_SECS, CLOCK_SKEW_MIN_SECS, DEFAULT_CLOCK_SKEW_SECS, allowed_origins,
    graceful_drain_timeout, ipfs_gateways, ipfs_max_attempts, ipfs_timeout, is_valid_oidc_endpoint,
    jwt_keys_from_env, max_concurrent_requests, moderation_finalize_interval, oidc_http_timeout,
    production_config_errors, rate_limit_per_min, validate_jwt_secret,
};

fn print_help() {
    eprintln!(
        "Usage: doctor [--strict]\n\
         \n\
         Validates runtime configuration without exposing secret values.\n\
         By default problems are reported as warnings (exit 0).\n\
         With --strict, any problem causes exit code 1."
    );
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// 引数解析の結果。副作用（ヘルプ表示・exit）は含めず、main 側で実行する。
#[derive(Debug, PartialEq, Eq)]
enum ParseOutcome {
    /// チェックを実行する。`strict` が真なら問題検出時に終了コード 1。
    Run { strict: bool },
    /// ヘルプを表示して終了コード 0。
    Help,
    /// 不明引数。ヘルプを表示して終了コード 1。
    UnknownArgument(String),
    /// 引数が多すぎる。ヘルプを表示して終了コード 1。
    TooManyArguments,
}

/// 引数を解析する純粋関数。先頭要素はプログラム名として読み飛ばす。
fn parse_args_from<I>(items: I) -> ParseOutcome
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = items.into_iter().skip(1).collect();
    // ヘルプは他引数より優先する。他 CLI と同じく `--strict --help` の併用を受理するため、
    // 不明引数や引数過多の判定より先にヘルプ要求を検出する（エラー種別は従来どおり）。
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return ParseOutcome::Help;
    }
    match args.as_slice() {
        [] => ParseOutcome::Run { strict: false },
        [arg] if arg == "--strict" => ParseOutcome::Run { strict: true },
        [other] => ParseOutcome::UnknownArgument(other.clone()),
        _ => ParseOutcome::TooManyArguments,
    }
}

/// strict モードで問題が 1 件以上なら失敗（終了コード 1）を返す純粋関数。
fn should_fail(strict: bool, problems: usize) -> bool {
    strict && problems > 0
}

/// 検査結果を (項目, 問題) の形で集める。
/// 環境変数は `lookup` 経由でのみ読み、テストではプロセス環境を汚さず差し替えられる。
/// 秘密情報の値は結果に含めない。
fn check_all_with<F>(lookup: F) -> Vec<(&'static str, Vec<String>)>
where
    F: Fn(&str) -> Option<String>,
{
    let mut results = Vec::new();

    let allow_mock_auth = lookup("ALLOW_MOCK_AUTH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    // 本番必須設定は既存の fail-closed 判定へ委譲する（再実装しない）。
    // OIDC 必須値は Some でも trim 後空文字なら「未設定」と同じく警告対象にする。
    // 値は判定にだけ使い、結果へは含めない（秘密情報の非エコー）。
    let oidc_value_present = |name: &str| {
        lookup(name)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    };
    let production = production_config_errors(
        allow_mock_auth,
        oidc_value_present("OIDC_ISSUER")
            && oidc_value_present("OIDC_CLIENT_ID")
            && oidc_value_present("OIDC_CLIENT_SECRET")
            && oidc_value_present("OIDC_REDIRECT_URI")
            && oidc_value_present("OIDC_AUTHORIZATION_ENDPOINT")
            && oidc_value_present("OIDC_TOKEN_ENDPOINT")
            && oidc_value_present("OIDC_JWKS_URI"),
        lookup("PINATA_JWT").as_deref(),
    );
    results.push((
        "production",
        production.into_iter().map(str::to_string).collect(),
    ));

    // モック認証と本番相当の設定の併用は危険（モック認証で本番データが触れる）。
    // ALLOW_MOCK_AUTH が真で実質的な本番設定が一つでもあれば警告する。秘密値は出力しない。
    const OIDC_REQUIRED: [&str; 7] = [
        "OIDC_ISSUER",
        "OIDC_CLIENT_ID",
        "OIDC_CLIENT_SECRET",
        "OIDC_REDIRECT_URI",
        "OIDC_AUTHORIZATION_ENDPOINT",
        "OIDC_TOKEN_ENDPOINT",
        "OIDC_JWKS_URI",
    ];
    // URL 検証の対象は 7 変数のうちエンドポイントを指す 5 つ（クライアントID/秘密鍵は除く）。
    const OIDC_ENDPOINT_KEYS: [&str; 5] = [
        "OIDC_ISSUER",
        "OIDC_REDIRECT_URI",
        "OIDC_AUTHORIZATION_ENDPOINT",
        "OIDC_TOKEN_ENDPOINT",
        "OIDC_JWKS_URI",
    ];
    let oidc_any = OIDC_REQUIRED.iter().any(|name| lookup(name).is_some());
    let pinata_production = lookup("PINATA_JWT")
        .map(|v| {
            let trimmed = v.trim();
            !trimmed.is_empty() && trimmed != "mock_jwt"
        })
        .unwrap_or(false);
    let mut mock_auth_errors = Vec::new();
    if allow_mock_auth
        && (pinata_production || oidc_any || lookup("JWT_RSA_PRIVATE_KEY_PEM").is_some())
    {
        mock_auth_errors.push(
            "ALLOW_MOCK_AUTH is enabled together with production-like settings; do not deploy this configuration".to_string(),
        );
    }
    results.push(("mock_auth_with_production_config", mock_auth_errors));

    // モック認証が有効なこと自体は開発環境では正しいが、本番設定としては不適切。
    // production_config_errors は allow_mock_auth で早期 return するため、
    // ALLOW_MOCK_AUTH=1 単独だと本番チェックが全スキップされてしまう。
    // ここで明示的に問題として計上し、--strict では exit 1 とする（既定は警告のみ）。
    let mock_auth_enabled_errors = if allow_mock_auth {
        vec![
            "ALLOW_MOCK_AUTH is enabled; acceptable for development but inappropriate for production"
                .to_string(),
        ]
    } else {
        Vec::new()
    };
    results.push(("mock_auth_enabled", mock_auth_enabled_errors));

    // JWT: HS256 は 32 バイト以上、RS256 は PEM と検証キーの整合を検査する。
    let jwt_pem = lookup("JWT_RSA_PRIVATE_KEY_PEM");
    let jwt_secret = lookup("JWT_SECRET");
    let jwt_errors = match jwt_keys_from_env(
        jwt_pem,
        jwt_secret,
        lookup("JWT_RSA_VERIFICATION_KEYS").as_deref(),
        lookup("JWT_REQUIRE_KID")
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(false),
    ) {
        Ok(_) => Vec::new(),
        Err(error) => vec![error],
    };
    results.push(("jwt", jwt_errors));

    let mut secret_errors = Vec::new();
    if let Some(secret) = lookup("JWT_SECRET")
        && let Err(error) = validate_jwt_secret(&secret)
    {
        secret_errors.push(error);
    }
    results.push(("jwt_secret", secret_errors));

    // IPFS: ゲートウェイは {cid} を含む http(s) URL テンプレートが必要。
    // ipfs_gateways() は不正値を除外して有効値だけ返すため、設定した非空エントリ数
    // との差を問題として計上する（値は出力しない）。未設定時は既定ゲートウェイを
    // 使うため加算しない。
    let gateways_env = lookup("IPFS_GATEWAYS");
    let gateways = ipfs_gateways(gateways_env.as_deref());
    let mut ipfs_errors = Vec::new();
    if let Some(value) = &gateways_env {
        let configured = value
            .split(',')
            .filter(|entry| !entry.trim().is_empty())
            .count();
        let invalid = configured.saturating_sub(gateways.len());
        if invalid > 0 {
            ipfs_errors.push(format!(
                "IPFS_GATEWAYS contains {invalid} invalid entry(ies); using only the valid ones"
            ));
        }
    }
    if gateways.is_empty() {
        ipfs_errors.push("no valid IPFS gateway configured".to_string());
    }
    results.push(("ipfs_gateways", ipfs_errors));

    // 各パーサは不正値・範囲外を既定へフォールバックするため、
    // 「正規化後の値が 0」といった判定は決して発火しない（死コード）。
    // rate_limit と同じ方式で「生のパース値」と「正規化後の値」を比較し、
    // フォールバックが発生した場合のみ警告する。設定値自体は出力しない。
    let mut timeout_errors = Vec::new();
    if let Some(value) = lookup("IPFS_TIMEOUT_SECS")
        && ipfs_timeout(Some(&value)).as_secs() != value.parse::<u64>().unwrap_or(0)
    {
        timeout_errors
            .push("IPFS_TIMEOUT_SECS is invalid or outside 1..=300; using default".to_string());
    }
    results.push(("ipfs_timeout", timeout_errors));

    let mut attempts_errors = Vec::new();
    if let Some(value) = lookup("IPFS_MAX_ATTEMPTS")
        && ipfs_max_attempts(Some(&value)) != value.parse::<usize>().unwrap_or(0)
    {
        attempts_errors
            .push("IPFS_MAX_ATTEMPTS is invalid or outside 1..=5; using default".to_string());
    }
    results.push(("ipfs_max_attempts", attempts_errors));

    let mut oidc_timeout_errors = Vec::new();
    if let Some(value) = lookup("OIDC_HTTP_TIMEOUT_SECS")
        && oidc_http_timeout(Some(&value)).as_secs() != value.parse::<u64>().unwrap_or(0)
    {
        oidc_timeout_errors
            .push("OIDC_HTTP_TIMEOUT_SECS is invalid or outside 1..=60; using default".to_string());
    }
    results.push(("oidc_http_timeout", oidc_timeout_errors));

    // GRACEFUL_DRAIN_TIMEOUT_SECS: lib.rs::graceful_drain_timeout と同一範囲
    // （1..=300・既定30秒）。不正値・範囲外は既定へフォールバックするため、
    // 「生のパース値」と「正規化後の値」を比較して警告する。値は出力しない。
    const DRAIN_WARNING: &str =
        "GRACEFUL_DRAIN_TIMEOUT_SECS is invalid or outside 1..=300; using default";
    let mut drain_timeout_errors = Vec::new();
    if let Some(value) = lookup("GRACEFUL_DRAIN_TIMEOUT_SECS")
        && graceful_drain_timeout(Some(&value)).as_secs() != value.parse::<u64>().unwrap_or(0)
    {
        drain_timeout_errors.push(DRAIN_WARNING.to_string());
    }
    results.push(("graceful_drain_timeout", drain_timeout_errors));

    // MODERATION_FINALIZE_INTERVAL_SECS: lib.rs::moderation_finalize_interval と
    // 同一範囲（30..=3600・既定300秒）。上記と同じ方式でフォールバックを検出し、
    // 設定値は出力しない。
    const FINALIZE_WARNING: &str =
        "MODERATION_FINALIZE_INTERVAL_SECS is invalid or outside 30..=3600; using default";
    let mut finalize_interval_errors = Vec::new();
    if let Some(value) = lookup("MODERATION_FINALIZE_INTERVAL_SECS")
        && moderation_finalize_interval(Some(&value)).as_secs() != value.parse::<u64>().unwrap_or(0)
    {
        finalize_interval_errors.push(FINALIZE_WARNING.to_string());
    }
    results.push(("moderation_finalize_interval", finalize_interval_errors));

    // OIDC_CLOCK_SKEW_SECS: 範囲外は既定 60 へフォールバックするため警告する。
    // 値は出力せず、範囲内かどうかのみを診断する。
    let oidc_clock_skew_errors = match lookup("OIDC_CLOCK_SKEW_SECS") {
        None => Vec::new(),
        Some(value) => match value.parse::<u64>() {
            Ok(seconds) if (CLOCK_SKEW_MIN_SECS..=CLOCK_SKEW_MAX_SECS).contains(&seconds) => {
                Vec::new()
            }
            _ => vec![format!(
                "OIDC_CLOCK_SKEW_SECS is outside {CLOCK_SKEW_MIN_SECS}..={CLOCK_SKEW_MAX_SECS}; using default {DEFAULT_CLOCK_SKEW_SECS} seconds"
            )],
        },
    };
    results.push(("oidc_clock_skew", oidc_clock_skew_errors));

    // OIDC エンドポイント URL: https 必須。ALLOW_MOCK_AUTH 有効時だけ
    // localhost 系の http を許容する（from_env_with と同じ判定を共有する）。
    // OIDC 7 変数が揃っているときに 5 エンドポイントを検証し、1 件でも不正なら
    // 固定文言で警告する。設定値は一切エコーしない。
    const OIDC_ENDPOINT_URL_WARNING: &str = "OIDC endpoints must be https (http is allowed only for localhost when ALLOW_MOCK_AUTH is enabled)";
    let oidc_endpoint_urls_errors = if OIDC_REQUIRED.iter().all(|name| lookup(name).is_some())
        && OIDC_ENDPOINT_KEYS
            .iter()
            .filter_map(|name| lookup(name))
            .any(|value| !is_valid_oidc_endpoint(&value, allow_mock_auth))
    {
        vec![OIDC_ENDPOINT_URL_WARNING.to_string()]
    } else {
        Vec::new()
    };
    results.push(("oidc_endpoint_urls", oidc_endpoint_urls_errors));

    // OIDC_MAX_AUTH_AGE_SECS: server::oidc::OidcState::from_env_with は
    // 0 / 負 / 非数値 / オーバーフローを Err（起動拒否）にして max_age 検証の
    // 黙った無効化を防ぐ。from_env_with は pub(crate) で doctor から直接呼べない
    // ため、ここで同一のパース検証を行い Err 相当を警告化する。値は出力しない。
    // from_env_with は必須 7 変数が揃っていれば mock 有効時も max_auth_age を検証するため、
    // doctor も 7 変数が揃っている場合は mock 有効でも警告する。
    // 7 変数未揃いなら from_env_with が Ok(None) を返して max_auth_age を見ないため、
    // mock 有効時は警告しない（本番相当では従来どおり警告する）。
    let oidc_max_auth_age_errors =
        if allow_mock_auth && !OIDC_REQUIRED.iter().all(|name| lookup(name).is_some()) {
            Vec::new()
        } else {
            match lookup("OIDC_MAX_AUTH_AGE_SECS") {
                None => Vec::new(),
                Some(value) => match value.parse::<u64>() {
                    Ok(0) => vec!["OIDC_MAX_AUTH_AGE_SECS must be greater than 0".to_string()],
                    Ok(_) => Vec::new(),
                    // 負数・非数値・オーバーフローはすべて Err 相当（起動拒否）。
                    Err(_) => vec!["OIDC_MAX_AUTH_AGE_SECS must be a positive integer".to_string()],
                },
            }
        };
    results.push(("oidc_max_auth_age", oidc_max_auth_age_errors));

    // OIDC_COOKIE_SECURE: 既定は Secure 付与（fail-closed）。mock 無効構成で
    // 偽値（0 / false）が設定されているなら警告する。値はエコーしない。
    let oidc_cookie_secure_errors = match lookup("OIDC_COOKIE_SECURE") {
        Some(value) => {
            let falsy = value == "0" || value.eq_ignore_ascii_case("false");
            if !allow_mock_auth && falsy {
                vec![
                    "OIDC_COOKIE_SECURE is disabled; session, refresh, and OIDC state cookies will lack the Secure attribute"
                        .to_string(),
                ]
            } else {
                Vec::new()
            }
        }
        None => Vec::new(),
    };
    results.push(("oidc_cookie_secure", oidc_cookie_secure_errors));

    // OIDC_REQUIRED_ACR / OIDC_REQUIRED_AMR: mock 無効時に未設定（または空要素
    // のみ）だと保証レベル（認証コンテキスト／認証方法）が一切強制されないため
    // 警告する。値は出力しない。
    let mut oidc_required_assurance_errors = Vec::new();
    if !allow_mock_auth {
        for (name, warning) in [
            (
                "OIDC_REQUIRED_ACR",
                "OIDC_REQUIRED_ACR is not set; no required authentication context class is enforced",
            ),
            (
                "OIDC_REQUIRED_AMR",
                "OIDC_REQUIRED_AMR is not set; no required authentication method is enforced",
            ),
        ] {
            let effective = lookup(name)
                .map(|value| value.split(',').any(|entry| !entry.trim().is_empty()))
                .unwrap_or(false);
            if !effective {
                oidc_required_assurance_errors.push(warning.to_string());
            }
        }
    }
    results.push(("oidc_required_assurance", oidc_required_assurance_errors));

    // 年齢ポリシー: 不正値は AgePolicy::from_env_with がエラーを返す。
    // プロセス環境を触らず lookup を渡すことで、CLI と同じ分岐をテストできる。
    let age_errors = match common::age::AgePolicy::from_env_with(&lookup) {
        Ok(_) => Vec::new(),
        Err(error) => vec![error],
    };
    results.push(("age_policy", age_errors));

    // レート制限: 既定値へフォールバックしていても警告する。
    let mut rate_errors = Vec::new();
    if let Some(value) = lookup("RATE_LIMIT_PER_MIN")
        && rate_limit_per_min(Some(&value)) != value.parse::<u32>().unwrap_or(0)
    {
        rate_errors.push("RATE_LIMIT_PER_MIN is outside 1..=600; using default".to_string());
    }
    if let Some(value) = lookup("MAX_CONCURRENT_REQUESTS")
        && max_concurrent_requests(Some(&value)) != value.parse::<usize>().unwrap_or(0)
    {
        rate_errors.push("MAX_CONCURRENT_REQUESTS is outside 1..=256; using default".to_string());
    }
    results.push(("rate_limit", rate_errors));

    // IPv6-only が既定（SEC-01）。ALLOW_IPV4_LOCALHOST が真なら IPv4 loopback を
    // 許可するため本番では不適切。値（1/true）は出力せず有無のみ警告する。
    let allow_ipv4_localhost = lookup("ALLOW_IPV4_LOCALHOST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let ipv4_localhost_errors = if allow_ipv4_localhost {
        vec![
            "ALLOW_IPV4_LOCALHOST is enabled; production should stay IPv6-only (SEC-01)"
                .to_string(),
        ]
    } else {
        Vec::new()
    };
    results.push(("ipv4_localhost", ipv4_localhost_errors));

    // TRUSTED_PROXY_CIDRS は X-Forwarded-For（XFF）を信頼する設定。設定自体は
    // 正当だが本番では意図確認が必要。CIDR 等の設定値は出力せず有無のみ警告する。
    let trusted_proxy_errors = if lookup("TRUSTED_PROXY_CIDRS").is_some() {
        vec![
            "TRUSTED_PROXY_CIDRS is configured; X-Forwarded-For (XFF) is trusted, confirm this is intentional in production"
                .to_string(),
        ]
    } else {
        Vec::new()
    };
    results.push(("trusted_proxy", trusted_proxy_errors));

    // EXPOSE_INTERNAL_ERRORS: 有効時（1 / true, 大小文字不問）は内部エラーの詳細が
    // クライアントへ露出するため警告する。mock 有効時は開発環境として許容し、
    // 値（1/true）はエコーしない。
    let expose_internal_errors_errors = match lookup("EXPOSE_INTERNAL_ERRORS") {
        Some(value) => {
            let enabled = value == "1" || value.eq_ignore_ascii_case("true");
            if !allow_mock_auth && enabled {
                vec![
                    "EXPOSE_INTERNAL_ERRORS is enabled; internal error details are exposed to clients"
                        .to_string(),
                ]
            } else {
                Vec::new()
            }
        }
        None => Vec::new(),
    };
    results.push(("expose_internal_errors", expose_internal_errors_errors));

    // ALLOWED_ORIGINS: mock 無効時に実パーサ allowed_origins() が空ベクタなら
    // すべての CORS オリジンが拒否される（fail-closed）。scheme なし等の不正要素は
    // パーサが捨てるため「非空」判定では見逃していた。オリジン値は出力しない。
    let allowed_origins_errors = if !allow_mock_auth {
        let no_valid_origin = lookup("ALLOWED_ORIGINS")
            .map(|value| allowed_origins(Some(&value), false).is_empty())
            .unwrap_or(true);
        if no_valid_origin {
            vec![
                "ALLOWED_ORIGINS has no valid http(s)/null origin (unset, empty, or all entries invalid) while mock auth is disabled; all CORS origins are rejected"
                    .to_string(),
            ]
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    results.push(("allowed_origins", allowed_origins_errors));

    // AUDIT_EXPECT_ROOT: 本番相当（mock 無効）では別経路保存の Merkle root（hex）が必須。
    // 未設定・空白のみ・前後空白付き・不正 hex なら固定文言で警告する。値はエコーしない。
    // backup-rotate.sh は未設定でも内部整合性チェックのみで動作する既存契約を維持し、
    // doctor は既定で警告のみ（`--strict` のときだけ失敗）。共有の
    // production_config_errors は起動判定へ影響するため変更しない。
    const AUDIT_EXPECT_ROOT_WARNING: &str =
        "AUDIT_EXPECT_ROOT is required in production (a hex Merkle root)";
    let audit_expect_root_errors = if allow_mock_auth {
        Vec::new()
    } else {
        // backup-rotate.sh と同じく生値へ [0-9a-fA-F]+ を適用する（trim しない）。
        let valid = lookup("AUDIT_EXPECT_ROOT")
            .map(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .unwrap_or(false);
        if valid {
            Vec::new()
        } else {
            vec![AUDIT_EXPECT_ROOT_WARNING.to_string()]
        }
    };
    results.push(("audit_expect_root", audit_expect_root_errors));

    results
}

/// 本番用の薄いラッパー。実環境の読み取りはここだけに閉じ込める。
fn check_all() -> Vec<(&'static str, Vec<String>)> {
    check_all_with(env)
}

fn main() {
    let strict = match parse_args_from(std::env::args()) {
        ParseOutcome::Run { strict } => strict,
        ParseOutcome::Help => {
            print_help();
            return;
        }
        ParseOutcome::UnknownArgument(other) => {
            eprintln!("unknown argument: {other}");
            print_help();
            std::process::exit(1);
        }
        ParseOutcome::TooManyArguments => {
            eprintln!("too many arguments");
            print_help();
            std::process::exit(1);
        }
    };

    let results = check_all();
    let mut problems = 0usize;
    for (name, errors) in &results {
        if errors.is_empty() {
            println!("OK   {name}");
        } else {
            problems += errors.len();
            for error in errors {
                eprintln!("WARN {name}: {error}");
            }
        }
    }
    println!(
        "doctor: {} check(s), {} problem(s)",
        results.len(),
        problems
    );
    if should_fail(strict, problems) {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- テストヘルパ ------------------------------------------------------

    /// プログラム名を含む引数ベクタを組み立てる（`std::env::args()` 相当）。
    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("doctor")
            .chain(args.iter().copied())
            .map(String::from)
            .collect()
    }

    /// (名前, 値) の組から環境変数ルックアップを生成する。
    fn lookup_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    /// 指定カテゴリの問題リストを返す（カテゴリ欠落はテスト失敗扱い）。
    fn errors_for<'a>(results: &'a [(&'static str, Vec<String>)], name: &str) -> &'a [String] {
        results
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, errors)| errors.as_slice())
            .unwrap_or_else(|| panic!("missing check category: {name}"))
    }

    fn problem_count(results: &[(&'static str, Vec<String>)]) -> usize {
        results.iter().map(|(_, errors)| errors.len()).sum()
    }

    // --- 1. 引数パース -----------------------------------------------------

    #[test]
    fn parse_no_arguments_runs_non_strict() {
        assert_eq!(
            parse_args_from(argv(&[])),
            ParseOutcome::Run { strict: false }
        );
    }

    #[test]
    fn parse_strict_flag_runs_strict() {
        assert_eq!(
            parse_args_from(argv(&["--strict"])),
            ParseOutcome::Run { strict: true }
        );
    }

    #[test]
    fn parse_help_flags_request_help() {
        assert_eq!(parse_args_from(argv(&["--help"])), ParseOutcome::Help);
        assert_eq!(parse_args_from(argv(&["-h"])), ParseOutcome::Help);
    }

    #[test]
    fn parse_unknown_argument_is_reported() {
        assert_eq!(
            parse_args_from(argv(&["--bogus"])),
            ParseOutcome::UnknownArgument("--bogus".to_string())
        );
    }

    #[test]
    fn parse_multiple_arguments_is_rejected() {
        // ヘルプを含む併用は Help となるため、ここではヘルプ無しの多引数を検証する。
        assert_eq!(
            parse_args_from(argv(&["--strict", "--unknown"])),
            ParseOutcome::TooManyArguments
        );
        assert_eq!(
            parse_args_from(argv(&["--strict", "--strict"])),
            ParseOutcome::TooManyArguments
        );
    }

    #[test]
    fn parse_strict_with_help_requests_help() {
        // 他 CLI と整合: --strict と --help の併用はエラーではなくヘルプ表示。
        assert_eq!(
            parse_args_from(argv(&["--strict", "--help"])),
            ParseOutcome::Help
        );
        assert_eq!(
            parse_args_from(argv(&["--strict", "-h"])),
            ParseOutcome::Help
        );
    }

    #[test]
    fn parse_help_with_strict_requests_help() {
        // 順序を逆にしてもヘルプが優先される（引数3つ以上も同様）。
        assert_eq!(
            parse_args_from(argv(&["--help", "--strict"])),
            ParseOutcome::Help
        );
        assert_eq!(
            parse_args_from(argv(&["-h", "--strict"])),
            ParseOutcome::Help
        );
        assert_eq!(
            parse_args_from(argv(&["--strict", "--bogus", "--help"])),
            ParseOutcome::Help
        );
    }

    #[test]
    fn parse_only_the_exact_strict_flag_enables_strict() {
        // 完全一致以外は true にならず、不明引数として扱われる。
        for arg in ["--STRICT", "strict", "--strict=true", "-s"] {
            assert_eq!(
                parse_args_from(argv(&[arg])),
                ParseOutcome::UnknownArgument(arg.to_string()),
                "unexpected outcome for {arg}"
            );
        }
    }

    // --- 2. 設定判定（依存注入） ------------------------------------------

    #[test]
    fn check_always_reports_twenty_three_categories() {
        // CLI が表示する "N check(s)" を固定する（既存挙動の回帰防止）。
        // 5 カテゴリ追加（oidc_max_auth_age / oidc_cookie_secure /
        // oidc_required_assurance / expose_internal_errors / allowed_origins）で
        // 14 -> 19。さらにフォールバック検出の2カテゴリ追加
        // （graceful_drain_timeout / moderation_finalize_interval）で 19 -> 21。
        // OIDC エンドポイント URL 検証（oidc_endpoint_urls）の追加で 21 -> 22。
        // AUDIT_EXPECT_ROOT 診断（audit_expect_root）の追加で 22 -> 23。
        // シナリオに依存せず常に同じカテゴリ数になることを保証する。
        let scenarios: [&[(&str, &str)]; 4] = [
            &[],
            &[("ALLOW_MOCK_AUTH", "1")],
            &[
                ("ALLOW_MOCK_AUTH", "1"),
                ("OIDC_ISSUER", "https://issuer.example"),
            ],
            &[
                ("ALLOW_IPV4_LOCALHOST", "1"),
                ("TRUSTED_PROXY_CIDRS", "203.0.113.7/32"),
            ],
        ];
        for pairs in scenarios {
            assert_eq!(
                check_all_with(lookup_from(pairs)).len(),
                23,
                "pairs={pairs:?}"
            );
        }
    }

    #[test]
    fn mock_auth_alone_adds_a_warning() {
        let results = check_all_with(lookup_from(&[("ALLOW_MOCK_AUTH", "1")]));
        let errors = errors_for(&results, "mock_auth_enabled");
        assert_eq!(errors.len(), 1, "mock 単独でも問題を計上すべき: {errors:?}");
        assert!(
            errors[0].contains("ALLOW_MOCK_AUTH is enabled"),
            "警告文に設定名を含める（秘密値は出力しない）: {errors:?}"
        );
        assert!(
            errors_for(&results, "mock_auth_with_production_config").is_empty(),
            "本番設定が無い場合は併用警告を出さない"
        );
        assert!(
            errors_for(&results, "production").is_empty(),
            "allow_mock_auth が真なら production_config_errors は空"
        );
    }

    #[test]
    fn mock_auth_truthy_values_are_recognized_and_falsy_are_not() {
        for value in ["1", "true", "TRUE", "TrUe"] {
            let results = check_all_with(lookup_from(&[("ALLOW_MOCK_AUTH", value)]));
            assert_eq!(
                errors_for(&results, "mock_auth_enabled").len(),
                1,
                "value={value}"
            );
        }
        for value in ["0", "false", "no", ""] {
            let results = check_all_with(lookup_from(&[("ALLOW_MOCK_AUTH", value)]));
            assert!(
                errors_for(&results, "mock_auth_enabled").is_empty(),
                "value={value}"
            );
        }
    }

    #[test]
    fn unset_mock_auth_is_disabled() {
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "mock_auth_enabled").is_empty());
        assert!(errors_for(&results, "mock_auth_with_production_config").is_empty());
    }

    #[test]
    fn mock_auth_combined_with_production_config_is_reported() {
        let scenarios: [&[(&str, &str)]; 3] = [
            &[
                ("ALLOW_MOCK_AUTH", "1"),
                ("OIDC_ISSUER", "https://issuer.example"),
            ],
            &[
                ("ALLOW_MOCK_AUTH", "1"),
                ("PINATA_JWT", "real-production-jwt"),
            ],
            &[
                ("ALLOW_MOCK_AUTH", "1"),
                ("JWT_RSA_PRIVATE_KEY_PEM", "-----BEGIN PRIVATE KEY-----"),
            ],
        ];
        for pairs in scenarios {
            let results = check_all_with(lookup_from(pairs));
            let errors = errors_for(&results, "mock_auth_with_production_config");
            assert_eq!(errors.len(), 1, "pairs={pairs:?}");
            assert!(errors[0].contains("do not deploy"), "{errors:?}");
        }
    }

    #[test]
    fn mock_auth_with_mock_pinata_value_is_not_a_combination() {
        for pinata in ["mock_jwt", " mock_jwt ", "\tmock_jwt", "", " ", "\t"] {
            let results = check_all_with(lookup_from(&[
                ("ALLOW_MOCK_AUTH", "1"),
                ("PINATA_JWT", pinata),
            ]));
            assert!(
                errors_for(&results, "mock_auth_with_production_config").is_empty(),
                "pinata={pinata}"
            );
        }
    }

    #[test]
    fn invalid_ipfs_gateway_entries_increase_problem_count() {
        let results = check_all_with(lookup_from(&[(
            "IPFS_GATEWAYS",
            "not-a-url, ftp://foo/{cid}, https://ok.example/ipfs/{cid}",
        )]));
        let errors = errors_for(&results, "ipfs_gateways");
        assert_eq!(
            errors.len(),
            1,
            "expected invalid-entry warning: {errors:?}"
        );
        assert!(
            errors[0].contains("2 invalid entry(ies)"),
            "不正な 2 件が件数に反映されるべき: {errors:?}"
        );
    }

    #[test]
    fn all_invalid_ipfs_gateways_report_both_warnings() {
        let results = check_all_with(lookup_from(&[("IPFS_GATEWAYS", "nope, ftp://x/{cid}")]));
        let errors = errors_for(&results, "ipfs_gateways");
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("2 invalid entry(ies)"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("no valid IPFS gateway"))
        );
    }

    #[test]
    fn unset_ipfs_gateways_uses_default_without_warning() {
        let results = check_all_with(lookup_from(&[]));
        assert!(
            errors_for(&results, "ipfs_gateways").is_empty(),
            "未設定時は既定ゲートウェイを使うため加算しない"
        );
    }

    #[test]
    fn valid_ipfs_gateways_add_no_invalid_entry_warning() {
        let results = check_all_with(lookup_from(&[(
            "IPFS_GATEWAYS",
            "https://a.example/ipfs/{cid},https://b.example/ipfs/{cid}",
        )]));
        assert!(errors_for(&results, "ipfs_gateways").is_empty());
    }

    #[test]
    fn secret_values_are_never_printed_in_problems() {
        const JWT: &str = "jwt-super-secret-value-1234567890-abcdef";
        const PINATA: &str = "pinata-super-secret-1234567890-abcdef";
        const OIDC: &str = "oidc-super-secret-1234567890-abcdef";
        let results = check_all_with(lookup_from(&[
            ("JWT_SECRET", JWT),
            ("PINATA_JWT", PINATA),
            ("OIDC_CLIENT_SECRET", OIDC),
            ("ALLOW_MOCK_AUTH", "1"),
        ]));
        for (_, errors) in &results {
            for error in errors {
                assert!(!error.contains(JWT), "JWT_SECRET が漏洩: {error}");
                assert!(!error.contains(PINATA), "PINATA_JWT が漏洩: {error}");
                assert!(!error.contains(OIDC), "OIDC_CLIENT_SECRET が漏洩: {error}");
            }
        }
    }

    // --- 2b. IPv4 loopback / 信頼済みプロキシ -------------------------------

    #[test]
    fn ipv4_localhost_truthy_values_add_a_warning() {
        // network.rs::ipv4_permitted と同じ真偽判定（大小文字無視）。
        // 値に依らず同じ警告文になることを固定し、値のエコーを防ぐ。
        const EXPECTED: &str =
            "ALLOW_IPV4_LOCALHOST is enabled; production should stay IPv6-only (SEC-01)";
        for value in ["1", "true", "TRUE", "TrUe"] {
            let results = check_all_with(lookup_from(&[("ALLOW_IPV4_LOCALHOST", value)]));
            let errors = errors_for(&results, "ipv4_localhost");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            assert_eq!(errors[0], EXPECTED, "value={value}");
        }
    }

    #[test]
    fn ipv4_localhost_falsy_and_unset_add_no_warning() {
        for value in ["0", "false", "no", ""] {
            let results = check_all_with(lookup_from(&[("ALLOW_IPV4_LOCALHOST", value)]));
            assert!(
                errors_for(&results, "ipv4_localhost").is_empty(),
                "value={value}"
            );
        }
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "ipv4_localhost").is_empty());
    }

    #[test]
    fn trusted_proxy_when_configured_warns_without_disclosing_value() {
        const CIDRS: &str = "203.0.113.7/32,198.51.100.0/24";
        let results = check_all_with(lookup_from(&[("TRUSTED_PROXY_CIDRS", CIDRS)]));
        let errors = errors_for(&results, "trusted_proxy");
        assert_eq!(errors.len(), 1, "errors={errors:?}");
        assert!(
            errors[0].contains("X-Forwarded-For") || errors[0].contains("XFF"),
            "XFF を信頼する旨の警告であるべき: {errors:?}"
        );
        for error in errors {
            assert!(!error.contains(CIDRS), "CIDR 値が漏洩: {error}");
            assert!(!error.contains("203.0.113.7"), "CIDR 値が漏洩: {error}");
            assert!(!error.contains("198.51.100.0"), "CIDR 値が漏洩: {error}");
        }
    }

    #[test]
    fn trusted_proxy_unset_adds_no_warning() {
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "trusted_proxy").is_empty());
    }

    // --- 2c. OIDC clock skew ----------------------------------------------

    #[test]
    fn oidc_clock_skew_in_range_and_unset_add_no_warning() {
        for value in ["1", "60", "300"] {
            let results = check_all_with(lookup_from(&[("OIDC_CLOCK_SKEW_SECS", value)]));
            assert!(
                errors_for(&results, "oidc_clock_skew").is_empty(),
                "value={value}"
            );
        }
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "oidc_clock_skew").is_empty());
    }

    #[test]
    fn oidc_clock_skew_out_of_range_warns_without_disclosing_value() {
        const EXPECTED: &str = "OIDC_CLOCK_SKEW_SECS is outside 1..=300; using default 60 seconds";
        const LEAKY: &str = "999999999";
        for value in ["0", "301", "-1", "abc", LEAKY] {
            let results = check_all_with(lookup_from(&[("OIDC_CLOCK_SKEW_SECS", value)]));
            let errors = errors_for(&results, "oidc_clock_skew");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            // 固定文言なので設定値は一切含まれない（値のエコーなし）。
            assert_eq!(errors[0], EXPECTED, "value={value}");
            assert!(!errors[0].contains(LEAKY), "設定値が漏洩");
        }
    }

    // --- 2d. タイムアウト / 試行回数のフォールバック検出 --------------------

    #[test]
    fn ipfs_timeout_fallback_is_warned_without_disclosing_value() {
        const EXPECTED: &str = "IPFS_TIMEOUT_SECS is invalid or outside 1..=300; using default";
        const LEAKY: &str = "999999999";
        for value in ["abc", "0", "301", LEAKY] {
            let results = check_all_with(lookup_from(&[("IPFS_TIMEOUT_SECS", value)]));
            let errors = errors_for(&results, "ipfs_timeout");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            // 固定文言なので設定値は一切含まれない（値のエコーなし）。
            assert_eq!(errors[0], EXPECTED, "value={value}");
            assert!(!errors[0].contains(LEAKY), "設定値が漏洩");
        }
        for value in ["1", "15", "300"] {
            let results = check_all_with(lookup_from(&[("IPFS_TIMEOUT_SECS", value)]));
            assert!(
                errors_for(&results, "ipfs_timeout").is_empty(),
                "value={value}"
            );
        }
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "ipfs_timeout").is_empty());
    }

    #[test]
    fn ipfs_max_attempts_fallback_is_warned_without_disclosing_value() {
        const EXPECTED: &str = "IPFS_MAX_ATTEMPTS is invalid or outside 1..=5; using default";
        const LEAKY: &str = "999999999";
        for value in ["abc", "0", "6", LEAKY] {
            let results = check_all_with(lookup_from(&[("IPFS_MAX_ATTEMPTS", value)]));
            let errors = errors_for(&results, "ipfs_max_attempts");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            assert_eq!(errors[0], EXPECTED, "value={value}");
            assert!(!errors[0].contains(LEAKY), "設定値が漏洩");
        }
        for value in ["1", "3", "5"] {
            let results = check_all_with(lookup_from(&[("IPFS_MAX_ATTEMPTS", value)]));
            assert!(
                errors_for(&results, "ipfs_max_attempts").is_empty(),
                "value={value}"
            );
        }
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "ipfs_max_attempts").is_empty());
    }

    #[test]
    fn oidc_http_timeout_fallback_is_warned_without_disclosing_value() {
        const EXPECTED: &str = "OIDC_HTTP_TIMEOUT_SECS is invalid or outside 1..=60; using default";
        const LEAKY: &str = "999999999";
        for value in ["abc", "0", "61", LEAKY] {
            let results = check_all_with(lookup_from(&[("OIDC_HTTP_TIMEOUT_SECS", value)]));
            let errors = errors_for(&results, "oidc_http_timeout");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            assert_eq!(errors[0], EXPECTED, "value={value}");
            assert!(!errors[0].contains(LEAKY), "設定値が漏洩");
        }
        for value in ["1", "10", "60"] {
            let results = check_all_with(lookup_from(&[("OIDC_HTTP_TIMEOUT_SECS", value)]));
            assert!(
                errors_for(&results, "oidc_http_timeout").is_empty(),
                "value={value}"
            );
        }
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "oidc_http_timeout").is_empty());
    }

    #[test]
    fn graceful_drain_timeout_fallback_is_warned_without_disclosing_value() {
        const EXPECTED: &str =
            "GRACEFUL_DRAIN_TIMEOUT_SECS is invalid or outside 1..=300; using default";
        const LEAKY: &str = "999999999";
        for value in ["abc", "0", "301", LEAKY] {
            let results = check_all_with(lookup_from(&[("GRACEFUL_DRAIN_TIMEOUT_SECS", value)]));
            let errors = errors_for(&results, "graceful_drain_timeout");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            // 固定文言なので設定値は一切含まれない（値のエコーなし）。
            assert_eq!(errors[0], EXPECTED, "value={value}");
            assert!(!errors[0].contains(LEAKY), "設定値が漏洩");
        }
        for value in ["1", "30", "300"] {
            let results = check_all_with(lookup_from(&[("GRACEFUL_DRAIN_TIMEOUT_SECS", value)]));
            assert!(
                errors_for(&results, "graceful_drain_timeout").is_empty(),
                "value={value}"
            );
        }
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "graceful_drain_timeout").is_empty());
    }

    #[test]
    fn moderation_finalize_interval_fallback_is_warned_without_disclosing_value() {
        const EXPECTED: &str =
            "MODERATION_FINALIZE_INTERVAL_SECS is invalid or outside 30..=3600; using default";
        const LEAKY: &str = "999999999";
        for value in ["abc", "0", "29", "3601", LEAKY] {
            let results =
                check_all_with(lookup_from(&[("MODERATION_FINALIZE_INTERVAL_SECS", value)]));
            let errors = errors_for(&results, "moderation_finalize_interval");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            // 固定文言なので設定値は一切含まれない（値のエコーなし）。
            assert_eq!(errors[0], EXPECTED, "value={value}");
            assert!(!errors[0].contains(LEAKY), "設定値が漏洩");
        }
        for value in ["30", "300", "3600"] {
            let results =
                check_all_with(lookup_from(&[("MODERATION_FINALIZE_INTERVAL_SECS", value)]));
            assert!(
                errors_for(&results, "moderation_finalize_interval").is_empty(),
                "value={value}"
            );
        }
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "moderation_finalize_interval").is_empty());
    }

    // --- 2e. 追加の設定診断（R2 監査対応） --------------------------------

    #[test]
    fn oidc_max_auth_age_invalid_warns_without_disclosing_value() {
        const GREATER: &str = "OIDC_MAX_AUTH_AGE_SECS must be greater than 0";
        const POSITIVE: &str = "OIDC_MAX_AUTH_AGE_SECS must be a positive integer";
        for (value, expected) in [
            ("0", GREATER),
            ("-1", POSITIVE),
            ("abc", POSITIVE),
            ("999999999999999999999999999999", POSITIVE),
        ] {
            let results = check_all_with(lookup_from(&[("OIDC_MAX_AUTH_AGE_SECS", value)]));
            let errors = errors_for(&results, "oidc_max_auth_age");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            // 固定文言なので設定値は一切含まれない（値のエコーなし）。
            assert_eq!(errors[0], expected, "value={value}");
        }
        for value in ["1", "60", "3600"] {
            let results = check_all_with(lookup_from(&[("OIDC_MAX_AUTH_AGE_SECS", value)]));
            assert!(
                errors_for(&results, "oidc_max_auth_age").is_empty(),
                "value={value}"
            );
        }
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "oidc_max_auth_age").is_empty());
        // mock 有効かつ 7 変数未揃いなら from_env_with は Ok(None) を返すため警告しない。
        let results = check_all_with(lookup_from(&[
            ("ALLOW_MOCK_AUTH", "1"),
            ("OIDC_MAX_AUTH_AGE_SECS", "0"),
        ]));
        assert!(errors_for(&results, "oidc_max_auth_age").is_empty());
    }

    #[test]
    fn oidc_max_auth_age_mock_with_complete_oidc_warns_without_disclosing_value() {
        // mock 有効でも OIDC 7 変数が揃っていれば from_env_with は max_auth_age を
        // 検証して Err を返すため、doctor も警告する（値はエコーしない）。
        const GREATER: &str = "OIDC_MAX_AUTH_AGE_SECS must be greater than 0";
        const POSITIVE: &str = "OIDC_MAX_AUTH_AGE_SECS must be a positive integer";
        for (value, expected) in [("0", GREATER), ("-1", POSITIVE), ("abc", POSITIVE)] {
            let mut pairs = oidc_complete_pairs("https://issuer.example.jp");
            pairs.push(("ALLOW_MOCK_AUTH", "1"));
            pairs.push(("OIDC_MAX_AUTH_AGE_SECS", value));
            let results = check_all_with(lookup_from(&pairs));
            let errors = errors_for(&results, "oidc_max_auth_age");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            assert_eq!(errors[0], expected, "value={value}");
        }
        // 有効値なら mock 有効・7 変数揃いでも警告しない。
        let mut pairs = oidc_complete_pairs("https://issuer.example.jp");
        pairs.push(("ALLOW_MOCK_AUTH", "1"));
        pairs.push(("OIDC_MAX_AUTH_AGE_SECS", "900"));
        let results = check_all_with(lookup_from(&pairs));
        assert!(errors_for(&results, "oidc_max_auth_age").is_empty());
    }

    #[test]
    fn oidc_max_auth_age_mock_with_incomplete_oidc_adds_no_warning() {
        // 7 変数が揃っていなければ from_env_with は Ok(None) を返し max_auth_age を
        // 解析しないため、mock 有効時は警告しない。
        let scenarios: [&str; 2] = ["OIDC_JWKS_URI", "OIDC_CLIENT_SECRET"];
        for missing in scenarios {
            let mut pairs = oidc_complete_pairs("https://issuer.example.jp");
            pairs.retain(|(key, _)| *key != missing);
            pairs.push(("ALLOW_MOCK_AUTH", "1"));
            pairs.push(("OIDC_MAX_AUTH_AGE_SECS", "0"));
            let results = check_all_with(lookup_from(&pairs));
            assert!(
                errors_for(&results, "oidc_max_auth_age").is_empty(),
                "missing={missing}"
            );
        }
    }

    #[test]
    fn production_warns_when_required_oidc_value_is_empty_without_disclosing_value() {
        // OIDC 必須値が Some でも trim 後空文字なら「未設定」と同じく警告する。
        // 警告文は固定で、設定値（秘密情報を含む）を一切エコーしない。
        const EXPECTED: &str = "OIDC is not configured (OIDC_* environment variables are required)";
        for key in ["OIDC_CLIENT_ID", "OIDC_CLIENT_SECRET"] {
            for raw in ["", "   ", "\t\n"] {
                let mut pairs = oidc_complete_pairs("https://issuer.example.jp");
                pairs.retain(|(name, _)| *name != key);
                pairs.push((key, raw));
                // PINATA_JWT を揃え、production の警告を OIDC 未設定だけに孤立させる。
                pairs.push(("PINATA_JWT", "real-production-jwt"));
                let results = check_all_with(lookup_from(&pairs));
                let errors = errors_for(&results, "production");
                assert_eq!(errors.len(), 1, "key={key}, raw={raw:?}, errors={errors:?}");
                // 固定文言なので空文字・空白の設定値は一切含まれない（値のエコーなし）。
                assert_eq!(errors[0], EXPECTED, "key={key}, raw={raw:?}");
            }
        }
    }

    #[test]
    fn expose_internal_errors_truthy_warns_without_disclosing_value() {
        const EXPECTED: &str =
            "EXPOSE_INTERNAL_ERRORS is enabled; internal error details are exposed to clients";
        for value in ["1", "true", "TRUE", "TrUe"] {
            let results = check_all_with(lookup_from(&[("EXPOSE_INTERNAL_ERRORS", value)]));
            let errors = errors_for(&results, "expose_internal_errors");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            assert_eq!(errors[0], EXPECTED, "value={value}");
        }
        for value in ["0", "false", "no", ""] {
            let results = check_all_with(lookup_from(&[("EXPOSE_INTERNAL_ERRORS", value)]));
            assert!(
                errors_for(&results, "expose_internal_errors").is_empty(),
                "value={value}"
            );
        }
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "expose_internal_errors").is_empty());
        // mock 有効時（開発環境）は警告しない。
        let results = check_all_with(lookup_from(&[
            ("ALLOW_MOCK_AUTH", "1"),
            ("EXPOSE_INTERNAL_ERRORS", "1"),
        ]));
        assert!(errors_for(&results, "expose_internal_errors").is_empty());
    }

    #[test]
    fn allowed_origins_missing_or_empty_warns_without_disclosing_value() {
        const EXPECTED: &str = "ALLOWED_ORIGINS has no valid http(s)/null origin (unset, empty, or all entries invalid) while mock auth is disabled; all CORS origins are rejected";
        let scenarios: [&[(&str, &str)]; 3] = [
            &[],
            &[("ALLOWED_ORIGINS", "")],
            &[("ALLOWED_ORIGINS", " , , ")],
        ];
        for pairs in scenarios {
            let results = check_all_with(lookup_from(pairs));
            let errors = errors_for(&results, "allowed_origins");
            assert_eq!(errors.len(), 1, "pairs={pairs:?}, errors={errors:?}");
            assert_eq!(errors[0], EXPECTED, "pairs={pairs:?}");
        }
        // オリジン値は出力しない。
        const ORIGIN: &str = "https://allowed-origin.example";
        let results = check_all_with(lookup_from(&[("ALLOWED_ORIGINS", ORIGIN)]));
        let errors = errors_for(&results, "allowed_origins");
        assert!(errors.is_empty(), "errors={errors:?}");
        for error in errors {
            assert!(!error.contains(ORIGIN), "オリジン値が漏洩: {error}");
        }
        // mock 有効時は未設定でも警告しない（開発用既定オリジンが使われる）。
        let results = check_all_with(lookup_from(&[("ALLOW_MOCK_AUTH", "1")]));
        assert!(errors_for(&results, "allowed_origins").is_empty());
    }

    #[test]
    fn allowed_origins_uses_real_parser_result_without_disclosing_value() {
        const EXPECTED: &str = "ALLOWED_ORIGINS has no valid http(s)/null origin (unset, empty, or all entries invalid) while mock auth is disabled; all CORS origins are rejected";

        // http(s) scheme のない不正要素は実パーサで全て捨てられ、空ベクタ→警告。
        // 旧「非空」判定では見逃していたケース。値はエコーしない。
        const INVALID: &str = "example.com,foo";
        let results = check_all_with(lookup_from(&[("ALLOWED_ORIGINS", INVALID)]));
        let errors = errors_for(&results, "allowed_origins");
        assert_eq!(errors.len(), 1, "errors={errors:?}");
        assert_eq!(errors[0], EXPECTED);
        assert!(!errors[0].contains(INVALID), "オリジン値が漏洩: {errors:?}");

        // 有効なオリジン（複数指定・null）は警告なし。
        for value in ["https://a.example,https://b.example", "null"] {
            let results = check_all_with(lookup_from(&[("ALLOWED_ORIGINS", value)]));
            assert!(
                errors_for(&results, "allowed_origins").is_empty(),
                "value={value}"
            );
        }

        // 未設定（mock 偽）は警告、mock 有効時は警告なし（既存挙動を維持）。
        let results = check_all_with(lookup_from(&[]));
        assert_eq!(errors_for(&results, "allowed_origins").len(), 1);
        let results = check_all_with(lookup_from(&[("ALLOW_MOCK_AUTH", "1")]));
        assert!(errors_for(&results, "allowed_origins").is_empty());
    }

    #[test]
    fn oidc_cookie_secure_falsy_warns_without_disclosing_value() {
        const EXPECTED: &str = "OIDC_COOKIE_SECURE is disabled; session, refresh, and OIDC state cookies will lack the Secure attribute";
        for value in ["0", "false", "FALSE", "FaLsE"] {
            let results = check_all_with(lookup_from(&[("OIDC_COOKIE_SECURE", value)]));
            let errors = errors_for(&results, "oidc_cookie_secure");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            assert_eq!(errors[0], EXPECTED, "value={value}");
        }
        for value in ["1", "true", "yes", ""] {
            let results = check_all_with(lookup_from(&[("OIDC_COOKIE_SECURE", value)]));
            assert!(
                errors_for(&results, "oidc_cookie_secure").is_empty(),
                "value={value}"
            );
        }
        let results = check_all_with(lookup_from(&[]));
        assert!(errors_for(&results, "oidc_cookie_secure").is_empty());
        // mock 有効時は警告しない。
        let results = check_all_with(lookup_from(&[
            ("ALLOW_MOCK_AUTH", "1"),
            ("OIDC_COOKIE_SECURE", "0"),
        ]));
        assert!(errors_for(&results, "oidc_cookie_secure").is_empty());
    }

    #[test]
    fn oidc_required_assurance_unset_warns_without_disclosing_value() {
        const ACR: &str =
            "OIDC_REQUIRED_ACR is not set; no required authentication context class is enforced";
        const AMR: &str =
            "OIDC_REQUIRED_AMR is not set; no required authentication method is enforced";

        let results = check_all_with(lookup_from(&[]));
        let errors = errors_for(&results, "oidc_required_assurance");
        assert_eq!(errors.len(), 2, "errors={errors:?}");
        assert_eq!(errors[0], ACR);
        assert_eq!(errors[1], AMR);

        // 空要素のみでも未設定と同じく警告する。
        let results = check_all_with(lookup_from(&[
            ("OIDC_REQUIRED_ACR", " , "),
            ("OIDC_REQUIRED_AMR", "   "),
        ]));
        assert_eq!(errors_for(&results, "oidc_required_assurance").len(), 2);

        // 両方設定されていれば警告しない。
        let results = check_all_with(lookup_from(&[
            ("OIDC_REQUIRED_ACR", "urn:example:acr"),
            ("OIDC_REQUIRED_AMR", "pwd,mfa"),
        ]));
        assert!(errors_for(&results, "oidc_required_assurance").is_empty());

        // 片方だけ設定なら未設定側のみ警告し、設定値は出力しない。
        let results = check_all_with(lookup_from(&[("OIDC_REQUIRED_ACR", "urn:example:acr")]));
        let errors = errors_for(&results, "oidc_required_assurance");
        assert_eq!(errors.len(), 1, "errors={errors:?}");
        assert_eq!(errors[0], AMR);
        assert!(
            !errors[0].contains("urn:example:acr"),
            "設定値が漏洩: {}",
            errors[0]
        );

        // mock 有効時は警告しない。
        let results = check_all_with(lookup_from(&[("ALLOW_MOCK_AUTH", "1")]));
        assert!(errors_for(&results, "oidc_required_assurance").is_empty());
    }

    // --- 2f. OIDC エンドポイント URL ---------------------------------------

    const OIDC_ENDPOINT_URL_KEYS: [&str; 5] = [
        "OIDC_ISSUER",
        "OIDC_REDIRECT_URI",
        "OIDC_AUTHORIZATION_ENDPOINT",
        "OIDC_TOKEN_ENDPOINT",
        "OIDC_JWKS_URI",
    ];
    const OIDC_ENDPOINT_URL_WARNING: &str = "OIDC endpoints must be https (http is allowed only for localhost when ALLOW_MOCK_AUTH is enabled)";

    /// OIDC 7 変数をすべて揃えた環境を組み立てる（5 エンドポイントは同じ URL）。
    fn oidc_complete_pairs<'a>(endpoint: &'a str) -> Vec<(&'static str, &'a str)> {
        let mut pairs: Vec<(&'static str, &'a str)> = vec![
            ("OIDC_CLIENT_ID", "client-id"),
            ("OIDC_CLIENT_SECRET", "client-secret"),
        ];
        pairs.extend(OIDC_ENDPOINT_URL_KEYS.iter().map(|key| (*key, endpoint)));
        pairs
    }

    #[test]
    fn oidc_endpoint_urls_https_adds_no_warning() {
        let pairs =
            oidc_complete_pairs("https://issuer.example.jp/.well-known/openid-configuration");
        let results = check_all_with(lookup_from(&pairs));
        assert!(
            errors_for(&results, "oidc_endpoint_urls").is_empty(),
            "errors={:?}",
            errors_for(&results, "oidc_endpoint_urls")
        );

        // 7 変数が揃っていなければ（例: OIDC_JWKS_URI 欠落）検証しない。
        let mut incomplete = oidc_complete_pairs("http://remote.example.jp/issuer");
        incomplete.retain(|(key, _)| *key != "OIDC_JWKS_URI");
        let results = check_all_with(lookup_from(&incomplete));
        assert!(errors_for(&results, "oidc_endpoint_urls").is_empty());
    }

    #[test]
    fn oidc_endpoint_urls_http_in_production_warns_without_disclosing_value() {
        const LEAKY: &str = "http://remote-secret.example.jp/path";
        let pairs = oidc_complete_pairs(LEAKY);
        let results = check_all_with(lookup_from(&pairs));
        let errors = errors_for(&results, "oidc_endpoint_urls");
        assert_eq!(errors.len(), 1, "errors={errors:?}");
        assert_eq!(errors[0], OIDC_ENDPOINT_URL_WARNING);
        assert!(!errors[0].contains(LEAKY), "設定値が漏洩: {}", errors[0]);
    }

    #[test]
    fn oidc_endpoint_urls_mock_allows_only_localhost_http() {
        // mock 有効時は localhost 系の http のみ警告なし。
        for host in ["localhost", "127.0.0.1", "[::1]", "::1"] {
            let url = if host == "::1" {
                "http://[::1]/endpoint".to_string()
            } else {
                format!("http://{host}/endpoint")
            };
            let mut pairs = oidc_complete_pairs(&url);
            pairs.push(("ALLOW_MOCK_AUTH", "1"));
            let results = check_all_with(lookup_from(&pairs));
            let errors = errors_for(&results, "oidc_endpoint_urls");
            assert!(errors.is_empty(), "host={host}, errors={errors:?}");
        }

        // mock 有効でもリモートの http は警告する（from_env_with と同じ条件）。
        const LEAKY: &str = "http://remote-secret.example.jp/issuer";
        let mut pairs = oidc_complete_pairs("https://issuer.example.jp");
        pairs.retain(|(key, _)| *key != "OIDC_ISSUER");
        pairs.push(("OIDC_ISSUER", LEAKY));
        pairs.push(("ALLOW_MOCK_AUTH", "1"));
        let results = check_all_with(lookup_from(&pairs));
        let errors = errors_for(&results, "oidc_endpoint_urls");
        assert_eq!(errors.len(), 1, "errors={errors:?}");
        assert_eq!(errors[0], OIDC_ENDPOINT_URL_WARNING);
        assert!(!errors[0].contains(LEAKY), "設定値が漏洩: {}", errors[0]);
    }

    // --- 2g. AUDIT_EXPECT_ROOT --------------------------------------------

    #[test]
    fn audit_expect_root_warns_in_production_without_disclosing_value() {
        const EXPECTED: &str = "AUDIT_EXPECT_ROOT is required in production (a hex Merkle root)";
        const INVALID: &str = "deadbeefcafebabe-nothex-ZZZZ";
        // 本番相当（mock 無効）: 未設定・空白のみ・不正 hex は警告。
        let scenarios: [&[(&str, &str)]; 4] = [
            &[],
            &[("AUDIT_EXPECT_ROOT", "")],
            &[("AUDIT_EXPECT_ROOT", " \t ")],
            &[("AUDIT_EXPECT_ROOT", INVALID)],
        ];
        for pairs in scenarios {
            let results = check_all_with(lookup_from(pairs));
            let errors = errors_for(&results, "audit_expect_root");
            assert_eq!(errors.len(), 1, "pairs={pairs:?}, errors={errors:?}");
            // 固定文言なので設定値は一切含まれない（値のエコーなし）。
            assert_eq!(errors[0], EXPECTED, "pairs={pairs:?}");
            assert!(!errors[0].contains(INVALID), "設定値が漏洩: {}", errors[0]);
        }
        // 有効 hex は警告なし（生値判定のため前後の空白は許容しない）。
        for value in ["deadbeefcafebabe", "0123456789abcdef", "ABCDEF0123456789"] {
            let results = check_all_with(lookup_from(&[("AUDIT_EXPECT_ROOT", value)]));
            assert!(
                errors_for(&results, "audit_expect_root").is_empty(),
                "value={value}"
            );
        }
        // 前後空白付き hex は生値へ hex 判定するため警告（backup-rotate と統一）。
        // 固定文言なので設定値は一切含まれない（値のエコーなし）。
        const PADDED: &str = "  ABCDEF0123456789  ";
        let results = check_all_with(lookup_from(&[("AUDIT_EXPECT_ROOT", PADDED)]));
        let errors = errors_for(&results, "audit_expect_root");
        assert_eq!(errors.len(), 1, "errors={errors:?}");
        assert_eq!(errors[0], EXPECTED);
        assert!(!errors[0].contains("ABCDEF"), "設定値が漏洩: {}", errors[0]);
        // mock 有効時は開発環境として警告しない（後方互換を維持）。
        for value in ["", INVALID, "deadbeefcafebabe"] {
            let results = check_all_with(lookup_from(&[
                ("ALLOW_MOCK_AUTH", "1"),
                ("AUDIT_EXPECT_ROOT", value),
            ]));
            assert!(
                errors_for(&results, "audit_expect_root").is_empty(),
                "value={value}"
            );
        }
        let results = check_all_with(lookup_from(&[("ALLOW_MOCK_AUTH", "1")]));
        assert!(errors_for(&results, "audit_expect_root").is_empty());
    }

    // --- 3. strict 判定 ----------------------------------------------------

    #[test]
    fn non_strict_never_fails() {
        assert!(!should_fail(false, 0));
        assert!(!should_fail(false, 1));
        assert!(!should_fail(false, usize::MAX));
    }

    #[test]
    fn strict_fails_only_when_problems_exist() {
        assert!(!should_fail(true, 0));
        assert!(should_fail(true, 1));
        assert!(should_fail(true, 2));
    }

    #[test]
    fn end_to_end_exit_decision_for_mock_only_configuration() {
        let results = check_all_with(lookup_from(&[("ALLOW_MOCK_AUTH", "1")]));
        let problems = problem_count(&results);
        assert!(problems > 0, "判定対象の問題が無いテストは無意味");
        assert!(!should_fail(false, problems));
        assert!(should_fail(true, problems));
    }
}
