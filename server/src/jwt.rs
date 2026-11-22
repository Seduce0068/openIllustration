//! JWT の署名・検証鍵。
//!
//! 既定は HS256（`JWT_SECRET`）で、`JWT_RSA_PRIVATE_KEY_PEM` が設定された
//! 場合だけ RS256 を使う。許可アルゴリズムは起動時に確定し、トークンの
//! ヘッダから選ばせない（アルゴリズム混同攻撃への対策）。

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePublicKey, LineEnding};
use rsa::traits::PublicKeyParts;
use std::collections::HashMap;

/// RS256 で要求する最小鍵長（ビット）。NIST SP 800-57 の 112 bit
/// セキュリティ強度は 2048 bit 以上で満たす。
const RSA_MIN_KEY_BITS: usize = 2048;

/// 署名鍵一式。起動時に一度だけ構築し、リクエストごとに再利用する。
/// RS256 では `kid` 付きヘッダを発行し、複数公開鍵で検証できるようにする。
pub struct JwtKeys {
    algorithm: Algorithm,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    kid: Option<String>,
    verification_keys: HashMap<String, DecodingKey>,
    /// true のときは `kid` 付きトークンのみ受理する（レガシー互換を無効化）。
    require_kid: bool,
}

impl JwtKeys {
    /// `JWT_SECRET` から HS256 の鍵を作る。
    pub fn from_hmac_secret(secret: &str) -> Result<Self, String> {
        if secret.len() < 32 {
            return Err(format!(
                "JWT_SECRET must be at least 32 bytes for HS256 (got {} bytes)",
                secret.len()
            ));
        }
        // 空の HMAC 鍵は jsonwebtoken が拒否する。長さ検査の後に作る。
        let encoding_key = EncodingKey::from_secret(secret.as_bytes())
            .map_err(|e| format!("JWT_SECRET is not usable for HS256: {e}"))?;
        let decoding_key = DecodingKey::from_secret(secret.as_bytes())
            .map_err(|e| format!("JWT_SECRET is not usable for HS256: {e}"))?;
        Ok(Self {
            algorithm: Algorithm::HS256,
            encoding_key,
            decoding_key,
            kid: None,
            verification_keys: HashMap::new(),
            require_kid: false,
        })
    }

    /// PKCS#8 PEM の RSA 秘密鍵から RS256 の鍵を作る。
    ///
    /// 検証用の公開鍵は秘密鍵から導出する。公開鍵を別途設定させると、
    /// 取り違えによる検証失敗や鍵の不一致を招くため。
    pub fn from_rsa_private_key_pem(pem: &str) -> Result<Self, String> {
        let private_key = rsa::PrivateKey::from_pkcs8_pem(pem).map_err(|e| {
            format!("JWT_RSA_PRIVATE_KEY_PEM is not a valid PKCS#8 private key: {e}")
        })?;

        let bits = private_key.n().bit_width() as usize;
        if bits < RSA_MIN_KEY_BITS {
            return Err(format!(
                "JWT_RSA_PRIVATE_KEY_PEM must be at least {RSA_MIN_KEY_BITS} bits (got {bits} bits)"
            ));
        }

        let public_pem = private_key
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .map_err(|e| format!("failed to encode the RSA public key: {e}"))?;

        let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes())
            .map_err(|e| format!("JWT_RSA_PRIVATE_KEY_PEM is not usable for RS256: {e}"))?;
        let decoding_key = DecodingKey::from_rsa_pem(public_pem.as_bytes())
            .map_err(|e| format!("failed to load the derived RSA public key: {e}"))?;
        let kid = Self::kid_from_public_pem(&public_pem)?;
        let mut verification_keys = HashMap::new();
        verification_keys.insert(kid.clone(), decoding_key.clone());

        Ok(Self {
            algorithm: Algorithm::RS256,
            encoding_key,
            decoding_key,
            kid: Some(kid),
            verification_keys,
            require_kid: false,
        })
    }

    /// 公開鍵 PEM から短い kid を導出する（SPKI DER の SHA-256 先頭16バイト）。
    /// 検証鍵登録時に引数 kid との整合検査へ使うためテストからも参照する。
    pub fn kid_from_public_pem(public_pem: &str) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let key = rsa::PublicKey::from_public_key_pem(public_pem)
            .map_err(|e| format!("invalid RSA public key PEM: {e}"))?;
        let der = key
            .to_public_key_der()
            .map_err(|e| format!("public key DER encoding failed: {e}"))?;
        let digest = Sha256::digest(der.as_ref());
        Ok(hex::encode(&digest[..16]))
    }

    /// 鍵ローテーション用に、旧公開鍵を kid 付きで検証鍵へ追加する。
    pub fn add_verification_key(&mut self, kid: &str, public_pem: &str) -> Result<(), String> {
        if kid.is_empty() || kid.len() > 128 {
            return Err("kid must be 1..=128 characters".to_string());
        }
        // 署名鍵と同じ kid は上書きを許さない（誤設定の防止）。
        if self.verification_keys.contains_key(kid) {
            return Err(format!("duplicate verification key kid: {kid}"));
        }
        // 引数 kid と公開鍵から導出した kid が違えば、ローテーション設定の取り違え。
        // 登録を拒否し、未知 kid による検証失敗や意図しない鍵の混入を防ぐ。
        let derived_kid = Self::kid_from_public_pem(public_pem)?;
        if derived_kid != kid {
            return Err(format!(
                "verification key kid mismatch: expected {derived_kid}, got {kid}"
            ));
        }
        // 検証用の旧公開鍵にも署名鍵と同じ最小鍵長を要求する。
        // ローテーション経由で 1024 bit 以下の弱鍵が混入するのを防ぐ。
        let public_key = rsa::PublicKey::from_public_key_pem(public_pem)
            .map_err(|e| format!("invalid RSA public key PEM: {e}"))?;
        let bits = public_key.n().bit_width() as usize;
        if bits < RSA_MIN_KEY_BITS {
            return Err(format!(
                "verification key must be at least {RSA_MIN_KEY_BITS} bits (got {bits} bits)"
            ));
        }
        let key = DecodingKey::from_rsa_pem(public_pem.as_bytes())
            .map_err(|e| format!("invalid RSA verification key: {e}"))?;
        self.verification_keys.insert(kid.to_string(), key);
        Ok(())
    }

    /// トークンを検証する。`kid` がある場合は対応する公開鍵のみを使う。
    /// 未知の `kid` はフェイルクローズで拒否する。
    pub fn decode<T: serde::de::DeserializeOwned>(
        &self,
        token: &str,
    ) -> Result<jsonwebtoken::TokenData<T>, String> {
        let header =
            jsonwebtoken::decode_header(token).map_err(|e| format!("invalid JWT header: {e}"))?;
        if header.alg != self.algorithm {
            return Err(format!("unexpected JWT algorithm: {:?}", header.alg));
        }
        if self.require_kid && header.kid.is_none() {
            return Err("JWT kid is required".to_string());
        }
        let key = match (self.algorithm, header.kid.as_deref()) {
            (Algorithm::RS256, Some(kid)) => self
                .verification_keys
                .get(kid)
                .ok_or_else(|| format!("unknown JWT kid: {kid}"))?,
            // kid なしは旧トークン（ローテーション前に発行）として署名鍵で検証する。
            _ => &self.decoding_key,
        };
        jsonwebtoken::decode::<T>(token, key, &self.validation())
            .map_err(|e| format!("invalid token: {e}"))
    }

    /// `kid` 無しトークン（レガシー）を拒否するかを設定する。
    pub fn set_require_kid(&mut self, required: bool) {
        self.require_kid = required;
    }

    /// 署名に使う JWT ヘッダ。許可アルゴリズムと kid はここで固定される。
    pub fn header(&self) -> Header {
        let mut header = Header::new(self.algorithm);
        header.kid = self.kid.clone();
        header
    }

    /// 検証設定。許可アルゴリズムを起動時の値に限定する。
    pub fn validation(&self) -> Validation {
        Validation::new(self.algorithm)
    }

    pub fn encoding_key(&self) -> &EncodingKey {
        &self.encoding_key
    }

    pub fn decoding_key(&self) -> &DecodingKey {
        &self.decoding_key
    }

    /// 設定済みのアルゴリズム名（ログ用）。
    pub fn algorithm_name(&self) -> &'static str {
        match self.algorithm {
            Algorithm::HS256 => "HS256",
            Algorithm::RS256 => "RS256",
            _ => "other",
        }
    }
}
