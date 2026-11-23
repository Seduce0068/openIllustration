---
type: Architecture Decision Record
title: "ADR-0013: 保留事項の推奨方針と実装優先順位"
description: "これまで設計判断待ちとして記録してきた項目に対し、プロジェクト側で決定可能な推奨方針を与え、実装順を定義する。"
tags: [adr, decisions, backlog, proposed]
generated:
  by: dsh
  at: "2026-11-23T14:45:00+09:00"
---

## ADR-0013: 保留事項の推奨方針と実装優先順位

- Status: Proposed
- Date: 2026-11-23
- Decision owners: Project owner（ユーザーより推奨方針の決定を委譲）
- Scope: `todo.md` のメモ欄に「設計判断待ち」として蓄積された項目

## Context

56 ラウンドにわたるクロスレビューで多数の改善点が検出され、うち外部仕様・法務・実機
検証を要するものと、プロジェクト内で決定可能なものが混在して「設計判断待ち」として
記録されてきた。ユーザーより「設計判断待ちの箇所は推奨するものを決めて良い」と委譲されたため、
本 ADR で方針を確定し、実装可能な項目から順に消化する。

## Decision

### A. 外部输入端に依存するため「方針のみ決定・実装は保留」

| 項目 | 推奨方針 |
| --- | --- |
| 本人確認監査（JPKI 信頼アンカー／chain／revocation） | 本プロトタイプでは JPKI 実証環境を用いず、信頼アンカーは設定注入・chain 検証は OP 委譲・revocation は ID トークン有効期限で代替する。実装は verified_age adapter の範疇に留める。 |
| OIDC `azp` 検証 | 単一クライアント運用を前提とし、`aud` 検証で代替する。複数クライアント化時に `azp` を追加する。 |
| 第三者監査アンカー／公開 ledger 意図 | 監査スナップショットの外部アンカーは「運用者が別経路で保管する署名付き JSON」と定義し、公開 ledger は本プロトタイプの範囲外とする。 |
| JPYC 手数料／返金／dispute | 決済は送金検証のみを実装し、返金・dispute は注文ステータス `MISMATCH` と運用者判断に委ねる。手数料は受取額に含めない。 |
| 脅威モデル | 詳細モデルは作成せず、既存の ADR-0001 信頼境界と `docs/OPERATIONAL_POLICY.md` の fail-closed 契約をもって代用する。 |
| 通知／再審査／UNPIN 手順 | 運用 Runbook に「運用者が `MODERATION_OPERATOR_HASHES` で識別し、`governance_audit_log` に記録する」旨を明記済みとし、自動通知は ALERT_CMD に一元化する。 |
| 機能検出代替 | 機能フラグは `option_env!` のビルド時確定を維持し、実行時機能検出は導入しない。 |

### B. プロジェクト内で決定し実装する（優先順位付き）

#### P1: 設定・運用の単一情報源（セキュリティ／本番安全性）

| 項目 | 推奨方針 |
| --- | --- |
| 起動設定一括 Config 化 | `RuntimeConfig::from_env(lookup)` を新設し、`run()` と `doctor` が同じ構造体と判定を参照する。段階的に移行する。 |
| BIND_ADDR / PORT | `BIND_ADDR`（既定 `[::]`）と `PORT`（既定 `3000`）を追加し、不正値は起動拒否。`doctor` と `.env.example` に反映。 |
| 数値 env 範囲外警告 | 正規化後の値と入力が一致しない場合に固定文言警告（本番は起動拒否）を出し、`doctor` と共有判定する。 |
| RUST_LOG 不正値警告 | `common::env::read_env` で分類し、NonUtf8／不正フィルタは固定文言警告へ。 |
| PINATA_JWT 起動時確定 | `AppState` に解決済み値を保持し、リクエスト毎の `read_env` を廃止。 |
| migrations dir override | `MIGRATIONS_DIR` を追加し、絶対パス・存在・連番を fail-closed 検証。`CARGO_MANIFEST_DIR` は開発フォールバックとして残す。 |
| アプリ systemd unit テンプレート | `deploy/openillustration.service` を追加し、WorkingDirectory／EnvironmentFile／UMask／再起動方針／migrations パスを明記。 |
| alert unit テンプレート | `deploy/openillustration-backup-alert.service.example` を追加し、OnFailure の差し替えを配置必須ステップ化。 |
| NTP 同期 | Runbook に「ホスト時刻同期はデプロイ必須前提」と明記し、doctor の production カテゴリへ時刻偏移量の警告を追加（閾値は設定可能）。 |
| HSTS | TLS 終端プロキシが責任を持つと決定し、Runbook／OPERATIONAL_POLICY に必須設定として明記。アプリ側は付与しない。 |

#### P2: バックアップ／復旧の完全性

| 項目 | 推奨方針 |
| --- | --- |
| RPO/RTO 定義 | RPO=24 時間、RTO=4 時間と定め、`BACKUP_RESTORE_RUNBOOK` と `DEC-07` に記載。 |
| バックアップ鮮度監視／ハートビート | 成功時にタイムスタンプファイルを公開し、想定窓（03:17 + RandomizedDelaySec + 1 日）超過で ALERT_CMD を発火する監視を追加。 |
| cron タイムアウト／flock | cron ジョブ行を `timeout 1800` + `flock -n` で包み、重複実行とハングを防止。 |
| オフサイト 2回コピーの世代対 | 監査 root を先に送り、成果物を後に送る。2回目失敗時に片方だけ残る場合は固定文言で警告し、手動_cleanup 手順を Runbook に記載。 |
| GPG 署名 | 暗号文に加えて detached 署名を生成し、復元前に検証を必須ゲート化。署名鍵は監査鍵と同等に管理。 |
| GPG trust 手順 | 本番公開鍵は `gpg --edit-key ... trust` で ultimately trusted に設定する手順を Runbook に明記（`--trust-model always` は使わない）。 |
| audit root 64hex 検証 | `is_hex_merkle_root` を 64 桁固定へ厳密化。既存の 16hex テストはフィクスチャを更新して移行する。 |
| アーカイブ寿命 | E2E 失敗アーカイブは 7 世代保持とし、成功時に古いものを削除する。 |
| export ファイル名世代 | `/me/export` のファイル名へ `generated_at` 由来の UTC タイムスタンプを付与する。 |
| 署名公開 rename TOCTOU | hard_link のみを要求し、rename フォールバックを廃止（クロスデバイスは明示エラー）。 |
| DB シンボリックパス | 本番設定で DB パスの symlink を拒否し、`canonicalize` 後にロックファイル名を導出。 |

#### P3: テスト／CI の信頼性

| 項目 | 推奨方針 |
| --- | --- |
| axe 自動スキャン | `@axe-core/playwright` を devDependency へ追加し、主要 5 画面で wcag2a/wcag2aa を既存ケース内の step として実行（E2E69 件維持）。 |
| E2E artifact 公開 | 独立ジョブへ昇格させ、失敗時に `actions/upload-artifact` で trace／screenshot／error-context を公開。 |
| E2E セットアップ `::error::` | `report_e2e_failure` を ERR trap 化し、セットアップ失敗にも段階名付きアノテーションを付与。 |
| E2E ビルド `::error::` | trunk build／型検査／外部アセットゲートの各失敗点へ段階名付き `::error::` を追加。 |
| systemd/cron 構文 CI ゲート | `systemd-analyze verify` と cron フィールド検査を extra_gates へ追加。 |
| CI バイナリキャッシュ | cargo-deny／betterleaks をバージョン＋checksum 付きでキャッシュし、ヒット時も `--version` を検証。 |
| go ツールチェーン前提 | `command -v go` を事前検査し、無い場合は固定版バイナリを直接ダウンロード。 |
| playwright install 再試行 | 有限回数（3 回）のリトライとバックオフを追加。 |
| モバイル viewport | オプトインのモバイルプロジェクトを追加し、主要画面のスモークを既存ケース内へ統合。 |
| Chrome 上限固定 | `MAX_CHROME_MAJOR` を CI で明示し、下限（`MIN_CHROME_MAJOR=120`）とともに検証。 |
| build.rs rerun-if-env-changed | `OIL_API_BASE`／`OIL_MOCK_AUTH`／`OIL_CLIENT_METRICS` を監視し、環境変更時の再ビルドを保証。 |
| real-api 宛先 env 化 | `E2E_SERVER_PORT`／`E2E_API_BASE` を追加し、3000 ハードコードを解消。 |
| ライセンス本文数照合 | `prepare-distribution.sh` の生成件数を実測し、`DOCUMENT_STATUS` と照合するゲートを追加。 |
| CLI-02 群テーブル欠落 | 欠落 3 件を追記し、doc-numbers の群テーブル照合を CLI-02/03 へ拡張。 |
| 数値 env／RUST_LOG 以外のテスト | verify_trunk_archive 入力検証、step summary 異常系、assert_major_numeric 境界、hash/view ルーティング単体、CLI NULL 列を追加。 |

#### P4: ランタイム保護／UX

| 項目 | 推奨方針 |
| --- | --- |
| リクエストタイムアウト／TimeoutLayer | tower-http の `timeout` feature を有効化し、監視パスを除外した `RequestBodyTimeoutLayer` を追加。タイムアウト秒数は設定化。 |
| tokio／tower features | 使用 feature のみへ絞り、resolver 3 と合わせてバイナリサイズを削減。 |
| reqwest TLS 二重スタック | `default-features = false` + `rustls-tls` へ統一し、native-tls 系 skip を削減。 |
| workspace 依存集約 | `[workspace.dependencies]` へ共通依存を集約し、各クレートは `workspace = true`。 |
| common RSA 推移依存 | `common` を feature 分割し、`client` は署名検証スタックを依存しない。 |
| 監査スクロール／non-modal Tab | non-modal を維持し、Tab の prevent_default を外して背景へ移動可能にする。 |
| Support overlay tabindex | 支援無効時は `tabindex` を外し、`aria-disabled` と説明テキストを付与。 |
| 再試行導線（Dashboard／Ledger／監査／投票一覧） | 共通の「再読み込み」ボタンと dedicated role=status を追加。429 専用文言も併せて追加。 |
| 投票一覧 401/403 | 401 は再ログイン案内、403 は「運用者のみ」の中立文言へ分岐。 |
| ログイン 403 IPv4 案内 | `/auth/login` の 403 に IPv6 要件の専用固定文言を追加。 |
| クライアント例外テレメトリ／refresh 失敗メトリクス | `event` 名と `count` のみを送る別イベントを追加。URL・IP・User-Agent は送らない既存契約を維持。 |
| console.error／pageerror 監視 | グローバル setup で収集し、許可リスト外で失敗させる（既存の履歴警告は許可リストへ）。 |
| ブラウザエラー観測 | 同上のテレメトリで代替し、別途サーバー側カウンタは追加しない。 |
| metrics Cookie | `metrics.spec.ts` へ `cookie` ヘッダ非含有をアサーション追加。 |
| 日時 locale／鮮度／端末時計 | UTC 固定 + 「(協定世界時)」表記を維持し、`lang="ja"` を明示。鮮度判定はサーバー時刻をレスポンスで渡す方式へ移行。 |
| 日時フォールバック | 不正 ISO は生文字列を返す既存契約を維持し、E2E でオラクル追加。 |
| JXL アニメーション／MAX_PIXELS | アニメーションは先頭フレーム固定を仕様化し、`MAX_PIXELS` を `MAX_DIMENSION²` より小さく設定して面積ガードを実効化。 |
| 価格入力指数表記 | 貼り付けは受理するが対話入力の中間値は signal を更新しない現状を維持し、コメントを実挙動へ修正。 |
| export sub 8 文字以下 | 8 文字以下はそのまま返す既存契約を仕様へ明記し、単体テスト追加。 |
| Blob／URL 生成エラー | 各固定文言を純粋な分類関数へ集約し、単体テスト追加。 |
| 開発モック user_hash 衝突 | mock 経路でも `public_key_der` を必須化し、birth_date/country を主体識別子に使わない。 |
| 失効閾値削除失敗警告格下げ | 旧閾値を残したまま警告＋メトリクス記録へ格下げし、VerifyResponse を返す。 |
| logout sub 不一致 Cookie 削除 | 不一致分岐でも削除 Cookie を付与してから 401 を返す。 |
| finalizer BEGIN 失敗計上 | 残件数をスキップ件数として `finalizer_failures` へ加算。 |
| refresh sub 失効競合 | 失効チェックを write_lock 後の tx 内へ移動し、トークン消費と同一トランザクション化。 |
| back-channel logout 部分失敗再送 | `changed==0` 分岐でも未使用リフレッシュトークンの消費を実行。 |
| mod_report／mod_vote rows drop | 1 行読取後に明示 `drop(rows)` し、他ハンドラと read-then-write 順序を統一。 |
| DB_WRITE_LOCKS ポインタキー | ロック識別子を DB パスへ変更し、接続破棄時にエントリを回収。 |
| 署名公開 rename TOCTOU | P2 と同じく hard_link のみ。 |

### C. 意図的に実装しない（方針として拒否）

| 項目 | 理由 |
| --- | --- |
| E2E 本番相当昇格 | モック認証ゲートと本番設定診断（doctor --strict）を分離する現在の二層構成を維持し、E2E を本番化しない。 |
| 実行時機能検出 | ビルド時確定の再現性を優先し、採用しない。 |
| `deny.toml` copyleft 明示拒否 | 自ワークスペースが AGPL-3.0-or-later であるため、方針コメントを「AGPL 系は workspace ライセンス上許容」へ修正し、外部 copyleft は個別審査とする。 |
| IPFS チェックサム不一致時フェイルオーバー | 改ざん検知を fail-closed で即拒否する要件を優先し、不一致時に他ゲートウェイへ進まない現状を維持（コードコメントへ明記）。 |

## Consequences

- 外部输入端に依存する項目は「方針のみ」とし、実装を再度保留する。
- P1〜P4 は以降のラウンドで順次実装し、Deterministic Gates を通過したコミットとして反映出す。
- 方針変更時は本 ADR の Status を Revised にし、`docs/adr/log.md` へ追記する。
