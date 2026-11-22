---
title: "運用ポリシー (Operational Policy)"
type: Policy
description: 認証、決済、監査、コンテンツの運用方針と本番化条件を定める。
tags: [operations, security, payments]
generated:
  by: dsh
  at: "2026-11-23T04:10:00+09:00"
---

## 運用ポリシー (Operational Policy)

本書は必須方針と現行プロトタイプの運用制約を定める。
設計決定は [ADR-0001](adr/ADR-0001-mvp-trust-boundaries.md) を参照する。

## 1. 認証・環境分離

- `ALLOW_MOCK_AUTH=1` はローカル開発・隔離テスト専用。本番環境では設定しない。
- クライアントのビルドでは本番成果物に `OIL_MOCK_AUTH=0` を指定し、
  ログイン画面の開発用モック認証注記（`data-testid="mock-auth-notice"`）を出さない。
  未設定（既定）は注記を表示する開発用契約で、`"0"` のみが無効化する。`ALLOW_MOCK_AUTH` がサーバー起動時の設定であるのに対し、`OIL_MOCK_AUTH` はクライアントのビルド時設定である。
  
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
- `/auth/refresh` は Cookie を読む前に `Origin` を `ALLOWED_ORIGINS` のいずれかと
  バイト単位で完全一致照合し、不一致は 401 で拒否する。`Sec-Fetch-Site: cross-site` も
  明示拒否するが、`same-origin` / `same-site` / ヘッダ欠損は許可し、
  `app.example.com` → `api.example.com` の same-site cross-origin 契約を維持する。
  `ALLOWED_ORIGINS` に `null` は設定不可（設定しても無視される）。信頼できない
  サブドメインを許可リストへ入れず、ブラウザが送るオリジンのみを列挙する。
- ログアウト時は `sid` とリフレッシュファミリの失効に加え、`/auth/logout` が
  `refresh_token=; Path=/auth; HttpOnly; SameSite=Strict; Max-Age=0` の `Set-Cookie` を返すことを確認する。
  DB 側のファミリ失効だけではブラウザに 30 日間有効な refresh_token Cookie が残るため、
  削除 Cookie の配信がログアウト完了の条件。`Secure` は `OIDC_COOKIE_SECURE` に追随し、
  未設定なら付与する（本番では HTTPS 配信で Secure を維持する）。
  session Cookie は発行しないため削除対象に含まない。JWT は応答本文のみで搬送する。back-channel logout では logout token の `iat` を更新時刻とみなし、
  `revoked_subs.revoked_at` は受け取った `iat` からのみ導出する（サーバー時刻 `Utc::now()` では上書きしない）。
  `iat > now + OIDC_CLOCK_SKEW_SECS` と未来 `nbf` は401で拒否し、nbf クレーム欠落は受理する。
  R2-AUTHZ により logout token は `sub` の存在を必須とする（`sub` 欠落は固定文言 `logout token requires sub` で401）。
  OIDC Back-Channel Logout 仕様上 `sub` は任意クレーム（後続の `sid` セッション失効のみでも仕様適合）だが、
  本システムはリフレッシュトークン失効を `revoked_subs`（`sub` キー）に依存しているため、
  `sid` のみを受理すると refresh 経由で新しいセッション JWT が発行され、ログアウト済みセッションが復活する。
  仕様の任意性より fail-closed を優先し、`sid` のみのログアウト要求を拒否する（却下理由）。
- クライアントの自動 refresh は単一フライトの完了値を無批判に採用しない。`/auth/refresh` の
  `VerifyResponse` は `authorized` / `user_hash` / `token` を契約とし、`user_hash` が
  現在のログインユーザーと不一致なら JWT を採用せず `Failed` として元の 401 を返す。
  refresh 失敗時は api_fetch 専用の `REFRESHED_JWT` をクリアし、失効済み JWT を
  送り続けない。さらに refresh フライトは `claim` 時に epoch を捕捉し、logout/login で
  進めた epoch と `complete` 時に照合して、不一致なら旧 JWT を書き戻さず待機者へ
  `Failed` を配る。いずれもクライアント側の fail-closed 契約であり、サーバーの
  Cookie ファミリ失効や Origin 検査を置き換えるものではない。既存の固定文言・
  data-testid・E2E69件 / check23 / doctor54 / migrations14 の数値は不変。
- 失効判定（`sid` と sub 失効閾値）は DB を正本とし、認証リクエストごとに DB へ照会するため、
  他インスタンスで記録された logout / BAN / backchannel logout が即時に反映される。
  起動時ロードのメモリは positive-only 失効キャッシュ（失効済みエントリのみ保持、
  miss 時に DB 照会、否定結果は非キャッシュ）として残し、判定の根拠には使わない。
  DB エラーは fail-closed（Database / Internal、いずれも HTTP 500）とし、失効なしとして扱わない。
- リフレッシュトークンの回転も DB を正本とする。消費 `UPDATE` と新トークン `INSERT` は単一 tx で
  原子的にコミットし、`changes() == 0` の競合はロールバック後に別tx の `revoke_refresh_family`
  でファミリ失効へつなぐ。BAN チェックと claims/JWT 事前計算は `BEGIN` 前に置き、`refresh` 内では
  `write_lock` の再取得（デッドロック）を避けるため別tx を取る `issue_refresh_token` を呼ばない。
- OIDC認証の未完了 `pending`（`state` / `nonce` / `code_verifier`）は10分TTLとし、保持数はコード定数
  `MAX_PENDING_AUTH = 1024` を上限とする。登録時・取出時に期限切れエントリを一括回収し、
  上限到達時は `GET /auth/oidc/start` が 429 を返して新規エントリを追加しない（R1-23）。
- セッション発行（`/auth/verify`）の BAN 判定は原子的に適用する。事前 `ensure_user_not_banned`
  に加え、`write_lock` 配下の write tx 内で `users.is_banned` を再読し、有効な BAN
  （`ban_until IS NULL OR ban_until > datetime('now')`）なら 403 `User is banned` で
  発行を拒否する。これにより事前チェック通過後から `iat` 採番号までの間に BAN が
  コミットされても新規セッションが発行されない。判定述語・固定文言は既存の
  `ensure_user_not_banned` と同じで、期限切れ BAN は従来どおり通過する。
  権限・状态码・既存テストの期待値（正常 200、BAN 後 401 等）は不変。
  未認証で呼べる開始エンドポイントからのメモリ枯渇を防ぐ境界であり、環境変数ではない。
  回収は認証リクエスト時だけに限らず、`run_moderation_finalizer` が起動時と周期 tick
  （`MODERATION_FINALIZE_INTERVAL_SECS`）にも `sweep_expired_pending` を呼ぶため、
  認証リクエストが来ないアイドル中も期限切れ `pending` は蓄積しない。
- OIDC callback（`/auth/oidc/callback`）のセッション発行も `/auth/verify` と同じ
  BAN 原子性を満たす。事前 `ensure_user_not_banned` に加え、`write_lock` 配下の
  write tx 内で `users.is_banned` / `ban_until` を再読し、有効 BAN ならコミット
  せずに tx を rollback して 403 `User is banned` を返す。INSERT・JWT 発行前の
  拒否を担保し、事前チェック通過後に BAN がコミットされてもセッションが発行
  されない。判定述語・固定文言は既存の `ensure_user_not_banned` と同一で、
  期限切れ BAN は従来どおり通過する。
- コンテンツ投稿（`/content/upload` の `upload_content`）の BAN 判定も原子的に適用する。
  事前 `ensure_user_not_banned` は `pin_to_ipfs`（最大30秒の外部 I/O）より前にあり、
  その間に BAN がコミットされる窓があるため、`begin_write_tx` 取得後の tx 内で
  `users.is_banned` / `ban_until` を `auth_verify` と同一述語
  （`ban_until IS NULL OR ban_until > datetime('now')`）で再読し、有効 BAN なら
  rollback して 403 `User is banned` を返す。期限切れ BAN は従来どおり通過する。
  ピン留め済みの失敗では `compensate_unpin` を必ず通し、孤立ピンを残さない。
  既存の補償経路・固定文言・状态码は不変。
- モデレーション投票（`/mod/vote` の `mod_vote`）の BAN 判定も原子的に適用する。
  事前 `ensure_user_not_banned` に加え、`begin_write_tx` 取得後の tx 内で
  `users.is_banned` / `ban_until` を `auth_verify` と同一述語で再読し、有効 BAN
  なら `votes` / `signatures` の INSERT 前に rollback して 403 `User is banned`
  を返す。集計側の BAN 除外（`VOTE_NOT_FROM_BANNED_VOTER_SQL`）は既存どおり維持し、
  期限切れ BAN の票は従来どおり定足数へ算入する。
- OIDC callback の `revoked_subs` メモリ削除は DB 削除（`delete_superseded_revocation`）
  の成功後に確定する。先にメモリを `remove` した設計では DB 削除失敗時に旧閾値が
  失われ、再起動後の refresh が古い閾値で塞がり得るため、失敗時は `auth_verify`
  と同じ `entry(...).or_insert(old_threshold)` でメモリを元の状態へ復元する。
  並行して新しい閾値が登録済みなら上書きしない（fail-closed を維持）。
- 失効レコードの保持期間は共通の `REVOCATION_RETENTION_SECS`（refresh TTL 30 日
  ＋マージン 60 秒）とする。refresh は `revoked_subs` の存在だけでセッション
  延長を拒否するため、refresh token が有効な 30 日間は失効閾値を残さないと
  失効が復活する。purge はこの保持期間より古い閾値だけを回収し、セッション TTL
  （24h）を超えても refresh 有効期間内のエントリは維持する。境界は doctor /
  dry-run で確認する。
- OIDC 経由の初回登録では署名鍵を配布しないため、`public_key_der` に subject
  由来の「署名鍵未登録」プレースホルダを保存する。NULL を残すと下流の署名検証
  が NULL デコードで 500 になるため、鍵未登録は固定文言の 403/501 として扱う方針（未実装）。
  既に実鍵が登録済みの主体は上書きせず、upload / vote の既存署名検証の期待値は
  不変である。
- モック経路で作成されたユーザーは `public_key_der` が NULL であり得る。
  `verify_user_signature` は NULL を `Option<Vec<u8>>` として読み、鍵が無ければ
  検証不能として `false` を返し、upload / vote の既存 401 経路へ載せる。
  NULL デコードによる 500 は発生させない。OIDC の「署名鍵未登録」プレースホルダ
  （`oidc-no-signing-key:`）は現行どおり 401 とし、403 / 501 化は設計判断待ちのため
  変更しない。

## 2. コンテンツ・個人データ

- 投稿バイト列、CID、チェックサム、ユーザーハッシュ、公開鍵、通報・投票記録を取り扱う。
- ユーザーハッシュは匿名化を保証しない。アクセス制御、保持期間、削除・訂正手続、バックアップ消去方針を本番前に定める。
- IPFSへの投稿は外部から取得可能になり得る。削除要請・違法コンテンツ対応・ピン解除の運用を整備するまで、永続公開を前提としたデータを安易に登録しない。
  `pin_to_ipfs` が実 Pinata へのピン留めに成功した後、アップロードの DB コミットが失敗した場合は
  `unpin` による補償を試みる。解除失敗は固定文言の警告ログのみへ記録し、トークン値・
  レスポンス本文・CID はエコーしない（手動解除が必要）。補償の成否にかかわらず元の
  エラーを返し、DB 未確定（＝アップロード失敗）を正とする。
- アップロード形式・10 MiB上限・外部取得10 MiB上限を守り、コンテンツ内容の合法性をハッシュ検証で保証しない。
- `POST` / `DELETE /content/{cid}/favorite` は `write_lock` 配下の同一 tx で
  `contents.status = 'Approved'` の存在確認と `favorites` の INSERT / DELETE を行う。
  確認後に状態が変わる競合を排除し、存在しない・非 Approved の CID は従来どおり
  404（有無を識別子で区別しない）とする。
- `GET /content/{cid}` も取得開始時の `status = 'Approved'` 確認だけを根拠に配信しない。
  IPFS バイト取得の間に `mod_report` で `Reported` / `Rejected` へ更新され得るため、
  レスポンス組み立て直前に `SELECT status FROM contents WHERE cid = ?` を再実行し、
  `Approved` でなければ 404 を返す。共有 `Connection` の制約上、読み取り tx ではなく
  再 SELECT で最新状態を確認する。チェックサム検証やヘッダ契約は変更しない。
- `GET /me/export` は認証必須（`AuthClaims`）。`version:1` / `generated_at` / `sub` と12表
  （`users` / `contents` / `favorites` / `transactions` / `signatures` / `content_audit_log` /
  `reports` / `votes` / `payment_orders` / `governance_audit_log` / `refresh_tokens` /
  `revoked_subs`）の全行を返し、行数上限・`truncated` フラグは設けない。
  `token_hash` / `sid` / `family_id` / 署名鍵 / XPRV / `PINATA_JWT` / 他ユーザーID は除外し、
  `reason` / `details` はマスクする。未認証は401で拒否する。削除権（`/me/erase`）は未実装。

## 3. 監査・モデレーション

- MVPではDBを正本とする。投票期間は通報から7日とし、期限到達後に集計する。定足数3票未達なら通報を棄却して `Approved` に復帰し、3票以上なら多数決（同数は `Rejected`、ADR-0004、
  公開継続リスクを避ける安全側判断）で確定する。期限前の即時確定は行わない。
  R2-AUTHZ により `appeal_of` を付けた異議（確定から30日以内の再通報）は、
  元通報者（`reports` に `reporter_hash` 列はないため `content_audit_log` の `changed_by`、
  reason が `User report:` の行から導出）またはコンテンツ所有者（`contents.owner_hash`）に限定する。
  それ以外の主体は固定文言 `Appeal is restricted to the original reporter or content owner` で403とし、通報行を作成しない。
  さらに同一 CID の異議は30日以内に1回までとし、クールダウン中の再異議は固定文言
  `Appeal cooldown: only one appeal per CID within 30 days` で403とする（運用者承認を要求する代替案の代わりに、
  自動的な再通報による審査の並行を防ぐ）。いずれも fail-closed であり、監査行が存在しない既存データでは所有者のみを受理する。
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
- BAN によるセッション失効も DB の `revoked_subs.revoked_at` を正本として判定する（認証時に毎回 SELECT し `iat` と比較）。BAN と back-channel logout
  のどちらも `iat` を更新時刻とみなし、同一トークンの再送や古い `iat` では `revoked_at` を前進させない（UPSERT の `changes() == 0` でメモリ更新とリフレッシュ消費をスキップする）。
  メモリの失効閾値は positive-only キャッシュに留め、書き込みは DB コミットを先に行うため、他インスタンスへ即時に反映される。
  UNBAN は `users` の BAN フラグ解除に加え、`revoked_subs` の失効閾値を削除せず
  `MAX(revoked_at, now)` で sub epoch を進め、コミット後に positive-only キャッシュも
  同じ epoch へ更新する。BAN 以前の JWT（`iat <= 旧閾値`）は復活せず、back-channel
  logout 由来の閾値も後退させずに保持する。refresh は `revoked_subs` の存在中は拒否され、
  再認証で新しい `iat` が発行されたときだけセッションが復活する（再ログイン必須）。
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
  監査ログ更新後は `verify_audit_log` の計算 root（`audit_chain_digest:` 行の `root=<hex>`）を
  再取得し、`backup-rotate.sh` が成果物と同じ stamp の `.audit-root`（0600）として世代保存する
  （暗号化時も同じ世代名、root 値は stdout / stderr / ALERT へエコーしない）。次回期待値への
  昇格は運用者の明示承認を要する手作業とし、スクリプトは期待値を自動上書きしない。
  リストアでは復元対象世代に紐付いた root を `--expect-root` へ渡す。
  本番では `AUDIT_EXPECT_ROOT` の設定が必須（本番起動ゲート）。本番相当（`ALLOW_MOCK_AUTH` 無効）では未設定・非 UTF-8・
   不正 hex（空・空白のみ・前後空白付きを含む）を固定文言で起動拒否し、値・バイトはエコーしない。
   判定は共有ヘルパ `production_audit_expect_root_error` が担い、`production_config_errors` と
   同じ設定フェーズ（DB 接続・マイグレーション・instance.lock・FK 検査より前）で検証する。
   `backup-rotate.sh` 自体は未設定時も内部整合性チェックのみで動作する従来契約を維持し、
  （既存運用を壊さない）うえで、verify_audit_log / verify_audit_export の双方で root 値はエラー文言へエコーしない。
   `doctor` は同じ共有判定を `audit_expect_root` カテゴリの固定文言警告へ反映し、
   値はエコーしない（既定は警告のみ、
   `--strict` のときだけ失敗）。
- 監査スナップショットは `sign_audit_export` で署名し、`verify_audit_export` で公開鍵検証する（[AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md)
  ）。秘密鍵はファイルで厳重管理し、CLI 引数や環境変数へ内容を渡さない。
  `sign_audit_export` は起動時に署名鍵 PEM を `canonicalize` して権限を検査し、owner-only
  （`0600` / `0400`）を要求する。親ディレクトリが group/other 書き込み可能なら拒否し、
  `0700` でなければ警告する。リモート libSQL のトークンは `DATABASE_TOKEN` 環境変数からのみ
  読み、`--database-token` は受け付けない（`argv` 露出対策）。出力先は `create_new` で原子的に
  生成し、既存ファイルの上書きを拒否する。公開は `restore_db` の staging + safety copy と同じ方式で、
  同ディレクトリの `0600` 一時ファイルへ書き `fsync` 後に `hard_link` で原子的に公開し、
  失敗時は一時ファイルを残す（詳細は直下の監査エクスポートの項目を参照）。
   手順は [AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md)。
   `verify_chain` は `reasons_masked=true` のとき全 entry の `reason` がマスク済みであることを検証する。
   署名対象 `signature_payload()` は `reasons_masked=true` のとき entries_json を `masked` 印付きで含むため、
   この検査は署名対象フィールドの整合確認であり、署名検証結果と矛盾しない。
- 監査エクスポートの公開経路は `hard_link` による create_new 相当の原子的公開へ変更し、既存ファイルは
  競合として拒否する（存在チェックと公開の TOCTOU を解消）。hard_link が使えない環境
  （クロスデバイス等）は `rename` フォールバックで最終パスの存在を再確認する。公開失敗時は
  ステージングを残し、値や CID をエコーしない。ステージング名は PID と unix_millis
  （`.tmp-<pid>-<unix_millis>`）を含め、クラッシュ残骸による再実行ブロックを防ぐ。
  `content_audit_log` / `governance_audit_log` が両方 0 件のときは `--allow-empty` の明示が無い限り
  エクスポートを拒否し、`verify_audit_log` も空ログを `--expect-root` 未指定時を含め固定文言で拒否する
  （`AUDIT_CHAIN_EMPTY` ダミールートを期待値として暗黙受理しない）。
- 監査 CLI の権限・トークン・上書き拒否は実装済み。`sign_audit_export` に自動テスト17件を追加済み
  （TEST_SPECIFICATION AUDIT-02）。Runbook の手動確認（鍵権限・親ディレクトリ権限・
  `DATABASE_TOKEN` 設定・既存出力の有無）は補助的な確認として扱う。
- 監査・バックアップ系 CLI（`backup_db` / `restore_db` / `sign_audit_export` /
  `verify_audit_export` / `verify_audit_log` / `verify_backup`）の argv 契約は
  `server::cli_args` に集約する（[DETAILED_DESIGN.md](DETAILED_DESIGN.md) の
  「共通 argv 契約」を参照）。各 bin が定義するのは受け付けるフラグの一覧だけとし、
  秘密値の非 argv 化・ハイフン始まりの値拒否・unknown argument の固定文言を
  bin ごとに複製しない。
- 具体的には、`--database-token` は
  `--database-token is not accepted; set DATABASE_TOKEN in the environment instead`
  で拒否して値をエコーせず、値取りは `<flag> requires a value`、
  未知の引数は `unknown argument: <arg>` の固定文言で拒否する。
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
- 送金不一致による `MISMATCH` マーキングも CAS 検査対象とする。検証中に注文 status が
  外部書き換えされて 0 件更新になった場合は 409（CONFLICT）で拒否し、書き換え後の値を
  上書きしない。1 件更新時は従来どおり 400（`transfer does not match the order`）を返し、
  注文を `MISMATCH` にする。
- `bind_wallet` のウォレット所有証明記録も CAS 検査対象とする。`write_lock` を取得した
  単一トランザクション内で `status` と `expires_at` を読み直し、期限再判定
  （`reject_if_expired`）後も `status = 'PENDING'` を条件に `wallet_bound = 1` を更新する。
  0 件更新（status が PENDING 以外へ進行した競合）は 409（CONFLICT、固定文言
  `order status changed during wallet binding`）で拒否し、`verify_order` と同じ
  単一ライター契約とする。正常系は 1 件更新で従来どおり成功する。
- `create_order` は `contents.price` / `status` の検証と `payment_orders` への INSERT を
  同じ `write_lock` 配下の tx で行う。事前検証は早期 400 と derivation index の
  無駄な消費防止のために残し、tx 内でも再検証して検証後の価格・状態変更（TOCTOU）を
  拒否する。HD / 固定アドレス分岐のどちらにも適用する。

## 5. 運用準備・未整備事項

- 本番用のオリジンを定義し、CORSを環境設定する。ローカル開発オリジンを本番許可設定に流用しない。
  クライアントの API オリジンはビルド時に `OIL_API_BASE` で注入する。値はオリジンのみ
  （`<scheme>://<host>[:<port>]`）とし、末尾 `/`・パス・クエリ・認証情報・機微情報は入れない。
  リリースビルドの const 検証は末尾 `/` も拒否するため、指定時に取り除くこと。
  未設定時は開発用の `http://localhost:3000`。HTTPS 配信では HTTPS オリジンを指定し mixed content を避ける。
  リリース成果物（`dev-console` feature なし）は `OIL_API_BASE` に `http://` または
  `localhost`（大小文字非区別）を含む値を指定すると const アサーションでビルドを拒否する
  （fail-closed）。本番は `https://` オリジンを明示指定し、未設定時に倒れる開発既定値を
  成果物へ持ち込まない。E2E / ローカル開発ビルドは `OIL_API_BASE` 未設定（開発既定値）または
  `dev-console` feature を使い、従来どおり平文 HTTP オリジンを扱える。
  このビルド時検証はクライアントへ注入する値に限り、サーバーの `ALLOWED_ORIGINS` 完全一致検査や
  `Origin` ゲートの代替ではない。
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
  失敗ファイルは適用されない。マイグレーションファイルが1つも無い（空ディレクトリで最低001が存在しない）場合も `run` / `verify` の双方で起動を拒否する（fail-closed）。未適用マイグレーシ
  ョンの判定では `db.query` の Rows を1行読んだ後に明示 `drop(rows)` してからトランザクションを開始し、読み取りカーソルを残さない。復旧は原因修正後の再起動とし、`_migrations` の記録を確認する。
  適用済みバージョンに対応するファイルが無い場合（古いイメージでの起動等）、
  およびマイグレーションディレクトリ自体が存在しない場合も起動を拒否する（fail-closed、ディレクトリ欠落で欠落検査をすり抜けるのを防止）。単一プロセス内の書き込みトランザクションは接続ごとのロックで直列化する（libSQL
  は同一接続で同時に1つの `BEGIN` しか保持できない）。ローカル DB では起動時に `<DB path>.instance.lock` を原子的に作成し、生存 PID
  のロックがある場合は第二インスタンスの起動を拒否する（fail-closed）。クラッシュ後の残骸（死んだ PID）は自動回収し、停止時にロックを削除する。
  ロックファイルの読取失敗や内容不正（所有者 PID を識別できない場合を含む）は stale とみなさず起動を拒否する（fail-closed）。リモート DB ではこの排他が使えないため警告のみとし、
  単一ライター運用を契約とする。バックアップは `backup_db`（`VACUUM INTO`・上書き拒否）で取得し、`verify_backup` と `verify_audit_log` で検証する。手順は
  [BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md) を参照。
  自動オーケストレーションの設定例（systemd timer ＋ 停止→バックアップ→起動 unit）は `deploy/` に提供済み。
  実環境への適用・オフサイト保存・実運用での復旧訓練は本番前に整備する。
- 失効判定の DB 正本化: 本番はリモート DSN を拒否し `file:` 絶対パス＋`instance.lock` の単一ライター契約とするが、
  手動起動やロック回収の競合で複数プロセスが存在し得るため、`sid` / sub の失効は DB を正本として
  認証リクエストごとに照会し、他プロセスの失効を即時に反映する。
  起動時ロードのメモリは positive-only キャッシュ（失効済みのみ保持、miss 時に DB 照会、否定結果は非キャッシュ）としてのみ残す。
  書き込み経路（logout / BAN / backchannel）は DB コミットを先に確定し、メモリ更新はその後に置く。
   backchannel の更新時刻は logout token の `iat` とし、再送・古い `iat` では `revoked_at` を前進させず、
   DB 変更0件のときはメモリ更新と `revoke_refresh_tokens_for_sub` を実行しない。
- 起動時の外部キー検査（`PRAGMA foreign_key_check`）で孤立行を検出した場合、本番相当
  （`ALLOW_MOCK_AUTH` 無効）は検査失敗時と同様に起動を拒否する（fail-closed）。
  開発（`ALLOW_MOCK_AUTH=1`）では起動を止めず warn のみとする。`PRAGMA foreign_keys` は
  コネクション設定だが `foreign_key_check` は別に動作するため、リモート DB も対象に含める。
  起動拒否文言には件数と `table(rowid=...) -> parent` のみを含め、秘密値はエコーしない。
  `doctor` は本番相当で同じ判定を `production` カテゴリ内へ警告としてマージし、`Unresolved`（未設定・非 UTF-8）時は `file:local.db` へフォールバックせず、検査失
  敗として fail-closed 警告を出す（値はエコーしない）。リモート DSN は doctor の検査対象なので誤って拒否しない。
  カテゴリ数（check23）は変更しない。是正 CLI や自動修復は提供しないため、
  孤立行の検出時はバックアップからの作り直しで対応する
  （[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)、[ADR-0010](adr/ADR-0010-foreign-key-startup-fail-closed.md)）。
- バックアップ運用では、サーバー稼働中は `backup_db` が拒否される。ソース DB（ローカルの場合）と
  出力先 DB の `<db>.instance.lock` を検査し、生存サーバーが保持していれば `refusing backup`
  で失敗する（判定不能時も拒否、stale ロックのみ許可、fail-closed）。定期実行は
  `deploy/openillustration-backup.timer` と `deploy/openillustration-backup-window.service`
  による停止→バックアップ→起動の自動オーケストレーションで運用する。`Type=oneshot` の
  `ExecStopPost` は成否にかかわらず実行されるため、バックアップ失敗時もアプリ unit が起動する。
  cron 版はラッパーのみを実行し、停止/起動は root 側スケジューラが担う（`openillustration`
  ユーザーへ `systemctl` 権限を与えない）。窓の前後で `/ready` 503 → `/health` / `/ready` 200
  を確認し、再起動失敗は unit 結果をアラート対象にする。`scripts/backup-rotate.sh` 単体は
  サーバーを停止しないため、オーケストレーション無しの稼働中実行は拒否されて失敗する。
  暗号化成果物は `<stamp>.db.gpg` で、復元時は `gpg --decrypt` の復号先を `RESTORED_DB` に
  統一する。`verify_backup --input "$RESTORED_DB"` の成功を前提に、
  `restore_db --backup "$RESTORED_DB"` を実行する（Runbook「リストア（復旧訓練）」参照）。
  `deploy/openillustration-backup.cron` のジョブ行は `backup-cron-wrapper.sh` のみを実行し、
  常時稼働環境では `instance.lock` で拒否されるため、root 側スケジューラで
  stop→wrapper→start を併設するまで有効化しない。配置後は手動1回実行して成功ログを確認する。
  詳細は [BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md) を参照。
- 定期実行スケジューラは systemd timer＋window unit
  （`deploy/openillustration-backup.timer` ＋ `deploy/openillustration-backup-window.service`）
  と cron（`deploy/openillustration-backup.cron`）の**二者択一**とし、両方を有効にしない。
  二重バックアップや停止/起動の競合を防ぐため、cron を選ぶ場合は
  `systemctl disable --now openillustration-backup.timer` でタイマーを無効化し、
  systemd を選ぶ場合は cron ジョブを配置しない。
- `payment_orders.derivation_index` には migration 003 の部分 UNIQUE インデックス
  （`WHERE derivation_index IS NOT NULL`）が適用済みである。固定受取アドレスの注文
  （`derivation_index IS NULL`）は制約対象外とし、xprv 設定時は注文ごとの受取先
  再利用を DB 制約でも拒否する。既存 DB に重複した `derivation_index` がある場合は
  インデックス作成時にマイグレーションが失敗して起動を中止する（fail-closed）。
  黙ってデータを修正せず、運用側の確認を強制するための動作である。
  並行注文対策（R1-26）は新規マイグレーションを追加せず、`create_order` が
  書き込みロックとトランザクション内で採番を直列化することで実現する。
   verify の CAS 更新が UNIQUE 違反で失敗したときは、メッセージ文字列だけを根拠に
   「同一送金の再利用」と判断せず、SQLite の extended code（2067）に加えて違反カラム名が
   `payment_orders(chain_id, tx_hash, log_index)` のときだけ 400 の再利用扱いとする。
   他の UNIQUE インデックス（例: `derivation_index`）や非 UNIQUE エラーは Internal(500) へ伝播させる。
- IPv6のみの制御をプロキシ経由で使う場合、`TRUSTED_PROXY_CIDRS` に信頼済みプロキシの範囲のみを
  設定する。プロキシの `X-Forwarded-For`（XFF）は**上書きを推奨**するが、**追記も可**である。
  追記型（末尾がピア）ではサーバーが末尾のピアを除去して最左のクライアントを採用し、
  上書き型（末尾がピアでない）では末尾を採用する。信頼済みピアで解決できない場合は 403 となる。
  ネットワーク層でも許可方針を実装する。
   読取は `common::env::read_env` に一本化し、非 UTF-8 は固定文言の警告のうえ空範囲として
   fail-closed に倒す（生バイトも設定値もログへ出さず、キー名と理由のみを警告する）。
   `run()` の起動ログも同じ分類結果を使い、非 UTF-8 は未設定と同じ「not configured」として扱う。
- ログにJWT、署名、個人情報、投稿本文、Pinataトークンを出さない。現状のエラー応答は500系の内部詳細を
  ログのみに保ち、クライアントへは固定文言 `Internal server error` を返す（開発時は
  `EXPOSE_INTERNAL_ERRORS=1` で詳細を公開）。観測性は `LOG_FORMAT=json` と `/metrics` で提供し、
  アラート通知先は導入環境で選定する。
- クライアント側のエラー表示方針: サイレント失敗を禁止し、成功・進捗は `role="status"`、
  失敗は専用の `role="alert"` で利用者へ告知する。失敗用の testid は `Login` の
  `data-testid="login-error"`、エクスポートの `data-testid="export-error"`、
  ContentViewer の監査の `data-testid="audit-error"`、モデレーション集計の
  `data-testid="moderation-stats-error"` を使い、HTTP ステータス・通信失敗・JSON 解析失敗を区別する。
  アップロードは成功・進捗を `data-testid="upload-status"`（`role="status"`）、失敗を
  `data-testid="upload-failed"`（`role="alert"`）へ表示し、準備失敗は
  `アップロードの準備に失敗しました。`、応答読取失敗は `アップロード結果の読み取りに失敗しました。`
  を使う。通信／HTTP 応答は `upload_error_message` の4分岐で固定文言を選ぶ:
  401=再ログイン（`再ログインが必要です。`）、413=サイズ超過（`ERROR: File exceeds the 10 MiB limit.`）、
  HTTP その他=`Failed to upload content (HTTP {status}).`、None（ステータス取得不可）=
  `Network error uploading content.`。Ok で返る `upload_error_message` は `SUCCESS:` 開始なら
  `upload-status`、それ以外は `upload-failed` へ振り分ける（文言は変更しない）。
  通報は成功・進捗を `role="status"`、送信失敗を `data-testid="report-failed"`
  （`role="alert"`、`Report failed.`）へ表示する。入力検証エラーは
  `data-testid="report-validation-error"` と区別する。サーバー本文・生エラーは表示しない。
  いずれの結果でも `loading` を解除してボタンを再活性化する。既存 E2E 内へ検証を追加し、
  test() 定義数69件は変更しない。
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
   `TZ` の読取は `common::env::read_env` ベースの `configured_tz()` に一本化し、
   `run()` の年齢境界判定と `validate_age` が同じ分類結果を共有する。非 UTF-8 は
   生バイトを保持せず `None`（UTC フォールバック）へ倒し、doctor は
   `classify(lookup("TZ"))` の `EnvValue::NonUtf8` を固定文言の未設定警告として扱う。
- 上記2カテゴリの追加項目は既存の23カテゴリ内に含まれ、カテゴリ数は変わらない。
   既定は警告のみで終了コード0、`--strict` のときだけ問題として終了コード1とする。
- **Runbook（本番起動前ローカルゲート）**: 秘密管理基盤から本番相当の環境変数を注入し、
  `cargo run -p server --bin doctor -- --strict` を実行して終了コード0を確認してからデプロイする。
  診断は既存の23カテゴリ固定（`check23`）と `audit_expect_root` / `production` /
  `oidc_endpoint_urls` 等のカテゴリを対象とし、カテゴリ数・判定ロジックは変更しない。
  問題検出時は終了コード1でゲート失敗とし、設定是正後に再実行する。
- liveness は `GET /health`（プロセス生存のみ、固定 200）。readiness は `GET /ready` とし、DB への `SELECT 1` が成功し停止処理中でなければ 200、それ以外は 503
  を返す。停止シグナル（SIGTERM / SIGINT）受信時は、まず `shutting_down` を立てて readiness を即 503 にし、
  固定のドレイン窓（`READINESS_DRAIN_WINDOW`）をおいてから graceful shutdown を開始する。
   LB がヘルスチェックでこのインスタンスを除外してから drain するため、新規リクエストの到着を抑える。いずれも IPv6/IP ゲートの対象（バイパスしない）。
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
  受け付けたイベント数の累積値）を公開する。カウンタはプロセス内で再起動時にゼロへ戻る。
  このカウンタは参考値であり、アラート判定や請求等の意思決定に単独で使わない。
  エンドポイントは認証不要だが、加算は1リクエストにつき1固定であり、
   `count=1000` の連投による増幅は60/分/IPのレート制限へ低減される。
  累積値は実際の被害や不正の規模を保証しない。判断には認証済みデータや
  サーバー側の独立した記録を突き合わせる。
- クライアント側メトリクス送信の無効化: ビルド時に `OIL_CLIENT_METRICS=0` を指定すると
  `POST /metrics/client-events` を一切送らない（未設定・その他の値は送信する）。
  有効時のペイロードは `event`（固定値 `neutralized_url`）と `count`（1〜1000 の整数）のみで、
  URL 本文・IP アドレス・User-Agent・認証情報は含めず、サーバーも受信・記録・ログ出力しない。
  送信を止めても `GET /metrics` の `openillustration_neutralized_urls_total` は
  それまでに受け付けたイベント数の累積値として残る。
- 自動確定（`finalize_due_reports*`）は一件ごとに `BEGIN` / `COMMIT` し、
  途中で一件が失敗しても既に確定済みの件数を失わず、成功件は `COMMIT` ごとに
  `note_finalizer_success(metrics, 1)` で `finalizer_processed` へ加算する。
  失敗件は `finalizer_failures` へ分離計上し、成功件と失敗件を混同しない。
  成功 0 件の記録（`note_finalizer_success(metrics, 0)`）は対象 0 件の正常終了の
  ときだけ行う。`failed > 0` かつ `finalized == 0`（対象はあるが一件も確定でき
  なかった）場合は成功 0 件を記録せず、`finalizer_failures` へ加算するだけに留める。
- finalizer／purge の観測: `GET /metrics` は `openillustration_finalizer_ticks_total`（周期 tick の実行回数）、
  `openillustration_finalizer_processed_total`（finalizer と周期 purge の処理件数）、
  `openillustration_finalizer_lock_busy_total`（書き込みロック取得失敗でスキップした回数）、
  `openillustration_finalizer_failures_total`（失敗回数）、
  `openillustration_finalizer_last_success_timestamp_seconds`（最後の成功時刻、gauge）の5種を公開する。
  値は `AtomicU64` のプロセス内カウンタで、再起動時にゼロ（最終成功時刻は 0）へ戻る。
  これらは参考値であり、中立化URL件数と同様に単独のアラート判定・請求判断へ使わない。

- CI の依存ゲートは cargo-deny の4契約（licenses / advisories / bans / sources）を
  `extra_gates` で実行する。`bans` は `multiple-versions = "deny"` とし、既知の重複だけを
  `deny.toml` の exact version skip（`version = "=x.y.z"`）へ列挙する。新規バージョンの
  重複は skip に一致しないためビルドを失敗させ、理由を添えて skip を追加する運用とする。
  `version = "*"` の skip は新規重複まで黙って通すため使用しない。
  `sources` は crates.io インデックスのみを許可し、未知レジストリ・Git 起源は拒否する。
  `wildcards = "deny"` でレジストリ依存の無制約 `version = "*"` を拒否し、
  workspace の path 依存（client / server）は skip と `allow-wildcard-paths` で例外的に許可する。
  取得元やライセンス検査を緩めるものではない。

## 6. 本番移行ゲート

本番移行には、実JPKI認証の独立検証、JPYC取引確認と手数料・失敗時運用、第三者検証可能な監査、プライバシー・法務レビュー、モデレーション承認、IPFS削除/通報対応、DB復旧訓練、環境別設定とセキュリティ試験を必要とする。
本書の記載だけで法令適合を保証しない。

- 性能（PERF-01）と互換性（COMPAT-01 / COMPAT-02）の **Level 1（受入）** も本番移行の
  ブロッカーに含める。Level 1 は
  [ADR-0011](adr/ADR-0011-performance-measurement.md) と
  [ADR-0012](adr/ADR-0012-compatibility-matrix.md) の合意後に適用する。
  合意前の **Level 0（観測可能性）** は観測値が有限かつ 0 以上で保存されることを
  pass 条件とし、受入の根拠には使わない。受入値・対応マトリクス・最小バージョンが
  未決定の間は「合意済み」とみなさず、移行を完了しない。

- 失効判定の DB 正本化に伴う per-request SELECT 増分は ADR-0011 の
  **Level 0（観測可能性）** の観測値として記録する。有限かつ 0 以上の値が保存されることを
  pass 条件とし、受入の根拠には使わない（Level 1 の受入値は ADR-0011 合意後に適用する）。

## 7. 環境変数リファレンス（サーバー＋運用スクリプトが読む環境変数）

サーバーと運用スクリプトが読む環境変数の一覧。本番では開発専用の項目を設定しない。

| 変数 | 必須 | 既定 | 説明 |
| ---- | ---- | ---- | ---- |
| `JWT_SECRET` | 必須 | なし | HS256署名鍵。未設定・32バイト未満は起動を拒否する。秘密管理基盤から注入する。`JWT_RSA_PRIVATE_KEY_PEM` 設定時は使われない。 |
| `JWT_RSA_PRIVATE_KEY_PEM` | 任意 | なし | PKCS#8 PEM の RSA 秘密鍵。設定時は RS256 で署名する。署名ヘッダには公開鍵由来の `kid` を付与する。2048bit 未満・不正な PEM は、`JWT_SECRET` が有効でも黙ってフォールバックせず起動を拒否する。秘密管理基盤から注入し、ログ・配布物へ出さない。 |
| `JWT_REQUIRE_KID` | 本番推奨 | 0 | 受理値は `1`/`true`（有効）と `0`/`false`（無効）のみ。`1` 設定時は `kid` 付きトークンのみ受理し、`kid` 無しレガシートークンを拒否（fail-closed）。RS256 限定。HS256 と併用すると起動拒否。受理値外（yes/no/空など）も起動拒否し、`doctor` の `jwt` カテゴリで警告。DB接続・マイグレーション・instance.lock取得・FK検査の前に検証し、失敗時にこれらを残さない。 |
| `JWT_RSA_VERIFICATION_KEYS` | 任意 | なし | RS256 のみ有効。`kid` → SPKI 公開鍵 PEM（`BEGIN PUBLIC KEY`）の JSON オブジェクト。旧鍵を検証用に保持して無停止ローテーションを行う。`add_verification_key` は引数 `kid` と`kid_from_public_pem`（SPKI DER の SHA-256 先頭16バイト）の導出値を照合し、不一致なら登録を拒否する。未知 `kid` は拒否（fail-closed）。`kid` 無しの旧トークンは現行署名鍵で検証する。手順は [KEY_ROTATION_RUNBOOK.md](KEY_ROTATION_RUNBOOK.md)。 |
| `DATABASE_URL` | 本番相当では全プロセス必須（開発 mock 時のみ任意） | なし（本番）/ `file:local.db`（開発 mock 時のみ） | libSQL 接続先。本番相当（`ALLOW_MOCK_AUTH` 無効）では未設定・非 UTF-8・空白のみ・旧既定の相対 `file:local.db`・リモート DSN（`file:` で始まらず `://` を含む `libsql://` / `https://` / `http://` / `wss://` / `foo://` 等）を起動拒否（fail-closed、値・トークンはエコーしない）。`file:/var/lib/openillustration/local.db` 等の絶対パスのみ指定する。`doctor` の FK 検査は未設定・非 UTF-8（Unresolved）のときに `file:local.db` へフォールバックせず、検査失敗として fail-closed 警告を出す（値はエコーしない）。 |
| `DATABASE_TOKEN` | 任意 | 空 | リモートlibSQLの認証トークン。`sign_audit_export` は環境変数からのみ読み、`--database-token` はエラーとなる（argv 露出対策）。`verify_audit_log` / `backup_db` / `restore_db` も含め全 CLI で `--database-token` は拒否し、`DATABASE_TOKEN` 環境変数のみを使用する。サーバー `run()` と doctor の FK 検査用 DB 接続も `common::env::read_env` で同じ値を読み、非 UTF-8 は `sign_audit_export` の `database_token_from_env_value` と同じ契約で固定文言 `DATABASE_TOKEN must be a valid UTF-8 string` の起動拒否／doctor 警告（fail-closed）へ倒す（未設定は空文字、UTF-8 はその値）。値・生バイトはエコーしない。`doctor --strict` は同じ判定を `production` カテゴリの警告へ反映するため、起動拒否なのに `23 check(s), 0 problem(s)` となる false pass は起こらない（check23 不変）。 |
| `ALLOWED_ORIGINS` | 本番では必須 | 開発用オリジン3件（`ALLOW_MOCK_AUTH=1` のときのみ） | CORS許可オリジン（カンマ区切り、有効な `http(s)://` オリジンのみ、末尾スラッシュは除去）。`null` は設定不可（起動時に無視し、`/auth/refresh` の完全一致検査にも入れない）。信頼できないサブドメインを入れない。本番相当（`ALLOW_MOCK_AUTH` 無効）では未設定・非 UTF-8・空・全不正（有効な `http(s)` オリジンが1つも無い場合）を起動拒否（fail-closed、値・オリジンはエコーしない）。判定は共有ヘルパ `production_allowed_origins_error` が担い、`run()` の設定フェーズと `doctor` の `allowed_origins` カテゴリが同じ固定文言を返す（`doctor` は既定で警告のみ、`--strict` のときだけ失敗）。 |
| `LOG_FORMAT` | 任意 | 未設定（テキスト） | `json` のとき1行1JSONの構造化ログを出力。未設定はテキスト形式（詳細は OPERATIONAL_POLICY のログ節）。読取は `common::env::read_env` に一本化し、未設定と `json` / `text`（大小文字不問）のみ受理。タイポ・非 UTF-8 は固定文言 `LOG_FORMAT must be json or text; using text (invalid value)` の警告のうえテキスト形式へフォールバックし、`doctor` の `production` カテゴリが同じ判定で `--strict` の警告対象にする（値・生バイトはエコーしない）。 |
| `RETENTION_DAYS` | バックアップ運用 | 30 | `backup-rotate.sh` の保持日数。0 以上で先頭ゼロなし。 |
| `MIN_KEEP` | バックアップ運用 | 3 | 最低保持世代数。1 以上。 |
| `IPFS_MAX_ATTEMPTS` | 任意 | 1 | ネットワークエラー時に各 IPFS ゲートウェイを再試行する回数（1〜5、範囲外は1）。HTTP エラー・サイズ超過・チェックサム不一致は再試行しない。 |
| `OIDC_REQUIRED_ACR` | 本番では必須（開発は任意） | 未設定 | 要求する ID トークンの `acr`（カンマ区切り、いずれか一致）。設定時にクレーム欠落/不一致なら認証拒否（fail-closed）。 |
| `OIDC_MAX_AUTH_AGE_SECS` | 本番では必須（開発は任意） | 未設定 | 認証からの最大経過時間（秒、正の整数）。設定時は認証要求へ `max_age` を付与し ID トークンの `auth_time` を検証。未設定は `auth_time` / `max_age` 検証を行わず、不正値（0・負・非数値・オーバーフロー）は起動拒否。上限 `MAX_AUTH_AGE_MAX_SECS = 2_592_000`（30日）を超える値も固定文言（値非エコー）のハードエラーで起動拒否（fail-closed）。 |
| `OIDC_REQUIRED_AMR` | 本番では必須（開発は任意） | 未設定 | 要求する ID トークンの `amr`（カンマ区切り、すべて含まれること）。設定時にクレーム欠落/不一致なら認証拒否（fail-closed）。 |
| `TRUSTED_PROXY_CIDRS` | 任意 | 未設定 | 信頼済みリバースプロキシの IP / CIDR（カンマ区切り、単一ホップ前提）。設定時のみ、範囲内のピアから届いた `X-Forwarded-For`（XFF）を解釈し、追記型（末尾がピア）は末尾のピアを除去して最左のクライアントを、上書き型（末尾がピアでない）は末尾をクライアントIPとして採用する。不正エントリ・解決不能は 403（fail-closed）。全アドレスを含む範囲（プレフィックス長 0）と不正な要素は起動時に警告のうえ無視する（範囲は IPv4 / IPv6 の正規形で記載）。未設定時は本ヘッダーを判定に使わない。`doctor --strict` の警告対象。読取は `common::env::read_env` に一本化し、非 UTF-8 は固定文言警告のうえ空範囲（fail-closed）。警告にはキー名と理由のみを含め、設定値・生バイトはエコーしない。`run()` の起動ログも同じ分類結果を使う。 |
| `RATE_LIMIT_PER_MIN` | 任意 | `60` | IP 単位のリクエスト上限（1〜600）。本番相当（`ALLOW_MOCK_AUTH` 無効）のみ適用。超出は 429。読取は `common::env::read_env` で一度だけ行い、非 UTF-8 は固定文言警告のうえ既定60へフォールバック。`doctor` も `classify` ベースで同じ警告を出す（値・生バイト非エコー）。 |
| `MAX_CONCURRENT_REQUESTS` | 任意 | `16` | 同時実行リクエスト上限（1〜256）。超過は 429。`/health`・`/ready`・`/metrics` は対象外。読取は `common::env::read_env` で一度だけ行い、非 UTF-8 は固定文言警告のうえ既定16へフォールバック。`doctor` も `classify` ベースで同じ警告を出す（値・生バイト非エコー）。 |
| `IPFS_GATEWAYS` | 任意 | 既定3件 | 取得に使う IPFS ゲートウェイの URL テンプレート（カンマ区切り）。各要素は `{cid}` を含む URL であること。スキームは `https` のみを受理し、`http` は `ALLOW_MOCK_AUTH` が有効な開発・隔離テスト環境でのみ受理する（本番相当では警告のうえ無視）。空要素、`{cid}` を含まない要素、その他スキームも警告のうえ無視する。CIDは英数字のみを許可し、パス・クエリ・フラグメント注入を拒否する。空にすると取得は常に失敗する。 |
| `IPFS_TIMEOUT_SECS` | 任意 | `15` | 各ゲートウェイへのリクエストタイムアウト秒（1〜300）。範囲外・不正値は既定値。 |
| `GRACEFUL_DRAIN_TIMEOUT_SECS` | 任意 | `30` | グレースフル停止の drain タイムアウト秒（1〜300）。読取は `common::env::read_env` で一度だけ行い、範囲外・不正値・非 UTF-8 は既定30へフォールバックし `doctor` が `classify` ベースの固定文言で警告（値・生バイトは出力しない）。 |
| `MODERATION_FINALIZE_INTERVAL_SECS` | 任意 | `300` | 自動確定ワーカーの周期（秒、30〜3600）。読取は `common::env::read_env` で一度だけ行い、範囲外・不正値・非 UTF-8 は既定300へフォールバックし `doctor` が `classify` ベースの固定文言で警告（値・生バイトは出力しない）。 |
| `MODERATION_OPERATOR_HASHES` | 本番では必須（未設定時は特権操作を全拒否） | 未設定 | カンマ区切りの運用者 user_hash。未設定・空要素のみなら空集合となり BAN/UNBAN/UNPIN は全拒否（fail-closed）。読取は `common::env::read_env` に一本化し、非 UTF-8 は固定文言 `MODERATION_OPERATOR_HASHES is not valid UTF-8; treating as unset (BAN/UNBAN/UNPIN are all rejected; value redacted)` の警告のうえ未設定扱いとする。生バイトは保持・エコーしない。`doctor --strict` は同じ共有文言を `production` カテゴリの警告へ反映し、全拒否になる設定ミスを検出する（check23 不変）。未設定自体の警告は従来どおり `run()` の起動ログが担う。 |
| `AGE_DEFAULT_MINIMUM` | 任意（非秘密） | `21` | 年齢判定の一律既定最少年齢。保守的安全余裕ベースライン（法的事実の確定主張ではない）。有効設定値は `GET /policy/age` で常時公開。 |
| `AGE_COUNTRY_OVERRIDES` | 任意（非秘密） | 空 | 国別しきい値上書き（`CC=NN,...`）。 |
| `AGE_TARGET_REGIONS` | 任意（非秘密） | 空 | 対象地域コード（カンマ区切り）。 |
| `AGE_ENFORCEMENT_MODE` | 任意（非秘密） | `self_reported` | `self_reported` 系または `trusted_attribute_required` 系。後者は現時点でフェイルクローズ。非 UTF-8 の値は「未設定」ではなく不正値として、受理値を列挙した固定文言で起動拒否（fail-closed、値・生バイトはエコーしない）。`doctor` の `age_policy` カテゴリでも同じ警告を出す。 |
| `AGE_REQUIRE_EXPLICIT_CONFIGURATION` | 任意（非秘密） | `false` | 明示設定未完了時に年齢検証を拒否するフェイルクローズ選択肢。真偽値は `1`/`true`/`0`/`false` のみ。非 UTF-8 および受理値外は受理値を列挙した固定文言で起動拒否（fail-closed、値・生バイトはエコーしない）。`doctor` の `age_policy` カテゴリでも同じ警告を出す。 |
| `AGE_POLICY_CONFIGURED`（廃止） | — | — | 旧手動フラグ。読取を廃止し設定されても無視される。充足判定は `AGE_DEFAULT_MINIMUM` / `AGE_COUNTRY_OVERRIDES` / `AGE_TARGET_REGIONS` の実効値から導出（`AgePolicy::configuration_satisfied()`）。 |
| `TZ` | 任意 | 未設定（UTC として扱う） | 年齢判定の基準日（ローカルの「今日」）を決める。`Asia/Tokyo` は日本の固定オフセット UTC+9、`Etc/GMT±N` は POSIX 規約に従い符号反転（例: `Etc/GMT-9` = UTC+9）。未設定・未対応名は UTC の日付へフォールバック。JST 深夜では UTC 日付と境界が 1 日ずれ得るため、対象地域に合わせて明示設定する（[AGE_VALIDATION_POLICY.md](AGE_VALIDATION_POLICY.md) 参照）。読取は `configured_tz()`（`common::env::read_env` ベース）に一本化し、非 UTF-8 は `None`（UTC フォールバック）として doctor の固定文言警告対象にする。生バイトは保持・エコーしない。 |
| `PAYMENT_RECEIVING_ADDRESS` | 任意 | 未設定 | JPYC注文の受取アドレス（`0x` + 40 hex）。未設定・不正時は注文作成を501で拒否する。 |
| `PAYMENT_RECEIVING_XPRV` | 任意 | 未設定 | BIP44派生用のxprv。設定時は注文ごとの受取アドレスを派生し、`PAYMENT_RECEIVING_ADDRESS` より優先する。秘密管理基盤から注入し、ログへ出さない。 |
| `OIDC_ISSUER` | 任意 | 未設定 | デジタル認証サービスの issuer。OIDC を有効化する場合は必須。URL は https が必須（下記 OIDC URL の規約を参照）。 |
| `OIDC_CLIENT_ID` | 任意 | 未設定 | RP のクライアントID。 |
| `OIDC_CLIENT_SECRET` | 任意 | 未設定 | RP のクライアントシークレット。秘密管理基盤から注入する。 |
| `OIDC_REDIRECT_URI` | 任意 | 未設定 | 認可コードのリダイレクトURI。BFF のコールバックを指定する。 |
| `OIDC_AUTHORIZATION_ENDPOINT` | 任意 | 未設定 | 認可エンドポイント。 |
| `OIDC_TOKEN_ENDPOINT` | 任意 | 未設定 | トークンエンドポイント。 |
| `OIDC_JWKS_URI` | 任意 | 未設定 | JWK Set の URL。 |
| `OIDC_HTTP_TIMEOUT_SECS` | 任意 | `10` | OIDC の token/JWKS 取得のタイムアウト（秒、1〜60）。読取は `common::env::read_env` で一度だけ行い、範囲外・不正値・非 UTF-8 は既定10へフォールバックし `doctor` が `classify` ベースの固定文言で警告（値・生バイトは出力しない）。 |
| `OIDC_SCOPES` | 任意 | `openid` | 要求スコープ。属性取得時は対象スコープを追加する。 |
| `OIDC_COOKIE_SECURE` | 任意 | 有効 | 受理値は `1`/`true`（有効）と `0`/`false`（無効）のみ。`0`/`false` で refresh / OIDC state Cookie の `Secure` を外す（開発時のみ）。受理値外は起動拒否、`doctor` の `oidc_cookie_secure` カテゴリで警告し、Secure は外さない。本番の state Cookie は `__Host-` プレフィックス（Secure 必須）で保存される。 |
| `OIDC_CLOCK_SKEW_SECS` | 任意 | `60` | IDトークンの時刻検証に許容する秒数（1〜300、範囲外は既定60へフォールバックし `doctor` が警告）。 |
| `PINATA_JWT` | 本番では必須 | `mock_jwt`（模擬、開発用） | IPFSピン用トークン。本番相当（`ALLOW_MOCK_AUTH` 無効）では実トークンが必須。未設定・空・空白のみ・`mock_jwt`（前後空白を含む）は起動拒否（fail-closed）。非 UTF-8 は `common::env` 分類器が `EnvValue::NonUtf8` として区別し、`run()` と `doctor` が同じ固定文言 `PINATA_JWT is empty` を返す（生バイトはエコーしない）。`doctor` の `production` カテゴリで同じ判定を診断し、値は出力しない。 |
| `RUST_LOG` | 任意 | `info` | ログレベルフィルタ。 |
| `EXPOSE_INTERNAL_ERRORS` | 開発専用 | 無効 | 受理値は `1`/`true`（有効）と `0`/`false`（無効）のみ。`1`/`true` で500系の内部詳細を応答へ含める。本番では設定しない。受理値外は起動拒否、`doctor` の `expose_internal_errors` カテゴリで警告。 |
| `ALLOW_MOCK_AUTH` | 開発専用 | 無効 | 受理値は `1`/`true`（有効）と `0`/`false`（無効）のみ。`1`/`true` で模擬JPKI・ゼロ署名を受理する。本番では設定しない。受理値外は起動拒否、`doctor` の `mock_auth_enabled` カテゴリで警告。 |
| `ALLOW_IPV4_LOCALHOST` | 開発専用 | 無効 | 受理値は `1`/`true`（有効）と `0`/`false`（無効）のみ。`1`/`true` で127.0.0.0/8からのアクセスを許可する。本番では設定しない。有効時は `doctor --strict` の警告対象。受理値外は起動時に警告のうえ per-request 判定で `false`（IPv4 不許可、fail-closed）とし、`doctor` の `ipv4_localhost` カテゴリでも警告。 |
| `ALLOW_UNENCRYPTED_BACKUP` | 開発専用 | `0` | `1` で平文バックアップを許可する。ローカル開発専用の明示オプトアウト。オフサイト保存設定時は拒否。本番では設定しない。 |
| `BACKUP_GPG_RECIPIENT` | 本番では必須 | 未設定 | GPG 公開鍵のメール/ID。設定時にバックアップを GPG 暗号化する。未設定時は平文作成を拒否（fail-closed）。gpg（または `GPG_BIN`）不在時も失敗する。 |
| `AUDIT_EXPECT_ROOT` | 本番起動必須（開発 mock 時のみ任意） | 未設定 | 別経路で保存した監査 Merkle root（hex、非機微）。設定時は `backup-rotate.sh` が `verify_audit_log --expect-root` へ渡し、監査ログの空化・末尾切詰めを自動検証で検出する。本番相当（`ALLOW_MOCK_AUTH` 無効）では未設定・非 UTF-8・不正 hex（空・空白のみ・前後空白付きを含む）を固定文言で起動拒否（fail-closed、値・バイトはエコーしない）。判定は共有ヘルパ `production_audit_expect_root_error` が担い、DB 接続・マイグレーション・instance.lock・FK 検査より前に検証する。`doctor` は同じ判定を `audit_expect_root` カテゴリの固定文言警告へ反映する（値は非エコー、既定は警告のみで `--strict` のときだけ失敗）。`backup-rotate.sh` 自体は未設定時も内部整合性チェックのみで動作する従来契約を維持し、verify_audit_log / verify_audit_export の双方で root 値はエラー文言へエコーしない。監査ログ更新時は `verify_audit_log` の計算 root を `<prefix>-<stamp>.audit-root`（0600）へ世代保存し、成果物と同じ stamp 名で公開する（暗号化時も同じ世代名、root 値は stdout/stderr/ALERT 非エコー）。次回期待値への昇格は運用者明示承認の手作業で自動上書きしない。リストア時は復元対象世代に紐付いた root を `--expect-root` に渡す。 |
| `BACKUP_ENV_FILE` | 任意 | `/etc/openillustration/backup.env` | cron 版は `backup-cron-wrapper.sh` がこのパスを読む。systemd 版はこの変数を使わず、`EnvironmentFile=` の固定パス（同パス）から読む。権限600・所有者固定、秘密情報は配布物へ含めない。 |
| `APP_UNIT` | 任意（systemd 窓 unit） | `openillustration.service` | `deploy/openillustration-backup-window.service` が停止/起動するアプリ unit 名。`deploy/` にはアプリ unit を置いていないため、導入環境の実 unit 名へ drop-in 等で上書きする。バックアップ失敗時も `ExecStopPost` が起動を保証する。 |
| `BACKUP_DIR` | 任意 | リポジトリルートの `backups/` | バックアップ出力先。deploy 設定例では `/var/backups/openillustration`。事前に存在させ、権限を適切に設定する。 |
| `BIN_DIR` | 任意 | リポジトリルートの `target/debug` | `backup_db` / `verify_backup` / `verify_audit_log` のあるディレクトリ。無ければ `cargo run` へフォールバック（開発用）。本番はビルド済みパスを指定。 |
| `ALERT_CMD` | 任意 | 空（無効） | 失敗通知コマンドの実行ファイルパス。設定時は1引数（メッセージ）で呼び出し、通知が失敗しても元の終了コードを維持。成功時は通知しない。実行不可なら起動拒否（fail-closed）。 |
| `OFFSITE_COPY_CMD` | 任意（`OFFSITE_DEST` と対で設定） | 空（無効） | オフサイトコピーコマンドの実行ファイルパス。`<成果物> <宛先>` の2引数で呼び出す（eval 不使用）。実行不可なら起動拒否（fail-closed）。 |
| `OFFSITE_DEST` | 任意（`OFFSITE_COPY_CMD` と対で設定） | 空（無効） | オフサイト保存の宛先。片方だけの設定は起動拒否、コピー失敗時はスクリプト全体を失敗させる（fail-closed）。 |
| `E2E_PROJECTS` | 任意（E2E時） | `chromium`（ローカル）／`chromium,firefox,webkit`（CI 未指定時） | E2E で実行する Playwright プロジェクト（カンマ区切り）。明示時は指定を厳守し未知値は拒否する。CI（`CI` 環境変数設定時）は未指定でも3プロジェクトを明示指定として扱い、ブラウザ導入を選定より前に実行して firefox / webkit を必ず導入・実行する。chromium 単一へのフォールバックが CI で発生した場合は `::error::` を出して失敗する（黙って SKIP しない）。ローカル未指定時は firefox / webkit の実行可能性（導入済みバイナリと共有ライブラリが解決できるか）を検出し、両方可能なら3プロジェクト、片方でも不可なら警告付きで `chromium` へフォールバックする。GitHub Actions の step summary には firefox / webkit の可否と選択プロジェクトを記録する。 |

- 本番相当（`ALLOW_MOCK_AUTH` 無効）では `DATABASE_URL` が全プロセス（サーバーおよび DB 接続3 CLI）
  で必須。サーバーは未設定・非 UTF-8・trim 後空・旧既定の相対 `file:local.db`・
  リモート DSN を起動拒否し（fail-closed、値・トークンはエコーしない）、
  `file:/var/lib/openillustration/local.db` 等の絶対パスのみ指定する。
  リモート DSN は `is_remote_database_url`（`file:` で始まらず `://` を含む値）で判定し、
  `run()` と doctor の FK 検査用 `open_db_for_foreign_key_check()` が同じヘルパを共有する。
  doctor の FK 検査は `Unresolved`（未設定・非 UTF-8）時に `file:local.db` へフォールバックせず、
  検査失敗として fail-closed 警告を出す（値はエコーしない）。リモート DSN は doctor の検査対象なので
  誤って拒否しない。
  開発（`ALLOW_MOCK_AUTH=1`）では従来どおり未設定時に `file:local.db` を使い、
  リモート DSN も許容する。
  DB 接続3 CLI（`backup_db` / `sign_audit_export` / `verify_audit_log`）は暗黙既定値を撤廃し、
  `--database-url` または `DATABASE_URL` の明示設定がなければ起動を拒否する
  （空・空白のみも未設定として扱い、値はエコーしない / fail-closed）。
  `doctor` は本番相当で同じ共通判定を `production` カテゴリ内で警告する（カテゴリ数は据え置き、
  既定は警告のみで `--strict` のときだけ失敗）。
  deploy 設定例（[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)）は
  `file:/var/lib/openillustration/local.db` を明示済みで、設定値の変更は不要。
  手動実行でも接続先を明示し、誤った DB を検証して成功と誤信するのを防ぐ。
- OIDC の URL 設定（`OIDC_ISSUER` / `OIDC_REDIRECT_URI` / `OIDC_AUTHORIZATION_ENDPOINT` /
  `OIDC_TOKEN_ENDPOINT` / `OIDC_JWKS_URI`）は https が必須。`ALLOW_MOCK_AUTH` が有効な場合のみ
  `localhost` / `127.0.0.1` / `[::1]` / `::1` の http を許容し、それ以外の http や不正 URL は
  起動拒否（fail-closed）。`doctor` の `oidc_endpoint_urls` カテゴリでも同じ判定で警告し、
  設定値は出力しない。
- OIDC 設定の判定は `validate_oidc_config_with` に集約し、`from_env_with` と `doctor` が同じ結果を共有する。
  ハードエラーは起動拒否と `oidc_*` 警告に、本番必須設定の不足は `required_present` 経由で `production` 警告に使い分ける。
  必須値が非 UTF-8 の場合は「未設定」ではなく不正値として扱い、`enabled=true` のまま固定文言
  `OIDC configuration values must be valid UTF-8` をハードエラーとして起動拒否し、`doctor` の
  `oidc_endpoint_urls` カテゴリで同じ固定文言の警告を出す（生バイトはエコーしない）。
- `PINATA_JWT` は本番相当では実トークン必須。未設定・空・空白のみ・開発用 `mock_jwt`
  （前後空白を含む）は起動拒否（fail-closed）。非 UTF-8 は `common::env` 分類器が
  `EnvValue::NonUtf8` を区別し、`run()` と `doctor` が同じ固定文言
  `PINATA_JWT is empty` を返す（生バイトはエコーしない）。`doctor` の `production`
  カテゴリでも同じ判定で警告し、設定値は出力しない
  （[DETAILED_DESIGN.md](DETAILED_DESIGN.md) と整合）。
  判定は共有ヘルパ `pinata_jwt_production_error` へ集約し（R2-PINATA リファクタ完了）、
  `production_config_errors` と `pinata_jwt_production_like`（`run()` / `doctor` /
  `pin_to_ipfs`）が同じ単一情報源を共有する。`allow_mock_auth` の早期 return からは独立。
- 真偽値は全フラグ共通で `1` / `true`（真）と `0` / `false`（偽）のみ受理する
  （trim・小文字化は維持）。`yes` / `no` / 空文字を含むそれ以外の値は
  拒否し、ランタイムは起動拒否、起動後に read するフラグは安全側（`false` または
  Secure 付与）へ倒す。`doctor` は既存カテゴリ内で固定文言の警告を出し、不正値そのものは
  エコーしない。`common::age::parse_bool_env` が共通基準である。
  真偽値の状態からの採用値と不正時文言の導出は `common::env::bool_env_result` へ集約する
  （既定値と警告/起動拒否の切替を引数化）。`bool_env_at_startup` / `doctor` の
  `bool_flag_with_warning` / `sign_audit_export` の `bool_flag_from_env_value` は
  いずれもこの共通ヘルパへ委譲し、受理値列挙の固定文言
  `must be one of 1/true/0/false` は lib.rs / doctor / sign_audit_export の
  共通ヘルパ（common 側）に集約されている。OIDC 共有バリデータは
  変数名入りの独立文言を維持する（受理値は同一）。
  呼び出し側は変数名（と doctor の既定値）を付けるだけで、受理値・既定値・既存文言は不変。
- 起動時に受理値を検証し不正値を起動拒否するフラグ4種（`ALLOW_MOCK_AUTH` / `EXPOSE_INTERNAL_ERRORS` / `OIDC_COOKIE_SECURE` / `JWT_REQUIRE_KID`）
  は設定フェーズ（`run()` の OIDC 検証直後）へ集約して検証する。
  DB 接続・マイグレーション・`instance.lock` 取得・FK 検査より前に起動拒否が確定し、失敗時にこれらを残さない。
- `AUDIT_EXPECT_ROOT` も同じ設定フェーズの起動ゲートへ加える。`run()` は `PINATA_JWT` 読込直後に
  `read_env("AUDIT_EXPECT_ROOT")` し、`production_config_errors` と同じ箇所で `config_errors` へ追加する。
  判定は共有ヘルパ `production_audit_expect_root_error` に委譲し、本番相当では未設定・非 UTF-8・
  不正 hex を固定文言で起動拒否する。DB 接続・マイグレーション・`instance.lock` 取得・FK 検査より
  前に確定し、値も生バイトもエコーしない。
- 非 UTF-8 の環境変数値は「未設定」ではなく不正値として扱う。`common::env::read_env` が
  `EnvValue::NonUtf8` を返し、`bool_env_state` は `BoolEnvState::Invalid` に分類する
  （受理値の基準は既存 `parse_bool_env` に委譲）。必須設定（`PINATA_JWT` 等）は
  `production_config_errors` 経由で起動拒否、per-request で読むフラグ
  （`EXPOSE_INTERNAL_ERRORS` / `ALLOW_IPV4_LOCALHOST`）は `Valid(true)` 以外を
  安全側 `false` として fail-closed とし、`doctor` は既存カテゴリ内で固定文言の警告を
  出す。いずれも生バイトはログ・エラー文言へエコーしない。
   `AGE_*` も `std::env::var` と同じ Result 契約で読み、`NotPresent` のみ「未設定」とする。
   非 UTF-8（`NotUnicode`）は Invalid として `AgePolicy::from_env_with` が固定文言で起動拒否する
   （`AGE_REQUIRE_EXPLICIT_CONFIGURATION` は真偽値受理値列挙、`AGE_ENFORCEMENT_MODE` は
   モード受理値列挙）。`doctor` は同じ判定を `age_policy` カテゴリの固定文言警告へ反映する。
   いずれも値・生バイトはエコーしない。
- `JWT_RSA_PRIVATE_KEY_PEM` / `JWT_SECRET` / `JWT_RSA_VERIFICATION_KEYS` と
   `IPFS_GATEWAYS` / `IPFS_TIMEOUT_SECS` / `IPFS_MAX_ATTEMPTS` /
   `PAYMENT_RECEIVING_ADDRESS` / `PAYMENT_RECEIVING_XPRV` も
   `common::env::read_env` で一度だけ読み、未設定 / 非 UTF-8 / 有効値を区別する。
   JWT の非 UTF-8 は「未設定」と混同すると署名アルゴリズムが黙って切り替わるため
   固定文言で起動拒否とし、IPFS / 決済の非 UTF-8 は固定文言の警告のうえ未設定と同じく
   機能を無効化する（fail-closed）。`doctor` は
   `common::env::classify(lookup(..))` と共有分類関数 `env_value_for_use` で同じ区別を行い、
   `production` カテゴリ内へ固定文言の警告をマージする（check23 は不変）。
   いずれも設定値・生バイトをエコーしない。
- 年齢/地域ポリシーの有効設定値（`AGE_*` と `GET /policy/age` の応答内容）は非秘密かつ常時公開。新しいポリシー項目を追加する際も秘密情報を含めないこと。
- マイグレーションディレクトリの探索には開発便宜として `CARGO_MANIFEST_DIR` を参照する।

## 8. CI 静的ゲート

CI は共通ワークフロー `Seduce0068/workflows/rust-gates.yml@v1` の `extra_gates` へ
シェル静的ゲートを追加する。対象は `scripts/*.sh` と `e2e/*.sh` を glob で明示し、
`bash -n`（構文検査）、`shellcheck`（静的解析）、`shfmt -l`（未整形ファイルの列挙）の3 command を
既存ゲートに追加する。既存のゲート行は削除・意味変更せず維持し、
E2E69件 / check23 / doctor54 / migrations14 の各数値は不変である。
`shfmt` は `shfmt -l scripts/*.sh e2e/*.sh` の出力が非空（未整形ファイルあり）のとき失敗とし、
未整形ファイル名をエラー出力へ出す（`bash -n` / `shellcheck` の行は維持する）。
runner への `shellcheck` / `shfmt` の導入は、
呼び出し側 `.github/workflows/ci.yml` の `apt_packages` で指定する（共通ワークフローの
apt 導入ステップでインストールされる）。本リポジトリには install 手順を置かない
（未導入時に CI が赤化する構成を避けるため、導入要否はワークフロー側のコメントで明示する）。

- Rust の toolchain 下限は workspace の `[workspace.package] rust-version = "1.85"` に一元化し、
  `common` / `server` / `client` は `rust-version.workspace = true` で継承する（edition 2024 の下限）。
  リポジトリには `rust-toolchain.toml` を置かず、`.github/workflows/ci.yml` にも `toolchain`
  指定は存在しないため、将来的に指定を追加する場合は必ず 1.85 へ合わせる。
  `doctor` 自体は toolchain を指定せず、この MSRV でビルド・実行する。

## 9. CI 前提条件の黙スキップ禁止

- `scripts/test-backup-rotate.sh` のケース8（実 gpg による暗号化→復号→バイト一致）は、
  gpg 不在・使い捨て鍵生成失敗・fingerprint 取得失敗時にローカルでは
  `SKIP case 8: <理由>`（PASS とは別の専用出力）を出す。CI（`CI` 設定時）では
  同じ条件を `FAIL case 8: required precondition missing in CI (<理由>)` として失敗させ、
  前提条件の欠落を黙って PASS にしない。CI runner には
  `.github/workflows/ci.yml` の `apt_packages` で `gpg` を導入する
  （`shellcheck` / `shfmt` は維持）。既存の `expect_failure` / `expect_success` 件数は不変。
- E2E のブラウザ導入は選定（キャッシュ走査）より前に実行する。CI では
  `E2E_PROJECTS` 未指定時に `chromium,firefox,webkit` を明示指定として扱い、
  firefox / webkit を必ず導入・実行する。chromium 単一へのフォールバックは
  CI では `::error::` アノテーションを出して失敗とし、黙って SKIP しない。
- IPFS ゲートウェイは本番相当（`ALLOW_MOCK_AUTH` 無効）では `https` のみを受理し、
  `http` は mock 有効時のみ受理する。拒否された値はログへ出力しない。
  既存テストの期待値は mock 有効ケースで不変である。

## 10. クライアント UX と依存監査の方針

- **件数告知のゲート**: ダッシュボードの `role=status aria-live=polite` は
  取得成功時のみ「`{}件のコンテンツを表示中`」を読み上げる。エラー時は前回成功値の
  件数を日本語の取得失敗告知へ切り替え、古い件数を読み上げない。
  既存の固定エラー文言（`CONTENT_INDEX_*` 等）は変更しない。
- **ファイル選択のタブ停止点**: 隠し `input[type=file]` は `tabindex="-1"`
  と `aria-hidden="true"` を付け、対応する `label` を唯一のタブ停止点にする。
  ラベルには `focus-visible` のフォーカス枠を付け、キーボード操作で
  ファイルピッカーを開けることを E2E で確認する。
- **リリースビルドのログ方針**: `EXPORT_RESULT` は `#[cfg(feature = "dev-console")]`
  でゲートしてリリースでは出力しない。一方 `warn_history_update_once` は
  economy.spec.ts の履歴警告オラクルを維持するため、リリース wasm でも固定文言
  `HISTORY_UPDATE_WARNING` を1回だけ `warn_1` で出力する（値・URL・JsValue は非エコー）。
- **エラー文言と言語宣言**: 既存固定文言は一切変更せず、英語の文言を出す
  `role=alert` / `role=status` 要素へ共通ヘルパ `error_lang` で `lang="en"` を付与し、
  日本語文言には `lang="ja"` を付与する（ルートは `lang="ja"` のまま）。
  ヘルパは favorite / upload / vote / content のエラー表示で共有し、
  文言ごとの属性付け忘れによる drift を防ぐ。
- **依存監査**: `.github/workflows/ci.yml` に週次 `schedule` トリガを追加し、
  `cargo deny check advisories` を定期実行する。既存の push / pull_request /
  workflow_dispatch トリガは維持する。`npm ci` の `--no-audit` を外し、
  `npm audit --audit-level=high` を `extra_gates` へ追加する。
  監査対象の npm 依存はルート / `e2e/` / `client/` の3 lockfile（いずれも
  `devDependencies`）で、本番ランタイムへはインストールしない。
  Dependabot は外部サービス設定（`.github/dependabot.yml`）のため本方針では追加せず、
  CI ゲート側で検出を担保する。

### R2-CLIENT-UX の契約（2026-11-22）

- **アップロード入力の絞り込み**: ファイル選択 UI の `input[type=file]` は
  JXL / Markdown の拡張子・MIME（`accept` 属性）のみを提示する。ブラウザが
  `accept` を迂回する経路（テストからの直接設定等）に備え、送信前にも
  `upload_media_type` を判定し、空なら専用固定文言を `role=alert` で出す。
  サーバー400へ遠回りさせず、生ファイル名や MIME はエラー文言へ含めない。
- **アクセシブル名と言語宣言の整合**: 英語固定見出し（Content Load Failed /
  JXL Decode Failed / Novel Decode Failed）とログイン画面の英語パラグラフは
  `lang="en"` を宣言し、日本語の詳細文言は `lang="ja"`（ルート既定）を維持する。
  既存固定文言・data-testid は変更しない。
- **認証失効の案内分岐**: 公開集計 `/mod/stats/public` が401のときだけ
  「再ログインが必要です。」と LOG OUT からの再ログイン案内を表示する。
  403 / 5xx / ネットワークエラーは「集計の取得に失敗しました。」を維持し、
  権限情報やサーバー本文（センチネル等）を画面へ出さない。
  既存 E2E ケース内へ統合し、E2E69件 / check23 / doctor54 / migrations14 /
  tests148 の数値は不変。
- **R2-CLIENT の運用契約**: 投票一覧取得失敗中は投票ボタンを disabled にし、
  監査タイムラインはパネルを開くたびに再取得する。アップロード送信中は
  ファイル差し替えを拒否して送信バイト列を固定し、TOC の未知 hash からの
  goBack は直前の既知 view へ正規化する。ログアウト API 失敗は dedicated
  `role=alert` で再試行を案内するが画面遷移は現行どおりログインへ戻す。
  集計 GENERATED_AT は整形表示して生 ISO を title 属性へ保持し、エクスポートの
  object URL は十分な余裕（60秒）後に失効させて失敗時は再試行案内を出す。
  いずれも既存 E2E ケース内へ統合し、E2E69件 / check23 / doctor54 /
  migrations14 / tests148 の数値は不変。CI の E2E ゲートは `bash e2e/ci-e2e.sh`
  を唯一の合否根拠とし、`npm test` は開発用ローカル実行として扱う。

### R2-CLIENT 追加契約（2026-11-23）

- **投票前の本文確認と通報導線**: 投票キューの `target_cid` は BROWSE の該当カードへ
  ディープリンクし、同じセッションで本文と監査タイムラインを確認してから投票する。
  カード側の `Report` ボタンは CID を `report_cid` クエリで渡し、MODERATION の
  通報フォームへプリフィルする。hash / query の正規化は両キーを保持し、
  未知キーやエンコード差でリンクが壊れないようにする。
- **英文 UI と言語宣言**: 検索フォームと Support オーバーレイの英文には静的な
  `lang="en"` を付与する。エラー文言の `error_lang` / `role=alert` 方針は変更せず、
  新規の英文は非エラー UI に限る。
- **エクスポート表示の機微情報方針**: 成功通知には整形済み `generated_at` と
  短縮 `sub` だけを dedicated 要素で出す。生 ISO、全文 `sub`、JWT、
  トークンハッシュは画面へ一切出さず、ダウンロード JSON の保存契約は不変。
- **公開集計の鮮度とオプインの分離**: 公開集計の `GENERATED_AT` が15分を超えた場合に
  鮮度警告を出し、「更新」操作では公開集計のみを再取得する。認証付きオプイン集計
  （`/mod/stats`）のマウント条件・リクエスト本数・権限判定は変更しない。
  いずれも既存 E2E ケース内へ統合し、E2E69件 / check23 / doctor54 /
  migrations14 / tests148 の数値は不変。

### R2-CLIENT（第2次）追加契約（2026-11-23）

- **公開集計の鮮度と順序**: 「更新」操作は最新のレスポンスのみを採用し、
  古い応答が遅れて到着しても集計表示を上書きしない。取得中は更新ボタンを
  `disabled` / `aria-busy=true` として連打を受け付けない。
- **履歴とディープリンク**: 戻る／進むでカードの強調表示と通報フォームの
  CID を URL から復元する。未知 hash を既知 view へ正規化する従来契約は維持する。
- **進捗と読み込みの告知**: ログインと取引履歴の取得中は dedicated
  `role=status` で日本語の進捗を告知し、空状態は取得完了後のみ描画する。
- **言語宣言と必須表現**: 全 `role=alert` / `role=status` は `error_lang` を
  経由して言語を宣言する（ルート既定への暗黙依存を使わない）。通報フォームは
  `required` を使わず `aria-required` と独自 `role=alert` で検証し、
  Enter 経路も同じ検証へ集約する。
- いずれも既存 E2E ケース内へ統合し、E2E69件 / check23 / doctor54 /
  migrations14 / tests148 の数値は不変。

### R2-ENV 追加契約（2026-11-23）

- **LOG_FORMAT の分類と doctor 診断**: `init_tracing()` の読取を
  `common::env::read_env("LOG_FORMAT")` へ移し、未設定と `json` / `text`
  （大小文字不問）のみを受理する。タイポ・非 UTF-8 は固定文言
  `LOG_FORMAT must be json or text; using text (invalid value)` の警告のうえ
  テキスト形式へフォールバックする。共有判定 `log_format_state` を `doctor` も使い、
  subscriber 初期化後に警告を出すため、`--strict` で設定ミスを検出できる。
  値・生バイトは一切エコーしない。`results.push` を増やさず既存 `production`
  カテゴリへマージするため check23 は不変で、doctor のテスト数は 53→54 へ追随した。
- **数値系 env の read_env 一本化と classify 統一**: `RATE_LIMIT_PER_MIN` /
  `MAX_CONCURRENT_REQUESTS` / `OIDC_HTTP_TIMEOUT_SECS` /
  `MODERATION_FINALIZE_INTERVAL_SECS` / `GRACEFUL_DRAIN_TIMEOUT_SECS` を
  `common::env::read_env` で一度だけ読み、`numeric_env_value` がパーサ入力と
  固定警告を導出する。非 UTF-8 は `<NAME> is not valid UTF-8; using default` の
  固定文言警告のうえ既定値へフォールバックし、`doctor` も `classify` ベースで
  同じ警告を出す。これにより「不正値は既定へフォールバックし doctor が警告する」
  という契約を実装と一致させた（値・生バイトは非エコー）。
  E2E69件 / check23 / migrations14 / tests148 は不変。

### R2-SERVER 追加契約（2026-11-23）

- **write_lock の poison を Result 化する**: `DB_WRITE_LOCKS` の
  `Mutex::lock()` とキー登録を含むロックテーブル操作全体を `Result` で包み、
  poison 時に panic（`expect`）せず `AppError::Internal` を返す。
  `write_lock()` の呼び出し側（`begin_write_tx_sql` / `acquire_purge_lock` /
  `finalize_due_reports`）は `?` で伝播し、purge 系のエラー型も `AppError` へ
  統一する。OIDC の refresh 等は `begin_write_tx().await?` を通じで同じ
  `AppError::Internal` を受け取る（oidc.rs 側の呼び出しは既に `?` 付き）。
- **UNBAN 時の revoked_subs 行不存在フォールバック**: `unban_user` は
  `UPDATE revoked_subs ... WHERE sub = ?` の影響件数を検査し、0 件なら
  同じ tx 内で `INSERT` して閾値行を作成する。`users.is_banned = TRUE` なのに
  `revoked_subs` 行が欠落していても旧 JWT が UNBAN を境に復活しない。
  既存の BAN/UNBAN 監査と positive-only キャッシュ更新の契約は不変。
- **監査連鎖カーソルの read-then-write 順序**: `update_content_status` と
  `process_report_conclusion` は `status` / `entry_hash` / 投票集計を読む
  `Rows` を `next()` 直後に `drop(rows)` し、その後に `tx.execute` を発行する。
  `migration.rs` と同じ順序に統一し、読み取りカーソルを残したまま書き込まない。
  既存のハッシュ連鎖・ログ ID 連続性・Merkle root の契約は変更しない。
  新規テスト関数は追加せず、E2E69件 / check23 / doctor54 / migrations14 /
  tests148 の数値は不変。
