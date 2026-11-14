---
type: Handoff
title: "開発引き継ぎコンテキスト"
description: "別端末で作業を再開するための現状、検証手順、残タスク、注意事項。"
tags:
  - handoff
  - status
  - open-illustration
generated:
  by: opencode
  at: "2026-11-03T03:10:00+09:00"
---

## 1. リポジトリ状態

- ブランチ: `master`
- 最新コミット: `8fb989c`（この文書の作成時点）
  （docs: add development handoff context）
- リモート: `origin/master` と同期済み
- 作業ツリー: clean
- 最終 CI: success
  <https://github.com/onoderakeita/openIllustration/actions/runs/41579032881>
- テスト: 89 件成功（client 6 / common 5 / server 78）

PC では最初に以下を実行する。

```bash
git pull --ff-only
cargo test --workspace
```

## 2. 完了した作業

### P0 検証基盤

- IPFS ゲートウェイとタイムアウトを `AppState` に注入可能化。
- 外部ネットワーク依存のテストをローカル HTTP モックへ置換。
- AUTH-01、CONTENT-02/03、10 MiB 境界、全ゲートウェイ失敗を回帰テスト化。
- `cargo-deny` によるライセンス検査を CI に追加。

### JPKI 認証（ADR-0002 Accepted）

- `server/src/oidc.rs` に `/auth/oidc/start` と `/auth/oidc/callback` を実装。
- state、nonce、code_verifier をサーバー側に保存し、state を単回消費。
- JWKS の `kid` による ES256 署名検証。
- `iss`、`aud`、`exp`、`nbf`、`iat`、`nonce`、`at_hash` を検証。
- PPID を `user_hash` としてセッションJWTと HttpOnly Cookie に結合。
- `sid` の denylist によるログアウト失効と back-channel logout。
- ローカル OP モックで正常系、state 再利用、nonce 不一致、
  ログアウト、back-channel logout を検証。

### JPYC 決済（ADR-0003 Accepted）

- `server/migrations/002_payment_orders.sql` と
  `003_payment_order_index.sql` を追加。
- `server/src/payments.rs` に注文作成、EIP-191 ウォレット所有証明、
  `PaymentVerifier` 境界、状態機械を実装。
- `(chain_id, tx_hash, log_index)` の一意制約で二重使用を拒否。
- `PAYMENT_RECEIVING_XPRV` による BIP44 受取アドレス派生
  （`m/44'/60'/0'/0/{index}`）と `derivation_index` の一意制約。
- モック verifier で注文ライフサイクル、未対応チェーン、
  誤ウォレット署名、アドレス派生を検証。

### 設計

- ADR-0002（JPKI）と ADR-0003（JPYC）を Accepted に昇格。
- ADR-0004（モデレーション手続）を Proposed として追加。
- `docs/DOCUMENT_STATUS.md` の DEC-01/03/05 を実装状況に合わせて更新。

## 3. 検証コマンド

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings -A clippy::module-inception
cargo test --workspace
cargo deny check licenses
cargo build -p client --target wasm32-unknown-unknown --locked
```

E2E は Google Chrome、trunk 0.21.14、Node 18 以上が必要。
クライアントの待受ポートは既定 8080。8080 が他プロセス
（例: code-server）に使われている場合は `E2E_CLIENT_PORT` で変更する。
2026-11-15 に trunk 0.22.1・`E2E_CLIENT_PORT=18080` で 9 件成功を確認。

```bash
cd e2e
npm ci
npm run typecheck
E2E_CLIENT_PORT=18080 npm test
```

## 4. PC でのセットアップ

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --version 0.21.14 --locked
cargo install cargo-deny --version 0.20.2 --locked
```

サーバー起動の最小設定:

```bash
export JWT_SECRET="$(openssl rand -hex 32)"
export ALLOW_IPV4_LOCALHOST=1
export ALLOW_MOCK_AUTH=1
cargo run -p server
```

`ALLOW_MOCK_AUTH=1` は開発専用。本番では設定しない。

## 5. 環境変数

### 認証

- `JWT_SECRET`: 必須。32バイト以上。
- `ALLOW_MOCK_AUTH`: 開発専用。
- `OIDC_ISSUER`
- `OIDC_CLIENT_ID`
- `OIDC_CLIENT_SECRET`
- `OIDC_REDIRECT_URI`
- `OIDC_AUTHORIZATION_ENDPOINT`
- `OIDC_TOKEN_ENDPOINT`
- `OIDC_JWKS_URI`
- `OIDC_SCOPES`: 既定 `openid`。
- `OIDC_COOKIE_SECURE`: 既定有効。
- `OIDC_CLOCK_SKEW_SECS`: 既定 60。

OIDC の必須項目が揃わない場合、OIDC エンドポイントは 501 を返す。

### IPFS

- `IPFS_GATEWAYS`: `{cid}` を含む http(s) URL テンプレート。
- `IPFS_TIMEOUT_SECS`: 1〜300。既定 15。
- `PINATA_JWT`: 未設定時は模擬動作。

### 決済

- `PAYMENT_RECEIVING_ADDRESS`: `0x` + 40 hex。
- `PAYMENT_RECEIVING_XPRV`: BIP44 派生用。設定時はアドレスより優先。
  秘密管理基盤から注入し、ログへ出さない。

### ネットワーク

- `ALLOWED_ORIGINS`
- `TRUSTED_PROXY_CIDRS`
- `ALLOW_IPV4_LOCALHOST`

### 開発

- `EXPOSE_INTERNAL_ERRORS`
- `RUST_LOG`

## 6. 主要ファイル

- `server/src/oidc.rs`: OIDC 認証、IDトークン検証、logout。
- `server/src/payments.rs`: JPYC 注文、署名検証、状態機械、BIP44 派生。
- `server/src/lib.rs`: ルーター、`AppState`、既存 API。
- `server/src/tests.rs`: 統合テストとモック OP / verifier。
- `server/migrations/002_payment_orders.sql`: 決済注文。
- `server/migrations/003_payment_order_index.sql`: 派生 index。
- `docs/adr/ADR-0002-jpki-authentication.md`: JPKI 方式。
- `docs/adr/ADR-0003-jpyc-settlement.md`: JPYC 方式。
- `docs/adr/ADR-0004-moderation-procedure.md`: モデレーション提案。
- `docs/DOCUMENT_STATUS.md`: 本番公開ゲートと状態。
- `docs/REVIEW_REPORT.md`: レビューと実装記録。

## 7. 残タスク

### ユーザーの手続き・判断が必要

- デジタル認証サービスのクライアントID・シークレット・
  リダイレクトURI登録、民間事業者向け申込と認定。
- JPYC の実RPCプロバイダ契約、HSM/マルチシグの鍵管理、
  返金・紛争対応、AML/KYC、資金移動業・税務の法的確認。
- ADR-0004（モデレーション手続）の承認と運営主体の決定。
- DEC-02（年齢・対象地域）の法務判断。
- 本番ホスティング、TLS、本番CORS、バックアップ、監視。

### 実装可能な残作業

- JPKI: `acr`/`amr` の保証レベル検証、`auth_time`/`max_age`、
  リフレッシュトークンのローテーションと盗用検知、
  `sub` 単位の全セッション失効。
- JPYC: 実RPC検証、2-of-3クォーラム、バックグラウンド確認ワーカー、
  コントラクトの `pause` 監視、再編成の再評価。
- モデレーション: ADR-0004 承認後の投票期間・定足数・同数処理。
- DB: ロールバック方針、並行起動の排他、バックアップ手順。
- 依存: `libsql`、`jsonwebtoken`、`imageproc`、`rsa` 経由の
  RUSTSEC。到達可能性と更新案は
  [脆弱性トリアージ](SECURITY_ADVISORIES.md) を参照する。
- E2E: ブラウザ別の URL 解釈、モデレーションの期限・異議。

## 8. 注意事項

- `ALLOW_MOCK_AUTH=1` は開発専用。本番では有効化しない。
- `PAYMENT_RECEIVING_XPRV`、`OIDC_CLIENT_SECRET`、`JWT_SECRET` は
  秘密情報。commit しない。
- JPYC のコントラクトアドレスと確認数は、実装前に JPYC の一次ソース
  （公式リポジトリ、公式サイト、Explorer）で再確認する。
- 旧 JPYC Prepaid の `0x431D...` は現行コントラクトではない。
- `cargo deny check advisories` は既存の RUSTSEC で失敗する。
  `cargo deny check licenses` のみ CI に導入済み。詳細は
  [脆弱性トリアージ](SECURITY_ADVISORIES.md)。
- Termux 固有の `rustls-platform-verifier` 失敗は、テストの
  ローカルモック化により解消済み。
