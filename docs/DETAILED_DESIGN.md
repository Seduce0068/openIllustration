---
type: Design
description: API、認証、データ設計および未実装事項を記録する。
tags: [detailed-design, api, security]
---

# 詳細設計書 (Detailed Design Document)

本書は現行APIを記録するとともに、必須要件を満たす目標設計・未実装事項を明確にする。
JPKI、JPYC、Rust優先の方針は [ADR-0001](adr/ADR-0001-mvp-trust-boundaries.md) を参照。

## 1. API仕様

実装ルート（`server/src/lib.rs`）:

| Method | Path | 認証 | 用途 |
| --- | --- | --- | --- |
| POST | `/auth/login` | 不要 | OID4VP風のnonce応答。完全な認証フローではない |
| POST | `/auth/verify` | 不要 | 開発用モックまたは証明書入力を処理しJWTを発行 |
| GET | `/auth/oidc/start` | 不要 | state/nonce/PKCEを生成し認可URLを返す。OIDC未設定時は501 |
| GET | `/auth/oidc/callback` | 不要 | 認可コードを交換し、IDトークン検証後にセッションJWTを発行する |
| GET | `/health` | 不要 | プロセスの簡易応答 |
| GET | `/content` | Bearer JWT | `Approved` コンテンツを最大50件返す |
| GET | `/content/:cid` | Bearer JWT | IPFS取得、サイズ・チェックサム確認後に返す |
| GET | `/content/:cid/audit` | Bearer JWT | DB内監査記録を返す |
| POST | `/content/upload` | Bearer JWT | コンテンツ投稿 |
| GET | `/transactions` | Bearer JWT | 既存の記録を最大100件返す。実決済証明ではない |
| POST | `/payments/orders` | Bearer JWT | JPYC注文を作成する。受取アドレス未設定時は501 |
| GET | `/payments/orders/:order_id` | Bearer JWT | 本人の注文状態を返す |
| POST | `/payments/orders/:order_id/bind` | Bearer JWT | 注文IDへのウォレット署名を検証する |
| POST | `/payments/orders/:order_id/verify` | Bearer JWT | 送金を検証する。RPC検証未設定時は501 |
| POST | `/content/:cid/support` | Bearer JWT | 無効化済み（501）。模擬ハッシュは支払証明にならないため、JPYC送金検証の実装時に決済APIへ置換する |
| POST | `/mod/report` | Bearer JWT | 通報登録と `Reported` 遷移 |
| GET | `/mod/reports` | Bearer JWT | `Reported` コンテンツの通報一覧 |
| POST | `/mod/vote` | Bearer JWT | 投票署名検証、投票登録、閾値判定 |

### API横断規則

- JWTはHS256既定検証を用い、有効期限を確認する。起動時に32バイト未満の `JWT_SECRET` を拒否する。鍵のローテーション・発行者/audience検証は別途要件化する。
- CORSは `ALLOWED_ORIGINS`（カンマ区切り）で設定する。未設定時はローカル開発オリジン（`localhost`/`127.0.0.1`/`[::1]` の8080）のみ許可する。空エントリと `http(s)://` 形式以外（`null` を除く）は無視する。末尾スラッシュは除去し、ワイルドカードは指定できない。本番配信オリジンの決定と値の運用は未決定。
- リクエスト本文は最大10 MiB。IPFS取得応答は `IPFS_TIMEOUT_SECS`（既定15秒）でタイムアウトし、最大10 MiB。
- 500系エラーの内部詳細（DB・IO・内部メッセージ）はログのみに出し、クライアントには `Internal server error` を返す。開発時は `EXPOSE_INTERNAL_ERRORS=1` で詳細を返す。
- ログは `tracing` + `tracing-subscriber` で標準出力へ出し、`RUST_LOG` でレベルを制御する（未設定時は INFO、タイムスタンプは RFC 3339 UTC）。JWT・認証ヘッダ・投稿本文など機微情報はログへ出さない。
- 停止時は SIGTERM/SIGINT を捕捉し、処理中リクエストの完了を待って終了する（graceful shutdown）。

## 2. 認証・認可（必須要件 / 現行実装との差分）

目標方式は [ADR-0002](adr/ADR-0002-jpki-authentication.md) を参照。

- 指定マイナアプリ/JPKIで認証できない利用者は、閲覧を含むサイト機能を利用できない。
- 現行の閲覧系ルート（`GET /content`、`GET /content/:cid`、
  `GET /content/:cid/audit`、`GET /transactions`、`GET /mod/reports`）は
  JWT認証を要求する。JPKI認証でJWTを発行するフローは未実装であり、
  現行JWT認証だけでは本番要件を満たさない。
- `/health` とJPKI認証開始・静的ログイン画面は認証前アクセスを許容できるが、コンテンツや利用者データを返さない。
- 開発用モックは `ALLOW_MOCK_AUTH=1` の場合のみ許可する。未設定時は無効。
- 認証が必要な読み取りルートは `AuthClaims` によりJWTを検証する。未認証アクセスを拒否するが、本人確認の真正性を確立するものではない。
- 現行 `/auth/login` のnonceは `/auth/verify` の検証に結び付いていない。nonce再利用防止やVP署名検証も未実装。
- `server/src/jpki.rs` は提示データからX.509証明書を解析し公開鍵を抽出するが、信頼チェーン、失効、VP署名、nonce、audience検証を完了していない。実JPKI認証として使用しない。
- 年齢判定はリクエストされた生年月日・国コードに依存する。モックでは本人属性として信頼しない。

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
   `events` を検証し、`sid` を失効させる。`sub` のみの logout は未実装で 501。
7. 未実装: `acr`/`amr` の保証レベル検証、`auth_time`/`max_age`、
   リフレッシュトークン、`sub` 単位の全セッション失効。
8. 実デジタル認証サービスへの接続は、クライアントID取得後に検証する。
   現行のテストはローカルOPモックでフローと検証項目を確認する。

## 3. コンテンツ処理

### 投稿 `POST /content/upload`

- 入力: `multipart/form-data` の `file`、`owner_hash`、`signature`、任意の `price`。
- JWT主体と `owner_hash` の一致を確認する。
- Content-Typeを検査し、IllustrationまたはNovelに分類する。
- `price` は64bit整数かつ0以上。上限は表現範囲（`i64::MAX`）のみで、取引上限は未実装（後述）。ファイル全体のHTTP本文上限は10 MiB。
- 署名対象はファイルのバイト列。開発モックでは全ゼロ署名を許容する。
- Pinataへのアップロード後、CID・SHA-256チェックサム等をDBへ登録する。Pinataへ送るファイル名はコンテンツ種別に応じた固定名（`upload.jxl` / `upload.md`）とし、ユーザー入力名は使わない。
- 同一バイト列を再投稿すると同一CIDとなるため、既存CIDとの衝突は400で拒否する（主キー衝突による500を返さない）。
- Content-Typeは `image/jxl` または `text/markdown` のみ許可し、JPEG XLシグネチャまたはMarkdownのUTF-8を検査する。既知フィールドの重複と未知フィールドは拒否する。
- **未実装**: 完全なJPEG XLデコード検証、画像デコード前検査、料金上限、ファイル内容に応じた安全な保存名。

### 取得 `GET /content/:cid`

- DBから期待チェックサムを取得し、設定されたIPFSゲートウェイ候補（`IPFS_GATEWAYS`、既定3件、http(s)のみ）を順に要求する。
- 1要求は `IPFS_TIMEOUT_SECS`（既定15秒、1〜300秒）、応答最大10 MiB。受信ストリームを上限内で蓄積し、SHA-256一致後に返却する。
- Content-TypeはIllustrationで `image/jxl`、Novelで `text/markdown; charset=utf-8` を返す。
- クライアントはMarkdownのraw HTMLイベントをテキストとしてHTML化し、タグをエスケープしてから `inner_html` に渡す。
- リンク・画像のURLは `http` / `https` と、リンクに限り `mailto`、およびスキームとして成立しない接頭辞（相対URL・フラグメント）のみ許可する。それ以外のスキームは `#` に置換する。この検査は描画境界のクライアントでのみ実施し、サーバーはMarkdownを描画せず原本バイト列を保持する。
- パブリックゲートウェイ依存（候補は `IPFS_GATEWAYS` で変更できるが、既定はパブリックで廃止・劣化に応じた見直しが必要）、CIDのURLパス注入拒否は実装済み。厳密なCID仕様検証、リトライ/キャッシュ方針、単一ゲートウェイ異常時の運用手順は未確定。

## 4. モデレーション

- 通報は `/mod/report` に `target_cid` と `reason` を送る。通報者のJWT主体を監査ログの `changed_by` に用いる。存在しないコンテンツは404、レビュー中（`Reported`）の重複通報は400で拒否する。
- 投票は `/mod/vote` に `report_id`、`juror_hash`、`result`、署名を送る。
  JWT主体と `juror_hash` の一致、結果値、重複投票を検査する。存在しない
  `report_id` は404、対象コンテンツが `Reported` でない（確定済み）場合は400で拒否する。
- 現行実装は3票以上で多数決し、RejectがApproveを上回る場合に `Rejected`、それ以外を `Approved` とする。同数はApprovedとなる。
- 確定後は追加投票を受け付けないため、多数決結果が後続票で再遷移することはない。
- **未決定**: 投票期間、投票資格・選出方法、同数時の再審査、通報者制限、結果への異議申立て、Rejected後のBAN方針。
- **未実装**: オンチェーン記録、ランダム陪審員選出、BANテーブル移行。モデレーション結果を法的判断とみなさない。

## 5. データモデル（現行起動時DDLの要約）

|Table|主な列|用途・制約|
|---|---|---|
|`users`|`user_hash`, `public_key_der`, `is_banned`, `banned_at`|利用者識別・公開鍵・BAN状態|
|`contents`|`cid`, `type`, `owner_hash`, `checksum`, `status`, `price`, `created_at`|CIDを主キーとするコンテンツメタデータ|
|`reports`|`report_id`, `target_cid`, `reason`, `created_at`|通報記録|
|`votes`|`vote_id`, `report_id`, `voter_hash`, `vote_result`, `voted_at`|投票記録。現行DDLに複合一意制約なし|
|`signatures`|`target_id`, `signer_hash`, `signature_blob`, `algorithm`|投稿・投票署名の保存|
|`content_audit_log`|状態、主体、理由、連鎖ハッシュ、時刻|DB内状態遷移記録|
|`transactions`|CID、送受信者、amount、`tx_hash`、時刻|現行の模擬支援記録。実決済の証明ではない|
|`payment_orders`|注文ID、PPID、チェーン、ウォレット、受取先、金額、状態、tx、確認数、期限|JPYC決済の状態機械。`(chain_id, tx_hash, log_index)` 一意|

DDLは `server/migrations/` の番号付きSQL（現行: `001_initial_schema.sql`、`002_payment_orders.sql`）としてバージョン管理され、起動時に `server/src/migration.rs` のランナーが未適用分のみ適用する。適用済みバージョンとSHA-256チェックサムは `_migrations` テーブルに記録され、適用済みマイグレーションの改変は起動時の検証で検出し起動を失敗させる。ロールバック（downマイグレーション）は未実装。外部キーはlibSQLで強制され、テストでも検証する。投票の重複は `votes(report_id, voter_hash)` の一意インデックスでも拒否する。

## 6. 監査ハッシュ

- `AuditLogEntry::calculate_hash` はCID、状態、主体、理由、前エントリーハッシュをSHA-256で結合する。
- 一意で曖昧さのない正規化シリアライズ、署名鍵、DB外アンカー、独立検証コマンドは未実装。
- 目的はDB内の連鎖整合性チェック補助に限定し、改ざん不能・否認防止を主張しない。

## 7. 必須要件を満たす目標フロー

### JPKI認証

サーバーは一意なchallenge/nonceを生成し、指定マイナアプリからの提示を受ける。信頼された発行元、署名、nonce結合、audience、証明書・失効状態、必要な年齢属性を公式仕様に基づき検証し、成功時のみ短寿命のAPIトークンを発行する。具体API・証明形式は公式仕様確認後に確定する。

トークン発行は検証済み認証トランザクションにサーバー側で結び付ける。クライアントが送る `user_hash` を認証主体として信用しない。安定識別子の生成方式は個人間追跡可能性を抑える設計と合わせて決定する。

### JPYC決済

目標方式は [ADR-0003](adr/ADR-0003-jpyc-settlement.md) を参照。

決済要求、利用者ウォレット署名、チェーン送信、トランザクション確認、最終性、失敗・再試行を別状態として扱う。受理対象チェーンとJPYCコントラクトをallowlistで識別し、要求ID・金額・受取先に結び付く送金を検証した後だけ有償機能を解放する。成功を確認できた取引だけを決済成立とする。実際の手数料と確認時間を利用者へ提示する。

### 検証可能な監査

重要イベントを正規化した形式で記録し、署名鍵・連鎖・第三者が検証可能な外部アンカー/追記専用媒体を組み合わせる。個人情報・認証秘密・不要なコンテンツ本文は公開台帳に記録しない。具体方式、運営主体、訂正・削除との調整は未決定。

## 8. 既知の未実装・設計保留

- 実JPKI/マイナアプリ連携と信頼検証。
- JPYC送金・チェーン確認・手数料表示と、無効化済み模擬支援API（501）の決済APIへの置き換え。
- 第三者検証可能な監査と運用鍵管理。
- 投票資格・選出・投票期間を含むモデレーション状態機械の完成（重複投票の一意制約と確定後の追加投票拒否は実装済み）。
- 本番CORSオリジンの決定、ロールバック手順を含むDBマイグレーション運用、ログ集約・アラート。
- Rust優先方針に沿う技術例外記録と、性能・軽量性・費用の計測値。
- Arweave、追加ゲートウェイ、Cloudflare Workers/R2、OpenTofu等の必要性・方式。
