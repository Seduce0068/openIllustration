#!/usr/bin/env bash
# CI 用の Playwright E2E 実行スクリプト。
# 共通ワークフロー rust-gates.yml の extra_gates から呼ばれ、Rust の
# ビルド成果物が温まった状態で実行されることを前提とする。
# ローカル実行は `cd e2e && npm test`（docs/TEST_SPECIFICATION.md）。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# trunk はコンパイルせず、固定バージョン + 固定 checksum のリリース
# アーカイブからバイナリを取り出す。README の
# `cargo install trunk --version 0.21.14 --locked` はローカル開発向けで、
# CI はダウンロード削減のため固定アーカイブ方式を採る（版は 0.21.14 で共通）。
# 更新時は version・sha256・資産名の 3 点を同時に変更する。
trunk_version="v0.21.14"
trunk_tarball="trunk-x86_64-unknown-linux-gnu.tar.gz"
trunk_sha256="f2b4680cd239693a646a2795e4633c625328d7b2a044fbe749fa3a2fe9e7036b"

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

# 固定版 trunk のアーカイブをローカルへキャッシュし、再ダウンロードを避ける。
# キャッシュの有無にかかわらず checksum を検証し、検証済みアーカイブから
# 毎回バイナリを展開して PATH に追加する（改ざんされたキャッシュの
# fail-open を防ぐ）。CI のコールド環境ではキャッシュが無いためダウンロードする。
trunk_cache="${repo_root}/.e2e-cache/trunk-${trunk_version}"
mkdir -p "$trunk_cache"
trunk_archive="${trunk_cache}/${trunk_tarball}"
if [[ -f "$trunk_archive" ]]; then
	echo "==> trunk ${trunk_version} のキャッシュ済みアーカイブを使用"
else
	echo "==> trunk ${trunk_version} を導入"
	curl -sSfL --connect-timeout 10 --retry 3 --retry-delay 2 \
		-o "$trunk_archive" \
		"https://github.com/trunk-rs/trunk/releases/download/${trunk_version}/${trunk_tarball}"
fi
# キャッシュヒット時も必ず検証を通す。不一致なら壊れたアーカイブを削除して失敗。
if ! echo "${trunk_sha256}  ${trunk_archive}" | sha256sum -c -; then
	rm -f "$trunk_archive"
	echo "trunk ${trunk_version} の sha256 が一致しない。" >&2
	exit 1
fi
mkdir -p "${workdir}/bin"
tar -xzf "$trunk_archive" -C "${workdir}/bin" trunk
chmod +x "${workdir}/bin/trunk"
export PATH="${workdir}/bin:${PATH}"

# trunk 0.21.14 は NO_COLOR=1 を --no-color として解釈し、
# 値が true/false でない場合にエラーになる。固定版では明示的に解除する。
unset NO_COLOR

# 失敗時の切り分け用に、リポジトリ外の依存バージョンを記録する。
trunk --version
node --version
npm --version

# 最低バージョンを検証する（package.json の engines と整合）。
min_node_major="${MIN_NODE_MAJOR:-18}"
node_major="$(node --version | sed -E 's/^v([0-9]+).*/\1/')"
# 非数値なら下限チェックと上限警告が黙ってスキップされるため、明示的に失敗させる。
if [[ ! "$node_major" =~ ^[0-9]+$ ]]; then
	echo "Node major version is not numeric: '${node_major}'." >&2
	exit 1
fi
if [[ "$node_major" =~ ^[0-9]+$ ]] && ((node_major < min_node_major)); then
	echo "Node ${node_major} is older than the required major ${min_node_major}." >&2
	exit 1
fi
# 上限は互換性未検証の可視化のみ（設定時は警告、失敗はしない）。
max_node_major="${MAX_NODE_MAJOR:-}"
if [[ -n "$max_node_major" && "$node_major" =~ ^[0-9]+$ ]] && ((node_major > max_node_major)); then
	echo "warning: Node ${node_major} is newer than the validated major ${max_node_major}." >&2
fi

# playwright.config.ts は channel: chrome を使うため、システムの
# Google Chrome が必要（GitHub Actions の ubuntu イメージには同梱）。
# Playwright は PATH に加えて /opt/google/chrome/chrome も参照する。
chrome_bin=""
if command -v google-chrome >/dev/null 2>&1; then
	chrome_bin="google-chrome"
elif [ -x /opt/google/chrome/chrome ]; then
	chrome_bin="/opt/google/chrome/chrome"
else
	echo "Google Chrome が見つからない。channel: chrome を使う E2E には必須。" >&2
	exit 1
fi
chrome_version="$($chrome_bin --version)"
echo "$chrome_version"
min_chrome_major="${MIN_CHROME_MAJOR:-120}"
chrome_major="$(echo "$chrome_version" | grep -oE '[0-9]+' | head -1)"
# 非数値なら下限チェックと上限警告が黙ってスキップされるため、明示的に失敗させる。
if [[ ! "$chrome_major" =~ ^[0-9]+$ ]]; then
	echo "Chrome major version is not numeric: '${chrome_major}'." >&2
	exit 1
fi
if ((chrome_major < min_chrome_major)); then
	echo "Chrome ${chrome_major} is older than the required major ${min_chrome_major}." >&2
	exit 1
fi
# 上側警告は非数値なら上の抽出エラーで既に失敗しているため追加対応不要（既存条件は維持）。
max_chrome_major="${MAX_CHROME_MAJOR:-}"
if [[ -n "$max_chrome_major" && "$chrome_major" =~ ^[0-9]+$ ]] && ((chrome_major > max_chrome_major)); then
	echo "warning: Chrome ${chrome_major} is newer than the validated major ${max_chrome_major}." >&2
fi

# 失敗時の調査用アーカイブの出力先（既定はリポジトリ直下の e2e-artifacts/）。
artifact_dir="${E2E_ARTIFACT_DIR:-${repo_root}/e2e-artifacts}"

# E2E 用 JWT シークレットと一時 DB は実行ごとに生成し、既知の固定値や
# リポジトリ内/前回実行の状態を CI で再利用しない（ローカルの固定既定値は
# e2e/playwright.config.ts を参照）。
E2E_JWT_SECRET="$(openssl rand -hex 32)"
export E2E_JWT_SECRET
export DATABASE_URL="file:${workdir}/e2e.db"
# ローカルで 8080 が code-server 等に占有されている場合でも実行できるよう、
# 未指定時は代替ポートを使う（CI では環境変数で上書き可能）。
export E2E_CLIENT_PORT="${E2E_CLIENT_PORT:-18080}"

# 実行対象プロジェクトを E2E_PROJECTS（カンマ区切り）で選択する。
# 未指定時は既存どおり chromium（システム Chrome）のみを実行する。
# firefox / webkit はオプトイン（ブラウザ導入と共有ライブラリが必要）。
all_projects=(chromium firefox webkit)
if [[ -n "${E2E_PROJECTS:-}" ]]; then
	IFS=',' read -r -a selected_projects <<<"$E2E_PROJECTS"
	for project in "${selected_projects[@]}"; do
		case "$project" in
		chromium | firefox | webkit) ;;
		*)
			echo "unknown E2E project: ${project} (allowed: ${all_projects[*]})" >&2
			exit 1
			;;
		esac
	done
else
	selected_projects=(chromium)
fi

# --project 引数と、導入が必要なブラウザバイナリを組み立てる。
# chromium は channel: chrome（システム Chrome）を使うため取得不要。
project_args=()
install_browsers=()
for project in "${selected_projects[@]}"; do
	project_args+=(--project="$project")
	case "$project" in
	firefox | webkit) install_browsers+=("$project") ;;
	esac
done

echo "==> サーバーとクライアントを事前ビルド"
cargo build -p server --locked
# Tailwind はビルド時 CSS のみを使う（実行時 CDN は禁止）。
(cd client && npm ci --ignore-scripts --no-audit --no-fund && npm run build-css)
(cd client && trunk build)
# 外部 CDN / フォントへの参照が残っていないことをゲートする。
if grep -qE 'cdn\.tailwindcss\.com|fonts\.googleapis\.com|fonts\.gstatic\.com' client/dist/index.html; then
	echo "external asset reference remains in client/dist/index.html" >&2
	exit 1
fi
# CSS 側からの外部参照もゲートする（入力 CSS と、存在する場合の生成 CSS）。
# 判定ロジック・エラー文言は index.html のゲートと同じものを使用する。
external_css_files=(client/input.css)
if [[ -d client/dist ]]; then
	while IFS= read -r generated_css; do
		external_css_files+=("$generated_css")
	done < <(find client/dist -type f -name '*.css' -print)
fi
for css_file in "${external_css_files[@]}"; do
	if grep -qE 'cdn\.tailwindcss\.com|fonts\.googleapis\.com|fonts\.gstatic\.com' "$css_file"; then
		echo "external asset reference remains in ${css_file}" >&2
		exit 1
	fi
done

echo "==> E2E 依存の導入（channel: chrome のバイナリは取得しない）"
(cd e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci)
if ((${#install_browsers[@]} > 0)); then
	# --with-deps は root 権限が必要なため CI では使わない。
	# firefox/webkit に必要なライブラリは同梱環境で揃っていることを確認済み。
	echo "==> Playwright ブラウザ導入: ${install_browsers[*]}"
	(cd e2e && npx playwright install "${install_browsers[@]}")
fi

echo "==> 型チェック"
(cd e2e && npm run typecheck)

echo "==> Playwright テスト: ${selected_projects[*]}"
if ! (cd e2e && npm test -- "${project_args[@]}"); then
	# extra_gates では artifact を回収できないため、失敗時は Playwright が
	# 生成した error-context をジョブログへ出し、Checks にアノテーションを残す。
	echo "::error::Playwright E2E failed"
	find e2e/test-results -name 'error-context.md' -print 2>/dev/null || true
	find e2e/test-results -name 'error-context.md' -exec head -c 20000 {} \; 2>/dev/null || true

	# トレース・スクリーンショット・HTML レポートをタイムスタンプ付きで保存する。
	mkdir -p "$artifact_dir" || true
	archive="${artifact_dir}/e2e-failure-$(date -u +%Y%m%dT%H%M%SZ).tar.gz"
	if tar -czf "$archive" -C "$repo_root" e2e/test-results e2e/playwright-report 2>/dev/null; then
		echo "E2E failure artifacts: ${archive}"
	else
		echo "warning: failed to archive E2E artifacts" >&2
	fi
	exit 1
fi
