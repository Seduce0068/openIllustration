---
type: Runbook
title: "監査エクスポートと署名検証手順"
description: "監査ログの署名付きスナップショット作成と公開鍵による検証手順。"
tags: [runbook, audit, verification, operations]
generated:
  by: dsh
  at: "2026-11-18T13:40:37+09:00"
---

## 目的

`content_audit_log` のスナップショットを運用者の秘密鍵で署名し、
公開鍵を持つ第三者が DB なしで完全性を検証できるようにする。
方式は [ADR-0005](adr/ADR-0005-audit-export.md) を参照。

## 前提

- 署名鍵は PKCS#8 PEM の RSA 秘密鍵（2048 bit 以上）をファイルで渡す。
  コマンドライン引数や環境変数に秘密鍵の内容を渡さない。
- 公開鍵は SubjectPublicKeyInfo の DER 形式で配布する。
- スナップショット JSON の出力先は既存ファイルであってはならない（上書き拒否）。

## 鍵ペアの準備（初期設定）

```bash
# Kubuntu / 安全管理したディレクトリで実行
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out audit-signing-key.pem
chmod 600 audit-signing-key.pem
openssl pkey -in audit-signing-key.pem -pubout -outform DER -out audit-public.der
```

秘密鍵はバックアップとともに厳重に管理する。公開鍵は利用者へ配布する。

## スナップショット作成

```bash
# Kubuntu / リポジトリルート
cargo run -p server --bin sign_audit_export -- \
  --database-url file:/path/to/local.db \
  --signing-key /secure/path/audit-signing-key.pem \
  --output /path/to/audit-snapshot-YYYYMMDD.json
```

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
改ざん・誤鍵・形式不正は終了コード 1 となる。

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

## 未解決事項

- 外部アンカー（公開台帳／追記専用媒体）への root 公開。
- 署名鍵の HSM 管理、ローテーション、旧公開鍵の配布方針。
- 公開範囲と個人情報（理由_TEXT 等）のマスキング方針。
- スナップショットの自動生成頻度と公開手順。
