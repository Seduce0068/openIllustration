---
type: Playbook
title: "署名鍵ローテーション・障害対応 Runbook"
description: "RS256 JWT 署名鍵と OIDC 検証鍵の無停止ローテーション、漏えい時対応、障害時の切り分け手順。"
tags:
  - runbook
  - key-rotation
  - security
  - operations
generated:
  by: dsh
  at: "2026-11-23T03:35:00+09:00"
---

## 目的

RS256 JWT 署名鍵（`JWT_RSA_PRIVATE_KEY_PEM`）と検証鍵
（`JWT_RSA_VERIFICATION_KEYS`）を、発行済みトークンを無効化せずに
ローテーションする手順を定義する。秘密鍵の漏えい時対応と、
認証障害時の切り分けも含む。

## 前提

- RSA 署名鍵は 2048bit 以上を要求する。未満の PEM は起動時に拒否される
  （`server/src/jwt.rs` の `RSA_MIN_KEY_BITS`）。
- ローテーション用の旧検証鍵（`JWT_RSA_VERIFICATION_KEYS` に登録する旧公開鍵）にも
  同じ RSA 2048bit 下限を適用する。1024bit などの弱鍵は
  `verification key must be at least 2048 bits` で起動時に拒否される
  （`server/src/jwt.rs` の `add_verification_key`）。
- `kid` は SPKI DER の SHA-256 先頭16バイトを hex エンコードした値で、
  公開鍵から決定的に導出される（`server/src/jwt.rs`）。
- 現行署名鍵の `kid` は自動的に検証鍵へ登録される。
- `JWT_RSA_VERIFICATION_KEYS` は `{"<kid>":"<SPKI 公開鍵 PEM>", ...}`
  形式の JSON（`-----BEGIN PUBLIC KEY-----`。`server/src/jwt.rs` の
  `PublicKey::from_public_key_pem` が要求する形式）。署名鍵と同じ `kid` は
  登録できない（起動時に拒否）。
- 未知の `kid` は拒否される（fail-closed）。`kid` 無しの旧トークンは
  現行署名鍵で検証される。
- HSM を用いる場合は、以下の「秘密鍵ファイルを生成する」手順を
  HSM の鍵生成・署名インターフェースへ置き換える。設定へ渡す値は
  PKCS#8 PEM である必要がある。

## 無停止ローテーション（2段階）

### 段階1: 新署名鍵の生成と kid 確認（旧公開鍵の登録不要）

1. 新署名鍵を生成する（Kubuntu / 作業ディレクトリ）:

```bash
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out new-signing-key.pem
openssl pkey -in new-signing-key.pem -pubout -out new-signing-key.pub.pem
```

1. 新旧の `kid` を求める:

```bash
for f in current-signing-key.pub.pem new-signing-key.pub.pem; do
  echo -n "$f: "
  openssl pkey -pubin -in "$f" -outform DER | sha256sum | cut -c1-32
done
```

1. 旧公開鍵の `JWT_RSA_VERIFICATION_KEYS` への登録は**不要**。
   現行署名鍵の kid は `JwtKeys` 生成時に `verification_keys` へ自動登録されており、
   同じ kid を手動登録すると `duplicate verification key kid` で起動拒否になる。
   この時点では署名鍵は旧のままなので、発行トークンは変わらない。
   旧公開鍵が RSA 2048bit 以上であることは事前に確認する（未満は署名鍵側で起動拒否）。
   新公開鍵の kid は署名鍵差し替え時に自動登録されるため設定しない。
   `JWT_RSA_VERIFICATION_KEYS` は段階2で旧公開鍵（旧 kid）を追加するときに設定する:

1. 段階1では設定変更もサービス再起動も行わない。この時点で `server.env` を
   更新せず、既存トークンで保護 API が通ることの確認のみを行う。
   `server.env` の更新は段階2で署名鍵を差し替えるときに行う（末尾の
   「systemd 配布の実注入経路」を参照）。

1. `/health` と `/ready` が 200 であることを確認し、既存トークンで
   保護 API が通ることを確認する。XFF fail-closed 下では信頼済みプロキシ経由、
   または `TRUSTED_PROXY_CIDRS` に合致する XFF 付与（値は非秘密）でアクセスする。
   信頼範囲外からの 403 は設定不具合ではなくゲートの正常動作である。

### 段階2: 署名鍵を切り替える

1. `JWT_RSA_PRIVATE_KEY_PEM` を新鍵へ差し替えて再起動する（systemd 配置では
   下記のとおり `server.env` を更新する）。旧公開鍵は
   `JWT_RSA_VERIFICATION_KEYS` に残したままにする:

```bash
export JWT_RSA_PRIVATE_KEY_PEM="$(cat new-signing-key.pem)"
```

   注: 上記 `export` 例は**開発用（対話シェル専用）**であり、本番の systemd
   サービスへは反映されない。実注入経路は `deploy/openillustration.service:25-30` の
   `EnvironmentFile=/etc/openillustration/server.env`（`Environment=` より
   後勝ち）である。段階2では末尾の「systemd 配布の実注入経路」の順序で
   `server.env` 更新 → doctor --strict → `systemctl restart` →
   `/health` / `/ready` 確認を行うこと。再起動後に doctor を回す逆順は不可。

1. 新しいトークンの `kid` が新鍵の値であることを確認する:

```bash
TOKEN="<新しく発行したアクセストークン>"
cut -d. -f1 <<<"$TOKEN" | tr '_-' '/+' | base64 -d 2>/dev/null | jq -r .kid
```

1. 旧 `kid` で署名した事前トークンと新 `kid` で署名した事後トークンを採取し、
   両方で保護 API が通ることを確認する。結果は時刻付きで外部の監視基盤・
   署名付きログなどの外部アンカーへ記録し、再起動で消えない証跡として残す。

1. 発行済み旧トークンの有効期限が切れた後、`JWT_RSA_VERIFICATION_KEYS`
   から旧 `kid` を削除して再起動する。削除前に外部アンカーの認証エラー
   時系列が増えていないことを確認する。`/metrics` のカウンタはプロセス内で
   再起動時にゼロへ戻るため、単独の無停止証明には使わない。

   旧 `kid` 削除後の再起動も末尾の「systemd 配布の実注入経路」に従い、
   `server.env` 更新 → doctor --strict → `systemctl restart` →
   `/health` / `/ready` 確認の順で適用する。外部アンカーの確認と
   `/metrics` カウンタが再起動でゼロへ戻る点は上記のとおり維持する。

## 漏えい時対応

1. 漏えいした鍵をただちに `JWT_RSA_PRIVATE_KEY_PEM` から外し、新鍵へ切り替える。
   実注入経路は `server.env` なので、末尾の「systemd 配布の実注入経路」の順で
   `server.env` の該当行更新 → doctor --strict → `systemctl restart` →
   `/health` / `/ready` の 200 確認 → 旧 `kid` 削除 → 再起動・再検証を行う。
   再起動後に doctor を回す逆順は不可。
2. `revoked_subs` テーブルへ `sub` 単位の失効閾値（`iat`）を記録し、
   漏えい時刻以前の全セッションを拒否する。再認証には新しい `iat` を発行する。
3. OIDC 側の `OIDC_CLIENT_SECRET` も漏えいの可能性がある場合は OP で再発行する。
   再発行値は `/etc/openillustration/server.env` の `OIDC_CLIENT_SECRET` 行へも
   反映し、上記と同じ `server.env` 更新 → doctor --strict → `systemctl restart`
   → `/health` / `/ready` 確認の順で適用する。対話シェルの `export` では
   systemd サービス環境へ届かない。
4. 監査エクスポートの署名鍵も漏えい対象になり得る場合は
   [AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md) の検証手順で
   過去スナップショットの完全性を確認する。
5. インシデントの時刻・影響範囲・対応を `governance_audit_log` へ記録する
   （運用者による BAN/UNBAN 監査と同じ経路）。

## 認証障害時の切り分け

| 症状 | 確認点 | 対応 |
| --- | --- | --- |
| 全トークンが 401 | `JWT_RSA_PRIVATE_KEY_PEM` の PEM 形式とビット長（2048以上） | 起動ログの `JWT_RSA_PRIVATE_KEY_PEM is invalid` を確認し鍵を修正 |
| 旧トークンのみ 401 | `JWT_RSA_VERIFICATION_KEYS` に旧 `kid` が登録されているか | 段階2の手順で旧 kid の公開鍵が登録されていることを確認 |
| 未知 `kid` のエラー | トークンの `kid` と登録済み `kid` の一致 | 署名鍵の導出 `kid` を再計算し登録 |
| `/auth/oidc/callback` が 401 | `OIDC_REQUIRED_ACR` / `OIDC_REQUIRED_AMR` / `OIDC_MAX_AUTH_AGE_SECS` と ID トークンのクレーム | OP が要求保証を提供しない場合は本番有効化せず、OP 側のクレーム提供を確認する。設定を下げて回避しない（fail-closed 方針、[DOCUMENT_STATUS](DOCUMENT_STATUS.md) 参照） |
| 再起動後にスケジュール障害 | バックアップは `ALERT_CMD` が通知しているか | [BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md) を参照 |

## 検証済み事項

- `kid` は公開鍵から決定的に導出され、署名鍵と同じ `kid` の登録は起動時に拒否される
  （`server/src/jwt.rs` のユニットテスト）。
- 旧公開鍵によるローテーション検証、未知 `kid` 拒否、`kid` 無し旧トークン互換は
  `server/src/tests.rs` の回帰テストで確認済み。
- 障害時ログは `LOG_FORMAT=json` で構造化出力され、
  `GET /metrics` のエラーカウンタで検知できる。

## 未解決事項（外部依存）

- HSM / 外部鍵管理サービスへの接続と鍵の非导出運用。
- 複数プロセス・サービス間の公開鍵配布の自動化（現状は環境変数で配布）。
- 監査署名鍵の外部アンカーと公開範囲の最終判断。

## systemd 配布の実注入経路（段階2・漏えい時共通）

`export JWT_RSA_PRIVATE_KEY_PEM=...` は対話シェル限定の環境変数であり、
systemd サービスの環境へは反映されない。実注入経路は
`deploy/openillustration.service:25-30` の
`EnvironmentFile=/etc/openillustration/server.env` である。`Environment=` より
`EnvironmentFile` が後勝ちのため、秘密値は `server.env` 側で管理する。

### 適用順序（逆順不可）

1. `/etc/openillustration/server.env` の該当行のみを更新する。
   `MIGRATIONS_DIR` など他行は保持し、所有者 `openillustration:openillustration`、
   権限 `0600` を維持する（`deploy/openillustration.service:26-27`）。
   秘密値そのものを本文やログへ書き出さないこと。

   ```bash
   sudo install -o openillustration -g openillustration -m 0600 \
     /etc/openillustration/server.env /etc/openillustration/server.env.new
   # server.env.new の該当行のみを新しい値へ差し替える（値はここへ記載しない）
   sudo stat -c '%U:%G %a' /etc/openillustration/server.env.new
   sudo mv -f /etc/openillustration/server.env.new /etc/openillustration/server.env
   ```

   `stat` の期待値は `openillustration:openillustration 0600`。権限・所有者が
   異なる場合は再起動前に修正する。

1. 本番相当の環境変数を注入して doctor を実行する。`server.env` は 0600 なので
   読み取り権限（`sudo` 等）が必要。doctor が失敗した場合は再起動せず、
   `server.env` の差し替えを見直す。CI の定型
   `( source scripts/doctor-ci-env.sh && cargo run -p server --bin doctor
   -- --strict )` は CI 用ダミー設定であり実秘密を source しないため、
   本番相当の検証では CI 用スクリプトではなく `server.env` を注入する。

   ```bash
   # 0600 を読める権限で実行する。呼び出し元シェルの開発用フラグが残ると
   # doctor が本番チェックをスキップして false pass するため明示的に unset する。
   # server.env は systemd 固有構文を含まない単純な KEY=value 行である前提。
   ( unset ALLOW_MOCK_AUTH ALLOW_IPV4_LOCALHOST EXPOSE_INTERNAL_ERRORS \
     ALLOW_UNENCRYPTED_BACKUP
   set -a; . /etc/openillustration/server.env; set +a; \
     cargo run -p server --bin doctor -- --strict )
   ```

1. doctor 成功後にサービスを再起動する。

   ```bash
   sudo systemctl restart openillustration.service
   ```

1. `/health` と `/ready` が 200 であることを手動確認する。信頼済みプロキシ経由、
   または `TRUSTED_PROXY_CIDRS` に合致する XFF 付与が必要。信頼範囲外からの
   403 は設定不具合ではなくゲートの正常動作である。

   ```bash
   curl -fsS -o /dev/null -w '%{http_code}\n' http://<APP_HOST>/health
   curl -fsS -o /dev/null -w '%{http_code}\n' http://<APP_HOST>/ready
   ```

   旧 `kid` の削除を伴う場合は、外部アンカーの認証エラー時系列を確認したうえで
   同じ順序でもう一度適用し、再起動後は新 `kid` トークンのみが通り、旧 `kid`
   トークンは `unknown JWT kid` で拒否されることを再検証する。旧・新双方が通る
   確認は削除前の段階2の手順であり、削除後に適用しない。

### 更新履歴

- 2026-11-28T19:59:20+09:00: 段階2・漏えい時対応を systemd の
  `EnvironmentFile` 実注入経路へ結線（段階1は設定変更・再起動不要の旨を明記）。
