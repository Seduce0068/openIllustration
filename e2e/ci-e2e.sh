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

echo "==> trunk ${trunk_version} を導入"
curl -sSfL -o "/tmp/${trunk_tarball}" \
  "https://github.com/trunk-rs/trunk/releases/download/${trunk_version}/${trunk_tarball}"
echo "${trunk_sha256}  /tmp/${trunk_tarball}" | sha256sum -c -
mkdir -p "${HOME}/.cargo/bin"
tar -xzf "/tmp/${trunk_tarball}" -C "${HOME}/.cargo/bin" trunk
export PATH="${HOME}/.cargo/bin:${PATH}"
trunk --version

# playwright.config.ts は channel: chrome を使うため、システムの
# Google Chrome が必要（GitHub Actions の ubuntu イメージには同梱）。
if ! command -v google-chrome >/dev/null 2>&1; then
  echo "google-chrome が見つからない。channel: chrome を使う E2E には必須。" >&2
  exit 1
fi

echo "==> サーバーとクライアントを事前ビルド"
cargo build -p server --locked
(cd client && trunk build)

echo "==> E2E 依存の導入（ブラウザは channel: chrome を使うため取得しない）"
(cd e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci)

echo "==> Playwright テスト"
(cd e2e && npx playwright test)
