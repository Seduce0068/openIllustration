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
  at: "2026-11-15T01:11:08+09:00"
verified:
  by: dsh
  at: "2026-11-17T01:56:58+09:00"
status: current
stale_after: "2027-02-15T00:00:00+09:00"
---

## 1. 結論

- `cargo deny check advisories`（cargo-deny 0.20.2）は 11 件の勧告で
  失敗する（初回は 14 件）。内訳は vulnerability 8 件、unmaintained 3 件。
- 11 件は 2 系統に集約できる。
  - libsql 0.10.0 の TLS / gRPC スタック: 10 件
  - leptos 経由のビルド時依存 paste: 1 件
- RustCrypto 0.13 系（RUSTSEC-2023-0071、RUSTSEC-2026-0387）は
  2026-11-15 の依存更新で解消済み（§3.2、§8）。
- 未使用の imageproc 経由で入っていた ttf-parser は、依存削除により
  解消済み（§3.3、§8）。
- `cargo deny check advisories` は既知の 11 件を理由付きで ignore して
  CI に導入済み。新しい勧告は CI を失敗させる（§5）。
- 既定の開発構成（`DATABASE_URL` 未設定で `file:local.db`）では、libsql の
  TLS / gRPC 経路は実行時に使われない。libsql 由来の 10 件は
  「ビルドはされるが既定構成では到達しない」。
- RSA と ECDSA は公開鍵検証のみで、秘密鍵演算はテストに限られる。
  Marvin 攻撃と timing 系勧告の現状の実害は低いと判断する。
- 本番公開前、およびリモート DB を採用する前に解消を必須とする。

## 2. 検出条件

- 対象: `master` の `8fb989c`（2026-11-03）。
- 実行: 2026-11-15 01:11 JST（初回）、同日 02:20 JST（依存更新後）。
  cargo-deny 0.20.2。
- コマンド: `cargo deny check advisories`。
- `Cargo.lock` には修正済みの新しい版（例: `rustls 0.23.46`、
  `rustls-webpki 0.103.16`、`h2 0.4.20`、`crypto-bigint 0.7.10`）が
  別経路で併存しており、勧告は旧版コピーだけに該当する。

## 3. 指摘一覧

### 3.1 libsql 0.10.0 の TLS / gRPC スタック（10 件）

- `RUSTSEC-2025-0141` bincode 1.3.3（unmaintained）
  - 経路: `libsql 0.10.0` → `bincode 1.3.3`
  - 修正版: なし（代替候補は wincode / postcard 等）
  - 評価: 本リポジトリのコードから直接使用しない。
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
- `RUSTSEC-2025-0134` rustls-pemfile 2.2.0（unmaintained）
  - 経路: `libsql` → `rustls-native-certs 0.7.3`
  - 修正版: なし（rustls-pki-types の `PemObject` へ移行）
  - 評価: 証明書読み込みの補助。既知の脆弱性はない。
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
    注意が必要で、10 件の中では優先度が高い。

### 3.2 RustCrypto 0.13 系（2 件）— 解消済み

- `RUSTSEC-2026-0387` crypto-bigint 0.5.5（timing）
  - 経路: `k256 0.13.4` / `jsonwebtoken 11.1.0` →
    `elliptic-curve 0.13.8` → `crypto-bigint 0.5.5`
  - 修正版: `>=0.7.6`
  - 評価: 秘密値に対する定数時間演算の欠陥。現行の利用は公開鍵検証のみ。
  - 対応: 2026-11-15 の依存更新で crypto-bigint 0.7.11 になり解消。
- `RUSTSEC-2023-0071` rsa 0.9.10（Marvin 攻撃）
  - 経路: `common` / `server` の直接依存と `jsonwebtoken 11.1.0`
  - 修正版: `>=0.10.0`
  - 評価: 秘密鍵演算のタイミング漏えい。現行の利用は公開鍵検証のみ。
  - 対応: 2026-11-15 に rsa 0.10.0 へ更新して解消。

### 3.3 ビルド時・未使用依存（2 件、うち ttf-parser は解消済み）

- `RUSTSEC-2024-0436` paste 1.0.15（unmaintained）
  - 経路: `leptos 0.8.22`（`either_of` / `reactive_graph` / `tachys`）
  - 修正版: なし（pastey / with_builtin_macros への移行）
  - 評価: 手続きマクロ。ビルド時にのみ使われ、実行時コードに残らない。
  - 対応: leptos 0.8.23 と 0.9.0-rc1 も `paste ^1.0` に依存するため
    削除は不可。既知の脆弱性がないため ignore で受容し、leptos の
    更新時に再評価する（§5）。
- `RUSTSEC-2026-0192` ttf-parser 0.25.1（unmaintained）— 解消済み
  - 経路: `imageproc 0.27.0` → `ab_glyph 0.2.32` → `ttf-parser 0.25.1`
  - 対応: `image` / `imageproc` / `image-webp` を 2026-11-15 に削除。
  - 評価: 未使用依存だったため、削除後は依存グラフに含まれない。

## 4. 到達可能性の評価

### 4.1 サーバー実行時（libsql）

- `server/src/lib.rs` は `DATABASE_URL` 未設定時に `file:local.db` を使う
  （256 行目付近）。`file:` または `://` を含まない値では
  `Builder::new_local` を通り、TLS も gRPC も張らない。
- `DATABASE_URL` に `libsql://` や `https://` を設定した場合だけ
  `Builder::new_remote` を通り、rustls / h2 / bincode の経路が有効になる。
- したがって 3.1 の 10 件は、既定の開発・CI 構成では到達しない。
  リモート DB を本番採用する場合は到達する。

### 4.2 暗号検証（RSA / ECDSA）

- `common::verify_rsa_signature` は `RsaPublicKey::verify` のみを使う
  （`common/src/lib.rs` 41 行目付近）。秘密鍵演算はない。
- サーバーの RSA 秘密鍵演算は `server/src/tests.rs` の署名生成だけ。
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

### 4.3 クライアント

- paste は leptos のマクロ展開などビルド時に使われ、生成物に実行時
  コードとして残らない。
- ttf-parser は `imageproc` → `ab_glyph` 経由で入る。`client/Cargo.toml`
  は `image`（28 行目）、`imageproc`（29 行目）、`image-webp`（30 行目）を
  宣言するが、`client/src` から参照されていない。
- したがって 3.3 の paste は実行時影響がない。ttf-parser は未使用依存の
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
    リモート DB 採用前に代替 crate を再評価する。
- D3: `cargo deny check advisories` を ignore リスト付きで CI に導入した。
  新しい勧告は CI を失敗させ、不要になった ignore も検出する。
- D4: 再評価のトリガーは、依存更新時、四半期ごと、本番化判断時。

### CI ゲートと ignore 運用

- `deny.toml` の `[advisories].ignore` に既知の 11 件を理由付きで列挙する。
  理由は「到達経路」と「受容の根拠」を1行で示す。
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

- RS256 の秘密鍵演算は監視・再評価の対象。鍵ローテーションと
  HSM 等の外部鍵管理は未実装（現状は PEM を環境変数で注入する）。
- libsql のローカル接続で bincode が実際に使われるかは未確認。
- リモート接続時の rustls / h2 経路はコード上の分岐のみ確認し、
  実接続試験は未実施。

## 8. 更新履歴

- 2026-11-15: 初版。`master` `8fb989c` 時点の 14 件を記録。
- 2026-11-15: 未使用の image 依存を削除し、ttf-parser を解消（14 → 13 件）。
- 2026-11-15: RustCrypto 系を更新し、rsa と crypto-bigint の勧告を解消
  （13 → 11 件）。
- 2026-11-15: 既知 11 件を ignore して `cargo deny check advisories` を
  CI に導入。paste は上流修正がないため受容した。
