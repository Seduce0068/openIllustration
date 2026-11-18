---
type: Architecture Decision Record
title: "ADR-0006: JPEG XL デコード検証の技術選定"
description: "アップロード時の JPEG XL 完全デコード検証に用いる crate と採用判断を記録する。"
tags: [adr, jxl, dependencies, accepted]
generated:
  by: dsh
  at: "2026-11-18T13:40:37+09:00"
---

## ADR-0006: JPEG XL デコード検証の技術選定

- Status: Accepted（技術選定と現時点の採用判断）
- Date: 2026-11-17
- Scope: アップロード時の完全な JPEG XL デコード検証

## Context

現行の `validate_upload_content` は JXL のシグネチャ（raw codestream の
`FF 0A` または ISOBMFF の `JXL` box）のみを検査する。完全なデコード検証は
未実装で、TASK_INVENTORY に残っている。グローバル指示に従い、新規依存を
追加する前に既存ソリューションを調査した。

なおクライアント側の描画用途では、後述の選定に基づき `jxl-oxide = "0.12"` を
依存へ追加し、`client/src/jxl.rs` で JXL をデコードして Canvas へ描画している。
これは表示のためのクライアント処理であり、サーバー側のアップロード検証とは
分離されている（「実装状況」参照）。

## 調査結果（2026-11-17）

| crate | 版 | ライセンス | 直近DL | 評価 |
| --- | --- | --- | --- | --- |
| [jxl-oxide](https://crates.io/crates/jxl-oxide) | 0.12.6 | MIT OR Apache-2.0 | 約115万/月 | pure Rust、30版、2026-05 公開、yanked なし。最も成熟・活発 |
| [ps-jxl-oxide](https://crates.io/crates/ps-jxl-oxide) | 0.13.3 | 要確認 | 約635 | jxl-oxide の fork、実績が少ない |
| [malmal-jxl-oxide](https://crates.io/crates/malmal-jxl-oxide) | 0.12.6 | 要確認 | 約22 | 一時的な fork、実績が極小 |
| [rff-codec-jxl](https://crates.io/crates/rff-codec-jxl) | 0.5.0 | 要確認 | 約1,591 | 新規、実績が少ない |

一次情報: [crates.io API](https://crates.io/api/v1/crates/jxl-oxide)（バージョン、
ライセンス、ダウンロード数、公開日を確認）。

## Decision

1. 将来デコード検証を実装する場合の第一候補は `jxl-oxide` とする。
   理由: pure Rust（unsafe を含む依存の増加を抑えられる）、AGPL-3.0-or-later と
   互換のデュアルライセンス、十分なダウンロード実績と継続的なリリース。
2. fork 群は上流が活発な現時点では採用しない（保守性・実績で劣る）。
3. **現時点ではサーバー側のアップロード検証へ依存を追加しない**。現行プロトタイプは
   署名対象バイト列の SHA-256 完全性検証と 10 MiB 境界で不正データを遮断しており、
   デコード失敗時の可用性低下・Wasm サイズ増・攻撃面拡大の利益が明確でない。
   実装は本番の受入要件（対応 JXL 機能セット、サイズ上限、性能）確定後に判断する。
   `validate_upload_content` は依然シグネチャ検査のみで、完全デコード検証は未実装。
4. 実装する場合は `default-features = false` 等で依存を最小化し、
   `cargo deny check licenses` / `advisories` を再実行する。

## 実装状況（2026-11-18）

- クライアントは `client/Cargo.toml` で `jxl-oxide = "0.12"` を利用し、
  `client/src/jxl.rs` の `decode_jxl_to_canvas` でアップロード済み JXL を
  デコードして Canvas へ描画する（8000x8000 / 64M ピクセル上限付き）。
- サーバー側の `validate_upload_content` は JXL シグネチャ検査のみのままで、
  完全デコード検証は未実装である。本 ADR の第一候補選定は将来のサーバー側
  実装判断に引き継がれる。
- `default-features = false` 等による依存最小化と `cargo deny check licenses` /
  `advisories` の再実行は、サーバー側へ実装する時点で改めて行う。

## Consequences

- 現行はシグネチャ一致でもデコード不能な JXL を受理し得る（既知の制約）。
  クライアントで描画できることはサーバー側の完全デコード検証を意味しない。
- 選定根拠を記録したため、将来の実装判断で調査をやり直す必要がない。
- 上流の活動が止まった場合は fork 群を再評価する。

## Links

- [DETAILED_DESIGN.md](../DETAILED_DESIGN.md)
- [TASK_INVENTORY.md](../TASK_INVENTORY.md)
