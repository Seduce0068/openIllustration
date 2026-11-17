---
title: "テスト仕様書 (Test Specification Document)"
type: Test Specification
description: 現行回帰テストと必須要件のリリース受け入れ条件を定義する。
tags: [testing, acceptance, quality]
generated:
  by: dsh
  at: "2026-11-17T13:02:57+09:00"
---

## テスト仕様書 (Test Specification Document)

本書は現行プロトタイプの回帰テストと、必須要件のリリース受け入れテストを区別する。
JPKI、JPYC、第三者検証可能な監査は、実装前だからといって最終検証対象から除外しない。

## 1. テスト層

- **Unit**: `common` のチェックサム・年齢計算・監査ハッシュ等。
- **Integration**: Axumルーター、JWT認可、DB状態遷移、制約の検証。
- **E2E**: PlaywrightによるUIフロー確認。APIをモックするテストは実バックエンド統合を証明しない。

## 2. 実装済みテストの概要

| 領域 | 現行カバレッジ | 制約 |
| --- | --- | --- |
| セキュリティヘッダ | 全レスポンスへ `X-Content-Type-Options: nosniff` と `X-Frame-Options: DENY` を付与（IP ゲートの 403 を含む） | 単体テストと実サーバーで確認済み。CSP は未設計 |
| 認証 | モックverify（nonce 必須・単回消費・再利用400）、無効トークン系、verify必須フィールド（birth_date/country_code）欠落と不正birth_dateの400拒否、OIDCはstate再利用/未知state、nonce不一致、JWKS未登録鍵での署名、kid欠落、at_hash不一致を401で拒否、OIDC未構成時のstart/callback/back-channel logoutは501、年齢判定（`common` のポリシー/計算ユニットテスト、`server` の検証・`GET /policy/age` 公開応答・明示設定必須/信頼属性必須フェイルクローズ、独立参照計算による proptest と境界月日の明示的サンプリング）、JWT秘密鍵長検査のユニットテスト、RS256 の `kid` 付与・一致検証・未知 `kid` 拒否・`kid` 無し旧トークン互換・旧公開鍵によるローテーション検証・重複 kid 拒否、refresh Cookie の Path=/auth（ログアウト時のファミリ失効到達）を検証 | 実JPKI/OID4VPの信頼検証、検証済み年齢属性入力、HSM 鍵管理は未テスト・未実装。現行年齢判定は申告値前提 |
| ネットワーク | IPv4拒否・IPv6許可・IPv4-mapped正規化・XFF偽装回帰・fail-closedのミドルウェアテスト、CORSオリジン解析テスト | 実プロキシ、多段構成、WAFは未検証 |
| エラー公開 | 500系詳細の既定非公開と `EXPOSE_INTERNAL_ERRORS` での公開のユニットテスト | 4xx系メッセージの公開範囲は未整理 |
| 可観測性 | `RUST_LOG` のフィルタ動作（未設定時INFO、不正値フォールバック）と WARN 出力の起動確認。`LOG_FORMAT=json` で全ログ行が JSON としてパースできることを実サーバーで確認。`/metrics` が Prometheus 形式でリクエスト数・4xx/5xx 数を公開し、404 後にエラーカウンタが進むことを単体テストと実サーバーで確認 | ログ集約基盤・アラート通知先の選定は未実施 |
| 停止動作 | `/health` 200・`/ready` 200・停止フラグ時の `/ready` 503・IP ゲート適用・`GRACEFUL_DRAIN_TIMEOUT_SECS` の既定/範囲外フォールバックを自動テストで確認。SIGTERM 時の drain ログと終了コード0は実起動確認済み。単一ライター検出（生存 PID 拒否・stale 回収・停止時削除）を単体テストと実プロセス2起動で確認 | 外部依存障害時の readiness 連動・drain タイムアウト発生時の実動作・リモート DB の並列起動制御は未試験 |
| DBマイグレーション | 未適用分の適用・冪等性・チェックサム改変検出・外部キー強制・既存DBの段階的アップグレード（new migration の backfill とデータ保持）・適用済みマイグレーションファイル欠落の検出をユニットテスト。migration 011 で追加したメタデータ列を既存行が空文字として読むことを `test_migration_011_existing_rows_read_as_empty_metadata` で検証 | ロールバック、複数インスタンス同時起動の排他は未実施 |
| 完全性 | SHA-256一致・不一致、IPFS取得系をローカルモックで検証（サイズ超過・タイムアウト・次ゲートウェイへのフォールバック・ストリーム途中切断を含む）、`IPFS_MAX_ATTEMPTS` の既定/範囲検証、CIDのURLパス注入拒否、取得/監査系の不正CID 400・不存在CID 404、チェックサム不一致時の WARN ログ | 実ゲートウェイの遅延・切断、実Pinata連携、厳密なCID仕様検証、ネットワークエラー時の実リトライは未検証 |
| Markdown表示 | raw HTMLのエスケープ、インライン・参照リンク・自動リンク・画像URLの危険スキーム拒否（実体参照による難読化、Unicode 空白/制御文字（ZWSP・NBSP）によるスキーム隠しを含む）と安全スキーム維持をユニットテスト。危険 URL が `render_markdown` を通した後も `href="#"` / `src="#"` に置換される統合検証も実施 | ブラウザ別URL解釈のE2Eは未実施 |
| 投稿入力 | 許可MIME、JPEG XLシグネチャ、Markdown UTF-8、重複CIDの再投稿拒否、未知フィールド拒否、全フィールド（`price` / `file` / `owner_hash` / `signature`）の重複 400 拒否、WASM 側 JXL デコードの寸法上限（8000px / 64M ピクセル）とチャンネル数別（1/2/3/4以上）RGBA 変換、クライアント選択時の 10 MiB 超過即時拒否（E2E・送信ボタン無効化と未送信も検証）を検証 | 実 JXL デコードの WASM 実行試験は未実施 |
| モデレーション | 通報状態遷移、重複通報拒否、通報reasonの空/過大拒否とトリム保存、期限前は確定しないこと、期限後確定（同数→`Rejected`、定足数未達→`Approved` 復帰）、期限切れ投票拒否、投稿者投票拒否、BAN済み投票者の拒否、30日以内の異議と30日超の通常通報、finalize エンドポイントの冪等性、重複投票拒否、確定後投票拒否、投票ペイロード検証（report_id/result）とトークンsub不一致401の拒否、投票署名メッセージが report_id と result の両方に束縛されること、運用者未設定時の BAN/UNBAN/UNPIN 拒否、運用者による BAN（根拠・期間）/UNBAN/UNPIN の監査記録、自己BAN・不正期間・空理由・BAN済み運用者の拒否、期限前スキップ・期限後自動確定・複数件処理・冪等性・ワーカー周期の範囲検証 | 同一接続への並行 `BEGIN` が失敗することの再現、書き込みロックによる直列化、並行投票と自動確定の同時実行を検証。ワーカー停止/再起動時の挙動、実運用手続、第三者検証可能な監査（DEC-04）の試験が不足。reason上限値の妥当性はADR-0004の運用で見直し |
| Upload | Content-Typeのユニット/APIテスト、負価格・非整数価格・64bit範囲外価格・上限超過価格の拒否（拒否理由を検証）、価格0と上限値 `MAX_ORDER_JPY` の受理と保存を検証、必須フィールド欠落・owner不一致・BANユーザーの拒否、実 RSA 鍵で署名したバイト列の受理と別バイト列への署名使い回し401拒否 | 実multipartサイズ境界・実Pinata連携の試験が不足 |
| ContentMeta / list_contents | `ContentMeta` が `title` / `description` / `tags` を返すこと、`list_contents` が省略可の `q` / `tag` / `type` / `limit` / `offset` を受理し既定値（limit=50、offset=0）と範囲（`1..=100`）へフォールバックすることを API テストで検証。`q` の部分一致、`tag` の前方一致、`type` の完全一致、`created_at DESC` の並びとページングの重複なしも確認 | 長さ上限・LIKE 注入拒否・XSS エスケープは CONTENT-06〜08 で個別に検証。タグの正規化・全文検索は未実装 |
| お気に入り | 追加/削除の冪等（重複追加・未登録削除）、未認証401・不正CID400・非Approved404、他人subからの操作拒否、`favorite_only` の本人分のみ返却（真理値パース・既存条件とのAND・ページング・Approved限定）、旧JSONの `is_favorite` 欠落互換、migration 012 の UNIQUE + FK CASCADE を API・ユニットテストで検証 | `user_hash` をログ・監査・レスポンスへ出さないことは設計方針で担保。実運用ログの確認は未実施 |
| 決済 | 注文ライフサイクル、未対応チェーン拒否、ウォレット署名不一致拒否、派生アドレスの一意性をモックverifierで検証、非UUID注文IDの400拒否、verifyの金額不一致（MISMATCH遷移）・送金再利用・期限切れ・確認数不足（CONFIRMING維持）の拒否、同一送金の確認数進展に伴う再verifyとFINALIZED到達、FINALIZED後の再verifyが状態を戻さないこと、`expires_at` 経過（status が PENDING のまま）の拒否と EXPIRED 更新、BIP44 derivation index の u32 境界拒否、EVM アドレスの大小文字非区別（正規化照合）を検証 | 実チェーン検証、2-of-3クォーラム、実チェーン上の再編成/二重通知の試験は未実施（DEC-03） |
| Support | 501無効化のAPIテスト、送信不可UIと模擬記録注記のE2Eテスト | JPYC決済は未実装、実送金の証明はない |
| 透明性ダッシュボード | `build_mod_stats` の空/同数/定足数未達/月境界、認可（未認証401・未設定/未登録/BAN済み403）、個人情報非含有、実DB集計を `server/src/tests.rs` で検証。E2E は `e2e/tests/moderation.spec.ts` で表示と403を確認（R5-09） | 認証内集計であり第三者公開ではない（DEC-04 未決定）。`quorum_not_met` は `totals` のみで月次表には含めない |

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
| FAV-01 | お気に入りの追加/削除を繰り返す | 重複追加も未登録削除も200で成功し、行数は0または1のまま。削除後の再追加も可 | 実装済み（`test_favorite_add_and_remove_are_idempotent`） |
| FAV-02 | 未認証・不正CID・非Approvedコンテンツへの操作 | それぞれ401 / 400 / 404で拒否し、お気に入りを登録しない | 実装済み（`test_favorite_requires_auth_and_valid_approved_cid`） |
| FAV-03 | 他人のsubを本文に指定した操作 | 増減するのは操作者本人の行のみ。他人の行は変化しない | 実装済み（`test_favorite_uses_claims_sub_only`） |
| FAV-04 | `favorite_only` による絞り込み | 本人分のみを新着順で返す。既存条件とAND、ページング重複なし、非Approved除外、他値は絞り込みなし | 実装済み（`test_favorite_only_lists_own_favorites_only` / `test_favorite_only_parser_and_query_builder`） |
| FAV-05 | 旧JSONとの後方互換 | `is_favorite` 欠落の旧 `ContentMeta` が `false` として読め、現行JSONは往復一致する | 実装済み（`test_content_meta_backward_compatible_without_is_favorite`） |
| FAV-06 | migration 012 の制約 | `UNIQUE(user_hash, cid)` 違反はエラー、`INSERT OR IGNORE` は冪等、FK CASCADE で削除連動 | 実装済み（`test_migration_012_favorites_unique_and_cascade`） |
| MOD-01 | 同一ユーザーの同一通報への再投票 | 拒否 | 実装済み（API検査＋DB一意インデックス）、回帰テストあり |
| MOD-02 | 3人の異なる投票者による結果 | 規則通り状態遷移 | 実装済み（確定後の追加投票は400で拒否）、回帰テストあり |
| MOD-03 | レビュー中コンテンツへの重複通報 | 拒否 | 実装済み、回帰テストあり |
| MOD-04 | `/mod/stats` の集計（`build_mod_stats`） | 通報・確定・BAN/UNBAN/UNPIN の件数を月別に集計する。空入力は0件、同数は `Rejected`、定足数未達は `quorum_not_met` に計上し `approved` から除外、月境界は UTC の `YYYY-MM` で分割 | 実装済み・検証済み（対象テスト全10件 pass）: `test_build_mod_stats_empty` / `test_build_mod_stats_tie_maps_to_rejected` / `test_build_mod_stats_quorum_not_met_excluded_from_approved` / `test_build_mod_stats_month_boundary_utc`。実DBからの集計は `test_mod_stats_aggregates_from_real_db` |
| MOD-05 | `/mod/stats` の認可 | 未認証は401。`MODERATION_OPERATOR_HASHES` 未設定・未登録・BAN済みの主体は403。登録済み運用者は200 | 実装済み・検証済み（対象テスト全10件 pass）: `test_mod_stats_requires_authentication` / `test_mod_stats_forbidden_when_operator_not_configured` / `test_mod_stats_forbidden_when_operator_not_registered` / `test_mod_stats_forbidden_when_operator_banned` |
| MOD-06 | `/mod/stats` のレスポンス | CID・reason・`user_hash` を含めず、`generated_at` / `totals` / `monthly` のホワイトリスト項目のみ返す。`quorum_not_met` は `totals` のみ | 実装済み・検証済み（対象テスト全10件 pass）: `test_mod_stats_response_contains_no_sensitive_fields` |
| MOD-07 | 透明性ダッシュボードのE2E（R5-09） | 運用者では集計が表示され、403では `運用者権限が必要です` が表示される | 実装済み・検証済み: `e2e/tests/moderation.spec.ts` の `Moderation & Governance (GOV-01, GOV-02)` 内、`R5-09: should display moderation stats for operators and permission error on 403`。chromium で3件 pass、フルスイート14件 pass（webkit はホスト側 ICU/libxml2 欠落で未検証） |
| SUPPORT-01 | 無効化済み支援APIへのPOST | 501で拒否（未認証は401）、模擬取引を記録しない | 実装済み、JPYC機能への置換要 |
| ID-01 | JPKI提示、署名、nonce、信頼元、失効、`acr`/`amr` 保証レベル、`auth_time`/`max_age` の検証 | 有効な本人確認だけを受理 | OIDC認証フローをモックOPで検証。`OIDC_REQUIRED_ACR`/`AMR` 設定時に満たさないトークンを401で拒否、`OIDC_MAX_AUTH_AGE_SECS` 設定時に認証要求へ `max_age` を付与し新鮮な `auth_time` を受理・古い値と欠落を401拒否（fail-closed）することを回帰テスト化。実JPKI実機・失効確認は未検証 |
| ID-02 | JPKI提示拒否・失効・期限切れ・再利用 | 保護機能を拒否 | state再利用とnonce不一致を検証。back-channel logout は `events` 欠落と `sid`/`sub` 両方欠落を401拒否。実JPKIの失効・期限切れは未検証 |
| ID-03 | 認証トランザクションとJWT主体の結合 | 別セッション・クライアント申告IDで主体を偽装できない | PPIDをセッションJWTに結合し、`/auth/logout` の `sid` 失効、back-channel logout の `sid`/`sub` 失効（`sub` はインメモリ `iat` 閾値方式、再認証で新セッション発行可）を検証。永続化・実OP連携は未検証 |
| ID-04 | 未認証での閲覧APIアクセス | コンテンツ・監査等を拒否。認証開始・health・非秘密の公開ポリシー情報（`GET /policy/age`）は許可 | JWT拒否実装。OIDC実サービスは未接続 |
| AGE-01 | 年齢ポリシー設定と公開 | 不正な `AGE_*` 設定は起動失敗（fail closed）。`GET /policy/age` は認証不要で有効設定値（非秘密）と `settings_always_public=true` を返す | 実装済み（`common`/`server` テスト）。`GET /policy/age` に秘密情報が含まれないことは設計/レビューで継続確認 |
| AGE-02 | 明示設定必須・信頼属性必須モード | `AGE_REQUIRE_EXPLICIT_CONFIGURATION` 未充足時および `trusted_attribute_required` は拒否（フェイルクローズ） | 実装済み（現行入力は申告値）。検証済み属性入力経路は未実装 |
| PAY-01 | 対応JPYCの有効な送金と最終性確認 | 確認後のみ決済成立 | 基盤実装済み。モックverifierで注文作成・ウォレット結合・検証・確定、BIP44受取アドレス派生を検証。実チェーンRPCは未実装 |
| PAY-02 | 非対応トークン、チェーン、再編成、二重通知 | 不成立または確認中として扱う | 一部実装。未対応チェーン、金額・送金元・受取先の不一致、同一tx再利用を検証。再編成・二重通知・別コントラクトは未実装 |
| PAY-03 | 取引要求ID・金額・受取先との不一致 | 有償機能を解放しない | 基盤実装済み。注文ID・金額・受取先・送金元の照合を検証 |
| AUDIT-01 | 監査記録の改変・欠落・順序入替 | ハッシュ連鎖・連番・Merkle root で改変を検出 | ローカルDBのハッシュ連鎖（v2 正規化＋v1 互換検証）・`log_id` 連続性・Merkle root と `--expect-root` による切詰め検出を実装。エントリハッシュが全内容フィールドに依存することを mutation テストで確認（`log_id` は連続性/Merkle root 側でカバー）。署名付きスナップショット（version 3）がコンテンツ監査と BAN/UNBAN/UNPIN 監査の両方を含み、改ざんで検証失敗することを単体テストと実DBで確認。外部アンカー・HSM 鍵管理は未実装、必須 |
| PERF-01 | 初期表示、API、転送量、Wasmサイズ、手数料 | 合意済みSLO内 | 指標・閾値未決定 |
| COMPAT-01 | 対応OS/ブラウザ/端末で閲覧・投稿・認証・決済 | 対応マトリクスを満たし、安全なフォールバックを表示 | 環境・閾値未決定 |
| COMPAT-02 | JXL/Wasm非対応・機能利用不可環境 | 代替表示または案内。認証・決済は迂回不可 | 方式・環境未決定 |

## 4. 実行コマンド

```bash
cargo test --workspace
cargo test -p server --lib upload   # CONTENT-01（MIME・価格）のAPI回帰のみ
cargo test -p server --lib mod_stats   # R5-09 集計・認可・プライバシーの回帰のみ
cd e2e && npm test
cd e2e && npm test -- search.spec.ts   # R5-01 検索 E2E のみ
cd e2e && npm test -- favorites.spec.ts   # R5-02 お気に入り E2E のみ
cd e2e && npm test -- moderation.spec.ts   # R5-09 透明性ダッシュボード E2E のみ
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

## 5. リリース証跡

リリースごとにテスト結果・ビルド成果物・依存ロックファイルを保存する。Nix再現可能ビルド、オンチェーン記録、バイナリ同一性の証明は別途設計・実装するまでリリース保証として主張しない。
