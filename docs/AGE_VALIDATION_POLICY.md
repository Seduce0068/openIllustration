---
title: "年齢判定ポリシー（DEC-02 採用方針・実装制約）"
type: Policy
description: 年齢判定ポリシーの採用方針（DEC-02）、現行実装、公開設定、制約を記録する。
tags: [age-validation, jpki, privacy, dec-02]
generated:
  by: dsh
  at: "2026-11-20T14:41:22+09:00"
---

## 年齢判定ポリシー（DEC-02 採用方針・実装制約）

> **状態:** DEC-02 の決定候補（方針フレームワーク）は採用し、コード/テストに反映済みです。ただし、対象地域・適用法令・属性ソース・保持期間・最終閾値の法的確定および専門家レビューは未完了です。
> 本書および実装を法的助言・各国法の網羅・本人確認済み年齢の証明・本番利用資格判定の完成として扱わないでください。

## 1. 採用した方針（DEC-02）

本プロジェクトは、年齢・対象地域の扱いについて以下の方向を採用する（実装済みの枠組みは `common/src/age.rs` および `server/src/lib.rs`）。

1. **保守的な一律安全余裕ベースライン**
   - 既定値は `AGE_DEFAULT_MINIMUM=21`（一律）。
   - これは安全余裕のための保守的な基準値であり、「21歳が世界で最も高い成人年齢である」といった事実の確定・普遍的正当性・法的適合性を主張するものではない。
2. **運用者による構成可能性と責任**
   - 対象地域や有効な年齢基準は運用者の判断・現地法令・運用方針に依存し得るため、国別しきい値・対象地域・適用モードを設定可能とする。
   - 既定値は保守的に保つ。運用者が設定を変更/緩和する場合は、適用法令・利用規約・対象コンテンツ・専門家レビューに基づく責任で行う。
3. **フェイルクローズ選択肢（明示設定の要求）**
   - `AGE_REQUIRE_EXPLICIT_CONFIGURATION=1` かつ `AGE_POLICY_CONFIGURED=0` の場合、年齢検証は通過させず拒否する（設定未完了としてフェイルクローズ）。
   - セッション発行の全経路（`/auth/verify` と OIDC の callback / refresh）が同じ設定ゲートで
     拒否する（後者は `ConfigurationRequired` / `TrustedAttributeRequired` を 403 に変換）。
4. **信頼できる属性に依存する検証方向（DEC-01連携）**
   - `AGE_ENFORCEMENT_MODE=trusted_attribute_required`（別名 `trusted_attribute`）は、検証済み年齢/居住地属性の入力経路が実装・接続されるまでフェイルクローズとする。
   - 現行API入力（`birth_date`, `country_code`）は自己申告であり、本人確認済み属性ではない（モック環境では特に信頼しない）。
5. **有効設定値の常時公開（非秘密）**
   - 有効な年齢/地域ポリシー設定値は常に公開可能な非秘密情報として扱う。
   - サーバーは認証不要の `GET /policy/age` で `AgePolicy::public_settings()`（`settings_always_public=true` を含む）を返す。
   - 公開対象はしきい値・国別上書き・対象地域・適用モード・明示設定状態・注記などのポリシー設定値に限る。`JWT_SECRET`, `JWT_RSA_PRIVATE_KEY_PEM`, `OIDC_CLIENT_SECRET`,
     `PAYMENT_RECEIVING_XPRV`, `PINATA_JWT`, DBトークン/認証情報その他の秘密情報は公開・ログ出力・応答に含めない。

## 2. 現行実装

### 2.1 実装箇所

- [common/src/age.rs](../common/src/age.rs): `AgePolicy`, `AgeEnforcementMode`, `AgeValidationError`,
  `AgePolicy::from_env()`, `AgePolicy::public_settings()`, `calculate_age`, `is_age_allowed_with_policy`。
- [server/src/lib.rs](../server/src/lib.rs): 起動時に `AgePolicy::from_env()` を読み込み検証（不正値は起動失敗）、`AppState.age_policy` に保持、
  `GET /policy/age` を公開、`/auth/verify` の年齢検証に `validate_age(&state.age_policy, ...)` を適用。
  `validate_age_at` / `today_for_policy` が基準日をローカル TZ で求め、`ensure_age_policy_configured` が
  セッション発行経路の設定ゲートを共通化する。
- [server/src/oidc.rs](../server/src/oidc.rs): `/auth/oidc/callback` と `/auth/refresh` が
  `ensure_age_policy_configured(&state.age_policy)?` を state / Cookie の消費より前に呼ぶ。
- [server/src/tests.rs](../server/src/tests.rs): `GET /policy/age` の公開応答、明示設定必須/信頼属性必須モードのフェイルクローズ、
  callback / refresh の共通ゲート拒否、TZ 境界の基準日を検証。

### 2.2 設定（すべて非秘密）

| 変数 | 既定値 | 説明 |
| --- | --- | --- |
| `AGE_DEFAULT_MINIMUM` | `21` | 一律の既定最少年齢（1以上、0 は年齢ゲート無効化のため拒否）。保守的な安全余裕ベースライン。 |
| `AGE_COUNTRY_OVERRIDES` | 空 | 国別上書き。形式: `CC=NN,CC=NN`（例: `JP=18,US=21`）。国コードは英字2文字を大文字化、年齢は1以上（0 は拒否）。 |
| `AGE_TARGET_REGIONS` | 空 | 対象地域コードのカンマ区切り（例: `JP,US,DE`）。英字2文字を大文字化し重複除去・ソート。英字2文字以外（例: `jpn`, `12`, `j-P`）は起動拒否（fail-closed）。 |
| `AGE_ENFORCEMENT_MODE` | `self_reported` | `self_reported` / `self_reported_dob_country`: 申告された生年月日・国コードで判定（信頼属性ではない）。`trusted_attribute_required` / `trusted_attribute`: 検証済み属性入力が未実装のため現時点では常に拒否（フェイルクローズ）。 |
| `AGE_REQUIRE_EXPLICIT_CONFIGURATION` | `false` | `1`/`true`=有効、`0`/`false`=無効のみ（yes/on/off/no は拒否）。明示設定を要求。 |
| `AGE_POLICY_CONFIGURED` | `false` | `1`/`true`=有効、`0`/`false`=無効のみ（yes/on/off/no は拒否）。設定確認/確定フラグ（前項目と併用）。 |

不正な値（未知モード、不正な真偽値、不正な国コード/年齢形式、範囲外年齢など）は `AgePolicy::from_env()` がエラーを返し、サーバー起動は失敗する（fail closed）。

### 2.3 公開エンドポイント

- `GET /policy/age`（認証不要）: 有効な年齢/地域ポリシー設定値を返す。設定値は常に公開される前提のため、秘密情報を含めない。
- 応答例の主なフィールド: `default_minimum_age`, `country_minimums`, `target_regions`, `operator_configured`,
  `require_explicit_configuration`, `enforcement_mode`, `settings_always_public`, `notes`。

### 2.4 基準日（ローカル TZ）

年齢判定の基準日は UTC の日付ではなく `TZ` 環境変数が指定するローカルの「今日」である。
`validate_age` は `std::env::var("TZ")` を読み、`today_for_policy` が次の規則で日付を求める。

| `TZ` の値 | 解釈される基準日 | 根拠 |
| --- | --- | --- |
| 未設定 / `UTC` / 未対応名（例: `America/New_York`） | UTC の現在日 | 未知の TZ で境界を意図せず早めない fail-safe な既定 |
| `Asia/Tokyo`（前後の空白は除去、大文字小文字を問わない） | 固定オフセット UTC+9 の現在日 | 日本に夏時間がなくオフセットが変化しないため |
| `Etc/GMT±N` | POSIX 規約に従い符号を反転（例: `Etc/GMT-9` = UTC+9、`Etc/GMT+5` = UTC-5） | `chrono::FixedOffset` への変換 |

JST 深夜の境界例: UTC `2024-01-01T16:00:00Z` は JST では `2024-01-02 01:00` になる。
既定の最低年齢 21 歳に対し、`2003-01-02` 生まれは JST（`Asia/Tokyo` / `Etc/GMT-9`）基準では
2024-01-02 時点で 21 歳として受理され、UTC（`TZ` 未設定 / `UTC`）基準では 2024-01-01 時点で
20 歳として 403 で拒否される。基準日が 1 日ずれるだけでも判定が反転し得るため、
本番では `TZ` を対象地域に合わせて明示設定する（[OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md) 参照）。

## 3. 現行の制約・未完了事項

- 本人確認済み発行者からの生年月日・居住国・年齢属性の取得、提示署名/nonce/信頼ルート/失効検証は未実装（DEC-01連携）。
- `trusted_attribute_required` モードは、検証済み属性入力経路がないため現時点で常に拒否する。
- 対象地域、必要属性、閾値、属性保持期間、コンテンツ種別ごとの適用法令の最終確定は未決定（DEC-02の残作業）。
- 「21歳が最も高い成人年齢」等の外部事実は一次ソースに基づく確認と専門家レビューが必要であり、本実装の既定値をその事実の証明として扱わない。
- 年齢判定の結果は、現行入力が自己申告である限り、法的・本人確認上の保証にならない。

## 4. 本番化前に必要なこと

- 対象地域・対象コンテンツ・適用閾値・信頼できる属性ソース・個人情報保持範囲を決定し、専門家レビューを完了する。
- DEC-01（JPKI/信頼属性）と接続し、`trusted_attribute_required` 運用時に検証済み属性のみを用いる経路を実装・検証する。
- `GET /policy/age` を含む公開面に秘密情報が混入しないことを継続的に確認する（設定追加時のレビュー対象）。
- 境界値（閏日、未来日、国コード正規化、設定変更時の挙動）と実運用手順（変更管理、監査、利用者告知）を整備する。
- 「保守的」「法的に安全」「準拠済み」等の表現は、根拠（一次ソース・レビュー記録・試験証跡）がある範囲に限定する。
