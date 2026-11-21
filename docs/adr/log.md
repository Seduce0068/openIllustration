# ADR 変更ログ

## 2026-11-22

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
