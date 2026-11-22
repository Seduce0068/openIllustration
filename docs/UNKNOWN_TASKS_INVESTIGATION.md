---
type: Reference
title: "未知の残タスク・機能追加案 調査レポート"
description: >-
  エージェントチーム探索で抽出した TASK_INVENTORY 未記載タスク
  （2026-11-18時点の履歴、E2E 当時9/35件・11 spec→現在69件/12 spec）。
  現在値は TEST_SPECIFICATION を参照。
tags:
  - tasks
  - backlog
  - investigation
  - agent-team
generated:
  by: dsh (agent team)
  at: "2026-11-22T10:30:00+09:00"
verified:
  by: dsh
  at: "2026-11-22T10:30:00+09:00"
status: historical
---

## 目的と方法

既存の [TASK_INVENTORY.md](TASK_INVENTORY.md)（2026-11-15 時点）は既知の残タスクを網羅している。
本調査はその**外側**にある未知の項目を、独立したエージェントチームで5ラウンド探索し、
6ラウンド目で重複排除・妥当性評価を行って統合する。

- ベースライン: `master` / `672a61d`（2026-11-17 確認、TASK_INVENTORY 記載の `3662145` より進行）
- 既知項目の扱い: TASK_INVENTORY に記載のある項目（対応済み・未対応を問わず）は原則として報告対象外。
  ただし**別の根本原因・別のエンドポイント・別の設定キー**によるものは未知として扱う。
- 安全第一: 外部仕様の推測は断定しない。秘密情報は扱わない。読み取り専用の調査。
- エージェント: 13体を起動（r1-auth / r1-payment / r1-client / r1-infra、r2-requirements /
  r2-design / r2-handoff、r3-server-tests / r3-e2e-gap、r4-production、r5-features、
  失敗2件の再起動 r1b-payment / r2b-design）。全て完了し、失敗2件も再起動で回収した。

## ラウンド進捗

| ラウンド | 観点 | 状態 | 生finding数 |
| --- | --- | --- | --- |
| 1 | コードベース監査（認証/決済/クライアント/自動化の4並列） | 完了 | 27 |
| 2 | 要件・設計ドキュメントと実装のギャップ | 完了 | 21 |
| 3 | テスト・検証カバレッジのギャップ | 完了 | 16 |
| 4 | 運用・本番化・CI/CD の残課題 | 完了 | 8 |
| 5 | 機能追加案・改善提案の発掘 | 完了 | 10 |
| 6 | 重複排除・妥当性評価・統合 | 完了 | 82 → 下記へ集約 |

## Round 1: コードベース監査

### r1-infra: 自動化基盤・CI/E2E/スクリプト

| # | finding | 重要度 | 根拠 | 信頼度 |
| --- | --- | --- | --- | --- |
| R1-01 | trunk キャッシュがヒット時に checksum を再検証せず fail-open で実行される | 中 | `e2e/ci-e2e.sh:21-35` — キャッシュヒット時に `sha256sum -c` を通さず PATH へ追加 | 0.85 |
| R1-02 | E2E がほぼ全テストで API をモックし、実サーバーとの結合が未検証 | 中 | `e2e/tests/economy.spec.ts:17-83` 他 — 実 API を通すのは `auth.spec.ts:4-18` のみ | 0.80 |
| R1-03 | バックアップのローテーション本体（保持期間・MIN_KEEP 削除）が自動テスト未カバー | 中 | `scripts/test-backup-rotate.sh:55-133`、`scripts/backup-rotate.sh:116-133` | 0.90 |
| R1-04 | clippy の警告ポリシーがローカルと CI で逆方向に差分 | 中 | `.github/workflows/ci.yml:31` vs `AGENTS.md:47` | 0.70 |
| R1-05 | cron 例の `MAILTO` がプレースホルダ `admin@example.com` のまま | 低 | `deploy/openillustration-backup.cron:5` | 0.75 |
| R1-06 | markdownlint ゲートが `docs/` 配下のみでルート直下 Markdown を検査しない | 低 | `.github/workflows/ci.yml:34` の glob が `"docs/**/*.md"` | 0.80 |
| R1-07 | `test-backup-rotate.sh` のテスト名が実際の検証内容と一致しない | 低 | `scripts/test-backup-rotate.sh:83` | 0.95 |
| R1-08 | Playwright の並列度が CI（workers=1）とローカル（CPU数）で異なる | 低 | `e2e/playwright.config.ts:31-34` | 0.65 |

**未確認（r1-infra）**: 共通ワークフロー `Seduce0068/workflows/rust-gates.yml@v1` はリポジトリ外のため
`extra_gates` の fail-open リスクは未検証。Rust 側とのレスポンススキーマ不一致も未検証。

### r1-client: クライアント / WASM / UI

| # | finding | 重要度 | 根拠 | 信頼度 |
| --- | --- | --- | --- | --- |
| R1-09 | アップロード UI が file パートに MIME を設定せず、実サーバーでは全アップロードが 400 になる | **高** | `client/src/lib.rs:519-525` の空 `Blob` vs `server/src/lib.rs:2487-2500` の許可リスト | 0.90 |
| R1-10 | `index.html` が実行時 CDN スクリプトと外部フォントを無 SRI で読み込む | **高** | `client/index.html:7,9`、`client/dist/index.html` も同様 | 0.95 |
| R1-11 | コンテンツ取得の HTTP ステータスを確認せず 403/500 を「完全性侵害」と誤表示 | 中 | `client/src/lib.rs:345-353`（ダッシュボード側 `:275-282` は検査している） | 0.90 |
| R1-12 | API エンドポイントが `http://localhost:3000` にハードコードされ mixed content | 中 | `client/src/lib.rs:8` の `API_BASE`、`client/Trunk.toml` に差し替え手段が無い | 0.95 |
| R1-13 | JXL デコード結果を破棄し、失敗時も VERIFIED の空キャンバスになる | 中 | `client/src/lib.rs:355-361,457-466`、`client/src/jxl.rs:81-136` | 0.85 |
| R1-14 | アップロード処理内の `unwrap` 連鎖で失敗時に「Processing...」のまま固まる（**対応済み 2026-11-18**: `5d9f5bd` で `unwrap` 12個を `Result` 伝播へ置換し、成否にかかわらず loading を解除してエラー表示） | 中 | `client/src/lib.rs:511-539`、`:552` に到達しない | 0.90 |
| R1-15 | ログイン失敗が完全に無音で、拒否理由がユーザーに伝わらない | 中 | `client/src/lib.rs:231-238`、エラー signal 領域が無い | 0.90 |
| R1-16 | ファイル選択 input が `display:none` でキーボード操作不能 | 中 | `client/src/lib.rs:561-564` | 0.90 |

**未確認（r1-client）**: 1件目は静的推論で実 multipart バイト列は未実行。見送り候補のうち
「セッション期限切れ時の自動 refresh/logout 欠如」は**基盤のみ実装 2026-11-22**: 共有 JWT signal と
`api_fetch` / `api_fetch_request` による 401 の1回 refresh 再試行・単一フライトの基盤を追加したが、
全 fetch への波及で既存の 401 固定文言ビューと一覧フェッチの再描画が競合したため
`ENABLE_AUTO_REFRESH=false` で自動有効化は保留。E2E 回復フェーズも未統合のまま（69件維持）。
「バイト単位のハッシュ接頭辞スライス」は未対応のまま候補として残す。
price 入力の `min`/`max` 未指定は**対応済み 2026-11-21**: `min="0"` /
`max="1000000"` を指定し、送信前ゲート（`client/src/lib.rs` の `price_range_error`）を
サーバー契約 0..=1_000_000 へ揃えた（E2E は `e2e/tests/upload.spec.ts` の価格境界シナリオ）。

### r1-auth: 認証・セッション・年齢ゲート

| # | finding | 重要度 | 根拠 | 信頼度 |
| --- | --- | --- | --- | --- |
| R1-17 | **年齢ゲートが開発用モック経路にしか配線されず、本番 OIDC が年齢検査をバイパス** | **高** | `server/src/lib.rs:1297` が `validate_age()` の唯一の呼び出し。`oidc.rs:587-800` に判定が無い | 0.95 |
| R1-18 | **refresh Cookie の `Path=/auth/refresh` 固定で、ログアウト時のファミリ失効が到達不能** | **高** | `server/src/oidc.rs:61` vs `server/src/lib.rs:1418-1447`。テスト `tests.rs:6976-6990` は手動付与 | 0.90 |
| R1-19 | OIDC ID トークンの `nbf` がデシリアライズのみで検証されていない（**対応済み 2026-11-18**: `d8e1393` で `Validation.validate_nbf=true` を明示有効化） | 中 | `server/src/oidc.rs:256,408-412`、`DETAILED_DESIGN.md:106` と矛盾 | 0.88 |
| R1-20 | back-channel logout の `sub` 失効が処理時刻を閾値にし、リプレイが後発セッションを誤失効 | 中 | `server/src/oidc.rs:817-833,496-534` | 0.65 |
| R1-21 | `/auth/login` が `ALLOW_MOCK_AUTH` 未設定時も常時有効 | 低 | `server/src/lib.rs:1266-1283,1100` | 0.90 |
| R1-22 | OIDC の `state`/`nonce` がユーザーエージェントに束縛されずログイン CSRF の余地（推測） | 中 | `server/src/oidc.rs:536-692` | 0.55 |
| R1-23 | 未認証で開始できる OIDC pending に上限がなく、TTL 掃除も取出し時のみ（**対応済み 2026-11-18**: `57949c3` で上限 1024件・429・期限切れ回収を追加） | 低 | `server/src/oidc.rs:109,547-557,191-201` | 0.60 |

**親の Runtime Proof**: `grep -rn "validate_age" server/src/` は `lib.rs:1297`（呼び出し）と
`lib.rs:2457`（定義）のみで、`oidc.rs` に呼び出しは無い。Cookie も `oidc.rs:61` で
`Path=/auth/refresh` を確認。R1-17 と R1-18 は**コード上確定**。

### r1b-payment: 決済監査（初回失敗につき再起動）

| # | finding | 重要度 | 根拠 | 信頼度 |
| --- | --- | --- | --- | --- |
| R1-24 | **ウォレット所有証明（bind）が状態に反映されず verify の必須条件になっていない** | **高** | `server/src/payments.rs:392-510`。migration 002/003/007 に bound 列が無い | 0.85 |
| R1-25 | **verify_order にステータスの事前条件（CAS）がなく、並行verify で状態が上書きし合う** | **高** | `server/src/payments.rs:428-504` は `WHERE order_id = ?` のみでトランザクション外 | 0.80 |
| R1-26 | BIP44 派生インデックスの MAX+1 がトランザクション外 read-modify-write（同時注文で 500） | 中 | `server/src/payments.rs:295-353`、`migration 003:7-9` | 0.75 |
| R1-27 | 受取アドレスの内容検証が無くゼロアドレスを受理しうる | 低 | `server/src/lib.rs:520-529`、`payments.rs:120-125,311-312,446` | 0.45 |

**未確認（r1b-payment）**: 競合・状態遷移はコード上の制御フローからの推論で再現テスト未実施。
JPYC 外部仕様は一次ソース未確認。R1-27 は信頼度 0.45 で「推測」として扱う。

## Round 2: 要件・設計ギャップ

### r2-handoff: 引継ぎ・レビュー記録・状態の食い違い

| # | finding | 重要度 | 根拠 | 信頼度 |
| --- | --- | --- | --- | --- |
| R2-01 | `DOCUMENT_STATUS.md:34` が `acr`/`amr` 検証を「未着手」と記載、他文書と矛盾 | 中 | `docs/DOCUMENT_STATUS.md:34` vs `TASK_INVENTORY.md:55-56`、同書 `:87` とも自己矛盾 | 0.85 |
| R2-02 | `HANDOFF.md` の trunk バージョンが固定版 0.21.14 と 0.22.1 で食い違う | 中 | `docs/HANDOFF.md:136` vs `:127,:130,:149` | 0.90 |
| R2-03 | `HANDOFF` の参照コミット・テスト件数がセクション別に古いまま混在 | 低 | `docs/HANDOFF.md:17-22,109,320` | 0.85 |
| R2-04 | `REVIEW_REPORT` の過去の「未実装/未対応」記録に追記が無く現行状態と矛盾 | 中 | `docs/REVIEW_REPORT.md:575-576,594-595` | 0.75 |
| R2-05 | セキュリティ勧告の受容根拠「秘密鍵演算はテストに限られる」が RS256 追加後も未更新 | 中 | `docs/SECURITY_ADVISORIES.md:36-37` vs `:146-150` | 0.80 |
| R2-06 | 「安定識別子の生成」「プライバシー保護」が未決定で担当・期限未定義 | 中 | `docs/REVIEW_REPORT.md:36`、`DETAILED_DESIGN.md:212` | 0.85 |
| R2-07 | ユーザーハッシュの保持期間・削除/訂正手続・バックアップ消去方針が未定義 | 中 | `docs/OPERATIONAL_POLICY.md:36` | 0.75 |
| R2-08 | 「エラー応答・観測性が本番要件を満たすか別途検証」が未消化 | 低 | `docs/OPERATIONAL_POLICY.md:84`、`REVIEW_REPORT.md:409-411` | 0.80 |

### r2-requirements: 要件と実装のトレーサビリティ

| # | finding | 重要度 | 根拠 | 信頼度 |
| --- | --- | --- | --- | --- |
| R2-09 | **通報中・却下コンテンツが直接 CID 取得で配信され「暫定非表示」が機能していない** | **高** | `REQUIREMENTS_SPECIFICATION.md:40`、`server/src/lib.rs:1498-1503` に status 条件が無い | 0.85 |
| R2-10 | 「本人確認結果」が監査対象として定義されずエクスポートにも含まれない | 中 | `REQUIREMENTS_SPECIFICATION.md:38` vs `lib.rs:1285-1348`、`sign_audit_export.rs:105-140` | 0.80 |
| R2-11 | モデレーションの「通知」「再審査」手続が未定義 | 中 | `REQUIREMENTS_SPECIFICATION.md:40`、`BASIC_DESIGN.md:73` | 0.80 |
| R2-12 | JPYC の「手数料表示」と失敗・返金・紛争処理が仕様化されず実装も無い | 中 | `REQUIREMENTS_SPECIFICATION.md:46`、`BASIC_DESIGN.md:34`、`payments.rs:17-38` | 0.85 |
| R2-13 | 画像デコード等の機能検出・理由表示・代替手段が未実装 | 中 | `REQUIREMENTS_SPECIFICATION.md:59`、`client/src/lib.rs:357-360`（R1-13 と同一コード） | 0.80 |
| R2-14 | コンテンツ削除・法的要請対応の手順が未設計（UNPIN は判断記録のみ） | 中 | `REQUIREMENTS_SPECIFICATION.md:64`、`server/src/lib.rs:2055-2080` | 0.80 |
| R2-15 | 「名前解決の単一障害点」「サービス停止権限」の脅威モデル化が無い | 中 | `REQUIREMENTS_SPECIFICATION.md:63`、`INFRA_NETWORK.md:36-42` | 0.70 |
| R2-16 | 認証境界の例外リストに `/metrics` が含まれていない（解釈依存） | 低 | `REQUIREMENTS_SPECIFICATION.md:22` vs `server/src/lib.rs:1130-1133` | 0.60 |

**親の Runtime Proof（R2-09）**: `server/src/lib.rs:1498-1504` の SQL は
`"SELECT checksum, type FROM contents WHERE cid = ?"` で status 条件が無い。
Reported/Rejected でも CID を知っていれば配信されることは**コード上確定**。

### r2b-design: 設計・ADR ギャップ（初回失敗につき再起動）

| # | finding | 重要度 | 根拠 | 信頼度 |
| --- | --- | --- | --- | --- |
| R2-17 | `DETAILED_DESIGN.md` の「実装ルート」表に運用者特権ルートと `/ready` が欠落 | 中 | `docs/DETAILED_DESIGN.md:18-44` vs `server/src/lib.rs:1112-1115,1131` | 0.95 |
| R2-18 | ADR-0002 の「実装状況（2026-11-02）」が対応済み機能を未実装と記載したまま | 中 | `docs/adr/ADR-0002:104-105` vs `oidc.rs:150,153,156`、`lib.rs:1104`、migration 008 | 0.90 |
| R2-19 | ADR-0005 の Scope/Decision が `governance_audit_log` を含む署名実装と不一致 | 中 | `docs/adr/ADR-0005:15,27` vs `sign_audit_export.rs:134-135` | 0.85 |
| R2-20 | `DETAILED_DESIGN.md:28` の sub 失効「インメモリ」記述が同一文書と矛盾 | 中 | 同書 `:115` は `revoked_subs` 永続化を記載、migration 008 が実在 | 0.90 |
| R2-21 | モック認証「`ALLOW_MOCK_AUTH=1` の場合のみ有効」が `/auth/login` 実装と不一致 | 中 | `DETAILED_DESIGN.md:90` vs `server/src/lib.rs:1100,1266-1283`（R1-21 と同一コード） | 0.90 |

## Round 3: テスト・検証ギャップ

### r3-server-tests: テストの mutation 検出力

| # | finding | 重要度 | 根拠 | 信頼度 |
| --- | --- | --- | --- | --- |
| R3-01 | メトリクス エラーカウンタが絶対値アサーションで、4xx 加算削除 mutation を通す | **高** | `server/src/tests.rs:7200-7226` が `errors_after >= 1` のみ | 0.92 |
| R3-02 | DB マイグレーションの冪等性テストが適用件数しか見ず、2回目のデータ消失を検出できない | 中 | `server/src/migration.rs:359-362` | 0.78 |
| R3-03 | JXL シグネチャ検証が2分岐のうち片方のみテストされ、コンテナ形式の誤拒否を通す | 中 | `server/src/tests.rs:1754-1758` vs `lib.rs:2507` | 0.90 |
| R3-04 | 設定境界の実値（30/3600 秒、1/300 秒）が未テストで off-by-one を通す | 中 | `server/src/tests.rs:6370-6418` vs `lib.rs:927,915` | 0.95 |
| R3-05 | 既定 IPFS ゲートウェイが件数とプレースホルダしか検証されず、http 差し替えを通す | 中 | `server/src/tests.rs:1038-1040` vs `lib.rs:823-827` | 0.82 |
| R3-06 | `parse_args_requires_both_paths` が必須性を検証せず、main のチェック削除を通す | 中 | `server/src/bin/restore_db.rs:218-226` vs `:108-109` | 0.90 |
| R3-07 | 証明書解析テストが「空でない」しか見ず、fixed-value 実装を通す | 中 | `server/src/jpki.rs:72-79` | 0.85 |
| R3-08 | `test_readiness_ok_and_shutting_down` の名前と検証内容が食い違う | 低 | `server/src/tests.rs:6796-6818` | 0.88 |

**注意**: mutation の成否は制御フローとアサーションからの推論で、実際の mutation 実行は未実施。

### r3-e2e-gap: E2E シナリオ・ブラウザカバレッジ

| # | finding | 重要度 | 根拠 | 信頼度 |
| --- | --- | --- | --- | --- |
| R3-09 | E2E が Chrome 1 ブラウザ固定で Edge/Brave/Firefox/Safari のカバレッジが皆無 | **高** | `e2e/playwright.config.ts:42-50`、`e2e/ci-e2e.sh:60-69,102-103` | 0.97 |
| R3-10 | ログイン→失効（logout/401/トークン更新）の UI journey に E2E が無い（**解消済み 2026-11-22**: `e2e/tests/auth.spec.ts` の既存 logout テスト内へ 401 → `/auth/refresh` 1回 → 再試行200 のフェーズを統合。新規 `test()` なしで69件維持） | **高** | `e2e/tests/auth.spec.ts:3-41`、`client/src/lib.rs:144-152,267-283` | 0.90 |
| R3-11 | 通報→投票→確定の「投票」区間に E2E が無く UI はゼロ署名 | **高** | `e2e/tests/moderation.spec.ts:6-116`、`client/src/lib.rs:710-732,777-784` | 0.88 |
| R3-12 | E2E の 8/10 テストが `/auth/verify` をスタブし実 JWT 経路を通らない（分母10件は2026-11-18当時で未更新。現在値は TEST_SPECIFICATION を参照） | **高** | `e2e/tests/economy.spec.ts:17-26` 他 | 0.93 |
| R3-13 | 空状態・エラー状態・ローディング状態の UI が E2E で未検証 | 中 | `client/src/lib.rs:304-312,645-647,790-792,423,253` | 0.95 |
| R3-14 | アップロード成功 E2E が不正な JXL バイトとスタブ応答で成功を宣言 | 中 | `e2e/tests/upload.spec.ts:25-43` vs `lib.rs:2504-2513` | 0.92 |
| R3-15 | 新規投稿→閲覧のクロス画面 journey が E2E で接続されていない | 中 | `upload.spec.ts:43` で終了し、閲覧側は別 CID をモック | 0.85 |
| R3-16 | `fullyParallel` と実サーバー再利用でローカル E2E が状態リークしうる | 低 | `e2e/playwright.config.ts:18-34,72,79` | 0.70 |

**補足**: 2026-11-18 当時は TASK_INVENTORY が「E2E 9件」と記載する一方、ソースは
`test()` 35 件（11 spec ファイル）で26件差があった（履歴）。2026-11-22 実測では
69 件 / 12 spec であり、当時の 9件/35件は現在値ではない。
旧割合は注記のみで現行値へ更新していない。現在値は TEST_SPECIFICATION を参照。

## Round 4: 運用・本番化・CI/CD

| # | finding | 重要度 | 根拠 | 信頼度 |
| --- | --- | --- | --- | --- |
| R4-01 | **OIDC 必須設定が未設定でも本番起動が継続する（fail-closed の穴）** | **高** | `server/src/oidc.rs:134-141` が `None` を返すだけ、`lib.rs:531-534` は info ログのみ | 0.93 |
| R4-02 | **全 API ルートにレート制限・同時実行数上限が存在しない** | **高** | `server/src/lib.rs:1097-1149` に throttling layer 無し、`:601` にプール上限なし | 0.92 |
| R4-03 | **OIDC の JWKS/token 取得に HTTP タイムアウトが無く設定キーも無い** | **高** | `server/src/oidc.rs:318-321,340-351` に `.timeout()` なし | 0.90 |
| R4-04 | **`PINATA_JWT` 未設定時に本番でもモック CID を発行しうる** | **高** | `server/src/lib.rs:2241-2245` が `QmMockPinata{hash}` を返す | 0.85 |
| R4-05 | 鍵ローテーション Runbook が RSA 最小鍵長を 3072bit と誤記載 | 中 | `KEY_ROTATION_RUNBOOK.md:105,44-45` vs `server/src/jwt.rs:14`（2048） | 0.95 |
| R4-06 | 無停止ローテーション検証が再起動でゼロに戻るカウンタに依存 | 中 | `KEY_ROTATION_RUNBOOK.md:85-87` vs `OPERATIONAL_POLICY.md:95` | 0.88 |
| R4-07 | 真偽値設定キーで解釈ルールが異なり `'yes'` 等が有効になってしまう | 中 | `OPERATIONAL_POLICY.md:146` vs `lib.rs:493-495`、`oidc.rs:143-145` | 0.86 |
| R4-08 | 署名付き監査スナップショットの自動生成・公開が手動依存 | 中 | `AUDIT_EXPORT_RUNBOOK.md:87`、`deploy/` に audit 用スケジューラ不在 | 0.78 |

**親の Runtime Proof**: `server/src/lib.rs:531-534` は OIDC 未設定でも info ログのみで起動継続
（R4-01 確定）。`:2241-2245` は `PINATA_JWT` 未設定時に `"mock_jwt"` を既定とする（R4-04 確定）。
両方とも `ALLOW_MOCK_AUTH` と無関係に発動しうる。

## Round 5: 機能追加案・改善提案

| # | 提案 | 重要度 | 根拠（拡張可能な既存コード） | 信頼度 |
| --- | --- | --- | --- | --- |
| R5-01 | コンテンツのメタデータ（タイトル/タグ/説明）と検索・絞り込み API | 中 | `server/src/lib.rs:1614`（固定 SQL `LIMIT 50`）、`:346`、`migration 001:13` | 0.85 |
| R5-02 | お気に入り / ブックマーク（コレクション） | 中 | `build_router` に favorites 系無し、`migration 001:5`、`client/src/lib.rs:386` | 0.80 |
| R5-03 | クリエイター別ページと投稿者表示 | 中 | `server/src/lib.rs:346,1614`、`migration 001:75` のインデックスが未活用 | 0.75 |
| R5-04 | データ主体の権利：自分のデータのエクスポートと削除権 | 高\* | `/me/export` / `/me/erase` が無い、`common/src/lib.rs:12` の `Claims.sub` で抽出可 | 0.70 |
| R5-05 | 同意管理（規約受諾の記録と再同意） | 中 | `server/src/lib.rs:1285`、`migration 001:5`（consent 列無し） | 0.60 |
| R5-06 | 設定検証 CLI（`doctor` コマンド） | 中 | 設定検証 bin が無く、検証ロジックが `lib.rs:721-761,763,771,834,898` に分散 | 0.90 |
| R5-07 | モデレーション判定結果の通知（R2-11 と同一要件の機能面） | 中 | `server/src/lib.rs:1812,1793`、`migration 001:58`、`client/src/lib.rs:188` | 0.65 |
| R5-08 | 支援者向けレポート / クリエイター別の支援集計 | 低 | `server/src/payments.rs:41`、`migration 002`、`lib.rs:1109,1642` | 0.60 |
| R5-09 | 透明性ダッシュボード（モデレーション集計の可視化） | 中 | `migration 005:1`、`server/src/lib.rs:933,1161`、`client/src/lib.rs:589` | 0.70 |
| R5-10 | 小説の目次（TOC）と見出しアンカー | 低 | `client/src/lib.rs:12`（Heading に id を付与しない）、`:401` | 0.65 |

\* R5-04 の「高」は機能提案としての優先度であり、r1〜r4 のバグ/セキュリティ finding の
「高」とは意味が異なる。推測を含む提案: R5-04、R5-05、R5-08、R5-09（外部要因に依存）。

## Round 6: 統合（重複排除・妥当性評価）

### 6.1 重複の吸収

82件の生findingを、根本原因・影響・対策が同一のものは1つのクラスタへ集約した。

| 統合項目 | 吸収した元finding | 統合の根拠 |
| --- | --- | --- |
| C1 E2E のモック依存と実 API 未検証 | R1-02, R3-12, R3-14 | 全て「`page.route` スタブで実サーバー応答を通さない」という同一原因 |
| C2 モック認証の本番露出 | R1-21, R2-21 | `/auth/login` が `ALLOW_MOCK_AUTH` でゲートされない同一コード |
| C3 JXL デコード失敗の誤表示 | R1-13, R2-13 | 同一の `let _ = decode_jxl_to_canvas(...)`。片方はコード、他方は要件側の観点 |
| C4 設計・状態ドキュメントの陳腐化 | R2-01, R2-02, R2-03, R2-04, R2-18, R2-20 | 全て「実装済み／現行値が古い記述のまま」という同一原因（現行状態欄は要修正、日付付き履歴欄は許容） |
| C5 モデレーション通知・再審査の未定義 | R2-11, R5-07 | 同じ要件（通知と異議後手続）の、実装ギャップ側と機能提案側 |
| C6 セキュリティ勧告の受容根拠の陳腐化 | R2-05, R4-06 | RS256 追加・ローテーション実装で根拠が無効化された同一テーマ（対象文書は異なるが併記） |

集約後は **82件 → 76項目**（重複6件を吸収）。うち C4 は6件を1項目にまとめたため実質的な削減は大きい。

### 6.2 妥当性評価

親が現行コードを直接読み、以下を裏取りした（Runtime Proof）。

- **確定（コード上）**: R1-09（空 Blob の MIME 不一致）、R1-10（無 SRI の CDN）、
  R1-17（`validate_age` が OIDC 経路に無い）、R1-18（Cookie Path 不一致）、
  R2-09（`get_content` に status 条件が無い）、R2-05 / R4-06（受容根拠が RS256 と矛盾）、
  R4-01（OIDC 未設定でも起動継続）、R4-04（`PINATA_JWT` 未設定でモック CID）。
- **妥当だが未実証（静的推論）**: R1-24 / R1-25 / R1-26（決済の状態記録・CAS・TOCTOU）、
  R3-01〜R3-08（mutation 検出力）、R4-02 / R4-03（レート制限・タイムアウト）。
  いずれも制御フローとアサーションに基づく仮説で、再現テストは未実施。
- **推測として扱う（低 confidence）**: R1-20（0.65）、R1-22（0.55）、R1-23（0.60）、
  R1-27（0.45）、R2-16（0.60）。実 OP・実ブラウザ・一次ソースの確認が必要。
- **却下／保留**: なし。ただし C4（文書陳腐化）のうち日付付き履歴スナップショットは
  「陳腐化」ではなく「履歴」として残す判断もあり得る。現行状態を謳う欄のみ修正対象とする。
- **既知項目との境界**: 全エージェントが TASK_INVENTORY 全読を必須とし、記載項目は除外した。
  失敗した2エージェント（r1-payment / r2-design）は再起動で回収し、未検証事項を残していない。

### 6.3 優先順位付きアクション

P0 — 本番公開前に解消すべきセキュリティ／データ完全性の穴（確定項目が中心）:

1. R1-17 年齢ゲートを OIDC callback / refresh へ配線し、信頼属性必須でフェイルクローズ。
2. R2-09 `get_content` へ status 条件を追加し、Reported/Rejected を 404/403 にする。
3. R1-18 refresh Cookie の `Path` を `/auth` へ広げ、logout 時のファミリ失効を有効化。
4. R1-24 ウォレット bind を DB に記録し verify の必須条件にする。
5. R4-01 / R4-04 本番起動時に OIDC 必須設定と `PINATA_JWT` を検査し fail-closed 化。
6. R1-09 アップロード UI の file パートへ MIME を設定（実サーバーで1件送信確認）。
7. R1-10 Tailwind をビルド時 CSS 化し、外部 CDN/フォント依存と無 SRI を解消。

P1 — 本番運用の健全性（静的推論だが影響が大きい）:

- R4-02 認証・アップロード・注文作成へレート制限と同時実行上限を導入。
- R4-03 OIDC HTTP タイムアウト設定キーを追加。
- C2 `/auth/login` を `ALLOW_MOCK_AUTH` でゲートし、設計書と実装を整合。
- R1-25 verify_order へ CAS とトランザクションを導入し、確定後不変を担保。
- R3-01 メトリクスカウンタを差分アサーション＋注入可能設計へ変更。
- C1 主要画面に実サーバー認証を使うスモーク E2E を1本追加。

P2 — 品質・保守・プロダクト拡張:

- C4 現行状態を謳う設計・状態ドキュメントを更新（ADR-0002 / DOCUMENT_STATUS /
  DETAILED_DESIGN:28,90 など）。日付付き履歴は残す。
- C6 セキュリティ勧告の受容根拠を RS256 本番署名に合わせて再評価。
- R3-09 に firefox/webkit プロジェクトを追加し、Chrome 固定を解消。
- R5-06 設定検証 CLI（`doctor`）を追加（R4-01 の事前検証としても有用）。
- R5-01 / R5-02 / R5-03 / R5-09 / R5-10 は機能提案。法務・決済依存の
  R5-04 / R5-05 / R5-08 は一次ソース確認と設計が先。

### 6.4 結論

既存 TASK_INVENTORY は「実装した機能の状態」の網羅には優れているが、
**「実装はあるが配線が死んでいる」「本番設定で fail-open になる」「要件・設計と実装の差分」**
という観点に盲点があった。本調査で抽出した P0 の7項目は、いずれも既存の対応済み記録と
矛盾しない形で潜んでいた未知の課題である。まずは Runtime Proof 済みの P0 から着手するのが妥当。

### 6.5 脆弱性勧告の受容件数（現行値）

- 既知の12件の RUSTSEC 勧告（脆弱性8件 / unmaintained4件 = libsql 由来10件・paste 1件・chrono 1件）を理由付きで ignore している（[SECURITY_ADVISORIES.md](SECURITY_ADVISORIES.md)、[deny.toml](../deny.toml)）。
- 2026-11-17 時点は11件だったが、RUSTSEC-2026-0543（chrono unmaintained）を受容して12件になった（履歴）。
