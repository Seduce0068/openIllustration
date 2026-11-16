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
  at: "2026-11-15T16:52:50+09:00"
---

## 1. リポジトリ状態

- ブランチ: `master`
- 参照コミット: 作業開始時点は `745810e`。以降の変更は `git log` を参照。
- 作業ツリー: clean（未コミット変更なし）。`client/dist/` 等の生成物はコミット対象外として扱う。
- 参考: 過去の記録として `8fb989c` 時点の CI success（run 41579032881）あり。以降のローカルコミットは未CI（push済みだがCI結果は別途確認）。
- 直近のローカル検証（2026-11-15、touch範囲）: `cargo fmt --all --check`、`cargo clippy -p common -p server --all-targets -- -D warnings -A clippy::module_inception`、`cargo test -p common -p server --quiet`（`common` 11件、`server` 93件成功）。

PC/別端末で再開する場合は以下を実行する。

```bash
git pull --ff-only
cargo test --workspace
```

## 2. 完了した作業

### DEC-02 年齢/地域ポリシー方針の採用と実装（本セッション）

- `common/src/age.rs`: `AgePolicy`/`AgeEnforcementMode`/`AgeValidationError` と `from_env()`・`public_settings()`・`is_age_allowed_with_policy()` を実装。既定は保守的一律ベースライン `21`（法的事実の確定主張ではない）、国別上書き・対象地域・明示設定必須フェイルクローズ・信頼属性必須モード（現時点フェイルクローズ）をサポート。
- `server/src/lib.rs`: 起動時に年齢ポリシーを読込/検証し `AppState.age_policy` に保持。認証不要の `GET /policy/age` で有効設定値（非秘密、`settings_always_public=true`）を常時公開。投稿時年齢検証をポリシーベースへ更新。
- `server/src/tests.rs`, `common/src/age.rs`: ポリシー公開・フェイルクローズ・既定/上書き等のテストを追加/更新。
- DEC-02関連ドキュメントを採用方針・公開設定要件・制約に合わせて更新（法的確定/専門家レビューは未完）。
- ローカル検証（touch範囲）: `cargo fmt --all --check`、`cargo clippy -p server -p common --all-targets -- -D warnings -A clippy::module_inception`、`cargo test -p common --quiet`（11件）、`cargo test -p server --quiet`（86件）成功。

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

### 依存更新（2026-11-15）

- `jsonwebtoken` 12.0.0、`rsa` 0.10.0、`k256` 0.14.0、`p256` 0.14.0 へ更新。
- Marvin 攻撃と crypto-bigint の timing 勧告を解消（RUSTSEC 13 → 11 件）。

### 設計

- ADR-0002（JPKI）と ADR-0003（JPYC）を Accepted に昇格。
- ADR-0004（モデレーション手続）を Proposed として追加。
- `docs/DOCUMENT_STATUS.md` の DEC-01/03/05 を実装状況に合わせて更新。DEC-02 は方針採用・実装進行中（法的確定/専門家レビュー未完）として更新。

### 入力バリデーションの防御的強化（2026-11-15）

- `ad9d2a8`: `server/src/payments.rs` の `load_order` で `order_id` の UUID 形式を検証し、不正形式は `400 Bad Request ("Invalid order id")` を返す（`GET/POST /payments/orders/{order_id}` 系）。回帰テスト `test_payment_order_rejects_invalid_order_id_format` を追加。
- `00a9176`: `POST /mod/report` で `target_cid` に `is_safe_cid` 検証を追加し、不正 CID を `400 Bad Request ("Invalid CID")` で早期拒否。回帰テスト `test_report_rejects_invalid_cid` を追加（既存の有効 CID・不存在ケース `404` は維持）。

## 3. 検証コマンド

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings -A clippy::module-inception
cargo test --workspace
cargo deny check licenses
cargo deny check advisories
cargo build -p client --target wasm32-unknown-unknown --locked
```

CI は 2026-11-16 時点で GitHub アカウントの請求/上限によりジョブが開始されない
（annotation: `recent account payments have failed or your spending limit needs to be increased`）。
解除まで上記コマンドをローカルで順に実行して代替ゲートとする。2026-11-17 の
ローカル実行では fmt / clippy / test(workspace: 188件) / deny(licenses, advisories) /
wasm build / E2E(9件) がすべて成功した。

配布物のライセンス同梱は `bash scripts/prepare-distribution.sh <出力先>` で生成（LICENSE / NOTICE / DEPENDENCIES.tsv / THIRD_PARTY_LICENSES/）。

`server` クレートは `verify_audit_log` bin も auto-discover されるため、
`cargo run` が曖昧にならないよう Cargo.toml に `default-run = "server"` を設定済み。
E2E の起動失敗はこれで解消している。

監査ログの連鎖・連番・Merkle root は次で検証できる。root を別経路で保存し、
`--expect-root` に渡すと欠落・末尾切り詰めを検出できる（実DBで確認済み）。

```bash
cargo run -p server --bin verify_audit_log -- \
  --database-url file:local.db --expect-root <保存した Root>
```

CI と同一の E2E ゲートは `bash e2e/ci-e2e.sh` でローカル再現できる（2026-11-17 に9件成功）。固定 trunk 0.21.14 は `NO_COLOR=1` の環境だと `--no-color` 解釈で失敗するため、スクリプト内で `unset NO_COLOR` している。8080 が他プロセスに占拠されても動くよう `E2E_CLIENT_PORT` の既定値は 18080。

E2E は Google Chrome（major 120 以上）、trunk 0.21.14、Node 18 以上が必要。`ci-e2e.sh` が `MIN_NODE_MAJOR` / `MIN_CHROME_MAJOR` で下限を検査する（環境変数で上書き可）。失敗時は `e2e/test-results` と `e2e/playwright-report` を `E2E_ARTIFACT_DIR`（既定 `e2e-artifacts/`）へタイムスタンプ付き tar.gz で保存する。
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

- `JWT_SECRET`: 必須。32バイト以上。`JWT_RSA_PRIVATE_KEY_PEM` 設定時は使われない（短い値が残っている場合は警告）。
- `JWT_RSA_PRIVATE_KEY_PEM`: 任意。PKCS#8 PEM の RSA 秘密鍵（2048bit以上）。不正な PEM は `JWT_SECRET` が有効でも起動を拒否する。RS256 の署名ヘッダには公開鍵 DER の SHA-256 先頭16バイトを `kid` として付与する。
- `JWT_RSA_VERIFICATION_KEYS`: 任意。RS256 のみ有効。`{"kid":"SPKI 公開鍵 PEM", ...}` 形式の JSON（`BEGIN PUBLIC KEY`）。鍵ローテーション時に旧公開鍵を検証用として登録する。未知の `kid` は拒否。署名鍵と同じ `kid` は登録不可。`kid` 無しの旧トークンは署名鍵で検証する。手順は [KEY_ROTATION_RUNBOOK.md](KEY_ROTATION_RUNBOOK.md)。
  設定時は RS256 で署名する。未設定なら HS256。
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
- `IPFS_MAX_ATTEMPTS`: ネットワークエラー時の再試行回数。既定 1、有効範囲 1〜5。
- `PINATA_JWT`: 未設定時は模擬動作。

### 年齢/地域ポリシー（DEC-02、非秘密・常時公開）

- `AGE_DEFAULT_MINIMUM`: 既定 `21`（保守的安全余裕ベースライン）。
- `AGE_COUNTRY_OVERRIDES`: `CC=NN,...`（例: `JP=18,US=21`）。
- `AGE_TARGET_REGIONS`: 対象地域コード（カンマ区切り）。
- `AGE_ENFORCEMENT_MODE`: `self_reported`（既定）または `trusted_attribute_required`（現時点フェイルクローズ）。
- `AGE_REQUIRE_EXPLICIT_CONFIGURATION`, `AGE_POLICY_CONFIGURED`: 明示設定フェイルクローズ用。
- 有効設定値は認証不要 `GET /policy/age` で常時公開。秘密情報を含めない。

### 決済

- `PAYMENT_RECEIVING_ADDRESS`: `0x` + 40 hex。
- `PAYMENT_RECEIVING_XPRV`: BIP44 派生用。設定時はアドレスより優先。
  秘密管理基盤から注入し、ログへ出さない。

### モデレーション運用者（ADR-0004）

- `MODERATION_OPERATOR_HASHES`: カンマ区切りの運用者 user_hash（JPKI 由来）。
  未設定・空要素は無視され、該当しない主体の BAN/UNBAN/UNPIN は `403` で拒否される。
  本番では実 JPKI 由来の hash のみを設定し、`ALLOW_MOCK_AUTH=1` と併用しない。
  セルフホストごとに異なる運用者を置けるため、プロジェクト側の固定値はない。
- `MODERATION_FINALIZE_INTERVAL_SECS`: 自動確定ワーカーの走査周期（秒）。既定 `300`、
  有効範囲 30〜3600。範囲外・不正値は既定へフォールバック。停止シグナルでワーカーも終了する。
- `GRACEFUL_DRAIN_TIMEOUT_SECS`: 停止時の drain 上限（秒）。既定 `30`、有効範囲 1〜300。
  超過時は警告ログを出して残接続を切断する。
- `OIDC_MAX_AUTH_AGE_SECS`: 認証からの最大経過時間。設定時は認証要求へ `max_age` を付与し ID トークンの `auth_time` を検証。
- `LOG_FORMAT`: `json` 設定時は 1行1 JSON でログ出力（ログ集約向け）。未設定は人間向けテキスト。
- `GET /metrics`: Prometheus 形式の軽量カウンタ（リクエスト数・4xx/5xx 数）。再起動でゼロに戻る。

### ネットワーク

- `ALLOWED_ORIGINS`: 本番では必須。`ALLOW_MOCK_AUTH=1` の開発時のみ未設定で localhost 系 8080 を許可し、それ以外で未設定なら全オリジン拒否（fail-closed）。
- `TRUSTED_PROXY_CIDRS`
- `ALLOW_IPV4_LOCALHOST`

### 開発

- `EXPOSE_INTERNAL_ERRORS`
- `RUST_LOG`

## 6. 主要ファイル

- `server/src/oidc.rs`: OIDC 認証、IDトークン検証、logout。
- `server/src/payments.rs`: JPYC 注文、署名検証、状態機械、BIP44 派生。
- `server/src/jwt.rs`: JWT 署名・検証鍵の集約（既定 HS256、`JWT_RSA_PRIVATE_KEY_PEM` 設定時 RS256）。
- `server/src/lib.rs`: ルーター、`AppState`、既存 API、`GET /policy/age`、年齢ポリシー読込/検証。
- `common/src/age.rs`: 年齢/地域ポリシー（DEC-02方針）、公開設定、検証ロジック。
- `server/src/tests.rs`: 統合テストとモック OP / verifier、年齢ポリシー公開/フェイルクローズ。
- `server/migrations/002_payment_orders.sql`: 決済注文。
- `server/migrations/003_payment_order_index.sql`: 派生 index。
- `server/migrations/004_moderation_deadlines.sql`: 投票期限・確定時刻・異議の連鎖。
- `server/migrations/005_governance_audit.sql`: 運用者の BAN/UNBAN/UNPIN 監査。
- `server/migrations/006_refresh_tokens.sql`: リフレッシュトークン（ハッシュのみ保存）。
- `server/migrations/007_payment_order_cid.sql`: 注文とコンテンツ CID の紐づけ。
- `server/migrations/008_revoked_subs.sql`: sub 単位のセッション失効閾値の永続化。
- 起動時は `<DB path>.instance.lock` で単一ライターを強制（第二インスタンスは拒否、停止時に削除）。同じ DB を複数プロセスで起動しないこと。
- `server/src/bin/backup_db.rs`: DB スナップショット作成（`VACUUM INTO`、上書き拒否）。
- `server/src/bin/verify_backup.rs`: バックアップの構造検証。
- `server/src/bin/restore_db.rs`: リストア（整合性検証・WAL ガード・上書き拒否・安全コピー・原子的置換）。手順は [BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)。
- `scripts/backup-rotate.sh`: バックアップ取得・検証・ローテーション（cron 等からの定期実行用）。`BACKUP_GPG_RECIPIENT` 設定時は GPG 暗号化し平文を削除。`OFFSITE_COPY_CMD` + `OFFSITE_DEST` でオフサイト保存（コピーコマンドのパスと宛先を指定）。`ALERT_CMD` で失敗通知（メッセージ1引数の実行ファイル）。
- `deploy/openillustration-backup.{service,timer,cron}`: systemd / cron の設定例。
- `server/src/bin/sign_audit_export.rs` / `verify_audit_export.rs`: 署名付き監査エクスポートと公開鍵検証（ADR-0005、[AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md)）。
- `docs/adr/ADR-0002-jpki-authentication.md`: JPKI 方式。
- `docs/adr/ADR-0003-jpyc-settlement.md`: JPYC 方式。
- `docs/adr/ADR-0004-moderation-procedure.md`: モデレーション手続（Accepted）。
- `docs/AGE_VALIDATION_POLICY.md`: DEC-02方針・実装・制約。
- `docs/DOCUMENT_STATUS.md`: 本番公開ゲートと状態。
- `docs/REVIEW_REPORT.md`: レビューと実装記録。

## 7. 残タスク

### ユーザーの手続き・判断が必要

- デジタル認証サービスのクライアントID・シークレット・
  リダイレクトURI登録、民間事業者向け申込と認定。
- JPYC の実RPCプロバイダ契約、HSM/マルチシグの鍵管理、
  返金・紛争対応、AML/KYC、資金移動業・税務の法的確認。
- モデレーション: このインスタンスの運用者 user_hash を `MODERATION_OPERATOR_HASHES` に設定する。プロジェクト側で固定された運営主体は存在せず、セルフホストする個人・企業がそれぞれ設定する（未設定なら特権操作は全拒否）。
- DEC-02（年齢・対象地域）の法務/専門家レビューと最終確定（対象地域・必要属性・最終閾値・保持期間・運用責任）。検証済み年齢/居住地属性の提供方式（DEC-01）と接続条件の確定。
- 本番ホスティング、TLS、本番CORS、バックアップ、監視。

### 実装可能な残作業

- DEC-02: 検証済み年齢/居住地属性入力経路の設計・実装（DEC-01連携）。`trusted_attribute_required` 運用時の検証ロジック拡張。公開設定(`GET /policy/age`)のスキーマ変更時に秘密情報非混入を保つレビュー/テスト整備。
- JPKI: `acr`/`amr` 保証レベル検証と `auth_time`/`max_age` 検証は実装済み（`OIDC_REQUIRED_ACR` / `OIDC_REQUIRED_AMR` / `OIDC_MAX_AUTH_AGE_SECS` 設定時のみ、欠落/不一致は fail-closed）。残るは実OP運用整合。
  リフレッシュトークンは実装済み（30日・HttpOnly Cookie・回転・再利用時ファミリ失効）。残るは失効状態の永続化と実OP運用整合。
  `sub` 単位の全セッション失効は `iat` 閾値方式で、`revoked_subs` テーブルへ永続化し起動時にロード（マイグレーション 008）。再認証で新しい `iat` を発行し DB の閾値も削除。複数プロセス共有・実OP連携/実機検証は残タスク。
- JPYC: 実RPC検証、2-of-3クォーラム、バックグラウンド確認ワーカー、
  コントラクトの `pause` 監視、再編成の再評価。
- モデレーション: ADR-0004（Accepted, 2026-11-16）の投票期間（7日）・定足数未達復帰・30日以内の異議・投稿者除外・期限切れ投票拒否・同数 `Rejected` を実装済み。BAN/UNBAN/UNPIN は `MODERATION_OPERATOR_HASHES` で設定した運用者だけが実行でき、`governance_audit_log` に記録する（未設定時は全拒否）。期限到達は起動時のワーカーが自動検知する（`MODERATION_FINALIZE_INTERVAL_SECS`、既定300秒、停止シグナルで終了）。残るは投票者選出・Sybil耐性、第三者検証可能な監査（DEC-04）。
- DB: ロールバック方針、並行起動の排他、バックアップ手順。
- 依存: 残る RUSTSEC は `libsql 0.10` 由来の 10 件と `paste` 1 件。
  到達可能性と更新案は
  [脆弱性トリアージ](SECURITY_ADVISORIES.md) を参照する。
- E2E: ブラウザ別の URL 解釈、モデレーションの期限・異議。

## 8. 注意事項

- `ALLOW_MOCK_AUTH=1` は開発専用。本番では有効化しない。
- `PAYMENT_RECEIVING_XPRV`、`OIDC_CLIENT_SECRET`、`JWT_SECRET`、
  `JWT_RSA_PRIVATE_KEY_PEM`、`PINATA_JWT`、DBトークン等は
  秘密情報。commit・ログ・公開応答に含めない。年齢/地域ポリシーの有効設定値（`GET /policy/age`）は常時公開の非秘密情報として扱う。
- DEC-02の既定値 `21` は保守的安全余裕ベースラインであり、法的事実/適合の確定主張ではない。`trusted_attribute_required` は検証済み属性入力が未実装のため現時点でフェイルクローズ。
- JPYC のコントラクトアドレスと確認数は、実装前に JPYC の一次ソース
  （公式リポジトリ、公式サイト、Explorer）で再確認する。
- 旧 JPYC Prepaid の `0x431D...` は現行コントラクトではない。
- `cargo deny check advisories` は既知の 11 件（`libsql` 由来 10 件と
  `paste` 1 件）を理由付きで ignore して CI に導入済み。新しい勧告は
  CI を失敗させる。詳細は [脆弱性トリアージ](SECURITY_ADVISORIES.md)。
- Termux 固有の `rustls-platform-verifier` 失敗は、テストの
  ローカルモック化により解消済み。
