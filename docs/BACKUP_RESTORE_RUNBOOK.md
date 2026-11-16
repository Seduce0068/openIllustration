---
type: Runbook
title: "openIllustration DB バックアップ・リストア手順"
description: "単一ライター運用を前提とした libSQL スナップショットの取得、検証、復旧手順。"
tags: [runbook, backup, operations, database]
generated:
  by: dsh
  at: "2026-11-17T01:20:00+09:00"
---

## 目的

本番 DB（`file:local.db` 等）の矛盾のないスナップショットを取得し、
復元可能なことを確認する手順を標準化する。現行運用は単一ライターを前提とし、
複数インスタンスからの同時書き込みは行わない。

## 前提

- サーバーバイナリと同じリポジトリでビルドされた `backup_db` / `verify_backup` / `verify_audit_log` を使う。
- `DATABASE_URL` はローカル DB のパス（`file:` プレフィックス）を指す。
- バックアップ先ディレクトリは事前に存在し、出力ファイルは**存在しない**こと。
  既存ファイルは上書きしない（安全側）。

## バックアップ取得

```bash
# Kubuntu / リポジトリルート
cargo run -p server --bin backup_db -- \
  --database-url file:/path/to/local.db \
  --output /path/to/backups/openillustration-YYYYMMDD-HHMMSS.db
```

`VACUUM INTO` により一貫性のあるスナップショットを作成する。
成功時は `backup_ok: output=... tables=...` を出力する。

## バックアップ検証

```bash
# Kubuntu / リポジトリルート
cargo run -p server --bin verify_backup -- --input /path/to/backups/openillustration-YYYYMMDD-HHMMSS.db
cargo run -p server --bin verify_audit_log -- --database-url file:/path/to/backups/openillustration-YYYYMMDD-HHMMSS.db
```

`verify_backup` はファイルが開けること、テーブルと `_migrations` が存在することを確認する。
`verify_audit_log` は監査ハッシュ連鎖・`log_id` 連続性・Merkle root を検証する。
両方成功しなければバックアップを本番バックアップとして認めない。

## リストア（復旧訓練）

1. 対象サーバーを停止する（SIGTERM）。readiness `/ready` が 503 になることを確認する。
2. リストアを実行する（WAL/SHM サイドカーが残っている場合は拒否される）:

```bash
# Kubuntu / リポジトリルート
cargo run -p server --bin restore_db -- \
  --backup /path/to/backups/openillustration-YYYYMMDD-HHMMSS.db \
  --target /path/to/local.db \
  --force
```

`--force` を付けないと既存 DB の上書きを拒否する。`--force` 時は
`<target>.pre-restore-<timestamp>` に安全コピーを作成してから原子的に置換する。
バックアップの `PRAGMA integrity_check` と `_migrations` の存在を検証し、
置換後にも再検証する。
3. サーバーを起動し、マイグレーション検証（`_migrations` チェックサム）が通ることを確認する。
4. `/health` 200、`/ready` 200 を確認する。
5. `verify_audit_log` を本番 DB に対して実行し、監査連鎖が健全であることを確認する。
6. 必要なコンテンツ・ユーザー・決済注文が存在することを抽样確認する。

## 検証済み事項（2026-11-17）

実 DB で以下を確認済み:

- `backup_db` がスナップショットを作成し、10 テーブル・5 マイグレーションを保持。
- 既存ファイルへの上書き試行が失敗（終了コード 1）。
- バックアップからユーザー・コンテンツ・`_migrations` が復元できる。
- `verify_backup` が正常ファイルで成功し、存在しないファイルで失敗。
- 復元 DB に対して `verify_audit_log` が成功。
- `restore_db` が既存 DB の上書きを `--force` 無しで拒否、WAL サイドカー存在時に拒否、`--force` で安全コピー作成＋原子的置換＋データ保持を確認。

## 未解決事項

- 複数インスタンス運用時の排他（現状は単一ライター前提）。
- 暗号化、オフサイト保存、保持期間、削除方針。
- 自動バックアップスケジュールとアラート。
- アプリケーションレベルの完全なリストア試験（アップロード済み IPFS コンテンツの再取得を含む）。
