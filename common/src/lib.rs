pub mod age;

use rsa::sha2::Sha256 as RsaSha256;
use rsa::{
    Pkcs1v15Sign, PrivateKey, PublicKey,
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePublicKey},
};
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

/// 現在サポートする監査エクスポート形式のバージョン。
pub const AUDIT_SNAPSHOT_VERSION: u32 = 3;

/// 署名付き監査スナップショット。第三者は公開鍵だけで検証できる。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct SignedAuditSnapshot {
    pub version: u32,
    /// reason/details をマスクしたエクスポートか。
    /// true の場合は entry_hash の再計算をスキップする（元の理由がスナップショットに無いため）。
    /// 署名が entry_hash 列全体を保護し、連鎖・連番・Merkle root も検証される。
    #[serde(default)]
    pub reasons_masked: bool,
    pub entries: Vec<AuditSnapshotEntry>,
    /// BAN / UNBAN / PIN 解除の運用判断（ADR-0004 §10）。
    #[serde(default)]
    pub governance_entries: Vec<GovernanceAuditEntry>,
    pub merkle_root: String,
    /// base64 エンコードした署名（署名対象は `signature_payload()`）。
    pub signature: String,
}

/// 公開用に理由テキストをマスクする。空でなければ別文字列であることを示す。
/// 本文は露出させず、長さと有無だけを残す（公開範囲の既定値）。
pub fn mask_reason(reason: &str, include: bool) -> String {
    if include {
        return reason.to_string();
    }
    if reason.is_empty() {
        String::new()
    } else {
        format!("[redacted:{} bytes]", reason.len())
    }
}

/// モデレーション画面向けの通報理由要約。
///
/// - 空文字はそのまま返す。
/// - PII を `[redacted]` に置換する。検出対象は以下。
///   - `@` を含むトークン（メールアドレス等）
///   - `://` を含むトークン（括弧等で囲まれた URL を含む）
///   - `www.` で始まるトークン（スキーム無し URL）
///   - IPv4 らしい `\d{1,3}(\.\d{1,3}){3}` 部分列を含むトークン
///   - Unicode 数字（全角数字等）の 4 連続
/// - 最大 120 Unicode 文字。超過時は先頭 119 文字 + `…`（文字境界を守る）。
///
/// 監査エクスポート向けの [`mask_reason`] とは用途が異なるため変更しない。
pub fn summarize_reason(reason: &str) -> String {
    if reason.is_empty() {
        return String::new();
    }

    const MAX_CHARS: usize = 120;

    let mut redacted = String::with_capacity(reason.len());
    let mut token = String::new();
    for ch in reason.chars() {
        if ch.is_whitespace() {
            push_redacted_token(&mut redacted, &token);
            token.clear();
            redacted.push(ch);
        } else {
            token.push(ch);
        }
    }
    push_redacted_token(&mut redacted, &token);

    // 置換で文字数が伸びる場合があるため、上限は置換後に適用する。
    if redacted.chars().count() <= MAX_CHARS {
        return redacted;
    }
    let mut summarized: String = redacted.chars().take(MAX_CHARS - 1).collect();
    summarized.push('…');
    summarized
}

/// 空白区切りの 1 トークンを PII 置換して `out` に追加する。
fn push_redacted_token(out: &mut String, token: &str) {
    if token.is_empty() {
        return;
    }
    // トークン全体が秘密情報と判定できるケース。
    if token.contains('@')
        || token.contains("://")
        || starts_with_www(token)
        || contains_ipv4_like(token)
    {
        out.push_str("[redacted]");
        return;
    }

    // トークン内の Unicode 数字 4 連続は部分置換する（既存挙動を維持）。
    let mut run = String::new();
    for ch in token.chars() {
        if ch.is_numeric() {
            run.push(ch);
        } else {
            push_numeric_run(out, &run);
            run.clear();
            out.push(ch);
        }
    }
    push_numeric_run(out, &run);
}

/// Unicode 数字の連続が 4 桁以上なら `[redacted]` に置換する。
/// 全角数字は UTF-8 で 3 バイトなので、バイト数ではなく文字数で判定する。
fn push_numeric_run(out: &mut String, run: &str) {
    if run.chars().count() >= 4 {
        out.push_str("[redacted]");
    } else {
        out.push_str(run);
    }
}

/// トークンが `www.` で始まるか（大文字小文字を区別しない）。
fn starts_with_www(token: &str) -> bool {
    token
        .as_bytes()
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"www."))
}

/// トークン内に IPv4 らしい `\d{1,3}(\.\d{1,3}){3}` 部分列があるか。
/// 各セグメントの 0-255 範囲は厳密に検査しない（検出は過剰気味でよい）。
fn contains_ipv4_like(token: &str) -> bool {
    let chars: Vec<char> = token.chars().collect();
    for start in 0..chars.len() {
        if !chars[start].is_numeric() {
            continue;
        }
        let mut pos = start;
        let mut segments = 0;
        loop {
            let mut len = 0;
            while pos < chars.len() && chars[pos].is_numeric() && len < 3 {
                pos += 1;
                len += 1;
            }
            if len == 0 {
                break;
            }
            segments += 1;
            if segments == 4 {
                return true;
            }
            if pos >= chars.len() || chars[pos] != '.' {
                break;
            }
            pos += 1; // 区切りドット
        }
    }
    false
}

/// 運用判断の監査イベント。コンテンツ監査とは別テーブルから抽出する。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct GovernanceAuditEntry {
    pub log_id: i64,
    pub action: String,
    pub target: String,
    pub actor_hash: String,
    pub reason: String,
    pub details: Option<String>,
    pub created_at: String,
}

/// スナップショットに含まれる監査エントリ。第三者がハッシュと連鎖を再計算できる。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct AuditSnapshotEntry {
    pub log_id: i64,
    pub cid: String,
    pub old_status: String,
    pub new_status: String,
    pub changed_by: String,
    pub reason: String,
    pub previous_log_hash: String,
    pub entry_hash: String,
}

impl AuditSnapshotEntry {
    /// v2 ハッシュを再計算し、保存された entry_hash と一致するか確認する。
    pub fn verify_hash(&self) -> bool {
        let entry = AuditLogEntry {
            log_id: Some(self.log_id),
            cid: self.cid.clone(),
            old_status: self.old_status.clone(),
            new_status: self.new_status.clone(),
            changed_by: self.changed_by.clone(),
            reason: self.reason.clone(),
            previous_log_hash: self.previous_log_hash.clone(),
        };
        entry.calculate_hash_v2() == self.entry_hash
    }
}

impl SignedAuditSnapshot {
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// 署名対象の正規化バイト列。JSON のフィールド順に依存しない。
    pub fn signature_payload(&self) -> Vec<u8> {
        let entries_json =
            serde_json::to_string(&self.entries).expect("serializing audit entries cannot fail");
        let governance_json = serde_json::to_string(&self.governance_entries)
            .expect("serializing governance entries cannot fail");
        // 後方互換: reasons_masked=false の既存 v3 スナップショットは旧形式の
        // 署名対象を維持する。true（マスク版）のときだけフラグを含め、
        // フラグ改ざん（true→false）も署名検証で検出できるようにする。
        if self.reasons_masked {
            format!(
                "OPENILLUSTRATION_AUDIT_EXPORT_V3\0{}\0masked\0{}\0{}\0{}",
                self.version, entries_json, governance_json, self.merkle_root
            )
            .into_bytes()
        } else {
            format!(
                "OPENILLUSTRATION_AUDIT_EXPORT_V3\0{}\0{}\0{}\0{}",
                self.version, entries_json, governance_json, self.merkle_root
            )
            .into_bytes()
        }
    }

    /// エントリ列のハッシュ連鎖・連番・Merkle root を再計算して検証する。
    pub fn verify_chain(&self) -> Result<(), ForensicError> {
        let mut expected_prev = "GENESIS_HASH".to_string();
        let mut hashes = Vec::with_capacity(self.entries.len());
        for (index, entry) in self.entries.iter().enumerate() {
            let expected_id = index as i64 + 1;
            if entry.log_id != expected_id {
                return Err(ForensicError::InvalidSignature(format!(
                    "non-contiguous log_id: expected {expected_id}, got {}",
                    entry.log_id
                )));
            }
            if !self.reasons_masked && !entry.verify_hash() {
                return Err(ForensicError::InvalidSignature(format!(
                    "entry_hash mismatch at log_id={}",
                    entry.log_id
                )));
            }
            if entry.previous_log_hash != expected_prev {
                return Err(ForensicError::InvalidSignature(format!(
                    "chain mismatch at log_id={}",
                    entry.log_id
                )));
            }
            expected_prev = entry.entry_hash.clone();
            hashes.push(entry.entry_hash.clone());
        }
        let digest = compute_audit_chain_digest(
            &self
                .entries
                .iter()
                .map(|entry| (entry.log_id, entry.entry_hash.clone()))
                .collect::<Vec<_>>(),
        )
        .map_err(ForensicError::InvalidSignature)?;
        if digest.root_hash != self.merkle_root {
            return Err(ForensicError::InvalidSignature(
                "Merkle root mismatch".to_string(),
            ));
        }
        Ok(())
    }
}

/// PEM 秘密鍵でスナップショットに署名する。
pub fn sign_audit_snapshot(
    snapshot: &SignedAuditSnapshot,
    private_key_pem: &str,
) -> Result<String, ForensicError> {
    if snapshot.version != AUDIT_SNAPSHOT_VERSION {
        return Err(ForensicError::InvalidSignature(format!(
            "unsupported audit snapshot version: {}",
            snapshot.version
        )));
    }
    let key = PrivateKey::from_pkcs8_pem(private_key_pem)
        .map_err(|e| ForensicError::Crypto(format!("Invalid private key: {e}")))?;
    let signing_key = rsa::pkcs1v15::SigningKey::<RsaSha256>::new(key);
    use rsa::signature::{SignatureEncoding, Signer};
    let signature = signing_key.sign(&snapshot.signature_payload());
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        signature.to_bytes(),
    ))
}

/// 公開鍵 DER でスナップショット署名を検証する。
pub fn verify_audit_snapshot(
    snapshot: &SignedAuditSnapshot,
    public_key_der: &[u8],
) -> Result<(), ForensicError> {
    // 未知の形式は fail-closed で拒否する（署名が正しくても受理しない）。
    if snapshot.version != AUDIT_SNAPSHOT_VERSION {
        return Err(ForensicError::InvalidSignature(format!(
            "unsupported audit snapshot version: {}",
            snapshot.version
        )));
    }
    let signature = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &snapshot.signature,
    )
    .map_err(|e| ForensicError::InvalidSignature(format!("Invalid base64: {e}")))?;
    verify_rsa_signature(public_key_der, &snapshot.signature_payload(), &signature)?;
    snapshot.verify_chain()
}

/// PEM 秘密鍵から公開鍵 DER を導出する（配布用）。
pub fn public_key_der_from_private_pem(private_key_pem: &str) -> Result<Vec<u8>, ForensicError> {
    let key = PrivateKey::from_pkcs8_pem(private_key_pem)
        .map_err(|e| ForensicError::Crypto(format!("Invalid private key: {e}")))?;
    key.to_public_key()
        .to_public_key_der()
        .map(|der| der.as_ref().to_vec())
        .map_err(|e| ForensicError::Crypto(format!("Public key encoding failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 決定論的 RNG により RSA 鍵生成を再現可能にする（既存テストと共有）。
    struct TestRng(u64);
    impl rsa::rand_core::TryRng for TestRng {
        type Error = rsa::rand_core::Infallible;
        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            Ok((self.next() >> 32) as u32)
        }
        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            Ok(self.next())
        }
        fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
            for chunk in dst.chunks_mut(8) {
                let value = self.next().to_le_bytes();
                chunk.copy_from_slice(&value[..chunk.len()]);
            }
            Ok(())
        }
    }
    impl TestRng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }
    impl rsa::rand_core::TryCryptoRng for TestRng {}

    fn test_key_pair() -> (String, Vec<u8>) {
        use rsa::pkcs8::EncodePrivateKey;
        let mut rng = TestRng(0x9E37_79B9_7F4A_7C15);
        let key = PrivateKey::new(&mut rng, 2048).unwrap();
        let private_pem = key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap()
            .to_string();
        let public_der = public_key_der_from_private_pem(&private_pem).unwrap();
        (private_pem, public_der)
    }

    /// 署名検証だけを独立に行い、連鎖検証と分離して検証できるようにする。
    fn assert_signature_is_valid(snapshot: &SignedAuditSnapshot, public_der: &[u8]) {
        let signature = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &snapshot.signature,
        )
        .expect("signature must be valid base64");
        verify_rsa_signature(public_der, &snapshot.signature_payload(), &signature)
            .expect("re-signed signature must verify");
    }

    /// log_id が 1..=count で連続し、previous_log_hash / entry_hash も整合する列を作る。
    fn build_chained_entries(count: usize) -> Vec<AuditSnapshotEntry> {
        let mut entries = Vec::with_capacity(count);
        let mut previous = "GENESIS_HASH".to_string();
        for id in 1..=count as i64 {
            let log = AuditLogEntry {
                log_id: Some(id),
                cid: format!("QmTest{id}"),
                old_status: "Approved".into(),
                new_status: "Reported".into(),
                changed_by: "reporter".into(),
                reason: format!("reason-{id}"),
                previous_log_hash: previous.clone(),
            };
            let entry_hash = log.calculate_hash_v2();
            entries.push(AuditSnapshotEntry {
                log_id: id,
                cid: log.cid,
                old_status: log.old_status,
                new_status: log.new_status,
                changed_by: log.changed_by,
                reason: log.reason,
                previous_log_hash: log.previous_log_hash,
                entry_hash: entry_hash.clone(),
            });
            previous = entry_hash;
        }
        entries
    }

    /// フィールド改ざん後に entry_hash を再計算する（reasons_masked=false 用）。
    fn refresh_entry_hash(entry: &mut AuditSnapshotEntry) {
        let log = AuditLogEntry {
            log_id: Some(entry.log_id),
            cid: entry.cid.clone(),
            old_status: entry.old_status.clone(),
            new_status: entry.new_status.clone(),
            changed_by: entry.changed_by.clone(),
            reason: entry.reason.clone(),
            previous_log_hash: entry.previous_log_hash.clone(),
        };
        entry.entry_hash = log.calculate_hash_v2();
    }

    /// entry_hash 列だけから Merkle root を求める（連番チェックに依存しない）。
    fn merkle_root_of(hashes: &[String]) -> String {
        let pairs: Vec<(i64, String)> = hashes
            .iter()
            .enumerate()
            .map(|(index, hash)| (index as i64 + 1, hash.clone()))
            .collect();
        compute_audit_chain_digest(&pairs).unwrap().root_hash
    }

    /// 未マスク版スナップショットに（再）署名して返す。
    fn resign(
        entries: Vec<AuditSnapshotEntry>,
        merkle_root: String,
        private_pem: &str,
    ) -> SignedAuditSnapshot {
        let mut snapshot = SignedAuditSnapshot {
            version: AUDIT_SNAPSHOT_VERSION,
            reasons_masked: false,
            entries,
            governance_entries: Vec::new(),
            merkle_root,
            signature: String::new(),
        };
        snapshot.signature = sign_audit_snapshot(&snapshot, private_pem).unwrap();
        snapshot
    }

    fn assert_chain_error(snapshot: &SignedAuditSnapshot, fragment: &str) {
        let error = snapshot.verify_chain().expect_err("chain must be rejected");
        match error {
            ForensicError::InvalidSignature(message) => assert!(
                message.contains(fragment),
                "expected error containing {fragment:?}, got {message:?}"
            ),
            other => panic!("expected InvalidSignature, got {other:?}"),
        }
    }

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

        // 各フィールドを1つずつ変えると両バージョンのハッシュが変わることを検証する。
        // これにより、あるフィールドが計算から漏れても検出できる。
        let base_v1 = entry.calculate_hash();
        let base_v2 = entry.calculate_hash_v2();
        let mutations = [
            (
                "cid",
                AuditLogEntry {
                    cid: "QmOther".into(),
                    ..entry.clone()
                },
            ),
            (
                "old_status",
                AuditLogEntry {
                    old_status: "Reported".into(),
                    ..entry.clone()
                },
            ),
            (
                "new_status",
                AuditLogEntry {
                    new_status: "Approved".into(),
                    ..entry.clone()
                },
            ),
            (
                "changed_by",
                AuditLogEntry {
                    changed_by: "user2".into(),
                    ..entry.clone()
                },
            ),
            (
                "reason",
                AuditLogEntry {
                    reason: "other".into(),
                    ..entry.clone()
                },
            ),
            (
                "previous_log_hash",
                AuditLogEntry {
                    previous_log_hash: "OTHER_GENESIS".into(),
                    ..entry.clone()
                },
            ),
            // log_id は entry_hash に意図的に含めない。連続性チェックと
            // Merkle root の (log_id, entry_hash) タプルで別途カバーする。
        ];
        for (name, mutated) in mutations {
            assert_ne!(
                base_v1,
                mutated.calculate_hash(),
                "v1 must depend on {name}"
            );
            assert_ne!(
                base_v2,
                mutated.calculate_hash_v2(),
                "v2 must depend on {name}"
            );
        }
    }

    fn sample_hashes() -> Vec<(i64, String)> {
        (1..=4)
            .map(|id| (id, "a".repeat(63) + &format!("{id:x}")))
            .collect()
    }

    #[test]
    fn signed_audit_snapshot_verifies_with_public_key() {
        let (private_pem, public_der) = test_key_pair();

        let entry = AuditLogEntry {
            log_id: Some(1),
            cid: "QmTest".into(),
            old_status: "Approved".into(),
            new_status: "Reported".into(),
            changed_by: "reporter".into(),
            reason: "illegal".into(),
            previous_log_hash: "GENESIS_HASH".into(),
        };
        let entry_hash = entry.calculate_hash_v2();
        let snapshot_entry = AuditSnapshotEntry {
            log_id: 1,
            cid: entry.cid.clone(),
            old_status: entry.old_status.clone(),
            new_status: entry.new_status.clone(),
            changed_by: entry.changed_by.clone(),
            reason: entry.reason.clone(),
            previous_log_hash: entry.previous_log_hash.clone(),
            entry_hash: entry_hash.clone(),
        };
        let digest = compute_audit_chain_digest(&[(1, entry_hash.clone())]).unwrap();
        let governance = GovernanceAuditEntry {
            log_id: 1,
            action: "BAN".into(),
            target: "bad_user".into(),
            actor_hash: "operator".into(),
            reason: "repeated violations".into(),
            details: Some("{\"duration_days\":30}".into()),
            created_at: "2026-11-17T00:00:00Z".into(),
        };
        let mut snapshot = SignedAuditSnapshot {
            version: 3,
            reasons_masked: false,
            entries: vec![snapshot_entry.clone()],
            governance_entries: vec![governance.clone()],
            merkle_root: digest.root_hash.clone(),
            signature: String::new(),
        };
        snapshot.signature = sign_audit_snapshot(&snapshot, &private_pem).unwrap();
        assert!(verify_audit_snapshot(&snapshot, &public_der).is_ok());

        // 後方互換: 既存 v3（フラグ無し）の署名対象は旧形式のまま。
        let legacy_payload = format!(
            "OPENILLUSTRATION_AUDIT_EXPORT_V3\0{}\0{}\0{}\0{}",
            snapshot.version,
            serde_json::to_string(&snapshot.entries).unwrap(),
            serde_json::to_string(&snapshot.governance_entries).unwrap(),
            snapshot.merkle_root
        );
        assert_eq!(snapshot.signature_payload(), legacy_payload.as_bytes());

        let tampered = SignedAuditSnapshot {
            reasons_masked: false,
            entries: vec![AuditSnapshotEntry {
                reason: "tampered".into(),
                ..snapshot_entry.clone()
            }],
            ..snapshot.clone()
        };
        assert!(
            verify_audit_snapshot(&tampered, &public_der).is_err(),
            "tampering with entry content must be detected"
        );

        let tampered_governance = SignedAuditSnapshot {
            reasons_masked: false,
            governance_entries: vec![GovernanceAuditEntry {
                reason: "tampered".into(),
                ..governance.clone()
            }],
            ..snapshot
        };
        assert!(
            verify_audit_snapshot(&tampered_governance, &public_der).is_err(),
            "tampering with governance audit must be detected"
        );

        let unsupported_version = SignedAuditSnapshot {
            version: 999,
            ..tampered_governance
        };
        assert!(
            verify_audit_snapshot(&unsupported_version, &public_der).is_err(),
            "unsupported snapshot versions must be rejected (fail-closed)"
        );

        // マスク済みスナップショットは元の reason が無いため entry_hash 再計算を
        // スキップし、署名・連鎖・Merkle root の検証は行う。
        let mut masked = SignedAuditSnapshot {
            version: 3,
            reasons_masked: true,
            entries: vec![AuditSnapshotEntry {
                reason: "[redacted:6 bytes]".into(),
                ..snapshot_entry
            }],
            governance_entries: vec![governance.clone()],
            merkle_root: digest.root_hash.clone(),
            signature: String::new(),
        };
        masked.signature = sign_audit_snapshot(&masked, &private_pem).unwrap();
        assert!(verify_audit_snapshot(&masked, &public_der).is_ok());
        // フラグ改ざんは署名検証で検出される。
        masked.reasons_masked = false;
        assert!(verify_audit_snapshot(&masked, &public_der).is_err());
    }

    /// 正常系（未マスク版）: 署名・連鎖・連番・Merkle root がすべて有効なら受理する。
    #[test]
    fn verify_chain_accepts_valid_unmasked_snapshot() {
        let (private_pem, public_der) = test_key_pair();
        let entries = build_chained_entries(3);
        let hashes: Vec<String> = entries
            .iter()
            .map(|entry| entry.entry_hash.clone())
            .collect();
        let snapshot = resign(entries, merkle_root_of(&hashes), &private_pem);

        assert_signature_is_valid(&snapshot, &public_der);
        assert!(
            snapshot.verify_chain().is_ok(),
            "valid unmasked chain must be accepted"
        );
        assert!(verify_audit_snapshot(&snapshot, &public_der).is_ok());
    }

    /// 連番飛ばし: 再署名で署名は有効なまま、verify_chain が non-contiguous で拒否する。
    #[test]
    fn verify_chain_rejects_log_id_gap_with_valid_signature() {
        let (private_pem, public_der) = test_key_pair();
        let mut entries = build_chained_entries(3);
        entries.remove(1); // log_id=2 を欠落させ [1, 3] にする
        assert_eq!(
            entries.iter().map(|entry| entry.log_id).collect::<Vec<_>>(),
            vec![1, 3]
        );
        let hashes: Vec<String> = entries
            .iter()
            .map(|entry| entry.entry_hash.clone())
            .collect();
        // Merkle root は欠落後の entry_hash 列に一致させておき、
        // 連番エラーが Merkle root 不一致にマスクされないようにする。
        let snapshot = resign(entries, merkle_root_of(&hashes), &private_pem);

        assert_signature_is_valid(&snapshot, &public_der);
        assert_chain_error(&snapshot, "non-contiguous log_id: expected 2, got 3");
        assert!(verify_audit_snapshot(&snapshot, &public_der).is_err());
    }

    /// 連鎖改ざん: previous_log_hash を壊しても entry_hash を再計算して再署名すれば
    /// 署名は有効だが、verify_chain が chain mismatch で拒否する。
    #[test]
    fn verify_chain_rejects_broken_previous_hash_with_valid_signature() {
        let (private_pem, public_der) = test_key_pair();
        let mut entries = build_chained_entries(2);
        entries[1].previous_log_hash = "BROKEN_PREVIOUS_HASH".into();
        refresh_entry_hash(&mut entries[1]); // verify_hash を通す
        let hashes: Vec<String> = entries
            .iter()
            .map(|entry| entry.entry_hash.clone())
            .collect();
        let snapshot = resign(entries, merkle_root_of(&hashes), &private_pem);

        assert!(
            snapshot.entries[1].verify_hash(),
            "前提: entry_hash は再計算済みで署名も有効"
        );
        assert_signature_is_valid(&snapshot, &public_der);
        assert_chain_error(&snapshot, "chain mismatch at log_id=2");
        assert!(verify_audit_snapshot(&snapshot, &public_der).is_err());
    }

    /// Merkle root 改ざん: 再署名で署名は有効なまま、verify_chain が
    /// Merkle root mismatch で拒否する。
    #[test]
    fn verify_chain_rejects_broken_merkle_root_with_valid_signature() {
        let (private_pem, public_der) = test_key_pair();
        let entries = build_chained_entries(2);
        let hashes: Vec<String> = entries
            .iter()
            .map(|entry| entry.entry_hash.clone())
            .collect();
        let original_root = merkle_root_of(&hashes);
        let wrong_root = if original_root == "b".repeat(64) {
            "c".repeat(64)
        } else {
            "b".repeat(64)
        };
        let snapshot = resign(entries, wrong_root, &private_pem);

        assert_signature_is_valid(&snapshot, &public_der);
        assert_chain_error(&snapshot, "Merkle root mismatch");
        assert!(verify_audit_snapshot(&snapshot, &public_der).is_err());
    }

    #[test]
    fn mask_reason_hides_content_by_default() {
        assert_eq!(mask_reason("secret details", false), "[redacted:14 bytes]");
        assert_eq!(mask_reason("", false), "");
        assert_eq!(mask_reason("secret details", true), "secret details");
    }

    #[test]
    fn summarize_reason_keeps_empty_and_short_text() {
        assert_eq!(summarize_reason(""), "");
        assert_eq!(summarize_reason("short reason"), "short reason");
        assert_eq!(summarize_reason("日本語の理由"), "日本語の理由");
        // 3 桁の数字は PII ではないため保持する。
        assert_eq!(summarize_reason("code 123"), "code 123");
    }

    #[test]
    fn summarize_reason_truncates_at_char_boundary() {
        let exactly_120 = "a".repeat(120);
        assert_eq!(summarize_reason(&exactly_120), exactly_120);

        let over_120 = "a".repeat(121);
        assert_eq!(summarize_reason(&over_120), "a".repeat(119) + "…");

        // マルチバイト文字の途中で切らず、文字数ベースで詰める。
        let jp = "あ".repeat(121);
        assert_eq!(summarize_reason(&jp), "あ".repeat(119) + "…");
        let mixed = "あい".repeat(60) + "x"; // 121 Unicode 文字 / 241 bytes
        let summarized = summarize_reason(&mixed);
        assert_eq!(summarized.chars().count(), 120);
        assert_eq!(summarized, "あい".repeat(59) + "あ" + "…");
        assert!(summarized.ends_with('…'));
    }

    #[test]
    fn summarize_reason_redacts_pii() {
        assert_eq!(
            summarize_reason("mail: user@example.com"),
            "mail: [redacted]"
        );
        assert_eq!(summarize_reason("card 1234567812345678"), "card [redacted]");
        assert_eq!(
            summarize_reason("see https://example.com/a"),
            "see [redacted]"
        );
        assert_eq!(summarize_reason("http://example.com"), "[redacted]");
        assert_eq!(
            summarize_reason("連絡先 foo@bar.jp 電話 09012345678"),
            "連絡先 [redacted] 電話 [redacted]"
        );
        // 4 桁ちょうどを検出する（`>=4` を `>=5` にする mutation を弾く）。
        assert_eq!(summarize_reason("1234"), "[redacted]");
        // 3 桁は PII ではないので残る。
        assert_eq!(summarize_reason("123"), "123");
    }

    #[test]
    fn summarize_reason_redacts_additional_pii_patterns() {
        // 括弧で始まる URL（トークン内の `://` を検出）。
        assert_eq!(summarize_reason("url(https://x)"), "[redacted]");
        // スキーム無し URL。
        assert_eq!(summarize_reason("www.example.com"), "[redacted]");
        // IPv4 らしいドット区切り（各セグメントが 4 桁未満でも検出）。
        assert_eq!(summarize_reason("10.0.0.1"), "[redacted]");
        // 全角（Unicode）数字の電話番号。
        assert_eq!(summarize_reason("０９０１２３４５６７８"), "[redacted]");
        // 複合ケース: 各パターンと非 PII（3 桁数字）が混在しても個別に置換される。
        assert_eq!(
            summarize_reason(
                "連絡先 foo@bar.jp IP 10.0.0.1 site www.example.com ref url(https://x) tel ０９０１２３４５６７８ code 123"
            ),
            "連絡先 [redacted] IP [redacted] site [redacted] ref [redacted] tel [redacted] code 123"
        );
    }

    #[test]
    fn summarize_reason_respects_limit_after_redaction() {
        let text = format!("{} user@example.com", "a".repeat(110));
        let summarized = summarize_reason(&text);
        assert_eq!(summarized.chars().count(), 120);
        assert!(summarized.ends_with('…'));
        assert!(!summarized.contains("example.com"));
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
