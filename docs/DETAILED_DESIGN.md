---
title: "詳細設計書 (Detailed Design Document)"
type: Design
description: API、認証、データ設計および未実装事項を記録する。
tags: [detailed-design, api, security]
generated:
  by: dsh
  at: "2026-11-17T13:02:57+09:00"
---

## 詳細設計書 (Detailed Design Document)

本書は現行APIを記録するとともに、必須要件を満たす目標設計・未実装事項を明確にする。
JPKI、JPYC、Rust優先の方針は [ADR-0001](adr/ADR-0001-mvp-trust-boundaries.md) を参照。

## 1. API仕様

実装ルート（`server/src/lib.rs`）:

| Method | Path | 認証 | 用途 |
| --- | --- | --- | --- |
| POST | `/auth/login` | 不要 | モック有効時（`ALLOW_MOCK_AUTH=1`）のみ公開。OID4VP風のnonce応答で完全な認証フローではない。無効時は501 |
| POST | `/auth/verify` | 不要 | 開発用モックまたは証明書入力を処理しJWTを発行 |
| GET | `/auth/oidc/start` | 不要 | state/nonce/PKCEを生成し認可URLを返す。OIDC未設定時は501 |
| GET | `/auth/oidc/callback` | 不要 | 認可コードを交換し、IDトークン検証後にセッションJWTを発行する |
| POST | `/auth/refresh` | Cookie | リフレッシュトークンを回転し新セッションJWTを発行。再利用検知時はファミリ失効 |
| POST | `/auth/logout` | Bearer JWT | `sid` を失効させる |
| POST | `/auth/backchannel-logout` | 不要 | logout token を検証し `sid` および/または `sub` を失効させる（`sub` は `revoked_subs` テーブルへ永続化し起動時にロード） |
| GET | `/health` | 不要 | プロセスの簡易応答 |
| GET | `/ready` | 不要 | readiness。DB接続と停止フラグのみを根拠とし、停止シグナルで503へ切り替わる |
| GET | `/policy/age` | 不要 | 有効な年齢/地域ポリシー設定値（非秘密）を返す。常時公開。 |
| GET | `/metrics` | 不要 | Prometheus 形式の軽量カウンタ（全リクエスト数・4xx/5xx 応答数）。外部クレートなし。 |
| GET | `/content` | Bearer JWT | `Approved` コンテンツを一覧返す。省略可の `q` / `tag` / `type` / `limit` / `offset` で検索・絞り込み・ページング（既定 `limit=50`、範囲 `1..=100`、`offset=0`）、`favorite_only` で本人のお気に入りへ絞り込み |
| GET | `/content/:cid` | Bearer JWT | IPFS取得、サイズ・チェックサム確認後に返す |
| POST | `/content/:cid/favorite` | Bearer JWT | お気に入り追加。`Claims.sub` のみで識別し、本人の行にだけ作用。重複追加は冪等で200 |
| DELETE | `/content/:cid/favorite` | Bearer JWT | お気に入り削除。同上の識別。未登録の削除も冪等で200 |
| GET | `/content/:cid/audit` | Bearer JWT | DB内監査記録を返す |
| POST | `/content/upload` | Bearer JWT | コンテンツ投稿。任意の `title` / `description` / `tags` も受理し長さ上限を検査 |
| GET | `/transactions` | Bearer JWT | 既存の記録を最大100件返す。実決済証明ではない |
| POST | `/payments/orders` | Bearer JWT | `cid`・`chain_id`・`wallet_address`・`amount_jpy` を受け取り、コンテンツの提示価格と `amount_jpy` を照合して JPYC注文を作成する。不正CID・不存在/レビュー中コンテンツ・価格不一致は拒否。受取アドレス未設定時は501 |
| GET | `/payments/orders/:order_id` | Bearer JWT | 本人の注文状態を返す |
| POST | `/payments/orders/:order_id/bind` | Bearer JWT | 注文IDへのウォレット署名を検証する |
| POST | `/payments/orders/:order_id/verify` | Bearer JWT | 送金を検証する。RPC検証未設定時は501 |
| POST | `/content/:cid/support` | Bearer JWT | 無効化済み（501）。模擬ハッシュは支払証明にならないため、JPYC送金検証の実装時に決済APIへ置換する |
| POST | `/mod/report` | Bearer JWT | 通報登録と `Reported` 遷移 |
| GET | `/mod/reports` | Bearer JWT | `Reported` コンテンツの通報一覧 |
| POST | `/mod/vote` | Bearer JWT | 投票署名検証、投票登録、閾値判定 |
| POST | `/mod/reports/{report_id}/finalize` | Bearer JWT | 期限到達済み通報を集計確定（冪等）。期限内・確定済みは変更しない |
| POST | `/mod/operators/{user_hash}/ban` | Bearer JWT | 運用者特権。`MODERATION_OPERATOR_HASHES` 登録者のみBAN可能。根拠・期間必須、`governance_audit_log` に記録 |
| POST | `/mod/operators/{user_hash}/unban` | Bearer JWT | 運用者特権。同上の運用者のみUNBAN可能、`governance_audit_log` に記録 |
| POST | `/mod/content/{cid}/unpin` | Bearer JWT | 運用者特権。ピン解除の判断と理由を `governance_audit_log` に記録（実Pinata操作は手動） |
| GET | `/mod/stats` | Bearer JWT + 運用者 | 認証内集計。`AuthClaims` 必須かつ `require_moderation_operator`（未設定・未登録・BAN済みは403）。`generated_at` / `totals` / `monthly` のみをホワイトリスト方式で返し、CID・reason・`user_hash` は返さない |

### API横断規則

- JWTはHS256既定検証を用い、有効期限を確認する。起動時に32バイト未満の `JWT_SECRET` を拒否する。`JWT_RSA_PRIVATE_KEY_PEM` 設定時は RS256 となり、署名ヘッダに公開鍵由来の `kid`
   を付与する。検証は `JWT_RSA_VERIFICATION_KEYS` に登録した `kid` → 公開鍵で行い、未知 `kid` は拒否する（fail-closed）。`kid` 無しの旧トークンは現行署名鍵で検証するが、
  `JWT_REQUIRE_KID=1` 設定時はこれを拒否する（本番推奨、RS256 限定・HS256 併用時は起動拒否）。発行者/audience検証は別途要件化する。
- CORSは `ALLOWED_ORIGINS`（カンマ区切り）で設定する。`ALLOW_MOCK_AUTH=1` の開発時のみ、未設定ならローカル開発オリジン（`localhost`/`127.0.0.1`/`[::1]`
  の8080）へフォールバックする。モック認証が無効な本番相当で未設定なら全オリジンを拒否（fail-closed、警告ログ付き）。空エントリと `http(s)://` 形式以外（`null` を除く）は無視する。
  末尾スラッシュは除去し、ワイルドカードは指定できない。本番配信オリジンの決定と値の運用は未決定。
- 本番相当（`ALLOW_MOCK_AUTH` 無効）のみ、IP 単位のレート制限と同時実行上限を適用する。
  `RATE_LIMIT_PER_MIN`（既定60/分、有効範囲1〜600）を超過、または `MAX_CONCURRENT_REQUESTS`
  （既定16、有効範囲1〜256）の同時実行数を超過した場合は 429 を返す。不正値・範囲外は既定値を使う。
  `/health`・`/ready`・`/metrics` は監視用のため除外し、`ALLOW_MOCK_AUTH=1` の開発時は適用しない。
- リクエスト本文は最大10 MiB。`Content-Length` が上限を超える場合はミドルウェアが Multipart 解析より先に 413 を返す（単体テストと実サーバーで確認）。`Content-Length`
  が無い／虚偽のチャンクド転送は axum の本文制限と Multipart パーサが守り、その場合は 400 になる（多層防御）。IPFS取得応答は `IPFS_TIMEOUT_SECS`（既定15秒）でタイムアウトし、最大10
  MiB（超過は 400）。
- IPFS 取得時にチェックサム不一致を検出した場合は 401 で拒否し、CID と redact 済みゲートウェイ URL のみを WARN ログへ記録する（本文・チェックサム値は出さない）。実サーバーで警告出力を確認済み。
  ゲートウェイは設定順に試し、ネットワークエラー（接続失敗・タイムアウト）に限り `IPFS_MAX_ATTEMPTS`（既定1、最大5）まで同じゲートウェイを再試行する。HTTP エラー・サイズ超過・不正データは再試行せず、
  不正データは次のゲートウェイへ送らず即拒否する。
- 全レスポンスへ `X-Content-Type-Options: nosniff` と `X-Frame-Options: DENY` を付与する（最外側ミドルウェア。
  IP ゲートの 403 を含む）。`/content/{cid}` のユーザー管理バイトが HTML としてスニッフィングされる
  リスクを防ぐ。CSP はクライアントのリソース読み込みへ影響するため本番構成確定時に別途設計する。
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
- `server/src/jpki.rs` は提示データからX.509証明書を解析し公開鍵を抽出するが、信頼チェーン、失効、VP署名、nonce、audience検証を完了していない。実JPKI認証として使用しない。
- 年齢判定は `common::age::AgePolicy`（起動時に環境変数から読込・検証）に基づく。既定は保守的一律ベースライン `21`（法的事実の確定主張ではない）で、
  国別上書き・対象地域・適用モード・明示設定必須のフェイルクローズを構成可能。`trusted_attribute_required` は検証済み属性入力経路が未実装のため現時点でフェイルクローズ。現行の生年月日・国コードは申告値であり、
  モック環境を含め本人確認済み属性として信頼しない。詳細は [AGE_VALIDATION_POLICY.md](AGE_VALIDATION_POLICY.md) を参照。
- 年齢ゲートは fail-closed。`AGE_ENFORCEMENT_MODE=trusted_attribute_required` では
  検証済み属性の入力経路が未実装のため、`/auth/oidc/callback` と `/auth/refresh` を
  `TrustedAttributeRequired` により 403 で拒否し、年齢判定のバイパスを防ぐ（R1-17）。

### OIDC認証フロー（実装済み・モックOPで検証）

1. `GET /auth/oidc/start` が `state`、`nonce`、`code_verifier` を生成し、
   サーバー側に10分間保存する。
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
8. `OIDC_REQUIRED_ACR` / `OIDC_REQUIRED_AMR` を設定すると ID トークンの
   `acr`（いずれか一致）と `amr`（すべて含有）を検証し、欠落・不一致なら
   認証を拒否する（fail-closed）。`OIDC_MAX_AUTH_AGE_SECS` を設定すると
   認証要求へ `max_age` を付与し、ID トークンの `auth_time` が
   `現在時刻 - max_age - clock_skew` 以降であることを要求する（欠落も拒否）。
   本番ではこれら3設定を必須とする。
   未実装: 複数プロセス間の失効状態共有・実OP運用整合
   （`sub` 失効の DB 永続化は実装済み）。
9. 実デジタル認証サービスへの接続は、クライアントID取得後に検証する。
   現行のテストはローカルOPモックでフローと検証項目を確認する。

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
- `title` / `description` / `tags` は省略可で、UTF-8 バイト数（`str::len()`）で `title` は200 B、
  `description` は2000 B、`tags` は10個まで・各タグは50 B を上限とする。超過は400で拒否し、
  空文字は受理し、空ベクトル（保存時は空文字）として扱う。`tags` はカンマ区切りの1フィールドで、
  未知フィールド・重複フィールドは既存どおり400で拒否する。
- **未実装**: 完全なJPEG XLデコード検証、画像デコード前検査。技術選定は [ADR-0006](adr/ADR-0006-jxl-decoding.md) に記録済み（第一候補 `jxl-oxide`、
  現時点では依存追加しない判断）。

### 一覧 `GET /content`

- `status = 'Approved'` のみを対象とし、`created_at DESC` で返す（既存の並び順を維持）。
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
- パブリックゲートウェイ依存（候補は `IPFS_GATEWAYS` で変更できるが、既定はパブリックで廃止・劣化に応じた見直しが必要）、CIDのURLパス注入拒否は実装済み。厳密なCID仕様検証、リトライ/キャッシュ方針、
  単一ゲートウェイ異常時の運用手順は未確定。

## 4. モデレーション

- 通報は `/mod/report` に `target_cid` と `reason` を送る。通報者のJWT主体を監査ログの `changed_by` に用いる。存在しないコンテンツは404、レビュー中（`Reported`
  ）の重複通報は400で拒否する。`reports.deadline_at` に受理から7日を記録する。確定から30日以内の再通報は `appeal_of` に元通報を記録する異議として受理し、元の確定を上書きしない。
- 投票は `/mod/vote` に `report_id`、`juror_hash`、`result`、署名を送る。
  JWT主体と `juror_hash` の一致、結果値、重複投票、期限切れ、対象投稿者（除外）を検査する。存在しない `report_id` は404、対象コンテンツが `Reported`
  でない（確定済み）場合・期限切れ・投稿者の場合は400で拒否する。
- 期限到達後に `POST /mod/reports/{report_id}/finalize`（認証必須）または `process_report_conclusion` で集計する。期限内に呼び出しても確定しない（3票即時確定は廃止）。
  定足数3票未達なら通報を棄却し `Approved` に復帰、到達時は Reject >= Approve なら `Rejected`、それ以外は `Approved` とする。同数は `Rejected`（ADR-0004）。
- 確定時刻は `reports.concluded_at` に記録し、確定後は追加投票を受け付けないため、多数決結果が後続票で再遷移することはない。
- 運用者はプロジェクト側で固定せず、各デプロイが `MODERATION_OPERATOR_HASHES` で設定する。未設定・未登録・BAN済みの主体は BAN/UNBAN/UNPIN を拒否する。BAN
  は根拠と期間（日数）を必須とし、UNPIN は判断と理由のみを記録する（実 Pinata ピン解除は手動）。これらは `governance_audit_log` に保存するが、DEC-04 の第三者検証可能な監査ではない。
- 期限到達は起動時のバックグラウンドワーカーが自動検知し、`MODERATION_FINALIZE_INTERVAL_SECS`（既定300秒、30〜3600秒）ごとに `finalize_due_reports` を実行する。
  停止シグナルでワーカーも終了する。手動の `POST /mod/reports/{report_id}/finalize` も残しており、両者は冪等。手動 finalize は認証済みユーザーなら誰でも呼べるが、
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

## 5. データモデル（現行起動時DDLの要約）

|Table|主な列|用途・制約|
|---|---|---|
|`users`|`user_hash`, `public_key_der`, `is_banned`, `banned_at`|利用者識別・公開鍵・BAN状態|
|`contents`|`cid`, `type`, `owner_hash`, `checksum`, `status`, `price`, `created_at`, `title`, `description`, `tags`|CIDを主キーとするコンテンツメタデータ。メタデータ3列は migration 011 で追加、`tags` はカンマ区切りで `idx_contents_tags` インデックス付き|
|`favorites`|`id`, `user_hash`, `cid`, `created_at`|お気に入り。`(user_hash, cid)` に一意インデックス。`users(user_hash)` / `contents(cid)` へ `ON DELETE CASCADE` のFK|
|`reports`|`report_id`, `target_cid`, `reason`, `created_at`, `deadline_at`, `concluded_at`, `appeal_of`|通報記録（7日期限・確定時刻・異議の連鎖）|
|`votes`|`vote_id`, `report_id`, `voter_hash`, `vote_result`, `voted_at`|投票記録。現行DDLに複合一意制約なし|
|`signatures`|`target_id`, `signer_hash`, `signature_blob`, `algorithm`|投稿・投票署名の保存|
|`content_audit_log`|状態、主体、理由、連鎖ハッシュ、時刻|DB内状態遷移記録|
|`governance_audit_log`|`action`（BAN/UNBAN/UNPIN）、対象、運用者、根拠、詳細、時刻|運用判断の記録。連鎖・署名なし（DEC-04 未達）|
|`transactions`|CID、送受信者、amount、`tx_hash`、時刻|現行の模擬支援記録。実決済の証明ではない|
|`payment_orders`|注文ID、PPID、CID、チェーン、ウォレット、受取先、金額、状態、tx、確認数、期限、派生index|JPYC決済の状態機械。`(chain_id, tx_hash, log_index)` 一意。`derivation_index` も一意|

DDLは `server/migrations/` の番号付きSQL（現行: `001_initial_schema.sql` 〜 `012_favorites.sql`）としてバージョン管理され、起動時に
`server/src/migration.rs` のランナーが未適用分のみ適用する。適用済みバージョンとSHA-256チェックサムは `_migrations` テーブルに記録され、
適用済みマイグレーションの改変は起動時の検証で検出し起動を失敗させる。起動時の検証は適用済みだがファイルが無いバージョンも拒否する（古いデプロイイメージの誤用防止）。既存DBに新規マイグレーションを追加する段階的アップグレードはテスト済み。
migration 011 は `contents` へ `title` / `description` / `tags`（いずれも `TEXT NOT NULL DEFAULT ''`）を追加し、
`tags` に `idx_contents_tags` インデックスを作成する。既存行のメタデータは空文字として読み出され、
適用済みマイグレーションの改変は従来どおり起動時に検出される。
migration 012 は `favorites` テーブルを作成する。`user_hash` は `users(user_hash)`、`cid` は
`contents(cid)` を参照し、いずれも `ON DELETE CASCADE`。一意性は `idx_favorites_user_cid`
（`UNIQUE(user_hash, cid)`）で担保し、本人分の新着順参照用に
`idx_favorites_user_created`（`user_hash, created_at DESC`）を作成する。
ロールバック（downマイグレーション）は未実装。外部キーはlibSQLで強制され、テストでも検証する。投票の重複は `votes(report_id, voter_hash)` の一意インデックスでも拒否する。

## 6. 監査ハッシュ

- 新規エントリは `AuditLogEntry::calculate_hash_v2` で生成する。CID、状態、主体、理由、前エントリーハッシュに長さプレフィックスを付け、ドメイン分離ラベルを付与した上でSHA-256化するため、
  フィールド境界の曖昧さによる衝突を排除する。既存DB向けに旧 `calculate_hash`（v1）も残し、`verify_audit_log` は v1/v2 の両方を検証する。
- `compute_audit_chain_digest` は `log_id` の連続性と entry_hash 列の Merkle root を計算する。`verify_audit_log --expect-root`
  で外部保存した root と照合すると、行の欠落・順序入替・末尾切り詰めを検出できる（実DBで確認済み）。
- `sign_audit_export` は件数と Merkle root を RSA 署名したスナップショットを生成し、`verify_audit_export` は公開鍵 DER のみで署名・完全性を検証する（ADR-0005、
  [AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md)）。これは運営者が公開したことを示すに留まり、DB外アンカーや追記専用媒体の代替ではない。
- DB外アンカー、信用できる第三者による独立検証、鍵の HSM 管理とローテーションは未実装。
- 目的はDB内の連鎖整合性チェック補助に限定し、改ざん不能・否認防止を主張しない。root の外部保存先は各デプロイの運用者が決める。

## 7. 必須要件を満たす目標フロー

### JPKI認証

サーバーは一意なchallenge/nonceを生成し、指定マイナアプリからの提示を受ける。信頼された発行元、署名、nonce結合、audience、証明書・失効状態、必要な年齢属性を公式仕様に基づき検証し、
成功時のみ短寿命のAPIトークンを発行する。具体API・証明形式は公式仕様確認後に確定する。

トークン発行は検証済み認証トランザクションにサーバー側で結び付ける。クライアントが送る `user_hash` を認証主体として信用しない。安定識別子の生成方式は個人間追跡可能性を抑える設計と合わせて決定する。

### JPYC決済

目標方式は [ADR-0003](adr/ADR-0003-jpyc-settlement.md) を参照。

`PAYMENT_RECEIVING_XPRV` が設定されている場合、注文ごとに BIP44
（`m/44'/60'/0'/0/{index}`）で受取アドレスを派生し、`derivation_index` を
一意に記録する。未設定時は `PAYMENT_RECEIVING_ADDRESS` の単一アドレスを使う。

決済要求、利用者ウォレット署名、チェーン送信、トランザクション確認、最終性、失敗・再試行を別状態として扱う。受理対象チェーンとJPYCコントラクトをallowlistで識別し、
要求ID・金額・受取先に結び付く送金を検証した後だけ有償機能を解放する。成功を確認できた取引だけを決済成立とする。実際の手数料と確認時間を利用者へ提示する。

`CONFIRMING` の注文は同じ送金で再 verify でき、確認数が進展すれば `FINALIZED` へ進む。`FINALIZED` は確定後不変とし、再 verify
や確認数の後退でも状態を戻さない（実チェーン上の再編成検知は RPC 実装時に別途検討）。

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
  全文検索、あいまい一致、スコアリング、およびブラウザ URL との検索条件同期は未実装。
