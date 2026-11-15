#!/usr/bin/env bash
# CI 用の Playwright E2E 実行スクリプト。
# 共通ワークフロー rust-gates.yml の extra_gates から呼ばれ、Rust の
# ビルド成果物が温まった状態で実行されることを前提とする。
# ローカル実行は `cd e2e && npm test`（docs/TEST_SPECIFICATION.md）。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# trunk はコンパイルせず、固定バージョン + 固定 checksum のリリース
# バイナリを一時ディレクトリへ展開する。更新時は version・sha256・
# 資産名の 3 点を同時に変更する。
trunk_version="v0.21.14"
trunk_tarball="trunk-x86_64-unknown-linux-gnu.tar.gz"
trunk_sha256="f2b4680cd239693a646a2795e4633c625328d7b2a044fbe749fa3a2fe9e7036b"

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

echo "==> trunk ${trunk_version} を導入"
curl -sSfL --connect-timeout 10 --retry 3 --retry-delay 2 \
  -o "${workdir}/${trunk_tarball}" \
  "https://github.com/trunk-rs/trunk/releases/download/${trunk_version}/${trunk_tarball}"
echo "${trunk_sha256}  ${workdir}/${trunk_tarball}" | sha256sum -c -
tar -xzf "${workdir}/${trunk_tarball}" -C "${workdir}" trunk
export PATH="${workdir}:${PATH}"

# 失敗時の切り分け用に、リポジトリ外の依存バージョンを記録する。
trunk --version
node --version
npm --version

# playwright.config.ts は channel: chrome を使うため、システムの
# Google Chrome が必要（GitHub Actions の ubuntu イメージには同梱）。
# Playwright は PATH に加えて /opt/google/chrome/chrome も参照する。
if command -v google-chrome >/dev/null 2>&1; then
  google-chrome --version
elif [ -x /opt/google/chrome/chrome ]; then
  /opt/google/chrome/chrome --version
else
  echo "Google Chrome が見つからない。channel: chrome を使う E2E には必須。" >&2
  exit 1
fi

# E2E 用 JWT シークレットと一時 DB は実行ごとに生成し、既知の固定値や
# リポジトリ内/前回実行の状態を CI で再利用しない（ローカルの固定既定値は
# e2e/playwright.config.ts を参照）。
export E2E_JWT_SECRET="$(openssl rand -hex 32)"
export DATABASE_URL="file:${workdir}/e2e.db"

echo "==> サーバーとクライアントを事前ビルド"
cargo build -p server --locked
(cd client && trunk build)

echo "==> E2E 依存の導入（ブラウザは channel: chrome を使うため取得しない）"
(cd e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci)

echo "==> 型チェック"
(cd e2e && npm run typecheck)

echo "==> Playwright テスト"
if ! (cd e2e && npm test); then
  # extra_gates では artifact を回収できないため、失敗時は Playwright が
  # 生成した error-context をジョブログへ出し、Checks にアノテーションを残す。
  echo "::error::Playwright E2E failed"
  find e2e/test-results -name 'error-context.md' -print 2>/dev/null || true
  find e2e/test-results -name 'error-context.md' -exec head -c 20000 {} \; 2>/dev/null || true
  exit 1
fi
