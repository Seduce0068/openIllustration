---
type: Runbook
title: "監査エクスポートと署名検証手順"
description: "監査ログの署名付きスナップショット作成と公開鍵による検証手順。"
tags: [runbook, audit, verification, operations]
generated:
  by: dsh
  at: "2026-11-22T12:32:00+09:00"
---

## 目的

`content_audit_log` のスナップショットを運用者の秘密鍵で署名し、
公開鍵を持つ第三者が DB なしで完全性を検証できるようにする。
方式は [ADR-0005](adr/ADR-0005-audit-export.md) を参照。

## 前提

- 署名鍵は PKCS#8 PEM の RSA 秘密鍵（2048 bit 以上）をファイルで渡す。
  コマンドライン引数や環境変数に秘密鍵の内容を渡さない。
- 公開鍵は SubjectPublicKeyInfo の DER 形式で配布する。
- 署名鍵ファイルは owner-only（0600 / 0400）で、親ディレクトリは 0700 で管理する。
  `sign_audit_export` は起動時にこれらを検査し、満たさない場合は拒否または警告する。
- リモート libSQL のトークンは `DATABASE_TOKEN` 環境変数でのみ渡す。`--database-token` は
  廃止され、指定するとエラーになる（`argv` は `ps` 等で公開されるため）。
- スナップショット JSON の出力先は既存ファイルであってはならない（上書き拒否）。
  CLI は `create_new` で存在チェックと生成を原子的に行う。

## 鍵ペアの準備（初期設定）

```bash
# Kubuntu / 安全管理したディレクトリで実行
mkdir -m 700 -p /secure/path
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out /secure/path/audit-signing-key.pem
chmod 600 /secure/path/audit-signing-key.pem
openssl pkey -in /secure/path/audit-signing-key.pem -pubout -outform DER -out audit-public.der
```

秘密鍵はバックアップとともに厳重に管理する。公開鍵は利用者へ配布する。

## スナップショット作成

```bash
# Kubuntu / リポジトリルート（リモート DB の場合は環境変数でトークンを渡す）
export DATABASE_TOKEN='<リモートlibSQLのトークン>'   # リモート DB を使う場合のみ。ローカル DB は不要
cargo run -p server --bin sign_audit_export -- \
  --database-url file:/path/to/local.db \
  --signing-key /secure/path/audit-signing-key.pem \
  --output /path/to/audit-snapshot-YYYYMMDD.json
```

`--database-token` は受け付けない（指定すると
`--database-token is not accepted; set DATABASE_TOKEN in the environment instead` で終了）。
`DATABASE_URL` は必須（`--database-url` または環境変数で明示、暗黙既定値なし・未設定時起動拒否）。
`DATABASE_TOKEN` の既定値は空。トークンはプロセス一覧へ
露出しないよう、必ず環境変数で渡すこと（`--database-token` は argv に載るため廃止）。

出力には `version`, `entries`（全コンテンツ監査エントリ）, `governance_entries`（BAN/UNBAN/ピン解除の運用判断）, `merkle_root`, `signature`
が含まれる（version 3、`common::AUDIT_SNAPSHOT_VERSION` で固定）。第三者は `entries` から v2 ハッシュ・連鎖・Merkle root を再計算し、
両エントリ列の内容を署名と突き合わせる。未知バージョンは署名が正しくても fail-closed で拒否される（`verify_audit_export` で実確認）。

公開用スナップショットでは `reason` / `details` は既定で `[redacted:N bytes]` にマスクされる。運用者が公開範囲を判断して本文を含める場合は `--include-reasons`
を明示する（オプトイン）。マスク後/オプトイン後の文字列が署名対象となるため、検証者は同じオプションの有無を前提に署名を検証する。

スナップショットには `reasons_masked` フラグが含まれ（署名対象）、マスク済みの場合は
`entry_hash` の再計算をスキップする。元の理由がスナップショットに無いため再計算が
一致しないためである。署名が `entry_hash` 列全体を保護し、連鎖・連番・Merkle root は
従来どおり検証される。フラグ改ざんは署名検証で検出される。既定（マスク）の
エクスポートが検証可能であることを実 DB で確認済み。

後方互換のため、`reasons_masked=false`（未マスク）の署名対象は旧形式のまま。
旧 v3 スナップショットはフラグ無しでも署名検証できる。新規のマスク版のみ
`masked` 識別子を署名対象へ含める。

## CLI の硬化（鍵権限・トークン・上書き拒否）

`sign_audit_export`（`server/src/bin/sign_audit_export.rs`）は秘密鍵と出力の取扱いを次のとおり検査する。

| 項目 | 契約 | 根拠 |
| --- | --- | --- |
| 鍵ファイル権限 | PEM を `canonicalize` して実ファイルを検査し、owner-only（`0600` / `0400`、`mode & 0o077 == 0`）を要求。満たさなければ拒否 | `ps` 等と同様にグループ/他ユーザーへ秘密鍵が読まれるリスクを防ぐ |
| 親ディレクトリ権限 | group/other 書き込み可能（`mode & 0o022 != 0`）なら拒否。`0700` でない（`mode & 0o055 != 0`）場合は警告を出力 | 鍵ファイルのすり替え・追加を防ぐ |
| リモート DB トークン | `DATABASE_TOKEN` 環境変数からのみ読む。`--database-token` は渡された時点でエラー | argv はプロセス一覧へ公開されるため |
| 出力の上書き | `OpenOptions::create_new(true)` で存在チェックと生成を原子的に行い、既存ファイルを拒否 | 存在チェックと生成の TOCTOU を防ぐ |

環境変数の読取は `common::env::read_env` に統一し、非 UTF-8 のバイト列は「未設定」や空文字と
区別して不正値として扱い、起動を拒否する。`AUDIT_EXPORT_INCLUDE_REASONS` と `ALLOW_MOCK_AUTH` は
`bool_env_state` の `Invalid` として fail-closed になり、受理値 `1` / `true` / `0` / `false`
（trim・小文字化は不変）以外と同様にエラーとなる。`DATABASE_URL` は `common::env::EnvValue` を
受け取る `server::resolve_database_url` に統一し、未設定・空・空白のみは `DatabaseUrlError::Required`
（固定文言 `DATABASE_URL is required: pass --database-url or set DATABASE_URL; no default is used`）、
非 UTF-8 は未設定へ潰さず `DatabaseUrlError::NotUtf8`（固定文言 `DATABASE_URL is not valid UTF-8`）で
拒否する。`DATABASE_TOKEN` はトークン値を含まない固定文言で拒否し、いずれも生バイトをエコーしない。

### 手動確認手順（Kubuntu / リポジトリルート）

鍵権限・トークン・上書き拒否は下記で確認できる。いずれも既存の署名鍵・DB を変更しないこと。

```bash
# 秘密鍵の緩い権限コピーは専用ディレクトリへ置き、成功・失敗いずれでも確実に削除する。
check_dir="$(mktemp -d)"
trap 'rm -rf -- "$check_dir"' EXIT
echo "== 1. 鍵が 0600 でない場合は拒否されること =="
cp /secure/path/audit-signing-key.pem "${check_dir}/loose-key.pem"
chmod 644 "${check_dir}/loose-key.pem"
cargo run -q -p server --bin sign_audit_export -- \
  --signing-key "${check_dir}/loose-key.pem" \
  --output "${check_dir}/should-not-exist.json"; echo "exit_code=$?"   # 1 (require owner-only 0600)

echo "== 2. --database-token が拒否されること =="
cargo run -q -p server --bin sign_audit_export -- \
  --signing-key /secure/path/audit-signing-key.pem \
  --database-token dummy \
  --output /tmp/should-not-exist.json; echo "exit_code=$?"   # 1 (not accepted)

echo "== 3. 既存出力の上書きが拒否されること =="
echo 'existing' > /tmp/existing-snapshot.json
cargo run -q -p server --bin sign_audit_export -- \
  --signing-key /secure/path/audit-signing-key.pem \
  --output /tmp/existing-snapshot.json; echo "exit_code=$?"   # 1 (must not already exist)
cat /tmp/existing-snapshot.json   # 'existing' のまま（上書きされていない）
```

期待結果: いずれも終了コード 1 となり、3 の出力ファイルは元の内容を保持する。
`trap` により一時ディレクトリ（緩い権限の鍵コピーを含む）は手順終了時に削除される。
正常系（鍵 0600・親 0700・新規出力・`DATABASE_TOKEN` 環境変数）は
「スナップショット作成」の手順に従って実行する。上記はコード上の契約から導いた確認手順であり、
本ドキュメント更新時点でこの手順の実行は行っていない。
`server/src/bin/sign_audit_export.rs` には鍵権限・トークン・上書き拒否の12テスト（include_reasons本番ガード2件含む）と DB 接続先必須化の5テスト（計17テスト）を追加済みであり、
継続的な検証は Runbook の本手順と
[TEST_SPECIFICATION.md](TEST_SPECIFICATION.md) の AUDIT-02 ゲートで扱う。

## 形式バージョンと互換範囲（v1 エントリ互換の限界）

監査には二つの検証経路があり、受け付ける形式が異なる。混同しないこと。

| 検証対象 | コマンド | 受け付ける形式 |
| --- | --- | --- |
| DB の `content_audit_log` | `verify_audit_log` | v2（length-prefixed）と legacy v1 のエントリハッシュ |
| 署名付きスナップショット | `verify_audit_export`（`common::verify_audit_snapshot`） | version 3 のみ |

- **エントリハッシュの v1 互換は DB 監査ログの検証に限定される。**
  `verify_audit_log` の出力 `legacy_v1=true` は、その DB 行の `entry_hash` が旧 v1
  形式の再計算値と一致したことを示すだけであり、署名付きエクスポートが v1/v2
  形式を受理するという意味ではない。
- **署名付きエクスポートの公式形式は version 3 のみ**
  （`common::AUDIT_SNAPSHOT_VERSION = 3`）。v1/v2 および未知のバージョンの
  スナップショットは、署名が正しくても fail-closed で拒否される。
  `verify_audit_export` は `unsupported audit snapshot version: N` を出力し、
  終了コード 1 を返す。
- この非対称は意図的な設計である。`common` に旧スナップショット受理の互換層は
  設けず、必要なら旧形式のデータを version 3 として再エクスポートする。

### 旧形式スナップショットの拒否確認（CLI）

version 3 のスナップショットを複製して `version` だけを 1 または 2 に書き換え、
`verify_audit_export` に渡すと、バージョン検査が署名検証より先に働いて拒否を
確認できる。

```bash
# Kubuntu / リポジトリルート
echo "== version 1 / 2 のスナップショットを準備 =="
jq '.version = 1' /path/to/audit-snapshot-YYYYMMDD.json > /tmp/audit-snapshot-v1.json
jq '.version = 2' /path/to/audit-snapshot-YYYYMMDD.json > /tmp/audit-snapshot-v2.json

echo "== version 1 を検証（拒否されること） =="
cargo run -q -p server --bin verify_audit_export -- \
  --snapshot /tmp/audit-snapshot-v1.json \
  --public-key /path/to/audit-public.der; echo "exit_code=$?"

echo "== version 2 を検証（拒否されること） =="
cargo run -q -p server --bin verify_audit_export -- \
  --snapshot /tmp/audit-snapshot-v2.json \
  --public-key /path/to/audit-public.der; echo "exit_code=$?"
```

期待結果: いずれも `Error: Invalid signature: unsupported audit snapshot version: 1`
（または `: 2`）を出力し、`exit_code=1` となる。オリジナルの version 3
スナップショットは下記「第三者検証」で `audit_export_verification: OK` となる。

## 第三者検証

```bash
# Kubuntu / リポジトリルート（公開鍵とスナップショットのみで検証可能）
cargo run -p server --bin verify_audit_export -- \
  --snapshot /path/to/audit-snapshot-YYYYMMDD.json \
  --public-key /path/to/audit-public.der
```

成功時は `audit_export_verification: OK` を出力し終了コード 0 を返す。
改ざん・誤鍵・形式不正は終了コード 1 となる。受理1行の形式は
`audit_export_verification: OK version=3 entries=N governance=N root=<hex>` である。

### 保存した Merkle root との照合（`--expect-root`）

署名検証に加え、過去に記録した Merkle root と突き合わせる場合は `--expect-root HEX` を付ける。
省略時は照合しない。比較は大小文字を区別せず（実装は `eq_ignore_ascii_case`）、
不一致なら `audit root mismatch (expected and computed roots withheld)`
を stderr へ出力し終了コード1となる。値を忘れた場合は `--expect-root requires a value`
で拒否される（いずれも実 CLI で確認）。

```bash
# Kubuntu / リポジトリルート
echo "== 1. 検証成功時に root を記録 =="
ROOT=$(cargo run -q -p server --bin verify_audit_export -- \
  --snapshot /path/to/audit-snapshot-YYYYMMDD.json \
  --public-key /path/to/audit-public.der | sed -n 's/.* root=\([0-9a-f]*\)$/\1/p')
echo "recorded root: $ROOT"

echo "== 2. 記録した root と一致する場合は受理されること =="
cargo run -p server --bin verify_audit_export -- \
  --snapshot /path/to/audit-snapshot-NEW.json \
  --public-key /path/to/audit-public.der \
  --expect-root "$ROOT"; echo "exit_code=$?"

echo "== 3. 不一致の例（拒否されること） =="
cargo run -p server --bin verify_audit_export -- \
  --snapshot /path/to/audit-snapshot-NEW.json \
  --public-key /path/to/audit-public.der \
  --expect-root deadbeef; echo "exit_code=$?"
```

期待結果: 2 は root が一致すれば `audit_export_verification: OK` と終了コード0、
3 は `Error: audit root mismatch (expected and computed roots withheld)` と終了コード1。
`--expect-root` は署名・連鎖・Merkle root の再計算検証の代替ではなく、記録済み root との
突き合わせ（切詰め・差し替え検知）を追加するオプションである。

## 検証済み事項（2026-11-17）

実 DB と OpenSSL 生成鍵で以下を確認済み:

- 署名付きスナップショットを作成し、公開鍵で検証成功（version 3、全エントリと BAN 監査を含有）。
- エントリの `reason` およびガバナンス監査の `reason` を改ざんしたスナップショットが検証失敗（終了コード 1）。
- common の単体テストで署名・検証・改ざん検出を確認。

## 検証済み事項（2026-11-18）

- `verify_audit_export` に version 1 / 2 のスナップショットを渡すと、いずれも
  `Error: Invalid signature: unsupported audit snapshot version: N`（終了コード 1）
  となり、旧形式が fail-closed で拒否されることを CLI で確認した。version 3 は
  バージョン検査を通過し署名検証へ進むことも確認した。
- `verify_audit_export` に `--expect-root` を追加し、`VerificationOutcome` /
  `verify_export` を I/O を持たない純粋関数へ分離した。`server/src/bin/verify_audit_export.rs`
  に20テスト（引数パース6 / 受理2 / 拒否5 / マスク出力3 / 一時ファイル1 / 境界・回帰3）を追加し、
  有効 v3・改ざん entry/root/署名・version 1/2 拒否・expect-root 不一致・マスク出力を検証する。
  上位ゲートは [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md) の CLI-02 を参照。

## 未解決事項

- 外部アンカー（公開台帳／追記専用媒体）への root 公開。
- 署名鍵の HSM 管理、ローテーション、旧公開鍵の配布方針。
- 公開範囲と個人情報（理由_TEXT 等）のマスキング方針。
- スナップショットの自動生成頻度と公開手順。
- AUDIT-02 の自動テストは `verify_audit_export.rs` の20件で充足済み。残るは外部アンカー運用と
  HSM 鍵管理の実試験（上記項目と重複）。
