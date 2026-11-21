//! 環境変数の分類器（非 UTF-8 値を「未設定」と区別する）。
//!
//! 判定は1つ、拒否/警告/安全側既定の使い分けは呼び出し側。
//! 本モジュールは生バイトを一切保持・露出させず、
//! 「未設定 / 非 UTF-8 / 有効な UTF-8」と
//! 「未設定 / 有効な真偽値 / 不正」を返すだけにとどまる。
//! 結果を起動拒否にするか、警告にするか、安全側既定へ倒すかは呼び出し側の責務である。

use crate::age::parse_bool_env;

/// 環境変数の読取結果。
///
/// `NonUtf8` はフラグのみで生バイト（`OsString`）を保持しないため、
/// デバッグ出力やログに非 UTF-8 値が漏れることはない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    /// 値が設定されており、UTF-8 として読めた。
    Utf8(String),
    /// 環境変数が未設定。
    Missing,
    /// 値は設定されているが UTF-8 として読めない（値は保持しない）。
    NonUtf8,
}

/// `std::env::var_os` で未設定 / 非 UTF-8 / UTF-8 を区別して読み取る。
///
/// `OsString` は `into_string()` の失敗時に破棄し、生バイトを戻り値へ含めない。
pub fn read_env(name: &str) -> EnvValue {
    match std::env::var_os(name) {
        None => EnvValue::Missing,
        Some(os) => match os.into_string() {
            Ok(value) => EnvValue::Utf8(value),
            // 生バイトは保持せず、フラグだけを返す。
            Err(_) => EnvValue::NonUtf8,
        },
    }
}

/// `std::env::var` の結果を注入して分類する純粋版。
///
/// `VarError` の `Debug` は `NotUnicode` が生バイトを含むため一切フォーマットせず、
/// バリアントのみを照合する。
pub fn classify(result: Result<String, std::env::VarError>) -> EnvValue {
    match result {
        Ok(value) => EnvValue::Utf8(value),
        Err(std::env::VarError::NotPresent) => EnvValue::Missing,
        Err(std::env::VarError::NotUnicode(_)) => EnvValue::NonUtf8,
    }
}

/// 真偽値環境変数の分類結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolEnvState {
    /// 環境変数が未設定。
    Unset,
    /// ドキュメントどおりの受理値（1/true/0/false）として解釈できた。
    Valid(bool),
    /// 非 UTF-8、または受理値以外の不正値。
    Invalid,
}

/// [`EnvValue`] を真偽値の状態へ分類する。
///
/// 判定は1つ、拒否/警告/安全側既定の使い分けは呼び出し側。
/// `Missing` → `Unset`、`NonUtf8` → `Invalid`、
/// `Utf8` → 既存の `parse_bool_env` 結果（`Some` は `Valid`、`None` は `Invalid`）。
/// 受理値の基準は既存 `parse_bool_env` に委譲し、ここでは変更しない。
pub fn bool_env_state(value: EnvValue) -> BoolEnvState {
    match value {
        EnvValue::Missing => BoolEnvState::Unset,
        EnvValue::NonUtf8 => BoolEnvState::Invalid,
        EnvValue::Utf8(raw) => match parse_bool_env(&raw) {
            Some(flag) => BoolEnvState::Valid(flag),
            None => BoolEnvState::Invalid,
        },
    }
}
