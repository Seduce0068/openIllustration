use chrono::NaiveDate;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum AgeValidationError {
    #[error("Invalid birth date format: {0}")]
    InvalidBirthDate(String),
    #[error("Under legal age for country {country}: age {age}, required {threshold}")]
    UnderAge {
        country: String,
        age: u32,
        threshold: u32,
    },
}

/// 国別の成人年齢閾値（docs/AGE_VALIDATION_POLICY.md）
pub fn legal_age_threshold(country_code: &str) -> u32 {
    match country_code {
        "US" => 21,
        "JP" | "DE" | "FR" => 18,
        _ => 18,
    }
}

/// 満年齢を計算する（基準日は呼び出し側で渡す）
pub fn calculate_age(birth_date: &str, today: NaiveDate) -> Result<u32, AgeValidationError> {
    let birth = NaiveDate::parse_from_str(birth_date, "%Y-%m-%d")
        .map_err(|e| AgeValidationError::InvalidBirthDate(e.to_string()))?;

    use chrono::Datelike;
    let mut age = today.year() - birth.year();
    if today.ordinal() < birth.ordinal() {
        age -= 1;
    }
    Ok(age as u32)
}

pub fn is_age_allowed(
    birth_date: &str,
    country_code: &str,
    today: NaiveDate,
) -> Result<(), AgeValidationError> {
    let age = calculate_age(birth_date, today)?;
    let threshold = legal_age_threshold(country_code);
    if age < threshold {
        return Err(AgeValidationError::UnderAge {
            country: country_code.to_string(),
            age,
            threshold,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn jp_adult_passes() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        assert!(is_age_allowed("2000-01-01", "JP", today).is_ok());
    }

    #[test]
    fn jp_minor_rejected() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        let err = is_age_allowed("2010-06-05", "JP", today).unwrap_err();
        assert!(matches!(err, AgeValidationError::UnderAge { .. }));
    }

    #[test]
    fn us_requires_21() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        assert!(is_age_allowed("2008-01-01", "US", today).is_err());
        assert!(is_age_allowed("2000-01-01", "US", today).is_ok());
    }
}
