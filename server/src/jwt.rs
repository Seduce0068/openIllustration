//! JWT の署名・検証鍵。
//!
//! 既定は HS256（`JWT_SECRET`）で、`JWT_RSA_PRIVATE_KEY_PEM` が設定された
//! 場合だけ RS256 を使う。許可アルゴリズムは起動時に確定し、トークンの
//! ヘッダから選ばせない（アルゴリズム混同攻撃への対策）。

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rsa::pkcs8::{DecodePrivateKey, EncodePublicKey, LineEnding};
use rsa::traits::PublicKeyParts;

/// RS256 で要求する最小鍵長（ビット）。NIST SP 800-57 の 112 bit
/// セキュリティ強度は 2048 bit 以上で満たす。
const RSA_MIN_KEY_BITS: usize = 2048;

/// 署名鍵一式。起動時に一度だけ構築し、リクエストごとに再利用する。
pub struct JwtKeys {
    algorithm: Algorithm,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
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

        Ok(Self {
            algorithm: Algorithm::RS256,
            encoding_key,
            decoding_key,
        })
    }

    /// 署名に使う JWT ヘッダ。許可アルゴリズムはここで固定される。
    pub fn header(&self) -> Header {
        Header::new(self.algorithm)
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
