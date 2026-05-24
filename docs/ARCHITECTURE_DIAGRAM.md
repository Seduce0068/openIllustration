# システム構成図 (System Architecture Diagram)

本ドキュメントは、各コンポーネント間の論理的な連携とデータフローをテキスト形式で可視化したものである。

## 1. 論理階層図

```text
[ ユーザー層 ]
    │ (WebAuthn/OID4VP)
    ▼
[ プレゼンテーション層 (SolidJS / Rust-Wasm) ]
    │ (HTTPS / IPFS CID)
    ▼
[ アプリケーション層 (Rust / Axum / Cloudflare Workers) ]
    │  │ (クエリ / 署名ログ)
    ▼  ▼
[ データ層 (Turso / libSQL) ]  [ ガバナンス層 (Smart Contract) ]
    │  │ (静的コンテンツ)
    ▼  ▼
[ 分散ストレージ層 (IPFS / Arweave) ]
```

## 2. データフロー詳細

1. **認証**: [ユーザー] --(JPKI OID4VP)--> [Rust Backend] --(認証検証)--> [Turso:User Table]
2. **投稿**: [ユーザー] --(IPFS CID)--> [Rust Backend] --(Hash検証)--> [Turso:Content Table]
3. **閲覧**: [ブラウザ:Rust-Wasm] --(CID照合)--> [IPFS]
4. **モデレーション**: [陪審員] --(投票)--> [Smart Contract] --(執行)--> [Rust Backend/Turso]

## 3. セキュリティ境界

* **公開不可領域**: JPKI署名キー（カード内蔵）、Turso接続情報（シークレット）
* **公開領域**: コンテンツ（IPFS CID）、ガバナンス履歴（ブロックチェーン）、MIT/AGPL ソースコード

## 4. コンポーネント役割定義

* **Rust Backend (Axum/Workers)**: メモリ安全性を担保し、認証・ガバナンス・IPFSアドレス管理の要となる。
* **Turso (libSQL)**: ユーザーハッシュとコンテンツのメタデータを保持する分散型 SQLite。
* **SolidJS フロントエンド**: ユーザー操作を Wasm モジュールに委譲し、真正性検証を行う。
* **IPFS / Arweave**: コンテンツの実体を CID ベースで不変的に保持・分散配信する。
* **Smart Contract**: 陪審員の投票およびモデレーション判断のログを不可逆的に記録する。
