---
type: Architecture Decision Record
title: "ADR-0005: 監査エクスポートと署名検証"
description: "監査ログを署名付きでエクスポートし、公開鍵で第三者検証可能にする方式を決定する。"
tags: [adr, audit, verification, accepted]
generated:
  by: dsh
  at: "2026-11-18T13:40:37+09:00"
---

## ADR-0005: 監査エクスポートと署名検証

- Status: Accepted（プロジェクトオーナーの委任に基づくエージェント専門家判断）
- Date: 2026-11-17
- Scope: `content_audit_log` と `governance_audit_log`（BAN/UNBAN/ピン解除）のエクスポート形式と署名検証

## Context

DEC-04 は第三者検証可能な監査を本番要件とする。現行は DB 内ハッシュ連鎖と
Merkle root により改変・欠落の検出はできるが、データ自体が本物であることを
利用者が検証する手段がない。外部アンカー先・鍵運用・公開範囲は未決定であり、
これらを待つと監査機能の前進が止まる。

## Decision

1. 監査エントリを正規化した JSON スナップショットとしてエクスポートする。
2. スナップショットはバージョン、**全エントリ（CID・状態・主体・理由・連鎖ハッシュ・エントリハッシュ）**、
   `governance_entries`（BAN/UNBAN/ピン解除の `action`・対象・運用者・根拠・詳細・時刻）、
   Merkle root、署名、`reasons_masked` フラグを含む。未マスク版では公開鍵を持つ第三者が
   エントリからハッシュ・連鎖・root を再計算できる。ただし既定のマスク版では理由本文が
   スナップショットに存在しないため、エントリハッシュの再計算は行わない（下記 6. 参照）。
   いずれの版も連鎖・連番・Merkle root と署名は検証可能である。
3. 署名は運用者が管理する RSA 秘密鍵（PKCS#8 PEM）で行い、公開鍵を配布する。
   署名対象は `content_audit_log` と `governance_audit_log` を含むスナップショット全体である。
4. 検証コマンドは公開鍵だけで署名・ハッシュ連鎖・Merkle root を検証する。
5. 署名は「この運営者がこのスナップショットを公開した」ことを示すのみで、
   外部アンカーや追記専用媒体の代替ではない。
6. `reason` / `details` は既定でマスクし、本文を含める場合は `--include-reasons`
   の明示的なオプトインを要求する。署名対象はマスク適用後の文字列である。
   スナップショットには `reasons_masked` フラグ（署名対象）を含め、マスク時は
   `entry_hash` の再計算をスキップする。元の理由がスナップショットに無いため
   再計算が一致しないためである。署名が `entry_hash` 列全体を保護し、
   連鎖・連番・Merkle root は従来どおり検証される。後方互換のため
   `reasons_masked=false`（未マスク）の署名対象は旧 v3 形式を維持し、
   既存 v3 スナップショットはフラグ無しでも署名検証できる。新規のマスク版のみ
   `masked` 識別子を署名対象へ含め、フラグ改ざん（true→false）を検出する。
7. 本番で必要な外部アンカー、鍵の HSM 管理、公開範囲の最終判断は別 ADR で決定する。

### 実装状況（2026-11-18）

- 既定（マスク）のエクスポートが署名検証できることを実 DB で確認済み。
  マスク時に `entry_hash` 再計算を不一致で失敗させていた不具合を、
  `reasons_masked` フラグの導入と再計算スキップで解消した。
- `governance_entries`（BAN/UNBAN/ピン解除）は version 3 スナップショットに
  含まれ、公開鍵を持つ第三者が改ざんを検出できる（ADR-0004 の運用操作を参照）。
- 署名付きスナップショットの生成・検証手順は
  [AUDIT_EXPORT_RUNBOOK.md](../AUDIT_EXPORT_RUNBOOK.md) に集約する。

## Consequences

- 公開鍵を入手した第三者は、DB にアクセスせずスナップショットの完全性を検証できる。
- 秘密鍵を共有した運用者はスナップショットを偽造できるため、鍵管理が信頼の境界になる。
- 外部アンカーが無いため、過去スナップショットの existence 改ざん（総削除等）は検出できない。
- 署名鍵のローテーションと旧公開鍵の保持方針は運用規約に委ねる。

## Alternatives

- **DB ハッシュ連鎖のみ**: 運用者が全データを差し替えても検出できないため不採用。
- **ブロックチェーンアンカー**: 外部依存と費用が大きく、現段階では未採用。
- **対称鍵 MAC**: 公開検証ができないため不採用。

## Links

- [ADR-0001](ADR-0001-mvp-trust-boundaries.md)
- [DETAILED_DESIGN.md](../DETAILED_DESIGN.md)
- [OPERATIONAL_POLICY.md](../OPERATIONAL_POLICY.md)
