---
type: Todo
title: "openIllustration プロジェクトTODO"
description: "openIllustration 固有のタスクを管理するプロジェクト内 TODO。複数プロジェクト横断のタスクはグローバルTODOへ置く。"
tags:
  - todo
  - openillustration
  - tasks
generated:
  by: dsh (lead)
  at: "2026-11-18T18:14:23+09:00"
status: current
---

## 進行中

- [x] プロジェクト全体の6パス・クロスレビュー（統合完了:
  [UNKNOWN_TASKS_INVESTIGATION.md](docs/UNKNOWN_TASKS_INVESTIGATION.md) §6.3）

## 未着手

### 優先度: 高

P0（本番公開前に解消すべきセキュリティ／データ完全性）:

- [x] 年齢ゲートの OIDC バイパスを遮断（R1-17、2026-11-18 未コミット対応）: callback / refresh が
  共通ゲート `ensure_age_policy_configured` を state / Cookie の消費前に呼び、
  `TrustedAttributeRequired` と `ConfigurationRequired`（`AGE_POLICY_CONFIGURED` 未設定）を 403 にする。
  AGE-03 のサーバー側テスト1件（AGE-02 の旧ゲートは 2026-11-17 対応済み）
- [x] 年齢判定の基準日を `TZ` ローカル日付へ（2026-11-18 対応済み・未コミット、AGE-04）:
  `validate_age` が `TZ` を読み、`Asia/Tokyo`（UTC+9）/`Etc/GMT±N`（POSIX 符号反転）を解釈、
  未設定・未対応名は UTC へフォールバック。JST 深夜境界（`2003-01-02` 生まれが JST で21歳・UTC で20歳）
  を含むサーバー側テスト2件
- [ ] 信頼できる年齢属性の実入力経路（外部 OP/JPKI のクレーム仕様確定後: DEC-01）
- [x] 通報中・却下コンテンツの直接 CID 取得を 404 化（R2-09、2026-11-17 対応済み）
- [x] refresh Cookie の `Path` を `/auth` へ広げ、logout 時のファミリ失効を有効化（R1-18、2026-11-17 対応済み）
- [x] ウォレット bind を DB に記録し verify の必須条件にする（R1-24、2026-11-17 対応済み）
- [x] 本番起動時に OIDC 必須設定と `PINATA_JWT` を検査し fail-closed 化（R4-01 / R4-04、2026-11-17 対応済み）
- [x] アップロード UI の file パートへ MIME を設定（R1-09、2026-11-17 対応済み）
- [x] Tailwind をビルド時 CSS 化し、外部 CDN/フォント依存と無 SRI を解消（R1-10、2026-11-17 対応済み）

### 優先度: 中

P1（本番運用の健全性）:

- [x] 認証・アップロード・注文作成へレート制限と同時実行上限を導入（R4-02、2026-11-18 対応済み）
- [x] OIDC HTTP タイムアウト設定キーを追加（R4-03、2026-11-18 対応済み）
- [x] `/auth/login` を `ALLOW_MOCK_AUTH` でゲートし設計書と実装を整合（C2 / R1-21、2026-11-17 対応済み）
- [x] `verify_order` へ CAS とトランザクションを導入し確定後不変を担保（R1-25、2026-11-18 対応済み）
- [x] メトリクスカウンタを差分アサーション＋注入可能設計へ変更（R3-01、2026-11-18 対応済み）
- [x] `observe_metrics` を `Router` 最外側へ移動し 429 / IPv4 403 / 413 /
  CORS プリフライト OPTIONS 204 を計上（2026-11-18 対応済み・未コミット、
  MET-02 サーバー側テスト4件）
- [x] 決済 bind 署名を v2 化（order_id / chain_id / wallet_address /
  recipient_address / amount_jpy）し、注文IDのみの旧形式署名を401拒否
  （盲署名対策、2026-11-18 対応済み・未コミット、PAY-05）
- [x] 監査 CLI（`sign_audit_export`）を硬化: 鍵権限 0600/0400・親 0700、
  `DATABASE_TOKEN` 環境変数限定（`--database-token` 廃止）、出力 `create_new` 上書き拒否
  （2026-11-18 対応済み・未コミット、AUDIT-02。bin 内に9テスト追加済み）
- [x] 3CLIへ自動テスト追加と契約の明文化（2026-11-18 対応済み・未コミット）:
  `doctor` の `parse_args_from` / `check_all_with` / `should_fail` を純粋化し24テスト、
  `verify_audit_export` に `--expect-root` と `VerificationOutcome` / `verify_export` の
  純粋化で17テスト、`verify_backup` は `--database-token` を argv で拒否（
  `DATABASE_TOKEN` 環境変数のみ）＋sidecar fail-closed で17テスト。
  全て `server/src/bin/` のサーバー側 bin テストであり E2E は40件のまま（CLI-01〜03）
- [x] SSRF リダイレクト無効化と XFF/IPv4-compatible 形式の hardened 解釈
  （2026-11-18 対応済み・未コミット、SSRF 2件 + XFF 11件のサーバー側テスト、
  `Policy::none()` で 3xx は追従せずフェイルオーバー、NET-05 / NET-06）
- [x] `doctor --strict` の警告対象へ `ALLOW_IPV4_LOCALHOST` / `TRUSTED_PROXY_CIDRS` を追加
  （2026-11-18 対応済み・未コミット、11→13カテゴリ・20→24テスト、CLI-01 / CFG-01）
- [x] `/me/export` の12表読み取りを単一の読み取りトランザクション化（2026-11-18 対応済み・未コミット、
   EXPORT-09 / EXPORT-10）: `BEGIN DEFERRED` で包み、`debug_assert!(!db.is_autocommit())` と明示
   `tx.commit()` を追加。別接続の未コミット BAN 書き込み下で書き込み前の単一時点スナップショットを
   返す検証を含むサーバー側テスト2件。E2E は40件のまま
- [x] 監査タイムラインの理由マスク（2026-11-18 対応済み・未コミット、AUDIT-03）:
   `get_content_audit` が `reason` に `summarize_reason` を適用し、ハッシュ検証用の `entry_hash`
   は DB の生値を維持。DB 全文も不変。サーバー側テスト1件
- [x] 投票署名を v2 化（2026-11-18 対応済み・未コミット、MOD-12）:
   `moderation_vote_message_v2` が `domain: "moderation-vote-v2"` / `report_id` / `result` /
   `juror_hash` を署名対象とし、`mod_vote` は v2 のみ受理。旧形式署名と別投票者向け署名の
   転用は401拒否。ADR-0004 の決定事項として追記、サーバー側テスト4件
- [x] 主要画面に実サーバー認証を使うスモーク E2E を1本追加（C1、2026-11-17 対応済み）

### 優先度: 低

P2（品質・保守・プロダクト拡張）:

- [x] 現行状態を謳う設計・状態ドキュメントを更新（C4 / R2-17〜R2-21、2026-11-18 対応済み）
- [x] セキュリティ勧告の受容根拠を RS256 本番署名に合わせて再評価（C6、2026-11-18 対応済み）
- [x] firefox / webkit プロジェクトを追加し Chrome 固定を解消（R3-09、2026-11-18 対応済み・オプトイン）
- [x] 設定検証 CLI（`doctor`）を追加（R5-06、2026-11-18 対応済み、`b39aba5`）
- [x] 機能提案 R5-10（小説 TOC と見出しアンカー、2026-11-18 対応済み）
- [x] 機能提案 R5-01（メタデータと検索・絞り込み、2026-11-18 対応済み）
- [x] 機能提案 R5-02（お気に入り / ブックマーク、2026-11-18 対応済み）
- [x] 機能提案 R5-09（透明性ダッシュボード、運用者限定の認証内集計、2026-11-18 対応済み）
- [x] 既存 /mod/reports の通報理由を要約化（2026-11-18 対応済み、ADR-0004 §9 に整合）
- [x] R5-04 のエクスポートのみ（2026-11-18 対応済み、削除権は法務・外部アンカー決定後）
- [x] client のエラー表示: /auth/verify 500 と /content/{cid}/audit 500 のサイレント失敗を修正（2026-11-18 対応済み）
- [x] CID 厳密検証（REVIEW_REPORT 残課題、2026-11-18 対応済み）: common::is_valid_cid で CIDv0/v1 を検証し全入力へ適用、mock CID も正規形式へ修正。
- [x] 中立化URLの件数指標（2026-11-18 対応済み）: POST /metrics/client-events で event/count のみ送信、
  openillustration_neutralized_urls_total を /metrics へ公開。URL本文・IP・UA は送らない。
- [x] 価格単位の型エイリアス固定（2026-11-18 対応済み）: AmountJpy = u64、wei と明確に区別。
- [x] 敵対的レビュー対応（2026-11-18）: CIDv0 の multihash prefix 検証、dag-cbor codec 対応、
  クライアントイベントの未知キー拒否、CORS 最外側化、API オリジンのビルド時注入。
- [x] BAN の `duration_days` を期限判定へ反映（2026-11-18 対応済み）: migration 013 で `users.ban_until` を追加し、
  `ban_user` が 1〜3650日（null=無期限）の期限を設定、`ensure_user_not_banned` が期限切れを判定上のみ解除。
  `unban` で NULL に戻し、`/me/export` の users 行にも含める（回帰テスト6件追加）。
- [x] 残指摘 R1-26: payment_orders の derivation_index 採番を書き込みロック＋トランザクション内へ移動し、
  受取先派生と INSERT を原子的化（2026-11-18 対応済み）。並行注文で連番の index と異なる受取先を
  割り当てる回帰テスト4件を追加。一意性は migration 003 の既存部分 UNIQUE インデックスで担保。
- [x] 残指摘対応（2026-11-18）: R1-23 OIDC pending 上限（1024件・429）と期限切れ回収、
  R1-14 アップロードの unwrap 連鎖を排除し失敗時に Processing 固まりを解消。
- [x] 敵対的レビュー対応（2026-11-18）: インスタンスロックの PID+starttime 検証と
  compare-and-delete、migration ファイル名・連続性の fail-closed 検証。
- [x] 敵対的レビュー対応（2026-11-18）: OIDC pending の起動時・周期 sweep、
  バックアップ時の生存ロック検査、監査スナップショットのマスク整合/版数検証。
- [x] E2E セレクタ堅牢化（2026-11-18）: alert を testid で特定、.group 依存を content-card へ変更。
  expect.poll の配列完全一致は過剰再フェッチ検出のため維持。
- [x] 投票フィードバック（2026-11-18）: 投票の成功通知・失敗エラーを表示（vote-error / vote-notice）、
  E2E 3件追加（現在の定義数は37件）。
- [x] クライアント堅牢化（2026-11-18）: TOC スラッグ O(N²) を O(1) 化（8千件で約257倍）、
  検索の250msデバウンス＋世代チェック、投票一覧のエラー表示（NO_PENDING_VOTES誤表示解消）。
  E2E 定義数は37件（現行）。
- [x] クライアント操作の堅牢化（2026-11-18）: お気に入りの二重送信防止・401案内、
  ダウンロード開始失敗のエラー表示とrevoke遅延延長。E2E 45件。
- [x] テスト強化（2026-11-18）: JPKI 証明書のエラーパス5件（mutation 3パターン検出）、
  proptest を dev-dependencies 化して long-standing unused 警告を解消＋property test 4件。
- [x] セキュリティヘッダ/同時実行の強化（2026-11-18）: CORS プリフライト含む全応答へ nosniff/DENY、
  logout 時の refresh Cookie 削除（Max-Age=0 / Path=/auth）、同時リクエスト上限の境界・動作テスト。
- [x] 運用スクリプトの硬化（2026-11-18）: cron env を制限付き KEY=VALUE パーサへ変更し
  source/eval を撤廃、所有者・symlink・権限を検証。backup-rotate は umask 077 と
  mktemp+atomic rename、deploy へ UMask=0077。
- [x] sign_audit_export の鍵権限チェックに自動テスト9件を追加（2026-11-18 対応済み）。
  0600受理/0644拒否/親0777拒否/0755警告/symlink解決/--database-token拒否/上書き拒否/新規生成。
  mutation で検出力を確認。
- [x] JWT/E2E のカバレッジと安定性（2026-11-18）: 検証鍵JSON・alg混同・kid境界など10テスト追加、
  favorites の固定sleep/`toHaveLength` と economy の opacity 待ちを修正（3回連続安定）。
- [x] 年齢 property test の実効化（2026-11-18）: 2/29・不正日・未来日・全角・TZ境界を追加、
  独立オラクルを同語反復から実判定へ変更。国別しきい値は AgePolicy 責務として現行仕様を維持。
- [x] 一覧の価格 NULL を購入不可0として返す（2026-11-18 対応済み・未コミット、CONTENT-10）:
   `COALESCE(price, 0) AS price` で NULL 行も500にならず0を返す。サーバー側テスト1件
- [x] エクスポート解析失敗の固定文言表示（2026-11-18 対応済み・未コミット、EXPORT-11）:
   `export_status_message` が解析失敗時に `エクスポート結果の解析に失敗しました。` を返し、
   「0 件」成功表示へ落とさない。クライアント側単体テスト1件
- [x] 型境界とビルドゲート（2026-11-18）: 注文のNULL価格を400へ統一、CSS側CDN参照と
  非数値nodeバージョンを拒否、trunk検証の静的回帰を追加。E2E 45件。
  ※ 本番投票は実JPKI署名が未実装のため現状401（外部要因DEC-01）。
- [x] トランザクション安全性（2026-11-18）: exportのBEGINをwrite_lock配下へ、
  期限切れ更新と固定アドレス注文INSERTをtx内へ統一（高リスクの500を解消）。
  ドロップ順序は現状(guard,tx)が安全なため維持。`verify_order` の期限 TOCTOU も修正し、
  検証完了後の CAS 前に tx 内の最新 `expires_at` で再判定して期限切れを EXPIRED＋400 にした。
- [x] 周期タスクの停止性と型安全（2026-11-18）: finalize/purge のロック待機を2秒に限定し
  取得後はSQLを完了まで実行、insert_payment_order を &Transaction 引数へ固定。
- [x] 起動/周期タスクの停止性（2026-11-18）: ロック待機に30秒上限を設定し、
  shutdown watch で即停止、周期finalizeはロック取得後は全件完了（誤報warnを解消）。
- [x] swarm-coding（mixed, R1〜R6, 2026-11-18）: 認証/CLI/client の10件を修正。
  JWT検証鍵2048bit、clock skew範囲+doctor、自己署名JPKI拒否、Pinata CID検証、
  auth_verifyのDB失効削除と自己デッドロック解消、DBトークンargv拒否、
  restoreのinstance lock検査、Dashboard/favorite/ContentViewerのエラー表示、cid encode。
- [x] 定数単一情報源化とエラー表示（2026-11-18）: clock skew 定数を pub use で共有、
  favorite 再試行クリアの E2E、Ledger の /transactions 失敗表示（role=alert）。
  E2E 45件。
- [x] クライアント境界の硬化（2026-11-18）: 中立化URL件数を描画ソース単位で1回に抑制、
  protocol-relative/UNC URL を拒否、ファイル選択解除時に古い File をクリア。
- [x] E2E契約の監査対応（2026-11-18）: 件数矛盾(37→45)・MOD-07/10/11・Ledger/METシナリオ
  を是正、vote-notice role=status、dashboard-error/content-load-error のtestid検証、
  protocol-relative URL と再送抑制のE2Eを追加。
- [x] 入力境界のDoS対策（2026-11-18）: JPKIトークン/envelope/DER長、RSA署名(DER)長、
  Ethereum hex事前長チェック、list_reports/get_content_audit のLIMIT(500)。
- [x] 監査ログAPI顺序の是正（2026-11-18）: get_content_audit が最新500件を新しい順に返すよう変更。
  古い行はAPIから参照不可のため完全検証には署名付きエクスポート/DBを使用。
- [x] 境界テストの補完（2026-11-18）: RSA-3072/4096の署名長・DER上限非誤拒否を実測、
  JPKI envelope の certificate / verifiableCredential 両方を長さ検査。
- [ ] R5-05 / R5-08 は一次ソース確認と設計が先
  - R5-03（クリエイターページ）は migration 不要だが、owner_hash の第三者公開範囲
    （DEC-04）が未決定のため見送り。認証必須・マスキング方針の合意後に再検討。

## 完了

- [x] todo をプロジェクト内（`todo.md`）へ配置し、グローバル AGENTS に運用を明記（2026-11-17）
- [x] 6パス・クロスレビューの統合結果を P0/P1/P2 として本ファイルへ転記（2026-11-17）
- [x] R2-09: `get_content` を `status = 'Approved'` で絞り、Reported/Rejected を 404 化（2026-11-17）
- [x] 未コミット変更のドキュメント反映（2026-11-18）: メトリクス最外側化・決済 bind 署名 v2・監査CLI硬化を
  DETAILED_DESIGN / OPERATIONAL_POLICY / TEST_SPECIFICATION / TASK_INVENTORY / AUDIT_EXPORT_RUNBOOK へ記載。
  E2E 定義は40件のまま（今回の追加はサーバー側テスト5件）。git commit/push は親エージェントが実施。
- [x] 3CLIの自動テスト追加のドキュメント反映（2026-11-18）: doctor / verify_audit_export /
  verify_backup の純粋関数化・`--expect-root`・`--database-token` 拒否・sidecar fail-closed と
  各24/17/17テストを TEST_SPECIFICATION（CLI-01〜03 ゲート）・DETAILED_DESIGN・
  AUDIT_EXPORT_RUNBOOK・BACKUP_RESTORE_RUNBOOK・TASK_INVENTORY へ反映。
  E2E 定義は40件のまま（全てサーバー側 bin テスト）。frontmatter 日時は `date` 実測値へ更新。
  git commit/push は親エージェントが実施。
- [x] ネットワーク hardening のドキュメント反映（2026-11-18）: SSRF リダイレクト無効化
  （`Policy::none()`、3xx フェイルオーバー、2件）、XFF 追記型/上書き型の解釈と
  IPv4-compatible（`::a.b.c.d`）拒否（11件）、`doctor --strict` の
  `ALLOW_IPV4_LOCALHOST` / `TRUSTED_PROXY_CIDRS` 警告（13カテゴリ・24テスト）を
  DETAILED_DESIGN / OPERATIONAL_POLICY / TEST_SPECIFICATION / INFRA_NETWORK /
  TASK_INVENTORY へ記載。E2E は40件のまま。frontmatter 日時は `date` 実測値へ更新。
  git commit/push は親エージェントが実施。
- [x] 年齢ゲート共通化・TZ ローカル日付・`/me/export` 読み取りトランザクションのドキュメント反映
   （2026-11-18）: 実コードを grep して確定した値のみを DETAILED_DESIGN / AGE_VALIDATION_POLICY /
   OPERATIONAL_POLICY / TEST_SPECIFICATION / TASK_INVENTORY へ記載。新規テスト5件は
   AGE-03 / AGE-04 / EXPORT-09 / EXPORT-10 の各ゲートへ反映し、E2E 定義は40件のまま。
   frontmatter 日時は `date` 実測値へ更新。git commit/push は親エージェントが実施。
- [x] 監査マスク・投票署名 v2・価格 NULL・エクスポート解析のドキュメント反映（2026-11-18）:
   実コード（`server/src/lib.rs` / `server/src/tests.rs` / `client/src/lib.rs`）を grep して確定した
   値のみを DETAILED_DESIGN / OPERATIONAL_POLICY / TEST_SPECIFICATION / TASK_INVENTORY /
   ADR-0004 へ記載。新規テスト7件（監査マスク1件・投票 v2 4件・価格 NULL 1件・クライアント解析1件）は
   AUDIT-03 / MOD-12 / CONTENT-10 / EXPORT-11 の各ゲートへ反映し、E2E 定義は40件のまま。
   frontmatter 日時は `date` 実測値へ更新。git commit/push は親エージェントが実施。
- [x] 敵対的レビュー指摘（期限 TOCTOU・旧設計記述）の対応（2026-11-18）:
   `verify_order` が検証 I/O 完了後に tx 内の最新 `expires_at` を再判定し、期限切れなら
   EXPIRED 更新をコミットして400を返す（FINALIZED へ進ませない）。回帰テスト
   `test_payment_verify_rechecks_expiry_after_verification` を追加し、再判定削除の mutation で
   失敗することを確認（一時変更は復元・sha256 一致）。DETAILED_DESIGN / TEST_SPECIFICATION の
   旧設計の無ロック記述を現行実装（`begin_write_tx`）へ是正し、直近の5テスト
   （export lock 1・決済期限2・固定アドレス2）と新規テストを TEST_SPECIFICATION へ反映。
   TASK_INVENTORY も対応済みへ更新。E2E 件数は37件で統一（旧表記を是正）。
   frontmatter 日時は `date` 実測値へ更新。git commit/push は親エージェントが実施。

## メモ

- 2026-11-18 時点: 作業ツリーはクリーンで、以下に「未コミット」とある記述は各作業当時の履歴。
  現行状態はすべてコミット済み（最新 `dd9aa7c`）。E2E 定義数は45件（12 spec）。
- 詳細な残タスク棚卸しは `docs/TASK_INVENTORY.md`、レビュー履歴は `docs/REVIEW_REPORT.md`、
  未知タスク調査は `docs/UNKNOWN_TASKS_INVESTIGATION.md` を参照。
- 本ファイルにはクロスレビューで統合・採用した項目を追記する。
- P0 の R1-17 / R1-18 / R2-09 はコード上確定（Runtime Proof 済み）。
  他の項目はレポートの静的推論を含むため、着手時に事実確認する。
- 今回の反映は未コミットの `server/src/lib.rs` / `oidc.rs` / `tests.rs` と
  `client/src/lib.rs` を grep した確定値に基づく。
  E2E 定義数40件は `e2e/tests/*.spec.ts` の `test()` 定義数を実測して確認した。
