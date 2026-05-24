# 詳細設計書 (Detailed Design Document)

本ドキュメントは、基本設計に基づきシステムの実装仕様を定義する。

## 1. API 仕様 (Rust / axum)

全てのエンドポイントは Bearer Token (OID4VP 署名) を必要とする。

### 1.1. 認証・認可

* `POST /auth/login`: JPKI OID4VP 認証フローを開始。
* `POST /auth/verify`: 送信された Verifiable Presentation (VP) を検証し、JWT を発行。

### 1.2. コンテンツ

* `POST /content/upload`:
  * Request: `Content-Type: multipart/form-data`
  * 処理:
        1. 投稿者の署名検証。
        2. 画像（JPEG XL）または小説（Markdown）のハッシュ計算。
        3. IPFS へのプッシュおよび CID の取得。
        4. Turso (libSQL) へのメタデータ登録。
* `GET /content/:cid`: IPFS ゲートウェイまたは R2 からコンテンツを返却。

### 1.3. モデレーション

* `POST /mod/report/:cid`: 違法コンテンツ報告。
* `POST /mod/vote/:report_id`: 陪審員による投票。

## 2. データベーススキーマ (Turso / libSQL)

```sql
-- ユーザーテーブル (JPKI ハッシュで管理)
CREATE TABLE users (
    user_hash TEXT PRIMARY KEY,
    is_banned BOOLEAN DEFAULT FALSE,
    banned_at DATETIME
);

-- コンテンツテーブル
CREATE TABLE contents (
    cid TEXT PRIMARY KEY,
    type TEXT CHECK(type IN ('Illustration', 'Novel')),
    owner_hash TEXT REFERENCES users(user_hash),
    checksum TEXT NOT NULL,
    status TEXT CHECK(status IN ('Approved', 'Reported', 'Rejected')) DEFAULT 'Approved',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- モデレーションテーブル
CREATE TABLE reports (
    report_id INTEGER PRIMARY KEY,
    target_cid TEXT REFERENCES contents(cid),
    reason TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- 投票テーブル
CREATE TABLE votes (
    report_id INTEGER REFERENCES reports(report_id),
    voter_hash TEXT REFERENCES users(user_hash),
    vote_result TEXT CHECK(vote_result IN ('Approve', 'Reject')),
    voted_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

## 3. 認証ステートマシン (OID4VP)

1. `Init`: ユーザーが JPカードをかざし、認証リクエスト生成。
2. `Challenge`: サーバーが JPKI 署名用のランダムナンスを提示。
3. `Verify`: ユーザーがアプリで署名し、VP を返却。Rust 側で署名検証。
4. `Authorized`: 18歳以上検証済みトークンの生成。

## 4. IPFS コンテンツ管理

* コンテンツは IPFS の Content Addressing に従い、CID v1 (base32) を使用する。
* コンテンツ取得時は `https://ipfs.io/ipfs/<cid>` 等のパブリックゲートウェイを直接参照せず、システム内の安全なゲートウェイ経由で解決する。
