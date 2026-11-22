---
title: "詳細設計書 (Detailed Design Document)"
type: Design
description: API、認証、データ設計および未実装事項を記録する。
tags: [detailed-design, api, security]
generated:
  by: dsh
  at: "2026-11-22T15:00:00+09:00"
---

## 詳細設計書 (Detailed Design Document)

本書は現行APIを記録するとともに、必須要件を満たす目標設計・未実装事項を明確にする。
JPKI、JPYC、Rust優先の方針は [ADR-0001](adr/ADR-0001-mvp-trust-boundaries.md) を参照。

## 1. API仕様

実装ルート（`server/src/lib.rs`、`/me/export` は `server/src/export.rs`）:

| Method | Path | 認証 | 用途 |
| --- | --- | --- | --- |
| POST | `/auth/login` | 不要 | モック有効時（`ALLOW_MOCK_AUTH=1`）のみ公開。OID4VP風のnonce応答で完全な認証フローではない。無効時は501 |
| POST | `/auth/verify` | 不要 | 開発用モックまたは証明書入力を処理しJWTを発行 |
| GET | `/auth/oidc/start` | 不要 | state/nonce/PKCEを生成し認可URLを返す。OIDC未設定時は501 |
| GET | `/auth/oidc/callback` | 不要 | 認可コードを交換し、IDトークン検証後にセッションJWTを発行する |
| POST | `/auth/refresh` | Cookie | リフレッシュトークンを回転し新セッションJWTを発行。再利用検知時はファミリ失効 |
| POST | `/auth/logout` | Bearer JWT | `sid` とリフレッシュファミリを失効させ、`refresh_token` の削除 Cookie（`Max-Age=0` / `Path=/auth` / `HttpOnly` / `SameSite=Strict`、Secure は `OIDC_COOKIE_SECURE` に追随）を返す |
| POST | `/auth/backchannel-logout` | 不要 | logout token を検証し `sid` および/または `sub` を失効させる（`sub` は `revoked_subs` テーブルへ永続化し起動時にロード） |
| GET | `/health` | 不要 | プロセスの簡易応答 |
| GET | `/ready` | 不要 | readiness。DB接続と停止フラグのみを根拠とし、停止シグナルで503へ切り替わる |
| GET | `/policy/age` | 不要 | 有効な年齢/地域ポリシー設定値（非秘密）を返す。常時公開。 |
| GET | `/metrics` | 不要 | Prometheus 形式の軽量カウンタ8種（全リクエスト数・4xx/5xx 応答数・中立化URL件数・finalizer/purge の ticks/processed/lock_busy/failures/last_success_timestamp）。外部クレートなし。 |
| POST | `/metrics/client-events` | 不要 | 中立化URL件数を受信。`{"event":"neutralized_url","count":N}`（`event` は固定値、`count` 既定1・範囲1〜1000、`deny_unknown_fields`、本文1 KiB 境界）。カウンタ加算は N によらず1リクエストにつき1固定。成功204・不正400（いずれも本文なし）。IP/User-Agent/URL は送らず記録もせず、レート制限の対象。 |
| GET | `/content` | Bearer JWT | `Approved` コンテンツを一覧返す。省略可の `q` / `tag` / `type` / `limit` / `offset` で検索・絞り込み・ページング（既定 `limit=50`、範囲 `1..=100`、`offset=0`）、`favorite_only` で本人のお気に入りへ絞り込み |
| GET | `/content/:cid` | Bearer JWT | IPFS取得、サイズ・チェックサム確認後に返す |
| POST | `/content/:cid/favorite` | Bearer JWT | お気に入り追加。`Claims.sub` のみで識別し、本人の行にだけ作用。重複追加は冪等で200 |
| DELETE | `/content/:cid/favorite` | Bearer JWT | お気に入り削除。同上の識別。未登録の削除も冪等で200 |
| GET | `/content/:cid/audit` | Bearer JWT | DB内監査記録を返す。最新 MAX_AUDIT_LIST(500) 件を `log_id` 新しい順（DESC）で返し、古い行はAPIから参照不可（完全検証には署名付きエクスポート/DBを使用）。`reason` は `summarize_reason` で要約して返し、`entry_hash` は DB の生値を維持する。レスポンスには `previous_log_hash` が無いため、レスポンス単体でのハッシュ連鎖検証は成立しない（詳細は「6. 監査ハッシュ」） |
| POST | `/content/upload` | Bearer JWT | コンテンツ投稿。任意の `title` / `description` / `tags` も受理し長さ上限を検査 |
| GET | `/transactions` | Bearer JWT | 既存の記録を最大100件返す。実決済証明ではない |
| POST | `/payments/orders` | Bearer JWT | `cid`・`chain_id`・`wallet_address`・`amount_jpy` を受け取り、コンテンツの提示価格と `amount_jpy` を照合して JPYC注文を作成する。不正CID・不存在/レビュー中コンテンツ・価格不一致は拒否。受取アドレス未設定時は501 |
| GET | `/payments/orders/:order_id` | Bearer JWT | 本人の注文状態を返す |
| POST | `/payments/orders/:order_id/bind` | Bearer JWT | ウォレット所有証明を検証する。署名対象は注文ID・chain_id・wallet_address・recipient_address・amount_jpy（v2）。注文IDのみの旧形式署名は盲署名対策として401で拒否 |
| POST | `/payments/orders/:order_id/verify` | Bearer JWT | 送金を検証する。RPC検証未設定時は501 |
| POST | `/content/:cid/support` | Bearer JWT | 無効化済み（501）。模擬ハッシュは支払証明にならないため、JPYC送金検証の実装時に決済APIへ置換する |
| POST | `/mod/report` | Bearer JWT | 通報登録と `Reported` 遷移 |
| GET | `/mod/reports` | Bearer JWT | `Reported` コンテンツの通報一覧 |
| POST | `/mod/vote` | Bearer JWT | 投票署名検証、投票登録、閾値判定 |
| POST | `/mod/reports/{report_id}/finalize` | Bearer JWT + 運用者 | 期限到達済み通報を集計確定（冪等）。期限内・確定済みは変更しない。手動は運用者限定（未設定・未登録・BAN済みは403）、自動確定スケジューラは認可対象外 |
| POST | `/mod/operators/{user_hash}/ban` | Bearer JWT | 運用者特権。`MODERATION_OPERATOR_HASHES` 登録者のみBAN可能。根拠必須。`duration_days`（1〜3650、null=無期限）から期限を設定し `governance_audit_log` に記録 |
| POST | `/mod/operators/{user_hash}/unban` | Bearer JWT | 運用者特権。同上の運用者のみUNBAN可能、`governance_audit_log` に記録 |
| POST | `/mod/content/{cid}/unpin` | Bearer JWT | 運用者特権。ピン解除の判断と理由を `governance_audit_log` に記録（実Pinata操作は手動） |
| GET | `/mod/stats` | Bearer JWT + 運用者 | 認証内集計。`AuthClaims` 必須かつ `require_moderation_operator`（未設定・未登録・BAN済みは403）。`generated_at` / `totals` / `monthly` のみをホワイトリスト方式で返し、CID・reason・`user_hash` は返さない |
| GET | `/mod/stats/public` | Bearer JWT | 認証済み利用者向けの公開集計（ADR-0008 段階2）。`AuthClaims` 必須、`require_moderation_operator` は呼ばないため未認証401・認証済みは運用者でなくても200。`generated_at` / `totals`（`reports` / `concluded` / `approved` / `rejected` / `quorum_not_met`）/ `monthly`（`month` と reports / concluded / approved / rejected）のみを返し、bans/unbans/unpins と CID・reason・`user_hash` は型に存在しない |
| GET | `/me/export` | Bearer JWT (`AuthClaims` 必須) | 自分のデータのエクスポート（R5-04）。`version:1` / `generated_at` / `sub` と12表（`users`, `contents`, `favorites`, `transactions`, `signatures`, `content_audit_log`, `reports`, `votes`, `payment_orders`, `governance_audit_log`, `refresh_tokens`, `revoked_subs`）の配列を返す。全行を返し行数上限・`truncated` フラグは設けない。`reason` / `details` は `mask_reason`（既定でマスク）、`token_hash` / `sid` / `family_id` / 署名鍵 / XPRV / `PINATA_JWT` / 他ユーザーID は除外。`Content-Disposition: attachment`、`Cache-Control: no-store`。本文はログへ出さない。12表は単一の読み取りトランザクション（`BEGIN DEFERRED`）で読み切る |

### API横断規則

- JWTはHS256既定検証を用い、有効期限を確認する。起動時に32バイト未満の `JWT_SECRET` を拒否する。`JWT_RSA_PRIVATE_KEY_PEM` 設定時は RS256 となり、署名ヘッダに公開鍵由来の `kid`
   を付与する。検証は `JWT_RSA_VERIFICATION_KEYS` に登録した `kid` → 公開鍵で行い、未知 `kid` は拒否する（fail-closed）。`kid` 無しの旧トークンは現行署名鍵で検証するが、
  `JWT_REQUIRE_KID=1` 設定時はこれを拒否する（本番推奨、RS256 限定・HS256 併用時は起動拒否）。発行者/audience検証は別途要件化する。
- CORSは `ALLOWED_ORIGINS`（カンマ区切り）で設定する。`ALLOW_MOCK_AUTH=1` の開発時のみ、未設定ならローカル開発オリジン（`localhost`/`127.0.0.1`/`[::1]`
  の8080）へフォールバックする。モック認証が無効な本番相当で未設定なら全オリジンを拒否（fail-closed、警告ログ付き）。空エントリと `http(s)://` 形式以外（`null` を除く）は無視する。
  末尾スラッシュは除去し、ワイルドカードは指定できない。本番配信オリジンの決定と値の運用は未決定。
- 本番相当（`ALLOW_MOCK_AUTH` 無効）のみ、IP 単位のレート制限と同時実行上限を適用する。
  `RATE_LIMIT_PER_MIN`（既定60/分、有効範囲1〜600）を超過、または `MAX_CONCURRENT_REQUESTS`
  （既定16、有効範囲1〜256）の同時実行数を超過した場合は 429 を返す。不正値・範囲外は既定値を使う
  （上限256超過はクランプせず既定16へ倒れる）。同時実行 permit は `tokio::Semaphore` から取得し、
  内側のハンドラがレスポンスを返した時点で `drop` して返却する（レスポンス完了で解放）。
  境界値（`test_max_concurrent_requests_defaults_and_bounds`）と上限超過の 429・permit 返却
  （`test_concurrent_request_limit_rejects_second_and_releases_permit`）をテストで固定済み。
  `/health`・`/ready`・`/metrics` は監視用のため除外し、`ALLOW_MOCK_AUTH=1` の開発時は適用しない。
- リクエスト本文は最大10 MiB。`Content-Length` が上限を超える場合はミドルウェアが Multipart 解析より先に 413 を返す（単体テストと実サーバーで確認）。`Content-Length`
  が無い／虚偽のチャンクド転送は axum の本文制限と Multipart パーサが守り、その場合は 400 になる（多層防御）。IPFS取得応答は `IPFS_TIMEOUT_SECS`（既定15秒）でタイムアウトし、最大10
  MiB（超過は 400）。
- IPFS 取得時にチェックサム不一致を検出した場合は 401 で拒否し、CID と redact 済みゲートウェイ URL のみを WARN ログへ記録する（本文・チェックサム値は出さない）。実サーバーで警告出力を確認済み。
  ゲートウェイは設定順に試し、ネットワークエラー（接続失敗・タイムアウト）に限り `IPFS_MAX_ATTEMPTS`（既定1、最大5）まで同じゲートウェイを再試行する。
  HTTP クライアントは `build_http_client`（`server/src/lib.rs`）で
  `reqwest::redirect::Policy::none()` を設定し、すべてのリダイレクトを追従しない（SSRF 対策）。
  3xx は `Location` 先へ接続せずレスポンスとして呼び出し側へ返り、2xx 以外の HTTP 応答
  （3xx を含む）は次のゲートウェイへフェイルオーバーする。サイズ超過・不正データ
  （チェックサム不一致等）は次のゲートウェイへ送らず即拒否する。OIDC の HTTP クライアントも
  同じ `build_http_client` を共有する。
- 接続元 IP の判定（`server/src/network.rs`）: `TRUSTED_PROXY_CIDRS` 未設定時は XFF ヘッダー
  全体を無視しピアアドレスのみを使う。設定時は `X-Forwarded-For`（XFF）のカンマ区切り複数値を
  解釈し、末尾が接続元ピアと一致する「追記型」では末尾のピア（複数ホップで連続し得る）を
  すべて除去して残ったチェーンの最左値＝元のクライアントを採用する。末尾がピアと一致しない
  「上書き型」では従来どおり末尾を採用する。1 つでも IP として解釈できないエントリがあれば
  None を返して fail-closed で拒否する。
- IPv4 拒否ゲート（SEC-01）: IPv4-mapped IPv6（`::ffff:a.b.c.d`）に加え IPv4-compatible IPv6
  （`::a.b.c.d`）も IPv4 として正規化して拒否する。ただし `::`（unspecified）と `::1`
  （loopback）は IPv6 のまま保持し、既存の IPv6 ループバック運用を壊さない（NET-02）。
- 全レスポンスへ `X-Content-Type-Options: nosniff` と `X-Frame-Options: DENY` を付与する。
  `security_headers` は `CorsLayer` の内側と外側（最外側）の2か所に置く。CORS プリフライト
  （OPTIONS 204）は `CorsLayer` で短絡して内側を通らないため、`CorsLayer` のさらに外側にも
  `security_headers` を置く（既存の内側の層は維持）。これで IP ゲートの 403、レート制限の 429、
  500、本文なしの 204、CORS プリフライト応答のすべてにヘッダーが付く（二重付与でも同値）。
  初期3ケース（プリフライト/429/500）に加え 204 も回帰テストで固定済み。
  `/content/{cid}` のユーザー管理バイトが HTML としてスニッフィングされるリスクを防ぐ。
  CSP はクライアントのリソース読み込みへ影響するため本番構成確定時に別途設計する。
- メトリクス観測（`observe_metrics`）は `Router` の最も外側（`security_headers` のさらに外側）に積む。
  内側のミドルウェアで短絡するレート制限の 429・IP ゲートの 403・本文サイズ拒否の 413・
  CORS プリフライトの OPTIONS 204 も取りこぼさず、
  `openillustration_http_requests_total`（全リクエスト）と
  `openillustration_http_errors_total`（4xx/5xx）へ計上する。429/403/413 は requests と
  errors の双方を、204 は requests のみを加算する（実装: `server/src/lib.rs` の
  `build_router` / `observe_metrics`）。
- 一覧検索の `q` / `tag` は SQL `LIKE` のパターンとして扱うため、`\` / `%` / `_` をバックスラッシュで
  エスケープし `ESCAPE '\'` 句で照合する（値はすべてバインドパラメータで渡し、SQL 文字列へ埋め込まない）。
  ユーザーが登録したメタデータ（タイトル・説明・タグ）はサーバーでエスケープせず原文を保持・返却し、
  描画境界のクライアントが `escape_html_text` で `&` `<` `>` `"` `'` を実体参照化してから
  `inner_html` へ渡す（XSS 対策は表示側の責務）。
- 500系エラーの内部詳細（DB・IO・内部メッセージ）はログのみに出し、クライアントには `Internal server error` を返す。開発時は `EXPOSE_INTERNAL_ERRORS=1` で詳細を返す。ログへ
  URL を出す際は `redact_url` で userinfo・query・fragment を除去する（フラグメントの秘密漏えいを防ぐためパース失敗時のフォールバックでも `#` 以降を落とす）。
- Rust コードの `unsafe` は禁止方針。CI の clippy に `-D clippy::undocumented_unsafe_blocks` を必須化し、やむを得ず使う場合は安全性根拠のコメントを要求する。現行の
  `unsafe` はテストヘルパーの環境変数操作（2関数）のみで、`ENV_LOCK` による直列化を根拠としている。
- ログは `tracing` + `tracing-subscriber` で標準出力へ出し、`RUST_LOG` でレベルを制御する（未設定時は INFO、タイムスタンプは RFC 3339 UTC）。`LOG_FORMAT=json`
   設定時は 1行1 JSON（timestamp / level / fields / target）で出力しログ集約基盤へ取り込める。起動メッセージも tracing 経由に統一し、JSON モードでパース失敗する行を作らない。
  JWT・認証ヘッダ・投稿本文など機微情報はログへ出さない。
- 停止時は SIGTERM/SIGINT を捕捉し、処理中リクエストの完了を待って終了する（graceful shutdown）。待ち時間は `GRACEFUL_DRAIN_TIMEOUT_SECS`（既定 30 秒、範囲
  1〜300）で上限を設け、超過時は警告ログを出して残接続を切断する。停止シグナルで readiness は 503 へ切り替わる。readiness は DB 接続と停止フラグのみを根拠とし、
  外部依存（IPFS/JPKI/チェーン）の障害は反映しない。外部依存の検知はエラーカウンタと WARN ログで行う。
- 起動時はローカル DB の `<DB path>.instance.lock` を原子的に作成し、単一ライターを強制する。生存 PID のロックがある場合は第二インスタンスの起動を拒否（fail-closed）。クラッシュ後の
  stale ロックは自動回収し、正常停止時に削除する。リモート DB はこの排他が使えず警告のみ。
- 本番相当（`ALLOW_MOCK_AUTH` 無効）では起動前に OIDC 必須設定（`OIDC_*`）と実 `PINATA_JWT`
  を検査し、未設定・空・開発用 `mock_jwt` なら起動を拒否する（fail-closed）。エラーには
  不足項目名のみを出し、秘密値は出さない。開発用モック認証時はこの検査を適用しない。

## 2. 認証・認可（必須要件 / 現行実装との差分）

目標方式は [ADR-0002](adr/ADR-0002-jpki-authentication.md) を参照。

- 指定マイナアプリ/JPKIで認証できない利用者は、閲覧を含むサイト機能を利用できない。
- 現行の閲覧系ルート（`GET /content`、`GET /content/:cid`、
  `GET /content/:cid/audit`、`GET /transactions`、`GET /mod/reports`）は
  JWT認証を要求する。OIDC認証でJWTを発行するフローは実装済みだが、
  実デジタル認証サービスへの接続・事業者申込・実機検証は未完了であり、
  本番要件を満たすには追加の検証が必要。
- `/health`、`GET /policy/age`、およびJPKI認証開始・静的ログイン画面は認証前アクセスを許容できるが、コンテンツや利用者データを返さない。`GET /policy/age`
  は有効な年齢/地域ポリシー設定値（非秘密）のみを返し、常時公開する。
- 開発用モックは `ALLOW_MOCK_AUTH=1` の場合のみ許可する。未設定時は無効。
- 認証が必要な読み取りルートは `AuthClaims` によりJWTを検証する。未認証アクセスを拒否するが、本人確認の真正性を確立するものではない。
- `/auth/login` と `/auth/verify` は開発用モック経路。`ALLOW_MOCK_AUTH=1` の場合のみ有効で、login が発行する nonce を verify で必須化し、5分 TTL
  の単回消費（リプレイ拒否）とする。VP 署名検証は未実装であり、本番では OIDC 経路を使用する。
- クライアントの `Login` は `error` シグナルを保持し、失敗を `<p role="alert" data-testid="login-error">` に
  表示する（サイレント失敗しない）。成功時にのみ error をクリアして認証済み画面へ進む。内部詳細は出さない。
- `Login` の error 文言は `/auth/login` と `/auth/verify` の失敗種別で次を区別する。
  - HTTP エラー: `認証に失敗しました (HTTP <status>).`
  - 通信失敗: `ネットワークエラーが発生しました。`
  - JSON 解析失敗: `認証要求の解析に失敗しました。` / `認証結果の解析に失敗しました。`
  - `authorized=false`: `認証が許可されませんでした。`
  - `user_hash` / `token` の欠落（情報不足）: `認証情報が不足しています。`
- クライアントの `Upload` は成否を `data-testid="upload-status"`（`[STATUS]` 接頭辞付き）へ表示し、
  内部例外や生データを画面へ出さない。準備失敗は `アップロードの準備に失敗しました。`、
  応答読み取り失敗は `アップロード結果の読み取りに失敗しました。` を使う。通信／HTTPエラーは
  `upload_error_message` の4分岐で選ぶ: 401=再ログイン（`再ログインが必要です。`）、
  413=サイズ超過（`ERROR: File exceeds the 10 MiB limit.`）、HTTP その他=
  `Failed to upload content (HTTP {status}).`、None（ステータス取得不可）=
  `Network error uploading content.`。ファイル未選択・クライアント側の10 MiB超過は
  それぞれ `ERROR: Select a file first.` / `ERROR: File exceeds the 10 MiB limit.` を表示する。
  成否にかかわらず `finally` 相当で `loading` を解除し、ボタンを再活性化する
  （パニックで loading が戻らない事故を防ぐ）。
- `server/src/jpki.rs` は提示データからX.509証明書を解析し公開鍵を抽出するが、自己署名は発行者=主体の DN 一致
  に加え証明書自身の公開鍵による暗号学的署名検証でも拒否する。信頼チェーン、失効、VP署名、nonce、audience検証
  を完了していない。実JPKI認証として使用しない。
- 年齢判定は `common::age::AgePolicy`（起動時に環境変数から読込・検証）に基づく。既定は保守的一律ベースライン `21`（法的事実の確定主張ではない）で、
  国別上書き・対象地域・適用モード・明示設定必須のフェイルクローズを構成可能。`trusted_attribute_required` は検証済み属性入力経路が未実装のため現時点でフェイルクローズ。現行の生年月日・国コードは申告値であり、
  モック環境を含め本人確認済み属性として信頼しない。詳細は [AGE_VALIDATION_POLICY.md](AGE_VALIDATION_POLICY.md) を参照。
- 年齢判定の基準日は `TZ` 環境変数で指定したローカルの「今日」。`validate_age`（`/auth/verify`）は
  `std::env::var("TZ")` を読み、`validate_age_at` → `today_for_policy` で日付を求める。`Asia/Tokyo` は
  日本の固定オフセット UTC+9、`Etc/GMT±N` は POSIX 規約に従い符号を反転（`Etc/GMT-9` = UTC+9）して解釈し、
  未設定・未対応の TZ 名は UTC の日付へフォールバックする（fail-safe）。この符号反転と範囲判定は
  `server/src/lib.rs` の `policy_utc_offset_hours` に一本化され、`run()` と `doctor` が **同じ関数を共有** する
  （`Etc/GMT±25` のように `FixedOffset` の範囲を超える値は両者とも `None`→UTC 扱い）。
  例: UTC 2024-01-01T16:00Z は
  JST では 2024-01-02 01:00 となり、2003-01-02 生まれは JST 基準で 21 歳、UTC 基準では 20 歳として判定される。
- 年齢ゲートは fail-closed。セッションを発行する全経路が共通ゲート `ensure_age_policy_configured`
  （`server/src/lib.rs`）を通る。`AGE_ENFORCEMENT_MODE=trusted_attribute_required` では
  `/auth/oidc/callback` と `/auth/refresh` が `TrustedAttributeRequired` を 403 で拒否し（R1-17）、
  `AGE_REQUIRE_EXPLICIT_CONFIGURATION=1` かつ `AGE_DEFAULT_MINIMUM` /
  `AGE_COUNTRY_OVERRIDES` / `AGE_TARGET_REGIONS` の実効値から導出した
  `configuration_satisfied()` が false のときは
  `ConfigurationRequired` を 403 で拒否する。両経路は state / Cookie の消費より前にゲートを適用し、
  数値年齢の判定は `/auth/verify` の `validate_age` に委ねる。

### OIDC認証フロー（実装済み・モックOPで検証）

1. `GET /auth/oidc/start` が `state`、`nonce`、`code_verifier` を生成し、
   サーバー側に10分間保存する。未完了の `pending` はコード定数
   `MAX_PENDING_AUTH = 1024`（`server/src/oidc.rs`）を上限とし、登録時・取出時に
   期限切れ（10分超）エントリを `retain` で一括回収する。上限到達時は新規エントリを
   追加せず 429 を返す（`/auth/oidc/start` も 429、R1-23）。上限は環境変数ではない。
2. 認可URLには `response_type=code`、`client_id`、`redirect_uri`、`scope`、
   `state`、`nonce`、`code_challenge`、`code_challenge_method=S256` を含める。
3. `GET /auth/oidc/callback` は `state` を単回消費し、認可コードと
   `code_verifier` をトークンエンドポイントへ送る。
4. IDトークンはJWKSの `kid` でES256署名を検証し、`iss`、`aud`、`exp`、
   `nbf`、`iat`、`nonce`、`at_hash` を確認する。
5. `sub`（PPID）を `user_hash` としてセッションJWTを発行し、
   HttpOnly Cookieで返す。JWTは `sid` を含み、`POST /auth/logout` で
   `sid` を失効させると以後のAPIは401を返す。
6. `POST /auth/backchannel-logout` は logout token の署名・`iss`・`aud`・
   `events` を検証し、`sid` を失効させる。`sid` はマイグレーション 009 の
   `revoked_sessions` テーブルへも保存し、起動時に in-memory セットへロードして
   再起動後も失効を維持する（`/auth/logout` も同じ経路）。`sub` が含まれる場合は
   `sub` 単位の失効閾値（`iat` 比較）を記録し既存セッションを拒否する。閾値は
   `revoked_subs` テーブルへも保存し、起動時にログインへロードして再起動後も失効を維持する。再認証時は閾値より新しい `iat` を発行して再ログインを許容し、DB の閾値も削除する（多プロセス共有・実OP連携は未検証）。
7. リフレッシュトークンは `refresh_tokens` テーブルに SHA-256 ハッシュのみ保存し、ログイン時に 30 日間有効なトークンを HttpOnly Cookie で発行する。`POST /auth/refresh`
   は毎回トークンを回転し、使用済みトークンの再提示を検知した場合はファミリ全体を失効する。ログアウト時もファミリを失効する。
   `POST /auth/logout` はファミリ失効に加え、`refresh_token=; Path=/auth; HttpOnly; SameSite=Strict; Max-Age=0`
   の `Set-Cookie` を返し、DB 失効だけではブラウザに残る Cookie を即時削除する（送られたトークン値は
   エコーしない）。`Secure` 属性は `OIDC_COOKIE_SECURE` に追随し、未設定なら付与する（fail-closed）。
   回帰テストで属性一式と本文 `true` の維持、および `OIDC_COOKIE_SECURE` による Secure の切替を確認済み。
8. `OIDC_REQUIRED_ACR` / `OIDC_REQUIRED_AMR` を設定すると ID トークンの
   `acr`（いずれか一致）と `amr`（すべて含有）を検証し、欠落・不一致なら
   認証を拒否する（fail-closed）。`OIDC_MAX_AUTH_AGE_SECS` を設定すると
   認証要求へ `max_age` を付与し、ID トークンの `auth_time` が
   `現在時刻 - max_age - clock_skew` 以降であることを要求する（欠落も拒否）。
   本番ではこれら3設定を必須とする。
   `OIDC_CLOCK_SKEW_SECS` は 1..=300 秒のみ受け付け、範囲外・不正値は既定 60 秒へ
   フォールバックする（`server/src/oidc.rs` の `CLOCK_SKEW_MIN_SECS` / `CLOCK_SKEW_MAX_SECS`）。
   `doctor` は範囲外設定を `OIDC_CLOCK_SKEW_SECS is outside 1..=300; using default 60 seconds`
   で警告する（`server/src/bin/doctor.rs`）。
   未実装: 複数プロセス間の失効状態共有・実OP運用整合
   （`sub` 失効の DB 永続化は実装済み）。
9. 実デジタル認証サービスへの接続は、クライアントID取得後に検証する。
   現行のテストはローカルOPモックでフローと検証項目を確認する。

期限切れ `pending` の回収は認証リクエスト時の `register` / `take` だけに依存しない。
`purge_expired_pending` が削除件数を返すようになり、`server/src/lib.rs` の
`run_moderation_finalizer` が起動時と周期 tick（`MODERATION_FINALIZE_INTERVAL_SECS`）に
`OidcState::sweep_expired_pending` を呼び、`OidcState` をワーカーへ配線した。
ロック取得失敗時は best-effort として 0 を返す。
認証リクエストが来ないアイドル中でも古い `pending` が残らないことを
起動時 sweep・周期 sweep・フロー維持の3テストで確認済み（配線の mutation で検出）。

SHA-256 ハッシュの算出は旧 `sha256` 便宜 crate を廃止し、`server/src/lib.rs` の
`sha256_hex`（`sha2` + `hex`）へ統一した。user_hash・リフレッシュトークンハッシュ・
コンテンツチェックサムの出力は小文字 64-hex で不変である。

## 3. コンテンツ処理

### 投稿 `POST /content/upload`

- 入力: `multipart/form-data` の `file`、`owner_hash`、`signature`、任意の `price`、
  および任意のメタデータ `title` / `description` / `tags`（省略時は空文字）。
- JWT主体と `owner_hash` の一致を確認する。
- Content-Typeを検査し、IllustrationまたはNovelに分類する。
- `price` は64bit整数で `0..=MAX_ORDER_JPY`（1,000,000）。注文側 `jpy_to_wei` と同じ上限をアップロード時にも適用し、注文不可能な価格の登録を拒否する。
  ファイル全体のHTTP本文上限は10 MiB。
- 署名対象はファイルのバイト列。開発モックでは全ゼロ署名を許容する。
- Pinataへのアップロード後、CID・SHA-256チェックサム等をDBへ登録する。Pinataへ送るファイル名はコンテンツ種別に応じた固定名（`upload.jxl` / `upload.md`）とし、ユーザー入力名は使わない。
- 同一バイト列を再投稿すると同一CIDとなるため、既存CIDとの衝突は400で拒否する（主キー衝突による500を返さない）。
- Content-Typeは `image/jxl` または `text/markdown` のみ許可し、JPEG XLシグネチャまたはMarkdownのUTF-8を検査する。既知フィールドの重複と未知フィールドは拒否する。
- 価格は `0..=MAX_ORDER_JPY` に制限し、注文上限と整合させる。IPFS 保存名はハンドラ側で `upload.jxl` / `upload.md` に固定し、利用者入力を使わない。
- クライアント側の価格入力は `min="0"` / `max="1000000"` とし、送信直前に
  `price_range_error` でサーバー契約 `0..=1_000_000` をゲートする。範囲外なら固定文言
  `Price must be between 0 and 1000000 JPY.` を表示して送信しない。`price_from_number` は
  入力途中の値を丸めず、範囲外の判定をこのゲートへ集約する。0 は購入不可を表すが、
  サーバー契約上は受理する値（一覧の `COALESCE(price, 0)` と同じ意味）である。
- `title` / `description` / `tags` は省略可で、UTF-8 バイト数（`str::len()`）で `title` は200 B、
  `description` は2000 B、`tags` は10個まで・各タグは50 B を上限とする。超過は400で拒否し、
  空文字は受理し、空ベクトル（保存時は空文字）として扱う。`tags` はカンマ区切りの1フィールドで、
  未知フィールド・重複フィールドは既存どおり400で拒否する。
- クライアント側の JXL デコード結果による検証は実装済み。`client/src/jxl.rs` の
  `decode_jxl_to_canvas`（`jxl-oxide`）でデコードし、チェックサム一致でもデコード失敗時は
  `verified` を `Some(false)` へ切り替えて `× DECODE FAILED` と
  `data-testid="jxl-decode-error"` の固定文言を表示する（`data-testid="content-verification-badge"`）。
  Illustration ではデコード成功時のみ `● VERIFIED` を表示し、デコーダのエラー値は画面へ出さない。
- **未実装**: サーバー側の完全なJPEG XLデコード検証、画像デコード前検査。
  `validate_upload_content` は JXL シグネチャ検査のみ。技術選定は
  [ADR-0006](adr/ADR-0006-jxl-decoding.md) に記録済み（第一候補 `jxl-oxide`、
  現時点ではサーバー側へ依存追加しない判断）。

### 一覧 `GET /content`

- `status = 'Approved'` のみを対象とし、`created_at DESC` で返す（既存の並び順を維持）。
- `price` が NULL の行は `COALESCE(price, 0) AS price` で 0（購入不可）として返す。一覧は価格を
  `i64` で返すため NULL のまま読むと500になるところを防ぐ。SELECT 句のみの読み替えで JOIN /
  WHERE / ORDER には影響せず、エクスポート側の `Option<i64>` も NULL を0として扱う経路と揃える
  （`test_list_contents_null_price_is_returned_as_zero`）。
- 省略可のクエリパラメータ: `q` は `title` / `description` への `LIKE` 部分一致（OR 条件、
  SQLite 既定の大小文字非区別）、`tag` はカンマ区切り `tags` の前方一致（先頭タグまたは
  カンマ直後のタグが対象）、`type` は `Illustration` / `Novel` の完全一致。
  `type` にそれ以外の値を渡しても400にはせず0件を返す。
- `limit` は既定50・範囲 `1..=100`、`offset` は既定0。範囲外・非数値の `limit` は既定50へ、
  負・非数値の `offset` は0へフォールバックする（エラーにはしない）。
- `q` / `tag` から `LIKE` パターンを生成する際は `\` / `%` / `_` をバックスラッシュで
  エスケープし `ESCAPE '\'` 句を付ける。`q` は `%...%`、`tag` は `...%` または `%,...%` とし、
  全パラメータをバインド引数として渡す（SQL 文字列への直接埋め込み禁止）。
- レスポンスの `ContentMeta` は `cid` / `checksum` / `type` / `status` / `price` に加えて
  `title` / `description` / `tags`（文字列配列）と `is_favorite`（bool）を含む。旧サーバー互換のため
  メタデータの欠落時は空値、`is_favorite` の欠落時は `false` として読む（`#[serde(default)]`）。
- `favorite_only` は値が `1` / `true`（大小文字無視・前後空白除去）のときのみ真とし、本人
  （`Claims.sub`）のお気に入りへ絞り込む。未指定およびそれ以外の値は安全側の false とし、
  既存条件とは AND で結合する。`Approved` 限定・`created_at DESC` も維持する。JOIN による
  重複でページング件数が壊れないよう相関 `EXISTS` を用い、`sub` は SQL 文へ埋め込まず
  バインドパラメータで渡す。
- `is_favorite` は閲覧者（`Claims.sub`）視点で設定する。他人のお気に入り状態は一覧から観測できない。
- サーバーは保存・JSON 返却ともに生テキストを扱い、HTML エスケープは行わない。
  XSS 対策は一覧カードを描画するクライアントの `escape_html_text` が担う（API横断規則を参照）。

### お気に入り `POST` / `DELETE /content/:cid/favorite`

- 認証必須（Bearer JWT）。未認証は401。`AuthClaims` の `sub` のみをユーザー識別に用い、
  リクエスト本文やクエリのIDは受け取らない（他人の `sub` を本文へ入れても本人の行は増えない）。
- CIDは既存の `is_safe_cid` で検証し、不正なら400。対象は `status = 'Approved'` のみで、
  存在しない・非Approvedは404（存在の有無を識別子で区別しない）。
- 追加は `INSERT OR IGNORE`、削除は `WHERE user_hash = ? AND cid = ?` の `DELETE` とし、
  重複追加・未登録削除のどちらも冪等（成功）。レスポンスは `is_favorite` のみを含み
  （追加時 `{"is_favorite": true}` / 削除時 `{"is_favorite": false}`）、ユーザー識別子は返さない。
- プライバシー方針: お気に入りの一覧・件数は本人以外へ返さない。`favorite_only` と
  `is_favorite` は閲覧者本人の視点に限定し、他ユーザーのお気に入りは観測できない。
  `user_hash`（`Claims.sub`）はDBへ識別子として保存するが、監査ログ・アプリログ・
  メトリクス・エラー応答へは出力しない（DB由来のエラー詳細は固定文言へ置換する）。

### 取得 `GET /content/:cid`

- DBから期待チェックサムを取得し、設定されたIPFSゲートウェイ候補（`IPFS_GATEWAYS`、既定3件、http(s)のみ）を順に要求する。
- 1要求は `IPFS_TIMEOUT_SECS`（既定15秒、1〜300秒）、応答最大10 MiB。受信ストリームを上限内で蓄積し、SHA-256一致後に返却する。
- Content-TypeはIllustrationで `image/jxl`、Novelで `text/markdown; charset=utf-8` を返す。
- クライアントはMarkdownのraw HTMLイベントをテキストとしてHTML化し、タグをエスケープしてから `inner_html` に渡す。
- リンク・画像のURLは `http` / `https` と、リンクに限り `mailto`、およびスキームとして成立しない接頭辞（相対URL・フラグメント）のみ許可する。それ以外のスキームは `#` に置換する。
  この検査は描画境界のクライアントでのみ実施し、サーバーはMarkdownを描画せず原本バイト列を保持する。
- 中立化件数の計測単位は**描画回数**である。`render_markdown()`（小説カードの `ContentViewer`）が
  1 回描画するごとに、その本文内で置換した危険 URL の件数を数え、1 描画につき 1 回だけ送信する。
  つまりコンテンツや URL の一意件数ではなく、同じ本文を再描画するたびにサーバー側カウンタへ1が再加算され得る。
  再描画（再取得・タブ再表示・一覧の再レンダリング等）でも値はリセットされず累積する。
  このカウンタは参考値であり、アラート判定や請求等の意思決定に単独で使わない
  （[運用ポリシー](OPERATIONAL_POLICY.md) の「5. 運用準備・未整備事項」を参照）。
- パブリックゲートウェイ依存（候補は `IPFS_GATEWAYS` で変更できるが、既定はパブリックで廃止・劣化に応じた見直しが必要）、CIDのURLパス注入拒否は実装済み。厳密なCID仕様検証、リトライ/キャッシュ方針、
  単一ゲートウェイ異常時の運用手順は未確定。
- クライアントの `ContentViewer` は監査ログ（`/content/{cid}/audit`）の取得失敗を `audit_error` シグナルへ
  保持し、`FORENSIC_TIMELINE` パネル内の `<p role="alert" data-testid="audit-error">` に表示する。
  HTTP エラーは `監査ログの取得に失敗しました (HTTP <status>).`、通信失敗は
  `Network error loading audit log.`、JSON 解析失敗は `監査ログの解析に失敗しました。` と区別する。
  成功時に `audit_error` をクリアし、過去のエラーが残らないようにする。内部詳細は表示しない。

## 4. モデレーション

- 通報は `/mod/report` に `target_cid` と `reason` を送る。通報者のJWT主体を監査ログの `changed_by` に用いる。存在しないコンテンツは404、レビュー中（`Reported`
  ）の重複通報は400で拒否する。`reports.deadline_at` に受理から7日を記録する。確定から30日以内の再通報は `appeal_of` に元通報を記録する異議として受理し、元の確定を上書きしない。
- 投票は `/mod/vote` に `report_id`、`juror_hash`、`result`、署名を送る。
  JWT主体と `juror_hash` の一致、結果値、重複投票、期限切れ、対象投稿者（除外）を検査する。存在しない `report_id` は404、対象コンテンツが `Reported`
  でない（確定済み）場合・期限切れ・投稿者の場合は400で拒否する。
- 投票署名は `moderation_vote_message_v2`（`server/src/lib.rs`）のみを受理する。署名対象メッセージは
  `domain: "moderation-vote-v2"` / `report_id` / `result` / `juror_hash` の4項目から成る JSON で、
  投票者ハッシュを束縛することで別投票者への署名転用と、アップロード署名など他プロトコル由来の署名の
  再生（クロスプロトコル再生）を防ぐ。旧 `moderation_vote_message`（`report_id` / `result` のみ）は
  `#[deprecated]` 付きで残すが、`mod_vote` は検証に使わず旧形式の署名は `401 Unauthorized` で拒否する。
  テストは4件（`test_moderation_vote_message_v2_binds_all_fields` /
  `test_vote_v2_signature_accepted` / `test_vote_rejects_legacy_v1_signature` /
  `test_vote_rejects_signature_bound_to_other_juror`）。
- 投票結果のフィードバック表示（`client/src/lib.rs`）: 投票ボタンの応答を待ち、成功時は
  `data-testid="vote-notice"` に固定文言 `投票を記録しました。` を表示する。失敗時は
  `data-testid="vote-error"`（`role="alert"`）に固定文言を表示する。HTTP エラーは
  `投票に失敗しました (HTTP <status>).`（例: 403 なら `投票に失敗しました (HTTP 403).`）、
  ステータスを取得できないネットワークエラーは `Network error submitting vote.` とする。
  サーバーが返すエラー本文や生のネットワークエラー文字列は画面へ出さない。
  新しい投票試行の開始時に通知とエラーの両方をクリアし、前回の結果が残らないようにする。
  E2E は `e2e/tests/moderation.spec.ts` の VOTE-FB-01〜03（chromium で3件 pass）で検証する。
- 期限到達後に `POST /mod/reports/{report_id}/finalize`（認証必須・運用者限定）または `process_report_conclusion` で集計する。期限内に呼び出しても確定しない（3票即時確定は廃止）。
  定足数3票未達なら通報を棄却し `Approved` に復帰、到達時は Reject >= Approve なら `Rejected`、それ以外は `Approved` とする。同数は `Rejected`（ADR-0004）。
- 確定時刻は `reports.concluded_at` に記録し、確定後は追加投票を受け付けないため、多数決結果が後続票で再遷移することはない。
- 運用者はプロジェクト側で固定せず、各デプロイが `MODERATION_OPERATOR_HASHES` で設定する。未設定・未登録・BAN済みの主体は BAN/UNBAN/UNPIN を拒否する。BAN
  は根拠を必須とし、リクエストの `duration_days`（1〜3650日、省略または null は無期限）から期限 `users.ban_until` を設定する。
  期限は SQLite `datetime()` 形式の TEXT（例 `2026-12-18 08:52:18`）で保存し、`UNBAN` で NULL に戻す。
  利用可否の判定は `is_banned = TRUE` かつ（`ban_until IS NULL` または `ban_until > datetime('now')`）のときのみ拒否する。
  期限切れでも `is_banned` フラグは TRUE のまま残り、BAN の履歴として扱う（判定上のみ解除される）。
  UNPIN は判断と理由のみを記録する（実 Pinata ピン解除は手動）。これらは `governance_audit_log` に保存するが、
  DEC-04 の第三者検証可能な監査ではない。
- 期限到達は起動時のバックグラウンドワーカーが自動検知し、`MODERATION_FINALIZE_INTERVAL_SECS`（既定300秒、30〜3600秒）ごとに `finalize_due_reports` を実行する。
  停止シグナルでワーカーも終了する。手動の `POST /mod/reports/{report_id}/finalize` も残しており、両者は冪等。手動 finalize の HTTP ハンドラは
  `require_moderation_operator` により運用者限定（未設定・未登録・BAN済みは403）で、入力検証より前に認可する。
  スケジューラ経路（`finalize_due_reports*` / `process_report_conclusion`）は認可対象外とし、期限到達済み通報の自動確定は認可なしで継続する。
  `process_report_conclusion` が期限到達・`Reported` 状態・未確定を検査し、投票結果を改変できないため、集計の確定という効果しか持たない（BAN/UNBAN/UNPIN
  のような運用判断エンドポイントではない）。確定理由は `content_audit_log` に記録される。
- **未実装**: 投票者選出・Sybil耐性、オンチェーン記録、ランダム陪審員選出、第三者検証可能な監査。モデレーション結果を法的判断とみなさない。

### 透明性ダッシュボード `GET /mod/stats`

- 認可: `AuthClaims` 必須。`require_moderation_operator` により、`MODERATION_OPERATOR_HASHES`
  未設定・未登録・BAN済みの主体は403で拒否する（フェイルクローズ）。
- レスポンス: `generated_at`（集計生成時刻）、`totals`、`monthly` のみ。`totals` の内訳は
  `reports` / `concluded` / `approved` / `rejected` / `quorum_not_met` / `bans` / `unbans` / `unpins`。
  `monthly` の各要素は `month`（`YYYY-MM`）と `reports` / `concluded` / `approved` / `rejected` /
  `bans` / `unbans` / `unpins`。`quorum_not_met` は `totals` のみで月次表の列には含めない
  （月次は提示されたインターフェースの列に合わせ、集計済み件数を持たない）。
- 集計ロジックは純粋関数 `build_mod_stats`（`server/src/lib.rs`）に集約し、DB行を直接返さない。
- プライバシー: CID・通報理由（`reason`）・`user_hash` は返さない。レスポンス項目をホワイトリスト方式で
  定義し、DB行をそのままシリアライズしない。集計値のみを返し、個人を特定する情報を含めない。
- 位置づけ: **認証内集計であり第三者公開ではない**。第三者検証可能な監査（DEC-04）は未決定のままであり、
  本エンドポイントをもって監査要件を満たしたとはみなさない。
- 既存ギャップ（注記のみ、修正しない）: `GET /mod/reports` は認証済み参加者へ `reason` を返す。
  本ダッシュボードは個人情報を返さないが、当該ルートの返却項目は現行のまま維持する。

### 公開集計 `GET /mod/stats/public`（ADR-0008 段階2）

- 位置づけ: 認証済み利用者向けの公開集計。`AuthClaims` 必須（未認証は401）だが
  `require_moderation_operator` は呼ばず、運用者未設定・未登録・BAN済みでも200を返す。
  既存の `/mod/stats` は運用者限定のまま変更しない。
- レスポンス型: `PublicModStatsResponse` / `PublicModStatsTotals` /
  `PublicModStatsMonthly`（いずれも `server/src/lib.rs`）。`generated_at` と `totals`
  （`reports` / `concluded` / `approved` / `rejected` / `quorum_not_met`）、`monthly`
  （`month` と `reports` / `concluded` / `approved` / `rejected`）のみ。
  `quorum_not_met` は `totals` のみで月次表には含めない。
- 除外項目: `bans` / `unbans` / `unpins` に加え、CID・`reason`・`user_hash` も
  レスポンス型に存在しない。DB行をそのままシリアライズせずホワイトリストで定義する。
- 集計: 既存の純粋関数 `build_mod_stats` で集計した結果を、
  `build_public_mod_stats(&ModStatsResponse)` が公開ホワイトリストへ縮約する。
  `generated_at` は元の値を引き継ぎ、集計ロジックの二重実装は行わない。
- 検証: `server/src/tests.rs` に認証なし401・一般sub 200・運用者sub 200・
  機微キー非含有・変換が元と同値のテストを追加済み（TEST_SPECIFICATION の MOD-13）。
  E2E 定義数は69件のまま変更しない。

## 5. データモデル（現行起動時DDLの要約）

|Table|主な列|用途・制約|
|---|---|---|
|`users`|`user_hash`, `public_key_der`, `is_banned`, `banned_at`, `ban_until`|利用者識別・公開鍵・BAN状態。`ban_until` は期限（NULL=無期限、期限切れでも `is_banned` は履歴として残る）|
|`contents`|`cid`, `type`, `owner_hash`, `checksum`, `status`, `price`, `created_at`, `title`, `description`, `tags`|CIDを主キーとするコンテンツメタデータ。メタデータ3列は migration 011 で追加、`tags` はカンマ区切りで `idx_contents_tags` インデックス付き|
|`favorites`|`id`, `user_hash`, `cid`, `created_at`|お気に入り。`(user_hash, cid)` に一意インデックス。`users(user_hash)` / `contents(cid)` へ `ON DELETE CASCADE` のFK|
|`reports`|`report_id`, `target_cid`, `reason`, `created_at`, `deadline_at`, `concluded_at`, `appeal_of`|通報記録（7日期限・確定時刻・異議の連鎖）|
|`votes`|`vote_id`, `report_id`, `voter_hash`, `vote_result`, `voted_at`|投票記録。現行DDLに複合一意制約なし|
|`signatures`|`target_id`, `signer_hash`, `signature_blob`, `algorithm`|投稿・投票署名の保存|
|`content_audit_log`|状態、主体、理由、連鎖ハッシュ、時刻|DB内状態遷移記録|
|`governance_audit_log`|`action`（BAN/UNBAN/UNPIN）、対象、運用者、根拠、詳細、時刻|運用判断の記録。連鎖・署名なし（DEC-04 未達）|
|`transactions`|CID、送受信者、amount、`tx_hash`、時刻|現行の模擬支援記録。実決済の証明ではない|
|`payment_orders`|注文ID、PPID、CID、チェーン、ウォレット、受取先、金額、状態、tx、確認数、期限、派生index|JPYC決済の状態機械。`(chain_id, tx_hash, log_index)` 一意。`derivation_index` は migration 003 の部分 UNIQUE（`derivation_index IS NOT NULL`）|

DDLは `server/migrations/` の番号付きSQL（現行: `001_initial_schema.sql` 〜 `014_refresh_tokens_expires_at_text.sql`）を
バージョン管理し、起動時に `server/src/migration.rs` のランナーが未適用分のみ適用する。適用済みバージョンとSHA-256チェックサムは
`_migrations` テーブルに記録され、適用済みマイグレーションの改変は起動時の検証で検出し起動を失敗させる。起動時の検証は適用済みだが
ファイルが無いバージョンも拒否する（古いデプロイイメージの誤用防止）。既存DBに新規マイグレーションを追加する段階的アップグレードはテスト済み。
migration 011 は `contents` へ `title` / `description` / `tags`（いずれも `TEXT NOT NULL DEFAULT ''`）を追加し、
`tags` に `idx_contents_tags` インデックスを作成する。既存行のメタデータは空文字として読み出され、
適用済みマイグレーションの改変は従来どおり起動時に検出される。
migration 012 は `favorites` テーブルを作成する。`user_hash` は `users(user_hash)`、`cid` は
`contents(cid)` を参照し、いずれも `ON DELETE CASCADE`。一意性は `idx_favorites_user_cid`
（`UNIQUE(user_hash, cid)`）で担保し、本人分の新着順参照用に
`idx_favorites_user_created`（`user_hash, created_at DESC`）を作成する。
migration 013 は `users` へ `ban_until TEXT` を追加し、`idx_users_ban_until` インデックスを作成する。既存行は NULL（無期限）として
読み出され、後方互換。期限切れ判定は判定側の時刻比較で行い、`is_banned` フラグ自体は履歴として保持する。
migration 014 は `refresh_tokens` を再作成し、`expires_at` の型宣言を `DATETIME` から `TEXT` へ変更する。006 の `DATETIME` は
SQLite の型 affinity に過ぎず、実際の格納値は RFC3339 文字列だったため、宣言と実データを整合させる。値は列明示の
`INSERT ... SELECT` で保持し、`used_at` / `created_at` は `DATETIME` のまま、CHECK 制約は設けない。
`idx_refresh_tokens_family` / `idx_refresh_tokens_sub` は再作成する。006 はチェックサム保護のため一切変更しない。
ロールバック（downマイグレーション）は未実装。ローカル DB では migration 前に `PRAGMA foreign_keys = ON` を適用して強制・検証する。
リモート DB では接続単位で効かないため書き込み側で担保する（起動時 warn）。投票の重複は `votes(report_id, voter_hash)` の一意インデックスでも拒否する。

### バックアップ（`backup_db`）の生存ロック検査

- `backup_db` は `VACUUM INTO` で一貫性のあるスナップショットを作成し、既存ファイルの上書きを拒否する。
  取得後にテーブル数と `_migrations` 台帳を検証する（手順は
  [BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)）。
- `VACUUM INTO` はインスタンスロックを取得しないため、サーバー起動時（特にマイグレーション実行中）
  と競合すると BUSY や旧スナップショットになる恐れがある。開始前に生存ロック検査を行う。
- 検査対象は `backup_db` ではソース DB（`DATABASE_URL` がローカル DB を指す場合）と出力先 DB、
  `restore_db` ではバックアップ元（`backup`）と復元先（`target`）の双方で、
  それぞれ `<db path>.instance.lock` を調べる。ロックが無い、または PID が存在しない
  stale ロックは続行を許可する。
- ロック内容の PID とプロセス開始時刻（`/proc/<pid>/stat` の field 22）を照合し、PID 再利用を防ぐ。
  旧形式（PID のみ）は後方互換のため PID の存在だけで判定する。
- ロック内容の解析・starttime 読み取り・所有者生存判定は `common/src/instance_lock.rs` の
  `parse_instance_lock` / `read_proc_starttime` / `instance_lock_owner_alive` に集約した
  単一情報源を使う。`parse_instance_lock` は純粋関数として無条件公開し、`/proc` 依存の
  2関数は `#[cfg(not(target_arch = "wasm32"))]` で分離する（common は wasm クライアントからも
  依存されるため）。`server/src/lib.rs` はこれを re-export し、`backup_db` / `restore_db` は
  `common::instance_lock` から直接 import して同じ判定を共有する（bin 内の再実装は削除）。
- 生存サーバーがロックを保持する場合は `backup_db` は
  `refusing backup: a live server instance holds the instance lock`、`restore_db` は
  `refusing restore: a live server instance holds the <side> instance lock`（`<side>` は
  `backup` / `target`）で拒否する。`/proc` が読めない等で判定不能な場合も「生存」として
  扱い拒否する（fail-closed）。`backup_db` は bin 全体で17テスト（うち生存ロック検査は7テスト、DB 接続先必須化5テスト）と
  実 CLI で拒否を実証済み。
  `restore_db` は `live_backup_lock_is_rejected` / `live_target_lock_is_rejected`
  で両側の拒否を確認済み。

## 6. 監査ハッシュ

- 新規エントリは `AuditLogEntry::calculate_hash_v2` で生成する。CID、状態、主体、理由、前エントリーハッシュに長さプレフィックスを付け、ドメイン分離ラベルを付与した上でSHA-256化するため、
  フィールド境界の曖昧さによる衝突を排除する。既存DB向けに旧 `calculate_hash`（v1）も残し、`verify_audit_log` は v1/v2 の両方を検証する。
- `get_content_audit`（`GET /content/:cid/audit`）は表示用の `reason` に `common::summarize_reason`
  を適用する（[PII 除去と120 Unicode 文字上限](OPERATIONAL_POLICY.md) の「3. 監査・モデレーション」を参照）。
  ハッシュ検証に使う `entry_hash` は DB の生値をそのまま返し、DB の `reason` 全文も変更しない。
  すなわちマスクは API レスポンス時にのみ行い、`calculate_hash_v2` の入力や検証結果には影響しない
  （`test_get_content_audit_masks_reason_but_preserves_entry_hash`）。
- `get_content_audit` の取得件数は `MAX_AUDIT_LIST`（`server/src/lib.rs`、500件）で制限し、
  SQL は `ORDER BY log_id DESC LIMIT ?` とする。最新 MAX_AUDIT_LIST(500) 件を log_id 新しい順で
  返す。古い行はAPIから参照不可、完全検証には署名付きエクスポート/DBを使用。
  レスポンスには previous_log_hash が無く reason はマスクされるため、レスポンス単体での
  ハッシュ連鎖検証は成立しない（`get_content_audit_caps_at_max_audit_list`）。
- `compute_audit_chain_digest` は `log_id` の連続性と entry_hash 列の Merkle root を計算する。`verify_audit_log --expect-root`
  で外部保存した root と照合すると、行の欠落・順序入替・末尾切り詰めを検出できる（実DBで確認済み）。
- `sign_audit_export` は件数と Merkle root を RSA 署名したスナップショットを生成し、`verify_audit_export` は公開鍵 DER のみで署名・完全性を検証する（ADR-0005、
  [AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md)）。これは運営者が公開したことを示すに留まり、DB外アンカーや追記専用媒体の代替ではない。
- `sign_audit_export` の CLI 契約（硬化済み、実装: `server/src/bin/sign_audit_export.rs`）:
  - 署名鍵 PEM は `canonicalize` でシンボリックリンク先を解決してから権限を検査し、owner-only
    （`0600` / `0400`、すなわち `mode & 0o077 == 0`）を要求する。満たさなければ拒否する。
  - 親ディレクトリが group/other 書き込み可能（`mode & 0o022 != 0`）なら拒否し、
    `0700` でない（`mode & 0o055 != 0`）場合は警告を出す。
  - リモート libSQL のトークンは `DATABASE_TOKEN` 環境変数からのみ読む。`--database-token` は
    渡された時点でエラーとなる（`argv` は `ps` 等で公開されるため）。`DATABASE_URL` は
    `--database-url` または環境変数で必須設定し、暗黙既定値は使わない（未設定時起動拒否）。
    `DATABASE_TOKEN` の既定値は空。
  - 出力は `OpenOptions::create_new(true)` で存在チェックと生成を原子的に行い、既存ファイルの
    上書きを拒否する（TOCTOU 対策）。
- マスク整合チェック: `sign_audit_snapshot` は `reasons_masked=true` のとき各 `entry.reason` が
  `mask_reason` のマスク済み形式（空文字または `[redacted:` 接頭辞）であることを検査し、
  平文が残るスナップショットは署名時に `InvalidSignature`（`unmasked entry reason with
  reasons_masked=true at log_id=...`）で拒否する。マスク時は検証側が `entry_hash` を
  再計算しないため署名時点で防ぐ（fail-closed）。`governance_entries` は既存スナップショットに
  平文が残り得るため対象外。
- バージョン検証: `verify_chain` は単体呼び出しでも先頭で `AUDIT_SNAPSHOT_VERSION`（現行 3）を
  検査し、不一致なら `unsupported audit snapshot version` で拒否する。署名・エクスポート検証に
  加え連鎖検証にも適用し、未サポート版の受理をふさぐ。
- DB外アンカー、信用できる第三者による独立検証、鍵の HSM 管理とローテーションは未実装。
- 目的はDB内の連鎖整合性チェック補助に限定し、改ざん不能・否認防止を主張しない。root の外部保存先は各デプロイの運用者が決める。

## 7. 必須要件を満たす目標フロー

### JPKI認証

サーバーは一意なchallenge/nonceを生成し、指定マイナアプリからの提示を受ける。信頼された発行元、署名、nonce結合、audience、証明書・失効状態、必要な年齢属性を公式仕様に基づき検証し、
成功時のみ短寿命のAPIトークンを発行する。具体API・証明形式は公式仕様確認後に確定する。

現行コードの確認範囲: `server/src/jpki.rs` の `verify_presentation` は提示トークン（または envelope の
`certificate` / `verifiableCredential`）を base64 デコードし、X.509 DER を解析して有効期限を確認してから
公開鍵を抽出する。自己署名は発行者=主体の DN 一致を予備検知とし、証明書自身の公開鍵による暗号学的署名検証
（`verify_signature(None)`）を主判定として拒否するため、issuer 名を変えた DN 比較の迂回も塞ぐ。
不正base64・非X.509バイト・期限切れ証明書・envelope空・空トークンの5エラーパスを
テストで固定し、いずれも `Unauthorized` を要求する（mutation 3パターンで検出を確認）。
これは公開鍵抽出と自己署名拒否にとどまり本人確認ではない。証明書の解析と有効期限の検証に限り、
信頼チェーン・失効・VP署名・nonce・audience の検証は未実装。

トークン発行は検証済み認証トランザクションにサーバー側で結び付ける。クライアントが送る `user_hash` を認証主体として信用しない。安定識別子の生成方式は個人間追跡可能性を抑える設計と合わせて決定する。

### JPYC決済

目標方式は [ADR-0003](adr/ADR-0003-jpyc-settlement.md) を参照。

`PAYMENT_RECEIVING_XPRV` が設定されている場合、注文ごとに BIP44
（`m/44'/60'/0'/0/{index}`）で受取アドレスを派生し、`derivation_index` を
一意に記録する。未設定時は `PAYMENT_RECEIVING_ADDRESS` の単一アドレスを使う。

`derivation_index` の採番は `create_order` の書き込みロック＋書き込みトランザクション内で
行う。ロック取得後に `MAX(derivation_index) + 1` を読み、受取アドレスの派生と注文の
INSERT を同じトランザクションでコミットする。この原子的予約により、同一 DB への
並行注文でも連番の index と異なる受取先が割り当てられ、UNIQUE 違反で片方が失敗しない。
途中で失敗した場合はロールバックして index を消費しない。チェーン ID とウォレット
アドレスの検証は採番より前に行うため、不正入力で拒否した注文も index を消費しない。
固定受取アドレス設定では `derivation_index` は NULL のままで、複数注文が同じ
アドレスを再利用できる。重複の最終防御は migration 003 の部分 UNIQUE インデックス
（`derivation_index IS NOT NULL`）が担う。

決済要求、利用者ウォレット署名、チェーン送信、トランザクション確認、最終性、失敗・再試行を別状態として扱う。受理対象チェーンとJPYCコントラクトをallowlistで識別し、
要求ID・金額・受取先に結び付く送金を検証した後だけ有償機能を解放する。成功を確認できた取引だけを決済成立とする。実際の手数料と確認時間を利用者へ提示する。

`CONFIRMING` の注文は同じ送金で再 verify でき、確認数が進展すれば `FINALIZED` へ進む。`FINALIZED` は確定後不変とし、再 verify
や確認数の後退でも状態を戻さない（実チェーン上の再編成検知は RPC 実装時に別途検討）。

`verify_order` は読み取り時の期限判定に加え、検証 I/O（非同期）完了後にも期限を再判定する。
再判定は `begin_write_tx` 内の最新 `expires_at` を使い、期限切れなら `EXPIRED` をコミットして
400 を返す。これにより、検証中に期限を過ぎた注文を読み取り時の古い status を期待する CAS で
`FINALIZED` へ進ませない（TOCTOU 対策）。期限前なら同じトランザクションと書き込みロックを
そのまま CAS 更新に使い、状態変更は2段の CAS（DETECTED → 次状態）で担保する。

ウォレット所有証明（`/payments/orders/:order_id/bind`）の署名対象は `wallet_bind_message_v2` で
`order_id` / `chain_id` / `wallet_address` / `recipient_address` / `amount_jpy` の5項目とする。
署名者が送金先・金額・チェーンを確認できるようにすることが盲署名対策の目的である。
アドレスは `normalize_address` で小文字化し、EVM の大小文字無視（表示用チェックサム差）による
検証失敗を防ぐ。`amount_jpy` は DB の `amount_wei` から `amount_jpy_from_wei` で導出し、
10^18 スケールで割り切れない値はデータ不整合として拒否する。旧 `wallet_bind_message`
（注文IDのみ）は API 互換のため `#[deprecated]` 付きで残すが、新規の署名生成には使わず、
旧形式の署名は 401 で拒否する（実装: `server/src/payments.rs` の `bind_wallet`）。

### 検証可能な監査

重要イベントを正規化した形式で記録し、署名鍵・連鎖・第三者が検証可能な外部アンカー/追記専用媒体を組み合わせる。個人情報・認証秘密・不要なコンテンツ本文は公開台帳に記録しない。具体方式、訂正・削除との調整は未決定。
運用者は各デプロイが設定する（`MODERATION_OPERATOR_HASHES`）。

## 8. 既知の未実装・設計保留

- 実JPKI/マイナアプリ連携と信頼検証、検証済み年齢/居住地属性の取得・検証経路（DEC-01/DEC-02連携）。
- DEC-02の法的確定: 対象地域・必要属性・最終閾値・属性保持期間の専門家/法務レビュー、現地法令適合の確認（`GET /policy/age` と構成可能ポリシーは方針フレームワークであり、法的確定ではない）。
- JPYC送金・チェーン確認・手数料表示と、無効化済み模擬支援API（501）の決済APIへの置き換え。
- 第三者検証可能な監査と運用鍵管理。
- 投票資格・選出・投票期間を含むモデレーション状態機械の完成（重複投票の一意制約と確定後の追加投票拒否は実装済み）。
- 本番CORSオリジンの決定、ログ集約・アラート。DB マイグレーションのロールバック判断手順は [BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md) に記載済み（down
  migration は未採用）。
- Rust優先方針に沿う技術例外記録と、性能・軽量性・費用の計測値。
- Arweave、追加ゲートウェイ、Cloudflare Workers/R2、OpenTofu等の必要性・方式。
- メタデータ検索は `LIKE` による部分一致・前方一致の簡易実装。正規化したタグテーブル、
  全文検索、あいまい一致、スコアリングは未実装。
- ブラウザ URL との同期は実装済み（R2-URL）。view は hash（`#dashboard` /
  `#upload` / `#moderation` / `#ledger`）、検索条件は query（`q` / `tag` / `type` /
  `favorite_only`）に載せる。タブ切替のみ `pushState`、ログイン成功・ログアウト・
  初期化と検索条件の書き出しは `replaceState`。`popstate` では view と 4 条件を
  同期し、条件は `batch()` で同時反映して二重 GET を防ぐ。未認証の直接リンクは
  ログイン画面から始め、認証成功後に保留した hash を適用する。許可リスト外の
  hash は dashboard に固定する。HTTP API 契約は変更しない。

## 9. データ主体の権利（エクスポート / 削除権）

### 実装範囲: エクスポートのみ

- `GET /me/export` は `AuthClaims` 必須とし、未認証は401で拒否する（ルート表は「1. API仕様」を参照）。
- 認証主体（`sub`）に紐づく行のみを返す。対象は `users` / `contents` / `favorites` / `transactions` /
  `signatures` / `content_audit_log` / `reports` / `votes` / `payment_orders` /
  `governance_audit_log` / `refresh_tokens` / `revoked_subs` の12表。
- トップレベル項目は `version:1` / `generated_at`（RFC 3339）/ `sub` と上記12表の配列。
  `revoked_sessions` は対象外。行数上限・`truncated` フラグは設けず全行を返す。
- 12表の読み取りは単一のトランザクション（`Connection::transaction()` = `BEGIN DEFERRED`）内で実行する。
  `me_export` は `begin_write_tx` で書き込みと同じ `write_lock` を取得してから BEGIN し、
  `collect_export_snapshot` が全クエリを同じトランザクションへ閉じ込め、
  `debug_assert!(!db.is_autocommit())` で検証する（cargo test の debug ビルドで有効）。
  共有 `Connection` は同時に1つの BEGIN しか保持できないため、書き込みと BEGIN を直列化し、
  アップロード等の書き込みトランザクション完了を待ってから開始する（旧実装の
  `cannot start a transaction within a transaction` による 500 を解消）。
  完了時に `tx.commit()` でトランザクションを閉じ、ガードは tx より後にドロップして
  コミット完了までロックを保持する。途中のクエリが失敗した場合は
  `libsql::Transaction` の Drop が ROLLBACK する。開始後は全12表が単一時点の
  スナップショットになる（書き込み待ちは BEGIN 前のみ）。
- `reason` / `details` は `mask_reason` により既定でマスクする（例: `[redacted:N bytes]`）。
- `token_hash` / `sid` / `family_id` / 署名鍵 / XPRV / `PINATA_JWT` /
  他ユーザーID はレスポンスへ含めない。
- `Content-Disposition: attachment` と `Cache-Control: no-store` を付与し、エクスポート本文は
  アプリログへ出力しない（認証ヘッダ・投稿本文と同様の機微情報扱い）。

### 未実装: 削除権（erase）

- 削除権（データ消去 / erase）は**未実装**である。エクスポートのみを提供し、
  削除要求を受け付ける API は存在しない。
- 実装着手の前提条件は次の3点の決定後とする。
  - 法務判断（対象法令・保持期間・例外・本人確認手続）。
  - 第三者検証可能な監査の外部アンカー方針（DEC-04、ADR-0005 の未実装項目）。
  - バックアップからの消去方針（消去対象・保持ライフサイクル・リストア時の整合性）。
- 決定前に削除 API を提供すると、監査連鎖の連続性・バックアップからの復元経路と矛盾するため、
  現時点では提供しない。

## 10. 運用CLI（7 bin）の設計

7つの CLI はいずれも `server/src/bin/` に置き、I/O・環境変数読み取り・終了処理を薄い `main` に
閉じ込め、判定を純粋関数（または入力を引数で受け取る境界関数）としてテストする。
テストは全てサーバー側 bin テストであり、E2E 定義数（当時59件、現在69件）は変更しない。
ゲート ID とテスト名は
[TEST_SPECIFICATION.md](TEST_SPECIFICATION.md) の CLI-01〜03 および「運用 bin のテスト数一覧」を参照。

| CLI | 純粋関数 / 境界関数 | テスト数 | 対応ゲート / 参照 |
| --- | --- | --- | --- |
| `backup_db` | `parse_args_from` / `parse_str_args` / `ensure_no_live_instance` / `resolve_database_url` | 17 | 詳細設計 §5 |
| `doctor` | `parse_args_from` / `check_all_with` / `should_fail` | 53（既存29＋境界テスト7＋OIDC URL検証3＋oidc_required_assurance1＋設定検証3＋ヘルプ優先2＋AUDIT_EXPECT_ROOT1＋AGE_POLICY_CONFIGURED3＋バックアップ/TZ診断3＋バックアップ生値判定1件） | CLI-01 / CFG-01 |
| `restore_db` | `parse_args_from` / `ensure_no_sidecars` / `ensure_restore_is_safe` | 20 | 詳細設計 §5 |
| `sign_audit_export` | `parse_args_from` / `check_signing_key_permissions` / `ensure_include_reasons_allowed` / `resolve_database_url` | 17（鍵権限・トークン・上書き拒否10＋include_reasons本番ガード2＋DB接続先必須化5） | AUDIT-02 |
| `verify_audit_export` | `parse_args_from` / `verify_export` / `VerificationOutcome::exit_code` | 20 | CLI-02 / AUDIT-01 |
| `verify_audit_log` | `parse_args_from` / `check_expect_root` / `resolve_database_url` | 14 | AUDIT-01 |
| `verify_backup` | `parse_args_from` / `decide_output` / `ensure_no_sidecars` / `verify_backup_at` | 19 | CLI-03 |

上記3 CLI（`backup_db` / `sign_audit_export` / `verify_audit_log`）の `resolve_database_url(cli, env)` は
`server/src/lib.rs` の共通ヘルパへ集約され、`env` は `common::env::read_env("DATABASE_URL")` が返す
`common::env::EnvValue`（`Utf8` / `Missing` / `NonUtf8`）で統一されている。CLI 引数を環境変数より
優先し、空・空白のみを未設定として扱う。両方が未設定なら `DatabaseUrlError::Required` に対応する
固定文言 `DATABASE_URL is required: pass --database-url or set DATABASE_URL; no default is used` で
起動を拒否し、非 UTF-8 の環境変数は未設定へ潰さず `DatabaseUrlError::NotUtf8` に対応する固定文言
`DATABASE_URL is not valid UTF-8`（本番の `production_database_url_error` と同一）で拒否する。
いかなる値・生バイトもエコーしない。`file:local.db` の暗黙既定は撤廃済み（deploy 設定例は明示済みのため変更なし）。

### doctor（`server/src/bin/doctor.rs`）

- `parse_args_from<I>(items) -> ParseOutcome` は先頭要素をプログラム名として読み飛ばし、
  `Run { strict }` / `Help` / `UnknownArgument(String)` / `TooManyArguments` を返す。
  `--strict` は完全一致のみ有効で、`-h` / `--help` はヘルプ表示（exit 0）へ変換する。
- `check_all_with<F>(lookup)` は環境変数を `lookup` 経由でのみ読み、23カテゴリの検査結果を
  返す。`production` / `mock_auth_with_production_config` / `mock_auth_enabled` / `jwt` /
  `jwt_secret` / `ipfs_gateways` / `ipfs_timeout` / `ipfs_max_attempts` /
  `oidc_http_timeout` / `oidc_clock_skew` / `oidc_endpoint_urls` / `age_policy` / `rate_limit` / `ipv4_localhost` /
  `trusted_proxy` / `oidc_max_auth_age` / `oidc_cookie_secure` / `oidc_required_assurance` /
  `expose_internal_errors` / `allowed_origins` / `graceful_drain_timeout` /
  `moderation_finalize_interval` / `audit_expect_root`。秘密情報の値は結果へ含めない。実環境用の `check_all()` は `env` を渡す
  ラッパーに留める。
  `oidc_endpoint_urls` は OIDC 7 変数がすべて揃ったときに `OIDC_ISSUER` / `OIDC_REDIRECT_URI` /
  `OIDC_AUTHORIZATION_ENDPOINT` / `OIDC_TOKEN_ENDPOINT` / `OIDC_JWKS_URI` の5エンドポイントURLを
  検証し、https のみを受理する（`ALLOW_MOCK_AUTH` 有効時は localhost の http のみ許容）。
  警告は固定文言で、設定値は出力しない。
- `ipfs_timeout` / `ipfs_max_attempts` / `oidc_http_timeout` は各パーサが不正値・範囲外を
  既定へフォールバックするため、`rate_limit` と同じ方式で「生のパース値」と「正規化後の値」を
  比較し、フォールバック時に当該カテゴリへ警告1件を計上する（設定値自体は出力しない）。
  フォールバック検出は5変数（旧3件＋新規カテゴリ2件＝`graceful_drain_timeout` /
  `moderation_finalize_interval`）。検査カテゴリ数は新規2件で19→21、`oidc_endpoint_urls` 追加で
  21→22、`audit_expect_root` 追加で22→23へ拡張（これらはカテゴリ数でテスト数ではない）。
  境界テスト7件（非数値 / `0` / 範囲外 / 有効値）でフォールバック検出と値非出力を検証する。
- `should_fail(strict, problems)` は `strict && problems > 0`。既定は警告のみで exit 0、
  `--strict` で問題があれば exit 1 とする。
- 真偽値フラグの解釈は `common::age::parse_bool_env` に統一する。受理値は `1` / `true`（真）と
  `0` / `false`（偽）のみで、trim・小文字化は維持する。`yes` / `no` / 空文字を含む受理値外は
  fail-closed（ランタイムは起動拒否、doctor は既存カテゴリ内で固定文言の警告）とし、値はエコーしない。
  6 フラグ（`ALLOW_MOCK_AUTH` / `AUDIT_EXPORT_INCLUDE_REASONS` / `EXPOSE_INTERNAL_ERRORS` /
  `JWT_REQUIRE_KID` / `OIDC_COOKIE_SECURE` / `ALLOW_IPV4_LOCALHOST`）の全 call site がこの関数を呼ぶ。
- `ALLOW_MOCK_AUTH` の真値は `1` / `true` のみ。`1` 単独でも `mock_auth_enabled` に1件計上され、
  `--strict` では exit 1 となる（23 check(s) 実測）。受理値外は同カテゴリに不正値警告を1件計上する。
- `ALLOW_IPV4_LOCALHOST` が真（`1` / `true`）のときと `TRUSTED_PROXY_CIDRS` が設定されているときは、
  それぞれ `ipv4_localhost` / `trusted_proxy` に警告を1件計上する（`doctor --strict` の警告対象）。
  `network::ipv4_permitted` は per-request 読込のため、受理値外は `None`→`false`（IPv4 不許可）とし、
  起動時警告と doctor 警告の二段構えとする。いずれも設定値は出力せず有無のみを警告する。
- `audit_expect_root` は本番相当（`ALLOW_MOCK_AUTH` 無効）の `AUDIT_EXPECT_ROOT` を診断する。
  `backup-rotate.sh` と同じく `lookup` の生値へ `[0-9a-fA-F]+`（hex Merkle root）を適用し
  （trim しない）、非空で全文字が hex なら警告なし、未設定・空白のみ・前後空白付き・不正 hex
  なら固定文言の警告1件を計上する。値はエコーしない。
  共有の `production_config_errors` は変更せず、`backup-rotate.sh` が未設定時に内部整合性
  チェックのみで動作する既存契約も維持する。既定は警告のみで exit 0、`--strict` のときだけ失敗する。

### verify_audit_export（`server/src/bin/verify_audit_export.rs`）

- 使用形: `verify_audit_export --snapshot PATH --public-key PATH [--expect-root HEX]`。
  `--expect-root` は任意で、過去に記録した Merkle root との照合に使う。値が無ければ
  `--expect-root requires a value` で拒否する。
- `parse_args_from` は必須引数の欠落も検出する。判定は `verify_export(snapshot, public_key,
  expect_root) -> VerificationOutcome` が担い、I/O と `process::exit` を行わない。
  `VerificationOutcome` は `Accepted { report }` / `Rejected { message }` で、
  `exit_code()` が受理0・拒否1を返す。
- `common::verify_audit_snapshot` が version 3 のみ受理し、署名・連鎖・Merkle root を検証する。
  v1 / v2 は `unsupported audit snapshot version: N` で fail-closed 拒否、
  改ざん entry / root / 署名は `Invalid signature` で拒否する。
- `--expect-root` の照合は `eq_ignore_ascii_case`（大小文字非区別）で、不一致なら
  `audit root mismatch (expected and computed roots withheld)` を返す。受理1行の形式は
  `audit_export_verification: OK version=3 entries=N governance=N root=<hex>` である。
  理由本文（マスク済み・平文の双方）と秘密値は出力しない。
- 運用上の使い方は
  [AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md) の「第三者検証」を参照。

### verify_backup（`server/src/bin/verify_backup.rs`）

- 使用形は `verify_backup --input PATH`。`parse_args_from` は argv のみを解釈し、
  環境変数・プロセス引数を読まない。
- トークン契約は `sign_audit_export` と同じ。`--database-token` が argv に現れた時点で
  `--database-token is not accepted; set DATABASE_TOKEN in the environment instead`
  を返し、渡された値はエラーへエコーしない。リモート DB トークンは `DATABASE_TOKEN`
  環境変数からのみ読む（argv は `ps` 等で公開されるため）。
- `ensure_no_sidecars(path)` は `<path>-wal` / `<path>-shm` / `<path>-journal` を DB オープン
  前に検査し、残っていれば `backup has a <path> sidecar; stop the server before verifying`
  で拒否する（restore_db と同じ fail-closed）。
- `verify_backup_at(path)` は存在確認 → サイドカー検査 → SQLite オープン →
  `PRAGMA integrity_check` が `ok` → テーブル数 > 0 → `_migrations` 件数 > 0 を順に検証する。
- `decide_output(result)` は成功時に `backup_verification: OK tables=N migrations=N` と
  終了コード0、失敗時は `(None, 1)` を返す。エラー本文は stdout へ出さず stderr の
  `Error: ...` に委譲する。手順は
  [BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md) を参照。

### 共通 argv 契約（`server::cli_args`）

- `backup_db` / `restore_db` / `sign_audit_export` / `verify_audit_export` /
  `verify_audit_log` / `verify_backup` の argv 解釈は `server/src/lib.rs` の
  `cli_args` モジュールへ集約する。各 bin に残すのは受け付けるフラグの定義だけとし、
  `take_value` / `parse_str_args` の共通部分 / `--database-token` 拒否 /
  ハイフン始まりの値拒否 / unknown argument の固定文言は共有実装の単一情報源とする。
- 秘密値は argv で受け付けない。`--database-token` が現れた時点で
  `--database-token is not accepted; set DATABASE_TOKEN in the environment instead`
  を返し、渡された値もトークン名もエラーへエコーしない。リモート libSQL のトークンは
  `DATABASE_TOKEN` 環境変数からのみ読む。
- 値取りは `-` で始まるオプション風トークンを値として受理せず、
  `<flag> requires a value` の固定文言を返す（値・トークンをエコーしない）。
- 未知の引数は `unknown argument: <arg>` の固定文言で拒否する。
  `<arg>` は引数名のみで、秘密値は含めない。
- 既存の CLI 引数回帰テストは各 bin の `#[cfg(test)]` 内に残し、共有関数へ委譲した後も
  期待文言・テスト名・件数を変更しない。
