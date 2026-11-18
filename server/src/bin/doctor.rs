//! 設定検証 CLI。本番起動前に設定を診断する（R5-06 / R4-01 の事前検証）。
//! 既定は警告のみ。`--strict` を付けたときのみ問題があれば終了コード 1 とする。
//! 秘密情報の値は一切出力しない。

use server::{
    ipfs_gateways, ipfs_max_attempts, ipfs_timeout, jwt_keys_from_env, max_concurrent_requests,
    oidc_http_timeout, production_config_errors, rate_limit_per_min, validate_jwt_secret,
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
    match args.as_slice() {
        [] => ParseOutcome::Run { strict: false },
        [arg] if arg == "--strict" => ParseOutcome::Run { strict: true },
        [arg] if arg == "-h" || arg == "--help" => ParseOutcome::Help,
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
    let production = production_config_errors(
        allow_mock_auth,
        lookup("OIDC_ISSUER").is_some()
            && lookup("OIDC_CLIENT_ID").is_some()
            && lookup("OIDC_CLIENT_SECRET").is_some()
            && lookup("OIDC_REDIRECT_URI").is_some()
            && lookup("OIDC_AUTHORIZATION_ENDPOINT").is_some()
            && lookup("OIDC_TOKEN_ENDPOINT").is_some()
            && lookup("OIDC_JWKS_URI").is_some(),
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
    let oidc_any = OIDC_REQUIRED.iter().any(|name| lookup(name).is_some());
    let pinata_production = lookup("PINATA_JWT")
        .map(|v| !v.is_empty() && v != "mock_jwt")
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

    let timeout_errors = if ipfs_timeout(lookup("IPFS_TIMEOUT_SECS").as_deref()).as_secs() == 0 {
        vec!["IPFS timeout resolved to zero".to_string()]
    } else {
        Vec::new()
    };
    results.push(("ipfs_timeout", timeout_errors));
    results.push((
        "ipfs_max_attempts",
        if ipfs_max_attempts(lookup("IPFS_MAX_ATTEMPTS").as_deref()) == 0 {
            vec!["IPFS attempts resolved to zero".to_string()]
        } else {
            Vec::new()
        },
    ));

    let oidc_timeout = oidc_http_timeout(lookup("OIDC_HTTP_TIMEOUT_SECS").as_deref());
    results.push((
        "oidc_http_timeout",
        if oidc_timeout.as_secs() == 0 {
            vec!["OIDC HTTP timeout resolved to zero".to_string()]
        } else {
            Vec::new()
        },
    ));

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
        assert_eq!(
            parse_args_from(argv(&["--strict", "--help"])),
            ParseOutcome::TooManyArguments
        );
        assert_eq!(
            parse_args_from(argv(&["--strict", "--strict"])),
            ParseOutcome::TooManyArguments
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
    fn check_always_reports_eleven_categories() {
        // CLI が表示する "N check(s)" を固定する（既存挙動の回帰防止）。
        let scenarios: [&[(&str, &str)]; 3] = [
            &[],
            &[("ALLOW_MOCK_AUTH", "1")],
            &[
                ("ALLOW_MOCK_AUTH", "1"),
                ("OIDC_ISSUER", "https://issuer.example"),
            ],
        ];
        for pairs in scenarios {
            assert_eq!(
                check_all_with(lookup_from(pairs)).len(),
                11,
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
        for pinata in ["mock_jwt", ""] {
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
