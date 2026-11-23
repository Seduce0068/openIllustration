//! DEC-01: 署名付き年齢属性の adapter 境界（mock 経路のみ・実証明書は未接続）。
//!
//! ここで扱うのは「署名済みであると提示された属性の束縛チェック」だけである。
//! 実 OP のクレーム仕様、署名検証・信頼ルート・失効確認は未確定のため、
//! 本モジュールは発行者許可リスト／audience／nonce の完全一致を検証し、
//! 年齢しきい値の判定は呼び出し側の既存ポリシーへ委ねる。
//!
//! 秘密情報は扱わない。発行者許可リストは `AGE_TRUSTED_ISSUERS` 由来の
//! 公開可能な非秘密設定であり、`GET /policy/age` で公開される。

use common::age::{AgeEnforcementMode, AgePolicy, AgeValidationError, VerifiedAgeAttribute};
use serde::Deserialize;

use crate::AppError;

/// 署名対象の年齢クレームと attestation を分離した adapter 入力。
///
/// `deny_unknown_fields` により、自己申告の `birth_date` / `country_code` を
/// 「検証済み属性」として混ぜ込むペイロードを拒否する。年齢クレームは
/// OIDC の `birthdate` 表記に限定し、attestation は既存の
/// [`VerifiedAgeAttribute`] を再利用する。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct VerifiedAgeClaims {
    /// 署名済みの生年月日（OIDC `birthdate` 相当、YYYY-MM-DD）。
    pub birthdate: String,
    /// 署名済みの居住国（ISO 3166-1 alpha-2）。自己申告の `country_code` とは別経路。
    pub country: String,
    /// 属性が発行された audience。省略時は呼び出し側が OIDC client_id を補う。
    pub audience: String,
    /// 署名済み属性の attestation（issuer / signed_subject / nonce）。
    pub attribute: VerifiedAgeAttribute,
}

/// adapter が束縛検証後に返す最小限の検証済み属性。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAge {
    pub issuer: String,
    pub signed_subject: String,
    pub birthdate: String,
    pub country: String,
}

/// 発行者許可リストを保持する adapter。
///
/// `verify` の引数は 4 つだけの狭い境界に保ち、許可リストは构建時に注入する。
pub struct VerifiedAgeAdapter {
    trusted_issuers: Vec<String>,
}

impl VerifiedAgeAdapter {
    pub fn new(trusted_issuers: Vec<String>) -> Self {
        Self { trusted_issuers }
    }

    /// 有効な年齢ポリシーから adapter を構築する。
    pub fn from_policy(policy: &AgePolicy) -> Self {
        Self::new(policy.trusted_issuers.clone())
    }

    /// 署名付き年齢属性の束縛を検証する。
    ///
    /// - `issuer` が許可リストに完全一致で含まれること
    /// - attestation の `issuer` が引数の `issuer` と一致すること
    /// - `audience` が引数の `audience` と一致すること（既定は OIDC client_id）
    /// - attestation の `nonce` が引数の `nonce` と一致すること
    /// - 年齢クレームと署名対象が欠落していないこと
    ///
    /// nonce の単回消費は呼び出し側の既存 `MOCK_NONCES.remove()` が担う。
    /// いずれの失敗も TrustedAttributeRequired と同等の 403 に変換する。
    pub fn verify(
        &self,
        issuer: &str,
        claims: &VerifiedAgeClaims,
        nonce: &str,
        audience: &str,
    ) -> Result<VerifiedAge, AppError> {
        if !self.trusted_issuers.iter().any(|trusted| trusted == issuer) {
            return Err(trusted_attribute_forbidden());
        }
        if claims.attribute.issuer != issuer {
            return Err(trusted_attribute_forbidden());
        }
        if claims.audience != audience {
            return Err(trusted_attribute_forbidden());
        }
        if claims.attribute.nonce != nonce {
            return Err(trusted_attribute_forbidden());
        }
        // country は ISO 3166-1 alpha-2（2 文字大文字）のみ受理する。
        // 不正値はデフォルト閾値へフォールバックして国別ポリシーをすり抜けるため fail-closed。
        let country = claims.country.trim();
        if claims.birthdate.trim().is_empty()
            || country.len() != 2
            || !country.bytes().all(|b| b.is_ascii_uppercase())
            || claims.attribute.signed_subject.trim().is_empty()
            || chrono::NaiveDate::parse_from_str(&claims.birthdate, "%Y-%m-%d").is_err()
        {
            return Err(trusted_attribute_forbidden());
        }

        Ok(VerifiedAge {
            issuer: issuer.to_string(),
            signed_subject: claims.attribute.signed_subject.clone(),
            birthdate: claims.birthdate.clone(),
            country: country.to_string(),
        })
    }
}

/// TrustedAttributeRequired と同一の 403 を返す（文言を単一ソースに集約）。
fn trusted_attribute_forbidden() -> AppError {
    crate::age_validation_error_to_app_error(AgeValidationError::TrustedAttributeRequired)
}

/// OIDC callback の設定ゲート直後に呼ぶ adapter 境界。
///
/// 実証明書／検証済み年齢属性の入力経路が未接続のため、信頼属性必須モードでは
/// fail closed とする（上記 `ensure_age_policy_configured` と同じ 403）。
/// 既定の自己申告モードでは何も変更しない（R1 item9: 自動昇格しない）。
pub fn verify_oidc_age_binding(policy: &AgePolicy) -> Result<(), AppError> {
    if policy.enforcement_mode == AgeEnforcementMode::TrustedAttributeRequired {
        return Err(trusted_attribute_forbidden());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::age::VERIFIED_AGE_ATTRIBUTE_FIXTURE;

    fn claims() -> VerifiedAgeClaims {
        VerifiedAgeClaims {
            birthdate: "2000-01-01".to_string(),
            country: "JP".to_string(),
            audience: "test-client".to_string(),
            attribute: serde_json::from_str(VERIFIED_AGE_ATTRIBUTE_FIXTURE).unwrap(),
        }
    }

    fn adapter() -> VerifiedAgeAdapter {
        VerifiedAgeAdapter::new(vec!["jpki-test".to_string()])
    }

    fn is_forbidden(err: &AppError) -> bool {
        matches!(err, AppError::Forbidden(_))
    }

    #[test]
    fn verify_accepts_valid_claims() {
        let verified = adapter()
            .verify("jpki-test", &claims(), "fixture-nonce-0001", "test-client")
            .expect("valid signed attribute must be accepted");
        assert_eq!(verified.issuer, "jpki-test");
        assert_eq!(verified.signed_subject, "age-over-20");
        assert_eq!(verified.birthdate, "2000-01-01");
        assert_eq!(verified.country, "JP");
    }

    #[test]
    fn self_reported_fields_cannot_be_injected_into_claims() {
        let payload = concat!(
            r#"{"birth_date":"2000-01-01","country_code":"JP","#,
            r#""audience":"test-client","#,
            r#""attribute":{"issuer":"jpki-test","signed_subject":"age-over-20","#,
            r#""nonce":"fixture-nonce-0001"}}"#
        );
        assert!(
            serde_json::from_str::<VerifiedAgeClaims>(payload).is_err(),
            "self-reported birth_date/country_code must not be accepted as verified claims"
        );
        // attestation 側も deny_unknown_fields で自己申告フィールドを拒否する。
        let inner = concat!(
            r#"{"issuer":"jpki-test","signed_subject":"age-over-20","#,
            r#""nonce":"fixture-nonce-0001","birth_date":"2000-01-01"}"#
        );
        assert!(serde_json::from_str::<VerifiedAgeAttribute>(inner).is_err());
    }

    #[test]
    fn verify_rejects_untrusted_issuer() {
        let err = adapter()
            .verify(
                "attacker.example",
                &claims(),
                "fixture-nonce-0001",
                "test-client",
            )
            .expect_err("issuer outside the allowlist must be rejected");
        assert!(is_forbidden(&err));
        // 空の許可リストはすべて拒否する（fail closed）。
        let err = VerifiedAgeAdapter::new(Vec::new())
            .verify("jpki-test", &claims(), "fixture-nonce-0001", "test-client")
            .expect_err("empty allowlist must reject everything");
        assert!(is_forbidden(&err));
    }

    #[test]
    fn verify_rejects_issuer_mismatch_between_argument_and_attestation() {
        let mut tampered = claims();
        tampered.attribute.issuer = "other-issuer".to_string();
        let err = adapter()
            .verify("jpki-test", &tampered, "fixture-nonce-0001", "test-client")
            .expect_err("issuer binding mismatch must be rejected");
        assert!(is_forbidden(&err));
    }

    #[test]
    fn verify_rejects_audience_mismatch() {
        let err = adapter()
            .verify("jpki-test", &claims(), "fixture-nonce-0001", "other-client")
            .expect_err("audience mismatch must be rejected");
        assert!(is_forbidden(&err));
    }

    #[test]
    fn verify_rejects_nonce_mismatch() {
        // 同一 nonce の再提示は呼び出し側の MOCK_NONCES.remove() で消費済みとなる。
        // adapter 自体は nonce の束縛（一致）を検証する。
        let err = adapter()
            .verify("jpki-test", &claims(), "replayed-nonce", "test-client")
            .expect_err("nonce mismatch must be rejected");
        assert!(is_forbidden(&err));
    }

    #[test]
    fn verify_rejects_missing_or_invalid_age_claim() {
        let mut invalid = claims();
        invalid.birthdate = "not-a-date".to_string();
        assert!(is_forbidden(
            &adapter()
                .verify("jpki-test", &invalid, "fixture-nonce-0001", "test-client")
                .expect_err("invalid birthdate must be rejected")
        ));

        let mut empty_subject = claims();
        empty_subject.attribute.signed_subject = String::new();
        assert!(is_forbidden(
            &adapter()
                .verify(
                    "jpki-test",
                    &empty_subject,
                    "fixture-nonce-0001",
                    "test-client"
                )
                .expect_err("empty signed_subject must be rejected")
        ));

        // country は ISO 3166-1 alpha-2（2 文字大文字）のみ。不正値はデフォルト閾値
        // へのフォールバックを防ぐため fail-closed で拒否する。
        for bad_country in ["jp", "JPN", "J1", "", " J"] {
            let mut bad = claims();
            bad.country = bad_country.to_string();
            assert!(
                is_forbidden(
                    &adapter()
                        .verify("jpki-test", &bad, "fixture-nonce-0001", "test-client")
                        .expect_err("invalid country must be rejected")
                ),
                "country={bad_country:?}"
            );
        }
    }

    #[test]
    fn oidc_binding_keeps_self_reported_mode_and_fails_closed_on_trusted_mode() {
        assert!(verify_oidc_age_binding(&AgePolicy::default()).is_ok());
        let trusted = AgePolicy {
            enforcement_mode: AgeEnforcementMode::TrustedAttributeRequired,
            trusted_issuers: vec!["jpki-test".to_string()],
            ..AgePolicy::default()
        };
        assert!(is_forbidden(
            &verify_oidc_age_binding(&trusted)
                .expect_err("unconnected trusted path must fail closed")
        ));
    }
}
