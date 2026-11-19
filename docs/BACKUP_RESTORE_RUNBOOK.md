---
type: Runbook
title: "openIllustration DB バックアップ・リストア手順"
description: "単一ライター運用を前提とした libSQL スナップショットの取得、検証、復旧手順。"
tags: [runbook, backup, operations, database]
generated:
  by: dsh
  at: "2026-11-20T06:14:49+09:00"
---

## 目的

本番 DB（`file:local.db` 等）の矛盾のないスナップショットを取得し、
復元可能なことを確認する手順を標準化する。現行運用は単一ライターを前提とし、
複数インスタンスからの同時書き込みは行わない。

## 前提

- サーバーバイナリと同じリポジトリでビルドされた `backup_db` / `verify_backup` / `verify_audit_log` を使う。
- `DATABASE_URL` はローカル DB のパス（`file:` プレフィックス）を指す。
- リモート DB のトークンは `DATABASE_TOKEN` 環境変数からのみ渡す。`verify_backup` は
  `--database-token` を argv で受け付けず、`sign_audit_export` と同じ契約で拒否する
  （下記「バックアップ検証」を参照）。
- バックアップ先ディレクトリは事前に存在し、出力ファイルは**存在しない**こと。
  既存ファイルは上書きしない（安全側）。
- バックアップ取得時はサーバーが停止していること。生存 `instance.lock` を検出した
  `backup_db` は `VACUUM INTO` を実行せず拒否する（下記「サーバー稼働中のロック拒否」を参照）。

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
古い世代の削除を一行で行う（サーバー停止中に実行する。稼働中は下記のとおり
`backup_db` がロックを検出して拒否する）:

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

導入前に `/etc/openillustration/backup.env`（所有者 `openillustration`、権限 600）へ
`BACKUP_GPG_RECIPIENT=<GPG 公開鍵のメール/ID>` を設定する。未設定なら
`backup-rotate.sh` は平文作成を拒否して失敗する（fail-closed）。systemd は
`EnvironmentFile=-`、cron は `scripts/backup-cron-wrapper.sh` 経由でこのファイルを読む。

`MAILTO` / `SyslogIdentifier` を実際の通知先へ変更してアラートを受け取る。
パス・ユーザー・グループは導入環境に合わせて修正する。

`RETENTION_DAYS`（既定30、上限36500＝100年）を超えた世代数を削除するが、直近
`MIN_KEEP`（既定3、最小1、上限1000）件は保持期間を超えていても残す。ファイル名の
UTC タイムスタンプ順に新しいものから判定し、作成したばかりのバックアップは
削除しない。

上限を超える値は起動前に拒否する（fail-closed）。比較は算術を行わず、
桁数と同桁の辞書順で行うため、巨大な整数でも安全に拒否できる。これにより
64bit 算術のオーバーフローで小さい正値へラップして検査を通過する問題
（例: `MIN_KEEP=18446744073709551617`）も防ぐ。先頭ゼロ・非整数も拒否する。
上限値そのもの（36500 / 1000）は受理し、一つ上の値（36501 / 1001）と
遥かに大きい値（100000）は拒否する境界値テストを
`scripts/test-backup-rotate.sh` に置いている。

暗号化は `BACKUP_GPG_RECIPIENT` を設定した場合に行う。未設定では平文作成を拒否し、
ローカル開発のみ `ALLOW_UNENCRYPTED_BACKUP=1` の明示オプトアウトで許可する
（オフサイト保存が設定されている場合はオプトアウトも拒否）。
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

## サーバー稼働中のロック拒否

`backup_db` は `VACUUM INTO` の前に、ソース DB（`DATABASE_URL` がローカル DB を指す場合）と
出力先 DB のそれぞれについて `<db path>.instance.lock` を検査する。生存サーバーがロックを
保持している場合は次のエラーで終了し、バックアップを作成しない（fail-closed）:

```text
refusing backup: a live server instance holds the instance lock (<lock path>, pid <pid>); stop the server or wait until startup/migration has finished
```

ロック内容は PID とプロセス開始時刻（`/proc/<pid>/stat` の field 22）で照合し、PID 再利用を防ぐ。
旧形式（PID のみ）は PID の存在だけで判定する。PID が存在しない stale ロックは続行を許可する。
`/proc` が読めない等で判定不能な場合も「生存」として扱い拒否する（fail-closed）。

`restore_db` も同じ PID + 開始時刻の判定を、バックアップ元（`backup`）と復元先（`target`）の
両方の `<db path>.instance.lock` に適用する。いずれかが生存ロックなら次のエラーで復元を
拒否する（fail-closed）:

```text
refusing restore: a live server instance holds the <side> instance lock (<lock path>, pid <pid>); stop the server or wait until startup/migration has finished
```

`<side>` は `backup` または `target`。ロックの読み取り不能・判定不能も拒否する。

対処: サーバーを停止して（SIGTERM、`/ready` が 503 になることを確認して）から
`backup_db` または `backup-rotate.sh` を再実行する。`scripts/backup-rotate.sh` は
サーバーを停止しないため、稼働中に cron / systemd から呼ぶと拒否されて失敗する。
定期実行はメンテナンス窓などサーバー停止中にスケジュールする。

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

### `verify_backup` の契約（トークン・サイドカー・出力）

使用形は `verify_backup --input PATH` である。判定は `parse_args_from` / `decide_output` /
`ensure_no_sidecars` / `verify_backup_at` に分離され、`server/src/bin/verify_backup.rs` に
17テスト（引数パース6 / 出力判定3 / サイドカー2 / 構造検証6）を追加済み。

- **トークン受け渡し**: `sign_audit_export` と同じ契約。`--database-token` は argv で
  受け付けず、`--database-token is not accepted; set DATABASE_TOKEN in the environment instead`
  で拒否する。渡されたトークン値はエラー文へエコーしない（`ps` 等で公開されるため）。
  リモート DB のトークンは `DATABASE_TOKEN` 環境変数からのみ読む。
- **サイドカー fail-closed**: `<path>-wal` / `<path>-shm` / `<path>-journal` のいずれかが
  残っていれば、DB を開く前に
  `backup has a <path> sidecar; stop the server before verifying` で拒否する。
  `restore_db` と同等の判定であり、WAL 未適用データを失わないための措置である。
- **構造検証**: 存在確認 → サイドカー検査 → SQLite オープン → `PRAGMA integrity_check` が
  `ok` → テーブル数 > 0 → `_migrations` 件数 > 0 の順に検査する。
- **出力**: 成功時は stdout へ `backup_verification: OK tables=N migrations=N` を1行出し
  終了コード0。失敗時は stdout へ何も出さず、stderr の `Error: ...` と終了コード1とする
  （エラー本文に機微情報が混じる可能性があるため）。

```bash
# Kubuntu / リポジトリルート
echo "== 1. --database-token は値を出さずに拒否されること =="
cargo run -q -p server --bin verify_backup -- \
  --input /path/to/backups/openillustration.db \
  --database-token TOP_SECRET_DATABASE_TOKEN_VALUE; echo "exit_code=$?"

echo "== 2. トークンはargvではなく環境変数から読まれること（リモートDBの場合） =="
# DATABASE_TOKEN は事前に環境変数へ設定しておく（値をargvやスクリプトへ書かない）
cargo run -q -p server --bin verify_backup -- \
  --input /path/to/backups/openillustration.db; echo "exit_code=$?"

echo "== 3. WALサイドカーが残っていれば拒否されること =="
touch /path/to/backups/openillustration.db-wal
cargo run -q -p server --bin verify_backup -- \
  --input /path/to/backups/openillustration.db; echo "exit_code=$?"
rm /path/to/backups/openillustration.db-wal
```

期待結果: 1 と 3 は終了コード1、2 は `backup_verification: OK` と終了コード0。
1 の出力に `TOP_SECRET_DATABASE_TOKEN_VALUE` は現れないこと。
自動テストの内訳とゲート ID は
[TEST_SPECIFICATION.md](TEST_SPECIFICATION.md) の CLI-03 を参照。

## リストア（復旧訓練）

1. 対象サーバーを停止する（SIGTERM）。readiness `/ready` が 503 になることを確認する。
   暗号化バックアップ（`.db.gpg`）の場合は、先に復号して平文の `.db` を用意する:

```bash
gpg --decrypt --output /path/to/restored.db /path/to/backups/openillustration-YYYYMMDD-HHMMSS.db.gpg
cargo run -p server --bin verify_backup -- --input /path/to/restored.db
```

1. リストアを実行する（WAL / SHM / journal サイドカーが残っている場合、またはバックアップ元・
   復元先のいずれかに生存 instance lock がある場合は `refusing restore: ...` で拒否される）:

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

1. サーバーを起動し、マイグレーション検証（`_migrations` チェックサム）が通ることを確認する。
1. `/health` 200、`/ready` 200 を確認する。
1. `verify_audit_log` を本番 DB に対して実行し、監査連鎖が健全であることを確認する。
   別経路で保存した Merkle root を `--expect-root` に渡すと、監査ログの欠落・
   末尾切り詰めを検出できる。`verify_backup` はテーブル数とマイグレーション台帳
   のみを検査するため、監査ログの空化はこの外部 root 照合で検出する。
1. 必要なコンテンツ・ユーザー・決済注文が存在することを抽样確認する。

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

- `backup_db` がスナップショットを作成し、13 テーブル・10 マイグレーションを保持（2026-11-17 実測）。
- 既存ファイルへの上書き試行が失敗（終了コード 1）。
- バックアップからユーザー・コンテンツ・`_migrations` が復元できる。
- `verify_backup` が正常ファイルで成功し、存在しないファイルと破損ファイルで失敗（integrity_check）。
- 復元 DB に対して `verify_audit_log` が成功。
- `restore_db` が既存 DB の上書きを `--force` 無しで拒否、WAL サイドカー存在時に拒否、`--force` で安全コピー作成＋原子的置換＋データ保持を確認。

## 検証済み事項（2026-11-18）

- `verify_backup` の判定を純粋関数 / 境界関数へ分離し、
  `server/src/bin/verify_backup.rs` に17テストを追加した。実 libsql の一時 DB で
  正常系・空 DB・台帳欠落・台帳空・不存在ファイル・非 SQLite ファイルを検証する。
- argv の `--database-token` は値をエコーせずに拒否されることを実 CLI で確認した
  （`Error: --database-token is not accepted; set DATABASE_TOKEN in the environment instead`、
  終了コード1）。トークンは `DATABASE_TOKEN` 環境変数からのみ渡す。
- サイドカーは `-wal` / `-shm` / `-journal` の3 suffix について、DB を開く前に fail-closed
  で拒否されることを検証した。上位ゲートは
  [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md) の CLI-03 を参照。

## 未解決事項

- 複数インスタンス同時起動は `<DB path>.instance.lock` で拒否する（単一ライター前提は継続）。
- スケジューラへの登録は設定例を提供済み（`deploy/`）。失敗時のアラート通知先の設定は未実施。
- アプリケーションレベルの完全なリストア試験（アップロード済み IPFS コンテンツの再取得を含む）。
