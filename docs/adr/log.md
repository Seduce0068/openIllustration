# ADR 変更ログ

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
