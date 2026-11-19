---
title: "クロスレビューレポート"
type: Review Report
description: 設計文書とコード変更のセキュリティ、構成、運用レビュー結果を記録する。
tags: [review, security, architecture, operations]
generated:
  by: dsh
  at: "2026-11-20T08:57:45+09:00"
---

## クロスレビューレポート

- 対象: README、`docs/*.md`、`docs/adr/ADR-0001-mvp-trust-boundaries.md`
- レビュー観点: セキュリティ、アーキテクチャ、運用保守
- 結果: **条件付き承認**
- 検証: Markdownlint、`git diff --check` 成功

## 主要結果

3視点でレビューを依頼した。セキュリティレビューでは、必須認証・決済を迂回するフォールバックは指摘されなかった。アーキテクチャレビューでは、Wasm SIMD等の未記載前提は採用せず、機能検出・代替表示の具体化を反映した。今回運用レビューも実施し、既存の開発専用警告・プロキシ信頼境界の記載を確認した。追加でテストfixtureのスキーマ差分を確認した。

後続の整合性確認で見つけた未認証読み取りAPIはJWT必須に変更した。raw HTML描画によるXSSにはエスケープ処理と回帰テストを追加した。JPKI連携は未実装のため、本人確認要件の達成とは扱わない。

今回の追加変更に対するサブエージェントの再レビューは、利用枠上限により完了できなかった。読み取りAPIの要件・例外・受け入れ試験、およびOSSライセンス未指定は手動照合で追記している。

ユーザーが明示した必須方針（指定マイナアプリ/JPKI認証、JPYCのみの有償取引、第三者検証可能な監査、Rust優先/Axum、OSS、反検閲、高速・軽量・低手数料、クロスプラットフォーム性）を、未実装を理由に対象外・任意機能へ後退させていない。

## 指摘と対応

### [High] JPKI検証結果とAPIセッションの結合

- **根拠:** [要件仕様書](REQUIREMENTS_SPECIFICATION.md) の認証要件と
  [詳細設計書](DETAILED_DESIGN.md) の目標認証フロー。
- **レビュー指摘:** JPKI提示の検証結果がセッション/JWT主体へ暗号学的に結び付かなければ、クライアント申告IDや別セッションへの置換を防げない。
- **対応:** 要件・ADR・詳細設計・受入テストに、サーバーが検証済み認証トランザクションにセッションを結合し、クライアント申告IDを信頼しないことを明記。
- **残課題:** 公式プロトコル、安定識別子の生成、プライバシー保護は未決定・未実装。現行プロトタイプを本番認証に使わない。

### [High] JPYC決済の執行点

- **根拠:** [要件仕様書](REQUIREMENTS_SPECIFICATION.md) の決済要件と
  [詳細設計書](DETAILED_DESIGN.md) のJPYC目標フロー。
- **レビュー指摘:** JPYCのみという方針だけでは、非対応チェーン・別トークン・別金額の支払い後に有償機能を解放する誤りを防げない。
- **対応:** 許可チェーン・トークンコントラクトを識別し、取引要求ID・金額・受取先との一致と最終性を確認した後だけ有償機能を解放する要件とテストを追記。
- **残課題:** 対応チェーン、確認数、再編成、失敗・返金、手数料基準は未決定・未実装。

### [High] 第三者検証可能な監査の具体化

- **根拠:** [ADR-0001](adr/ADR-0001-mvp-trust-boundaries.md)、[基本設計書](BASIC_DESIGN.md)、[詳細設計書](DETAILED_DESIGN.md)。
- **レビュー指摘:** 監査ログの生成主体、完全性・順序の検証方法、公開範囲、個人情報分離の具体方式が未確定。
- **対応:** DB内ハッシュ連鎖を最終要件とみなさず、署名・外部検証手段・鍵管理・公開範囲・個人情報保護を必須設計項目とした。
- **残課題:** 形式、アンカー/追記媒体、削除・訂正要求との調整は未決定。運用者は各デプロイが設定する（プロジェクト側で固定しない）。

### [Medium] 性能・反検閲の測定と脅威モデル

- **根拠:** [ADR-0001](adr/ADR-0001-mvp-trust-boundaries.md)、[アーキテクチャ図](ARCHITECTURE_DIAGRAM.md)、[要件仕様書](REQUIREMENTS_SPECIFICATION.md)。
- **レビュー指摘:** 高速・軽量・低手数料は測定値がなく、遮断・停止の脅威モデルも未確定。
- **対応:** 達成済みと主張せず、初期表示、転送量、Wasmサイズ、可用性、運用費、取引手数料の指標と脅威モデルを未決事項として記録。
- **残課題:** SLO・受入閾値、複数ゲートウェイ/ピン留め/ミラーの構成は未決定。

### [Medium] クロスプラットフォーム性の対象環境

- **根拠:** [要求定義書](REQUIREMENTS_DEFINITION.md)、[要件仕様書](REQUIREMENTS_SPECIFICATION.md)、[基本設計書](BASIC_DESIGN.md)、[テスト仕様書](TEST_SPECIFICATION.md)。
- **レビュー指摘:** Rust/Wasm採用のみでは、OS/ブラウザ差、JPEG XLデコード、JPKIアプリへの遷移、ウォレット接続の互換性を保証しない。
- **対応:** 複数OS・端末・ブラウザ対応を目的に追加し、環境マトリクス、実機試験、セキュリティ要件を迂回しないフォールバックを要件化。
- **対応:** アーキテクチャ図とテスト仕様に、JXL/Wasm機能検出、非対応時の案内・代替表示、必須認証・決済を迂回させない条件を追加。
- **残課題:** 対応対象・最小バージョン・代替描画方式・アクセシビリティ基準は未決定。

### [Medium] 本番運用の設定・ライフサイクル手順

- **根拠:** [運用ポリシー](OPERATIONAL_POLICY.md)。
- **レビュー指摘:** シークレットの検証・ローテーション、ヘルスチェック、依存サービス障害、SIGTERM時の処理が運用要件として不足。
- **対応:** 起動時fail-closed設定検証、秘密管理・ローテーション、readiness/liveness、依存障害、graceful shutdownを本番化前の整備事項として追記。
- **残課題:** 実装、Runbook、障害訓練は未完了。

### [High] OSS公開ライセンスが未指定

- **根拠:** リポジトリルートにライセンスファイルが存在しない。
- **影響:** OSSを目指す方針は記載されているが、利用・改変・再配布条件を
  利用者に提示できず、OSS公開準備が完了したとは言えない。
- **対応:** [文書・設計判断の確定状況](DOCUMENT_STATUS.md) に
  ライセンス選定と依存ライセンス確認を公開前ゲートとして追加。
- **更新:** `AGPL-3.0-or-later` を確定し、GNU公式本文をルートの `LICENSE` に追加した。
- **残課題:** 適用範囲の明示と依存ライセンス確認は未完了。

### [High] 未認証の読み取りAPI

- **根拠:** [詳細設計書](DETAILED_DESIGN.md) の現行ルート一覧と認証要件。
- **影響:** コンテンツ一覧・取得、監査、取引、通報情報の読み取りルートが未認証で公開され、閲覧にも本人確認を必須とする方針を満たしていなかった。
- **対応:** 全読み取りルートにJWT検証を追加し、未認証拒否と認証済み一覧取得のテストを追加。認証開始とhealth確認は限定例外として残す。
- **残課題:** JWTはJPKI本人確認を表明しない。JPKIとJWTセッションの安全な結合は未実装で、本番公開ゲートとして残る。

### [High] Markdown raw HTMLによるスクリプト実行

- **根拠:** クライアントはMarkdown出力を `inner_html` で描画していた。
- **影響:** raw HTMLを含む投稿が、そのままDOM要素として解釈される可能性があった。
- **対応:** pulldown-cmarkのHTMLイベントをテキストイベントへ変換してからHTML化し、タグをエスケープする回帰テストを追加。
- **対応:** URLリンク先も検証し、`http` / `https` とリンクの `mailto`、およびスキームとして成立しない接頭辞（相対URL・フラグメント）のみを許可する。その他のスキームは `#` に置換し、危険リンク・画像URLと安全URLの回帰テストを追加。
- **残課題:** ブラウザ別URL解釈のE2E確認。

## 指摘のトリアージ

- ゼロ知識証明、エスクロー、ウォーターマーク等、文書に記載も要求もされていない方式を前提とする指摘は採用しなかった。
- Rust/Axum採用自体をセキュリティ・性能保証とみなさず、入力上限、認証、運用制御を別要件として扱う。
- Wasm SIMDが必須という指摘は文書に要件がなく、採用しなかった。JXL/Wasmの実機互換性試験自体は必須要件として追加した。
- READMEと運用ポリシーは、モック認証を開発専用とし、実JPKI/JPYC未実装の間は本番利用しないことを既に明記している。運用レビューの同趣旨の警告追加は重複として新規指摘にしなかった。
- `INFRA_NETWORK.md` と運用ポリシーは、未検証の `X-Forwarded-For` と信頼プロキシ設定を本番前の課題として既に記載している。これは文書欠落ではなく、未解消の実装・配置リスク。
- `AGE_VALIDATION_POLICY.md` は現行マッピングをプロトタイプと明記しており、年齢属性が本人確認されていない点も記載済み。
- ライセンスは `AGPL-3.0-or-later` に確定し、GNU公式本文を `LICENSE` に追加した。依存物のライセンス確認は別途必要。
- **[Resolved / code-test consistency]** 読み取り認証済み一覧テストの追加時に、
  `server/src/tests.rs` のfixtureへ本番DDLと同じ `price` 列を追加した。
  これにより現行の一覧取得クエリがテストDBでも実行可能になった。
- 監査にゼロ知識証明を必須とする根拠は確認できないため採用していない。個人情報の非公開と第三者検証可能性は必須要件として残す。

## 総評

文書はプロダクト方針と現行実装の差を明確にする段階まで整合した。ただし必須要件の多くは未実装であり、この文書レビューは本番準備完了を意味しない。JPKI、JPYC、監査、配信分散、性能・費用の設計確定と、それぞれの試験・運用承認をリリースゲートとする。

## 変更差分クロスレビュー（2026-10）

- 対象: `f004c8a..4b809b6` の15コミット（マイグレーションランナー、入力検証強化、
  CORS設定の外部化、エラー公開制御、模擬支援API無効化、JWT鍵検証、tracing導入、
  CI保守、ライセンス宣言）。
- 観点: セキュリティ、アーキテクチャ、運用保守（サブエージェント3体で並列レビュー）。
- 結果: **条件付き承認**（Critical 0件。High指摘は現行実装の確認により
  文書化・小規模修正の課題へ整理）。

### 指摘と対応

#### [Medium] CORS設定値の正規化不足（アーキテクチャ視点）

- **指摘:** 末尾スラッシュ付きオリジンはブラウザの `Origin` と一致せず、
  ワイルドカードの扱いも未明文化。
- **対応:** `ALLOWED_ORIGINS` 解析で末尾スラッシュを除去し回帰テストを追加。
  ワイルドカード非対応を詳細設計へ明記。
- **残課題:** なし。

#### [Medium] グレースフルシャットダウン未実装（運用視点）

- **指摘:** SIGTERM/SIGINT で処理中リクエストが切断される。
- **対応:** `axum::serve` へ `with_graceful_shutdown` を追加（unix: SIGTERM、
  全環境: ctrl-c）。実起動でシグナル受信・drainログ・終了コード0を確認。
- **残課題:** readiness連動とdrainタイムアウトは本番前に決定。

#### [Medium] マイグレーションの原子性・排他・復旧（アーキテクチャ/運用視点の仮説）

- **指摘:** 複数ファイル適用の原子性、同時起動時の競合、失敗時ロールバック手順が不明確。
- **確認:** 現行実装は1ファイル1トランザクションで適用と履歴記録をコミットし、
  失敗ファイルは記録・適用されない（`server/src/migration.rs`）。
  前方のみでロールバックは提供しない設計。
- **対応:** 単一ライター起動の前提、失敗時の復旧手順（原因修正後の再起動と
  `_migrations` の確認）、排他手段の決定を運用ポリシーへ追記。
- **残課題:** 複数インスタンス展開時の排他手段。

#### [Medium] 環境変数の網羅的一覧の欠如（運用視点）

- **指摘:** 全環境変数の必須・既定・本番推奨値の一覧がない。
- **対応:** 運用ポリシーに環境変数リファレンスを追加（秘密・開発専用の区別を明記）。
- **残課題:** なし。

#### [Low] ログへの機微情報混入（3視点共通の仮説）

- **指摘:** 内部エラーログに機密が含まれ得る。
- **確認:** 出力するのは内部エラー詳細とシグネチャ検証エラーに限られ、
  JWT・トークン・リクエスト本文は出さない。
- **対応:** ログ方針（機微情報を出さない）を詳細設計へ明記。
- **残課題:** なし。

### 却下候補

- マイグレーションのDownスクリプト不在は前方互換のみの設計であり、
  機能欠陥として扱わない。
- 模擬支援APIの501無効化は要件に基づく意図的な設計。
- IPFSゲートウェイのPinata置換は停止サービスへの保守対応。
- マイグレーションSQLの単純な `;` 分割は、リテラル内セミコロン禁止の制約が
  `migration.rs` に文書化済みで、現行SQLは制約に適合する。
- JWT鍵規則が詳細設計と運用ポリシーに重複するのは役割分担による意図的な記載。

### 検証

- ワークスペーステスト 49件（client 3 / common 5 / server 41）成功。
- `cargo clippy --workspace --all-targets -- -D warnings -A clippy::module-inception`
  警告なし、`cargo fmt --all -- --check` 成功。
- 実起動: SIGTERMでdrainログ出力後に終了コード0。CORSは有効オリジンの一致と
  無効エントリの無視を確認。

## 変更差分クロスレビュー（2026-10-18）

- 対象: `b19e477..d370715` の6コミット。
  - E2E の git 衛生・可搬化・CI ゲート統合（`f5ae404`〜`7675eab`）。
  - 上記に対するクロスレビュー反映（`4c6e236`、`d370715`）。
- 観点: セキュリティ、アーキテクチャ、運用保守（サブエージェント3体で並列レビュー）。
- 結果: **条件付き承認**（Critical 0 件。High は採用修正または軽減策を実施し、
  残課題を文書化）。

### 指摘と対応

#### [High] E2E 失敗時に CI 上へ診断情報が残らない（運用視点）

- **指摘:** `extra_gates` は単一 `run` ステップのため
  `actions/upload-artifact` を追加できず、trace・スクリーンショット・
  HTML レポートがランナー消滅と同時に失われる。
- **対応:** 独立ジョブ化は共通ワークフローの拡張が必要なため見送り、
  `e2e/ci-e2e.sh` が失敗時に `::error::` アノテーションと Playwright の
  `error-context.md` をジョブログへ出力する。レポート添付が必要になった
  場合は独立した E2E ジョブへの昇格を検討する旨をテスト仕様書へ明記。
- **残課題:** artifact 回収とチェック結果の分離。

#### [High] E2E 失敗の帰属が他ゲートと混ざる（運用視点）

- **指摘:** 追加ゲートは fmt/check/test/clippy と同一ジョブ・同一チェック
  結果のため、E2E 起因の失敗とユニットテスト失敗の切り分けが付かない。
- **対応:** ゲート複製禁止の方針（共通ワークフローへ委譲）を優先し、
  失敗時の `::error::Playwright E2E failed` とステップ名ログで帰属を明示。
  独立ジョブ化は共通ワークフロー側の将来拡張として記録。
- **残課題:** 独立ジョブ・status check 分離。

#### [High] Chrome / Node が未固定で外部変化により静かに崩れうる（運用視点）

- **指摘:** Playwright は lock で固定される一方、`channel: chrome` の
  Chrome と Node はランナーイメージ依存で、差分が追えない。
- **対応:** `e2e/ci-e2e.sh` で trunk / node / npm / Google Chrome の版を
  ログ出力し、`engines.node >= 18` を宣言。Chromium への切替はローカル
  開発（実 Chrome）との乖離が大きいため見送り、記録で追跡する。
- **残課題:** Chrome バージョンの固定方針の決定。

#### [Medium] 型検査がどのゲートにもない（アーキテクチャ/運用視点）

- **指摘:** NodeNext への移行が CI で検証されない。
- **対応:** `e2e/package.json` に `typecheck` を追加し、`e2e/ci-e2e.sh` の
  `npm ci` 直後に `npm run typecheck` を実行。
- **残課題:** なし。

#### [Medium] ローカル E2E の前提・依存更新手順が未文書化（運用視点）

- **指摘:** Google Chrome・trunk・Node 22 以上の前提と、trunk の
  バージョン更新手順がコメントにしかない。README の trunk は版未指定で
  CI と乖離しうる。
- **対応:** README とテスト仕様書へ前提と再現手順を追記し、trunk を
  v0.21.14 に統一。version・sha256・資産名の3点同時更新を手順化。
- **残課題:** なし。

#### [Medium] ジョブ全体のタイムアウトが未管理（運用視点）

- **指摘:** E2E 追加分の所要時間が未計測で、60 分枠超過時に
  切り分け情報ごと失われる。
- **対応:** `ci.yml` で `timeout_minutes: 30` を明示
  （共通ワークフローの入力仕様を実装で確認済み）。
- **残課題:** コールドキャッシュ時の実測に基づく見直し。

#### [Medium] ローカル実行の再現性（永続 DB と再利用）（運用視点）

- **指摘:** `reuseExistingServer` と `server/local.db` の再利用により、
  ローカルだけ落ちる/通る状態や古いバイナリの検証が生じうる。
- **対応:** `E2E_REUSE=0` で再利用を無効化できるようにし、CI は常に
  クリーンな DB・ワークスペースで起動することを文書化。
- **残課題:** E2E 専用の一時 DB は必要になった時点で検討。

#### [Medium] CI の JWT シークレット固定値（セキュリティ/アーキテクチャ視点）

- **指摘:** E2E 用の固定シークレットが CI でも使われる。
- **対応:** `e2e/ci-e2e.sh` が実行ごとに `openssl rand -hex 32` で生成。
  ローカルの固定既定値は開発専用とコメント・文書へ明記。
- **残課題:** なし。

#### [Low] 外部取得の一時障害と固定パス（セキュリティ/運用視点）

- **指摘:** trunk の展開先固定パスと、`curl` 再試行なし。
- **対応:** `mktemp -d` + `trap` による一時ディレクトリ、
  `curl --connect-timeout 10 --retry 3 --retry-delay 2`、
  `.gitignore` の Playwright 生成物パターンのパス非依存化、
  Chrome 検査の PATH / `/opt/google/chrome/chrome` 両対応、
  `npm test` への統一（npx の暗黙ダウンロード回避）。
- **残課題:** なし。

### 却下・先送りした指摘

- 独立ジョブ化と artifact 回収は、ゲートのリポジトリ間重複を作らない方針
  （`rust-gates.yml` へ委譲）を優先して先送り。共通ワークフロー側の
  入力拡張として将来対応する。
- `channel: chromium` への切替はローカル開発との検証乖離のため見送り。
- flake 率の集計、ドキュメントのみの変更での paths 条件、CI での
  HTML レポート無効化は、現段階のテスト規模では効果が小さいため見送り。
- `X-Forwarded-For` の信頼境界は本変更セットの対象外（NET-02）とした。
  別コミットで `TRUSTED_PROXY_CIDRS` による信頼モデルとして対応した
  （実装・レビュー・残存制約は末尾「NET-02 実装セキュリティレビュー」
  節を参照）。
- git 履歴内の node_modules blob は履歴書き換えの破壊性のため除去しない。

### 検証

- CI run `39090972639`: fmt/check/test/clippy + 追加ゲート
  （trunk sha256 検証、`npm ci`、Playwright 9 passed / 18.2s）成功。
- CI run `39092552795`（`4c6e236`）: ランダム JWT シークレット生成と
  型検査を追加した追加ゲートで成功（4m14s）。
- CI run `39092852806`（`d370715`）: 失敗時診断（`::error::` と
  error-context）・依存版記録・`npm test` 統一を反映した追加ゲートで
  成功（4m26s）。
- ローカル検証: `bash -n`、`tsc --noEmit`、YAML/JSON 構文検証、
  `cargo run --locked` 起動、`git diff --check`。

## NET-02 実装セキュリティレビュー（2026-10-18）

`TRUSTED_PROXY_CIDRS` による `X-Forwarded-For` 信頼モデル（NET-02）の
実装に対し、security-reviewer（読み取り専用）レビューを 2 回実施した。
1 回目で Critical 1・High 1・Medium 4・Low 4 を検出して修正し、
2 回目で Verdict: APPROVE（新規 Critical / High なし）を確認した。

- 実装: `server/src/network.rs`（信頼判定・正規化）、`server/src/lib.rs`
  （起動時評価）、`server/src/tests.rs`（統合回帰）。

### 1 回目の指摘と対応

| ID | 重大度 | 指摘 | 対応 |
| --- | --- | --- | --- |
| C-1 | Critical | IPv4-mapped IPv6（`::ffff:a.b.c.d`）が IPv4 判定をすり抜け、dual-stack bind で IPv4 拒否（SEC-01）が発火しない | 実機で修正前の 200 迂回を確認。ピア・XFF・範囲指定・照合対象を `Ipv6Addr::to_ipv4_mapped` で正規化 |
| H-1 | High | 信頼済みプロキシで XFF 解決不能時にピアへフォールバックし、IPv6 プロキシ越しの IPv4 クライアントが通過し得る | 解決不能時は 403（fail-closed）に変更 |
| M-1 | Medium | 多段プロキシでは末尾値が直前ホップのアドレスになる | 単一ホップ前提として `INFRA_NETWORK.md` に記録 |
| M-2 | Medium | 全アドレス範囲（`/0`）が無検証で受理され、不正要素も黙って無視される | プレフィックス長 0・幅超過・不正形式を無効化し、起動時に警告。評価は `OnceLock` で一度だけ |
| M-3 | Medium | 拒否時に理由が記録されない | IPv4 拒否は debug、解決不能は warn。IP アドレスは出力しない |
| M-4 | Medium | 文書が「迂回を解消した」と断定 | 実装に合わせて `INFRA_NETWORK.md`・`OPERATIONAL_POLICY.md` を修正 |
| L-1 | Low | プレフィックス長の暗黙丸め | 幅超過を無効化 |
| L-2 | Low | テストの環境変数依存 | 判定本体を `evaluate_access` に分離し、環境非依存テストを追加 |
| L-3 | Low | リクエスト毎の環境変数再評価 | 起動時評価へ変更し、設定状態をログ出力 |
| L-4 | Low | 開発フラグの本番ガードなし | 本変更の範囲外。本番移行ゲートで扱う |

### 2 回目の新規指摘と対応

- **[Medium] fail-closed とプローブの相互作用**: 信頼範囲内からの
  XFF なしリクエストは 403 となるため、liveness/readiness プローブ・
  監視・内部呼び出しの経路設計が必要。`INFRA_NETWORK.md` に決定事項
  として記録（本番インフラ確定時に決定）。
- **[Medium] 信頼済み経路の自動テスト不在**: 判定本体を
  `evaluate_access` として分離し、fail-closed・IPv4 拒否・許可・
  偽装 XFF 無視を環境非依存で検証するテストを追加。
- **[Medium] プロキシ側の前提（`real_ip` 等）**: クライアント供給
  ヘッダから `$remote_addr` を再定義する構成の制限を
  `INFRA_NETWORK.md` に追記。
- **[Low] v4-mapped 表記の CIDR・v4-compatible 表記・`/1` 分割・
  テストの残存 env 依存**: 実害なしまたは範囲外として記録。v4-mapped
  表記の CIDR は正規形指定の要件として `OPERATIONAL_POLICY.md` に明記。

APPROVE 後の追補（`evaluate_access` 分離、起動ログ改善、文書追記）は、
全テストと実バイナリの再プローブで再検証した。

### 実行時検証（一時 DB・実バイナリ）

- 既定構成: IPv4 直接接続 403（修正前 200）、偽装 XFF でも 403。
- `ALLOW_IPV4_LOCALHOST=1`: IPv4 ループバック 200（開発・E2E 互換）。
- `TRUSTED_PROXY_CIDRS="127.0.0.1/32, 0.0.0.0/0, bogus"`:
  XFF なし 403、`::1` 200、`::ffff:203.0.113.1` 403、
  `::ffff:127.0.0.1` 200、不正 XFF 403。起動ログに無効要素の警告と
  有効範囲数を確認。
- `cargo fmt --all` / `cargo check -p server --all-targets` /
  `cargo test --workspace`（server 51 件を含め全件成功）/
  `cargo clippy --workspace --all-targets -- -D warnings` を通過。

## 変更差分クロスレビュー（2026-10-19）: Markdown URL検証と CONTENT-01 回帰テスト

- 対象: `38a202f..587af30` の2コミット（本節が初回記録）。
  - `32b4f8e` クライアントのMarkdownリンク・画像URL検証と回帰テスト。
  - `587af30` CONTENT-01（許可外MIME・価格）のmultipart API回帰テストと
    テスト仕様書更新。
- 観点: セキュリティ、アーキテクチャ、運用保守
  （読み取り専用サブエージェント3体で並列レビュー）。
- 結果: **条件付き承認**（Critical 0 件）。High 1 件・Medium 3 件・Low 5 件を
  検出（3視点の重複は統合）し、High / Medium と Low の一部を採用して修正した。
  修正は本節より前の `fix(server)` / `test(server)` / `test(client)` / `docs`
  コミット（`b20a389`、`db5cc63`、`d16bd32`、`aff64e0`）に含む。
  H-1（価格テストの空文化）は3視点すべてが独立に指摘した。

### 対象コードと検証の要点

- 実装: `client/src/lib.rs`（`render_markdown` のURL書き換え、
  `is_safe_markdown_url`）、`server/src/lib.rs`（`upload_content` の価格解析）、
  `server/src/tests.rs`（CONTENT-01 回帰）。
- URLサニタイズは描画境界（クライアント）にあり、`inner_html` へ渡す唯一の
  経路であることを確認した。サーバーはMarkdownを描画せず原本バイト列を保持する。
- スキーム判定の主要な回避経路（大文字混在、制御文字・空白挿入、
  実体参照・percent 難読化、参照リンク定義、自動リンク）はいずれも
  許可リストで拒否され、`pulldown-cmark` 側の `escape_href` が属性境界を
  エスケープすることを依存クレートのソースで確認した。

### 指摘と対応

| ID | 重大度 | 指摘 | 対応 |
| --- | --- | --- | --- |
| H-1 | High | 価格拒否テストが `price` フィールドのみの本文を送るため、価格検証を削除しても「Missing file」で 400 となりテストが通る。回帰保証がなく、文書は実装済みと記載していた | 価格以外を正常値にした完全な本文へ変更し、状態コードと応答本文の拒否理由を併せて検証。拒否時に保存されないことも確認 |
| M-1 | Medium | テスト仕様書の CONTENT-01 / Upload 行が、実効性のない価格テストを根拠に「実装済み」と記載 | 実測に一致する記述へ更新し、残課題を取引上限・実multipartサイズ境界・実鍵署名へ整理 |
| M-2 | Medium | 「multipart異常系のAPI試験は未実装」が既存テストと矛盾（陳腐化） | 実際の未実装範囲（重複フィールド・本文サイズ上限）へ修正 |
| M-3 | Medium | 価格の範囲検証がハンドラ内にインラインで、解析型が暗黙に決まる。取引上限が未実装 | 解析型を `i64` に固定し、境界（未指定=0、`0`、`i64::MAX`、範囲外）をAPI回帰テストで固定。取引上限は未決定事項として残す |
| L-1 | Low | 許可外MIMEテストの本文が JPEG XL でないため、MIME検査を緩めても 400 となり帰属が不明 | 有効なシグネチャへ変更して拒否理由まで検証。MIME検査を独立したテストへ分離 |
| L-2 | Low | `multipart_upload_request` に正常系の対照がない | 価格境界テストが同ヘルパ経由で 200 と保存値を検証 |
| L-3 | Low | 参照リンク・自動リンク・実体参照で難読化したスキームの回帰テストがない | 3経路と相対参照の維持を追加（実測でいずれも `#` に中立化） |
| L-4 | Low | 詳細設計にURLポリシーが未記載。既存記述の「相対URLのみ」は不正確 | 詳細設計へポリシーを追記し、表現を「スキームとして成立しない接頭辞」に修正 |
| L-5 | Low | 画像の `#` 中立化は `src="#"` となり、現ドキュメントへのサブリソース要求を誘発し得る | URLは実行されないため安全側。表示方針の決定と併せて扱う記録とする |

### 却下・先送りした指摘

- **[採用せず] サーバー側でのURLスキーム再検証**: Markdownを描画するのは
  現行クライアントのみで、サーバーは原本バイト列を保持する。受信時に
  書き換えるとバイト列・CID・チェックサムが変わり、内容アドレスと監査の
  前提を壊す。単一レンダラ前提と、第二の描画経路が生じた時点で共有化する
  条件を [詳細設計書](DETAILED_DESIGN.md) に記録した。
- **[先送り] `is_safe_markdown_url` の `image: bool` 廃止と `common` への移動**:
  描画経路が1つの現段階では利益が小さく、別クライアント追加時に実施する。
- **[記録] 価格の単位（JPYCの整数か基準単位か）と取引上限**: 型に単位の
  意味がなく、決済照合・表示・保存で二重定義になり得る。単位と上限の決定は
  [文書・設計判断の確定状況](DOCUMENT_STATUS.md) の DEC-03 の範囲で扱う。
- **[記録] 4xx の可観測性**: 拒否理由は応答本文のみでログに残らない
  （本変更前からの挙動）。アクセスログ・メトリクス導入の設計課題として
  残し、IPアドレスや入力内容を出力しない方針は維持する。
- **[記録] リモート画像の読み込み**: `https:` の画像を許可するため、
  閲覧時に投稿者側ホストへ接続する。追跡ピクセルや閲覧者のIP開示は
  受容する範囲として扱い、制限が必要なら別途表示ポリシーで決める。
- **[却下] REVIEW_REPORT の `対応` 箇条が2行続く点**: 同一指摘に対する
  段階的な対応を分けて記録する既存の記述様式であり、重複ではない。
- **[却下] クライアントテストの部分一致アサーション**: 中立化と維持の
  両方向を検証しており、属性エスケープは `escape_href` の実装で確認済み。
  描画構造の完全一致は現段階で過剰。
- **[却下] `is_safe_markdown_url` が `"` `<` `>` を拒否しない点**: 属性境界は
  `pulldown-cmark` の `escape_href` が `%22` 等へエンコードすることを
  依存クレートのソースで確認した。自己完結性のため拒否を追加する案は
  将来の選択肢として記録するに留める。
- **[記録] 中立化されたURLの可観測性**: クライアントは置換時に何も記録せず、
  利用者からはリンク切れと区別できない。入力内容を出さない方針を保ったまま
  件数指標を残す案を、アクセスログ整備と併せて検討する。

### 実行時検証（一時DB・実ハンドラ）

- 変更前の実測（実ハンドラ）: `price` の解析型が暗黙の `i32` となり、
  `2147483647` は 200、`2147483648` と `9223372036854775807` は
  `400 {"error":"price must be an integer"}`。
- 変更後: `0` と `9223372036854775807` は 200 で保存値も一致し、
  `9223372036854775808`（`i64::MAX`+1）と `i64::MIN` は 400。
- 変異試験（テストが検証対象を固定していることの確認）:
  - 価格解析を `i32` に戻す: `test_upload_accepts_valid_price_boundaries` と
    `test_upload_rejects_invalid_price` が失敗。
  - 負価格検査を削除: `test_upload_rejects_invalid_price` のみ失敗。
  - 許可リストへ `application/zip` を追加:
    `test_upload_rejects_unsupported_media_type` のみ失敗。
  - 修正前の本文（`price` のみ）は価格検査を削除しても
    `400 {"error":"Missing file"}` を返し、テストが通っていたことを実測で
    確認（H-1 の根拠）。
- 標準検証ループ（最終コード状態）: `cargo fmt --all -- --check` 成功、
  `cargo clippy --workspace --all-targets -- -D warnings` 警告なし、
  `cargo test --workspace` 65 件成功（client 6 / common 5 / server 54）。
- E2E（ローカル）: `cd e2e && npm ci && npm run typecheck` の後、
  `E2E_REUSE=0 npm test` を実行。既定の並列実行（`fullyParallel`）では
  `auth` / `economy` / `moderation` の一部が 30 秒タイムアウトで失敗したが、
  失敗集合は実行ごとに変わり、`npx playwright test --workers=1` では
  9 件すべて成功（24.0s、CI 記録 9 passed と一致）。変更はクライアントの
  `#[cfg(test)]` モジュール追加とサーバーの整数型注釈で、E2E が通る経路を
  変更しないため、失敗はローカルの並列実行によるリソース競合と判断した。
- Markdown描画のブラウザ別URL解釈は未実施のまま（CONTENT-04 の残課題）。

## 変更差分クロスレビュー（2026-11-02）: IPFSゲートウェイ設定とP0テスト基盤

- 対象: `server/src/lib.rs`、`server/src/tests.rs`、`deny.toml`、
  `.github/workflows/ci.yml`、`README.md`、関連docs。
- レビュー観点: セキュリティ、テスト品質。
- 結果: **承認（P0範囲）**。Critical 1件、High 3件は修正済み。
- 検証: `cargo fmt --all --check` 成功、
  `cargo clippy --all-targets -- -D warnings` 警告なし、
  `cargo test --workspace` 77件成功（common 6 / client 5 / server 66）、
  `cargo deny check licenses` 成功。

### 変更内容

- `AppState` に `ipfs_gateways` と `ipfs_timeout` を追加し、
  `IPFS_GATEWAYS` / `IPFS_TIMEOUT_SECS` で上書き可能にした。
- テストの外部ネットワーク依存を除去し、ローカルHTTPモックで
  チェックサム不一致、10 MiB超過、タイムアウト、フォールバック、
  全ゲートウェイ失敗を検証する。
- `ALLOW_MOCK_AUTH` 無効時の `mock_token` とゼロ署名の拒否を検証する。
- `cargo-deny` 0.20.2 と `deny.toml` を追加し、CIで
  `cargo deny check licenses` を実行する。

### 指摘と対応

| ID | 重要度 | 指摘 | 対応 |
| --- | --- | --- | --- |
| T-1 | Critical | ゼロ署名テストが `sub` 不一致で早期401になり、署名検証に到達していなかった | `insert_juror` で主体と `owner_hash` を一致させ、`Invalid digital signature` を検証 |
| T-2 | High | モックがCIDパスを検証せず、`{cid}` 置換の欠落を検出できなかった | `expected_cid` を追加し、不一致は404 |
| T-3 | High | Novel経路のContent-Typeが未検証だった | 現行の `image/jxl` 固定を明示する回帰テストを追加 |
| T-4 | High | 全ゲートウェイ失敗と未設定の経路が未検証だった | 500と本文非漏えいを検証するテストを追加 |
| S-1 | Medium | `IPFS_GATEWAYS` がスキーム検証を行っていなかった | http(s)のみ許可する検証を追加 |
| S-2 | Medium | `cargo deny check advisories` が未導入 | 既存依存に未修正のRUSTSECが複数あるため、別タスクへ分離 |
| S-3 | Low | `IPFS_TIMEOUT_SECS` に上限がなかった | 1〜300秒に制限 |

### 残課題

- `cargo deny check advisories` は、既存依存のRUSTSEC
  （`rsa`、`rustls-webpki`、`h2`、`ttf-parser` 等）により失敗する。
  依存更新とリスク受容の判断を別タスクで行う。
- `std::env` を変更する既存テストは並列実行で競合し得る。
  `EXPOSE_INTERNAL_ERRORS` の状態注入を`AppState`へ移す改善を検討する。
- CID形式検証、ゲートウェイURLのログマスク、`cargo-deny`の
  バイナリ導入は未対応。
- 追記: ゲートウェイURLのログマスクとCIDのURLパス注入拒否は対応済み。
  厳密なCID仕様検証と`cargo-deny`のバイナリ導入は未対応。

## ADRクロスレビュー（2026-11-02）: JPKI認証とJPYC決済

- 対象: `docs/adr/ADR-0002-jpki-authentication.md`、
  `docs/adr/ADR-0003-jpyc-settlement.md`。
- レビュー観点: アーキテクチャ、セキュリティ。
- 結果: **条件付き承認（Proposedのまま）**。Critical 2件、High 9件、
  Mediumの主要項目を反映済み。Accepted への昇格は未承認。
- 検証: ドキュメントのみ。`git diff --check` 成功。

### 主要な指摘と対応

- [Critical] JPYCの送金元ウォレットとPPIDの結合が未定義。
  → 注文作成時のウォレット署名検証と `(ppid, wallet_address)` の
  結合を Decision 3 に追加。
- [Critical] 受取アドレスの生成・秘密鍵管理が未定義。
  → 注文ごとのHD派生アドレス、HSM/マルチシグ、Runbook化を Decision 4 に追加。
  共有アドレス方式は注文を識別できないため不採用とした。
- [High] `state`/`nonce`/`code_verifier` の保存・単回消費が未定義。
  → BFFの短寿命ストアとone-time消費を Decision 2 に追加。
- [High] 認証保証レベル（`acr`/`amr`）の確認がない。
  → Decision 3 に要求を追加し、提供有無を Open Questions に残した。
- [High] 現行HS256 JWTとの移行方針がない。
  → Decision 6 に移行期間・停止・失効を追加。
- [High] UserInfo属性の1時間削除と監査証跡が両立しない。
  → 取得を生年月日・居住地域に限定し、長期保持項目を Decision 7 に列挙。
- [High] 二重使用防止が `tx_hash` 単独。
  → `(chain_id, tx_hash, log_index)` の複合一意制約に変更。
- [High] `Transfer` ログの真正性検証がない。
  → `logs[N].address`、`removed`、`decimals()`、`status == 1` を追加。
- [High] allowlistの更新権限・緊急delistがない。
  → コード定数、ADR・マイグレーション、DEC-03完了条件を追加。
- [High] RPCの単一応答依存。
  → 2-of-3の一致、stale判定、不一致時の非確定を Decision 9 に追加。
- [Medium] セッション失効、リフレッシュトークンのローテーション・盗用検知、
  注文IDのエントロピー、`decimals`の整数比較、`pause`監視、reorg再評価、
  秘密管理を反映。

### 残課題

- `acr`/`amr` の提供有無と要求値の確定。
- 本人確認、疑わしい取引の届出、記録保存の具体化（後続ADR）。
- HDウォレットのマスターシード保管構成と承認者。
- 追加チェーンの受入基準。
- Accepted 昇格にはプロジェクトオーナーの承認が必要。

## 実装記録（2026-11-02）: JPYC決済基盤

- `server/migrations/002_payment_orders.sql` を追加。
- `server/src/payments.rs` に注文作成、ウォレット所有証明（EIP-191）、
  検証インターフェース、状態遷移を実装。
- API: `POST /payments/orders`、`GET /payments/orders/:id`、
  `POST /payments/orders/:id/bind`、`POST /payments/orders/:id/verify`。
- `PAYMENT_RECEIVING_XPRV` による注文ごとのBIP44受取アドレス派生を追加。
- テスト: 注文ライフサイクル、未対応チェーン、誤ウォレット署名、
  派生の決定性・一意性、注文ごとのアドレス差分。
- 検証: `cargo test -p server --lib payment` 4件、
  `derive_receiving` 1件成功。
- 未実装: 実RPC検証、2-of-3クォーラム、バックグラウンド確認ワーカー、
  返金、AML/KYC。

## 実装記録（2026-11-02）: JPKI OIDC認証

- `server/src/oidc.rs` を追加し、`/auth/oidc/start` と
  `/auth/oidc/callback` を実装。
- state/nonce/code_verifier のサーバー側保存と単回消費、PKCE。
- JWKS の `kid` による ES256 署名検証、`iss`/`aud`/`exp`/`nbf`/`iat`/
  `nonce`/`at_hash` の検証。
- PPID を `user_hash` としてセッションJWTとHttpOnly Cookieに結合。
- `POST /auth/logout` と `POST /auth/backchannel-logout` で `sid` を失効させる。
- テスト: 正常系、state 再利用拒否、nonce 不一致拒否、
  ログアウト後の `sid` 失効、back-channel logout の `sid` 失効。
- 検証: `cargo test -p server --lib oidc` 3件、`logout` 1件、
  `backchannel` 1件成功。
- 未実装: `acr`/`amr`、`auth_time`/`max_age`、`sub` 単位の全セッション失効、
  リフレッシュトークン、実サービス接続。

## 実装記録（2026-11-15）: RustCrypto 依存の更新

- `jsonwebtoken` 11.1.0 → 12.0.0、`rsa` 0.9.10 → 0.10.0、
  `k256` 0.13.4 → 0.14.0、`p256` 0.13.2 → 0.14.0 へ更新。
- 解消: RUSTSEC-2023-0071（Marvin）と RUSTSEC-2026-0387
  （crypto-bigint の timing）。`cargo deny check advisories` の検出は
  13 件から 11 件になった。
- 追従した API 変更: `EncodingKey`/`DecodingKey::from_secret` の
  `Result` 化、`VerifyingKey::to_encoded_point` → `to_sec1_point`、
  `sign_prehash_recoverable` の戻り値変更、`rsa::PrivateKey` への改名。
- テスト用 RSA 鍵生成は `rand_core` 0.10 で `OsRng` が廃止されたため、
  テスト専用の決定的 RNG へ置き換えた（新規依存なし）。
- 検証: `cargo fmt --all --check`、`cargo clippy --workspace
  --all-targets -- -D warnings`、`cargo test --workspace` 89 件、
  `cargo deny check licenses`、`cargo build -p client --target
  wasm32-unknown-unknown --locked`、ローカル E2E 9 件がすべて成功。
- 未対応: `cargo deny check advisories` の CI 導入（libsql 0.10 由来の
  既知勧告 10 件と paste 1 件が残るため）。

## 実装記録（2026-11-15）: 脆弱性勧告の CI ゲート

- `deny.toml` の `[advisories]` に既知 11 件の ignore と理由を追加し、
  `unused-ignored-advisory = "deny"` を設定した。
- CI の追加ゲートに `cargo deny check advisories` を追加した。
- `paste`（RUSTSEC-2024-0436）は leptos 0.8.23 / 0.9.0-rc1 も依存し
  削除できないため、ビルド時のみ・既知の脆弱性なしとして受容した。
- 検証: `cargo deny check advisories` と `cargo deny check licenses` が
  ローカルで成功。不要な ignore を追加すると失敗することも確認した。

## 実装記録（2026-11-15）: RS256 署名の追加

- `server/src/jwt.rs` を追加し、JWT の署名・検証鍵を `JwtKeys` に集約した。
- 既定は HS256（`JWT_SECRET`）。`JWT_RSA_PRIVATE_KEY_PEM`（PKCS#8 PEM、
  2048bit 以上）を設定した場合のみ RS256 を使う。
- 検証用の公開鍵は秘密鍵から導出し、別途設定させない。
- 許可アルゴリズムは起動時に確定し、トークンのヘッダの `alg` からは
  選ばせない（アルゴリズム混同攻撃への対策）。
- 起動時検証: 短い `JWT_SECRET`、2048bit 未満の RSA 鍵、不正な PEM、
  鍵がどちらも未設定の場合は起動を拒否する（4 種を実起動で確認）。
- テスト: RS256 のラウンドトリップ、`alg` 差し替え拒否、1024bit 鍵の拒否、
  環境変数の優先順位、RS256 トークンでの認証済み API 通過を追加。
- 検証: `cargo test --workspace` 95 件、`cargo clippy --workspace
  --all-targets -- -D warnings`、ローカル E2E 9 件が成功。RS256 での
  実起動ログ（`JWT signing algorithm: RS256`）も確認した。
- 未実装: 鍵ローテーション、`kid` による複数鍵の併用、HSM 等の外部鍵管理。
- 追記（2026-11-17）: 上記は当時の記録。`kid` 複数鍵併用と無停止ローテーション手順は実装済み（[KEY_ROTATION_RUNBOOK.md](KEY_ROTATION_RUNBOOK.md)）。残るは HSM 等の外部鍵管理のみ。
