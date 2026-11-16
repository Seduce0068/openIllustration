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
| POST | `/auth/refresh` | Cookie | リフレッシュトークンを回転し新セッションJWTを発行。再利用検知時はファミリ失効 |
| POST | `/auth/logout` | Bearer JWT | `sid` を失効させる |
| POST | `/auth/backchannel-logout` | 不要 | logout token を検証し `sid` および/または `sub` を失効させる（`sub` はインメモリ失効閾値方式） |
| GET | `/health` | 不要 | プロセスの簡易応答 |
| GET | `/policy/age` | 不要 | 有効な年齢/地域ポリシー設定値（非秘密）を返す。常時公開。 |
| GET | `/content` | Bearer JWT | `Approved` コンテンツを最大50件返す |
| GET | `/content/:cid` | Bearer JWT | IPFS取得、サイズ・チェックサム確認後に返す |
| GET | `/content/:cid/audit` | Bearer JWT | DB内監査記録を返す |
| POST | `/content/upload` | Bearer JWT | コンテンツ投稿 |
| GET | `/transactions` | Bearer JWT | 既存の記録を最大100件返す。実決済証明ではない |
| POST | `/payments/orders` | Bearer JWT | `cid`・`chain_id`・`wallet_address`・`amount_jpy` を受け取り、コンテンツの提示価格と `amount_jpy` を照合して JPYC注文を作成する。不正CID・不存在/レビュー中コンテンツ・価格不一致は拒否。受取アドレス未設定時は501 |
| GET | `/payments/orders/:order_id` | Bearer JWT | 本人の注文状態を返す |
| POST | `/payments/orders/:order_id/bind` | Bearer JWT | 注文IDへのウォレット署名を検証する |
| POST | `/payments/orders/:order_id/verify` | Bearer JWT | 送金を検証する。RPC検証未設定時は501 |
| POST | `/content/:cid/support` | Bearer JWT | 無効化済み（501）。模擬ハッシュは支払証明にならないため、JPYC送金検証の実装時に決済APIへ置換する |
| POST | `/mod/report` | Bearer JWT | 通報登録と `Reported` 遷移 |
| GET | `/mod/reports` | Bearer JWT | `Reported` コンテンツの通報一覧 |
| POST | `/mod/vote` | Bearer JWT | 投票署名検証、投票登録、閾値判定 |

### API横断規則

- JWTはHS256既定検証を用い、有効期限を確認する。起動時に32バイト未満の `JWT_SECRET` を拒否する。`JWT_RSA_PRIVATE_KEY_PEM` 設定時は RS256 となり、署名ヘッダに公開鍵由来の `kid` を付与する。検証は `JWT_RSA_VERIFICATION_KEYS` に登録した `kid` → 公開鍵で行い、未知 `kid` は拒否する（fail-closed）。`kid` 無しの旧トークンは現行署名鍵で検証する。発行者/audience検証は別途要件化する。
- CORSは `ALLOWED_ORIGINS`（カンマ区切り）で設定する。`ALLOW_MOCK_AUTH=1` の開発時のみ、未設定ならローカル開発オリジン（`localhost`/`127.0.0.1`/`[::1]` の8080）へフォールバックする。モック認証が無効な本番相当で未設定なら全オリジンを拒否（fail-closed、警告ログ付き）。空エントリと `http(s)://` 形式以外（`null` を除く）は無視する。末尾スラッシュは除去し、ワイルドカードは指定できない。本番配信オリジンの決定と値の運用は未決定。
- リクエスト本文は最大10 MiB。IPFS取得応答は `IPFS_TIMEOUT_SECS`（既定15秒）でタイムアウトし、最大10 MiB。
- 500系エラーの内部詳細（DB・IO・内部メッセージ）はログのみに出し、クライアントには `Internal server error` を返す。開発時は `EXPOSE_INTERNAL_ERRORS=1` で詳細を返す。
- ログは `tracing` + `tracing-subscriber` で標準出力へ出し、`RUST_LOG` でレベルを制御する（未設定時は INFO、タイムスタンプは RFC 3339 UTC）。JWT・認証ヘッダ・投稿本文など機微情報はログへ出さない。
- 停止時は SIGTERM/SIGINT を捕捉し、処理中リクエストの完了を待って終了する（graceful shutdown）。待ち時間は `GRACEFUL_DRAIN_TIMEOUT_SECS`（既定 30 秒、範囲 1〜300）で上限を設け、超過時は警告ログを出して残接続を切断する。停止シグナルで readiness は 503 へ切り替わる。
- 起動時はローカル DB の `<DB path>.instance.lock` を原子的に作成し、単一ライターを強制する。生存 PID のロックがある場合は第二インスタンスの起動を拒否（fail-closed）。クラッシュ後の stale ロックは自動回収し、正常停止時に削除する。リモート DB はこの排他が使えず警告のみ。

## 2. 認証・認可（必須要件 / 現行実装との差分）

目標方式は [ADR-0002](adr/ADR-0002-jpki-authentication.md) を参照。

- 指定マイナアプリ/JPKIで認証できない利用者は、閲覧を含むサイト機能を利用できない。
- 現行の閲覧系ルート（`GET /content`、`GET /content/:cid`、
  `GET /content/:cid/audit`、`GET /transactions`、`GET /mod/reports`）は
  JWT認証を要求する。OIDC認証でJWTを発行するフローは実装済みだが、
  実デジタル認証サービスへの接続・事業者申込・実機検証は未完了であり、
  本番要件を満たすには追加の検証が必要。
- `/health`、`GET /policy/age`、およびJPKI認証開始・静的ログイン画面は認証前アクセスを許容できるが、コンテンツや利用者データを返さない。`GET /policy/age` は有効な年齢/地域ポリシー設定値（非秘密）のみを返し、常時公開する。
- 開発用モックは `ALLOW_MOCK_AUTH=1` の場合のみ許可する。未設定時は無効。
- 認証が必要な読み取りルートは `AuthClaims` によりJWTを検証する。未認証アクセスを拒否するが、本人確認の真正性を確立するものではない。
- `/auth/login` と `/auth/verify` は開発用モック経路。`ALLOW_MOCK_AUTH=1` の場合のみ有効で、login が発行する nonce を verify で必須化し、5分 TTL の単回消費（リプレイ拒否）とする。VP 署名検証は未実装であり、本番では OIDC 経路を使用する。
- `server/src/jpki.rs` は提示データからX.509証明書を解析し公開鍵を抽出するが、信頼チェーン、失効、VP署名、nonce、audience検証を完了していない。実JPKI認証として使用しない。
- 年齢判定は `common::age::AgePolicy`（起動時に環境変数から読込・検証）に基づく。既定は保守的一律ベースライン `21`（法的事実の確定主張ではない）で、国別上書き・対象地域・適用モード・明示設定必須のフェイルクローズを構成可能。`trusted_attribute_required` は検証済み属性入力経路が未実装のため現時点でフェイルクローズ。現行の生年月日・国コードは申告値であり、モック環境を含め本人確認済み属性として信頼しない。詳細は [AGE_VALIDATION_POLICY.md](AGE_VALIDATION_POLICY.md) を参照。

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
   `events` を検証し、`sid` を失効させる。`sub` が含まれる場合は `sub` 単位の失効閾値（`iat` 比較）を記録し既存セッションを拒否する。閾値は `revoked_subs` テーブルへも保存し、起動時にログインへロードして再起動後も失効を維持する。再認証時は閾値より新しい `iat` を発行して再ログインを許容し、DB の閾値も削除する（多プロセス共有・実OP連携は未検証）。
7. リフレッシュトークンは `refresh_tokens` テーブルに SHA-256 ハッシュのみ保存し、ログイン時に 30 日間有効なトークンを HttpOnly Cookie で発行する。`POST /auth/refresh` は毎回トークンを回転し、使用済みトークンの再提示を検知した場合はファミリ全体を失効する。ログアウト時もファミリを失効する。
8. 未実装: `acr`/`amr` の保証レベル検証、`auth_time`/`max_age`、
   複数プロセス間の失効状態共有・実OP運用整合（`sub` 失効の DB 永続化は実装済み）。
9. 実デジタル認証サービスへの接続は、クライアントID取得後に検証する。
   現行のテストはローカルOPモックでフローと検証項目を確認する。

## 3. コンテンツ処理

### 投稿 `POST /content/upload`

- 入力: `multipart/form-data` の `file`、`owner_hash`、`signature`、任意の `price`。
- JWT主体と `owner_hash` の一致を確認する。
- Content-Typeを検査し、IllustrationまたはNovelに分類する。
- `price` は64bit整数で `0..=MAX_ORDER_JPY`（1,000,000）。注文側 `jpy_to_wei` と同じ上限をアップロード時にも適用し、注文不可能な価格の登録を拒否する。ファイル全体のHTTP本文上限は10 MiB。
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

- 通報は `/mod/report` に `target_cid` と `reason` を送る。通報者のJWT主体を監査ログの `changed_by` に用いる。存在しないコンテンツは404、レビュー中（`Reported`）の重複通報は400で拒否する。`reports.deadline_at` に受理から7日を記録する。確定から30日以内の再通報は `appeal_of` に元通報を記録する異議として受理し、元の確定を上書きしない。
- 投票は `/mod/vote` に `report_id`、`juror_hash`、`result`、署名を送る。
  JWT主体と `juror_hash` の一致、結果値、重複投票、期限切れ、対象投稿者（除外）を検査する。存在しない `report_id` は404、対象コンテンツが `Reported` でない（確定済み）場合・期限切れ・投稿者の場合は400で拒否する。
- 期限到達後に `POST /mod/reports/{report_id}/finalize`（認証必須）または `process_report_conclusion` で集計する。期限内に呼び出しても確定しない（3票即時確定は廃止）。定足数3票未達なら通報を棄却し `Approved` に復帰、到達時は Reject >= Approve なら `Rejected`、それ以外は `Approved` とする。同数は `Rejected`（ADR-0004）。
- 確定時刻は `reports.concluded_at` に記録し、確定後は追加投票を受け付けないため、多数決結果が後続票で再遷移することはない。
- 運用者はプロジェクト側で固定せず、各デプロイが `MODERATION_OPERATOR_HASHES` で設定する。未設定・未登録・BAN済みの主体は BAN/UNBAN/UNPIN を拒否する。BAN は根拠と期間（日数）を必須とし、UNPIN は判断と理由のみを記録する（実 Pinata ピン解除は手動）。これらは `governance_audit_log` に保存するが、DEC-04 の第三者検証可能な監査ではない。
- 期限到達は起動時のバックグラウンドワーカーが自動検知し、`MODERATION_FINALIZE_INTERVAL_SECS`（既定300秒、30〜3600秒）ごとに `finalize_due_reports` を実行する。停止シグナルでワーカーも終了する。手動の `POST /mod/reports/{report_id}/finalize` も残しており、両者は冪等。
- **未実装**: 投票者選出・Sybil耐性、オンチェーン記録、ランダム陪審員選出、第三者検証可能な監査。モデレーション結果を法的判断とみなさない。

## 5. データモデル（現行起動時DDLの要約）

|Table|主な列|用途・制約|
|---|---|---|
|`users`|`user_hash`, `public_key_der`, `is_banned`, `banned_at`|利用者識別・公開鍵・BAN状態|
|`contents`|`cid`, `type`, `owner_hash`, `checksum`, `status`, `price`, `created_at`|CIDを主キーとするコンテンツメタデータ|
|`reports`|`report_id`, `target_cid`, `reason`, `created_at`, `deadline_at`, `concluded_at`, `appeal_of`|通報記録（7日期限・確定時刻・異議の連鎖）|
|`votes`|`vote_id`, `report_id`, `voter_hash`, `vote_result`, `voted_at`|投票記録。現行DDLに複合一意制約なし|
|`signatures`|`target_id`, `signer_hash`, `signature_blob`, `algorithm`|投稿・投票署名の保存|
|`content_audit_log`|状態、主体、理由、連鎖ハッシュ、時刻|DB内状態遷移記録|
|`governance_audit_log`|`action`（BAN/UNBAN/UNPIN）、対象、運用者、根拠、詳細、時刻|運用判断の記録。連鎖・署名なし（DEC-04 未達）|
|`transactions`|CID、送受信者、amount、`tx_hash`、時刻|現行の模擬支援記録。実決済の証明ではない|
|`payment_orders`|注文ID、PPID、CID、チェーン、ウォレット、受取先、金額、状態、tx、確認数、期限、派生index|JPYC決済の状態機械。`(chain_id, tx_hash, log_index)` 一意。`derivation_index` も一意|

DDLは `server/migrations/` の番号付きSQL（現行: `001_initial_schema.sql`、`002_payment_orders.sql`、`003_payment_order_index.sql`、`004_moderation_deadlines.sql`、`005_governance_audit.sql`）としてバージョン管理され、起動時に `server/src/migration.rs` のランナーが未適用分のみ適用する。適用済みバージョンとSHA-256チェックサムは `_migrations` テーブルに記録され、適用済みマイグレーションの改変は起動時の検証で検出し起動を失敗させる。起動時の検証は適用済みだがファイルが無いバージョンも拒否する（古いデプロイイメージの誤用防止）。既存DBに新規マイグレーションを追加する段階的アップグレードはテスト済み。ロールバック（downマイグレーション）は未実装。外部キーはlibSQLで強制され、テストでも検証する。投票の重複は `votes(report_id, voter_hash)` の一意インデックスでも拒否する。

## 6. 監査ハッシュ

- 新規エントリは `AuditLogEntry::calculate_hash_v2` で生成する。CID、状態、主体、理由、前エントリーハッシュに長さプレフィックスを付け、ドメイン分離ラベルを付与した上でSHA-256化するため、フィールド境界の曖昧さによる衝突を排除する。既存DB向けに旧 `calculate_hash`（v1）も残し、`verify_audit_log` は v1/v2 の両方を検証する。
- `compute_audit_chain_digest` は `log_id` の連続性と entry_hash 列の Merkle root を計算する。`verify_audit_log --expect-root` で外部保存した root と照合すると、行の欠落・順序入替・末尾切り詰めを検出できる（実DBで確認済み）。
- `sign_audit_export` は件数と Merkle root を RSA 署名したスナップショットを生成し、`verify_audit_export` は公開鍵 DER のみで署名・完全性を検証する（ADR-0005、[AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md)）。これは運営者が公開したことを示すに留まり、DB外アンカーや追記専用媒体の代替ではない。
- DB外アンカー、信用できる第三者による独立検証、鍵の HSM 管理とローテーションは未実装。
- 目的はDB内の連鎖整合性チェック補助に限定し、改ざん不能・否認防止を主張しない。root の外部保存先は各デプロイの運用者が決める。

## 7. 必須要件を満たす目標フロー

### JPKI認証

サーバーは一意なchallenge/nonceを生成し、指定マイナアプリからの提示を受ける。信頼された発行元、署名、nonce結合、audience、証明書・失効状態、必要な年齢属性を公式仕様に基づき検証し、成功時のみ短寿命のAPIトークンを発行する。具体API・証明形式は公式仕様確認後に確定する。

トークン発行は検証済み認証トランザクションにサーバー側で結び付ける。クライアントが送る `user_hash` を認証主体として信用しない。安定識別子の生成方式は個人間追跡可能性を抑える設計と合わせて決定する。

### JPYC決済

目標方式は [ADR-0003](adr/ADR-0003-jpyc-settlement.md) を参照。

`PAYMENT_RECEIVING_XPRV` が設定されている場合、注文ごとに BIP44
（`m/44'/60'/0'/0/{index}`）で受取アドレスを派生し、`derivation_index` を
一意に記録する。未設定時は `PAYMENT_RECEIVING_ADDRESS` の単一アドレスを使う。

決済要求、利用者ウォレット署名、チェーン送信、トランザクション確認、最終性、失敗・再試行を別状態として扱う。受理対象チェーンとJPYCコントラクトをallowlistで識別し、要求ID・金額・受取先に結び付く送金を検証した後だけ有償機能を解放する。成功を確認できた取引だけを決済成立とする。実際の手数料と確認時間を利用者へ提示する。

`CONFIRMING` の注文は同じ送金で再 verify でき、確認数が進展すれば `FINALIZED` へ進む。`FINALIZED` は確定後不変とし、再 verify や確認数の後退でも状態を戻さない（実チェーン上の再編成検知は RPC 実装時に別途検討）。

### 検証可能な監査

重要イベントを正規化した形式で記録し、署名鍵・連鎖・第三者が検証可能な外部アンカー/追記専用媒体を組み合わせる。個人情報・認証秘密・不要なコンテンツ本文は公開台帳に記録しない。具体方式、訂正・削除との調整は未決定。運用者は各デプロイが設定する（`MODERATION_OPERATOR_HASHES`）。

## 8. 既知の未実装・設計保留

- 実JPKI/マイナアプリ連携と信頼検証、検証済み年齢/居住地属性の取得・検証経路（DEC-01/DEC-02連携）。
- DEC-02の法的確定: 対象地域・必要属性・最終閾値・属性保持期間の専門家/法務レビュー、現地法令適合の確認（`GET /policy/age` と構成可能ポリシーは方針フレームワークであり、法的確定ではない）。
- JPYC送金・チェーン確認・手数料表示と、無効化済み模擬支援API（501）の決済APIへの置き換え。
- 第三者検証可能な監査と運用鍵管理。
- 投票資格・選出・投票期間を含むモデレーション状態機械の完成（重複投票の一意制約と確定後の追加投票拒否は実装済み）。
- 本番CORSオリジンの決定、ロールバック手順を含むDBマイグレーション運用、ログ集約・アラート。
- Rust優先方針に沿う技術例外記録と、性能・軽量性・費用の計測値。
- Arweave、追加ゲートウェイ、Cloudflare Workers/R2、OpenTofu等の必要性・方式。
