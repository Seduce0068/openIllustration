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

/// 検査結果を (項目, 問題) の形で集める。
fn check_all() -> Vec<(&'static str, Vec<String>)> {
    let mut results = Vec::new();

    // 本番必須設定は既存の fail-closed 判定へ委譲する（再実装しない）。
    let production = production_config_errors(
        env("ALLOW_MOCK_AUTH")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false),
        env("OIDC_ISSUER").is_some()
            && env("OIDC_CLIENT_ID").is_some()
            && env("OIDC_CLIENT_SECRET").is_some()
            && env("OIDC_REDIRECT_URI").is_some()
            && env("OIDC_AUTHORIZATION_ENDPOINT").is_some()
            && env("OIDC_TOKEN_ENDPOINT").is_some()
            && env("OIDC_JWKS_URI").is_some(),
        env("PINATA_JWT").as_deref(),
    );
    results.push((
        "production",
        production.into_iter().map(str::to_string).collect(),
    ));

    // JWT: HS256 は 32 バイト以上、RS256 は PEM と検証キーの整合を検査する。
    let jwt_pem = env("JWT_RSA_PRIVATE_KEY_PEM");
    let jwt_secret = env("JWT_SECRET");
    let jwt_errors = match jwt_keys_from_env(
        jwt_pem,
        jwt_secret,
        env("JWT_RSA_VERIFICATION_KEYS").as_deref(),
        env("JWT_REQUIRE_KID")
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(false),
    ) {
        Ok(_) => Vec::new(),
        Err(error) => vec![error],
    };
    results.push(("jwt", jwt_errors));

    let mut secret_errors = Vec::new();
    if let Some(secret) = env("JWT_SECRET")
        && let Err(error) = validate_jwt_secret(&secret)
    {
        secret_errors.push(error);
    }
    results.push(("jwt_secret", secret_errors));

    // IPFS: ゲートウェイは {cid} を含む http(s) URL テンプレートが必要。
    let gateways = ipfs_gateways(env("IPFS_GATEWAYS").as_deref());
    let ipfs_errors = if gateways.is_empty() {
        vec!["no valid IPFS gateway configured".to_string()]
    } else {
        Vec::new()
    };
    results.push(("ipfs_gateways", ipfs_errors));

    let timeout_errors = if ipfs_timeout(env("IPFS_TIMEOUT_SECS").as_deref()).as_secs() == 0 {
        vec!["IPFS timeout resolved to zero".to_string()]
    } else {
        Vec::new()
    };
    results.push(("ipfs_timeout", timeout_errors));
    results.push((
        "ipfs_max_attempts",
        if ipfs_max_attempts(env("IPFS_MAX_ATTEMPTS").as_deref()) == 0 {
            vec!["IPFS attempts resolved to zero".to_string()]
        } else {
            Vec::new()
        },
    ));

    let oidc_timeout = oidc_http_timeout(env("OIDC_HTTP_TIMEOUT_SECS").as_deref());
    results.push((
        "oidc_http_timeout",
        if oidc_timeout.as_secs() == 0 {
            vec!["OIDC HTTP timeout resolved to zero".to_string()]
        } else {
            Vec::new()
        },
    ));

    // 年齢ポリシー: 不正値は AgePolicy::from_env がエラーを返す。
    let age_errors = match common::age::AgePolicy::from_env() {
        Ok(_) => Vec::new(),
        Err(error) => vec![error],
    };
    results.push(("age_policy", age_errors));

    // レート制限: 既定値へフォールバックしていても警告する。
    let mut rate_errors = Vec::new();
    if let Some(value) = env("RATE_LIMIT_PER_MIN")
        && rate_limit_per_min(Some(&value)) != value.parse::<u32>().unwrap_or(0)
    {
        rate_errors.push("RATE_LIMIT_PER_MIN is outside 1..=600; using default".to_string());
    }
    if let Some(value) = env("MAX_CONCURRENT_REQUESTS")
        && max_concurrent_requests(Some(&value)) != value.parse::<usize>().unwrap_or(0)
    {
        rate_errors.push("MAX_CONCURRENT_REQUESTS is outside 1..=256; using default".to_string());
    }
    results.push(("rate_limit", rate_errors));

    results
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let strict = match args.as_slice() {
        [] => false,
        [arg] if arg == "--strict" => true,
        [arg] if arg == "-h" || arg == "--help" => {
            print_help();
            return;
        }
        [other] => {
            eprintln!("unknown argument: {other}");
            print_help();
            std::process::exit(1);
        }
        _ => {
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
    if strict && problems > 0 {
        std::process::exit(1);
    }
}
