use crate::AppError;
use base64::Engine;
use serde::Deserialize;
use x509_parser::{prelude::*, public_key::PublicKey};

/// JPKI リーフに許す RSA 公開鍵の最小鍵長（ビット）。
/// JWT の署名鍵と同様、NIST SP 800-57 の 112 bit 強度を満たす 2048 bit を下限とする。
const JPKI_RSA_MIN_KEY_BITS: usize = 2048;

/// 解析済み公開鍵が JPKI の鍵ポリシーを満たすかを判定する。
/// 非 RSA（EC / DSA 等）と RSA 2048 bit 未満は拒否する。
fn public_key_meets_policy(key: &PublicKey<'_>) -> bool {
    match key {
        PublicKey::RSA(rsa) => rsa.key_size() >= JPKI_RSA_MIN_KEY_BITS,
        _ => false,
    }
}

/// OID4VP / JPKI プレゼンテーション検証（SEC-02）
pub fn verify_presentation(
    presentation_token: &str,
    explicit_public_key_der: Option<Vec<u8>>,
    allow_mock: bool,
) -> Result<Vec<u8>, AppError> {
    if allow_mock && presentation_token == "mock_token" {
        return explicit_public_key_der
            .ok_or_else(|| AppError::Unauthorized("mock_token requires public_key_der".into()));
    }

    let cert_der = decode_certificate_der(presentation_token)?;
    extract_subject_public_key(&cert_der)
}

fn decode_certificate_der(token: &str) -> Result<Vec<u8>, AppError> {
    if let Ok(vp) = serde_json::from_str::<PresentationEnvelope>(token)
        && let Some(cert_b64) = vp.certificate.or(vp.verifiable_credential)
    {
        return base64::engine::general_purpose::STANDARD
            .decode(cert_b64.trim())
            .map_err(|e| AppError::Unauthorized(format!("Invalid certificate base64: {e}")));
    }

    base64::engine::general_purpose::STANDARD
        .decode(token.trim())
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token.trim()))
        .map_err(|e| AppError::Unauthorized(format!("Invalid JPKI presentation token: {e}")))
}

fn extract_subject_public_key(cert_der: &[u8]) -> Result<Vec<u8>, AppError> {
    let (_, cert) = X509Certificate::from_der(cert_der)
        .map_err(|e| AppError::Unauthorized(format!("X.509 parse failed: {e}")))?;

    if !cert.validity().is_valid() {
        return Err(AppError::Unauthorized(
            "Certificate is expired or not yet valid".into(),
        ));
    }

    // 発行者と主体が等しい自己署名リーフは受け付けない。
    // 信頼アンカーが確定していない証明書を「検証済み」として扱うのは危険なため。
    // 完全な chain 検証は信頼アンカーの配布という外部要因に依存するため、
    // ここでは自己署名の拒否と公開鍵ポリシーのみを適用する（SEC-02）。
    if cert.issuer() == cert.subject() {
        return Err(AppError::Unauthorized(
            "Self-signed JPKI certificate is not trusted".into(),
        ));
    }

    let parsed = cert
        .public_key()
        .parsed()
        .map_err(|_| AppError::Unauthorized("JPKI leaf public key cannot be parsed".into()))?;
    if !public_key_meets_policy(&parsed) {
        return Err(AppError::Unauthorized(
            "JPKI leaf public key must be RSA 2048 bits or stronger".into(),
        ));
    }

    Ok(cert.public_key().raw.to_vec())
}

#[derive(Debug, Deserialize)]
struct PresentationEnvelope {
    certificate: Option<String>,
    #[serde(rename = "verifiableCredential")]
    verifiable_credential: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト専用の決定的 RNG（`rsa::rand_core::TryRng` を実装）。
    struct KeyGenRng(u64);

    impl KeyGenRng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    impl rsa::rand_core::TryRng for KeyGenRng {
        type Error = rsa::rand_core::Infallible;

        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            Ok((self.next() >> 32) as u32)
        }

        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            Ok(self.next())
        }

        fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
            for chunk in dst.chunks_mut(8) {
                let bytes = self.next().to_le_bytes();
                chunk.copy_from_slice(&bytes[..chunk.len()]);
            }
            Ok(())
        }
    }

    impl rsa::rand_core::TryCryptoRng for KeyGenRng {}

    /// `CertificateParams::new` は全証明書が同じ既定 DN になるため、
    /// 発行者と主体を区別できるよう CommonName を明示する。
    fn named_params(cn: &str) -> rcgen::CertificateParams {
        let mut params =
            rcgen::CertificateParams::new(vec![format!("{cn}.jpki.test")]).expect("params");
        let mut dn = rcgen::DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, cn);
        params.distinguished_name = dn;
        params
    }

    /// rcgen は ring バックエンドのため RSA 鍵生成に非対応。
    /// `rsa` crate で生成した PKCS#8 DER を読み込んで RSA 2048 鍵を作る。
    fn rsa_signing_key() -> rcgen::KeyPair {
        use rsa::pkcs8::EncodePrivateKey;
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut rng = KeyGenRng(0x9E37_79B9_7F4A_7C15 ^ n.wrapping_mul(0x2545_F491_4F6C_DD1D));
        let key = rsa::PrivateKey::new(&mut rng, 2048).expect("generate RSA 2048 key");
        let der = key.to_pkcs8_der().expect("encode PKCS#8 DER");
        rcgen::KeyPair::try_from(der.as_bytes()).expect("import RSA key into rcgen")
    }

    #[test]
    fn mock_token_requires_allow_flag() {
        assert!(verify_presentation("mock_token", Some(vec![1]), false).is_err());
    }

    #[test]
    fn mock_token_with_key_succeeds() {
        let key = vec![1u8, 2, 3];
        let got = verify_presentation("mock_token", Some(key.clone()), true).unwrap();
        assert_eq!(got, key);
    }

    /// rcgen の `self_signed` は発行者=主体の自己署名リーフ。
    /// 信頼アンカーが未確定のため受け付けない（鍵は ECDSA P-256 だが、
    /// 自己署名チェックが公開鍵ポリシーより先に働く）。
    #[test]
    fn rejects_self_signed_rcgen_certificate() {
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec!["guild.test".to_string()]).unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(cert.der());
        let err = verify_presentation(&b64, None, false).unwrap_err();
        assert_unauthorized_contains(err, "Self-signed");
    }

    /// 自己署名であれば RSA 2048 bit でも拒否する（自己署名ガードの検証）。
    #[test]
    fn rejects_self_signed_rsa_2048_certificate() {
        let key = rsa_signing_key();
        let params = named_params("self-signed-rsa");
        let cert = params.self_signed(&key).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(cert.der());
        let err = verify_presentation(&b64, None, false).unwrap_err();
        assert_unauthorized_contains(err, "Self-signed");
    }

    /// CA 署名の RSA 2048 bit リーフは受け付ける（過剰拒否の回帰防止）。
    #[test]
    fn accepts_ca_signed_rsa_2048_certificate() {
        use rcgen::{CertifiedIssuer, IsCa};

        let ca_key = rsa_signing_key();
        let mut ca_params = named_params("jpki-test-ca");
        ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca = CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();

        let leaf_key = rsa_signing_key();
        let leaf_params = named_params("jpki-test-leaf");
        let leaf = leaf_params.signed_by(&leaf_key, &ca).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(leaf.der());
        let pk = verify_presentation(&b64, None, false).unwrap();
        assert!(!pk.is_empty());
    }

    /// CA 署名でも非 RSA リーフは鍵ポリシーで拒否する。
    #[test]
    fn rejects_ca_signed_ecdsa_certificate() {
        use rcgen::{CertifiedIssuer, IsCa};

        let ca_key = rsa_signing_key();
        let mut ca_params = named_params("jpki-test-ec-ca");
        ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca = CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();

        let leaf_key = rcgen::KeyPair::generate().unwrap(); // ECDSA P-256
        let leaf_params = named_params("jpki-test-ec-leaf");
        let leaf = leaf_params.signed_by(&leaf_key, &ca).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(leaf.der());
        let err = verify_presentation(&b64, None, false).unwrap_err();
        assert_unauthorized_contains(err, "RSA 2048");
    }

    /// 鍵ポリシー自体の境界（2048 受理 / 1024 拒否 / 非 RSA 拒否）。
    #[test]
    fn public_key_policy_requires_rsa_2048() {
        use x509_parser::public_key::RSAPublicKey;
        let exponent = [0x01u8, 0x00, 0x01];
        // DER INTEGER の最上位ビットが 1 の場合、先頭に 0x00 が付く。
        // key_size() はその先頭バイトを除いた長さ * 8 を返す。
        let modulus_2048 = [vec![0u8], vec![0x80; 256]].concat();
        let modulus_1024 = [vec![0u8], vec![0x80; 128]].concat();
        let strong = PublicKey::RSA(RSAPublicKey {
            modulus: &modulus_2048,
            exponent: &exponent,
        });
        let weak = PublicKey::RSA(RSAPublicKey {
            modulus: &modulus_1024,
            exponent: &exponent,
        });
        assert!(public_key_meets_policy(&strong));
        assert!(!public_key_meets_policy(&weak));
        assert!(!public_key_meets_policy(&PublicKey::Unknown(&[])));
    }

    fn assert_unauthorized_contains(err: AppError, needle: &str) {
        match err {
            AppError::Unauthorized(msg) => assert!(
                msg.contains(needle),
                "expected Unauthorized message containing {needle:?}, got {msg:?}"
            ),
            other => panic!("expected AppError::Unauthorized, got {other:?}"),
        }
    }

    #[test]
    fn invalid_base64_token_is_unauthorized() {
        let err = verify_presentation("!!!not-base64!!!", None, false).unwrap_err();
        assert_unauthorized_contains(err, "Invalid JPKI presentation token");
    }

    #[test]
    fn non_x509_bytes_are_rejected() {
        // "AAECAw==" is valid base64 for 00 01 02 03, not a DER X.509 certificate.
        let err = verify_presentation("AAECAw==", None, false).unwrap_err();
        assert_unauthorized_contains(err, "X.509 parse failed");
    }

    #[test]
    fn expired_certificate_is_rejected() {
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["expired.test".to_string()]).unwrap();
        params.not_before = rcgen::date_time_ymd(2020, 1, 1);
        params.not_after = rcgen::date_time_ymd(2021, 1, 1);
        let cert = params.self_signed(&key_pair).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(cert.der());
        let err = verify_presentation(&b64, None, false).unwrap_err();
        assert_unauthorized_contains(err, "Certificate is expired or not yet valid");
    }

    #[test]
    fn envelope_without_credentials_falls_back_to_raw_token() {
        // Both fields are None: decode_certificate_der must fall through to raw
        // base64 decoding of the JSON text, which then fails.
        let json = r#"{"certificate":null,"verifiableCredential":null}"#;
        let err = verify_presentation(json, None, false).unwrap_err();
        assert_unauthorized_contains(err, "Invalid JPKI presentation token");
    }

    #[test]
    fn empty_token_is_rejected() {
        // "" is valid base64 for an empty byte sequence, so it reaches X.509 parsing.
        let err = verify_presentation("", None, false).unwrap_err();
        assert_unauthorized_contains(err, "X.509 parse failed");
    }
}
