#![cfg(test)]
use chrono::{Datelike, NaiveDate};
use common::age::{calculate_age, legal_age_threshold};
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_age_validation_policy(
        year in 1900..2030i32,
        // 今日と同じ月日を必ず含め、未誕生日判定の off-by-one を検出できるようにする。
        month_day in prop_oneof![
            (1..13u32, 1..29u32).prop_map(|(m, d)| (m, d)),
            Just((5u32, 22u32)),
        ],
        country in prop_oneof![Just("JP"), Just("US"), Just("DE"), Just("XX")]
    ) {
        let today = NaiveDate::from_ymd_opt(2026, 5, 22).unwrap();
        let (month, day) = month_day;
        let birth_date = format!("{:04}-{:02}-{:02}", year, month, day);
        let age = calculate_age(&birth_date, today).unwrap();
        let threshold = legal_age_threshold(country);

        // 独立した参照計算（年差 - 未誕生日）と突き合わせる。
        // 同語反復の検証では実装バグを検出できないため。
        let birth = NaiveDate::from_ymd_opt(year, month, day).unwrap();
        let mut expected = today.year() - birth.year();
        if (today.month(), today.day()) < (birth.month(), birth.day()) {
            expected -= 1;
        }
        let expected = expected.max(0) as u32;
        prop_assert_eq!(age, expected);
        prop_assert_eq!(age >= threshold, expected >= threshold);
    }
}
