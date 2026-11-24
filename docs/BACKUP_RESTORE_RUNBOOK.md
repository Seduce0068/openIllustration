---
type: Runbook
title: "openIllustration DB バックアップ・リストア手順"
description: "単一ライター運用を前提とした libSQL スナップショットの取得、検証、復旧手順。"
tags: [runbook, backup, operations, database]
generated:
  by: dsh
  at: "2026-11-23T02:55:00+09:00"
---

## 目的

本番 DB（`file:/var/lib/openillustration/local.db` 等の絶対パス）の矛盾のないスナップショットを取得し、
復元可能なことを確認する手順を標準化する。現行運用は単一ライターを前提とし、
複数インスタンスからの同時書き込みは行わない。

## 前提

- サーバーバイナリと同じリポジトリでビルドされた `backup_db` / `verify_backup` / `verify_audit_log` を使う。
- 本番相当（`ALLOW_MOCK_AUTH` 無効）では `DATABASE_URL` が全プロセスで必須。ローカル DB の
  絶対パス（例: `file:/var/lib/openillustration/local.db`）を指す。サーバーは未設定・非 UTF-8・
  空白のみ・旧既定の相対 `file:local.db` を起動拒否し（fail-closed、値はエコーしない）、
  開発（`ALLOW_MOCK_AUTH=1`）のみ `file:local.db` を許容する。
  `backup_db` / `verify_audit_log` は `--database-url` または環境変数での明示設定がなければ
  起動を拒否する（暗黙の `file:local.db` 既定は撤廃済み、値はエコーしない）。両者と
  `sign_audit_export` の DB 接続先解決は `server/src/lib.rs` の `resolve_database_url` へ集約し、
  環境変数は `common::env::read_env("DATABASE_URL")` の `common::env::EnvValue` で統一した。
  未設定・空・空白のみは固定文言 `DATABASE_URL is required: pass --database-url or set DATABASE_URL; no default is used`、
  非 UTF-8 は未設定へ潰さず固定文言 `DATABASE_URL is not valid UTF-8`（本番の
  `production_database_url_error` と同一）で拒否する。
- リモート DB のトークンは `DATABASE_TOKEN` 環境変数からのみ渡す。`verify_backup` は
  `--database-token` を argv で受け付けず、`sign_audit_export` と同じ契約で拒否する
  （下記「バックアップ検証」を参照）。
- バックアップ先ディレクトリは事前に存在し、出力ファイルは**存在しない**こと。
  既存ファイルは上書きしない（安全側）。
- バックアップ取得時はサーバーが停止していること。生存 `instance.lock` を検出した
  `backup_db` は `VACUUM INTO` を実行せず拒否する（下記「サーバー稼働中のロック拒否」を参照）。
  定期実行では `deploy/openillustration-backup-window.service` が停止→バックアップ→起動を
  自動オーケストレーションするため、手動停止は不要（下記「自動オーケストレーション」を参照）。
- ホスト時刻同期（NTP / chrony 等）は本番デプロイの必須前提。監査タイムスタンプ、
  トークン有効期限、バックアップ世代名（`stat -c %Y`）がホスト時刻に依存するため、
  同期状態が未確認のホストへデプロイしない（[OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- HSTS（`Strict-Transport-Security`）は TLS 終端プロキシが責任を持ち、アプリ側は付与しない。
  プロキシ側で `max-age` / `includeSubDomains` / `preload` を設定すること
  （[OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。

## 復旧目標（RPO/RTO）と署名検証ゲート

- 復旧目標は RPO=24 時間、RTO=4 時間とする
  （[ADR-0013 P2](adr/ADR-0013-deferred-decisions.md) / DEC-07）。
  日次バックアップ窓（03:17、`RandomizedDelaySec=300`）の成功を毎日確認し、
  直近成功世代からの損失が24時間以内に収まることを担保する。成功時に
  `backup-rotate.sh` が `${BACKUP_DIR}/.last-success`（UTC RFC3339、0600）を公開するため、
  mtime と内容を確認する。cron 例は 03:55 に実行し `-mmin -120` で当日の成功を
  判定する。当日成功の経過は 3〜38 分、前日成功は 1443 分となるため、閾値を
  1445 分のままにすると前日成功を「新鮮」と誤判定し当日未実行を見逃す。
  チェック時刻と閾値は必ずセットで設計し、変更時は両方を更新すること。
  systemd timer 構成では外部監視基盤から同じファイルを監視し、cron と同等の検知を提供する。
  復旧訓練では
  停止から `/health` / `/ready` 200 と `verify_audit_log` 成功までを計測し、
  4時間以内に完了することを確認する。計測条件は同一ハードウェア・
  コールドキャッシュ・ローカル libSQL とし、受入値の確定は DEC-07 の合意後に行う。
- HSTS は TLS 終端プロキシの責任である（上記「前提」に記載済み）。アプリ側は付与しないため、
  ここでは重複して設定項目を定義しない。
- 本番成果物は暗号文（`<stamp>.db.gpg`）と、同じ stamp の detached 署名
  （`<stamp>.db.gpg.asc`）、監査 root（`<stamp>.audit-root`）の3点を世代対として公開する。署名鍵は監査署名鍵と同等に管理し、
  秘密鍵の内容を CLI 引数・環境変数へ渡さない。
- 本番公開鍵の trust は `ultimately trusted`（`5`）に設定する。
  `--trust-model always` は署名検証を実質的に迂回するため使用しない。

### 本番公開鍵の trust 設定

```bash
# Kubuntu / バックアップ検証ホスト（対象公開鍵の fingerprint を明示）
gpg --list-keys --keyid-format long
gpg --edit-key <本番公開鍵の fingerprint>
# プロンプトで "trust" → "5 = ultimately trusted" → "quit" を選択
gpg --list-keys --keyid-format long
```

`gpg --list-keys` の trust 表示が `ultimate` であることを確認する。
信頼性が解決できない鍵は署名検証失敗として扱い、復元へ進まない（fail-closed）。
`--trust-model always` は使わない。

### 復元前の署名検証（必須ゲート）

復元対象の stamp を明示し、アーカイブと detached 署名の組を検証する。
検証は `gpg --decrypt` と `verify_backup` の**前**に実行し、成功した世代だけを
復元対象とする（下記「リストア（復旧訓練）」の step も参照）。

```bash
# Kubuntu / バックアップ保管ホスト（stamp を明示、ワイルドカード指定ではない）
STAMP=20261123T031700Z
PREFIX=openillustration
gpg --verify "${PREFIX}-${STAMP}.db.gpg.asc" "${PREFIX}-${STAMP}.db.gpg"
```

署名ファイル欠落・検証失敗・鍵信頼性の解決不能のいずれも復元を拒否し、
`ALERT_CMD` 設定時は通知する。検証に使った stamp と結果は運用ログへ記録する
（署名や鍵の内容はログへ出さない）。

### 暗号文・署名・監査root（3点世代対）の手動 cleanup

`backup-rotate.sh` の自動ローテーション対象は `.db` / `.db.gpg` / `.db.gpg.asc` であり、
同世代の `.audit-root` と合わせた3点を世代対として管理する。削除は stamp を明示し、
ワイルドカード一括削除を行わない。

1. 削除対象 stamp を決め、`.db.gpg` / `.db.gpg.asc` / `.audit-root` の3点が揃うことを確認する。
1. 上記の署名検証を実行し、成功した世代であることを確認する。検証失敗の世代は
   復元対象から外し、原因を記録してから運用判断で削除する（E2E 失敗アーカイブの
   7世代保持ルールはバックアップ世代には適用しない）。
1. 保持期間と最低保持件数（`RETENTION_DAYS` / `MIN_KEEP` 相当）を満たすか判断し、
   不要なら3点をまとめて削除する。
1. オフサイト 3点コピー（監査 root → 暗号文 `.db.gpg` → detached 署名 `.db.gpg.asc`、
   暗号化無効時は監査 root → 平文成果物の2点）で一部だけ残った場合は、
   その世代を不完全として固定文言で警告し、孤立ファイルを明示的に削除する。
   完全な世代対だけを再送し、一部だけの状態を完成扱いにしない（署名が欠けると
   オフサイト先で復元前の署名検証ゲートを通せない）。
1. 削除後に同じ stamp の残骸が無いことを確認し、stamp・理由・実施者を運用ログへ記録する。

```bash
# Kubuntu / バックアップ保管ホスト（対象 stamp を明示）
STAMP=20261123T031700Z
PREFIX=openillustration
ls -l "${PREFIX}-${STAMP}.db.gpg" \
      "${PREFIX}-${STAMP}.db.gpg.asc" \
      "${PREFIX}-${STAMP}.audit-root"
gpg --verify "${PREFIX}-${STAMP}.db.gpg.asc" "${PREFIX}-${STAMP}.db.gpg"
# 3点が揃い検証成功した世代だけを削除する
rm -- "${PREFIX}-${STAMP}.db.gpg" \
      "${PREFIX}-${STAMP}.db.gpg.asc" \
      "${PREFIX}-${STAMP}.audit-root"
# 残骸が無いことを確認
ls -l "${PREFIX}-${STAMP}"* 2>/dev/null || true
```

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
`VACUUM INTO` が失敗した場合は途中まで作られた出力ファイルを削除する。取得後の
検証（テーブル数 / `_migrations`）に失敗した場合も出力を削除し、壊れた
スナップショットや部分ファイルを残さない（元の失敗理由はそのまま伝わる）。

定期実行には `scripts/backup-rotate.sh` を使う。取得→`verify_backup` 検証→
`verify_audit_log` 検証→古い世代の削除を一行で行う。取得は停止済みサーバーを前提とし、
systemd 版は下記「自動オーケストレーション」が停止と起動を担う（手動で直接呼ぶ場合は
サーバーを停止してから実行する。稼働中は `backup_db` がロックを検出して拒否する）:

```bash
# Kubuntu / cron 等から実行（BIN_DIR はビルド済みバイナリのディレクトリ）
BACKUP_DIR=/var/backups/openillustration \
BIN_DIR=/opt/openIllustration/target/release \
RETENTION_DAYS=30 MIN_KEEP=3 \
AUDIT_EXPECT_ROOT=<別経路で保存した Merkle root（hex）> \
DATABASE_URL=file:/var/lib/openillustration/local.db \
  /opt/openIllustration/scripts/backup-rotate.sh
```

自動取得では、生成直後の平文 DB（mktemp の作業ディレクトリ）に対して
`verify_backup` と `verify_audit_log` の両方を実行し、両方成功した場合のみ
成果物を公開する。監査ログの検証は
`verify_audit_log --database-url file:<平文 DB>` で行い、argv にトークンを
渡さない（リモート DB のトークンは `DATABASE_TOKEN` 環境変数のみ）。
いずれかが失敗した場合は公開せず、一時平文を清掃して終了し、`ALERT_CMD`
設定時は通知する（fail-closed）。各 CLI の契約は下記「バックアップ検証」を参照。
暗号化後の復号検証は GPG 秘密鍵の保管運用に依存するため自動化対象外で、
リストア時の手動検証で担保する。

監査ログの空化・末尾切詰めを自動取得時にも検出するには、別経路で保存した
Merkle root を `AUDIT_EXPECT_ROOT` へ設定する。設定時は `backup-rotate.sh` が
`verify_audit_log --database-url file:<平文 DB> --expect-root <値>` を実行し、
外部 root との照合まで自動化する（引数ログやエラー文言へ root 値をエコーしない
既存契約は維持）。成功時は計算 root を `<prefix>-<stamp>.audit-root`（権限 0600）へ保存し、
成果物（`.db` または `.db.gpg`）と同じ stamp 世代で公開する。暗号化時も成果物と同じ
世代名を使い、root 値は stdout / stderr / `ALERT_CMD` へ出さない。
**本番起動では `AUDIT_EXPECT_ROOT` の設定が必須**で、未設定・非 UTF-8・
不正 hex は固定文言で起動拒否（fail-closed）となる。
`backup-rotate.sh` 自体は未設定時も内部整合性チェックのみを行い、外部 root 照合は行わない
（既存運用を壊さないための後方互換動作）。

### `AUDIT_EXPECT_ROOT` の段階ロールアウト

本番へ一括で設定を投入せず、次の順で段階的にゲートを効かせる。

1. **初期 root provision**: まず最初の監査エントリを1件発行してから
   `verify_audit_log --database-url <DB>` を実行し、出力された Merkle root を別経路
   （秘密管理基盤等、root は非機微な hex）へ保存する。空ログに対して `verify_audit_log` は
   `--expect-root` 未指定でも固定文言で終了コード1となる（空ログを暗黙に有効としない契約）
   ため、空の状態では root を取得できない。この時点では `AUDIT_EXPECT_ROOT` をサーバー環境へ
   設定しない（`backup-rotate.sh` は未設定でも内部整合性チェックのみで動作する従来契約を維持する）。
2. **`doctor --strict`**: 保存した root を `AUDIT_EXPECT_ROOT` へ設定し、本番相当の環境変数を
   注入して `cargo run -p server --bin doctor -- --strict` を実行する。`audit_expect_root`
   カテゴリが警告なし（hex 生値で前後空白なし）であることを確認する。未設定・非 UTF-8・
   不正 hex はここで検出され、終了コード1でゲート失敗となる。
3. **起動ゲート**: `doctor --strict` 通過後にサーバーを起動する。`run()` は `PINATA_JWT` 読込
   直後に `AUDIT_EXPECT_ROOT` を読み、共有ヘルパ `production_audit_expect_root_error` で検証
   する。本番相当（モック認証無効）で未設定・非 UTF-8・不正 hex は固定文言で起動拒否し、
   DB 接続・マイグレーション・instance.lock・FK 検査より前に失敗を確定させる。
   値や生バイトはエラー文言へエコーしない。
4. **監査ログ更新後の再取得と世代保存**: 監査ログへ新しいエントリが追加されると root が
   変わるため、取得済みバックアップに対して `verify_audit_log` を実行し、成功出力の
   `audit_chain_digest:` 行の `root=<hex>` を再取得する。`backup-rotate.sh` はこの
   計算 root を `<prefix>-<stamp>.audit-root`（0600）として成果物と同じ stamp 世代で
   公開する（暗号化時も同じ世代名）。root と成果物は世代の対として保管し、
   監査ログ追加以後の照合に使う。
5. **次回期待値への昇格は明示承認**: 保存した次世代 root を `AUDIT_EXPECT_ROOT` の
   期待値へ昇格するのは運用者の明示的な承認を要する手作業とし、自動上書きを禁止する。
   `backup-rotate.sh` は保存した root を既存期待値や `backup.env` へ書き戻さない。
   不一致が続く場合は、意図的なロールオーバーか監査ログの改ざん・欠落を切り分けてから
   承認する。

`BIN_DIR` に `backup_db` / `verify_backup` / `verify_audit_log` があればそれを使い、
無ければ `cargo run` にフォールバックする（開発用）。設定例:

- systemd（推奨）: `deploy/openillustration-backup.timer`（毎日 03:17、`Persistent=false`、
  `RandomizedDelaySec=300`）→ `deploy/openillustration-backup-window.service`
  （停止→`backup-rotate.sh`→起動）。`deploy/openillustration-backup.service` は
  オーケストレーション無しの単体実行用として残す。
- cron: `deploy/openillustration-backup.cron`（`/etc/cron.d/` に配置）。
  このジョブはラッパーのみを実行し、停止/起動は行わない（root 側スケジューラで行う）。

定期実行スケジューラは systemd timer＋window unit と cron の**二者択一**であり、
両方を有効にしないこと（二重バックアップ・停止/起動の競合を防ぐ）。
cron を選ぶ場合は `systemctl disable --now openillustration-backup.timer` で
タイマーを無効化し、systemd を選ぶ場合は cron ジョブを配置しない。

導入前に `/etc/openillustration/backup.env`（所有者 `openillustration`、権限 600）へ
`BACKUP_GPG_RECIPIENT=<GPG 公開鍵のメール/ID>` を設定する。未設定なら
`backup-rotate.sh` は平文作成を拒否して失敗する（fail-closed）。systemd は
`EnvironmentFile=-`、cron は `scripts/backup-cron-wrapper.sh` 経由でこのファイルを読む。
同じファイルへ `AUDIT_EXPECT_ROOT=<別経路保存の Merkle root>` も設定する
（root は非機微な hex だが、本番では必須）。

アラートの通知先は `ALERT_CMD`（失敗通知コマンド）または cron の `MAILTO` で設定する。
`SyslogIdentifier` は journald の検索タグであり通知先ではない（変更しても通知は飛ばない）。
cron テンプレートの `MAILTO=__SET_ME__` は配置前に実際の宛先へ差し替えること。
未差し替えのまま配置すると失敗通知が届かず、ローテーション失敗や
`AUDIT_EXPECT_ROOT` 不一致を検知できない。`ALERT_CMD` を `backup.env` で設定する場合は
cron / systemd 共通の通知経路として統一し、`MAILTO` は空にして二重通知を避ける。
パス・ユーザー・グループは導入環境に合わせて修正する。

### アラート通知の役割分担と unit 失敗通知（OnFailure）

通知経路は2つあり、役割が異なるため混同しない。

- `ALERT_CMD`（`scripts/backup-rotate.sh` が呼ぶ）= **バックアップ詳細の通知**。
  取得・検証・暗号化・オフサイト転送・ローテーションの失敗時に1引数メッセージで呼び、
  成功時は通知しない。失敗理由や世代などバックアップ固有の情報を伝える。
- `OnFailure=`（systemd が呼ぶ）= **unit 結果の通知**。
  unit が failed 状態になったときは**常に発火する**。失敗の原因はバックアップに限らず、
  アプリ unit の停止・起動や Timeout によるものも含まれる。

`OnFailure` は unit failed 時に常に発火するため、バックアップ失敗では
`ALERT_CMD` と二重通知になり得る。役割分担は「`ALERT_CMD` = バックアップ詳細、
`OnFailure` = unit 結果」とし、宛先側で重複を許容するか、どちらか一方へ集約するかを
配置時に決定する（下記「配置時の必須ステップ: alert unit」を参照）。
再起動（`ExecStopPost`）の失敗は `ALERT_CMD` では検知できないため、
`OnFailure` が自動検知の経路となる。

`RETENTION_DAYS`（既定30、上限20000＝約55年）を超えた世代数を削除するが、直近
`MIN_KEEP`（既定3、最小1、上限1000）件は保持期間を超えていても残す。直近件の選択は
ファイル名の UTC タイムスタンプ順（新しい順）で行い、残りの保持期間判定は mtime
（`stat -c %Y`）と `cutoff = 現在時刻 - RETENTION_DAYS*86400` の比較で行う。
上限は cutoff が負にならない（現在時刻の日数より小さい）範囲に制限しており、
上限値でも古い世代が実際に削除される。cutoff が非正になる値は起動時に拒否する
（fail-closed）。

上限を超える値は起動前に拒否する（fail-closed）。比較は算術を行わず、
桁数と同桁の辞書順で行うため、巨大な整数でも安全に拒否できる。これにより
64bit 算術のオーバーフローで小さい正値へラップして検査を通過する問題
（例: `MIN_KEEP=18446744073709551617`）も防ぐ。先頭ゼロ・非整数も拒否する。
上限値そのもの（20000 / 1000）は受理し、一つ上の値（20001 / 1001）と
遥かに大きい値（100000）は拒否する境界値テスト、および上限値で mtime を
1970 年代に設定した古い世代が実際に prune されるテストを
`scripts/test-backup-rotate.sh` に置いている。

暗号化は `BACKUP_GPG_RECIPIENT` を設定した場合に行う。未設定では平文作成を拒否し、
ローカル開発のみ `ALLOW_UNENCRYPTED_BACKUP=1` の明示オプトアウトで許可する
（オフサイト保存が設定されている場合はオプトアウトも拒否）。
設定時に `gpg`（または `GPG_BIN`）が無ければ失敗する（fail-closed）。平文は
暗号文の公開（`assert_publishable` と `mv`）成功後にのみ削除し、暗号化・公開のいずれが失敗しても平文を残す。
成果物は `<timestamp>.db.gpg` となる。ローテーション対象は `.db` と `.db.gpg` の両方。
復元時は `gpg --decrypt` で平文に戻してから `restore_db` を使う。
実 DB で暗号化→暗号文公開→平文削除→復号→`verify_backup` 成功を確認済み。

オフサイト保存は `OFFSITE_COPY_CMD`（実行ファイルのパス）と `OFFSITE_DEST`（宛先）
の両方を設定した場合のみ行う。`eval` は使わず、コピーコマンドは
`<成果物> <宛先>` の2引数で呼び出す（`rsync` / `scp` / `rclone` 等のラッパーを
用意して指定する）。片方だけの設定は起動前に拒否し、コピー失敗時は
スクリプト全体を失敗させる（fail-closed）。暗号化併用時は `.db.gpg` を送る。
成果物に加えて同世代の `.audit-root` と detached 署名 `.db.gpg.asc`（暗号化時）も
同じ宛先へコピーする。これら3点が完全な1世代を構成する（ADR-0013 P2）。
順序は監査 root → 暗号文 → 署名の順で、途中失敗時は固定文言で警告し、片送りを
完成扱いにしない。オフサイト先で root が欠けた場合は `--expect-root` による
欠落・末尾切詰め検知が不能になるため、コピー前に root の存在を検証し、欠落時は失敗させる。
署名が欠けた世代は復元前の `gpg --verify` ゲートを通せないため再送対象とする。
設定不整合・成功・コピー失敗（exit 7）・オフサイト root の世代対、を実確認済み。

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

この解析・判定は `common/src/instance_lock.rs` の共通ヘルパ（`parse_instance_lock` /
`read_proc_starttime` / `instance_lock_owner_alive`）に集約されている。`backup_db` / `restore_db`
は `common::instance_lock` から直接 import し、`server/src/lib.rs` は同じ関数を re-export して
起動時検査と判定を共有する（bin 内に同じ判定の再実装は置かない）。`parse_instance_lock` は
純粋関数として無条件公開し、`/proc` 依存の2関数は `#[cfg(not(target_arch = "wasm32"))]` で分離する。

`restore_db` も同じ PID + 開始時刻の判定を、バックアップ元（`backup`）と復元先（`target`）の
両方の `<db path>.instance.lock` に適用する。いずれかが生存ロックなら次のエラーで復元を
拒否する（fail-closed）:

```text
refusing restore: a live server instance holds the <side> instance lock (<lock path>, pid <pid>); stop the server or wait until startup/migration has finished
```

`<side>` は `backup` または `target`。ロックの読み取り不能・判定不能も拒否する。

対処: 定期実行では `deploy/openillustration-backup.timer` と
`deploy/openillustration-backup-window.service` による自動オーケストレーションを使う
（下記「自動オーケストレーション」を参照）。手動で再実行する場合はサーバーを停止して
（SIGTERM、`/ready` が 503 になることを確認して）から `backup_db` または
`backup-rotate.sh` を実行し、完了後にサーバーを起動して `/health` / `/ready` が 200
になることを確認する。`scripts/backup-rotate.sh` は単体ではサーバーを停止しないため、
オーケストレーション無しで稼働中に呼ぶと拒否されて失敗する。

## 自動オーケストレーション（停止→バックアップ→起動）

常時稼働環境で 03:17 にバックアップを直接実行すると生存 `instance.lock` により必ず失敗する。
systemd 版ではタイマーが窓 unit を起動し、次の順で実行する。

1. `deploy/openillustration-backup.timer` が `OnCalendar=*-*-* 03:17:00`
   （`Persistent=false`、`RandomizedDelaySec=300`）で
   `openillustration-backup-window.service` を起動する。ホスト停止中に逃した実行は補完しないため、
   必要なら手動バックアップで取得する。
1. `ExecStartPre=+/usr/bin/systemctl stop ${APP_UNIT}` でアプリ unit を停止する。
1. `ExecStart=/opt/openIllustration/scripts/backup-rotate.sh` で取得・検証・ローテーションを行う。
1. `ExecStopPost=+/usr/bin/systemctl start ${APP_UNIT}` でアプリ unit を起動する。
   `Type=oneshot` の `ExecStopPost` は `ExecStartPre` / `ExecStart` の成否にかかわらず
   実行されるため、バックアップ失敗時も起動が保証される。

`APP_UNIT` は設定可能値で、既定は `openillustration.service`。`deploy/openillustration.service`
を同梱しているため、導入環境の実 unit 名と一致していることを確認する（`systemctl edit` の drop-in で
`[Service] Environment=APP_UNIT=<実 unit 名>` を上書き）。`+` 接頭辞は systemd が
`User=` / `Group=` を無視して当該コマンドを root で実行するものであり、`sudoers` / `polkit`
での権限付与は不要。`backup-rotate.sh` 自体は `User=openillustration` で実行され、
成果物の所有者は従来どおり維持される。

```bash
echo "== 窓 unit とタイマーの状態確認 =="
systemctl status openillustration.service
systemctl status openillustration-backup-window.service
systemctl list-timers openillustration-backup.timer
```

### 配置時の必須ステップ: alert unit（OnFailure 先）

`deploy/openillustration-backup-window.service` は
`OnFailure=openillustration-backup-alert.service` を参照する。配置時に次を省略しないこと。

1. `deploy/openillustration-backup-alert.service.example` を
   `/etc/systemd/system/openillustration-backup-alert.service` へコピーする。
   `[Install]` 節は無く、`OnFailure=` から起動されるため直接 `enable` しない。
1. `ExecStart` の `__SET_ME__` を**実宛先の通知コマンド**（メール / チャット /
   Webhook 等のラッパー）へ差し替える。`backup.env` の `ALERT_CMD` を再利用する場合は
   `EnvironmentFile=-/etc/openillustration/backup.env` を追加し
   `ExecStart=${ALERT_CMD} "backup alert unit fired"` とする。
   `%N` は通知 unit 自身へ展開されるため失敗元を特定できない。失敗元を伝える場合は
   `openillustration-backup-alert@.service` テンプレート化し、各 unit の `OnFailure=` を
   `@%n.service` として `%i` から失敗元名を渡すこと。未差し替えでは通知が届かない。
1. `systemctl daemon-reload` を実行し、`systemd-analyze verify` で構文を確認する。
1. 通知確認: 通知コマンドを1回直接呼ぶか、失敗 unit を一時実行して実宛先へ
   `unit ... failed` のメッセージが届くことを確認する。あわせて
   `journalctl -u openillustration-backup-alert.service` で起動結果を確認する。
1. `ALERT_CMD` との二重通知の扱いを決定する（上記「アラート通知の役割分担と
   unit 失敗通知（OnFailure）」を参照）。同一宛先へ集約する場合は重複を許容するか
   片方を無効化し、設定根拠を記録する。

`deploy/openillustration.service`（アプリ unit テンプレート）を配置する際は
`/etc/openillustration/server.env` を所有者 `openillustration`・権限 `0600` で用意し、
同梱 migrations のパス `MIGRATIONS_DIR=/opt/openIllustration/migrations` を確認すること。
`backup.env` の `DATABASE_URL` は `server.env` と同じ絶対 `file:` パスへ同期させること
（unit/cron には既定値をハードコードしていないため、未設定なら起動前に失敗する）
（サーバー側の `MIGRATIONS_DIR` override 実装と合わせて有効化）。

- 停止待ち時間の契約: window unit の `TimeoutStartSec=1800` は自 unit の Exec にしかかからず、
  `ExecStartPre` の `systemctl stop` が待つアプリ unit の `TimeoutStopSec` は別値。
  同梱の `deploy/openillustration.service` は `TimeoutStopSec=330`（drain 上限300秒 + 予備）を
  設定済み。`GRACEFUL_DRAIN_TIMEOUT_SECS` を上限300秒まで変更した場合のみ、
  それを超える値へ drop-in で追随させること。
  停止がタイムアウトすると ExecStartPre が失敗してバックアップがスキップされるため、
  アラート対象に含めること。
- バックアップ窓の間はアプリ unit が停止しているため `/ready` は応答しない（接続拒否が正常）。
  停止確認は `systemctl is-active <APP_UNIT>` が `inactive` であることで行う。
  `/ready` が 503 を返すのは SIGTERM 直後の graceful drain 窓だけであり、窓全体ではない。
- アプリ unit は `StartLimitIntervalSec=300` / `StartLimitBurst=5` を設定している。
  設定不正で5回失敗すると `failed` / `start-limit-hit` 状態になり、値を修正しても
  ウィンドウ経過まで起動しない。復旧手順:
  `systemctl reset-failed openillustration.service` → `systemctl start openillustration.service`
  → `/health` と `/ready` の 200 を確認する。設定不正は事前に `doctor --strict` で検知する。
- 再起動後は `/health` 200 と `/ready` 200 を**手動で**確認する。
  `ExecStopPost` の `systemctl start` は起動ジョブの受付成否しか unit 結果へ反映せず、
  ヘルス確認は unit 側で行わない（`systemctl start` が 0 を返しても起動直後に
  クラッシュし得る）。"/health と /ready の 200 が窓 unit 完了の条件" という表現は
  誤りで、これらは手動確認の必須ステップである。自動検知が必要なら
  `OnFailure=` で通知 unit を実装するか、ヘルスチェックを unit の Exec へ組み込むこと。
- バックアップ失敗は `ALERT_CMD`（設定時）が通知する。さらに再起動（`ExecStopPost`）が
  失敗した場合は窓 unit の結果が失敗となり `OnFailure=` が発火する。
  `deploy/openillustration-backup-alert.service.example` を実宛先へ差し替えて配置し、
  アプリが停止したまま放置されないようにする（上記「配置時の必須ステップ」を参照）。

cron 版は `backup-cron-wrapper.sh` のみを実行し、`systemctl` の stop/start は行わない。
オーケストレーションが必要なら root 側スケジューラ（上記 systemd timer と窓 unit、
または root crontab でラッパーの前後に stop/start を置く構成）を使う。
`openillustration` ユーザーへ `systemctl` 実行権限を与える方式は採らない。

**cron 配置の必須条件**: `deploy/openillustration-backup.cron` のジョブ行は
`backup-cron-wrapper.sh` のみを実行するため、常時稼働環境では生存 `instance.lock` により
`backup_db` が拒否される。root 側スケジューラで stop→wrapper→start を併設するまで
ジョブを有効化しないこと。配置後は下記の手動1回実行で成功ログを確認してから有効化する。

```bash
echo "== cron 配置後の手動1回実行（root 側オーケストレーション併設後） =="
# cron ジョブ行と同じ環境変数でラッパーを手動実行する
/usr/bin/runuser -u openillustration -- env \
  BACKUP_DIR=/var/backups/openillustration \
  BIN_DIR=/opt/openIllustration/target/release \
  RETENTION_DAYS=30 MIN_KEEP=3 \
  DATABASE_URL=file:/var/lib/openillustration/local.db \
  /opt/openIllustration/scripts/backup-cron-wrapper.sh
# journald / メールで backup_ok: と backup_verification: OK の成功ログを確認してから有効化
```

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
別経路で保存した root を `--expect-root` に渡すと空化・末尾切り詰めを検出できる
（手動手順は下記「リストア（復旧訓練）」を参照）。
両方成功しなければバックアップを本番バックアップとして認めない。
`backup-rotate.sh` はこの2つを生成直後の平文 DB に対して自動実行し、
`AUDIT_EXPECT_ROOT` 設定時は同じ root 照合も自動で行う。失敗時は fail-closed となる
（上記「バックアップ取得」を参照）。

### `verify_backup` の契約（トークン・サイドカー・出力）

使用形は `verify_backup --input PATH` である。判定は `parse_args_from` / `decide_output` /
`ensure_no_sidecars` / `verify_backup_at` に分離され、`server/src/bin/verify_backup.rs` に
19テスト（引数パース6 / 出力判定3 / サイドカー2 / 構造検証6 / 境界・回帰2）を追加済み。

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
   **復元前の署名検証（必須ゲート）**: 対象世代の detached 署名を `gpg --verify` で検証し、
   成功した世代だけを復元対象とする（`--trust-model always` は使わない）。
   署名欠落・検証失敗・鍵信頼性の解決不能は復元を拒否する（fail-closed）。
   手順は上記「復元前の署名検証（必須ゲート）」を参照。
   暗号化成果物は `<stamp>.db.gpg` である。`.db.gpg` の場合は `gpg --decrypt` で平文へ戻し、
   復号先をシェル変数 `RESTORED_DB` に統一する。下記 step2 の `restore_db --backup` でも
   同じ `RESTORED_DB` を指定する。`verify_backup` の成功を復元の前提とし、失敗した場合は
   step2 へ進まない。下記2ブロックは同一シェルセッションで実行し `RESTORED_DB` を共有する
   （平文 `.db` を直接復元する場合は `RESTORED_DB` にその `.db` パスを設定し復号を飛ばす）:

```bash
# Kubuntu / リポジトリルート
# 暗号化成果物 <stamp>.db.gpg を復号し、復号先を RESTORED_DB に統一する
RESTORED_DB=/path/to/restored.db
gpg --decrypt --output "$RESTORED_DB" \
  /path/to/backups/openillustration-YYYYMMDD-HHMMSS.db.gpg
# verify_backup 成功が復元の前提（失敗時は復元しない）
cargo run -p server --bin verify_backup -- --input "$RESTORED_DB"
```

1. リストアを実行する。step1 の `verify_backup` 成功を確認したうえで、同じ `RESTORED_DB` を
   `--backup` に指定する（WAL / SHM / journal サイドカーが残っている場合、またはバックアップ元・
   復元先のいずれかに生存 instance lock がある場合は `refusing restore: ...` で拒否される）:

```bash
# Kubuntu / リポジトリルート（step1 と同一シェル）
# --backup には step1 の verify_backup を通した RESTORED_DB をそのまま使う
cargo run -p server --bin restore_db -- \
  --backup "$RESTORED_DB" \
  --target /path/to/local.db \
  --force
```

`--force` を付けないと既存 DB の上書きを拒否する。`--force` 時は
`<target>.pre-restore-<timestamp>` に安全コピーを作成してから原子的に置換する。
バックアップの `PRAGMA integrity_check` と `_migrations` の存在を検証し、
置換後にも再検証する。`-journal` サイドカーの拒否も実確認済み。
safety copy / staging tmp の作成が途中失敗した場合は部分ファイルを削除する。
`restore_db` は staging に対して `PRAGMA integrity_check` と `_migrations` の検証を
先に行い、成功したファイルだけを `rename` で公開する。置換後の再検証が失敗した
場合は safety copy へ自動で書き戻し（権限 `0600` を維持）、失敗理由と出所パスを
併記する。初回復元で safety copy が無い場合は、公開した target を削除して復元前の
状態（target 不存在）へ戻す。削除自体が失敗した場合は、その旨と対象パスを元の
検証失敗へ併記して報告する。
`sign_audit_export` の監査スナップショット公開も、この staging + safety copy と同じ方式で
同ディレクトリの `0600` 一時ファイルへ書き `fsync` 後に `rename` する。
書き込み・fsync・rename の失敗時は一時ファイルを削除し、既存の最終パスは上書きしない。

1. 復元 DB の外部キー孤立行を確認する。本番相当の環境変数（`DATABASE_URL` に復元先を指定し、
   `ALLOW_MOCK_AUTH` は無効）で `cargo run -p server --bin doctor -- --strict` を実行し、
   `production` カテゴリに孤立行警告が出ないことを確認する。孤立行がある構成ではサーバーが
   起動を拒否するため（fail-closed）、その DB で運用を再開しない。
1. 孤立行が検出された場合は作り直しを判断する。是正 CLI や自動修復は提供しないため、
   孤立行の無い別のバックアップを選んで再リストアするか、参照先データを作り直して
   整合する状態を再構築する。失われるデータ範囲と利用者への通知は各デプロイの運用者が
   判断する（[ADR-0010](adr/ADR-0010-foreign-key-startup-fail-closed.md)）。
1. サーバーを起動し、マイグレーション検証（`_migrations` チェックサム）が通ることを確認する。
1. `/health` 200、`/ready` 200 を確認する。
1. `verify_audit_log` を本番 DB に対して実行し、監査連鎖が健全であることを確認する。
   別経路で保存した Merkle root を `--expect-root` に渡すと、監査ログの欠落・
   末尾切り詰めを検出できる。`verify_backup` はテーブル数とマイグレーション台帳
   のみを検査するため、監査ログの空化はこの外部 root 照合で検出する。
   自動取得時は同じ照合を `AUDIT_EXPECT_ROOT` で行う（本番では設定必須、
   上記「バックアップ取得」を参照）。リストア時は復元対象世代に紐付いた
   `.audit-root`（または別経路保存の同じ世代の root）を `--expect-root` に渡す。
   現在の `AUDIT_EXPECT_ROOT` をそのまま使うと、監査ログ追加以後の別世代を復元した
   場合に不一致となり正しい復元を拒否してしまう。世代と root の対応を確認してから
   実行すること。
1. 必要なコンテンツ・ユーザー・決済注文が存在することを抽样確認する。

### safety copy の取り扱い

`--force` 時に作られる `<target>.pre-restore-<timestamp>` は**平文**の DB コピーである。
`restore_db` は `OpenOptions` の `create_new` と `mode(0o600)` により、生成時から
権限 `0600`（所有者のみ読み書き）で作成する。staging tmp も `0600` で生成し、
safety copy を target へ書き戻すロールバック時も `0600` を維持する（umask 非依存）。

- **GPG / RETENTION の対象外**: safety copy は暗号化されず、`backup-rotate.sh` の
  ローテーション対象（`.db` / `.db.gpg`）にも含まれない。平文のまま残るため、
  バックアップ先以外へコピーしたり共有したりしないこと。
- **保持期間の目安**: 7 日。復元結果の確認が終わり、以降の使用見込みがなければ
  速やかに削除する。やむを得ず長期保存する場合は保管場所とアクセス権限を
  見直し、平文であることを前提にリスクを評価する。
- **復元先の推奨権限**: 親ディレクトリは `0700` を推奨する。DB ファイル自体は
  `0600` でも、親が group/other から読めるとメタデータや一時ファイルが
  露出し得る。staging tmp は置換時に rename されるため、復元後の DB も
  `0600` が維持される。
- **削除手順**: 上記の復元確認を終えたうえで、所有者が対象パスを明示して削除する。
  ワイルドカード指定で他の世代を誤削除しないこと。

```bash
echo "== safety copy の特定と権限確認（0600 であること） =="
ls -l /path/to/local.db.pre-restore-*
stat -c '%a %n' /path/to/local.db.pre-restore-*

echo "== 不要になった safety copy の削除（対象を明示） =="
rm -- /path/to/local.db.pre-restore-<timestamp>
```

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
  `server/src/bin/verify_backup.rs` に19テストを追加した。実 libsql の一時 DB で
  正常系・空 DB・台帳欠落・台帳空・不存在ファイル・非 SQLite ファイルを検証する。
- argv の `--database-token` は値をエコーせずに拒否されることを実 CLI で確認した
  （`Error: --database-token is not accepted; set DATABASE_TOKEN in the environment instead`、
  終了コード1）。トークンは `DATABASE_TOKEN` 環境変数からのみ渡す。
- サイドカーは `-wal` / `-shm` / `-journal` の3 suffix について、DB を開く前に fail-closed
  で拒否されることを検証した。上位ゲートは
  [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md) の CLI-03 を参照。

## テスト前提条件の扱い（数値不変）

- `scripts/test-backup-rotate.sh` のケース8は実 gpg による暗号化→復号→バイト一致を検証する。
  gpg 不在・使い捨て鍵生成失敗・fingerprint 取得失敗時、ローカルは `SKIP case 8: <理由>`
  （PASS とは別の専用出力）とし、CI（`CI` 設定時）は `FAIL case 8: required precondition
  missing in CI (<理由>)` として失敗させる。CI runner には `.github/workflows/ci.yml` の
  `apt_packages` で `gpg` を導入する（`shellcheck` / `shfmt` は維持）。
  既存 `expect_failure` / `expect_success` 件数は不変。
- リストア時に IPFS コンテンツを再取得する場合、`IPFS_GATEWAYS` は本番相当
  （`ALLOW_MOCK_AUTH` 無効）では `https` のみが受理され、`http` は mock 有効時のみ受理される。
  暗号化バックアップの復元は `<stamp>.db.gpg` を `gpg --decrypt` で戻す手順を維持する。

## 未解決事項

- 複数インスタンス同時起動は `<DB path>.instance.lock` で拒否する（単一ライター前提は継続）。
- スケジューラへの登録と alert unit の雛形は提供済み（`deploy/`）。実宛先への差し替え・
  通知確認は各デプロイの配置作業であり未実施。
- アプリケーションレベルの完全なリストア試験（アップロード済み IPFS コンテンツの再取得を含む）。
