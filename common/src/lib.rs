pub mod age;

use rsa::sha2::Sha256 as RsaSha256;
use rsa::{Pkcs1v15Sign, RsaPublicKey, pkcs8::DecodePublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct User {
    pub user_hash: String,
    pub public_key_der: Option<Vec<u8>>,
    pub is_banned: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum ForensicError {
    #[error("Checksum mismatch")]
    ChecksumMismatch,
    #[error("Invalid signature: {0}")]
    InvalidSignature(String),
    #[error("Crypto error: {0}")]
    Crypto(String),
}

pub fn verify_checksum(data: &[u8], expected_checksum: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    hex::encode(result) == expected_checksum
}

pub fn verify_rsa_signature(
    public_key_der: &[u8],
    data: &[u8],
    signature: &[u8],
) -> Result<(), ForensicError> {
    let public_key = RsaPublicKey::from_public_key_der(public_key_der)
        .map_err(|e| ForensicError::Crypto(format!("Invalid public key: {}", e)))?;

    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();

    public_key
        .verify(Pkcs1v15Sign::new::<RsaSha256>(), &digest, signature)
        .map_err(|e| ForensicError::InvalidSignature(e.to_string()))
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuditLogEntry {
    pub log_id: Option<i64>,
    pub cid: String,
    pub old_status: String,
    pub new_status: String,
    pub changed_by: String,
    pub reason: String,
    pub previous_log_hash: String, // Chain for integrity
}

impl AuditLogEntry {
    pub fn calculate_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.cid.as_bytes());
        hasher.update(self.old_status.as_bytes());
        hasher.update(self.new_status.as_bytes());
        hasher.update(self.changed_by.as_bytes());
        hasher.update(self.reason.as_bytes());
        hasher.update(self.previous_log_hash.as_bytes());
        hex::encode(hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_checksum_matches_sha256_hex() {
        let data = b"forensic payload";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let expected = hex::encode(hasher.finalize());
        assert!(verify_checksum(data, &expected));
        assert!(!verify_checksum(data, "deadbeef"));
    }

    #[test]
    fn audit_log_hash_is_deterministic() {
        let entry = AuditLogEntry {
            log_id: None,
            cid: "QmTest".into(),
            old_status: "Approved".into(),
            new_status: "Reported".into(),
            changed_by: "user1".into(),
            reason: "report".into(),
            previous_log_hash: "GENESIS_HASH".into(),
        };
        assert_eq!(entry.calculate_hash(), entry.calculate_hash());
    }
}
