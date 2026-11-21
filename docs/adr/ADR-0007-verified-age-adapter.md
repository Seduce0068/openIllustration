---
type: Architecture Decision Record
title: "ADR-0007: 署名付き年齢属性 adapter の境界と発行者許可リスト"
description: "DEC-01 連携に向けた署名付き年齢属性 adapter の境界、mock 経路のみの結線、発行者許可リストの変更管理を記録する。"
tags: [adr, age-validation, dec-01, adapter, accepted]
generated:
  by: dsh
  at: "2026-11-22T02:38:26+09:00"
---

## ADR-0007: 署名付き年齢属性 adapter の境界と発行者許可リスト

- Status: Accepted
- Date: 2026-11-21
- Scope: 署名付き年齢属性の検証境界、mock 経路の結線、発行者許可リストの運用

## Context

年齢判定は現在、自己申告の `birth_date` / `country_code` に基づく
`SelfReportedDobCountry` モードを既定としている。DEC-01（JPKI / 信頼属性）の
実証明書・OP クレーム仕様は未確定であり、検証済み属性の入力経路は存在しない。
一方で、信頼属性必須モード（R1-17）は fail closed を維持する必要がある。

R1 分析では、署名付き属性を扱う責務を狭い adapter 境界に閉じ込み、
自己申告値と混同しないこと、発行者を許可リストで制御することが求められた。

## Decision

### adapter 構成

- 新規 `server/src/verified_age.rs` に adapter を置く。検証境界は
  `verify(issuer, claims, nonce, audience) -> Result<VerifiedAge, AppError>`
  の4引数に限定し、署名・信頼ルート・失効の具体検証は呼び出し側へ漏らさない。
- 既存の `VerifiedAgeAttribute`（`issuer` / `signed_subject` / `nonce`、
  `deny_unknown_fields`）を attestation として再利用し、年齢クレーム
  （`birthdate` / `country`）と分離する。自己申告フィールドを混ぜた
  JSON は `deny_unknown_fields` で拒否する。
- adapter は発行者許可リストとの完全一致、attestation の発行者束縛、
  audience 束縛、nonce 束縛、クレームの存在を検証する。いずれの失敗も
  `TrustedAttributeRequired` と同等の 403 に変換する。
- nonce の単回消費は既存 `MOCK_NONCES.remove()` を再利用する。
  audience は OIDC `client_id` をフォールバックに使用する。
- OIDC callback では `ensure_age_policy_configured` の直後に adapter 境界を
  呼ぶ。実証明書は未接続のため信頼属性必須モードでは fail closed とし、
  認可フロー（state / PKCE / ID トークン検証）は変更しない。
- `auth_verify` の mock 分岐では `allow_mock_auth` 有効時のみ
  `verified_claims` を受理する。欠落・不正は 403 とし、自己申告の
  `birth_date` / `country_code` は信頼属性として扱わない。
- `auth_verify` の verified_claims 経路の `user_hash` は
  `sha256("verified-age:v1:{issuer}:{signed_subject}")` から導出する（R2-USERHASH）。
  `nonce` / `birthdate` / `country` は入力に含めず、sha256 の 64-hex を維持する。
  `issuer` と `signed_subject` は adapter が返す `VerifiedAge` から取得し、
  `signed_subject` が trim 後も空なら adapter の一段目に加え導出ヘルパの二段目でも
  `TrustedAttributeRequired`（403）で拒否する。OIDC callback の `sub` 生値は
  別契約（R1 の経路別契約判断）であり変更しない。verify_key 分岐・自己申告分岐も
  変更しない。

### 発行者許可リストの構成

- `AgePolicy::trusted_issuers` と `AGE_TRUSTED_ISSUERS` を追加する。
  カンマ区切り・trim・空要素除去を行い、比較は完全一致のみ（部分一致や
  大文字小文字の正規化は行わない）。
- `TrustedAttributeRequired` かつ許可リストが空の場合は起動を拒否する
  （fail closed）。既定の `SelfReportedDobCountry` では許可リストは
  任意であり、信頼属性必須モードへ自動昇格しない。

### 非秘密性

- 発行者識別子と閾値・対象地域・適用モードと同様に、許可リストは
  公開可能な非秘密設定として扱う。`GET /policy/age` の
  `AgePolicy::public_settings()` から公開され、`settings_always_public=true`
  を維持する。秘密情報（JWT 鍵、OIDC クライアントシークレット、
  DB トークン等）は一切含めない。

## 変更管理

- `AGE_TRUSTED_ISSUERS` の追加・削除は設定リポジトリの変更として管理し、
  レビューと承認を経て from_env に反映する。不正値は起動拒否となる。
- 発行者を追加する場合は、属性スキーマ、署名検証方針、失効確認方針の
  確定と専門家レビューを前提とする。実証明書未接続の段階では
  `trusted_attribute_required` への切替えを行わない。
- `GET /policy/age` は設定変更の公開確認に使う。公開面へ秘密情報が
  混入していないことを変更のたびに確認する。
- `user_hash` 導出契約（`"verified-age:v1:"` プレフィックスと入力フィールド）の
  変更は `users.user_hash` と JWT `sub` を変える破壊的変更として扱い、
  ADR 改訂と開発 DB の作り直しを伴う。バージョンを上げる場合は移行方針を
  本 ADR に記録する。実証明書接続前に `signed_subject` の pairwise 性を
  確定し、必要なら導出契約を見直す。

## 緊急 delist

- 発行者の侵害や誤発行が疑われる場合は、許可リストから当該発行者を
  削除して設定をロールバックし、サーバーを再起動する（設定は起動時に
  読み込むため再起動が必要）。
- delist 後は当該発行者からの属性が 403 で拒否されることを
  `AGE_TRUSTED_ISSUERS` の公開応答と検証ログで確認する。
- 恒久対応として署名キー失効・証明書失効の反映と事後レビューを
  行い、再加入には上記変更管理を再度経る。

## Consequences

- mock 経路のみの結線であり、実証明書検証・失効確認は未実装のままである。
- 発行者許可リストが公開されるため、信頼対象の列挙が外部へ開示される
  （非秘密設定として許容する設計判断）。
- 許可リスト変更時は再起動が必要で、誤設定は起動拒否として検出される。
- 旧 `sha256("{birthdate}:{country}")` で発行した開発 DB の `user_hash` は
  新導出と一致しないため、開発 DB は作り直し前提となる（移行パスなし）。
- `signed_subject` の pairwise 性が未確定のため、発行者が固定値を返す場合は
  同じ主体の hash が使い回される。実証明書接続前の仕様確定が必須である。

## Links

- [AGE_VALIDATION_POLICY.md](../AGE_VALIDATION_POLICY.md)
- [ADR-0002](ADR-0002-jpki-authentication.md)
- [TEST_SPECIFICATION.md](../TEST_SPECIFICATION.md)
