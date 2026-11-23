#!/usr/bin/env bash
# CI の `doctor --strict` ゲート向け本番相当ダミー設定。
# すべてダミー値で、秘密情報・実オリジン・実鍵は一切含まない。
# source してから doctor を実行する（export はこのファイル側で行う）。
# 共通ワークフローの env_vars が extra_gates へ伝播しても本番相当を保つため、
# doctor が警告対象とする開発用フラグを明示的に落とす（伝播仕様への暗黙依存を排除）。
unset ALLOW_MOCK_AUTH
unset ALLOW_IPV4_LOCALHOST
unset EXPOSE_INTERNAL_ERRORS
unset ALLOW_UNENCRYPTED_BACKUP
export DATABASE_URL='file:/tmp/oil-doctor-test.db'
export JWT_SECRET='0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
export ALLOWED_ORIGINS='https://app.example.org'
export OIDC_ISSUER='https://issuer.example.org'
export OIDC_JWKS_URI='https://issuer.example.org/jwks'
export OIDC_CLIENT_ID='test-client'
export OIDC_CLIENT_SECRET='dummy-client-secret'
export OIDC_REDIRECT_URI='https://api.example.org/auth/oidc/callback'
export OIDC_AUTHORIZATION_ENDPOINT='https://issuer.example.org/authorize'
export OIDC_TOKEN_ENDPOINT='https://issuer.example.org/token'
export OIDC_REQUIRED_ACR='urn:example:acr'
export OIDC_REQUIRED_AMR='["pwd"]'
export PINATA_JWT='dummy-pinata-jwt'
export IPFS_GATEWAYS='https://gateway.example.org/ipfs/{cid}'
export AUDIT_EXPECT_ROOT='0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
export BACKUP_GPG_RECIPIENT='dummy-gpg-recipient'
export TZ='Asia/Tokyo'
