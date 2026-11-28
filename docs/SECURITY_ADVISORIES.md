---
type: Security Advisory Triage
title: "依存クレートの脆弱性トリアージ"
description: "cargo-deny が検出した RUSTSEC 勧告の到達可能性と対応方針を記録する。"
tags:
  - security
  - dependencies
  - rustsec
  - triage
generated:
  by: dsh
  at: "2026-11-22T23:15:00+09:00"
verified:
  by: dsh
  at: "2026-11-28T16:06:31+09:00"
status: current
stale_after: "2027-02-15T00:00:00+09:00"
---

## 1. 結論

- `cargo deny check advisories`（cargo-deny 0.20.2）は 13件の勧告で
  失敗する（初回は 14 件）。内訳は脆弱性 9 件（当時・現在値は TEST_SPECIFICATION の実測を参照）、
  unmaintained 4 件。
- 13件は 4 系統に集約できる。
  - libsql 0.10.0 の TLS / gRPC スタック: 10 件
  - chrono の unmaintained 情報勧告: 1 件
  - url 2.5.8 の blob: URL スタックオーバーフロー（RUSTSEC-2026-0592）: 1 件
  - paste 1.0.15 のビルド時マクロ（RUSTSEC-2024-0436、unmaintained）: 1 件
- RustCrypto 0.13 系（RUSTSEC-2023-0071、RUSTSEC-2026-0387）は
  2026-11-15 の依存更新で解消済み（§3.2、§8）。
- 未使用の imageproc 経由で入っていた ttf-parser は、依存削除により
  解消済み（§3.3、§8）。
- `cargo deny check advisories` は既知の 13件を理由付きで ignore して
  CI に導入済み。新しい勧告は CI を失敗させる（§5）。
- 本番は `production_database_url_error` のゲートでリモート DSN
  （`libsql://` / `https://` / `http://` / `wss://` / `foo://` 等）を
  固定文言で起動拒否し、`file:` 絶対パスのみ受理する（fail-closed）。
  libsql 由来の 10 件の受容根拠は、本番がリモート DSN をゲートで禁止する
  点にある（§4.1、§5）。
- ECDSA は公開鍵検証のみ。RSA は `JWT_RSA_PRIVATE_KEY_PEM` 設定時に
  RS256 本番署名で秘密鍵演算を実行する。受容根拠は「演算なし」ではなく、
  rsa 0.10 の定数時間実装と 2048bit 以上の鍵長要求であり、
  Marvin 攻撃と timing 系勧告は解消済み（§3.2、§4.2）。
- 本番はリモート DSN をゲートで禁止する。リモート DB を採用する場合は、
  ゲートの解除と合わせて 3.1 の 10 件（bincode / rustls-pemfile を
  含む）の解消を必須とする。

## 2. 検出条件

- 対象: `master` の `8fb989c`（2026-11-03）。
- 実行: 2026-11-15 01:11 JST（初回）、同日 02:20 JST（依存更新後）。
  cargo-deny 0.20.2。
- コマンド: `cargo deny check advisories`。
- `Cargo.lock` には修正済みの新しい版（例: `rustls 0.23.46`、
  `rustls-webpki 0.103.16`、`h2 0.4.20`、`crypto-bigint 0.7.10`）が
  別経路で併存しており、勧告は旧版コピーだけに該当する。

## 3. 指摘一覧

### 3.1 libsql 0.10.0 の TLS / gRPC スタック（現行 10 件）

- `RUSTSEC-2025-0141`（2026-11-28 ignore 復帰、2026-11-22 の解消判断は誤り） bincode 1.3.3（unmaintained）
  - 経路: `libsql 0.10.0` → `bincode 1.3.3`
  - 修正版: なし（代替候補は wincode / postcard 等）
  - 評価: 本リポジトリのコードから直接使用しない。libsql の remote 接続時にのみ
    到達し、本番はリモート DSN をゲートで禁止する。
- `RUSTSEC-2026-0258` h2 0.3.27（低深刻度）
  - 経路: `libsql` → `tonic 0.11.0` → `hyper 0.14.32` → `h2 0.3.27`
  - 修正版: `>=0.4.16`（tonic / libsql の更新待ち）
  - 評価: gRPC クライアント経路。リモート接続時のみ到達する。
- `RUSTSEC-2026-0346` rustls 0.22.4（Debug が TLS 秘密を露出）
  - 経路: `libsql` → `hyper-rustls 0.25.0` → `rustls 0.22.4`
  - 修正版: `>=0.23.46`
  - 評価: セッション値を `Debug` 出力する独自実装を使わない限り影響なし。
- `RUSTSEC-2026-0349` rustls 0.22.4（ServerHello ダウングレード）
  - 経路: 同上
  - 修正版: `>=0.23.46`
  - 評価: リモート TLS 接続時のみ。PQ 機密性の低下であり鍵漏えいはない。
- `RUSTSEC-2025-0134`（2026-11-28 ignore 復帰、2026-11-22 の解消判断は誤り） rustls-pemfile 2.2.0（unmaintained）
  - 経路: `libsql` → `rustls-native-certs 0.7.3`
  - 修正版: なし（rustls-pki-types の `PemObject` へ移行）
  - 評価: 証明書読み込みの補助。TLS / remote 接続時にのみ到達し、既知の脆弱性はない。
- `RUSTSEC-2026-0049` rustls-webpki 0.102.8（CRL 照合）
  - 経路: `libsql` → `rustls 0.22.4` → `rustls-webpki 0.102.8`
  - 修正版: `>=0.103.10`
  - 評価: CRL を使わないため既定では到達しない。
- `RUSTSEC-2026-0098` rustls-webpki 0.102.8（URI 名前制約）
  - 経路: 同上
  - 修正版: `>=0.103.12`
  - 評価: 誤発行証明書が前提。通常の接続では実害が考えにくい。
- `RUSTSEC-2026-0099` rustls-webpki 0.102.8（ワイルドカード名前制約）
  - 経路: 同上
  - 修正版: `>=0.103.12`
  - 評価: 誤発行証明書が前提。通常の接続では実害が考えにくい。
- `RUSTSEC-2026-0104` rustls-webpki 0.102.8（CRL 解析の panic）
  - 経路: 同上
  - 修正版: `>=0.103.13`
  - 評価: CRL を使わないため既定では到達しない。
- `RUSTSEC-2026-0382` rustls-webpki 0.102.8（信頼しない中間 CA）
  - 経路: 同上
  - 修正版: `>=0.103.16`
  - 評価: 既知の emSign 中間鍵を含む経路を受理し得る。リモート接続時に
    注意が必要で、13 件の中では優先度が高い。

### 3.2 RustCrypto 0.13 系（2 件）— 解消済み

- `RUSTSEC-2026-0387` crypto-bigint 0.5.5（timing）
  - 経路: `k256 0.13.4` / `jsonwebtoken 11.1.0` →
    `elliptic-curve 0.13.8` → `crypto-bigint 0.5.5`
  - 修正版: `>=0.7.6`
  - 評価: 秘密値に対する定数時間演算の欠陥。RS256 秘密鍵署名は rsa 0.10 の
    定数時間実装と 2048bit 以上の鍵長要求で受容し、解消済み（§4.2）。
  - 対応: 2026-11-15 の依存更新で crypto-bigint 0.7.11 になり解消。
- `RUSTSEC-2023-0071` rsa 0.9.10（Marvin 攻撃）
  - 経路: `common` / `server` の直接依存と `jsonwebtoken 11.1.0`
  - 修正版: `>=0.10.0`
  - 評価: 秘密鍵演算のタイミング漏えい。2026-11-15 に追加した RS256 署名は
    rsa 0.10 の定数時間実装と 2048bit 以上の鍵長要求で受容し、解消済み。
  - 対応: 2026-11-15 に rsa 0.10.0 へ更新して解消。

### 3.3 ビルド時・未使用依存（2 件、うち ttf-parser は解消済み）

- `RUSTSEC-2024-0436` paste 1.0.15（unmaintained・2026-11-28 ignore 復帰、2026-11-22 の解消判断は誤り）
  - 経路: `leptos 0.8.22`（`either_of` / `reactive_graph` / `tachys`）
  - 修正版: なし（pastey / with_builtin_macros への移行）
  - 評価: 手続きマクロ。ビルド時にのみ使われ、実行時コードに残らない。
  - 対応: leptos 0.8.23 と 0.9.0-rc1 も `paste ^1.0` に依存するため
    削除は不可。既知の脆弱性がないため ignore で受容し、leptos の
    更新時に再評価する（§5）。2026-11-28 に ignore を復帰（2026-11-22 の
    削除判断は誤り）。Cargo.lock に実在し、RustSec DB にも継続掲載。
- `RUSTSEC-2026-0192` ttf-parser 0.25.1（unmaintained）— 解消済み
  - 経路: `imageproc 0.27.0` → `ab_glyph 0.2.32` → `ttf-parser 0.25.1`
  - 対応: `image` / `imageproc` / `image-webp` を 2026-11-15 に削除。
  - 評価: 未使用依存だったため、削除後は依存グラフに含まれない。

## 4. 到達可能性の評価

### 4.1 サーバー実行時（libsql）

- `server/src/lib.rs` の `is_remote_database_url` が DSN 判定の単一情報源で、
  `starts_with("file:") || !contains("://")` の否定（`file:` 開始または
  `://` 非含有ならローカル、それ以外がリモート）として実装する。
- 本番相当（`ALLOW_MOCK_AUTH` 無効）では `production_database_url_error` が
  `libsql://` / `https://` / `http://` / `wss://` / `foo://` 等のリモート DSN を
  固定文言で起動拒否し、`file:/var/lib/openillustration/local.db` 等の
  `file:` 絶対パスのみ受理する（fail-closed、値・トークンはエコーしない）。
  `run()` と doctor の FK 検査用 `open_db_for_foreign_key_check()` は
  同じヘルパで分岐する。
- 本番で受理される値は `Builder::new_local` を通り、TLS も gRPC も張らない。
  3.1 の 10 件の受容根拠は、本番がリモート DSN をゲートで禁止する点にある。
  開発（`ALLOW_MOCK_AUTH=1`）では従来どおりリモート DSN も許容する。
- 残リスク: 本番の production gate は `server` 本体と `doctor` にのみ実装され、
  CLI bin（`backup_db` / `restore_db` 等）には適用されないため remote DSN を
  許容する。CLI 経路からのリモート接続は別途ゲートを検討する。

### 4.2 暗号検証（RSA / ECDSA）

- `common::verify_rsa_signature` は `RsaPublicKey::verify` のみを使う
  （`common/src/lib.rs` 41 行目付近）。この検証ヘルパー自体は公開鍵のみを使う。
- RSA 秘密鍵演算は `JWT_RSA_PRIVATE_KEY_PEM` 設定時の RS256 本番署名と、
  テストの署名生成（`server/src/tests.rs`）で行う。
- `k256` はウォレット署名の検証（公開鍵演算）に使う
  （`server/src/payments.rs` 10 行目付近）。
- 2026-11-15 に rsa 0.10.0 / jsonwebtoken 12.0.0 / k256 0.14.0 へ更新し、
  Marvin（RUSTSEC-2023-0071）と crypto-bigint の timing 勧告
  （RUSTSEC-2026-0387）は解消した。
- 2026-11-15 に RS256 署名（RSA 秘密鍵演算）を追加した。`JWT_RSA_PRIVATE_KEY_PEM`
  設定時のみ有効で、既定は HS256 のまま。rsa 0.10 の定数時間実装を使い、
  鍵長は 2048bit 以上を要求する。秘密鍵は PEM から読み、検証用の公開鍵は
  秘密鍵から導出する。許可アルゴリズムは起動時に確定し、トークンの
  ヘッダからは選ばせない。
- JPKI リーフ公開鍵も RSA 2048bit 以上のみ受理する（`server/src/jpki.rs` の
  `JPKI_RSA_MIN_KEY_BITS` / `public_key_meets_policy`）。ECDSA / DSA 等の非 RSA は
  `JPKI leaf public key must be RSA 2048 bits or stronger` で拒否する。

### 4.3 クライアント

- paste は leptos のマクロ展開などビルド時に使われ、生成物に実行時
  コードとして残らない。
- ttf-parser は `imageproc` → `ab_glyph` 経由で入る。**2026-11-15 時点の履歴**として、
  当時の `client/Cargo.toml` は `image` / `imageproc` / `image-webp` を宣言していたが、
  `client/src` から参照されておらず、その後削除済み（§8 参照）。現行の `client/Cargo.toml`
  にはこれらの直接依存は存在しない。
- paste（RUSTSEC-2024-0436）は 2026-11-28 に ignore を復帰（2026-11-22 の
  解消判断は誤り）。Cargo.lock に実在し RustSec DB にも継続掲載されるが、
  ビルド時マクロで実行時コードに残らず到達なし。ttf-parser は未使用依存の
  削除により依存グラフから外れた（2026-11-15）。

## 5. 対応方針

- D1: 既定構成のまま本番公開しない。DEC-08（本番ネットワーク）と
  DEC-09（OSS ライセンス）のゲートと整合させる。
- D2: 解消可能な系統から段階的に更新する。
  - RustCrypto: `jsonwebtoken` 12.0.0、`rsa` 0.10.0、`k256` 0.14.0、
    `p256` 0.14.0 へ 2026-11-15 に更新し、3.2 の 2 件を解消した。
    ユニットテスト 89 件と E2E 9 件で回帰を確認済み。
  - クライアント: 未使用の `image` / `imageproc` / `image-webp` を削除し、
    3.3 の ttf-parser を解消した（2026-11-15）。
  - libsql: 0.10.0 が最新で、上流の tonic / rustls 更新待ち。
    2026-11-17 に crates.io の `max_version` を再確認し、現行版と同一
    （2026-10-13 公開）であることを確認済み。受容判断は変更なし。
    リモート DB 採用前に代替 crate を再評価する。
  - chrono: 2026-11-17 発行の RUSTSEC-2026-0543 は unmaintained（情報区分）で、
    修正版なし。一次情報 [RUSTSEC-2026-0543](https://rustsec.org/advisories/RUSTSEC-2026-0543.html)
    で確認。`common`/`server`/`libsql` 経由で使用。jiff / time への移行を
    別タスクとして計画し、現状は理由付きで受容（13件中 1 件）。
- D3: `cargo deny check advisories` を ignore リスト付きで CI に導入した。
  新しい勧告は CI を失敗させ、不要になった ignore も検出する。
- D4: 再評価のトリガーは、依存更新時、四半期ごと、本番化判断時。

### CI ゲートと ignore 運用

- `deny.toml` の `[advisories].ignore` に既知の 13件を理由付きで列挙する。
  理由は「経路」と「受容の根拠（本番はリモート DSN をゲートで禁止する）」を
  1 行で示す。
- `unused-ignored-advisory = "deny"` により、依存から消えた勧告の
  ignore が残っていれば CI を失敗させる。
- 新しい勧告は ignore しない限り CI を失敗させる。ignore の追加は
  本書の到達可能性評価を更新してから行う。
- 見直しは四半期ごと、依存更新時、本番化判断時に行う。

## 6. 再現手順

```bash
cargo deny check advisories
```

## 7. 未確認事項

- RS256 の秘密鍵演算（`JWT_RSA_PRIVATE_KEY_PEM` 設定時）は監視・再評価の対象。
  受容根拠は rsa 0.10 の定数時間実装と 2048bit 以上の鍵長要求で、
  Marvin/timing 勧告は解消済み（§3.2、§4.2）。
  鍵ローテーション手順は [鍵ローテーション Runbook](KEY_ROTATION_RUNBOOK.md) に記録済み。
  HSM 等の外部鍵管理は未実装（現状は PEM を環境変数で注入する）。
- libsql のローカル接続で bincode が実際に使われるかは未確認。
- リモート接続時の rustls / h2 経路はコード上の分岐のみ確認し、
  実接続試験は未実施。

## 8. 更新履歴

- 2026-11-15: 初版。`master` `8fb989c` 時点の 14 件を記録。
- 2026-11-15: 未使用の image 依存を削除し、ttf-parser を解消（14 → 13 件）。
- 2026-11-15: RustCrypto 系を更新し、rsa と crypto-bigint の勧告を解消
  （13 → 11 件、2026-11-17 時点は11件）。
- 2026-11-15: 既知 11 件（2026-11-17 時点は11件、履歴）を ignore して `cargo deny check advisories` を
  CI に導入。paste は上流修正がないため受容した。
- 2026-11-17: 依存更新時に再評価。libsql は 0.10.0（現行と同一）が最新で、
  受容 11 件（2026-11-17 時点は11件）に変更なし。uuid 1.29.2 / coins-bip32 0.17.2 へパッチ更新済み。
- 2026-11-17（履歴）: 11→12件。新規勧告 RUSTSEC-2026-0543（chrono unmaintained）を
  受容。脆弱性ではなく情報区分で修正版なし。jiff/time 移行を計画。
- 2026-11-22（履歴）: 12→10件。RUSTSEC-2025-0141 / 2025-0134 / 2024-0436 が
  DB 上検出されなくなったため ignore を削除し、新規 RUSTSEC-2026-0592
  （url 2.5.8）を理由付きで追加。当時の内訳は脆弱性 9 件 / unmaintained 1 件
  （libsql 系 8 件 + chrono 1 件 + url 1 件）。
- 2026-11-28: 10→13件（3件復帰）。RUSTSEC-2025-0141 / 2025-0134 / 2024-0436 は
  Cargo.lock に実在し RustSec DB にも継続掲載（withdraw なし）。2026-11-22 の
  解消判断は誤りとして ignore を復帰。現行内訳は脆弱性 9 件 / unmaintained 4 件
  （libsql 系 10 件 + chrono 1 件 + url 1 件 + paste 1 件、4 系統）。
