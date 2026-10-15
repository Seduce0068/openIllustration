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
| GET | `/health` | 不要 | プロセスの簡易応答 |
| GET | `/content` | Bearer JWT | `Approved` コンテンツを最大50件返す |
| GET | `/content/:cid` | Bearer JWT | IPFS取得、サイズ・チェックサム確認後に返す |
| GET | `/content/:cid/audit` | Bearer JWT | DB内監査記録を返す |
| POST | `/content/upload` | Bearer JWT | コンテンツ投稿 |
| GET | `/transactions` | Bearer JWT | 既存の記録を最大100件返す。実決済証明ではない |
| POST | `/content/:cid/support` | Bearer JWT | 現行コードの模擬支援記録。MVP方針上は削除・無効化対象 |
| POST | `/mod/report` | Bearer JWT | 通報登録と `Reported` 遷移 |
| GET | `/mod/reports` | Bearer JWT | `Reported` コンテンツの通報一覧 |
| POST | `/mod/vote` | Bearer JWT | 投票署名検証、投票登録、閾値判定 |

### API横断規則

- JWTはHS256既定検証を用い、有効期限を確認する。鍵の強度・ローテーション・発行者/audience検証は別途要件化する。
- CORSは現在のローカル開発オリジンに限定。本番配信オリジンは未決定。
- リクエスト本文は最大10 MiB。IPFS取得応答は15秒タイムアウト、最大10 MiB。
- エラー応答に内部DB・ゲートウェイエラーが含まれ得るため、本番向けの公開エラー設計は未完了。

## 2. 認証・認可（必須要件 / 現行実装との差分）

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

## 3. コンテンツ処理

### 投稿 `POST /content/upload`

- 入力: `multipart/form-data` の `file`、`owner_hash`、`signature`、任意の `price`。
- JWT主体と `owner_hash` の一致を確認する。
- Content-Typeを検査し、IllustrationまたはNovelに分類する。
- `price` は整数かつ0以上。ファイル全体のHTTP本文上限は10 MiB。
- 署名対象はファイルのバイト列。開発モックでは全ゼロ署名を許容する。
- Pinataへのアップロード後、CID・SHA-256チェックサム等をDBへ登録する。
- Content-Typeは `image/jxl` または `text/markdown` のみ許可し、JPEG XLシグネチャまたはMarkdownのUTF-8を検査する。既知フィールドの重複と未知フィールドは拒否する。
- **未実装**: 完全なJPEG XLデコード検証、画像デコード前検査、料金上限、ファイル内容に応じた安全な保存名。

### 取得 `GET /content/:cid`

- DBから期待チェックサムを取得し、固定されたIPFSゲートウェイ候補を順に要求する。
- 1要求15秒、応答最大10 MiB。受信ストリームを上限内で蓄積し、SHA-256一致後に返却する。
- 現行応答のContent-Typeは `image/jxl` 固定。Novel対応の正しいContent-Type付与は未実装。
- クライアントはMarkdownのraw HTMLイベントをテキストとしてHTML化し、タグをエスケープしてから `inner_html` に渡す。
- パブリックゲートウェイ依存（候補はコード内固定で、廃止・劣化に応じた見直しが必要）、CID形式検証、リトライ/キャッシュ方針、単一ゲートウェイ異常時の運用手順は未確定。

## 4. モデレーション

- 通報は `/mod/report` に `target_cid` と `reason` を送る。通報者のJWT主体を監査ログの `changed_by` に用いる。
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

DDLは `server/migrations/` の番号付きSQL（現行: `001_initial_schema.sql`）としてバージョン管理され、起動時に `server/src/migration.rs` のランナーが未適用分のみ適用する。適用済みバージョンとSHA-256チェックサムは `_migrations` テーブルに記録され、適用済みマイグレーションの改変は起動時の検証で検出し起動を失敗させる。ロールバック（downマイグレーション）は未実装。外部キーはlibSQLで強制され、テストでも検証する。投票の重複は `votes(report_id, voter_hash)` の一意インデックスでも拒否する。

## 6. 監査ハッシュ

- `AuditLogEntry::calculate_hash` はCID、状態、主体、理由、前エントリーハッシュをSHA-256で結合する。
- 一意で曖昧さのない正規化シリアライズ、署名鍵、DB外アンカー、独立検証コマンドは未実装。
- 目的はDB内の連鎖整合性チェック補助に限定し、改ざん不能・否認防止を主張しない。

## 7. 必須要件を満たす目標フロー

### JPKI認証

サーバーは一意なchallenge/nonceを生成し、指定マイナアプリからの提示を受ける。信頼された発行元、署名、nonce結合、audience、証明書・失効状態、必要な年齢属性を公式仕様に基づき検証し、成功時のみ短寿命のAPIトークンを発行する。具体API・証明形式は公式仕様確認後に確定する。

トークン発行は検証済み認証トランザクションにサーバー側で結び付ける。クライアントが送る `user_hash` を認証主体として信用しない。安定識別子の生成方式は個人間追跡可能性を抑える設計と合わせて決定する。

### JPYC決済

決済要求、利用者ウォレット署名、チェーン送信、トランザクション確認、最終性、失敗・再試行を別状態として扱う。受理対象チェーンとJPYCコントラクトをallowlistで識別し、要求ID・金額・受取先に結び付く送金を検証した後だけ有償機能を解放する。成功を確認できた取引だけを決済成立とする。実際の手数料と確認時間を利用者へ提示する。

### 検証可能な監査

重要イベントを正規化した形式で記録し、署名鍵・連鎖・第三者が検証可能な外部アンカー/追記専用媒体を組み合わせる。個人情報・認証秘密・不要なコンテンツ本文は公開台帳に記録しない。具体方式、運営主体、訂正・削除との調整は未決定。

## 8. 既知の未実装・設計保留

- 実JPKI/マイナアプリ連携と信頼検証。
- JPYC送金・チェーン確認・手数料表示と、現行模擬支援APIの置き換え。
- 第三者検証可能な監査と運用鍵管理。
- 投票資格・選出・投票期間を含むモデレーション状態機械の完成（重複投票の一意制約と確定後の追加投票拒否は実装済み）。
- 環境別CORS、エラー公開方針、ロールバック手順を含むDBマイグレーション運用、可観測性。
- Rust優先方針に沿う技術例外記録と、性能・軽量性・費用の計測値。
- Arweave、追加ゲートウェイ、Cloudflare Workers/R2、OpenTofu等の必要性・方式。
