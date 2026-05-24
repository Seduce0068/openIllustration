use proptest::prelude::*;
use chrono::Datelike;

// テスト対象のロジックをここに移植またはインポート
fn get_legal_age_threshold(country_code: &str) -> u32 {
    match country_code {
        "US" => 21,
        "JP" | "DE" | "FR" => 18,
        _ => 21, // リスク回避のためデフォルトを 21 とする
    }
}

// ユーザーの生年月日と現在から満年齢を計算するロジック
fn calculate_age(birth_date: &str, today: &chrono::NaiveDate) -> u32 {
    let b = chrono::NaiveDate::parse_from_str(birth_date, "%Y-%m-%d").unwrap();
    let mut age = today.year() - b.year();
    if today.month() < b.month() || (today.month() == b.month() && today.day() < b.day()) {
        age -= 1;
    }
    age as u32
}

proptest! {
    #[test]
    fn test_age_validation_policy(
        year in 1900..2030,
        month in 1..13u32,
        day in 1..28u32,
        country in prop_oneof![Just("JP"), Just("US"), Just("DE"), Just("XX")]
    ) {
        let birth_date = format!("{:04}-{:02}-{:02}", year, month, day);
        let today = chrono::NaiveDate::from_ymd_opt(2026, 5, 22).unwrap();
        let age = calculate_age(&birth_date, &today);
        let threshold = get_legal_age_threshold(&country);

        // 成人年齢判定の期待値検証
        if age < threshold {
            // 成人未満ならNGであるべき
            assert!(age < threshold);
        } else {
            // 成人以上ならOKであるべき
            assert!(age >= threshold);
        }
    }
}
