pub mod age;
pub mod env;
pub mod instance_lock;

use rsa::sha2::Sha256 as RsaSha256;
use rsa::{
    Pkcs1v15Sign, PrivateKey, PublicKey,
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePublicKey},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 任意メタデータ・通報理由の上限（文字数ではなく UTF-8 バイト数）。
/// サーバー (`validate_content_metadata` / `mod_report`) とクライアントの送信前ゲートで共有する。
pub const MAX_CONTENT_TITLE_BYTES: usize = 200;
pub const MAX_CONTENT_DESCRIPTION_BYTES: usize = 2000;
pub const MAX_CONTENT_TAG_BYTES: usize = 50;
pub const MAX_CONTENT_TAGS: usize = 10;
pub const MAX_REPORT_REASON_BYTES: usize = 2000;

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

/// PKCS#1 v1.5 署名の受付長（RSA 2048〜4096 bit）。
const MIN_RSA_SIGNATURE_LEN: usize = 256;
const MAX_RSA_SIGNATURE_LEN: usize = 512;
/// 公開鍵 DER の受付上限（2 KiB）。RSA 4096 bit の SPKI でも十分に収まる。
const MAX_PUBLIC_KEY_DER_LEN: usize = 2 * 1024;

pub fn verify_rsa_signature(
    public_key_der: &[u8],
    data: &[u8],
    signature: &[u8],
) -> Result<(), ForensicError> {
    // 既存の不正 DER テストが Crypto("Invalid public key") を期待するため、
    // DER の長さ検査とパースを署名長ガードより先に行う。
    if public_key_der.len() > MAX_PUBLIC_KEY_DER_LEN {
        return Err(ForensicError::Crypto(format!(
            "Invalid public key: DER length {} exceeds {} bytes",
            public_key_der.len(),
            MAX_PUBLIC_KEY_DER_LEN
        )));
    }
    let public_key = PublicKey::from_public_key_der(public_key_der)
        .map_err(|e| ForensicError::Crypto(format!("Invalid public key: {}", e)))?;

    if !(MIN_RSA_SIGNATURE_LEN..=MAX_RSA_SIGNATURE_LEN).contains(&signature.len()) {
        return Err(ForensicError::InvalidSignature(format!(
            "signature length must be {MIN_RSA_SIGNATURE_LEN}..={MAX_RSA_SIGNATURE_LEN} bytes, got {}",
            signature.len()
        )));
    }

    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();

    public_key
        .verify(Pkcs1v15Sign::new::<RsaSha256>(), &digest, signature)
        .map_err(|e| ForensicError::InvalidSignature(e.to_string()))
}

/// base58btc アルファベット（CIDv0 で使用）。
const BASE58_BTC_ALPHABET: &[u8; 58] =
    b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// RFC 4648 base32 の小文字アルファベット（パディング無しの CIDv1 で使用）。
const BASE32_LOWER_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// CID 文字列の受付上限。これを超える入力は解析前に拒否する。
const MAX_CID_LEN: usize = 128;

/// CID が仕様どおりの厳密な形式かどうかを判定する（外部依存なし）。
///
/// - CIDv0: `Qm` + base58btc 44 文字（全体で 46 文字）。文字列全体を base58btc
///   デコードすると 34 バイトの `0x12 0x20` + 32 バイト digest になること。
/// - CIDv1: `b` + パディング無し base32lower。デコード結果が
///   varint version=1 / codec=0x55(raw), 0x70(dag-pb), 0x71(dag-cbor) /
///   multihash=0x12(sha2-256) / digest 長 32 / 32 バイト digest で終わること。
///
/// 空文字・不正文字・上限超過・末尾の余りバイトはすべて拒否する。
/// digest の内容（元データとのハッシュ一致）は検証しない。
pub fn is_valid_cid(cid: &str) -> bool {
    if cid.is_empty() || cid.len() > MAX_CID_LEN {
        return false;
    }

    if cid.starts_with("Qm") {
        return cid.len() == 46
            && cid.as_bytes()[2..]
                .iter()
                .all(|byte| BASE58_BTC_ALPHABET.contains(byte))
            && decode_base58_btc(cid).is_some_and(|bytes| is_valid_v0_bytes(&bytes));
    }

    if let Some(body) = cid.strip_prefix('b') {
        return decode_base32_lower(body).is_some_and(|bytes| is_valid_v1_bytes(&bytes));
    }

    false
}

/// base58btc 文字列を自前でデコードする（外部依存なし）。
///
/// 先頭の `1` はゼロバイトとして扱う。不正文字が含まれる場合は `None`。
fn decode_base58_btc(input: &str) -> Option<Vec<u8>> {
    if input.is_empty() {
        return None;
    }

    let leading_zeros = input.bytes().take_while(|&byte| byte == b'1').count();
    // 数値部をリトルエンディアンで保持し、58 倍して桁を加えていく。
    // 46 文字でも高々 34 バイトなので、入力長 + 1 バイトで十分に収まる。
    let mut magnitude = Vec::with_capacity(input.len() + 1);
    for &ch in input.as_bytes() {
        let digit = BASE58_BTC_ALPHABET.iter().position(|&c| c == ch)? as u16;
        let mut carry = digit;
        for byte in &mut magnitude {
            let value = u16::from(*byte) * 58 + carry;
            *byte = value as u8;
            carry = value >> 8;
        }
        while carry > 0 {
            magnitude.push(carry as u8);
            carry >>= 8;
        }
    }

    let mut decoded = Vec::with_capacity(leading_zeros + magnitude.len());
    decoded.resize(leading_zeros, 0);
    // magnitude はリトルエンディアンなので、base58 の桁順（ビッグエンディアン）に直す。
    decoded.extend(magnitude.iter().rev());
    Some(decoded)
}

/// CIDv0 のデコード済みバイト列が sha2-256 / 32 バイト digest の multihash か判定する。
fn is_valid_v0_bytes(bytes: &[u8]) -> bool {
    bytes.len() == 34 && bytes.starts_with(&[0x12u8, 0x20])
}

/// パディング無し base32lower をデコードする。
/// 不正文字・非正規の末尾ビット・途中で切れた文字列は拒否する。
fn decode_base32_lower(input: &str) -> Option<Vec<u8>> {
    if input.is_empty() {
        return None;
    }

    let mut decoded = Vec::with_capacity(input.len() * 5 / 8);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &ch in input.as_bytes() {
        let value = BASE32_LOWER_ALPHABET.iter().position(|&c| c == ch)? as u32;
        acc = (acc << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            decoded.push((acc >> bits) as u8);
        }
    }

    // 残りビットが 5 以上だと 1 バイトに足りない（文字数 mod 8 が 1/3/6）。
    if bits >= 5 {
        return None;
    }
    // パディングに相当する末尾ビットは 0 でなければ非正規表現として拒否する。
    if bits > 0 && acc & ((1u32 << bits) - 1) != 0 {
        return None;
    }

    Some(decoded)
}

/// CIDv1 のデコード済みバイト列が、許可された形かどうかを判定する。
fn is_valid_v1_bytes(bytes: &[u8]) -> bool {
    let mut pos = 0;

    let Some(version) = read_varint(bytes, &mut pos) else {
        return false;
    };
    if version != 1 {
        return false;
    }

    let Some(codec) = read_varint(bytes, &mut pos) else {
        return false;
    };
    if codec != 0x55 && codec != 0x70 && codec != 0x71 {
        return false;
    }

    let Some(hash_code) = read_varint(bytes, &mut pos) else {
        return false;
    };
    if hash_code != 0x12 {
        return false;
    }

    let Some(digest_len) = read_varint(bytes, &mut pos) else {
        return false;
    };
    if digest_len != 32 {
        return false;
    }

    // read_varint は読み取った分しか pos を進めないため、ここでは pos <= bytes.len()。
    // digest の後ろに余りバイトがあってはならない。
    bytes.len() - pos == 32
}

/// 符号なし LEB128 (varint) を 1 つ読み、位置を進める。
/// 10 バイト超・u64 オーバーフロー・非最小表現は拒否する。
fn read_varint(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    let mut count = 0;
    loop {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        count += 1;
        if count > 10 {
            return None;
        }
        let payload = (byte & 0x7f) as u64;
        if shift == 63 && payload > 1 {
            return None;
        }
        value |= payload << shift;
        if byte & 0x80 == 0 {
            // 複数バイトで 0 を表す等非最小表現は受け付けない。
            if count > 1 && payload == 0 {
                return None;
            }
            return Some(value);
        }
        shift += 7;
    }
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

/// `--expect-root` 不一致時の固定エラー文言。
/// expected / computed の root 値は一切含めない（非エコー契約）。
/// verify_audit_log / verify_audit_export の両 CLI で共有し、片方だけの文言変更を構造的に防ぐ。
pub const AUDIT_ROOT_MISMATCH: &str = "audit root mismatch (expected and computed roots withheld)";

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

/// `mask_reason(reason, false)` が生成し得る値か。
/// 平文はスナップショットに残らないため、再適用による一致は検証できず、
/// 空文字または `[redacted:<ASCII digits> bytes]` の完全一致で判定する。
/// 接頭辞だけを見ると平文がマスク済みと誤判定されるため、末尾と数字部も検証する。
fn reason_is_masked(reason: &str) -> bool {
    if reason.is_empty() {
        return true;
    }
    let Some(inner) = reason.strip_prefix("[redacted:") else {
        return false;
    };
    let Some(digits) = inner.strip_suffix(" bytes]") else {
        return false;
    };
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
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
    /// 単体呼び出しでも未サポート版を受理しないよう、先頭でバージョンを検証する。
    pub fn verify_chain(&self) -> Result<(), ForensicError> {
        if self.version != AUDIT_SNAPSHOT_VERSION {
            return Err(ForensicError::InvalidSignature(format!(
                "unsupported audit snapshot version: {}",
                self.version
            )));
        }
        // reasons_masked=true のとき、すべての entry の reason がマスク済みであることを検証する。
        // 署名対象 `signature_payload()` は reasons_masked=true のとき entries_json を
        // "masked" 印付きで含めるため、この検査は署名対象に含まれるフィールドの整合性確認であり、
        // 署名検証結果と矛盾しない（署名時に sign_audit_snapshot も同じ判定で拒否する）。
        // entry_hash は元の理由が無いためマスク版では再計算しない（下の分岐を参照）。
        if self.reasons_masked {
            for entry in &self.entries {
                if !reason_is_masked(&entry.reason) {
                    return Err(ForensicError::InvalidSignature(format!(
                        "unmasked entry reason with reasons_masked=true at log_id={}",
                        entry.log_id
                    )));
                }
            }
        }
        let mut expected_prev = "GENESIS_HASH".to_string();
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
    // reasons_masked=true で平文 reason が残っていると、検証側は entry_hash の
    // 再計算をスキップするため検出できない。署名時に拒否する。
    // governance_entries は既存の正常スナップショットに平文が残り得るため対象外。
    if snapshot.reasons_masked {
        for entry in &snapshot.entries {
            if !reason_is_masked(&entry.reason) {
                return Err(ForensicError::InvalidSignature(format!(
                    "unmasked entry reason with reasons_masked=true at log_id={}",
                    entry.log_id
                )));
            }
        }
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

    /// シードを指定して決定論的に RSA 鍵ペアを生成する（既存テストと共有）。
    fn test_key_pair_from_seed(seed: u64) -> (String, Vec<u8>) {
        use rsa::pkcs8::EncodePrivateKey;
        let mut rng = TestRng(seed);
        let key = PrivateKey::new(&mut rng, 2048).unwrap();
        let private_pem = key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap()
            .to_string();
        let public_der = public_key_der_from_private_pem(&private_pem).unwrap();
        (private_pem, public_der)
    }

    fn test_key_pair() -> (String, Vec<u8>) {
        test_key_pair_from_seed(0x9E37_79B9_7F4A_7C15)
    }

    /// 生データへ PKCS#1 v1.5 + SHA-256 署名する（署名検証エラーパスの対照用）。
    /// rsa::signature::Signer は入力をハッシュするため、verify_rsa_signature と同じ
    /// ダイジェストが使われる。
    fn sign_bytes(private_pem: &str, data: &[u8]) -> Vec<u8> {
        use rsa::signature::{SignatureEncoding, Signer};
        let key = PrivateKey::from_pkcs8_pem(private_pem).unwrap();
        let signing_key = rsa::pkcs1v15::SigningKey::<RsaSha256>::new(key);
        signing_key.sign(data).to_bytes().to_vec()
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

    // ---- 監査で「中リスク・未テスト」と指摘された暗号検証のエラーパス ----

    /// 監査対象 1: 不正 DER 公開鍵は Crypto("Invalid public key") で拒否される。
    #[test]
    fn verify_rsa_signature_rejects_invalid_public_key_der() {
        let data = b"forensic payload";
        let signature = b"any signature bytes";
        // 空、ASN.1 として不正、バイト列だけ与えたゴミ、の 3 系統。
        let invalid_ders: [&[u8]; 3] = [b"", b"not a DER key", &[0xFF; 256]];
        for der in invalid_ders {
            let error = verify_rsa_signature(der, data, signature)
                .expect_err("invalid DER public key must be rejected");
            assert!(
                matches!(&error, ForensicError::Crypto(message) if message.contains("Invalid public key")),
                "expected Crypto(Invalid public key), got {error:?}"
            );
        }
    }

    /// 監査対象 1: 別鍵で検証すると InvalidSignature になる（正鍵は通る対照付き）。
    #[test]
    fn verify_rsa_signature_rejects_wrong_key() {
        let (private_pem, public_der) = test_key_pair();
        let (_, wrong_public_der) = test_key_pair_from_seed(0xDEAD_BEEF_0BAD_F00D);
        assert_ne!(
            public_der, wrong_public_der,
            "test precondition: the two key pairs must differ"
        );
        let data = b"forensic payload for wrong-key check";
        let signature = sign_bytes(&private_pem, data);

        assert!(
            verify_rsa_signature(&public_der, data, &signature).is_ok(),
            "control: signature must verify with the correct public key"
        );
        let error = verify_rsa_signature(&wrong_public_der, data, &signature)
            .expect_err("signature must not verify with a different public key");
        assert!(
            matches!(&error, ForensicError::InvalidSignature(_)),
            "expected InvalidSignature, got {error:?}"
        );
    }

    /// 監査対象 1: 署名バイトの欠落・反転・ゼロ埋めは InvalidSignature になる。
    #[test]
    fn verify_rsa_signature_rejects_invalid_signature_bytes() {
        let (private_pem, public_der) = test_key_pair();
        let data = b"forensic payload for signature-byte check";
        let valid = sign_bytes(&private_pem, data);
        assert!(verify_rsa_signature(&public_der, data, &valid).is_ok());

        let mut bit_flipped = valid.clone();
        bit_flipped[0] ^= 0x01;
        let zeroed = vec![0u8; valid.len()];
        let invalid_signatures: [&[u8]; 4] =
            [b"", &valid[..valid.len() - 1], &bit_flipped, &zeroed];
        for signature in invalid_signatures {
            let error = verify_rsa_signature(&public_der, data, signature)
                .expect_err("tampered signature bytes must be rejected");
            assert!(
                matches!(&error, ForensicError::InvalidSignature(_)),
                "expected InvalidSignature for {signature:?}, got {error:?}"
            );
        }
    }

    /// 署名長ガード: 256..=512 バイトのみ受理し、範囲外は InvalidSignature で拒否する。
    #[test]
    fn verify_rsa_signature_enforces_signature_length_bounds() {
        let (private_pem, public_der) = test_key_pair();
        let data = b"signature length boundary";
        let valid = sign_bytes(&private_pem, data);
        assert_eq!(valid.len(), 256, "control: RSA 2048 signature is 256 bytes");

        // 空・255・513 バイトは正しい公開鍵でも署名長ガードで拒否する。
        let too_short = vec![0u8; 255];
        let too_long = vec![0u8; 513];
        for signature in [&[][..], &too_short[..], &too_long[..]] {
            let error = verify_rsa_signature(&public_der, data, signature)
                .expect_err("out-of-range signature length must be rejected");
            assert!(
                matches!(&error, ForensicError::InvalidSignature(message)
                    if message.contains("signature length must be")),
                "expected length-guard error for {} bytes, got {error:?}",
                signature.len()
            );
        }

        // 境界値 256 バイトはガードを通過して検証へ進む（正常系は受理）。
        assert!(verify_rsa_signature(&public_der, data, &valid).is_ok());
        let mut tampered = valid.clone();
        tampered[0] ^= 0x01;
        let error = verify_rsa_signature(&public_der, data, &tampered).unwrap_err();
        assert!(
            matches!(&error, ForensicError::InvalidSignature(message)
                if !message.contains("signature length must be")),
            "256-byte signature must reach verification, got {error:?}"
        );
    }

    /// 公開鍵 DER が 2 KiB を超える場合は署名検証前に拒否する。
    #[test]
    fn verify_rsa_signature_rejects_oversized_public_key_der() {
        let der = vec![0u8; MAX_PUBLIC_KEY_DER_LEN + 1];
        let error = verify_rsa_signature(&der, b"oversized der", &[0u8; 256])
            .expect_err("oversized public key DER must be rejected");
        assert!(
            matches!(&error, ForensicError::Crypto(message)
                if message.contains("Invalid public key") && message.contains("DER length")),
            "expected oversized-DER rejection, got {error:?}"
        );
    }

    // ---- RSA-3072 / RSA-4096 の正当な署名長が誤拒否されないことの担保 ----

    /// 既存の決定論的 RNG を使って、指定ビット数の鍵ペアを生成する（使い回し可能な共通ヘルパー）。
    fn test_key_pair_from_seed_with_bits(seed: u64, bits: usize) -> (String, Vec<u8>) {
        use rsa::pkcs8::EncodePrivateKey;
        let mut rng = TestRng(seed);
        let key = PrivateKey::new(&mut rng, bits).unwrap();
        let private_pem = key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap()
            .to_string();
        let public_der = public_key_der_from_private_pem(&private_pem).unwrap();
        (private_pem, public_der)
    }

    /// テスト専用: RSA 鍵生成なしで構造的に正しい SPKI DER を組み立てる。
    ///
    /// n = 2^bits - 1（奇数）, e = 65537 なので `PublicKey::new` の検証を通り、
    /// `to_public_key_der()` で実際の SPKI DER が得られる。署名検証自体は通らないが、
    /// 「署名長ガードと DER 上限を通過したか」だけを確認する目的には十分である。
    /// RSA-4096 の鍵生成が重い場合の代替手段として 4096 bit に使う。
    fn synthetic_spki_der(bits: usize) -> Vec<u8> {
        use rsa::pkcs8::EncodePublicKey;
        let modulus_bytes = vec![0xFF_u8; bits / 8];
        let modulus = rsa::BoxedUint::from_be_slice(&modulus_bytes, bits as u32)
            .expect("synthetic modulus must fit its precision");
        let exponent = rsa::BoxedUint::from(65_537_u64);
        let public_key = rsa::PublicKey::new(modulus, exponent)
            .expect("synthetic RSA public key components must be structurally valid");
        public_key
            .to_public_key_der()
            .expect("SPKI DER encoding of the synthetic RSA public key must succeed")
            .as_ref()
            .to_vec()
    }

    /// ダミー署名が「署名長ガード」を通過して暗号検証まで到達したことを確認する。
    ///
    /// ダミーなので最終結果は必ず InvalidSignature だが、ガードで拒否された場合は
    /// メッセージに "signature length must be" が含まれるため、両者を分離できる。
    fn assert_passes_length_guard(der: &[u8], data: &[u8], signature: &[u8]) {
        let error = verify_rsa_signature(der, data, signature)
            .expect_err("dummy signature must reach verification and then fail");
        assert!(
            matches!(&error, ForensicError::InvalidSignature(message)
                if !message.contains("signature length must be")),
            "signature of {} bytes must pass the length guard, got {error:?}",
            signature.len()
        );
    }

    /// RSA-3072（384 バイト）の正当な署名が受理される。
    /// 上限を 384 未満に誤変更すると署名長ガードで拒否され、このテストは失敗する。
    #[test]
    fn verify_rsa_signature_accepts_3072_bit_signature() {
        let (private_pem, public_der) = test_key_pair_from_seed_with_bits(0x3072_5EED, 3072);
        let data = b"rsa-3072 signature length guard check";
        let signature = sign_bytes(&private_pem, data);

        assert_eq!(signature.len(), 384, "RSA-3072 signature must be 384 bytes");
        assert!(
            public_der.len() <= MAX_PUBLIC_KEY_DER_LEN,
            "RSA-3072 SPKI DER must fit the 2 KiB limit, got {}",
            public_der.len()
        );
        assert!(
            verify_rsa_signature(&public_der, data, &signature).is_ok(),
            "legitimate RSA-3072 signature must be accepted"
        );
    }

    /// RSA-4096（512 バイト）の署名長が上限 `..=512` で受理される。
    /// 上限を 511 / 384 に誤変更するとガードエラーへ変わり、このテストは失敗する。
    #[test]
    fn verify_rsa_signature_accepts_4096_bit_signature_length() {
        // ガードの契約そのものも明示する（境界値リテラルの mutation 検出）。
        assert_eq!((MIN_RSA_SIGNATURE_LEN, MAX_RSA_SIGNATURE_LEN), (256, 512));

        let der = synthetic_spki_der(4096);
        let signature = vec![0u8; MAX_RSA_SIGNATURE_LEN];
        assert_eq!(signature.len(), 512, "RSA-4096 signature must be 512 bytes");
        // 上限が 511 や 384 に下げられていれば、ここでガードエラーになる。
        assert_passes_length_guard(&der, b"rsa-4096 signature length guard check", &signature);
    }

    /// RSA-4096 の SPKI 公開鍵 DER は約 550 バイトで、2 KiB 上限に十分収まる。
    /// 上限を 512 バイト等に誤変更すると、このテストは失敗する。
    #[test]
    fn rsa4096_spki_der_fits_under_2kib_limit() {
        let der = synthetic_spki_der(4096);
        assert!(
            (500..=600).contains(&der.len()),
            "4096-bit SPKI DER should be about 550 bytes, got {}",
            der.len()
        );
        assert!(
            der.len() <= MAX_PUBLIC_KEY_DER_LEN,
            "4096-bit SPKI DER must fit the 2 KiB limit, got {}",
            der.len()
        );

        // 振る舞いでも確認する。DER 上限が下がっていれば Crypto エラーになる。
        assert_passes_length_guard(&der, b"rsa-4096 der limit check", &[0u8; 512]);
    }

    /// 監査対象 2: 不正 base64 署名は署名検証より前に InvalidSignature で拒否される。
    #[test]
    fn verify_audit_snapshot_rejects_invalid_base64_signature() {
        let (private_pem, public_der) = test_key_pair();
        let entries = build_chained_entries(1);
        let hashes: Vec<String> = entries
            .iter()
            .map(|entry| entry.entry_hash.clone())
            .collect();
        let valid = resign(entries, merkle_root_of(&hashes), &private_pem);
        assert!(valid.verify_chain().is_ok(), "control: chain must be valid");
        assert!(verify_audit_snapshot(&valid, &public_der).is_ok());

        for broken in ["!!!not-base64!!!", "@@@@", "A"] {
            let mut snapshot = valid.clone();
            snapshot.signature = broken.into();
            let error = verify_audit_snapshot(&snapshot, &public_der)
                .expect_err("invalid base64 signature must be rejected");
            assert!(
                matches!(&error, ForensicError::InvalidSignature(message) if message.contains("Invalid base64")),
                "expected Invalid base64 error for {broken:?}, got {error:?}"
            );
        }
    }

    /// 監査対象 2: 正しい署名でも別鍵の公開鍵では検証に失敗する。
    #[test]
    fn verify_audit_snapshot_rejects_wrong_key() {
        let (private_pem, public_der) = test_key_pair();
        let (_, wrong_public_der) = test_key_pair_from_seed(0xDEAD_BEEF_0BAD_F00D);
        assert_ne!(public_der, wrong_public_der);

        let entries = build_chained_entries(2);
        let hashes: Vec<String> = entries
            .iter()
            .map(|entry| entry.entry_hash.clone())
            .collect();
        let snapshot = resign(entries, merkle_root_of(&hashes), &private_pem);

        assert!(
            snapshot.verify_chain().is_ok(),
            "control: chain must be valid"
        );
        assert!(verify_audit_snapshot(&snapshot, &public_der).is_ok());
        let error = verify_audit_snapshot(&snapshot, &wrong_public_der)
            .expect_err("signature must not verify with a different public key");
        assert!(
            matches!(&error, ForensicError::InvalidSignature(_)),
            "expected InvalidSignature, got {error:?}"
        );
    }

    /// 監査対象 3: 未サポートバージョンは署名生成時に拒否される（fail-closed）。
    #[test]
    fn sign_audit_snapshot_rejects_unsupported_version() {
        let (private_pem, _) = test_key_pair();
        let entries = build_chained_entries(1);
        let hashes: Vec<String> = entries
            .iter()
            .map(|entry| entry.entry_hash.clone())
            .collect();
        let valid = resign(entries, merkle_root_of(&hashes), &private_pem);
        assert!(!valid.signature.is_empty(), "control: signing must succeed");

        for version in [0, 2, AUDIT_SNAPSHOT_VERSION + 1, 999] {
            let mut snapshot = valid.clone();
            snapshot.version = version;
            let error = sign_audit_snapshot(&snapshot, &private_pem)
                .expect_err("unsupported version must be rejected");
            assert!(
                matches!(&error, ForensicError::InvalidSignature(message)
                    if message.contains("unsupported audit snapshot version")
                        && message.contains(&version.to_string())),
                "expected version rejection for {version}, got {error:?}"
            );
        }
    }

    /// reasons_masked=true なのに平文 reason が残るスナップショットは署名時に拒否する。
    #[test]
    fn sign_audit_snapshot_rejects_plaintext_reason_when_masked() {
        let (private_pem, _) = test_key_pair();
        let entries = build_chained_entries(2);
        let hashes: Vec<String> = entries
            .iter()
            .map(|entry| entry.entry_hash.clone())
            .collect();
        let mut snapshot = SignedAuditSnapshot {
            version: AUDIT_SNAPSHOT_VERSION,
            reasons_masked: true,
            entries,
            governance_entries: Vec::new(),
            merkle_root: merkle_root_of(&hashes),
            signature: String::new(),
        };
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| !reason_is_masked(&entry.reason)),
            "control: plaintext reasons must be present for this check"
        );

        let error = sign_audit_snapshot(&snapshot, &private_pem)
            .expect_err("plaintext reason with reasons_masked=true must be rejected");
        assert!(
            matches!(&error, ForensicError::InvalidSignature(message)
                if message.contains("reasons_masked=true") && message.contains("log_id=1")),
            "expected masked-flag inconsistency error, got {error:?}"
        );

        // 対照: mask_reason でマスクし直せば署名でき、連鎖検証も通る。
        // entry_hash は据え置き（reasons_masked=true では再計算しない）。
        for entry in &mut snapshot.entries {
            entry.reason = mask_reason(&entry.reason, false);
        }
        snapshot.signature =
            sign_audit_snapshot(&snapshot, &private_pem).expect("masked reasons must be signable");
        assert!(snapshot.verify_chain().is_ok());

        // 空文字（mask_reason("") の結果）も正当なマスク済み値として受理する。
        let mut empty_reason = snapshot.clone();
        empty_reason.entries[1].reason = String::new();
        empty_reason.signature = String::new();
        assert!(sign_audit_snapshot(&empty_reason, &private_pem).is_ok());
    }

    /// 監査対象 4: verify_hash は保存済み entry_hash と v2 再計算値の一致だけを判定する。
    #[test]
    fn audit_snapshot_entry_verify_hash_accepts_match_and_rejects_mismatch() {
        let entries = build_chained_entries(1);
        let entry = &entries[0];
        assert!(
            entry.verify_hash(),
            "control: freshly built entry must match"
        );

        let mut tampered = entry.clone();
        tampered.reason = "tampered reason".into();
        assert!(
            !tampered.verify_hash(),
            "changed reason must invalidate the stored hash"
        );

        let mut wrong_hash = entry.clone();
        wrong_hash.entry_hash = "0".repeat(64);
        assert!(
            !wrong_hash.verify_hash(),
            "wrong stored hash must be rejected"
        );

        // v1 ハッシュ（区切りなし連結）を入れても v2 検証では受理されない。
        let mut v1_hash = entry.clone();
        let log = AuditLogEntry {
            log_id: Some(entry.log_id),
            cid: entry.cid.clone(),
            old_status: entry.old_status.clone(),
            new_status: entry.new_status.clone(),
            changed_by: entry.changed_by.clone(),
            reason: entry.reason.clone(),
            previous_log_hash: entry.previous_log_hash.clone(),
        };
        v1_hash.entry_hash = log.calculate_hash();
        assert!(
            !v1_hash.verify_hash(),
            "v1 hash must not satisfy v2 verification"
        );
    }

    /// 監査対象 5: 不正 PEM は Crypto("Invalid private key") で拒否される。
    #[test]
    fn public_key_der_from_private_pem_rejects_invalid_pem() {
        let (private_pem, public_der) = test_key_pair();
        assert!(!public_der.is_empty(), "control: valid PEM must yield DER");

        // 正しい PEM の base64 本体を 1 文字だけ壊したものも混入させる。
        let mut corrupted_lines: Vec<String> = private_pem.lines().map(str::to_string).collect();
        let first_char = corrupted_lines[1].chars().next().unwrap();
        let replacement = if first_char == 'A' { 'B' } else { 'A' };
        corrupted_lines[1].replace_range(0..1, &replacement.to_string());
        let corrupted_pem = corrupted_lines.join("\n");

        for pem in ["", "not a PEM", corrupted_pem.as_str()] {
            let error = public_key_der_from_private_pem(pem)
                .expect_err("invalid private key PEM must be rejected");
            assert!(
                matches!(&error, ForensicError::Crypto(message) if message.contains("Invalid private key")),
                "expected Crypto(Invalid private key) for {pem:?}, got {error:?}"
            );
        }
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

    /// verify_chain を単体で呼んでも未サポート版は拒否される。
    #[test]
    fn verify_chain_rejects_unsupported_version() {
        let (private_pem, _) = test_key_pair();
        let entries = build_chained_entries(1);
        let hashes: Vec<String> = entries
            .iter()
            .map(|entry| entry.entry_hash.clone())
            .collect();
        let valid = resign(entries, merkle_root_of(&hashes), &private_pem);
        assert!(
            valid.verify_chain().is_ok(),
            "control: v3 chain must be valid"
        );

        for version in [0, 2, AUDIT_SNAPSHOT_VERSION + 1, 999] {
            let mut snapshot = valid.clone();
            snapshot.version = version;
            assert_chain_error(&snapshot, "unsupported audit snapshot version");
        }
    }

    #[test]
    fn mask_reason_hides_content_by_default() {
        assert_eq!(mask_reason("secret details", false), "[redacted:14 bytes]");
        assert_eq!(mask_reason("", false), "");
        assert_eq!(mask_reason("secret details", true), "secret details");
    }

    #[test]
    fn reason_is_masked_accepts_only_canonical_masked_form() {
        // (a) mask_reason の出力はマスク済みと判定される。
        assert!(reason_is_masked(&mask_reason("secret", false)));
        // (b) 正規形 `[redacted:<digits> bytes]` はマスク済み。
        assert!(reason_is_masked("[redacted:123 bytes]"));
        // (c) 数字部が整数でない場合は平文扱い。
        assert!(!reason_is_masked("[redacted:not-a-number bytes]"));
        // (d) 正規形に平文が続く場合は平文扱い（prefix 一致だけでは誤判定する）。
        assert!(!reason_is_masked("[redacted:12 bytes] and more plaintext"));
        // (e) 末尾の ` bytes]` が欠落する場合は平文扱い。
        assert!(!reason_is_masked("[redacted:12 bytes"));
        // 空文字（mask_reason("") の結果）と include=true の平文も区別する。
        assert!(reason_is_masked(""));
        assert!(!reason_is_masked(&mask_reason("secret", true)));
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

    // ---- CID 厳密検証 (is_valid_cid) ----

    /// リポジトリ内の監査ログ例にも登場する既知の CIDv1。
    const VALID_CIDV1: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

    /// テスト用: バイト列をパディング無し base32lower に符号化する。
    fn encode_base32_lower(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
        let mut encoded = String::new();
        let mut acc: u32 = 0;
        let mut bits: u32 = 0;
        for &byte in bytes {
            acc = (acc << 8) | u32::from(byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                encoded.push(ALPHABET[((acc >> bits) & 0x1f) as usize] as char);
            }
        }
        if bits > 0 {
            encoded.push(ALPHABET[((acc << (5 - bits)) & 0x1f) as usize] as char);
        }
        encoded
    }

    /// テスト用: version=1 / mh=0x12 / digest 長 32 の 36 バイト列を作る。
    fn v1_bytes(codec: u8) -> Vec<u8> {
        let mut raw = vec![1, codec, 0x12, 32];
        raw.extend(core::array::from_fn::<u8, 32, _>(|i| i as u8));
        raw
    }

    fn cid_from_bytes(raw: &[u8]) -> String {
        format!("b{}", encode_base32_lower(raw))
    }

    #[test]
    fn is_valid_cid_accepts_known_cidv0() {
        // 使用前に前提を自前で検証する（46 文字 / Qm / 残り 44 文字が base58btc）。
        const CIDV0: &str = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5AXQfD";
        assert_eq!(CIDV0.len(), 46);
        assert!(CIDV0.starts_with("Qm"));
        assert!(
            CIDV0.as_bytes()[2..]
                .iter()
                .all(|byte| BASE58_BTC_ALPHABET.contains(byte))
        );
        // 全体を base58btc デコードすると sha2-256 multihash（34 バイト）になる。
        let decoded = decode_base58_btc(CIDV0).expect("known CIDv0 must decode");
        assert_eq!(decoded.len(), 34);
        assert_eq!(&decoded[..2], &[0x12, 0x20]);
        assert!(is_valid_cid(CIDV0));
    }

    /// prefix チェックを外す mutation を検出するためのテスト。
    /// `Qm` + `1`×44 は 34 バイトにデコードされるが、先頭が 0x12 0x1e で不正。
    #[test]
    fn is_valid_cid_rejects_cidv0_with_wrong_multihash_prefix() {
        let bad = format!("Qm{}", "1".repeat(44));
        let decoded = decode_base58_btc(&bad).expect("must decode to 34 bytes");
        assert_eq!(decoded.len(), 34, "test precondition: length check passes");
        assert_ne!(
            &decoded[..2],
            &[0x12, 0x20],
            "test precondition: wrong prefix"
        );
        assert!(
            !is_valid_cid(&bad),
            "base58 string without sha2-256 prefix must be rejected"
        );
    }

    /// デコード長が 34 バイトでない CIDv0 は拒否される。
    #[test]
    fn is_valid_cid_rejects_cidv0_with_wrong_decoded_length() {
        // デコード済みバイト列そのものを判定するヘルパーを直接叩く。
        assert!(!is_valid_v0_bytes(&[]));
        assert!(!is_valid_v0_bytes(&[0x12, 0x20]));
        let short = [0x12, 0x20]
            .into_iter()
            .chain([0u8; 31])
            .collect::<Vec<_>>();
        assert_eq!(short.len(), 33);
        assert!(!is_valid_v0_bytes(&short));
        let long = [0x12, 0x20]
            .into_iter()
            .chain([0u8; 33])
            .collect::<Vec<_>>();
        assert_eq!(long.len(), 35);
        assert!(!is_valid_v0_bytes(&long));

        // Qm 文字列側の長さガードも残っている。
        assert!(!is_valid_cid(&format!("Qm{}", "1".repeat(43))));
        assert!(!is_valid_cid(&format!("Qm{}", "1".repeat(45))));
    }

    #[test]
    fn is_valid_cid_accepts_known_cidv1() {
        // デコード結果が version=1 / dag-pb / sha2-256 / digest 32 バイトであることを確認。
        let decoded = decode_base32_lower(&VALID_CIDV1[1..]).expect("known CIDv1 must decode");
        assert_eq!(decoded.len(), 36);
        assert_eq!(&decoded[..4], &[1, 0x70, 0x12, 32]);
        assert!(is_valid_cid(VALID_CIDV1));

        // raw codec (0x55) も許容される。
        let raw_cid = cid_from_bytes(&v1_bytes(0x55));
        assert!(
            is_valid_cid(&raw_cid),
            "raw CIDv1 must be accepted: {raw_cid}"
        );
    }

    /// dag-cbor codec (0x71) の CIDv1 も受理される（既存ヘルパーで自前構築）。
    #[test]
    fn is_valid_cid_accepts_dag_cbor_v1() {
        let dag_cbor_cid = cid_from_bytes(&v1_bytes(0x71));
        let decoded = decode_base32_lower(&dag_cbor_cid[1..]).expect("must decode");
        assert_eq!(decoded.len(), 36);
        assert_eq!(&decoded[..4], &[1, 0x71, 0x12, 32]);
        assert!(
            is_valid_cid(&dag_cbor_cid),
            "dag-cbor CIDv1 must be accepted: {dag_cbor_cid}"
        );
    }

    #[test]
    fn is_valid_cid_rejects_invalid_plain_strings() {
        let mut invalid: Vec<String> = [
            "",
            "QmTestCID",
            "../etc/passwd",
            "?etc/passwd",
            "#etc/passwd",
            "/etc/passwd",
            "b",
            "b0189",
            "b!!!!",
            "aafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();

        // 区切り文字の混入（先頭・末尾）。
        for sep in ['?', '#', '/'] {
            invalid.push(format!("{sep}{VALID_CIDV1}"));
            invalid.push(format!("{VALID_CIDV1}{sep}"));
        }
        // base58 に存在しない文字。
        for bad in ['0', 'O', 'I', 'l'] {
            invalid.push(format!("Qm{}", bad.to_string().repeat(44)));
        }
        // 長さ違反（46 文字でない Qm / 上限 128 文字超）。
        // 47 文字の base58 文字列は上限ガードにマスクされず、CIDv0 の
        // ちょうど 46 文字というチェックだけが効くケースとして残す。
        invalid.push(format!("Qm{}", "a".repeat(44) + "x"));
        invalid.push(format!("{VALID_CIDV1}x"));
        invalid.push("b".to_string() + &"a".repeat(200));
        invalid.push("a".repeat(129));
        invalid.push("b".repeat(129));
        invalid.push("Qm".to_string() + &"a".repeat(127));
        invalid.push("Qm".to_string() + &"a".repeat(200));

        for cid in invalid {
            assert!(!is_valid_cid(&cid), "must reject {cid:?}");
        }
    }

    #[test]
    fn is_valid_cid_rejects_wrong_v1_structure() {
        let mut cases: Vec<(&str, String)> = Vec::new();

        // 不正 version（version チェックの mutation を弾く）。
        for version in [0u8, 2, 0x7f] {
            let mut raw = v1_bytes(0x70);
            raw[0] = version;
            cases.push(("version", cid_from_bytes(&raw)));
        }
        // 不正 codec（raw(0x55)/dag-pb(0x70)/dag-cbor(0x71) 以外）。
        for codec in [0u8, 0x54, 0x72, 0x7f] {
            cases.push(("codec", cid_from_bytes(&v1_bytes(codec))));
        }
        // 不正 multihash code。
        let mut raw = v1_bytes(0x70);
        raw[2] = 0x13;
        cases.push(("multihash", cid_from_bytes(&raw)));
        // 不正 digest 長。
        for len in [0u8, 31, 33] {
            let mut raw = v1_bytes(0x70);
            raw[3] = len;
            cases.push(("digest_len", cid_from_bytes(&raw)));
        }
        // digest が 1 バイト足りない / 余分にある。
        let mut short = v1_bytes(0x70);
        short.pop();
        cases.push(("short digest", cid_from_bytes(&short)));
        let mut long = v1_bytes(0x70);
        long.push(0);
        cases.push(("trailing byte", cid_from_bytes(&long)));
        // 非最小 varint（version=1 を 0x81 0x00 で表現）。
        let mut non_minimal = vec![0x81, 0x00, 0x70, 0x12, 32];
        non_minimal.extend([0u8; 32]);
        cases.push(("non-minimal varint", cid_from_bytes(&non_minimal)));

        for (name, cid) in cases {
            assert!(!is_valid_cid(&cid), "{name} must be rejected: {cid}");
        }
    }

    #[test]
    fn is_valid_cid_rejects_invalid_base32_and_noncanonical_padding() {
        let valid = cid_from_bytes(&v1_bytes(0x70));
        assert!(
            is_valid_cid(&valid),
            "control: generated CIDv1 must be valid"
        );

        // なお 'l' は base32lower の有効文字なのでここには含めない。
        for ch in ['0', '1', '8', '9', '=', 'A', 'O', 'I', '/', '?', '#'] {
            let mut broken = valid.clone();
            broken.replace_range(10..11, &ch.to_string());
            assert!(
                !is_valid_cid(&broken),
                "invalid base32 char {ch:?} must be rejected"
            );
        }

        // 末尾 2 ビットのパディングが 0 でない非正規 base32（'i'=8 の下位 2 ビットは 00、'j'=9 は 01）。
        let mut noncanonical = VALID_CIDV1.to_string();
        assert_eq!(noncanonical.pop(), Some('i'));
        noncanonical.push('j');
        assert!(!is_valid_cid(&noncanonical));
    }
}
