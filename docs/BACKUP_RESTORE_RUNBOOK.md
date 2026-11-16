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
成功時は `backup_ok: output=... tables=... migrations=...` を出力する。
取得直後にテーブル数と `_migrations` 台帳の存在を検証し、欠けていれば失敗する
（`verify_backup` はさらに `PRAGMA integrity_check` も実施する二段構え）。

定期実行には `scripts/backup-rotate.sh` を使う。取得→`verify_backup` 検証→
古い世代の削除を一行で行う（server 起動中でも実行可）:

```bash
# Kubuntu / cron 等から実行（BIN_DIR はビルド済みバイナリのディレクトリ）
BACKUP_DIR=/var/backups/openillustration \
BIN_DIR=/opt/openIllustration/target/release \
RETENTION_DAYS=30 MIN_KEEP=3 \
DATABASE_URL=file:/var/lib/openillustration/local.db \
  /opt/openIllustration/scripts/backup-rotate.sh
```

`BIN_DIR` に `backup_db` / `verify_backup` があればそれを使い、無ければ
`cargo run` にフォールバックする（開発用）。設定例:

- systemd: `deploy/openillustration-backup.service` と
  `deploy/openillustration-backup.timer`（毎日 03:17、`Persistent=true`）
- cron: `deploy/openillustration-backup.cron`（`/etc/cron.d/` に配置）

`MAILTO` / `SyslogIdentifier` を実際の通知先へ変更してアラートを受け取る。
パス・ユーザー・グループは導入環境に合わせて修正する。

`RETENTION_DAYS`（既定30）を超えた世代数を削除するが、直近 `MIN_KEEP`（既定3、
最小1）件は保持期間を超えていても残す。ファイル名の UTC タイムスタンプ順に
新しいものから判定し、作成したばかりのバックアップは削除しない。

暗号化は `BACKUP_GPG_RECIPIENT` を設定した場合のみ行う（未設定なら平文のまま）。
設定時に `gpg`（または `GPG_BIN`）が無ければ失敗する（fail-closed）。平文は
`verify_backup` 成功後かつ暗号化成功後にのみ削除し、暗号化失敗時は平文を残す。
成果物は `<timestamp>.db.gpg` となる。ローテーション対象は `.db` と `.db.gpg` の両方。
復元時は `gpg --decrypt` で平文に戻してから `restore_db` を使う。
実 DB で暗号化→平文削除→復号→`verify_backup` 成功を確認済み。

オフサイト保存は `OFFSITE_COPY_CMD`（実行ファイルのパス）と `OFFSITE_DEST`（宛先）
の両方を設定した場合のみ行う。`eval` は使わず、コピーコマンドは
`<成果物> <宛先>` の2引数で呼び出す（`rsync` / `scp` / `rclone` 等のラッパーを
用意して指定する）。片方だけの設定は起動前に拒否し、コピー失敗時は
スクリプト全体を失敗させる（fail-closed）。暗号化併用時は `.db.gpg` を送る。
設定不整合・成功・コピー失敗（exit 7）の3ケースを実確認済み。

失敗通知は `ALERT_CMD`（実行ファイルのパス）を設定すると有効になる。
`<メッセージ>` の1引数で呼び出し、通知自体が失敗しても元の終了コードを維持する。
成功時は通知しない。実行不可のパスを指定した場合は起動前に拒否する（fail-closed）。
メール・チャット・Webhook 等のラッパーを指定して導入環境の通知先へ接続する。
成功時無通知・失敗時通知・不正パス拒否の3ケースを実確認済み。
`bash -n` / `shellcheck` / `shfmt -d` に加え、実 DB で古い世代3件が削除され
新しい世代が保持されることを確認済み。

## バックアップ検証

```bash
# Kubuntu / リポジトリルート
cargo run -p server --bin verify_backup -- --input /path/to/backups/openillustration-YYYYMMDD-HHMMSS.db
cargo run -p server --bin verify_audit_log -- --database-url file:/path/to/backups/openillustration-YYYYMMDD-HHMMSS.db
```

`verify_backup` はファイルが開けること、`PRAGMA integrity_check` が `ok` であること、
テーブルと `_migrations` が存在することを確認する。破損したページを持つ
バックアップを実 DB で作り、integrity_check エラーで拒否されることを確認済み。
`verify_audit_log` は監査ハッシュ連鎖・`log_id` 連続性・Merkle root を検証する。
両方成功しなければバックアップを本番バックアップとして認めない。

## リストア（復旧訓練）

1. 対象サーバーを停止する（SIGTERM）。readiness `/ready` が 503 になることを確認する。
   暗号化バックアップ（`.db.gpg`）の場合は、先に復号して平文の `.db` を用意する:

```bash
gpg --decrypt --output /path/to/restored.db /path/to/backups/openillustration-YYYYMMDD-HHMMSS.db.gpg
cargo run -p server --bin verify_backup -- --input /path/to/restored.db
```

2. リストアを実行する（WAL / SHM / journal サイドカーが残っている場合は拒否される）:

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
置換後にも再検証する。`-journal` サイドカーの拒否も実確認済み。
3. サーバーを起動し、マイグレーション検証（`_migrations` チェックサム）が通ることを確認する。
4. `/health` 200、`/ready` 200 を確認する。
5. `verify_audit_log` を本番 DB に対して実行し、監査連鎖が健全であることを確認する。
6. 必要なコンテンツ・ユーザー・決済注文が存在することを抽样確認する。

## マイグレーション失敗時のロールバック判断

マイグレーションは前方のみ（down migration なし）。適用後にアプリが壊れた場合は、
新しいバイナリを止めて **直前のバイナリ＋直前のバックアップ** に戻す。

1. `.pre-restore-<timestamp>` の安全コピーまたは直近のバックアップを特定する。
2. どちらを復元するかを判断する:
   - **マイグレーション適用前のバックアップがある場合**: それを `restore_db` で復元し、
     以前のバイナリで起動する。マイグレーション後のデータは失われる。
   - **適用前のバックアップが無い場合**: `_migrations` の記録を確認し、
     欠落バージョンが無いか確認した上で、原因修正版のバイナリを待つ。
     安全コピーへ戻すと適用済みレコードとバイナリが不整合になる可能性がある。
3. 復元後は「リストア（復旧訓練）」の手順 3〜6 を必ず実行する。

失われるデータの範囲を利用者へ通知する判断は各デプロイの運用者が行う。
監査ログ（v2 ハッシュ連鎖）は復元したスナップショット時点まで戻るため、
以降の監査イベントは再記録されない。

## 検証済み事項（2026-11-17）

実 DB で以下を確認済み:

- `backup_db` がスナップショットを作成し、10 テーブル・5 マイグレーションを保持。
- 既存ファイルへの上書き試行が失敗（終了コード 1）。
- バックアップからユーザー・コンテンツ・`_migrations` が復元できる。
- `verify_backup` が正常ファイルで成功し、存在しないファイルと破損ファイルで失敗（integrity_check）。
- 復元 DB に対して `verify_audit_log` が成功。
- `restore_db` が既存 DB の上書きを `--force` 無しで拒否、WAL サイドカー存在時に拒否、`--force` で安全コピー作成＋原子的置換＋データ保持を確認。

## 未解決事項

- 複数インスタンス同時起動は `<DB path>.instance.lock` で拒否する（単一ライター前提は継続）。
- 暗号化、オフサイト保存、保持期間、削除方針。
- スケジューラへの登録は設定例を提供済み（`deploy/`）。失敗時のアラート通知先の設定は未実施。
- バックアップの暗号化とオフサイト保存。
- アプリケーションレベルの完全なリストア試験（アップロード済み IPFS コンテンツの再取得を含む）。
