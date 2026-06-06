# OpenIllustration / Digital Guild

分散型インフラ（IPFS）と公的個人認証（JPKI）を基盤とした、次世代イラスト・小説投稿プラットフォーム。
ディジタルフォレンジクス（証拠性）を核に設計されており、全ての投稿とガバナンス操作が数学的に証明可能です。

## 🌟 妥協なき主要機能

1.  **フォレンジック・アップロード**: 投稿バイナリに対し、クライアント側で生成されたデジタル署名を付与。否認不可性を担保します。
2.  **分散型不変ストレージ**: コンテンツは IPFS に格納。CID によるコンテンツ・アドレッシングで改ざんを防止します。
3.  **Wasm 描画検証**: IPFS から取得したデータを、ブラウザ内の Rust/Wasm モジュールが再検証。DB のハッシュと一致しない限り表示を許可しません。
4.  **陪審制ガバナンス (Jury System)**: ユーザー間の相互監視。投票には陪審員の署名が必須であり、全ての操作ログが追記専用の監査台帳に記録されます。
5.  **多層防御セキュリティ**: JWT 認可ミドルウェアと RSA 署名検証による二重の防壁。

## 🛠 技術スタック

- **Backend**: Rust (Axum, libSQL/Turso, x509-parser, jsonwebtoken)
- **Frontend**: Rust (Leptos CSR, Tailwind CSS)
- **Shared**: Rust (common crate)
- **Infrastructure**: IPFS (Pinata integration)
- **Tools**: Trunk (Wasm bundler)

## 🚀 クイックスタート

### 1. 依存ツールのインストール

```bash
cargo install --locked trunk
rustup target add wasm32-unknown-unknown
```

### 2. バックエンドの起動

```bash
# JWT_SECRET を設定して起動（開発時は IPv4 ループバックを許可可能）
export JWT_SECRET="your_secure_secret"
export ALLOW_IPV4_LOCALHOST=1   # ローカル開発のみ
export ALLOW_MOCK_AUTH=1          # mock JPKI / ゼロ署名（本番では 0）
cargo run -p server
```

### 3. フロントエンドの起動

```bash
cd client
trunk serve   # http://127.0.0.1:8080
```

本番では **IPv6 のみ** を受け付けます（`docs/INFRA_NETWORK.md`）。IPv4 クライアントは API が 403 を返します。


### 4. テストの実行

```bash
# ワークスペース全体のユニット / 統合テスト
cargo test

# サーバーのみ
cargo test -p server

# 共通クレート（年齢検証・チェックサム・監査ログ）
cargo test -p common
```

## ⚖️ ディジタルフォレンジクス・ポリシー

本システムは、**NIST SP 800-86** および **ISO/IEC 27037** に準拠した証拠保全フローを実装しています。
- **Chain of Custody**: 投稿から閲覧、通報に至る全ての遷移が `content_audit_log` に記録されます。
- **Non-repudiation**: 秘密鍵による署名なしでは、いかなるステータス変更も不可能です。

---
Produced by Gemini CLI Agent.
