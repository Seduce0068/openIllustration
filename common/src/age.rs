use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use thiserror::Error;

/// DEC-02: 年齢判定ポリシー（有効設定値は常に公開可能な非秘密情報）
///
/// 既定値は安全側の一律基準（21歳）とし、運用者が地域・基準に応じて設定で上書きできる。
/// このモジュールは法的助言・法的適合の断定を提供しない。対象地域・閾値・
/// 信頼できる属性ソース等の最終判断と専門家レビューは運用者/意思決定者の責務である。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgeEnforcementMode {
    /// アプリが受け取った生年月日・国コードによる自己申告ベースの暫定判定。
    /// DEC-01 の信頼できる属性検証が利用可能になるまでの運用モード。
    SelfReportedDobCountry,
    /// 信頼できる属性（JPKI/OID4VP/IdP 等）による検証済み年齢属性のみを受け付ける
    /// 方針を表す。現行実装では検証済み属性入力経路が未提供のため fail closed とする。
    TrustedAttributeRequired,
}

impl fmt::Display for AgeEnforcementMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfReportedDobCountry => write!(f, "self_reported_dob_country"),
            Self::TrustedAttributeRequired => write!(f, "trusted_attribute_required"),
        }
    }
}

/// 検証済み年齢属性のペイロード（DEC-01 連携準備用・外部仕様待ち）。
///
/// 現行の年齢判定には結線しておらず、この型の値が署名検証・発行者検証済みを意味しない。
/// 未知フィールドを拒否し、自己申告の `birth_date` / `country_code` を
/// 「検証済み属性」として誤って受け付けないようにする。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct VerifiedAgeAttribute {
    /// 属性の発行者識別子（JPKI / OID4VP / IdP 等、外部仕様待ち）。
    pub issuer: String,
    /// 署名がカバーする対象（属性クレームの識別子、外部仕様待ち）。
    pub signed_subject: String,
    /// 提示ごとのリプレイ防止 nonce（検証手順は外部仕様待ち）。
    pub nonce: String,
}

/// `VerifiedAgeAttribute` の serde フィクスチャ（秘密情報を含まないテスト値）。
pub const VERIFIED_AGE_ATTRIBUTE_FIXTURE: &str = concat!(
    r#"{"issuer":"jpki-test","signed_subject":"age-over-20","#,
    r#""nonce":"fixture-nonce-0001"}"#
);

/// 公開可能な年齢判定の有効設定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgePolicy {
    /// 既定の最低年齢。未指定国や上書きのない国に適用される。
    pub default_minimum_age: u32,
    /// 国コード（大文字）ごとの最低年齢上書き。
    pub country_minimums: BTreeMap<String, u32>,
    /// 運用者が対象とする地域・運用範囲の宣言（メタデータ、公開可）。
    pub target_regions: Vec<String>,
    /// 運用者が明示的に設定済みと宣言したか（`AGE_POLICY_CONFIGURED`）。
    /// 単独では必須設定値の充足を意味しない（doctor の `age_policy` 診断で警告）。
    pub operator_configured: bool,
    /// 明示設定がない場合に fail closed するか（`AGE_REQUIRE_EXPLICIT_CONFIGURATION`）。
    pub require_explicit_configuration: bool,
    /// 判定モード。
    pub enforcement_mode: AgeEnforcementMode,
    /// 設定値は常に公開される前提であることを示す固定フラグ。
    pub settings_always_public: bool,
    /// 実装上の注意（公開可）。
    pub notes: Vec<String>,
}

impl Default for AgePolicy {
    fn default() -> Self {
        Self {
            // DEC-02 採用方針: 保守的一律基準（世界で高い成人年齢の一つである 21 歳）を
            // 安全側の既定値とする。これは運用上の保守的既定であり、全法域の法的事実や
            // 法的適合を断定するものではない。運用者は対象地域の基準に応じて設定で調整する。
            default_minimum_age: 21,
            country_minimums: BTreeMap::new(),
            target_regions: Vec::new(),
            operator_configured: false,
            require_explicit_configuration: false,
            enforcement_mode: AgeEnforcementMode::SelfReportedDobCountry,
            settings_always_public: true,
            notes: vec![
                "Effective age policy settings are always public by design."
                    .to_string(),
                "Default minimum age is a conservative baseline (21) and is not a claim of universal legal compliance."
                    .to_string(),
                "Operators are responsible for configuring thresholds/regions according to applicable local rules and for obtaining expert review where required."
                    .to_string(),
                "Self-reported birth date/country input is not a trusted verified age attribute; see DEC-01."
                    .to_string(),
            ],
        }
    }
}

impl AgePolicy {
    /// 環境変数から公開可能な年齢判定設定を読み込む。
    ///
    /// 不正な値は安全側に倒してエラーとする（起動失敗させる用途を想定）。
    pub fn from_env() -> Result<Self, String> {
        // 欠落・非 Unicode の環境変数は従来どおり「未設定」として扱う。
        Self::from_env_with(|key| std::env::var(key).ok())
    }

    /// 依存注入版の `from_env`。
    ///
    /// `getter` は環境変数名を受け取り、設定されていれば値を `Some` で返す。
    /// プロセス環境を直接変更せずに分岐・境界値をテストするための入口。
    pub fn from_env_with(getter: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut policy = Self::default();

        if let Some(value) = getter("AGE_DEFAULT_MINIMUM") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                let parsed = trimmed
                    .parse::<u32>()
                    .map_err(|_| "AGE_DEFAULT_MINIMUM must be a positive integer".to_string())?;
                policy.default_minimum_age = validate_minimum_age(parsed, "AGE_DEFAULT_MINIMUM")?;
            }
        }

        if let Some(value) = getter("AGE_COUNTRY_OVERRIDES") {
            policy.country_minimums = parse_country_minimums(&value)?;
        }

        if let Some(value) = getter("AGE_TARGET_REGIONS") {
            policy.target_regions = parse_target_regions(&value)?;
        }

        if let Some(value) = getter("AGE_ENFORCEMENT_MODE") {
            let normalized = value.trim().to_ascii_lowercase();
            policy.enforcement_mode = match normalized.as_str() {
                "" | "self_reported" | "self_reported_dob_country" => {
                    AgeEnforcementMode::SelfReportedDobCountry
                }
                "trusted_attribute_required" | "trusted_attribute" => {
                    AgeEnforcementMode::TrustedAttributeRequired
                }
                _ => {
                    // 受理値は入力値をエコーせず、別名と既定（空）を含めて列挙する。
                    return Err(
                        "AGE_ENFORCEMENT_MODE must be one of: empty (default/self_reported), \
                         self_reported, self_reported_dob_country, \
                         trusted_attribute, trusted_attribute_required"
                            .to_string(),
                    );
                }
            };
        }

        if let Some(value) = getter("AGE_REQUIRE_EXPLICIT_CONFIGURATION") {
            policy.require_explicit_configuration = parse_bool_env(&value).ok_or_else(|| {
                "AGE_REQUIRE_EXPLICIT_CONFIGURATION must be a boolean (1/0/true/false)".to_string()
            })?;
        }

        if let Some(value) = getter("AGE_POLICY_CONFIGURED") {
            policy.operator_configured = parse_bool_env(&value).ok_or_else(|| {
                "AGE_POLICY_CONFIGURED must be a boolean (1/0/true/false)".to_string()
            })?;
        }

        policy.normalize();
        Ok(policy)
    }

    fn normalize(&mut self) {
        self.target_regions.retain(|r| !r.trim().is_empty());
        self.target_regions.sort();
        self.target_regions.dedup();
        self.notes.sort();
        self.notes.dedup();
        if self.enforcement_mode == AgeEnforcementMode::TrustedAttributeRequired {
            let note = "Trusted-attribute-only enforcement is declared, but this implementation has no verified age-attribute input path; requests requiring age validation fail closed."
                .to_string();
            if !self.notes.contains(&note) {
                self.notes.push(note);
                self.notes.sort();
            }
        }
    }

    /// 指定国コードに対する有効な最低年齢。
    pub fn minimum_age_for(&self, country_code: &str) -> u32 {
        let normalized = normalize_country_code(country_code);
        self.country_minimums
            .get(&normalized)
            .copied()
            .unwrap_or(self.default_minimum_age)
    }

    /// 公開用の有効設定値（同一構造体。全フィールドが非秘密の前提）。
    pub fn public_settings(&self) -> &Self {
        self
    }
}

/// 年齢しきい値を検証する。0 を許すと年齢ゲートが無効化されるため拒否する。
fn validate_minimum_age(age: u32, label: &str) -> Result<u32, String> {
    if age == 0 {
        return Err(format!("{label} must be at least 1"));
    }
    Ok(age)
}

/// 真偽値はドキュメントどおり `1`/`true` と `0`/`false` のみ受理する。
/// `yes`/`on`/`no`/`off` のような別名を受理すると、ドキュメント外の設定が
/// 黙って有効化される fail-open を招くため拒否する（trim・小文字化は維持）。
fn parse_bool_env(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

fn parse_csv_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn normalize_country_code(code: &str) -> String {
    code.trim().to_ascii_uppercase()
}

/// `AGE_TARGET_REGIONS` を大文字化・検証する。
///
/// `AGE_COUNTRY_OVERRIDES` と同じく英字2文字（ASCII alphabet のみ）のみ受理し、
/// 不正値は fail closed で起動拒否する。エラー文言には入力値をエコーしない。
fn parse_target_regions(value: &str) -> Result<Vec<String>, String> {
    let mut regions = parse_csv_list(value);
    for region in &mut regions {
        *region = normalize_country_code(region);
        if region.len() != 2 || !region.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(
                "AGE_TARGET_REGIONS must be comma-separated two-letter region codes (ASCII letters only)"
                    .to_string(),
            );
        }
    }
    Ok(regions)
}

fn parse_country_minimums(value: &str) -> Result<BTreeMap<String, u32>, String> {
    let mut map = BTreeMap::new();
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (code, age) = part.split_once('=').ok_or_else(|| {
            "AGE_COUNTRY_OVERRIDES must be comma-separated entries like JP=20,US=21".to_string()
        })?;
        let code = normalize_country_code(code);
        if code.len() != 2 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(format!(
                "AGE_COUNTRY_OVERRIDES has invalid country code '{code}'; expected two ASCII letters"
            ));
        }
        let age = age
            .trim()
            .parse::<u32>()
            .map_err(|_| format!("AGE_COUNTRY_OVERRIDES has invalid age for '{code}'"))?;
        let age = validate_minimum_age(age, &format!("AGE_COUNTRY_OVERRIDES age for '{code}'"))?;
        map.insert(code, age);
    }
    Ok(map)
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum AgeValidationError {
    #[error("Invalid birth date format: {0}")]
    InvalidBirthDate(String),
    #[error("Under minimum age for country {country}: age {age}, required {threshold}")]
    UnderAge {
        country: String,
        age: u32,
        threshold: u32,
    },
    #[error("Age policy requires explicit operator configuration")]
    ConfigurationRequired,
    #[error(
        "Trusted age attribute verification is required by policy, but no verified attribute input is available"
    )]
    TrustedAttributeRequired,
}

/// 満年齢を計算する（基準日は呼び出し側で渡す）
pub fn calculate_age(birth_date: &str, today: NaiveDate) -> Result<u32, AgeValidationError> {
    let birth = NaiveDate::parse_from_str(birth_date, "%Y-%m-%d")
        .map_err(|e| AgeValidationError::InvalidBirthDate(e.to_string()))?;

    use chrono::Datelike;
    let mut age = today.year() - birth.year();
    let (birth_month, birth_day) = (birth.month(), birth.day());
    let (today_month, today_day) = (today.month(), today.day());
    if (today_month, today_day) < (birth_month, birth_day) {
        age -= 1;
    }
    if age < 0 {
        // 未来日や異常値は birth > today 相当として扱わず、計算上0未満にならないよう防ぐ
        // （実際には上の分岐で年差が負になるケースは限定的）
        age = 0;
    }
    Ok(age as u32)
}

/// 互換用: 既定ポリシー（保守的一律 21 歳、国別上書きなし）で判定する。
pub fn is_age_allowed(
    birth_date: &str,
    country_code: &str,
    today: NaiveDate,
) -> Result<(), AgeValidationError> {
    let policy = AgePolicy::default();
    is_age_allowed_with_policy(&policy, birth_date, country_code, today)
}

/// ポリシーに基づいて年齢を判定する。
pub fn is_age_allowed_with_policy(
    policy: &AgePolicy,
    birth_date: &str,
    country_code: &str,
    today: NaiveDate,
) -> Result<(), AgeValidationError> {
    if policy.require_explicit_configuration && !policy.operator_configured {
        return Err(AgeValidationError::ConfigurationRequired);
    }
    if policy.enforcement_mode == AgeEnforcementMode::TrustedAttributeRequired {
        return Err(AgeValidationError::TrustedAttributeRequired);
    }

    let age = calculate_age(birth_date, today)?;
    let country = normalize_country_code(country_code);
    let threshold = policy.minimum_age_for(&country);
    if age < threshold {
        return Err(AgeValidationError::UnderAge {
            country,
            age,
            threshold,
        });
    }
    Ok(())
}

/// 互換用: 既定ポリシーにおける国別閾値（上書きなし）。
pub fn legal_age_threshold(_country_code: &str) -> u32 {
    AgePolicy::default().default_minimum_age
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn default_policy_is_conservative_21() {
        let policy = AgePolicy::default();
        assert_eq!(policy.default_minimum_age, 21);
        assert!(policy.country_minimums.is_empty());
        assert!(policy.settings_always_public);
    }

    #[test]
    fn jp_adult_passes_under_default() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        assert!(is_age_allowed("2000-01-01", "JP", today).is_ok());
    }

    #[test]
    fn jp_minor_rejected_under_default() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        let err = is_age_allowed("2010-06-05", "JP", today).unwrap_err();
        assert!(matches!(err, AgeValidationError::UnderAge { .. }));
    }

    #[test]
    fn us_requires_21_under_default() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        assert!(is_age_allowed("2008-01-01", "US", today).is_err());
        assert!(is_age_allowed("2000-01-01", "US", today).is_ok());
    }

    #[test]
    fn operator_override_allows_local_threshold() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        let mut policy = AgePolicy::default();
        policy.country_minimums.insert("JP".to_string(), 18);
        assert!(is_age_allowed_with_policy(&policy, "2007-01-01", "JP", today).is_ok());
        // US など未指定国は既定の保守的閾値が維持される
        assert!(is_age_allowed_with_policy(&policy, "2007-01-01", "US", today).is_err());
    }

    #[test]
    fn require_explicit_configuration_fails_closed() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        let policy = AgePolicy {
            require_explicit_configuration: true,
            operator_configured: false,
            ..AgePolicy::default()
        };
        let err = is_age_allowed_with_policy(&policy, "1990-01-01", "JP", today).unwrap_err();
        assert_eq!(err, AgeValidationError::ConfigurationRequired);
    }

    #[test]
    fn trusted_attribute_mode_fails_closed_without_verified_input() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        let policy = AgePolicy {
            enforcement_mode: AgeEnforcementMode::TrustedAttributeRequired,
            ..AgePolicy::default()
        };
        let err = is_age_allowed_with_policy(&policy, "1990-01-01", "JP", today).unwrap_err();
        assert_eq!(err, AgeValidationError::TrustedAttributeRequired);
    }

    #[test]
    fn birthday_boundary_at_threshold_is_allowed() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        // 閾値 21 歳の誕生日当日は通過し、前日は 20 歳として拒否される
        assert!(is_age_allowed("2005-06-05", "JP", today).is_ok());
        let err = is_age_allowed("2005-06-06", "JP", today).unwrap_err();
        assert!(matches!(err, AgeValidationError::UnderAge { .. }));
    }

    #[test]
    fn future_birth_date_is_treated_as_zero_age() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        // 未来の生年月日は年齢 0 として扱われ、既定閾値で拒否される
        let err = is_age_allowed("2030-01-01", "JP", today).unwrap_err();
        assert!(matches!(err, AgeValidationError::UnderAge { age: 0, .. }));
    }

    #[test]
    fn invalid_birth_dates_are_rejected() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        for birth in ["", "not-a-date", "2000-13-01", "2000-02-30"] {
            let err = is_age_allowed(birth, "JP", today).unwrap_err();
            assert!(
                matches!(err, AgeValidationError::InvalidBirthDate(_)),
                "birth date {birth:?} must be invalid"
            );
        }
    }

    #[test]
    fn country_code_matching_is_case_insensitive_and_trimmed() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        let mut policy = AgePolicy::default();
        policy.country_minimums.insert("JP".to_string(), 18);
        assert!(is_age_allowed_with_policy(&policy, "2007-01-01", "jp", today).is_ok());
        assert!(is_age_allowed_with_policy(&policy, "2007-01-01", " jp ", today).is_ok());
        // 正規化後も未指定国は既定の保守的閾値が維持される
        assert!(is_age_allowed_with_policy(&policy, "2007-01-01", "us", today).is_err());
    }

    #[test]
    fn parse_country_overrides() {
        let parsed = parse_country_minimums("jp=20, US = 21, de=18").unwrap();
        assert_eq!(parsed.get("JP"), Some(&20));
        assert_eq!(parsed.get("US"), Some(&21));
        assert_eq!(parsed.get("DE"), Some(&18));
    }

    #[test]
    fn parse_country_overrides_rejects_invalid() {
        assert!(parse_country_minimums("JPN=20").is_err());
        assert!(parse_country_minimums("JP=xx").is_err());
        assert!(parse_country_minimums("JP").is_err());
        // 0 は年齢ゲートを無効化するため拒否する。
        assert!(parse_country_minimums("JP=0").is_err());
        assert!(validate_minimum_age(0, "AGE_DEFAULT_MINIMUM").is_err());
        assert_eq!(validate_minimum_age(1, "AGE_DEFAULT_MINIMUM").unwrap(), 1);
    }

    /// テスト用の環境変数 getter。呼び出し元のエントリ列を借用し、同じ文の中で使う。
    fn getter_from<'a>(
        entries: &'a [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            entries
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn from_env_with_returns_conservative_defaults_when_unset() {
        let policy = AgePolicy::from_env_with(getter_from(&[])).unwrap();
        assert_eq!(policy.default_minimum_age, 21);
        assert!(policy.country_minimums.is_empty());
        assert!(policy.target_regions.is_empty());
        assert!(!policy.operator_configured);
        assert!(!policy.require_explicit_configuration);
        assert_eq!(
            policy.enforcement_mode,
            AgeEnforcementMode::SelfReportedDobCountry
        );
        assert!(policy.settings_always_public);
        // from_env 経由では normalize() が notes をソートするため、内容を集合として比較する。
        let mut expected_notes = AgePolicy::default().notes;
        expected_notes.sort();
        assert_eq!(policy.notes, expected_notes);
    }

    #[test]
    fn from_env_with_rejects_zero_default_minimum() {
        // 0 を許すと年齢ゲートが実質無効化されるため、起動前に拒否する。
        for raw in ["0", " 0 "] {
            let err = AgePolicy::from_env_with(getter_from(&[("AGE_DEFAULT_MINIMUM", raw)]))
                .expect_err("zero minimum must be rejected");
            assert!(
                err.contains("AGE_DEFAULT_MINIMUM"),
                "error must name the variable: {err}"
            );
        }
        assert_eq!(
            AgePolicy::from_env_with(getter_from(&[("AGE_DEFAULT_MINIMUM", "1")]))
                .unwrap()
                .default_minimum_age,
            1
        );
    }

    #[test]
    fn from_env_with_rejects_invalid_boolean_configuration_flag() {
        // 不正値は受け入れて緩めるのではなく、エラーとして起動を失敗させる（fail closed）。
        let err = AgePolicy::from_env_with(getter_from(&[(
            "AGE_REQUIRE_EXPLICIT_CONFIGURATION",
            "maybe",
        )]))
        .expect_err("invalid boolean must be rejected");
        assert!(
            err.contains("AGE_REQUIRE_EXPLICIT_CONFIGURATION"),
            "error must name the variable: {err}"
        );

        let enabled = AgePolicy::from_env_with(getter_from(&[(
            "AGE_REQUIRE_EXPLICIT_CONFIGURATION",
            "true",
        )]))
        .unwrap();
        assert!(enabled.require_explicit_configuration);
        let disabled =
            AgePolicy::from_env_with(getter_from(&[("AGE_REQUIRE_EXPLICIT_CONFIGURATION", "0")]))
                .unwrap();
        assert!(!disabled.require_explicit_configuration);
    }

    #[test]
    fn from_env_with_parses_trusted_attribute_required_mode() {
        for raw in ["trusted_attribute_required", " TRUSTED_ATTRIBUTE_REQUIRED "] {
            let policy =
                AgePolicy::from_env_with(getter_from(&[("AGE_ENFORCEMENT_MODE", raw)])).unwrap();
            assert_eq!(
                policy.enforcement_mode,
                AgeEnforcementMode::TrustedAttributeRequired
            );
        }
    }

    #[test]
    fn parse_bool_env_accepts_only_documented_values() {
        // 有効値: 1/true。無効値: 0/false。trim と小文字化は維持される。
        for raw in ["1", "true", " TRUE ", "0", "false", " FALSE "] {
            assert!(parse_bool_env(raw).is_some(), "{raw:?} must be accepted");
        }
        assert_eq!(parse_bool_env("1"), Some(true));
        assert_eq!(parse_bool_env("true"), Some(true));
        assert_eq!(parse_bool_env("0"), Some(false));
        assert_eq!(parse_bool_env("false"), Some(false));
        assert_eq!(parse_bool_env(" TRUE "), Some(true));
        assert_eq!(parse_bool_env(" FALSE "), Some(false));
    }

    #[test]
    fn parse_bool_env_rejects_undocumented_aliases() {
        // yes/on/no/off はドキュメント外。受理すると fail-open を招くため拒否する。
        for raw in ["yes", "on", "no", "off", "YES", " On ", "NO ", "maybe", ""] {
            assert_eq!(parse_bool_env(raw), None, "{raw:?} must be rejected");
        }
    }

    #[test]
    fn from_env_with_rejects_undocumented_boolean_alias_fail_open() {
        // AGE_POLICY_CONFIGURED=yes のような設定が黙って有効化されないことを保証する。
        for raw in ["yes", "on", "no", "off"] {
            let err = AgePolicy::from_env_with(getter_from(&[("AGE_POLICY_CONFIGURED", raw)]))
                .unwrap_err();
            assert!(
                err.contains("AGE_POLICY_CONFIGURED"),
                "error must name the variable: {err}"
            );
        }
    }

    #[test]
    fn from_env_with_accepts_enforcement_mode_aliases() {
        // 別名と既定（空）が受理され、対応するモードへ正規化されること。
        for (raw, expected) in [
            ("", AgeEnforcementMode::SelfReportedDobCountry),
            ("self_reported", AgeEnforcementMode::SelfReportedDobCountry),
            (
                " SELF_REPORTED ",
                AgeEnforcementMode::SelfReportedDobCountry,
            ),
            (
                "self_reported_dob_country",
                AgeEnforcementMode::SelfReportedDobCountry,
            ),
            (
                "trusted_attribute",
                AgeEnforcementMode::TrustedAttributeRequired,
            ),
            (
                "trusted_attribute_required",
                AgeEnforcementMode::TrustedAttributeRequired,
            ),
        ] {
            let policy =
                AgePolicy::from_env_with(getter_from(&[("AGE_ENFORCEMENT_MODE", raw)])).unwrap();
            assert_eq!(policy.enforcement_mode, expected, "raw={raw:?}");
        }
    }

    #[test]
    fn enforcement_mode_error_lists_all_accepted_values_without_echoing_input() {
        let invalid = "totally_invalid_mode";
        let err = AgePolicy::from_env_with(getter_from(&[("AGE_ENFORCEMENT_MODE", invalid)]))
            .expect_err("invalid mode must be rejected");
        // 別名と既定（空）を含む全受理値が列挙されていること。
        for accepted in [
            "empty",
            "default",
            "self_reported",
            "self_reported_dob_country",
            "trusted_attribute",
            "trusted_attribute_required",
        ] {
            assert!(
                err.contains(accepted),
                "error must list {accepted:?}: {err}"
            );
        }
        // 入力値自体はエコーしない（設定名のみで機微情報を扱わない）。
        assert!(!err.contains(invalid), "error must not echo input: {err}");
    }

    #[test]
    fn from_env_with_normalizes_country_override_codes() {
        let policy =
            AgePolicy::from_env_with(getter_from(&[("AGE_COUNTRY_OVERRIDES", "jp=18, US = 21")]))
                .unwrap();
        assert_eq!(policy.country_minimums.get("JP"), Some(&18));
        assert_eq!(policy.country_minimums.get("US"), Some(&21));
        assert_eq!(policy.minimum_age_for("jp"), 18);
        assert_eq!(policy.minimum_age_for(" us "), 21);
        // 上書きのない国は保守的既定値のまま。
        assert_eq!(policy.minimum_age_for("DE"), 21);
    }

    #[test]
    fn from_env_with_rejects_invalid_country_code_length() {
        let err = AgePolicy::from_env_with(getter_from(&[("AGE_COUNTRY_OVERRIDES", "JPN=20")]))
            .expect_err("three-letter country code must be rejected");
        assert!(
            err.contains("JPN"),
            "error must name the offending code: {err}"
        );
        assert!(
            AgePolicy::from_env_with(getter_from(&[("AGE_COUNTRY_OVERRIDES", "J1=20",)])).is_err()
        );
    }

    #[test]
    fn from_env_with_rejects_zero_country_override_age() {
        let err = AgePolicy::from_env_with(getter_from(&[("AGE_COUNTRY_OVERRIDES", "JP=0")]))
            .expect_err("zero country override age must be rejected");
        assert!(err.contains("JP"), "error must name the country: {err}");
    }

    #[test]
    fn from_env_with_trims_and_deduplicates_target_regions() {
        let policy = AgePolicy::from_env_with(getter_from(&[(
            "AGE_TARGET_REGIONS",
            " JP , US ,, DE , JP ",
        )]))
        .unwrap();
        assert_eq!(policy.target_regions, vec!["DE", "JP", "US"]);
    }

    #[test]
    fn from_env_with_uppercases_lowercase_target_regions() {
        // 既存の小文字 jp/us は大文字化して受理し、重複も除去する（互換維持）。
        let entries = [("AGE_TARGET_REGIONS", "jp,us,jp")];
        let policy = AgePolicy::from_env_with(getter_from(&entries)).unwrap();
        assert_eq!(policy.target_regions, vec!["JP", "US"]);
    }

    #[test]
    fn from_env_with_rejects_non_two_letter_target_regions() {
        // 英字2文字（ASCII alphabet のみ）でない値は fail closed で起動拒否。
        for raw in ["jpn", "12", "j-P", "J1", "日本"] {
            let entries = [("AGE_TARGET_REGIONS", raw)];
            let err = AgePolicy::from_env_with(getter_from(&entries))
                .expect_err("invalid region code must be rejected");
            assert!(
                err.contains("AGE_TARGET_REGIONS"),
                "error must name the variable: {err}"
            );
        }
    }

    #[test]
    fn target_regions_error_does_not_echo_input() {
        // エラー文言には設定名のみを含め、入力値はエコーしない。
        for raw in ["jpn", "12", "j-P", "J1", "日本"] {
            let entries = [("AGE_TARGET_REGIONS", raw)];
            let err = AgePolicy::from_env_with(getter_from(&entries))
                .expect_err("invalid region code must be rejected");
            assert!(!err.contains(raw), "error must not echo input: {err}");
        }
    }

    #[test]
    fn verified_age_attribute_fixture_roundtrips_and_denies_self_reported_fields() {
        let attribute: VerifiedAgeAttribute =
            serde_json::from_str(VERIFIED_AGE_ATTRIBUTE_FIXTURE).unwrap();
        assert_eq!(attribute.issuer, "jpki-test");
        assert_eq!(attribute.signed_subject, "age-over-20");
        assert_eq!(attribute.nonce, "fixture-nonce-0001");
        let roundtrip: VerifiedAgeAttribute =
            serde_json::from_str(&serde_json::to_string(&attribute).unwrap()).unwrap();
        assert_eq!(roundtrip, attribute);

        // 自己申告の birth_date / country_code を混ぜた JSON は検証済み属性として拒否する。
        let self_reported = concat!(
            r#"{"issuer":"jpki-test","signed_subject":"age-over-20","#,
            r#""nonce":"fixture-nonce-0001","#,
            r#""birth_date":"2000-01-01","country_code":"JP"}"#
        );
        assert!(
            serde_json::from_str::<VerifiedAgeAttribute>(self_reported).is_err(),
            "self-reported fields must not be accepted as a verified attribute"
        );
    }

    /// 自己申告の birth_date / country_code は TrustedAttributeRequired を通過しない。
    #[test]
    fn trusted_attribute_mode_rejects_self_reported_birth_date_and_country() {
        #[derive(Deserialize)]
        struct SelfReportedClaim {
            birth_date: String,
            country_code: String,
        }

        let claim: SelfReportedClaim =
            serde_json::from_str(r#"{"birth_date":"2000-01-01","country_code":"JP"}"#).unwrap();
        let policy = AgePolicy {
            enforcement_mode: AgeEnforcementMode::TrustedAttributeRequired,
            ..AgePolicy::default()
        };
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        // 成人として十分古い申告値・国コード指定でも、検証済み属性が無い限り拒否される。
        for (birth, country) in [("2000-01-01", "JP"), ("1990-01-01", " us ")] {
            let err = is_age_allowed_with_policy(&policy, birth, country, today).unwrap_err();
            assert_eq!(err, AgeValidationError::TrustedAttributeRequired);
        }
        // serde フィクスチャから読んだ申告値も同じ境界を通過しない。
        let err =
            is_age_allowed_with_policy(&policy, &claim.birth_date, &claim.country_code, today)
                .unwrap_err();
        assert_eq!(err, AgeValidationError::TrustedAttributeRequired);
    }
}
