---
title: "テスト仕様書 (Test Specification Document)"
type: Test Specification
description: 現行回帰テストと必須要件のリリース受け入れ条件を定義する。
tags: [testing, acceptance, quality]
generated:
  by: dsh
  at: "2026-11-22T23:55:00+09:00"
verified:
  by: dsh
  at: "2026-11-22T23:55:00+09:00"
---

## テスト仕様書 (Test Specification Document)

本書は現行プロトタイプの回帰テストと、必須要件のリリース受け入れテストを区別する。
JPKI、JPYC、第三者検証可能な監査は、実装前だからといって最終検証対象から除外しない。

## 1. テスト層

- **Unit**: `common` のチェックサム・年齢計算・監査ハッシュ等。
- **Integration**: Axumルーター、JWT認可、DB状態遷移、制約の検証。
- **E2E**: PlaywrightによるUIフロー確認。APIをモックするテストは実バックエンド統合を証明しない。
  実サーバー結合は `e2e/tests/real-api.spec.ts`（4件: JXL アップロード / 検索 `q` /
  お気に入り追加削除 / `me/export`）でカバーする。実測は全69件（auth 3 / build-gate 15 / economy 3 /
  error-paths 6 / export 3 / favorites 5 / integrity 6 / metrics 3 / moderation 14 /
  real-api 4 / search 1 / upload 6）。この内訳は `e2e/tests/*.spec.ts` の `test()` 定義を数えた値である。
  auth は3件のまま。認証トークン失効時の回復経路は有効化済み（`ENABLE_AUTO_REFRESH=true`）で、
  単一フライトで競合解決済み。refresh 失敗時は元の 401 を返す契約を E2E 既存ケース内で検証する
  （新規 `test()` は追加せず69件を維持する）。
  R2-URL の回帰は既存 E2E 内へ統合し（下記の EXPORT-11 / REPORTS-ERR / 監査注記も既存テスト内へ統合）、新規 `test()` は追加していない（69件を維持）。
  低優先(1)〜(7)の E2E 側反映でも（EXPORT-11 / REPORTS-ERR / 監査注記の追加分は既存テスト内へ統合済み） `test()` 定義は増えていない（69件を維持）。
  `upload.spec.ts` / `export.spec.ts` の件数文言は2件のみ更新し、GENERATED_AT 期待値と
   固定文言（禁止リスト）は据え置いた。
   EXPORT-11 の解析失敗フェーズは `export.spec.ts` の既存1件目（ダウンロード成功シナリオ）内へ、
   REPORTS-ERR の解析失敗フェーズは `moderation.spec.ts` の REPORTS-ERR-01 内へ、
   監査注記のアサーション更新は `integrity.spec.ts` の既存 test 内へ追加した
   （いずれも既存テスト内へ統合、新規 `test()` なし、E2E69件を維持）。
   `moderation.spec.ts` は REPORT-ERR-01 内へ
  空 CID / 空 reason の検証フェーズを、`search.spec.ts` は既存1件内へ `q='Ghost'` の
  0件表示とクリア復帰フェーズを、`integrity.spec.ts` は既存2件内へ
  aria-label フォールバックと要約注記のアサーションを追加した（新規 `test()` なし）。
   byte 契約（CONTENT-06 / MOD-08）の境界回帰もクライアント側の既存 `#[cfg(test)]`
   モジュール内へ単体テストとして統合し、E2E の `test()` は追加していない（69件を維持）。
  新規に参照する testid は `clear-search-filters`（条件付き空状態のクリアボタン）と
  `report-validation-error`（通報入力検証の `role="alert"`）、`upload-failed` /
  `report-failed`（失敗専用の `role="alert"`）で、既存の `dashboard-error` /
  `upload-status`（成功・進捗の `role="status"`） / `export-error` 等と併せて固定文言契約を検証する。
  既存 test() 内へのアサーション追加のみで、E2E 定義数69件は変更しない。
  `search.spec.ts` は重複 `q` と未知キーを含む URL を注入したうえで正規化書き出しを検証し、
  GET を増やさず `?q=Neon` と `#dashboard` へ冪等に収束することを確認する。
  `economy.spec.ts` は `history.pushState` を失敗させ、履歴更新失敗の固定警告を捕捉して
  view が BROWSE のまま変わらない fail-closed を検証し、API 復元後は通常のタブ遷移を継続する。
  `favorites.spec.ts` は `favorite_only` の ON/OFF が URL クエリへ同期し、解除時に
  クエリが空へ戻ることを既存のリクエスト完全一致アサーションを維持したまま確認する。
  favorites の5件はトグル基本フローに加え、二重クリック防止・401の再ログイン案内・
  失敗後の再トグルで旧エラーを消すこと・ネットワークエラー文言の表示を検証する。
  export の3件目はダウンロード開始失敗時の固定エラー表示（EXPORT-07）で、
  EXPORT-11 の解析失敗とは別事象である。
  error-paths の6件は DASH-ERR / export network / AUTH-500 / AUDIT-500 に加え、
  CONTENT-ERR-404 / CONTENT-ERR-500（取得失敗表示と改ざん検知の誤表示回避）を含む。
  metrics の3件は MET-01 と E2E 固有の NEUT-URL-02 / NEUT-URL-03 であり、
  サーバー側 `observe_metrics` の MET-02 とは別 ID である。
  moderation の14件のうち3件は投票結果のフィードバック表示（VOTE-FB-01〜03）、
  2件は投票一覧の取得失敗表示（REPORTS-ERR-01〜02）、2件は通報送信失敗表示
  （REPORT-ERR-01〜02、失敗は dedicated `role="alert"`）、2件は二重送信ガード
  （REPORT-GUARD-01 / VOTE-GUARD-01）の回帰である。
  integrity の6件は正規・改ざんに加え、3件目が R1-13（チェックサム一致だが JXL デコード失敗時に
  VERIFIED にしない）、4件目が AUDIT-04 の FORENSIC_TIMELINE 順序、
  5件目が Audit トグルの ARIA 状態と Escape 時のフォーカス復帰（a11y）、
  6件目が CLOSE ボタンでダイアログを閉じてトグルへフォーカスが復帰することを検証する。
  upload の6件は送信成功・10 MiB 拒否・INT-02 に加え、4件目が R1-14
  （`arrayBuffer()` 拒否時に失敗用 `role="alert"` の `upload-failed` へ固定エラー文言を
  表示し、`upload-status` の `role="status"` も維持したうえでボタンを再活性化）、
  5件目がファイル選択 label のキーボード操作と価格入力の上下限（a11y）、
  6件目が R2 価格境界テスト（-1 / 1000001 がクライアントの送信前ゲートで固定文言により
  拒否されて route 0回、0 / 1000000 が丸めずに送信されること）を検証する。
  メトリクス最外側化・bind 署名 v2・監査CLI硬化の反映では E2E 定義は増えていないため、上記の内訳を反映（当時59件、現在69件）。
  追加された5件はいずれも `server/src/tests.rs` のサーバー側テスト（MET-02 の4件と PAY-05 の1件）である。
  今回の3CLIへのテスト追加（doctor 53件（既存29件＋境界テスト7件＋OIDC URL検証3件＋
  oidc_required_assurance1件＋設定検証3件＋ヘルプ優先2件＋AUDIT_EXPECT_ROOT1件＋
  AGE_POLICY_CONFIGURED3件＋バックアップ/TZ診断3件＋バックアップ生値判定1件） / verify_audit_export 20件 / verify_backup 19件）も
  全て `server/src/bin/` 配下のサーバー側 bin テストであり、E2E 定義数は当時59件のまま（現在69件）。
  直近の SSRF 2件 / XFF 11件 / doctor 追加4件もサーバー側テストであり、E2E 定義数は当時59件のまま（現在69件）。
   直近の年齢ゲート共通化（callback / refresh の設定必須403）・TZ 境界・`/me/export` の
   読み取りトランザクションの5件も `server/src/tests.rs` のサーバー側テストであり、
   E2E 定義数は当時59件のまま変更なし（現在69件）。
   直近の監査理由マスク1件・投票署名 v2 の4件・一覧の `price` NULL 1件は
   `server/src/tests.rs` のサーバー側テスト、エクスポート解析失敗1件は
   `client/src/lib.rs` のクライアント側単体テストである。いずれも E2E 定義を増やさず、
   E2E 定義数は当時59件のまま変更しない（現在69件）。

## 2. 実装済みテストの概要

| 領域 | 現行カバレッジ | 制約 |
| --- | --- | --- |
| セキュリティヘッダ | 全レスポンスへ `X-Content-Type-Options: nosniff`、`X-Frame-Options: DENY`、`Content-Security-Policy: default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'` を付与（通常 200、IP ゲートの 403、レート制限の 429、内部エラーの 500、本文なしの 204、CORS プリフライトの OPTIONS 成功レスポンスを含む）。CorsLayer の短絡を避けるため最外側にも `security_headers` を置く | 単体テストと実サーバーで確認済み。CSP は API レスポンスのみへ付与し、HTML 配信は配信側担当 |
| 認証 | モックverify（nonce 必須・単回消費・再利用400）、無効トークン系、verify必須フィールド（birth_date/country_code）欠落と不正birth_dateの400拒否、OIDCはstate再利用/未知state、nonce不一致、JWKS未登録鍵での署名、kid欠落、at_hash不一致を401で拒否、OIDC未構成時のstart/callback/back-channel logoutは501、年齢判定（`common` のポリシー/計算ユニットテスト、`server` の検証・`GET /policy/age` 公開応答・明示設定必須/信頼属性必須フェイルクローズ・OIDC callback/refresh の設定必須403・`TZ` ローカル日付の境界、独立参照計算による proptest と境界月日の明示的サンプリング）、JWT秘密鍵長検査のユニットテスト、RS256 の `kid` 付与・一致検証・未知 `kid` 拒否・`kid` 無し旧トークン互換・旧公開鍵によるローテーション検証・重複 kid 拒否、refresh Cookie の Path=/auth（ログアウト時のファミリ失効到達）を検証。JPKI は証明書の base64 デコード・X.509 パース・有効期限・自己署名（DN 一致 + 暗号学的検証）・公開鍵ポリシーのエラーパス10件を Unauthorized の固定メッセージまで検証。開発用モック経路の `/auth/verify` では所有証明（PoP・opt-in）をハンドラ経由4件で検証（正常署名200・改ざん nonce/別鍵署名401・省略時200）。ログアウトは `refresh_token` 削除 Cookie（空値・`Path=/auth`・`HttpOnly`・`SameSite=Strict`・`Max-Age=0`・`OIDC_COOKIE_SECURE` 追随の `Secure`、提示トークンはエコーせず本文 `true` を維持）を検証。クライアントは `/auth/login` が 501 のとき固定文言 `本番認証は未実装です。` を出して `/auth/verify` を送らず早期 return し、`/auth/verify` が 501 のときも同文言へ変換する（403 / 500 等は既存 `認証に失敗しました (HTTP <status>).` を維持し、サーバー本文は非表示）。ビルド時に `OIL_MOCK_AUTH=0` を指定するとログイン画面のモック認証注記を非表示にする。リフレッシュは BAN チェックと claims/JWT 事前計算を `BEGIN` 前に済ませ、消費 UPDATE（変更0件なら rollback 後に別tx でファミリ失効）と新トークン INSERT（`issue_refresh_token_in_tx`）を単一 tx で原子的に実行し、`test_refresh_rotates_token_and_issues_new_session` が旧 `used_at` 非NULL・新 `used_at` NULL を検証する。`OIDC_MAX_AUTH_AGE_SECS` は上限 `MAX_AUTH_AGE_MAX_SECS = 2_592_000`（30日）超過を固定文言ハードエラーで起動拒否（fail-closed・値非エコー）し、`u64::MAX` は拒否される境界テストで確認する | 実JPKI/OID4VPの信頼検証、検証済み年齢属性入力、HSM 鍵管理は未テスト・未実装。現行年齢判定は申告値前提 |
| ネットワーク | IPv4拒否・IPv6許可・IPv4-mapped正規化・XFF偽装回帰・fail-closedのミドルウェアテスト、XFF追記型（末尾ピア除去で最左クライアント採用）/上書き型（末尾採用）、複数値・不正エントリのfail-closed、IPv4-compatible IPv6（`::a.b.c.d`）拒否と `::` / `::1` 維持、SSRFリダイレクト無効化（3xxは追従せずフェイルオーバー）、CORSオリジン解析テスト、`MAX_CONCURRENT_REQUESTS` の境界値（既定16、有効範囲 1..=256、範囲外は既定へフォールバック）と同時実行上限 1 の実リクエスト（2件目は429、permit 返却後は200）を検証 | 実プロキシ、多段構成、WAFは未検証 |
| エラー公開 | 500系詳細の既定非公開と `EXPOSE_INTERNAL_ERRORS` での公開のユニットテスト | 4xx系メッセージの公開範囲は未整理。オラクル: 代表4xx（400/404/429）の本文は `{"error": "<固定文言>"}` のみで、入力値・内部詳細・トークンをエコーしない（400: `Invalid CID` / `Invalid report reason`、404: `Resource not found`、429: `rate limit exceeded` / `too many concurrent requests`）。500系の既定非公開とは独立に仕様化する |
| 可観測性 | `RUST_LOG` のフィルタ動作（未設定時INFO、不正値フォールバック）と WARN 出力の起動確認。`LOG_FORMAT=json` で全ログ行が JSON としてパースできることを実サーバーで確認。`/metrics` が Prometheus 形式でリクエスト数・4xx/5xx 数を公開し、404 後にエラーカウンタが進むことを単体テストと実サーバーで確認。新カウンタ `openillustration_neutralized_urls_total` の差分加算とペイロード非機微性を下記 MET-01 で検証。finalizer/purge の5カウンタも同じ MET-01 で HELP/TYPE 形式と進捗を検証。`observe_metrics` を `Router` 最外側へ移し、429 / IPv4 403 / 413 / CORS プリフライト OPTIONS 204 の4ケースを差分で計上する下記 MET-02 の4件を追加。E2E の protocol-relative / 再送抑制は下記 NEUT-URL-02 / NEUT-URL-03 として別 ID で仕様化し、サーバー側 MET-02 と衝突させない | ログ集約基盤・アラート通知先の選定は未実施 |
| 停止動作 | `/health` 200・`/ready` 200・停止フラグ時の `/ready` 503・IP ゲート適用・`GRACEFUL_DRAIN_TIMEOUT_SECS` の既定/範囲外フォールバックを自動テストで確認。SIGTERM 時の drain ログと終了コード0は実起動確認済み。単一ライター検出（生存 PID 拒否・stale 回収・停止時削除）を単体テストと実プロセス2起動で確認 | 外部依存障害時の readiness 連動・drain タイムアウト発生時の実動作・リモート DB の並列起動制御は未試験 |
| DBマイグレーション | 未適用分の適用・冪等性・チェックサム改変検出・外部キー強制・既存DBの段階的アップグレード（new migration の backfill とデータ保持）・適用済みマイグレーションファイル欠落の検出をユニットテスト。migration 011 で追加したメタデータ列を既存行が空文字として読むことを `test_migration_011_existing_rows_read_as_empty_metadata` で検証。migration 014 は `test_migration_014_declares_expires_at_as_text` / `test_migration_014_preserves_rfc3339_expires_at` / `test_migration_014_rewrite_detects_checksum_mismatch` の3件で検証 | ロールバック、複数インスタンス同時起動の排他は未実施 |
| 完全性 | SHA-256一致・不一致、IPFS取得系をローカルモックで検証（サイズ超過・タイムアウト・次ゲートウェイへのフォールバック・ストリーム途中切断を含む）、`IPFS_MAX_ATTEMPTS` の既定/範囲検証、CIDのURLパス注入拒否、取得/監査系の不正CID 400・不存在CID 404、チェックサム不一致時の WARN ログ、チェックサム一致でも JXL デコード失敗時は VERIFIED にしないこと（E2E INT-01 / R1-13） | 実ゲートウェイの遅延・切断、実Pinata連携、厳密なCID仕様検証、ネットワークエラー時の実リトライは未検証 |
| Markdown表示 | raw HTMLのエスケープ、インライン・参照リンク・自動リンク・画像URLの危険スキーム拒否（実体参照による難読化、Unicode 空白/制御文字（ZWSP・NBSP）によるスキーム隠しを含む）と安全スキーム維持をユニットテスト。危険 URL が `render_markdown` を通した後も `href="#"` / `src="#"` に置換される統合検証も実施 | ブラウザ別URL解釈のE2Eは未実施 |
| 投稿入力 | 許可MIME、JPEG XLシグネチャ、Markdown UTF-8、重複CIDの再投稿拒否、未知フィールド拒否、全フィールド（`price` / `file` / `owner_hash` / `signature`）の重複 400 拒否、WASM 側 JXL デコードの寸法上限（8000px / 64M ピクセル）とチャンネル数別（1/2/3/4以上）RGBA 変換、クライアント選択時の 10 MiB 超過即時拒否（E2E・送信ボタン無効化と未送信も検証）を検証 | 実 JXL デコードの WASM 実行試験は未実施 |
| モデレーション | 通報状態遷移、重複通報拒否、通報reasonの空/過大拒否とトリム保存、期限前は確定しないこと、期限後確定（同数→`Rejected`、定足数未達→`Approved` 復帰）、期限切れ投票拒否、投稿者投票拒否、BAN済み投票者の拒否、30日以内の異議と30日超の通常通報、finalize エンドポイントの冪等性、重複投票拒否、確定後投票拒否、投票ペイロード検証（report_id/result）とトークンsub不一致401の拒否、投票署名メッセージが report_id と result の両方に束縛されること、運用者未設定時の BAN/UNBAN/UNPIN 拒否、運用者による BAN（根拠・期間）/UNBAN/UNPIN の監査記録、自己BAN・不正期間・空理由・BAN済み運用者の拒否、期限前スキップ・期限後自動確定・複数件処理・冪等性・ワーカー周期の範囲検証 | 同一接続への並行 `BEGIN` が失敗することの再現、書き込みロックによる直列化、並行投票と自動確定の同時実行を検証。ワーカー停止/再起動時の挙動、実運用手続、第三者検証可能な監査（DEC-04）の試験が不足。reason上限値の妥当性はADR-0004の運用で見直し |
| Upload | Content-Typeのユニット/APIテスト、負価格・非整数価格・64bit範囲外価格・上限超過価格の拒否（拒否理由を検証）、価格0と上限値 `MAX_ORDER_JPY` の受理と保存を検証、必須フィールド欠落・owner不一致・BANユーザーの拒否、実 RSA 鍵で署名したバイト列の受理と別バイト列への署名使い回し401拒否 | 実multipartサイズ境界・実Pinata連携の試験が不足 |
| ContentMeta / list_contents | `ContentMeta` が `title` / `description` / `tags` を返すこと、`list_contents` が省略可の `q` / `tag` / `type` / `limit` / `offset` を受理し既定値（limit=50、offset=0）と範囲（`1..=100`）へフォールバックすることを API テストで検証。`q` の部分一致、`tag` の前方一致、`type` の完全一致、`created_at DESC` の並びとページングの重複なしも確認 | 長さ上限・LIKE 注入拒否・XSS エスケープは CONTENT-06〜08 で個別に検証。タグの正規化・全文検索は未実装 |
| お気に入り | 追加/削除の冪等（重複追加・未登録削除）、未認証401・不正CID400・非Approved404、他人subからの操作拒否、`favorite_only` の本人分のみ返却（真理値パース・既存条件とのAND・ページング・Approved限定）、旧JSONの `is_favorite` 欠落互換、migration 012 の UNIQUE + FK CASCADE を API・ユニットテストで検証 | `user_hash` をログ・監査・レスポンスへ出さないことは設計方針で担保。実運用ログの確認は未実施 |
| 決済 | 注文ライフサイクル、未対応チェーン拒否、ウォレット署名不一致拒否、派生アドレスの一意性をモックverifierで検証、非UUID注文IDの400拒否、verifyの金額不一致（MISMATCH遷移）・送金再利用・期限切れ・確認数不足（CONFIRMING維持）の拒否、同一送金の確認数進展に伴う再verifyとFINALIZED到達、FINALIZED後の再verifyが状態を戻さないこと、`expires_at` 経過（status が PENDING のまま）の拒否と EXPIRED 更新、BIP44 derivation index の u32 境界拒否、EVM アドレスの大小文字非区別（正規化照合）を検証。`wallet_bind_message_v2` が注文ID・chain_id・wallet_address・recipient_address・amount_jpy を署名対象とし、注文IDのみの旧形式署名を401で拒否する下記 PAY-05 を追加 | 実チェーン検証、2-of-3クォーラム、実チェーン上の再編成/二重通知の試験は未実施（DEC-03） |
| Support | 501無効化のAPIテスト、送信不可UIと模擬記録注記のE2Eテスト | JPYC決済は未実装、実送金の証明はない |
| 透明性ダッシュボード | `build_mod_stats` の空/同数/定足数未達/月境界、認可（未認証401・未設定/未登録/BAN済み403）、個人情報非含有、実DB集計を `server/src/tests.rs` で検証。公開集計ルートは MOD-13 の5件で検証。E2E は `e2e/tests/moderation.spec.ts` で表示と403を確認（R5-09） | 運用者限定の認証内集計。公開範囲は [ADR-0008](adr/ADR-0008-moderation-stats-publication.md) で決定済み（認証済み利用者向け公開・未認証は401）。段階2の公開ルート `GET /mod/stats/public` は実装済み（MOD-13、運用者認可なし）。クライアントは初期表示で `/mod/stats` を0本とし、オプイン押下時のみプローブする。403 は一般画面へ描画しない。`quorum_not_met` は `totals` のみで月次表には含めない |
| 通報理由の要約 | `common::summarize_reason` の単体テスト（空・119/120/121 Unicode文字境界・日本語・`@` を含むトークン／`://` を含むトークン／`www.` で始まるトークン／IPv4 らしいドット区切り／Unicode 数字4連続（全角含む）の `[redacted]` 置換、4桁ちょうど境界）と `GET /mod/reports` の統合テスト（要約値のみ返却、全文・PII 非露出、認証必須・Reported 限定・新着順・運用者非限定の維持） | DB 全文と監査エクスポートの本文は変更しない（EXPORT-04 のマスクは従来どおり）。第三者公開範囲は [ADR-0008](adr/ADR-0008-moderation-stats-publication.md) で決定済み（要約は集計レスポンスへ含めない） |
| データエクスポート | `GET /me/export` の全12表網羅・他ユーザー非漏えい・空ユーザー・機微項目除外・`reason`/`details` マスク・未認証401・ヘッダを `server/src/tests.rs` の `me_export_*` で検証（R5-04）。全12表が単一の読み取りトランザクション内で読み切られること（`debug_assert!`）と、別接続の未コミット BAN 書き込みに対して書き込み前の単一時点スナップショットを返すことを2件で検証。E2E は `e2e/tests/export.spec.ts` の定義3件でダウンロード・401表示・ダウンロード開始失敗を確認（chromium で3/3 pass） | 削除権（erase）は未実装。法務・外部アンカー・バックアップ消去方針の決定後に着手（詳細設計「9. データ主体の権利」） |
| 監査CLI | `sign_audit_export` の鍵権限検査（0600/0400・親ディレクトリ0700）・`DATABASE_TOKEN` 環境変数限定（`--database-token` 拒否）・出力 `create_new` による上書き拒否を下記 AUDIT-02 で仕様化。検証系CLIは下記 CLI-01〜03 を参照 | 実装は完了。`sign_audit_export` は bin 内に17テスト（鍵権限・トークン・上書き拒否10件＋include_reasons本番ガード2件＋DB接続先必須化5件）、`doctor` 53件（既存29件＋境界テスト7件＋OIDC URL検証3件＋oidc_required_assurance1件＋設定検証3件＋ヘルプ優先2件＋AUDIT_EXPECT_ROOT1件＋AGE_POLICY_CONFIGURED3件＋バックアップ/TZ診断3件＋バックアップ生値判定1件） / `verify_audit_export` 20件 / `verify_backup` 19件の bin テストを追加済み。匿名化・外部アンカーは未実装 |
| 監査一覧API | `GET /content/{cid}/audit` が最新 MAX_AUDIT_LIST(500) 件を `log_id DESC` で返すこと、FORENSIC_TIMELINE がその順序で描画されることを仕様化（AUDIT-04） | 実装済み。サーバー側 `get_content_audit_caps_at_max_audit_list`（501件投入で先頭501・末尾2・降順）。`previous_log_hash` 非含有・`reason` マスクのためレスポンス単体の連鎖検証は不可。完全検証は署名付きエクスポート/DB（AUDIT-01） |
| 性質テスト（proptest） | `server` の `property_tests` 4件（金額換算・CID 形式・理由要約の境界と冪等・Unicode 文字境界）、各128ケース。`proptest` は `server/Cargo.toml` の dev-dependencies（1.12）。年齢判定の proptest は認証行を参照 | 実JPKI実機・実チェーンとの突き合わせは未実施 |
| 直近の未コミット変更の反映（2026-11-18） | 監査タイムラインの `reason` 要約と `entry_hash` 生値維持1件、投票署名 v2 の4件（全フィールド束縛・正常系受理・旧 v1 署名拒否・別投票者向け署名の転用拒否）、一覧の `price` NULL を0として返す1件、エクスポート解析失敗時の固定文言表示1件 | サーバー側6件は `server/src/tests.rs`、クライアント側1件は `client/src/lib.rs`。E2E 定義数は当時59件のまま（現在69件）。対応ゲートは AUDIT-03 / MOD-12 / CONTENT-10 / EXPORT-11 |

SHA-256 の算出は旧 `sha256` 便宜 crate を廃止し、`server/src/lib.rs` の `sha256_hex`（`sha2` + `hex`）へ統一した。出力は小文字 64-hex で不変であり、ハッシュ値を照合する上記テストの期待値・契約は変更していない。

### 認証トークン失効時の回復経路（有効化済み・単一フライトで競合解決・69件維持）

`client/src/lib.rs` の `api_fetch` / `api_fetch_request` は共有 JWT signal から試行ごとにトークンを読み、
401 のときだけ `/auth/refresh` を単一フライトで1回呼び、新トークンで1回だけ再送する基盤を持つ。
refresh 失敗時は元の 401 Response をそのまま返す。`auth/login` / `auth/verify` /
`metrics/client-events`（再帰防止・認証不要）と既存契約を維持する `auth/logout` は対象外。
ボディ付きの `/content/upload` / `/mod/report` / `/mod/vote` は `api_fetch_request` を使い、
FormData / JSON を試行ごとに作り直して再送する。

全 fetch へ自動適用した際の 401 固定文言ビューと一覧フェッチの再描画の競合は、
refresh の単一フライト化と失敗時の元 401 返却で解消済み。`ENABLE_AUTO_REFRESH=true` で
有効化済みで、E2E 回復フェーズは既存4テストの setup へ失敗側 refresh モックを追加して統合した。
auth は既存3件・全体69件を維持する。
成功する refresh → 1回再送 → 200 の回復フェーズを `export.spec.ts` の R5-04 401ケース
（既存 `test()`）内へ統合した。最初のクリックでは refresh を失敗側（401）のまま
固定文言「ログインが必要です」を検証し、既存アサーション完了後に route ハンドラ内フラグで
`authorized:true, user_hash:'mock_user_hash', token:'refreshed_jwt_token'` の200へ差し替えて
再クリックする。`export-error` 消滅・`export-status`（エクスポート完了）表示・
`refreshAttempts>=1`・`Bearer refreshed_jwt_token` での再送を同じ `test()` 内で確認する
（新規 `test()` なし、export3件・全体69件を維持）。

### 言語宣言方針とリリース時ログ方針（R2-LEGACY-LANG）

- **言語宣言**: `client/index.html` は `lang="ja"` を維持し、日本語が主言語であることを宣言する。
  外国人向けUIへ切り替えるのではなく、英語の固定ラベルを描画する要素へ個別に `lang="en"` を付与する。
  対象は `CONNECT WITH JPKI` / `BROWSE` / `UPLOAD` / `MODERATION` / `LEDGER` / `LOG OUT` /
  `SHOW STATS` / `SUBMIT FORENSIC REPORT` / `DOWNLOAD ALL OF YOUR DATA AS JSON` 等である。
  これによりスクリーンリーダーの発音と言語宣言が整合する。ボタンの `textContent` と
  `data-testid` は不変で、E2E は `e2e/tests/integrity.spec.ts` の既存 ARIA テスト内へ
  `lang="en"` のアサーションを追加するのみ（新規 `test()` なし、E2E69件を維持）。
- **リリース無ログ**: `client/src/lib.rs` の `EXPORT_RESULT` コンソール出力は
  `#[cfg(feature = "dev-console")]` でゲートし、`dev-console` を明示した開発ビルド以外では
  コンソールへ出力しない。エクスポート結果（件数・成功可否）や内部 URL・リクエストデータを
  リリース wasm のコンソールへ出さない方針であり、詳細設計のメトリクス方針（機微データ非送出）と整合する。
  あわせてサーバー側の旧プロジェクト名（コメント・起動ログ）を `openIllustration` へ統一し、
  意味の無いマーカーを削除する。いずれもテスト件数（E2E69件ほか）は不変である。
   `warn_history_update_once` は履歴警告オラクル（economy.spec.ts）維持のため
   リリース wasm でも固定文言 `HISTORY_UPDATE_WARNING` を1回だけ出力する
   （`EXPORT_RESULT` の dev-console ゲートとは方針が異なる）。
- **エラー文言の言語宣言**: 既存固定文言（`CONTENT_INDEX_*` / `Report failed.` /
  `認証に失敗しました (HTTP 500).` / `Network error updating favorite.` 等）は変更せず、
  英語文言を出す `role=alert` / `role=status` 要素へ `lang="en"` を、
  日本語文言へ `lang="ja"` を付与する。`client/src/lib.rs` の共通ヘルパ `error_lang`
  が favorite / upload / vote / content のエラー表示で判定を共有し、
  日本語文字（ひらがな・カタカナ・漢字）の有無で分類するため個別文言の追加でも drift しない。
  ユニットテストで英語・日本語それぞれの分類を確認する。
- **アクセシビリティの件数告知**: ダッシュボードの `role=status aria-live=polite` は
  `error.get().is_empty()` をゲートにし、成功時は従来の件数文言、エラー時は
  「コンテンツを取得できませんでした」を読み上げる。
  `e2e/tests/error-paths.spec.ts` の DASH-ERR 既存ケース内へ、
  エラー時に古い件数（`件のコンテンツを表示中`）が残らないアサーションを追加した
  （新規 `test()` なし、error-paths 6件・全体69件を維持）。
- **ファイル選択のタブ停止点**: `#file-upload` へ `tabindex="-1"` / `aria-hidden="true"`
  を付け、`label[for="file-upload"]` を唯一のタブ停止点として `focus-visible` 枠を付与する。
  `e2e/tests/upload.spec.ts` の既存キーボードケース内へ、ラベルから Tab 1回で価格入力へ
  移ることを統合検証した（新規 `test()` なし、upload 6件を維持）。

## 3. 必須要件のリリースゲート

| ID | ケース | 期待結果 | 状態 |
| --- | --- | --- | --- |
| AUTH-01 | `ALLOW_MOCK_AUTH` 未設定、`JWT_REQUIRE_KID` 有効 | モック認証とゼロ署名、`kid` 無しトークンを拒否 | 実装済み。`mock_token` とゼロ署名を拒否する回帰テストあり。`JWT_REQUIRE_KID=1` で kid 無し拒否・kid 付き受理、HS256 併用は起動拒否を実プロセスで確認。クライアント側は `OIL_MOCK_AUTH=0` ビルドでモック注記（`data-testid="mock-auth-notice"`）を出さず、`/auth/login` の501では固定文言 `本番認証は未実装です。` を出して `/auth/verify` を送らない（`auth.spec.ts` の既存 LOG OUT テスト内で検証、E2E 定義数69件不変） |
| AUTH-02 | 不正・期限切れJWT、`exp` 欠落トークン | 認証済みAPIが401を返す | 期限切れ（`exp` が過去）と `exp` 欠落トークンを手組みで署名し、保護 API が 401 を返す回帰テストを追加 |
| NET-01 | IPv4アクセスと開発ループバック例外 | 設定に応じて拒否/許可 | 一部実装済み |
| NET-02 | 信頼されないX-Forwarded-For | クライアント入力で接続元判定を迂回できない | 実装済み（IPv4-mapped 正規化、`TRUSTED_PROXY_CIDRS` 未設定時はピアアドレスのみで判定、信頼済みで解決不能時は403。回帰テストあり）。実プロキシ環境・多段構成の試験は要。`TRUSTED_PROXY_CIDRS` の読取は `common::env::read_env` に一本化し、非 UTF-8 は固定文言警告のうえ空範囲（fail-closed）。警告はキー名と理由のみで入力値をエコーしない。`run()` の起動ログも同じ分類結果を使用する |
| CONTENT-01 | 許可外形式、負価格、非整数価格、64bit範囲外価格、10 MiB 超本文 | 400で拒否 | 実装済み。有効な本文に不正値を1つだけ含めて拒否理由まで検証し、正常な価格境界（未指定=0、`0`、`MAX_ORDER_JPY`）の受理とDB保存も検証。拒否時に保存されないことを確認。上限超過（1,000,001 と `i64::MAX`）と 10 MiB 超 multipart 本文（Content-Length 検査ミドルウェアが 413 を返す）の拒否も検証 |
| 決済 CID/価格 | 注文作成時に CID 形式・コンテンツ存在・`Approved` 状態・提示価格と `amount_jpy` の一致を検証 | 不一致・不存在・レビュー中・不正 CID を拒否し、注文取得時に CID が保存される | 実装済み。実チェーン上で注文金額と実際の送金額を照合する検証は未実装（実RPC待ち） |
| CONTENT-02 | IPFS応答が10 MiB超過、遅延、切断 | 上限/タイムアウトを適用し、完全データとして返さない | 実装済み。10 MiB 超過とタイムアウト、次ゲートウェイへのフォールバック、ストリーム途中の切断（Partial ボディを返さない）をローカルモックで検証 |
| CONTENT-03 | SHA-256不一致 | コンテンツ描画を拒否 | 実装済み。checksum 一致で 200、不一致で 401、Illustration/Novel の Content-Type をローカルモックで検証 |
| CONTENT-05 | 通報中/却下コンテンツの直接 CID 取得 | 404 で配信しない | 実装済み。`status = 'Approved'` で絞り、Reported/Rejected が 404 になることを mutation テストで確認 |
| CONTENT-04 | 小説に `<script>` 等のraw HTML、`javascript:` / `data:` リンク・画像URL | raw HTMLを実行せず、危険なURLスキームを無効化 | 実装済み、回帰テストあり |
| CONTENT-06 | `title` / `description` / `tags` の上限超過（UTF-8 バイト数）とメタデータ重複フィールド | 400で拒否し、保存・一覧表示に反映しない | 実装済み。`title` 200 B、`description` 2000 B、`tags` 10個・各50 B の境界値と超過を検証（`test_upload_rejects_oversized_metadata` / `test_content_metadata_validation_bounds`）。省略・空文字は受理（`test_upload_accepts_optional_metadata`）。`tags` はカンマ区切り1フィールドで、空文字は空ベクトルとして保存。具体的な byte 境界は `title` が日本語66文字（198 B）受理／67文字（201 B）拒否、`description` が666文字（1998 B）受理／667文字（2001 B）拒否、各タグが16文字（48 B）受理／17文字（51 B）拒否、タグ個数が10個受理／11個拒否。クライアントは送信前ゲート `metadata_byte_error` でこれらを通信前に拒否し、HTTP リクエストを1本も送らない（HTML の `maxlength` は UTF-16 コードユニット単位のため日本語では上限を守れない）。境界回帰はクライアント側の既存 `#[cfg(test)]` モジュール内の単体テストで検証（E2E `test()` 69件は不変） |
| CONTENT-07 | `GET /content` の `q` / `tag` / `type` / `limit` / `offset` | 条件に一致する `Approved` のみを `created_at DESC` で返す | 実装済み。`q` は title/description の部分一致（OR）、`tag` はカンマ区切りタグの前方一致、`type` は完全一致（不正値は0件で400にしない）。範囲外・非数値の `limit` は50、負・非数値の `offset` は0へフォールバック（`test_list_contents_filters_by_query_tag_and_type` / `test_list_contents_limit_boundaries_and_offset` / `test_content_query_parser_defaults_and_bounds`） |
| CONTENT-08 | `q` / `tag` に `%` `_` `\` を含む入力、メタデータに HTML タグを含む値 | ワイルドカードとして解釈されず、描画時もスクリプトが実行されない | 実装済み。サーバーは `ESCAPE '\'` とバインドパラメータでリテラル照合（`test_like_escaping_keeps_wildcards_literal`、E2E は `test_like_wildcards_are_literal`）、クライアントは `escape_html_text` で `&` `<` `>` `"` `'` を実体参照化。サーバーは生テキストを保存・返却し、消毒は描画側の責務 |
| CONTENT-09 | チェックサム一致だが JXL として不正なバイト列（例: `not a jxl codestream`）を HTTP 200 で返すゲートウェイ | チェックサム検証は成功しても VERIFIED にせず、`× DECODE FAILED` と固定エラー文言を表示する | 実装済み。クライアントの `decode_error` 分岐で `set_verified = Some(false)` に切り替え、デコーダのエラー値は画面へ出さない。E2E は `e2e/tests/integrity.spec.ts` の `should not mark VERIFIED when checksum matches but JXL decoding fails (R1-13)` で検証。補助する単体テストは `verification_badge_distinguishes_decode_failure_from_checksum_mismatch` |
| INT-01 | フォレンジック完全性検証 E2E（正規／改ざん／デコード失敗／監査順序／Audit トグルの a11y） | 正規は `● VERIFIED`、改ざんは `× COMPROMISED`、デコード失敗は `× DECODE FAILED` を表示し、失敗時に VERIFIED を出さない。Audit トグルは `aria-expanded=false→true→false`、`aria-controls` が CID 由来の `forensic-timeline-<token>` で dialog の `id` と厳密一致し、`role="dialog"`（名前 `Forensic Timeline`）を満たし、Escape でトグルへフォーカスが復帰する | 実装済み。`integrity.spec.ts` の6件（R1-13 のデコード失敗、AUDIT-04 の FORENSIC_TIMELINE 順序、R2 の Audit トグル a11y と CLOSE ボタンのフォーカス復帰を追加）で検証。既存の genuine / tampered のセレクタ・文言は変更なし |
| A11Y-01 | Upload フォームのキーボード操作と価格入力の境界（R2） | ファイル選択 `label[for="file-upload"]` が `tabindex="0"` を持ち、フォーカス＋Enter で `filechooser` が発火する。`#upload-price` は `min="0"` / `max="1000000"` を満たし、範囲外は送信前ゲートで拒否される | 実装済み・検証済み: `e2e/tests/upload.spec.ts` の `R2: file-upload label is keyboard operable and price input has bounds` と価格境界シナリオ。upload.spec.ts 定義6件 |
| FAV-01 | お気に入りの追加/削除を繰り返す | 重複追加も未登録削除も200で成功し、行数は0または1のまま。削除後の再追加も可 | 実装済み（`test_favorite_add_and_remove_are_idempotent`） |
| FAV-02 | 未認証・不正CID・非Approvedコンテンツへの操作 | それぞれ401 / 400 / 404で拒否し、お気に入りを登録しない | 実装済み（`test_favorite_requires_auth_and_valid_approved_cid`） |
| FAV-03 | 他人のsubを本文に指定した操作 | 増減するのは操作者本人の行のみ。他人の行は変化しない | 実装済み（`test_favorite_uses_claims_sub_only`） |
| FAV-04 | `favorite_only` による絞り込み | 本人分のみを新着順で返す。既存条件とAND、ページング重複なし、非Approved除外、他値は絞り込みなし | 実装済み（`test_favorite_only_lists_own_favorites_only` / `test_favorite_only_parser_and_query_builder`） |
| FAV-05 | 旧JSONとの後方互換 | `is_favorite` 欠落の旧 `ContentMeta` が `false` として読め、現行JSONは往復一致する | 実装済み（`test_content_meta_backward_compatible_without_is_favorite`） |
| FAV-06 | migration 012 の制約 | `UNIQUE(user_hash, cid)` 違反はエラー、`INSERT OR IGNORE` は冪等、FK CASCADE で削除連動 | 実装済み（`test_migration_012_favorites_unique_and_cascade`） |
| MOD-01 | 同一ユーザーの同一通報への再投票 | 拒否 | 実装済み（API検査＋DB一意インデックス）、回帰テストあり |
| MOD-02 | 3人の異なる投票者による結果 | 規則通り状態遷移 | 実装済み（確定後の追加投票は400で拒否）、回帰テストあり |
| MOD-03 | レビュー中コンテンツへの重複通報 | 拒否 | 実装済み、回帰テストあり |
| MOD-04 | `/mod/stats` の集計（`build_mod_stats`） | 通報・確定・BAN/UNBAN/UNPIN の件数を月別に集計する。空入力は0件、同数は `Rejected`、定足数未達は `quorum_not_met` に計上し `approved` から除外、月境界は UTC の `YYYY-MM` で分割 | 実装済み・検証済み（対象テスト全12件 pass）: `test_build_mod_stats_empty` / `test_build_mod_stats_counts_rejected_status` / `test_build_mod_stats_quorum_not_met_excluded_from_approved` / `test_build_mod_stats_month_boundary_utc` / `test_mod_stats_quorum_excludes_banned_juror_votes`（BAN済み投票者の票を定足数へ算入しない）。同数→`Rejected` の実判定は `test_consensus_tie_resolves_to_rejected` が検証。実DBからの集計は `test_mod_stats_aggregates_from_real_db` |
| MOD-05 | `/mod/stats` と手動 finalize の認可 | `/mod/stats` は未認証401。`MODERATION_OPERATOR_HASHES` 未設定・未登録・BAN済みの主体は403。登録済み運用者は200。手動 `POST /mod/reports/{report_id}/finalize`（HTTP ハンドラ）も `require_moderation_operator` で運用者限定とし、未登録 sub は403。スケジューラ経路（`finalize_due_reports*` / `process_report_conclusion`）は認可対象外で自動確定を継続 | 実装済み・検証済み（`/mod/stats` 対象テスト全12件 pass）: `test_mod_stats_requires_authentication` / `test_mod_stats_forbidden_when_operator_not_configured` / `test_mod_stats_forbidden_when_operator_not_registered` / `test_mod_stats_forbidden_when_operator_banned`。手動 finalize は `test_finalize_is_noop_before_deadline` の既存1件内で運用者200と未登録subの403を検証（新規テスト関数なし、件数不変） |
| MOD-06 | `/mod/stats` のレスポンス | CID・reason・`user_hash` を含めず、`generated_at` / `totals` / `monthly` のホワイトリスト項目のみ返す。`quorum_not_met` は `totals` のみ | 実装済み・検証済み（対象テスト全12件 pass）: `test_mod_stats_response_contains_no_sensitive_fields` |
| MOD-07 | 透明性ダッシュボードのE2E（R5-09） | 初期表示は `/mod/stats` を0本とし、中立文言のボタン押下時にのみ1本送信する。200 応答のみを根拠に運用者パネルをマウントし、403 では一般画面へ権限エラーを描画せず黙って閉じる。不正 JSON ではパネル内へ `集計の解析に失敗しました。` を表示し、センチネル `RAW_SERDE_ERROR_SENTINEL_DO_NOT_SHOW` を出さない | 実装済み・検証済み: `e2e/tests/moderation.spec.ts` の `Moderation & Governance (GOV-01, GOV-02)` 内、`R5-09: should mount operator stats panel only on opt-in and stay silent for general users`。初期0本→オプイン200→403パネル内（否定オラクル）の3フェーズを既存1件で検証。testid: `moderation-stats-toggle` / `moderation-stats-optin`。chromium で pass（moderation.spec.ts 全14件、フルスイート定義数69件を維持。webkit はホスト側 ICU/libxml2 欠落で未検証） |
| MOD-08 | `common::summarize_reason` の境界・日本語・PII除去 | 空は空のまま。119/120 Unicode 文字は変更なし、121 文字は先頭119文字＋`…`。日本語も文字数で計数し同じ境界になる。`@` を含むトークン（メール）、`://` を含むトークン（括弧付き URL を含む）、`www.` で始まるトークン、IPv4 らしいドット区切り、Unicode 数字4連続（全角含む）は `[redacted]` に置換し、3桁の数字列は残る | 実装済み（`summarize_reason_redacts_pii` の4桁ちょうど境界と `summarize_reason_redacts_additional_pii_patterns`）。E2E オラクルは `e2e/tests/moderation.spec.ts` の `GOV-01 (summary): PII を含む元理由は要約済み文字列のみ表示され、メール/電話/URL は露出しない`（下記「GOV-01 要約表示の E2E」）。通報理由の送信時バイト契約（2000 B）も本ゲートで扱う: 日本語666文字（1998 B）は受理、667文字（2001 B）は拒否。クライアントは `submit_report` の `validate_report_input` 直後に `report_reason_byte_error` を検査し、固定文言を validationError（`role="alert"`）へ set して早期 return するため、超過入力は通信前に拒否され HTTP リクエストは0本。これは要約の120 Unicode 文字上限（文字数）とは別の UTF-8 バイト上限であり、`maxlength` は UTF-16 単位のため守れない。境界回帰はクライアント側の既存 `#[cfg(test)]` モジュール内の単体テストへ統合し、E2E `test()` 69件は不変 |
| MOD-09 | `GET /mod/reports` への要約適用と全文非漏えい | `reason` は要約後の値のみ。全文・メール・URL・4桁以上の数字列がレスポンスへ現れない。DB 全文と監査エクスポートは不変。認証必須・Reported 限定・新着順・運用者非限定を維持 | 実装済み（`server` に統合テストを追加。第三者公開範囲は [ADR-0008](adr/ADR-0008-moderation-stats-publication.md) で決定済み）。E2E ではモック `/mod/reports` に要約済み理由 `不審な投稿です [redacted]` を返し、`[redacted]` は表示、メール `reporter@example.com` / 電話 `090-1234-5678` / URL `https://phish.example.com/login` は `body` 未含有を検証（下記「GOV-01 要約表示の E2E」） |
| MOD-10 | 投票結果のフィードバック表示（VOTE-FB-01〜03） | `/mod/vote` の失敗時に `data-testid="vote-error"`（`role="alert"`）へ固定文言 `投票に失敗しました (HTTP <status>).` または `Network error submitting vote.` を表示する。成功時は `data-testid="vote-notice"`（`role="status"`）に `投票を記録しました。`。新しい試行で両方クリアし、サーバー本文・生エラーは表示しない | 実装済み・検証済み: `e2e/tests/moderation.spec.ts` の VOTE-FB-01〜03（403 / network error / 成功とクリア。chromium で3件 pass、moderation.spec.ts は VOTE-FB 3件・二重送信ガード2件を含め全14件 pass）。クライアント側単体テスト `vote_feedback_uses_only_fixed_phrases` も追加済み |
| MOD-11 | 投票一覧取得失敗の表示（REPORTS-ERR-01〜02） | `GET /mod/reports` の失敗時に `data-testid="reports-error"`（`role="alert"`）へ固定文言を表示する。HTTP エラーは `投票一覧の取得に失敗しました (HTTP <status>).`、ネットワークエラーは `Network error loading reports.`、JSON 解析失敗は `投票一覧の解析に失敗しました。`。失敗しても既存データは維持し、`NO_PENDING_VOTES` を誤表示しない。成功時にエラーをクリアし、サーバー本文・生エラーは表示しない | 実装済み・検証済み: `e2e/tests/moderation.spec.ts` の REPORTS-ERR-01（500）/ 02（route.abort）が chromium で2件 pass、moderation.spec.ts 全14件 pass。クライアント側単体テスト `reports_error_uses_only_fixed_phrases` も追加済み。REPORTS-ERR-01 の既存1件内へ、ステータス200だがフィールド型が壊れた JSON（`report_id` が文字列・`reason` が数値）を返すフェーズを追加し、`reports-error` の固定文言 `投票一覧の解析に失敗しました。`・サーバー本文センチネル非表示・`NO_PENDING_VOTES` 非表示を検証（新規 `test()` なし、moderation14件・全体69件を維持） |
| MOD-13 | `GET /mod/stats/public`（ADR-0008 段階2の公開集計） | 未認証は401。認証済みは運用者未設定・未登録・BAN済みでも200。レスポンスは `generated_at` / `totals`（`reports` / `concluded` / `approved` / `rejected` / `quorum_not_met`）/ `monthly`（`month` と reports / concluded / approved / rejected）のみで、bans / unbans / unpins と CID・reason・`user_hash` は型に存在しない。`require_moderation_operator` を呼ばない | 実装済み（3表の読み取りは `BEGIN DEFERRED` 単一スナップショット、`/mod/stats` と同一の読み取りトランザクション）。サーバー側テスト5件: `test_public_mod_stats_requires_authentication` / `test_public_mod_stats_allows_general_sub` / `test_public_mod_stats_allows_operator_sub` / `test_public_mod_stats_response_contains_no_forbidden_fields` / `test_build_public_mod_stats_matches_source`。E2E 定義数は69件のまま変更なし（新規 E2E なし）。既存ゲート（E2E69件 / check23 / doctor53 / migrations14）は不変 |
| EXPORT-01 | `GET /me/export` の全表網羅と他ユーザー非漏えい | `version:1` / `generated_at` / `sub` と12表の配列を返す。他ユーザーの行・IDは混入しない | テスト名確定: `me_export_returns_only_own_data`。12表は `users` / `contents` / `favorites` / `transactions` / `signatures` / `content_audit_log` / `reports` / `votes` / `payment_orders` / `governance_audit_log` / `refresh_tokens` / `revoked_subs`（`revoked_sessions` は除外） |
| EXPORT-02 | データを持たないユーザーのエクスポート | 12表の配列はすべて空で、メタ3項目（`version` / `generated_at` / `sub`）は通常どおり返る | テスト名確定: `me_export_empty_user_has_empty_arrays` |
| EXPORT-03 | 機微項目の除外 | `token_hash` / `sid` / `family_id` / 署名鍵 / XPRV / `PINATA_JWT` / 他ユーザーID がJSONに現れない | テスト名確定: `me_export_excludes_sensitive_fields` |
| EXPORT-04 | `reason` / `details` のマスク | `mask_reason` により既定でマスクされ、元の本文が現れない（例: `[redacted:N bytes]`） | テスト名確定: `me_export_masks_reason_and_details` |
| EXPORT-05 | 未認証でのエクスポート要求 | 401で拒否し、データを返さない | テスト名確定: `me_export_requires_auth` |
| EXPORT-06 | エクスポートのレスポンスヘッダとログ非出力 | `Content-Disposition: attachment; filename="openillustration-export.json"` と `Cache-Control: no-store` を付与。本文はログへ出さない | テスト名確定: `me_export_headers`。行数上限・`truncated` フラグは設けず全行を返す |
| EXPORT-07 | エクスポートのE2E（R5-04） | 認証トークン付きで `GET /me/export` を呼ぶとJSONダウンロードがトリガされる。401では `role="alert"` に「ログインが必要です」が表示される。ダウンロード開始が例外を投げた場合は `data-testid="export-error"`（`role="alert"`）に固定文言 `ダウンロードの開始に失敗しました。` を出し、`export-status` は出さない（成功文言も出さない）。これは EXPORT-11 の「レスポンス解析失敗」とは別事象 | 実装済み（`e2e/tests/export.spec.ts` 定義3件。chromium で pass を確認済み。webkit はホスト側 libicu/libxml2 不在で起動不可、環境依存）: `R5-04: GET /me/export を認証トークン付きで呼び、JSON ダウンロードがトリガされること` / `R5-04: /me/export が 401 の場合は「ログインが必要です」を role=alert で表示すること` / `R5-04: click が例外を投げた場合は固定文言のエラーを表示し、完了文言を出さないこと`。固定文言は `client/src/lib.rs:753` の確定値 |
| EXPORT-08 | 削除権（erase）の不在 | 削除要求を受け付けるAPIが存在しない | 未実装（設計で明記）。エクスポートのみ実装。法務・外部アンカー・バックアップ消去方針の決定後に着手 |
| EXPORT-09 | `/me/export` の読み取りトランザクション | 12表の全クエリが単一の読み取りトランザクション（`BEGIN DEFERRED`）で実行され、`collect_export_snapshot` の `debug_assert!(!db.is_autocommit())` が発火しない。完了時に `tx.commit()`、エラー時は Drop が ROLLBACK | 実装済み。`me_export_queries_run_inside_read_transaction`（`db.transaction()` / `tx.commit()` を外す mutation で失敗）と、並行書き込み待ち（直列化）を検証する `me_export_waits_for_concurrent_write_transaction` を EXPORT-09 の一部として含む。下記「エクスポートの読み取りトランザクション検証」を参照 |
| EXPORT-10 | 並行 BAN 書き込み下の単一時点スナップショット | 別接続が `BEGIN IMMEDIATE` で BAN 更新と監査ログ挿入を未コミットのまま保持しても、エクスポートは書き込み前の `users` / `governance_audit_log` を一貫して返す | 実装済み。`me_export_snapshot_consistent_under_concurrent_ban_write`（一時 SQLite ファイル・2接続）。E2E 定義数は当時59件のまま（現在69件） |
| SUPPORT-01 | 無効化済み支援APIへのPOST | 501で拒否（未認証は401）、模擬取引を記録しない | 実装済み、JPYC機能への置換要 |
| ECO-01 | 取引履歴の取得失敗表示（Ledger E2E） | `GET /transactions` の失敗時に `data-testid="ledger-error"`（`role="alert"`）へ固定文言 `取引履歴の取得に失敗しました。` を表示し、`NO_TRANSACTIONS_RECORDED` を出さない。サーバー本文は表示しない | 実装済み・検証済み: `e2e/tests/economy.spec.ts` の HTTP 500 / `route.abort()` の2件が chromium で pass（固定文言は `client/src/lib.rs` の `LEDGER_ERROR`）。下記「E2E 取引履歴エラーシナリオ（ECO-01）」を参照 |
| ID-01 | JPKI提示、署名、nonce、信頼元、失効、`acr`/`amr` 保証レベル、`auth_time`/`max_age` の検証 | 有効な本人確認だけを受理 | OIDC認証フローをモックOPで検証。`OIDC_REQUIRED_ACR`/`AMR` 設定時に満たさないトークンを401で拒否、`OIDC_MAX_AUTH_AGE_SECS` 設定時に認証要求へ `max_age` を付与し新鮮な `auth_time` を受理・古い値と欠落を401拒否（fail-closed）することを回帰テスト化。`OIDC_MAX_AUTH_AGE_SECS` は上限 `MAX_AUTH_AGE_MAX_SECS = 2_592_000`（30日）を超える値（`u64::MAX` 境界を含む）も固定文言のハードエラーで起動拒否し、設定値をエコーしない（fail-closed）。R1-19 の nbf 検証（`validate_nbf`）は `test_oidc_rejects_future_nbf`（未来 nbf は401拒否）/`test_oidc_accepts_missing_nbf`（クレーム欠落は受理）/`test_oidc_accepts_nbf_within_clock_skew`（60秒の clock skew 内は受理）の3件で検証。`server/src/jpki.rs` の SEC-02 証明書エラーパス10件（既存の `invalid_base64_token_is_unauthorized` / `non_x509_bytes_are_rejected` / `expired_certificate_is_rejected` / `envelope_without_credentials_falls_back_to_raw_token` / `empty_token_is_rejected` に、自己署名拒否（DN 一致 + 暗号学的検証）2件 `rejects_self_signed_rcgen_certificate` / `rejects_self_signed_rsa_2048_certificate`、CA署名 ECDSA 拒否 `rejects_ca_signed_ecdsa_certificate`、鍵ポリシー境界 `public_key_policy_requires_rsa_2048`、issuer DN 迂回検知 `rejects_leaf_signed_by_issuer_key_with_different_dn` を追加）を検証。CA署名 RSA 2048 の受理は `accepts_ca_signed_rsa_2048_certificate` で正常系対照を確保。Unauthorized 系はいずれも `AppError::Unauthorized` と固定メッセージ（Invalid JPKI presentation token / X.509 parse failed / Certificate is expired or not yet valid / Self-signed JPKI certificate is not trusted / JPKI leaf public key must be RSA 2048 bits or stronger）を検証。下記「JPKI 証明書エラーパスの検証（ID-01）」を参照。実JPKI実機・失効確認は未検証 |
| ID-02 | JPKI提示拒否・失効・期限切れ・再利用 | 保護機能を拒否 | state再利用とnonce不一致を検証。back-channel logout は `events` 欠落と `sid`/`sub` 両方欠落を401拒否。R2-LOGOUT-TOKEN では logout token の新鮮性（`validate_nbf = true` で未来 nbf を401拒否、`iat > now + clock_skew_secs` も401拒否、nbf 欠落は受理）を `test_backchannel_logout_rejects_future_iat_and_nbf` で検証する。R1-23 の pending 登録上限（`MAX_PENDING_AUTH`）は `test_oidc_pending_limit_rejects_excess_registration`（上限超過は429で拒否し保存しない）/`test_oidc_start_returns_429_when_pending_full`（上限到達時に `/auth/oidc/start` が429）/`test_oidc_pending_limit_purges_expired_entries`（期限切れエントリを掃除して空きを作る）/`test_oidc_pending_limit_keeps_start_callback_flow`（上限近くでも start/callback フローが成立）の4件で検証。リフレッシュトークンの再利用検知は、消費 UPDATE の変更0件を tx ロールバック後に別tx のファミリ失効へつなぐ単一 tx 構成で担保し、`test_refresh_reuse_revokes_family` がファミリ全体の失効を検証する。実JPKIの失効・期限切れは未検証 |
| ID-03 | 認証トランザクションとJWT主体の結合 | 別セッション・クライアント申告IDで主体を偽装できない | PPIDをセッションJWTに結合し、`/auth/logout` の `sid` 失効、back-channel logout の `sid`/`sub` 失効（`sub` は logout token の `iat` を閾値として DB とメモリへ記録し、再認証で新セッション発行可）を検証。R2-LOGOUT-TOKEN の再送冪等は `test_backchannel_logout_replay_keeps_revoked_at_and_reauthenticated_session`（同一 token の再送で `revoked_at` 不変・再認証セッション200維持）と `test_backchannel_logout_stale_iat_does_not_advance_threshold`（古い `iat` の再送で閾値を前進させない）で検証する。R1-18 のログアウト削除 Cookie は `test_logout_clears_refresh_cookie`（空値・`Path=/auth`・`HttpOnly`・`SameSite=Strict`・`Max-Age=0`、提示トークン非エコー、本文 `true` 維持）と `test_logout_deletion_cookie_follows_oidc_cookie_secure`（`OIDC_COOKIE_SECURE` が `0` / `false` では Secure なし、`1` / 未設定では Secure 付き）で検証。リフレッシュの原子的回転（`issue_refresh_token_in_tx`）により旧トークンは消費済み・新トークンは未使用で登録され、`test_refresh_rotates_token_and_issues_new_session` が新旧の `used_at` を検証する。下記「ログアウト削除 Cookie の検証（ID-03）」を参照。永続化・実OP連携は未検証 |
| ID-04 | 未認証での閲覧APIアクセス | コンテンツ・監査等を拒否。認証開始・health・非秘密の公開ポリシー情報（`GET /policy/age`）は許可 | JWT拒否実装。OIDC実サービスは未接続 |
| AGE-01 | 年齢ポリシー設定と公開 | 不正な `AGE_*` 設定は起動失敗（fail closed）。`GET /policy/age` は認証不要で有効設定値（非秘密）と `settings_always_public=true` を返す | 実装済み（`common`/`server` テスト）。`GET /policy/age` に秘密情報が含まれないことは設計/レビューで継続確認 |
| AGE-02 | 明示設定必須・信頼属性必須モード | `AGE_REQUIRE_EXPLICIT_CONFIGURATION` 未充足時および `trusted_attribute_required` は拒否（フェイルクローズ）。設定チェックは `/auth/verify` と OIDC callback / refresh の共通ゲートで行う | 実装済み（現行入力は申告値）。検証済み属性入力経路は未実装。必須設定値が未充足（`configuration_satisfied() == false`）のときの全経路拒否は AGE-03 で検証 |
| AGE-03 | OIDC callback / refresh の設定必須ゲート（R1-17） | `AGE_REQUIRE_EXPLICIT_CONFIGURATION=1` かつ必須設定値が未充足（`default_minimum_age=21` ・`country_minimums` 空・`target_regions` 空）のとき、callback と refresh は state / Cookie の消費前に 403（`ConfigurationRequired`）。`trusted_attribute_required` も 403（`TrustedAttributeRequired`） | 実装済み。`test_require_explicit_configuration_blocks_oidc_callback_and_refresh` で両経路の 403 を検証。下記「年齢ゲート共通化と TZ 境界の検証」を参照。リフレッシュの原子化後も年齢ゲート共通ゲートは `BEGIN` より前に実行され、BAN チェックと claims/JWT 事前計算も `BEGIN` 前に配置して write_lock 保持中の待ちを避ける |
| AGE-04 | 年齢基準日の TZ ローカル化 | `TZ` の `Asia/Tokyo`（UTC+9）/ `Etc/GMT±N`（POSIX 符号反転）/ 未設定・未対応名（UTC フォールバック）で「今日」が変わり、年齢境界が1日ずれる。UTC 2024-01-01T16:00Z の JST は翌日 | 実装済み。`test_today_for_policy_uses_configured_local_timezone` と `test_validate_age_at_uses_local_today_for_age_boundary` の2件。下記「年齢ゲート共通化と TZ 境界の検証」を参照。`TZ` 読取は `configured_tz()`（`common::env::read_env` ベース）へ一本化し、非 UTF-8 は UTC フォールバックのまま doctor の `classify(lookup("TZ"))` 固定文言警告対象とする（生バイト非保持・非エコー） |
| AGE-05 | 署名付き年齢属性 adapter の正常系（DEC-01 / ADR-0007） | `verify(issuer, claims, nonce, audience)` が許可リスト完全一致・発行者束縛・audience 束縛・nonce 束縛を通した場合のみ `VerifiedAge` を返す。mock 経路では自己申告の成人判定を上書きして検証済み生年月日で判定。`verified_claims` 経路の `user_hash` は issuer + signed_subject 由来 | 実装済み。`verified_age.rs` の `verify_accepts_valid_claims` と `test_verified_age_accepts_valid_signed_attribute_in_mock_path`（hash 契約は AGE-12） |
| AGE-06 | 自己申告混入拒否 | `birth_date` / `country_code` を `verified_claims` へ混ぜた JSON は `deny_unknown_fields` で拒否。信頼属性必須モードで `verified_claims` 欠落時は自己申告が成人でも 403 | 実装済み。`self_reported_fields_cannot_be_injected_into_claims` と `test_verified_age_rejects_self_reported_fields_and_missing_claims` |
| AGE-07 | nonce 再利用拒否 | 既存 `MOCK_NONCES.remove()` の単回消費を再利用し、2回目の提示は 400 | 実装済み。`test_verified_age_rejects_reused_nonce`、`verify_rejects_nonce_mismatch` |
| AGE-08 | 未許可 issuer 拒否 | 許可リストに無い発行者、空リスト、発行者束縛不一致は 403 | 実装済み。`verify_rejects_untrusted_issuer`、`verify_rejects_issuer_mismatch_between_argument_and_attestation`、`test_verified_age_rejects_untrusted_issuer` |
| AGE-09 | audience 不一致拒否 | `claims.audience` が引数 audience（OIDC client_id フォールバック）と不一致なら 403 | 実装済み。`verify_rejects_audience_mismatch`、`test_verified_age_rejects_audience_mismatch` |
| AGE-10 | 発行者許可リスト設定（非秘密） | `AGE_TRUSTED_ISSUERS` は trim・空要素除去・完全一致。信頼属性必須モードで空なら起動拒否。既定モードは自動昇格しない | 実装済み（`common/src/age.rs`）。`from_env_with_trims_deduplicates_trusted_issuers`、`trusted_issuer_matching_is_exact`、`trusted_attribute_mode_without_trusted_issuers_fails_closed`、`self_reported_mode_does_not_require_trusted_issuers` |
| AGE-11 | mock 無効時の verified_claims 拒否・OIDC callback の adapter 境界 | `allow_mock_auth` 無効では `verified_claims` を 403。callback は `ensure_age_policy_configured` 直後に adapter 境界を呼び、信頼属性必須モードでは fail closed（認可フローは変更しない） | 実装済み。`test_verified_age_claims_rejected_when_mock_auth_disabled`、`oidc_binding_keeps_self_reported_mode_and_fails_closed_on_trusted_mode` |
| AGE-12 | verified_claims 経路の user_hash 導出（R2-USERHASH） | `sha256("verified-age:v1:{issuer}:{signed_subject}")`（64-hex）。`birthdate` / `country` / `nonce` は不使用。旧 `sha256("2000-01-01:JP")` とは異なる。signed_subject 変更で hash が変わる。空 subject は 403。OIDC callback の `sub` 生値は別契約で変更なし | 実装済み。`test_verified_age_accepts_valid_signed_attribute_in_mock_path` の (a)(b)(c) と `verify_rejects_missing_or_invalid_age_claim` の empty_subject で検証。旧 hash の開発 DB は作り直し前提 |
| PAY-01 | 対応JPYCの有効な送金と最終性確認 | 確認後のみ決済成立 | 基盤実装済み。モックverifierで注文作成・ウォレット結合・検証・確定、BIP44受取アドレス派生を検証。金額・確認数・署名復元・期限の境界5件は下記「決済ユーティリティの境界の検証（PAY-01〜03）」を参照。R1-26 の並行注文は `test_parallel_orders_derive_sequential_indices_and_unique_addresses`（連番の `derivation_index` と一意な受取先）/`test_identical_parallel_orders_all_succeed`（同一内容の並行注文5件が全件成功）/`test_fixed_receiving_address_keeps_derivation_index_null`（固定受取アドレスは index NULL を維持）/`test_rejected_orders_do_not_consume_derivation_index`（拒否注文は index を消費しない）の4件で検証。実チェーンRPCは未実装 |
| PAY-02 | 非対応トークン、チェーン、再編成、二重通知 | 不成立または確認中として扱う方針（未実装） | 一部実装。未対応チェーン、金額・送金元・受取先の不一致、同一tx再利用を検証。再編成・二重通知・別コントラクトは未実装 |
| PAY-03 | 取引要求ID・金額・受取先との不一致 | 有償機能を解放しない | 基盤実装済み。注文ID・金額・受取先・送金元の照合を検証 |
| AUDIT-01 | 監査記録の改変・欠落・順序入替 | ハッシュ連鎖・連番・Merkle root で改変を検出 | ローカルDBのハッシュ連鎖（v2 正規化＋v1 互換検証）・`log_id` 連続性・Merkle root と `--expect-root` による切詰め検出を実装。エントリハッシュが全内容フィールドに依存することを mutation テストで確認（`log_id` は連続性/Merkle root 側でカバー）。署名付きスナップショット（version 3）がコンテンツ監査と BAN/UNBAN/UNPIN 監査の両方を含み、改ざんで検証失敗することを単体テストと実DBで確認。暗号エラーパス12件（署名・DER検証8件と `verify_chain_*` 4件）は下記「暗号検証エラーパスの検証（AUDIT-01）」を参照。CLI 契約（鍵権限・トークン・上書き拒否）は下記 AUDIT-02 を参照。外部アンカー・HSM 鍵管理は未実装、必須。`verify_chain` は `reasons_masked=true` のとき全 entry の `reason` がマスク済み（`reason_is_masked`）であることを検証し、平文が残っていれば拒否する。署名対象 `signature_payload()` はマスク版で entries_json を `masked` 印付きで含むため、この検査は署名対象の整合確認であり既存の verify 期待値は不変 |
| AUDIT-02 | `sign_audit_export` の鍵権限・トークン受け渡し・出力上書き | 鍵 PEM は owner-only（0600/0400）を要求し、親ディレクトリの group/other 書き込みを拒否（0700 以外は警告）。リモート DB トークンは `DATABASE_TOKEN` 環境変数からのみ読み、`--database-token` はエラー。出力は `create_new` で既存ファイルを上書きしない | 実装済み（下記「監査CLIの権限・トークン・上書き拒否の検証（AUDIT-02）」にテスト設計を記載）。`server/src/bin/sign_audit_export.rs` に17テスト（既存10件＋include_reasons本番ガード2件＋DB接続先必須化5件）を追加済み。Runbookの手動確認手順も併用する |
| PERF-01 | 初期表示、API、転送量、Wasmサイズ、手数料 | 合意済みSLO内 | status=not_evaluated（未作成、ADR-0011 で合意待ち。下記「性能・互換ゲートの評価状況」を参照） |
| COMPAT-01 | 対応OS/ブラウザ/端末で閲覧・投稿・認証・決済 | 対応マトリクスを満たし、安全なフォールバックを表示 | status=not_evaluated（未作成、ADR-0012 で合意待ち。下同節を参照） |
| COMPAT-02 | JXL/Wasm非対応・機能利用不可環境 | 代替表示または案内。認証・決済は迂回不可 | status=not_evaluated（未作成、ADR-0012 で合意待ち。下同節を参照） |
| CFG-01 | `doctor` の設定不整合警告（R5-06 / R4-01） | `ALLOW_MOCK_AUTH` と PINATA_JWT / `OIDC_*` / RSA 秘密鍵の併用で警告。既定は警告のみで exit 0、`--strict` では exit 1。5パターンの実出力を確認 | 実装済み。純粋関数化と53テスト（既存29件＋境界テスト7件＋OIDC URL検証3件＋oidc_required_assurance1件＋設定検証3件＋ヘルプ優先2件＋AUDIT_EXPECT_ROOT1件＋AGE_POLICY_CONFIGURED3件＋バックアップ/TZ診断3件＋バックアップ生値判定1件）を追加し、23カテゴリ固定を検証。`ALLOW_IPV4_LOCALHOST` / `TRUSTED_PROXY_CIDRS` も警告対象。`AUDIT_EXPECT_ROOT` は本番相当で未設定/空白/不正 hex なら `audit_expect_root` に警告（値は非エコー）。下記「設定診断（`doctor`）の検証」および CLI-01 を参照 |
| NET-03 | レート制限キーの解決済みクライアント IP 優先 | 信頼プロキシ配下では XFF 解決 IP ごとに独立バケット。未設定・信頼外ではピア IP をキーにする | 実装済み。`test_rate_limit_key_prefers_resolved_client_ip` / `test_rate_limit_uses_resolved_client_ip_under_trusted_proxy` / `test_rate_limit_key_falls_back_to_peer_without_trusted_proxy` |
| NET-04 | `MAX_CONCURRENT_REQUESTS` の境界と同時実行上限（R4-02） | 未設定は既定16、有効範囲は 1..=256。`0` / `-1` / 空 / 非数値 / `257` / 極端な過大値は上限へクランプせず既定16へフォールバック。同時実行上限1では1件目保持中に2件目が429で、permit 返却後の再リクエストは200 | 実装済み。`test_max_concurrent_requests_defaults_and_bounds`（境界）と `test_concurrent_request_limit_rejects_second_and_releases_permit`（`Notify` で1件目を保持し permit 0 → 2件目429 → 解放 → permit 1 → 3件目200、レート制限は100/min で切り分け）。下記「同時リクエスト上限の検証（NET-04）」を参照 |
| NET-05 | SSRF: IPFSゲートウェイのリダイレクト追従拒否 | 302 + `Location` で内部サービスへ誘導されても追従せず、リダイレクト先へ到達しない。3xx はエラーとして次のゲートウェイへフェイルオーバーする | 実装済み。`build_http_client` が `Policy::none()` を設定。サーバー側テスト2件（`test_get_content_does_not_follow_gateway_redirect` / `test_get_content_redirect_falls_back_to_next_gateway`）。下記「SSRF リダイレクト無効化の検証（NET-05）」を参照 |
| NET-06 | XFF複数値解釈と IPv4-compatible 形式の拒否 | 追記型は末尾ピアを除去して最左クライアントを採用、上書き型は末尾を採用、不正エントリは fail-closed。`::a.b.c.d` は IPv4 として拒否、`::` / `::1` は IPv6 を維持 | 実装済み。サーバー側テスト11件（`server/src/network.rs` 6件 + `server/src/tests.rs` 5件）。下記「XFF 複数値・IPv4互換形式の検証（NET-06）」を参照 |
| PAY-04 | `verify_order` の CAS 2段目と `bind_wallet` の CAS | 確定処理の2段目 UPDATE が `changes() == 0` なら 409 を返し、成功応答しない。`bind_wallet` も tx 内で `status` / `expires_at` を読み直し、`status = 'PENDING'` を条件に `wallet_bound = 1` を更新して 0 件更新なら 409 | 実装済み（`server/src/payments.rs`）。`bind_wallet` は競合時に固定文言 `order status changed during wallet binding` を返し、`verify_order` と同じ単一ライター契約を満たす。下記「CAS 2段目と失効閾値レースの検証」を参照 |
| PAY-05 | ウォレット bind 署名 v2 と旧形式拒否（盲署名対策） | 署名対象は `order_id` / `chain_id` / `wallet_address` / `recipient_address` / `amount_jpy`。注文IDのみを署名対象にした旧形式は 401 で拒否する | 実装済み。`wallet_bind_message_v2` と `amount_jpy_from_wei` を追加し、`bind_wallet` を v2 へ切替。`test_bind_wallet_rejects_legacy_order_only_signature`（旧形式は `UNAUTHORIZED`）を追加。既存の bind 回帰はhelperを v2 署名へ更新。下記「ウォレット bind 署名 v2 の検証（PAY-05）」を参照 |
| ID-05 | 再認証時の失効閾値レース | 古い閾値以下の行だけを削除し、back-channel logout が登録した新しい失効を残す | 実装済み（`server/src/oidc.rs`）。R2-LOGOUT-TOKEN で失効時刻を logout token の `iat` に統一し、UPSERT の `changes() == 0`（再送・古いトークン）ではメモリ更新とリフレッシュ消費をスキップして閾値を前進させない。下記「CAS 2段目と失効閾値レースの検証」を参照 |
| ID-07 | JWT 検証の追加カバレッジ（2026-11-18） | 検証鍵JSON（旧kid登録/不正JSON/kid境界/不正PEM）、alg混同の直接検証、`JWT_REQUIRE_KID`解析 | 実装済み・検証済み: `test_jwt_verification_keys_json_*` / `test_jwt_rejects_hs256_token_signed_with_public_pem` / `test_jwt_add_verification_key_kid_bounds` / `test_jwt_require_kid_from_env_values`（server/src/tests.rs、10件） |
| ID-08 | 開発用モック経路の所有証明（PoP・opt-in、段階1-2） | `VerifyRequest.proof_of_possession` 省略時は従来どおり200。添付時は nonce に対する RSASSA-PKCS1-v1_5 + SHA-256 署名（base64 STANDARD）を `public_key_der`（SPKI）で検証し、正常署名は200、署名対象 nonce の改ざん・`pop.nonce` 不一致・別鍵署名は401。署名 decoded 512バイト超・公開鍵DER 16 KiB 超も401 | 実装済み。ハンドラ経由4件（`test_auth_verify_pop_valid_signature_is_accepted` / `test_auth_verify_pop_tampered_nonce_is_rejected` / `test_auth_verify_pop_other_key_signature_is_rejected` / `test_auth_verify_pop_is_opt_in`）を `server/src/tests.rs` に追加。純粋関数4件と mock nonce 単回消費テストは維持。E2E 定義数は69件のまま変更なし |
| ID-06 | 全レスポンスのセキュリティヘッダ（SEC-01） | `X-Content-Type-Options: nosniff`、`X-Frame-Options: DENY`、`Content-Security-Policy: default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'` を全レスポンスへ付与する。HTML 配信の CSP は配信側担当。CorsLayer が短絡する CORS プリフライト（OPTIONS の 200/204 成功レスポンスを含む）にも最外側の `security_headers` で付与し、本文なしの 204 でも欠落させない | 実装済み。`assert_security_headers` で検証: 403（`test_security_headers_on_error_response`）、OPTIONS プリフライト（`test_security_headers_on_cors_preflight`）、429（`test_security_headers_on_rate_limited_response`）、500（`test_security_headers_on_internal_error_response`）、204（`test_security_headers_on_no_content_response`）、通常 200（既存アサーション）。下記「全レスポンスのセキュリティヘッダの検証（ID-06）」を参照。CSP は API レスポンスのみへ付与し、HTML 配信は配信側担当 |
| MET-01 | 中立化URL件数指標とプライバシー | 危険 URL を中和した件数だけを送り、URL 本文・IP・User-Agent を送らず記録しない。`GET /metrics` のカウンタは差分で増える | 実装済み。下記「中立化URL件数指標の検証（MET-01）」を参照。カウンタは参考値であり、単独のアラート判定・請求判断には使わない |
| MET-02 | `observe_metrics` の最外側配置（サーバー側） | 内側で短絡する 429 / IPv4 403 / 413 / CORS プリフライト OPTIONS 204 を取りこぼさず `openillustration_http_requests_total` と `openillustration_http_errors_total` へ計上する | 実装済み。`observe_metrics` を `Router` 最外側へ移動。下記4件の差分アサーションを追加（下記「observe_metrics の最外側配置の検証（MET-02）」を参照）。旧 E2E 名 MET-02 / MET-03 は下記 NEUT-URL-02 / NEUT-URL-03 へ改名済み |
| NEUT-URL-02 | プロトコル相対 URL の中立化と件数送信（E2E） | `//host/path` と Windows UNC を `href="#"` / `src="#"` へ中立化し、`{"event":"neutralized_url","count":2}` を1回だけ送る。通常の相対 URL（`/chapter/1` / `../chapter/1`）は維持 | 実装済み・検証済み: `e2e/tests/metrics.spec.ts` の `NEUT-URL-02: protocol-relative URL は # に中立化され件数が送信される`。旧 E2E 名 `MET-02` はサーバー側 `observe_metrics` の ID と衝突したため改名済み |
| NEUT-URL-03 | 同一本文の再描画で client event を再送しない（E2E） | BROWSE → UPLOAD → BROWSE で ContentViewer を再マウントしても本文取得は合計2回、`POST /metrics/client-events` は1回のまま（`REPORTED` 集合による重複除外） | 実装済み・検証済み: `e2e/tests/metrics.spec.ts` の `NEUT-URL-03: 同一本文の再描画では client event を再送しない`。旧 E2E 名 `MET-03` は仕様に存在しなかったため改名済み |
| PROP-01 | `prop_jpy_to_wei_boundaries_and_reference` | 任意の `u64` で panic しない。`0` と `MAX_ORDER_JPY` 超（上限+1 と `u64::MAX`）は `Err`、範囲内は `WEI_PER_JPY = 10^18` の独立参照値（u128 計算）と一致 | 実装済み（128ケース）。下記「proptest の検証（PROP-01〜04）」を参照 |
| PROP-02 | `prop_is_valid_cid_only_accepts_canonical_ascii` | 空・非ASCII・128バイト超は必ず拒否。受理値は独立 oracle（v0: `Qm` + base58btc 44文字で計46文字、v1: `b` + base32lower 1文字以上）を満たす。既知の正規 CID（v0 / v1）は必ず受理 | 実装済み（128ケース）。下記「proptest の検証（PROP-01〜04）」を参照 |
| PROP-03 | `prop_summarize_reason_is_bounded_and_idempotent` | 任意の Unicode 文字列で panic せず、結果は120 Unicode 文字以下。空文字の対応を保持し、要約結果へ再適用しても値は変わらない（冪等） | 実装済み（128ケース）。下記「proptest の検証（PROP-01〜04）」を参照 |
| PROP-04 | `prop_summarize_reason_truncates_safe_text_at_char_boundary` | PII を含まない文字（あ / 知 / A / b / 空白 / 改行 / 🍣 / 。、0..=160個）で、120文字以下は等価、超過は先頭119文字＋`…` に切り詰め（Unicode 境界を維持） | 実装済み（128ケース）。下記「proptest の検証（PROP-01〜04）」を参照 |
| CLI-01 | `doctor` の純粋関数化と引数・設定判定（R5-06 / R4-01） | `parse_args_from` / `check_all_with` / `should_fail` を純粋関数として検証。23カテゴリ固定、秘密値非出力、既定は警告のみで exit 0、`--strict` は問題検出時のみ exit 1 | 実装済み。`server/src/bin/doctor.rs` に53テスト（既存29件＋境界テスト7件＋OIDC URL検証3件＋oidc_required_assurance1件＋設定検証3件＋ヘルプ優先2件＋AUDIT_EXPECT_ROOT1件＋AGE_POLICY_CONFIGURED3件＋バックアップ/TZ診断3件＋バックアップ生値判定1件）を追加（ネットワーク設定警告4件を含む）。下記「3CLIの純粋関数化と自動テスト（CLI-01〜03）」を参照 |
| CLI-02 | `verify_audit_export` の `--expect-root` と検証結果の純粋関数化 | version 3 のみ受理し v1/v2 は fail-closed 拒否。改ざん entry / root / 署名を拒否。`--expect-root` は大小文字を区別せず照合し、不一致は `audit root mismatch` で拒否。受理・拒否いずれも理由本文と秘密値を出力しない | 実装済み。`server/src/bin/verify_audit_export.rs` に当初17件を追加し、現在は20件（HELP不変テスト等を追加）。`VerificationOutcome` / `verify_export` を純粋化。下記「3CLIの純粋関数化と自動テスト（CLI-01〜03）」を参照 |
| CLI-03 | `verify_backup` の `--database-token` 拒否と sidecar fail-closed | `--database-token` は argv で受け付けず値もエコーしない（`DATABASE_TOKEN` 環境変数のみ）。`-wal` / `-shm` / `-journal` サイドカーが残っていれば DB を開く前に拒否。成功行は `backup_verification: OK tables=N migrations=N`、失敗時は stdout にエラー本文を出さない | 実装済み。`server/src/bin/verify_backup.rs` に当初17件を追加し、現在は19件（HELP不変テスト等を追加）。`parse_args_from` / `decide_output` / `ensure_no_sidecars` / `verify_backup_at` を境界化。下記「3CLIの純粋関数化と自動テスト（CLI-01〜03）」を参照 |
| AUDIT-03 | `GET /content/{cid}/audit` の理由マスクと `entry_hash` 維持 | `reason` は `summarize_reason` の結果のみで、メール・URL・4桁数字・DB 全文が漏れない。`entry_hash` は生 `reason` から計算した DB の生値と一致し、DB 全文は不変 | 実装済み（`test_get_content_audit_masks_reason_but_preserves_entry_hash`）。下記「監査理由マスク・投票署名 v2・価格 NULL・エクスポート解析の検証」を参照 |
| AUDIT-04 | `GET /content/{cid}/audit` の取得上限・新着順と検証限界 | 最新 MAX_AUDIT_LIST(500) 件を `log_id` 新しい順（DESC）で返す。501件投入なら先頭 `log_id=501`・末尾 `2`・全体降順（最古の1は除外）。古い行はAPIから参照不可で、完全検証には署名付きエクスポート/DBを使用。レスポンスには `previous_log_hash` が無く `reason` はマスクされるため、レスポンス単体でのハッシュ連鎖検証は成立しない | 実装済み（`get_content_audit` の `ORDER BY log_id DESC LIMIT ?`）。サーバー側テスト `get_content_audit_caps_at_max_audit_list` が501件投入・500件返却・先頭501・末尾2・降順を検証。E2E は `e2e/tests/integrity.spec.ts` の `/audit` モック（log_id DESC の3件）で FORENSIC_TIMELINE の表示順が新しい順であることを検証。完全な連鎖検証は AUDIT-01（署名付きエクスポート/DB）を参照 |
| MOD-12 | 投票署名 v2 と投票者束縛（旧形式拒否） | 署名対象は `domain: "moderation-vote-v2"` / `report_id` / `result` / `juror_hash`。旧 v1 形式の署名と別投票者向けに束縛された署名は401で拒否し票を記録しない。正常系は受理され1票が記録される | 実装済み。サーバー側テスト4件（`test_moderation_vote_message_v2_binds_all_fields` / `test_vote_v2_signature_accepted` / `test_vote_rejects_legacy_v1_signature` / `test_vote_rejects_signature_bound_to_other_juror`）。下記同節を参照 |
| CONTENT-10 | 一覧の `price` NULL | `status = 'Approved'` の行の `price` が NULL でも500にならず、購入不可を表す0として返る | 実装済み（`test_list_contents_null_price_is_returned_as_zero`）。`SELECT` 句は `COALESCE(price, 0) AS price`。下記同節を参照 |
| EXPORT-11 | エクスポート解析失敗の表示 | `DataExport` として読めないレスポンスでは「0 件」成功にせず、`data-testid="export-error"` に固定文言 `エクスポート結果の解析に失敗しました。` を表示し、`export-status` は空のまま | 実装済み。クライアント側単体テスト `export_parse_failure_uses_fixed_phrase_without_success_status`（`client/src/lib.rs`）。正常系は件数入りの成功文言を維持。EXPORT-07 のダウンロード開始失敗（`anchor.click()` の例外、`ダウンロードの開始に失敗しました。`）は別事象であり、本ゲートはレスポンス解析失敗のみを扱う。E2E は `export.spec.ts` の既存1件目（ダウンロード成功シナリオ）内へ `version` が文字列の不正 JSON を返すフェーズを追加し、`export-error` の固定文言と `export-status` が空であることを検証（新規 `test()` なし、export3件・全体69件を維持）。下記同節を参照 |
| BUILD-01 | cargo-deny の4ゲート（licenses / advisories / bans / sources） | 既知の重複は exact version skip でのみ許容し、新規重複・未知取得元はビルドを失敗させる | 実装済み。`deny.toml` は `multiple-versions = "deny"` と skip の `version = "=x.y.z"` 固定（`version = "*"` は不使用）。`extra_gates` は licenses / advisories / bans sources の3コマンドで4ゲートを実行。`wildcards = "allow"` は workspace の path 依存に対する誤検知回避のみ |
| BUILD-02 | リリース wasm の API オリジン fail-closed | `OIL_API_BASE` に `http://` または `localhost` を含む値を指定するとビルド失敗。E2E / 開発ビルドは未設定既定値で従来どおり動作 | 実装済み。`client/src/lib.rs` の const アサーションは `dev-console` feature なしで有効。`OIL_MOCK_AUTH` と同じ `option_env!` パターンで未設定時は開発既定値を維持。E2E 定義数69件・check23・doctor53・migrations14・tests136 は不変 |

### 性能・互換ゲートの評価状況（PERF-01 / COMPAT-01 / COMPAT-02）

R1 分析に従い、3 ゲートを「閾値未決定のまま名目上 pass」から評価対象へ移す。
現状の status は **not_evaluated** である。「閾値未決定」ではなく
**未作成（ADR-0011/0012 で合意待ち）** と明記する。個別の対応 ADR は下表を参照。

- **Level 0（観測可能性）**: 今すぐの pass 条件。観測値が有限かつ 0 以上で、
  エビデンスが `testInfo.attach` に保存されること。大小比較・閾値判定は行わない。
- **Level 1（受入）**: ADR-0011 / ADR-0012 で受入値と対応マトリクスが合意された後に
  適用する。合意前は適用せず、本番移行ゲートのブロッカーとして扱う。

| ID | status | test oracle | evidence | 対応ADR |
| --- | --- | --- | --- | --- |
| PERF-01 | not_evaluated | 計測値（E2E 所要時間・paint / navigation timing）が有限かつ 0 以上。大小比較なし | `e2e/tests/metrics.spec.ts` の `perf-observation` 添付 | [ADR-0011](adr/ADR-0011-performance-measurement.md) |
| COMPAT-01 | not_evaluated | 選択プロジェクトと firefox / webkit の可否が添付に記録される。実機 verified は未実施 | `e2e/tests/build-gate.spec.ts` の `compat-selection` 添付 | [ADR-0012](adr/ADR-0012-compatibility-matrix.md) |
| COMPAT-02 | not_evaluated | JXL / Wasm 非対応時の代替表示・案内が存在すること。認証・決済は迂回不可 | ADR-0012 の状態クラス（functional_only / blocked） | [ADR-0012](adr/ADR-0012-compatibility-matrix.md) |

### 年齢ゲート共通化と TZ 境界の検証（AGE-02〜04）

テストの根拠は R1-17（年齢ゲートのバイパス遮断）と DEC-02 のフェイルクローズ方針。
`server/src/tests.rs` に次の3件を追加し、`server/src/lib.rs` の `ensure_age_policy_configured` /
`today_for_policy` / `validate_age_at` を直接・間接に検証する。

| テスト関数 | 入力・操作 | 期待結果（test oracle） |
| --- | --- | --- |
| `test_require_explicit_configuration_blocks_oidc_callback_and_refresh` | `require_explicit_configuration: true` / 必須設定値は既定のまま（`configuration_satisfied() == false`）で callback（任意 state）と refresh（任意 Cookie）を呼ぶ | 両方とも 403。state / Cookie の消費前に共通ゲートで拒否 |
| `test_today_for_policy_uses_configured_local_timezone` | UTC `2024-01-01T16:00:00Z` に `None` / `UTC` / `Etc/GMT+5` / `Etc/GMT-9` / ` Asia/Tokyo ` / `Asia/Tokyo` / `America/New_York` を渡す | 最初の3件と未対応名は 2024-01-01、`Etc/GMT-9` と `Asia/Tokyo`（空白入り含む）は 2024-01-02 |
| `test_validate_age_at_uses_local_today_for_age_boundary` | 2003-01-02 生まれ・JP・既定下限 21 歳、同じ現在時刻で TZ を変える | `Asia/Tokyo` / `Etc/GMT-9` は 21 歳で `Ok`、`UTC` は 20 歳で `Forbidden`（403） |

TZ の強制変更はプロセス環境を汚さないため、純粋関数 `today_for_policy` / `validate_age_at` に
基準時刻と TZ を明示して検証する。未知の TZ は UTC へフォールバックする分岐も含める。
年齢数値の判定は `/auth/verify` の `validate_age` に委ね、callback / refresh では設定エラーのみを
403 へ変換する。これらはサーバー側テストであり、E2E 定義数は当時59件のまま変わらず（現在69件）。

充足判定は `AgePolicy::configuration_satisfied()` が実効値から導出する
（`default_minimum_age != 21` / `country_minimums` 非空 / `target_regions` 非空のいずれか）。
旧手動フラグ `AGE_POLICY_CONFIGURED` の env 読取は廃止し、`GET /policy/age` と起動ログの公開値は
`configuration_satisfied`（計算結果）へ置換した。`doctor` の `age_policy` カテゴリは
`require_explicit_configuration` 有効かつ未充足のときに固定文言で警告する（値はエコーしない）。

### 署名付き年齢属性 adapter の検証（AGE-05〜12）

R1 分析と ADR-0007 に基づき、`server/src/verified_age.rs` の adapter 境界と
`auth_verify` の mock 分岐を検証する。実証明書は未接続のため、すべて
`allow_mock_auth=1` の開発用経路での検証である。

| テスト関数 | 入力・操作 | 期待結果（test oracle） |
| --- | --- | --- |
| `verify_accepts_valid_claims` | 許可済み `jpki-test`・audience `test-client`・`fixture-nonce-0001` | `VerifiedAge` を返し、issuer / signed_subject / birthdate / country がクレームどおり |
| `self_reported_fields_cannot_be_injected_into_claims` | `verified_claims` に `birth_date` / `country_code`、attestation に `birth_date` を混入 | ともにデシリアライズ失敗（`deny_unknown_fields`） |
| `verify_rejects_untrusted_issuer` | 未許可 issuer、空許可リスト | 403 |
| `verify_rejects_audience_mismatch` | 引数 audience と `claims.audience` が不一致 | 403 |
| `verify_rejects_nonce_mismatch` | 引数 nonce と attestation の nonce が不一致 | 403 |
| `test_verified_age_accepts_valid_signed_attribute_in_mock_path` | 信頼属性必須モード・自己申告は未成年（2010年生）・検証済みは 2000年生。2回目は別 nonce で signed_subject を `"age-over-20-pairwise"` に変更 | 200。自己申告値が信頼属性として使われない。(a) `user_hash` は64文字hex、(b) `sha256("2000-01-01:JP")` と異なる、(c) signed_subject 変更で hash が変わる |
| `test_verified_age_rejects_reused_nonce` | 同一 nonce で `/auth/verify` を2回 | 1回目 200、2回目 400（`MOCK_NONCES.remove()` で消費済み） |
| `test_verified_age_rejects_untrusted_issuer` / `test_verified_age_rejects_audience_mismatch` | 信頼属性必須モード・mock 経路で不正な issuer / audience | 403 |
| `test_verified_age_claims_rejected_when_mock_auth_disabled` | `allow_mock_auth=false` で `verified_claims` を提示 | 403 |
| `test_verified_age_default_mode_keeps_self_reported` | 既定ポリシーで自己申告のみ | 200（R1 item9: 自動昇格しない） |

E2E 定義は追加していない（現在69件を維持）。

### エクスポートの読み取りトランザクション検証（EXPORT-09/10）

R5-04 の12表エクスポートを単一時点のスナップショットにするため、`me_export` は
`begin_write_tx`（`write_lock` 取得後に `BEGIN DEFERRED`）で `collect_export_snapshot` を包み、
`tx.commit()` で閉じる。共有 `Connection` の BEGIN をアップロード等の書き込みと直列化するため、
同時書き込みがあっても 500 にならず、書き込み完了を待ってから開始する。

| テスト関数 | 検証する test oracle |
| --- | --- |
| `me_export_queries_run_inside_read_transaction` | `version:1` が返り、`debug_assert!(!db.is_autocommit())` が発火しない。`db.transaction()` / `tx.commit()` を外す mutation で失敗 |
| `me_export_snapshot_consistent_under_concurrent_ban_write` | 一時 DB の2接続で、書き込み側の `BEGIN IMMEDIATE` 未コミット BAN 更新・監査挿入がエクスポート結果に現れない（`is_banned: false`、BAN 監査なし）。完了後に書き込みを `COMMIT` |
| `me_export_waits_for_concurrent_write_transaction` | 外部ライターが `write_lock` と BEGIN を保持している間は完了せず（200ms タイムアウト）、解放後に成功する。`begin_write_tx` を素の `db.transaction()` に戻す mutation で失敗 |

3件目の `me_export_waits_for_concurrent_write_transaction` は EXPORT-09（読み取りトランザクションの直列化）に帰属し、1件目と同じく `begin_write_tx` のロック取得を検証する。
これらはサーバー側テスト3件のみで、E2E 定義数は当時59件のまま変わらず（現在69件）。

### 期限再判定とトランザクション直列化の回帰（PAY-01/04/05）

共有 `Connection` の BEGIN 直列化と期限判定の回帰テストを `server/src/tests.rs` に追加した。
すべてサーバー側テストであり、E2E 定義数は当時59件のまま変わらず（現在69件）。

| テスト関数 | 検証する test oracle |
| --- | --- |
| `test_expired_bind_waits_for_write_transaction` | 期限切れ注文の bind は外部ライターの `write_lock` / BEGIN 保持中は完了せず、解放後に EXPIRED 更新をコミットして400を返す。EXPIRED 更新を tx 外の `db.execute` に戻す mutation で失敗 |
| `test_expired_verify_waits_for_write_transaction` | 期限切れ注文の verify も同様に待ち、解放後に400と EXPIRED 更新を返す |
| `test_fixed_receiving_address_parallel_orders_all_saved` | `PAYMENT_RECEIVING_XPRV` 未設定（固定受取アドレス）で並行注文が全件成功し、全注文が保存される（`derivation_index` は NULL のまま） |
| `test_fixed_address_order_waits_for_write_transaction` | 固定アドレス分岐の INSERT も HD 分岐と同じく `write_lock` 経由。ロック保持中は完了せず、解放後に成功する。`insert_payment_order(&state.db, ...)` に戻す mutation で失敗 |
| `test_payment_verify_rechecks_expiry_after_verification` | verifier が `expires_at` を過去へ書き換えても、CAS 前に tx 内の最新値で再判定し、EXPIRED＋400を返す。FINALIZED へ進まず `tx_hash` / `confirmations` も書き込まれない。再判定を外す mutation で失敗 |

### 決済 DB エラー判別のオラクル（PAY）

verify の CAS 更新失敗を「送金の再利用」とみなすのは UNIQUE 違反のみで、
それ以外の DB エラーは Internal(500) へ写像する（内部詳細はマスク）。
判別は `is_unique_constraint_violation` がローカル／リモート双方の表現を扱う。

| テスト関数 | 検証する test oracle |
| --- | --- |
| `local_unique_error_is_detected` | ローカル `SqliteFailure(2067, "UNIQUE constraint failed: ...")` は違反と判定。`SqliteFailure(5, "database is locked")` は非違反 |
| `remote_unique_error_is_detected_by_code_and_extended_code` | リモート `RemoteSqliteFailure` を extended code(2067) / basic code(2067) / メッセージの3経路で検出。`RemoteSqliteFailure(19, 5, "database is locked")` は非違反 |
| `test_payment_verify_maps_non_unique_db_error_to_internal` | CAS 更新に `RAISE(ABORT)` トリガを張り、非 UNIQUE の DB エラーが400（再利用）へ写像されず Internal(500)＋マスク済み本文（`Internal server error`）になることを確認 |

いずれもサーバー側テストであり、E2E 定義数は当時59件のまま変わらず（現在69件）。

### 通報理由の要約のテスト設計（R5-04 関連）

`common::summarize_reason` と `GET /mod/reports` を対象に、PII 除去・境界・日本語・
統合時の全文非漏えいを検証する。テストの根拠は ADR-0004 決定9（理由の要約を公開し、
通報者・投票者の識別情報は公開しない）と運用ポリシーの閲覧範囲（認証必須・
Reported 限定・新着順・運用者非限定）である。設計技法は同値クラスと境界値解析
（ISO/IEC/IEEE 29119-4 の境界値解析）を用いる。

| ケース | 入力例 | 期待結果 |
| --- | --- | --- |
| 空文字 | `""` | `""` |
| 境界内（ASCII） | 119文字 / 120文字 | 変更なし |
| 境界超過（ASCII） | 121文字 | 先頭119文字＋`…` |
| 境界内（日本語） | 漢字・かな 120 Unicode 文字 | 変更なし。バイト数ではなく文字数で計数 |
| 境界超過（日本語） | 漢字・かな 121 Unicode 文字 | 先頭119文字＋`…` |
| メール | `user@example.com` を含む文字列 | `@` を含むトークンが `[redacted]` |
| スキーム付き URL | `https://example.com/path` / `http://example.com` / `url(https://x)` | `://` を含むトークン（括弧で囲まれても）が `[redacted]` |
| スキーム無し URL | `www.example.com` | `www.` で始まるトークンが `[redacted]` |
| IPv4 | `10.0.0.1` | IPv4 らしい `\d{1,3}(\.\d{1,3}){3}` 部分列を含むトークンが `[redacted]` |
| Unicode 数字列 | `1234` / `123` / `０９０１２３４５６７８` | Unicode 数字4連続（全角含む）は `[redacted]`。3桁は残る |
| 複合 PII | 日本語＋メール＋URL＋IPv4＋全角数字＋長文 | 要約後も PII が残らず上限120文字を守る |
| 統合 | PII を含む通報を登録し `GET /mod/reports` を呼ぶ | `reason` は要約値のみ。全文・PII がレスポンスへ現れない。未認証401・Reported 限定・新着順・運用者非限定も維持 |

対応する単体テストは `summarize_reason_redacts_additional_pii_patterns`（括弧付き URL /
`www.` / IPv4 / 全角数字と、非 PII の3桁数字が混在する複合ケース）と
`summarize_reason_redacts_pii` 内の4桁ちょうど境界アサーション（`1234` → `[redacted]`、
`123` → 保持。`>=4` を `>=5` にする mutation を弾く）である。いずれも `common` に実装済み。

DB 全文（`reports.reason`）と監査エクスポートの本文は変更しない。エクスポートの
マスクは EXPORT-04 の既存挙動を維持する。本テストは第三者公開範囲
（[ADR-0008](adr/ADR-0008-moderation-stats-publication.md) で決定済み、段階2の
公開ルートは MOD-13 で検証）を証明するものではない。

### 設定診断（`doctor`）の検証

`cargo run -p server --bin doctor -- [--strict]` は秘密値を出力せず、問題を検出しても
既定では終了コード0、`--strict` のときだけ1を返す。モック認証と本番相当設定の併用は
次の警告を出す。

```text
WARN mock_auth_with_production_config: ALLOW_MOCK_AUTH is enabled together with production-like settings; do not deploy this configuration
```

ダミーの `JWT_SECRET` を設定した5パターンの実出力は次のとおり（秘密値は出力されない）。

| パターン | 環境変数（差分） | 実出力（要約） | 既定 | `--strict` |
| --- | --- | --- | --- | --- |
| P1 | `ALLOW_MOCK_AUTH=1` のみ | `doctor: 23 check(s), 1 problem(s)` + `mock_auth_enabled` 警告 | 0 | 1 |
| P2 | P1 + `PINATA_JWT=<本番相当値>` | `23 check(s), 2 problem(s)` + `mock_auth_with_production_config` / `mock_auth_enabled` 警告 | 0 | 1 |
| P3 | P1 + `OIDC_ISSUER` / `OIDC_CLIENT_ID` | `23 check(s), 2 problem(s)` + 同上の2警告 | 0 | 1 |
| P4 | P1 + `JWT_RSA_PRIVATE_KEY_PEM`（不正ダミー） | `23 check(s), 3 problem(s)` + `jwt` / `mock_auth_with_production_config` / `mock_auth_enabled` 警告 | 0 | 1 |
| P5 | P1 + `PINATA_JWT=mock_jwt`（境界値） | `23 check(s), 1 problem(s)` + `mock_auth_enabled` 警告 | 0 | 1 |

いずれもダミーの `JWT_SECRET`（32バイト以上）を設定した実測値である（2026-11-18 初回確認、
`audit_expect_root` 追加後の 2026-11-20 にビルド済み `target/debug/doctor` で再確認）。
`mock_auth_enabled` / `ipv4_localhost` / `trusted_proxy` の追加により検査数は14となり
（その後の追加を経て現行は23）、`ALLOW_MOCK_AUTH=1` 単独でも問題が1件計上される。
`ALLOW_IPV4_LOCALHOST` / `TRUSTED_PROXY_CIDRS` 未設定なら追加問題は出ない。よって P5 は
`--strict` で終了コード1であり（旧挙動の0ではない）、`--strict` の判定は
`should_fail(strict, problems)` に委譲される。P4 の jwt 警告はダミー PEM が無効なことに
よるもので、mock 警告とは独立している。別途 `env -i ALLOW_MOCK_AUTH=1 ./target/debug/doctor`
の実出力で `23 check(s), 2 problem(s)`（`JWT_SECRET` 未設定による `jwt` 警告を含む）を確認した。

### 3CLIの純粋関数化と自動テスト（CLI-01〜03）

今回の追加は全て `server/src/bin/` 配下のサーバー側 bin テストであり、E2E 定義数は当時59件のまま（現在69件）
変更しない。各 CLI は I/O・環境変数読み取り・`std::process::exit` を薄い `main` 側へ追い出し、
判定部分を純粋関数（または入力を引数で受け取る境界関数）としてテストする。

#### CLI-01: `doctor`（`server/src/bin/doctor.rs`、53テスト（既存29件＋境界テスト7件＋OIDC URL検証3件＋oidc_required_assurance1件＋設定検証3件＋ヘルプ優先2件＋AUDIT_EXPECT_ROOT1件＋AGE_POLICY_CONFIGURED3件＋バックアップ/TZ診断3件＋バックアップ生値判定1件））

- `parse_args_from<I>(items) -> ParseOutcome`: 引数は `Vec<String>` で受け取り、先頭要素を
  プログラム名として読み飛ばす。`[]` は `Run { strict: false }`、`["--strict"]` は
  `Run { strict: true }`、`-h` / `--help` は `Help`、単一の不明引数は
  `UnknownArgument(String)`、複数引数は `TooManyArguments` を返す（完全一致のみ有効）。
- `check_all_with<F>(lookup) -> Vec<(&'static str, Vec<String>)>`: 環境変数を `lookup`
  経由でのみ読み、23カテゴリを固定で返す（`production` /
  `mock_auth_with_production_config` / `mock_auth_enabled` / `jwt` / `jwt_secret` /
  `ipfs_gateways` / `ipfs_timeout` / `ipfs_max_attempts` / `oidc_http_timeout` /
  `graceful_drain_timeout` / `moderation_finalize_interval` /
  `oidc_clock_skew` / `age_policy` / `rate_limit` / `ipv4_localhost` / `trusted_proxy` /
  `expose_internal_errors` / `allowed_origins` / `audit_expect_root` ほか）。
  秘密値は問題文へ含めない。`check_all()` は実環境の `env` を渡すだけのラッパーである。
- `should_fail(strict, problems) -> bool`: `strict && problems > 0` のみ。既定は警告表示で
  exit 0、`--strict` のとき問題が1件以上なら exit 1。
- `ipfs_timeout` / `ipfs_max_attempts` / `oidc_http_timeout` に加え
  `graceful_drain_timeout` / `moderation_finalize_interval` も `rate_limit` と同じく
  「生のパース値」と「正規化後の値」を比較し、既定へフォールバックした場合に当該カテゴリへ
  警告1件を計上する。後者2つは新規カテゴリとして追加し `check(s)` は19→21→22→23へ増えた（現行23）。
- OIDC 必須 7 変数のいずれかが `Some` で trim 後空文字のとき、共通バリデータが
  `OIDC configuration values must not be empty` をハードエラーとして起動を拒否する。
  doctor は同じ固定文言を `oidc_endpoint_urls` カテゴリへ警告として流し、
  `production` カテゴリも既存どおり
  `OIDC is not configured (OIDC_* environment variables are required)` を警告する
  （必須値の空文字は `oidc_endpoint_urls` と `production` の両方で固定文言警告、
  設定値・生バイトは非エコー）。`oidc_endpoint_urls` への追加は `results.push` を
  増やさず、23カテゴリ・53テストを維持する。未設定・未登録時は `hard_errors` が
  積まれず、従来どおり警告なし。

| 群 | 件数 | テスト名 |
| --- | --- | --- |
| 引数パース | 8 | `parse_no_arguments_runs_non_strict` / `parse_strict_flag_runs_strict` / `parse_help_flags_request_help` / `parse_unknown_argument_is_reported` / `parse_multiple_arguments_is_rejected` / `parse_only_the_exact_strict_flag_enables_strict` / `parse_strict_with_help_requests_help` / `parse_help_with_strict_requests_help` |
| 設定判定 | 42 | `production_backup_misconfiguration_warns_without_disclosing_values` / `production_backup_whitespace_only_values_warn_without_disclosing_values` / `age_policy_tz_unset_warns_without_disclosing_values` / `complete_backup_and_tz_configuration_adds_no_new_warnings` / `check_always_reports_twenty_three_categories` / `audit_expect_root_warns_in_production_without_disclosing_value` / `mock_auth_alone_adds_a_warning` / `mock_auth_truthy_values_are_recognized_and_falsy_are_not` / `unset_mock_auth_is_disabled` / `mock_auth_combined_with_production_config_is_reported` / `mock_auth_with_mock_pinata_value_is_not_a_combination` / `invalid_ipfs_gateway_entries_increase_problem_count` / `all_invalid_ipfs_gateways_report_both_warnings` / `unset_ipfs_gateways_uses_default_without_warning` / `valid_ipfs_gateways_add_no_invalid_entry_warning` / `secret_values_are_never_printed_in_problems` / `ipv4_localhost_truthy_values_add_a_warning` / `ipv4_localhost_falsy_and_unset_add_no_warning` / `trusted_proxy_when_configured_warns_without_disclosing_value` / `trusted_proxy_unset_adds_no_warning` / `oidc_clock_skew_in_range_and_unset_add_no_warning` / `oidc_clock_skew_out_of_range_warns_without_disclosing_value` / `ipfs_timeout_fallback_is_warned_without_disclosing_value` / `ipfs_max_attempts_fallback_is_warned_without_disclosing_value` / `oidc_http_timeout_fallback_is_warned_without_disclosing_value` / `graceful_drain_timeout_fallback_is_warned_without_disclosing_value` / `moderation_finalize_interval_fallback_is_warned_without_disclosing_value` / `oidc_endpoint_urls_https_adds_no_warning` / `oidc_endpoint_urls_http_in_production_warns_without_disclosing_value` / `oidc_endpoint_urls_mock_allows_only_localhost_http` / `oidc_max_auth_age_mock_with_incomplete_oidc_adds_no_warning` / `oidc_max_auth_age_mock_with_complete_oidc_warns_without_disclosing_value` / `production_warns_when_required_oidc_value_is_empty_without_disclosing_value` / `expose_internal_errors_truthy_warns_without_disclosing_value` / `allowed_origins_missing_or_empty_warns_without_disclosing_value` / `allowed_origins_uses_real_parser_result_without_disclosing_value` / `oidc_cookie_secure_falsy_warns_without_disclosing_value` / `oidc_required_assurance_unset_warns_without_disclosing_value` / `oidc_max_auth_age_invalid_warns_without_disclosing_value` / `age_policy_flag_only_warns_without_disclosing_values` / `age_policy_flag_with_explicit_values_has_no_warning` / `age_policy_flag_unset_keeps_existing_behavior` |
| strict判定 | 3 | `non_strict_never_fails` / `strict_fails_only_when_problems_exist` / `end_to_end_exit_decision_for_mock_only_configuration` |

受理値は `1` / `true`（真）と `0` / `false`（偽）のみで、trim・小文字化は維持する。
`yes` / `no` / 空文字を含む受理値外は拒否し、fail-closed とする。ランタイムは起動拒否となり、
起動前診断では既存カテゴリ内へ固定文言の警告が追加され、値はエコーしない。
判定は全フラグ共通の `common::age::parse_bool_env` に統一する。
状態からの採用値と不正時文言の導出は `common::env::bool_env_result` へ集約し、
`bool_env_at_startup` / `bool_flag_with_warning` / `bool_flag_from_env_value` が委譲する。
受理値列挙の固定文言 `must be one of 1/true/0/false` は common 側の共通ヘルパに集約され、
  OIDC 共有バリデータは変数名入りの独立文言を維持する（受理値は同一）。
各呼び出し側は変数名（doctor は既定値も）を付けるだけである。
受理値・既定値・既存文言・check23 / doctor53 は不変（新規テスト関数なし）。
`ALLOW_MOCK_AUTH=1` 単独でも `mock_auth_enabled` に警告が計上され、`--strict` で exit 1 になる。

上表の「設定判定」42件のうち `oidc_max_auth_age` カテゴリは、共通バリデータの固定文言をそのまま警告へ流す。上限 `MAX_AUTH_AGE_MAX_SECS = 2_592_000`（30日）超過
  （固定文言 `OIDC_MAX_AUTH_AGE_SECS must not exceed 2592000 seconds (30 days)`、`u64::MAX` 境界を含む）は既存テスト内のアサーション追加で検証し、
  設定値はエコーしない（テスト数・件数は不変）。

##### doctor のネットワーク設定警告（追加4件）

`ipv4_localhost` と `trusted_proxy` の2カテゴリを追加し、その後の `oidc_clock_skew` 追加と
フォールバック検出テスト5件（graceful drain / moderation finalize の2件を含む）を含め、
現行は23カテゴリ・53テスト（既存29件＋境界テスト7件＋OIDC URL検証3件＋oidc_required_assurance1件＋設定検証3件＋ヘルプ優先2件＋AUDIT_EXPECT_ROOT1件＋AGE_POLICY_CONFIGURED3件＋バックアップ/TZ診断3件＋バックアップ生値判定1件）である
（既存 `check_always_reports_eleven_categories` は `check_always_reports_nineteen_categories`、
`check_always_reports_twenty_two_categories` を経て
`check_always_reports_twenty_three_categories` へ段階的に改称）。
ネットワーク設定警告の追加4件の oracle は次のとおり。

| テスト | 条件 | 期待結果 |
| --- | --- | --- |
| `ipv4_localhost_truthy_values_add_a_warning` | `ALLOW_IPV4_LOCALHOST` が `1` / `true` / `TRUE` / `TrUe` | `ipv4_localhost` に警告1件。文言は固定で値をエコーしない |
| `ipv4_localhost_falsy_and_unset_add_no_warning` | 偽値 `0` / `false`、未設定は警告なし。`no` / 空は受理値外 | 後者は `ipv4_localhost` に不正値警告1件（fail-closed で IPv4 不許可、値はエコーしない） |
| `trusted_proxy_when_configured_warns_without_disclosing_value` | `TRUSTED_PROXY_CIDRS` 設定 | `trusted_proxy` に警告1件。`X-Forwarded-For` / `XFF` を含むが CIDR 値は漏えいしない |
| `trusted_proxy_unset_adds_no_warning` | 未設定 | 警告なし |

`should_fail(strict, problems)` の判定は不変で、`--strict` のときのみ問題があれば exit 1 となる。
実バイナリで `env -i ALLOW_MOCK_AUTH=1 ./target/debug/doctor` を実行し、
`doctor: 23 check(s), 2 problem(s)`（`mock_auth_enabled` + `jwt`）を確認した。

##### doctor のフォールバック検出（追加5件）

`ipfs_timeout` / `ipfs_max_attempts` / `oidc_http_timeout` / `graceful_drain_timeout` /
`moderation_finalize_interval` の各パーサは不正値・範囲外を既定へフォールバックするため、
従前の「正規化後の値が 0」という判定は発火しなかった（死コード）。
`rate_limit` と同じ方式で「生のパース値」と「正規化後の値」を比較し、差分がある場合に当該
カテゴリへ警告1件を計上する。警告文は固定で設定値は出力しない。

| テスト | フォールバックを検出する値 | 警告なし（有効値） |
| --- | --- | --- |
| `ipfs_timeout_fallback_is_warned_without_disclosing_value` | `abc` / `0` / `301`（範囲外） / `999999999` | `1` / `15` / `300` / 未設定 |
| `ipfs_max_attempts_fallback_is_warned_without_disclosing_value` | `abc` / `0` / `6`（範囲外） / `999999999` | `1` / `3` / `5` / 未設定 |
| `oidc_http_timeout_fallback_is_warned_without_disclosing_value` | `abc` / `0` / `61`（範囲外） / `999999999` | `1` / `10` / `60` / 未設定 |
| `graceful_drain_timeout_fallback_is_warned_without_disclosing_value` | `abc` / `0` / `301`（範囲外） / `999999999` | `1` / `30` / `300` / 未設定 |
| `moderation_finalize_interval_fallback_is_warned_without_disclosing_value` | `abc` / `0` / `29` / `3601`（範囲外） / `999999999` | `30` / `300` / `3600` / 未設定 |

各テストは警告文の完全一致で値のエコーを防ぎ、有効値と未設定では警告が出ないことも検証する。
`graceful_drain_timeout` / `moderation_finalize_interval` は新規カテゴリとして追加し、
`doctor: 23 check(s)` となる（旧19から+4、途中21・22を経由）。`lib.rs` のパーサ範囲（1..=300 / 30..=3600）
と同一の境界を doctor 側でも比較する。

##### doctor の AUDIT_EXPECT_ROOT 診断（追加1件）

本番相当（`ALLOW_MOCK_AUTH` 無効）では `AUDIT_EXPECT_ROOT` に別経路保存の Merkle root（hex）を
設定する。`audit_expect_root` カテゴリは `common::env::classify(lookup(...))` と共有ヘルパ
`production_audit_expect_root_error` へ委譲する（trim しない hex 判定は `is_hex_merkle_root` が共有）。
非空で全文字が hex なら警告なし、未設定・非 UTF-8・
前後空白付き・不正 hex なら固定文言の警告1件を計上する。本番起動ゲートも同ヘルパで同じ固定文言の拒否を返す。
`production_config_errors` のシグネチャは変更せず、独立ヘルパを `run()` の設定フェーズ（DB 接続前）で `config_errors` へ追加し、本番起動ゲートへ昇格した。
`backup-rotate.sh` は未設定でも内部整合性チェックのみで動作する従来契約を維持し、
既定の doctor は警告のみで exit 0、`--strict` のときだけ失敗する。

| テスト | 条件 | 期待結果 |
| --- | --- | --- |
| `audit_expect_root_warns_in_production_without_disclosing_value` | 本番相当で未設定 / 空白のみ / 不正 hex | `audit_expect_root` に警告1件。文言は固定で設定値は漏えいしない |
| 同上 | 前後空白付き hex（hex 部は有効） | `audit_expect_root` に警告1件。設定値は漏えいしない |
| 同上 | 有効 hex（生値、前後空白なし） | 警告なし |
| 同上 | `ALLOW_MOCK_AUTH=1`（値の有無を問わず） | 警告なし（後方互換） |

#### CLI-02: `verify_audit_export`（`server/src/bin/verify_audit_export.rs`、20テスト）

- 使用形: `verify_audit_export --snapshot PATH --public-key PATH [--expect-root HEX]`。
  `--expect-root` は任意で、省略時は照合しない。値が無い場合は `--expect-root requires a value`
  で拒否される（実 CLI で確認）。
- `parse_args_from<I>(items) -> anyhow::Result<Args>` は純粋に引数を解釈し、`--help` を除き
  `--snapshot` / `--public-key` の欠落もここで検出する。
- `VerificationOutcome` は `Accepted { report }` / `Rejected { message }` の2値で、
  `exit_code()` は受理0・拒否1を返す。
- `verify_export(snapshot, public_key, expect_root) -> VerificationOutcome` は I/O も
  `std::process::exit` も行わない。`common::verify_audit_snapshot` が version 3 のみ受理し、
  署名・連鎖・Merkle root を検証する。`--expect-root` は `eq_ignore_ascii_case` で照合し、
  不一致なら `audit root mismatch (expected and computed roots withheld)` を返す。
- 受理時の1行は
  `audit_export_verification: OK version=3 entries=N governance=N root=<hex>` である。

| 群 | 件数 | テスト名 |
| --- | --- | --- |
| 引数パース | 6 | `parse_args_accepts_all_flags` / `parse_args_without_expect_root_is_valid` / `parse_args_accepts_help_without_required_args` / `parse_args_rejects_unknown_argument` / `parse_args_rejects_missing_values` / `parse_args_rejects_missing_required_arguments` |
| 受理 | 2 | `valid_v3_snapshot_from_temp_files_is_accepted` / `matching_expect_root_is_accepted_case_insensitively` |
| 拒否 | 5 | `mismatched_expect_root_is_rejected` / `tampered_entry_is_rejected` / `tampered_merkle_root_is_rejected` / `tampered_signature_is_rejected` / `unsupported_versions_1_and_2_are_rejected` |
| マスク出力 | 3 | `masked_snapshot_output_has_no_secret_or_reason_body` / `plaintext_reason_snapshot_output_still_hides_reasons` / `rejected_output_has_no_secret_or_reason_body` |
| 一時ファイル | 1 | `temp_files_are_removed_on_drop` |

改ざん entry / Merkle root / 署名はいずれも `Invalid signature` で拒否され、version 1 / 2 は
`unsupported audit snapshot version: N` で拒否される。マーカー
`TOP-SECRET-INVESTIGATION-NOTE-DO-NOT-LEAK` を理由本文へ含めても受理・拒否のどちらの出力にも
漏れないことを検証する。

#### CLI-03: `verify_backup`（`server/src/bin/verify_backup.rs`、19テスト）

- 使用形は `verify_backup --input PATH`。`parse_args_from` は環境変数・プロセス引数を読まない。
- `--database-token` は argv で受け付けず、
  `--database-token is not accepted; set DATABASE_TOKEN in the environment instead`
  で拒否する。トークン値はエラーへエコーしない（`ps` 等での露出を防ぐ）。
  これは `sign_audit_export` と同じ契約であり、値は `DATABASE_TOKEN` 環境変数のみから読む。
- `ensure_no_sidecars(path)` は `<path>-wal` / `<path>-shm` / `<path>-journal` を DB を開く
  前に検査し、存在すれば `backup has a <path> sidecar; stop the server before verifying`
  で拒否する（`restore_db` と同等の fail-closed）。
- `verify_backup_at(path)` は存在確認 → サイドカー検査 → SQLite オープン →
  `PRAGMA integrity_check` が `ok` → テーブル数 > 0 → `_migrations` の件数 > 0 を検証する。
- `decide_output(result)` は成功時に
  `backup_verification: OK tables=N migrations=N` と終了コード0、失敗時は `(None, 1)` を
  返す。エラー本文は stdout へ出さず stderr の `Error: ...` に委譲する。

| 群 | 件数 | テスト名 |
| --- | --- | --- |
| 引数パース | 6 | `parse_args_accepts_backup_path` / `parse_args_accepts_help_without_input` / `parse_args_rejects_missing_input_value` / `parse_args_rejects_missing_required_input` / `parse_args_rejects_unknown_argument` / `database_token_argument_is_rejected_without_leaking_value` |
| 出力判定 | 3 | `success_output_keeps_existing_format` / `failure_has_no_stdout_and_exit_code_one` / `failure_never_writes_error_text_to_stdout` |
| サイドカー | 2 | `sidecar_suffixes_are_appended_to_full_path` / `sidecars_are_rejected_before_opening_the_backup` |
| 構造検証 | 6 | `valid_backup_is_accepted` / `empty_database_is_rejected` / `missing_migration_ledger_is_rejected` / `empty_migration_ledger_is_rejected` / `nonexistent_file_is_rejected` / `non_sqlite_file_is_rejected` |
| 境界・回帰 | 2 | `help_text_does_not_advertise_database_token` / `option_like_value_is_rejected_without_echo` |

### 運用 bin のテスト数一覧（全7 bin）

CLI-01〜03 に加え、`server/src/bin/` 配下の全 bin について実装済みテスト数を
`scripts/test-doc-numbers.sh` の bin 数照合で検証する。現在値は次のとおり。

| bin | テスト数 | 主な検証内容 |
| --- | --- | --- |
| `backup_db` | 17件 | `VACUUM INTO` の一貫スナップショット、上書き拒否、生存ロックの fail-closed、DB 接続先必須化 |
| `doctor` | 53件 | 23カテゴリの設定診断、`--strict` 判定、秘密値非出力 |
| `restore_db` | 20件 | 引数検証、サイドカー拒否、生存ロック両側検査、復元前検証とロールバック安全コピー |
| `sign_audit_export` | 17件 | 署名鍵権限、親ディレクトリ権限、`DATABASE_TOKEN` 限定、出力上書き拒否、include_reasons 本番ガード、DB 接続先必須化 |
| `verify_audit_export` | 20件 | v3 スナップショット検証、`--expect-root` 照合、改ざん拒否 |
| `verify_audit_log` | 14件 | `expect-root` 照合、v1/v2 連鎖検証、root 不一致拒否、DB 接続先必須化 |
| `verify_backup` | 19件 | `--database-token` 拒否、サイドカー fail-closed、構造検証 |

DB 接続3 CLI（`backup_db` / `sign_audit_export` / `verify_audit_log`）は暗黙の `file:local.db` 既定を撤廃した。
`server/src/lib.rs` の共通ヘルパ `resolve_database_url(cli, env)` が `--database-url` と `DATABASE_URL` を解決し、
`env` は `common::env::read_env("DATABASE_URL")` が返す `common::env::EnvValue` で統一されている。
両方が未設定または空・空白のみなら `DatabaseUrlError::Required`（固定文言
`DATABASE_URL is required: pass --database-url or set DATABASE_URL; no default is used`）で、
非 UTF-8 なら `DatabaseUrlError::NotUtf8`（固定文言 `DATABASE_URL is not valid UTF-8`、本番の
`production_database_url_error` と同一）で起動を拒否する（値・生バイトはエコーしない）。
各 CLI へ追加した5テスト（計15件）が、両方未設定の拒否、CLI 優先、環境変数のみの受理、
空/空白拒否、非エコーを検証する。

本節の各 bin テスト数（`verify_audit_log` 14件を含む）は今回の監査対応でも不変である。
`AUDIT_EXPECT_ROOT` の更新ライフサイクルと配布物生成の回帰はシェル側で担保する。
`scripts/test-backup-rotate.sh` には既存 AUDIT_EXPECT_ROOT ケース内へ、計算 root の
`.audit-root` 世代保存（成果物と同一 stamp・0600・内容一致・stdout/stderr 非エコー）の
オラクルを追加した（新規 `expect_*` 呼び出しなし、`expect_failure` 28件 /
`expect_success` 12件を維持）。
`scripts/test-prepare-distribution.sh` には `..` 成分拒否・`repo_root`／祖先置換拒否・
`jq` / `cargo` 未検出 fail-closed・`INT` / `TERM` → rollback・旧成果物削除失敗時の
published rollback の5ケースを追加した。実 `repo_root` を壊さないため
スクリプトと `LICENSE` は work 配下のコピーで実行する。

**Runbook 手動手順の注記（数値不変）**: 暗号化バックアップ（`<stamp>.db.gpg`）の復元は
[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md) の「リストア（復旧訓練）」に従い、
`gpg --decrypt` の復号先を `RESTORED_DB` に統一し、`verify_backup` 成功を前提として
`restore_db --backup "$RESTORED_DB"` を実行する。cron 版は
`deploy/openillustration-backup.cron` のジョブ行が `backup-cron-wrapper.sh` のみを実行するため、
常時稼働環境では `instance.lock` で拒否される。root 側スケジューラで
stop→wrapper→start を併設してから有効化し、配置後に手動1回実行して成功ログを確認する。
本節の各 bin テスト数は変更しない。

### レート制限キーの検証（NET-03）

`server/src/tests.rs` の3テストで、解決済みクライアント IP の優先とピア IP への
フォールバックを検証する。

- `test_rate_limit_key_prefers_resolved_client_ip`: request extensions に解決済み IP が
  あればキーとして優先し、無ければピア IP を使う。
- `test_rate_limit_uses_resolved_client_ip_under_trusted_proxy`: 信頼プロキシ配下で
  クライアントAは2回通過し3回目が429、別バケットのクライアントBは404のまま。
- `test_rate_limit_key_falls_back_to_peer_without_trusted_proxy`: XFF 未設定または
  信頼プロキシ外では XFF を無視し、ピア IP バケットで制限する。

### SSRF リダイレクト無効化の検証（NET-05）

`server/src/lib.rs` の `build_http_client` が `reqwest::redirect::Policy::none()` を設定し、
テスト用アプリも同じクライアントを使う（`setup_app_with_options`）。ローカルの302ゲートウェイが
`Location` に内部サービスへ見立てたローカルサーバーを返し、その本文を CID の checksum と
一致させておく。リダイレクトを追従した場合にだけ検証成功（200）になる仕掛けである。

| テスト | 期待結果 |
| --- | --- |
| `test_get_content_does_not_follow_gateway_redirect` | 500。リダイレクト先の hit 数は 0、本文に内部サービスの本文（`metadata from internal service`）が漏れない |
| `test_get_content_redirect_falls_back_to_next_gateway` | 次の正常ゲートウェイから 200 で本文一致。リダイレクト先の hit 数は 0（Location 先へ接続しない） |

3xx は 2xx 以外の HTTP 応答として扱われ、次ゲートウェイへフェイルオーバーする。これは
`server/src/lib.rs` の `get_content` の分岐で固定している。

### XFF 複数値・IPv4互換形式の検証（NET-06）

`server/src/network.rs` 6件と `server/src/tests.rs` 5件の計11件で、追記型／上書き型の解釈、
複数値・不正エントリの fail-closed、IPv4-compatible IPv6（`::a.b.c.d`）の拒否を検証する。
`::` / `::1` は IPv6 のまま保持し、既存の IPv6 ループバック運用を壊さない。

| テスト（配置） | test model / 期待結果 |
| --- | --- |
| `trusted_forwarded_header_drops_trailing_peer_and_uses_original_client`（network.rs） | `client, proxy1, peer` で client。単一ホップ・ピア連続追記・空白・mapped ピア混在でも元クライアントを採用 |
| `trusted_forwarded_header_overwrite_style_uses_last_entry`（network.rs） | 末尾がピアでなければ末尾を採用。途中にピアが現れても誤判定しない |
| `forwarded_header_ignored_when_proxy_untrusted_multi_value`（network.rs） | 信頼プロキシ未設定時は複数値 XFF 全体を無視しピアを返す |
| `trusted_forwarded_header_with_invalid_entry_is_rejected`（network.rs） | 不正 IP・空エントリが1つでもあれば None（fail-closed） |
| `ipv4_compatible_form_is_canonicalized_to_ipv4`（network.rs） | `::192.0.2.1` は `192.0.2.1` へ正規化。`::` / `::1` / `2001:db8::1` は IPv6 を維持 |
| `evaluate_access_rejects_ipv4_compatible_form`（network.rs） | ピア・XFF・上書き型 XFF 末尾の compatible 形式を `Ipv4Forbidden` で拒否 |
| `test_trusted_proxy_append_style_xff_uses_original_client`（tests.rs） | 追記型で IPv4 クライアントを採用し 403（末尾ピア採用の回帰を検出） |
| `test_trusted_proxy_overwrite_style_xff_uses_last_entry`（tests.rs） | 上書き型は末尾の IPv6 を採用し 200 |
| `test_untrusted_proxy_ignores_multi_value_xff`（tests.rs） | 信頼プロキシ外は複数値 XFF を無視しピアで 200 |
| `test_ipv4_compatible_peer_is_rejected`（tests.rs） | `::192.0.2.1` ピアを 403 で拒否 |
| `test_ipv4_compatible_forwarded_client_is_rejected`（tests.rs） | 信頼プロキシ経由 XFF の `::192.0.2.1` を 403 で拒否 |

設計技法は同値クラス（追記型／上書き型／信頼外）と境界値・不正入力の網羅であり、
実プロキシ環境・多段構成・WAF は引き続き未検証である。

### CAS 2段目と失効閾値レースの検証

- PAY-04: `verify_order` の確定処理は2段目の UPDATE も CAS で、`tx.changes() != 1` のとき
  `AppError::Conflict`（HTTP 409、`order status changed during finalization`）を返す。
  1段目の CAS は `test_payment_verify_cas_rejects_stale_status` が検証済みで、
  古い書き込みが勝たないことを確認している。
- R2-BIND-WALLET: `bind_wallet` も同じ単一ライター契約を満たす。`write_lock` 取得後の
  単一 tx で `status` / `expires_at` を読み直し、期限再判定（`reject_if_expired`）の後で
  `status = 'PENDING'` を CAS 条件に `wallet_bound = 1` を更新する。`tx.changes() != 1`
  は `AppError::Conflict`（HTTP 409）で拒否し、競合時に所有証明を上書きしない。
  固定文言は `order status changed during wallet binding` で、正常系は 1 件更新で従来どおり
  成功する。既存の `test_bind_wallet_rejects_legacy_order_only_signature` 内に、
  status が PENDING 以外へ進行したケースの 409 と未書き込みのアサーションを追加した
  （テスト関数数および各種固定数値は不変）。
- ID-05: 再認証時に `revoked_subs` の閾値をメモリから取り出し、DB は
  `DELETE FROM revoked_subs WHERE sub = ? AND revoked_at <= ?` で古い閾値以下のみ削除する。
  これにより削除前に back-channel logout が登録した新しい失効が残る。既存の関連テストは
  `test_backchannel_logout_for_sub_allows_reauthentication` と `test_purge_stale_revoked_subs`。
- リフレッシュ回転（R2-REFRESH-MAXAUTH）: `/auth/refresh` は write_lock → `BEGIN` → 消費 `UPDATE` → `changes()` 判定 → `INSER
  T`（`issue_refresh_token_in_tx`）→ `COMMIT` の単一 tx で実行する。変更0件（競合による再利用）は `tx.rollback()` の後にガードを解放し、別tx の `rev
  oke_refresh_family` へつなぐ（ガードを保持したまま再取得するとデッドロックするため、refresh 内では別tx を取る `issue_refresh_token` を呼ばない）。BAN チェック
  と claims/JWT 事前計算は `BEGIN` 前に配置する。`test_refresh_rotates_token_and_issues_new_session`（旧 `used_at` 非NULL・新 `u
  sed_at` NULL）と `test_refresh_reuse_revokes_family` で検証する。
- セッション発行の BAN 原子性（R2-SERVER）: `/auth/verify` は事前 `ensure_user_not_banned`
  に加え、`write_lock` 配下の write tx 内で `users.is_banned` を再読し、有効な BAN
  （`ban_until IS NULL OR ban_until > datetime('now')`）なら 403 `User is banned` で拒否する。
  事前チェック通過後に BAN がコミットされても `iat` 採番号前に発行が止まることを、
  既存 `test_auth_verify_write_waits_for_write_lock` 内へのアサーション追加で検証する
  （新規 test 関数なし、test 数不変）。正常 200・BAN 後 401 等の既存期待値は不変。
- 専用テスト名は確定済みで、PAY-04 は `test_payment_verify_stage2_cas_rejects_race_after_detected`、
  ID-05 は `test_reauthentication_delete_keeps_newer_revocation_threshold` が検証する。
- R2-LOGOUT-TOKEN: logout token の `iat` を失効閾値 `revoked_subs.revoked_at` として扱い、UPSERT の
  `changes()` が0件（同一 `iat` の再送、または古いトークン）ならメモリ更新と
  `revoke_refresh_tokens_for_sub` をスキップして200を返す。新鮮性は `validate_nbf = true` と
  `iat > now + clock_skew_secs` の401拒否で担保し、nbf 欠落は受理する。検証は
  `test_backchannel_logout_replay_keeps_revoked_at_and_reauthenticated_session` /
  `test_backchannel_logout_rejects_future_iat_and_nbf` /
  `test_backchannel_logout_stale_iat_does_not_advance_threshold` の3件（新規のみ、固定数値は不変）。

### 失効保持期間・callback BAN 原子性・OIDC 署名フローの検証（R2-BAN）

- 失効保持期間: `revoked_subs` の purge cutoff をセッション TTL（24h）+60 秒から
  共通の `REVOCATION_RETENTION_SECS`（refresh TTL 30 日 + マージン 60 秒）へ延長した。
  refresh は `revoked_subs` の存在だけでセッション延長を拒否するため、
  refresh token が有効な 30 日間は失効閾値を残す必要がある。境界は既存
  `test_purge_stale_revoked_subs`（期待値を変えず古い側のシード時刻のみ相対関係を
  維持して更新）と新規 `test_purge_stale_revoked_subs_keeps_refresh_window`
  （25 時間前は保持、31 日前は回収）で検証する。並行 purge の
  `test_purge_waits_for_write_lock` のシードも同じ保持期間へ更新した。
- callback BAN 原子性: `/auth/oidc/callback` は事前 `ensure_user_not_banned` に加え、
  `write_lock` 配下の tx 内で `users.is_banned` / `ban_until` を再読し、有効 BAN
  ならコミット前に rollback して 403 `User is banned` を返す。既存
  `test_oidc_callback_rejects_banned_user` が固定文言・トークン非発行を検証する。
  同テストは送信前に BAN をコミットするため事前チェックで拒否され、事前チェック通過
  後の BAN コミットという競合窓の再読パスは未カバー（原子性はコードレビューで担保）。
- UNBAN の失効解除: `unban_user` は BAN 由来の `revoked_subs` 閾値を同じ tx 内で
  削除し、コミット後に positive-only キャッシュからも除去する。新規
  `test_unban_clears_revoked_subs_threshold` が DB 側の削除を検証する。
- OIDC 署名フロー: callback 初回登録で `public_key_der` に subject 由来の
  「署名鍵未登録」プレースホルダを保存し、NULL による下流署名検証の 500 を回避する。
  実鍵登録済みの主体は上書きしない。鍵未登録の固定文言 403/501 は下流の署名検証側
  で扱う方針とし、upload / vote の既存期待値は不変である。
- 既存の固定数値（E2E69 / check23 / doctor53 / migrations14）は不変。新規 test 関数は
  上記2件のみで、既存 test 関数名は変更していない。

### 暗号検証エラーパスの検証（AUDIT-01）

`common/src/lib.rs` の12テストで、公開鍵 DER・署名バイト・監査スナップショットの
エラーパスとハッシュ連鎖（`verify_chain_*`）を検証する（3a6bd64）。テスト名は次のとおり。

- `verify_rsa_signature_rejects_invalid_public_key_der`: 空・不正 ASN.1・ゴミ DER を拒否。
- `verify_rsa_signature_rejects_wrong_key`: 別鍵は `InvalidSignature`、正鍵は受理（対照付き）。
- `verify_rsa_signature_rejects_invalid_signature_bytes`: 欠落・反転・ゼロ埋め署名を拒否。
- `verify_audit_snapshot_rejects_invalid_base64_signature`: 不正 base64 署名を検証前に拒否。
- `verify_audit_snapshot_rejects_wrong_key`: 正しい署名も別公開鍵では検証失敗。
- `sign_audit_snapshot_rejects_unsupported_version`: 未サポート version を fail-closed で拒否。
- `audit_snapshot_entry_verify_hash_accepts_match_and_rejects_mismatch`: 保存済みハッシュと
  v2 再計算値の一致判定。改変値・v1 ハッシュは拒否。
- `public_key_der_from_private_pem_rejects_invalid_pem`: 不正 PEM を拒否。
- `verify_chain_accepts_valid_unmasked_snapshot`: 署名・連鎖・連番・Merkle root が有効な
  スナップショットを受理する（正常系の対照）。
- `verify_chain_rejects_log_id_gap_with_valid_signature`: 署名は有効なまま `log_id` の
  飛ばし（連番不連続）を拒否する。
- `verify_chain_rejects_broken_previous_hash_with_valid_signature`: 再署名済みでも
  `previous_log_hash` の改ざんを chain mismatch で拒否する。
- `verify_chain_rejects_broken_merkle_root_with_valid_signature`: 再署名済みでも
  Merkle root の改ざんを拒否する。

### 決済ユーティリティの境界の検証（PAY-01〜03）

`server/src/tests.rs` の5テストで、換算・確認数・署名復元・期限の境界を検証する（7821611）。
テスト名は次のとおり。

- `test_jpy_to_wei_bounds`: 0 / 上限超 / i64・u64 境界を拒否し上限値まで正確に換算。
- `test_required_confirmations_by_chain`: 4チェーンの確認数と未対応チェーンの `i64::MAX`。
- `test_recover_ethereum_address_rejects_invalid_signatures`: 非 hex・長さ不正・
  recovery id 不正・復元失敗の拒否。
- `test_recover_ethereum_address_accepts_hex_variants`: 0x 有無と recovery id の
  27/28 形式を同じアドレスへ復元。
- `test_order_expiry_boundary_is_inclusive`: `expires_at == now` も期限切れ（<= 判定）。

### JPKI 証明書エラーパスの検証（ID-01）

`server/src/jpki.rs` の `verify_presentation` を対象に、base64 デコード・X.509 パース・
有効期限・自己署名（DN 一致 + 暗号学的検証）・公開鍵ポリシーのエラーパス10件を検証する（a6eba14 の5件に
鍵ポリシー強化の4件と issuer DN 迂回検知の1件を追加）。`assert_unauthorized_contains` が
`AppError::Unauthorized` と固定メッセージの部分列を確認する。

| テスト | 入力 | 期待 Unauthorized メッセージ |
| --- | --- | --- |
| `invalid_base64_token_is_unauthorized` | `!!!not-base64!!!` | `Invalid JPKI presentation token` |
| `non_x509_bytes_are_rejected` | `AAECAw==`（base64 で `00 01 02 03`） | `X.509 parse failed` |
| `expired_certificate_is_rejected` | rcgen で `not_before=2020-01-01` / `not_after=2021-01-01` の証明書 | `Certificate is expired or not yet valid` |
| `envelope_without_credentials_falls_back_to_raw_token` | `{"certificate":null,"verifiableCredential":null}` | `Invalid JPKI presentation token`（JSON テキストを生 base64 としてデコードし失敗） |
| `empty_token_is_rejected` | `""`（空バイト列は有効な base64） | `X.509 parse failed` |
| `rejects_self_signed_rcgen_certificate` | rcgen の自己署名リーフ（ECDSA P-256、発行者=主体かつ自身の鍵で署名検証可能） | `Self-signed JPKI certificate is not trusted` |
| `rejects_self_signed_rsa_2048_certificate` | 自己署名の RSA 2048bit リーフ（発行者=主体かつ自身の鍵で署名検証可能、鍵長は満たすが信頼アンカー未確定） | `Self-signed JPKI certificate is not trusted` |
| `rejects_ca_signed_ecdsa_certificate` | CA 署名の ECDSA P-256 リーフ | `JPKI leaf public key must be RSA 2048 bits or stronger` |
| `public_key_policy_requires_rsa_2048` | 鍵ポリシー境界（RSA 2048 受理 / RSA 1024 と非 RSA 拒否） | `public_key_meets_policy` の真偽を直接検証（`assert!`、Unauthorized メッセージではない） |
| `rejects_leaf_signed_by_issuer_key_with_different_dn` | issuer≠subject だが同一 PKCS#8 秘密鍵で署名したリーフ | `Self-signed JPKI certificate is not trusted` |

正常系の対照として `mock_token_requires_allow_flag` / `mock_token_with_key_succeeds` /
`accepts_ca_signed_rsa_2048_certificate`（CA 署名の RSA 2048bit リーフを受理）も維持する。
実JPKI実機・失効確認は未検証。

### モック経路の所有証明の検証（ID-08）

`auth_verify` の開発用モック分岐に限り、`VerifyRequest.proof_of_possession`（省略可）
をハンドラ経由で検証する。署名は base64(STANDARD)、公開鍵は `public_key_der`（SPKI DER）
をそのまま流用し、証明書パースは通さない。検証対象は `/auth/login` が発行し
5分間単回消費で保持する nonce である。署名デコード後512バイトと公開鍵DER16 KiB の
上限超過はデコード前後の二段で拒否する（DoS 対策）。`ProofOfPossession` は
`Deserialize` のみを derive するため、リクエスト側のフィールドは
`skip_serializing` とし、ハンドラ tests では JSON を明示的に組み立てて送る。

| テスト（`server/src/tests.rs`） | 入力 | 期待結果 |
| --- | --- | --- |
| `test_auth_verify_pop_valid_signature_is_accepted` | 発行 nonce を CA 署名 RSA 2048 鍵で署名し、base64 化して添付 | 200 |
| `test_auth_verify_pop_tampered_nonce_is_rejected` | 署名対象を別文字列に差し替え／`pop.nonce` を差し替え | 401 |
| `test_auth_verify_pop_other_key_signature_is_rejected` | 別の秘密鍵で発行 nonce に署名 | 401 |
| `test_auth_verify_pop_is_opt_in` | PoP を省略（公開鍵も None） | 200（従来どおり受理） |

実証明書経路の `verify_presentation` と PoP 必須化（段階4）は本段階の対象外で、
E2E 定義数は69件のまま変更しない。

### ログアウト削除 Cookie の検証（ID-03）

`POST /auth/logout` は DB 側のファミリ失効に加え、ブラウザへ残った Cookie を即時削除する。

- `test_logout_clears_refresh_cookie`: 提示した `refresh_token` の値をエコーせず、
  `refresh_token=;`（空値）、`Path=/auth;`、`HttpOnly`、`SameSite=Strict`、`Max-Age=0`
  を含む `Set-Cookie` を返す。ステータスは200、本文は既存の `true` を維持する。
- `test_logout_deletion_cookie_follows_oidc_cookie_secure`: OIDC 設定の `cookie_secure` が
  false / true のとき削除 Cookie の Secure 有無が追随する（発行側と単一ソースを共有）。
- `test_logout_deletion_cookie_secure_by_default_without_oidc`: OIDC 未構成（mock 認証等）では
  環境変数に関わらず Secure 付き（fail-closed）。

### 全レスポンスのセキュリティヘッダの検証（ID-06）

`security_headers` を CORS 層の外側にも置き、CorsLayer が短絡するプリフライトにも
`X-Content-Type-Options: nosniff` と `X-Frame-Options: DENY` を付与する。
`assert_security_headers` で次の応答を検証する。

| テスト | 対象応答 |
| --- | --- |
| `test_security_headers_on_error_response` | IP ゲートの 403 |
| `test_security_headers_on_cors_preflight` | OPTIONS プリフライト（CorsLayer の短絡、成功ステータス 200/204） |
| `test_security_headers_on_rate_limited_response` | レート制限の 429（1回目 404、2回目 429） |
| `test_security_headers_on_internal_error_response` | IPFS ゲートウェイ未設定の 500 |
| `test_security_headers_on_no_content_response` | `POST /metrics/client-events` の本文なし 204 |
| 既存アサーション | 通常 200 のコンテンツ応答 |

### リクエスト相関 ID の検証（OBS-01 / OBS-02）

全レスポンスの `x-request-id` 契約を仕様化する。新規テストは追加せず、既存実装の
`server/src/tests.rs` 3件を根拠とする。

- ユニーク性: リクエストごとに不同な v4 UUID を生成し、正規形（小文字・ハイフン区切り）で
  `x-request-id` ヘッダへ格納する。
- 付与範囲: IP ゲートの 403 などのエラー応答と、CorsLayer が短絡する OPTIONS プリフライト
  （成功ステータス 200/204）にも付与し、既存のセキュリティヘッダを壊さない。
- extensions 格納: 生成 ID は `RequestId` として request extensions へ格納する。
  ヘッダ値の正規ソースは extensions の値で、ハンドラは `Extension<RequestId>` から
  同じ値を取得できる。アクセスログは task-local の相関 ID を使う。
- テスト: `test_request_id_header_is_unique_uuid`（3リクエストで v4・正規形・一意）、
  `test_request_id_header_on_error_and_preflight_responses`（403 / OPTIONS）、
  `test_request_id_stored_in_request_extensions`（extensions とヘッダ値の一致）。

### 同時リクエスト上限の検証（NET-04）

- `test_max_concurrent_requests_defaults_and_bounds`: 実装は
  `.filter(|n| (1..=256).contains(n)).unwrap_or(DEFAULT)`。未設定 / `16` / `1` / `256` は
  それぞれ 16 / 16 / 1 / 256。`0` / `-1` / `bad` / 空 / `257` /
  `999999999999999999999999` はすべて既定16へフォールバック（上限へクランプしない）。
- `test_concurrent_request_limit_rejects_second_and_releases_permit`: 同時実行上限1、
  レート制限100/min の状態で `/hold` を `Notify` で保持する。`available_permits()` が 0 の間に
  `/fast` の2件目は429、1件目を解放して200で完了すると permit が1に戻り3件目は200。

### proptest の検証（PROP-01〜04）

`server/src/tests.rs` の `mod property_tests` に4件。モジュールへ
`#![proptest_config(ProptestConfig::with_cases(128))]` を設定しているため各128ケース。
`proptest` は `server/Cargo.toml` の dev-dependencies（1.12）で、テスト専用とする（79ca230）。

- `prop_jpy_to_wei_boundaries_and_reference`: `0` / `MAX_ORDER_JPY` / `MAX_ORDER_JPY + 1` /
  `u64::MAX` / 任意の `u64` で panic しない。0 と上限超は `Err`、範囲内は
  ADR-0003 の `WEI_PER_JPY = 1_000_000_000_000_000_000` で乗じた独立参照値と一致する。
- `prop_is_valid_cid_only_accepts_canonical_ascii`: 既知 CID（v0/v1）、空文字、
  `🍣` 50連続、`a` 129連続、任意文字列で panic しない。空・非ASCII・128バイト超は拒否し、
  受理値は v0（`Qm` + base58btc 計46文字）または v1（`b` + base32lower 1文字以上）の
  独立 oracle を満たす。既知の正規 CID は必ず受理する。
- `prop_summarize_reason_is_bounded_and_idempotent`: 任意の `String` で panic せず、
  結果は120 Unicode 文字以下、空文字の対応を保持、再適用で値が変わらない（冪等）。
- `prop_summarize_reason_truncates_safe_text_at_char_boundary`: あ / 知 / A / b / 空白 /
  改行 / 🍣 / 。から0..=160個を連結した PII 非含有入力で、120文字以下は等価、
  超過は先頭119文字＋`…` となる（Unicode 境界を維持）。

根拠は金額が ADR-0003 の 10^18 最小単位、CID が v0/v1 の文字集合と形式、
要約が MOD-08 の120文字上限と PII 除去の方針。設計技法はプロパティベーステストによる
不変条件の検証と、仕様から導いた独立 oracle との照合である。

### 監査理由マスク・投票署名 v2・価格 NULL・エクスポート解析の検証（AUDIT-03 / MOD-12 / CONTENT-10 / EXPORT-11）

直近の未コミット変更7件のうち6件は `server/src/tests.rs`、1件は `client/src/lib.rs` の
クライアント側単体テストである。いずれも E2E 定義を増やさず、E2E 定義数は当時59件のまま（現在69件）。

#### AUDIT-03: 監査理由の表示マスクと entry_hash の生値維持

`test_get_content_audit_masks_reason_but_preserves_entry_hash` が、PII を含む生 reason
（`contact user@example.com https://example.com/a code 1234`）を登録した上で
`GET /content/{cid}/audit` を呼び、次を検証する。

| 検証点 | 期待結果 |
| --- | --- |
| レスポンスの `reason` | `common::summarize_reason(raw_reason)` と一致し、`[redacted]` を含む。生 reason・メール・URL・4桁数字は本文に現れない |
| レスポンスの `entry_hash` | 生 reason から `calculate_hash_v2()` で計算した値と一致（マスク後では再計算しない） |
| DB の `reason` / `entry_hash` | 更新前と同じ生値。再計算値も DB の `entry_hash` と一致 |

#### 監査 Reason 注記の案内文言

FORENSIC_TIMELINE の各 Reason 行に添わる注記は、UI から到達可能な案内へ変更した。
`client/src/lib.rs` では `※ 表示は要約です。全文は運用者へお問い合わせください` を表示し、
旧文言 `※ 表示は要約です。全文は署名付きエクスポートで確認` は使用しない。
`entry_hash` の生値表示と DB 全文は不変で、完全検証は従来どおり AUDIT-01 が担う。
E2E は `e2e/tests/integrity.spec.ts` の FORENSIC_TIMELINE 既存 test 内で同注記が3行あることを検証し
（新規 `test()` なし、integrity6件・全体69件を維持）、DB 全文や署名付きエクスポートの本文には触れない。

#### MOD-12: 投票署名 v2 の4件

`moderation_vote_message_v2` は `domain` / `report_id` / `result` / `juror_hash` の4項目を
含む JSON（`domain` の値は `moderation-vote-v2`）を生成する。`mod_vote` はこの v2
メッセージのみを署名検証に使い、旧形式は受理しない。

| テスト | 操作 | 期待結果 |
| --- | --- | --- |
| `test_moderation_vote_message_v2_binds_all_fields` | report_id / result / juror_hash を1つずつ変えてメッセージを比較 | 各々で不一致。同一入力は一致。JSON の4項目は確定値どおり。旧 v1 形式とも不一致 |
| `test_vote_v2_signature_accepted` | v2 メッセージへ正しい秘密鍵で署名して投票 | 200、`votes` に1行記録 |
| `test_vote_rejects_legacy_v1_signature` | 旧 `moderation_vote_message` のメッセージへ署名 | 401（UNAUTHORIZED）、票は0行 |
| `test_vote_rejects_signature_bound_to_other_juror` | B の鍵で A の juror_hash に束縛したメッセージへ署名し B として投票 | 401、票は0行 |

#### MOD-13: 公開集計ルート `GET /mod/stats/public`

ADR-0008 段階2として、`AuthClaims` のみを要求する公開集計ルートを `/mod/stats` の直後に追加した。
`require_moderation_operator` は呼ばず、認証済みなら運用者登録の有無にかかわらず 200 を返す。
集計は既存の `build_mod_stats` を再利用し、純粋関数 `build_public_mod_stats` が
公開ホワイトリストへ縮約する（`generated_at` は引き継ぐ）。

`/mod/stats` と `/mod/stats/public` の双方で、3 表（`content_audit_log` /
`governance_audit_log` / `reports`）の読み取りを `begin_write_tx`（libsql の
`Connection::transaction()` = `BEGIN DEFERRED`）配下で行い、単一スナップショットとして
集計する。`require_moderation_operator`（`/mod/stats`）と `AuthClaims` 認証
（`/mod/stats/public`）は BEGIN より前に解決し、トランザクションは読み取り専用でも
明示的に `commit` する。宣言順は `(_write_guard, tx)` とし、コミット後にガードを
ドロップして `write_lock` を解放する。クエリが `?` で早期 return した場合は
`libsql::Transaction` の Drop が ROLLBACK し、中途半端なトランザクションや共有ロックは
残らない。これは R5-04 `/me/export` の `collect_export_snapshot` と同じ読み取り
トランザクション方式である。認可・レスポンス型・集計ロジック・各テストの期待値は不変。

| テスト | 配置 | 検証する test oracle |
| --- | --- | --- |
| `test_public_mod_stats_requires_authentication` | `server/src/tests.rs` | 認証ヘッダなしでは 401。`AuthClaims` extractor が認証を強制すること |
| `test_public_mod_stats_allows_general_sub` | `server/src/tests.rs` | 運用者未設定の一般 sub で 200。空の集計が返ること |
| `test_public_mod_stats_allows_operator_sub` | `server/src/tests.rs` | 登録済み運用者 sub でも同じ契約で 200 であること |
| `test_public_mod_stats_response_contains_no_forbidden_fields` | `server/src/tests.rs` | 機微センチネルと `reason` / `cid` / `user_hash` / `bans` / `unbans` / `unpins` が本文に現れないこと。キー集合が `generated_at` / `monthly` / `totals`、totals が5項目、monthly が5項目であること |
| `test_build_public_mod_stats_matches_source` | `server/src/tests.rs` | `build_mod_stats` の結果から公開項目が同値に変換されること。BAN/UNBAN/UNPIN の件数が public 側へ漏れないこと |

既存の E2E 定義は変更せず、E2E69件・check23・doctor53・migrations14 の各ゲートは不変である。

#### CONTENT-10: 一覧の price NULL

`test_list_contents_null_price_is_returned_as_zero` が、`price` が NULL の Approved 行を
挿入して `GET /content` を呼ぶ。200 で1件返り、`price` は0（購入不可）であることを検証する。
実装は `build_content_query` の `SELECT` を `COALESCE(price, 0) AS price` とし、
JOIN / WHERE / ORDER には影響しない。

#### EXPORT-11: エクスポート解析失敗の固定文言

`client/src/lib.rs` の `export_status_message` はレスポンスを `DataExport` へ解析し、
失敗時は `EXPORT_PARSE_ERROR`（`エクスポート結果の解析に失敗しました。`）を返す。
`export_parse_failure_uses_fixed_phrase_without_success_status` が、
`{"version":"invalid","contents":[]}` で固定文言が返ることと、正常系
（`{"version":1,...}`）で従来どおり `エクスポート完了: 1 件のレコードをダウンロードしました。`
が返ることを検証する。エラー時は `export-error` のみへ表示し `export-status` は空のまま。

E2E 側は `e2e/tests/export.spec.ts` のダウンロード成功シナリオ（既存1件目）へフェーズを追加し、
モック応答を `version: 'invalid'` の不正 JSON へ差し替えて同じボタンを再クリックする。
`export-error` に固定文言 `エクスポート結果の解析に失敗しました。` が `role="alert"` 付きで出る、
`export-status` は `toHaveCount(0)` であることを検証する（新規 `test()` なし、export3件・全体69件を維持）。

### 監査一覧の上限・新着順の検証（AUDIT-04）

`get_content_audit`（`GET /content/{cid}/audit`）は全行を `Vec` 化せず、SQL の
`ORDER BY log_id DESC LIMIT ?` で最新 `MAX_AUDIT_LIST`（500件）だけを返す。
レスポンスは `previous_log_hash` を含まず `reason` は `summarize_reason` でマスクされるため、
レスポンス単体ではハッシュ連鎖を検証できない。古い行を含む完全検証は AUDIT-01 の
署名付きエクスポートまたは DB 直接読取りを使用する（DB 内ハッシュ連鎖は第三者検証可能な監査ではない）。

| テスト | 配置 | 検証する test oracle |
| --- | --- | --- |
| `get_content_audit_caps_at_max_audit_list` | `server/src/tests.rs` | `MAX_AUDIT_LIST == 500`。`log_id` 1〜501 を投入すると返却件数は500、先頭は501、末尾は2、全 `log_id` が `(2..=501).rev()` の降順（最古の1は除外）。`ORDER BY` を ASC へ戻すと失敗 |
| `test_get_content_audit_masks_reason_but_preserves_entry_hash` | `server/src/tests.rs` | 表示用 `reason` のみマスクし `entry_hash` は DB の生値を維持（AUDIT-03、既存の回帰） |
| `/audit` モックの FORENSIC_TIMELINE E2E | `e2e/tests/integrity.spec.ts` | `log_id` DESC の3件を返すモックで、`content-card` 内の FORENSIC_TIMELINE の `Reason:` 行が新しい順（3→2→1）に並ぶこと |

サーバー側テストは `server/src/tests.rs`、表示順の確認は `e2e/tests/integrity.spec.ts` に置く。
既存のハッシュ連鎖検証（`test_audit_log_chain_integrity` 等）は DB を直接読むため、
LIMIT 追加の影響を受けず、署名付きエクスポートの改ざん検証は AUDIT-01 が継続して担う。

## 4. 実行コマンド

```bash
cargo test --workspace
cargo test -p server jpki   # ID-01 JPKI 証明書エラーパス（10件）
cargo test -p server auth_verify_pop   # ID-08 PoP opt-in ハンドラ回帰（4件）
cargo test -p server logout_clears_refresh_cookie   # ID-03 ログアウト削除 Cookie
cargo test -p server backchannel_logout   # ID-02/03/05 back-channel logout（新鮮性・再送冪等を含む既存＋新規3件）
cargo test -p server security_headers   # ID-06 nosniff/DENY（OPTIONS 204 含む）
cargo test -p server metrics_count   # MET-02 最外側メトリクス4件（429 / IPv4 403 / 413 / OPTIONS）
cargo test -p server redirect   # NET-05 SSRF リダイレクト2件
cargo test -p server forwarded   # NET-06 XFF 解釈 4件（network.rs）
cargo test -p server xff   # NET-06 追記型/上書き型/信頼外 3件（tests.rs）
cargo test -p server ipv4_compatible   # NET-06 IPv4互換形式 4件
cargo test -p server --bin doctor ipv4_localhost   # doctor 追加警告 2件
cargo test -p server --bin doctor trusted_proxy   # doctor 追加警告 2件
cargo test -p server test_bind_wallet_rejects_legacy_order_only_signature   # PAY-05 旧形式署名の401拒否
cargo test -p server max_concurrent_requests_defaults_and_bounds   # NET-04 境界値
cargo test -p server concurrent_request_limit_rejects_second_and_releases_permit   # NET-04 実リクエスト
cargo test -p server property_tests   # PROP-01〜04（各128ケース）
cargo test -p server --lib upload   # CONTENT-01（MIME・価格）のAPI回帰のみ
cargo test -p server --lib mod_stats   # R5-09 集計・認可・プライバシーの回帰のみ
cargo test -p server public_mod_stats   # MOD-13 公開集計ルートの回帰のみ（5件）
cargo test -p server --lib me_export   # R5-04 エクスポートの回帰のみ
cargo test -p common summarize_reason   # 要約の単体テスト（PII除去・境界・日本語）
cargo test -p server list_reports       # GET /mod/reports の要約適用・全文非漏えいの回帰のみ
cargo test -p server test_get_content_audit_masks_reason_but_preserves_entry_hash   # AUDIT-03 監査理由マスクと entry_hash 維持
cargo test -p server get_content_audit_caps_at_max_audit_list   # AUDIT-04 監査一覧の上限・新着順（501件投入で先頭501・末尾2・降順）
cargo test -p server moderation_vote_message_v2   # MOD-12 メッセージの全フィールド束縛
cargo test -p server test_vote_v2_signature_accepted   # MOD-12 正常系の受理
cargo test -p server test_vote_rejects_legacy_v1_signature   # MOD-12 旧形式署名の401拒否
cargo test -p server test_vote_rejects_signature_bound_to_other_juror   # MOD-12 別投票者向け署名の転用拒否
cargo test -p server test_list_contents_null_price_is_returned_as_zero   # CONTENT-10 価格 NULL の0扱い
cargo test -p client export_parse_failure_uses_fixed_phrase_without_success_status   # EXPORT-11 クライアント側
cargo run -p server --bin doctor -- --strict   # CFG-01 設定診断（問題ありで exit 1）
for f in scripts/*.sh e2e/*.sh; do bash -n "$f"; done    # シェル構文検査（対象を glob で明示）
for f in scripts/*.sh e2e/*.sh; do shellcheck "$f"; done # シェル静的解析（apt_packages で導入）
shfmt_files="$(shfmt -l scripts/*.sh e2e/*.sh)" || exit 1; if [ -n "$shfmt_files" ]; then echo "shfmt: unformatted: $shfmt_files" >&2; exit 1; fi   # shfmt -l の出力が非空なら失敗（apt_packages で導入）
cd e2e && npm test
cd e2e && npm test -- auth.spec.ts   # 認証 E2E（3件: モックJPKIログイン / 年齢ゲート拒否 / LOG OUT 回帰＋501固定文言・verify 未送信）のみ
cd e2e && npm test -- build-gate.spec.ts   # ビルドゲート E2E（trunk archive 検証 3件 + version gate 9件 + E2E_PROJECTS選択 3件の計15件）のみ
cd e2e && npm test -- search.spec.ts   # R5-01 検索 E2E のみ
cd e2e && npm test -- favorites.spec.ts   # R5-02 お気に入り E2E のみ
cd e2e && npm test -- economy.spec.ts   # Support & Ledger E2E（3件: 取得失敗 HTTP 500 / route.abort の2件含む）のみ
cd e2e && npm test -- moderation.spec.ts   # R5-09 透明性ダッシュボード + 投票フィードバック + 投票一覧エラー・集計解析エラー E2E（14件）
cd e2e && npm test -- export.spec.ts   # R5-04 データエクスポート E2E のみ
cd e2e && npm test -- metrics.spec.ts   # 中立化URL E2E（3件: MET-01 / NEUT-URL-02 protocol-relative / NEUT-URL-03 再送抑制）のみ
cd e2e && npm test -- real-api.spec.ts   # 実サーバー結合 E2E（4件）のみ
cd e2e && npm test -- error-paths.spec.ts   # エラーパス E2E（6件: DASH-ERR / export network / AUTH-500 / AUDIT-500 / CONTENT-ERR-404 / CONTENT-ERR-500）のみ
cd e2e && npm test -- integrity.spec.ts   # INT-01 完全性 E2E（6件: 正規/改ざん/R1-13 デコード失敗/FORENSIC_TIMELINE 順序/R2 Audit トグル a11y/CLOSE ボタンのフォーカス復帰）のみ
cd e2e && npm test -- upload.spec.ts   # R1-14 / INT-02 / CONTENT-01 / R2 a11y Upload E2E（6件）のみ
```

E2E の初回実行時は `cd e2e && npm ci` で依存を導入する。
実行対象は条件付きで選択する。`E2E_PROJECTS=chromium,firefox,webkit` の明示指定は
厳守し、未知値は拒否する。ブラウザ導入は選定（キャッシュ走査）より前に実行する。
CI（`CI` 設定時）では未指定でも3プロジェクトを明示指定として扱い、firefox / webkit を
必ず導入・実行する。chromium 単一へフォールバックした場合は CI 限定で
`::error::E2E browser fallback to chromium is not allowed in CI ...` を出して失敗とし、
黙って SKIP しない。ローカル未指定時は firefox / webkit のバイナリ導入と共有ライブラリの
解決可否を検出し、両方実行可能なら3プロジェクトを実行する。片方でも不可なら
`chromium` へフォールバックし、固定形の
`::warning::E2E browser fallback: firefox=<0|1> webkit=<0|1>; running <projects>`
annotation を出す。GitHub Actions の step summary には firefox / webkit の可否と
選択プロジェクトを記録し、フォールバック時も選択プロジェクトを残す。
本ホスト（Kubuntu 26.04）では Playwright 未対応プラットフォームのため
firefox / webkit の実実行は未検証（CI 環境での導入・検証は別途検討）。
開発サーバーとクライアントは `e2e/playwright.config.ts` の `webServer` が
自動起動する（モック認証 `ALLOW_MOCK_AUTH=1`、ローカルでは開発用の
固定 JWT シークレット。`E2E_JWT_SECRET` で上書きできる）。
`channel: chrome` を使うため Google Chrome・クライアントビルド用の
trunk・Node 22 以上が必要である。trunk は CI と同じ
`cargo install trunk --version 0.21.14 --locked` を推奨する。

ローカルでは既存プロセスと `server/local.db` を再利用するため、
クリーンな状態で検証する場合は `E2E_REUSE=0` で起動を強制する。このとき
一時 DB パスが server の `DATABASE_URL` へ渡される（計算のみで未使用だった
問題を修正）。`server/local.db` と稼働中プロセスを確認する。CI は
チェックアウト直後のクリーンな DB とワークスペースを使う。

CI では共通ワークフロー `rust-gates.yml` の追加ゲートとして
`e2e/ci-e2e.sh` を実行する。trunk は v0.21.14 に固定し
（sha256 検証。更新時は version・sha256・資産名の 3 点を同時に変更）、
E2E 用 JWT シークレットは実行ごとに生成する。型検査
（`npm run typecheck`）も同じゲートで実行する。
シェル静的ゲート（`bash -n` / `shellcheck` / `shfmt -l`）も同じ `extra_gates` に追加し、
対象は `scripts/*.sh` と `e2e/*.sh` を glob で明示する（上記「4. 実行コマンド」参照）。
`shfmt` は `shfmt -l` の出力が非空（未整形ファイルあり）のとき失敗とし、`bash -n` / `shellcheck` の行は維持する。
`shellcheck` / `shfmt` の runner 導入は共通ワークフロー側で対応し、導入済みなら
本リポジトリ側の追加導入は不要とする（本リポジトリに install 手順は置かない）。
SC2034 はファイル単位の `-e` 対象から外し、nameref を受け取る関数直前に
`# shellcheck disable=SC2034` を置いて関数単位で除外する（`e2e/ci-e2e.sh`）。

CI では HTML レポート・trace をアップロードしない（追加ゲートの制約）。
失敗時は追加ゲートのログ（`::error::` アノテーションと error-context）で
切り分け、ローカルで `cd e2e && npm test` により再現する。レポートの
添付が必要になった場合は、独立した E2E ジョブへの昇格を検討する。

実JPKI実機・JPYCテスト環境の試験計画は、対応仕様を確定した後に作成する。これらの必須ゲートを通過する前に本番利用資格判定・決済を有効化しない。

### E2E 検索シナリオ（R5-01）

`e2e/tests/search.spec.ts` は `/content` を URL predicate でフックし、送信された `q` を記録しながら
`title` の部分一致で絞り込むモックを返す。各アイテムの `/content/{cid}` と監査エンドポイントも
モックする。ブラウザ URL は同期せず、入力に応じた AJAX 再取得のみを検証する。

1. 認証モック（`ALLOW_MOCK_AUTH=1`）でログインし、BROWSE 画面へ遷移する。初期表示で
   `Moonlit Harbor` と `Neon Alley` の2件、および説明 `Cyberpunk city sketch` とタグ
   `#neon` が表示されることを確認する。
2. `getByLabel('Search')` に `Neon` を入力する。`/content` へのリクエストから記録した
   `q` の値に `Neon` が含まれ、`Neon Alley` が残り `Moonlit Harbor` が消えることを確認する。
3. タグ・種別入力、結果件数告知（`role="status"`）、XSS エスケープ表示は API テスト
   （CONTENT-06〜08）で担保し、本シナリオは `q` の絞り込みとメタデータ表示に焦点を当てる。

### E2E お気に入りシナリオ（R5-02）

`e2e/tests/favorites.spec.ts` の `Favorites / Bookmarks (R5-02)` は、モック認証
（`ALLOW_MOCK_AUTH=1`）でログイン後、一覧カードのトグルボタンからPOST/DELETEを発行し、
`favorite_only=1` の絞り込みまでを通しで確認する（テスト名:
`toggles favorites via POST/DELETE and filters with favorite_only=1`）。

1. BROWSE画面の一覧カードで、未お気に入りのボタンが `☆`（`aria-label="Add to favorites"`、
   `aria-pressed="false"`）であることを確認する。クリックすると `POST /content/{cid}/favorite` が
   送信され、`★`（`aria-label="Remove from favorites"`、`aria-pressed="true"`）へ変わることを確認する。
2. 再度クリックすると `DELETE /content/{cid}/favorite` が送信され、`☆` へ戻ることを確認する。
   通信失敗時はカード内の `data-testid="favorite-error"`（`role="alert"`）に
   `Failed to update favorite (HTTP <status>).` が表示される。`route.abort()` による
   ネットワークエラーの検証では `Network error updating favorite.` が表示される。
   固定文言は `client/src/lib.rs` の `favorite_error_message(None)` と単体テスト
   `favorite_error_message_keeps_existing_http_and_network_text` で担保する。
3. お気に入り状態のコンテンツで検索フォームの `Favorites only` チェックボックスをONにし、
   `/content` へ `favorite_only=1` が付与され、該当コンテンツのみが表示されることを確認する。
   既存の `q` / `tag` / `type` との併用はAPIテスト（FAV-04）で担保する。
4. リクエスト回数と内容を完全一致で検証する。追加後の一覧再取得は `/content` への
   リクエストがちょうど2回で、クエリは `{ q: null, tag: null, type: null, favorite_only: null }`
   の2要素に一致する。一連の操作全体では8回で、`favorite_only=1` や `q` の付与順まで
   配列の完全一致で確認する。お気に入りの POST / DELETE もメソッドとパス
   （`/content/{cid}/favorite`）の完全一致で検証し、過剰な再フェッチや想定外の
   クエリ混入を検出する。

5. 失敗 → 再トグルで旧エラーが消える（`re-toggle after a failure clears the stale favorite-error`）:
   1回目の POST を HTTP 500 で失敗させ、`favorite-error`（`role="alert"`）に
   `Failed to update favorite (HTTP 500).` が表示され、トグルが再活性化して
   `aria-pressed="false"` のままであることを確認する。2回目のトグルを開始すると、
    レスポンスを保留している間でも `favorite-error` は `toHaveCount(0)` で消える。
   このとき POST は `/content/{cid}/favorite` へちょうど2回、`/content` の再取得は
   失敗トグルでは行われず1回のままである。2回目のレスポンス後に `aria-pressed="true"` へ進む。
6. 401 の固定文言（`401 shows re-login guidance in favorite-error`）: POST が401を返すと、
   `favorite-error`（`role="alert"`）に `client/src/lib.rs:1330` の確定値
   `再ログインが必要です。` が表示される。POST は1回、`/content` の再取得は1回のままで、
   トグルは再活性化し `aria-pressed="false"` を維持する。401 専用の文言であり、
   500 の `Failed to update favorite (HTTP 500).` やネットワークエラーの
   `Network error updating favorite.` とは切り替えて検証する。

### E2E 取引履歴エラーシナリオ（ECO-01）

`e2e/tests/economy.spec.ts` の `Support & Ledger Flow (ECO-01)` に追加した2件が、
`GET /transactions` の取得失敗を画面へ告知することを検証する。固定文言は
`client/src/lib.rs` の `LEDGER_ERROR`（`取引履歴の取得に失敗しました。`）の確定値を使う。

1. ECO-01（HTTP 500）: `/transactions` を HTTP 500 で応答させ、本文に非表示確認用の
   文字列を含める。`data-testid="ledger-error"` が `role="alert"` 付きで表示され、
   固定文言 `取引履歴の取得に失敗しました。` になることを確認する。
2. ECO-01（network error）: `/transactions` を `route.abort()` で失敗させ、同様に
   `ledger-error` へ固定文言が表示されることを確認する。
3. いずれのケースも `NO_TRANSACTIONS_RECORDED` が `toHaveCount(0)` で非表示であり、
   取得失敗を「取引なし」と誤表示しないことを確認する。実行例は
   `cd e2e && npm test -- economy.spec.ts`。

### E2E 透明性ダッシュボードシナリオ（R5-09）

`e2e/tests/moderation.spec.ts` の `Moderation & Governance (GOV-01, GOV-02)` に含まれる
`R5-09: should mount operator stats panel only on opt-in and stay silent for general users` が、
モック認証（`ALLOW_MOCK_AUTH=1`）で `GET /mod/stats` の応答を差し替えて3フェーズを確認する。
既存の1 test() 内で完結させ、moderation.spec.ts 14件・フルスイート69件は変更しない。

0. **公開集計（フェーズ0、ADR-0008 段階2）**: MODERATION 画面の初期表示ではオプインを
   待たずに `GET /mod/stats/public` が `Authorization: Bearer mock_jwt_token` 付きで1本だけ
   送られる。`data-testid="public-mod-stats"` に `GENERATED_AT: <generated_at>`、
   `data-testid="public-mod-stats-totals"` に Reports / Concluded / Approved / Rejected /
   Quorum Not Met、`data-testid="public-mod-stats-monthly"` に Month / Reports /
   Concluded / Approved / Rejected が描画される。機微キー文字列（`reason` / `cid` /
   `user_hash` / `bans` / `unbans` / `unpins`）は `body` に現れない。
   `publicStatsRequests` の完全一致で初期表示1本を検証し、この時点で `/mod/stats` は0本。
   BROWSE → MODERATION の再マウントで公開集計は2本目が送られる（公開側は2本）。
   `test.beforeEach` は全テスト共通の `**/mod/stats/public` 既定モック（200・公開DTO）を
   登録し、R5-09 は個別ルートモックで上書きしてリクエストを記録する。
1. **初期表示（一般画面）**: MODERATION 画面を開いても `/mod/stats` は0本である。
   `data-testid="moderation-stats-toggle"`（中立文言 `SHOW STATS`）は表示されるが、
   `data-testid="moderation-stats-section"` / `moderation-stats-error` は0件で、
   `TRANSPARENCY DASHBOARD` と固定文言 `運用者権限が必要です` は `body` に現れない。
2. **オプイン200（運用者パネル）**: トグル押下で1本だけ送信し、200 応答を根拠に
   `TRANSPARENCY DASHBOARD`（`data-testid="moderation-stats-section"`）と
   `GENERATED_AT: <generated_at>` をマウントする。`data-testid="moderation-stats-totals"` に
   Reports / Concluded / Approved / Rejected / Quorum Not Met / Bans / Unbans / Unpins、
   `data-testid="moderation-stats-monthly"` に Month / Reports / Concluded / Approved /
   Rejected / Bans / Unbans / Unpins の列が並ぶことを確認する。
3. **403（パネル内）**: 応答を403へ切り替えて再度トグルを押すと、パネルを黙って閉じ、
   `moderation-stats-section` / `moderation-stats-error` は0件、固定文言も `body` に現れない。
   さらに画面遷移で再マウントしても初期表示は0本のままである。
   固定文言 `運用者権限が必要です` は一般画面への否定オラクルとして検証を維持する。
4. 動的文字列（`generated_at` / `month` / 数値）は既存の `escape_html_text` を通して描画される。
   レスポンスに CID / reason / `user_hash` は無いため表示されず、サーバー側の
   `test_mod_stats_response_contains_no_sensitive_fields` と合わせて個人情報非含有を担保する。
   本シナリオは認証内集計の表示確認であり、第三者公開（ADR-0008 の範囲。
   段階2の公開ルートは MOD-13 で検証）を証明するものではない。
5. `/mod/stats` へのリクエストはトグル押下ごとに1本で、いずれもメソッド `GET`、パス
   `/mod/stats`、`Authorization: Bearer mock_jwt_token` であることを配列の完全一致で検証する
   （初期表示と再マウント後は0本、200→403の2押下で合計2本）。余計な再フェッチや
   未認証リクエストを検出する。
6. `R5-09 (parse error)`: トグル押下後に `/mod/stats` の `totals` へ文字列を入れた不正 JSON
   を返すと、`data-testid="moderation-stats-error"`（`role="alert"`）に固定文言
   `集計の解析に失敗しました。` が表示され、集計カード（`moderation-stats`）は出ない。
   注入したセンチネル `RAW_SERDE_ERROR_SENTINEL_DO_NOT_SHOW` はページ全体で非表示で、
   serde の生エラー（フィールド名・型情報・解析位置）も出ないことを確認する。
   文言は `client/src/lib.rs` の `stats_error_message(StatsFailure::Parse)` の確定値を使う。

7. **未認証401（実サーバー）**: `page.request.get('http://127.0.0.1:3000/mod/stats/public')`
   で認証ヘッダなしのリクエストを送り、ステータス 401 を同じ test() 内で確認する。
   `page.request`（APIRequestContext）は `page.route` の対象外のため実サーバーへ届き、
   client の baseURL（8080）ではなく絶対 URL のポート 3000 を明示する。
   未認証拒否（`AuthClaims` 必須）を E2E でも担保する。

8. 公開集計の解析失敗（既存1件目の追加フェーズ）: `**/mod/stats/public` を200のまま
   `totals` が文字列の不正 JSONへ差し替え、BROWSE → MODERATION で再マウントする。
   `data-testid="public-mod-stats-error"`（`role="alert"`）に固定文言
   `集計の解析に失敗しました。`（`statsParseErrorText`）を表示し、
   `public-mod-stats-content` と `GENERATED_AT` は出ない。センチネル
   `RAW_SERDE_ERROR_SENTINEL_DO_NOT_SHOW` も出さない。公開集計の解析失敗は
   `StatsFailure::Fetch` ではなく `StatsFailure::Parse` へ分類し、運用者向け
   `/mod/stats` と統一する。オプイン押下がないため `/mod/stats` は2本のまま、
   公開集計は3本目が送られる。moderation14件・全体69件は不変である。

### E2E 投票フィードバックシナリオ（VOTE-FB-01〜03）

`e2e/tests/moderation.spec.ts` の `Moderation & Governance (GOV-01, GOV-02)` に追加した
3件が、投票結果の通知・エラー表示を検証する。いずれもモック認証でログインし、
`GET /mod/reports` に `report_id: 42` の通報1件を返して投票キューを表示した状態から始める。
文言は `client/src/lib.rs` の `VOTE_NOTICE` と `vote_error_message` の確定値を使用する。

1. VOTE-FB-01: `/mod/vote` を403で応答させ、サーバー本文に非表示確認用の文字列を含める。
   APPROVE クリック後に `data-testid="vote-error"` が `role="alert"` 付きで表示され、
   固定文言 `投票に失敗しました (HTTP 403).` になることを確認する。`HTTP 403` を含むことと、
   サーバー本文が出ないこと、`vote-notice` が出ないことを合わせて検証する。
   リクエストは `POST /mod/vote`、`Authorization: Bearer mock_jwt_token` の1回のみ。
2. VOTE-FB-02: `/mod/vote` を `route.abort()` で失敗させる。APPROVE クリック後に
   `vote-error` へ固定文言 `Network error submitting vote.` が表示され、
   `vote-notice` が出ないことを確認する。
3. VOTE-FB-03: 403 → 200 → 403 と応答を切り替えて3回投票する。403 ではエラー、
   200 では `vote-notice` に `投票を記録しました。` を表示する。各試行で
   前回の通知・エラーが消えること（`vote-error` / `vote-notice` の `toHaveCount(0)`）と、
   `POST /mod/vote` がちょうど3回であることを確認する。

実行例は `cd e2e && npm test -- moderation.spec.ts`。chromium で14件すべて pass を確認済み。

### E2E 投票一覧エラーシナリオ（REPORTS-ERR-01〜02）

`e2e/tests/moderation.spec.ts` の `Moderation & Governance (GOV-01, GOV-02)` に追加した
2件が、`GET /mod/reports` の取得失敗を画面へ告知することを検証する。文言は
`client/src/lib.rs` の `reports_error_message` / `REPORTS_PARSE_ERROR` を grep して
確定した値を使用する（推測しない）。両ケースともモック認証でログインし、
MODERATION 画面を開いた直後の初回フェッチを差し替える。

1. REPORTS-ERR-01: `/mod/reports` を HTTP 500 で応答させ、本文に非表示確認用の
   文字列を含める。`data-testid="reports-error"`（`role="alert"`）に固定文言
   `投票一覧の取得に失敗しました (HTTP 500).` が表示され、`HTTP 500` を含むこと、
   サーバー本文が出ないことを確認する。`NO_PENDING_VOTES` が出ないことと、
   `GET /mod/reports` が `Authorization: Bearer mock_jwt_token` の1回のみも確認する。
2. REPORTS-ERR-02: `/mod/reports` を `route.abort()` で失敗させる。`reports-error` に
   固定文言 `Network error loading reports.` が表示され、`NO_PENDING_VOTES` が
   出ないことを確認する。
3. REPORTS-ERR-01 の追加フェーズ（解析失敗）: 同じ test() 内でモック応答をステータス200の
   `{ report_id: 'not-a-number', target_cid: 42, reason: 123, created_at: [], status: true,
   debug: 'SERVER_DETAIL_MUST_NOT_BE_SHOWN' }` を要素に持つ配列へ差し替え、再マウントして
   初回フェッチをやり直す。`Vec<Report>` として読めないため `reports-error` に
   `投票一覧の解析に失敗しました。`（REPORTS_PARSE_ERROR）が `role="alert"` 付きで表示され、
   サーバー本文センチネルと `NO_PENDING_VOTES` は出ない。新規 `test()` は追加せず
   REPORTS-ERR-01 の既存1件内へ統合（moderation14件・全体69件を維持）。

実装は失敗時に `reports` を空にせず既存データを維持し、成功時にのみ一覧を更新して
エラーをクリアする。JSON 解析失敗は固定文言 `投票一覧の解析に失敗しました。` を
表示し、解析エラーの詳細は出さない。この文言はクライアント側単体テスト
`reports_error_uses_only_fixed_phrases` で担保する。実行例は
`cd e2e && npm test -- moderation.spec.ts`。chromium で14件すべて pass を確認済み。

### GOV-01 要約表示の E2E（MOD-08 / MOD-09）

`e2e/tests/moderation.spec.ts` の
`GOV-01 (summary): PII を含む元理由は要約済み文字列のみ表示され、メール/電話/URL は露出しない`
が、実サーバー結合を行わずモック `/mod/reports` に要約済み理由を返して UI 側の表示契約を検証する。
要約処理自体の oracle は MOD-08 / MOD-09 のサーバー側テストが担い、ここでは表示と非漏えいを検証する。

1. モックは `reason: '不審な投稿です [redacted]'` の通報1件を返す。これは
   `common::summarize_reason` が元全文
   `不審な投稿です。連絡先: reporter@example.com / 090-1234-5678 / https://phish.example.com/login`
   を要約した後の固定値を想定する（PII は `[redacted]` へ置換される）。
2. モック認証で MODERATION 画面を開くと、`Jury Voting Queue` の `Reason:` 行に
   要約済み文字列と `[redacted]` が表示される（`toContainText`）。
3. 元全文に含まれていたメール `reporter@example.com`、電話 `090-1234-5678`、
   URL `https://phish.example.com/login` の3値について、`Reason:` 行と `body` の両方で
   `not.toContainText` を検証する。サーバー本文・生の PII は画面へ出さない。
4. 本シナリオは定義14件の moderation.spec.ts に含まれ、chromium で pass を確認済み。
   認証内集計の表示確認であり、第三者公開（ADR-0008 の範囲。
   段階2の公開ルートは MOD-13 で検証）を証明するものではない。

### 二重送信ガードの E2E（REPORT-GUARD-01 / VOTE-GUARD-01）

`e2e/tests/moderation.spec.ts` の REPORT-GUARD-01 / VOTE-GUARD-01 は、
`client/src/lib.rs` の `claim_submission` + `BusyGuard` による二重送信防止を UI 側で検証する。
`/mod/report` と `/mod/vote` の応答を `page.route` で意図的に保留し、固定 sleep を使わずに
ボタンの状態遷移を待つ。オラクルは次のとおり。

1. 送信ボタンは初期状態で有効（`toBeEnabled`）かつ `aria-busy="false"` である。
2. 同一タスクで `el.click()` を2回連続で発行する予告なしの高速2クリックでも、
   `POST /mod/report` / `POST /mod/vote` は1本だけである（`expect.poll` で件数を確認）。
3. レスポンス保留中はボタンが `disabled`（`toBeDisabled`）かつ `aria-busy="true"` へ遷移し、
   その状態でもう一度クリックしても POST は増えない。
4. 応答を解放するとボタンは再び有効かつ `aria-busy="false"` へ戻り、
   成功固定文言（通報は status 文言、投票は `data-testid="vote-notice"`）が表示される。
5. 記録したリクエストは1件のみで、`method: 'POST'`、パス `/mod/report` / `/mod/vote`、
   `Authorization: Bearer mock_jwt_token` であることを完全一致で検証する。
   2件目の POST や別パス・別ヘッダのリクエストは存在しない。
6. 本シナリオは `moderation.spec.ts` 全14件に含まれる2件であり、
   定義件数は14件のまま（既存の投票3回完全一致・お気に入り2/8回アサーションは不変）。

### E2E エクスポートシナリオ（R5-04）

`e2e/tests/export.spec.ts` の `R5-04: Export My Data` は、モック認証
（`ALLOW_MOCK_AUTH=1`）でログイン後、Dashboard の `データをエクスポート`
（`aria-label` 付き、`data-testid="export-my-data"`）ボタンからエクスポートを実行する。

1. 認証トークン付きで `GET /me/export` を呼び、`page.waitForEvent('download')` で
   JSONダウンロードがトリガされることを確認する（テスト名:
   `R5-04: GET /me/export を認証トークン付きで呼び、JSON ダウンロードがトリガされること`）。
2. `/me/export` の応答を401に差し替えると、`getByRole('alert')` に
   「ログインが必要です」が表示されることを確認する（テスト名:
   `R5-04: /me/export が 401 の場合は「ログインが必要です」を role=alert で表示すること`）。
3. レスポンス本文の全12表・他ユーザー非漏えい・除外項目・マスク・ヘッダはサーバー側の
   `me_export_*` テスト（EXPORT-01〜06）で担保する。本シナリオは UI からのダウンロード起動と
   エラー表示に焦点を当てる。削除権は未実装のため UI に存在しない。
4. ダウンロード開始失敗（`R5-04: click が例外を投げた場合は固定文言のエラーを表示し、完了文言を出さないこと`）:
   `/me/export` の応答は成功のまま、`HTMLAnchorElement.prototype.click` を例外に差し替えて
   ダウンロード開始だけを失敗させる。`data-testid="export-error"` が `role="alert"` 付きで
   `client/src/lib.rs:753` の確定値 `ダウンロードの開始に失敗しました。` になり、
   `data-testid="export-status"` は `toHaveCount(0)`（成功文言も出ない）。
   これは EXPORT-11 の `エクスポート結果の解析に失敗しました。`（レスポンス解析失敗）とは
   別事象であり、固定文言と testid の両方で切り分ける。
5. `e2e/tests/export.spec.ts` は定義3件（上記1・2・4）。上記1のダウンロード成功シナリオ内へ
   EXPORT-11 の解析失敗フェーズを追加し、`version` が文字列の不正 JSON で
   `export-error` に固定文言、`export-status` は空を検証（既存テスト内へ統合、新規 `test()` なし、
   export3件・全体69件を維持）。chromium で pass を確認済み。
   webkit はホスト側の libicu/libxml2 不在で起動不可（環境依存）のため未検証。

### E2E エラーパスシナリオ（DASH-ERR / R5-04 network / AUTH-500 / AUDIT-500 / CONTENT-ERR-404 / 500）

`e2e/tests/error-paths.spec.ts` は API 自体の挙動ではなく、クライアントが失敗を画面へ告知することを
検証する（6件）。実行コマンドは `cd e2e && npm test -- error-paths.spec.ts`。
テスト名は次のとおり。

1. `DASH-ERR: GET /content が network error のときダッシュボードにエラー表示が出ること`:
   `/content` を abort し、`data-testid="dashboard-error"`（`role="alert"`）に
   `Network error loading content.` が表示される。文言のみではなく testid で特定し、
   ほかの `role="alert"`（例: `login-error`）と混同しないようにする。
2. `R5-04: GET /me/export が network error のとき role=alert にネットワークエラー文言が出ること`:
   `/me/export` を abort し、`data-testid="export-error"` の `role="alert"` に
   `ネットワークエラーが発生しました。` が表示され、成功ステータスが出ない。
3. `AUTH-500: /auth/verify が HTTP 500 のとき login-error に文言が出て UPLOAD が出ないこと`:
   `/auth/verify` を500に差し替え、`data-testid="login-error"` の `role="alert"` に
   `認証に失敗しました (HTTP 500).` が表示され UPLOAD は出ない。
   対照として 501 は別文言へ変換する。`e2e/tests/auth.spec.ts` の LOG OUT 回帰（既存の認証3件のまま）内で
   `/auth/login` を501＋センチネル本文へ差し替えて再クリックし、`data-testid="login-error"` の
   `role="alert"` に固定文言 `本番認証は未実装です。` が出ること、UPLOAD が出ないこと、
   センチネル本文が画面へ現れないこと、未モックの `/auth/verify` が送られないことを検証する
   （`/auth/verify` の501も同文言へ変換。403 / 500 等は既存の `認証に失敗しました (HTTP <status>).` を維持）。
   新規 `test()` は追加せず、auth 3件・全体69件を維持する。
4. `AUDIT-500: /content/{cid}/audit が HTTP 500 のとき監査パネルに role=alert が出ること`:
   監査エンドポイントを500に差し替え、監査パネルの `data-testid="audit-error"` の
   `role="alert"` に `監査ログの取得に失敗しました (HTTP 500).` が表示される。

5. `CONTENT-ERR-404: /content/{cid} が HTTP 404 のとき取得失敗文言を出し改ざん検知と誤表示しないこと`:
   `/content/{cid}` を404で応答させ、本文へセンチネル `SERVER_DETAIL_MUST_NOT_BE_SHOWN` を含める。
   `data-testid="content-load-error"` が `role="alert"` 付きで `Content Load Failed` になり、
   `data-testid="content-load-error-detail"` は固定文言
   `コンテンツの取得に失敗しました (HTTP 404).` になる。バッジ
   `data-testid="content-verification-badge"` は `× LOAD FAILED` で、`COMPROMISED` を含まない。
   `Integrity Breach Detected` とサーバー詳細センチネルは出ない。
6. `CONTENT-ERR-500: /content/{cid} が HTTP 500 のとき取得失敗文言を出し改ざん検知と誤表示しないこと`:
   404 と同一のオラクルを HTTP 500 で繰り返す。詳細文言は
   `コンテンツの取得に失敗しました (HTTP 500).` であり、バッジは `× LOAD FAILED` のまま。
   取得失敗はチェックサム不一致（`× COMPROMISED`）・デコード失敗（`× DECODE FAILED`）と排他である。
   文言・testid は `client/src/lib.rs` の `content_load_error_message` /
   `verification_badge_text` の確定値を grep して使用する（推測しない）。

DASH-ERR の既存 test() 内へ 401 の追加フェーズを統合した。`/content` が401のときだけ
`dashboard-error` に専用文言 `再ログインが必要です。` を出し、`dashboard-relogin-hint` で
既存の `LOG OUT` ボタン（`logout-button`）からの再ログインを案内する。
network error の `Network error loading content.`（`CONTENT_INDEX_NETWORK_ERROR`）と、
500 等の `Failed to load content index.`（`CONTENT_INDEX_LOAD_ERROR`）は変更しない。
401 のレスポンス本文センチネルは画面へ出さない。error-paths の6件・全体69件は不変である。

### E2E 完全性検証シナリオ（INT-01 / R1-13）

`e2e/tests/integrity.spec.ts` の `Forensic Integrity Verification (INT-01)` は、モック認証
（`ALLOW_MOCK_AUTH=1`）でログイン後、`/content` と `/content/{cid}` を差し替えて検証状態の
表示を確認する（6件）。genuine / tampered の既存2件に、R1-13 のデコード失敗、
AUDIT-04 の FORENSIC_TIMELINE 順序、R2 の Audit トグル a11y（ARIA 状態・Escape、
CLOSE ボタンのフォーカス復帰）の4件を追加した。

1. 正規ケース: 本文と一致する SHA-256 を登録し、`span` hasText `● VERIFIED` が出ることを確認する。
2. 改ざんケース: チェックサム不一致の本文を返し、`× COMPROMISED` と
   `Integrity Breach Detected` / `Wasm checksum does not match forensic registry.` が出ることを確認する。
3. デコード失敗ケース（`should not mark VERIFIED when checksum matches but JXL decoding fails (R1-13)`）:
   `type: 'Illustration'` のメタデータに対し、`not a jxl codestream` とその SHA-256 を返す
   モックゲートウェイを用意する。取得は HTTP 200 で1回だけであり、チェックサムは一致するが
   JXL デコードに失敗する。実装側と確定した次文言・testid で失敗表示を確認する。
   - 共通コンテナ: `data-testid="content-verification-error"`
   - 見出し: `role="alert"` かつ `data-testid="jxl-decode-error"` の `JXL Decode Failed`
   - 詳細: `data-testid="jxl-decode-error-detail"` の `JXL のデコードに失敗しました。`
   - バッジ: `data-testid="content-verification-badge"` の `× DECODE FAILED`
4. デコード失敗ケースでは `● VERIFIED`、`Integrity Breach Detected`、
   `Wasm checksum does not match forensic registry.` が出ないことも同時に確認する。
   デコーダのエラー値（内部情報）は画面へ出さず、デコード成功時のみ VERIFIED に戻る。
5. 監査順序（`should render FORENSIC_TIMELINE in log_id DESC order (newest first)`）:
   `/audit` モックが `log_id` 3→2→1 の降順で返すとき、`content-card` 内の FORENSIC_TIMELINE の
   `Reason:` 行が `Reason: newest log 3` / `Reason: middle log 2` / `Reason: oldest log 1`
   の順に並ぶ。取得成功なので `audit-error` は出ない（AUDIT-04）。
6. a11y（`R2: Audit toggle exposes ARIA state and dialog closes on Escape`）:
   `data-testid="content-card"` 内の Audit トグルは初期状態で `aria-expanded="false"` と
   `aria-controls` が CID 由来の `forensic-timeline-<token>` を持つ。クリックで `aria-expanded="true"` になり、
   `role="dialog"` かつアクセシブル名 `Forensic Timeline`（`id="forensic-timeline-<token>"`）の
   ダイアログが表示される。トグルへフォーカスを当てて Escape を押すと `aria-expanded` が
   `false` に戻り、ダイアログは `toHaveCount(0)`、フォーカスはトグルへ復帰する（`toBeFocused()`）。

`/content/{cid}` が HTTP エラー／ネットワークエラーの場合の取得失敗表示
（`content-load-error` / `content-load-error-detail` / `× LOAD FAILED`）は、
error-paths.spec.ts の CONTENT-ERR-404 / CONTENT-ERR-500 で検証するため、ここでは重複して数えない。
この分岐はチェックサム不一致（`× COMPROMISED`）・デコード失敗（`× DECODE FAILED`）と排他である。

正規ケースの既存 test() 内へ、チェックサム一致だが UTF-8 として不正なバイト列
（`0xff, 0xfe, 0xfd`）を返す追加フェーズを統合した。クライアントは
`String::from_utf8_lossy` を `String::from_utf8` へ変更し、失敗時に lossy 置換で成功扱いしない。
失敗時は JXL デコード失敗と同じバッジ契約（`× DECODE FAILED`）を保ちつつ、小説本文専用の
`role="alert"`（`data-testid="novel-decode-error"` / `Novel Decode Failed`）と詳細
（`novel-decode-error-detail` / `小説本文の UTF-8 デコードに失敗しました。`）を表示する。
`● VERIFIED` と `Integrity Breach Detected` は出ない。entry_hash と検証バッジの正規経路は不変で、
integrity の6件・全体69件も変更しない。

### E2E 通報送信失敗シナリオ（REPORT-ERR-01 / 02）

`e2e/tests/moderation.spec.ts` の REPORT-ERR-01（HTTP 500）/ 02（`route.abort()`）は、
`POST /mod/report` の失敗を dedicated な `data-testid="report-failed"`
（`role="alert"`、固定文言 `Report failed.`）で告知することを検証する。
入力検証エラーは既存の `data-testid="report-validation-error"`（`role="alert"`）と区別し、
成功文言 `Report accepted. Content marked Reported.` と進捗 `通報を送信中...` は
既存どおり `role="status"` に描画する。新しい試行と送信開始時に失敗表示をクリアする。
いずれも既存 test() 内へアサーションを追加するだけで、E2E 定義数69件は不変である。

HTTP 失敗・ネットワーク失敗の双方で、失敗アームが `set_status.set(String::new())` を呼び、
送信開始時に出した `通報を送信中...` を `p[role="status"]` から消す（do_vote の完了処理と揃える）。
REPORT-ERR-01 / 02 の既存 test() 内で、`p[role="status"]:not([data-testid])` が0件になり
body に進捗文言が残らないことを検証する。失敗告知は `report-failed` の `role="alert"` に残り、
固定文言 `Report failed.` は変更しない。

### E2E Upload シナリオ（R1-14 / INT-02 / CONTENT-01）

`e2e/tests/upload.spec.ts` の `Content Upload Flow` は、モック認証（`ALLOW_MOCK_AUTH=1`）
でログイン後に UPLOAD 画面へ進み、送信成功と失敗時の表示、および R2 の a11y と価格境界を検証する（6件）。
テスト名と確認点は次のとおり。

1. `should upload JXL content successfully`: JXL ファイルを選択して送信し、
   `SUCCESS: Content registered` が表示されることを確認する。
2. `should reject files over 10 MiB before upload`: 10 MiB 超のファイルを選択した時点で
   `ERROR: File exceeds the 10 MiB limit.` を表示し、送信ボタンは無効のまま、
   `/content/upload` へのリクエストが0回（未送信）であることを確認する。
3. `INT-02: should reject ZIP files`: 許可外 MIME のファイルを送信すると API が400を返し、
   `Failed to upload content (HTTP 400).` が表示されることを確認する。
4. `R1-14: shows an error and re-enables the button when arrayBuffer() rejects`:
   `Blob.prototype.arrayBuffer` を拒否させ、準備フェーズの unwrap パニック
   （`Processing...` のまま固まる状態）を再現する。`data-testid="upload-failed"`
   （`role="alert"`）に固定文言 `アップロードの準備に失敗しました。` が表示されることを
   確認する。既存の `data-testid="upload-status"`（`role="status"` / `aria-live="polite"`）の
   検証は維持し、送信ボタンが再活性化して `Processing...` のまま残らないことを確認する。

5. `R2: file-upload label is keyboard operable and price input has bounds`:
   `label[for="file-upload"]` が表示され、`tabindex="0"` を持つことを確認する。
   ラベルへフォーカスして Enter を押すと `filechooser` イベントが発火する。
   価格入力 `#upload-price` は `min="0"` と `max="1000000"` の属性を持ち、
   0〜1000000 の範囲が入力可能であることを検証する（0 は購入不可を表す）。

6. `R2 (price boundary): 送信前ゲートで範囲外を拒否し、0/1000000 を丸めずに送信する`:
   `#upload-price` へ -1 / 1000001 / 0 / 1000000 の順に入力して送信する。
   -1 と 1000001 はクライアントの送信前ゲート（`sign_and_upload` の
   `price_range_error`）で拒否され、失敗用の `data-testid="upload-failed"`
   （`role="alert"`）に固定文言 `Price must be between 0 and 1000000 JPY.` が表示される。
   `page.route('**/content/upload', ...)` の呼び出し回数は0回で、
   HTTP エラー文言や生エラー本文は表示されない。0（購入不可）と 1000000 は送信され、
   multipart の `price` フィールドは `['0', '1000000']` であることを確認する
   （`price_from_number` は丸めない）。モック応答はサーバー契約 0..=1_000_000 と同じ
   境界を模すが、実サーバーへは送信しないため実 API の検証を証明しない。

失敗時は生のエラー値・内部情報を画面へ出さず、固定文言のみを表示する。クライアント
（`client/src/lib.rs`）は準備・通信・応答読取の各段を `Result` で伝播させ、
成否にかかわらず `set_loading(false)` を実行してボタンを再活性化する。
固定文言は準備・応答読取の2種に加え、通信／HTTP応答で4分岐する（後述）。

| 失敗段階 | 固定文言 |
| --- | --- |
| 準備（`array_buffer` / `FormData` / `Blob` 生成など） | `アップロードの準備に失敗しました。` |
| 通信（`Request::send` の `Err`） | `Network error uploading content.` |
| 応答読取（`response.json::<String>()`） | `アップロード結果の読み取りに失敗しました。` |

`data-testid="upload-status"` は `[STATUS]` の接頭辞に続けて成功・進捗文言を描画し、
`data-testid="upload-failed"` は `[FAILED]` の接頭辞に続けて失敗文言を `role="alert"` で描画する。
文言が空のときは `Show` により非表示になる。loading 中は送信ボタンが `Processing...`
かつ `disabled` で、完了後に `Sign & Submit to Evidence Ledger` へ戻る。
通信／HTTP応答は `upload_error_message` の4分岐で固定文言を選ぶ: 401=再ログイン
（`再ログインが必要です。`）、413=サイズ超過（`ERROR: File exceeds the 10 MiB limit.`）、
HTTP その他=`Failed to upload content (HTTP {status}).`、None（ステータス取得不可）=
`Network error uploading content.`。サーバー本文・生エラーは表示しない。

### 中立化URL件数指標の検証（MET-01）

`POST /metrics/client-events` と `GET /metrics` の `openillustration_neutralized_urls_total` を対象に、
クライアント送信とサーバー側の集計を分離して検証する。

このカウンタは参考値であり、アラート判定や請求等の意思決定に単独で使わない。
`POST /metrics/client-events` は認証不要だが、加算は1リクエストにつき1固定であり、
`count=1000` の連投による増幅は60/分/IPのレート制限へ低減される。
累積値を不正検知や課金の根拠として扱うアサーションは設けない。
検証は差分加算・ペイロードの非機微性・レート制限の適用に限定し、
判断には認証済みデータやサーバー側の独立した記録を突き合わせる。

#### サーバー側差分アサーション

カウンタはプロセス内で他のテストと値を共有するため、絶対値ではなく送信前後の差分（`+N`）で検証する。
テスト名は実装側と連携して確定した次の値を使う。

- `test_metrics_expose_prometheus_counters`: 既存テストを拡張し、新カウンタが HELP/TYPE 行付きで
  Prometheus 形式に現れ、POST 前後の差分が1増えることを確認する。
- `test_client_events_neutralized_url_increments_counter`: 正常系（count 指定・省略）で 204
  （本文なし）を返し、カウンタが1リクエストにつき1だけ増えることを確認する。
- `test_client_events_rejects_invalid_payloads`: 不正 event、count（0 / 1001 / 負 / 小数 /
  文字列）、不正 JSON を 400 で拒否し、カウンタが増えないことを確認する。
- `test_client_events_rejects_oversized_body`: 1 KiB 超のボディを 400 で拒否し、
  カウンタが増えないことを確認する。
- `test_client_events_is_rate_limited`: レート制限の対象であり、超過時に 429 を返すことを確認する。
- URL 本文・IP・User-Agent は送らず、サーバーも受信・記録・ログ出力しない。ペイロードのキーは
  `event` と `count` のみで、成功レスポンスにも本文を返さない。

#### finalizer／purge カウンタの検証（既存ゲート MET-01 を再利用）

`GET /metrics` の finalizer/purge 5種（ticks / processed / lock_busy / failures /
last_success_timestamp）について、公開形式と実際の進捗を下記の実在テストで検証する。
新規ゲート ID は設けず、既存の MET-01 に含める。

- `test_metrics_expose_finalizer_counters`: `GET /metrics` の本文に5種の `# HELP` / `# TYPE` 行が
  あり、4種が `counter`、`openillustration_finalizer_last_success_timestamp_seconds` が `gauge`
  であることと、各数値行が読み取れることをオラクルとする。
- `test_finalizer_metrics_increment`: 期限切れリフレッシュトークンを1件置いて finalizer を起動し、
  `finalizer_ticks` と `finalizer_processed` が 1 以上、`finalizer_last_success_secs` が 0 より
  大きくなることをオラクルとする。
- `test_client_events_neutralized_url_increments_counter`: `count` 指定7・省略時1・上限1000で
  204（本文なし）を返し、加算は1固定でカウンタが before+1 / +2 / +3 と進むことをオラクルとする。
- `test_client_events_rejects_invalid_payloads`: 不正 event（固定値以外）・`count` の 0 / 1001 /
  負 / 小数 / 文字列 / null・未知キー（`url` / `ip` / `user_agent`）・不正 JSON を本文なしの400で
  拒否し、カウンタが増えないことをオラクルとする（`deny_unknown_fields`）。
- `test_client_events_rejects_oversized_body`: 1 KiB（1024 バイト）は受理し、1025 バイトと
  10 MiB 超の `Content-Length` 宣言は本文なしの400で拒否し、カウンタが +1 のままであることを
  オラクルとする。
- `test_client_events_is_rate_limited`: レート制限 quota 2 の状態で3回目の POST が 429 になる。
  `/metrics` と異なり、このエンドポイントはレート制限対象から除外されていない。

#### クライアント側単体テスト

- `client_event_payload_is_minimal_and_clamped`: ペイロードが `event` と `count` のみで、
  0 件は送信せず、上限は 1000 にクランプされることを確認する。
- `render_markdown_counts_neutralized_urls`: `render_markdown()` 1 回で中和された危険 URL の
  件数を数え、1 描画につき 1 回の送信へ集約されることを確認する。
- `markdown_protocol_relative_and_unc_urls_are_neutralized`: プロトコル相対 URL
  （`//host/path`）と Windows UNC（`\\host\path`、スラッシュ混在形式を含む）を
  `is_safe_markdown_url` で拒否し、`href="#"` / `src="#"` へ中立化する。
  通常の相対 URL（`/chapter/1` / `../chapter/1`）は維持し、中立化カウントにも
  含める（`client/src/lib.rs:520-539,2507`）。
- `neutralized_report_is_sent_once_per_render_source`: 描画ソース（markdown 本文）の
  ハッシュをページ全体の `REPORTED` 集合へ記録し、同一ソースの再描画・再マウントでは
  送信せず、別ソースのみ新たに1回送信する（`client/src/lib.rs:358-369,400-425,2615`）。

#### E2E シナリオ（MET-01 / NEUT-URL-02 / NEUT-URL-03）

`e2e/tests/metrics.spec.ts` の
`MET-01: neutralized URL count is posted once as a minimal client event` が、モック認証
（`ALLOW_MOCK_AUTH=1`）で次を確認する（chromium）。metrics.spec.ts は定義3件で、
残る2件は NEUT-URL-02 / NEUT-URL-03 である。
E2E の `test()` 名も NEUT-URL-02 / NEUT-URL-03 へ改名済みで、仕様 ID と一致する。
旧 E2E 名 `MET-02` はサーバー側 `observe_metrics` のゲート ID（`observe_metrics` の最外側配置の検証）と衝突し、
旧 `MET-03` は仕様に存在しなかったため、それぞれ NEUT-URL-02 / NEUT-URL-03 へ統一した。

1. 危険 URL を 2 件含む Markdown（`[click](javascript:alert(1))` と
   `![img](data:text/html,alert)`）を Novel 本文として返し、チェックサム一致後に
   `a[href="#"]` と `img[src="#"]` が描画される。
2. `page.route('**/metrics/client-events', ...)` で捕捉した POST はちょうど1本で、
   本文は `{"event":"neutralized_url","count":2}` と JSON deep-equal になる。
3. 生 JSON に URL 本文（`javascript:alert(1)` / `data:text/html,alert` / タイトル）が含まれず、
   キーは `count` と `event` のみ。`authorization` / `x-forwarded-for` ヘッダも送らない。
4. 204（本文なし）のモック応答で画面に `role="alert"` が出ない。API 側のバリデーションと
   カウンタの差分検証はサーバー側単体テストへ委譲し、本シナリオでは重複して検証しない。
5. NEUT-URL-02（E2E test 名は `NEUT-URL-02: protocol-relative URL は # に中立化され件数が送信される`）:
   本文 `[phish](//evil.example/phish) ![img](//evil.example/img.png)` を Novel として返す。
   `a[href="#"]` と `img[src="#"]` が各1件、`a[href^="//"]` と `img[src^="//"]` が0件であることを確認し、
   `POST /metrics/client-events` は `{"event":"neutralized_url","count":2}` の1本だけとする。
   通常の相対 URL（`/chapter/1` / `../chapter/1`）は中立化せずカウントにも含めない
   （`client/src/lib.rs:520-539,2507`）。対応ゲートは NEUT-URL-02。
6. NEUT-URL-03（E2E test 名は `NEUT-URL-03: 同一本文の再描画では client event を再送しない`）:
   上記 MET-01 と同じ危険 URL 2件の本文で描画し、本文取得1回・POST1回を確認した後、
   BROWSE → UPLOAD → BROWSE で ContentViewer を再マウントする。本文取得は合計2回へ増えるが、
   `REPORTED` 集合（描画ソースのハッシュ）により同一ソースの再描画では送信せず、
   POST は1本のまま `{"event":"neutralized_url","count":2}` であることを確認する
   （`client/src/lib.rs:358-369,400-425`）。仕様 ID と E2E test 名は NEUT-URL-03 に統一済みである。

### observe_metrics の最外側配置の検証（MET-02）

`observe_metrics` は `Router` の最も外側（`security_headers` のさらに外側）に置く。内側の
ミドルウェアで短絡するレスポンスも取りこぼさないことを、`server/src/tests.rs` の4件で
`openillustration_http_requests_total` / `openillustration_http_errors_total` の差分から検証する。
各テストは事前に `GET /metrics` で基準値を読み、対象リクエスト後に再度読む。事後読み自体も
最外側で計上されるため requests は +2（対象 + 事後 GET）となり、errors は対象が 4xx/5xx の
場合だけ +1 になる。ヘルパーは `read_http_metrics(app)`。

| テスト名 | 短絡元と対象レスポンス | 期待差分 |
| --- | --- | --- |
| `test_metrics_count_rate_limited_response` | `rate_limit_test_state(1)` で quota を使い切り、2回目の `/no-such-path` が 429 | requests +2 / errors +1 |
| `test_metrics_count_ipv4_forbidden_response` | `ALLOW_IPV4_LOCALHOST` 未設定、IPv4 ピア `203.0.113.1` の `/health` が 403 | requests +2 / errors +1 |
| `test_metrics_count_oversized_body_rejection` | `Content-Length` を `10 * 1024 * 1024 + 1` と宣言した `/content/upload` が 413 | requests +2 / errors +1 |
| `test_metrics_count_cors_preflight` | `Origin` と `Access-Control-Request-Method: POST` 付き OPTIONS `/auth/login`（CorsLayer の短絡、成功） | requests +2 / errors 増なし |

### ウォレット bind 署名 v2 の検証（PAY-05）

`wallet_bind_message_v2(order_id, chain_id, wallet_address, recipient_address, amount_jpy)` が
5項目を署名対象とし、注文IDのみを署名対象にする旧形式を拒否することを検証する。

- `test_bind_wallet_rejects_legacy_order_only_signature`（新規）: 注文を作成した後、
  `openIllustration payment order {order_id}` という旧形式メッセージへウォレット鍵で署名して
  `POST /payments/orders/{order_id}/bind` に送る。期待は `401 Unauthorized`。
  金額・送金先・チェーンが署名に含まれないため受理されない（盲署名対策の回帰）。
- 既存の bind 回帰（`test_payment_order_lifecycle` /
  `test_payment_order_rejects_wrong_wallet_signature` / `test_bind_wallet_rejects_elapsed_expiry`）
  は v2 署名へ更新した。テストヘルパー `bind_payment_order` は引数へ
  `chain_id` / `recipient_address` / `amount_jpy` を追加し、v2 メッセージを生成する。
- `amount_jpy` は `amount_jpy_from_wei` で DB の `amount_wei` から導出する（10^18 スケールで
  割り切れない値は `Internal` として拒否）。アドレスは `normalize_address` で小文字化する。

### 監査CLIの権限・トークン・上書き拒否の検証（AUDIT-02）

`server/src/bin/sign_audit_export.rs` の硬化項目に対するテスト設計。実装は完了しており、
同ファイル内に鍵権限・トークン・上書き拒否の10テストと include_reasons 本番ガード2件、DB 接続先必須化5件の計17テスト（内訳: 鍵権限・トークン・上書き拒否10件＋include_reasons本番ガード2件＋DB接続先必須化5件）を追加済み。下表はその契約の一覧であり、
Runbookの手動確認手順を継続して併用する。同じトークン契約は `verify_backup` にも適用される
（CLI-03 を参照）。

| 検証項目 | 条件 | 期待結果 |
| --- | --- | --- |
| 鍵ファイル権限 | PEM が `0644` 等 `mode & 0o077 != 0` | 拒否（`require owner-only 0600`） |
| 鍵ファイル権限 | PEM が `0600` / `0400` | 受理 |
| 親ディレクトリ権限 | `mode & 0o022 != 0`（group/other 書き込み可） | 拒否 |
| 親ディレクトリ権限 | `mode & 0o055 != 0`（`0700` 以外、例 `0755`） | 警告を出力して継続 |
| トークン受け渡し | `--database-token TOKEN` を指定 | エラー（`--database-token is not accepted; set DATABASE_TOKEN in the environment instead`） |
| トークン受け渡し | `DATABASE_TOKEN` 環境変数を設定 | 読み込んでリモート libSQL へ渡す（未設定は空） |
| 出力の上書き | `--output` に既存ファイルを指定 | 拒否（`create_new` により原子的に失敗）。原子的公開経路も既存ファイルを拒否し内容を保持する |
| 出力の原子性 | 新規 `--output` を指定 | 同ディレクトリの `0600` 一時ファイルへ書き `fsync` 後に `rename` で公開。失敗時は一時ファイルを削除し、最終パスに中途半端なファイルが残らない |
| 平文理由のエクスポート | 本番相当（`ALLOW_MOCK_AUTH` 無効・`AUDIT_EXPORT_INCLUDE_REASONS` 未設定）で `--include-reasons` を指定 | 拒否（`--include-reasons is not allowed in production without explicit opt-in; keep reasons masked or set AUDIT_EXPORT_INCLUDE_REASONS=1 deliberately`）。フラグ値・環境変数値はエコーしない |
| 平文理由のエクスポート | `ALLOW_MOCK_AUTH=1`（開発）または `AUDIT_EXPORT_INCLUDE_REASONS=1`（明示オプトイン）付きで `--include-reasons` を指定 | 受理（理由をマスクせずエクスポート） |

新規回帰テストは `include_reasons_requires_explicit_opt_in_in_production`
（本番相当での拒否と明示オプトインによる break-glass 受理）と
`include_reasons_is_allowed_with_mock_auth`（mock 認証時の受理）の2件。

`AUDIT_EXPECT_ROOT` の更新ライフサイクルは Runbook の段階ロールアウト／リストア手順で
規定する（[BACKUP_RESTORE_RUNBOOK.md](BACKUP_RESTORE_RUNBOOK.md)）。監査ログ更新のたびに
`verify_audit_log` の計算 root（`audit_chain_digest:` 行の `root=<hex>`）を再取得し、
`backup-rotate.sh` が成果物と同じ stamp の `.audit-root`（0600）として世代保存する
（root 値は stdout / stderr / ALERT へエコーしない）。次回期待値への昇格は運用者の
明示承認を要する手作業で自動上書きしない。リストアでは復元対象世代に紐付いた root を
`--expect-root` に渡す。`verify_audit_log` の bin テスト14件は不変で、新規オラクルは
`scripts/test-backup-rotate.sh` の既存ケース内に追加し呼び出し件数を維持する。

## 5. リリース証跡

リリースごとにテスト結果・ビルド成果物・依存ロックファイルを保存する。Nix再現可能ビルド、オンチェーン記録、バイナリ同一性の証明は別途設計・実装するまでリリース保証として主張しない。

## 6. クライアント単体テスト（一覧表）

対象は client/src/lib.rs の `#[cfg(test)] mod tests` 配下 `#[test] fn`。
本表の名前は `test_` 接頭辞を持たない。scripts/test-doc-numbers.sh は下表の
バッククォート名を抽出し、実コードの `#[test] fn` 名と doc→code の一方向で照合する
（実コード側の全テストを表へ載せることは要求しない）。

| テスト名 | 検証内容 |
| --- | --- |
| `should_refresh_requires_401_and_auth` | 有効化済み。401 かつ認証コンテキスト時のみ更新対象（WWW-Authenticate 付き401/auth=false/403/500/200 は対象外）。失敗時は元の401を返す契約・単一フライトで競合解決済み・E2E69件維持 |
| `allows_retry_stops_after_one_refresh` | 更新後の再試行は1回で打ち止め |
| `refresh_slot_single_flights_and_broadcasts_one_outcome` | 同時更新は単一フライトで1結果を配信 |
| `refresh_slot_waker_is_registered_once_and_cleared_on_completion` | waker は1度だけ登録し完了時に解除 |
| `claim_submission_rejects_clicks_while_busy_and_recovers_after_release` | ビジー中の二重送信を拒否し解除で復帰 |
| `file_picker_activates_only_on_enter_or_space` | ファイル選択は Enter/Space のみ活性化 |
| `favorite_error_message_uses_dedicated_401_text` | お気に入り401は専用固定文言 |
| `favorite_error_message_keeps_existing_http_and_network_text` | HTTP/ネットワーク文言は既存を維持 |
| `upload_error_message_uses_dedicated_401_and_413_text` | アップロード401/413は専用固定文言 |
| `upload_error_message_uses_http_status_and_network_text` | HTTP ステータスとネットワーク文言を使用 |
| `upload_error_message_never_leaks_raw_error_or_server_body` | 生エラー・サーバー本文を一切出さない |
| `price_parse_failure_keeps_previous_valid_value` | 解析失敗時は直前の有効値を保持 |
| `price_range_error_is_fixed_text_without_raw_input` | 範囲外は生入力を含まない固定文言 |
| `markdown_headings_get_ids_for_h1_h2_h3` | h1/h2/h3 へ見出し id を付与 |
| `toc_anchors_match_body_heading_ids` | 目次アンカーが本文 id と一致 |
| `cid_prefix_namespaces_heading_ids_and_toc_anchors` | CID 接頭辞が id とアンカーを名前空間化 |
| `cid_scope_token_is_attribute_safe_and_stable` | CID スコープトークンは属性安全で安定 |
| `forensic_panel_id_is_unique_per_cid` | CID ごとに forensic パネル id が一意 |
| `duplicate_headings_get_sequential_slugs` | 重複見出しへ連番スラッグを付与 |
| `heading_slugs_are_sanitized_against_injection` | 見出しスラッグを注入に対して無害化 |
| `prefixed_render_with_xss_cid_cannot_break_html_attributes` | XSS CID でも属性を破壊できない |
| `unicode_heading_slugs_are_identifiable_and_attribute_safe` | Unicode 見出しスラッグは識別可能で属性安全 |
| `japanese_slug_does_not_collide_with_cid_prefixed_slug` | 日本語スラッグと CID 接頭辞が衝突しない |
| `symbol_only_headings_fall_back_to_unique_section_slugs` | 記号のみ見出しは一意セクションスラッグへ退避 |
| `assign_unique_slug_records_next_suffix_per_base` | 基底ごとの次サフィックスを記録 |
| `ten_thousand_identical_headings_get_unique_sequential_slugs` | 1万件の同一見出しへ一意連番を付与 |
| `upload_media_type_prefers_browser_type_then_extension` | MIME はブラウザ種別→拡張子の優先順 |
| `markdown_raw_html_is_escaped` | 生 HTML をエスケープ |
| `markdown_unsafe_link_and_image_schemes_are_neutralized` | 危険なリンク/画像スキームを中立化 |
| `markdown_protocol_relative_and_unc_urls_are_neutralized` | プロトコル相対/UNC URL を中立化 |
| `api_origin_has_no_trailing_slash` | API オリジンは末尾スラッシュなし |
| `client_event_payload_is_minimal_and_clamped` | ペイロードは最小項目・0送料・上限クランプ |
| `neutralized_report_is_sent_once_per_render_source` | 中和報告は描画ソースごとに1回 |
| `render_markdown_counts_neutralized_urls` | 中和 URL 件数を1描画1回で集計 |
| `markdown_safe_link_schemes_and_relative_urls_are_preserved` | 安全スキームと相対 URL は維持 |
| `markdown_reference_autolink_and_entity_schemes_are_neutralized` | 参照/オートリンク/実体参照の危険スキームを中立化 |
| `content_query_is_built_from_optional_filters` | 任意フィルタから検索クエリを構築 |
| `content_query_roundtrip_preserves_japanese_ampersand_and_favorite_only` | 日本語・&・お気に入りの往復保存 |
| `content_query_parse_ignores_unknown_keys_and_accepts_valueless_flag` | 未知キーを無視し値なしフラグを受理 |
| `canonical_content_query_is_idempotent` | 正規化クエリは冪等 |
| `canonical_content_query_keeps_last_value_for_duplicate_keys` | 重複キーは最後の値を保持 |
| `canonical_content_query_drops_unknown_keys_and_normalizes_order` | 未知キーを破棄し順序を正規化 |
| `canonical_content_query_decodes_plus_and_percent_before_reencoding` | 再符号化前に plus/percent を復号 |
| `cid_is_percent_encoded_for_url_paths` | CID は URL パス用にパーセント符号化 |
| `only_latest_generation_response_is_applied` | 最新世代のレスポンスのみ適用 |
| `search_debounce_delay_is_within_expected_range` | 検索デバウンス遅延は想定範囲内 |
| `dashboard_error_uses_only_fixed_phrases` | ダッシュボードエラーは固定文言のみ |
| `content_meta_html_escapes_user_supplied_fields` | メタ HTML は利用者入力をエスケープ |
| `content_meta_accepts_legacy_json_without_new_fields` | 新フィールド無しの旧 JSON を受理 |
| `content_meta_reads_is_favorite_from_current_json` | 現行 JSON から is_favorite を読む |
| `moderation_stats_json_matches_server_interface` | 集計 JSON がサーバー契約と一致 |
| `moderation_stats_html_escapes_server_supplied_strings` | 集計 HTML がサーバー文字列をエスケープ |
| `public_mod_stats_json_matches_server_interface` | 公開集計 JSON がサーバー契約と一致 |
| `public_mod_stats_html_omits_moderation_actions_and_private_fields` | 公開 HTML に操作・非公開項目を含まない |
| `public_mod_stats_html_escapes_server_supplied_strings` | 公開 HTML がサーバー文字列をエスケープ |
| `vote_feedback_uses_only_fixed_phrases` | 投票フィードバックは固定文言のみ |
| `reports_error_uses_only_fixed_phrases` | 一覧エラーは固定文言のみ |
| `stats_error_uses_only_fixed_phrases` | 集計エラーは固定文言のみ |
| `stats_forbidden_phrase_is_preserved_but_never_rendered_on_general_screen` | 禁止文言は保持するが一般画面へ描画しない |
| `verification_badge_distinguishes_decode_failure_from_checksum_mismatch` | デコード失敗と checksum 不一致を区別 |
| `content_load_error_uses_only_fixed_phrases` | 本文読込エラーは固定文言のみ |
| `wasm_checksum_matches_common_crate` | WASM checksum が common クレートと一致 |
| `wasm_detects_tampered_content` | 改ざん本文を WASM が検出 |
| `export_parse_failure_uses_fixed_phrase_without_success_status` | 解析失敗は固定文言・成功文言なし |
| `data_export_deserializes_all_agreed_fields` | 合意済み全フィールドを復元 |
| `data_export_defaults_missing_fields` | 欠落フィールドは既定値 |
| `download_start_error_message_is_fixed_phrase` | ダウンロード開始失敗は固定文言 |
| `export_filename_is_fixed_constant` | ファイル名は固定定数 |
| `report_validation_rejects_blank_cid_and_reason` | 空 CID/理由を拒否 |
| `report_input_maxlength_matches_server_limits` | maxlength がサーバー上限と一致 |
| `upload_maxlength_constants_match_common_byte_limits` | アップロード上限が common のバイト制限と一致 |
| `metadata_byte_error_enforces_utf8_title_boundary` | タイトルの UTF-8 境界を検査 |
| `metadata_byte_error_enforces_utf8_description_boundary` | 説明の UTF-8 境界を検査 |
| `metadata_byte_error_enforces_utf8_tag_boundary` | タグの UTF-8 境界を検査 |
| `metadata_byte_error_counts_tags_like_server_parser` | タグ数をサーバーパーサと同じに計数 |
| `report_reason_byte_error_enforces_utf8_boundary` | 通報理由の UTF-8 境界を検査 |
| `content_empty_state_message_switches_on_active_filters` | 空状態文言が絞り込みで切り替わる |
| `illustration_aria_label_uses_cid_prefix_when_title_blank` | 無題時に CID 接頭辞を aria ラベルへ使用 |
| `client_metrics_enabled_only_disabled_by_zero` | 0 のみクライアントメトリクスを無効化 |
| `client_metrics_gate_controls_payload` | ゲートがペイロード送信を制御 |
| `mock_auth_enabled_only_disabled_by_zero` | 0 のみモック認証を無効化（未設定は有効） |

## CI 前提条件の検証契約（数値不変）

- バックアップローテーションのケース8（実 gpg の暗号化→復号→バイト一致）は、
  gpg 不在・鍵生成失敗・fingerprint 取得失敗をローカルでは `SKIP case 8: <理由>`
  （PASS と区別する専用出力）として扱う。CI（`CI` 設定時）では同じ条件を
  `FAIL case 8: required precondition missing in CI (<理由>)` として失敗させ、
  黙スキップを禁止する。runner には `apt_packages` で `gpg` を導入し、
  `shellcheck` / `shfmt` は維持する。既存 `expect_failure` / `expect_success` 件数は不変。
- E2E はブラウザ導入を選定より前に実行し、CI では未指定時も
  `chromium,firefox,webkit` を明示指定として扱う。chromium 単一へのフォールバックは
  CI ではエラー終了とし、選択の明示厳守・未知値拒否のルールは不変。
  E2E69件、フルスイートの内訳は不変。
- IPFS ゲートウェイ設定テストは既存の mock 有効ケース（http 受理）の期待値を維持する。
  `http` は `ALLOW_MOCK_AUTH` 有効時のみ受理し、本番相当（無効時）は `https` のみ受理
  （拒否値は非エコー）。check23 / doctor53 / migrations14 / tests136 の数値は不変。

## R2-CLIENT-UX の受け入れ契約（既存ケース内へ統合・数値不変）

- **ファイル選択の accept と送信前ゲート**: `input[type=file]` は
  `accept=".jxl,.md,.markdown,.txt,image/jxl,text/markdown"` を宣言する。
  `upload_media_type` が空（MIME／拡張子が受理対象外）のときは送信前に
  固定文言 `ERROR: Unsupported file type. Select a JXL or Markdown file.` を
  dedicated `role=alert` で出し、`/content/upload` への HTTP 400 遠回りをしない。
  Playwright の `setInputFiles` は `accept` を迂回するため、`upload.spec.ts` の
  INT-02 内で属性と送信前拒否（リクエスト0本）を併せて検証する。
- **英語 fixed 文言の言語宣言**: `Content Load Failed` / `JXL Decode Failed` /
  `Novel Decode Failed` の3つの `role=alert` 見出しと、ログイン画面の
  英語パラグラフ2つへ `lang="en"` を付与する。既存固定文言・data-testid は不変。
  `integrity.spec.ts` の既存 ARIA／デコード失敗ケース内へ `lang` アサーションを追加し、
  新規 `test()` は追加しない（E69件を維持）。
- **公開集計401の分岐**: `/mod/stats/public` が401のときだけ dedicated 固定文言
  `再ログインが必要です。` と `LOG OUT からログアウトし、再度 JPKI でログインしてください。`
  を出す。403 / 5xx / network は従来の `集計の取得に失敗しました。` を維持する。
  `moderation.spec.ts` の R5-09 内へ401フェーズと403回帰フェーズを統合し、
  新規 `test()` は追加しない（moderation14件・E2E69件を維持）。
  check23 / doctor53 / migrations14 / tests136 の数値は不変。
