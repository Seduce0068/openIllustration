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
  at: "2026-11-17T06:45:00+09:00"
---

## 目的

RS256 JWT 署名鍵（`JWT_RSA_PRIVATE_KEY_PEM`）と検証鍵
（`JWT_RSA_VERIFICATION_KEYS`）を、発行済みトークンを無効化せずに
ローテーションする手順を定義する。秘密鍵の漏えい時対応と、
認証障害時の切り分けも含む。

## 前提

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

### 段階1: 旧公開鍵を検証用へ登録

1. 新署名鍵を生成する（Kubuntu / 作業ディレクトリ）:

```bash
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out new-signing-key.pem
openssl pkey -in new-signing-key.pem -pubout -out new-signing-key.pub.pem
```

1. 新旧の `kid` を求める:

```bash
for f in current-signing-key.pub.pem new-signing-key.pub.pem; do
  echo -n "$f: "
  openssl pkey -pubin -in "$f" -outform DER | sha256sum | cut -c1-32
done
```

1. 現行（旧）公開鍵を `JWT_RSA_VERIFICATION_KEYS` へ追加して再起動する。
   この時点では署名鍵は旧のままなので、発行トークンは変わらない:

```bash
export JWT_RSA_VERIFICATION_KEYS="$(jq -n --arg kid "$OLD_KID" --rawfile pem old-signing-key.pub.pem '{($kid): $pem}')"
```

1. `/health` と `/ready` が 200 であることを確認し、既存トークンで
   保護 API が通ることを確認する。

### 段階2: 署名鍵を切り替える

1. `JWT_RSA_PRIVATE_KEY_PEM` を新鍵へ差し替えて再起動する。
   旧公開鍵は `JWT_RSA_VERIFICATION_KEYS` に残したままにする:

```bash
export JWT_RSA_PRIVATE_KEY_PEM="$(cat new-signing-key.pem)"
```

1. 新しいトークンの `kid` が新鍵の値であることを確認する:

```bash
TOKEN="<新しく発行したアクセストークン>"
cut -d. -f1 <<<"$TOKEN" | tr '_-' '/+' | base64 -d 2>/dev/null | jq -r .kid
```

1. 旧 `kid` のトークンがまだ検証できることを確認する（無停下の証明）。

1. 発行済み旧トークンの有効期限が切れた後、`JWT_RSA_VERIFICATION_KEYS`
   から旧 `kid` を削除して再起動する。削除前に `/metrics` の
   `openillustration_http_errors_total` が増えていないことを確認する。

## 漏えい時対応

1. 漏えいした鍵をただちに `JWT_RSA_PRIVATE_KEY_PEM` から外し、新鍵へ切り替える。
2. `revoked_subs` テーブルへ `sub` 単位の失効閾値（`iat`）を記録し、
   漏えい時刻以前の全セッションを拒否する。再認証には新しい `iat` を発行する。
3. OIDC 側の `OIDC_CLIENT_SECRET` も漏えいの可能性がある場合は OP で再発行する。
4. 監査エクスポートの署名鍵も漏えい対象になり得る場合は
   [AUDIT_EXPORT_RUNBOOK.md](AUDIT_EXPORT_RUNBOOK.md) の検証手順で
   過去スナップショットの完全性を確認する。
5. インシデントの時刻・影響範囲・対応を `governance_audit_log` へ記録する
   （運用者による BAN/UNBAN 監査と同じ経路）。

## 認証障害時の切り分け

| 症状 | 確認点 | 対応 |
| --- | --- | --- |
| 全トークンが 401 | `JWT_RSA_PRIVATE_KEY_PEM` の PEM 形式とビット長（3072以上） | 起動ログの `JWT_RSA_PRIVATE_KEY_PEM is invalid` を確認し鍵を修正 |
| 旧トークンのみ 401 | `JWT_RSA_VERIFICATION_KEYS` に旧 `kid` が登録されているか | 段階1の手順で旧公開鍵を再登録 |
| 未知 `kid` のエラー | トークンの `kid` と登録済み `kid` の一致 | 署名鍵の導出 `kid` を再計算し登録 |
| `/auth/oidc/callback` が 401 | `OIDC_REQUIRED_ACR` / `OIDC_REQUIRED_AMR` / `OIDC_MAX_AUTH_AGE_SECS` と ID トークンのクレーム | 設定値を OP の実際のクレームに合わせる（本番では設定必須） |
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
