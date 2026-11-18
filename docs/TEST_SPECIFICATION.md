---
title: "テスト仕様書 (Test Specification Document)"
type: Test Specification
description: 現行回帰テストと必須要件のリリース受け入れ条件を定義する。
tags: [testing, acceptance, quality]
generated:
  by: dsh
  at: "2026-11-18T17:18:25+09:00"
verified:
  by: dsh
  at: "2026-11-18T17:18:25+09:00"
---

## テスト仕様書 (Test Specification Document)

本書は現行プロトタイプの回帰テストと、必須要件のリリース受け入れテストを区別する。
JPKI、JPYC、第三者検証可能な監査は、実装前だからといって最終検証対象から除外しない。

## 1. テスト層

- **Unit**: `common` のチェックサム・年齢計算・監査ハッシュ等。
- **Integration**: Axumルーター、JWT認可、DB状態遷移、制約の検証。
- **E2E**: PlaywrightによるUIフロー確認。APIをモックするテストは実バックエンド統合を証明しない。
  実サーバー結合は `e2e/tests/real-api.spec.ts`（4件: JXL アップロード / 検索 `q` /
  お気に入り追加削除 / `me/export`）でカバーする。実測は全35件（auth 2 / economy 1 /
  error-paths 4 / export 3 / favorites 3 / integrity 3 / metrics 1 / moderation 9 /
  real-api 4 / search 1 / upload 4）。この内訳は `e2e/tests/*.spec.ts` の `test()` 定義を数えた値である。
  favorites の3件目以降は二重クリック防止と401の再ログイン案内、export の3件目は
  ダウンロード開始失敗時の固定エラー表示を検証する。
  moderation の9件のうち3件は投票結果のフィードバック表示（VOTE-FB-01〜03）、
  2件は投票一覧の取得失敗表示（REPORTS-ERR-01〜02）の回帰である。
  integrity の3件目は R1-13（チェックサム一致だが JXL デコード失敗時に VERIFIED にしない）の回帰である。
  upload の4件目は R1-14（`arrayBuffer()` 拒否時に固定エラー文言を表示しボタンを再活性化）の回帰である。
  メトリクス最外側化・bind 署名 v2・監査CLI硬化の反映では E2E 定義は増えていないため、上記35件の内訳は変更しない。
  追加された5件はいずれも `server/src/tests.rs` のサーバー側テスト（MET-02 の4件と PAY-05 の1件）である。
  今回の3CLIへのテスト追加（doctor 24件 / verify_audit_export 17件 / verify_backup 17件）も
  全て `server/src/bin/` 配下のサーバー側 bin テストであり、E2E 定義数は35件のまま変わらない。
  直近の SSRF 2件 / XFF 11件 / doctor 追加4件もサーバー側テストであり、E2E 定義数は35件のまま。
   直近の年齢ゲート共通化（callback / refresh の設定必須403）・TZ 境界・`/me/export` の
   読み取りトランザクションの5件も `server/src/tests.rs` のサーバー側テストであり、
   E2E 定義数は35件のまま変更なし。
   直近の監査理由マスク1件・投票署名 v2 の4件・一覧の `price` NULL 1件は
   `server/src/tests.rs` のサーバー側テスト、エクスポート解析失敗1件は
   `client/src/lib.rs` のクライアント側単体テストである。いずれも E2E 定義を増やさず、
   E2E 定義数は35件のまま変更しない。

## 2. 実装済みテストの概要

| 領域 | 現行カバレッジ | 制約 |
| --- | --- | --- |
| セキュリティヘッダ | 全レスポンスへ `X-Content-Type-Options: nosniff` と `X-Frame-Options: DENY` を付与（通常 200、IP ゲートの 403、レート制限の 429、内部エラーの 500、本文なしの 204、CORS プリフライトの OPTIONS 成功レスポンスを含む）。CorsLayer の短絡を避けるため最外側にも `security_headers` を置く | 単体テストと実サーバーで確認済み。CSP は未設計 |
| 認証 | モックverify（nonce 必須・単回消費・再利用400）、無効トークン系、verify必須フィールド（birth_date/country_code）欠落と不正birth_dateの400拒否、OIDCはstate再利用/未知state、nonce不一致、JWKS未登録鍵での署名、kid欠落、at_hash不一致を401で拒否、OIDC未構成時のstart/callback/back-channel logoutは501、年齢判定（`common` のポリシー/計算ユニットテスト、`server` の検証・`GET /policy/age` 公開応答・明示設定必須/信頼属性必須フェイルクローズ・OIDC callback/refresh の設定必須403・`TZ` ローカル日付の境界、独立参照計算による proptest と境界月日の明示的サンプリング）、JWT秘密鍵長検査のユニットテスト、RS256 の `kid` 付与・一致検証・未知 `kid` 拒否・`kid` 無し旧トークン互換・旧公開鍵によるローテーション検証・重複 kid 拒否、refresh Cookie の Path=/auth（ログアウト時のファミリ失効到達）を検証。JPKI は証明書の base64 デコード・X.509 パース・有効期限のエラーパス5件を Unauthorized の固定メッセージまで検証。ログアウトは `refresh_token` 削除 Cookie（空値・`Path=/auth`・`HttpOnly`・`SameSite=Strict`・`Max-Age=0`・`OIDC_COOKIE_SECURE` 追随の `Secure`、提示トークンはエコーせず本文 `true` を維持）を検証 | 実JPKI/OID4VPの信頼検証、検証済み年齢属性入力、HSM 鍵管理は未テスト・未実装。現行年齢判定は申告値前提 |
| ネットワーク | IPv4拒否・IPv6許可・IPv4-mapped正規化・XFF偽装回帰・fail-closedのミドルウェアテスト、XFF追記型（末尾ピア除去で最左クライアント採用）/上書き型（末尾採用）、複数値・不正エントリのfail-closed、IPv4-compatible IPv6（`::a.b.c.d`）拒否と `::` / `::1` 維持、SSRFリダイレクト無効化（3xxは追従せずフェイルオーバー）、CORSオリジン解析テスト、`MAX_CONCURRENT_REQUESTS` の境界値（既定16、有効範囲 1..=256、範囲外は既定へフォールバック）と同時実行上限 1 の実リクエスト（2件目は429、permit 返却後は200）を検証 | 実プロキシ、多段構成、WAFは未検証 |
| エラー公開 | 500系詳細の既定非公開と `EXPOSE_INTERNAL_ERRORS` での公開のユニットテスト | 4xx系メッセージの公開範囲は未整理 |
| 可観測性 | `RUST_LOG` のフィルタ動作（未設定時INFO、不正値フォールバック）と WARN 出力の起動確認。`LOG_FORMAT=json` で全ログ行が JSON としてパースできることを実サーバーで確認。`/metrics` が Prometheus 形式でリクエスト数・4xx/5xx 数を公開し、404 後にエラーカウンタが進むことを単体テストと実サーバーで確認。新カウンタ `openillustration_neutralized_urls_total` の差分加算とペイロード非機微性を下記 MET-01 で検証。`observe_metrics` を `Router` 最外側へ移し、429 / IPv4 403 / 413 / CORS プリフライト OPTIONS 204 の4ケースを差分で計上する下記 MET-02 の4件を追加 | ログ集約基盤・アラート通知先の選定は未実施 |
| 停止動作 | `/health` 200・`/ready` 200・停止フラグ時の `/ready` 503・IP ゲート適用・`GRACEFUL_DRAIN_TIMEOUT_SECS` の既定/範囲外フォールバックを自動テストで確認。SIGTERM 時の drain ログと終了コード0は実起動確認済み。単一ライター検出（生存 PID 拒否・stale 回収・停止時削除）を単体テストと実プロセス2起動で確認 | 外部依存障害時の readiness 連動・drain タイムアウト発生時の実動作・リモート DB の並列起動制御は未試験 |
| DBマイグレーション | 未適用分の適用・冪等性・チェックサム改変検出・外部キー強制・既存DBの段階的アップグレード（new migration の backfill とデータ保持）・適用済みマイグレーションファイル欠落の検出をユニットテスト。migration 011 で追加したメタデータ列を既存行が空文字として読むことを `test_migration_011_existing_rows_read_as_empty_metadata` で検証 | ロールバック、複数インスタンス同時起動の排他は未実施 |
| 完全性 | SHA-256一致・不一致、IPFS取得系をローカルモックで検証（サイズ超過・タイムアウト・次ゲートウェイへのフォールバック・ストリーム途中切断を含む）、`IPFS_MAX_ATTEMPTS` の既定/範囲検証、CIDのURLパス注入拒否、取得/監査系の不正CID 400・不存在CID 404、チェックサム不一致時の WARN ログ、チェックサム一致でも JXL デコード失敗時は VERIFIED にしないこと（E2E INT-01 / R1-13） | 実ゲートウェイの遅延・切断、実Pinata連携、厳密なCID仕様検証、ネットワークエラー時の実リトライは未検証 |
| Markdown表示 | raw HTMLのエスケープ、インライン・参照リンク・自動リンク・画像URLの危険スキーム拒否（実体参照による難読化、Unicode 空白/制御文字（ZWSP・NBSP）によるスキーム隠しを含む）と安全スキーム維持をユニットテスト。危険 URL が `render_markdown` を通した後も `href="#"` / `src="#"` に置換される統合検証も実施 | ブラウザ別URL解釈のE2Eは未実施 |
| 投稿入力 | 許可MIME、JPEG XLシグネチャ、Markdown UTF-8、重複CIDの再投稿拒否、未知フィールド拒否、全フィールド（`price` / `file` / `owner_hash` / `signature`）の重複 400 拒否、WASM 側 JXL デコードの寸法上限（8000px / 64M ピクセル）とチャンネル数別（1/2/3/4以上）RGBA 変換、クライアント選択時の 10 MiB 超過即時拒否（E2E・送信ボタン無効化と未送信も検証）を検証 | 実 JXL デコードの WASM 実行試験は未実施 |
| モデレーション | 通報状態遷移、重複通報拒否、通報reasonの空/過大拒否とトリム保存、期限前は確定しないこと、期限後確定（同数→`Rejected`、定足数未達→`Approved` 復帰）、期限切れ投票拒否、投稿者投票拒否、BAN済み投票者の拒否、30日以内の異議と30日超の通常通報、finalize エンドポイントの冪等性、重複投票拒否、確定後投票拒否、投票ペイロード検証（report_id/result）とトークンsub不一致401の拒否、投票署名メッセージが report_id と result の両方に束縛されること、運用者未設定時の BAN/UNBAN/UNPIN 拒否、運用者による BAN（根拠・期間）/UNBAN/UNPIN の監査記録、自己BAN・不正期間・空理由・BAN済み運用者の拒否、期限前スキップ・期限後自動確定・複数件処理・冪等性・ワーカー周期の範囲検証 | 同一接続への並行 `BEGIN` が失敗することの再現、書き込みロックによる直列化、並行投票と自動確定の同時実行を検証。ワーカー停止/再起動時の挙動、実運用手続、第三者検証可能な監査（DEC-04）の試験が不足。reason上限値の妥当性はADR-0004の運用で見直し |
| Upload | Content-Typeのユニット/APIテスト、負価格・非整数価格・64bit範囲外価格・上限超過価格の拒否（拒否理由を検証）、価格0と上限値 `MAX_ORDER_JPY` の受理と保存を検証、必須フィールド欠落・owner不一致・BANユーザーの拒否、実 RSA 鍵で署名したバイト列の受理と別バイト列への署名使い回し401拒否 | 実multipartサイズ境界・実Pinata連携の試験が不足 |
| ContentMeta / list_contents | `ContentMeta` が `title` / `description` / `tags` を返すこと、`list_contents` が省略可の `q` / `tag` / `type` / `limit` / `offset` を受理し既定値（limit=50、offset=0）と範囲（`1..=100`）へフォールバックすることを API テストで検証。`q` の部分一致、`tag` の前方一致、`type` の完全一致、`created_at DESC` の並びとページングの重複なしも確認 | 長さ上限・LIKE 注入拒否・XSS エスケープは CONTENT-06〜08 で個別に検証。タグの正規化・全文検索は未実装 |
| お気に入り | 追加/削除の冪等（重複追加・未登録削除）、未認証401・不正CID400・非Approved404、他人subからの操作拒否、`favorite_only` の本人分のみ返却（真理値パース・既存条件とのAND・ページング・Approved限定）、旧JSONの `is_favorite` 欠落互換、migration 012 の UNIQUE + FK CASCADE を API・ユニットテストで検証 | `user_hash` をログ・監査・レスポンスへ出さないことは設計方針で担保。実運用ログの確認は未実施 |
| 決済 | 注文ライフサイクル、未対応チェーン拒否、ウォレット署名不一致拒否、派生アドレスの一意性をモックverifierで検証、非UUID注文IDの400拒否、verifyの金額不一致（MISMATCH遷移）・送金再利用・期限切れ・確認数不足（CONFIRMING維持）の拒否、同一送金の確認数進展に伴う再verifyとFINALIZED到達、FINALIZED後の再verifyが状態を戻さないこと、`expires_at` 経過（status が PENDING のまま）の拒否と EXPIRED 更新、BIP44 derivation index の u32 境界拒否、EVM アドレスの大小文字非区別（正規化照合）を検証。`wallet_bind_message_v2` が注文ID・chain_id・wallet_address・recipient_address・amount_jpy を署名対象とし、注文IDのみの旧形式署名を401で拒否する下記 PAY-05 を追加 | 実チェーン検証、2-of-3クォーラム、実チェーン上の再編成/二重通知の試験は未実施（DEC-03） |
| Support | 501無効化のAPIテスト、送信不可UIと模擬記録注記のE2Eテスト | JPYC決済は未実装、実送金の証明はない |
| 透明性ダッシュボード | `build_mod_stats` の空/同数/定足数未達/月境界、認可（未認証401・未設定/未登録/BAN済み403）、個人情報非含有、実DB集計を `server/src/tests.rs` で検証。E2E は `e2e/tests/moderation.spec.ts` で表示と403を確認（R5-09） | 認証内集計であり第三者公開ではない（DEC-04 未決定）。`quorum_not_met` は `totals` のみで月次表には含めない |
| 通報理由の要約 | `common::summarize_reason` の単体テスト（空・119/120/121 Unicode文字境界・日本語・`@` を含むトークン／`://` を含むトークン／`www.` で始まるトークン／IPv4 らしいドット区切り／Unicode 数字4連続（全角含む）の `[redacted]` 置換、4桁ちょうど境界）と `GET /mod/reports` の統合テスト（要約値のみ返却、全文・PII 非露出、認証必須・Reported 限定・新着順・運用者非限定の維持） | DB 全文と監査エクスポートの本文は変更しない（EXPORT-04 のマスクは従来どおり）。第三者公開範囲（DEC-04）は未決定 |
| データエクスポート | `GET /me/export` の全12表網羅・他ユーザー非漏えい・空ユーザー・機微項目除外・`reason`/`details` マスク・未認証401・ヘッダを `server/src/tests.rs` の `me_export_*` で検証（R5-04）。全12表が単一の読み取りトランザクション内で読み切られること（`debug_assert!`）と、別接続の未コミット BAN 書き込みに対して書き込み前の単一時点スナップショットを返すことを2件で検証。E2E は `e2e/tests/export.spec.ts` でダウンロードと401表示を確認（chromium / firefox で2/2 pass） | 削除権（erase）は未実装。法務・外部アンカー・バックアップ消去方針の決定後に着手（詳細設計「9. データ主体の権利」） |
| 監査CLI | `sign_audit_export` の鍵権限検査（0600/0400・親ディレクトリ0700）・`DATABASE_TOKEN` 環境変数限定（`--database-token` 拒否）・出力 `create_new` による上書き拒否を下記 AUDIT-02 で仕様化。検証系CLIは下記 CLI-01〜03 を参照 | 実装は完了。`sign_audit_export` は bin 内に9テスト、`doctor` 24件 / `verify_audit_export` 17件 / `verify_backup` 17件の bin テストを追加済み。匿名化・外部アンカーは未実装 |
| 性質テスト（proptest） | `server` の `property_tests` 4件（金額換算・CID 形式・理由要約の境界と冪等・Unicode 文字境界）、各128ケース。`proptest` は `server/Cargo.toml` の dev-dependencies（1.12）。年齢判定の proptest は認証行を参照 | 実JPKI実機・実チェーンとの突き合わせは未実施 |
| 直近の未コミット変更の反映（2026-11-18） | 監査タイムラインの `reason` 要約と `entry_hash` 生値維持1件、投票署名 v2 の4件（全フィールド束縛・正常系受理・旧 v1 署名拒否・別投票者向け署名の転用拒否）、一覧の `price` NULL を0として返す1件、エクスポート解析失敗時の固定文言表示1件 | サーバー側6件は `server/src/tests.rs`、クライアント側1件は `client/src/lib.rs`。E2E 定義数は35件のまま。対応ゲートは AUDIT-03 / MOD-12 / CONTENT-10 / EXPORT-11 |

## 3. 必須要件のリリースゲート

| ID | ケース | 期待結果 | 状態 |
| --- | --- | --- | --- |
| AUTH-01 | `ALLOW_MOCK_AUTH` 未設定、`JWT_REQUIRE_KID` 有効 | モック認証とゼロ署名、`kid` 無しトークンを拒否 | 実装済み。`mock_token` とゼロ署名を拒否する回帰テストあり。`JWT_REQUIRE_KID=1` で kid 無し拒否・kid 付き受理、HS256 併用は起動拒否を実プロセスで確認 |
| AUTH-02 | 不正・期限切れJWT、`exp` 欠落トークン | 認証済みAPIが401を返す | 期限切れ（`exp` が過去）と `exp` 欠落トークンを手組みで署名し、保護 API が 401 を返す回帰テストを追加 |
| NET-01 | IPv4アクセスと開発ループバック例外 | 設定に応じて拒否/許可 | 一部実装済み |
| NET-02 | 信頼されないX-Forwarded-For | クライアント入力で接続元判定を迂回できない | 実装済み（IPv4-mapped 正規化、`TRUSTED_PROXY_CIDRS` 未設定時はピアアドレスのみで判定、信頼済みで解決不能時は403。回帰テストあり）。実プロキシ環境・多段構成の試験は要 |
| CONTENT-01 | 許可外形式、負価格、非整数価格、64bit範囲外価格、10 MiB 超本文 | 400で拒否 | 実装済み。有効な本文に不正値を1つだけ含めて拒否理由まで検証し、正常な価格境界（未指定=0、`0`、`MAX_ORDER_JPY`）の受理とDB保存も検証。拒否時に保存されないことを確認。上限超過（1,000,001 と `i64::MAX`）と 10 MiB 超 multipart 本文（Content-Length 検査ミドルウェアが 413 を返す）の拒否も検証 |
| 決済 CID/価格 | 注文作成時に CID 形式・コンテンツ存在・`Approved` 状態・提示価格と `amount_jpy` の一致を検証 | 不一致・不存在・レビュー中・不正 CID を拒否し、注文取得時に CID が保存される | 実装済み。実チェーン上で注文金額と実際の送金額を照合する検証は未実装（実RPC待ち） |
| CONTENT-02 | IPFS応答が10 MiB超過、遅延、切断 | 上限/タイムアウトを適用し、完全データとして返さない | 実装済み。10 MiB 超過とタイムアウト、次ゲートウェイへのフォールバック、ストリーム途中の切断（Partial ボディを返さない）をローカルモックで検証 |
| CONTENT-03 | SHA-256不一致 | コンテンツ描画を拒否 | 実装済み。checksum 一致で 200、不一致で 401、Illustration/Novel の Content-Type をローカルモックで検証 |
| CONTENT-05 | 通報中/却下コンテンツの直接 CID 取得 | 404 で配信しない | 実装済み。`status = 'Approved'` で絞り、Reported/Rejected が 404 になることを mutation テストで確認 |
| CONTENT-04 | 小説に `<script>` 等のraw HTML、`javascript:` / `data:` リンク・画像URL | raw HTMLを実行せず、危険なURLスキームを無効化 | 実装済み、回帰テストあり |
| CONTENT-06 | `title` / `description` / `tags` の上限超過（UTF-8 バイト数）とメタデータ重複フィールド | 400で拒否し、保存・一覧表示に反映しない | 実装済み。`title` 200 B、`description` 2000 B、`tags` 10個・各50 B の境界値と超過を検証（`test_upload_rejects_oversized_metadata` / `test_content_metadata_validation_bounds`）。省略・空文字は受理（`test_upload_accepts_optional_metadata`）。`tags` はカンマ区切り1フィールドで、空文字は空ベクトルとして保存 |
| CONTENT-07 | `GET /content` の `q` / `tag` / `type` / `limit` / `offset` | 条件に一致する `Approved` のみを `created_at DESC` で返す | 実装済み。`q` は title/description の部分一致（OR）、`tag` はカンマ区切りタグの前方一致、`type` は完全一致（不正値は0件で400にしない）。範囲外・非数値の `limit` は50、負・非数値の `offset` は0へフォールバック（`test_list_contents_filters_by_query_tag_and_type` / `test_list_contents_limit_boundaries_and_offset` / `test_content_query_parser_defaults_and_bounds`） |
| CONTENT-08 | `q` / `tag` に `%` `_` `\` を含む入力、メタデータに HTML タグを含む値 | ワイルドカードとして解釈されず、描画時もスクリプトが実行されない | 実装済み。サーバーは `ESCAPE '\'` とバインドパラメータでリテラル照合（`test_like_escaping_keeps_wildcards_literal`、E2E は `test_like_wildcards_are_literal`）、クライアントは `escape_html_text` で `&` `<` `>` `"` `'` を実体参照化。サーバーは生テキストを保存・返却し、消毒は描画側の責務 |
| CONTENT-09 | チェックサム一致だが JXL として不正なバイト列（例: `not a jxl codestream`）を HTTP 200 で返すゲートウェイ | チェックサム検証は成功しても VERIFIED にせず、`× DECODE FAILED` と固定エラー文言を表示する | 実装済み。クライアントの `decode_error` 分岐で `set_verified = Some(false)` に切り替え、デコーダのエラー値は画面へ出さない。E2E は `e2e/tests/integrity.spec.ts` の `should not mark VERIFIED when checksum matches but JXL decoding fails (R1-13)` で検証。補助する単体テストは `verification_badge_distinguishes_decode_failure_from_checksum_mismatch` |
| INT-01 | フォレンジック完全性検証 E2E（正規／改ざん／チェックサム一致＋デコード失敗） | 正規は `● VERIFIED`、改ざんは `× COMPROMISED`、デコード失敗は `× DECODE FAILED` を表示し、失敗時に VERIFIED を出さない | 実装済み。`integrity.spec.ts` の3件（R1-13 対応でデコード失敗ケースを追加）で検証。既存の genuine / tampered のセレクタ・文言は変更なし |
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
| MOD-05 | `/mod/stats` の認可 | 未認証は401。`MODERATION_OPERATOR_HASHES` 未設定・未登録・BAN済みの主体は403。登録済み運用者は200 | 実装済み・検証済み（対象テスト全12件 pass）: `test_mod_stats_requires_authentication` / `test_mod_stats_forbidden_when_operator_not_configured` / `test_mod_stats_forbidden_when_operator_not_registered` / `test_mod_stats_forbidden_when_operator_banned` |
| MOD-06 | `/mod/stats` のレスポンス | CID・reason・`user_hash` を含めず、`generated_at` / `totals` / `monthly` のホワイトリスト項目のみ返す。`quorum_not_met` は `totals` のみ | 実装済み・検証済み（対象テスト全12件 pass）: `test_mod_stats_response_contains_no_sensitive_fields` |
| MOD-07 | 透明性ダッシュボードのE2E（R5-09） | 運用者では集計が表示され、403では `運用者権限が必要です` が表示される | 実装済み・検証済み: `e2e/tests/moderation.spec.ts` の `Moderation & Governance (GOV-01, GOV-02)` 内、`R5-09: should display moderation stats for operators and permission error on 403`。chromium で pass（投票フィードバック3件と投票一覧エラー2件を追加した現在は moderation.spec.ts 全9件 pass、フルスイートの定義数は32件。webkit はホスト側 ICU/libxml2 欠落で未検証） |
| MOD-08 | `common::summarize_reason` の境界・日本語・PII除去 | 空は空のまま。119/120 Unicode 文字は変更なし、121 文字は先頭119文字＋`…`。日本語も文字数で計数し同じ境界になる。`@` を含むトークン（メール）、`://` を含むトークン（括弧付き URL を含む）、`www.` で始まるトークン、IPv4 らしいドット区切り、Unicode 数字4連続（全角含む）は `[redacted]` に置換し、3桁の数字列は残る | 実装済み（`summarize_reason_redacts_pii` の4桁ちょうど境界と `summarize_reason_redacts_additional_pii_patterns`） |
| MOD-09 | `GET /mod/reports` への要約適用と全文非漏えい | `reason` は要約後の値のみ。全文・メール・URL・4桁以上の数字列がレスポンスへ現れない。DB 全文と監査エクスポートは不変。認証必須・Reported 限定・新着順・運用者非限定を維持 | 実装済み（`server` に統合テストを追加。第三者公開は DEC-04 未決定） |
| MOD-10 | 投票結果のフィードバック表示（VOTE-FB-01〜03） | `/mod/vote` の失敗時に `data-testid="vote-error"`（`role="alert"`）へ固定文言 `投票に失敗しました (HTTP <status>).` または `Network error submitting vote.` を表示する。成功時は `data-testid="vote-notice"` に `投票を記録しました。`。新しい試行で両方クリアし、サーバー本文・生エラーは表示しない | 実装済み・検証済み: `e2e/tests/moderation.spec.ts` の VOTE-FB-01〜03（403 / network error / 成功とクリア。chromium で3件 pass、moderation.spec.ts は既存6件と合わせ9件 pass）。クライアント側単体テスト `vote_feedback_uses_only_fixed_phrases` も追加済み |
| MOD-11 | 投票一覧取得失敗の表示（REPORTS-ERR-01〜02） | `GET /mod/reports` の失敗時に `data-testid="reports-error"`（`role="alert"`）へ固定文言を表示する。HTTP エラーは `投票一覧の取得に失敗しました (HTTP <status>).`、ネットワークエラーは `Network error loading reports.`、JSON 解析失敗は `投票一覧の解析に失敗しました。`。失敗しても既存データは維持し、`NO_PENDING_VOTES` を誤表示しない。成功時にエラーをクリアし、サーバー本文・生エラーは表示しない | 実装済み・検証済み: `e2e/tests/moderation.spec.ts` の REPORTS-ERR-01（500）/ 02（route.abort）が chromium で2件 pass、既存7件と合わせ moderation.spec.ts 全9件 pass。クライアント側単体テスト `reports_error_uses_only_fixed_phrases` も追加済み |
| EXPORT-01 | `GET /me/export` の全表網羅と他ユーザー非漏えい | `version:1` / `generated_at` / `sub` と12表の配列を返す。他ユーザーの行・IDは混入しない | テスト名確定: `me_export_returns_only_own_data`。12表は `users` / `contents` / `favorites` / `transactions` / `signatures` / `content_audit_log` / `reports` / `votes` / `payment_orders` / `governance_audit_log` / `refresh_tokens` / `revoked_subs`（`revoked_sessions` は除外） |
| EXPORT-02 | データを持たないユーザーのエクスポート | 12表の配列はすべて空で、メタ3項目（`version` / `generated_at` / `sub`）は通常どおり返る | テスト名確定: `me_export_empty_user_has_empty_arrays` |
| EXPORT-03 | 機微項目の除外 | `token_hash` / `sid` / `family_id` / 署名鍵 / XPRV / `PINATA_JWT` / 他ユーザーID がJSONに現れない | テスト名確定: `me_export_excludes_sensitive_fields` |
| EXPORT-04 | `reason` / `details` のマスク | `mask_reason` により既定でマスクされ、元の本文が現れない（例: `[redacted:N bytes]`） | テスト名確定: `me_export_masks_reason_and_details` |
| EXPORT-05 | 未認証でのエクスポート要求 | 401で拒否し、データを返さない | テスト名確定: `me_export_requires_auth` |
| EXPORT-06 | エクスポートのレスポンスヘッダとログ非出力 | `Content-Disposition: attachment; filename="openillustration-export.json"` と `Cache-Control: no-store` を付与。本文はログへ出さない | テスト名確定: `me_export_headers`。行数上限・`truncated` フラグは設けず全行を返す |
| EXPORT-07 | エクスポートのE2E（R5-04） | 認証トークン付きで `GET /me/export` を呼ぶとJSONダウンロードがトリガされる。401では `role="alert"` に「ログインが必要です」が表示される | 実装済み・検証済み（chromium / firefox で2/2 pass。webkit はホスト側 libicu/libxml2 不在で起動不可、環境依存）: `e2e/tests/export.spec.ts` の `R5-04: Export My Data` 内、`R5-04: GET /me/export を認証トークン付きで呼び、JSON ダウンロードがトリガされること` / `R5-04: /me/export が 401 の場合は「ログインが必要です」を role=alert で表示すること` |
| EXPORT-08 | 削除権（erase）の不在 | 削除要求を受け付けるAPIが存在しない | 未実装（設計で明記）。エクスポートのみ実装。法務・外部アンカー・バックアップ消去方針の決定後に着手 |
| EXPORT-09 | `/me/export` の読み取りトランザクション | 12表の全クエリが単一の読み取りトランザクション（`BEGIN DEFERRED`）で実行され、`collect_export_snapshot` の `debug_assert!(!db.is_autocommit())` が発火しない。完了時に `tx.commit()`、エラー時は Drop が ROLLBACK | 実装済み。`me_export_queries_run_inside_read_transaction`（`db.transaction()` / `tx.commit()` を外す mutation で失敗）。下記「エクスポートの読み取りトランザクション検証」を参照 |
| EXPORT-10 | 並行 BAN 書き込み下の単一時点スナップショット | 別接続が `BEGIN IMMEDIATE` で BAN 更新と監査ログ挿入を未コミットのまま保持しても、エクスポートは書き込み前の `users` / `governance_audit_log` を一貫して返す | 実装済み。`me_export_snapshot_consistent_under_concurrent_ban_write`（一時 SQLite ファイル・2接続）。E2E 定義数は35件のまま |
| SUPPORT-01 | 無効化済み支援APIへのPOST | 501で拒否（未認証は401）、模擬取引を記録しない | 実装済み、JPYC機能への置換要 |
| ID-01 | JPKI提示、署名、nonce、信頼元、失効、`acr`/`amr` 保証レベル、`auth_time`/`max_age` の検証 | 有効な本人確認だけを受理 | OIDC認証フローをモックOPで検証。`OIDC_REQUIRED_ACR`/`AMR` 設定時に満たさないトークンを401で拒否、`OIDC_MAX_AUTH_AGE_SECS` 設定時に認証要求へ `max_age` を付与し新鮮な `auth_time` を受理・古い値と欠落を401拒否（fail-closed）することを回帰テスト化。R1-19 の nbf 検証（`validate_nbf`）は `test_oidc_rejects_future_nbf`（未来 nbf は401拒否）/`test_oidc_accepts_missing_nbf`（クレーム欠落は受理）/`test_oidc_accepts_nbf_within_clock_skew`（60秒の clock skew 内は受理）の3件で検証。`server/src/jpki.rs` の SEC-02 証明書エラーパス5件（`invalid_base64_token_is_unauthorized` / `non_x509_bytes_are_rejected` / `expired_certificate_is_rejected` / `envelope_without_credentials_falls_back_to_raw_token` / `empty_token_is_rejected`）を追加し、いずれも `AppError::Unauthorized` と固定メッセージ（Invalid JPKI presentation token / X.509 parse failed / Certificate is expired or not yet valid）を検証。下記「JPKI 証明書エラーパスの検証（ID-01）」を参照。実JPKI実機・失効確認は未検証 |
| ID-02 | JPKI提示拒否・失効・期限切れ・再利用 | 保護機能を拒否 | state再利用とnonce不一致を検証。back-channel logout は `events` 欠落と `sid`/`sub` 両方欠落を401拒否。R1-23 の pending 登録上限（`MAX_PENDING_AUTH`）は `test_oidc_pending_limit_rejects_excess_registration`（上限超過は429で拒否し保存しない）/`test_oidc_start_returns_429_when_pending_full`（上限到達時に `/auth/oidc/start` が429）/`test_oidc_pending_limit_purges_expired_entries`（期限切れエントリを掃除して空きを作る）/`test_oidc_pending_limit_keeps_start_callback_flow`（上限近くでも start/callback フローが成立）の4件で検証。実JPKIの失効・期限切れは未検証 |
| ID-03 | 認証トランザクションとJWT主体の結合 | 別セッション・クライアント申告IDで主体を偽装できない | PPIDをセッションJWTに結合し、`/auth/logout` の `sid` 失効、back-channel logout の `sid`/`sub` 失効（`sub` はインメモリ `iat` 閾値方式、再認証で新セッション発行可）を検証。R1-18 のログアウト削除 Cookie は `test_logout_clears_refresh_cookie`（空値・`Path=/auth`・`HttpOnly`・`SameSite=Strict`・`Max-Age=0`、提示トークン非エコー、本文 `true` 維持）と `test_logout_deletion_cookie_follows_cookie_secure_setting`（`OIDC_COOKIE_SECURE` が `0` / `false` では Secure なし、`1` / 未設定では Secure 付き）で検証。下記「ログアウト削除 Cookie の検証（ID-03）」を参照。永続化・実OP連携は未検証 |
| ID-04 | 未認証での閲覧APIアクセス | コンテンツ・監査等を拒否。認証開始・health・非秘密の公開ポリシー情報（`GET /policy/age`）は許可 | JWT拒否実装。OIDC実サービスは未接続 |
| AGE-01 | 年齢ポリシー設定と公開 | 不正な `AGE_*` 設定は起動失敗（fail closed）。`GET /policy/age` は認証不要で有効設定値（非秘密）と `settings_always_public=true` を返す | 実装済み（`common`/`server` テスト）。`GET /policy/age` に秘密情報が含まれないことは設計/レビューで継続確認 |
| AGE-02 | 明示設定必須・信頼属性必須モード | `AGE_REQUIRE_EXPLICIT_CONFIGURATION` 未充足時および `trusted_attribute_required` は拒否（フェイルクローズ）。設定チェックは `/auth/verify` と OIDC callback / refresh の共通ゲートで行う | 実装済み（現行入力は申告値）。検証済み属性入力経路は未実装。`AGE_POLICY_CONFIGURED` 未設定時の全経路拒否は AGE-03 で検証 |
| AGE-03 | OIDC callback / refresh の設定必須ゲート（R1-17） | `AGE_REQUIRE_EXPLICIT_CONFIGURATION=1` かつ `AGE_POLICY_CONFIGURED` 未設定/`0` のとき、callback と refresh は state / Cookie の消費前に 403（`ConfigurationRequired`）。`trusted_attribute_required` も 403（`TrustedAttributeRequired`） | 実装済み。`test_require_explicit_configuration_blocks_oidc_callback_and_refresh` で両経路の 403 を検証。下記「年齢ゲート共通化と TZ 境界の検証」を参照 |
| AGE-04 | 年齢基準日の TZ ローカル化 | `TZ` の `Asia/Tokyo`（UTC+9）/ `Etc/GMT±N`（POSIX 符号反転）/ 未設定・未対応名（UTC フォールバック）で「今日」が変わり、年齢境界が1日ずれる。UTC 2024-01-01T16:00Z の JST は翌日 | 実装済み。`test_today_for_policy_uses_configured_local_timezone` と `test_validate_age_at_uses_local_today_for_age_boundary` の2件。下記「年齢ゲート共通化と TZ 境界の検証」を参照 |
| PAY-01 | 対応JPYCの有効な送金と最終性確認 | 確認後のみ決済成立 | 基盤実装済み。モックverifierで注文作成・ウォレット結合・検証・確定、BIP44受取アドレス派生を検証。金額・確認数・署名復元・期限の境界5件は下記「決済ユーティリティの境界の検証（PAY-01〜03）」を参照。R1-26 の並行注文は `test_parallel_orders_derive_sequential_indices_and_unique_addresses`（連番の `derivation_index` と一意な受取先）/`test_identical_parallel_orders_all_succeed`（同一内容の並行注文5件が全件成功）/`test_fixed_receiving_address_keeps_derivation_index_null`（固定受取アドレスは index NULL を維持）/`test_rejected_orders_do_not_consume_derivation_index`（拒否注文は index を消費しない）の4件で検証。実チェーンRPCは未実装 |
| PAY-02 | 非対応トークン、チェーン、再編成、二重通知 | 不成立または確認中として扱う | 一部実装。未対応チェーン、金額・送金元・受取先の不一致、同一tx再利用を検証。再編成・二重通知・別コントラクトは未実装 |
| PAY-03 | 取引要求ID・金額・受取先との不一致 | 有償機能を解放しない | 基盤実装済み。注文ID・金額・受取先・送金元の照合を検証 |
| AUDIT-01 | 監査記録の改変・欠落・順序入替 | ハッシュ連鎖・連番・Merkle root で改変を検出 | ローカルDBのハッシュ連鎖（v2 正規化＋v1 互換検証）・`log_id` 連続性・Merkle root と `--expect-root` による切詰め検出を実装。エントリハッシュが全内容フィールドに依存することを mutation テストで確認（`log_id` は連続性/Merkle root 側でカバー）。署名付きスナップショット（version 3）がコンテンツ監査と BAN/UNBAN/UNPIN 監査の両方を含み、改ざんで検証失敗することを単体テストと実DBで確認。暗号エラーパス12件（署名・DER検証8件と `verify_chain_*` 4件）は下記「暗号検証エラーパスの検証（AUDIT-01）」を参照。CLI 契約（鍵権限・トークン・上書き拒否）は下記 AUDIT-02 を参照。外部アンカー・HSM 鍵管理は未実装、必須 |
| AUDIT-02 | `sign_audit_export` の鍵権限・トークン受け渡し・出力上書き | 鍵 PEM は owner-only（0600/0400）を要求し、親ディレクトリの group/other 書き込みを拒否（0700 以外は警告）。リモート DB トークンは `DATABASE_TOKEN` 環境変数からのみ読み、`--database-token` はエラー。出力は `create_new` で既存ファイルを上書きしない | 実装済み（下記「監査CLIの権限・トークン・上書き拒否の検証（AUDIT-02）」にテスト設計を記載）。`server/src/bin/sign_audit_export.rs` に9テストを追加済み。Runbookの手動確認手順も併用する |
| PERF-01 | 初期表示、API、転送量、Wasmサイズ、手数料 | 合意済みSLO内 | 指標・閾値未決定 |
| COMPAT-01 | 対応OS/ブラウザ/端末で閲覧・投稿・認証・決済 | 対応マトリクスを満たし、安全なフォールバックを表示 | 環境・閾値未決定 |
| COMPAT-02 | JXL/Wasm非対応・機能利用不可環境 | 代替表示または案内。認証・決済は迂回不可 | 方式・環境未決定 |
| CFG-01 | `doctor` の設定不整合警告（R5-06 / R4-01） | `ALLOW_MOCK_AUTH` と PINATA_JWT / `OIDC_*` / RSA 秘密鍵の併用で警告。既定は警告のみで exit 0、`--strict` では exit 1。5パターンの実出力を確認 | 実装済み。純粋関数化と24テストを追加し、13カテゴリ固定を検証。`ALLOW_IPV4_LOCALHOST` / `TRUSTED_PROXY_CIDRS` も警告対象。下記「設定診断（`doctor`）の検証」および CLI-01 を参照 |
| NET-03 | レート制限キーの解決済みクライアント IP 優先 | 信頼プロキシ配下では XFF 解決 IP ごとに独立バケット。未設定・信頼外ではピア IP をキーにする | 実装済み。`test_rate_limit_key_prefers_resolved_client_ip` / `test_rate_limit_uses_resolved_client_ip_under_trusted_proxy` / `test_rate_limit_key_falls_back_to_peer_without_trusted_proxy` |
| NET-04 | `MAX_CONCURRENT_REQUESTS` の境界と同時実行上限（R4-02） | 未設定は既定16、有効範囲は 1..=256。`0` / `-1` / 空 / 非数値 / `257` / 極端な過大値は上限へクランプせず既定16へフォールバック。同時実行上限1では1件目保持中に2件目が429で、permit 返却後の再リクエストは200 | 実装済み。`test_max_concurrent_requests_defaults_and_bounds`（境界）と `test_concurrent_request_limit_rejects_second_and_releases_permit`（`Notify` で1件目を保持し permit 0 → 2件目429 → 解放 → permit 1 → 3件目200、レート制限は100/min で切り分け）。下記「同時リクエスト上限の検証（NET-04）」を参照 |
| NET-05 | SSRF: IPFSゲートウェイのリダイレクト追従拒否 | 302 + `Location` で内部サービスへ誘導されても追従せず、リダイレクト先へ到達しない。3xx はエラーとして次のゲートウェイへフェイルオーバーする | 実装済み。`build_http_client` が `Policy::none()` を設定。サーバー側テスト2件（`test_get_content_does_not_follow_gateway_redirect` / `test_get_content_redirect_falls_back_to_next_gateway`）。下記「SSRF リダイレクト無効化の検証（NET-05）」を参照 |
| NET-06 | XFF複数値解釈と IPv4-compatible 形式の拒否 | 追記型は末尾ピアを除去して最左クライアントを採用、上書き型は末尾を採用、不正エントリは fail-closed。`::a.b.c.d` は IPv4 として拒否、`::` / `::1` は IPv6 を維持 | 実装済み。サーバー側テスト11件（`server/src/network.rs` 6件 + `server/src/tests.rs` 5件）。下記「XFF 複数値・IPv4互換形式の検証（NET-06）」を参照 |
| PAY-04 | `verify_order` の CAS 2段目 | 確定処理の2段目 UPDATE が `changes() == 0` なら 409 を返し、成功応答しない | 実装済み（`server/src/payments.rs`）。下記「CAS 2段目と失効閾値レースの検証」を参照 |
| PAY-05 | ウォレット bind 署名 v2 と旧形式拒否（盲署名対策） | 署名対象は `order_id` / `chain_id` / `wallet_address` / `recipient_address` / `amount_jpy`。注文IDのみを署名対象にした旧形式は 401 で拒否する | 実装済み。`wallet_bind_message_v2` と `amount_jpy_from_wei` を追加し、`bind_wallet` を v2 へ切替。`test_bind_wallet_rejects_legacy_order_only_signature`（旧形式は `UNAUTHORIZED`）を追加。既存の bind 回帰はhelperを v2 署名へ更新。下記「ウォレット bind 署名 v2 の検証（PAY-05）」を参照 |
| ID-05 | 再認証時の失効閾値レース | 古い閾値以下の行だけを削除し、back-channel logout が登録した新しい失効を残す | 実装済み（`server/src/oidc.rs`）。下記「CAS 2段目と失効閾値レースの検証」を参照 |
| ID-07 | JWT 検証の追加カバレッジ（2026-11-18） | 検証鍵JSON（旧kid登録/不正JSON/kid境界/不正PEM）、alg混同の直接検証、`JWT_REQUIRE_KID`解析 | 実装済み・検証済み: `test_jwt_verification_keys_json_*` / `test_jwt_rejects_hs256_token_signed_with_public_pem` / `test_jwt_add_verification_key_kid_bounds` / `test_jwt_require_kid_from_env`（server/src/tests.rs、10件） |
| ID-06 | 全レスポンスのセキュリティヘッダ（SEC-01） | `X-Content-Type-Options: nosniff` と `X-Frame-Options: DENY` を全レスポンスへ付与する。CorsLayer が短絡する CORS プリフライト（OPTIONS の 200/204 成功レスポンスを含む）にも最外側の `security_headers` で付与し、本文なしの 204 でも欠落させない | 実装済み。`assert_security_headers` で検証: 403（`test_security_headers_on_error_response`）、OPTIONS プリフライト（`test_security_headers_on_cors_preflight`）、429（`test_security_headers_on_rate_limited_response`）、500（`test_security_headers_on_internal_error_response`）、204（`test_security_headers_on_no_content_response`）、通常 200（既存アサーション）。下記「全レスポンスのセキュリティヘッダの検証（ID-06）」を参照。CSP は未設計 |
| MET-01 | 中立化URL件数指標とプライバシー | 危険 URL を中和した件数だけを送り、URL 本文・IP・User-Agent を送らず記録しない。`GET /metrics` のカウンタは差分で増える | 実装済み。下記「中立化URL件数指標の検証（MET-01）」を参照。カウンタは参考値であり、単独のアラート判定・請求判断には使わない |
| MET-02 | `observe_metrics` の最外側配置 | 内側で短絡する 429 / IPv4 403 / 413 / CORS プリフライト OPTIONS 204 を取りこぼさず `openillustration_http_requests_total` と `openillustration_http_errors_total` へ計上する | 実装済み。`observe_metrics` を `Router` 最外側へ移動。下記4件の差分アサーションを追加（下記「observe_metrics の最外側配置の検証（MET-02）」を参照） |
| PROP-01 | `prop_jpy_to_wei_boundaries_and_reference` | 任意の `u64` で panic しない。`0` と `MAX_ORDER_JPY` 超（上限+1 と `u64::MAX`）は `Err`、範囲内は `WEI_PER_JPY = 10^18` の独立参照値（u128 計算）と一致 | 実装済み（128ケース）。下記「proptest の検証（PROP-01〜04）」を参照 |
| PROP-02 | `prop_is_valid_cid_only_accepts_canonical_ascii` | 空・非ASCII・128バイト超は必ず拒否。受理値は独立 oracle（v0: `Qm` + base58btc 44文字で計46文字、v1: `b` + base32lower 1文字以上）を満たす。既知の正規 CID（v0 / v1）は必ず受理 | 実装済み（128ケース）。下記「proptest の検証（PROP-01〜04）」を参照 |
| PROP-03 | `prop_summarize_reason_is_bounded_and_idempotent` | 任意の Unicode 文字列で panic せず、結果は120 Unicode 文字以下。空文字の対応を保持し、要約結果へ再適用しても値は変わらない（冪等） | 実装済み（128ケース）。下記「proptest の検証（PROP-01〜04）」を参照 |
| PROP-04 | `prop_summarize_reason_truncates_safe_text_at_char_boundary` | PII を含まない文字（あ / 知 / A / b / 空白 / 改行 / 🍣 / 。、0..=160個）で、120文字以下は等価、超過は先頭119文字＋`…` に切り詰め（Unicode 境界を維持） | 実装済み（128ケース）。下記「proptest の検証（PROP-01〜04）」を参照 |
| CLI-01 | `doctor` の純粋関数化と引数・設定判定（R5-06 / R4-01） | `parse_args_from` / `check_all_with` / `should_fail` を純粋関数として検証。13カテゴリ固定、秘密値非出力、既定は警告のみで exit 0、`--strict` は問題検出時のみ exit 1 | 実装済み。`server/src/bin/doctor.rs` に24テストを追加（ネットワーク設定警告4件を含む）。下記「3CLIの純粋関数化と自動テスト（CLI-01〜03）」を参照 |
| CLI-02 | `verify_audit_export` の `--expect-root` と検証結果の純粋関数化 | version 3 のみ受理し v1/v2 は fail-closed 拒否。改ざん entry / root / 署名を拒否。`--expect-root` は大小文字を区別せず照合し、不一致は `audit root mismatch` で拒否。受理・拒否いずれも理由本文と秘密値を出力しない | 実装済み。`server/src/bin/verify_audit_export.rs` に17テストを追加し、`VerificationOutcome` / `verify_export` を純粋化。下記「3CLIの純粋関数化と自動テスト（CLI-01〜03）」を参照 |
| CLI-03 | `verify_backup` の `--database-token` 拒否と sidecar fail-closed | `--database-token` は argv で受け付けず値もエコーしない（`DATABASE_TOKEN` 環境変数のみ）。`-wal` / `-shm` / `-journal` サイドカーが残っていれば DB を開く前に拒否。成功行は `backup_verification: OK tables=N migrations=N`、失敗時は stdout にエラー本文を出さない | 実装済み。`server/src/bin/verify_backup.rs` に17テストを追加し、`parse_args_from` / `decide_output` / `ensure_no_sidecars` / `verify_backup_at` を境界化。下記「3CLIの純粋関数化と自動テスト（CLI-01〜03）」を参照 |
| AUDIT-03 | `GET /content/{cid}/audit` の理由マスクと `entry_hash` 維持 | `reason` は `summarize_reason` の結果のみで、メール・URL・4桁数字・DB 全文が漏れない。`entry_hash` は生 `reason` から計算した DB の生値と一致し、DB 全文は不変 | 実装済み（`test_get_content_audit_masks_reason_but_preserves_entry_hash`）。下記「監査理由マスク・投票署名 v2・価格 NULL・エクスポート解析の検証」を参照 |
| MOD-12 | 投票署名 v2 と投票者束縛（旧形式拒否） | 署名対象は `domain: "moderation-vote-v2"` / `report_id` / `result` / `juror_hash`。旧 v1 形式の署名と別投票者向けに束縛された署名は401で拒否し票を記録しない。正常系は受理され1票が記録される | 実装済み。サーバー側テスト4件（`test_moderation_vote_message_v2_binds_all_fields` / `test_vote_v2_signature_accepted` / `test_vote_rejects_legacy_v1_signature` / `test_vote_rejects_signature_bound_to_other_juror`）。下記同節を参照 |
| CONTENT-10 | 一覧の `price` NULL | `status = 'Approved'` の行の `price` が NULL でも500にならず、購入不可を表す0として返る | 実装済み（`test_list_contents_null_price_is_returned_as_zero`）。`SELECT` 句は `COALESCE(price, 0) AS price`。下記同節を参照 |
| EXPORT-11 | エクスポート解析失敗の表示 | `DataExport` として読めないレスポンスでは「0 件」成功にせず、`data-testid="export-error"` に固定文言 `エクスポート結果の解析に失敗しました。` を表示し、`export-status` は空のまま | 実装済み。クライアント側単体テスト `export_parse_failure_uses_fixed_phrase_without_success_status`（`client/src/lib.rs`）。正常系は件数入りの成功文言を維持。下記同節を参照 |

### 年齢ゲート共通化と TZ 境界の検証（AGE-02〜04）

テストの根拠は R1-17（年齢ゲートのバイパス遮断）と DEC-02 のフェイルクローズ方針。
`server/src/tests.rs` に次の3件を追加し、`server/src/lib.rs` の `ensure_age_policy_configured` /
`today_for_policy` / `validate_age_at` を直接・間接に検証する。

| テスト関数 | 入力・操作 | 期待結果（test oracle） |
| --- | --- | --- |
| `test_require_explicit_configuration_blocks_oidc_callback_and_refresh` | `require_explicit_configuration: true` / `operator_configured: false` で callback（任意 state）と refresh（任意 Cookie）を呼ぶ | 両方とも 403。state / Cookie の消費前に共通ゲートで拒否 |
| `test_today_for_policy_uses_configured_local_timezone` | UTC `2024-01-01T16:00:00Z` に `None` / `UTC` / `Etc/GMT+5` / `Etc/GMT-9` / ` Asia/Tokyo ` / `Asia/Tokyo` / `America/New_York` を渡す | 最初の3件と未対応名は 2024-01-01、`Etc/GMT-9` と `Asia/Tokyo`（空白入り含む）は 2024-01-02 |
| `test_validate_age_at_uses_local_today_for_age_boundary` | 2003-01-02 生まれ・JP・既定下限 21 歳、同じ現在時刻で TZ を変える | `Asia/Tokyo` / `Etc/GMT-9` は 21 歳で `Ok`、`UTC` は 20 歳で `Forbidden`（403） |

TZ の強制変更はプロセス環境を汚さないため、純粋関数 `today_for_policy` / `validate_age_at` に
基準時刻と TZ を明示して検証する。未知の TZ は UTC へフォールバックする分岐も含める。
年齢数値の判定は `/auth/verify` の `validate_age` に委ね、callback / refresh では設定エラーのみを
403 へ変換する。これらはサーバー側テストであり、E2E 定義数は35件のまま変わらない。

### エクスポートの読み取りトランザクション検証（EXPORT-09〜10）

R5-04 の12表エクスポートを単一時点のスナップショットにするため、`me_export` は
`db.transaction()`（`BEGIN DEFERRED`）で `collect_export_snapshot` を包み、`tx.commit()` で閉じる。
書き込みロックを取らないため、共有 `Connection` 上で別リクエストが書き込んでも競合しない。

| テスト関数 | 検証する test oracle |
| --- | --- |
| `me_export_queries_run_inside_read_transaction` | `version:1` が返り、`debug_assert!(!db.is_autocommit())` が発火しない。`db.transaction()` / `tx.commit()` を外す mutation で失敗 |
| `me_export_snapshot_consistent_under_concurrent_ban_write` | 一時 DB の2接続で、書き込み側の `BEGIN IMMEDIATE` 未コミット BAN 更新・監査挿入がエクスポート結果に現れない（`is_banned: false`、BAN 監査なし）。完了後に書き込みを `COMMIT` |

これらはサーバー側テスト2件のみで、E2E 定義数は35件のまま変わらない。

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
マスクは EXPORT-04 の既存挙動を維持する。本テストは第三者公開（DEC-04 未決定）を
証明するものではない。

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
| P1 | `ALLOW_MOCK_AUTH=1` のみ | `doctor: 13 check(s), 1 problem(s)` + `mock_auth_enabled` 警告 | 0 | 1 |
| P2 | P1 + `PINATA_JWT=<本番相当値>` | `13 check(s), 2 problem(s)` + `mock_auth_with_production_config` / `mock_auth_enabled` 警告 | 0 | 1 |
| P3 | P1 + `OIDC_ISSUER` / `OIDC_CLIENT_ID` | `13 check(s), 2 problem(s)` + 同上の2警告 | 0 | 1 |
| P4 | P1 + `JWT_RSA_PRIVATE_KEY_PEM`（不正ダミー） | `13 check(s), 3 problem(s)` + `jwt` / `mock_auth_with_production_config` / `mock_auth_enabled` 警告 | 0 | 1 |
| P5 | P1 + `PINATA_JWT=mock_jwt`（境界値） | `13 check(s), 1 problem(s)` + `mock_auth_enabled` 警告 | 0 | 1 |

いずれもダミーの `JWT_SECRET`（32バイト以上）を設定した実測値である（2026-11-18、ビルド済み
`target/debug/doctor` で確認）。`mock_auth_enabled` / `ipv4_localhost` / `trusted_proxy` の追加に
より検査数は13となり、`ALLOW_MOCK_AUTH=1` 単独でも問題が1件計上される。
`ALLOW_IPV4_LOCALHOST` / `TRUSTED_PROXY_CIDRS` 未設定なら追加問題は出ない。よって P5 は
`--strict` で終了コード1であり（旧挙動の0ではない）、`--strict` の判定は
`should_fail(strict, problems)` に委譲される。P4 の jwt 警告はダミー PEM が無効なことに
よるもので、mock 警告とは独立している。別途 `env -i ALLOW_MOCK_AUTH=1 ./target/debug/doctor`
の実出力で `13 check(s), 2 problem(s)`（`JWT_SECRET` 未設定による `jwt` 警告を含む）を確認した。

### 3CLIの純粋関数化と自動テスト（CLI-01〜03）

今回の追加は全て `server/src/bin/` 配下のサーバー側 bin テストであり、E2E 定義数は35件のまま
変更しない。各 CLI は I/O・環境変数読み取り・`std::process::exit` を薄い `main` 側へ追い出し、
判定部分を純粋関数（または入力を引数で受け取る境界関数）としてテストする。

#### CLI-01: `doctor`（`server/src/bin/doctor.rs`、24テスト）

- `parse_args_from<I>(items) -> ParseOutcome`: 引数は `Vec<String>` で受け取り、先頭要素を
  プログラム名として読み飛ばす。`[]` は `Run { strict: false }`、`["--strict"]` は
  `Run { strict: true }`、`-h` / `--help` は `Help`、単一の不明引数は
  `UnknownArgument(String)`、複数引数は `TooManyArguments` を返す（完全一致のみ有効）。
- `check_all_with<F>(lookup) -> Vec<(&'static str, Vec<String>)>`: 環境変数を `lookup`
  経由でのみ読み、13カテゴリを固定で返す（`production` /
  `mock_auth_with_production_config` / `mock_auth_enabled` / `jwt` / `jwt_secret` /
  `ipfs_gateways` / `ipfs_timeout` / `ipfs_max_attempts` / `oidc_http_timeout` /
  `age_policy` / `rate_limit` / `ipv4_localhost` / `trusted_proxy`）。秘密値は問題文へ
  含めない。`check_all()` は実環境の `env` を渡すだけのラッパーである。
- `should_fail(strict, problems) -> bool`: `strict && problems > 0` のみ。既定は警告表示で
  exit 0、`--strict` のとき問題が1件以上なら exit 1。

| 群 | 件数 | テスト名 |
| --- | --- | --- |
| 引数パース | 6 | `parse_no_arguments_runs_non_strict` / `parse_strict_flag_runs_strict` / `parse_help_flags_request_help` / `parse_unknown_argument_is_reported` / `parse_multiple_arguments_is_rejected` / `parse_only_the_exact_strict_flag_enables_strict` |
| 設定判定 | 15 | `check_always_reports_thirteen_categories` / `mock_auth_alone_adds_a_warning` / `mock_auth_truthy_values_are_recognized_and_falsy_are_not` / `unset_mock_auth_is_disabled` / `mock_auth_combined_with_production_config_is_reported` / `mock_auth_with_mock_pinata_value_is_not_a_combination` / `invalid_ipfs_gateway_entries_increase_problem_count` / `all_invalid_ipfs_gateways_report_both_warnings` / `unset_ipfs_gateways_uses_default_without_warning` / `valid_ipfs_gateways_add_no_invalid_entry_warning` / `secret_values_are_never_printed_in_problems` / `ipv4_localhost_truthy_values_add_a_warning` / `ipv4_localhost_falsy_and_unset_add_no_warning` / `trusted_proxy_when_configured_warns_without_disclosing_value` / `trusted_proxy_unset_adds_no_warning` |
| strict判定 | 3 | `non_strict_never_fails` / `strict_fails_only_when_problems_exist` / `end_to_end_exit_decision_for_mock_only_configuration` |

真値は `1` と `true`（大文字小文字不問）のみで、`0` / `false` / `no` / 空文字は偽である。
`ALLOW_MOCK_AUTH=1` 単独でも `mock_auth_enabled` に1件計上され、`--strict` で exit 1 になる。

##### doctor のネットワーク設定警告（追加4件）

`ipv4_localhost` と `trusted_proxy` の2カテゴリを追加し、検査数は11→13、bin テストは20→24件へ
増えた（既存 `check_always_reports_eleven_categories` は
`check_always_reports_thirteen_categories` へ改称）。追加4件の oracle は次のとおり。

| テスト | 条件 | 期待結果 |
| --- | --- | --- |
| `ipv4_localhost_truthy_values_add_a_warning` | `ALLOW_IPV4_LOCALHOST` が `1` / `true` / `TRUE` / `TrUe` | `ipv4_localhost` に警告1件。文言は固定で値をエコーしない |
| `ipv4_localhost_falsy_and_unset_add_no_warning` | `0` / `false` / `no` / 空 / 未設定 | 警告なし |
| `trusted_proxy_when_configured_warns_without_disclosing_value` | `TRUSTED_PROXY_CIDRS` 設定 | `trusted_proxy` に警告1件。`X-Forwarded-For` / `XFF` を含むが CIDR 値は漏えいしない |
| `trusted_proxy_unset_adds_no_warning` | 未設定 | 警告なし |

`should_fail(strict, problems)` の判定は不変で、`--strict` のときのみ問題があれば exit 1 となる。
実バイナリで `env -i ALLOW_MOCK_AUTH=1 ./target/debug/doctor` を実行し、
`doctor: 13 check(s), 2 problem(s)`（`mock_auth_enabled` + `jwt`）を確認した。

#### CLI-02: `verify_audit_export`（`server/src/bin/verify_audit_export.rs`、17テスト）

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
  不一致なら `audit root mismatch: expected=<値>, computed=<root>` を返す。
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

#### CLI-03: `verify_backup`（`server/src/bin/verify_backup.rs`、17テスト）

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
- ID-05: 再認証時に `revoked_subs` の閾値をメモリから取り出し、DB は
  `DELETE FROM revoked_subs WHERE sub = ? AND revoked_at <= ?` で古い閾値以下のみ削除する。
  これにより削除前に back-channel logout が登録した新しい失効が残る。既存の関連テストは
  `test_backchannel_logout_for_sub_allows_reauthentication` と `test_purge_stale_revoked_subs`。
- 専用テスト名は確定済みで、PAY-04 は `test_payment_verify_stage2_cas_rejects_race_after_detected`、
  ID-05 は `test_reauthentication_delete_keeps_newer_revocation_threshold` が検証する。

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
有効期限のエラーパス5件を検証する（a6eba14）。`assert_unauthorized_contains` が
`AppError::Unauthorized` と固定メッセージの部分列を確認する。

| テスト | 入力 | 期待 Unauthorized メッセージ |
| --- | --- | --- |
| `invalid_base64_token_is_unauthorized` | `!!!not-base64!!!` | `Invalid JPKI presentation token` |
| `non_x509_bytes_are_rejected` | `AAECAw==`（base64 で `00 01 02 03`） | `X.509 parse failed` |
| `expired_certificate_is_rejected` | rcgen で `not_before=2020-01-01` / `not_after=2021-01-01` の証明書 | `Certificate is expired or not yet valid` |
| `envelope_without_credentials_falls_back_to_raw_token` | `{"certificate":null,"verifiableCredential":null}` | `Invalid JPKI presentation token`（JSON テキストを生 base64 としてデコードし失敗） |
| `empty_token_is_rejected` | `""`（空バイト列は有効な base64） | `X.509 parse failed` |

正常系の対照として `mock_token_requires_allow_flag` / `mock_token_with_key_succeeds` /
`parses_rcgen_certificate` も維持する。実JPKI実機・失効確認は未検証。

### ログアウト削除 Cookie の検証（ID-03）

`POST /auth/logout` は DB 側のファミリ失効に加え、ブラウザへ残った Cookie を即時削除する。

- `test_logout_clears_refresh_cookie`: 提示した `refresh_token` の値をエコーせず、
  `refresh_token=;`（空値）、`Path=/auth;`、`HttpOnly`、`SameSite=Strict`、`Max-Age=0`
  を含む `Set-Cookie` を返す。ステータスは200、本文は既存の `true` を維持する。
- `test_logout_deletion_cookie_follows_cookie_secure_setting`: `OIDC_COOKIE_SECURE` が
  `0` / `false` のときは Secure なし、`1` / 未設定のときは Secure 付き（未設定は fail-closed）。

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
クライアント側単体テストである。いずれも E2E 定義を増やさず、E2E 定義数は35件のまま。

#### AUDIT-03: 監査理由の表示マスクと entry_hash の生値維持

`test_get_content_audit_masks_reason_but_preserves_entry_hash` が、PII を含む生 reason
（`contact user@example.com https://example.com/a code 1234`）を登録した上で
`GET /content/{cid}/audit` を呼び、次を検証する。

| 検証点 | 期待結果 |
| --- | --- |
| レスポンスの `reason` | `common::summarize_reason(raw_reason)` と一致し、`[redacted]` を含む。生 reason・メール・URL・4桁数字は本文に現れない |
| レスポンスの `entry_hash` | 生 reason から `calculate_hash_v2()` で計算した値と一致（マスク後では再計算しない） |
| DB の `reason` / `entry_hash` | 更新前と同じ生値。再計算値も DB の `entry_hash` と一致 |

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

## 4. 実行コマンド

```bash
cargo test --workspace
cargo test -p server jpki   # ID-01 JPKI 証明書エラーパス（5件）
cargo test -p server logout_clears_refresh_cookie   # ID-03 ログアウト削除 Cookie
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
cargo test -p server --lib me_export   # R5-04 エクスポートの回帰のみ
cargo test -p common summarize_reason   # 要約の単体テスト（PII除去・境界・日本語）
cargo test -p server list_reports       # GET /mod/reports の要約適用・全文非漏えいの回帰のみ
cargo test -p server test_get_content_audit_masks_reason_but_preserves_entry_hash   # AUDIT-03 監査理由マスクと entry_hash 維持
cargo test -p server moderation_vote_message_v2   # MOD-12 メッセージの全フィールド束縛
cargo test -p server test_vote_v2_signature_accepted   # MOD-12 正常系の受理
cargo test -p server test_vote_rejects_legacy_v1_signature   # MOD-12 旧形式署名の401拒否
cargo test -p server test_vote_rejects_signature_bound_to_other_juror   # MOD-12 別投票者向け署名の転用拒否
cargo test -p server test_list_contents_null_price_is_returned_as_zero   # CONTENT-10 価格 NULL の0扱い
cargo test -p client export_parse_failure_uses_fixed_phrase_without_success_status   # EXPORT-11 クライアント側
cargo run -p server --bin doctor -- --strict   # CFG-01 設定診断（問題ありで exit 1）
cd e2e && npm test
cd e2e && npm test -- search.spec.ts   # R5-01 検索 E2E のみ
cd e2e && npm test -- favorites.spec.ts   # R5-02 お気に入り E2E のみ
cd e2e && npm test -- moderation.spec.ts   # R5-09 透明性ダッシュボード + 投票フィードバック + 投票一覧エラー E2E（9件）
cd e2e && npm test -- export.spec.ts   # R5-04 データエクスポート E2E のみ
cd e2e && npm test -- metrics.spec.ts   # MET-01 中立化URL件数 E2E のみ
cd e2e && npm test -- real-api.spec.ts   # 実サーバー結合 E2E（4件）のみ
cd e2e && npm test -- error-paths.spec.ts   # エラーパス E2E（4件）のみ
cd e2e && npm test -- integrity.spec.ts   # INT-01 完全性 E2E（3件: 正規/改ざん/デコード失敗）のみ
```

E2E の初回実行時は `cd e2e && npm ci` で依存を導入する。
既定は `chromium`（システム Chrome）のみ。`E2E_PROJECTS=chromium,firefox,webkit`
で複数ブラウザを選択できるが、firefox / webkit はバイナリ導入と共有ライブラリが
必要で、本ホスト（Kubuntu 26.04）では Playwright 未対応プラットフォームのため
未検証（CI 環境での導入は別途検討）。
開発サーバーとクライアントは `e2e/playwright.config.ts` の `webServer` が
自動起動する（モック認証 `ALLOW_MOCK_AUTH=1`、ローカルでは開発用の
固定 JWT シークレット。`E2E_JWT_SECRET` で上書きできる）。
`channel: chrome` を使うため Google Chrome・クライアントビルド用の
trunk・Node 18 以上が必要である。trunk は CI と同じ
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
   通信失敗時はカード内に `Failed to update favorite (HTTP <status>).` または
   `Network error updating favorite.` が表示される。
3. お気に入り状態のコンテンツで検索フォームの `Favorites only` チェックボックスをONにし、
   `/content` へ `favorite_only=1` が付与され、該当コンテンツのみが表示されることを確認する。
   既存の `q` / `tag` / `type` との併用はAPIテスト（FAV-04）で担保する。
4. リクエスト回数と内容を完全一致で検証する。追加後の一覧再取得は `/content` への
   リクエストがちょうど2回で、クエリは `{ q: null, tag: null, type: null, favorite_only: null }`
   の2要素に一致する。一連の操作全体では8回で、`favorite_only=1` や `q` の付与順まで
   配列の完全一致で確認する。お気に入りの POST / DELETE もメソッドとパス
   （`/content/{cid}/favorite`）の完全一致で検証し、過剰な再フェッチや想定外の
   クエリ混入を検出する。

### E2E 透明性ダッシュボードシナリオ（R5-09）

`e2e/tests/moderation.spec.ts` の `Moderation & Governance (GOV-01, GOV-02)` に含まれる
`R5-09: should display moderation stats for operators and permission error on 403` が、
モック認証（`ALLOW_MOCK_AUTH=1`）で `GET /mod/stats` の応答を差し替えて次を確認する。

1. 運用者としてログインし、MODERATION 画面を開く。`TRANSPARENCY DASHBOARD`
   （`data-testid="moderation-stats-section"`）と `GENERATED_AT: <generated_at>` が表示され、
   `data-testid="moderation-stats-totals"` に Reports / Concluded / Approved / Rejected /
   Quorum Not Met / Bans / Unbans / Unpins、`data-testid="moderation-stats-monthly"` に
   Month / Reports / Concluded / Approved / Rejected / Bans / Unbans / Unpins の列が並ぶことを確認する。
2. 同じ画面で `/mod/stats` の応答を403に差し替えると、`role="alert"` と
   `data-testid="moderation-stats-error"` に `運用者権限が必要です` が表示されることを確認する。
3. 動的文字列（`generated_at` / `month` / 数値）は既存の `escape_html_text` を通して描画される。
   レスポンスに CID / reason / `user_hash` は無いため表示されず、サーバー側の
   `test_mod_stats_response_contains_no_sensitive_fields` と合わせて個人情報非含有を担保する。
   本シナリオは認証内集計の表示確認であり、第三者公開（DEC-04 未決定）を証明するものではない。
4. `/mod/stats` へのリクエストは初期表示と画面遷移後のちょうど2回で、いずれも
   メソッド `GET`、パス `/mod/stats`、`Authorization: Bearer mock_jwt_token` であることを
   配列の完全一致で検証する（回数は `statsRequestCount === 2` も確認）。余計な再フェッチや
   未認証リクエストを検出する。

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

実行例は `cd e2e && npm test -- moderation.spec.ts`。chromium で9件すべて pass を確認済み。

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

実装は失敗時に `reports` を空にせず既存データを維持し、成功時にのみ一覧を更新して
エラーをクリアする。JSON 解析失敗は固定文言 `投票一覧の解析に失敗しました。` を
表示し、解析エラーの詳細は出さない。この文言はクライアント側単体テスト
`reports_error_uses_only_fixed_phrases` で担保する。実行例は
`cd e2e && npm test -- moderation.spec.ts`。chromium で9件すべて pass を確認済み。

### E2E エクスポートシナリオ（R5-04）

`e2e/tests/export.spec.ts` の `R5-04: Export My Data` は、モック認証
（`ALLOW_MOCK_AUTH=1`）でログイン後、Dashboard の `EXPORT MY DATA`
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
4. 実装側で chromium / firefox の2テストが pass していることを確認済み。webkit はホスト側の
   libicu/libxml2 不在で起動不可（環境依存）のため未検証。

### E2E エラーパスシナリオ（DASH-ERR / R5-04 network / AUTH-500 / AUDIT-500）

`e2e/tests/error-paths.spec.ts` は API 自体の挙動ではなく、クライアントが失敗を画面へ告知することを
検証する（4件）。実行コマンドは `cd e2e && npm test -- error-paths.spec.ts`。
テスト名は次のとおり。

1. `DASH-ERR: GET /content が network error のときダッシュボードにエラー表示が出ること`:
   `/content` を abort し、`Network error loading content.` が表示される。
2. `R5-04: GET /me/export が network error のとき role=alert にネットワークエラー文言が出ること`:
   `/me/export` を abort し、`data-testid="export-error"` の `role="alert"` に
   `ネットワークエラーが発生しました。` が表示され、成功ステータスが出ない。
3. `AUTH-500: /auth/verify が HTTP 500 のとき login-error に文言が出て UPLOAD が出ないこと`:
   `/auth/verify` を500に差し替え、`data-testid="login-error"` の `role="alert"` に
   `認証に失敗しました (HTTP 500).` が表示され UPLOAD は出ない。
4. `AUDIT-500: /content/{cid}/audit が HTTP 500 のとき監査パネルに role=alert が出ること`:
   監査エンドポイントを500に差し替え、監査パネルの `data-testid="audit-error"` の
   `role="alert"` に `監査ログの取得に失敗しました (HTTP 500).` が表示される。

### E2E 完全性検証シナリオ（INT-01 / R1-13）

`e2e/tests/integrity.spec.ts` の `Forensic Integrity Verification (INT-01)` は、モック認証
（`ALLOW_MOCK_AUTH=1`）でログイン後、`/content` と `/content/{cid}` を差し替えて検証状態の
表示を確認する（3件）。genuine / tampered の既存2件は変更せず、R1-13 の回帰1件を追加した。

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

### E2E Upload シナリオ（R1-14 / INT-02 / CONTENT-01）

`e2e/tests/upload.spec.ts` の `Content Upload Flow` は、モック認証（`ALLOW_MOCK_AUTH=1`）
でログイン後に UPLOAD 画面へ進み、送信成功と失敗時の表示を検証する（4件）。
テスト名と確認点は次のとおり。

1. `should upload JXL content successfully`: JXL ファイルを選択して送信し、
   `SUCCESS: Content registered` が表示されることを確認する。
2. `should reject files over 10 MiB before upload`: 10 MiB 超のファイルを選択した時点で
   `ERROR: File exceeds the 10 MiB limit.` を表示し、送信ボタンは無効のまま、
   `/content/upload` へのリクエストが0回（未送信）であることを確認する。
3. `INT-02: should reject ZIP files`: 許可外 MIME のファイルを送信すると API が400を返し、
   `ERROR: Upload failed` が表示されることを確認する。
4. `R1-14: shows an error and re-enables the button when arrayBuffer() rejects`:
   `Blob.prototype.arrayBuffer` を拒否させ、準備フェーズの unwrap パニック
   （`Processing...` のまま固まる状態）を再現する。`data-testid="upload-status"` に固定
   文言 `アップロードの準備に失敗しました。` が表示され、送信ボタンが再活性化して
   `Processing...` のまま残らないことを確認する。

失敗時は生のエラー値・内部情報を画面へ出さず、固定文言のみを表示する。クライアント
（`client/src/lib.rs`）は準備・通信・応答読取の各段を `Result` で伝播させ、
成否にかかわらず `set_loading(false)` を実行してボタンを再活性化する。固定文言は3種類。

| 失敗段階 | 固定文言 |
| --- | --- |
| 準備（`array_buffer` / `FormData` / `Blob` 生成など） | `アップロードの準備に失敗しました。` |
| 通信（`Request::send`） | `ネットワークエラーが発生しました。` |
| 応答読取（`response.json::<String>()`） | `アップロード結果の読み取りに失敗しました。` |

`data-testid="upload-status"` は `[STATUS]` の接頭辞に続けて状態文言を描画し、
文言が空のときは `Show` により非表示になる。loading 中は送信ボタンが `Processing...`
かつ `disabled` で、完了後に `Sign & Submit to Evidence Ledger` へ戻る。
HTTP エラー応答（非2xx）は固定文言3種とは別に `ERROR: Upload failed.` を表示する。

### 中立化URL件数指標の検証（MET-01）

`POST /metrics/client-events` と `GET /metrics` の `openillustration_neutralized_urls_total` を対象に、
クライアント送信とサーバー側の集計を分離して検証する。

このカウンタは参考値であり、アラート判定や請求等の意思決定に単独で使わない。
`POST /metrics/client-events` は認証不要のため `count=1000` の連投でカウンタを汚染でき、
累積値を不正検知や課金の根拠として扱うアサーションは設けない。
検証は差分加算・ペイロードの非機微性・レート制限の適用に限定し、
判断には認証済みデータやサーバー側の独立した記録を突き合わせる。

#### サーバー側差分アサーション

カウンタはプロセス内で他のテストと値を共有するため、絶対値ではなく送信前後の差分（`+N`）で検証する。
テスト名は実装側と連携して確定した次の値を使う。

- `test_metrics_expose_prometheus_counters`: 既存テストを拡張し、新カウンタが HELP/TYPE 行付きで
  Prometheus 形式に現れ、POST 前後の差分が送信 count と一致することを確認する。
- `test_client_events_neutralized_url_increments_counter`: 正常系（count 指定・省略）で 204
  （本文なし）を返し、カウンタが `count` だけ増えることを確認する。
- `test_client_events_rejects_invalid_payloads`: 不正 event、count（0 / 1001 / 負 / 小数 /
  文字列）、不正 JSON を 400 で拒否し、カウンタが増えないことを確認する。
- `test_client_events_rejects_oversized_body`: 1 KiB 超のボディを 400 で拒否し、
  カウンタが増えないことを確認する。
- `test_client_events_is_rate_limited`: レート制限の対象であり、超過時に 429 を返すことを確認する。
- URL 本文・IP・User-Agent は送らず、サーバーも受信・記録・ログ出力しない。ペイロードのキーは
  `event` と `count` のみで、成功レスポンスにも本文を返さない。

#### クライアント側単体テスト

- `client_event_payload_is_minimal_and_clamped`: ペイロードが `event` と `count` のみで、
  0 件は送信せず、上限は 1000 にクランプされることを確認する。
- `render_markdown_counts_neutralized_urls`: `render_markdown()` 1 回で中和された危険 URL の
  件数を数え、1 描画につき 1 回の送信へ集約されることを確認する。

#### E2E シナリオ

`e2e/tests/metrics.spec.ts` の
`MET-01: neutralized URL count is posted once as a minimal client event` が、モック認証
（`ALLOW_MOCK_AUTH=1`）で次を確認する（chromium、1件）。

1. 危険 URL を 2 件含む Markdown（`[click](javascript:alert(1))` と
   `![img](data:text/html,alert)`）を Novel 本文として返し、チェックサム一致後に
   `a[href="#"]` と `img[src="#"]` が描画される。
2. `page.route('**/metrics/client-events', ...)` で捕捉した POST はちょうど1本で、
   本文は `{"event":"neutralized_url","count":2}` と JSON deep-equal になる。
3. 生 JSON に URL 本文（`javascript:alert(1)` / `data:text/html,alert` / タイトル）が含まれず、
   キーは `count` と `event` のみ。`authorization` / `x-forwarded-for` ヘッダも送らない。
4. 204（本文なし）のモック応答で画面に `role="alert"` が出ない。API 側のバリデーションと
   カウンタの差分検証はサーバー側単体テストへ委譲し、本シナリオでは重複して検証しない。

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
同ファイル内に鍵権限・トークン・上書き拒否の9テストを追加済み。下表はその契約の一覧であり、
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
| 出力の上書き | `--output` に既存ファイルを指定 | 拒否（`create_new` により原子的に失敗） |

## 5. リリース証跡

リリースごとにテスト結果・ビルド成果物・依存ロックファイルを保存する。Nix再現可能ビルド、オンチェーン記録、バイナリ同一性の証明は別途設計・実装するまでリリース保証として主張しない。
