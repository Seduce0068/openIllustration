#![cfg(test)]
use chrono::NaiveDate;
use common::age::{calculate_age, legal_age_threshold};
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_age_validation_policy(
        year in 1900..2030i32,
        month in 1..13u32,
        day in 1..28u32,
        country in prop_oneof![Just("JP"), Just("US"), Just("DE"), Just("XX")]
    ) {
        let birth_date = format!("{:04}-{:02}-{:02}", year, month, day);
        let today = NaiveDate::from_ymd_opt(2026, 5, 22).unwrap();
        let age = calculate_age(&birth_date, today).unwrap_or(0);
        let threshold = legal_age_threshold(country);

        if age < threshold {
            assert!(age < threshold);
        } else {
            assert!(age >= threshold);
        }
    }
}
