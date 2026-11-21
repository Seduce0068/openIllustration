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
- クライアント側メトリクスはビルド時に `OIL_CLIENT_METRICS=0` で無効化できます（未設定時は有効）。
  `POST /metrics/client-events` に送るのは `event` と `count` のみで、URL・IP アドレス・User-Agent は送りません。
- `ALLOW_MOCK_AUTH=1` は開発専用です。指定マイナアプリ/JPKI認証の代替ではなく、本番では有効化しません。
- DB内ハッシュ連鎖は第三者検証可能な監査の要件を満たしません。改ざん不能性・否認防止・法的証拠性を保証しません。
- 模擬支援APIは501で無効化済みです。JPYC送金検証の実装まで支援・有償サービスを提供しません。
- 高速・軽量・低手数料は設計目標です。数値目標と達成状況は未決定です。
  ADR は [ADR-0011](docs/adr/ADR-0011-performance-measurement.md) で合意待ち（未作成）です。
- クロスプラットフォーム性は設計目標です。対応OS・ブラウザ・端末、最小バージョンは未決定で、JXL/JPKI/ウォレット対応を含む実機検証が必要です。
  ADR は [ADR-0012](docs/adr/ADR-0012-compatibility-matrix.md) で合意待ち（未作成）です。

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
cd client && npm ci && npm run build-css && cd ..   # Tailwind CSS をビルド時に生成
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

# E2E（初回は cd e2e && npm ci。Google Chrome・trunk・Node 22 以上が必要。
# 開発サーバーは Playwright が自動起動する）
cd e2e && npm test
```

### 5. 運用CLI（バックアップ・監査）

サーバー起動前に `doctor --strict` で設定を診断し、バックアップ取得・検証・リストア・
監査エクスポートの各CLIを Runbook の手順で実行します。トークン値や署名鍵の内容は
argv に渡さず、`DATABASE_TOKEN` 環境変数または権限を制限したファイルで渡します。
DB 接続3 CLI（`backup_db` / `sign_audit_export` / `verify_audit_log`）は
`DATABASE_URL` の暗黙既定値（`file:local.db`）を使わず、`--database-url` または
`DATABASE_URL` の明示設定が必須です（未設定・空・空白のみなら起動拒否、値はエコーしません）。

```bash
# Kubuntu / リポジトリルート
cargo run -p server --bin doctor -- --strict
cargo run -p server --bin backup_db -- --database-url file:/path/to/local.db --output /path/to/backup.db
cargo run -p server --bin verify_backup -- --input /path/to/backup.db
cargo run -p server --bin restore_db -- --backup /path/to/backup.db --target /path/to/local.db --force
cargo run -p server --bin sign_audit_export -- --database-url file:/path/to/local.db \
  --signing-key /secure/path/audit-signing-key.pem --output /path/to/audit-snapshot.json
cargo run -p server --bin verify_audit_export -- --snapshot /path/to/audit-snapshot.json \
  --public-key /path/to/audit-public.der
cargo run -p server --bin verify_audit_log -- --database-url file:/path/to/local.db
```

各コマンドの前提・失敗時の扱い・復旧手順は
[バックアップ・リストア Runbook](docs/BACKUP_RESTORE_RUNBOOK.md) と
[監査エクスポート Runbook](docs/AUDIT_EXPORT_RUNBOOK.md) を参照してください。
7 つの運用 CLI のうち `doctor` は設定診断専用の別契約で DB トークンを扱わない。
残り6つ（`backup_db` / `restore_db` / `sign_audit_export` / `verify_audit_log` /
`verify_audit_export` / `verify_backup`）はすべて `--database-token` を拒否する
（argv は ps 等で公開されるため）。トークンが必要な DB 接続3 CLI
（`backup_db` / `sign_audit_export` / `verify_audit_log`）は `DATABASE_TOKEN`
環境変数のみを使用し、`restore_db`（ローカル DB のみ）と
`verify_backup` / `verify_audit_export`（DB 非接続）はトークン自体を使わない。

## 年齢/地域ポリシー（DEC-02 方針、公開設定）

- 有効な年齢/地域ポリシー設定値（非秘密）は認証不要 `GET /policy/age` で常時公開される
  （`settings_always_public=true`）。
- 既定は保守的一律ベースライン `AGE_DEFAULT_MINIMUM=21`（法的事実/適合の確定主張ではない）。
  `AGE_COUNTRY_OVERRIDES`・`AGE_TARGET_REGIONS`・`AGE_ENFORCEMENT_MODE`・
  `AGE_REQUIRE_EXPLICIT_CONFIGURATION` で運用者が構成可能。旧手動フラグ
  `AGE_POLICY_CONFIGURED` は読取を廃止し、`configuration_satisfied()` が
  `AGE_DEFAULT_MINIMUM` / `AGE_COUNTRY_OVERRIDES` / `AGE_TARGET_REGIONS` の
  実効値から充足を導出する。
- `trusted_attribute_required` は検証済み属性入力経路が未実装のため現時点でフェイルクローズ。
  現行入力は自己申告であり、本人確認済み属性ではない。詳細と制約は
  [docs/AGE_VALIDATION_POLICY.md](docs/AGE_VALIDATION_POLICY.md) を参照。
