---
type: Runbook
title: "監査エクスポートと署名検証手順"
description: "監査ログの署名付きスナップショット作成と公開鍵による検証手順。"
tags: [runbook, audit, verification, operations]
generated:
  by: dsh
  at: "2026-11-17T01:30:00+09:00"
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

出力には `version`, `entry_count`, `merkle_root`, `signature` が含まれる。

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

- 署名付きスナップショットを作成し、公開鍵で検証成功。
- `entry_count` を改ざんしたスナップショットが検証失敗（終了コード 1）。
- common の単体テストで署名・検証・改ざん検出を確認。

## 未解決事項

- 外部アンカー（公開台帳／追記専用媒体）への root 公開。
- 署名鍵の HSM 管理、ローテーション、旧公開鍵の配布方針。
- 公開範囲と個人情報（理由_TEXT 等）のマスキング方針。
- スナップショットの自動生成頻度と公開手順。
