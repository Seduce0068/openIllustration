---
title: "依存ライセンス監査"
type: License Audit
description: 実行時依存のライセンス棚卸しと AGPL-3.0-or-later との整合を記録する。
tags: [licenses, dependencies, compliance]
generated:
  by: dsh
  at: "2026-11-20T15:22:58+09:00"
verified:
  by: dsh
  at: "2026-11-20T15:22:58+09:00"
status: current
---

## 依存ライセンス監査

## 1. 結論

- 本リポジトリの全クレートは `AGPL-3.0-or-later`（[LICENSE](../LICENSE)）で提供し、
  `[workspace.package].license` として Cargo メタデータにも宣言済み。
- 配布物（server バイナリ、client wasm）へ組み込まれる実行時依存（和集合 457 パッケージ、
  2026-11-18 再計算）はすべて許容的ライセンス（MIT/Apache-2.0系、BSD系、ISC、Zlib、
  Unicode-3.0、Unlicense、0BSD、BSL-1.0、CDLA-Permissive-2.0、CC0-1.0）で構成される。
- 第三者依存にコピーレフトを強制するものはない。コピーレフトは自リポジトリの
  3 クレート（`AGPL-3.0-or-later`）のみである。
- CI の `cargo deny check licenses` と配布スクリプトの事前検査で検証済み。
  ライセンス本文・NOTICE・依存一覧の同梱は `scripts/prepare-distribution.sh` で生成する（§5）。
- 本監査は 2026-11-18 に再計算した（11/18 の `governor` / `@tailwindcss/cli` などの依存追加を反映）。
  2026-11-17 以前の値は §6 の履歴に記録し、現在値と混同しない。

数字の定義（以下はすべて 2026-11-18 再計算の現在値。混同しないこと）:

| 値 | 意味 | 出典 |
| --- | --- | --- |
| 457 | 実行時依存（server 342 / client wasm 235）の和集合 | `cargo metadata` の normal edges |
| 822 | 全依存（build/dev 含む）のライセンス式×クレート行 | `cargo deny list --format json` |
| 467 | ローカル cargo レジストリから取得できたライセンス本文ファイル数 | `prepare-distribution.sh` の成果物 |

## 2. 調査方法

実行時依存（normal edges）のみを対象とする。build-dependencies と
dev-dependencies は配布物へ組み込まれないため対象外
（`cargo metadata` の解決総数 551 のうち、実行時は server 342、
client(wasm) 235、和集合 457）。server は `x86_64-unknown-linux-gnu`、
client は `wasm32-unknown-unknown` で絞り込み、workspace メンバと
`source == null` のクレートを除外した。

再現コマンド（2026-11-18 実測）:

```bash
cargo metadata --format-version 1 --filter-platform x86_64-unknown-linux-gnu > /tmp/meta-server.json
cargo metadata --format-version 1 --filter-platform wasm32-unknown-unknown > /tmp/meta-client.json
# 値の定義は metadata の closure: resolve.nodes の normal edges
# （dep_kinds[].kind == null）を server / client ルートから辿り、
# workspace メンバと source == null を除外して和集合を数える
# 以下は参考のクロスチェック（単一クレート視点、数字は metadata 側を採用）
cargo tree -p server --edges normal --prefix none --target x86_64-unknown-linux-gnu | sort -u
cargo tree -p client --edges normal --prefix none --target wasm32-unknown-unknown | sort -u
cargo deny check licenses
cargo deny list --format json | jq -r '.licenses[] | .[0] as $l | .[1][] | [$l, .] | @tsv' | wc -l
bash scripts/prepare-distribution.sh /tmp/dist-license
```

SPDX フィールドを持たないクレートは自リポジトリの 3 クレートのみで、
本監査と同時に宣言を追加した。

## 3. 実行時依存のライセンス分布（2026-11-15 時点のスナップショット）

以下の内訳は 2026-11-15 時点の過去のスナップショットで、現在の合計（和集合 457）とは一致しない。
現在の許容ライセンス一覧は `deny.toml`、全依存のライセンス式×件数は
`cargo deny list --format json` が権威情報である。

| ライセンス式 | 件数 |
| ------------ | ---- |
| MIT OR Apache-2.0 | 191 |
| MIT | 98 |
| Apache-2.0 OR MIT | 45 |
| MIT-0 | 2 |
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

現行の実行時グラフ（和集合 457）に、コピーレフトを強制する第三者依存はない。
以前の監査で挙げた `self_cell` と `r-efi` は現行グラフから外れた。
`cargo deny check licenses` は現行グラフで成功している。

## 5. 配布物の同梱

`bash scripts/prepare-distribution.sh <出力ディレクトリ>` で以下を生成する
（新規依存なし、`cargo deny` と `cargo metadata` を利用）:

- `LICENSE`: プロジェクトの AGPL-3.0-or-later 本文
- `NOTICE`: 帰属表示と配布物のライセンス方針
- `DEPENDENCIES.tsv`: ライセンス式 → クレート名/バージョン/出典の一覧
- `THIRD_PARTY_LICENSES/`: ローカル cargo レジストリから取得できた依存ライセンス本文
  （2026-11-18 再計算で 467 件。2026-11-17 時点は 452 件で、`tracing-serde` 追加に伴う +1 だった）
- 配布スクリプトは事前に `cargo deny check licenses` を実行し、許容外ライセンスがあれば成果物を生成しない。
  NOTICE に生成時刻と依存ライセンスエントリ数（2026-11-18 現在 822）を記録する。
- 権限契約: 出力先ディレクトリは `0755`、配下の `LICENSE` / `NOTICE` /
  `DEPENDENCIES.tsv` / `THIRD_PARTY_LICENSES/*.txt` は `0644`（他者読取可）。
  これらはライセンス本文と帰属表示のみで非機微な公開物のため、`backup-rotate` の
  秘密データ（`0600`）とは権限契約が異なる。生成途中の作業ディレクトリは
  `umask 077` で `0700` を維持し、公開直前にのみ読取権限を付与する。

配布形態（バイナリ / コンテナ / ソース）ごとに、これらを同梱する手順を
リリースプロセスへ組み込む。AGPL のソース提供義務（対応ソースの提示手段）は
配布チャネル確定時に別途定める。

- `cargo-deny` 0.20.2 を CI に導入した。`deny.toml` の許可リストで
  `cargo deny check licenses` を実行し、違反時は CI を失敗させる。
- 依存更新のたびに本監査を再実行する（現状は手動。手順は §2）。
  2026-11-18 に再計算済み（和集合 457、全行 822、本文 467）。
- 脆弱性勧告（RUSTSEC）はライセンスとは別に
  [Security advisories](SECURITY_ADVISORIES.md) で管理する。

## 6. 再計算履歴（過去値との区別）

§1 の表は 2026-11-18 再計算の現在値。下表は過去の監査時点の値であり、
依存追加前の履歴として保持する（現在値ではない）。

| 再計算日 | 実行時和集合 | `cargo deny list` 全行 | ライセンス本文 | 備考 |
| --- | --- | --- | --- | --- |
| 2026-11-18 | 457（server 342 / client 235） | 822 | 467 | 現在値。`governor` / `@tailwindcss/cli` などの 11/18 依存追加を反映 |
| 2026-11-17 | 417（server 300 / client 235） | 798 | 452 | `tracing-serde` 追加を反映。未使用依存 2 件削除後 |
| 2026-11-15 | — | — | — | ライセンス式の内訳スナップショットは §3 を参照 |
