---
type: Todo
title: "openIllustration プロジェクトTODO"
description: "openIllustration 固有のタスクを管理するプロジェクト内 TODO。複数プロジェクト横断のタスクはグローバルTODOへ置く。"
tags:
  - todo
  - openillustration
  - tasks
generated:
  by: dsh (lead)
  at: "2026-11-18T18:14:23+09:00"
status: current
---

## 進行中

- [x] プロジェクト全体の6パス・クロスレビュー（統合完了:
  [UNKNOWN_TASKS_INVESTIGATION.md](docs/UNKNOWN_TASKS_INVESTIGATION.md) §6.3）
- [x] **OIDC エンドポイントの https 要件を doctor 診断へ反映** — 根拠
  `server/src/bin/doctor.rs:91-100` / `server/src/oidc.rs:249-314` / 発掘 R9 / 中。
  `oidc.rs` と同様に `doctor` も `oidc_endpoint_urls` で5エンドポイントURLの https を検証
  （mock 有効時のみ localhost の http 許容、設定値は非出力）。doctor は54テスト。
- [x] **エクスポート応答の value（機微値）漏洩を精査・是正** — 根拠
  `server/src/lib.rs:4486-4511` / `client/src/lib.rs:811-818` / 発掘 R9 / 中。
  エクスポートのレスポンス値・表示値に機微情報が混ざらないことを確認し是正済み。
  `e2e/tests/real-api.spec.ts` の既知シークレット値比較をキー名へも拡張。
- [x] **価格境界 E2E の「実送信」表現をモック応答と明記へ是正** — 根拠
  `docs/TEST_SPECIFICATION.md:1020` / `e2e/tests/upload.spec.ts:177-204` / 発掘 R9 / 中。
  `/content/upload` は `page.route` + `route.fulfill` のモックであることを仕様側へ明記済み。
  E2E の test 名・定義件数は不変。
- [x] **report/vote 二重送信ガードの E2E を追加** — 根拠
  `client/src/lib.rs:2205-2237` / `e2e/tests/moderation.spec.ts:629-735` / 発掘 R9 / 中。
  実装は `claim_submission` + `BusyGuard` で対応済み。二重クリックで POST が1本だけであることを
  検証する REPORT-GUARD-01 / VOTE-GUARD-01 を追加済み（moderation.spec.ts 全14件）。

## 未着手

### 優先度: 高

- [x] **LOG OUTのrefresh Cookie失効を修正** — 2026-11-22 実装済み（`credentials: Include`）。E2Eはlocalhost/127.0.0.1別ホストのためCookie送信は未検証、同一サイト構成で検証可能な旨を記録。
- [x] **未適用マイグレーションのカーソルを閉じてからBEGIN** — 根拠 `server/src/migration.rs:283` / review R2-ER / high。
- [x] **auth_verify失効閾値削除をDB正本先行へ** — 根拠 `server/src/lib.rs:2936` / review R2-ER / high。
 メモリ先行だとDB失敗時に再起動前後で判定が分岐する。
- [x] **空migrationsディレクトリを拒否** — 根拠 `server/src/migration.rs:146` / review R2-ER / 中。
- [x] **DATABASE_TOKEN非UTF-8をEnvValue分類へ** — 根拠 `server/src/lib.rs:1334` / review R2-ER / 中。
- [x] **bind_walletをtx内でstatus再確認** — 根拠 `server/src/payments.rs:579` / review R2-ER / 中。
- [x] **6 CLIのargvパーサを共通化** — 根拠 `server/src/bin/*` / review R1-AR / 中。
- [x] **cargo deny bansを実効ゲート化** — 根拠 `deny.toml:74` / review R1-AR+R3-TQ / 中。warn+ワイルドカードskipで実質無効。
- [x] **APIオリジンの平文HTTPを本番fail-closed化** — 根拠 `client/src/lib.rs:39` / review R3-AU / 中。
- [x] **Back-Channel Logoutトークンにiat/nbf新鮮性検証** — 根拠 `server/src/oidc.rs:861` / review R3-AU / 中。再送で後続セッションを失効させない。
- [x] **IPFSゲートウェイのhttpを本番拒否** — 根拠 `server/src/lib.rs:1818` / review R3-AU / 低。
- [x] **gpg実暗号化テストの黙スキップを防止** — 根拠 `scripts/test-backup-rotate.sh:690` / discovery R3-TQ / 中。
- [x] **CIクロスブラウザ選定を導入前に修正** — 根拠 `e2e/ci-e2e.sh:315` / discovery R3-TQ / 中。検出がinstall前に走りchromiumへ自己完結フォールバック。
- [x] **依存監査の定期実行とnpm audit** — 根拠 `.github/workflows/ci.yml:10` / discovery R3-TQ / 中。
- [x] **deny.toml先頭コメントを4ゲート実態へ** — 根拠 `deny.toml:1` / review R1-AR / 低。
- [x] **todo.md doctor件数43→53** — 根拠 `todo.md:22` / review R1-AR / 低。
- [x] **OPERATIONAL_POLICYのALLOW_MOCK_AUTH改行損壊を修正** — 根拠 `docs/OPERATIONAL_POLICY.md:20` / review R1-AR / 低。
- [x] **ADR-0001へStatus/Accepted表示を付与** — 根拠 `docs/adr/index.md:3` / review R4 / 低。
- [x] **SC2016除外を行単位へ縮小** — 根拠 `.github/workflows/ci.yml:54` / review R4 / 低。
- [x] **wasm-bindgen-test未使用依存を整理** — 根拠 `client/Cargo.toml:41` / review R1-AR / 低。
- [x] **Runbook/TASK_INVENTORYのmigration件数を履歴明示** — 根拠 `docs/BACKUP_RESTORE_RUNBOOK.md:459` / discovery R5-VU / 低。
- [x] **E2Eエラーパスの行番号コメントをシンボル参照へ** — 根拠 `e2e/tests/error-paths.spec.ts:8` / discovery R5-VU / 低。
- [x] **aria-live件数をエラー時にゲート** — 根拠 `client/src/lib.rs:2168` / discovery R5-VU / 中。
- [x] **ファイル選択のタブ停止を1つへ** — 根拠 `client/src/lib.rs:3086` / discovery R5-VU / 中。
- [x] **warn_history_update_onceをdev-consoleゲートへ** — 根拠 `client/src/lib.rs:1502` / discovery R5-VU / 中。
- [x] **エラー文言の日英混在を統一方針化** — 根拠 `client/src/lib.rs:2328` / discovery R5-VU / 中。

- [x] **AGE_* 非UTF-8をfail-closed化（env分類器へ統一）**
  — 根拠 `common/src/age.rs:110` / `server/src/bin/doctor.rs:440` / review R1-AR+R2-ER / high。
  std::env::var(key).ok() が NotUnicode を None 化し明示設定必須・信頼属性必須を解除し得る。
- [x] **ALLOWED_ORIGINSを本番起動ゲートへ**
  — 根拠 `docs/OPERATIONAL_POLICY.md:333` / `server/src/lib.rs:1268` / review R1-AR / high。
  doctor は必須とするが run() の config_errors に無く、未設定でも起動して CORS/refresh が全拒否になる。
- [x] **本番UIのモック認証表示を整備** — 根拠 `client/src/lib.rs:1774` / `README.md:22` / discovery R5-VU / high。
  「CONNECT WITH JPKI」と表示しつつ mock_token を送信。501時に「本番認証は未実装」文言へ。
- [x] **shfmt -d を実効ゲートへ** — 根拠 `.github/workflows/ci.yml:57` / review R4-OPS / high。
  差分があっても終了コード0で書式ゲートが無効。shfmt -l の出力非空を失敗にする。
- [x] **AUDIT_EXPECT_ROOTの更新ライフサイクルを定義** — 根拠 `docs/BACKUP_RESTORE_RUNBOOK.md:75` / discovery R5-VU / high。
  監査ログ追加以後の root 再取得・世代保存・リストア時選択の手順が無く毎日のバックアップが失敗し得る。
- [x] **prepare-distribution.shの出力先検証を回帰テストへ** — 根拠 `scripts/test-prepare-distribution.sh` / discovery R3-TQ / high。
  '..'成分・repo_root置換・jq/cargo欠落・INT/TERMrollback・旧成果物削除失敗が未検証。

- [x] **リモートDBでJWT失効がメモリに閉じ込められる問題を是正** — 根拠 `server/src/lib.rs` AuthClaims抽出器 / review R1-AR / 重大度 high。
  revoked_sessions/revoked_subs_before がプロセス内のみで、複数インスタンス構成では logout/BAN が他インスタンスへ波及しない。
  DB照会または単一インスタンス強制でfail-closedにする。
- [x] **instance lock読取失敗のfail-closed化** — 根拠 `server/src/lib.rs` acquire_instance_lock / review R2-DI / 重大度 high。
  read_to_stringのErrを空内容として扱い有効ロックを奪取し得る。最初の読取を?伝播し再読込も失敗時bailする。
- [x] **deny のRUSTSEC ignoreを本番リモートDB構成ですり抜けないように** — 根拠 `deny.toml:48` / review R3-DE / 重大度 high。
  h2/rustls系ignoreの理由が「リモートDB接続時のみ」なのに本番推奨構成そのものが到達経路を持つ。
  構成別denyプロファイルまたはリモートDB検出ゲート、あるいは依存更新。
- [x] **アップロード/通報失敗をrole=alertで告知**
  — 根拠 `client/src/lib.rs` upload-status / `docs/OPERATIONAL_POLICY.md:202` / discovery R5-VU / 重大度 high。
  成功はrole=statusのまま、失敗時のみrole=alert専用領域へ。E2E既存2件内へアサーション、69件維持。
- [x] **maxlengthをUTF-8バイト基準へ**
  — 根拠 `client/src/lib.rs` REPORT_REASON_MAX_LEN 等 / `server/src/lib.rs` validate_content_metadata / discovery R5-VU
    重大度 high。
  ブラウザmaxlengthはUTF-16 code unitsでサーバはUTF-8バイト。日本語67文字超で400になる。
  TextEncoderでバイト数ゲートするかサーバをUnicode文字数へ。E2E既存2件内へ日本語ケース、69件維持。
- [x] **定期バックアップとinstance.lockの衝突を解消**
  — 根拠 `deploy/openillustration-backup.timer` / `scripts/backup-rotate.sh` / review R5-DR / 重大度 high。
  常時稼働環境では03:17の実行が必ず失敗する。停止→backup→起動のオーケストレーションまたはオンライン快照方式へ。
- [x] **検証済み年齢属性の user_hash を signed_subject 由来へ変更**
  — 根拠 `server/src/lib.rs` auth_verify (verified_age 分岐) / review R3-AU / 重大度 high。
  verified_claims 受理時に `sha256(birthdate:country)` を user_hash としており、同一生年月日・居住国の
  ユーザーが subject 衝突する。実証明書接続時に本番の主体分離が崩れるため、署名済み subject を
  主体識別子とし、birthdate/country は年齢判定専用に留める。
- [x] **非 UTF-8 環境変数を fail-closed 起動拒否へ**
  — 根拠 `server/src/lib.rs` bool_env_at_startup / `server/src/oidc.rs` validate_oidc_config_with / review R2-ER
    重大度 high。
  `std::env::var` の Err を全て「未設定」として飲み込み、NotUnicode が起動拒否をすり抜ける。
  `var_os` で None と非 UTF-8 を区別し、doctor/per-request 読込も同じ lookup 契約へ統一する（値は非エコー）。
- [x] **DATABASE_URL のサーバー既定をfail-closed化しdoctor診断を追加**
  — 根拠 `server/src/lib.rs` run() / `server/src/bin/doctor.rs` production / review R1-CF / 重大度 high。
  CLI3本は未設定起動拒否なのにサーバーは `file:local.db` へ暗黙フォールバックし、doctor も診断しない。
  本番相当では未設定起動拒否、または既定値使用を production カテゴリへ警告追加。
- [x] **cron 配布テンプレートの MAILTO をプレースホルダ化** — 根拠 `deploy/openillustration-backup.cron:5` / review R4-DR / 重大度 high。
  `admin@example.com` が残っており、そのまま導入するとバックアップ失敗通知が届かない。
  `__SET_ME__` 等へ変更し、Runbook に差し替え手順と ALERT_CMD 必須化を追記。
- [x] **JWT 期限切れ (401) の自動 refresh を有効化（基盤は実装済み・既定無効）**
  2026-11-22: 共有トークン signal・api_fetch/api_fetch_request（再試行・単一フライト）・
  CORS allow_credentials を実装済み。当初 `ENABLE_AUTO_REFRESH=false` で段階導入していたが、
  呼び出し側のエラー表示を refresh 非依存へ移行し、2026-11-22 に `true` へ反転済み
  （現行コード・TEST_SPECIFICATION と整合）。履歴として記録する。
  — 根拠 `client/src/lib.rs` fetch 系ハンドラ群 / `docs/UNKNOWN_TASKS_INVESTIGATION.md` / discovery R5-VU / 重大度 high。
  `/auth/refresh` がクライアントに存在せず、期限切れ後は全ビューが固定エラーで固まる。
  共通 fetch ラッパーで401を1回 refresh 再試行する基盤を追加。失敗時は元の401を返し
  各ビューの固定文言で案内する契約（有効化時に画面遷移の要否を再設計）。E2E69件維持。

P0（本番公開前に解消すべきセキュリティ／データ完全性）:

- [x] **JPKI 証明書の暗号学的自己署名検証を追加（DN 迂回の遮断）** — 根拠 `server/src/jpki.rs:98-108` / R3-AU02 / 重大度 high。
  2026-11-21 対応: `x509-parser` の `verify` feature を有効化し `cert.verify_signature(None).is_ok()` を
  DN 比較と OR で判定。issuer 名を変えた同一鍵署名リーフを拒否する回帰テストを追加。
  信頼アンカー照合・チェーン構築・OCSP/CRL は外部要因（ADR-0002）のため未実装のまま。
- [x] **[完了:DEC-01 adapter 配線]** **auth_verify へ年齢属性の署名済み入力と共通ゲートを結線**
  — 根拠 `server/src/lib.rs:2376-2384,4416-4444` / R3-AU03 / 重大度 high。
  クライアント送信の `birth_date`/`country_code` を生のまま判定しており改ざんで閾値を回避できる。
  検証済みクレーム由来のみ受け付け、JPKI 経路でも `ensure_age_policy_configured` を呼ぶ（DEC-01 の入力経路確定と simultaneous）。
- [x] **リフレッシュトークンの BAN 判定順序は現状維持（対応不要と裁定）** — 根拠 `server/src/oidc.rs:1127-1175` / R2-ER01 / 重大度 high。
  2026-11-21 吟味の結果却下: BAN 判定を消費前に移動する実装を試したが、実環境の `ban_user()` は
  `revoked_subs` 記録＋未使用トークン消費を先に行うため refresh 側の BAN 判定には到達せず、
  既存の 401（再利用検知／失効 sub）が fail-closed に働く。BAN ユーザーは延長できず、
  DB 一時障害時も `ensure_user_not_banned` の Err が UPDATE 前に伝播する点も R4 が確認。
  既存の BAN 失効テスト期待値（トークン消費）と整合させるため差分は差し戻し。
- [x] **ban_user / backchannel_logout の失効を DB コミット先行へ統一** — 根拠
  `server/src/lib.rs:3496-3522` / `server/src/oidc.rs:1235-1261` / R2-ER02〜03 / 重大度 high。
  2026-11-21 対応: ban_user は revoked_subs UPSERT を users 更新と同じ tx へ移動し、
  backchannel_logout の sub 分岐は DB UPSERT を refresh 消費・メモリ更新より前にコミット。
  revoke_session（DB先・メモリ後）は既に安全なため無変更。単一tx化は write_lock 自己
  デッドロック回避のため見送り、再起動窓の回帰2テストを追加。
- [x] **監査スナップショットのマスク判定を完全一致へ** — 根拠 `common/src/lib.rs:400-415` / R2-IV04 / 重大度 high。
  2026-11-21 対応: `reason_is_masked` を `^\[redacted:\d+ bytes\]$` の正規形完全一致へ変更（空文字は維持）。
  平文が `[redacted:` で始まるだけでマスク済みと誤判定される経路を塞ぎ、回帰テスト5件を追加。
  `mask_reason` の出力形式と verify_chain / sign_audit_snapshot の契約は不変。
- [x] **[完了:AGE 設定充足導出]** **AGE_POLICY_CONFIGURED を実際の設定充足から導出**
  — 根拠 `common/src/age.rs:63,163,334` / R1-CF03 / 重大度 high。
  手動フラグ 1 だけで既定値（21歳・target_regions 空）のまま `AGE_REQUIRE_EXPLICIT_CONFIGURATION` を通過できる。
  2026-11-21 部分対応: doctor の age_policy カテゴリへフラグ単独＋既定値の固定警告を追加（check23不変）。
  フラグを充足結果へ導出すると /policy/age の公開値とゲート契約が変わるため後方互換のため見送り、未着手を維持。
- [x] **[完了:DATABASE_URL 未設定時起動拒否]** **DATABASE_URL の既定値を CLI と deploy 設定例で統一** — 根拠 `server/src/bin/backup_db.rs:202` /
  `deploy/openillustration-backup.cron:20` / R1-CF01 / 重大度 high。
  CLI 既定 `file:local.db` と deploy の `/var/lib/openillustration/local.db` が別 DB を指し、
  2026-11-21 部分対応: CLI コメントと OPERATIONAL_POLICY へ差異と明示設定の案内を追記（既定値は後方互換のため不変）。
  既定値の統一と doctor 診断対象への追加は未着手を維持。
- [x] **README の --database-token 拒否を実契約へ修正** — 根拠 `README.md:111-117` / R3-FG03 / 重大度 high。
  2026-11-21 対応: 6 運用 CLI（backup_db / restore_db / sign_audit_export / verify_audit_log /
  verify_audit_export / verify_backup）が拒否し doctor は別契約である旨を明記。各 CLI の
  DATABASE_TOKEN 利用有無（3 CLI のみ環境変数を使用）も記載。
- [x] **投票ボタンを「REJECT (VOTE)」へ変更** — 根拠 `client/src/lib.rs:2615` /
  `e2e/tests/moderation.spec.ts:73` / R1-UX02 / 重大度 high。
  2026-11-21 対応: ADR-0004 の「7日後確定・自動BANなし」に合わせラベルと E2E アサーションを更新。
  E2E 件数69件は不変。

- [x] 年齢ゲートの OIDC バイパスを遮断（R1-17、2026-11-18 未コミット対応）: callback / refresh が
  共通ゲート `ensure_age_policy_configured` を state / Cookie の消費前に呼び、
  `TrustedAttributeRequired` と `ConfigurationRequired`（実効値から導出した
  `configuration_satisfied()` が false）を 403 にする。
  AGE-03 のサーバー側テスト1件（AGE-02 の旧ゲートは 2026-11-17 対応済み）
- [x] 年齢判定の基準日を `TZ` ローカル日付へ（2026-11-18 対応済み・未コミット、AGE-04）:
  `validate_age` が `TZ` を読み、`Asia/Tokyo`（UTC+9）/`Etc/GMT±N`（POSIX 符号反転）を解釈、
  未設定・未対応名は UTC へフォールバック。JST 深夜境界（`2003-01-02` 生まれが JST で21歳・UTC で20歳）
  を含むサーバー側テスト2件
- [x] 信頼できる年齢属性の実入力経路（外部 OP/JPKI のクレーム仕様確定後: DEC-01）
- [x] 通報中・却下コンテンツの直接 CID 取得を 404 化（R2-09、2026-11-17 対応済み）
- [x] refresh Cookie の `Path` を `/auth` へ広げ、logout 時のファミリ失効を有効化（R1-18、2026-11-17 対応済み）
- [x] ウォレット bind を DB に記録し verify の必須条件にする（R1-24、2026-11-17 対応済み）
- [x] 本番起動時に OIDC 必須設定と `PINATA_JWT` を検査し fail-closed 化（R4-01 / R4-04、2026-11-17 対応済み）
- [x] アップロード UI の file パートへ MIME を設定（R1-09、2026-11-17 対応済み）
- [x] Tailwind をビルド時 CSS 化し、外部 CDN/フォント依存と無 SRI を解消（R1-10、2026-11-17 対応済み）

### 優先度: 中

- [x] **投票一覧取得失敗時に古い通報を表示し投票ボタンを有効にしない** — 根拠 `client/src/lib.rs:3529` / discovery R5-VU / 中。
- [x] **監査タイムラインを再オープン時に再取得** — 根拠 `client/src/lib.rs:2597` / discovery R5-VU / 中。
- [x] **アップロード送信中のファイル差し替えを防止** — 根拠 `client/src/lib.rs:3056` / discovery R5-VU / 中。
- [x] **TOCアンカー後のgoBackで直前viewへ戻る** — 根拠 `client/src/lib.rs:1686` / discovery R5-VU / 中。
- [x] **logout API失敗をrole=alertで通知** — 根拠 `client/src/lib.rs:1753` / discovery R5-VU / 中。
- [x] **集計GENERATED_ATをformat_local_datetimeで整形** — 根拠 `client/src/lib.rs:736` / discovery R5-VU / 低。
- [x] **export object URL revokeを保存開始に結び付ける** — 根拠 `client/src/lib.rs:1514` / discovery R5-VU / 低。
- [x] **exportボタンのaria-labelをbusy状態へ同期** — 根拠 `client/src/lib.rs:2395` / discovery R5-VU / 低。
- [x] **READMEへCI E2Eゲートを追記** — 根拠 `README.md:76` / discovery R5-VU / 低。
- [x] **CONTENT-01 10MiBちょうどの受理テスト** — 根拠 `server/src/tests.rs:4106` / discovery R3-TQ / 中。
- [x] **CONTENT-06/MOD-08 送信前ゲートHTTP0本のE2E検証** — 根拠 `e2e/tests/upload.spec.ts` / discovery R3-TQ / 中。
- [x] **/auth/verify 501→固定文言のE2E検証** — 根拠 `e2e/tests/auth.spec.ts` / discovery R3-TQ / 中。
- [x] **tokio-test未使用依存を削除** — 根拠 `server/Cargo.toml:51` / discovery R3-TQ / 低。
- [x] **finalizer部分コミットのprocessedメトリクス過小計上を是正** — 根拠 `server/src/lib.rs:2099` / review R2-ER / 中。
- [x] **readiness 503をgraceful shutdown開始前に設定** — 根拠 `server/src/lib.rs:1692` / review R2-ER / 中。
- [x] **JWT/IPFS/決済のEnvValue取りこぼしを解消** — 根拠 `server/src/lib.rs:1505` / review R2-ER / 中。
- [x] **RSA検証鍵のkidと公開鍵の整合を検証** — 根拠 `server/src/jwt.rs:104` / review R2-ER / 低。
- [x] **UNIQUE違反判定のメッセージ文字列依存を排除** — 根拠 `server/src/payments.rs:672` / review R2-ER / 低。
- [x] **auth.spec.ts refreshモックをVerifyResponse契約へ** — 根拠 `e2e/tests/auth.spec.ts:84` / review R1-AR / 低（前ラウンド対応済み・完了扱いで記録）。
- [x] **doctor OIDC_COOKIE_SECURE警告文からsession Cookie言及を削除** — 根拠 `server/src/bin/doctor.rs:414` / review R1-AR / 低。
- [x] **E2E失敗側refreshモックを共有ヘルパへ** — 根拠 `e2e/tests/*.spec.ts` / review R1-AR / 低。

- [x] **auth_verifyのBANチェックを原子的化** — 根拠 `server/src/lib.rs:3015` / review R2-ER / 中。
  BANチェックとiat採番が別時点で、同時BANを新規JWTがすり抜ける。
- [x] **CI shellcheck契約記述を統一** — 根拠 `.github/workflows/ci.yml:65` / review R4 / 中。
- [x] **REVIEW_REPORTへ2026-11-22レビュー履歴を追記** — 根拠 `docs/REVIEW_REPORT.md:580` / review R4 / 中。
- [x] **バックアップ失敗通知の実設定手順を必須化** — 根拠 `docs/BACKUP_RESTORE_RUNBOOK.md:490` / review R4 / 中。
- [x] **TRUSTED_PROXY_CIDRSをread_env分類へ** — 根拠 `server/src/network.rs:98` / review R1-AR / 低。値エコーも廃止。
- [x] **TZをread_env単一情報源化** — 根拠 `server/src/lib.rs:2921` / review R1-AR / 低。
- [x] **真偽値env→Result変換をcommonへ集約** — 根拠 `common/src/env.rs` / review R1-AR / 低。3箇所の重複。
- [x] **監査reasons_maskedの不変条件を検証側でも強制** — 根拠 `common/src/lib.rs:677` / review R2-ER / 低。
- [x] **sign_audit_exportをatomic publishへ** — 根拠 `server/src/bin/sign_audit_export.rs:137` / review R2-ER / 低。
- [x] **client/jxl.rsの3テストをdoc-numbers照合へ** — 根拠 `scripts/test-doc-numbers.sh:579` / discovery R3-TQ / 中。
- [x] **E2E test()抽出をコメント除外へ堅牢化** — 根拠 `scripts/test-doc-numbers.sh:30` / discovery R3-TQ / 低。
- [x] **UNKNOWN_TASKS R1-05の古い根拠を更新** — 根拠 `docs/UNKNOWN_TASKS_INVESTIGATION.md:57` / review R4 / 低（対応済み・完了扱いで記録）。
- [x] **client/Cargo.tomlのuuidコメント訂正** — 根拠 `client/Cargo.toml:35` / review R1-AR / 低（対応済み・完了扱いで記録）。

- [x] **リフレッシュトークン回転をアトミック化** — 根拠 `server/src/oidc.rs:1384` / review R2-ER / 中。
  消費コミット後にbanチェック・発行が失敗すると旧トークン消費済み・新規未発行でファミリ失効する。
- [x] **OIDC_MAX_AUTH_AGE_SECSに上限を設定** — 根拠 `server/src/oidc.rs:42` / review R2-ER / 中。
  u64::MAX等が受理されauth_time検証が実質無効化されるfail-open。
- [x] **verify_order MISMATCH更新のCAS影響件数 checking** — 根拠 `server/src/payments.rs:736` / review R2-ER / 中。
- [x] **doctor FK検査の未設定/非UTF-8時スキップ** — 根拠 `server/src/lib.rs:1194` / review R1-AR+R2-ER / 中。
  file:local.dbへフォールバックし無関係DBを「production」として報告する。
- [x] **PINATA判定を真の単一情報源へ** — 根拠 `server/src/lib.rs:980` / review R1-AR / 中。
  production_config_errorsとpinata_jwt_production_likeが同一比較を再実装している。
- [x] **instance.lock/resolve_database_url判定をcommonへ集約** — 根拠 `server/src/bin/backup_db.rs:118` / review R1-AR / 中。
  3CLIで約40行ずつ複製されfail-closed判定がdriftし得る。
- [x] **session Cookie未使用の露出を解消** — 根拠 `server/src/oidc.rs:1284` / review R3-AU / 中。
  Path=/ SameSite=Laxで全リクエストに送られ logout時も削除されない。
- [x] **/metrics/client-eventsのcount改ざん防止** — 根拠 `server/src/lib.rs:429` / review R3-AU / 中。
  匿名でcount=1000まで加算できメトリクスを押し上げられる。
- [x] **通報失敗時に進捗文言をクリア** — 根拠 `client/src/lib.rs:3525` / discovery R5-VU / 中。
- [x] **小説本文の不正UTF-8をDECODE FAILEDへ** — 根拠 `client/src/lib.rs:2514` / discovery R5-VU / 中。
- [x] **/content 401に再ログイン導線を追加** — 根拠 `client/src/lib.rs:2000` / discovery R5-VU / 中。
- [x] **公開集計の解析失敗をStatsFailure::Parseへ** — 根拠 `client/src/lib.rs:3414` / discovery R5-VU / 低。
- [x] **暗号化バックアップの復元パスを統一** — 根拠 `docs/BACKUP_RESTORE_RUNBOOK.md:155` / discovery R5-VU / 中。
- [x] **cron版にオーケストレーション完成例を同梱** — 根拠 `deploy/openillustration-backup.cron:24` / discovery R5-VU / 中。
- [x] **SC2034除外を関数単位へ限定** — 根拠 `.github/workflows/ci.yml:54` / review R4-OPS / 低。
- [x] **README/REVIEW_REPORTのADR状態記述を是正** — 根拠 `README.md:26` / `docs/REVIEW_REPORT.md:507` / discovery+review / 低。
- [x] **旧プロジェクト名とconsole.logを整理** — 根拠 `server/src/lib.rs:1` / `client/src/lib.rs:2233` / review+discovery / 低。
- [x] **lang=jaと英語ラベルの不一致を解消** — 根拠 `client/index.html:2` / discovery R5-VU / 低。
- [x] **全文検索/sha3保留/iss-aud要件を棚卸しへ追記**
  — 根拠 `docs/DETAILED_DESIGN.md:587` / `docs/HANDOFF.md:341` / `todo.md:872` / discovery / 低。
- [x] **request_idと4xx本文のオラクルを仕様へ** — 根拠 `docs/TEST_SPECIFICATION.md` / discovery R3-TQ / 低。

- [x] **AUDIT_EXPECT_ROOTをサーバ起動ゲートへ**
  — 根拠 `server/src/bin/doctor.rs` / `server/src/lib.rs` production_config_errors / review R1-CF / 中。
  doctor --strict独自必須で起動は未設定のまま通る。hex検証をproduction_config_errorsへ追加するか「推奨」へ格下げ。
- [x] **全bool_env検証をDB接触前に集約** — 根拠 `server/src/lib.rs:1277` JWT_REQUIRE_KID / review R2-ER / 中。
  JWT_REQUIRE_KIDだけがmigrate/lock/FK検査後に検証され、不正値で部分適用状態を残す。
- [x] **公開集計のSELECTを読み取りトランザクション化** — 根拠 `server/src/lib.rs` public_mod_stats / review R2-DI / 中。
  3クエリが別トランザクションで並行書き込みとスナップショット不一貫。me_exportと境界を統一。
- [x] **TZ判定・PINATA_JWT分類の単一情報源化**
  — 根拠 `server/src/bin/doctor.rs` policy_fixed_offset再実装 / `server/src/lib.rs` production_config_errors / review R1-CF
    中。
- [x] **/auth/refreshのOrigin検証とnullオリジン拒否**
  — 根拠 `server/src/oidc.rs` refresh / `server/src/lib.rs` is_origin_shape / review R3-AU / 中。
  credentialed CORSと組み合わせ別オリジンからJWT再発行の余地。Sec-Fetch-Site/Origin照合、null明示拒否。
- [x] **XFFデコード不能ヘッダのfail-closed化とsafety copy権限**
  — 根拠 `server/src/network.rs` forwarded_client_ip / `server/src/bin/restore_db.rs` / review R2-ER + R5-VU / 中。
- [x] **systemd/cronバックアップの排他と通知経路**
  — 根拠 `docs/BACKUP_RESTORE_RUNBOOK.md` / `deploy/openillustration-backup.service` / review R5-VU + R5-DR / 中。
- [x] **sha256便宜crate削除とwasm-bindgen-test整理** — 根拠 `server/Cargo.toml:29` / `client/Cargo.toml` / discovery R4-DP / 中。
  sha2+hexへ置換し重複バージョンskipを外す。未使用のwasm-bindgen-testは削除または実ゲート追加。
- [x] **doc-numbersへクライアント単体テスト名とJPKI件数照合を追加** — 根拠 `scripts/test-doc-numbers.sh` / discovery R4-DP / 中。
- [x] **CIへシェル静的ゲート(bash -n/shellcheck/shfmt)を追加** — 根拠 `.github/workflows/ci.yml:47` / review R5-CI / 中。
- [x] **脆弱性受容件数11/12不一致とADR状態記述を是正**
  — 根拠 `docs/SECURITY_ADVISORIES.md:22` / `docs/DOCUMENT_STATUS.md:39` / `README.md:26` / review R5-DC + discovery R5
    中。
- [x] **/mod/reports と finalize へ運用者認可を追加** — 根拠 `server/src/lib.rs` list_reports / finalize_report / review R3-AU / 中。
  任意の認証済みユーザーが通報キュー閲覧と確定を実行でき、運用判断を代行できる。
  `require_moderation_operator` を ban_user と同じ位置で呼ぶ（スケジューラ専用なら別トークン分離）。
- [x] **doctor が OIDC 空値ハードエラーを警告へ流す** — 根拠 `server/src/bin/doctor.rs` oidc_* カテゴリ / review R2-ER / 中。
  hard_errors のうち OIDC_EMPTY_VALUE_ERROR 等が警告化されず、起動は拒否されるのに --strict が exit 0 になる。
- [x] **URL 同期の履歴失敗・冪等性を堅牢化** — 根拠 `client/src/lib.rs` navigate() / current_content_query() / review R2-DI / 中。
  履歴 API の Result を破棄し set_view を先に行うため URL と画面が不整合になり得る。
  成功後に set_view、popstate ガード、生 search の双方向一致比較で正規化へ。
- [x] **ADR-0008 段階2 (公開集計ルート) を実装** — 根拠 `docs/adr/ADR-0008...` / `server/src/lib.rs` ルート表 / discovery R1-FG / 中。
  Status Accepted の公開範囲（件数・月次分布・generated_at、機微除外）が未実装。
  未認証401・認証済み200・機微キー非含有をサーバー/E2E で固定（69件維持）。
- [x] **外部キー孤立行の fail-closed 方針を決定** — 根拠 `server/src/lib.rs` find_foreign_key_violations / discovery R1-OPS / 中。
  warn のみで起動継続。本番相当でエラー化するか現状維持を裁定し、コードコメントと運用方針へ明記。
- [x] **Runbook のテスト数と export E2E 記述を実測へ**
  — 根拠 `docs/BACKUP_RESTORE_RUNBOOK.md`, `docs/AUDIT_EXPORT_RUNBOOK.md`, `docs/TEST_SPECIFICATION.md` / review R5-DC
    中。
  verify_backup 17→19、verify_audit_export 17→20、export E2E「2/2 pass」→3件。doc-numbers 照合対象外。
- [x] **CI の deny ゲートへ bans/sources を追加**
  — 根拠 `.github/workflows/ci.yml:40-42`, `deny.toml` / review R3-DE + discovery R4-DP / 中。
  licenses/advisories のみで重複バージョン・未知レジストリを検出できていない。
- [x] **E2E ブラウザフォールバックを可視化** — 根拠 `e2e/ci-e2e.sh` 選択ロジック / discovery R4-OPS / 中。
  firefox/webkit 不可時の chromium フォールバックが stderr 警告のみで、1ブラウザ green が見えない。
  ::warning:: アノテーションとジョブサマリへ選択プロジェクトを出力。
- [x] **性能/互換ゲート PERF-01/COMPAT-01/COMPAT-02 の測定条件を確定** — 根拠 `docs/TEST_SPECIFICATION.md:159-161` / discovery R4 / 中。
  閾値・環境未決定のまま名目ゲート。E2E69件を増やさず既存 spec 内へ計測を統合。
- [x] **doc-numbers の照合対象を拡張** — 根拠 `scripts/test-doc-numbers.sh` / review R1-CF + discovery R4-OPS / 中。
  Runbook・todo.md・CLI (Test_SPEC/DETAILED_DESIGN 以外) とクライアント単体テスト名が対象外。
  E2E カウントの `^  test(` インデント依存も `test(` 全般＋describe 除外へ。

P1（本番運用の健全性）:

- [x] **logout の refresh Cookie 削除を OidcConfig.cookie_secure に統一** — 根拠 `server/src/oidc.rs:133-144` / R1-AP06 / 中。
  2026-11-21 対応: `delete_refresh_cookie(secure)` を追加し、発行側と同じ `config.cookie_secure`
  （OIDC 未構成時は fail-closed で true）を共有。env 直接再読込を削除、回帰2テストを追加。
- [x] **全フラグの真偽値解釈を共通 parse_bool_env へ統一** — 根拠 `common/src/age.rs:212-221` /
  `server/src/lib.rs:1424-1428` / `server/src/bin/doctor.rs:157` / R1-AR03+R1-CF05 / 中。
  AGE_* は未定義値を起動拒否する一方 JWT_REQUIRE_KID / OIDC_COOKIE_SECURE / ALLOW_IPV4_LOCALHOST は「0/false 以外」を真とし、
  doctor は pub ヘルパを使わず再実装。不正値は fail-closed、doctor とランタイムで同一関数を呼ぶ。
- [x] **get_content の chunk() エラーでもゲートウェイフェイルオーバー** — 根拠 `server/src/lib.rs:2783-2806` / R2-ER05 / 中。
  2026-11-21 対応: chunk() エラーを `describe_reqwest_error` で分類記録し次のゲートウェイへ進む。
  全失敗時 fail-closed・checksum/サイズ契約は不変。回帰2テストを追加。
- [x] **refresh/logout の refresh_token Cookie 重複を拒否** — 根拠 `server/src/oidc.rs:212-242` / R2-IV06 / 中。
  2026-11-21 対応: `refresh_cookie_from_headers` を state Cookie と同じ基準で追加し refresh/logout の双方で利用。
  複数出現時は 401／ファミリ失効スキップ。回帰2テストを追加。
- [x] **amount_wei を u128 数値比較へ** — 根拠 `server/src/payments.rs:166-174,722-730` / R2-IV08 / 中。
  2026-11-21 対応: `parse_amount_wei` で両側 u128 パース比較。非数値/オーバーフローは Internal fail-closed。
  先頭ゼロ受理・非数値拒否・u128 超過の回帰を追加。DB 保存形式と既存 MISMATCH 400 は不変。
- [x] **BIP44 derivation_index の non-hardened 上限を明示エラーへ** — 根拠 `server/src/payments.rs:200-211` / R2-BV07 / 中。
  2026-11-21 対応: `derivation_index_to_u32` を `0..2^31` 範囲チェックへ変更。上限到達前に Internal で
  拒否し、注文がすべて 500 になる問題を解消。境界回帰テストを追加。
- [x] **doctor へバックアップ必須設定と TZ の診断を追加** — 根拠 `server/src/bin/doctor.rs:163-189,459-486` / R1-CF02,CF07 / 中。
  2026-11-21 対応: production カテゴリへバックアップ3条件（recipient/OFFSITE片方/ALERT_CMD実行不可）、
  age_policy へ TZ 未設定と地域オフセット不整合の固定文言警告を追加（check23不変）。
  4変数は backup-rotate.sh と同じ生値判定（空白のみも設定あり）に統一。回帰4テストを追加。
- [x] **--include-reasons へ本番ガードと break-glass を追加** — 根拠 `server/src/bin/sign_audit_export.rs:139-170` / R1-CF08 / 中。
  2026-11-21 対応: 本番相当では `AUDIT_EXPORT_INCLUDE_REASONS=1` の明示オプトインが無ければ拒否。
  `ALLOW_MOCK_AUTH=1` は認証を無効化するため逃げ道に使わない設計。回帰2テストを追加。
- [x] **OIDC 設定バリデータを doctor と run() の共通 API へ** — 根拠 `server/src/oidc.rs:287-340` /
  `server/src/bin/doctor.rs:84-94` / R1-AR07 / 中。
  from_env_with が pub(crate) のため doctor が必須変数・URL・cookie_secure を再実装し、ルール追加時にドリフトする。
- [x] **検索条件と SPA ビューを URL / history へ同期** — 根拠 `client/src/lib.rs:116-138,895-1002` / R1-UX03,UX04 / 中。
  リロード・共有で条件が消え、ブラウザ「戻る」が画面遷移と連動しない。hash ベースでも双方向同期＋popstate。
- [x] **マイグレーション SQL 分割器を quote 対応へ** — 根拠 `server/src/migration.rs:204-271` / R1-ST04 / 中。
  2026-11-21 対応: `split_statements` を新設し single/double quote と行コメントを認識。
  クォート内の `;` / `--` を保持、チェックサムと既存 migration 001-013 は不変。回帰2テストを追加。

- [x] 認証・アップロード・注文作成へレート制限と同時実行上限を導入（R4-02、2026-11-18 対応済み）
- [x] OIDC HTTP タイムアウト設定キーを追加（R4-03、2026-11-18 対応済み）
- [x] `/auth/login` を `ALLOW_MOCK_AUTH` でゲートし設計書と実装を整合（C2 / R1-21、2026-11-17 対応済み）
- [x] `verify_order` へ CAS とトランザクションを導入し確定後不変を担保（R1-25、2026-11-18 対応済み）
- [x] メトリクスカウンタを差分アサーション＋注入可能設計へ変更（R3-01、2026-11-18 対応済み）
- [x] `observe_metrics` を `Router` 最外側へ移動し 429 / IPv4 403 / 413 /
  CORS プリフライト OPTIONS 204 を計上（2026-11-18 対応済み・未コミット、
  MET-02 サーバー側テスト4件）
- [x] 決済 bind 署名を v2 化（order_id / chain_id / wallet_address /
  recipient_address / amount_jpy）し、注文IDのみの旧形式署名を401拒否
  （盲署名対策、2026-11-18 対応済み・未コミット、PAY-05）
- [x] 監査 CLI（`sign_audit_export`）を硬化: 鍵権限 0600/0400・親 0700、
  `DATABASE_TOKEN` 環境変数限定（`--database-token` 廃止）、出力 `create_new` 上書き拒否
  （2026-11-18 対応済み・未コミット、AUDIT-02。bin 内に9テスト追加済み）
- [x] 3CLIへ自動テスト追加と契約の明文化（2026-11-18 対応済み・未コミット）:
  `doctor` の `parse_args_from` / `check_all_with` / `should_fail` を純粋化し24テスト、
  `verify_audit_export` に `--expect-root` と `VerificationOutcome` / `verify_export` の
  純粋化で17テスト、`verify_backup` は `--database-token` を argv で拒否（
  `DATABASE_TOKEN` 環境変数のみ）＋sidecar fail-closed で17テスト。
  全て `server/src/bin/` のサーバー側 bin テストであり E2E は40件のまま（CLI-01〜03）
- [x] SSRF リダイレクト無効化と XFF/IPv4-compatible 形式の hardened 解釈
  （2026-11-18 対応済み・未コミット、SSRF 2件 + XFF 11件のサーバー側テスト、
  `Policy::none()` で 3xx は追従せずフェイルオーバー、NET-05 / NET-06）
- [x] `doctor --strict` の警告対象へ `ALLOW_IPV4_LOCALHOST` / `TRUSTED_PROXY_CIDRS` を追加
  （2026-11-18 対応済み・未コミット、11→13カテゴリ・20→24テスト、CLI-01 / CFG-01）
- [x] `/me/export` の12表読み取りを単一の読み取りトランザクション化（2026-11-18 対応済み・未コミット、
   EXPORT-09 / EXPORT-10）: `BEGIN DEFERRED` で包み、`debug_assert!(!db.is_autocommit())` と明示
   `tx.commit()` を追加。別接続の未コミット BAN 書き込み下で書き込み前の単一時点スナップショットを
   返す検証を含むサーバー側テスト2件。E2E は40件のまま
- [x] 監査タイムラインの理由マスク（2026-11-18 対応済み・未コミット、AUDIT-03）:
   `get_content_audit` が `reason` に `summarize_reason` を適用し、ハッシュ検証用の `entry_hash`
   は DB の生値を維持。DB 全文も不変。サーバー側テスト1件
- [x] 投票署名を v2 化（2026-11-18 対応済み・未コミット、MOD-12）:
   `moderation_vote_message_v2` が `domain: "moderation-vote-v2"` / `report_id` / `result` /
   `juror_hash` を署名対象とし、`mod_vote` は v2 のみ受理。旧形式署名と別投票者向け署名の
   転用は401拒否。ADR-0004 の決定事項として追記、サーバー側テスト4件
- [x] 主要画面に実サーバー認証を使うスモーク E2E を1本追加（C1、2026-11-17 対応済み）

### 優先度: 低

- [x] **k-匿名性の要否を裁定 — 却下（2026-11-22）** — 根拠 `todo.md:709` / discovery R5-VU / 保留(speculative)。
  公開集計の小セル推論（review R3-AU medium）と同一課題。ADR-0008と照らして採用/却下を決定。
  裁定メモ: 未リリースで小規模デプロイの想定はない。ADR-0008 の公開範囲はホワイトリストの集計値のみで、
  個別情報（CID / `user_hash` / 理由本文・要約 / 個別行）は既に除外済み。必要になった場合は Level 1（受入）で再評価。

- [x] **E2E解析失敗フェーズを既存2件へ統合** — 根拠 `e2e/tests/export.spec.ts` / `e2e/tests/moderation.spec.ts` / discovery R4 / 低。
  EXPORT-11(不正JSON)とREPORTS-ERR(型壊れJSON)を既存test内へ。69件維持。
- [x] **ADR-0011の3層計測と実装の差分を解消** — 根拠 `docs/adr/ADR-0011` / `e2e/tests/metrics.spec.ts` / discovery R5 / 低。
  層3(転送量/wasmサイズ)と3回中央値を追加するか、Level 0は部分計測と明記。
- [x] **ADR-0012の互換マトリクス実体を作成** — 根拠 `docs/adr/ADR-0012` / discovery R5 / 低。
- [x] **監査Reasonの「全文は署名付きエクスポート」案内を到達可能な表現へ** — 根拠 `client/src/lib.rs:2708` / discovery R5 / 低。
- [x] **doc-numbersのコメント「実害なし」根拠を実態へ** — 根拠 `scripts/test-doc-numbers.sh:36` / review R5 / 低。
- [x] **UI 文言を日本語へ統一（日英混在解消）** — 根拠 `client/src/lib.rs` alert/status/sr-only 文言 / discovery R5-VU / 中。
  `<html lang="ja">` と Intl ja-JP なのにナビ・エラーが英語固定。読み上げ部分を優先して辞書化。
- [x] **通報フォームの必須入力と dedicated エラーを追加** — 根拠 `client/src/lib.rs` submit_report / discovery R5-VU / 中。
  TARGET CID / REASON に required・maxlength が無く、空送信が汎用 "Report failed." になる。
- [x] **検索0件の条件付き文言とクリア導線** — 根拠 `client/src/lib.rs` 空状態 / discovery R5-VU / 中。
  条件指定時も「承認済みコンテンツはありません」でサイト側問題と誤認される。
- [x] **無題イラストの aria-label フォールバック** — 根拠 `client/src/lib.rs` canvas role=img / discovery R5-VU / 中。
  title 空で aria-label="" になり画像が無名。cid 短縮形か「無題のイラスト」を表示。
- [x] **監査 Reason が要約表示である注記** — 根拠 `client/src/lib.rs` audit reason / discovery R5-VU / 低。
- [x] **クライアントテレメトリの告知とオプトアウト** — 根拠 `client/src/lib.rs` /metrics/client-events 送信 / discovery R5-VU / 低。
- [x] **E2E コメントの陳腐化行番号を識別子参照へ** — 根拠 `e2e/tests/error-paths.spec.ts`, `moderation.spec.ts` / discovery R5 / 低。

P2（品質・保守・プロダクト拡張）:

- [x] **ドキュメントの数値・記述を現行実装へ同期** — 根拠 `docs/DETAILED_DESIGN.md:602`（E2E 当時59件→現在69件）/
  `docs/TEST_SPECIFICATION.md:25`（実測は全69件）/ `docs/OPERATIONAL_POLICY.md:255,274`（全CLIの`--database-token`拒否・TZ）/
  `README.md:19,114-116`（RUST_LOG / 全CLIの`--database-token`拒否）/ `~/.agents/AGENTS.md:1`（共通正本）/
  `docs/UNKNOWN_TASKS_INVESTIGATION.md:4,16,175,181`（status: historical 済み、当時9/35件→現在69件/12 spec）/ R3-FG01〜11 / 低。
- [x] **finalizer/purge メトリクス5種と POST /metrics/client-events を設計・運用仕様へ追記** — 根拠
  `server/src/lib.rs:361-375,2169` / R3-FG01,FG02 / 低。実装済みだと仕様から見えない。
- [x] **支援オーバーレイをタッチ到達可能へ / UTC 日時をローカライズ / スキップリンク追加** — 根拠
  `client/src/lib.rs:1800-1808,1859,949-987` / R1-UX05,UX06,UX10 / 低。WCAG 2.1.1 / 2.4.1 観点。

- [x] 現行状態を謳う設計・状態ドキュメントを更新（C4 / R2-17〜R2-21、2026-11-18 対応済み）
- [x] セキュリティ勧告の受容根拠を RS256 本番署名に合わせて再評価（C6、2026-11-18 対応済み）
- [x] firefox / webkit プロジェクトを追加し Chrome 固定を解消（R3-09、2026-11-18 対応済み・オプトイン）
- [x] 設定検証 CLI（`doctor`）を追加（R5-06、2026-11-18 対応済み、`b39aba5`）
- [x] 機能提案 R5-10（小説 TOC と見出しアンカー、2026-11-18 対応済み）
- [x] 機能提案 R5-01（メタデータと検索・絞り込み、2026-11-18 対応済み）
- [x] 機能提案 R5-02（お気に入り / ブックマーク、2026-11-18 対応済み）
- [x] 機能提案 R5-09（透明性ダッシュボード、運用者限定の認証内集計、2026-11-18 対応済み）
- [x] 既存 /mod/reports の通報理由を要約化（2026-11-18 対応済み、ADR-0004 §9 に整合）
- [x] R5-04 のエクスポートのみ（2026-11-18 対応済み、削除権は法務・外部アンカー決定後）
- [x] client のエラー表示: /auth/verify 500 と /content/{cid}/audit 500 のサイレント失敗を修正（2026-11-18 対応済み）
- [x] CID 厳密検証（REVIEW_REPORT 残課題、2026-11-18 対応済み）: common::is_valid_cid で CIDv0/v1 を検証し全入力へ適用、mock CID も正規形式へ修正。
- [x] 中立化URLの件数指標（2026-11-18 対応済み）: POST /metrics/client-events で event/count のみ送信、
  openillustration_neutralized_urls_total を /metrics へ公開。URL本文・IP・UA は送らない。
- [x] 価格単位の型エイリアス固定（2026-11-18 対応済み）: AmountJpy = u64、wei と明確に区別。
- [x] 敵対的レビュー対応（2026-11-18）: CIDv0 の multihash prefix 検証、dag-cbor codec 対応、
  クライアントイベントの未知キー拒否、CORS 最外側化、API オリジンのビルド時注入。
- [x] BAN の `duration_days` を期限判定へ反映（2026-11-18 対応済み）: migration 013 で `users.ban_until` を追加し、
  `ban_user` が 1〜3650日（null=無期限）の期限を設定、`ensure_user_not_banned` が期限切れを判定上のみ解除。
  `unban` で NULL に戻し、`/me/export` の users 行にも含める（回帰テスト6件追加）。
- [x] 残指摘 R1-26: payment_orders の derivation_index 採番を書き込みロック＋トランザクション内へ移動し、
  受取先派生と INSERT を原子的化（2026-11-18 対応済み）。並行注文で連番の index と異なる受取先を
  割り当てる回帰テスト4件を追加。一意性は migration 003 の既存部分 UNIQUE インデックスで担保。
- [x] 残指摘対応（2026-11-18）: R1-23 OIDC pending 上限（1024件・429）と期限切れ回収、
  R1-14 アップロードの unwrap 連鎖を排除し失敗時に Processing 固まりを解消。
- [x] 敵対的レビュー対応（2026-11-18）: インスタンスロックの PID+starttime 検証と
  compare-and-delete、migration ファイル名・連続性の fail-closed 検証。
- [x] 敵対的レビュー対応（2026-11-18）: OIDC pending の起動時・周期 sweep、
  バックアップ時の生存ロック検査、監査スナップショットのマスク整合/版数検証。
- [x] E2E セレクタ堅牢化（2026-11-18）: alert を testid で特定、.group 依存を content-card へ変更。
  expect.poll の配列完全一致は過剰再フェッチ検出のため維持。
- [x] 投票フィードバック（2026-11-18）: 投票の成功通知・失敗エラーを表示（vote-error / vote-notice）、
  E2E 3件追加（当時37件、2026-11-20 時点65件 → 11-21 時点66件 → 現在69件）。
- [x] クライアント堅牢化（2026-11-18）: TOC スラッグ O(N²) を O(1) 化（8千件で約257倍）、
  検索の250msデバウンス＋世代チェック、投票一覧のエラー表示（NO_PENDING_VOTES誤表示解消）。
  E2E 定義数は当時37件（2026-11-18）、11-20 時点65件 → 11-21 時点66件 → 現在69件。
- [x] クライアント操作の堅牢化（2026-11-18）: お気に入りの二重送信防止・401案内、
  ダウンロード開始失敗のエラー表示とrevoke遅延延長。E2E 当時56件（2026-11-18）→ 現在69件。
- [x] テスト強化（2026-11-18）: JPKI 証明書のエラーパス5件（mutation 3パターン検出）、
  proptest を dev-dependencies 化して long-standing unused 警告を解消＋property test 4件。
- [x] セキュリティヘッダ/同時実行の強化（2026-11-18）: CORS プリフライト含む全応答へ nosniff/DENY、
  logout 時の refresh Cookie 削除（Max-Age=0 / Path=/auth）、同時リクエスト上限の境界・動作テスト。
- [x] 運用スクリプトの硬化（2026-11-18）: cron env を制限付き KEY=VALUE パーサへ変更し
  source/eval を撤廃、所有者・symlink・権限を検証。backup-rotate は umask 077 と
  mktemp+atomic rename、deploy へ UMask=0077。
- [x] sign_audit_export の鍵権限チェックに自動テスト9件を追加（2026-11-18 対応済み）。
  0600受理/0644拒否/親0777拒否/0755警告/symlink解決/--database-token拒否/上書き拒否/新規生成。
  mutation で検出力を確認。
- [x] JWT/E2E のカバレッジと安定性（2026-11-18）: 検証鍵JSON・alg混同・kid境界など10テスト追加、
  favorites の固定sleep/`toHaveLength` と economy の opacity 待ちを修正（3回連続安定）。
- [x] 年齢 property test の実効化（2026-11-18）: 2/29・不正日・未来日・全角・TZ境界を追加、
  独立オラクルを同語反復から実判定へ変更。国別しきい値は AgePolicy 責務として現行仕様を維持。
- [x] 一覧の価格 NULL を購入不可0として返す（2026-11-18 対応済み・未コミット、CONTENT-10）:
   `COALESCE(price, 0) AS price` で NULL 行も500にならず0を返す。サーバー側テスト1件
- [x] エクスポート解析失敗の固定文言表示（2026-11-18 対応済み・未コミット、EXPORT-11）:
   `export_status_message` が解析失敗時に `エクスポート結果の解析に失敗しました。` を返し、
   「0 件」成功表示へ落とさない。クライアント側単体テスト1件
- [x] 型境界とビルドゲート（2026-11-18）: 注文のNULL価格を400へ統一、CSS側CDN参照と
  非数値nodeバージョンを拒否、trunk検証の静的回帰を追加。E2E 56件。
  ※ 本番投票は実JPKI署名が未実装のため現状401（外部要因DEC-01）。
- [x] トランザクション安全性（2026-11-18）: exportのBEGINをwrite_lock配下へ、
  期限切れ更新と固定アドレス注文INSERTをtx内へ統一（高リスクの500を解消）。
  ドロップ順序は現状(guard,tx)が安全なため維持。`verify_order` の期限 TOCTOU も修正し、
  検証完了後の CAS 前に tx 内の最新 `expires_at` で再判定して期限切れを EXPIRED＋400 にした。
- [x] 周期タスクの停止性と型安全（2026-11-18）: finalize/purge のロック待機を2秒に限定し
  取得後はSQLを完了まで実行、insert_payment_order を &Transaction 引数へ固定。
- [x] 起動/周期タスクの停止性（2026-11-18）: ロック待機に30秒上限を設定し、
  shutdown watch で即停止、周期finalizeはロック取得後は全件完了（誤報warnを解消）。
- [x] swarm-coding（mixed, R1〜R6, 2026-11-18）: 認証/CLI/client の10件を修正。
  JWT検証鍵2048bit、clock skew範囲+doctor、自己署名JPKI拒否、Pinata CID検証、
  auth_verifyのDB失効削除と自己デッドロック解消、DBトークンargv拒否、
  restoreのinstance lock検査、Dashboard/favorite/ContentViewerのエラー表示、cid encode。
- [x] 定数単一情報源化とエラー表示（2026-11-18）: clock skew 定数を pub use で共有、
  favorite 再試行クリアの E2E、Ledger の /transactions 失敗表示（role=alert）。
  E2E 56件。
- [x] クライアント境界の硬化（2026-11-18）: 中立化URL件数を描画ソース単位で1回に抑制、
  protocol-relative/UNC URL を拒否、ファイル選択解除時に古い File をクリア。
- [x] E2E契約の監査対応（2026-11-18）: 件数矛盾(37→45)・MOD-07/10/11・Ledger/METシナリオ
  を是正、vote-notice role=status、dashboard-error/content-load-error のtestid検証、
  protocol-relative URL と再送抑制のE2Eを追加。
- [x] 入力境界のDoS対策（2026-11-18）: JPKIトークン/envelope/DER長、RSA署名(DER)長、
  Ethereum hex事前長チェック、list_reports/get_content_audit のLIMIT(500)。
- [x] 監査ログAPI顺序の是正（2026-11-18）: get_content_audit が最新500件を新しい順に返すよう変更。
  古い行はAPIから参照不可のため完全検証には署名付きエクスポート/DBを使用。
- [x] 境界テストの補完（2026-11-18）: RSA-3072/4096の署名長・DER上限非誤拒否を実測、
  JPKI envelope の certificate / verifiableCredential 両方を長さ検査。
- [x] 監査タイムラインの仕様化（2026-11-18）: APIが最新500件を新着順で返す契約を
  設計・テスト仕様へ反映し、FORENSIC_TIMELINEの表示順E2Eを追加（46件）。
- [x] ドキュメント数値の自動照合（2026-11-18）: scripts/test-doc-numbers.sh を追加し
  E2E件数・内訳・doctor check数・migration範囲を CI ゲート化。8進数解釈バグを修正。
- [x] ゲートと設定判定の整合（2026-11-18）: EXPOSE_INTERNAL_ERRORS の true 解釈、
  doc-numbers の動的内訳・8進数対策、backup-rotate の先頭ゼロ拒否、AGENTS.md のclippy整合。
- [x] テスト名の実在照合をゲート化（2026-11-18）: doc-numbers がドキュメント記載の
  Rustテスト名99件を実コードと照合。古い `test_jwt_require_kid_from_env` を検出し修正。
- [x] fail-open・境界・アクセシビリティ（2026-11-18）: Chromeバージョン抽出ガード、
  amount_jpy_from_wei境界テスト、監査パネル/ファイル選択/Moderation/upload のARIA、
  backup-rotate のディレクトリ権限 fail-closed、運用記述を是正。
- [x] wei変換と価格フォームの整合（2026-11-18）: amount_jpy_from_wei の0円をfail-closed拒否、
  アップロード価格inputへid/name/min/maxとラベル関連付けを追加。
- [x] a11y属性のE2E検証（2026-11-18）: Auditトグルのaria-expanded/dialog/Escape、
  ファイル選択ラベルのキーボード操作と価格input範囲を検証（48件）。
  aria-expandedは"true"/"false"文字列を返すよう実装を修正。
- [x] E2Eシナリオ仕様の是正（2026-11-18）: 11件の未反映を反映。MET ID衝突を
  NEUT-URL-02/03へ分離、a11y/CONTENT-ERR/favorites/export/GOV-01のオラクルを明記。
- [x] dialog/フォーカスとE2E堅牢性（2026-11-18）: aria-modal+初期フォーカス+Tab trap、
  label role=button、CLOSE/SpaceのE2E、否定アサーションの状態確定、MET poll延長。
  E2E 56件。
- [x] Audit dialog のARIA整合（2026-11-18）: 実挙動（背景操作可のnon-modal）に合わせ
  aria-modal を削除し、初期フォーカスとTab/Shift+TabトラップをE2Eで検証。
- [x] fail-closed・設定診断の実効化（2026-11-18/19）: backup find失敗伝播、CDNゲートの
  grep/findエラー、doctorの3カテゴリ死コード、OIDC_MAX_AUTH_AGE不正値起動拒否、
  restore_db必須引数、ライセンス本文コピー、未使用user_hash/allowを整理。
- [x] 本番設定診断と起動の整合（2026-11-19）: doctorへ5カテゴリ追加(14→19・34テスト)、
  OIDC起動エラーをpanicからanyhow化、EXPOSE_INTERNAL_ERRORS/ALLOWED_ORIGINS診断、
  restore_db/verify_audit_exportの--database-token拒否を統一。
- [x] CLI引数値の境界（2026-11-19）: take_valueヘルパでハイフン始まりの値を拒否し、
  unknown argument経由の秘密値エコーを防止（5 CLI）。
- [x] **[完了:DEC-04 公開範囲 ADR-0008]** R5-05 / R5-08 は一次ソース確認と設計が先
  - R5-03（クリエイターページ）は migration 不要だが、owner_hash の第三者公開範囲
    （DEC-04）が未決定のため見送り。認証必須・マスキング方針の合意後に再検討。

### 2026-11-19 全体レビューで追加（高）

- [x] **[完了:JPKI PoP mock 結線]** **JPKI 提示に所有証明（nonce 署名検証）を追加** — 根拠 `server/src/jpki.rs:27-45` / R3-AU01 / 重大度 critical。
  現状は証明書の公開鍵を取り出すだけで秘密鍵保持を検証せず、他人の公開証明書でなりすまし可能。
- [x] **OIDC callback に BAN チェックを追加** — 根拠 `server/src/oidc.rs:846-900` / R3-AU01 / 重大度 high。
  auth_verify/refresh は確認するが callback は欠落し、BAN 済み sub が再認証でセッション取得可能。
  2026-11-19 対応: INSERT 前に ensure_user_not_banned、mutation 検証・回帰テスト追加。
- [x] **API レスポンスへの CSP 付与** — 根拠 `server/src/lib.rs:241-247` / R1-ST01 / 重大度 high。
  2026-11-20 対応: `default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`
  を nosniff/DENY と同じ二層ミドルウェアで付与（contains_key ガード・プリフライト両対応）。
  配信側 CSP（dist/index.html のインライン script に nonce/hash が必要）は本番配信構成の設計判断のため別タスク。
  2026-11-20 再評価: サーバは HTML を配信せず（Leptos CSR は trunk serve が描画）、API 側 CSP は
  nosniff の多層化に留まり実益が限定的。配信側 CSP は dist/index.html のインライン script に
  nonce/hash が必要で本番配信構成（reverse proxy 等）の設計判断が先。段階: API 側 default-src 'none'
  等を nosniff と同じ二層へ追加 → 配信側は別タスク。
  security_headers は X-Content-Type-Options/X-Frame-Options のみ。CSP は本番構成確定時に別途設計と明記済み。
- [x] **外部キー制約を有効化** — 根拠 `server/src/lib.rs:851-858` / R2-DI01 / 重大度 high。
  libSQL は既定 foreign_keys=OFF。`PRAGMA foreign_keys=ON` と起動時 `foreign_key_check` を追加。
  2026-11-19 対応: migration前にON、孤立行はwarn、3テスト追加。
- [x] **CORS の allow_methods に DELETE を追加** — 根拠 `server/src/lib.rs:1773` / R1-AP01 / 重大度 high。
  remove_favorite は DELETE だが許可メソッドは GET/POST のみ。2026-11-19 対応済み。
- [x] **doctor --strict をデプロイ前ゲートへ組み込み（ローカル／Runbook）** — 根拠 `AGENTS.md:53` / R4-DR01 / 重大度 high。
  2026-11-20 対応: AGENTS.md / HANDOFF.md の検証コマンドと OPERATIONAL_POLICY の Runbook へ追加。
  CI の extra_gates は `ALLOW_IPV4_LOCALHOST=1` で strict が必ず失敗するため未変更（別ジョブ化が条件）。
  共通ワークフロー @v1 参照は不変。
  現状 doctor は警告のみで呼び出し経路が無い。
- [x] **backup-rotate に audit log 検証の自動統合を追加** — 根拠 `scripts/backup-rotate.sh:190-194` / R4-DR01 / 重大度 high。
  Runbook は両方成功を条件とするが自動取得は verify_backup のみだった。2026-11-20 対応済み:
  `verify_backup` 直後に生成直後の平文 tmp_db へ `verify_audit_log --database-url file:${tmp_db}` を実行し、
  失敗時は成果物を公開せず一時平文を清掃して `ALERT_CMD` で通知する（fail-closed）。
- [x] **暗号化後復号検証のテスト限定自動化** — 根拠 `scripts/test-backup-rotate.sh:637-709` / R4-DR01 / 重大度 high。
  2026-11-20 対応: ケース8で一時 GNUPGHOME の使い捨て鍵による暗号化→復号→バイト一致を検証。
  秘密鍵は一時領域にのみ存在し trap で削除、gpg 不在時はスキップ PASS。本番は鍵非保持設計を維持。
  2026-11-20 再評価: 本番は作成者が復号鍵を持たない設計を維持し、自動化はテスト内の使い捨て GPG
  鍵（mktemp GNUPGHOME + trap、リポジトリへ秘密鍵を置かない）で暗号化→復号→バイト一致を検証する
  スコープへ絞るのが妥当。gpg 未インストール環境では skip するゲートが必要。本番自動化は外部運用
  判断（鍵保管場所・復号担当者・検証タイミング）が確定するまで保留。
  復号には GPG 秘密鍵が必要だが、バックアップ作成者が復号鍵を持たない運用が安全なため自動化は対象外。
  鍵の保管場所・復号担当者・検証タイミングは外部運用側の判断待ち（未着手）。現状はリストア時の手動検証で担保。
- [x] **backup-rotate の失敗時一時ディレクトリ・平文 DB を清掃** — 根拠 `scripts/backup-rotate.sh:91-105` / R4-DR01 / 重大度 high。
  EXIT trap は rotation_list のみ削除し、平文 tmp_db が残置される。2026-11-19 対応済み。
- [x] **定足数を QUORUM_VOTES 定数へ集約** — 根拠 `server/src/lib.rs:2816,2834,3773` / R1-AR01 / 重大度 high。
  SQL と Rust で `3` が個別ハードコードされ、変更時に確定ロジックと集計が食い違う。2026-11-19 対応済み。
- [x] **リクエスト相関 ID とアクセスログを追加** — 根拠 `server/src/lib.rs:234-251` / R4-OB01 / 重大度 high。
  x-request-id 生成・伝播と method/path/status/duration ログが無く、Internal エラーの事後調査不可。
  2026-11-19 対応: tower-http trace feature、MatchedPathベースのJSONログ、3テスト。
- [x] **単一ライター・ロックの stale 回収 TOCTOU を解消** — 根拠 `server/src/lib.rs:840-882` / R2-BV01 / 重大度 high。
  2026-11-20 吟味の結果却下（対応済みとしてクローズ）: 現行実装は stale 判定後の再読込一致チェックと
  create_new の AlreadyExists 拒否を備え、同時起動では必ず片方が失敗するため排他は破れない。
  OS の flock への置換は不要（追加依存・プラットフォーム差のリスクのみ）。
- [x] **通報送信 (/mod/report) の失敗系 E2E を追加** — 根拠 `e2e/tests/moderation.spec.ts:102` / disc TQ / 重大度 high。
  HTTP 500/network abort の固定文言・サーバー本文非表示が未検証。2026-11-19 対応済み。
- [x] **ビルドゲート E2E を実バイナリ改ざんの統合テストへ** — 根拠 `e2e/tests/build-gate.spec.ts:5-8` / R5-TS01 / 重大度 high。
  現状はシェル文字列パターンのみで fail-open 回帰を実挙動で担保できていない。
  2026-11-19 対応: 検証をverify_trunk_archive.shへ抽出し実改ざんでexit1/未展開を検証（E2E54件）。
- [x] **GOV-01 要約 E2E を生 PII 入力→[redacted] 表示へ** — 根拠 `e2e/tests/moderation.spec.ts:9-14` / R5-TS01 / 重大度 high。
  却下してクローズ（2026-11-19 判断、本ファイル「メモ」の Runtime Proof と整合）。要約は
  `common::summarize_reason` がサーバー側で行い、`e2e/tests/moderation.spec.ts:6-8` のとおり
  E2E は要約済み値をモックから返す設計。生 PII を E2E 入力へ入れると DOM に露出し実装と
  矛盾するため却下（R5-TS01 / 過去の判断を維持）。担保は Rust 単体・property テスト。
- [x] **Upoad 価格境界 E2E を属性存在→実送信へ** — 根拠 `e2e/tests/upload.spec.ts:172-174` / R5-TS01 / 重大度 high。
  min/max 属性だけでは JS/API バリデーション回帰を検出できない。2026-11-19 対応済み。
- [x] **ADR-0002/0003 の「提案」残存を確定記述へ** — 根拠 `docs/adr/ADR-0002-jpki-authentication.md:35` / R5-DC01 / 重大度 high。
  Status: Accepted なのに本文・tags が proposed のまま。2026-11-19 対応済み。
- [x] **ADR/Runbook/OPERATIONAL_POLICY の矛盾を是正** — 根拠 `docs/HANDOFF.md:84`,
  `docs/BACKUP_RESTORE_RUNBOOK.md:264`, `docs/OPERATIONAL_POLICY.md:97-99` / R5-DC01 / 重大度 high。
  ADR-0004 を Proposed とする記載、暗号化・オフサイトの未解決重複、--database-token 付与例と文命題切れ。
  2026-11-19 対応済み（R4 指摘で保持期間・「のみ」表現も修正）。

### 2026-11-19 全体レビューで追加（中）

- [x] **verify_audit_export へ take_value を追加（5 CLI の取り残し）** — 根拠
  `server/src/bin/verify_audit_export.rs:33-50` / disc FG / 優先度 中。2026-11-20 対応済み。
- [x] **IPFS チェックサム不一致を 401 ではなく 502/409 へ** — 根拠 `server/src/lib.rs:2294` / R1-AP01 / 中。
  2026-11-20 対応: `AppError::BadGateway` を 502 で返し、`#[error]` と IntoResponse 本文を
  `Upstream content failed integrity verification` へ統一（409 は採用せず 502 へ集約）。
  全ゲートウェイ失敗の 500→502 統一は設計判断で範囲拡大のため見送り。
- [x] **一覧 API のページネーション共通化（段1）** — 根拠 `server/src/lib.rs:2849-2873` / R1-AP01 / 中。
  2026-11-20 対応: limit/offset 解析を `parse_pagination` へ抽出し `parse_content_query` が委譲。
  既定50・範囲1..=100・offset>=0・trim・フォールバック・レスポンスは不変。単体テスト2件追加。
  段2（/transactions・/mod/reports への省略可 limit/offset）と段3（page/per_page+メタデータ）は
  利用側の要求が確認できてから別タスク（既存の裸配列レスポンスを壊さないため）。
- [x] **エラー本文スキーマを code/message/status へ統一** — 根拠 `server/src/lib.rs:548-576` / R1-AP01 / 中。
  2026-11-20 吟味の結果却下（対応済みとしてクローズ）: AppError の全バリアントとミドルウェアの
  エラー応答は既に `{"error": message}` に統一済み。`code/message/status` への変更はクライアントの
  `body.error` 依存を壊す破壊的変更で、現状の利益を上回らない。/ready の `{"status": ...}` は
  ヘルスチェックの状態表現でエラー契約とは用途が異なる。
- [x] **note 注文期限・refresh purge の RFC3339 文字列比較をパース比較へ** — 根拠 `server/src/payments.rs:524-526` / R2-BV01 / 中。
  2026-11-20 対応: is_order_expired をパース比較＋fail-closed、purge を datetime() 正規化。
- [x] **JPKI 発行セッションへ sid を付与し logout を有効化** — 根拠 `server/src/lib.rs:2046-2051` / R3-AU01 / 中。
  2026-11-20 対応: auth_verify の Claims に sid 付与、logout を冪等化（lib.rs:2221, 2285）。
- [x] **OIDC state を UA/クッキーへバインド（ログイン CSRF）** — 根拠 `server/src/oidc.rs:353-381` / R3-AU01 / 中。
  2026-11-20 対応: double-submit Cookie を本番（`cookie_secure=true`）では
  `__Host-oidc_state` / Path=/ / Secure、mock では従来の `oidc_state` / Path=/auth で
  発行・検証（start と callback で同名・同 Path）。同一名 Cookie の重複は攻撃の兆候として
  401 で拒否。PKCE / nonce / 年齢ゲート順序 / state 検証位置は不変。
- [x] **OIDC エンドポイント URL へ https 必須化** — 根拠 `server/src/oidc.rs:249-276` / R3-AU01 / 中。
  2026-11-20 対応: `from_env_with` が全5エンドポイントを検証し https のみ受理、`ALLOW_MOCK_AUTH`
  有効時のみ localhost 系の http を許容（fail-closed・設定値はエコーしない、実装は oidc.rs:280-314）。
  R5 で R4 採用指摘を反映し、エラー文言を実判定へ整合（`test_oidc_from_env_with_endpoint_url_scheme_validation`
  の期待値も同一文言へ更新）。判定条件・mock 許容ロジックは不変。
- [x] **BAN 時に既存セッション/リフレッシュトークンを即失効** — 根拠 `server/src/lib.rs:2951-2985` / R3-AU01 / 中。
  2026-11-20 対応: コミット後に DB 閾値→refresh 消費の順で失効、閾値は単調増加（lib.rs:3207-3235）。
- [x] **[完了:payment_orders 却下 ADR-0009]** **payment_orders の tx_hash=NULL 重複を部分 UNIQUE で防止** — 根拠
  `server/migrations/002_payment_orders.sql:16` / R2-DI01 / 中。
  2026-11-20 吟味の結果却下: order_id は毎回新規 UUID で同じ ppid/cid の複数注文（再購入）
  が正当。固定受取アドレスの同一 (chain, recipient, amount) 3回テストが 500 になる回帰を
  Runtime Proof で確認。DB制約ではなくアプリ側の冪等性設計を維持。
- [x] **[完了:refresh_tokens migration 014]** **refresh_tokens.expires_at の時刻形式を RFC3339 へ統一**
  — 根拠 `server/migrations/006_refresh_tokens.sql:7` / R2-DI01 / 中。
- [x] **GRACEFUL_DRAIN/MODERATION_FINALIZE を doctor フォールバック検出へ** — 根拠 `server/src/lib.rs:1342,1354` / R1-CF01 / 中。
  2026-11-20 対応: 2カテゴリ追加（19→21 check(s)、29→36テスト）。
- [x] **ALLOWED_ORIGINS の doctor 判定を allowed_origins() 結果へ** — 根拠 `server/src/bin/doctor.rs:354-369` / R1-CF01 / 中。
  2026-11-20 対応: 実パーサ結果で判定、警告文を実態に合わせ修正、check数21不変。
- [x] **wasm リリースから console_log/panic_hook をゲート** — 根拠 `client/Cargo.toml:13-14` / R3-DE01 / 中。
  2026-11-20 対応: optional + dev-console feature、既定off、main()をcfgでゲート。
- [x] **JXL デコーダにサイズ・次元上限と fuzz を追加** — 根拠 `client/Cargo.toml:26` / R3-DE01 / 中。
  2026-11-20 対応: `dimensions_allowed` が `MAX_DIMENSION` / `MAX_PIXELS` を `saturating_mul` で検査し、
  決定論的 xorshift64* による 10,000 通りの fuzz プロパティテストを追加。R5 で R4 採用指摘を反映し、
  「独立オラクル」表現を「上限定義との整合を再確認する」へ是正、`expected` に既に含まれる
  冗長な面積 assert を削除（fuzz 本体・10,000 サンプリングは維持）。
- [x] **再利用ワークフロー参照をコミット SHA 固定へ** — 根拠 `.github/workflows/ci.yml:26` / R3-DE01 / 中。
  2026-11-20 再評価の結果「対応しない」としてクローズ: 共通指示（~/.agents/AGENTS.md:85）と
  リポジトリ AGENTS.md:55 が `rust-gates.yml@v1` タグピンを明記しており、SHA 固定は方針と矛盾。
  変更するなら提供側 Seduce0068/workflows のタグ運用設計が必要で本リポジトリのスコープ外。
- [x] **npx --yes を package-lock + npm ci へ** — 根拠 `.github/workflows/ci.yml:38` / R3-DE01 / 中。
  2026-11-20 対応: CI は `npm ci --ignore-scripts --no-audit --no-fund` の後に
  `npx markdownlint-cli2` でローカル固定版を実行。`package.json` へ
  `engines.node = ">=22"` を明記（markdownlint-cli2 0.24.0 が Node>=22 を要求）。
  `package-lock.json` は依存記述のみで差分不要（親が `npm ci` で確認）。
- [x] **[完了:E2E 条件付き3ブラウザ]** **E2E の既定ブラウザを 3 プロジェクトへ** — 根拠 `e2e/ci-e2e.sh:121` / R4-CI01 / 中。
- [x] **E2E の npm ci を --ignore-scripts 統一** — 根拠 `e2e/ci-e2e.sh:211` / R4-CI01 / 中。
  2026-11-20 対応: client と同一引数（--ignore-scripts --no-audit --no-fund）。
- [x] **監視パス (/health,/ready,/metrics) を observe_metrics から除外** — 根拠 `server/src/lib.rs:315-319` / R4-OB01 / 中。
  2026-11-20 対応: GET スクレイプのみ除外し、POST /metrics などの非 GET は
  requests / errors へ計上（非 GET 計上テストを追加）。/ready の 503 も GET
  スクレイプとして除外し、readiness は /ready レスポンス自体で監視（新規メトリクスなし）。
- [x] **finalizer/purge のカウンタを /metrics へ公開** — 根拠 `server/src/lib.rs:346-392` / R4-OB01 / 中。
  2026-11-20 対応: ticks / processed / lock_busy / failures / last_success を
  HELP/TYPE 付きで公開。`test_metrics_expose_finalizer_counters` と
  `test_periodic_finalize_lock_wait_is_bounded` で差分検証（purge 3 + finalize 1）。
- [x] **起動時 IPFS 警告の生 URL を redact_url 経由へ** — 根拠 `server/src/lib.rs:1071-1074` / R4-OB01 / 中。
  2026-11-20 確認: 起動時ログは URL ではなく gateway 件数のみを出力し、
  取得時エラーは `redact_url` 経由（lib.rs:2679 / 2721 / 2737）で生 URL を残さない。
- [x] **投票・通報フォームへ二重送信ガード** — 根拠 `client/src/lib.rs:2205-2237` / disc VU / 中。
  2026-11-20 対応: `claim_submission` による同期判定と `BusyGuard` のスコープ離脱時解除を実装し、
  `disabled` は bool のまま、`aria-busy` を `"true"` / `"false"` 文字列列挙値へ整合。
  通報と投票の横断排他は意図的分離の可能性があるため今回は見送り。
- [x] **複数カードの DOM id / TOC アンカーを cid 名前空間化** — 根拠 `client/src/lib.rs:505-517,1649` / disc VU / 中。
  2026-11-20 対応: cid_scope_token + prefixed描画、audit id も cid付与、StoredValue化。
- [x] **一覧カードの監査取得をトグル時のみへ削減** — 根拠 `client/src/lib.rs:1543-1582` / disc VU / 中。
  2026-11-20 対応: IntersectionObserver は真因ではないため使わず、監査 GET を show_audit=true 初回
  遷移時のみへ移動（audit_fetched_cid フラグを await 前に設定）。本文 GET は従来どおり。
  前回の real-api 失敗は他 spec との並走が真因で既に 2 段階実行で解消済み。
  2026-11-20 吟味の結果見送り: A/B で全件並走時に real-api 4件が /auth/verify タイムアウトで
  失敗したが、単独実行では差分の有無にかかわらず成功。真の原因は他 spec との並走による
  実サーバー競合で IntersectionObserver 差分とは非因果（ci-e2e.sh で real-api 単独実行へ
  分離して解消）。遅延ロード自体は再検討候補として残す。
- [x] **アップロード失敗を HTTP ステータス別固定文言へ** — 根拠 `client/src/lib.rs:1825-1826` / disc VU / 中。
  2026-11-20 対応: `upload_error_message` の4分岐（401=再ログイン / 413=サイズ超過 /
  その他=`Failed to upload content (HTTP {status}).` / None=`Network error uploading content.`）を
  TEST_SPECIFICATION / DETAILED_DESIGN / E2E (`upload.spec.ts`) の確定文言へ整合。
- [x] **価格 parse 失敗の 0 円化を防止し dedicated エラー** — 根拠 `client/src/lib.rs:1873-1880` / disc VU / 中。
  2026-11-20: parse_price_input + valueAsNumber で対応済み（Round 7）。
  前回の親判断（R9 で再確認）: `price_range_error` はテスト内ヘルパー
  （`client/src/lib.rs:2676-2705`）のみに留め、本番の `sign_and_upload` には接続しない。
  発掘エージェントは「E2E を変更したなら本番へ接続すべき」と指摘するが、価格境界 E2E は
  0 / 1000001 を丸めずに送ってサーバー（モックが同じ境界を模す）の拒否を確認する契約であり、
  接続するとクライアント側で弾かれてサーバー拒否を確認する契約が壊れる。
  E2E 契約維持のための意図的な未接続であり、E2E 側の契約変更がない限り接続しない。
- [x] **日本語見出し slug を Unicode 対応へ** — 根拠 `client/src/lib.rs:261-273` / disc VU / 中。
  2026-11-20 対応: is_alphanumeric + to_lowercase再フィルタ（U+0130対策）。
- [x] **MET ID 衝突（仕様 NEUT-URL-02/03 vs E2E MET-02/03）を解消** — 根拠 `e2e/tests/metrics.spec.ts:206` / disc TQ / 中。
  2026-11-20 対応: E2E の test 名を `NEUT-URL-02` / `NEUT-URL-03` へ改名し、TEST_SPECIFICATION の表・シナリオ記述も改名済み名へ整合。
- [x] **オーファン ゲート ID（EXPORT-12, PAY-06〜08）を整理** — 根拠 `docs/TEST_SPECIFICATION.md:194,209` / disc TQ / 中。
  2026-11-20 対応: TEST_SPECIFICATION の実 ID（EXPORT-09/10、PAY-01/04）へ統一し、
  TASK_INVENTORY の旧 ID 記述も置換済み。
- [x] **価格境界 E2E の送信検証（モック応答）** — disc TQ / 中（R5-TS01 と同一、統合済み）。
  2026-11-20: upload.spec.ts の0/1000001/1000000送信で対応済み。`/content/upload` は
  `page.route` + `route.fulfill` によるモック応答であり、実サーバーへの送信ではない
  （R9 で「実送信」表現を是正）。境界判定は実サーバーと同じ <0 / >1000000 をモックで模す。

### 2026-11-19 全体レビューで追加（低）

- [x] **UI 文言を日本語へ統一（英語・内部トークン混在）** — 根拠 `client/src/lib.rs:69-70,1094-1097` / R5-UX01 / 低。
  2026-11-20 対応: E2E アサート文言は維持、内部トークン/進行状態10箇所を日本語化。
  aria-label を可視文言へ整合（Label in Name）、Processing 否定アサートをE2E側も更新。
- [x] **ナビゲーションへ aria-current を付与** — 根拠 `client/src/lib.rs:836-855` / R5-UX01 / 低。
- [x] **SPA ビュー切替で document.title を更新** — 根拠 `client/src/lib.rs:824` / R5-UX01 / 低。
- [x] **データ表へ caption と th scope** — 根拠 `client/src/lib.rs:1993-2001` / R5-UX01 / 低。
- [x] **canvas へ role=img/aria-label** — 根拠 `client/src/lib.rs:1599-1600` / R5-UX01 / 低。
- [x] **PRODUCT 名と (Rust) 表記を統一** — 根拠 `client/index.html:6` / R5-UX01 / 低。
- [x] **verify_backup/verify_audit_export のヘルプへ DATABASE_TOKEN 契約** — 根拠
  `server/src/bin/verify_backup.rs:12` / disc FG / 低。
- [x] **doctor --strict --help 併用を他 CLI と整合** — 根拠 `server/src/bin/doctor.rs:43-50` / disc FG / 低。
- [x] **README に運用 CLI 案内を追加** — 根拠 `README.md:34-87` / disc FG / 低。
- [x] **ENV_LOCK の読み手保護を設定注入へ** — 根拠 `server/src/tests.rs:87-108` / R5-TS01 / 低。
- [x] **定期 finalize テストの固定スリープを同期チャネルへ** — 根拠 `server/src/tests.rs:15490-15564` / R5-TS01 / 低。
- [x] **NEUT-URL-03 再送抑制 E2E の quiescence 待ち** — 根拠 `e2e/tests/metrics.spec.ts:227-252` / R5-TS01 / 低。

## 完了

- [x] todo をプロジェクト内（`todo.md`）へ配置し、グローバル AGENTS に運用を明記（2026-11-17）
- [x] 6パス・クロスレビューの統合結果を P0/P1/P2 として本ファイルへ転記（2026-11-17）
- [x] R2-09: `get_content` を `status = 'Approved'` で絞り、Reported/Rejected を 404 化（2026-11-17）
- [x] 未コミット変更のドキュメント反映（2026-11-18）: メトリクス最外側化・決済 bind 署名 v2・監査CLI硬化を
  DETAILED_DESIGN / OPERATIONAL_POLICY / TEST_SPECIFICATION / TASK_INVENTORY / AUDIT_EXPORT_RUNBOOK へ記載。
  E2E 定義は40件のまま（今回の追加はサーバー側テスト5件）。git commit/push は親エージェントが実施。
- [x] 3CLIの自動テスト追加のドキュメント反映（2026-11-18）: doctor / verify_audit_export /
  verify_backup の純粋関数化・`--expect-root`・`--database-token` 拒否・sidecar fail-closed と
  各24/17/17テストを TEST_SPECIFICATION（CLI-01〜03 ゲート）・DETAILED_DESIGN・
  AUDIT_EXPORT_RUNBOOK・BACKUP_RESTORE_RUNBOOK・TASK_INVENTORY へ反映。
  E2E 定義は40件のまま（全てサーバー側 bin テスト）。frontmatter 日時は `date` 実測値へ更新。
  git commit/push は親エージェントが実施。
- [x] ネットワーク hardening のドキュメント反映（2026-11-18）: SSRF リダイレクト無効化
  （`Policy::none()`、3xx フェイルオーバー、2件）、XFF 追記型/上書き型の解釈と
  IPv4-compatible（`::a.b.c.d`）拒否（11件）、`doctor --strict` の
  `ALLOW_IPV4_LOCALHOST` / `TRUSTED_PROXY_CIDRS` 警告（13カテゴリ・24テスト）を
  DETAILED_DESIGN / OPERATIONAL_POLICY / TEST_SPECIFICATION / INFRA_NETWORK /
  TASK_INVENTORY へ記載。E2E は40件のまま。frontmatter 日時は `date` 実測値へ更新。
  git commit/push は親エージェントが実施。
- [x] 年齢ゲート共通化・TZ ローカル日付・`/me/export` 読み取りトランザクションのドキュメント反映
   （2026-11-18）: 実コードを grep して確定した値のみを DETAILED_DESIGN / AGE_VALIDATION_POLICY /
   OPERATIONAL_POLICY / TEST_SPECIFICATION / TASK_INVENTORY へ記載。新規テスト5件は
   AGE-03 / AGE-04 / EXPORT-09 / EXPORT-10 の各ゲートへ反映し、E2E 定義は40件のまま。
   frontmatter 日時は `date` 実測値へ更新。git commit/push は親エージェントが実施。
- [x] 監査マスク・投票署名 v2・価格 NULL・エクスポート解析のドキュメント反映（2026-11-18）:
   実コード（`server/src/lib.rs` / `server/src/tests.rs` / `client/src/lib.rs`）を grep して確定した
   値のみを DETAILED_DESIGN / OPERATIONAL_POLICY / TEST_SPECIFICATION / TASK_INVENTORY /
   ADR-0004 へ記載。新規テスト7件（監査マスク1件・投票 v2 4件・価格 NULL 1件・クライアント解析1件）は
   AUDIT-03 / MOD-12 / CONTENT-10 / EXPORT-11 の各ゲートへ反映し、E2E 定義は40件のまま。
   frontmatter 日時は `date` 実測値へ更新。git commit/push は親エージェントが実施。
- [x] 敵対的レビュー指摘（期限 TOCTOU・旧設計記述）の対応（2026-11-18）:
   `verify_order` が検証 I/O 完了後に tx 内の最新 `expires_at` を再判定し、期限切れなら
   EXPIRED 更新をコミットして400を返す（FINALIZED へ進ませない）。回帰テスト
   `test_payment_verify_rechecks_expiry_after_verification` を追加し、再判定削除の mutation で
   失敗することを確認（一時変更は復元・sha256 一致）。DETAILED_DESIGN / TEST_SPECIFICATION の
   旧設計の無ロック記述を現行実装（`begin_write_tx`）へ是正し、直近の5テスト
   （export lock 1・決済期限2・固定アドレス2）と新規テストを TEST_SPECIFICATION へ反映。
   TASK_INVENTORY も対応済みへ更新。E2E 件数は37件で統一（旧表記を是正）。
   frontmatter 日時は `date` 実測値へ更新。git commit/push は親エージェントが実施。

## メモ

- 2026-11-24 コンボ第83ラウンド（review/discovery）: 全7担当完了。高は0件。
  R6 で以下を是正: READMEメトリクス完全名化+ルートnpm ci手順、
  favoriteのaria-label固定化（状態はaria-pressed/★☆）+E2E testid化、
  backup unit/cronのDATABASE_URLハードコード除去+backup.env同期必須、
  prometheus target PORT追随注記、構成図HTTPS表記。
  discovery指摘のうちdialog lang/live region常設化は既存E2E契約と衝突したため
  元へ戻し（R2-LANG契約・toHaveCount(0)オラクルを維持）、E2Eを再green化。
  R4指摘0件。全ゲート green（E2E69/check23/doctor54/migrations14/tests156、
  doctor 0 problem(s)、yaml/tsc green、E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: retry generationガード、401時の再読み込み抑制、axe WCAG AA導入、
  globalSetup認証後ビュー監視、fail-closedカウンタ2種のテスト・仕様、
  error_lang UTC例外テスト、aria-label/placeholderのlang、通信タイムアウトのテレメトリ、
  export revoke delayオラクル汚染、fetch timeout分岐テスト、jxl fuzz独立オラクル、
  BAN/UNBAN再送回帰テスト、空sub拒否回帰テスト、Node/Chrome比較分岐テスト、
  CIツールchecksumピン留め、署名公開失敗回帰、pushState失敗告知、
  ナビ/投票の24pxタッチ目標、Markdown heading階層、投票.disabled理由、
  noscript/fallback、お気に入り削除の冪等性、governance_entries構造検証、
  refresh iatのDB正本化、E2E_SERVER_PORTのクライアント伝播、npm auditトリアージ、
  バックアップ鮮度timer、TASK_INVENTORY historical化、E2E69下限緩和）は記録のみで保留。

- 2026-11-24 コンボ第82ラウンド（review/discovery）: 全7担当完了。
  R6 でhigh1件を是正: 期間付きBAN分岐のCAS漏れ（無期限BANが期間付きで上書きされ
  期限到達後に意図せず解除される／再送で監査重複・期限延長）を修正。
  他: perform_refreshとlogoutの/auth/logoutもfetch_with_timeout対象へ
  （起動時復元/自動refresh/logoutの無期限ハング防止）。
  R4指摘1件（allow(unused_assignments)をif/else式変換で除去）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  wasm/tsc green、E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: retry generationガード、401時の再読み込み抑制、axe WCAG AA導入、
  globalSetup認証後ビュー監視、fail-closedカウンタ2種のテスト・仕様、
  error_lang UTC例外テスト、aria-label/placeholderのlang、通信タイムアウトのテレメトリ、
  jxl fuzz独立オラクル、BAN/UNBAN再送回帰テスト、空sub拒否回帰テスト、
  Node/Chrome比較分岐テスト、CIツールchecksumピン留め、署名公開失敗回帰、
  pushState失敗告知、ナビ/LOG OUT/投票の24pxタッチ目標、Markdown heading階層、
  投票.disabled理由、noscript/fallback、お気に入り削除の冪等性、
  governance_entries構造検証、refresh iatのDB正本化、公開集計閾値設定化、
  RuntimeConfig一括化、doctor MIGRATIONS_DIR診断、sign_audit 2ログ原子スナップショット、
  E2E_SERVER_PORTのクライアント伝播、npm auditトリアージ、バックアップ鮮度timer、
  TASK_INVENTORY historical化、E2E69下限緩和）は記録のみで保留。

- 2026-11-24 コンボ第81ラウンド（review/discovery）: 全7担当完了。高は0件。
  R6 で以下を是正: OIDC ID/logoutトークンの空subをfail-closed拒否、
  BAN/UNBANのusers UPDATEをCAS化して再送時の監査重複を防止
  （BANは0行でも閾値UPSERT+トークン消費を継続）、
  公開集計鮮度判定を非wasm純粋関数(is_stale_seconds)へ分離し単体テスト追加、
  backup-rotate件数コメントを実測31/16/47へ修正。
  R4指摘（BAN早期returnでトークン消費がスキップされる/UNBAN未冪等）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  wasm/tsc green、E2E_CLIENT_PORT=18123で実施）。cargo deny一時障害は再試行で解消。
  残存（次ラウンド: retry generationガード、401時の再読み込み抑制、axe WCAG AA導入、
  globalSetup認証後ビュー監視、fail-closedカウンタ2種のテスト・仕様、
  error_lang UTC例外テスト、jxl fuzz独立オラクル、Node/Chrome比較分岐テスト、
  CIツールchecksumピン留め、署名公開失敗回帰、pushState失敗告知、
  ナビ/LOG OUT/投票の24pxタッチ目標、Markdown heading階層オフセット、
  投票.disabled理由、noscript/fallback、公開集計閾値の設定化、RuntimeConfig一括化、
  doctor MIGRATIONS_DIR診断、sign_audit 2ログ原子スナップショット、
  logout失効記録なし200のfail-open、孤立ピンrescan、E2E69下限緩和）は記録のみで保留。

- 2026-11-24 コンボ第80ラウンド（review/discovery）: 全7担当完了。
  R6 でhigh1件を是正: クライアントfetchにタイムアウト追加（futures::select +
  setTimeout/JsFuture、30秒でgloo_net::Errorを返しBusyGuard解除を保証）。
  他: Node22下限検査、TEST_SPEC CI3ブラウザ必須記述、Runbook 7世代誤引用修正、
  iOS PWA用PNGアイコン(180/192/512)追加、監査見出しh3化。
  R4指摘1件（icon-192/512のcopy-file漏れ）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  wasm/tsc green、E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: retry generationガード、401時の再読み込み抑制、axe WCAG AA導入、
  globalSetup認証後ビュー監視、fail-closedカウンタ2種のテスト・仕様、
  error_lang UTC例外テスト、jxl fuzz独立オラクル、Node/Chrome比較分岐テスト、
  backup-cron-wrapper所有者/mode分岐テスト、CIツールchecksumピン留め、
  署名公開失敗回帰、pushState失敗告知、ナビ/LOG OUT/投票の24pxタッチ目標、
  Markdown heading階層オフセット、投票.disabled理由、noscript/fallback、
  公開集計閾値の設定化、RuntimeConfig一括化、doctor MIGRATIONS_DIR診断、
  sign_audit 2ログ原子スナップショット、E2E69下限緩和）は記録のみで保留。

- 2026-11-24 コンボ第79ラウンド（review/discovery）: 全7担当完了。高は0件。
  R6 で以下を是正: bindをfinalizer spawn前に移動（起動失敗時のデタッチ防止）、
  canvas aria-labelをdecode/load失敗状態へ連動+E2E回帰、
  E2E_MOBILE追記時のiphone/pixel重複除去、TEST_SPEC export説明、
  HANDOFF/READMEへshfmt fail-closed+markdownlint追加。
  R4指摘2件（doc-numbersのバイト/文字位置fail-openを前文抽出方式へ、
  canvas aria-label回帰テスト）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  doc-numbers/tests=156、tsc/shfmt/shellcheck green、E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: retry generationガード、401時の再読み込み抑制、axe WCAG AA導入、
  globalSetup認証後ビュー監視、fail-closedカウンタ2種のテスト・仕様、
  error_lang UTC例外テスト、jxl fuzz独立オラクル、Node/Chrome比較分岐テスト、
  backup-cron-wrapper所有者/mode分岐テスト、doctor-ci-env双方向照合、
  署名公開失敗回帰、pushState失敗告知、ナビ/LOG OUT/投票の24pxタッチ目標、
  投票.disabled理由、noscript/fallback、apple-touch-icon PNG、
  公開集計閾値の設定化、RuntimeConfig一括化、doctor MIGRATIONS_DIR診断、
  sign_audit 2ログ原子スナップショット、backup_db所有権再確認、
  Prometheus finalizer gaugeアラート、real-api後始末、E2E69下限緩和）は記録のみで保留。

- 2026-11-24 コンボ第78ラウンド（review/discovery）: 全7担当完了。高は0件。
  R6 で以下を是正: retryのaria-busyをARIAトークン値("true"/"false")化、
  commonのbase64をoptional+audit featureへ（checksum-only wasmから除外）、
  取引履歴caption/public-stats再ログイン案内へlang、GENERATED_ATラベルをspan lang=en化、
  export_timestamp境界テスト（月13/時24/分60/秒60）、expect_*件数ゲートを
  空白許容grepへ（実測31/16/47、docs追随）、R4指摘（スクリプト先頭コメント）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  doc-numbers/tests=156、wasm green、cargo treeでbase64除外確認、
  E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: retry generationガード、401時の再読み込み抑制、axe WCAG AA導入、
  globalSetup認証後ビュー監視、fail-closedカウンタ2種のテスト・仕様、
  error_lang UTC例外テスト、jxl fuzz独立オラクル、Node/Chrome比較分岐テスト、
  backup-cron-wrapper所有者/mode分岐テスト、doctor-ci-env双方向照合、
  署名公開失敗回帰、pushState失敗告知、ナビ/LOG OUT/投票の24pxタッチ目標、
  投票.disabled理由、noscript/fallback、apple-touch-icon PNG、
  公開集計閾値の設定化、RuntimeConfig一括化、doctor MIGRATIONS_DIR診断、
  backup_db created()非対応FS、sign_audit親dir fsync、migration分割器制限、
  Runbook平文オフサイト記述、HANDOFF metrics記述、E2E69下限緩和）は記録のみで保留。

- 2026-11-24 コンボ第77ラウンド（review/discovery）: 全7担当完了。高は0件。
  R6 で以下を是正: セッション発行のiatをメモリだけでなくDB正本(revoked_subs)から
  採番（auth_verify/OIDC callback両方、issued_iat_from_dbヘルパ追加）、
  revoke_session/is_session_revokedのpoisonを警告格下げしてDB照会へ、
  READMEの/metricsをcounter/gauge区別（finalizer_last_success/rate_limiter_keysはgauge）。
  R4指摘1件（is_session_revoked側のpoison）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  server全テスト green、E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: retry generationガード、401時の再読み込み抑制、axe WCAG AA導入、
  globalSetup認証後ビュー監視、fail-closedカウンタ2種のテスト・仕様、
  error_lang UTC例外テスト、jxl fuzz独立オラクル、Node/Chrome比較分岐テスト、
  backup-cron-wrapper所有者/mode分岐テスト、web-sys未参照feature5件、
  doctor-ci-env双方向照合、署名公開失敗回帰、pushState失敗告知、
  ナビ/LOG OUT/投票の24pxタッチ目標、投票.disabled理由、noscript/fallback、
  apple-touch-icon PNG、公開集計閾値の設定化、RuntimeConfig一括化、
  doctor MIGRATIONS_DIR診断、REVIEW_REPORT未更新、E2E69下限緩和）は記録のみで保留。

- 2026-11-24 コンボ第76ラウンド（review/discovery）: 全7担当完了。高は0件。
  R6 で以下を是正: GPG署名テストの本番trust-model違いをテスト専用と明記、
  RunbookのTimeoutStopSec根拠（同梱unitは330秒）とStartLimit到達時の
  reset-failed復旧手順、静的日本語ノードへlang="ja"付与。
  retry連打ガードはE2Eの段階遷移契約（401→403→429即時遷移）と衝突したため
  同一ハンドラ再入のみの現状を維持し、generationベースの完全修正は残存へ。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  backup-rotate tests OK、markdownlint35ファイル、E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: retry generationガード、axe WCAG AA自動スキャン導入、
  globalSetup認証後ビュー監視、fail-closedカウンタ2種のテスト・仕様、
  error_lang UTC例外テスト、jxl fuzz独立オラクル、pushState失敗告知、
  ナビ/LOG OUT/投票の24pxタッチ目標、投票.disabled理由、noscript/fallback、
  apple-touch-icon PNG、公開集計閾値の設定化、RuntimeConfig一括化、
  doctor MIGRATIONS_DIR診断、REQUEST_TIMEOUT二重読込、REVIEW_REPORT未更新、
  url_decode不正percentテスト、ui_errorテスト仕様化、日本語文言ピリオド統一）は記録のみで保留。

- 2026-11-24 コンボ第75ラウンド（残存high: リロード時セッション復元）:
  起動時にsessionStorageマーカー（oi_session_restore）がある場合のみ/auth/refreshを
  呼び、成功時にtoken/user_hash/viewを復元（URLはReplaceで正規化、last_known_viewも更新）。
  login成功でマーク、logoutで削除。globalSetupの未認証401許可はマーカー導入で
  不要になったため削除。R4指摘3件（epoch照合・last_known_view更新・global-setup根拠）
  も修正。npm audit一時障害は再試行で解消。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  tsc/wasm green、E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: axe WCAG AA自動スキャン導入、globalSetup認証後ビュー監視、
  fail-closedカウンタ2種のテスト・仕様、error_lang UTC例外テスト、jxl fuzz独立オラクル、
  pushState失敗告知、ナビ/LOG OUT/投票の24pxタッチ目標、投票.disabled理由、
  noscript/fallback、apple-touch-icon PNG、公開集計閾値の設定化、RuntimeConfig一括化、
  doctor MIGRATIONS_DIR診断、REQUEST_TIMEOUT二重読込、REVIEW_REPORT未更新、
  url_decode不正percentテスト、ui_errorテスト仕様化）は記録のみで保留。

- 2026-11-24 コンボ第74ラウンド（review/discovery）: 全7担当完了。
  R6 でhigh2件を是正: migration適用済み判定をwrite_lock取得後・tx内SELECTへ移動
  （前回のwrite_lock化がロック外判定を残していた）、CI cronゲートがroot crontab例を
  /etc/cron.d形式として解析し常時失敗する問題（cron-d-exampleマーカー除外）。
  他: refresh再利用検知を常に401＋ファミリ失効失敗カウンタ格下げ、
  logout全失敗経路で削除Cookie付与（R4指摘のis_sub_revokedも修正）、
  README cdサブシェル化、PWAアセット静的検証。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  yaml/tsc OK、E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: リロード時セッション復元、axe WCAG AA自動スキャン導入、
  globalSetup認証後ビュー監視、fail-closedカウンタ2種のテスト・仕様、
  error_lang UTC例外テスト、jxl fuzz独立オラクル、pushState失敗告知、
  ナビ/LOG OUT/投票の24pxタッチ目標、投票.disabled理由、noscript/fallback、
  apple-touch-icon PNG、公開集計閾値の設定化、RuntimeConfig一括化、
  doctor MIGRATIONS_DIR診断、REQUEST_TIMEOUT二重読込、REVIEW_REPORT未更新、
  url_decode不正percentテスト、ui_errorテスト仕様化）は記録のみで保留。

- 2026-11-24 コンボ第73ラウンド（review/discovery）: 全7担当完了。
  R6 でhigh1件を是正: CI Chrome上限140がハードコードでランナー更新時に恒久赤になる問題
  （ci.yml env_varsでMAX_CHROME_MAJOR注入可能化、READMEに上限/更新手順）。
  他: CIキャッシュコメントを実態（未接続・毎回コールド）化、
  バックアップ鮮度.last-success監視手順追記、READMEローカルゲートfail-closed化、
  verified_claimsのnonceをverify前に消費（PoPはpayload.nonce読直し）、
  PWA manifest/favicon/theme-color追加、globalSetupのfavicon許可削除。
  R4指摘0件。全ゲート green（E2E69/check23/doctor54/migrations14/tests156、
  doctor 0 problem(s)、manifest JSON OK、E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: axe WCAG AA自動スキャン導入、globalSetup認証後ビュー監視、
  fail-closedカウンタ2種のテスト・仕様、error_lang UTC例外テスト、
  E2Eブラウザ選択CI fail-closedテスト、jxl fuzz独立オラクル、
  RuntimeConfig一括化、doctor MIGRATIONS_DIR診断、REQUEST_TIMEOUT二重読込、
  restore原子的ロールバック、ContentViewer世代ガード、REVIEW_REPORT未更新、
  url_decode不正percentテスト、ui_errorテスト仕様化、
  英語placeholderのlang付与、ハンドル済みUI失敗テレメトリ）は記録のみで保留。

- 2026-11-24 コンボ第72ラウンド（review/discovery）: 全7担当完了。
  R6 でhigh1件を是正: リモートDBでmigrationがwrite_lockを取らず冪等でない
  ALTER/DROP SQLが並行起動時に二重実行される問題（apply_migrationを
  crate::begin_write_tx経由化、begin_write_txをpub(crate)化）。
  他: back-channel poison格下げ、logoutのsub401/sid失敗もCookie付与、
  backup-rotate Merkle root 64hex厳密化（root抽出+AUDIT_EXPECT_ROOT）、
  OnFailure %Nコメント/TimeoutStopSec根拠修正、REQUEST_TIMEOUT_SECS表記、
  Export 401再ログイン案内、/metrics IPv4 200+security headers回帰、
  deny.tomlコメント/MET-02整合、R4指摘（AUDIT_EXPECT_ROOT側64hex）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  backup-rotate tests OK、E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: axe WCAG AA自動スキャン導入、globalSetup認証後ビュー監視、
  fail-closedカウンタ2種のテスト・仕様、error_lang UTC例外テスト、
  再試行フィードバック、RuntimeConfig一括化、doctor MIGRATIONS_DIR診断、
  REQUEST_TIMEOUT二重読込、restore原子的ロールバック、ContentViewer世代ガード、
  REVIEW_REPORT未更新、url_decode不正percentテスト、ui_errorテスト仕様化、
  英語placeholderのlang付与、ハンドル済みUI失敗テレメトリ）は記録のみで保留。

- 2026-11-24 コンボ第71ラウンド（残存a11y/UX一式）: 2担当（nav/UX）。
  nav landmarkへaria-label+lang、4タブをbuttonからhref付きaへ（clickでprevent_default後に
  navigate）、E2Eセレクタ71箇所をlinkへ更新。error_langをひらがな/カタカナ判定へ強化し
  自由入力への適用を禁止、SHOW STATS 403 dedicated案内+aria-describedby、
  logout文言を再ログイン案内へ、aria-invalid/describedby付与と成功時解除、
  サイズ超過file inputクリア、retry statusを操作案内固定文言へ（二重読み上げ解消）、
  カード24pxタッチ目標、監査見出し初期フォーカス。
  親がNodeRef型名(HtmlHeadingElement→H4)を修正。R4指摘0件。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  wasm/tsc green、server全テスト green、E2E_CLIENT_PORT=18123で実施）。
  残存（次ラウンド: axe WCAG AA自動スキャン導入、再試行フィードバック、
  RuntimeConfig一括化、doctor MIGRATIONS_DIR診断、REQUEST_TIMEOUT二重読込、
  restore原子的ロールバック、ContentViewer世代ガード、バックアップunit network-online、
  REVIEW_REPORT未更新、TimeoutLayer408統合テスト、url_decode不正percentテスト、
  ui_errorテスト仕様化、ハンドル済みUI失敗テレメトリ）は記録のみで保留。

- 2026-11-24 コンボ第69ラウンド（ADR完了後8回目review/discovery）: 全7担当完了。
  高は0件。R6 で以下を是正: global-setup.tsをtsc型検査対象へ追加（型エラーも修正）、
  公開集計の鮮度判定をサーバー時刻(server_now)方式へ移行（旧サーバーは端末時刻へ
  フォールバック、ホワイトリストテスト更新）、pinレスポンス検証失敗時の孤立ピン
  カウンタ計上、retry連打ガード(aria-busy)、DOCUMENT_STATUS DEC-01自己矛盾、
  Trunk watchからinput.css除外（手動build-css明記）。
  R4指摘2件（MOD-13のserver_now記述・DEC-01外部条件文言）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  server全テスト green、tsc green、E2E_CLIENT_PORT=18123で実施）。
  discovery静的は指摘0。残存（次ラウンド: axe WCAG AA自動スキャン導入、
  retry_status二重アナウンス・再試行フィードバック、nav landmark名、タブbutton→a、
  error_lang中国語判定、RuntimeConfig一括化、doctor MIGRATIONS_DIR診断、
  REQUEST_TIMEOUT二重読込、restore原子的ロールバック、ContentViewer世代ガード、
  バックアップunit network-online、REVIEW_REPORT未更新、TimeoutLayer408統合テスト、
  url_decode不正percentテスト、ui_errorテスト仕様化、SHOW STATS 403案内、
  logout文言、aria-invalid関連付け、file inputクリア、ハンドル済みUI失敗テレメトリ）は
  記録のみで保留。

- 2026-11-24 コンボ第68ラウンド（ADR完了後7回目review/discovery）: 全7担当完了。
  R6 でhigh1件を是正: 復元必須ゲートが存在しない.tar.gz/.tar.gz.sigを参照していた問題
  （Runbook/OPERATIONAL_POLICYを実生成物.db.gpg/.db.gpg.asc/.audit-rootの3点世代対へ統一）。
  medium: /metricsへsecurity_headers付与、.env.exampleのMIGRATIONS_DIRをコメントアウト
  （空値起動拒否を回避）、moderation未認証401宛先をE2E_API_BASE/E2E_SERVER_PORTから解決、
  英文role=alertへlang=enアサーション。R4指摘1件（旧gzip表記残存）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  E2E_CLIENT_PORT=18123で実施）。discovery静的は指摘0。
  残存（次ラウンド: retry_status二重アナウンス・再試行フィードバック・nav landmark名・
  タブbutton→a・error_lang中国語判定・RuntimeConfig一括化・doctor MIGRATIONS_DIR診断・
  REQUEST_TIMEOUT二重読込・restore原子的ロールバック・ContentViewer世代ガード・
  公開集計サーバー時刻方式・バックアップunit network-online・REVIEW_REPORT未更新・
  TimeoutLayer408統合テスト・url_decode不正percentテスト・ui_errorテスト仕様化・
  SHOW STATS 403案内・logout文言・aria-invalid関連付け・file inputクリア・
  shfmtゲート名表記・ハンドル済みUI失敗テレメトリ）は記録のみで保留。

- 2026-11-24 コンボ第67ラウンド（ADR完了後6回目review/discovery）: 全7担当完了。
  R6 でhigh2件を是正: 【high】/metricsがIPv4ゲートで403になる問題（ dedicated router
  化、/health /ready は従来どおり）、【high】logout部分失敗でCookie削除が付かない問題
  （ファミリ失効失敗を警告+token_cleanup_failures格下げ、oidc::revoke_refresh_familyを
  pub(crate)化）。medium: cron雛形の有効ジョブ行をコメントアウトし手順と整合、
  CI cronフィールド範囲検証（分0-59/時0-23/日1-31/月1-12/曜0-7、下限も検証）、
  ci-e2e.shへE2E_REUSE=0追加。R4指摘1件（cron下限）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  server全テスト green、E2E_CLIENT_PORT=18123で実施。既定18080がllama-serverと
  衝突した環境要因）。
  discovery静的のOIDC 403/501はADR-0013区分A既知項目として却下。
  残存（次ラウンド: retry_status二重アナウンス・再試行フィードバック・nav landmark名・
  タブbutton→a・error_lang中国語判定・RuntimeConfig一括化・doctor MIGRATIONS_DIR診断・
  REQUEST_TIMEOUT二重読込・restore原子的ロールバック・ContentViewer世代ガード・
  公開集計サーバー時刻方式・バックアップunit network-online・REVIEW_REPORT未更新・
  TimeoutLayer408統合テスト・url_decode不正percentテスト・ui_errorテスト仕様化）は
  記録のみで保留。

- 2026-11-23 コンボ第66ラウンド（ADR完了後5回目review/discovery）: 全7担当完了。
  高は0件。R6 で以下を是正: ContentViewer本文取得401へdedicated再ログイン案内、
  検索条件クリアでdeep_link_cidも除去、OIL_CLIENT_METRICS=0ビルドの
  client_event_payloadテストをフラグ対応（ui_error側と契約統一）、
  doctor-ci-env.shの自己完結化（ALLOW_IPV4_LOCALHOST等unset）、
  READMEの/metrics一覧+公開範囲+prometheus例リンク、
  OPERATIONAL_POLICYのtestid一覧拡張+dashboard-reloginへrole=status、
  globalSetupを選択プロジェクト全エンジン監視へ（chrome固定解消）。
  R4指摘1件（testid一覧と実装の不整合）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  backup-rotate tests OK、npm audit3lockfile、markdownlint35ファイル、
  OIL_CLIENT_METRICS=0 client test green）。
  discovery静的は指摘0。残存（次ラウンド: retry_status二重アナウンス、再試行フィードバック、
  nav landmark名、タブbutton→a、error_lang中国語判定、RuntimeConfig一括化、
  restore原子的ロールバック、ContentViewer世代ガード、公開集計サーバー時刻方式、
  バックアップunit network-online、REVIEW_REPORT未更新、owner不一致テスト、
  license件数不整合テスト、TimeoutLayer408統合テスト、jxl fuzz不感、
  url_decode不正percentテスト、ui_errorテスト仕様化、assert_major_numeric到達不能分岐）は
  記録のみで保留。

- 2026-11-23 コンボ第65ラウンド（ADR完了後4回目review/discovery）: 全7担当完了。
  R6 で以下を是正: 【high】CI cron構文ゲートがSHELL/PATH/MAILTO行をジョブ行と誤判定
  して常時失敗する問題（KEY=VALUE行スキップ）、unban_userのmemory poisonを
  ban_userと対称に警告格下げ、ban_userのトークン消費失敗も警告+専用カウンタへ格下げ
  （再試行での監査重複防止）、unpin補償失敗の専用カウンタ追加、
  deploy/prometheus.yml.example追加（counter resetはrate/increaseで扱う）、
  TEST_SPECIFICATIONのfeature分離「未実施」陳腐化とtodo doctor53→54修正。
  R4指摘1件（文章の句点欠落）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  backup-rotate tests OK、npm audit3lockfile、markdownlint35ファイル）。
  discovery静的のOIDC 403/501・通知/再審査・本人確認監査・脅威モデル等は
  ADR-0013区分A（外部入力依存・方針のみ決定済み）の既知項目として重複却下。
  残存（次ラウンド: retry_status二重アナウンス、再試行フィードバック、nav landmark名、
  タブbutton→a、error_lang中国語判定、globalSetup認証済み画面、RuntimeConfig一括化、
  restore原子的ロールバック、ContentViewer世代ガード、公開集計サーバー時刻方式、
  バックアップunit network-online、REVIEW_REPORT未更新、owner不一致テスト、
  license件数不整合テスト、TimeoutLayer408統合テスト、jxl fuzz不感、
  url_decode不正percentテスト、ui_errorテスト仕様化、都度flask）は記録のみで保留。

- 2026-11-23 コンボ第64ラウンド（ADR完了後3回目review/discovery）: 全7担当完了。
  高は0件、R6 で medium を中心に是正: TimeoutLayerをobserve_metrics内側へ移動し
  408をHTTPメトリクスへ計上、CI extra_gatesの&&連鎖を分離（npm ci失敗の黙飲み防止）、
  web-sys featuresへWindow/Document/EventTarget明示、common chronoを
  default-features=false化（wasmのclock系依存除外）、Runbook/テストを
  オフサイト3点世代対（.audit-root/.db.gpg/.db.gpg.asc）記述へ更新。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  backup-rotate tests OK、npm audit3lockfile、markdownlint39ファイル）。
  残存（次ラウンド: retry_status二重アナウンス、再試行フィードバック、nav landmark名、
  タブbutton→a、error_lang中国語判定、ci-extra-gates抽出、globalSetup認証済み画面、
  RuntimeConfig一括化、OIDC初回登録403/501、restore原子的ロールバック、
  unpin補償メトリクス、Vote disabled理由、公開集計鮮度警告、E2Eコメント行番号、
  5秒スリープ、gpg前提README、Prometheus scrape、カードタッチ目標24px、
  監査初期フォーカス、HANDOFF履歴、DOCUMENT_STATUS ADR反映、
  url_decode不正percentテスト、ui_errorテスト仕様化）は記録のみで保留。

- 2026-11-23 コンボ第63ラウンド（ADR完了後2回目review/discovery）: 全7担当完了。
  R6 で以下を是正: 【high】ui_errorがサーバ側ホワイトリストで全件400拒否されていた問題
  （ui_error受理+専用カウンタopenillustration_client_ui_errors_total+回帰テスト）、
  unhandledrejectionリスナ追加（spawn_localの非同期例外を件数だけ計測）、
  アプリunitへStartLimitIntervalSec/Burst+Environment順序+Documentation修正、
  docsの機能検出旧記述をADR-0013のビルド時確定方針へ是正、OPERATIONAL_POLICYへ
  ui_errorと2カウンタ明記、ci-extra-gates抽出/globalSetup認証/issuesは次ラウンド以降。
  R4指摘1件（server側ui_error回帰テスト）も修正。
  親が/tmp枯渇（過去ラウンドの一時target 17G）を解消して全ゲート green。
  E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)。
  残存（次ラウンド: ci-extra-gates抽出、globalSetup認証済み画面監視、
  TimeoutLayer外側メトリクス、RuntimeConfig一括化、OIDC初回登録403/501、
  restore原子的ロールバック、unpin補償メトリクス、Vote disabled理由、
  公開集計鮮度警告、E2Eコメント行番号、5秒スリープ、gpg前提README、
  Prometheus scrape、カードタッチ目標24px、監査初期フォーカス、
  HANDOFF履歴、DOCUMENT_STATUS ADR反映）は記録のみで保留。

- 2026-11-23 コンボ第62ラウンド（ADR完了後初回review/discovery）: 全7担当完了。
  R6 で以下を是正: オフサイト転送へdetached署名(.asc)追加（3点世代対）、
  アプリ/backup両unitへOnFailure、backup_db VACUUM INTO TOCTOU保護、
  ban_userメモリpoison警告格下げ+トークン消費、migration ON CONFLICT冪等化、
  export generated_atをtx開始直後へ、決済warn固定文言、instance.lock失敗時削除、
  doctorのBIND_ADDR/PORT注入修正、ContentViewer再試行、検索クリア後フォーカス、
  retry_statusのlangを本文から算出、ADR-0013 Status=Implemented反映、
  JXL仕様60M/rename記述削除/REQUEST_TIMEOUT_SECS/.env/空セクション削除。
  R4指摘1件（retry_status lang）も修正。親がE2E moderation strict mode 1件を修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  backup-rotate tests OK、npm audit3lockfile、markdownlint40ファイル）。
  残存（design-wait区分A・外部入力依存: JPKI信頼アンカー/OIDC azp/第三者監査アンカー/
  JPYC返金dispute/脅威モデル 等は方針決定済みで記録のみ）、
  新規検出の低優先（RuntimeConfig一括化・OIDC初回登録403/501・restore原子的ロールバック・
  unpin補償メトリクス・Voteボタンdisabled理由・公開集計鮮度警告・E2Eコメント行番号・
  5秒スリープ・gpg前提README・Prometheus scrape設定例）は次ラウンド以降。

- 2026-11-23 コンボ第61ラウンド（ADR-0013 P4 実装・全区分完了）: 3担当（server/client/deps）。
  TimeoutLayer+REQUEST_TIMEOUT_SECS、tokio/tower/reqwest features絞り、workspace.dependencies集約、
  common RSA feature分割（client wasmからRSA除外）、監査non-modal Tab、Support overlay、
  再試行導線4画面+429文言、投票一覧401/403、ログイン403 IPv6、日時lang、JXL MAX_PIXELS=60M、
  Blobエラー分類、ui_error、console.error/pageerror globalSetup、metrics Cookie、
  失効閾値警告格下げ、logout Cookie、finalizer残件計上、refresh tx内、
  back-channel再送、rows drop、DB_WRITE_LOCKSパスキー。
  親がclippy2件・common cfg化・E2E strict mode3件・doctor request_timeout診断・
  コメント陳腐化・reqwest workspace指定・R4指摘3件を修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  client89/server492、npm audit3lockfile、markdownlint40ファイル、wasm green、RSA除外確認）。
  ADR-0013 の P1（設定）P2（バックアップ）P3（CI）P4（ランタイム/UX）がすべて実装完了。
  以降は通常の review/discovery サイクルへ戻り、残存する設計判断待ち（区分A：外部入力依存）
  は方針のみ決定済みとして記録を維持する。

- 2026-11-23 コンボ第60ラウンド（ADR-0013 P3 実装）: 3担当（E2E/CIクライアント/CI）。
  ERR trap段階名付き::error::、playwright install 3回再試行、MAX_CHROME_MAJOR=140 fail-closed、
  E2E_MOBILE opt-in、E2E_SERVER_PORT/E2E_API_BASE、build-gate境界テスト、
  client/build.rs rerun-if-env-changed、hash/view/sub境界/NULL列テスト、
  CIバイナリキャッシュ+--version検証、go不在時ダウンロード、systemd/cron構文ゲート、
  CLI-02群テーブル3件+双方向照合、ライセンス本文数動的表現。
  親がshellcheck SC2016、export_status_details配列fail-closed、doc-numbers据え置き、
  R4指摘2件（早期失敗::error::・Chrome上限ドキュメント）を修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  client87/server491/doctor54 green、npm audit3lockfile、markdownlint40ファイル）。
  cargo deny advisory DBのSSL一時障害は再試行で解消。
  次は ADR-0013 P4（ランタイム保護／UX）へ進む。

- 2026-11-23 コンボ第59ラウンド（ADR-0013 P2 実装）: 3担当（backup/server/docs）。
  .last-successハートビート+cron鮮度チェック/timeout1800+flock、オフサイト順序
  （audit root→成果物）+部分失敗警告、detached署名(.asc)生成、audit root 64hex厳密化、
  sign_audit_exportのrename廃止(hard_linkのみ)、DB symlink拒否+canonicalizeロック、
  exportファイル名へgenerated_at由来UTC世代、RPO24h/RTO4h+GPG trust+署名検証必須。
  親がdoctor 128hex化(置換連鎖)/clippy needless_borrow/署名ファイル名パス露出/
  doc-numbers件数(31/15/46)/Runbook .sig→.asc を修正。
  R4指摘2件も修正。全ゲート green（E2E69/check23/doctor54/migrations14/tests156、
  doctor 0 problem(s)、backup-rotate 46ケース、npm audit3lockfile、markdownlint40ファイル）。
  次は ADR-0013 P3（テスト／CI の信頼性）へ進む。

- 2026-11-23 コンボ第58ラウンド（ADR-0013 P1 実装）: 3担当（server設定一式/migration/deploy+docs）。
  BIND_ADDR/PORT（既定[::]:3000・不正/特権/範囲外fail-closed・doctor production診断）、
  RUST_LOG不正値警告、PINATA_JWTのAppState起動時確定、数値env範囲外警告（7変数）、
  MIGRATIONS_DIR override（絶対パス・連番fail-closed）、
  アプリsystemd unit + alert unit雛形、NTP必須/HSTSプロキシ責任/OnFailure役割分担を実装。
  親がclippy一時値寿命エラー、R4指摘4件（.env.exampleのBIND_ADDR=[::]、doctor診断、
  MIGRATIONS_DIR記載、陳腐化した「アプリunitなし」記述）を修正。
  R4残1件（数値envのdoctor共有判定）は範囲定数共有の設計変更を伴うため次ラウンド以降。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  npm audit3lockfile、markdownlint40ファイル）。
  ※ ゴール maxGoalRounds を 5000 へ変更（ユーザー明示指示）。

- 2026-11-23 設計判断委譲: ユーザーより「設計判断待ちの箇所は推奨するものを決めて良い」と
  委譲された。docs/adr/ADR-0013-deferred-decisions.md を追加し、外部输入端依存（A）、
  プロジェクト内で決定し実装する P1〜P4、意図的に実装しない（C）を整理。
  P1=設定/運用の単一情報源、P2=バックアップ復旧、P3=テスト/CI信頼性、P4=ランタイム/UX。
  以降のラウンドで P1 から順に実装する。
  （ラウンド57の回帰オラクル3件はコミット a794cd8 済み）

- 2026-11-23 コンボ第57ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  ban_user失効閾値max単調化の回帰テスト、drain timeoutシグナル後限定ロジックを
  await_server_with_drain_timeoutヘルパへ抽出+回帰テスト、中立化URLキーへscope導入の
  純粋関数化+テスト+仕様更新。
  ※ ユーザーより「設計判断待ちの箇所は推奨方針を決めてよい」との委譲を受けた。
  次ラウンド以降で設計判断待ちリストをカテゴリ別に整理し方針決定＋実装を進める。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、server487/client85、
  doctor 0 problem(s)、npm audit3lockfile、markdownlint39ファイル）。
  設計判断待ち（これまでの全項目。ユーザー委譲により方針決定対象へ移行）は
  docs/adr/ADR-0012-design-decisions.md で整理する予定。

- 2026-11-23 コンボ第56ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  鍵ローテーションRunbook段階1を「旧公開鍵登録不要（現行署名鍵kidは自動登録、同kid登録は
  duplicateで起動拒否）」へ修正（見出し/段階2/切り分け表も含む）、
  ban_userのrevoked_subsメモリ更新をmax単調化、web-sys featuresへ直接使用型6つ明記、
  DOCUMENT_STATUS generated.at更新、README Chrome120/npm audit追記、
  OPERATIONAL_POLICYのnpm audit記述を実態へ、AUDIT_EXPORT_RUNBOOK 3手順へdatabase-url。
  R4指摘4件（鍵Runbook残り3箇所・npm audit記述）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  npm audit3lockfile、markdownlint39ファイル）。npm audit一時障害は再試行で解消。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、E2E本番相当昇格、
  Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト/TimeoutLayer、E2E artifact公開、
  GPG署名、RPO/RTO定義、バックアップ鮮度監視/ハートビート、trunk部分アーカイブ、
  cronタイムアウト/flock、GPG trust、cron allowlistログキー、license_file相対パス、
  BACKUP_PREFIX検証、axe自動スキャン、開発モックuser_hash衝突、再試行導線、
  クライアント例外テレメトリ、refresh失敗メトリクス、playwright install再試行、
  window unit停止済みアプリ再起動、監査スクロール/non-modal Tab、E2Eビルド::error::、
  reqwest TLS二重スタック、tokio/tower features、CIバイナリキャッシュ、
  失効閾値削除失敗警告格下げ、logout sub不一致Cookie削除、finalizer BEGIN失敗計上、
  NTP同期、ブラウザエラー観測、429/503再試行、JXLアニメーション/MAX_PIXELS、
  日時フォールバック/鮮度/端末時計、cron設定集約、audit root 64hex、ライセンス本文数、
  アーカイブ寿命、exportファイル名世代、real-api宛先env化、deny.toml copyleft/bans skip、
  R2-REFRESH-ORIGIN E2E、systemd/cron構文CIゲート、backup.service OnFailure、
  E2Eセットアップ::error::、署名公開rename TOCTOU、IPFSチェックサムフェイルオーバー、
  429専用文言、nonce取得失敗上書き、aria-describedby/aria-invalid、Audit/dialog lang、
  export sub 8文字以下、Blob/URL生成エラー、common RSA推移依存、HSTS、日時locale、
  DBシンボリックパス、オフサイト2回コピー世代対、投票一覧401/403、console.error/pageerror、
  CLI-02群テーブル欠落、metrics Cookie、E2E_SERVER_PORT、価格指数表記、RUST_LOG不正値、
  DB_WRITE_LOCKSポインタキー、mod_report/mod_vote rows drop、refresh sub失効競合、
  back-channel logout部分失敗再送、モバイルviewport、ログイン403 IPv4案内、
  バックアップ未実行検出、hash/viewルーティング単体テスト、verify_trunk_archive入力検証、
  step summary異常系、assert_major_numeric境界、数値env範囲外警告、監査CLI NULL列、Support overlay tabindex）は記録のみで保留。

- 2026-11-23 コンボ第55ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  verify_orderの1段目/2段目CASへstatus<>'MISMATCH'を追加し不一致確定の終端保護+
  回帰テスト（test_payment_verify_mismatch_is_terminal）、OPERATIONAL_POLICYの
  DATABASE_TOKEN契約（3CLIのみ使用）と欠落5変数、ブラウザ順序（選定→導入→実行）を
  TEST_SPECIFICATION/OPERATIONAL_POLICYへ反映、doctor-ci-env.shをCI前提条件へ記載。
  R4指摘2件（CAS保護の実適用漏れ・順序記述の残り3箇所）も修正。
  親が複数回の置換失敗（cas1の出現数/ヘルパ名）を解消。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  npm audit3lockfile、markdownlint39ファイル、payment21 green）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、E2E本番相当昇格、
  Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト/TimeoutLayer、E2E artifact公開、
  GPG署名、RPO/RTO定義、バックアップ鮮度監視/ハートビート、trunk部分アーカイブ、
  cronタイムアウト/flock、GPG trust、cron allowlistログキー、license_file相対パス、
  BACKUP_PREFIX検証、axe自動スキャン、開発モックuser_hash衝突、再試行導線、
  クライアント例外テレメトリ、refresh失敗メトリクス、playwright install再試行、
  window unit停止済みアプリ再起動、監査スクロール/non-modal Tab、E2Eビルド::error::、
  reqwest TLS二重スタック、tokio/tower features、CIバイナリキャッシュ、
  失効閾値削除失敗警告格下げ、logout sub不一致Cookie削除、finalizer BEGIN失敗計上、
  NTP同期、ブラウザエラー観測、429/503再試行、JXLアニメーション/MAX_PIXELS、
  日時フォールバック/鮮度/端末時計、cron設定集約、audit root 64hex、ライセンス本文数、
  アーカイブ寿命、exportファイル名世代、real-api宛先env化、deny.toml copyleft/bans skip、
  R2-REFRESH-ORIGIN E2E、systemd/cron構文CIゲート、backup.service OnFailure、
  E2Eセットアップ::error::、署名公開rename TOCTOU、IPFSチェックサムフェイルオーバー、
  429専用文言、nonce取得失敗上書き、aria-describedby/aria-invalid、Audit/dialog lang、
  export sub 8文字以下、Blob/URL生成エラー、common RSA推移依存、HSTS、日時locale、
  DBシンボリックパス、オフサイト2回コピー世代対、投票一覧401/403、console.error/pageerror、
  CLI-02群テーブル欠落、metrics Cookie、E2E_SERVER_PORT、価格指数表記、RUST_LOG不正値、
  DB_WRITE_LOCKSポインタキー、mod_report/mod_vote rows drop、refresh sub失効競合、
  back-channel logout部分失敗再送、モバイルviewport、ログイン403 IPv4案内、
  バックアップ未実行検出、hash/viewルーティング単体テスト、
  verify_trunk_archive入力検証、step summary異常系、assert_major_numeric境界）は記録のみで保留。

- 2026-11-23 コンボ第54ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  【critical】graceful drain timeoutをserver全体ではなくshutdown watch receipt後にのみ適用
  （シグナル無し30秒でプロセスが落ちる問題を修正、Box::pin+select!で実装）、
  【high】OIDC保証レベル4変数(ACR/AMR/SCOPES/CLOCK_SKEW)のNonUtf8を起動拒否へ、
  MOD-14をDOCUMENT_STATUS/TASK_INVENTORY/DETAILED_DESIGN/HANDOFFへ反映。
  R4指摘2件（TEST_SPECIFICATIONのfail-closed記載・NonUtf8回帰オラクル）も修正。
  親がselect!のmoveエラー/型エラー/clippy manual_containsを解消。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、doctor 0 problem(s)、
  npm audit3lockfile、markdownlint39ファイル、server485/OIDC green）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、E2E本番相当昇格、
  Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト/TimeoutLayer、E2E artifact公開、
  GPG署名、RPO/RTO定義、バックアップ鮮度監視/ハートビート、trunk部分アーカイブ、
  cronタイムアウト/flock、GPG trust、cron allowlistログキー、license_file相対パス、
  BACKUP_PREFIX検証、axe自動スキャン、開発モックuser_hash衝突、再試行導線、
  クライアント例外テレメトリ、refresh失敗メトリクス、playwright install再試行、
  window unit停止済みアプリ再起動、監査スクロール到達/non-modal Tab統一、
  E2Eビルド::error::、reqwest TLS二重スタック、tokio/tower features、CIバイナリキャッシュ、
  失効閾値削除失敗の警告格下げ、logout sub不一致時Cookie削除、finalizer BEGIN失敗計上、
  NTP同期要件、ブラウザエラー観測、429/503再試行、JXLアニメーション/MAX_PIXELS、
  日時フォールバック/鮮度NaNオラクル/オプイン集計鮮度/端末時計依存、cron設定集約、
  audit root 64hex検証、ライセンス本文件数照合、アーカイブ保持寿命、exportファイル名世代、
  real-api宛先env化、deny.toml copyleft方針/bans skip副作用、R2-REFRESH-ORIGIN E2E、
  systemd/cron構文CIゲート、backup.service OnFailure、E2Eセットアップ失敗::error::、
  署名公開rename TOCTOU、IPFSチェックサム不一致時フェイルオーバー、429専用文言、
  nonce取得失敗の上書き、aria-describedby/aria-invalid、Auditボタン/dialog lang、
  export sub 8文字以下契約、Blob/URL生成エラー分岐、common RSA推移依存、HSTS方針、
  日時locale固定、DBシンボリックパスのinstance.lock回避、オフサイト2回コピーの世代対中途失敗、
  投票一覧401/403分岐、console.error/pageerror監視、CLI-02群テーブル3件欠落、
  metrics Cookie非含有オラクル、E2E_SERVER_PORT、価格入力指数表記、RUST_LOG不正値警告、
  DB_WRITE_LOCKSポインタキー、mod_report/mod_vote rows drop、refresh sub失効競合、
  back-channel logout部分失敗再送、モバイルviewport E2E、ログイン403 IPv4案内、
  バックアップ鮮度未実行検出、hash/viewルーティング単体テスト）は記録のみで保留。

- 2026-11-23 コンボ第53ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  workspace resolver 2→3（edition 2024/MSRV1.88と整合）、
  MAX_BODY_BYTESをモジュール定数化しヘッダ検査とDefaultBodyLimitの単一情報源化、
  中立化URL送信済みキーへCID由来scopeを組み込み同一本文別コンテンツの過小計上を防止、
  月次集計/LEDGERへth scope=row、prepare-distribution権限コメントを実動作へ。
  親が非wasmスタブのdead_code/unusedを解消。
  R4 指摘なし。全ゲート green（E2E69/check23/doctor54/migrations14/tests156、
  doctor 0 problem(s)、npm audit3lockfile、markdownlint39ファイル、resolver3）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、E2E本番相当昇格、
  Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト/TimeoutLayer、E2E artifact公開、
  GPG署名、RPO/RTO定義、バックアップ鮮度監視/ハートビート、trunk部分アーカイブ、
  cronタイムアウト/flock、GPG trust、cron allowlistログキー、license_file相対パス、
  BACKUP_PREFIX検証、axe自動スキャン、開発モックuser_hash衝突、再試行導線、
  クライアント例外テレメトリ、refresh失敗メトリクス、playwright install再試行、
  window unit停止済みアプリ再起動、監査スクロール到達/non-modal Tab統一、
  E2Eビルド::error::、reqwest TLS二重スタック、tokio/tower features、CIバイナリキャッシュ、
  失効閾値削除失敗の警告格下げ、logout sub不一致時Cookie削除、finalizer BEGIN失敗計上、
  NTP同期要件、ブラウザエラー観測、429/503再試行、JXLアニメーション/MAX_PIXELS、
  日時フォールバック/鮮度NaNオラクル/オプイン集計鮮度/端末時計依存、cron設定集約、
  audit root 64hex検証、ライセンス本文件数照合、アーカイブ保持寿命、exportファイル名世代、
  real-api宛先env化、deny.toml copyleft方針/bans skip副作用、R2-REFRESH-ORIGIN E2E、
  systemd/cron構文CIゲート、backup.service OnFailure、E2Eセットアップ失敗::error::、
  署名公開rename TOCTOU、IPFSチェックサム不一致時フェイルオーバー、429専用文言、
  nonce取得失敗の上書き、aria-describedby/aria-invalid、Auditボタン/dialog lang、
  export sub 8文字以下契約、Blob/URL生成エラー分岐、common RSA推移依存、HSTS方針、
  日時locale固定、DBシンボリックパスのinstance.lock回避、オフサイト2回コピーの世代対中途失敗、
  投票一覧401/403分岐、console.error/pageerror監視、CLI-02群テーブル3件欠落、
  metrics Cookie非含有オラクル、E2E_SERVER_PORT、価格入力指数表記、
  RUST_LOG不正値警告、DB_WRITE_LOCKSポインタキー、mod_report/mod_vote rows drop）は記録のみで保留。

- 2026-11-23 コンボ第52ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  【high】doctor --strictをCIゲート化（ダミー本番envをscripts/doctor-ci-env.shへ、
  後続モックE2Eへの漏れを防ぐためサブシェル隔離）、Moderation finalizerの
  read-then-BEGIN順序、backup_db部分バックアップ削除失敗のエラー集約。
  親が初回全ゲートでdoctor用envがE2Eへ漏れて失敗したことを検出し修正。
  R4 指摘なし。全ゲート green（E2E69/check23/doctor54/migrations14/tests156、
  doctor 23 check(s) 0 problem(s)、npm audit3lockfile、markdownlint39ファイル）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、E2E本番相当昇格、
  Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト、E2E artifact公開、GPG署名、
  RPO/RTO定義、バックアップ鮮度監視/ハートビート、trunk部分アーカイブ、cronタイムアウト/
  flock、GPG trust、cron allowlistログキー、license_file相対パス、BACKUP_PREFIX検証、
  axe自動スキャン、開発モックuser_hash衝突、再試行導線、クライアント例外テレメトリ、
  refresh失敗メトリクス、playwright install再試行、window unit停止済みアプリ再起動、
  監査スクロール到達/non-modal Tab統一、E2Eビルド::error::、reqwest TLS二重スタック、
  tokio/tower features、resolver3、CIバイナリキャッシュ、失効閾値削除失敗の警告格下げ、
  logout sub不一致時Cookie削除、finalizer BEGIN失敗計上、NTP同期要件、ブラウザエラー観測、
  429/503再試行、JXLアニメーション/MAX_PIXELS、日時フォールバック/鮮度NaNオラクル/
  オプイン集計鮮度、cron設定集約、audit root 64hex検証（既存16hexテストと互換検討）、
  ライセンス本文件数照合、アーカイブ保持寿命、exportファイル名世代、real-api宛先env化、
  deny.toml copyleft方針/bans skip副作用、R2-REFRESH-ORIGIN E2E、systemd/cron構文CIゲート、
  backup.service OnFailure、E2Eセットアップ失敗::error::、署名公開rename TOCTOU、
  IPFSチェックサム不一致時フェイルオーバー、429専用文言、nonce取得失敗の上書き、
  aria-describedby/aria-invalid、Auditボタン/dialog lang、export sub 8文字以下契約、
  Blob/URL生成エラー分岐、common RSA推移依存、HSTS方針、日時locale固定、
  DBシンボリックパスのinstance.lock回避、オフサイト2回コピーの世代対中途失敗、
  投票一覧401/403分岐、console.error/pageerror監視、CLI-02群テーブル3件欠落、
  metrics Cookie非含有オラクル、E2E_SERVER_PORT、価格入力指数表記、
  AUDIT_EXPECT_ROOT 64hex、RUST_LOG不正値警告、DB_WRITE_LOCKSポインタキー）は記録のみで保留。

- 2026-11-23 コンボ第51ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  【high】ALLOW_MOCK_AUTH=1と本番相当設定の併用を起動拒否（doctorと共通判定、
  任意user_hashのJWT発行を防止）、EXPOSE_INTERNAL_ERRORSのper-request env読みを
  AppState/task-local注入へ移行、loopback代替表現(2130706433/0x7f000001/[::]/localhost.)
  をclient/server双方で拒否、reduced-motion対応、検証バッジrole=status、
  AGE_TRUSTED_ISSUERSを.env.exampleへ追記。
  親がtask_local直上のdocコメント2件を通常コメント化してclippy green、
  R4指摘（server側loopback代替表現）を修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、npm audit3lockfile、
  markdownlint39ファイル）。npm auditのSSL一時障害は再試行で解消。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  E2E本番相当昇格、Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト、E2E artifact公開、GPG署名、
  RPO/RTO定義、バックアップ鮮度監視/ハートビート、trunk部分アーカイブ、cronタイムアウト/
  flock、GPG trust、cron allowlistログキー、license_file相対パス、BACKUP_PREFIX検証、
  axe自動スキャン、開発モックuser_hash衝突、再試行導線、クライアント例外テレメトリ、
  refresh失敗メトリクス、playwright install再試行、window unit停止済みアプリ再起動、
  監査スクロール到達/non-modal Tab統一、E2Eビルド::error::、reqwest TLS二重スタック、
  tokio features full、CIバイナリキャッシュ、失効閾値削除失敗の警告格下げ、
  logout sub不一致時Cookie削除、finalizer BEGIN失敗計上、NTP同期要件、ブラウザエラー観測、
  429/503再試行、JXLアニメーション/MAX_PIXELS、日時フォールバック/鮮度NaNオラクル/
  オプイン集計鮮度、cron設定集約、audit root 64hex検証、ライセンス本文件数照合、
  アーカイブ保持寿命、exportファイル名世代、real-api宛先env化、resolver3、
  deny.toml copyleft方針/bans skip副作用、R2-REFRESH-ORIGIN E2E、systemd/cron構文CIゲート、
  backup.service OnFailure、E2Eセットアップ失敗::error::、署名公開rename TOCTOU、
  IPFSチェックサム不一致時フェイルオーバー、429専用文言、nonce取得失敗の上書き、
  aria-describedby/aria-invalid、Auditボタン/dialog lang、export sub 8文字以下契約、
  Blob/URL生成エラー分岐、common RSA推移依存、HSTS方針、日時locale固定、
  DBシンボリックパスのinstance.lock回避、オフサイト2回コピーの世代対中途失敗）は記録のみで保留。

- 2026-11-23 コンボ第50ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  オフサイト転送へ同世代.audit-rootを含め欠落時fail-closed+回帰テスト/Runbook、
  Totalsラベル/投票キューReasonへlang、README/HANDOFFへCI再現ゲート、
  TASK_INVENTORY descriptionを本文HEADと整合。
  R4 指摘なし。全ゲート green（E2E69/check23/doctor54/migrations14/tests156、
  npm audit3lockfile、markdownlint39ファイル、backup-rotate tests OK）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  E2E本番相当昇格、Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト、E2E artifact公開、GPG署名、
  RPO/RTO定義、バックアップ鮮度監視/ハートビート、trunk部分アーカイブ、cronタイムアウト/
  flock、GPG trust、cron allowlistログキー、license_file相対パス、BACKUP_PREFIX検証、
  axe自動スキャン、開発モックuser_hash衝突、再試行導線、クライアント例外テレメトリ、
  refresh失敗メトリクス、playwright install再試行、window unit停止済みアプリ再起動、
  監査スクロール到達/non-modal Tab統一、E2Eビルド::error::、reqwest TLS二重スタック、
  tokio features full、CIバイナリキャッシュ、失効閾値削除失敗の警告格下げ、
  logout sub不一致時Cookie削除、finalizer BEGIN失敗計上、NTP同期要件、ブラウザエラー観測、
  429/503再試行、prefers-reduced-motion、JXLアニメーション/MAX_PIXELS、
  日時フォールバック/鮮度NaNオラクル/オプイン集計鮮度、cron設定集約、audit root 64hex検証、
  ライセンス本文件数照合、アーカイブ保持寿命、exportファイル名世代、real-api宛先env化、
  resolver3、deny.toml copylet方針/bans skip副作用、R2-REFRESH-ORIGIN E2E、
  systemd/cron構文CIゲート、backup.service OnFailure、E2Eセットアップ失敗::error::、
  署名公開rename TOCTOU、IPFSチェックサム不一致時フェイルオーバー、429専用文言、
  nonce取得失敗の上書き、aria-describedby/aria-invalid、Auditボタン/dialog lang、
  export sub 8文字以下契約、Blob/URL生成エラー分岐、common RSA推移依存、HSTS方針、
  日時locale固定）は記録のみで保留。

- 2026-11-23 コンボ第49ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  E2E_PROJECTSの未知値/空要素（bogus/,/chromium,）をfail-closedで拒否（親がexit 1実測）、
  BACKUP_DIR自体のシンボリックリンク拒否、TEST_SPECIFICATIONへE2E選択fail-closed記載。
  R4指摘（空要素すり抜け）も修正。
  却下: アップロード失敗時のstatusクリアは既存E2E（R1-14がupload-status存在を期待）と
  衝突したため取り消し（Runtime Proof優先、根拠記録）。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、npm audit3lockfile、
  markdownlint39ファイル、backup-rotate tests OK）。cargo deny advisory DB取得のSSL一時障害は再試行で解消。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  E2E本番相当昇格、Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト、E2E artifact公開、GPG署名、
  RPO/RTO定義、バックアップ鮮度監視、trunk部分アーカイブ、cronタイムアウト、GPG trust、
  cron allowlistログキー、license_file相対パス、BACKUP_PREFIX検証、axe自動スキャン、
  開発モックuser_hash衝突、再試行導線、クライアント例外テレメトリ、refresh失敗メトリクス、
  playwright install再試行、window unit停止済みアプリ再起動、監査スクロール到達、
  E2Eビルド::error::、reqwest TLS二重スタック、tokio features full、CIバイナリキャッシュ、
  失効閾値削除失敗の警告格下げ、logout sub不一致時Cookie削除、finalizer BEGIN失敗計上、
  NTP同期要件、ブラウザエラー観測、429/503再試行、prefers-reduced-motion、
  JXLアニメーション/MAX_PIXELS、日時フォールバック/鮮度NaNのオラクル、cron設定集約、
  audit root 64hex検証、ライセンス本文件数照合、アーカイブ保持寿命、exportファイル名世代、
  real-api宛先のenv化、resolver3、deny.toml copyleft方針/bans skip副作用、
  R2-REFRESH-ORIGIN E2E、systemd/cron構文CIゲート、オフサイトMonitor.Missing.rootコピー、
  backup.service OnFailure、E2Eセットアップ失敗の::error::、署名公開rename TOCTOU、
  IPFSチェックサム不一致時フェイルオーバー、429専用文言、nonce取得失敗の上書き、
  aria-describedby/aria-invalid、Auditボタン/dialog lang、export sub 8文字以下契約、
  Blob/URL生成エラー分岐、common RSA推移依存、HSTS方針、 upload status失敗時クリア（契約衝突で却下）、
  監査パネルnon-modal Tab、投票キュー再取得、READMEローカル起動手順）は記録のみで保留。

- 2026-11-23 コンボ第48ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  ban_user/unban_user/record_unpinのread-then-write順序統一（rows drop）、
  エクスポート失敗時に前回成功の生成日時/subをクリア、
  todo.mdのENABLE_AUTO_REFRESH記述をコード/仕様と整合（true反転済み）。
  R4 指摘なし。全ゲート green（E2E69/check23/doctor54/migrations14/tests156、
  npm audit3lockfile、markdownlint39ファイル）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  E2E本番相当昇格、Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト、E2E artifact公開、GPG署名、
  RPO/RTO定義、バックアップ鮮度監視、trunk部分アーカイブ、cronタイムアウト、GPG trust、
  cron allowlistログキー、license_file相対パス、BACKUP_PREFIX検証、axe自動スキャン、
  開発モックuser_hash衝突、再試行導線、クライアント例外テレメトリ、refresh失敗メトリクス、
  playwright install再試行、window unit停止済みアプリ再起動、監査スクロール到達、
  E2Eビルド::error::、reqwest TLS二重スタック、tokio features full、CIバイナリキャッシュ、
  失効閾値削除失敗の警告格下げ、logout sub不一致時Cookie削除、finalizer BEGIN失敗計上、
  NTP同期要件、ブラウザエラー観測、429/503再試行、prefers-reduced-motion、
  JXLアニメーション/MAX_PIXELS、日時フォールバック/鮮度NaNのオラクル、cron設定集約、
  audit root 64hex検証、ライセンス本文件数照合、アーカイブ保持寿命、exportファイル名世代、
  real-api宛先のenv化、resolver3、deny.toml copyleft方針/bans skip副作用、
  R2-REFRESH-ORIGIN E2E、systemd/cron構文CIゲート、オフサイトMonitor.Missing.rootコピー、
  backup.service OnFailure、E2Eセットアップ失敗の::error::、署名公開rename TOCTOU、
  IPFSチェックサム不一致時フェイルオーバー、429専用文言、nonce取得失敗の上書き、
  aria-describedby/aria-invalid、Auditボタン/dialog lang、export sub 8文字以下契約、
  Blob/URL生成エラー分岐）は記録のみで保留。

- 2026-11-23 コンボ第47ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  エクスポート/アップロードへclaim_submission同期ガード+BusyGuard（二重送信防止、
  exportへaria-busy）、セッションJWTのnbf検証を明示有効+アサーション、
  ビルド時フラグ契約（0/false大小文字無視・サーバfail-closedとの差）をdocs/コメントへ反映、
  TEST_SPECIFICATIONのdoctor内訳（LOG_FORMAT1件/add設定判定43件）、
  SECURITY_ADVISORIES §4.3のimage系依存を履歴・削除済みと明記。
  R4指摘2件（fail-closed差の注記/OIL_CLIENT_METRICS大小文字無視）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、npm audit3lockfile、
  markdownlint39ファイル）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  E2E本番相当昇格、Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト、E2E artifact公開、GPG署名、
  RPO/RTO定義、バックアップ鮮度監視、trunk部分アーカイブ、cronタイムアウト、GPG trust、
  cron allowlistログキー、license_file相対パス、BACKUP_PREFIX検証、axe自動スキャン、
  開発モックuser_hash衝突、再試行導線、クライアント例外テレメトリ、refresh失敗メトリクス、
  playwright install再試行、window unit停止済みアプリ再起動、監査スクロール到達、
  E2Eビルド::error::、reqwest TLS二重スタック、tokio features full、CIバイナリキャッシュ、
  失効閾値削除失敗の警告格下げ、logout sub不一致時Cookie削除、finalizer BEGIN失敗計上、
  NTP同期要件、ブラウザエラー観測、429/503再試行、prefers-reduced-motion、
  JXLアニメーション/MAX_PIXELS、日時フォールバック/鮮度NaNのオラクル、cron設定集約、
  audit root 64hex検証、ライセンス本文件数照合、アーカイブ保持寿命、exportファイル名世代、
  real-api宛先のenv化、resolver3、deny.toml copyleft方針、R2-REFRESH-ORIGIN E2E、
  systemd/cron構文CIゲート）は記録のみで保留。

- 2026-11-23 コンボ第46ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  ローカルnpm testの既定をchromiumのみへ（E2E_PROJECTS opt-in、CI時は3プロジェクト登録で
  ci-e2e.shの--project選択と整合）、READMEのE2E前提を実態へ、
  .env.exampleへビルド時/CLI変数追記、restore_dbの部分コピー削除失敗を元エラーへ集約。
  R4 high指摘（CIでfirefox/webkit未登録になりProject not found）を修正し、
  CI=1で207件/--project=firefoxで69件が解決することを実測確認。
  R4 low指摘（restore_db削除失敗のテスト）は記録のみで保留。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、npm audit3lockfile、
  markdownlint39ファイル）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  E2E本番相当昇格、Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト、E2E artifact公開、GPG署名、
  RPO/RTO定義、バックアップ鮮度監視、trunk部分アーカイブ、cronタイムアウト、GPG trust、
  cron allowlistログキー、license_file相対パス、BACKUP_PREFIX検証、axe自動スキャン、
  開発モックuser_hash衝突、再試行導線、クライアント例外テレメトリ、refresh失敗メトリクス、
  playwright install再試行、window unit停止済みアプリ再起動、監査スクロール到達、
  E2Eビルド::error::、reqwest TLS二重スタック、tokio features full、CIバイナリキャッシュ、
  失効閾値削除失敗の警告格下げ、logout sub不一致時Cookie削除、finalizer BEGIN失敗計上、
  NTP同期要件、ブラウザエラー観測、429/503再試行、prefers-reduced-motion、
  JXLアニメーション/MAX_PIXELS、日時フォールバック/鮮度NaNのオラクル、cron設定集約、
  audit root 64hex検証、ライセンス本文件数照合、アーカイブ保持寿命、exportファイル名世代、
  real-api宛先のenv化、resolver3）は記録のみで保留。

- 2026-11-23 コンボ第45ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  JXLのchannels=0を明示拒否+NaN/±infのsaturating castオラクル、
  auth_verifyのMOCK_NONCE消費を年齢判定より前に移動（拒否時も単回消費）、
  署名済みcountryをISO 3166-1 alpha-2（2文字大文字）のみ受理し不正はfail-closed、
  日時表示へ「(協定世界時)」付与（ISOのTを残さない既存契約維持）。
  R4指摘2件（nonce/Countryの回帰オラクル）も既存テスト内へ追加。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、npm audit3lockfile、
  markdownlint39ファイル、server485/jxl3 green）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  E2E本番相当昇格、Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト、E2E artifact公開、GPG署名、
  RPO/RTO定義、バックアップ鮮度監視、trunk部分アーカイブ、cronタイムアウト、GPG trust、
  cron allowlistログキー、license_file相対パス、BACKUP_PREFIX検証、axe自動スキャン、
  開発モックuser_hash衝突、再試行導線、クライアント例外テレメトリ、refresh失敗メトリクス、
  playwright install再試行、window unit停止済みアプリ再起動、監査スクロールキーボード到達、
  E2Eビルド::error::、reqwest TLS二重スタック、tokio features full、CIバイナリキャッシュ、
  失効閾値削除失敗の警告格下げ、logout sub不一致時Cookie削除、finalizer BEGIN失敗計上、
  NTP同期要件、ブラウザエラー観測、429/503再試行、prefers-reduced-motion、
  JXLアニメーション複数フレーム、MAX_PIXELS実効化、CI fallbackガード到達性）は記録のみで保留。

- 2026-11-23 コンボ第44ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  異議通報のcreated_at欠損時にfail-closedで所有者のみ受理、
  verify_migrationsのchecksum読取カーソルをdrop、413/1KiB超過400へ
  セキュリティヘッダ+x-request-id検証、uploadのaria-busyをtrue/false文字列化
  （upload-submit testid追加）、doc-numbersセクション改番、ci-e2eコメント、
  BUILD-01 wildcard例外、ARCHITECTURE_DIAGRAMのfile:のみ記載。
  R4指摘（ci-e2eコメント参照方向）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、npm audit3lockfile、
  markdownlint39ファイル、server485/upload6 green）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  E2E本番相当昇格、Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  build.rs rerun-if-env-changed、リクエストタイムアウト、E2E artifact公開、GPG署名、
  RPO/RTO定義、バックアップ鮮度監視、trunk部分アーカイブ、cronタイムアウト、GPG trust、
  cron allowlistログキー、license_file相対パス、BACKUP_PREFIX検証、axe自動スキャン、
  開発モックuser_hash衝突、再試行導線、クライアント例外テレメトリ、refresh失敗メトリクス、
  playwright install再試行、window unit停止済みアプリ再起動、監査スクロールキーボード到達、
  JXL channels=0/非有限サンプル、E2Eビルド::error::、reqwest TLS二重スタック、
  tokio features full、CIバイナリキャッシュ、失効閾値削除失敗の警告格下げ、
  logout sub不一致時のCookie削除、finalizer BEGIN失敗のスキップ計上）は記録のみで保留。

- 2026-11-23 コンボ第43ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  CIシェル静的ゲートをbackupテスト/E2Eより前に移動（E2E失敗時にスキップされないように）、
  doc-numbersへspecの現行tests件数照合を追加（履歴語併記行は「現行...testsN」を優先）、
  ログイン画面の内部ID(SEC-01)文言を平易な英語案内へ置換(E2E追随)、
  エクスポート成功表示GENERATED_AT:/SUB:へspan lang=en。
  R4指摘（履歴語判定粒度）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、npm audit3lockfile、
  markdownlint39ファイル、shellcheck/shfmt green）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  E2E本番相当昇格、Pinata timeout設定、migrations dir override、refresh Cookie __Host- prefix、
  instance.lock部分書込み、build.rs rerun-if-env-changed、リクエストタイムアウト、
  E2E artifact公開、GPG署名、RPO/RTO定義、バックアップ鮮度監視、trunk部分アーカイブ、
  cronタイムアウト、GPG trust、cron allowlistログキー、license_file相対パス、
  BACKUP_PREFIX検証、axe自動スキャン、開発モックuser_hash衝突、ネットワークエラー再試行、
  クライアント例外テレメトリ、refresh失敗メトリクス、playwright install再試行、
  window unit停止済みアプリ再起動、監査スクロールキーボード到達、
  JXL channels=0/非有限サンプル、backup expect件数spec照合、E2Eビルド::error::）は記録のみで保留。

- 2026-11-23 コンボ第42ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  【high】PAYMENT_RECEIVING_ADDRESSのゼロアドレスを拒否（is_valid_eth_address）+回帰テスト、
  .env.exampleを起動可能テンプレートへ（JWT RSAコメントアウト・IPFS {cid}・非ゼロアドレス）、
  RunbookのAUDIT_EXPECT_ROOT初期取得を「最初の監査エントリ発行後」へ（空ログ拒否と整合）、
  ダーク背景の非アクティブナビ/集計th/監査日時をgray-400へ+集計thへlang=en。
  R4指摘3件（envゼロアドレス・テスト不足・ナビ残り）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、npm audit3lockfile、
  markdownlint39ファイル、server485テスト）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  E2E本番相当昇格、refresh Cookie __Host- prefix、instance.lock部分書込み、
  build.rs rerun-if-env-changed、リクエストタイムアウト、E2E artifact公開、GPG署名、
  RPO/RTO定義、バックアップ鮮度監視、trunk部分アーカイブ、cronタイムアウト、GPG trust、
  cron allowlistログキー、license_file相対パス、BACKUP_PREFIX検証、doc-numbers照合拡張、
  axe自動スキャン、開発モックuser_hash衝突、ネットワークエラー再試行導線、
  クライアント例外テレメトリ、refresh失敗メトリクス、playwright install再試行）は記録のみで保留。

- 2026-11-23 コンボ第41ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  【high】auth_verify/OIDC callbackの失効閾値削除をトークン発行後へ遅延（clear_superseded_revocation）、
  logoutのrefresh token sub束縛、OIDC endpoint userinfo拒否、
  MSRV 1.85→1.88（let chains使用）、.env.example追加、advisory内訳/README CI記述修正。
  親がclippy collapsible_if 2件をlet chainsで修正、tests156追随。
  R4指摘2件（旧1.85記述/env example不足）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests156、npm audit3lockfile、
  markdownlint39ファイル）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  E2Eを本番相当へ昇格、リクエストタイムアウト、E2E artifact公開、GPG署名、RPO/RTO定義、
  バックアップ鮮度監視、trunk部分アーカイブ、cronタイムアウト、GPG trust手順、
  cron allowlistログキー、license_file相対パス、BACKUP_PREFIX検証、doc-numbers照合拡張、
  axe自動スキャン、8pxテキスト、TASK_INVENTORYメタデータ、APP_UNIT doctor診断、
  ダッシュボード再試行、開発モックuser_hash衝突、旧JWT復活の完全回避検証）は記録のみで保留。

- 2026-11-23 コンボ第40ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  BetterleaksのPATH更新/all-zero beforeフォールバック/wasm target clippy追加、
  revoked_sessionsメモリを周期purgeと同じcutoffで回収、JWT鍵+AGE検証のDB接触前移動と
  InstanceLockGuard(Drop削除)、ALLOWED_ORIGINS本番https強制(doctor共通判定)、
  JXL/小説detailへlang=ja、鮮度警告文言の定数化、低コントラスト色をgray-400以上へ、
  6pxハッシュ表示を10pxへ。
  親が旧4関数を#[cfg(test)]化してlib dead_code解消、tests153追随。
  R4指摘（8px残存）は元から8pxの箇所で6px→10px要件は充足と判定し却下。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests153、npm audit3lockfile、
  markdownlint39ファイル、betterleaks全履歴no leaks）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  GPG復号訓練、Dependabot、axe自動スキャン、MSRV下限CI、二次処理5xx伝播、
  finalizer異常終了readiness、動的alert、401復帰導線共通化、クライアントエラーテレメトリ、
  .audit-rootオフサイト、E2E artifact回収、Chromeバージョン固定、OIL_MOCK_AUTH=0検証、
  BUILD-02負のビルドゲート、retries方針、TOC scroll-margin、日時ja-JP+UTC不整合、
  公開集計鮮度の端末時計依存/復旧経路、backup-cron-wrapper owner/mode分岐、
  cargo-deny aarch64/licenses方針、docs/log.md運用、HANDOFF CI記述、
  restore safety copy原子性、OIDC閾値削除と発行の原子化、ALERT_CMD死活観測、
  cron allowlistログキー、license_file相対パス解決、集計/投票キュー英文lang、
  logout再試行文言、Sizes超過再選択、Back-ChannelLogout部分失敗テスト）は記録のみで保留。

- 2026-11-23 コンボ第39ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  Betterleaksをschedule/workflow_dispatchで全履歴走査へ（PR=base..head/push=before..HEADは維持）、
  .betterleaks.tomlでredact_urlダミーURLの誤検知4件をallowlist（親がconfig無し4件/有り0件を実測）、
  pre-commit hookへconfig引数、error_langの漢字範囲を\u{9fff}へ拡張、
  「tests152→tests152」自己矛盾を当時/現行分離、client/Cargo.tomlコメント実態追随、
  TEST_SPECIFICATIONへR2-A11Y第4次追記。
  R4指摘2件（Cargoコメント/allowlist説明）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests152、npm audit3lockfile、
  markdownlint39ファイル、betterleaks全履歴no leaks）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  GPG復号訓練、Dependabot、axe自動スキャン、MSRV下限CI、二次処理5xx伝播、
  finalizer異常終了readiness、動的alert、401復帰導線共通化、クライアントエラーテレメトリ、
  .audit-rootオフサイト、E2E artifact回収、Chromeバージョン固定、OIL_MOCK_AUTH=0検証、
  BUILD-02負のビルドゲート、retries方針、TOC scroll-margin、日時ja-JP+UTC不整合、
  公開集計鮮度の端末時計依存/復旧経路、backup-cron-wrapper owner/mode分岐、
  cargo-deny aarch64/licenses方針、docs/log.md運用、HANDOFF CI記述、
  restore safety copy原子性、refreshファミリ失効失敗、prepare-distribution全denyゲート、
  通知世代識別、OKFメタデータCI検証、監査要約窓口、ALERT_CMD失敗時代替検知、
  OFFSITE_DEST非エコー、E2E error-context出所ラベル、OIDC閾値削除と発行の原子化、
  集計/投票キュー英文lang、ログアウト再試行文言、サイズ超過再選択、
  Back-Channel Logout部分失敗テスト）は記録のみで保留。

- 2026-11-23 コンボ第38ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  CI Betterleaksをイベント別範囲（PR=base..head / push=before..HEAD）へ修正、
  advisory内訳/epochテスト名/sha2-hex推移依存/SECURITY_ADVISORIES §3.1見出しの
  ドキュメント実態追随、Runbookのヘルス確認を手動必須ステップと明記、
  h1/検証バッジ/種別バッジへのlang=en、view変更時#main-contentフォーカス
  （初回マウント除外の否定オラクル追加）。
  R4指摘3件（tests152追随/advisory見出し/否定オラクル）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests152、npm audit3lockfile、
  markdownlint39ファイル）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  GPG復号訓練、Dependabot、axe自動スキャン、MSRV下限CI、二次処理5xx伝播、
  finalizer異常終了readiness、動的alert、401復帰導線共通化、クライアントエラーテレメトリ、
  .audit-rootオフサイト、E2E artifact回収、Chromeバージョン固定、OIL_MOCK_AUTH=0検証、
  BUILD-02負のビルドゲート、retries方針、TOC scroll-margin、日時ja-JP+UTC不整合、
  公開集計鮮度の端末時計依存/境界テスト、backup-cron-wrapper owner/mode分岐、
  cargo-deny aarch64/licenses方針、docs/log.md運用、HANDOFF CI記述、
  restore safety copy原子性、refreshファミリ失効失敗、prepare-distribution全denyゲート、
  通知世代識別、OKFメタデータCI検証、監査要約窓口、ALERT_CMD失敗時代替検知、
  OFFSITE_DEST非エコー、E2E error-context出所ラベル）は記録のみで保留。

- 2026-11-23 コンボ第37ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  CI Betterleaksを--stagedから--log-opts範囲走査へ（checkout直後0バイト走査で実質無効だった、
  親がstaged=0B/log-opts=13KBを実測）、sign_audit_exportの公開後staging削除失敗を警告分離、
  restore_db初回復元の公開後検証失敗時ロールバック、doctorのOFFSITE_COPY_CMD実行可能性診断、
  検証待ちrole=status/lang、Ledger 401 dedicated案内、login二重送信ガード、web-sys Url feature。
  親がHRTBエラーをクロージャで修正。R4 指摘なし。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests152、npm audit3lockfile、
  markdownlint39ファイル、bin tests green）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/OIDC azp、第三者監査アンカー、起動設定一括Config化、
  BIND_ADDR/PORT設定化、workspace依存集約、孤立Pinataピン補償、alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict CIゲート、
  GPG復号訓練、Dependabot、axe自動スキャン、MSRV下限CI、二次処理5xx伝播、
  finalizer異常終了readiness、動的alert、401復帰導線共通化、クライアントエラーテレメトリ、
  .audit-rootオフサイト、E2E artifact回収、Chromeバージョン固定、OIL_MOCK_AUTH=0検証、
  BUILD-02負のビルドゲート、retries方針、TOC scroll-margin、日時ja-JP+UTC不整合、
  公開集計鮮度の端末時計依存、backup-cron-wrapper owner/mode分岐テスト、cargo-deny
  aarch64/licenses方針、docs/log.md運用、HANDOFF CI記述、restore safety copy原子性、
  refresh再利用検知時ファミリ失効失敗、prepare-distribution全denyゲート、notify世代識別、
  OKFメタデータCI検証、監査要約窓口、ボディ付きAPI refresh再送E2E）は記録のみで保留。

- 2026-11-23 コンボ第36ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  pre-commitシークレットスキャン導入（Betterleaks v1.10.1ピン・CI併設・.gitignore拡張）、
  back-channel logoutのsid/sub独立試行+失敗集約、GET /transactionsのclaims.sub主体スコープ、
  finalize_reportのread-then-write順序、epochガード回帰テスト、tests152追随。
  R4指摘（tests150→152の記述追随）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests152、npm audit3lockfile、
  markdownlint39ファイル、betterleaks git staged no leaks）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/chain構築、OIDC署名鍵403/501化+azp検証、
  第三者監査アンカー、公開ledger意図、起動設定一括Config化、BIND_ADDR/PORT設定化、
  workspace依存バージョン集約、孤立Pinataピン補償、OnFailure alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict の
  CIゲート化、GPG復号訓練、Dependabot方針、axe自動スキャン、MSRV下限CI検証、
  コミット後二次処理の5xx伝播、finalizer異常終了readiness/失敗メトリクス、
  back-channel logoutのsid/sub別tx完了保証、動的alertアナウンス、401復帰導線共通化、
  クライアントエラーテレメトリ、.audit-rootオフサイト、CORSスタブOriginエコー、
  E2E artifact回収/結果分離、Chromeバージョン固定、OIL_MOCK_AUTH=0ビルド検証、
  BUILD-02負のビルドゲート、retries方針、warn_history1回だけ検証、
  prepare-distribution親ディレクトリ権限、assert_major_numericデッドコード、
  公開集計鮮度境界/shorten_export_sub境界テスト、error-paths行番号コメント、
  warn_history完了記録の実態漂移、投票キュー要約注記/lang、検証待ちrole=status、
  systemd/cron二重有効化検出、日時UTC明示/ja-JP+UTC不整合、.geminiignore drift、
  backup-cron-wrapper owner/mode分岐テスト、cargo-deny aarch64/licenses方針、
  docs/log.md運用、HANDOFF CI記述、TOC scroll-margin、公開集計鮮度の端末時計依存、
  restore_db safety copy原子性、refresh再利用検知時ファミリ失効失敗）は記録のみで保留。

- 2026-11-23 コンボ第35ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  AGE_COUNTRY_OVERRIDES重複国コードfail-closed拒否、CI concurrencyへevent_name追加
  （週次監査がpushでキャンセルされないように）、Runbook timer手順の文途切れ修正、
  logoutがsub失効閾値を検査（AuthClaimsと同判定で401）、rate limiterキー回収
  （retain_recent+キー数/スイープメトリクス）、CORS expose_headers x-request-id、
  機微APIへCache-Control no-store（監視パス除外）。
  R4 指摘なし。全ゲート green（E2E69/check23/doctor54/migrations14/tests150、
  npm audit3lockfile、markdownlint39ファイル）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/chain構築、OIDC署名鍵403/501化+azp検証、
  第三者監査アンカー、公開ledger意図、起動設定一括Config化、BIND_ADDR/PORT設定化、
  workspace依存バージョン集約、孤立Pinataピン補償、OnFailure alert unitテンプレート、
  IPFSゲートウェイ異常運用手順、監査エクスポート自動スケジューラ、doctor --strict の
  CIゲート化、GPG復号訓練、Dependabot方針、axe自動スキャン、MSRV下限CI検証、
  コミット後二次処理の5xx伝播、finalizer異常終了readiness/失敗メトリクス、
  back-channel logoutのsid/sub別tx、動的alertアナウンス、401復帰導線共通化、
  クライアントエラーテレメトリ、.audit-rootオフサイト、CORSスタブOriginエコー、
  E2E artifact回収/結果分離、Chromeバージョン固定、OIL_MOCK_AUTH=0ビルド検証、
  BUILD-02負のビルドゲート、retries方針、warn_history1回だけ検証、
  prepare-distribution親ディレクトリ権限、assert_major_numericデッドコード、
  公開集計鮮度境界/shorten_export_sub境界テスト、error-paths行番号コメント、
  warn_history完了記録の実態漂移、投票キュー要約注記/lang、検証待ちrole=status、
  systemd/cron二重有効化検出、日時UTC明示、.geminiignore drift、
  backup-cron-wrapper owner/mode分岐テスト）は記録のみで保留。

- 2026-11-23 コンボ第34ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  CI all-features clippyへundocumented_unsafe_blocks追加、バックアップtimer Persistent=false
  （窓外補完による予告なし停止を防止）+RunbookのTimeoutStopSec/drain手順、
  AUDIT-02をhard_link原子公開+空ログ拒否へ追随、HANDOFF doctor54/clippy lint、
  監査dialogのlangを子要素単位へ（英語ラベル=en・日本語注記=ja）、
  代表的静的英文UIへlang=en、タブタイトルE2Eの仕様反映。
  R4 指摘3件（HANDOFF doctor54/clippy/タブタイトル仕様）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests148、npm audit3lockfile、
  markdownlint39ファイル）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/chain構築、OIDC署名鍵403/501化+azp検証、
  第三者監査アンカー、公開ledger意図、起動設定一括Config化、BIND_ADDR/PORT設定化、
  workspace依存バージョン集約、孤立Pinataピン補償、OnFailure alert unitテンプレート、
  監査エクスポート自動スケジューラ、doctor --strict の CI ゲート化、GPG復号訓練、
  Dependabot方針、axe自動スキャン、MSRV下限CI検証、コミット後二次処理の5xx伝播、
  finalizer異常終了readiness/失敗メトリクス、back-channel logoutのsid/sub別tx、
  動的alertアナウンス、401復帰導線共通化、クライアントエラーテレメトリ、
  .audit-rootオフサイト、CORSスタブOriginエコー、E2E artifact回収/結果分離、
  Chromeバージョン固定、OIL_MOCK_AUTH=0ビルド検証、BUILD-02負のビルドゲート、
  retries方針、warn_history1回だけ検証、prepare-distribution親ディレクトリ権限、
  assert_major_numericデッドコード、公開集計鮮度境界/shorten_export_sub境界テスト、
  error-paths行番号コメント、warn_history完了記録の実態漂移）は記録のみで保留。

- 2026-11-23 コンボ第33ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  sign_audit_exportをhard_linkによる原子的公開へ（rename競合ウィンドウ解消）、
  staging名へpid+unix_millis、空ログを--allow-empty無しで拒否、
  verify_audit_logも空ログを暗黙OKとしない、DEC-06/ARCHITECTURE_DIAGRAMの
  記述を実態（ブラウザ一覧決定済み・mockは開発時のみ/OIDC実装済み）へ整合、
  search.spec.tsへタブタイトルE2E追加。
  R4 指摘なし。全ゲート green（E2E69/check23/doctor54/migrations14/tests148、
  npm audit3lockfile、markdownlint39ファイル、bin tests 17/14）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/chain構築、OIDC署名鍵403/501化+azp検証、
  第三者監査アンカー、公開ledger意図、起動設定一括Config化、BIND_ADDR/PORT設定化、
  workspace依存バージョン集約、監査エクスポート自動スケジューラ、doctor --strict の
  CIゲート化、GPG復号訓練、Dependabot方針、axe自動スキャン、MSRV下限CI検証、
  コミット後二次処理の5xx伝播、finalizer異常終了readiness/失敗メトリクス、
  back-channel logoutのsid/sub別tx、動的alertアナウンス、401復帰導線共通化、
  クライアントエラーテレメトリ、.audit-rootオフサイト、Persistent補完と窓の矛盾、
  alert unitテンプレート、CORSスタブOriginエコー、E2E artifact回収/結果分離、
  Chromeバージョン固定、OIL_MOCK_AUTH=0ビルド検証、BUILD-02負のビルドゲート、
  retries方針、warn_history1回だけ検証、prepare-distribution親ディレクトリ権限、
  assert_major_numericデッドコード、公開集計鮮度境界/shorten_export_sub境界テスト）は記録のみで保留。

- 2026-11-23 コンボ第32ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  cron推奨構成(root crontab)でDATABASE_URLがallowlistを通らず本番バックアップが起動拒否
  になる回帰を修正（DATABASE_URL/DATABASE_TOKENをallowlistへ追加）、
  改ざん検知表示へrole=alert+lang=en、TOC summaryへlang=en、
  serverテスト用アプリ24箇所をSSRF防御済みtest_http_client()へ統一、NET-05記述整合。
  R4 指摘なし。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests148、npm audit3lockfile、
  markdownlint39ファイル、server483テスト、backup-cron-wrapper green）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー/chain構築、OIDC署名鍵403/501化+azp検証、
  第三者監査アンカー、公開ledger意図、起動設定一括Config化、BIND_ADDR/PORT設定化、
  監査エクスポート自動スケジューラ、doctor --strict の CI ゲート化、GPG復号訓練、
  Dependabot方針、axe自動スキャン、MSRV下限CI検証、コミット後二次処理の5xx伝播、
  finalizer異常終了のreadiness反映/失敗メトリクス、back-channel logoutのsid/sub別tx、
  動的alertアナウンス、401復帰導線共通化、クライアントエラーテレメトリ、
  .audit-rootオフサイト、Persistent補完と窓の矛盾、alert unitテンプレート、
  CORSスタブOriginエコー、E2E artifact回収/結果分離、Chromeバージョン固定、
  OIL_MOCK_AUTH=0ビルド検証、BUILD-02負のビルドゲート、retries方針、
  warn_history1回だけ検証、prepare-distribution親ディレクトリ権限、
  assert_major_numericデッドコード、公開集計鮮度境界/shorten_export_sub境界テスト）は記録のみで保留。

- 2026-11-23 コンボ第31ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  補償unpin判定をContentAlreadyExists dedicatedバリアントへ、write_lock poisonをResult化、
  unban_userのrevoked_subs行不存在INSERTフォールバック、監査連鎖Rows drop統一、
  backup-cron-wrapperのキー名エコー除去、CIへdev-console feature(--all-features)検証追加、
  公開集計更新の世代ガード+連打抑止、deep link CIDのpopstate往復、
  login aria-busy+進捗status、Ledger loading、error_lang全alert/status統一、
  通報フォームrequired撤廃+aria-required+form on:submit。
  R4指摘（tests148追随・重複CID Rows drop）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests148、npm audit3lockfile、
  markdownlint39ファイル、clippy --all-features も別途green）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー、OIDC署名鍵403/501化+azp検証、公開ledger意図、
  起動設定一括Config化、BIND_ADDR/PORT設定化、監査エクスポート自動スケジューラ、
  doctor --strict の CI ゲート化、GPG復号訓練、Dependabot方針、MSRV下限CI検証、
  コミット後二次処理の5xx伝播、finalizer異常終了のreadiness反映/失敗メトリクス、
  back-channel logoutのsid/sub別tx、動的alertアナウンス、401復帰導線共通化、
  a11y自動スキャン、クライアントエラーテレメトリ、.audit-rootオフサイト、
  Persistent補完と窓の矛盾、alert unitテンプレート、CORSスタブOriginエコー、
  E2E artifact回収/結果分離、Chromeバージョン固定、OIL_MOCK_AUTH=0ビルド検証、
  BUILD-02負のビルドゲート、retries方針、warn_history1回だけ検証、
  prepare-distribution親ディレクトリ権限）は記録のみで保留。

- 2026-11-23 コンボ第30ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  監査Runbookの秘密鍵コピーをmktemp+trapで確実削除、未マッチ経路の生パスをアクセスログ/
  トレースから除外（"unmatched"固定値）、README doctor --strictへ本番env注入明記、
  doc-numbersのbin件数照合へTASK_INVENTORY追加、公開集計鮮度警告へrole=status+⚠、
  通報フォームへlabel for、uploadボタンへaria-busy、TASK_INVENTORY advisory内訳修正。
  R4 指摘（advisory内訳）も修正。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests147、npm audit3lockfile、
  markdownlint39ファイル）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー、OIDC署名鍵403/501化、公開ledger意図、
  起動設定一括Config化、BIND_ADDR/PORT設定化、E2E_SERVER_PORT設定化、
  監査エクスポート自動スケジューラ、doctor --strict の CI ゲート化、GPG復号訓練、
  Dependabot方針、MSRV下限CI検証、コミット後二次処理の5xx伝播、
  finalizer異常終了のreadiness反映、back-channel logoutのsid/sub別tx、
  動的alertのアナウンス、401復帰導線の全ビュー共通化、a11y自動スキャン、
  クライアントエラーテレメトリ、.audit-rootオフサイト、Persistent補完と窓の矛盾、
  alert unitテンプレート、CORSスタブのOriginエコー、base障害メトリクス過小報告）は記録のみで保留。

- 2026-11-23 コンボ第29ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  instance lock の PID=0 を backup_db/restore_db でも fail-closed 拒否（共有 owner_unknown）、
  LOG_FORMAT と数値系5envを read_env 分類＋doctor 診断（check23維持・doctor54）、
  OIL_API_BASE の origin-only 検証（path/query/userinfo/loopback拒否）、
  login の .unwrap() を Result 伝播へ、client の sha2/hex 直接依存削除、
  英文UI lang 一括付与、restore_db の unparsable lock テスト追加。
  R4 指摘で sha2/hex はcommon経由で推移的に残る旨を明記、tests147 追随、
  OIL_API_BASE 末尾スラッシュ拒否を docs へ反映。
  全ゲート green（E2E69/check23/doctor54/migrations14/tests147、npm audit3lockfile、
  markdownlint 39ファイル、restore_db21件）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー、OIDC署名鍵403/501化、公開ledger意図、
  起動設定一括Config化、E2E_SERVER_PORT設定化、監査エクスポート自動スケジューラ、
  doctor --strict の CI ゲート化、GPG復号訓練、Dependabot方針、MSRV下限CI検証、
  コミット後二次処理の5xx伝播、finalizer異常終了のreadiness反映、back-channel logout
  のsid/sub別tx、動的alert挿入のアナウンス、401復帰導線の全ビュー共通化、
  a11y自動スキャン、年齢拒否E2Eモックの閾値）は記録のみで保留。

- 2026-11-23 コンボ第28ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  再アップロード既存CIDで補償unpinが正当ピンを解除する回帰を修正（自前変更の修正）、
  backup-cron-wrapper へ env キー allowlist 導入（BASH_ENV/PATH/LD_* 等の搬入を拒否、
  case8 追加）、鍵ローテーション Runbook の fail-closed 矛盾と XFF プローブ手順、
  HANDOFF の advisory 内訳を SECURITY_ADVISORIES 現行値へ更新。
  設計判断待ち（OIDC callback の閾値削除とセッション発行の原子化、refresh の BAN 再読、
  監査エクスポート2表の単一スナップショット、ban/unban 冪等キー、logout の部分失敗継続、
  doctor --strict の CI ゲート化、alert unit テンプレート、.audit-root のオフサイト対象、
  E2E epoch ガード/JXL寸法ガードのテスト、jxl.rs/fuzz_test.rs の仕様一覧、
  CORSプリフライトテスト、 keluaran lang、README /metrics 注意、favorites コメント）は記録のみで保留。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests145、npm audit3lockfile、
  markdownlint 39ファイル、backup-cron-wrapper case1-8）。

- 2026-11-23 コンボ第27ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  doc-numbers へ test.describe.only/skip/fixme 検出を追加（親が実測で fail 確認）、
  backup-rotate.sh の on_failure trap を ALERT_CMD 定義直後へ前移（設定エラーでも通知）、
  window unit へ OnFailure、cron へ MAILTO 明記と二者択一手順、
  OIL_MOCK_AUTH/OIL_CLIENT_METRICS の false/FALSE 無効化を共通 build_flag_enabled へ集約、
  upload_content/mod_vote の tx 内 BAN 再読、OIDC callback の revoked_subs 失敗時復元。
  finalizer の success(0) 条件は既に満たされていた（コメントのみ追記）。
  R4 指摘で tests145 追随とテスト名を *_disabled_by_zero_or_false へ改名。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests145、npm audit3lockfile、
  markdownlint 39ファイル）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー、OIDC署名鍵403/501化、公開ledger意図、
  起動設定一括Config化、E2E_SERVER_PORT設定化、監査エクスポート自動スケジューラ、
  doctor --strict の CI ゲート化、GPG復号訓練、Dependabot方針、MSRV下限CI検証、
  cron/systemd の通知unit実体、BUILD-01/MSRV/HANDOFF の残存記述）は記録のみで保留。

- 2026-11-23 コンボ第26ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  doc-numbers へ test.only/skip/fixme 残存検出を追加、client の未使用 base64 依存を削除、
  backup.service へ TimeoutStartSec=1800、README へ OIL_API_BASE 本番指定・/metrics・
  LOG_FORMAT・restore --force 注意、運用ポリシーの npm audit 3 lockfile 表記・env一覧
  (LOG_FORMAT/RETENTION_DAYS/MIN_KEEP)・改行損壊、e2e/package.json のライセンス
  AGPL-3.0-or-later+private、.amazonq/rules を markdownlint 対象へ追加+OKF title。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests142、npm audit3lockfile、
  markdownlint 39ファイル）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー、OIDC署名鍵403/501化、公開ledger意図、
  OIL_API_BASE未設定fail-openの自動ゲート化、起動設定一括Config化、E2E_SERVER_PORT設定化、
  監査要約窓口、監査エクスポート自動スケジューラ、doctor --strict の CI ゲート化、
  doc-numbers のRustテスト総数照合、ローカルE2Eトレース、GPG復号訓練の自動化）は記録のみで保留。

- 2026-11-23 コンボ第25ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  doctor が非UTF-8 TRUSTED_PROXY_CIDRS を診断せず false pass していた問題を classify 判定へ修正、
  doc-numbers の E2E test() 抽出を count_e2e_tests ヘルパでコメント除外へ堅牢化（全3箇所統一、
  コメントに test( を入れても69件維持を親が実測）、backup window unit へ TimeoutStartSec=1800、
  Runbook の /ready 手順を「停止中は接続拒否が正常」へ修正、timer Persistent 注記、
  markdownlint 版記録 0.24.2 へ追随。
  R4 の2件（timer参照名・TASK_INVENTORY文途切れ）も修正。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests142、npm audit3lockfile）。
  初回 cargo deny advisory DB と npm audit が network reset で失敗したが再実行で green。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー、OIDC署名鍵403/501化、公開ledger意図、
  OIL_API_BASE未設定fail-open、起動設定一括Config化、E2E_SERVER_PORT設定化、
  監査要約の問い合わせ窓口、SPAビュー切替のフォーカス移動）は記録のみで保留。

- 2026-11-23 コンボ第24ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で以下を是正:
  doctor --strict へ DATABASE_TOKEN 非UTF-8 と MODERATION_OPERATOR_HASHES の production 診断
  追加（check23不変・値非エコー）、MSRV1.85 を workspace 一元化、
  doc-numbers のクライアント走査を client/src/**/*.rs へ拡張（jxl.rs3テストを仕様へ追記）、
  ADR log へ2026-11-23 異議認可エントリ追加。
  子の「MISMATCH→FINALIZED 上書き」指摘は既存2段目CASが0行更新で409を返すため誤検知と
  判定し却下（試行した早期リターンは既存テストを壊したため取り消し）。
  R4 指摘で doc-numbers のコメント/FAIL文言を全モジュール走査へ合わせ、
  shellcheck の引用警告を mapfile で解消。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests142、npm audit3lockfile）。
  初回 npm audit は ECONNRESET で失敗したが再実行で green（ネットワーク要因）。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー、OIDC署名鍵403/501化、公開ledger意図、
  OIL_API_BASE未設定fail-open、起動設定一括Config化、BIND_ADDR/PORT設定化）は記録のみで保留。

- 2026-11-23 コンボ第23ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し新規タスクが得られたため継続。R6 で high2件+medium2件を是正:
  sid-only Logout Token を sub必須でfail-closed拒否（refresh復活防止）、
  appealを元通報者/所有者に限定+30日クールダウン、
  get_contentのIPFS I/O後status再SELECT（通報中配信防止）、
  verify_user_signatureのNULL公開鍵を401へ載せる。
  R4 指摘で appeal の元通報者判定を appeal_of 対象通報の created_at に最も近い監査行へ
  紐付け（最古行固定による誤認可を解消）、docs の tests142 追随。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests142、npm audit3lockfile）。
  tests142 は doc→code テスト名照合値であり実テスト総数ではない点に注意。
  設計判断待ち（本人確認監査、通知/再審査、UNPIN、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー、OIDC署名鍵未登録の403/501化、公開ledgerの設計意図、
  npm audit moderate 方針）は記録のみで保留。

- 2026-11-23 コンボ第22ラウンド（review4+discovery3）: 全7担当完了。TODO未着手0件の状態で
  補充を実行し、新規タスクが得られたためループ継続。R6 で以下を是正・記録:
  MISMATCH→EXPIRED 上書き防止、IPFS孤立ピン補償、favorite/注文tx原子化、UNBAN旧JWT復活防止、
  backup_db/restore_db中途失敗クリーンアップ、投票キュー本文導線、card→通報プリフィル、
  英文UI lang、export生成時刻/sub表示、公開集計鮮度警告、deny wildcards=deny+skip例外、
  npm audit 3 lockfile（ルート/e2e/client）。
  R4 指摘（docs旧値・backup_db検証cleanup漏れ・MISMATCH期限文言・restore safety_copy None・
  URLコメント・E2E2E69タイポ）も修正。子の「OPERATIONAL_POLICY:22 行末空白」は
  親が markdownlint exit 0 を確認し却下。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests136、npm audit 3 lockfile 含む）。
  設計判断待ち（本人確認監査対象、通知/再審査、UNPIN手順、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー、client lockfile既知脆弱性の解消方針）は記録のみで保留。

- 2026-11-23 coding（21件）: server側はfinalizer部分コミット計上・readinessドレイン窓・
  EnvValue取りこぼし・RSA kid整合・UNIQUEカラム判定・doctor文言・tokio-test削除。
  client側は投票一覧error時disabled・監査再取得・upload loading固定・popstate正規化・
  logout失敗alert・集計GENERATED_AT整形・export URL/aria-label・README CIゲート。
  親がclippy explicit_auto_deref・doc-numbersワイルドカード・表結合・E2E3件
  (moderation/search/upload)を修正。R4で「upload差し替えE2E未検証」を明記。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests136）。
  残り高1件（DEC-01 外部OP/JPKIクレーム仕様確定待ち）のみ。

- 2026-11-22 コンボ第20ラウンド（review4+discovery3）: 全7担当完了。R6 で refresh セキュリティ
  3件を是正: refresh失敗時にREFRESHED_JWTクリア、VerifyResponse.user_hash照合、
  REFRESH_SLOT epochガード（logout/login時にインクリメント）。auth.spec.tsのrefreshモックを
  VerifyResponse契約へ統一。doc陳腐化（UNKNOWN_TASKSのrefresh有効化/R3-10根拠/
  cron MAILTO/R1-17,R1-18/OPERATIONAL_POLICYのtests136/重複語）も修正。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests136、npm audit ルート+e2e 含む）。
  残タスク候補（投票一覧取得失敗時の古い通報、監査タイムライン再取得、アップロード送信中差し替え、
  TOCのgoBack、logout失敗通知、集計GENERATED_AT整形、export object URL、aria-label、
  README CIゲート、CONTENT-01/06/MOD-08/AUTH/E2Eテストギャップ、tokio-test未使用、
  finalizer部分コミットメトリクス、readinessドレイン、EnvValue取りこぼし、RSA kid整合 等）は
  次ラウンドでタスク化して処理する。DEC-01は外部OP/JPKIクレーム仕様確定待ちで保留継続。

- 2026-11-22 coding（高1件・JWT refresh 有効化）: ENABLE_AUTO_REFRESH=true へ反転。
  単一フライト＋refresh失敗時に元の401を返す契約で旧競合を解消。
  さらに refresh 成功時の新JWTを App の token signal ではなく api_fetch 専用
  REFRESHED_JWT スロットへ保持し、一覧 Effect 再加熱によるビュー再マウントで
  export-status が消える問題を根本解消。ログイン/ログアウトでクリア。
  既存export 401ケース内へ「失敗側401→固定文言→成功側refresh→新JWT再送→200」の
  回復フェーズを統合（新規 test() なし・69件維持）。R4 指摘でデバッグログ削除・
  旧コメント修正・docs/tests136 追随。全ゲート green。
  残り高1件（DEC-01 外部OP/JPKIのクレーム仕様確定待ち）のみ。

- 2026-11-22 コンボ第17ラウンド（review4+discovery3）: 全7担当完了。R6 で high3件を是正:
  revoked_subs purge を refresh TTL(30日)+マージンへ延長、OIDC callback の tx 内 BAN 再読、
  OIDC 初回登録の public_key_der プレースホルダ（500→401 改善）。UNBAN も失効閾値を削除。
  即時項目: e2e/package も npm audit 対象へ、cron 起動失敗の終了コード反映、
  .audit-root の世代同期削除、INFRA_NETWORK/SECURITY_ADVISORIES の内訳是正。
  R4 指摘で「callback 再読パス未カバー」「UNBAN が sub 全閾値を削除」「401 の方針記述」を
  実態どおりドキュメントへ明記。
  設計判断待ち（本人確認の監査対象、通知/再審査、UNPIN手順、脅威モデル、JPYC手数料/返金、
  機能検出代替、JPKI信頼アンカー）は外部仕様・法務待ちのため記録のみで保留。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests136、npm audit ルート+e2e 含む）。残り高2（外部要因）。

- 2026-11-22 coding（中13件）: auth_verifyのBAN原子的化(tx内再読403)、真偽値判定を
  common::env::bool_env_resultへ集約、verify_chainのreasons_masked検査、
  sign_audit_exportのatomic publish、TRUSTED_PROXY_CIDRS/TZのread_env統一、
  および前ラウンドの残項目13件を消化。
  R4は「BAN再読テストが事前チェックで終わっており再読経路を検証していない」ことを指摘し、
  テストの主張を実態（再読はコードレビュー担保）へ修正。docsのtests数値を132へ追随、
  oidc.rsの独立文言維持を明記。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests132、npm audit 含む）。残り高2（外部要因）。

- 2026-11-22 コンボ第16ラウンド（review4+discovery3）: 全7担当完了。R6 で重複排除のうえ
  以下を即時是正: npm audit 常時失敗（markdownlint-cli2 0.24.2）、DATABASE_TOKEN 非UTF-8
  のbackup_db/verify_audit_log格下げ、ALLOWED_ORIGINSの二重読取、ALLOW_MOCK_AUTH改行再発、
  deny/SECURITY_ADVISORIESのadvisory内訳（10件=libsql8+chrono1+url1）。
  review/discovery で得た残項目（auth_verify BAN原子的、CI shellcheck契約、REVIEW_REPORT履歴、
  バックアップ通知、TRUSTED_PROXY_CIDRS/TZのread_env統一、真偽値ヘルパ共通化、
  監査reasons_masked検証、sign_audit_export atomic publish、client/jxl.rsテストのゲート漏れ、
  E2E test()抽出の堅牢性 等）は次ラウンドで処理する。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests131、npm audit 含む）。残り高2（外部要因）。

- 2026-11-22 coding（中1+低16）: back-channel logout の iat/nbf 新鮮性と再送冪等、
  gpg CI FAIL化、CIクロスブラウザ選定、IPFS http本番拒否、週次監査+npm audit、
  aria-live件数ゲート、ファイル選択タブ統合、error_lang一方針化、warn_historyは
  E2Eオラクル維持のためリリースでも警告（他担当のdev-consoleゲート化は親が差し戻し）。
  RustSec DB更新でadvisory受容が12→10件（ignore3削除+url1追加）へ追随。
  親が未使用変数/dead_code・SC2016構文・doc-numbersコメント・markdownlint・advisory内訳を修正。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests131）。残り高2（外部要因のみ）。

- 2026-11-22 coding（高2+中6）: migrationのdrop(rows)→BEGIN、空migrations拒否、
  auth_verify DB正本先行、DATABASE_TOKEN NonUtf8拒否、bind_wallet PENDING CAS、
  6CLI argv共通化、deny multiple-versions=deny+exact skip、API origin本番HTTPS const拒否。
  親がmarkdownlint行長1箇所を修正。全ゲート green（E2E69/check23/doctor53/migrations14/tests128）。
  残り中7・低8・高2（外部要因/JWT段階移行）。

- 2026-11-22 コンボ第13ラウンド（review4+discovery3）: 全7担当完了。
  R6 で重複排除し、LOG OUT の refresh Cookie 欠落（2担当独立確認・high）は即時修正済み
  （client の handle_logout へ credentials: Include）。E2E は localhost/127.0.0.1 別ホストのため
  Cookie 送信は検証不可、同一サイト構成で検証可能な旨を記録。
  その他25件（高2・中15・低8）を追記。後続ADR待ち（DEC-01/02/03/04・PoP・PPID・JPYC税務・
  リリース証跡）とセッションCookie廃止のXSSトレードオフは記録のみで保留。

- 2026-11-22 coding（低6件）: SC2034関数単位除外、README/REVIEW_REPORTのADR状態是正、
  旧名/console.log整理、英語ラベルlang="en"、全文検索/sha3/iss-aud棚卸し、
  request_id/4xx本文オラクルを仕様化。k-匿名性は却下として前ラウンドで完了済み。
  親がSC2034 disable構文・adr/index.md Accepted表記・doc-numbersコメントを修正。
  R4の「Guild残存」はブランド表記として却下（仕様はサーバー側のみを対象）。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests128）。残り高2（外部要因）のみ。

- 2026-11-22 coding（中6件+低2件）: session Cookie発行廃止、metrics count加算1固定、
  通報失敗時statusクリア、小説不正UTF-8をDECODE FAILED化、/content 401再ログイン導線、
  公開集計Parse分類、暗号化バックアップ復元パス統一、cron必須步骤明記。
  親がPrometheus期待値+1とdoc-numbersコメント内test()3箇所を修正。全ゲート green。
  残り高2（外部要因）・低7。

- 2026-11-22 coding（中優先6件）: リフレッシュ回転を単一tx原子化、OIDC_MAX_AUTH_AGEに30日上限、
  verify_order MISMATCH をCAS検査対象化、doctor FK のUnresolvedフォールバック廃止、
  PINATA判定を共有ヘルパ化、instance.lockをcommon集約＋resolve_database_urlを3CLI共通化。
  親がmarkdownlint行長4箇所・MISMATCHコメントを修正。全ゲート green。
  残り高2（外部要因）・中6・低9。

- 2026-11-22 coding（高優先6件）: AGE_*/ALLOWED_ORIGINSをenv分類器と共有ヘルパで起動ゲート化、
  OIL_MOCK_AUTH=0と501固定文言で本番UIの誤認を解消、shfmt -l を実効ゲートへ。
  AUDIT_EXPECT_ROOTの世代ライフサイクルとroot保存、prepare-distribution回帰5ケースを追加。
  親がLeptos文字列リテラル・HRTBクロージャ・doc-numbersコメント・markdownlintを修正。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests123）。残り高2（外部要因）・中20・低9。

- 2026-11-22 コンボ第8ラウンド（review 4 + discovery 3 担当）: 全7担当完了。
  R6 で重複排除・Runtime Proof 確認のうえ新規28件（高6/中20/低2＋保留1）を追記。
  高優先は AGE_* 非UTF-8 fail-open、ALLOWED_ORIGINS 起動ゲート欠落、本番UIのモック認証、
  shfmt -d の実効性欠如、AUDIT_EXPECT_ROOT 更新ライフサイクル、prepare-distribution 破壊範囲検証。
  k-匿名性は speculative のまま保留、国別しきい値は DEC-02 法務待ちで却下せず据え置き。
  次は高優先から coding。残り高8・中20・低9。

- 2026-11-22 coding（低優先5件）: EXPORT-11/REPORTS-ERRの解析失敗フェーズを既存E2E内へ、
  監査Reason注記を到達可能な表現へ。ADR-0011/0012を実装実態へ同期（Proposed維持）、
  doc-numbersコメントの根拠を実態へ修正。親がコメント内test()の誤計測を修正。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests123）。残りは高2のみ。

- 2026-11-22 coding（中優先7件）: /auth/refreshへOrigin完全一致＋cross-site拒否、
  nullオリジン拒否。XFF非UTF-8 fail-closed、safety copy 0600、systemd/cron排他明記。
  sha256便宜crate削除(sha2+hex)、doc-numbersへclientテスト/JPKI10件/advisory12件照合、
  CIへシェル静的ゲート（apt_packages導入・誤検知除外）。
  親がAppState上書き・doctor null期待・E2E Sec-Fetch-Site欠損・shellcheck誤検知を修正。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests123）。残り中0・低5・高2。

- 2026-11-22 coding（中優先4件）: AUDIT_EXPECT_ROOTを共有hexバリデータで起動ゲートへ昇格、
  JWT_REQUIRE_KID検証をDB接触前へ移動、公開/運用者集計をbegin_write_tx単一スナップショット化、
  TZオフセット判定とPINATA_JWT分類をEnvValueベースの単一情報源へ集約。
  pub use self重複(E0255)とmarkdownlint行長は親が修正。全ゲート green。残り中7・低5・高2。

- 2026-11-22 coding（高優先3件）: 本番リモートDSNを拒否しis_remote_database_urlを単一情報源化、
  deny受容根拠を「ゲートで禁止」へ変更（件数11→12は履歴として保持）。
  アップロード/通報の失敗をdedicated role=alertへ分離（固定文言不変）。
  バックアップは停止→backup→起動の窓unitを追加し失敗時も再起動を保証。
  前ラウンドのリモート失効DB正本照会は「複数プロセス對策」として統合し矛盾を解消。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests123）。残りは高2・中11・低5。

- 2026-11-22 coding（高優先3件）: リモートDB失効をDB正本照会+positiveキャッシュへ、
  instance lock読取をfail-closed化、maxlengthをUTF-8 byte基準へ（common定数集約）。
  me_exportテストのrevoked_subsシードとDB正本照会が衝突したため専用JWTへiat付与で解消。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests123）。残りは高5・中11・低5。

- 2026-11-22 コンボ第3ラウンド（review 4担当 + discovery 3担当）: 全7担当完了。
  R6 で重複排除・Runtime Proof 確認のうえ新規25件（高7/中11/低5）を追記。
  高優先はリモートDBのJWT失効メモリ閉じんめ・ロック読取失敗の奪取・deny ignoreすり抜け・
  role=status/alert矛盾・UTF-16/UTF-8 maxlength・定期バックアップのinstance.lock衝突・
  脆弱性受容件数不一致。speculativeのk-匿名性は候補として保留。次は高優先から coding。

- 2026-11-22 coding: 手動 finalize_report へ require_moderation_operator を追加
  （未設定/未登録/BAN済み403、入力検証前）。スケジューラ経路は認可対象外で自動確定継続。
  list_reports は ADR-0004:114 / ADR-0008 の要約公開契約を維持するため無変更。
  全ゲート green（E2E69/check23/doctor53/migrations14/tests118）。

- 2026-11-22 coding: 本番相当では DATABASE_URL を全プロセス必須・既定なしへ。
  サーバーは read_env 由来で未設定/非UTF-8/空/旧既定の相対 file:local.db を起動拒否、
  doctor production が同一判定警告（check23不変）。受理値と接続URLの trim を統一。
  開発(mock有効)は従来どおり。全ゲート green。

- 2026-11-22 coding: common::env に EnvValue/BoolEnvState 分類器を新設し、起動時・
  per-request・OIDC validation・CLI の全経路で非UTF-8を「未設定」ではなく不正値として
  fail-closed に統一。OIDC_MAX_AUTH_AGE_SECS の非UTF-8がauth_time検証を黙って無効化する
  fail-open も R4 で発見・是正。check23/doctor53/E2E69件は不変、全ゲート green。

- 2026-11-22 coding: 検証済み年齢の user_hash を issuer+signed_subject 由来の sha256(64-hex)へ
  是正。nonce/birthdate/country は除外、空 subject は二段目403。OIDC callback は sub 生値の
  別契約として維持。pairwise 性は実証明書接続前に ADR で確定する前提。全ゲート green。

- 2026-11-22 コンボ第2ラウンド（無制限・review 4担当＋discovery 3担当）: 全7担当完了。
  R6 で重複排除・Runtime Proof 確認のうえ、新規21件（高5/中10/低6）を未着手へ追記。
  高優先: 検証済み年齢のuser_hash主体衝突・非UTF-8環境変数のfail-closedすり抜け・
  DATABASE_URLサーバー既定とCLIの矛盾・cron MAILTO未差し替え・JWT期限切れの導線欠如。
  却下: sha256→sha2統一（既知判断と重複）・npm test workers=1（CI側対応済み）・
  client logout Cookie（speculative・root cause未確定）。次は高優先から coding を継続。

- 2026-11-22 swarm コンボ（無制限・R1 5担当＋R2 5担当＋R4 2担当＋R5 3担当）:
  parse_bool_env を6フラグ13箇所へ統一（fail-closed・check23/doctor53不変）、OIDC設定判定を
  非秘密の validate_oidc_config_with へ集約、検索条件とSPA viewをURL/historyへ同期、
  ドキュメント旧値を履歴化、finalizerメトリクス/client-events仕様を追記、
  支援オーバーレイのフォーカス到達・UTC日時整形・スキップリンクを実装。
  全ゲート green（E2E69/check23/migrations14/tests117）。E2E69件・数値は不変。
  残1件 DEC-01実入力経路は外部OP/JPKIのクレーム仕様確定待ち（adapter土台は完了済み）。

- 2026-11-21 swarm-unlimited（hy4-flash-free 無制限キャンペーン・並列8/深度1・R1 8担当＋R2 8担当
  ＋R4 2担当＋R5 2担当）: 「後方互換」と誤称されていた判断を撤回し、設定契約を是正。
  AGE_POLICY_CONFIGURED 廃止→configuration_satisfied 導出、DB接続3CLI未設定時起動拒否、
  価格ゲートをサーバ契約 0..=1_000_000 へ整合、lib.rs の export を分割（5140→4689行）、
  DEC-01 verified_age adapter / DEC-04 ADR-0008 / JPKI PoP mock 結線 / migration 014 /
  payment_orders 却下 ADR-0009 を完了。全ゲート green（E2E69/check23/migrations14/tests115）。
  残7件は外部仕様確定・設計判断・大規模変更の残作業。

- 2026-11-21 swarm コンボ完了: review(7担当)+discovery+coding を9サイクル・起動38/上限40で実行。
  最終レビューで新規の安全なローカル項目0件のため停止。残15件は外部仕様(DEC-01/04)・
  後方互換判断・大規模リファクタ・却下済み。全ゲート green（E2E69/check23/doctor53/migration13）。

- 2026-11-21 コンボ初回（swarm-project-review + swarm-todo-discovery、7担当・深度1）:
  約60件の指摘を重複排除・Runtime Proof で裁定し、新規17件を上記へ反映。
  却下: JPKI PoP/VerifiedAgeAttribute 未結線（外部仕様待ちで既記録・型先行済み）、価格 min/max
  未接続（E2E契約で記録済み）、部分UNIQUE/refresh migration（却下・チェックサム保護）、
  lib.rs god module 分割（前回レビューで大規模リファクタとして却下済み）、TRANSPARENCY
  DASHBOARD/公開範囲（DEC-04 要判断で speculative）、JWKS サイズ上限（speculative）。

- 2026-11-21 (swarm-limited mixed 完了): 2イテレーション・起動15/上限40で停止。
  dogfood/ux/ops の新観点と型先行候補を消化後、最終レビューで新規の安全なローカル項目は0件。
  残6件は外部仕様/却下/既対応/バイナリ要件で、結線・スキーマ変更・配信設計はユーザー判断待ち。

- 2026-11-21 (swarm-limited mixed 第1イテレ): dogfood/ux/ops の新観点で実ユーザー導線を発掘。
  監査失敗後再取得・LOG OUT 導線・Search 即時検索・投票中ステータス・運用記述5件を完了。
  E2E 65→66件（auth 3件）。起動10/上限40。
- 2026-11-21 完了（型・テスト先行）: JPKI所有証明の ProofOfPossession + 純粋署名検証 + 4テスト、
  E2E_PROJECTS条件付き選択関数 + 3テスト、VerifiedAgeAttribute 型と境界テスト、refresh_tokens
  RFC3339発行契約テスト、payment_orders正当複数注文の回帰固定。いずれも認証パス/DB制約へは未結線。
  起動累計14/上限40。E2E 66→69件。
- 2026-11-21 進行中（外部仕様確定後に結線する項目）: JPKI所有証明の
  PresentationEnvelope型+純粋署名検証+フィクスチャ、E2E3ブラウザの条件付き選択関数、
  refresh_tokens の RFC3339 契約テスト、payment_orders の正当な複数注文の回帰固定、
  VerifiedAgeAttribute 型とフェイルクローズ境界テスト、R5-05/R5-08 の内部データ型。
  いずれも外部仕様/設計判断が確定する前の安全な部分のみ。

- 2026-11-21 (swarm-limited 完了): 2 イテレーション・起動17/上限40で停止。残6件は外部仕様
  (DEC-01/R5-05/R5-08/JPKI所有証明)・却下(payment_orders部分UNIQUE)・既対応(refresh_tokens
  はdatetime正規化済み)・バイナリ要件(Firefox/WebKit)で、安全なローカル項目は0件。配信側CSPは
  本番配信構成の設計判断が先のため別タスクとして残す。

- 2026-11-20 (swarm-limited 第2イテレーション): テスト限定 GPG 往復検証（ケース8）と API 側 CSP
  を完了。R4 が TASK_INVENTORY の「CSP は未設計」残存を指摘し是正。起動 16 担当 / 上限 40。

- 2026-11-20 (swarm-limited): 未着手を再評価し、ページネーション段1・監査GET削減・doctor strict
  ローカルゲートを完了、SHA固定は方針矛盾でクローズ。CSP・復号検証は設計判断のため条件付きで残す。
  起動 11 担当 / 上限 40、連続イテレーション 1 / 上限 3。

- 2026-11-20: prepare-distribution成果物を公開用権限(dir755/file644)へ修正。
  umask077は生成中のみ維持し公開直前にchmod、実行ビット付きライセンスも644へ
  正規化（ケース7で検証）。backup-rotateの0600契約と分離。

- 2026-11-20: prepare-distribution.shを私有tmp生成＋atomic公開へ変更。失敗時に既存の完全な
  成果物が不変であることをケース6で検証。R4指摘（危険out_dir正規化拒否・SIGINT/TERM・
  old削除失敗時エラー・ケース4/5のオラクル弱さ）を反映。

- 2026-11-20: AGE_TARGET_REGIONSをドキュメント約束へ整合（小文字大文字化＋英字2文字検証、
  不正値はfail-closed起動拒否・値は非エコー）。既存小文字設定は大文字化して互換維持。

- 2026-11-20: doc-numbers堅牢性3件（sign_audit前方切り詰め削除・backup triple全マッチ照合・
  doc_bin_counts履歴除外のマッチ部分限定）を反映。R4のL49誤除外指摘は隣接制約復元で解消。
  破壊テスト（古いtriple・L49単独改変）でいずれもFAIL・復元PASSを確認。

- 2026-11-20: doc-numbersへbackup回帰期待数(28+12=40)とsign_audit_export10件の
  他ドキュメント(HANDOFF/TASK_INVENTORY)との照合を追加。破壊テストで expect_failure
  29件→HANDOFF/TASK_INVENTORYの両方でFAILを確認。履歴除外はマッチ部分前方30文字に限定。

- 2026-11-20: doc-numbersへE2E総数の未注記・古い件数を検出するfail-closedゲートを追加。
  MOD-07/EXPORT-10の59件残存を注記し、注記キーワード/正規表現の不整合（現在59件の誤救済、
  外側5〜6文字prefixの未走査）を是正。破壊テストで2通りともFAILを確認。

- 2026-11-20: AGE真偽値を1/true/0/falseのみへ絞りfail-open防止（yes/on/no/off拒否）、
  AGE_ENFORCEMENT_MODEの全受理値をエラー文言へ明記。cronラッパーの不正行/不正キーを
  行番号のみの固定文言へ（cronメールへの値漏洩防止）。
- 2026-11-20: doc-numbersのCLI-01群名照合を双方向化（重複・欠落・非テストfn誤受理を検出）、
  HANDOFFのゲート件数を実測679件（common66/server536/client57/restore20）・E2E65件へ更新。

- 2026-11-20: restore_db の一時ファイル削除結果を伝播し残置を通知。safety_copy=None /
  置換成功 / 削除失敗の3分岐をオラクル化。doc-numbers へ CLI-01 群テスト名の実在照合を追加
  （46名すべて doctor.rs の fn と一致、typo で fail-closed）。

- 2026-11-20 (impl-cli01-breakdown): CLI-01 群内訳テーブルの引数パースを6→8件へ是正
  （`parse_strict_with_help_requests_help` / `parse_help_with_strict_requests_help` の2件を追記）、
  合計44→46件を実 #[test] 46件へ整合。`scripts/test-doc-numbers.sh` へ群テーブル件数の
  合計照合を追加し、抽出失敗は fail-closed で FAIL とする。

- 2026-11-20 (impl-docnum-allbins): doc-numbers の bin テスト数照合を3 bin から全7 bin
  （backup_db / doctor / restore_db / sign_audit_export / verify_audit_export /
  verify_audit_log / verify_backup）へ拡張。記載なしで FAIL になるため、新規4 bin は
  TEST_SPECIFICATION へ「運用 bin のテスト数一覧」を、DETAILED_DESIGN へ CLI 件数表4行を追加。
  陳腐化件数は sign_audit_export 9→10（6箇所）・backup_db 7→12（うち生存ロック検査は7件）へ是正。
  実測 verify_backup は19件（指示書の12は backup_db との取り違え）で既存記載どおり変更なし。
  check23 / E2E65件 / migration13 / doctor46 は不変、fail-closed 維持。

- 2026-11-20: CIバージョン閾値の fail-open（"08" が8進数解釈で比較 false、2^64+1 が
  オーバーフロー）を修正。assert_major_numeric に先頭ゼロ拒否と桁数＋辞書順上限、
  比較側へ 10# 化を追加しオラクル(f)(g)(h)。E2E 65件（build-gate 15件）。

- 2026-11-20: 最終レビューで doc-numbers の照合漏れ（DETAILED_DESIGN の CLI 件数表・個別
  spec 実行例）を発見しゲートを拡張。doc numbers が green でもドキュメント間矛盾が残る問題を
  根本解消。E2E 62件（build-gate 9件）・バックアップ35件（当時、現在37件）・CLI 45/20/19 を実測値へ統一。

- 2026-11-20: エラー本文スキーマ統一を却下クローズ（AppErrorは既に{"error":message}統一済み、
  code/message/status化はクライアントのbody.error依存を壊す破壊的変更で利益を上回らない）。
- 2026-11-20: backup暗号文公開順序を修正（先に暗号文mv→平文rm）。公開失敗時に平文が残り、
  当日バックアップの完全喪失を防止。3経路（assert/mv/gpg）でオラクル検証。

- 2026-11-20: real-api E2E の /auth/verify タイムアウトは他 spec との並走による実サーバー競合が
  真の原因。ci-e2e.sh を real-api 単独実行＋残り11spec の2段階へ分離し、失敗時後処理を
  共通関数化（artifact 収集を両ブロックで実施）。52+4=56件 green を確認。

- 2026-11-20: real-api.spec.ts の /auth/verify 待ちタイムアウトは、全E2E実行時のみ再現し
  単独実行（E2E_REUSE=0 E2E_CLIENT_PORT=18125）では4件すべて成功。R4 レビューでも今回の
  差分（AppError::BadGateway・client 文言/busy ガード）と因果関係なしと判定。
  既存の環境依存フレーキーとして記録し、後続タスクで webServer 起動待ちを検討する。

- 2026-11-19 却下（Runtime Proof 済み）: 単一ライター・ロックの stale 回収 TOCTOU は
  `server/src/lib.rs:725-733` で再読込一致チェック＋create_new の AlreadyExists 拒否が既にあり、
  同時起動でも排他が破れないため却下。GOV-01 要約 E2E の生PII→[redacted] は
  `server/src/lib.rs:1886` のサーバー側 summarize_reason が担い、クライアントは生表示しないため
  E2E でのオラクル変更は実装と矛盾し却下（Rust 単体/property テストが担保）。

- 2026-11-19 全体レビュー（swarm-project-review + swarm-todo-discovery、15担当・深度1・無制限プロファイル）:
  約100件の指摘を重複排除・Runtime Proof で裁定し、上記へ反映。主な却下: lib.rs god module 分割・
  AppState FromRef 分割・ドメインエラー型導入（大規模リファクタで既存ゲートとのトレードオフ判断が必要）、
  finalize_report の運用者チェック（設計判断済み）、confidence=speculative の将来リスク
  （短い JWT_SECRET 露出、Merkle root 保存経路、ロールバック対応関係）は候補として保留。
- セッション JWT の iss/aud 検証は「別途要件化」で未定義（DEC 判断 cần）、TrustedAttributeRequired の
  実入力経路は DEC-01 外部待ちのため既存未着手を維持。

- 2026-11-18 時点: 作業ツリーはクリーンで、以下に「未コミット」とある記述は各作業当時の履歴。
  現行状態はすべてコミット済み（最新 `dd9aa7c`）。E2E 定義数は56件（12 spec）。
- 詳細な残タスク棚卸しは `docs/TASK_INVENTORY.md`、レビュー履歴は `docs/REVIEW_REPORT.md`、
  未知タスク調査は `docs/UNKNOWN_TASKS_INVESTIGATION.md` を参照。
- 本ファイルにはクロスレビューで統合・採用した項目を追記する。
- P0 の R1-17 / R1-18 / R2-09 はコード上確定（Runtime Proof 済み）。
  他の項目はレポートの静的推論を含むため、着手時に事実確認する。
- 今回の反映は未コミットの `server/src/lib.rs` / `oidc.rs` / `tests.rs` と
  `client/src/lib.rs` を grep した確定値に基づく。
  E2E 定義数40件は `e2e/tests/*.spec.ts` の `test()` 定義数を実測して確認した。

- 2026-11-20 (R5 / セキュリティレビュー採用指摘の反映): OIDC state Cookie の `__Host-`
  プレフィックスを本番（`OIDC_COOKIE_SECURE` 有効）のみへ条件付き採用（mock は Secure
  が無いとブラウザが保存を拒否しフローが壊れるため旧名を維持）、同一名 Cookie の重複
  拒否、`doctor` 警告文を session / refresh / OIDC state を含む表現へ更新。`__Host-`
  を常時使わない理由はコードコメントに明記。E2E 定義数は不変（変更はサーバー側のみ）。
  `docs/OPERATIONAL_POLICY.md:263` の `OIDC_COOKIE_SECURE` 説明は `0337d64` で更新済み（クローズ）。
- 2026-11-20 (R5 / R4 採用指摘の反映): 採用分の OIDC 空値検証・doctor max_auth_age 整合・
  shutdown 2件・シェル2件・filechooser オラクルは全て既に `[x]` であり、該当する未着手は
  存在しなかった。本次で doctor 内訳の「＋設定検証3件」6箇所（TASK_INVENTORY 3箇所 /
  TEST_SPECIFICATION 2箇所 / 本ファイル`:22` の 40→43 テスト）、CLI-01 設定判定テスト名
  34件目（`oidc_max_auth_age_mock_with_complete_oidc_warns_without_disclosing_value`）、
  backup-rotate の RETENTION_DAYS 境界（当時 36500 受理 / 36501・100000 拒否。
   R14 で上限を 20000 へ変更済み、末尾メモ参照）を反映。
  未対応のまま残すもの（本次で確認した未着手）: `doctor --strict` のデプロイ前ゲート
  組み込み（`:239`）、backup-rotate の verify_audit_log と暗号化後復号検証の追加
  （`:241`、チェックボックス表記が `- []` で不正）、`doctor --strict --help` 併用の
  他CLI整合（`:368`）。

- 2026-11-20 (R14 / ops-docs 発掘3件の反映): `scripts/backup-rotate.sh` の
   `RETENTION_DAYS` 上限を 36500 → 20000 日へ変更。旧上限では
   `cutoff = now - 36500*86400` が負になり `mtime < cutoff` が常に false で
   古い世代が削除されない（fail-open）ため、`now/86400` より小さい値へ制限し、
   cutoff が非正になる場合も明示拒否する防御チェックを追加。境界は
   「20000 受理 / 20001・100000 拒否」へ更新し、上限値で mtime を 1970 年代に
   設定した古い世代が実際に prune されるテストを追加。定数・テスト・
   `docs/BACKUP_RESTORE_RUNBOOK.md` の記載をすべて整合させた（上記の
   36500 は R5 当時の履歴）。`scripts/test-backup-rotate.sh` の実測は
   `expect_failure` 28件＋`expect_success` 12件=40件（4fb80e0 で root 不一致オラクル1件を追加、
   R14 時点は 27＋12=39件）。`docs/TASK_INVENTORY.md:481` / `docs/HANDOFF.md:262` の
   「5件・5件」を是正。
   `docs/OPERATIONAL_POLICY.md:265` の `PINATA_JWT` を本番必須・実トークン必須
   （未設定/空/空白/mock_jwt 前後空白含むは起動拒否、`doctor` の `production`
   カテゴリで診断）へ更新し `docs/DETAILED_DESIGN.md:123` と整合。
   今回の発掘分は3件すべて対応済みで、新たな未着手はなし。

- 2026-11-20 (R5 / R4 採用指摘の反映): `doctor --strict` への `AUDIT_EXPECT_ROOT`
  警告追加は 2026-11-20 対応済み。`doctor` に独立カテゴリ `audit_expect_root` を追加し
  22→23 カテゴリ・45→46 テストへ更新。本番相当（mock 無効）で未設定・空白のみ・不正 hex
  なら固定文言の警告（値は非エコー、既定は警告のみで `--strict` 時のみ失敗）。
  共有の `production_config_errors` と `backup-rotate.sh` の未設定時契約は変更しない。

- 2026-11-20 (R5 / R4 採用指摘の反映・4件): `doctor` の `AUDIT_EXPECT_ROOT` 判定を
  `backup-rotate.sh` と統一し、`lookup` の生値へ `^[0-9a-fA-F]+$` を適用（trim 廃止）。
  前後空白付き hex は警告対象（値は非エコー）。境界テストの有効 hex 配列から
  `"  ABCDEF0123456789  "` を除外し警告ケースへ移動（doctor テスト数は46のまま）。
  TEST_SPECIFICATION の E2E 59件表記6箇所へ「当時59件、現在65件」を注記（当時の"現在"=2026-11-20時点、以降 11-21時点66件→現在69件）し、
  `23 check(s)` 実測の出所を 2026-11-20 再確認へ更新。DETAILED_DESIGN の
  フォールバック5変数（新規カテゴリ2件）と 19→21→22→23（カテゴリ数）の内訳を分離。

- 2026-11-20 (R5 / 採用指摘の反映): 実バイナリ起動テストの指摘は見送り。親が Runtime Proof で
  実 stderr の非エコー（値0回）を確認済みであり、純粋関数オラクル＋固定 `eprintln!` により
  実装上の漏えい余地が無いため、実バイナリ起動テストはコスト対効果が低いと判断した。
  併せて AUDIT_ROOT_MISMATCH を `common/src/lib.rs` の `pub const` へ単一化し両 CLI から参照、
  restore_db の safety copy 復元成功時に「safety copy was restored successfully from `<path>`」を
  併記して成否を観測可能とした（失敗時文言・`safety_copy:` 出力・instance lock・引数契約は不変）。

- 2026-11-20 (R5 / 採用指摘の反映): `redact_url` のパース失敗時フォールバックを、先に
  `scheme://` 直後の authority（最初の `/` まで）を切り出して userinfo を除去し、その後に
  クエリ/フラグメントを除去する順序へ是正。userinfo 内に `?` / `#` を含む
  `https://user:pa?ss@exa mple.com/path` でも userinfo が残存しない回帰テストを追加。
  スキームレス URL（`//` 始まりを含む）は authority 走査の対象外として userinfo を除去しない
  （クエリ/フラグメントは従来どおり除去）判断を doc コメントへ明記。
  `scripts/test-doc-numbers.sh` は E2E 総数ゲートの修飾語上限（`{0,40}`）を撤廃し、
  bin 照合を `server/src/bin/*.rs` の実ファイル列挙へ変更（未知 bin は既存分岐で fail-closed）。
  check23 / E2E65件 / migration13 / doctor46 は不変。

- 2026-11-20 (impl-bincounts-final): doc_bin_counts の bin 名⇔数値間修飾語上限
  （`[^0-9]{0,8}`）を撤廃し `[^0-9]*` へ変更（E2E 総数側 total_count_pattern と方針統一、
  TEST_SPECIFICATION:84 の10文字空きを走査対象化）。TASK_INVENTORY の現行記述2箇所
  （:36 / :338）の「bin 内に9テスト」を実測10テストへ是正。check23 / E2E65件 /
  migration13 / doctor46 / sign_audit_export10 は不変。除外ロジック・全7bin照合・
  DETAILED_DESIGN CLI表照合・E2E/doctor check/migration/群名双方向/総数未注記ゲートは維持。
