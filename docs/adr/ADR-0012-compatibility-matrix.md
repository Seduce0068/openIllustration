---
type: Architecture Decision Record
title: "ADR-0012: ブラウザ互換マトリクスと状態分類"
description: "ブラウザ×OS×主要フローのマトリクスと verified/proxy_only/not_verified/blocked の状態を定義する。"
tags: [adr, compatibility, browser, proposed]
generated:
  by: dsh
  at: "2026-11-22T07:00:00+09:00"
---

## ADR-0012: ブラウザ互換マトリクスと状態分類

- Status: Proposed
- Date: 2026-11-22
- Decision owners: Project owner
- Scope: 対応ブラウザ・OS・主要フローの可視化と未検証領域の扱い

## Context

README は Chrome・Edge・Brave・Firefox・Safari を基本対象とするが、
対応 OS・端末・最小バージョンは未決定（未作成）である。
現行 E2E は chromium で実行され、firefox / webkit はホスト側の共有ライブラリ不足で
実実行していない。同一エンジンを別ブラウザ名で「対応」と数えると検証範囲を過大主張し、
未検証の組み合わせが暗黙に除外される。
R1 分析に従い、状態を明示して未検証を可視化する。

## Decision

1. マトリクスは **ブラウザ × OS × 主要フロー** の3軸で作る。
   主要フローは閲覧・投稿・認証・決済とし、JXL / Wasm デコードは閲覧に含める。
2. 各セルの状態は次の4値のいずれかで表す。
   - `verified`: 実ブラウザ・実 OS で当該フローを実行し確認した。
   - `proxy_only`: 同一レンダリングエンジンの他ブラウザで代替確認したのみ。
   - `not_verified`: 未検証。実機も代替も行っていない。
   - `blocked`: 環境・仕様上の理由で検証不能または機能提供不可。理由を併記する。
3. ブラウザ名とエンジンの対応は次の修飾子で記録し、`verified` へ昇格させない。
   - Edge / Brave は `same_engine_proxy`（Chromium 系としての代替確認）。
   - Safari は `webkit_proxy`（WebKit 系としての代替確認）。
4. 認証・決済の実サービス接続を伴わない確認は `functional_only` と記録する。
   `functional_only` はモック OP / モック verifier によるフロー確認であり、
   実 JPKI または実チェーンの検証を証明しない。
5. 最小バージョンは **未決定** とする。合意までは「対応」と書かず、
   バージョンを明示した行を追加しない。
6. **暗黙除外禁止**: マトリクスに載っていない組み合わせは「対象外」とみなさず、
   `not_verified` として扱う。対象外にするには `blocked` と理由の明示が必要である。
7. 現状の E2E プロジェクト選択（`chromium` / `firefox` / `webkit`）とその可否は
   添付 `compat-selection` に記録し、マトリクスの入力とする
   （[TEST_SPECIFICATION](../TEST_SPECIFICATION.md) COMPAT-01）。
8. 本 ADR が Accepted へ進み最小バージョンと受入フローが合意されるまで、
   COMPAT-01 / COMPAT-02 は `not_evaluated` を維持し、
   本番移行ゲートの Level 1 をブロッカーとして扱う。

## Consequences

- 未検証の組み合わせが `not_verified` として残るため、暗黙に消えない。
- 同一エンジンの代替確認を `verified` と混同しないため、対応範囲を過大主張しない。
- 認証・決済を `functional_only` と明示することで、実接続未検証が可視化される。
- 最小バージョン未決定の間は「全対応」を主張できない。
- マトリクスの維持コストが増えるが、ゲートの根拠を監査可能にする。

## Alternatives

- **ブラウザ名のみで対応表を作る**: エンジン共有を根拠に過大な対応主張となり、
  未検証領域が隠れるため却下。
- **未記載を対象外とみなす**: 暗黙除外を許容し、
  R1 分析が指摘する「名目ゲート」と同じ問題を再現するため却下。
- **最小バージョンを暫定値で先行決定**: 実機検証の根拠がなく、
  合意なき数値が事実上の仕様化されるため見送り。

## Links

- [TEST_SPECIFICATION](../TEST_SPECIFICATION.md)（COMPAT-01 / COMPAT-02 / 性能・互換ゲートの評価状況）
- [運用ポリシー](../OPERATIONAL_POLICY.md)（第6章 本番移行ゲート）
- [ADR-0011: 性能計測の3層と受入値の扱い](ADR-0011-performance-measurement.md)
- [e2e/tests/build-gate.spec.ts](../../e2e/tests/build-gate.spec.ts)（`compat-selection` 添付）
