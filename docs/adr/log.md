# ADR 変更ログ

## 2026-11-23

- ADR-0013 の P1（設定・運用の単一情報源）／P2（バックアップ／復旧の完全性）／
  P3（テスト／CI の信頼性）／P4（ランタイム保護／UX）がすべて実装完了。
  Deterministic Gates（fmt/clippy/test/wasm/deny/markdownlint/backup/doc-numbers/E2E）green
  （E2E69 / check23 / doctor54 / migrations14 / tests156）。Status を Implemented へ更新。

- [ADR-0004](ADR-0004-moderation-procedure.md): 異議（appeal_of 付き再通報）の認可を追加。
  提出主体を元通報者（監査ログの `User report:` 行の `changed_by`）またはコンテンツ所有者
  に限定し、第三者は固定文言 403 で拒否。同一 CID の異議は30日以内1回のクールダウンを
  導入。運用者承認必須案は稼働依存で遅延するため却下し、自動クールダウンを採用。
  元通報者判定は appeal_of 対象通報の `created_at` に最も近い監査行を選択する。

## 2026-11-22

- [ADR-0010](ADR-0010-foreign-key-startup-fail-closed.md): 新規作成。本番相当では
  `PRAGMA foreign_key_check` の孤立行・検査失敗で起動を拒否（fail-closed）、
  開発（mock 有効）では warn のみ、`doctor` は production カテゴリ内へ同じ判定の
  警告をマージ（check23 維持）。是正 CLI は設けず、孤立行検出時はバックアップからの
  作り直し前提とする。Status: Accepted。
- [ADR-0007](ADR-0007-verified-age-adapter.md): verified_claims 経路の `user_hash` を
  `issuer` + `signed_subject` 由来
  （`sha256("verified-age:v1:{issuer}:{signed_subject}")`）へ変更する契約を
  Decision / Consequences / 変更管理へ追記（R2-USERHASH）。旧
  `birthdate:country` hash の開発 DB は作り直し前提。OIDC callback の `sub` 生値は
  別契約として変更しない。Status: Accepted 維持。
- [ADR-0004](ADR-0004-moderation-procedure.md): 手動 finalize の HTTP ハンドラを
  `require_moderation_operator` による運用者認可必須（未設定・未登録・BAN 済みは 403）
  と明記。スケジューラ経路（`finalize_due_reports*` / `process_report_conclusion`）は
  認可対象外で自動確定を継続。`GET /mod/reports` の要約公開は現状維持。
  Status: Accepted 維持。
- [ADR-0008](ADR-0008-moderation-stats-publication.md): 段階2（公開集計ルート）を
  実装済みへ更新。`GET /mod/stats/public` は `AuthClaims` のみ必須（未認証 401・
  認証済みは運用者でなくても 200）、`require_moderation_operator` は呼ばない。
  公開型は `PublicModStatsResponse` / `PublicModStatsTotals` /
  `PublicModStatsMonthly` とし、bans/unbans/unpins と CID・reason・`user_hash` を
  含めない。純粋関数 `build_public_mod_stats` が `build_mod_stats` の結果を縮約。
   Status: Accepted 維持。
- [ADR-0011](ADR-0011-performance-measurement.md): 新規作成。計測3層（E2E 所要時間 /
  `first-contentful-paint` と `responseEnd` / 転送量と wasm サイズ）、3回測定の中央値、
  受入値は未合意、メモリは参考値のみ。Level 0 を今すぐの pass 条件、
  Level 1 を ADR 合意後の受入と定義。Status: Proposed。
- [ADR-0012](ADR-0012-compatibility-matrix.md): 新規作成。ブラウザ×OS×主要フローの
  マトリクスと `verified` / `proxy_only` / `not_verified` / `blocked` の状態を定義。
  Edge / Brave は `same_engine_proxy`、Safari は `webkit_proxy`、
  認証 / 決済は `functional_only`、最小バージョン未決定、暗黙除外禁止。
  Status: Proposed。
- [ADR-0011](ADR-0011-performance-measurement.md): 現状の Level 0 実装が層1（E2E 所要時間）＋
  層2（`first-contentful-paint` / `responseEnd`）の単回計測のみであることを Decision /
  Consequences へ明記。層3（転送バイト数・wasm サイズ）と3回測定の中央値は Level 1
  （受入値合意後）の実装項目として宣言を実態へ合わせ（実装は追加せず）。
  Status: Proposed 維持。
- [ADR-0012](ADR-0012-compatibility-matrix.md): ブラウザ（Chrome / Edge / Brave / Firefox /
  Safari）× OS × 閲覧 / 投稿 / 認証 / 決済の現状マトリクスを表として追加。
  Edge / Brave は `same_engine_proxy`、Safari は `webkit_proxy`、認証 / 決済は
  `functional_only`、最小バージョン未決定・暗黙除外禁止を明示し、入力を
  `compat-selection` 添付とする旨を記載。Status: Proposed 維持。
