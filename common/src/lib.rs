pub mod age;

use rsa::sha2::Sha256 as RsaSha256;
use rsa::{Pkcs1v15Sign, PublicKey, pkcs8::DecodePublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
    /// セッション識別子。既存トークンとの互換のため省略可能。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    /// 発行時刻（Unix秒）。既存トークンとの互換のため省略可能。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iat: Option<usize>,
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
    let public_key = PublicKey::from_public_key_der(public_key_der)
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
    /// v1（既存互換）。フィールドを単純連結するため、値の境界が曖昧になる。
    /// 既存DBの検証にのみ使用し、新規エントリの生成には使わない。
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

    /// v2: フィールドに長さプレフィックスを付け、値の境界を一意にする。
    /// 例: cid="ab", old="c" と cid="a", old="bc" は異なるハッシュになる。
    pub fn calculate_hash_v2(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"OPENILLUSTRATION_AUDIT_V2\0");
        for field in [
            self.cid.as_bytes(),
            self.old_status.as_bytes(),
            self.new_status.as_bytes(),
            self.changed_by.as_bytes(),
            self.reason.as_bytes(),
            self.previous_log_hash.as_bytes(),
        ] {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field);
        }
        hex::encode(hasher.finalize())
    }
}

/// AUDIT-01: 監査ログの欠落・順序入替・末尾切り詰めを検出するための補助情報。
/// これは改ざん不能性を保証するものではなく、ローカルDBの連鎖整合性チェックの範囲である。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditChainDigest {
    pub count: usize,
    pub first_log_id: Option<i64>,
    pub last_log_id: Option<i64>,
    pub root_hash: String,
}

/// 連鎖エントリ（log_id, entry_hash）の連番と Merkle root を計算する。
/// log_id は 1 から連続していることを要求する（欠落・挿入の検出）。
pub fn compute_audit_chain_digest(entries: &[(i64, String)]) -> Result<AuditChainDigest, String> {
    for (expected_id, (log_id, hash)) in (1_i64..).zip(entries.iter()) {
        if *log_id != expected_id {
            return Err(format!(
                "non-contiguous log_id: expected {expected_id}, got {log_id}"
            ));
        }
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("invalid entry_hash at log_id={log_id}"));
        }
    }

    let root_hash = if entries.is_empty() {
        hex::encode(Sha256::digest(b"AUDIT_CHAIN_EMPTY"))
    } else {
        let mut level: Vec<Vec<u8>> = entries
            .iter()
            .map(|(_, hash)| hex::decode(hash).expect("validated hex"))
            .collect();
        while level.len() > 1 {
            let mut next = Vec::with_capacity(level.len().div_ceil(2));
            for pair in level.chunks(2) {
                let mut hasher = Sha256::new();
                hasher.update(&pair[0]);
                if let Some(right) = pair.get(1) {
                    hasher.update(right);
                }
                next.push(hasher.finalize().to_vec());
            }
            level = next;
        }
        hex::encode(&level[0])
    };

    Ok(AuditChainDigest {
        count: entries.len(),
        first_log_id: entries.first().map(|(id, _)| *id),
        last_log_id: entries.last().map(|(id, _)| *id),
        root_hash,
    })
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

    fn sample_hashes() -> Vec<(i64, String)> {
        (1..=4)
            .map(|id| (id, "a".repeat(63) + &format!("{id:x}")))
            .collect()
    }

    #[test]
    fn audit_hash_v2_separates_ambiguous_field_boundaries() {
        let base = AuditLogEntry {
            log_id: None,
            cid: "ab".into(),
            old_status: "c".into(),
            new_status: "Approved".into(),
            changed_by: "user".into(),
            reason: "r".into(),
            previous_log_hash: "GENESIS_HASH".into(),
        };
        let shifted = AuditLogEntry {
            cid: "a".into(),
            old_status: "bc".into(),
            ..base.clone()
        };
        assert_eq!(
            base.calculate_hash(),
            shifted.calculate_hash(),
            "v1 must remain reproducible for existing databases"
        );
        assert_ne!(
            base.calculate_hash_v2(),
            shifted.calculate_hash_v2(),
            "v2 must disambiguate field boundaries"
        );
        assert_eq!(base.calculate_hash_v2(), base.calculate_hash_v2());
    }

    #[test]
    fn audit_chain_digest_is_stable_and_ordered() {
        let entries = sample_hashes();
        let first = compute_audit_chain_digest(&entries).unwrap();
        let second = compute_audit_chain_digest(&entries).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.count, 4);
        assert_eq!(first.first_log_id, Some(1));
        assert_eq!(first.last_log_id, Some(4));
        assert_eq!(first.root_hash.len(), 64);

        let mut reversed = entries;
        reversed.reverse();
        assert!(compute_audit_chain_digest(&reversed).is_err());
    }

    #[test]
    fn audit_chain_digest_detects_gaps_and_truncation() {
        let entries = sample_hashes();
        let full = compute_audit_chain_digest(&entries).unwrap();
        let truncated = compute_audit_chain_digest(&entries[..3]).unwrap();
        assert_ne!(full.root_hash, truncated.root_hash);

        let mut with_gap = entries;
        with_gap.remove(1);
        assert!(compute_audit_chain_digest(&with_gap).is_err());
    }

    #[test]
    fn audit_chain_digest_rejects_invalid_hash() {
        let mut entries = sample_hashes();
        entries[0].1 = "not-a-hash".to_string();
        assert!(compute_audit_chain_digest(&entries).is_err());
        let empty = compute_audit_chain_digest(&[]).unwrap();
        assert_eq!(empty.count, 0);
        assert_eq!(empty.first_log_id, None);
        assert_eq!(empty.last_log_id, None);
    }
}
