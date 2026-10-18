#!/usr/bin/env bash
# CI 用の Playwright E2E 実行スクリプト。
# 共通ワークフロー rust-gates.yml の extra_gates から呼ばれ、Rust の
# ビルド成果物が温まった状態で実行されることを前提とする。
# ローカル実行は `cd e2e && npx playwright test`（docs/TEST_SPECIFICATION.md）。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# trunk はコンパイルせず、固定バージョン + 固定 checksum の
# リリースバイナリを導入する（更新時は version と sha256 を揃えて変更する）。
trunk_version="v0.21.14"
trunk_tarball="trunk-x86_64-unknown-linux-gnu.tar.gz"
trunk_sha256="f2b4680cd239693a646a2795e4633c625328d7b2a044fbe749fa3a2fe9e7036b"

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

echo "==> trunk ${trunk_version} を導入"
curl -sSfL --retry 3 --retry-delay 2 -o "${workdir}/${trunk_tarball}" \
  "https://github.com/trunk-rs/trunk/releases/download/${trunk_version}/${trunk_tarball}"
echo "${trunk_sha256}  ${workdir}/${trunk_tarball}" | sha256sum -c -
mkdir -p "${HOME}/.cargo/bin"
tar -xzf "${workdir}/${trunk_tarball}" -C "${HOME}/.cargo/bin" trunk
export PATH="${HOME}/.cargo/bin:${PATH}"
trunk --version

# playwright.config.ts は channel: chrome を使うため、システムの
# Google Chrome が必要（GitHub Actions の ubuntu イメージには同梱）。
if ! command -v google-chrome >/dev/null 2>&1; then
  echo "google-chrome が見つからない。channel: chrome を使う E2E には必須。" >&2
  exit 1
fi

# E2E 用 JWT シークレットは実行ごとに生成し、既知の固定値を CI で
# 使わない（ローカルの固定既定値は e2e/playwright.config.ts を参照）。
export E2E_JWT_SECRET="$(openssl rand -hex 32)"

echo "==> サーバーとクライアントを事前ビルド"
cargo build -p server --locked
(cd client && trunk build)

echo "==> E2E 依存の導入（ブラウザは channel: chrome を使うため取得しない）"
(cd e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci)

echo "==> 型チェック"
(cd e2e && npm run typecheck)

echo "==> Playwright テスト"
(cd e2e && npx playwright test)
