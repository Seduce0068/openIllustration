# インフラ定義とネットワーク制約 (Infrastructure & Network Policy)

本ドキュメントは、ネットワークアクセス要件およびインフラ構成における厳格な制約を定義する。

## 1. ネットワーク要件 (IPv6-Only Policy)
本システムは、現代的なネットワークプロトコルである **IPv6 限定 (IPv6-Only)** で運用する。

* **接続性要件**: ユーザーおよびサーバー間通信は全て IPv6 に限定される。IPv4 のみを利用するクライアントからの接続は、ネットワーク層で拒否する。
* **採用理由**:
    * **攻撃対象領域の削減**: IPv4 ターゲットのレガシーボットネットを物理的に遮断する。
    * **ゼロトラストの徹底**: ネットワーク環境においても現代的なプロトコルを強要することで、ギルドの参加要件を技術的にも高める。
    * **アドレス空間の完全性**: ユーザーごとのプレフィックス単位での管理を容易にし、共有 IP リスクを排除する。

## 2. インフラ定義 (OpenTofu)
Cloudflare エッジ設定にて IPv6 接続のみを許可するポリシーを強制する。

```hcl
# Cloudflare Zone 設定による IPv6 強制
resource "cloudflare_zone_settings_override" "guild_settings" {
  zone_id = var.cloudflare_zone_id
  settings {
    # IPv6 の有効化
    ipv6 = "on"
    
    # 悪意のある IPv4 トラフィックを遮断するための WAF ルールを付与
    # (※実際の実装では cloudflare_ruleset を使用して IPv4 接続を Reject する)
  }
}
```

## 3. ユーザーへの告知 (README / Landing Page)
サイト入場ゲート（Legal Wall）において、以下の警告を明示する。
> **"IPv6 Only Access Requirement: This guild requires an IPv6-enabled network connection. Legacy IPv4-only networks are not supported. This is a deliberate architectural decision to ensure modern networking security and to filter out automated legacy traffic."**
> （IPv6 限定アクセス要件: 本ギルドは IPv6 対応ネットワーク接続を必要とします。レガシーな IPv4 専用ネットワークはサポートされません。これはモダンなネットワークセキュリティを確保し、自動化されたレガシー通信を除外するための意図的な設計判断です。）

## 4. 真正性への影響
IPv6 への限定は、IP アドレスベースの不正アクセス判定をより高精度にする。従来の IPv4 NAT 環境下での「同一 IP による攻撃」という懸念を排除し、マイナンバーカードハッシュと IPv6 プレフィックスの相関を強めることで、BAN の精度を最大化する。
