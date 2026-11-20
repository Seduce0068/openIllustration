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
- [x] **OIDC エンドポイントの https 要件を doctor 診断へ反映** — 根拠
  `server/src/bin/doctor.rs:91-100` / `server/src/oidc.rs:249-314` / 発掘 R9 / 中。
  `oidc.rs` と同様に `doctor` も `oidc_endpoint_urls` で5エンドポイントURLの https を検証
  （mock 有効時のみ localhost の http 許容、設定値は非出力）。doctor は43テスト。
- [x] **エクスポート応答の value（機微値）漏洩を精査・是正** — 根拠
  `server/src/lib.rs:4486-4511` / `client/src/lib.rs:811-818` / 発掘 R9 / 中。
  エクスポートのレスポンス値・表示値に機微情報が混ざらないことを確認し是正済み。
  `e2e/tests/real-api.spec.ts` の既知シークレット値比較をキー名へも拡張。
- [x] **価格境界 E2E の「実送信」表現をモック応答と明記へ是正** — 根拠
  `docs/TEST_SPECIFICATION.md:1020` / `e2e/tests/upload.spec.ts:177-204` / 発掘 R9 / 中。
  `/content/upload` は `page.route` + `route.fulfill` のモックであることを仕様側へ明記済み。
  E2E の test 名・定義件数は不変。
- [x] **report/vote 二重送信ガードの E2E を追加** — 根拠
  `client/src/lib.rs:2205-2237` / `e2e/tests/moderation.spec.ts:629-735` / 発掘 R9 / 中。
  実装は `claim_submission` + `BusyGuard` で対応済み。二重クリックで POST が1本だけであることを
  検証する REPORT-GUARD-01 / VOTE-GUARD-01 を追加済み（moderation.spec.ts 全14件）。

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
  ダウンロード開始失敗のエラー表示とrevoke遅延延長。E2E 56件。
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
  非数値nodeバージョンを拒否、trunk検証の静的回帰を追加。E2E 56件。
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
  E2E 56件。
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
- [x] 監査タイムラインの仕様化（2026-11-18）: APIが最新500件を新着順で返す契約を
  設計・テスト仕様へ反映し、FORENSIC_TIMELINEの表示順E2Eを追加（46件）。
- [x] ドキュメント数値の自動照合（2026-11-18）: scripts/test-doc-numbers.sh を追加し
  E2E件数・内訳・doctor check数・migration範囲を CI ゲート化。8進数解釈バグを修正。
- [x] ゲートと設定判定の整合（2026-11-18）: EXPOSE_INTERNAL_ERRORS の true 解釈、
  doc-numbers の動的内訳・8進数対策、backup-rotate の先頭ゼロ拒否、AGENTS.md のclippy整合。
- [x] テスト名の実在照合をゲート化（2026-11-18）: doc-numbers がドキュメント記載の
  Rustテスト名99件を実コードと照合。古い `test_jwt_require_kid_from_env` を検出し修正。
- [x] fail-open・境界・アクセシビリティ（2026-11-18）: Chromeバージョン抽出ガード、
  amount_jpy_from_wei境界テスト、監査パネル/ファイル選択/Moderation/upload のARIA、
  backup-rotate のディレクトリ権限 fail-closed、運用記述を是正。
- [x] wei変換と価格フォームの整合（2026-11-18）: amount_jpy_from_wei の0円をfail-closed拒否、
  アップロード価格inputへid/name/min/maxとラベル関連付けを追加。
- [x] a11y属性のE2E検証（2026-11-18）: Auditトグルのaria-expanded/dialog/Escape、
  ファイル選択ラベルのキーボード操作と価格input範囲を検証（48件）。
  aria-expandedは"true"/"false"文字列を返すよう実装を修正。
- [x] E2Eシナリオ仕様の是正（2026-11-18）: 11件の未反映を反映。MET ID衝突を
  NEUT-URL-02/03へ分離、a11y/CONTENT-ERR/favorites/export/GOV-01のオラクルを明記。
- [x] dialog/フォーカスとE2E堅牢性（2026-11-18）: aria-modal+初期フォーカス+Tab trap、
  label role=button、CLOSE/SpaceのE2E、否定アサーションの状態確定、MET poll延長。
  E2E 56件。
- [x] Audit dialog のARIA整合（2026-11-18）: 実挙動（背景操作可のnon-modal）に合わせ
  aria-modal を削除し、初期フォーカスとTab/Shift+TabトラップをE2Eで検証。
- [x] fail-closed・設定診断の実効化（2026-11-18/19）: backup find失敗伝播、CDNゲートの
  grep/findエラー、doctorの3カテゴリ死コード、OIDC_MAX_AUTH_AGE不正値起動拒否、
  restore_db必須引数、ライセンス本文コピー、未使用user_hash/allowを整理。
- [x] 本番設定診断と起動の整合（2026-11-19）: doctorへ5カテゴリ追加(14→19・34テスト)、
  OIDC起動エラーをpanicからanyhow化、EXPOSE_INTERNAL_ERRORS/ALLOWED_ORIGINS診断、
  restore_db/verify_audit_exportの--database-token拒否を統一。
- [x] CLI引数値の境界（2026-11-19）: take_valueヘルパでハイフン始まりの値を拒否し、
  unknown argument経由の秘密値エコーを防止（5 CLI）。
- [ ] R5-05 / R5-08 は一次ソース確認と設計が先
  - R5-03（クリエイターページ）は migration 不要だが、owner_hash の第三者公開範囲
    （DEC-04）が未決定のため見送り。認証必須・マスキング方針の合意後に再検討。

### 2026-11-19 全体レビューで追加（高）

- [ ] **JPKI 提示に所有証明（nonce 署名検証）を追加** — 根拠 `server/src/jpki.rs:27-45` / R3-AU01 / 重大度 critical。
  現状は証明書の公開鍵を取り出すだけで秘密鍵保持を検証せず、他人の公開証明書でなりすまし可能。
- [x] **OIDC callback に BAN チェックを追加** — 根拠 `server/src/oidc.rs:846-900` / R3-AU01 / 重大度 high。
  auth_verify/refresh は確認するが callback は欠落し、BAN 済み sub が再認証でセッション取得可能。
  2026-11-19 対応: INSERT 前に ensure_user_not_banned、mutation 検証・回帰テスト追加。
- [x] **API レスポンスへの CSP 付与** — 根拠 `server/src/lib.rs:241-247` / R1-ST01 / 重大度 high。
  2026-11-20 対応: `default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`
  を nosniff/DENY と同じ二層ミドルウェアで付与（contains_key ガード・プリフライト両対応）。
  配信側 CSP（dist/index.html のインライン script に nonce/hash が必要）は本番配信構成の設計判断のため別タスク。
  2026-11-20 再評価: サーバは HTML を配信せず（Leptos CSR は trunk serve が描画）、API 側 CSP は
  nosniff の多層化に留まり実益が限定的。配信側 CSP は dist/index.html のインライン script に
  nonce/hash が必要で本番配信構成（reverse proxy 等）の設計判断が先。段階: API 側 default-src 'none'
  等を nosniff と同じ二層へ追加 → 配信側は別タスク。
  security_headers は X-Content-Type-Options/X-Frame-Options のみ。CSP は本番構成確定時に別途設計と明記済み。
- [x] **外部キー制約を有効化** — 根拠 `server/src/lib.rs:851-858` / R2-DI01 / 重大度 high。
  libSQL は既定 foreign_keys=OFF。`PRAGMA foreign_keys=ON` と起動時 `foreign_key_check` を追加。
  2026-11-19 対応: migration前にON、孤立行はwarn、3テスト追加。
- [x] **CORS の allow_methods に DELETE を追加** — 根拠 `server/src/lib.rs:1773` / R1-AP01 / 重大度 high。
  remove_favorite は DELETE だが許可メソッドは GET/POST のみ。2026-11-19 対応済み。
- [x] **doctor --strict をデプロイ前ゲートへ組み込み（ローカル／Runbook）** — 根拠 `AGENTS.md:53` / R4-DR01 / 重大度 high。
  2026-11-20 対応: AGENTS.md / HANDOFF.md の検証コマンドと OPERATIONAL_POLICY の Runbook へ追加。
  CI の extra_gates は `ALLOW_IPV4_LOCALHOST=1` で strict が必ず失敗するため未変更（別ジョブ化が条件）。
  共通ワークフロー @v1 参照は不変。
  現状 doctor は警告のみで呼び出し経路が無い。
- [x] **backup-rotate に audit log 検証の自動統合を追加** — 根拠 `scripts/backup-rotate.sh:190-194` / R4-DR01 / 重大度 high。
  Runbook は両方成功を条件とするが自動取得は verify_backup のみだった。2026-11-20 対応済み:
  `verify_backup` 直後に生成直後の平文 tmp_db へ `verify_audit_log --database-url file:${tmp_db}` を実行し、
  失敗時は成果物を公開せず一時平文を清掃して `ALERT_CMD` で通知する（fail-closed）。
- [x] **暗号化後復号検証のテスト限定自動化** — 根拠 `scripts/test-backup-rotate.sh:637-709` / R4-DR01 / 重大度 high。
  2026-11-20 対応: ケース8で一時 GNUPGHOME の使い捨て鍵による暗号化→復号→バイト一致を検証。
  秘密鍵は一時領域にのみ存在し trap で削除、gpg 不在時はスキップ PASS。本番は鍵非保持設計を維持。
  2026-11-20 再評価: 本番は作成者が復号鍵を持たない設計を維持し、自動化はテスト内の使い捨て GPG
  鍵（mktemp GNUPGHOME + trap、リポジトリへ秘密鍵を置かない）で暗号化→復号→バイト一致を検証する
  スコープへ絞るのが妥当。gpg 未インストール環境では skip するゲートが必要。本番自動化は外部運用
  判断（鍵保管場所・復号担当者・検証タイミング）が確定するまで保留。
  復号には GPG 秘密鍵が必要だが、バックアップ作成者が復号鍵を持たない運用が安全なため自動化は対象外。
  鍵の保管場所・復号担当者・検証タイミングは外部運用側の判断待ち（未着手）。現状はリストア時の手動検証で担保。
- [x] **backup-rotate の失敗時一時ディレクトリ・平文 DB を清掃** — 根拠 `scripts/backup-rotate.sh:91-105` / R4-DR01 / 重大度 high。
  EXIT trap は rotation_list のみ削除し、平文 tmp_db が残置される。2026-11-19 対応済み。
- [x] **定足数を QUORUM_VOTES 定数へ集約** — 根拠 `server/src/lib.rs:2816,2834,3773` / R1-AR01 / 重大度 high。
  SQL と Rust で `3` が個別ハードコードされ、変更時に確定ロジックと集計が食い違う。2026-11-19 対応済み。
- [x] **リクエスト相関 ID とアクセスログを追加** — 根拠 `server/src/lib.rs:234-251` / R4-OB01 / 重大度 high。
  x-request-id 生成・伝播と method/path/status/duration ログが無く、Internal エラーの事後調査不可。
  2026-11-19 対応: tower-http trace feature、MatchedPathベースのJSONログ、3テスト。
- [x] **単一ライター・ロックの stale 回収 TOCTOU を解消** — 根拠 `server/src/lib.rs:840-882` / R2-BV01 / 重大度 high。
  2026-11-20 吟味の結果却下（対応済みとしてクローズ）: 現行実装は stale 判定後の再読込一致チェックと
  create_new の AlreadyExists 拒否を備え、同時起動では必ず片方が失敗するため排他は破れない。
  OS の flock への置換は不要（追加依存・プラットフォーム差のリスクのみ）。
- [x] **通報送信 (/mod/report) の失敗系 E2E を追加** — 根拠 `e2e/tests/moderation.spec.ts:102` / disc TQ / 重大度 high。
  HTTP 500/network abort の固定文言・サーバー本文非表示が未検証。2026-11-19 対応済み。
- [x] **ビルドゲート E2E を実バイナリ改ざんの統合テストへ** — 根拠 `e2e/tests/build-gate.spec.ts:5-8` / R5-TS01 / 重大度 high。
  現状はシェル文字列パターンのみで fail-open 回帰を実挙動で担保できていない。
  2026-11-19 対応: 検証をverify_trunk_archive.shへ抽出し実改ざんでexit1/未展開を検証（E2E54件）。
- [x] **GOV-01 要約 E2E を生 PII 入力→[redacted] 表示へ** — 根拠 `e2e/tests/moderation.spec.ts:9-14` / R5-TS01 / 重大度 high。
  却下してクローズ（2026-11-19 判断、本ファイル「メモ」の Runtime Proof と整合）。要約は
  `common::summarize_reason` がサーバー側で行い、`e2e/tests/moderation.spec.ts:6-8` のとおり
  E2E は要約済み値をモックから返す設計。生 PII を E2E 入力へ入れると DOM に露出し実装と
  矛盾するため却下（R5-TS01 / 過去の判断を維持）。担保は Rust 単体・property テスト。
- [x] **Upoad 価格境界 E2E を属性存在→実送信へ** — 根拠 `e2e/tests/upload.spec.ts:172-174` / R5-TS01 / 重大度 high。
  min/max 属性だけでは JS/API バリデーション回帰を検出できない。2026-11-19 対応済み。
- [x] **ADR-0002/0003 の「提案」残存を確定記述へ** — 根拠 `docs/adr/ADR-0002-jpki-authentication.md:35` / R5-DC01 / 重大度 high。
  Status: Accepted なのに本文・tags が proposed のまま。2026-11-19 対応済み。
- [x] **ADR/Runbook/OPERATIONAL_POLICY の矛盾を是正** — 根拠 `docs/HANDOFF.md:84`,
  `docs/BACKUP_RESTORE_RUNBOOK.md:264`, `docs/OPERATIONAL_POLICY.md:97-99` / R5-DC01 / 重大度 high。
  ADR-0004 を Proposed とする記載、暗号化・オフサイトの未解決重複、--database-token 付与例と文命題切れ。
  2026-11-19 対応済み（R4 指摘で保持期間・「のみ」表現も修正）。

### 2026-11-19 全体レビューで追加（中）

- [x] **verify_audit_export へ take_value を追加（5 CLI の取り残し）** — 根拠
  `server/src/bin/verify_audit_export.rs:33-50` / disc FG / 優先度 中。2026-11-20 対応済み。
- [x] **IPFS チェックサム不一致を 401 ではなく 502/409 へ** — 根拠 `server/src/lib.rs:2294` / R1-AP01 / 中。
  2026-11-20 対応: `AppError::BadGateway` を 502 で返し、`#[error]` と IntoResponse 本文を
  `Upstream content failed integrity verification` へ統一（409 は採用せず 502 へ集約）。
  全ゲートウェイ失敗の 500→502 統一は設計判断で範囲拡大のため見送り。
- [x] **一覧 API のページネーション共通化（段1）** — 根拠 `server/src/lib.rs:2849-2873` / R1-AP01 / 中。
  2026-11-20 対応: limit/offset 解析を `parse_pagination` へ抽出し `parse_content_query` が委譲。
  既定50・範囲1..=100・offset>=0・trim・フォールバック・レスポンスは不変。単体テスト2件追加。
  段2（/transactions・/mod/reports への省略可 limit/offset）と段3（page/per_page+メタデータ）は
  利用側の要求が確認できてから別タスク（既存の裸配列レスポンスを壊さないため）。
- [x] **エラー本文スキーマを code/message/status へ統一** — 根拠 `server/src/lib.rs:548-576` / R1-AP01 / 中。
  2026-11-20 吟味の結果却下（対応済みとしてクローズ）: AppError の全バリアントとミドルウェアの
  エラー応答は既に `{"error": message}` に統一済み。`code/message/status` への変更はクライアントの
  `body.error` 依存を壊す破壊的変更で、現状の利益を上回らない。/ready の `{"status": ...}` は
  ヘルスチェックの状態表現でエラー契約とは用途が異なる。
- [x] **note 注文期限・refresh purge の RFC3339 文字列比較をパース比較へ** — 根拠 `server/src/payments.rs:524-526` / R2-BV01 / 中。
  2026-11-20 対応: is_order_expired をパース比較＋fail-closed、purge を datetime() 正規化。
- [x] **JPKI 発行セッションへ sid を付与し logout を有効化** — 根拠 `server/src/lib.rs:2046-2051` / R3-AU01 / 中。
  2026-11-20 対応: auth_verify の Claims に sid 付与、logout を冪等化（lib.rs:2221, 2285）。
- [x] **OIDC state を UA/クッキーへバインド（ログイン CSRF）** — 根拠 `server/src/oidc.rs:353-381` / R3-AU01 / 中。
  2026-11-20 対応: double-submit Cookie を本番（`cookie_secure=true`）では
  `__Host-oidc_state` / Path=/ / Secure、mock では従来の `oidc_state` / Path=/auth で
  発行・検証（start と callback で同名・同 Path）。同一名 Cookie の重複は攻撃の兆候として
  401 で拒否。PKCE / nonce / 年齢ゲート順序 / state 検証位置は不変。
- [x] **OIDC エンドポイント URL へ https 必須化** — 根拠 `server/src/oidc.rs:249-276` / R3-AU01 / 中。
  2026-11-20 対応: `from_env_with` が全5エンドポイントを検証し https のみ受理、`ALLOW_MOCK_AUTH`
  有効時のみ localhost 系の http を許容（fail-closed・設定値はエコーしない、実装は oidc.rs:280-314）。
  R5 で R4 採用指摘を反映し、エラー文言を実判定へ整合（`test_oidc_from_env_with_endpoint_url_scheme_validation`
  の期待値も同一文言へ更新）。判定条件・mock 許容ロジックは不変。
- [x] **BAN 時に既存セッション/リフレッシュトークンを即失効** — 根拠 `server/src/lib.rs:2951-2985` / R3-AU01 / 中。
  2026-11-20 対応: コミット後に DB 閾値→refresh 消費の順で失効、閾値は単調増加（lib.rs:3207-3235）。
- [ ] **payment_orders の tx_hash=NULL 重複を部分 UNIQUE で防止** — 根拠
  `server/migrations/002_payment_orders.sql:16` / R2-DI01 / 中。
  2026-11-20 吟味の結果却下: order_id は毎回新規 UUID で同じ ppid/cid の複数注文（再購入）
  が正当。固定受取アドレスの同一 (chain, recipient, amount) 3回テストが 500 になる回帰を
  Runtime Proof で確認。DB制約ではなくアプリ側の冪等性設計を維持。
- [ ] **refresh_tokens.expires_at の時刻形式を RFC3339 へ統一** — 根拠 `server/migrations/006_refresh_tokens.sql:7` / R2-DI01 / 中。
- [x] **GRACEFUL_DRAIN/MODERATION_FINALIZE を doctor フォールバック検出へ** — 根拠 `server/src/lib.rs:1342,1354` / R1-CF01 / 中。
  2026-11-20 対応: 2カテゴリ追加（19→21 check(s)、29→36テスト）。
- [x] **ALLOWED_ORIGINS の doctor 判定を allowed_origins() 結果へ** — 根拠 `server/src/bin/doctor.rs:354-369` / R1-CF01 / 中。
  2026-11-20 対応: 実パーサ結果で判定、警告文を実態に合わせ修正、check数21不変。
- [x] **wasm リリースから console_log/panic_hook をゲート** — 根拠 `client/Cargo.toml:13-14` / R3-DE01 / 中。
  2026-11-20 対応: optional + dev-console feature、既定off、main()をcfgでゲート。
- [x] **JXL デコーダにサイズ・次元上限と fuzz を追加** — 根拠 `client/Cargo.toml:26` / R3-DE01 / 中。
  2026-11-20 対応: `dimensions_allowed` が `MAX_DIMENSION` / `MAX_PIXELS` を `saturating_mul` で検査し、
  決定論的 xorshift64* による 10,000 通りの fuzz プロパティテストを追加。R5 で R4 採用指摘を反映し、
  「独立オラクル」表現を「上限定義との整合を再確認する」へ是正、`expected` に既に含まれる
  冗長な面積 assert を削除（fuzz 本体・10,000 サンプリングは維持）。
- [x] **再利用ワークフロー参照をコミット SHA 固定へ** — 根拠 `.github/workflows/ci.yml:26` / R3-DE01 / 中。
  2026-11-20 再評価の結果「対応しない」としてクローズ: 共通指示（~/.agents/AGENTS.md:85）と
  リポジトリ AGENTS.md:55 が `rust-gates.yml@v1` タグピンを明記しており、SHA 固定は方針と矛盾。
  変更するなら提供側 Seduce0068/workflows のタグ運用設計が必要で本リポジトリのスコープ外。
- [x] **npx --yes を package-lock + npm ci へ** — 根拠 `.github/workflows/ci.yml:38` / R3-DE01 / 中。
  2026-11-20 対応: CI は `npm ci --ignore-scripts --no-audit --no-fund` の後に
  `npx markdownlint-cli2` でローカル固定版を実行。`package.json` へ
  `engines.node = ">=22"` を明記（markdownlint-cli2 0.24.0 が Node>=22 を要求）。
  `package-lock.json` は依存記述のみで差分不要（親が `npm ci` で確認）。
- [ ] **E2E の既定ブラウザを 3 プロジェクトへ** — 根拠 `e2e/ci-e2e.sh:121` / R4-CI01 / 中。
- [x] **E2E の npm ci を --ignore-scripts 統一** — 根拠 `e2e/ci-e2e.sh:211` / R4-CI01 / 中。
  2026-11-20 対応: client と同一引数（--ignore-scripts --no-audit --no-fund）。
- [x] **監視パス (/health,/ready,/metrics) を observe_metrics から除外** — 根拠 `server/src/lib.rs:315-319` / R4-OB01 / 中。
  2026-11-20 対応: GET スクレイプのみ除外し、POST /metrics などの非 GET は
  requests / errors へ計上（非 GET 計上テストを追加）。/ready の 503 も GET
  スクレイプとして除外し、readiness は /ready レスポンス自体で監視（新規メトリクスなし）。
- [x] **finalizer/purge のカウンタを /metrics へ公開** — 根拠 `server/src/lib.rs:346-392` / R4-OB01 / 中。
  2026-11-20 対応: ticks / processed / lock_busy / failures / last_success を
  HELP/TYPE 付きで公開。`test_metrics_expose_finalizer_counters` と
  `test_periodic_finalize_lock_wait_is_bounded` で差分検証（purge 3 + finalize 1）。
- [x] **起動時 IPFS 警告の生 URL を redact_url 経由へ** — 根拠 `server/src/lib.rs:1071-1074` / R4-OB01 / 中。
  2026-11-20 確認: 起動時ログは URL ではなく gateway 件数のみを出力し、
  取得時エラーは `redact_url` 経由（lib.rs:2679 / 2721 / 2737）で生 URL を残さない。
- [x] **投票・通報フォームへ二重送信ガード** — 根拠 `client/src/lib.rs:2205-2237` / disc VU / 中。
  2026-11-20 対応: `claim_submission` による同期判定と `BusyGuard` のスコープ離脱時解除を実装し、
  `disabled` は bool のまま、`aria-busy` を `"true"` / `"false"` 文字列列挙値へ整合。
  通報と投票の横断排他は意図的分離の可能性があるため今回は見送り。
- [x] **複数カードの DOM id / TOC アンカーを cid 名前空間化** — 根拠 `client/src/lib.rs:505-517,1649` / disc VU / 中。
  2026-11-20 対応: cid_scope_token + prefixed描画、audit id も cid付与、StoredValue化。
- [x] **一覧カードの監査取得をトグル時のみへ削減** — 根拠 `client/src/lib.rs:1543-1582` / disc VU / 中。
  2026-11-20 対応: IntersectionObserver は真因ではないため使わず、監査 GET を show_audit=true 初回
  遷移時のみへ移動（audit_fetched_cid フラグを await 前に設定）。本文 GET は従来どおり。
  前回の real-api 失敗は他 spec との並走が真因で既に 2 段階実行で解消済み。
  2026-11-20 吟味の結果見送り: A/B で全件並走時に real-api 4件が /auth/verify タイムアウトで
  失敗したが、単独実行では差分の有無にかかわらず成功。真の原因は他 spec との並走による
  実サーバー競合で IntersectionObserver 差分とは非因果（ci-e2e.sh で real-api 単独実行へ
  分離して解消）。遅延ロード自体は再検討候補として残す。
- [x] **アップロード失敗を HTTP ステータス別固定文言へ** — 根拠 `client/src/lib.rs:1825-1826` / disc VU / 中。
  2026-11-20 対応: `upload_error_message` の4分岐（401=再ログイン / 413=サイズ超過 /
  その他=`Failed to upload content (HTTP {status}).` / None=`Network error uploading content.`）を
  TEST_SPECIFICATION / DETAILED_DESIGN / E2E (`upload.spec.ts`) の確定文言へ整合。
- [x] **価格 parse 失敗の 0 円化を防止し dedicated エラー** — 根拠 `client/src/lib.rs:1873-1880` / disc VU / 中。
  2026-11-20: parse_price_input + valueAsNumber で対応済み（Round 7）。
  前回の親判断（R9 で再確認）: `price_range_error` はテスト内ヘルパー
  （`client/src/lib.rs:2676-2705`）のみに留め、本番の `sign_and_upload` には接続しない。
  発掘エージェントは「E2E を変更したなら本番へ接続すべき」と指摘するが、価格境界 E2E は
  0 / 1000001 を丸めずに送ってサーバー（モックが同じ境界を模す）の拒否を確認する契約であり、
  接続するとクライアント側で弾かれてサーバー拒否を確認する契約が壊れる。
  E2E 契約維持のための意図的な未接続であり、E2E 側の契約変更がない限り接続しない。
- [x] **日本語見出し slug を Unicode 対応へ** — 根拠 `client/src/lib.rs:261-273` / disc VU / 中。
  2026-11-20 対応: is_alphanumeric + to_lowercase再フィルタ（U+0130対策）。
- [x] **MET ID 衝突（仕様 NEUT-URL-02/03 vs E2E MET-02/03）を解消** — 根拠 `e2e/tests/metrics.spec.ts:206` / disc TQ / 中。
  2026-11-20 対応: E2E の test 名を `NEUT-URL-02` / `NEUT-URL-03` へ改名し、TEST_SPECIFICATION の表・シナリオ記述も改名済み名へ整合。
- [x] **オーファン ゲート ID（EXPORT-12, PAY-06〜08）を整理** — 根拠 `docs/TEST_SPECIFICATION.md:194,209` / disc TQ / 中。
  2026-11-20 対応: TEST_SPECIFICATION の実 ID（EXPORT-09/10、PAY-01/04）へ統一し、
  TASK_INVENTORY の旧 ID 記述も置換済み。
- [x] **価格境界 E2E の送信検証（モック応答）** — disc TQ / 中（R5-TS01 と同一、統合済み）。
  2026-11-20: upload.spec.ts の0/1000001/1000000送信で対応済み。`/content/upload` は
  `page.route` + `route.fulfill` によるモック応答であり、実サーバーへの送信ではない
  （R9 で「実送信」表現を是正）。境界判定は実サーバーと同じ <0 / >1000000 をモックで模す。

### 2026-11-19 全体レビューで追加（低）

- [x] **UI 文言を日本語へ統一（英語・内部トークン混在）** — 根拠 `client/src/lib.rs:69-70,1094-1097` / R5-UX01 / 低。
  2026-11-20 対応: E2E アサート文言は維持、内部トークン/進行状態10箇所を日本語化。
  aria-label を可視文言へ整合（Label in Name）、Processing 否定アサートをE2E側も更新。
- [x] **ナビゲーションへ aria-current を付与** — 根拠 `client/src/lib.rs:836-855` / R5-UX01 / 低。
- [x] **SPA ビュー切替で document.title を更新** — 根拠 `client/src/lib.rs:824` / R5-UX01 / 低。
- [x] **データ表へ caption と th scope** — 根拠 `client/src/lib.rs:1993-2001` / R5-UX01 / 低。
- [x] **canvas へ role=img/aria-label** — 根拠 `client/src/lib.rs:1599-1600` / R5-UX01 / 低。
- [x] **PRODUCT 名と (Rust) 表記を統一** — 根拠 `client/index.html:6` / R5-UX01 / 低。
- [x] **verify_backup/verify_audit_export のヘルプへ DATABASE_TOKEN 契約** — 根拠
  `server/src/bin/verify_backup.rs:12` / disc FG / 低。
- [x] **doctor --strict --help 併用を他 CLI と整合** — 根拠 `server/src/bin/doctor.rs:43-50` / disc FG / 低。
- [x] **README に運用 CLI 案内を追加** — 根拠 `README.md:34-87` / disc FG / 低。
- [x] **ENV_LOCK の読み手保護を設定注入へ** — 根拠 `server/src/tests.rs:87-108` / R5-TS01 / 低。
- [x] **定期 finalize テストの固定スリープを同期チャネルへ** — 根拠 `server/src/tests.rs:15490-15564` / R5-TS01 / 低。
- [x] **NEUT-URL-03 再送抑制 E2E の quiescence 待ち** — 根拠 `e2e/tests/metrics.spec.ts:227-252` / R5-TS01 / 低。

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

- 2026-11-21 (swarm-limited mixed 第1イテレ): dogfood/ux/ops の新観点で実ユーザー導線を発掘。
  監査失敗後再取得・LOG OUT 導線・Search 即時検索・投票中ステータス・運用記述5件を完了。
  E2E 65→66件（auth 3件）。起動10/上限40。
- 2026-11-21 完了（型・テスト先行）: JPKI所有証明の ProofOfPossession + 純粋署名検証 + 4テスト、
  E2E_PROJECTS条件付き選択関数 + 3テスト、VerifiedAgeAttribute 型と境界テスト、refresh_tokens
  RFC3339発行契約テスト、payment_orders正当複数注文の回帰固定。いずれも認証パス/DB制約へは未結線。
  起動累計14/上限40。E2E 66→69件。
- 2026-11-21 進行中（外部仕様確定後に結線する項目）: JPKI所有証明の
  PresentationEnvelope型+純粋署名検証+フィクスチャ、E2E3ブラウザの条件付き選択関数、
  refresh_tokens の RFC3339 契約テスト、payment_orders の正当な複数注文の回帰固定、
  VerifiedAgeAttribute 型とフェイルクローズ境界テスト、R5-05/R5-08 の内部データ型。
  いずれも外部仕様/設計判断が確定する前の安全な部分のみ。

- 2026-11-21 (swarm-limited 完了): 2 イテレーション・起動17/上限40で停止。残6件は外部仕様
  (DEC-01/R5-05/R5-08/JPKI所有証明)・却下(payment_orders部分UNIQUE)・既対応(refresh_tokens
  はdatetime正規化済み)・バイナリ要件(Firefox/WebKit)で、安全なローカル項目は0件。配信側CSPは
  本番配信構成の設計判断が先のため別タスクとして残す。

- 2026-11-20 (swarm-limited 第2イテレーション): テスト限定 GPG 往復検証（ケース8）と API 側 CSP
  を完了。R4 が TASK_INVENTORY の「CSP は未設計」残存を指摘し是正。起動 16 担当 / 上限 40。

- 2026-11-20 (swarm-limited): 未着手を再評価し、ページネーション段1・監査GET削減・doctor strict
  ローカルゲートを完了、SHA固定は方針矛盾でクローズ。CSP・復号検証は設計判断のため条件付きで残す。
  起動 11 担当 / 上限 40、連続イテレーション 1 / 上限 3。

- 2026-11-20: prepare-distribution成果物を公開用権限(dir755/file644)へ修正。
  umask077は生成中のみ維持し公開直前にchmod、実行ビット付きライセンスも644へ
  正規化（ケース7で検証）。backup-rotateの0600契約と分離。

- 2026-11-20: prepare-distribution.shを私有tmp生成＋atomic公開へ変更。失敗時に既存の完全な
  成果物が不変であることをケース6で検証。R4指摘（危険out_dir正規化拒否・SIGINT/TERM・
  old削除失敗時エラー・ケース4/5のオラクル弱さ）を反映。

- 2026-11-20: AGE_TARGET_REGIONSをドキュメント約束へ整合（小文字大文字化＋英字2文字検証、
  不正値はfail-closed起動拒否・値は非エコー）。既存小文字設定は大文字化して互換維持。

- 2026-11-20: doc-numbers堅牢性3件（sign_audit前方切り詰め削除・backup triple全マッチ照合・
  doc_bin_counts履歴除外のマッチ部分限定）を反映。R4のL49誤除外指摘は隣接制約復元で解消。
  破壊テスト（古いtriple・L49単独改変）でいずれもFAIL・復元PASSを確認。

- 2026-11-20: doc-numbersへbackup回帰期待数(28+12=40)とsign_audit_export10件の
  他ドキュメント(HANDOFF/TASK_INVENTORY)との照合を追加。破壊テストで expect_failure
  29件→HANDOFF/TASK_INVENTORYの両方でFAILを確認。履歴除外はマッチ部分前方30文字に限定。

- 2026-11-20: doc-numbersへE2E総数の未注記・古い件数を検出するfail-closedゲートを追加。
  MOD-07/EXPORT-10の59件残存を注記し、注記キーワード/正規表現の不整合（現在59件の誤救済、
  外側5〜6文字prefixの未走査）を是正。破壊テストで2通りともFAILを確認。

- 2026-11-20: AGE真偽値を1/true/0/falseのみへ絞りfail-open防止（yes/on/no/off拒否）、
  AGE_ENFORCEMENT_MODEの全受理値をエラー文言へ明記。cronラッパーの不正行/不正キーを
  行番号のみの固定文言へ（cronメールへの値漏洩防止）。
- 2026-11-20: doc-numbersのCLI-01群名照合を双方向化（重複・欠落・非テストfn誤受理を検出）、
  HANDOFFのゲート件数を実測679件（common66/server536/client57/restore20）・E2E65件へ更新。

- 2026-11-20: restore_db の一時ファイル削除結果を伝播し残置を通知。safety_copy=None /
  置換成功 / 削除失敗の3分岐をオラクル化。doc-numbers へ CLI-01 群テスト名の実在照合を追加
  （46名すべて doctor.rs の fn と一致、typo で fail-closed）。

- 2026-11-20 (impl-cli01-breakdown): CLI-01 群内訳テーブルの引数パースを6→8件へ是正
  （`parse_strict_with_help_requests_help` / `parse_help_with_strict_requests_help` の2件を追記）、
  合計44→46件を実 #[test] 46件へ整合。`scripts/test-doc-numbers.sh` へ群テーブル件数の
  合計照合を追加し、抽出失敗は fail-closed で FAIL とする。

- 2026-11-20 (impl-docnum-allbins): doc-numbers の bin テスト数照合を3 bin から全7 bin
  （backup_db / doctor / restore_db / sign_audit_export / verify_audit_export /
  verify_audit_log / verify_backup）へ拡張。記載なしで FAIL になるため、新規4 bin は
  TEST_SPECIFICATION へ「運用 bin のテスト数一覧」を、DETAILED_DESIGN へ CLI 件数表4行を追加。
  陳腐化件数は sign_audit_export 9→10（6箇所）・backup_db 7→12（うち生存ロック検査は7件）へ是正。
  実測 verify_backup は19件（指示書の12は backup_db との取り違え）で既存記載どおり変更なし。
  check23 / E2E65件 / migration13 / doctor46 は不変、fail-closed 維持。

- 2026-11-20: CIバージョン閾値の fail-open（"08" が8進数解釈で比較 false、2^64+1 が
  オーバーフロー）を修正。assert_major_numeric に先頭ゼロ拒否と桁数＋辞書順上限、
  比較側へ 10# 化を追加しオラクル(f)(g)(h)。E2E 65件（build-gate 15件）。

- 2026-11-20: 最終レビューで doc-numbers の照合漏れ（DETAILED_DESIGN の CLI 件数表・個別
  spec 実行例）を発見しゲートを拡張。doc numbers が green でもドキュメント間矛盾が残る問題を
  根本解消。E2E 62件（build-gate 9件）・バックアップ35件（当時、現在37件）・CLI 45/20/19 を実測値へ統一。

- 2026-11-20: エラー本文スキーマ統一を却下クローズ（AppErrorは既に{"error":message}統一済み、
  code/message/status化はクライアントのbody.error依存を壊す破壊的変更で利益を上回らない）。
- 2026-11-20: backup暗号文公開順序を修正（先に暗号文mv→平文rm）。公開失敗時に平文が残り、
  当日バックアップの完全喪失を防止。3経路（assert/mv/gpg）でオラクル検証。

- 2026-11-20: real-api E2E の /auth/verify タイムアウトは他 spec との並走による実サーバー競合が
  真の原因。ci-e2e.sh を real-api 単独実行＋残り11spec の2段階へ分離し、失敗時後処理を
  共通関数化（artifact 収集を両ブロックで実施）。52+4=56件 green を確認。

- 2026-11-20: real-api.spec.ts の /auth/verify 待ちタイムアウトは、全E2E実行時のみ再現し
  単独実行（E2E_REUSE=0 E2E_CLIENT_PORT=18125）では4件すべて成功。R4 レビューでも今回の
  差分（AppError::BadGateway・client 文言/busy ガード）と因果関係なしと判定。
  既存の環境依存フレーキーとして記録し、後続タスクで webServer 起動待ちを検討する。

- 2026-11-19 却下（Runtime Proof 済み）: 単一ライター・ロックの stale 回収 TOCTOU は
  `server/src/lib.rs:725-733` で再読込一致チェック＋create_new の AlreadyExists 拒否が既にあり、
  同時起動でも排他が破れないため却下。GOV-01 要約 E2E の生PII→[redacted] は
  `server/src/lib.rs:1886` のサーバー側 summarize_reason が担い、クライアントは生表示しないため
  E2E でのオラクル変更は実装と矛盾し却下（Rust 単体/property テストが担保）。

- 2026-11-19 全体レビュー（swarm-project-review + swarm-todo-discovery、15担当・深度1・無制限プロファイル）:
  約100件の指摘を重複排除・Runtime Proof で裁定し、上記へ反映。主な却下: lib.rs god module 分割・
  AppState FromRef 分割・ドメインエラー型導入（大規模リファクタで既存ゲートとのトレードオフ判断が必要）、
  finalize_report の運用者チェック（設計判断済み）、confidence=speculative の将来リスク
  （短い JWT_SECRET 露出、Merkle root 保存経路、ロールバック対応関係）は候補として保留。
- セッション JWT の iss/aud 検証は「別途要件化」で未定義（DEC 判断 cần）、TrustedAttributeRequired の
  実入力経路は DEC-01 外部待ちのため既存未着手を維持。

- 2026-11-18 時点: 作業ツリーはクリーンで、以下に「未コミット」とある記述は各作業当時の履歴。
  現行状態はすべてコミット済み（最新 `dd9aa7c`）。E2E 定義数は56件（12 spec）。
- 詳細な残タスク棚卸しは `docs/TASK_INVENTORY.md`、レビュー履歴は `docs/REVIEW_REPORT.md`、
  未知タスク調査は `docs/UNKNOWN_TASKS_INVESTIGATION.md` を参照。
- 本ファイルにはクロスレビューで統合・採用した項目を追記する。
- P0 の R1-17 / R1-18 / R2-09 はコード上確定（Runtime Proof 済み）。
  他の項目はレポートの静的推論を含むため、着手時に事実確認する。
- 今回の反映は未コミットの `server/src/lib.rs` / `oidc.rs` / `tests.rs` と
  `client/src/lib.rs` を grep した確定値に基づく。
  E2E 定義数40件は `e2e/tests/*.spec.ts` の `test()` 定義数を実測して確認した。

- 2026-11-20 (R5 / セキュリティレビュー採用指摘の反映): OIDC state Cookie の `__Host-`
  プレフィックスを本番（`OIDC_COOKIE_SECURE` 有効）のみへ条件付き採用（mock は Secure
  が無いとブラウザが保存を拒否しフローが壊れるため旧名を維持）、同一名 Cookie の重複
  拒否、`doctor` 警告文を session / refresh / OIDC state を含む表現へ更新。`__Host-`
  を常時使わない理由はコードコメントに明記。E2E 定義数は不変（変更はサーバー側のみ）。
  `docs/OPERATIONAL_POLICY.md:263` の `OIDC_COOKIE_SECURE` 説明は `0337d64` で更新済み（クローズ）。
- 2026-11-20 (R5 / R4 採用指摘の反映): 採用分の OIDC 空値検証・doctor max_auth_age 整合・
  shutdown 2件・シェル2件・filechooser オラクルは全て既に `[x]` であり、該当する未着手は
  存在しなかった。本次で doctor 内訳の「＋設定検証3件」6箇所（TASK_INVENTORY 3箇所 /
  TEST_SPECIFICATION 2箇所 / 本ファイル`:22` の 40→43 テスト）、CLI-01 設定判定テスト名
  34件目（`oidc_max_auth_age_mock_with_complete_oidc_warns_without_disclosing_value`）、
  backup-rotate の RETENTION_DAYS 境界（当時 36500 受理 / 36501・100000 拒否。
   R14 で上限を 20000 へ変更済み、末尾メモ参照）を反映。
  未対応のまま残すもの（本次で確認した未着手）: `doctor --strict` のデプロイ前ゲート
  組み込み（`:239`）、backup-rotate の verify_audit_log と暗号化後復号検証の追加
  （`:241`、チェックボックス表記が `- []` で不正）、`doctor --strict --help` 併用の
  他CLI整合（`:368`）。

- 2026-11-20 (R14 / ops-docs 発掘3件の反映): `scripts/backup-rotate.sh` の
   `RETENTION_DAYS` 上限を 36500 → 20000 日へ変更。旧上限では
   `cutoff = now - 36500*86400` が負になり `mtime < cutoff` が常に false で
   古い世代が削除されない（fail-open）ため、`now/86400` より小さい値へ制限し、
   cutoff が非正になる場合も明示拒否する防御チェックを追加。境界は
   「20000 受理 / 20001・100000 拒否」へ更新し、上限値で mtime を 1970 年代に
   設定した古い世代が実際に prune されるテストを追加。定数・テスト・
   `docs/BACKUP_RESTORE_RUNBOOK.md` の記載をすべて整合させた（上記の
   36500 は R5 当時の履歴）。`scripts/test-backup-rotate.sh` の実測は
   `expect_failure` 28件＋`expect_success` 12件=40件（4fb80e0 で root 不一致オラクル1件を追加、
   R14 時点は 27＋12=39件）。`docs/TASK_INVENTORY.md:481` / `docs/HANDOFF.md:262` の
   「5件・5件」を是正。
   `docs/OPERATIONAL_POLICY.md:265` の `PINATA_JWT` を本番必須・実トークン必須
   （未設定/空/空白/mock_jwt 前後空白含むは起動拒否、`doctor` の `production`
   カテゴリで診断）へ更新し `docs/DETAILED_DESIGN.md:123` と整合。
   今回の発掘分は3件すべて対応済みで、新たな未着手はなし。

- 2026-11-20 (R5 / R4 採用指摘の反映): `doctor --strict` への `AUDIT_EXPECT_ROOT`
  警告追加は 2026-11-20 対応済み。`doctor` に独立カテゴリ `audit_expect_root` を追加し
  22→23 カテゴリ・45→46 テストへ更新。本番相当（mock 無効）で未設定・空白のみ・不正 hex
  なら固定文言の警告（値は非エコー、既定は警告のみで `--strict` 時のみ失敗）。
  共有の `production_config_errors` と `backup-rotate.sh` の未設定時契約は変更しない。

- 2026-11-20 (R5 / R4 採用指摘の反映・4件): `doctor` の `AUDIT_EXPECT_ROOT` 判定を
  `backup-rotate.sh` と統一し、`lookup` の生値へ `^[0-9a-fA-F]+$` を適用（trim 廃止）。
  前後空白付き hex は警告対象（値は非エコー）。境界テストの有効 hex 配列から
  `"  ABCDEF0123456789  "` を除外し警告ケースへ移動（doctor テスト数は46のまま）。
  TEST_SPECIFICATION の E2E 59件表記6箇所へ「当時59件、現在65件」を注記し、
  `23 check(s)` 実測の出所を 2026-11-20 再確認へ更新。DETAILED_DESIGN の
  フォールバック5変数（新規カテゴリ2件）と 19→21→22→23（カテゴリ数）の内訳を分離。

- 2026-11-20 (R5 / 採用指摘の反映): 実バイナリ起動テストの指摘は見送り。親が Runtime Proof で
  実 stderr の非エコー（値0回）を確認済みであり、純粋関数オラクル＋固定 `eprintln!` により
  実装上の漏えい余地が無いため、実バイナリ起動テストはコスト対効果が低いと判断した。
  併せて AUDIT_ROOT_MISMATCH を `common/src/lib.rs` の `pub const` へ単一化し両 CLI から参照、
  restore_db の safety copy 復元成功時に「safety copy was restored successfully from `<path>`」を
  併記して成否を観測可能とした（失敗時文言・`safety_copy:` 出力・instance lock・引数契約は不変）。

- 2026-11-20 (R5 / 採用指摘の反映): `redact_url` のパース失敗時フォールバックを、先に
  `scheme://` 直後の authority（最初の `/` まで）を切り出して userinfo を除去し、その後に
  クエリ/フラグメントを除去する順序へ是正。userinfo 内に `?` / `#` を含む
  `https://user:pa?ss@exa mple.com/path` でも userinfo が残存しない回帰テストを追加。
  スキームレス URL（`//` 始まりを含む）は authority 走査の対象外として userinfo を除去しない
  （クエリ/フラグメントは従来どおり除去）判断を doc コメントへ明記。
  `scripts/test-doc-numbers.sh` は E2E 総数ゲートの修飾語上限（`{0,40}`）を撤廃し、
  bin 照合を `server/src/bin/*.rs` の実ファイル列挙へ変更（未知 bin は既存分岐で fail-closed）。
  check23 / E2E65件 / migration13 / doctor46 は不変。

- 2026-11-20 (impl-bincounts-final): doc_bin_counts の bin 名⇔数値間修飾語上限
  （`[^0-9]{0,8}`）を撤廃し `[^0-9]*` へ変更（E2E 総数側 total_count_pattern と方針統一、
  TEST_SPECIFICATION:84 の10文字空きを走査対象化）。TASK_INVENTORY の現行記述2箇所
  （:36 / :338）の「bin 内に9テスト」を実測10テストへ是正。check23 / E2E65件 /
  migration13 / doctor46 / sign_audit_export10 は不変。除外ロジック・全7bin照合・
  DETAILED_DESIGN CLI表照合・E2E/doctor check/migration/群名双方向/総数未注記ゲートは維持。
