---
type: License Audit
description: 実行時依存のライセンス棚卸しと AGPL-3.0-or-later との整合を記録する。
tags: [licenses, dependencies, compliance]
---

# 依存ライセンス監査

## 1. 結論

- 本リポジトリの全クレートは `AGPL-3.0-or-later`（[LICENSE](../LICENSE)）で提供し、
  `[workspace.package].license` として Cargo メタデータにも宣言済み。
- 配布物（server バイナリ、client wasm）へ組み込まれる実行時依存（433パッケージ）は
  すべて許容的ライセンス（MIT/Apache-2.0系、BSD系、ISC、Zlib、Unicode-3.0、
  Unlicense、0BSD、BSL-1.0、CDLA-Permissive-2.0、CC0-1.0）で構成される。
- 第三者依存にコピーレフトを強制するものはない。コピーレフトは自リポジトリの
  3 クレート（`AGPL-3.0-or-later`）のみである。
- CI の `cargo deny check licenses` で検査済み。配布時のライセンス本文・
  NOTICE の同梱運用は未整備（§5）。
- 本監査は 2026-11-15 の依存更新後に再実行した。

## 2. 調査方法

実行時依存（normal edges）のみを対象とする。build-dependencies と
dev-dependencies は配布物へ組み込まれないため対象外
（`cargo metadata` の解決総数 532 のうち、実行時は server 310、
client(wasm) 234、和集合 433）。

再現コマンド:

```bash
cargo metadata --format-version 1 > /tmp/meta.json
cargo tree -p server --edges normal --prefix none | sort -u
cargo tree -p client --target wasm32-unknown-unknown \
  --edges normal --prefix none | sort -u
cargo deny check licenses
```

SPDX フィールドを持たないクレートは自リポジトリの 3 クレートのみで、
本監査と同時に宣言を追加した。

## 3. 実行時依存のライセンス分布（和集合 433）

| ライセンス式 | 件数 |
| ------------ | ---- |
| MIT OR Apache-2.0 | 191 |
| MIT | 98 |
| Apache-2.0 OR MIT | 45 |
| MIT/Apache-2.0 | 26 |
| Unicode-3.0 | 18 |
| Apache-2.0 OR ISC OR MIT | 7 |
| BSD-3-Clause | 5 |
| Zlib | 5 |
| Apache-2.0 | 4 |
| ISC | 4 |
| AGPL-3.0-or-later | 3 |
| BSD-2-Clause OR Apache-2.0 OR MIT | 3 |
| Unlicense OR MIT | 3 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 2 |
| Apache-2.0/MIT | 2 |
| CDLA-Permissive-2.0 | 2 |
| MIT AND BSD-3-Clause | 2 |
| Unlicense/MIT | 2 |
| その他（単一の複合式 11 件） | 11 |

## 4. 注意が必要な依存

現行の実行時グラフ（和集合 433）に、コピーレフトを強制する第三者依存はない。
以前の監査で挙げた `self_cell` と `r-efi` は現行グラフから外れた。
`cargo deny check licenses` は現行グラフで成功している。

## 5. 残作業

- 配布形態ごとにライセンス本文・NOTICE の同梱方法を決める
  （Apache-2.0 依存の帰属表示と、AGPL のソース提供義務の履行手段）。
- `cargo-deny` 0.20.2 を CI に導入した。`deny.toml` の許可リストで
  `cargo deny check licenses` を実行し、違反時は CI を失敗させる。
- 依存更新のたびに本監査を再実行する（現状は手動。手順は §2）。
  2026-11-15 の依存更新後に再実行済み（和集合 433）。
- 脆弱性勧告（RUSTSEC）はライセンスとは別に
  [Security advisories](SECURITY_ADVISORIES.md) で管理する。
