---
type: Architecture Decision Record
title: "ADR-0011: 性能計測の3層と受入値の扱い"
description: "E2E 所要時間・描画タイミング・転送量を3層で計測し、受入値は合意後に設定する方針を記録する。"
tags: [adr, performance, measurement, proposed]
generated:
  by: dsh
  at: "2026-11-22T07:00:00+09:00"
---

## ADR-0011: 性能計測の3層と受入値の扱い

- Status: Proposed
- Date: 2026-11-22
- Decision owners: Project owner
- Scope: 性能指標の定義、計測手順、受入値の合意プロセス

## Context

README と TEST_SPECIFICATION の PERF-01 は「高速・軽量」を設計目標としているが、
数値目標と達成状況は未決定（未作成）のままである。現行の E2E は機能の成否を検証するだけで、
描画速度・API 応答・配信サイズを記録していない。
このまま名目上のゲートだけを pass 扱いにすると、受入値がないまま
「性能要件を満たした」と誤って主張する危険がある。
R1 分析を受け、まず観測可能性（Level 0）を整備し、受入（Level 1）は合意後に分離する。

## Decision

1. 計測は次の3層で行う。
   - 層1: E2E 所要時間。既存 E2E シナリオの wall-clock 経過時間を記録する。
   - 層2: `first-contentful-paint` と `responseEnd`。ブラウザの Paint / Navigation Timing
     API から取得する。
   - 層3: 転送量と wasm サイズ。ネットワーク転送バイト数とビルド成果物の wasm サイズを記録する。
2. 各層は3回測定し、中央値を代表値とする。単回の最小値・最大値は受入判定へ使わない。
3. 受入値（SLO しきい値）は **未合意** とする。合意するまで大小比較・閾値判定は行わない。
   Level 0（観測可能性）は値が有限かつ 0 以上でエビデンスへ保存されることを pass 条件とし、
   Level 1（受入）は本 ADR が Accepted へ進み受入値が確定した後に適用する。
4. メモリ使用量は **参考値のみ** とし、受入条件・アラート判定へ使わない。
   計測環境差が大きく、単一のしきい値を設ける根拠が現状ないためである。
5. 観測値は E2E の `testInfo.attach('perf-observation', ...)` に JSON として保存し、
   リリース証跡から参照できるようにする。観測値に個人情報・トークン・生 URL は含めない。
6. 受入値の合意までは PERF-01 を `not_evaluated` のまま維持し、
   本番移行ゲートでは Level 1 をブロッカーとして扱う
   （[運用ポリシー](../OPERATIONAL_POLICY.md) 第6章）。

## Consequences

- 閾値のないまま「pass」と表示される名目ゲートを解消し、未合意を明示できる。
- 3回測定の中央値により、単発のジッタで結論が変わることを抑える。
- 観測値そのものは信頼できるが、受入値が無い期間は性能を主張できない。
- メモリを参考値へ留めるため、メモリ関連の障害は別途の観測・調査で扱う。
- 受入値の合意には計測環境（CI ホスト、ブラウザ、ビルド成果物）の固定が必要である。

## Alternatives

- **既存の名目ゲートを維持**: 受入値が無いまま pass と誤認され、
  R1 分析のリスクを解消できないため却下。
- **単回計測で受入値を仮置き**: 環境差とジッタに対して誤判定が生じ、
  合意なき数値が事実上の仕様化されるため却下。
- **メモリもしきい値対象に含める**: 計測環境差が大きく、
  妥当な単一の値を現時点で根拠付けできないため見送り。

## Links

- [TEST_SPECIFICATION](../TEST_SPECIFICATION.md)（PERF-01 / 性能・互換ゲートの評価状況）
- [運用ポリシー](../OPERATIONAL_POLICY.md)（第6章 本番移行ゲート）
- [ADR-0012: ブラウザ互換マトリクスと状態分類](ADR-0012-compatibility-matrix.md)
- [e2e/tests/metrics.spec.ts](../../e2e/tests/metrics.spec.ts)（`perf-observation` 添付）
