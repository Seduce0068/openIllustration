# ADR 変更ログ

## 2026-11-22

- [ADR-0007](ADR-0007-verified-age-adapter.md): verified_claims 経路の `user_hash` を
  `issuer` + `signed_subject` 由来
  （`sha256("verified-age:v1:{issuer}:{signed_subject}")`）へ変更する契約を
  Decision / Consequences / 変更管理へ追記（R2-USERHASH）。旧
  `birthdate:country` hash の開発 DB は作り直し前提。OIDC callback の `sub` 生値は
  別契約として変更しない。Status: Accepted 維持。
