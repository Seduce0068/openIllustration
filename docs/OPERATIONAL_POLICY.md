---
title: "運用ポリシー (Operational Policy)"
type: Policy
description: 認証、決済、監査、コンテンツの運用方針と本番化条件を定める。
tags: [operations, security, payments]
generated:
  by: dsh
  at: "2026-11-21T21:33:39+09:00"
---

## 運用ポリシー (Operational Policy)

本書は必須方針と現行プロトタイプの運用制約を定める。
設計決定は [ADR-0001](adr/ADR-0001-mvp-trust-boundaries.md) を参照する。

## 1. 認証・環境分離

- `ALLOW_MOCK_AUTH=1` はローカル開発・隔離テスト専用。本番環境では設定しない。
- `JWT_SECRET` は必須。32バイト未満は起動時に拒否する。十分な強度のランダム値を秘密管理基盤から注入し、ソース・ログ・配布物に含めない。
- `JWT_RSA_PRIVATE_KEY_PEM` を設定すると RS256 で署名する。検証用の公開鍵は
  秘密鍵から導出し、別途設定させない。許可アルゴリズムは起動時に確定し、
  トークンのヘッダから選ばせない。RS256 の秘密鍵演算は rsa 0.10 の
  定数時間実装を使い、0.9 へ戻さない。検証は
  [脆弱性トリアージ](SECURITY_ADVISORIES.md) を参照する。
- `PINATA_JWT` 未設定時のモック動作は開発用途に限る。本番でモックCIDを保存しない。
- 本番起動時に必須設定の存在・形式・相互整合性を検査し、不正・不足設定ではfail closedで起動を拒否する。現行の設定確認だけでは本番要件を満たさない。
- JWT署名鍵、JPKI信頼情報、JPYC運用鍵等の保管者、最小権限、ローテーション、漏えい時失効・再発行手順を定める。秘密をログや監査公開データへ出さない。
- 年齢/地域ポリシーの有効設定値（`AGE_*`）は非秘密情報として `GET /policy/age` で常時公開する。公開対象に秘密情報（`JWT_SECRET`, `JWT_RSA_PRIVATE_KEY_PEM`,
  `OIDC_CLIENT_SECRET`, `PAYMENT_RECEIVING_XPRV`, `PINATA_JWT`, DBトークン等）を含めない。ポリシー設定変更は運用者の責任で管理し、適用法令・専門家レビューの根拠を記録する。
- `AGE_REQUIRE_EXPLICIT_CONFIGURATION=1` で必須設定値が未充足（`AGE_DEFAULT_MINIMUM=21` かつ
  `AGE_COUNTRY_OVERRIDES` / `AGE_TARGET_REGIONS` が空）のときは、
  `/auth/verify` だけでなく `/auth/oidc/callback` と `/auth/refresh` も共通ゲート
  `ensure_age_policy_configured` が `ConfigurationRequired` を 403 にして全経路 fail-closed とする。
  充足判定は `AgePolicy::configuration_satisfied()` が実効値から導出し、旧手動フラグ
  `AGE_POLICY_CONFIGURED` は読取を廃止したため通過条件に使わない。
  `AGE_ENFORCEMENT_MODE=trusted_attribute_required` も同様に `TrustedAttributeRequired` を 403 にする。
  設定ゲートは state / Cookie の消費より前に適用し、年齢数値判定は `/auth/verify` に委ねる。
- 指定マイナアプリ/JPKI認証を本番利用資格の必須条件とする。未認証者には利用させない。
- 実JPKI/マイナアプリの信頼検証が未完成の現行プロトタイプは、本番利用資格判定に使わない。
- ログアウト時は `sid` とリフレッシュファミリの失効に加え、`/auth/logout` が
  `refresh_token=; Path=/auth; HttpOnly; SameSite=Strict; Max-Age=0` の `Set-Cookie` を返すことを確認する。
  DB 側のファミリ失効だけではブラウザに 30 日間有効な refresh_token Cookie が残るため、
  削除 Cookie の配信がログアウト完了の条件。`Secure` は `OIDC_COOKIE_SECURE` に追随し、
  未設定なら付与する（本番では HTTPS 配信で Secure を維持する）。
- OIDC認証の未完了 `pending`（`state` / `nonce` / `code_verifier`）は10分TTLとし、保持数はコード定数
  `MAX_PENDING_AUTH = 1024` を上限とする。登録時・取出時に期限切れエントリを一括回収し、
  上限到達時は `GET /auth/oidc/start` が 429 を返して新規エントリを追加しない（R1-23）。
  未認証で呼べる開始エンドポイントからのメモリ枯渇を防ぐ境界であり、環境変数ではない。
  回収は認証リクエスト時だけに限らず、`run_moderation_finalizer` が起動時と周期 tick
  （`MODERATION_FINALIZE_INTERVAL_SECS`）にも `sweep_expired_pending` を呼ぶため、
  認証リクエストが来ないアイドル中も期限切れ `pending` は蓄積しない。

## 2. コンテンツ・個人データ

- 投稿バイト列、CID、チェックサム、ユーザーハッシュ、公開鍵、通報・投票記録を取り扱う。
- ユーザーハッシュは匿名化を保証しない。アクセス制御、保持期間、削除・訂正手続、バックアップ消去方針を本番前に定める。
- IPFSへの投稿は外部から取得可能になり得る。削除要請・違法コンテンツ対応・ピン解除の運用を整備するまで、永続公開を前提としたデータを安易に登録しない。
- アップロード形式・10 MiB上限・外部取得10 MiB上限を守り、コンテンツ内容の合法性をハッシュ検証で保証しない。
- `GET /me/export` は認証必須（`AuthClaims`）。`version:1` / `generated_at` / `sub` と12表
  （`users` / `contents` / `favorites` / `transactions` / `signatures` / `content_audit_log` /
  `reports` / `votes` / `payment_orders` / `governance_audit_log` / `refresh_tokens` /
  `revoked_subs`）の全行を返し、行数上限・`truncated` フラグは設けない。
  `token_hash` / `sid` / `family_id` / 署名鍵 / XPRV / `PINATA_JWT` / 他ユーザーID は除外し、
  `reason` / `details` はマスクする。未認証は401で拒否する。削除権（`/me/erase`）は未実装。

## 3. 監査・モデレーション

- MVPではDBを正本とする。投票期間は通報から7日とし、期限到達後に集計する。定足数3票未達なら通報を棄却して `Approved` に復帰し、3票以上なら多数決（同数は `Rejected`、ADR-0004、
  公開継続リスクを避ける安全側判断）で確定する。期限前の即時確定は行わない。
- 期限到達はバックグラウンドワーカーが自動検知して確定する（周期は `MODERATION_FINALIZE_INTERVAL_SECS`、既定300秒、範囲30〜3600秒）。手動の
  `POST /mod/reports/{report_id}/finalize` も可能で、両者は冪等。複数インスタンス運用時は同じDBに対する simultaneous finalize を避けるため、
  現状は単一ライター運用を前提とする。
- 運用者はプロジェクト側で固定せず、各デプロイの所有者（個人・企業いずれでも可）が `MODERATION_OPERATOR_HASHES` に JPKI 由来の user_hash を設定する。未設定なら
  BAN/UNBAN/UNPIN はすべて拒否される。本番では実 JPKI 由来の hash のみ設定し、`ALLOW_MOCK_AUTH=1` と併用しない。
- BAN は根拠を必須とし、`duration_days` で期間（1〜3650日、省略/null は無期限）を指定する。期限は `users.ban_until` へ SQLite
  `datetime()` 形式の TEXT で保存し、UNBAN で NULL に戻す。期限切れの BAN は判定上のみ無効化され、`is_banned` フラグは履歴として
  TRUE のまま残す（自動でフラグを落とさない）。UNBAN・UNPIN とともに主体・対象・理由を `governance_audit_log` に記録する。
  これは運用ログであり第三者検証可能な監査ではない。
  投票者選出・Sybil耐性と監査方式（DEC-04）は未実装・未決定であり、モデレーション結果を本番の最終判断として扱わない。
- DB内監査ハッシュは現状の変更検知補助であり、必須要件の第三者検証可能な監査を満たさない。
- 認証済みユーザーは `GET /mod/reports`（通報対象 CID・要約済み理由・状態）と `GET /transactions`（sender/receiver hash・金額・tx_hash）を参照できる。
  通報一覧は認証必須・Reported 限定・新着順を維持し、運用者限定にはしない。理由は `common::summarize_reason` で要約して返す。
  要約は空文字をそのまま返し、`@` を含むトークン / `://` を含むトークン / `www.` で始まるトークン /
  IPv4 らしい部分列 / 全角を含む Unicode 数字4連続を `[redacted]` に置換する。
  置換後に最大120 Unicode 文字（超過は先頭119文字+`…`）。
  DB の全文と監査エクスポートの本文は変更せず、要約は API レスポンス時のみ適用する。
  これらはデプロイ内の参加者向けの閲覧機能であり、第三者への公開範囲（DEC-04 未決定）を意図するものではない。本番公開前に公開範囲を確定する。
- `GET /content/{cid}/audit`（監査タイムライン）の `reason` も `common::summarize_reason` で
  要約して返す。要約規則は `GET /mod/reports` と同じ（PII トークンを `[redacted]` に置換し、
  最大120 Unicode 文字）。**DB の `content_audit_log.reason` は全文のまま保持し、
  ハッシュ検証に使う `entry_hash` は DB の生値をそのまま返す**。マスクは API レスポンス時のみで、
  マスク後の文字列からハッシュを再計算したり DB を書き換えたりしない
  （`test_get_content_audit_masks_reason_but_preserves_entry_hash` で回帰）。
  これも認証済み参加者向けの表示であり、第三者公開（DEC-04 未決定）を意図するものではない。
- `GET /mod/stats` は運用者限定の集計。`AuthClaims` 必須かつ `require_moderation_operator` で認可し、
  未認証は401、`MODERATION_OPERATOR_HASHES` 未設定・未登録・BAN済みの主体は403を返す。
  `generated_at` / `totals` / `monthly` のホワイトリスト項目のみ返し、CID・reason・`user_hash` は露出しない。
  これは認証内のデプロイ向け集計であり、第三者への公開を意図するものではない（DEC-04 未決定）。
- ローカルDBの `content_audit_log` ハッシュ連鎖は
  `cargo run -p server --bin verify_audit_log` に `--database-url` / `--cid` /
  `--expect-root` の各オプションを付けて読み取り専用に検証できる。
  リモート DB のトークンは `DATABASE_TOKEN` 環境変数からのみ渡し、`--database-token` は受け付けない。
  連鎖に加え `log_id` の連続性と entry_hash 列の Merkle root を出力し、別経路で保存した root を `--expect-root`
  に渡すと末尾切り詰め・欠落を検出できる（実測で検出確認済み）。エントリハッシュは v2（長さプレフィックス付き正規化）を生成し、v1（既存互換）も検証できる。これは計算整合性の確認であり、
  第三者検証可能な監査方式・公開範囲・運用の代替ではない。root の外部保存・公開は各デプロイの運用者が行う。
  `scripts/backup-rotate.sh` は `AUDIT_EXPECT_ROOT` が設定されていれば同じ root を
  `verify_audit_log --expect-root` へ自動で渡し、取得直後の検証で空化・末尾切詰めを検出する。
  本番では `AUDIT_EXPECT_ROOT` の設定を必須とする。未設定時は内部整合性チェックのみを行い
  （既存運用を壊さない）、verify_audit_log / verify_audit_export の双方で root 値はエラー文言へエコーしない。
   `doctor` は本番相当（`ALLOW_MOCK_AUTH` 無効）で未設定・空白のみ・不正 hex なら
   `audit_expect_root` カテゴリに固定文言の警告を出し、値はエコーしない（既定は警告のみ、
   `--strict` のときだけ失敗）。共有の起動判定 `production_config_errors` は変更しない。
- 監査スナップショットは `sign_audit_export` で署名し、`verify_audit_export` で公開鍵検証する（[AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md)
  ）。秘密鍵はファイルで厳重管理し、CLI 引数や環境変数へ内容を渡さない。
  `sign_audit_export` は起動時に署名鍵 PEM を `canonicalize` して権限を検査し、owner-only
  （`0600` / `0400`）を要求する。親ディレクトリが group/other 書き込み可能なら拒否し、
  `0700` でなければ警告する。リモート libSQL のトークンは `DATABASE_TOKEN` 環境変数からのみ
  読み、`--database-token` は受け付けない（`argv` 露出対策）。出力先は `create_new` で原子的に
  生成し、既存ファイルの上書きを拒否する。手順は [AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md)。
- 監査 CLI の権限・トークン・上書き拒否は実装済み。`sign_audit_export` に自動テスト17件を追加済み
  （TEST_SPECIFICATION AUDIT-02）。Runbook の手動確認（鍵権限・親ディレクトリ権限・
  `DATABASE_TOKEN` 設定・既存出力の有無）は補助的な確認として扱う。
- 本番前に署名鍵の HSM 管理とローテーション、監査対象・公開範囲、外部アンカー、個人情報保護、障害時運用を定めて試験する。

## 4. 支援・決済

- プラットフォーム内の有償取引・支援にはJPYCのみを必須の決済手段とする。他方式は提供しない。
- 閲覧のみの利用者にJPYC保有を求める方針ではない。
- 対応チェーン、トークンコントラクト、送金先変更、取引確認・最終性、手数料、返金・紛争の各運用を決定する。
- 模擬支援APIは501で無効化済み。取引記録は実決済の証明ではない。JPYC検証の実装時に決済APIへ置換し、模擬処理は再導入しない。
- ウォレット所有証明（bind）の署名対象は `wallet_bind_message_v2` により、注文IDに加えて
  `chain_id` / `wallet_address` / `recipient_address` / `amount_jpy` を含むものとする。
  注文IDのみを署名対象にする旧形式は、送金先・金額・チェーンを確認できない盲署名リスクがあるため
  受理しない（401拒否）。金額は DB の `amount_wei` から円建てへ導出し、アドレスは小文字化して照合する。

## 5. 運用準備・未整備事項

- 本番用のオリジンを定義し、CORSを環境設定する。ローカル開発オリジンを本番許可設定に流用しない。
  クライアントの API オリジンはビルド時に `OIL_API_BASE` で注入する。値はオリジンのみ
  （`<scheme>://<host>[:<port>]`、末尾 `/` は除去）とし、パス・クエリ・認証情報・機微情報は入れない。
  未設定時は開発用の `http://localhost:3000`。HTTPS 配信では HTTPS オリジンを指定し mixed content を避ける。
- 全レスポンスへ `X-Content-Type-Options: nosniff` と `X-Frame-Options: DENY` を付与する。
  `security_headers` は `CorsLayer` の内側に加えてさらに外側にも置く。CORS プリフライト（OPTIONS 204）は
  `CorsLayer` で短絡して内側を通らないため、外側の層が無いとヘッダーが落ちる。
  429 / 403 / 500 / 204 とプリフライト応答を含む全経路でヘッダーが付くことをゲートで確認する。
- 同時リクエスト上限 `MAX_CONCURRENT_REQUESTS`（既定16・範囲1〜256・超過は429）は、IP 単位の
  レート制限とは別の同時実行境界である。permit はレスポンス完了時に解放され、長時間ハンドラが
  permit を保持している間は後続リクエストが 429 になる。値を下げるとバースト時の 429 が増え、
  上げると DB や外部依存への同時負荷が増えるため、本番ではキャパシティ試験の結果で設定する。
  範囲外の値はクランプせず既定16へ倒れ、`doctor --strict` が設定不整合として警告する。
- DBスキーマは起動時にバージョン管理マイグレーションで適用する（`_migrations` にchecksumを記録し、1ファイル1トランザクションで適用、前方のみでロールバックは提供しない）。失敗時は起動を中止し、
  失敗ファイルは適用されない。復旧は原因修正後の再起動とし、`_migrations` の記録を確認する。適用済みバージョンに対応するファイルが無い場合（古いイメージでの起動等）、
  およびマイグレーションディレクトリ自体が存在しない場合も起動を拒否する（fail-closed、ディレクトリ欠落で欠落検査をすり抜けるのを防止）。単一プロセス内の書き込みトランザクションは接続ごとのロックで直列化する（libSQL
  は同一接続で同時に1つの `BEGIN` しか保持できない）。ローカル DB では起動時に `<DB path>.instance.lock` を原子的に作成し、生存 PID
  のロックがある場合は第二インスタンスの起動を拒否する（fail-closed）。クラッシュ後の残骸（死んだ PID）は自動回収し、停止時にロックを削除する。リモート DB ではこの排他が使えないため警告のみとし、
  単一ライター運用を契約とする。バックアップは `backup_db`（`VACUUM INTO`・上書き拒否）で取得し、`verify_backup` と `verify_audit_log` で検証する。手順は
  [BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md) を参照。自動化・オフサイト保存・実運用での復旧訓練は本番前に整備する。
- バックアップ運用では、サーバー稼働中は `backup_db` が拒否される。ソース DB（ローカルの場合）と
  出力先 DB の `<db>.instance.lock` を検査し、生存サーバーが保持していれば `refusing backup`
  で失敗する（判定不能時も拒否、stale ロックのみ許可、fail-closed）。cron / systemd の定期実行は
  サーバー停止中（メンテナンス窓）に行う。`scripts/backup-rotate.sh` はサーバーを停止せず既定どおり
  `backup_db` を呼ぶため、稼働中に実行すると拒否されて失敗する（取得→検証→ローテーションの既定動作）。
  ロック拒否時の対処は [BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md) を参照。
- `payment_orders.derivation_index` には migration 003 の部分 UNIQUE インデックス
  （`WHERE derivation_index IS NOT NULL`）が適用済みである。固定受取アドレスの注文
  （`derivation_index IS NULL`）は制約対象外とし、xprv 設定時は注文ごとの受取先
  再利用を DB 制約でも拒否する。既存 DB に重複した `derivation_index` がある場合は
  インデックス作成時にマイグレーションが失敗して起動を中止する（fail-closed）。
  黙ってデータを修正せず、運用側の確認を強制するための動作である。
  並行注文対策（R1-26）は新規マイグレーションを追加せず、`create_order` が
  書き込みロックとトランザクション内で採番を直列化することで実現する。
- IPv6のみの制御をプロキシ経由で使う場合、`TRUSTED_PROXY_CIDRS` に信頼済みプロキシの範囲のみを
  設定する。プロキシの `X-Forwarded-For`（XFF）は**上書きを推奨**するが、**追記も可**である。
  追記型（末尾がピア）ではサーバーが末尾のピアを除去して最左のクライアントを採用し、
  上書き型（末尾がピアでない）では末尾を採用する。信頼済みピアで解決できない場合は 403 となる。
  ネットワーク層でも許可方針を実装する。
- ログにJWT、署名、個人情報、投稿本文、Pinataトークンを出さない。現状のエラー応答は500系の内部詳細を
  ログのみに保ち、クライアントへは固定文言 `Internal server error` を返す（開発時は
  `EXPOSE_INTERNAL_ERRORS=1` で詳細を公開）。観測性は `LOG_FORMAT=json` と `/metrics` で提供し、
  アラート通知先は導入環境で選定する。
- クライアント側のエラー表示方針: サイレント失敗を禁止し、HTTP 通信の失敗は必ず `role="alert"` で
  利用者へ告知する。`Login` は `data-testid="login-error"`、エクスポートは `data-testid="export-error"`、
  ContentViewer の監査は `data-testid="audit-error"`、モデレーション集計は
  `data-testid="moderation-stats-error"` を使い、HTTP ステータス・通信失敗・JSON 解析失敗を区別する。
  アップロードは `data-testid="upload-status"` に成否を表示し、準備失敗は
  `アップロードの準備に失敗しました。`、応答読取失敗は `アップロード結果の読み取りに失敗しました。`
  を使う。通信／HTTP 応答は `upload_error_message` の4分岐で固定文言を選ぶ:
  401=再ログイン（`再ログインが必要です。`）、413=サイズ超過（`ERROR: File exceeds the 10 MiB limit.`）、
  HTTP その他=`Failed to upload content (HTTP {status}).`、None（ステータス取得不可）=
  `Network error uploading content.`。サーバー本文・生エラーは表示しない。いずれの結果でも
  `loading` を解除してボタンを再活性化する。
  JXL デコード失敗は `content-verification-badge` を `× DECODE FAILED` とし、
  `data-testid="jxl-decode-error"` に固定文言を表示する（デコーダのエラー値は出さない）。
  Illustration ではデコード成功時のみ `● VERIFIED` を表示し、サーバー側の完全デコード検証は未実装である。
  成功時にエラー表示をクリアする。内部例外・スタックトレース・署名やトークン等の内部詳細は画面へ出さない。
- 起動時に設定を検証し、ヘルスチェック、メトリクス、アラート、障害対応・鍵ローテーション手順を整備する。グレースフル停止（SIGTERM/SIGINTで処理中リクエストを完了待ち）は実装済み。drain は
  `GRACEFUL_DRAIN_TIMEOUT_SECS`（既定 30 秒、1〜300 秒）を上限とし、超過時は警告を出して残接続を切断する。
- 本番起動前の設定診断は `cargo run -p server --bin doctor -- [--strict]` で行う。
  既定は問題を警告するのみで終了コード0、`--strict` を付けたときのみ問題があれば終了コード1とする。
  秘密情報の値は一切出力しない。本番必須設定の判定は既存の fail-closed ロジックへ委譲する。
   `doctor --strict` の警告対象には `ALLOW_IPV4_LOCALHOST`（有効時）と `TRUSTED_PROXY_CIDRS`
   （設定時）も含まれる。いずれも設定値は出力せず有無のみを警告する。前者は本番で無効化し、
   後者は信頼済みプロキシ範囲のみを設定して意図を確認する（意図的な利用なら警告は許容）。
- `production` カテゴリはバックアップ設定の整合も診断する（本番相当・`ALLOW_MOCK_AUTH` 無効時のみ）。
   `BACKUP_GPG_RECIPIENT` 未設定、`OFFSITE_COPY_CMD` / `OFFSITE_DEST` の片方だけ設定、
   `ALERT_CMD` に実行不可のパスを指定した場合に固定文言で警告し、設定値は出力しない。
- `age_policy` カテゴリは年齢判定の基準日（`TZ`）も診断する。`TZ` 未設定で UTC へ
   フォールバックする場合、`AGE_TARGET_REGIONS` に対して基準日のオフセットが不整合な場合に
   固定文言で警告し、設定値は出力しない。
- 上記2カテゴリの追加項目は既存の23カテゴリ内に含まれ、カテゴリ数は変わらない。
   既定は警告のみで終了コード0、`--strict` のときだけ問題として終了コード1とする。
- **Runbook（本番起動前ローカルゲート）**: 秘密管理基盤から本番相当の環境変数を注入し、
  `cargo run -p server --bin doctor -- --strict` を実行して終了コード0を確認してからデプロイする。
  診断は既存の23カテゴリ固定（`check23`）と `audit_expect_root` / `production` /
  `oidc_endpoint_urls` 等のカテゴリを対象とし、カテゴリ数・判定ロジックは変更しない。
  問題検出時は終了コード1でゲート失敗とし、設定是正後に再実行する。
- liveness は `GET /health`（プロセス生存のみ、固定 200）。readiness は `GET /ready` とし、DB への `SELECT 1` が成功し停止処理中でなければ 200、それ以外は 503
  を返す。停止シグナル受信時は readiness を即 503 にしてから drain する。いずれも IPv6/IP ゲートの対象（バイパスしない）。
- readiness の意味は「このインスタンスが API リクエストを受けられるか」に限定する。IPFS / JPKI / チェーンなどの外部依存が壊れても、影響を受けるエンドポイントは個別にエラーを返し、他機能は継続できるため
  readiness は 200 を維持する（意図的な設計判断）。外部依存の障害検知は `GET /metrics` のエラーカウンタと WARN ログ（IPFS チェックサム不整合等）で行い、アラート通知先へ接続して監視する。
  外部依存を丸ごと停止させる運用が必要になった場合は、ロードバランサー側のヘルスチェックではなく専用の依存チェックを別途設計する。
- ログ集約向けに `LOG_FORMAT=json` を設定すると 1行1 JSON（timestamp / level / fields / target）で出力する。実サーバーで全行が JSON としてパースできることを確認済み。
  アラート通知先と集約基盤の選定は導入環境に委ねる。
- `GET /metrics` が Prometheus 形式の `openillustration_http_requests_total` と `openillustration_http_errors_total`
  （4xx/5xx）を公開する。カウンタはプロセス内（単一ライター前提）で、再起動時にゼロへ戻る。個人情報・リクエストパス・クエリは含めない。
- 中立化URL件数指標: クライアントが Markdown 描画時に危険スキームの URL を中和した件数を
  `POST /metrics/client-events` へ `{"event":"neutralized_url","count":N}`（`event` は固定値、
  `count` は 1〜1000 の整数、1 回の描画につき最大 1 回）で送る。認証不要、成功時は 204
  （本文なし）。URL 本文・IP アドレス・User-Agent・認証情報はペイロードへ含めず、
  サーバーも受信・記録・ログ出力しない（監視用の `/metrics` とは異なり、IP ゲートと
  既存レート制限の適用対象）。
  `GET /metrics` は `openillustration_neutralized_urls_total`（Prometheus counter、
  送信された count の累積値）を公開する。カウンタはプロセス内で再起動時にゼロへ戻る。
  このカウンタは参考値であり、アラート判定や請求等の意思決定に単独で使わない。
  エンドポイントは認証不要のため `count=1000` の連投で指標を容易に汚染でき、
  累積値は実際の被害や不正の規模を保証しない。判断には認証済みデータや
  サーバー側の独立した記録を突き合わせる。

## 6. 本番移行ゲート

本番移行には、実JPKI認証の独立検証、JPYC取引確認と手数料・失敗時運用、第三者検証可能な監査、プライバシー・法務レビュー、モデレーション承認、IPFS削除/通報対応、DB復旧訓練、環境別設定とセキュリティ試験を必要とする。
本書の記載だけで法令適合を保証しない。

## 7. 環境変数リファレンス（サーバー＋運用スクリプトが読む環境変数）

サーバーと運用スクリプトが読む環境変数の一覧。本番では開発専用の項目を設定しない。

| 変数 | 必須 | 既定 | 説明 |
| ---- | ---- | ---- | ---- |
| `JWT_SECRET` | 必須 | なし | HS256署名鍵。未設定・32バイト未満は起動を拒否する。秘密管理基盤から注入する。`JWT_RSA_PRIVATE_KEY_PEM` 設定時は使われない。 |
| `JWT_RSA_PRIVATE_KEY_PEM` | 任意 | なし | PKCS#8 PEM の RSA 秘密鍵。設定時は RS256 で署名する。署名ヘッダには公開鍵由来の `kid` を付与する。2048bit 未満・不正な PEM は、`JWT_SECRET` が有効でも黙ってフォールバックせず起動を拒否する。秘密管理基盤から注入し、ログ・配布物へ出さない。 |
| `JWT_REQUIRE_KID` | 本番推奨 | 0 | `1` 設定時は `kid` 付きトークンのみ受理し、`kid` 無しレガシートークンを拒否（fail-closed）。RS256 限定。HS256 と併用すると起動拒否。 |
| `JWT_RSA_VERIFICATION_KEYS` | 任意 | なし | RS256 のみ有効。`kid` → SPKI 公開鍵 PEM（`BEGIN PUBLIC KEY`）の JSON オブジェクト。旧鍵を検証用に保持して無停止ローテーションを行う。未知 `kid` は拒否（fail-closed）。`kid` 無しの旧トークンは現行署名鍵で検証する。手順は [KEY_ROTATION_RUNBOOK.md](KEY_ROTATION_RUNBOOK.md)。 |
| `DATABASE_URL` | DB 接続3 CLI では必須（サーバーは任意） | なし（CLI）/ `file:local.db`（サーバー） | libSQL 接続先。DB 接続3 CLI は未設定時に起動拒否（下記注記を参照）。 |
| `DATABASE_TOKEN` | 任意 | 空 | リモートlibSQLの認証トークン。`sign_audit_export` は環境変数からのみ読み、`--database-token` はエラーとなる（argv 露出対策）。`verify_audit_log` / `backup_db` / `restore_db` も含め全 CLI で `--database-token` は拒否し、`DATABASE_TOKEN` 環境変数のみを使用する。 |
| `ALLOWED_ORIGINS` | 本番では必須 | 開発用オリジン3件（`ALLOW_MOCK_AUTH=1` のときのみ） | CORS許可オリジン（カンマ区切り、`http(s)://` または `null`、末尾スラッシュは除去）。モック認証無効時に未設定なら全オリジン拒否（fail-closed）。 |
| `IPFS_MAX_ATTEMPTS` | 任意 | 1 | ネットワークエラー時に各 IPFS ゲートウェイを再試行する回数（1〜5、範囲外は1）。HTTP エラー・サイズ超過・チェックサム不一致は再試行しない。 |
| `OIDC_REQUIRED_ACR` | 本番では必須（開発は任意） | 未設定 | 要求する ID トークンの `acr`（カンマ区切り、いずれか一致）。設定時にクレーム欠落/不一致なら認証拒否（fail-closed）。 |
| `OIDC_MAX_AUTH_AGE_SECS` | 本番では必須（開発は任意） | 未設定 | 認証からの最大経過時間（秒、正の整数）。設定時は認証要求へ `max_age` を付与し ID トークンの `auth_time` を検証。未設定は `auth_time` / `max_age` 検証を行わず、不正値（0・負・非数値・オーバーフロー）は起動拒否。 |
| `OIDC_REQUIRED_AMR` | 本番では必須（開発は任意） | 未設定 | 要求する ID トークンの `amr`（カンマ区切り、すべて含まれること）。設定時にクレーム欠落/不一致なら認証拒否（fail-closed）。 |
| `TRUSTED_PROXY_CIDRS` | 任意 | 未設定 | 信頼済みリバースプロキシの IP / CIDR（カンマ区切り、単一ホップ前提）。設定時のみ、範囲内のピアから届いた `X-Forwarded-For`（XFF）を解釈し、追記型（末尾がピア）は末尾のピアを除去して最左のクライアントを、上書き型（末尾がピアでない）は末尾をクライアントIPとして採用する。不正エントリ・解決不能は 403（fail-closed）。全アドレスを含む範囲（プレフィックス長 0）と不正な要素は起動時に警告のうえ無視する（範囲は IPv4 / IPv6 の正規形で記載）。未設定時は本ヘッダーを判定に使わない。`doctor --strict` の警告対象。 |
| `RATE_LIMIT_PER_MIN` | 任意 | `60` | IP 単位のリクエスト上限（1〜600）。本番相当（`ALLOW_MOCK_AUTH` 無効）のみ適用。超出は 429。 |
| `MAX_CONCURRENT_REQUESTS` | 任意 | `16` | 同時実行リクエスト上限（1〜256）。超過は 429。`/health`・`/ready`・`/metrics` は対象外。 |
| `IPFS_GATEWAYS` | 任意 | 既定3件 | 取得に使う IPFS ゲートウェイの URL テンプレート（カンマ区切り）。各要素は `{cid}` を含む http(s) URL であること。空要素、`{cid}` を含まない要素、http(s) 以外の要素は警告のうえ無視する。CIDは英数字のみを許可し、パス・クエリ・フラグメント注入を拒否する。空にすると取得は常に失敗する。 |
| `IPFS_TIMEOUT_SECS` | 任意 | `15` | 各ゲートウェイへのリクエストタイムアウト秒（1〜300）。範囲外・不正値は既定値。 |
| `GRACEFUL_DRAIN_TIMEOUT_SECS` | 任意 | `30` | グレースフル停止の drain タイムアウト秒（1〜300）。範囲外・不正値は既定30へフォールバックし `doctor` が警告（値は出力しない）。 |
| `MODERATION_FINALIZE_INTERVAL_SECS` | 任意 | `300` | 自動確定ワーカーの周期（秒、30〜3600）。範囲外・不正値は既定300へフォールバックし `doctor` が警告（値は出力しない）。 |
| `AGE_DEFAULT_MINIMUM` | 任意（非秘密） | `21` | 年齢判定の一律既定最少年齢。保守的安全余裕ベースライン（法的事実の確定主張ではない）。有効設定値は `GET /policy/age` で常時公開。 |
| `AGE_COUNTRY_OVERRIDES` | 任意（非秘密） | 空 | 国別しきい値上書き（`CC=NN,...`）。 |
| `AGE_TARGET_REGIONS` | 任意（非秘密） | 空 | 対象地域コード（カンマ区切り）。 |
| `AGE_ENFORCEMENT_MODE` | 任意（非秘密） | `self_reported` | `self_reported` 系または `trusted_attribute_required` 系。後者は現時点でフェイルクローズ。 |
| `AGE_REQUIRE_EXPLICIT_CONFIGURATION` | 任意（非秘密） | `false` | 明示設定未完了時に年齢検証を拒否するフェイルクローズ選択肢。 |
| `AGE_POLICY_CONFIGURED`（廃止） | — | — | 旧手動フラグ。読取を廃止し設定されても無視される。充足判定は `AGE_DEFAULT_MINIMUM` / `AGE_COUNTRY_OVERRIDES` / `AGE_TARGET_REGIONS` の実効値から導出（`AgePolicy::configuration_satisfied()`）。 |
| `TZ` | 任意 | 未設定（UTC として扱う） | 年齢判定の基準日（ローカルの「今日」）を決める。`Asia/Tokyo` は日本の固定オフセット UTC+9、`Etc/GMT±N` は POSIX 規約に従い符号反転（例: `Etc/GMT-9` = UTC+9）。未設定・未対応名は UTC の日付へフォールバック。JST 深夜では UTC 日付と境界が 1 日ずれ得るため、対象地域に合わせて明示設定する（[AGE_VALIDATION_POLICY.md](AGE_VALIDATION_POLICY.md) 参照）。 |
| `PAYMENT_RECEIVING_ADDRESS` | 任意 | 未設定 | JPYC注文の受取アドレス（`0x` + 40 hex）。未設定・不正時は注文作成を501で拒否する。 |
| `PAYMENT_RECEIVING_XPRV` | 任意 | 未設定 | BIP44派生用のxprv。設定時は注文ごとの受取アドレスを派生し、`PAYMENT_RECEIVING_ADDRESS` より優先する。秘密管理基盤から注入し、ログへ出さない。 |
| `OIDC_ISSUER` | 任意 | 未設定 | デジタル認証サービスの issuer。OIDC を有効化する場合は必須。URL は https が必須（下記 OIDC URL の規約を参照）。 |
| `OIDC_CLIENT_ID` | 任意 | 未設定 | RP のクライアントID。 |
| `OIDC_CLIENT_SECRET` | 任意 | 未設定 | RP のクライアントシークレット。秘密管理基盤から注入する。 |
| `OIDC_REDIRECT_URI` | 任意 | 未設定 | 認可コードのリダイレクトURI。BFF のコールバックを指定する。 |
| `OIDC_AUTHORIZATION_ENDPOINT` | 任意 | 未設定 | 認可エンドポイント。 |
| `OIDC_TOKEN_ENDPOINT` | 任意 | 未設定 | トークンエンドポイント。 |
| `OIDC_JWKS_URI` | 任意 | 未設定 | JWK Set の URL。 |
| `OIDC_HTTP_TIMEOUT_SECS` | 任意 | `10` | OIDC の token/JWKS 取得のタイムアウト（秒、1〜60）。範囲外は既定へフォールバック。 |
| `OIDC_SCOPES` | 任意 | `openid` | 要求スコープ。属性取得時は対象スコープを追加する。 |
| `OIDC_COOKIE_SECURE` | 任意 | 有効 | `0`/`false` で session / refresh / OIDC state Cookie の `Secure` を外す。開発時のみ。本番の state Cookie は `__Host-` プレフィックス（Secure 必須）で保存される。 |
| `OIDC_CLOCK_SKEW_SECS` | 任意 | `60` | IDトークンの時刻検証に許容する秒数（1〜300、範囲外は既定60へフォールバックし `doctor` が警告）。 |
| `PINATA_JWT` | 本番では必須 | `mock_jwt`（模擬、開発用） | IPFSピン用トークン。本番相当（`ALLOW_MOCK_AUTH` 無効）では実トークンが必須。未設定・空・空白のみ・`mock_jwt`（前後空白を含む）は起動拒否（fail-closed）。`doctor` の `production` カテゴリで同じ判定を診断し、値は出力しない。 |
| `RUST_LOG` | 任意 | `info` | ログレベルフィルタ。 |
| `EXPOSE_INTERNAL_ERRORS` | 開発専用 | 無効 | `1`/`true` で500系の内部詳細を応答へ含める。本番では設定しない。 |
| `ALLOW_MOCK_AUTH` | 開発専用 | 無効 | `1`/`true` で模擬JPKI・ゼロ署名を受理する。本番では設定しない。 |
| `ALLOW_IPV4_LOCALHOST` | 開発専用 | 無効 | `1`/`true` で127.0.0.0/8からのアクセスを許可する。本番では設定しない。有効時は `doctor --strict` の警告対象。 |
| `ALLOW_UNENCRYPTED_BACKUP` | 開発専用 | `0` | `1` で平文バックアップを許可する。ローカル開発専用の明示オプトアウト。オフサイト保存設定時は拒否。本番では設定しない。 |
| `BACKUP_GPG_RECIPIENT` | 本番では必須 | 未設定 | GPG 公開鍵のメール/ID。設定時にバックアップを GPG 暗号化する。未設定時は平文作成を拒否（fail-closed）。gpg（または `GPG_BIN`）不在時も失敗する。 |
| `AUDIT_EXPECT_ROOT` | 本番では必須 | 未設定 | 別経路で保存した監査 Merkle root（hex、非機微）。設定時は `backup-rotate.sh` が `verify_audit_log --expect-root` へ渡し、監査ログの空化・末尾切詰めを自動検証で検出する。未設定時は内部整合性チェックのみ（既存運用を壊さない）。verify_audit_log / verify_audit_export の双方で root 値はエラー文言へエコーしない。本番相当（`ALLOW_MOCK_AUTH` 無効）で未設定・空白のみ・不正 hex なら `doctor` の `audit_expect_root` カテゴリで固定文言の警告（値は非エコー、既定は警告のみで `--strict` のときだけ失敗）。 |
| `BACKUP_ENV_FILE` | 任意 | `/etc/openillustration/backup.env` | cron 版は `backup-cron-wrapper.sh` がこのパスを読む。systemd 版はこの変数を使わず、`EnvironmentFile=` の固定パス（同パス）から読む。権限600・所有者固定、秘密情報は配布物へ含めない。 |
| `BACKUP_DIR` | 任意 | リポジトリルートの `backups/` | バックアップ出力先。deploy 設定例では `/var/backups/openillustration`。事前に存在させ、権限を適切に設定する。 |
| `BIN_DIR` | 任意 | リポジトリルートの `target/debug` | `backup_db` / `verify_backup` / `verify_audit_log` のあるディレクトリ。無ければ `cargo run` へフォールバック（開発用）。本番はビルド済みパスを指定。 |
| `ALERT_CMD` | 任意 | 空（無効） | 失敗通知コマンドの実行ファイルパス。設定時は1引数（メッセージ）で呼び出し、通知が失敗しても元の終了コードを維持。成功時は通知しない。実行不可なら起動拒否（fail-closed）。 |
| `OFFSITE_COPY_CMD` | 任意（`OFFSITE_DEST` と対で設定） | 空（無効） | オフサイトコピーコマンドの実行ファイルパス。`<成果物> <宛先>` の2引数で呼び出す（eval 不使用）。実行不可なら起動拒否（fail-closed）。 |
| `OFFSITE_DEST` | 任意（`OFFSITE_COPY_CMD` と対で設定） | 空（無効） | オフサイト保存の宛先。片方だけの設定は起動拒否、コピー失敗時はスクリプト全体を失敗させる（fail-closed）。 |
| `E2E_PROJECTS` | 任意（E2E時） | `chromium` | E2E で実行する Playwright プロジェクト（カンマ区切り）。明示時は指定を厳守し未知値は拒否する。未指定時は firefox / webkit の実行可能性（導入済みバイナリと共有ライブラリが解決できるか）を検出し、両方可能なら3プロジェクトを実行する。片方でも不可なら `chromium` へフォールバックし警告を出す（共有ライブラリ不足環境で CI が壊れるのを防ぐため、既定は `chromium` のまま）。 |

- サーバーは `DATABASE_URL` 未設定時に `file:local.db` を使うが、DB 接続3 CLI
  （`backup_db` / `sign_audit_export` / `verify_audit_log`）は暗黙既定値を撤廃し、
  `--database-url` または `DATABASE_URL` の明示設定がなければ起動を拒否する
  （空・空白のみも未設定として扱い、値はエコーしない / fail-closed）。
  deploy 設定例（[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)）は
  `file:/var/lib/openillustration/local.db` を明示済みで、設定値の変更は不要。
  手動実行でも接続先を明示し、誤った DB を検証して成功と誤信するのを防ぐ。
- OIDC の URL 設定（`OIDC_ISSUER` / `OIDC_REDIRECT_URI` / `OIDC_AUTHORIZATION_ENDPOINT` /
  `OIDC_TOKEN_ENDPOINT` / `OIDC_JWKS_URI`）は https が必須。`ALLOW_MOCK_AUTH` が有効な場合のみ
  `localhost` / `127.0.0.1` / `[::1]` / `::1` の http を許容し、それ以外の http や不正 URL は
  起動拒否（fail-closed）。`doctor` の `oidc_endpoint_urls` カテゴリでも同じ判定で警告し、
  設定値は出力しない。
- `PINATA_JWT` は本番相当では実トークン必須。未設定・空・空白のみ・開発用 `mock_jwt`
  （前後空白を含む）は起動拒否（fail-closed）。`doctor` の `production` カテゴリでも
  同じ判定で警告し、設定値は出力しない（[DETAILED_DESIGN.md](DETAILED_DESIGN.md) と整合）。
- 真偽値はいずれも `1` または `true`（大文字小文字を問わない）で有効になる。
- 年齢/地域ポリシーの有効設定値（`AGE_*` と `GET /policy/age` の応答内容）は非秘密かつ常時公開。新しいポリシー項目を追加する際も秘密情報を含めないこと。
- マイグレーションディレクトリの探索には開発便宜として `CARGO_MANIFEST_DIR` を参照する।
