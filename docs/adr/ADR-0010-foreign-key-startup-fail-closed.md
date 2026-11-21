---
type: Architecture Decision Record
title: "ADR-0010: 起動時 FK 孤立行の fail-closed 化"
description: "本番起動時に外部キー孤立行で起動を拒否し、開発では warn に留める方針を記録する。"
tags: [adr, database, foreign-key, operations, accepted]
generated:
  by: dsh
  at: "2026-11-22T06:20:00+09:00"
---

## ADR-0010: 起動時 FK 孤立行の fail-closed 化

- Status: Accepted
- Date: 2026-11-22
- Decision owners: Project owner
- Scope: 起動時の参照整合性検査と孤立行検出時の運用

## Context

`enable_foreign_keys` は `PRAGMA foreign_keys` をコネクション単位で有効化するが、
既存 DB に含まれる参照先不存在の行（孤立行）は新規 INSERT の拒否では解消されない。
従来は `find_foreign_key_violations` が `PRAGMA foreign_key_check` で孤立行を検出しても
warn を出すだけだったため、参照整合性が壊れた DB で本番起動できていた。
`PRAGMA foreign_key_check` は `PRAGMA foreign_keys` と別に動作するため、
リモート libSQL でも検査可能である。R1 分析を受け、本番の起動契約を安全側へ寄せる。

## Decision

1. 起動時に `PRAGMA foreign_key_check` を実行し、孤立行の有無と検査成否を判定する。
2. 本番相当（`ALLOW_MOCK_AUTH` 無効）では、孤立行が1件以上または検査失敗の場合に
   起動を拒否する（fail-closed）。ローカル DB とリモート DB の双方が対象。
3. 開発（`ALLOW_MOCK_AUTH=1`）では起動を止めず、従来どおり warn のみとする。
4. 判定は純粋関数 `foreign_key_startup_error(allow_mock_auth, violations, check_failed)`
   に集約する。拒否文言には件数と `table(rowid=...) -> parent` のみを含め、
   環境変数値や秘密値（`DATABASE_TOKEN` 等）はエコーしない。
5. `doctor` は本番相当で同じ判定を `production` カテゴリ内へ警告としてマージする。
   `results.push` は増やさず check23 を維持し、`check_all_with` のシグネチャは変更しない。
6. 是正 CLI や自動修復は追加しない。孤立行が検出された DB は作り直し前提とし、
   バックアップからの再リストアまたは参照先データの再構築で対応する。

## Consequences

- 参照整合性が壊れた DB では本番が起動しないため、サイレントな不正データ運用を防ぐ。
- リモート DB でも `foreign_key_check` が動作するため、コネクション設定の制約を
  理由にした見落としがなくなる。
- 開発では warn のみなので既存の開発フローは維持される。
- 孤立行検出時の復旧は手運用（作り直し）となり、自動是正は行わない。
- 起動拒否文言に秘密値を含めないため、ログやエラー出力からの漏えいリスクは増えない。

## Alternatives

- **従来どおり warn のみ**: 破損した参照整合性で本番運用が継続でき、
  R1 分析のリスクを解消できないため却下。
- **是正 CLI / 自動修復の追加**: 孤立行の削除や親行の自動生成はデータ喪失・
  誤った整合性回復を招く判断を含み、本 ADR のスコープ外とした。
- **`PRAGMA foreign_keys` のみでの判定**: コネクション設定であり既存孤立行を
  検出できないため却下。

## Links

- [運用ポリシー](../OPERATIONAL_POLICY.md)
- [バックアップ・リストア手順](../BACKUP_RESTORE_RUNBOOK.md)
- [ADR-0001: MVP の信頼境界](ADR-0001-mvp-trust-boundaries.md)
- [server/src/lib.rs](../../server/src/lib.rs)（`foreign_key_startup_error` / `run()`）
- [server/src/bin/doctor.rs](../../server/src/bin/doctor.rs)（`production` カテゴリ）
