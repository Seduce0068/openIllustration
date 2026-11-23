//! `client` の `option_env!` が読むビルド時環境変数を監視する。
//!
//! 生成物は作らず、Cargo への `rerun-if-env-changed` 宣言だけを行う。
//! これにより `OIL_API_BASE` / `OIL_MOCK_AUTH` / `OIL_CLIENT_METRICS` を
//! 変更したときだけクレートが再ビルドされ、埋め込み値が更新される。

fn main() {
    for name in ["OIL_API_BASE", "OIL_MOCK_AUTH", "OIL_CLIENT_METRICS"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
}
