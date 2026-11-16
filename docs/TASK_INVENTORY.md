---
type: Reference
title: "タスク棚卸し（残タスク・未決事項）"
description: "openIllustration の現状（master 3662145, 2026-11-15 再確認）を基に、安全第一で洗い出した残タスク・未決事項・検証ギャップ。"
tags:
  - tasks
  - backlog
  - release-gates
  - security
  - operations
generated:
  by: dsh (opencode-go/iceman-contributor-free)
  at: "2026-11-15T12:22:13+09:00"
verified:
  by: dsh
  at: "2026-11-15T12:39:43+09:00"
status: current
---

## 1. 前提（確認時点の事実）

- 確認: 2026-11-15T16:05:09+09:00、ブランチ `master` は `origin/master` と同期（`3662145`）、作業ツリー clean。
- 直近の主な到達点: RustCrypto 更新・`cargo deny check advisories` CIゲート（既知11件を理由付き ignore、`unused-ignored-advisory = "deny"`）・RS256 JWT署名のオプション実装（`JWT_RSA_PRIVATE_KEY_PEM` 設定時のみ、既定は HS256）・CI 成功。DEC-02採用方針のフレームワーク実装/文書化（`0958c60`: 保守的一律既定21・構成可能な年齢/地域ポリシー・明示設定必須/信頼属性必須のフェイルクローズ・認証不要 `GET /policy/age` による非秘密の有効設定公開、法的確定/専門家レビューは未完）と公開レスポンスの非秘密性回帰テスト追加（`3662145`）。
- 方針: `ALLOW_MOCK_AUTH=1` は開発専用。未実装機能を本番対応済みと扱わない。模擬支援APIは 501 で無効（JPYC送金検証の実装まで）。DB内ハッシュ連鎖は第三者検証可能な監査ではない。
- 参照: [HANDOFF.md](HANDOFF.md), [DOCUMENT_STATUS.md](DOCUMENT_STATUS.md), [DETAILED_DESIGN.md](DETAILED_DESIGN.md), [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md), [REVIEW_REPORT.md](REVIEW_REPORT.md), [OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md), [SECURITY_ADVISORIES.md](SECURITY_ADVISORIES.md), [DEPENDENCY_LICENSES.md](DEPENDENCY_LICENSES.md), [adr/index.md](adr/index.md)。

## 2. 本番公開ゲート・意思決定・外部手続き（ユーザー判断/手続きが必要）

安全第一のため、実装・設定より先に方針・契約・法務・運用体制の確定が必要な項目。

| 区分 | 内容 | 状態/根拠 | 次のアクション |
| --- | --- | --- | --- |
| DEC-01 JPKI認証連携 | 実サービス接続、`acr`/`amr`、`sub` 単位の全セッション失効の永続化/運用整合、事業者申込・クライアントID/シークレット/リダイレクトURI登録 | ADR-0002 Accepted。OIDCフローはモックOPで検証済み、`sub` 単位失効はインメモリの閾値方式を実装済みだが、永続化・多プロセス共有・実JPKI/実機・失効・保証レベルは未検証（[DOCUMENT_STATUS.md](DOCUMENT_STATUS.md), [DETAILED_DESIGN.md](DETAILED_DESIGN.md), [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)） | 事業者申込と認定、実OP接続情報の取得・秘密管理、保証レベル要件の確定、失効方式の運用/永続化方針の確定 |
| DEC-02 年齢・対象地域 | 対象地域、必要属性、最終閾値、属性保持期間の専門家/法務レビューと確定。検証済み年齢/居住地属性の入力経路（DEC-01連携）と保持・開示方針の確定 | **方針採用・実装進行中（法的確定/専門家レビューは未完）**（[DOCUMENT_STATUS.md](DOCUMENT_STATUS.md), [AGE_VALIDATION_POLICY.md](AGE_VALIDATION_POLICY.md)）。コード/テストで反映済み: 保守的一律既定 `21`、国別上書き・対象地域・適用モードの構成、明示設定必須のフェイルクローズ、信頼属性必須モードのフェイルクローズ、認証不要 `GET /policy/age` による有効設定値の常時公開（非秘密） | 法務/専門家レビューで対象地域・適用法令・最終閾値・保持期間を確定し、DEC-01の検証済み属性と接続。公開設定に秘密情報が含まれないことを継続確認 |
| DEC-03 JPYC決済 | 実RPCプロバイダ契約、鍵運用（HSM/マルチシグ）、返金・紛争対応、AML/KYC、資金移動業・税務の法的確認。コントラクトアドレス/確認数は実装前に一次ソースで再確認 | ADR-0003 Accepted。注文作成・署名検証・状態機械・BIP44派生等は実装済み、実チェーン検証/鍵運用は未着手（[HANDOFF.md](HANDOFF.md), [OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)） | 契約・法務確認、一次ソース確認、鍵管理方式の決定と秘密管理基盤 |
| DEC-04 検証可能な監査 | 対象イベント、正規化形式、署名鍵、外部検証方式、公開範囲、削除・訂正との調整 | 未決定。DB内ハッシュ連鎖は第三者検証可能な監査ではない（[DOCUMENT_STATUS.md](DOCUMENT_STATUS.md), [DETAILED_DESIGN.md](DETAILED_DESIGN.md)） | 監査要件の定義（外部アンカー/追記専用媒体/独立検証コマンド/鍵管理）と承認 |
| DEC-05 モデレーション | 投票者選出・Sybil耐性、第三者検証可能な監査（DEC-04） | **ADR-0004 Accepted（2026-11-16）**。期限ベース確定（`deadline_at`/`concluded_at`、3票即時確定廃止）、定足数未達復帰、30日以内の異議（`appeal_of`）、投稿者投票除外、期限切れ投票拒否、同数 `Rejected` は実装済み（[DOCUMENT_STATUS.md](DOCUMENT_STATUS.md), [adr/ADR-0004](adr/ADR-0004-moderation-procedure.md)）。BAN/UNBAN/UNPIN の監査付き運用操作と期限到達の自動確定ワーカー（`MODERATION_FINALIZE_INTERVAL_SECS`）も実装済み。運用者は各デプロイが設定し、プロジェクト側の決定不要 | 未実装項目の設計・実装 |
| DEC-06 クロスプラットフォーム | 対応OS/端末/ブラウザ/最小バージョン、JXL/JPKI/ウォレット実機試験範囲、安全なフォールバック | 未決定（[DOCUMENT_STATUS.md](DOCUMENT_STATUS.md), [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)） | 対応マトリクスと受入基準の定義 |
| DEC-07 性能・費用 | 初期表示/API/転送量/Wasmサイズ/運用費/手数料の測定条件と受入値（SLO） | 未決定（[DOCUMENT_STATUS.md](DOCUMENT_STATUS.md), [DETAILED_DESIGN.md](DETAILED_DESIGN.md)） | 計測条件・閾値の合意と計測 |
| DEC-08 本番ネットワーク・運用 | ホスティング、TLS、プロキシ、CORS、本番オリジン、IPポリシー、復旧手順、鍵ローテーション | 未決定/未整備（[DOCUMENT_STATUS.md](DOCUMENT_STATUS.md), [OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)） | 本番構成・責任分界・Runbook整備 |
| DEC-09 OSSライセンス運用 | 配布チャネル確定時の同梱手順 | `AGPL-3.0-or-later` 宣言・依存棚卸し・`cargo-deny` 検査・`scripts/prepare-distribution.sh`（事前ライセンス検査＋LICENSE/NOTICE/依存一覧/本文452件を生成）は実装済み（[DEPENDENCY_LICENSES.md](DEPENDENCY_LICENSES.md)） | 配布形態（バイナリ/コンテナ/ソース）の確定とリリースプロセスへの組込み、AGPL ソース提供手段の決定 |

## 3. 実装可能な残作業（優先度の目安つき）

依存関係・リスクに応じて順序調整が必要。`[要判断]` は外部仕様/承認待ちを含む。

### 3.1 認証・セッション（JPKI/OIDC/JWT）

- [対応済み] `acr`/`amr` 保証レベル検証: `OIDC_REQUIRED_ACR` / `OIDC_REQUIRED_AMR` 設定時に ID トークンを検証し、欠落・不一致は401拒否（fail-closed）。モックOPで成功/拒否を実証（[DETAILED_DESIGN.md](DETAILED_DESIGN.md)）。
- [対応済み] `auth_time`/`max_age` 検証: `OIDC_MAX_AUTH_AGE_SECS` 設定時に認証要求へ `max_age` を付与し、`auth_time` の新鮮さを検証。欠落・範囲外は401拒否（fail-closed）。モックOPで3ケース実証。
- [高][要判断] **実JPKI/実OP接続と検証**: 実機・失効確認（`server/src/oidc.rs`, [HANDOFF.md](HANDOFF.md), [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)）。
- [対応済み] **`sub` 単位の全セッション失効と永続化**: back-channel logout で `sid`/`sub` を失効対象とし、`iat` 閾値で既存セッションを拒否。マイグレーション 008 の `revoked_subs` に閾値を保存し起動時にロード、再認証時に削除。再認証で新セッション発行可。
- [対応済み] **`sid` 単位の失効永続化**: マイグレーション 009 の `revoked_sessions` にログアウト済み sid を保存し起動時にロード。`/auth/logout` と back-channel logout の両方を修正。実プロセスで「ログアウト→再起動→401 維持」を確認（[HANDOFF.md](HANDOFF.md)）。
- [対応済み] 古い失効 sid の自動回収: セッション有効期限(24h)+60s 超のレコードを起動時/ワーカー周期で削除（リフレッシュは常に新 sid を発行するため再利用なし）。実プロセスで 2件→起動→1件を確認。
- [中] 複数プロセス共有セッション表・実OP/運用要件との完全整合、実機検証は継続課題（DEC-01/ADR-0002）。
- [対応済み] リフレッシュトークンのローテーションと盗用検知: マイグレーション 006 でハッシュのみ保存。ログイン時に 30 日トークンを HttpOnly Cookie 発行、`POST /auth/refresh` で毎回回転。再利用検知時はファミリ全体を失効。ログアウト時もファミリ失効。回帰テスト5件（[HANDOFF.md](HANDOFF.md)）。
- [対応済み] 期限切れリフレッシュトークンの自動回収: 起動時と finalizer 周期で `expires_at <= now` を削除。単体テストと実プロセス（2件→起動→1件、purge ログ）で確認。
- [対応済み] **JWT 設定の fail-closed 化**: 不正な `JWT_RSA_PRIVATE_KEY_PEM` は有効な `JWT_SECRET` があってもフォールバックせず起動拒否。短い `JWT_SECRET` が RS256 選択で隠れる場合は警告。
- [対応済み] **RS256 の `kid` 複数鍵検証**: 署名ヘッダへ `kid` を付与し、`JWT_RSA_VERIFICATION_KEYS` で旧公開鍵を登録可能にした。未知 `kid` 拒否・`kid` 無し旧トークン互換・重複 kid 拒否を回帰テストで確認。
- [対応済み] `JWT_REQUIRE_KID`: 本番向けに `kid` 無しレガシートークンを拒否するオプションを追加。HS256 併用は起動拒否（fail-closed）。単体テスト2件と実プロセスで確認。
- [中][運用設計] **RS256 運用強化の残項目**: HSM/外部鍵管理、複数プロセス/サービス間の公開鍵配布・ローテーション手順（`JWT_RSA_PRIVATE_KEY_PEM`/`OIDC_CLIENT_SECRET` の秘密管理・ログ非出力を継続）（[SECURITY_ADVISORIES.md](SECURITY_ADVISORIES.md), [REVIEW_REPORT.md](REVIEW_REPORT.md), [OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- [対応済み] 開発用モック認証の nonce: `/auth/login` が発行した nonce を `/auth/verify` で必須化し、5分 TTL の単回消費でリプレイを拒否。クライアントも login→verify のフローへ更新（[DETAILED_DESIGN.md](DETAILED_DESIGN.md)）。
- [中] 開発用モックの VP 署名検証は未実装（本番は OIDC 経路を使用する前提）。

### 3.2 決済（JPYC）

- [高][要判断] **実チェーンRPC検証** と検証ワーカー: `chain verification is not configured` パスの解消、送金元/受取先/金額/コントラクト/チェーン/最終性/再編成の検証（`server/src/payments.rs`, [HANDOFF.md](HANDOFF.md), [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)）。
- [高][要判断] **2-of-3 クォーラム**、バックグラウンド確認ワーカー、コントラクト `pause` 監視、再編成の再評価（[HANDOFF.md](HANDOFF.md), [REVIEW_REPORT.md](REVIEW_REPORT.md)）。
- [高] **支援APIの置き換え**: `POST /support/*` は 501 無効化中。JPYC送金検証の実装まで支援・有償サービスを提供しない（`server/src/lib.rs`, [OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- [中] 受取アドレス未設定時の注文作成 501 の運用設計（`PAYMENT_RECEIVING_ADDRESS` / `PAYMENT_RECEIVING_XPRV`）、鍵の秘密管理。
- [対応済み] 価格上限の整合: アップロード `price` を注文と同じ `MAX_ORDER_JPY`（1,000,000）以下に制限し、境界・上限超過を API 回帰テストで確認（[DETAILED_DESIGN.md](DETAILED_DESIGN.md)）。
- [対応済み] 注文と価格の照合: `CreateOrderRequest` に `cid` を追加し、マイグレーション 007 で注文へ CID を記録。存在しない・レビュー中・価格不一致・不正 CID の注文を拒否（回帰テスト4件）。
- [対応済み ad9d2a8] 注文IDの基本入力検証: `GET/POST /payments/orders/{order_id}` 系で `order_id` の UUID 形式を検証し、不正形式は `400 Bad Request`（DB参照前に拒否）。有効 UUID の不存在/非所有は従来どおり `404`（`server/src/payments.rs`, 回帰テスト `test_payment_order_rejects_invalid_order_id_format`）。
- [一部対応] 決済異常系: モックで同一送金の再 verify・確認数進展→`FINALIZED`・確定後不変を検証（[TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)）。実チェーン上の再編成・二重通知・別コントラクト検知は RPC 実装時に検討。
- [対応済み e87df61] 決済 verify の防御分岐を回帰テストで保護: 金額不一致（`MISMATCH` 遷移）、送金再利用（`"transaction is already used by another order"`）、期限切れ注文（`"order is no longer verifiable"`）、必要確認数未満（`CONFIRMING` 維持）の4ケース（`test_payment_verify_rejects_transfer_mismatch`, `test_payment_verify_rejects_reused_transaction`, `test_payment_verify_rejects_expired_order`, `test_payment_verify_keeps_confirming_until_threshold`）。ハンドラ挙動変更なし。
- [対応済み] `expires_at` 経過の実質的な期限切れ注文を拒否: 旧実装は `status` が `PENDING` のまま期限を過ぎると verify / bind を通してしまうため、共通ヘルパーで時刻比較と `EXPIRED` 更新を追加。両エンドポイントの回帰テストで確認。

### 3.3 モデレーション・コンテンツ

- [対応済み] **ADR-0004 の期限・定足数・異議・投票資格**: マイグレーション004で `deadline_at`/`concluded_at`/`appeal_of` を追加。3票即時確定を廃止し期限後確定、定足数未達時 `Approved` 復帰、30日以内の異議、投稿者除外、期限切れ投票拒否を実装。回帰テスト7件を追加、既存3件を適応。
- [対応済み] **ADR-0004 の運用者とBAN/ピン解除監査**: `MODERATION_OPERATOR_HASHES` でセルフホストごとに運用者を設定（未設定なら特権操作を全拒否）。BAN（根拠・期間必須）/UNBAN/UNPIN を `governance_audit_log` に記録。
- [対応済み] **期限到達の自動検知**: 起動時のバックグラウンドワーカーが `MODERATION_FINALIZE_INTERVAL_SECS`（既定300秒・30〜3600秒）ごとに期限超過通報を走査して確定。停止シグナルで終了し、手動 finalize と冪等。
- [高][要判断] **ADR-0004 の残項目**: 投票者選出・Sybil耐性・オンチェーン記録方針、`governance_audit_log` の第三者検証可能な監査への昇格（DEC-04 と連携）（[DETAILED_DESIGN.md](DETAILED_DESIGN.md), [DOCUMENT_STATUS.md](DOCUMENT_STATUS.md)）。
- [対応済み] 並行投票の競合対策: libSQL ローカル接続は同時に1つの `BEGIN` しか保持できないため、接続ごとの書き込みロックでトランザクションをプロセス内直列化。競合の再現・ロック有効性・並行投票と確定の同時実行を回帰テストで確認（[TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)）。
- [対応済み 00a9176] 通報入力の基本検証: `POST /mod/report` で `target_cid` に `is_safe_cid` を適用し、不正 CID を `400 Bad Request` で早期拒否（DB参照前）。有効 CID で対象コンテンツ不存在時の `404` は維持（`server/src/lib.rs`, 回帰テスト `test_report_rejects_invalid_cid`）。厳密な CID 仕様検証（3.6）は別課題。
- [対応済み 1dfbbf6] 通報 reason の基本検証: `POST /mod/report` で reason を前後空白除去の上、空/2000バイト超を `400 Bad Request ("Invalid report reason")` で拒否（DB参照前）。保存値と監査ログは除去後の reason を使用。詳細なモデレーション方針（文字数上限の妥当性含む）は ADR-0004 承認待ち。二重通報の `400` 拒否（`test_report_duplicate_rejected`）と reason 検証（`test_report_reason_validated`）の回帰テストを追加。
- [対応済み 2f0d057] アップロード入力境界の防御分岐を回帰テストで保護: 必須フィールド欠落（file/owner_hash/signature）→ `400`、トークン sub と owner_hash の不一致 → `401 "Token sub mismatch"`、BAN済みユーザー → `403 "User is banned"`（`test_upload_rejects_missing_required_fields`, `test_upload_rejects_owner_mismatch`, `test_upload_rejects_banned_user`）。ハンドラ挙動変更なし。
- [対応済み fefa49a] 投票入力の基本検証テスト: `POST /mod/vote` で `report_id <= 0` と未知の `result` を `400` で拒否すること、トークン `sub` と `vote.juror_hash` の不一致（署名が正しくても）を `401` で拒否し票を記録しないことを回帰テストで保護（`test_vote_with_invalid_payload_rejected`, `test_vote_with_sub_mismatch_rejected`）。ハンドラ自体の挙動変更はない。

### 3.4 監査（DEC-04）

- [一部対応] **署名付き監査エクスポート**: ADR-0005 に基づき、`sign_audit_export` が全コンテンツ監査エントリ＋`governance_audit_log`（BAN/UNBAN/UNPIN）と Merkle root を RSA 署名した version 3 スナップショットを生成。`verify_audit_export` が公開鍵のみで検証（両エントリ列の改ざん検出を実測確認）。手順は [AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md)。
- [対応済み] 監査エクスポートの公開範囲: `reason`/`details` を既定でマスクし、`--include-reasons` オプトインで本文を含める。マスク後の文字列が署名対象（[AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md)）。
- [高][要判断] **第三者検証可能な監査の残項目**: DB外アンカー/追記専用媒体、公開範囲の最終判断、訂正/削除との調整、鍵の HSM 管理とローテーション（[DETAILED_DESIGN.md](DETAILED_DESIGN.md), [REVIEW_REPORT.md](REVIEW_REPORT.md)）。運用者は各デプロイが設定する。
- [中] [一部対応] ローカルDBの `content_audit_log` ハッシュ連鎖は `cargo run -p server --bin verify_audit_log -- [--database-url URL] [--database-token TOKEN] [--cid CID] [--expect-root HEX]` で読み取り専用に検証可能（`common::AuditLogEntry::calculate_hash` と同一計算）。`log_id` 連続性チェックと Merkle root を追加し、外部保存 root との照合で欠落・末尾切り詰めを検出できる（実測確認済み）。新規エントリは長さプレフィックス付き v2 正規化ハッシュを生成し、v1 も投稿時に互換検証する。これは計算整合性の確認であり、第三者検証可能な監査方式の代替ではない（AUDIT-01/DEC-04 は未充足）。
- [対応済み] 署名付き監査スナップショットのバージョン検証: `AUDIT_SNAPSHOT_VERSION=3` を定数化し、署名・検証の両方で不一致を fail-closed 拒否。CLI で未知バージョン 999 が拒否されることを確認（[AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md)）。

### 3.5 DB・移行・信頼性

- [対応済み] **既存DBの段階的アップグレード試験**: 001→003 適用済みDBに 004 を追加して新カラムの backfill と既存データ保持を確認。`verify_migrations` は適用済みだがファイルが無いバージョンを検出し起動を拒否（古いデプロイイメージ誤用の防止）。
- [対応済み] 単一ライター起動の強制: ローカル DB 起動時に `<DB path>.instance.lock` を原子的作成し、生存 PID のロックでは第二インスタンスを拒否。stale ロックは自動回収、停止時に削除（[OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。リモート DB は警告のみ。
- [対応済み] ロールバック判断手順: 前方のみ（down migration なし）の方針を明示し、適用前バックアップ＋旧バイナリへの復帰手順、失われるデータと監査ログの範囲を [BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md) に記載。
- [対応済み] マイグレーション検証の fail-closed 強化: ディレクトリ欠落時に `verify_migrations` が `Ok(())` を返し「適用済みファイル欠落」検査をすり抜けていた問題を修正。単体テストで拒否を確認。
- [低] down migration 自体の実装（現状は未採用、必要になった時点で設計）。
- [対応済み] 複数インスタンス起動方針: **単一ライター運用**を初期運用の前提として決定・文書化。プロセス内書き込みは接続ごとロックで直列化し、プロセス間排他はアーキテクチャ側で回避（[OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- [対応済み] バックアップ/リストア手順: `backup_db`（VACUUM INTO・上書き拒否）と `verify_backup`（構造検証）を追加。実 DB で作成・検証・データ保持・監査連動を確認し、[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md) に手順化。
- [対応済み] リストア CLI: `restore_db` が整合性検証・WAL ガード・上書き拒否・安全コピー・原子的置換・置換後再検証を実施（[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)）。
- [対応済み] バックアップの自動ローテーション: `scripts/backup-rotate.sh` が取得・検証・保持期間（既定30日）超過分の削除を実施。直近 `MIN_KEEP`（既定3）件は保護。静的検証と実 DB で動作確認（[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)）。
- [対応済み] `verify_backup` に `PRAGMA integrity_check` を追加（旧実装はテーブル/マイグレーション数のみで破損を見逃す）。実 DB で破損ページを持つバックアップが拒否されることを確認。
- [対応済み] `backup_db` の取得後検証に `_migrations` 台帳チェックを追加（旧実装はコメントで保証を謳いながら未検証）。`backup_ok ... migrations=9` を実出力で確認。
- [対応済み] スケジューラ設定例: `deploy/openillustration-backup.{service,timer,cron}` を追加。`backup-rotate.sh` は `BIN_DIR` のビルド済みバイナリを優先し、実リリースバイナリで取得・検証・ローテーションを確認（[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)）。
- [対応済み] バックアップ暗号化: `BACKUP_GPG_RECIPIENT` 設定時に GPG 暗号化（gpg 不在時は fail-closed）、平文は検証・暗号化成功後に削除。復号→verify_backup まで実確認（[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)）。
- [対応済み] オフサイト保存: `OFFSITE_COPY_CMD` + `OFFSITE_DEST` で汎用コピーコマンドを呼び出し（eval 不使用）。設定不整合・コピー失敗は fail-closed。暗号化成果物の送信にも対応（[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)）。
- [対応済み] バックアップ失敗通知: `ALERT_CMD`（実行ファイルのパス）を `<メッセージ>` 1引数で呼び出し。成功時無通知、通知失敗は元の終了コードを維持、不正パスは fail-closed。実3ケース確認（[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)）。
- [中] 通知先ラッパーの導入環境への接続と実運用での復旧訓練。

### 3.6 ネットワーク・アップロード・整合性

- [対応済み] CORS の fail-closed 化: `ALLOW_MOCK_AUTH=1` の開発時のみ未設定で開発オリジンへフォールバックし、本番相当（mock 無効）で未設定なら全オリジン拒否。実サーバーで ACAO ヘッダの有無を確認（[DETAILED_DESIGN.md](DETAILED_DESIGN.md), [OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- [中] 本番配信オリジンの具体値の決定と設定（ローカル開発オリジンを本番に流用しない）。
- [中] プロキシ/WAF/多段構成下の IP 判定・`TRUSTED_PROXY_CIDRS` 運用の実環境検証（[TEST_SPECIFICATION.md](TEST_SPECIFICATION.md), [OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- [対応済み] IPFS 完全性監視: チェックサム不一致時に CID と redact 済みゲートウェイ URL のみを WARN ログへ記録（本文/チェックサム値は出さない）。実サーバーで確認（[DETAILED_DESIGN.md](DETAILED_DESIGN.md)）。
- [対応済み] IPFS ストリーム切断: 宣言 Content-Length を実際より大きくしたローカルモックで不完全データを返さないことを検証（[TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)）。
- [対応済み] IPFS リトライ方針: `IPFS_MAX_ATTEMPTS`（既定1、最大5）でネットワークエラーのみ再試行。HTTP エラー/不正データは即拒否。パーサ回帰テストと実起動確認（[OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- [中] IPFS: 厳密なCID仕様検証、キャッシュ方針、単一ゲートウェイ異常時の運用手順、実ゲートウェイ/Pinata連携検証（[DETAILED_DESIGN.md](DETAILED_DESIGN.md), [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)）。
- [対応済み] 実 multipart サイズ境界: `Content-Length` 検査ミドルウェアを追加し、10 MiB 超を 413 で拒否（単体テストと実サーバーで確認）。虚偽/欠落時は axum の本文制限が多層防御（[TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)）。
- [対応済み] アップロード重複フィールド: 同一 `price` の二重送信が 400（Duplicate price field）で拒否される回帰テストを追加（[TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)）。
- [対応済み] 実鍵署名試験: 実 RSA 秘密鍵で署名したアップロードの受理と、署名を別バイト列へ使い回した場合の 401 拒否を検証（[TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)）。
- [対応済み] JXL デコード検証の技術選定: `jxl-oxide` 0.12.6 を第一候補とし、現時点では依存追加しない判断を [ADR-0006](adr/ADR-0006-jxl-decoding.md) に記録。
- [低] 本番受入要件確定後の完全なJPEG XLデコード検証の実装（[DETAILED_DESIGN.md](DETAILED_DESIGN.md), [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)）。

### 3.7 運用・可観測性・停止動作

- [対応済み] 構造化ログ: `LOG_FORMAT=json` で 1行1 JSON 出力（tracing-serde は tracing-subscriber の公式依存）。起動メッセージも tracing 統一。実サーバーで全行パース成功（[OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- [対応済み] 最小メトリクス: `GET /metrics` が Prometheus 形式の全リクエスト数・4xx/5xx 数を公開（外部クレートなし、パス/個人情報なし）。単体テストと実サーバーで確認（[OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- [対応済み] 鍵ローテーション・障害対応 Runbook: [KEY_ROTATION_RUNBOOK.md](KEY_ROTATION_RUNBOOK.md) に `kid` 無停止ローテーション2段階、漏えい時対応、認証障害切り分け表を記載。`openssl` の SPKI DER 出力を実確認し、既存ドキュメントの公開鍵形式表記を SPKI へ修正。
- [中] ログ集約基盤・アラート通知先の選定（[DETAILED_DESIGN.md](DETAILED_DESIGN.md), [OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- [対応済み] liveness `/health` と readiness `/ready` を分離。readiness は DB `SELECT 1` と停止フラグを検査し、停止時は 503。IP ゲートはバイパスしない（[OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- [対応済み] readiness の意味を「API リクエストを受けられるか」と定義し、外部依存障害は反映しない方針を明記。外部依存の検知は /metrics と WARN ログ（IPFS チェックサム不整合等）で実施（[OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md)）。
- [中] 外部依存のアラート通知先への接続と実運用でのデプロイ・復旧訓練。
- [対応済み] グレースフル停止: `GRACEFUL_DRAIN_TIMEOUT_SECS`（既定 30 秒、1〜300 秒）で drain に上限を設定し、超過時は警告ログを出して残接続を切断。停止シグナルで readiness を 503 化（[REVIEW_REPORT.md](REVIEW_REPORT.md), [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md)）。

### 3.8 依存・セキュリティ衛生

- [中] **既知の脆弱性勧告（受容中）**: `libsql 0.10` 由来 10 件（上流/代替の見直し、特にリモートDB利用前）と `paste` 1 件（RUSTSEC-2024-0436, unmaintained・ビルド時マクロ・既知脆弱性なし・パッチなし、Leptos 側の除去待ち）。CI で `cargo deny check advisories` を通過させつつ四半期/更新時に再評価（[SECURITY_ADVISORIES.md](SECURITY_ADVISORIES.md), [deny.toml](../deny.toml)）。
- [対応済み] 配布物のライセンス同梱: `scripts/prepare-distribution.sh` が LICENSE・NOTICE・依存一覧・依存ライセンス本文（451件）を生成（[DEPENDENCY_LICENSES.md](DEPENDENCY_LICENSES.md)）。
- [対応済み] 配布時のライセンス再監査: `scripts/prepare-distribution.sh` が `cargo deny check licenses` を事前実行し、不整合なら成果物を生成しない。NOTICE に生成時刻とエントリ数を記録（[DEPENDENCY_LICENSES.md](DEPENDENCY_LICENSES.md)）。
- [対応済み] ライセンス監査ドキュメントの数字を定義別に整理: 実行時和集合 419 / `cargo deny list` 全行 800 / 取得ライセンス本文 452。旧スナップショットは明示して現在値と混同しないようにした（[DEPENDENCY_LICENSES.md](DEPENDENCY_LICENSES.md)）。

### 3.9 将来検討（必要性・方式未決定）

- Arweave、追加ゲートウェイ、Cloudflare Workers/R2、OpenTofu 等の必要性・方式（[DETAILED_DESIGN.md](DETAILED_DESIGN.md)）。
- Rust優先方針に沿う技術例外記録、性能・軽量性・費用の計測値（[DETAILED_DESIGN.md](DETAILED_DESIGN.md)）。

## 4. テスト・検証ギャップ

[TEST_SPECIFICATION.md](TEST_SPECIFICATION.md) および設計文書から抽出。

| 領域 | 未実施/不足の例 |
| --- | --- |
| 認証 | 実JPKI/OID4VP・失効・期限切れの実機検証、`acr`/`amr`/`auth_time`/`max_age`、`sub`全失効の永続化/多プロセス/実OP連携検証、リフレッシュローテーション/盗用検知 |
| 決済 | 実チェーンRPC、再編成・二重通知・別コントラクト、クォーラム/ワーカー/pause監視、実送金証明 |
| モデレーション | 実運用手続（BAN/ピン解除の監査・権限）、ワーカー停止/再起動時の挙動（並行投票の競合は検証済み） |
| 監査 | AUDIT-01（改変・欠落・順序入替の独立検証）— ローカル検証コマンドに連番・Merkle root・`--expect-root` を追加済み（欠落/切詰め検出は実測済み）。署名・外部アンカー・公開範囲・実運用試験は未実施（DEC-04） |
| 互換性/性能 | COMPAT-01/02, PERF-01 の環境・閾値未決定、Wasm/JXL非対応時フォールバック |
| ネットワーク/運用 | 実プロキシ/多段/WAF、ログ集約・アラート、drain/readiness |
| DB | ロールバック、並行起動（段階的アップグレードと欠落マイグレーション検出は検証済み） |
| IPFS/アップロード | 実ゲートウェイ遅延・切断・Pinata連携、厳密CID検証、完全JXLデコード・デコード前検査、実multipart境界・実鍵署名 |
| 表示/E2E | Markdown のブラウザ別URL解釈（CONTENT-04）、モデレーション期限・異議のE2E、OIDC/決済/RS256経路のE2E拡張 |
| テスト基盤 | [対応済み 22dc270] `server/src/tests.rs` の環境変数変更テストを async mutex で直列化（`unsafe` env操作自体は std API 上必要） |

## 5. CI・開発体験の未処理/先送り（REVIEW_REPORT由来）

- [対応済み] Playwright E2E の起動失敗: `server` クレートに `verify_audit_log` bin があり `cargo run` が曖昧になっていた。`default-run = "server"` を設定して解消（ローカル E2E 9件成功）。
- [外部要因] 2026-11-16 時点で GitHub Actions のジョブが請求/上限により開始されない。CI 再開までローカル検証ループ（fmt/clippy/test/deny/wasm/E2E）を代替ゲートとする。

- [対応済み] E2E 失敗時アーカイブ: `ci-e2e.sh` が `e2e/test-results`（trace/スクリーンショット/error-context）と `e2e/playwright-report`（HTML）をタイムスタンプ付き tar.gz へ保存しパスを表示。`E2E_ARTIFACT_DIR` で出力先を変更可。アーカイブ内容を実確認（[REVIEW_REPORT.md](REVIEW_REPORT.md)）。
- [中] CI 側の artifact アップロードとチェック結果の分離（共通ワークフロー拡張が必要）。
- [対応済み] CI E2E スクリプトのローカル再現: trunk 0.21.14 固定版の `NO_COLOR=1` 解釈エラーを `unset NO_COLOR` で回避。8080 占有時に備え `E2E_CLIENT_PORT` 既定値 18080 を設定。`bash e2e/ci-e2e.sh` が9件成功（CI と同一ゲート）。
- [対応済み] E2E 依存バージョン検証: `ci-e2e.sh` が `MIN_NODE_MAJOR`（既定18、`package.json` の engines と整合）と `MIN_CHROME_MAJOR`（既定120）を検査し、下回れば失敗。拒否ケース（999指定）と通常実行（9件成功）を確認。
- [対応済み] E2E 依存バージョンの固定: `package-lock.json` が `@playwright/test` 1.60.0 等を固定し `npm ci` が厳密に再現。`ci-e2e.sh` の下限検査と併せて環境ばらつきを抑制。
- [低] Node/Chrome の上限バージョン検証（現行は下限のみ）。
- [対応済み] タイムアウト見直しの判断材料: ローカル全ゲート（fmt/clippy/test/wasm/denies/E2E）はホットキャッシュで約64秒（2026-11-17 実測、`/usr/bin/time`）。CI の `timeout_minutes: 30` はコールドキャッシュ（依存ビルド・`cargo install cargo-deny`・Chrome 導入）を含むため現状維持と判断。コールド実測が取れた時点で再検討。
- [低] [一部対応 b6e63d3] CIスクリプト [e2e/ci-e2e.sh](../e2e/ci-e2e.sh) は一時DBを使用。ローカルでサーバーを新規起動する E2E 実行（`E2E_REUSE=0`、非CI）では [e2e/playwright.config.ts](../e2e/playwright.config.ts) が未指定時に一時DBを既定使用（`DATABASE_URL` 指定時は尊重）。既存プロセス/DBを再利用するローカル実行の分離方針は必要に応じて検討。

## 6. ドキュメント・軽微な整合性（確認候補）

- [対応済み・本変更で反映] [HANDOFF.md](HANDOFF.md) の「6. 主要ファイル」に `server/src/jwt.rs` を追記。
- [対応済み・本変更で反映] [DETAILED_DESIGN.md](DETAILED_DESIGN.md) のマイグレーション現行記載に `003_payment_order_index.sql` を追記。
- [対応済み・本変更で反映] DEC-02採用方針（有効設定値の常時公開・`GET /policy/age`・年齢ポリシー設定項目・制約）を [AGE_VALIDATION_POLICY.md](AGE_VALIDATION_POLICY.md), [DOCUMENT_STATUS.md](DOCUMENT_STATUS.md), [DETAILED_DESIGN.md](DETAILED_DESIGN.md), [OPERATIONAL_POLICY.md](OPERATIONAL_POLICY.md), [TEST_SPECIFICATION.md](TEST_SPECIFICATION.md), [HANDOFF.md](HANDOFF.md), [REQUIREMENTS_DEFINITION.md](REQUIREMENTS_DEFINITION.md), [REQUIREMENTS_SPECIFICATION.md](REQUIREMENTS_SPECIFICATION.md), [README.md](../README.md) に反映（法的確定/専門家レビューは未完）。
- [対応済み 3662145] `GET /policy/age` が非秘密の公開設定のみを返すことを検証する回帰テストを追加（既知の秘密識別子を含まないことの防御的確認を含む）。
- [対応済み] `docs/index.md` の索引を全文書（Runbook 含む）へ反映。
- [対応済み] [SECURITY_ADVISORIES.md](SECURITY_ADVISORIES.md) に `verified` メタデータを追加し、11件の受容を再確認（2026-11-17）。

## 7. 実施の原則（安全第一）

- 外部事実（JPYCコントラクト/チェーン/確認数、法令、事業者仕様）は一次ソースで再確認し、出典不明のまま断定しない。
- 秘密情報（`JWT_SECRET`, `JWT_RSA_PRIVATE_KEY_PEM`, `OIDC_CLIENT_SECRET`, `PAYMENT_RECEIVING_XPRV`, `PINATA_JWT` 等）を commit・ログ・応答・配布物に含めない。
- 本番設定で `ALLOW_MOCK_AUTH` / `EXPOSE_INTERNAL_ERRORS` / `ALLOW_IPV4_LOCALHOST` を有効化しない。
- 依存追加や破壊的変更は事前確認を取り、標準検証ループ（fmt/clippy/test/deny/wasm/E2E）で確認する。
