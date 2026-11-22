//! 設定検証 CLI。本番起動前に設定を診断する（R5-06 / R4-01 の事前検証）。
//! 既定は警告のみ。`--strict` を付けたときのみ問題があれば終了コード 1 とする。
//! 秘密情報の値は一切出力しない。

use server::{
    CLOCK_SKEW_MAX_SECS, CLOCK_SKEW_MIN_SECS, DEFAULT_CLOCK_SKEW_SECS, MAX_AUTH_AGE_MAX_SECS,
    graceful_drain_timeout, ipfs_gateways, ipfs_max_attempts, ipfs_timeout, jwt_keys_from_env,
    max_concurrent_requests, moderation_finalize_interval, oidc_http_timeout,
    pinata_jwt_production_like, policy_utc_offset_hours, production_allowed_origins_error,
    production_audit_expect_root_error, production_config_errors, rate_limit_per_min,
    validate_jwt_secret, validate_oidc_config_with,
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

fn env(name: &str) -> Result<String, std::env::VarError> {
    std::env::var(name)
}

// --- 新規診断の固定文言（値は一切エコーしない） ------------------------------

/// 本番のバックアップ必須設定が欠落している場合の固定警告。
const BACKUP_RECIPIENT_UNSET_WARNING: &str =
    "BACKUP_GPG_RECIPIENT is not set; encrypted backups are required in production";
const BACKUP_OFFSITE_PAIR_WARNING: &str = "OFFSITE_COPY_CMD and OFFSITE_DEST must be set together";
const BACKUP_ALERT_CMD_WARNING: &str = "ALERT_CMD is set but not executable";

/// 年齢判定の基準日（TZ）に関する固定警告。
const TZ_UNSET_WARNING: &str =
    "TZ is not set; age baseline date falls back to UTC (set it to match AGE_TARGET_REGIONS)";
const TZ_REGION_MISMATCH_WARNING: &str =
    "AGE_TARGET_REGIONS is configured but TZ does not provide a matching local baseline date";

/// パスが実行可能なファイルかを判定する（`backup-rotate.sh` の `-x` と同趣旨）。
/// 判定結果だけを返し、入力パスは結果へ含めない。
fn is_executable_file(path: &str) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// 明示設定必須なのに必須設定値が未充足で年齢検証が拒否される設定への固定警告（値はエコーしない）。
const AGE_POLICY_UNSATISFIED_WARNING: &str =
    "AGE_REQUIRE_EXPLICIT_CONFIGURATION is set but required age policy values are unsatisfied";

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

/// 真偽値フラグを共通 `bool_env_state` / `bool_env_result` で解釈する
/// （受理値の基準は従来どおり `common::age::parse_bool_env`）。
/// 不正値は入力値をエコーせず、安全側の `default` へ倒した上で固定文言の警告を返す
/// （doctor は診断専用なので起動拒否の代わりに既存カテゴリ内で警告する）。
fn bool_flag_with_warning<F>(lookup: &F, name: &str, default: bool) -> (bool, Option<String>)
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    // 真偽値の分類・既定値適用・不正判定は共通ヘルパへ委譲する。
    // 非 UTF-8 は生バイトを保持せず Invalid として既存の不正値警告へマージする。
    let state = match lookup(name) {
        Err(std::env::VarError::NotPresent) => common::env::EnvValue::Missing,
        Err(std::env::VarError::NotUnicode(_)) => common::env::EnvValue::NonUtf8,
        Ok(value) => common::env::EnvValue::Utf8(value),
    };
    let (flag, invalid_message) =
        common::env::bool_env_result(common::env::bool_env_state(state), default, true);
    // doctor は診断専用なので、既存の固定文言（既定値を含む）を維持したまま警告を返す。
    let warning = invalid_message
        .map(|_| format!("{name} has an invalid boolean value; using {default} (fail-closed)"));
    (flag, warning)
}

/// 検査結果を (項目, 問題) の形で集める。
/// 環境変数は `lookup` 経由でのみ読み、テストではプロセス環境を汚さず差し替えられる。
/// 秘密情報の値は結果に含めない。
fn check_all_with<F>(lookup: F) -> Vec<(&'static str, Vec<String>)>
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    let mut results = Vec::new();

    let (allow_mock_auth, mock_auth_bool_warning) =
        bool_flag_with_warning(&lookup, "ALLOW_MOCK_AUTH", false);
    // OIDC の判定は共通バリデータへ集約する。doctor には非秘密の結果だけが渡され、
    // OidcState / client_secret を渡す必要はない（R1 の重要制約）。
    let validation = validate_oidc_config_with(&lookup);

    // 本番必須設定は既存の fail-closed 判定へ委譲する（再実装しない）。
    // required_present は必須 7 変数の存在と trim 後非空を反映するため、
    // 空文字の必須値も「未設定」と同じく警告対象になる。値は結果へ含めない。
    // PINATA_JWT は classify で EnvValue へ変換して共通判定へ渡す。
    // 非 UTF-8 は NonUtf8 として区別され、run() と同じ固定文言を警告する（値は非エコー）。
    let mut production: Vec<String> = production_config_errors(
        allow_mock_auth,
        validation.required_present,
        common::env::classify(lookup("PINATA_JWT")),
    )
    .into_iter()
    .map(str::to_string)
    .collect();

    // バックアップ設定の整合を production カテゴリ内へ追加する（check 数は据え置き）。
    // 本番相当（mock 無効）のみを診断し、backup-rotate.sh の fail-closed と同じ条件を
    // 警告化する。4 変数とも AUDIT_EXPECT_ROOT と同じく生値の非空判定（trim しない）で
    // `[[ -n "$var" ]]` と一致させる。ALERT_CMD は実行ファイルのパスなので生値のまま
    // `-x "$alert_cmd"` 相当を調べ、値は出力しない。
    if !allow_mock_auth {
        // DATABASE_URL の必須化もサーバー起動判定と同じ共通判定で診断する。
        // results.push は増やさず production カテゴリ内へマージする（check23 維持）。
        // 非 UTF-8 は classify が NonUtf8 へ変換し、固定文言のみを警告する（値は非エコー）。
        if let Some(message) = server::production_database_url_error(
            allow_mock_auth,
            common::env::classify(lookup("DATABASE_URL")),
        ) {
            production.push(message.to_string());
        }
        let recipient_set = lookup("BACKUP_GPG_RECIPIENT")
            .ok()
            .map(|value| !value.is_empty())
            .unwrap_or(false);
        let copy_set = lookup("OFFSITE_COPY_CMD")
            .ok()
            .map(|value| !value.is_empty())
            .unwrap_or(false);
        let dest_set = lookup("OFFSITE_DEST")
            .ok()
            .map(|value| !value.is_empty())
            .unwrap_or(false);
        if !recipient_set {
            production.push(BACKUP_RECIPIENT_UNSET_WARNING.to_string());
        }
        if copy_set != dest_set {
            production.push(BACKUP_OFFSITE_PAIR_WARNING.to_string());
        }
        if let Ok(alert) = lookup("ALERT_CMD")
            && !alert.is_empty()
            && !is_executable_file(alert.as_str())
        {
            production.push(BACKUP_ALERT_CMD_WARNING.to_string());
        }
    }
    results.push(("production", production));

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
    let oidc_any = OIDC_REQUIRED.iter().any(|name| lookup(name).is_ok());
    // run() / pin_to_ipfs と同じ共通ヘルパで本番相当かを判定する（単一情報源）。
    let pinata_production = pinata_jwt_production_like(common::env::classify(lookup("PINATA_JWT")));
    let mut mock_auth_errors = Vec::new();
    if allow_mock_auth
        && (pinata_production || oidc_any || lookup("JWT_RSA_PRIVATE_KEY_PEM").is_ok())
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
    let mut mock_auth_enabled_errors = if allow_mock_auth {
        vec![
            "ALLOW_MOCK_AUTH is enabled; acceptable for development but inappropriate for production"
                .to_string(),
        ]
    } else {
        Vec::new()
    };
    // 不正値は有効化せず、この既存カテゴリ内で固定文言の警告として計上する。
    if let Some(warning) = mock_auth_bool_warning {
        mock_auth_enabled_errors.push(warning);
    }
    results.push(("mock_auth_enabled", mock_auth_enabled_errors));

    // JWT: HS256 は 32 バイト以上、RS256 は PEM と検証キーの整合を検査する。
    let jwt_pem = lookup("JWT_RSA_PRIVATE_KEY_PEM").ok();
    let jwt_secret = lookup("JWT_SECRET").ok();
    let (require_kid, jwt_bool_warning) = bool_flag_with_warning(&lookup, "JWT_REQUIRE_KID", false);
    let mut jwt_errors = match jwt_keys_from_env(
        jwt_pem,
        jwt_secret,
        lookup("JWT_RSA_VERIFICATION_KEYS").ok().as_deref(),
        require_kid,
    ) {
        Ok(_) => Vec::new(),
        Err(error) => vec![error],
    };
    if let Some(warning) = jwt_bool_warning {
        jwt_errors.push(warning);
    }
    results.push(("jwt", jwt_errors));

    let mut secret_errors = Vec::new();
    if let Ok(secret) = lookup("JWT_SECRET")
        && let Err(error) = validate_jwt_secret(&secret)
    {
        secret_errors.push(error);
    }
    results.push(("jwt_secret", secret_errors));

    // IPFS: ゲートウェイは {cid} を含む http(s) URL テンプレートが必要。
    // ipfs_gateways() は不正値を除外して有効値だけ返すため、設定した非空エントリ数
    // との差を問題として計上する（値は出力しない）。未設定時は既定ゲートウェイを
    // 使うため加算しない。
    let gateways_env = lookup("IPFS_GATEWAYS").ok();
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
    if let Ok(value) = lookup("IPFS_TIMEOUT_SECS")
        && ipfs_timeout(Some(&value)).as_secs() != value.parse::<u64>().unwrap_or(0)
    {
        timeout_errors
            .push("IPFS_TIMEOUT_SECS is invalid or outside 1..=300; using default".to_string());
    }
    results.push(("ipfs_timeout", timeout_errors));

    let mut attempts_errors = Vec::new();
    if let Ok(value) = lookup("IPFS_MAX_ATTEMPTS")
        && ipfs_max_attempts(Some(&value)) != value.parse::<usize>().unwrap_or(0)
    {
        attempts_errors
            .push("IPFS_MAX_ATTEMPTS is invalid or outside 1..=5; using default".to_string());
    }
    results.push(("ipfs_max_attempts", attempts_errors));

    let mut oidc_timeout_errors = Vec::new();
    if let Ok(value) = lookup("OIDC_HTTP_TIMEOUT_SECS")
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
    if let Ok(value) = lookup("GRACEFUL_DRAIN_TIMEOUT_SECS")
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
    if let Ok(value) = lookup("MODERATION_FINALIZE_INTERVAL_SECS")
        && moderation_finalize_interval(Some(&value)).as_secs() != value.parse::<u64>().unwrap_or(0)
    {
        finalize_interval_errors.push(FINALIZE_WARNING.to_string());
    }
    results.push(("moderation_finalize_interval", finalize_interval_errors));

    // OIDC_CLOCK_SKEW_SECS: 共通バリデータが範囲外・不正値を既定 60 へ
    // フォールバックさせるため、実効値と生のパース値が異なれば警告する。
    // 値は出力せず、範囲内かどうかのみを診断する。
    let oidc_clock_skew_errors = match lookup("OIDC_CLOCK_SKEW_SECS").ok() {
        None => Vec::new(),
        Some(value) if value.parse::<u64>().ok() == Some(validation.clock_skew_secs) => Vec::new(),
        Some(_) => vec![format!(
            "OIDC_CLOCK_SKEW_SECS is outside {CLOCK_SKEW_MIN_SECS}..={CLOCK_SKEW_MAX_SECS}; using default {DEFAULT_CLOCK_SKEW_SECS} seconds"
        )],
    };
    results.push(("oidc_clock_skew", oidc_clock_skew_errors));

    // OIDC エンドポイント URL: https 必須。ALLOW_MOCK_AUTH 有効時だけ
    // localhost 系の http を許容する（共通バリデータの判定を共有する）。
    // 必須 7 変数が揃っているときに 5 エンドポイントを検証し、1 件でも不正なら
    // 固定文言で警告する。設定値は一切エコーしない。
    const OIDC_ENDPOINT_URL_WARNING: &str = "OIDC endpoints must be https (http is allowed only for localhost when ALLOW_MOCK_AUTH is enabled)";
    // 非 UTF-8 の必須値は共通バリデータが固定文言のハードエラーへ変換するため、
    // 同じカテゴリ内へマージする（results.push の数は据え置き、生バイトは出さない）。
    const OIDC_NON_UTF8_WARNING: &str = "OIDC configuration values must be valid UTF-8";
    // 必須値の trim 後空文字は from_env_with が起動拒否するハードエラーなので、
    // 共通バリデータと同じ固定文言をこのカテゴリへも警告化する（--strict の fail-open 解消）。
    // 未登録・未設定時は hard_errors に積まれないため従来どおり警告なし。値はエコーしない。
    const OIDC_EMPTY_VALUE_WARNING: &str = "OIDC configuration values must not be empty";
    let mut oidc_endpoint_urls_errors = Vec::new();
    if validation.hard_errors.contains(&OIDC_ENDPOINT_URL_WARNING) {
        oidc_endpoint_urls_errors.push(OIDC_ENDPOINT_URL_WARNING.to_string());
    }
    if validation.hard_errors.contains(&OIDC_NON_UTF8_WARNING) {
        oidc_endpoint_urls_errors.push(OIDC_NON_UTF8_WARNING.to_string());
    }
    if validation.hard_errors.contains(&OIDC_EMPTY_VALUE_WARNING) {
        oidc_endpoint_urls_errors.push(OIDC_EMPTY_VALUE_WARNING.to_string());
    }
    results.push(("oidc_endpoint_urls", oidc_endpoint_urls_errors));

    // OIDC_MAX_AUTH_AGE_SECS: 共通バリデータが 0 / 負 / 非数値 / オーバーフロー /
    // 上限超過を起動拒否相当のハードエラーとして返すため、その固定文言を警告化する。
    // 設定値は出力しない（上限値は定数なので警告文に示してよい）。
    // 必須 7 変数が揃っていれば mock 有効時も警告する。7 変数未揃いなら from_env_with は
    // Ok(None) を返して max_auth_age を見ないため、mock 有効時は警告しない
    // （本番相当では従来どおり警告する）。
    const MAX_AGE_GREATER: &str = "OIDC_MAX_AUTH_AGE_SECS must be greater than 0";
    const MAX_AGE_POSITIVE: &str = "OIDC_MAX_AUTH_AGE_SECS must be a positive integer";
    // 上限超過文言は oidc.rs の固定文言と完全一致させる（値はエコーしない）。
    let max_age_too_large =
        format!("OIDC_MAX_AUTH_AGE_SECS must not exceed {MAX_AUTH_AGE_MAX_SECS} seconds (30 days)");
    let oidc_max_auth_age_errors = if allow_mock_auth && !validation.required_present {
        Vec::new()
    } else {
        validation
            .hard_errors
            .iter()
            .filter(|error| {
                **error == MAX_AGE_GREATER
                    || **error == MAX_AGE_POSITIVE
                    || **error == max_age_too_large.as_str()
            })
            .map(|error| (*error).to_string())
            .collect()
    };
    results.push(("oidc_max_auth_age", oidc_max_auth_age_errors));

    // OIDC_COOKIE_SECURE: 共通バリデータの実効値（既定は Secure 付与 / fail-closed）。
    // mock 無効構成で偽値（0 / false）が設定されているなら警告する。値はエコーしない。
    let (_, cookie_bool_warning) = bool_flag_with_warning(&lookup, "OIDC_COOKIE_SECURE", true);
    let mut oidc_cookie_secure_errors = if !allow_mock_auth && !validation.cookie_secure {
        vec![
            "OIDC_COOKIE_SECURE is disabled; session, refresh, and OIDC state cookies will lack the Secure attribute"
                .to_string(),
        ]
    } else {
        Vec::new()
    };
    if let Some(warning) = cookie_bool_warning {
        oidc_cookie_secure_errors.push(warning);
    }
    results.push(("oidc_cookie_secure", oidc_cookie_secure_errors));

    // OIDC_REQUIRED_ACR / OIDC_REQUIRED_AMR: mock 無効時に未設定（または空要素
    // のみ）だと保証レベル（認証コンテキスト／認証方法）が一切強制されないため
    // 警告する。有効要素の有無は共通バリデータの acr_amr_present を使う。値は出力しない。
    let mut oidc_required_assurance_errors = Vec::new();
    if !allow_mock_auth {
        if !validation.acr_amr_present.0 {
            oidc_required_assurance_errors.push(
                "OIDC_REQUIRED_ACR is not set; no required authentication context class is enforced"
                    .to_string(),
            );
        }
        if !validation.acr_amr_present.1 {
            oidc_required_assurance_errors.push(
                "OIDC_REQUIRED_AMR is not set; no required authentication method is enforced"
                    .to_string(),
            );
        }
    }
    results.push(("oidc_required_assurance", oidc_required_assurance_errors));

    // 年齢ポリシー: 不正値は AgePolicy::from_env_with がエラーを返す。
    // プロセス環境を触らず lookup を渡すことで、CLI と同じ分岐をテストできる。
    let mut age_errors = Vec::new();
    // AgePolicy は std::env::var と同じ Result 契約のため、lookup をそのまま渡す。
    // 非 UTF-8 は Invalid として固定文言の警告へ変換され、生バイトはエコーしない。
    match common::age::AgePolicy::from_env_with(&lookup) {
        Ok(policy) => {
            // 手動フラグは廃止され、必須設定値の充足結果からゲートを判定する。
            // 明示設定必須なのに未充足なら年齢検証は fail closed（403）となるため警告する。
            if policy.require_explicit_configuration && !policy.configuration_satisfied() {
                age_errors.push(AGE_POLICY_UNSATISFIED_WARNING.to_string());
            }
        }
        Err(error) => age_errors.push(error),
    }

    // 年齢判定の基準日（TZ）診断を age_policy カテゴリ内へ追加する（check 数は据え置き）。
    // 未設定は UTC フォールバックとなり、対象地域のローカル日付と境界がずれ得る。
    // いずれも固定文言の警告のみで、既定は終了コードへ影響しない（--strict 時のみ失敗）。
    // read_env ベースの run() と同じ分類を経由する（非 UTF-8 は生バイトを保持せず、
    // 固定文言の未設定警告へ倒す。UTC フォールバック判定も run() と一致する）。
    let tz_value = common::env::classify(lookup("TZ"));
    let tz_raw = match tz_value {
        common::env::EnvValue::Utf8(value) => Some(value),
        common::env::EnvValue::Missing | common::env::EnvValue::NonUtf8 => None,
    };
    let tz_present = tz_raw
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if !tz_present {
        age_errors.push(TZ_UNSET_WARNING.to_string());
    }
    let regions_raw = lookup("AGE_TARGET_REGIONS").ok();
    let regions: Vec<&str> = regions_raw
        .as_deref()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .collect()
        })
        .unwrap_or_default();
    // run() と同じ共有関数を使う。未解決・範囲外は None なので UTC（0）へ倒し、
    // 既存の警告条件を維持する（Etc/GMT±25 等の境界差分を解消）。
    let offset_hours = policy_utc_offset_hours(tz_raw.as_deref()).unwrap_or(0);
    let jp_targeted = regions
        .iter()
        .any(|region| region.eq_ignore_ascii_case("JP"));
    if !regions.is_empty() && (offset_hours == 0 || (jp_targeted && offset_hours != 9)) {
        age_errors.push(TZ_REGION_MISMATCH_WARNING.to_string());
    }
    results.push(("age_policy", age_errors));

    // レート制限: 既定値へフォールバックしていても警告する。
    let mut rate_errors = Vec::new();
    if let Ok(value) = lookup("RATE_LIMIT_PER_MIN")
        && rate_limit_per_min(Some(&value)) != value.parse::<u32>().unwrap_or(0)
    {
        rate_errors.push("RATE_LIMIT_PER_MIN is outside 1..=600; using default".to_string());
    }
    if let Ok(value) = lookup("MAX_CONCURRENT_REQUESTS")
        && max_concurrent_requests(Some(&value)) != value.parse::<usize>().unwrap_or(0)
    {
        rate_errors.push("MAX_CONCURRENT_REQUESTS is outside 1..=256; using default".to_string());
    }
    results.push(("rate_limit", rate_errors));

    // IPv6-only が既定（SEC-01）。ALLOW_IPV4_LOCALHOST が真なら IPv4 loopback を
    // 許可するため本番では不適切。値（1/true）は出力せず有無のみ警告する。
    let (allow_ipv4_localhost, ipv4_bool_warning) =
        bool_flag_with_warning(&lookup, "ALLOW_IPV4_LOCALHOST", false);
    let mut ipv4_localhost_errors = if allow_ipv4_localhost {
        vec![
            "ALLOW_IPV4_LOCALHOST is enabled; production should stay IPv6-only (SEC-01)"
                .to_string(),
        ]
    } else {
        Vec::new()
    };
    if let Some(warning) = ipv4_bool_warning {
        ipv4_localhost_errors.push(warning);
    }
    results.push(("ipv4_localhost", ipv4_localhost_errors));

    // TRUSTED_PROXY_CIDRS は X-Forwarded-For（XFF）を信頼する設定。設定自体は
    // 正当だが本番では意図確認が必要。CIDR 等の設定値は出力せず有無のみ警告する。
    let trusted_proxy_errors = if lookup("TRUSTED_PROXY_CIDRS").is_ok() {
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
    let (expose_internal_errors, expose_bool_warning) =
        bool_flag_with_warning(&lookup, "EXPOSE_INTERNAL_ERRORS", false);
    let mut expose_internal_errors_errors = if !allow_mock_auth && expose_internal_errors {
        vec![
            "EXPOSE_INTERNAL_ERRORS is enabled; internal error details are exposed to clients"
                .to_string(),
        ]
    } else {
        Vec::new()
    };
    if let Some(warning) = expose_bool_warning {
        expose_internal_errors_errors.push(warning);
    }
    results.push(("expose_internal_errors", expose_internal_errors_errors));

    // ALLOWED_ORIGINS: 起動ゲートと同じ共有ヘルパ production_allowed_origins_error で判定する。
    // mock 無効時に実パーサ allowed_origins() が空ベクタなら全 CORS オリジンが拒否される。
    // オリジン値・生バイトは出力しない。既定は警告のみ（check23・文言・件数は不変）。
    const ALLOWED_ORIGINS_WARNING: &str = "ALLOWED_ORIGINS has no valid http(s) origin (unset, empty, or all entries invalid) while mock auth is disabled; all CORS origins are rejected";
    let allowed_origins_errors = if production_allowed_origins_error(
        allow_mock_auth,
        common::env::classify(lookup("ALLOWED_ORIGINS")),
    )
    .is_some()
    {
        vec![ALLOWED_ORIGINS_WARNING.to_string()]
    } else {
        Vec::new()
    };
    results.push(("allowed_origins", allowed_origins_errors));

    // AUDIT_EXPECT_ROOT: 本番相当（mock 無効）では別経路保存の Merkle root（hex）が必須。
    // 未設定・非 UTF-8・不正 hex（空白のみ・前後空白付きを含む）なら固定文言で警告する。
    // 値はエコーしない。backup-rotate.sh は未設定でも内部整合性チェックのみで動作する
    // 既存契約を維持し、doctor は既定で警告のみ（`--strict` のときだけ失敗）。
    // 判定は起動ゲートと同じ共有ヘルパ production_audit_expect_root_error へ委譲する。
    const AUDIT_EXPECT_ROOT_WARNING: &str =
        "AUDIT_EXPECT_ROOT is required in production (a hex Merkle root)";
    let audit_expect_root_errors = if production_audit_expect_root_error(
        allow_mock_auth,
        common::env::classify(lookup("AUDIT_EXPECT_ROOT")),
    )
    .is_some()
    {
        vec![AUDIT_EXPECT_ROOT_WARNING.to_string()]
    } else {
        Vec::new()
    };
    results.push(("audit_expect_root", audit_expect_root_errors));

    results
}

/// 本番用の薄いラッパー。実環境の読み取りはここだけに閉じ込める。
fn check_all() -> Vec<(&'static str, Vec<String>)> {
    check_all_with(env)
}

#[tokio::main]
async fn main() {
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

    let mut results = check_all();

    // FK 孤立行は起動時 fail-closed と同じ判定（server::foreign_key_doctor_warnings）
    // で production カテゴリへ警告としてマージする。results.push を増やさず check23 を維持。
    // 本番相当のみ警告し、開発（mock 有効）は起動時と同じく warn のみ（ここでは加えない）。
    let fk_warnings = server::foreign_key_doctor_warnings().await;
    if let Some((_, errors)) = results.iter_mut().find(|(name, _)| *name == "production") {
        errors.extend(fk_warnings);
    }

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
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    // --- テストヘルパ ------------------------------------------------------

    /// プログラム名を含む引数ベクタを組み立てる（`std::env::args()` 相当）。
    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("doctor")
            .chain(args.iter().copied())
            .map(String::from)
            .collect()
    }

    /// (名前, 値) の組から環境変数ルックアップを生成する。
    fn lookup_from<'a>(
        pairs: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> Result<String, std::env::VarError> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
                .ok_or(std::env::VarError::NotPresent)
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
        for value in ["0", "false"] {
            let results = check_all_with(lookup_from(&[("ALLOW_MOCK_AUTH", value)]));
            assert!(
                errors_for(&results, "mock_auth_enabled").is_empty(),
                "value={value}"
            );
        }
        // 受理値外（no / 空）は有効化せず、fail-closed の固定警告を1件だけ出す。
        // 警告文は設定名と安全側既定値のみで、不正値はエコーしない。
        const INVALID: &str =
            "ALLOW_MOCK_AUTH has an invalid boolean value; using false (fail-closed)";
        for value in ["no", ""] {
            let results = check_all_with(lookup_from(&[("ALLOW_MOCK_AUTH", value)]));
            let errors = errors_for(&results, "mock_auth_enabled");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            assert_eq!(errors[0], INVALID, "value={value}");
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

        // 非 UTF-8 の必須値は生バイトを一切警告へ出さず、固定文言だけを出す。
        let pairs: [(&str, &str); 2] = [("OIDC_CLIENT_SECRET", OIDC), ("ALLOW_MOCK_AUTH", "1")];
        let base = lookup_from(&pairs);
        let results = check_all_with(|name: &str| {
            if name == "OIDC_ISSUER" {
                Err(std::env::VarError::NotUnicode(
                    std::ffi::OsString::from_vec(vec![0xff]),
                ))
            } else {
                base(name)
            }
        });
        let errors = errors_for(&results, "oidc_endpoint_urls");
        assert_eq!(
            errors.len(),
            1,
            "非 UTF-8 は固定文言の警告になる: {errors:?}"
        );
        assert_eq!(
            errors[0], "OIDC configuration values must be valid UTF-8",
            "非 UTF-8 は固定文言の警告になる: {errors:?}"
        );
        for (_, errors) in &results {
            for error in errors {
                assert!(!error.contains('\u{fffd}'), "非 UTF-8 が漏洩: {error}");
                assert!(
                    !error.as_bytes().contains(&0xff),
                    "非 UTF-8 バイトが漏洩: {error}"
                );
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
        for value in ["0", "false"] {
            let results = check_all_with(lookup_from(&[("ALLOW_IPV4_LOCALHOST", value)]));
            assert!(
                errors_for(&results, "ipv4_localhost").is_empty(),
                "value={value}"
            );
        }
        // 受理値外は IPv4 を許可しない（fail-closed）ため有効化警告は出さず、
        // 既定カテゴリに不正値警告のみを1件出す（値はエコーしない）。
        const INVALID: &str =
            "ALLOW_IPV4_LOCALHOST has an invalid boolean value; using false (fail-closed)";
        for value in ["no", ""] {
            let results = check_all_with(lookup_from(&[("ALLOW_IPV4_LOCALHOST", value)]));
            let errors = errors_for(&results, "ipv4_localhost");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            assert_eq!(errors[0], INVALID, "value={value}");
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
        const TOO_LARGE: &str = "OIDC_MAX_AUTH_AGE_SECS must not exceed 2592000 seconds (30 days)";
        for (value, expected) in [
            ("0", GREATER),
            ("-1", POSITIVE),
            ("abc", POSITIVE),
            ("18446744073709551615", TOO_LARGE), // 上限超過
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
                // PINATA_JWT とバックアップ必須設定を揃え、production の警告を
                // OIDC 未設定だけに孤立させる（新規バックアップ警告の混入を防ぐ）。
                pairs.push(("PINATA_JWT", "real-production-jwt"));
                pairs.push(("BACKUP_GPG_RECIPIENT", "backup@example.jp"));
                // DB 必須化の警告を混入させないため絶対パスを明示（値はエコーしない）。
                pairs.push(("DATABASE_URL", "file:/var/lib/openillustration/local.db"));
                let results = check_all_with(lookup_from(&pairs));
                let errors = errors_for(&results, "production");
                assert_eq!(errors.len(), 1, "key={key}, raw={raw:?}, errors={errors:?}");
                // 固定文言なので空文字・空白の設定値は一切含まれない（値のエコーなし）。
                assert_eq!(errors[0], EXPECTED, "key={key}, raw={raw:?}");

                // 同じ空値ハードエラーは oidc_endpoint_urls にも固定文言で流れる。
                // 起動拒否なのに --strict が exit 0 になる fail-open を防ぐ。
                const EMPTY_WARNING: &str = "OIDC configuration values must not be empty";
                let oidc_errors = errors_for(&results, "oidc_endpoint_urls");
                assert_eq!(
                    oidc_errors.len(),
                    1,
                    "key={key}, raw={raw:?}, oidc_errors={oidc_errors:?}"
                );
                assert_eq!(oidc_errors[0], EMPTY_WARNING, "key={key}, raw={raw:?}");
                // 生値（空文字・空白・設定値）や 0xff は一切エコーしない。
                for error in oidc_errors {
                    assert!(!error.is_empty(), "空の警告文: key={key}, raw={raw:?}");
                    assert!(
                        !error.as_bytes().contains(&0xff),
                        "0xff が漏洩: key={key}, raw={raw:?}, error={error}"
                    );
                    for (_, configured) in &pairs {
                        if !configured.is_empty() {
                            assert!(
                                !error.contains(*configured),
                                "設定値が漏洩: key={key}, raw={raw:?}, configured={configured:?}, error={error}"
                            );
                        }
                    }
                }
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
        for value in ["0", "false"] {
            let results = check_all_with(lookup_from(&[("EXPOSE_INTERNAL_ERRORS", value)]));
            assert!(
                errors_for(&results, "expose_internal_errors").is_empty(),
                "value={value}"
            );
        }
        // 受理値外は内部詳細を公開せず（fail-closed）、不正値警告のみを1件出す。
        const INVALID: &str =
            "EXPOSE_INTERNAL_ERRORS has an invalid boolean value; using false (fail-closed)";
        for value in ["no", ""] {
            let results = check_all_with(lookup_from(&[("EXPOSE_INTERNAL_ERRORS", value)]));
            let errors = errors_for(&results, "expose_internal_errors");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            assert_eq!(errors[0], INVALID, "value={value}");
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
        const EXPECTED: &str = "ALLOWED_ORIGINS has no valid http(s) origin (unset, empty, or all entries invalid) while mock auth is disabled; all CORS origins are rejected";
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
        const EXPECTED: &str = "ALLOWED_ORIGINS has no valid http(s) origin (unset, empty, or all entries invalid) while mock auth is disabled; all CORS origins are rejected";

        // http(s) scheme のない不正要素は実パーサで全て捨てられ、空ベクタ→警告。
        // 旧「非空」判定では見逃していたケース。値はエコーしない。
        const INVALID: &str = "example.com,foo";
        let results = check_all_with(lookup_from(&[("ALLOWED_ORIGINS", INVALID)]));
        let errors = errors_for(&results, "allowed_origins");
        assert_eq!(errors.len(), 1, "errors={errors:?}");
        assert_eq!(errors[0], EXPECTED);
        assert!(!errors[0].contains(INVALID), "オリジン値が漏洩: {errors:?}");

        // 有効なオリジン（複数指定）は警告なし。null opaque origin は受け付けない。
        let value = "https://a.example,https://b.example";
        let results = check_all_with(lookup_from(&[("ALLOWED_ORIGINS", value)]));
        assert!(
            errors_for(&results, "allowed_origins").is_empty(),
            "value={value}"
        );
        let results = check_all_with(lookup_from(&[("ALLOWED_ORIGINS", "null")]));
        assert_eq!(
            errors_for(&results, "allowed_origins").len(),
            1,
            "null origin must be rejected"
        );

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
        for value in ["1", "true"] {
            let results = check_all_with(lookup_from(&[("OIDC_COOKIE_SECURE", value)]));
            assert!(
                errors_for(&results, "oidc_cookie_secure").is_empty(),
                "value={value}"
            );
        }
        // 受理値外（yes / 空）は Secure を外さない（fail-closed）ため無効化警告は出さず、
        // 不正値警告のみを1件出す（値はエコーしない）。
        const INVALID: &str =
            "OIDC_COOKIE_SECURE has an invalid boolean value; using true (fail-closed)";
        for value in ["yes", ""] {
            let results = check_all_with(lookup_from(&[("OIDC_COOKIE_SECURE", value)]));
            let errors = errors_for(&results, "oidc_cookie_secure");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            assert_eq!(errors[0], INVALID, "value={value}");
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

        // 共有ヘルパ production_audit_expect_root_error の直接アサーション（起動ゲートと同一判定）。
        assert_eq!(
            production_audit_expect_root_error(false, common::env::EnvValue::Missing),
            Some(EXPECTED)
        );
        let non_utf8 = common::env::classify(Err::<String, _>(std::env::VarError::NotUnicode(
            std::ffi::OsString::from_vec(vec![0xff]),
        )));
        assert_eq!(
            production_audit_expect_root_error(false, non_utf8),
            Some(EXPECTED)
        );
        for value in ["deadbeefcafebabe", "0123456789abcdef", "ABCDEF0123456789"] {
            assert_eq!(
                production_audit_expect_root_error(
                    false,
                    common::env::EnvValue::Utf8(value.to_string())
                ),
                None,
                "value={value}"
            );
        }
        // 前後空白付き hex は trim しないため不正（backup-rotate と統一）。
        assert_eq!(
            production_audit_expect_root_error(
                false,
                common::env::EnvValue::Utf8(PADDED.to_string())
            ),
            Some(EXPECTED)
        );
        // 開発（mock 有効）では値の有無を問わず受理（後方互換）。
        for value in [
            common::env::EnvValue::Missing,
            common::env::EnvValue::NonUtf8,
            common::env::EnvValue::Utf8(PADDED.to_string()),
        ] {
            assert_eq!(production_audit_expect_root_error(true, value), None);
        }
    }

    // --- 2h. 明示設定必須と必須設定値の充足結果の整合 ----------------------

    #[test]
    fn age_policy_flag_only_warns_without_disclosing_values() {
        // TZ を明示して基準日警告を除き、未充足警告だけを検証する。
        for value in ["1", "true", "TRUE"] {
            let results = check_all_with(lookup_from(&[
                ("AGE_REQUIRE_EXPLICIT_CONFIGURATION", value),
                ("TZ", "Asia/Tokyo"),
            ]));
            let errors = errors_for(&results, "age_policy");
            assert_eq!(errors.len(), 1, "value={value}, errors={errors:?}");
            assert_eq!(errors[0], AGE_POLICY_UNSATISFIED_WARNING, "value={value}");
        }
        // 非 UTF-8 は「未設定」ではなく Invalid として age_policy カテゴリへ
        // 受理値列挙の固定文言警告を出す。生バイトはエコーしない（件数は不変）。
        let results = check_all_with(|name: &str| match name {
            "AGE_REQUIRE_EXPLICIT_CONFIGURATION" => Err(std::env::VarError::NotUnicode(
                std::ffi::OsString::from_vec(vec![0xff]),
            )),
            "TZ" => Ok("Asia/Tokyo".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        });
        let errors = errors_for(&results, "age_policy");
        assert_eq!(errors.len(), 1, "errors={errors:?}");
        assert_eq!(
            errors[0],
            "AGE_REQUIRE_EXPLICIT_CONFIGURATION must be a boolean (1/0/true/false)"
        );
        assert!(!errors[0].contains('\u{fffd}'), "生バイトを文字列化しない");
        // 廃止した手動フラグは単独では警告を生じない（充足判定に使わない）。
        let results = check_all_with(lookup_from(&[
            ("AGE_POLICY_CONFIGURED", "1"),
            ("TZ", "Asia/Tokyo"),
        ]));
        assert!(errors_for(&results, "age_policy").is_empty());
    }

    #[test]
    fn age_policy_flag_with_explicit_values_has_no_warning() {
        // 必須設定値が 1 つでも充足していれば未充足警告は出ない。
        // TZ は対象地域 JP に合わせて明示し、基準日警告を混入させない。
        let scenarios: [&[(&str, &str)]; 3] = [
            &[
                ("AGE_REQUIRE_EXPLICIT_CONFIGURATION", "1"),
                ("AGE_DEFAULT_MINIMUM", "20"),
                ("TZ", "Asia/Tokyo"),
            ],
            &[
                ("AGE_REQUIRE_EXPLICIT_CONFIGURATION", "1"),
                ("AGE_TARGET_REGIONS", "JP"),
                ("TZ", "Asia/Tokyo"),
            ],
            &[
                ("AGE_REQUIRE_EXPLICIT_CONFIGURATION", "1"),
                ("AGE_COUNTRY_OVERRIDES", "JP=20"),
                ("TZ", "Asia/Tokyo"),
            ],
        ];
        for pairs in scenarios {
            let results = check_all_with(lookup_from(pairs));
            assert!(
                errors_for(&results, "age_policy").is_empty(),
                "pairs={pairs:?}"
            );
        }
    }

    #[test]
    fn age_policy_flag_unset_keeps_existing_behavior() {
        // TZ を明示して基準日警告を除き、明示設定必須が無効な場合の挙動を検証する。
        // require 未設定なら必須設定値が既定でも警告しない。
        let results = check_all_with(lookup_from(&[("TZ", "Asia/Tokyo")]));
        assert!(errors_for(&results, "age_policy").is_empty());
        // 設定値だけ明示されていても警告しない。
        let results = check_all_with(lookup_from(&[
            ("AGE_TARGET_REGIONS", "JP"),
            ("TZ", "Asia/Tokyo"),
        ]));
        assert!(errors_for(&results, "age_policy").is_empty());
        // require=1 のみなら必須設定値が未充足で、年齢判定は 403・doctor は警告する。
        let results = check_all_with(lookup_from(&[
            ("AGE_REQUIRE_EXPLICIT_CONFIGURATION", "1"),
            ("TZ", "Asia/Tokyo"),
        ]));
        assert_eq!(
            errors_for(&results, "age_policy"),
            vec![AGE_POLICY_UNSATISFIED_WARNING.to_string()]
        );
    }

    // --- 2i. バックアップ設定と基準日（TZ）の診断 ---------------------------

    #[test]
    fn production_backup_misconfiguration_warns_without_disclosing_values() {
        // 本番相当（mock 無効）で、BACKUP_GPG_RECIPIENT 未設定・OFFSITE 片方設定・
        // ALERT_CMD 実行不可を固定文言で警告する。設定値はエコーしない。
        const ALERT: &str = "/no/such/alert-command-for-doctor-test";
        let results = check_all_with(lookup_from(&[
            ("OFFSITE_COPY_CMD", "/usr/bin/true"),
            ("ALERT_CMD", ALERT),
        ]));
        let errors = errors_for(&results, "production");
        for expected in [
            BACKUP_RECIPIENT_UNSET_WARNING,
            BACKUP_OFFSITE_PAIR_WARNING,
            BACKUP_ALERT_CMD_WARNING,
        ] {
            assert!(
                errors.iter().any(|error| error.as_str() == expected),
                "missing: {expected}, errors={errors:?}"
            );
        }
        for error in errors {
            assert!(!error.contains(ALERT), "設定値が漏洩: {error}");
            assert!(!error.contains("/usr/bin/true"), "設定値が漏洩: {error}");
        }
    }

    #[test]
    fn production_backup_whitespace_only_values_warn_without_disclosing_values() {
        // backup-rotate.sh は `[[ -n "$var" ]]` / `-x "$alert_cmd"` で生値を判定するため、
        // 空白のみの値も「設定あり」として扱われ、実行不可なら警告になる（trim しない）。
        // 旧 trim 判定では警告されなかったケースの回帰防止。値はエコーしない。
        const WHITESPACE: &str = "   ";
        let results = check_all_with(lookup_from(&[
            ("OFFSITE_COPY_CMD", WHITESPACE),
            ("ALERT_CMD", WHITESPACE),
        ]));
        let errors = errors_for(&results, "production");
        // 空白のみの OFFSITE_COPY_CMD は設定あり、OFFSITE_DEST 未設定との不整合で警告。
        let pair = errors
            .iter()
            .find(|error| error.as_str() == BACKUP_OFFSITE_PAIR_WARNING)
            .unwrap_or_else(|| panic!("OFFSITE 不整合の警告が必要: {errors:?}"));
        assert!(!pair.contains(WHITESPACE), "設定値が漏洩: {pair}");
        // 空白のみの ALERT_CMD は生値のまま -x 判定され、実行不可で警告。
        let alert = errors
            .iter()
            .find(|error| error.as_str() == BACKUP_ALERT_CMD_WARNING)
            .unwrap_or_else(|| panic!("ALERT_CMD 実行不可の警告が必要: {errors:?}"));
        assert!(!alert.contains(WHITESPACE), "設定値が漏洩: {alert}");
    }

    #[test]
    fn age_policy_tz_unset_warns_without_disclosing_values() {
        // TZ 未設定は UTC フォールバックとなるため固定文言で警告する。
        let results = check_all_with(lookup_from(&[]));
        let errors = errors_for(&results, "age_policy");
        assert_eq!(errors.len(), 1, "errors={errors:?}");
        assert_eq!(errors[0], TZ_UNSET_WARNING);

        // 対象地域を設定したまま TZ が UTC なら不整合も警告する（値はエコーしない）。
        let results = check_all_with(lookup_from(&[
            ("AGE_TARGET_REGIONS", "JP"),
            ("TZ", "Etc/UTC"),
        ]));
        let errors = errors_for(&results, "age_policy");
        assert!(
            errors
                .iter()
                .any(|error| error.as_str() == TZ_REGION_MISMATCH_WARNING),
            "errors={errors:?}"
        );
        for error in errors {
            assert!(!error.contains("JP"), "設定値が漏洩: {error}");
            assert!(!error.contains("Etc/UTC"), "設定値が漏洩: {error}");
        }

        // 共有関数の符号規約（Asia/Tokyo = +9, Etc/GMT-9 = +9, Etc/GMT+5 = -5）。
        assert_eq!(policy_utc_offset_hours(Some("Asia/Tokyo")), Some(9));
        assert_eq!(policy_utc_offset_hours(Some("Etc/GMT-9")), Some(9));
        assert_eq!(policy_utc_offset_hours(Some("Etc/GMT+5")), Some(-5));
        // 範囲外（±25）と数値なしの Etc/GMT は None（→ doctor は 0 として警告条件を維持）。
        // run() と doctor が同じ Option を共有するため境界で判定が分岐しない。
        for tz in ["Etc/GMT-25", "Etc/GMT+25", "Etc/GMT"] {
            assert_eq!(policy_utc_offset_hours(Some(tz)), None, "tz={tz}");
            let results = check_all_with(lookup_from(&[("AGE_TARGET_REGIONS", "JP"), ("TZ", tz)]));
            let errors = errors_for(&results, "age_policy");
            assert!(
                errors
                    .iter()
                    .any(|error| error.as_str() == TZ_REGION_MISMATCH_WARNING),
                "tz={tz}, errors={errors:?}"
            );
        }
    }

    #[test]
    fn complete_backup_and_tz_configuration_adds_no_new_warnings() {
        // 実行可能なパスはテストバイナリ自身を使い、ファイル生成なしで -x 判定を通す。
        let executable = std::env::current_exe()
            .expect("test binary path")
            .to_string_lossy()
            .into_owned();
        let mut pairs: Vec<(&str, &str)> = oidc_complete_pairs("https://issuer.example.jp");
        pairs.extend([
            ("PINATA_JWT", "real-production-jwt"),
            ("AUDIT_EXPECT_ROOT", "deadbeefcafebabe"),
            ("ALLOWED_ORIGINS", "https://example.jp"),
            ("OIDC_REQUIRED_ACR", "urn:example:acr"),
            ("OIDC_REQUIRED_AMR", "pwd,mfa"),
            ("BACKUP_GPG_RECIPIENT", "backup@example.jp"),
            ("OFFSITE_COPY_CMD", executable.as_str()),
            ("OFFSITE_DEST", "/var/backups/offsite"),
            ("ALERT_CMD", executable.as_str()),
            ("TZ", "Asia/Tokyo"),
            ("AGE_TARGET_REGIONS", "JP"),
            // 本番相当では DATABASE_URL も必須。絶対パスを明示して空期待を維持する。
            ("DATABASE_URL", "file:/var/lib/openillustration/local.db"),
        ]);
        let results = check_all_with(lookup_from(&pairs));
        assert!(
            errors_for(&results, "production").is_empty(),
            "production={:?}",
            errors_for(&results, "production")
        );
        assert!(
            errors_for(&results, "age_policy").is_empty(),
            "age_policy={:?}",
            errors_for(&results, "age_policy")
        );
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
