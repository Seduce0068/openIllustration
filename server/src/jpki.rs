use crate::AppError;
use base64::Engine;
use serde::Deserialize;
use x509_parser::prelude::*;

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

    #[test]
    fn parses_rcgen_certificate() {
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec!["guild.test".to_string()]).unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(cert.der());
        let pk = verify_presentation(&b64, None, false).unwrap();
        assert!(!pk.is_empty());
    }
}
