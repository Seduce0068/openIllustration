---
type: Project Overview
description: プロジェクトの目的、現行状況、開発手順を示す概要。
tags: [open-illustration, overview, rust]
---

# OpenIllustration / Digital Guild

OpenIllustrationは、OSSによる反検閲性、高速・軽量・クロスプラットフォームな閲覧、ほぼゼロに近い利用者手数料を目指すイラスト・小説サイトです。指定マイナアプリ/JPKI認証を利用資格の必須条件とし、認証できない利用者は閲覧を含め利用できません。有償取引・支援にはJPYCのみを用います（閲覧のみの利用にJPYC保有を求めるものではありません）。バックエンドはRust/Axumを基準にRustを優先採用します。

現在のコードはPinata/IPFS、libSQL、Leptos、開発用モック認証を用いたプロトタイプです。実JPKI認証、JPYC送金・検証、第三者検証可能な監査、分散運用は未完成であり、必須要件を満たす本番サービスではありません。

プロジェクトは GNU AGPL v3 or any later version
（SPDX: `AGPL-3.0-or-later`）でライセンスされています。詳細は [LICENSE](LICENSE) を参照してください。

## 現行プロトタイプと目標要件

- 現行プロトタイプはJPEG XL静止画・Markdown小説、CID/SHA-256照合、JWT、DB内通報・投票を備えます。
- ログは標準出力へ出し、`RUST_LOG` で制御します（例: `RUST_LOG=server=debug`。未設定時は INFO）。
- `ALLOW_MOCK_AUTH=1` は開発専用です。指定マイナアプリ/JPKI認証の代替ではなく、本番では有効化しません。
- DB内ハッシュ連鎖は第三者検証可能な監査の要件を満たしません。改ざん不能性・否認防止・法的証拠性を保証しません。
- 模擬支援APIは501で無効化済みです。JPYC送金検証の実装まで支援・有償サービスを提供しません。
- 高速・軽量・低手数料は設計目標です。数値目標と達成状況は未決定です。
- クロスプラットフォーム性は設計目標です。対応OS・ブラウザ・端末、最小バージョンは未決定で、JXL/JPKI/ウォレット対応を含む実機検証が必要です。

## 🛠 技術スタック

- **Backend**: Rust (Axum, libSQL/Turso, x509-parser, jsonwebtoken)
- **Frontend**: Rust (Leptos CSR, Tailwind CSS)
- **Shared**: Rust (common crate)
- **Infrastructure**: IPFS (Pinata integration)
- **Tools**: Trunk (Wasm bundler)

## 🚀 クイックスタート

### 1. 依存ツールのインストール

```bash
cargo install trunk --version 0.21.14 --locked   # CI と同じ版を推奨（E2E の前提）
rustup target add wasm32-unknown-unknown
```

### 2. バックエンドの起動

```bash
# JWT_SECRET に32バイト以上のランダム値を設定して起動（未設定・短すぎる場合は起動しない）
export JWT_SECRET="$(openssl rand -hex 32)"
# 任意: RS256 へ切り替える場合は PKCS#8 PEM の RSA 秘密鍵（2048bit以上）を設定する。
# 設定時は JWT_SECRET より優先され、署名アルゴリズムは起動時に確定する。
# export JWT_RSA_PRIVATE_KEY_PEM="$(cat jwt-rsa-private.pem)"
export ALLOW_IPV4_LOCALHOST=1   # ローカル開発のみ
export ALLOW_MOCK_AUTH=1          # 開発専用: mock JPKI / ゼロ署名。本番では未設定または 0
# 任意: IPFS ゲートウェイ（`{cid}` を含む URL テンプレート）とタイムアウト
export IPFS_GATEWAYS="https://ipfs.io/ipfs/{cid},https://gateway.pinata.cloud/ipfs/{cid}"
export IPFS_TIMEOUT_SECS=15
cargo run -p server
```

### 3. フロントエンドの起動

```bash
cd client
trunk serve   # http://127.0.0.1:8080
```

この構成は開発用です。`ALLOW_MOCK_AUTH=1` は本番環境で設定しないでください。
実JPKI認証、JPYC決済、第三者検証可能な監査、本番デプロイは未整備です。
設計方針と実装差分は [ADR-0001](docs/adr/ADR-0001-mvp-trust-boundaries.md)
と各設計書を参照してください。

### 4. テストの実行

```bash
# ワークスペース全体のユニット / 統合テスト
cargo test

# サーバーのみ
cargo test -p server

# 共通クレート（年齢検証・チェックサム・監査ログ）
cargo test -p common

# E2E（初回は cd e2e && npm ci。Google Chrome・trunk・Node 18 以上が必要。
# 開発サーバーは Playwright が自動起動する）
cd e2e && npm test
```
