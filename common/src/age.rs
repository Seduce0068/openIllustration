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
        let mut policy = Self::default();

        if let Ok(value) = std::env::var("AGE_DEFAULT_MINIMUM") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                policy.default_minimum_age = trimmed.parse::<u32>().map_err(|_| {
                    "AGE_DEFAULT_MINIMUM must be a non-negative integer".to_string()
                })?;
            }
        }

        if let Ok(value) = std::env::var("AGE_COUNTRY_OVERRIDES") {
            policy.country_minimums = parse_country_minimums(&value)?;
        }

        if let Ok(value) = std::env::var("AGE_TARGET_REGIONS") {
            policy.target_regions = parse_csv_list(&value);
        }

        if let Ok(value) = std::env::var("AGE_ENFORCEMENT_MODE") {
            let normalized = value.trim().to_ascii_lowercase();
            policy.enforcement_mode = match normalized.as_str() {
                "" | "self_reported" | "self_reported_dob_country" => {
                    AgeEnforcementMode::SelfReportedDobCountry
                }
                "trusted_attribute_required" | "trusted_attribute" => {
                    AgeEnforcementMode::TrustedAttributeRequired
                }
                _ => {
                    return Err(
                        "AGE_ENFORCEMENT_MODE must be self_reported_dob_country or trusted_attribute_required"
                            .to_string(),
                    );
                }
            };
        }

        if let Ok(value) = std::env::var("AGE_REQUIRE_EXPLICIT_CONFIGURATION") {
            policy.require_explicit_configuration = parse_bool_env(&value).ok_or_else(|| {
                "AGE_REQUIRE_EXPLICIT_CONFIGURATION must be a boolean (1/0/true/false)".to_string()
            })?;
        }

        if let Ok(value) = std::env::var("AGE_POLICY_CONFIGURED") {
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

fn parse_bool_env(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
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
    }
}
