---
type: Architecture Decision Record
title: "ADR-0009: payment_orders 部分 UNIQUE の却下"
description: "payment_orders への属性の部分 UNIQUE 追加を却下し、三層の同一性定義を記録する。"
tags: [adr, payment, payment-orders, rejected]
generated:
  by: dsh
  at: "2026-11-21T21:33:39+09:00"
---

## ADR-0009: payment_orders 部分 UNIQUE の却下

- Status: Rejected
- Date: 2026-11-20（吟味）
- Re-evaluated: 2026-11-21（再評価。却下を維持）
- Decision owners: Project owner
- Scope: `payment_orders` の同一性定義と一意制約
- 採番: DEC-04 側が ADR-0008 を使用するため、本 ADR は ADR-0009 とする（採番衝突回避）。

## Context

`payment_orders` に `(ppid, cid, recipient_address, amount_wei)` の部分 UNIQUE を追加し、
同一内容の注文をデータベース側で重複排除する提案が 2026-11-20 に吟味された。
しかし同じコンテンツを同じ金額で複数回購入するのは正当なユースケースであり、
属性の組は注文を識別しない。2026-11-21 に再評価したが、却下を維持する。

## Decision

提案を却下し、`payment_orders` のスキーマ変更を行わない。同一性は次の三層で定義する。

1. **注文同一性 = `order_id`（UUIDv4）**: 注文の作成単位を識別する。128bit CSPRNG で
   生成し、`migrations/002_payment_orders.sql` の `order_id TEXT PRIMARY KEY` で一意性を
   担保する。ウォレット署名の bind も `order_id` 単位で行う。同じ利用者・同じコンテンツ・
   同じ金額の注文でも、別の `order_id` を持つ正当な複数注文として扱う。
2. **送金同一性 = `(chain_id, tx_hash, log_index)`**: オンチェーン上の1回の送金を識別する。
   `migrations/002_payment_orders.sql` の `UNIQUE (chain_id, tx_hash, log_index)` が
   同一送金の二重計上を防ぐ。注文の作成回数を制限する制約ではない。
3. **受取先同一性 = `derivation_index`**: 注文ごとに派生した受取アドレスを識別する。
   `migrations/003_payment_order_index.sql` の部分 UNIQUE
   （`WHERE derivation_index IS NOT NULL`）がアドレス再利用を防ぐ。固定受取アドレスの
   レガシー注文は NULL を許容する。

三層は直交する。ある層の同一性を別の層の属性で置き換えると、正当な注文を誤って拒否する。

## 根拠

- `test_duplicate_ppid_cid_recipient_amount_orders_are_allowed`
  （`server/src/tests.rs`）: 同一 `ppid` / `cid` / 受取先 / 金額の注文を2件作成し、
  異なる `order_id` で両方が保存されることを固定する。部分 UNIQUE は2回目以降の
  正当な注文を作成不能にする。
- `migrations/002_payment_orders.sql`: `order_id TEXT PRIMARY KEY` と
  `UNIQUE (chain_id, tx_hash, log_index)` を既に定義済み。注文同一性と送金同一性は
  ここで担保されており、属性の組による代用は不要である。
- `migrations/003_payment_order_index.sql`: `derivation_index` の部分 UNIQUE が
  受取先同一性を担保する。R1-26 の並行注文対策も新規 migration を追加せず、
  `create_order` 側の原子的採番で対応済み（`TASK_INVENTORY.md`）。
- [ADR-0003](ADR-0003-jpyc-settlement.md) の決定 3（注文とウォレットの結合）と
  決定 4（注文ごとの受取アドレス）と整合する。注文の識別は `order_id`、
  送金の識別は `(chain_id, tx_hash, log_index)` であり、
  `(ppid, cid, recipient_address, amount_wei)` は識別子ではない。
- 2026-11-21 の再評価でも、スキーマ変更で解消すべき不具合は確認されなかった。
  チェックサム保護済みの migration 002 / 003 は変更しない。

## Consequences

- 同一内容の注文を複数作成でき、正確な購入履歴が残る。
- 重複注文の検出は `order_id` と送金同一性の組み合わせで行い、属性の組では行わない。
- `payment_orders` のスキーマは変更せず、既存 migration のチェックサムは維持される。
- 属性の組による誤った重複排除を防ぐため、本 ADR と回帰テストを参照する。

## Alternatives

- **部分 UNIQUE `(ppid, cid, recipient_address, amount_wei)`**: 正当な再購入を
  拒否するため却下（2026-11-20 吟味、2026-11-21 再評価でも維持）。
- **属性の組を注文IDに代用**: 注文同一性が送金・受取の状態に依存し、署名 bind と
  監査の対応が壊れるため却下。

## Links

- [ADR-0003: JPYC決済方式](ADR-0003-jpyc-settlement.md)（相互リンク）
- [詳細設計書](../DETAILED_DESIGN.md)
- [タスク一覧](../TASK_INVENTORY.md)
- [migrations/002_payment_orders.sql](../../server/migrations/002_payment_orders.sql)
- [migrations/003_payment_order_index.sql](../../server/migrations/003_payment_order_index.sql)
