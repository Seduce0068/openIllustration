#!/usr/bin/env bash
# CI 用の Playwright E2E 実行スクリプト。
# 共通ワークフロー rust-gates.yml の extra_gates から呼ばれ、Rust の
# ビルド成果物が温まった状態で実行されることを前提とする。
# ローカル実行は `cd e2e && npm test`（docs/TEST_SPECIFICATION.md）。
set -euo pipefail

# major バージョン（抽出値・閾値とも）を検査する共通ヘルパ。
# allow_empty=1 のときだけ空文字を「未設定」として許容する（MAX_* の既定）。
# 空以外の非数値は bash 算術で 0 として扱われ、下限/上限チェックが黙って
# スキップされるため、抽出値・閾値の別なく即座に失敗させる。
assert_major_numeric() {
	local label="$1"
	local value="$2"
	local allow_empty="${3:-0}"
	if [[ -z "$value" ]]; then
		if ((allow_empty)); then
			return 0
		fi
		echo "${label} is empty: expected a numeric major version." >&2
		exit 1
	fi
	# 先頭ゼロは算術解釈を 8 進数化して fail-open を招くため拒否する。
	if [[ ! "$value" =~ ^(0|[1-9][0-9]*)$ ]]; then
		echo "${label} is not numeric: '${value}'." >&2
		exit 1
	fi
	# 巨大整数は算術で 2^64+1 → 1 のようにラップし得るため、backup-rotate.sh と
	# 同じ「桁数＋辞書順」方式で上限を拒否する（算術は使わない）。先頭ゼロは
	# 上の正規表現で拒否済みなので、同桁の辞書順は数値順と一致する。
	# major version は実用上 3 桁程度であり、9 桁 (999999999) を上限とする。
	local max_major="999999999"
	if ((${#value} > ${#max_major})); then
		echo "${label} exceeds the supported major version range." >&2
		exit 1
	fi
	if ((${#value} == ${#max_major})) &&
		[[ "$value" != "$max_major" ]] &&
		printf '%s\n%s\n' "$max_major" "$value" | sort -C; then
		echo "${label} exceeds the supported major version range." >&2
		exit 1
	fi
}

# 実行対象プロジェクトを選択する純粋関数（副作用なし・E2E_LIB_ONLY=1 で
# source 可能）。実際のブラウザ検出は行わず、渡された実行可能性だけを見る。
# 入力:
#   $1 = E2E_PROJECTS の明示値（空なら未指定）
#   $2 = firefox が実行可能か (1/0)
#   $3 = webkit が実行可能か (1/0)
# 出力（nameref）:
#   $4 = 選択プロジェクト配列
#   $5 = 警告フラグ (1=chromium へフォールバック)
# 戻り値: 明示値に未知プロジェクトがあれば 1（呼び出し側で終了する）。
# ルール: 明示時は指定を厳守（未知値は拒否）。未指定時は firefox/webkit が
# 両方実行可能なら3プロジェクト、片方でも不可なら chromium へフォールバック
# し警告を立てる。
select_e2e_projects() {
	local explicit="${1:-}"
	local firefox_ok="${2:-0}"
	local webkit_ok="${3:-0}"
	local -n out_projects="$4"
	local -n out_warning="$5"
	local -a all_projects=(chromium firefox webkit)
	local -a parsed=()
	local project

	out_projects=()
	out_warning=0

	if [[ -n "$explicit" ]]; then
		IFS=',' read -r -a parsed <<<"$explicit"
		for project in "${parsed[@]}"; do
			case "$project" in
			chromium | firefox | webkit) ;;
			*)
				echo "unknown E2E project: ${project} (allowed: ${all_projects[*]})" >&2
				return 1
				;;
			esac
		done
		out_projects=("${parsed[@]}")
		return 0
	fi

	if [[ "$firefox_ok" == "1" && "$webkit_ok" == "1" ]]; then
		out_projects=("${all_projects[@]}")
	else
		out_projects=(chromium)
		out_warning=1
	fi
	return 0
}

# テストから関数定義だけを source 可能にする（E2E_LIB_ONLY=1 のときは
# 副作用のあるダウンロード/ビルドへ進まず、ここで source を終了する）。
# 通常の CI 実行では環境変数が無いため、以降の処理はこれまでどおり動く。
if [[ "${E2E_LIB_ONLY:-0}" == "1" ]]; then
	# source された場合は return、直接実行された場合は明示拒否（ゲート回避防止）。
	if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
		return 0
	fi
	echo "E2E_LIB_ONLY=1 is for sourcing function definitions only; direct execution is refused." >&2
	exit 1
fi

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
# キャッシュヒット時も必ず検証を通す。検証と展開は
# e2e/verify_trunk_archive.sh に委譲し、checksum 不一致時は exit 1
# （= アーカイブを展開しない）。固定版の version / sha256 は変更せず、
# 環境変数として共通ゲートへ渡す。
env \
	TRUNK_ARCHIVE="$trunk_archive" \
	TRUNK_SHA256="$trunk_sha256" \
	TRUNK_VERSION="$trunk_version" \
	TRUNK_EXTRACT_DIR="${workdir}/bin" \
	bash "${repo_root}/e2e/verify_trunk_archive.sh"
chmod +x "${workdir}/bin/trunk"
export PATH="${workdir}/bin:${PATH}"

# trunk 0.21.14 は NO_COLOR=1 を --no-color として解釈し、
# 値が true/false でない場合にエラーになる。固定版では明示的に解除する。
unset NO_COLOR

# 失敗時の切り分け用に、リポジトリ外の依存バージョンを記録する。
trunk --version
node --version
npm --version

# assert_major_numeric はファイル先頭で定義済み。E2E_LIB_ONLY=1 で
# 関数だけを source して実挙動を検証できるようにしている（VG-01）。

# 最低バージョンを検証する（ルート package.json の engines.node と整合）。
min_node_major="${MIN_NODE_MAJOR:-22}"
assert_major_numeric "MIN_NODE_MAJOR" "$min_node_major" 1
node_major="$(node --version | sed -E 's/^v([0-9]+).*/\1/')"
assert_major_numeric "Node major version" "$node_major"
if ((10#$node_major < 10#$min_node_major)); then
	echo "Node ${node_major} is older than the required major ${min_node_major}." >&2
	exit 1
fi
# 上限は互換性未検証の可視化のみ（設定時は警告、失敗はしない）。
max_node_major="${MAX_NODE_MAJOR:-}"
assert_major_numeric "MAX_NODE_MAJOR" "$max_node_major" 1
if [[ -n "$max_node_major" ]] && ((10#$node_major > 10#$max_node_major)); then
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
assert_major_numeric "MIN_CHROME_MAJOR" "$min_chrome_major" 1
chrome_major="$(echo "$chrome_version" | grep -oE '[0-9]+' | head -1)"
assert_major_numeric "Chrome major version" "$chrome_major"
if ((10#$chrome_major < 10#$min_chrome_major)); then
	echo "Chrome ${chrome_major} is older than the required major ${min_chrome_major}." >&2
	exit 1
fi
# 上限は互換性未検証の可視化のみ（設定時は警告、失敗はしない）。
max_chrome_major="${MAX_CHROME_MAJOR:-}"
assert_major_numeric "MAX_CHROME_MAJOR" "$max_chrome_major" 1
if [[ -n "$max_chrome_major" ]] && ((10#$chrome_major > 10#$max_chrome_major)); then
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

# firefox / webkit の実行可能性を検出する（fail-closed）。
# 既存の Chrome 検出（command -v + --version）と同じく、実際に導入済みで
# 共有ライブラリも解決できる場合だけ実行可能と判定する。満たさなければ 1
# （実行不可）を返し、呼び出し側は chromium へフォールバックする。
detect_browser_executable() {
	local browser="$1"
	local cache_root="${PLAYWRIGHT_BROWSERS_PATH:-${HOME}/.cache/ms-playwright}"
	local binary="" ldd_out=""
	case "$browser" in
	firefox)
		binary="$(
			find "$cache_root" -maxdepth 4 -type f -name firefox-bin \
				-perm -u+x -print -quit 2>/dev/null || true
		)"
		;;
	webkit)
		binary="$(
			find "$cache_root" -maxdepth 5 -type f -path '*/bin/MiniBrowser' \
				-perm -u+x -print -quit 2>/dev/null || true
		)"
		;;
	*)
		return 1
		;;
	esac
	[[ -n "$binary" ]] || return 1
	ldd_out="$(ldd "$binary" 2>/dev/null)" || return 1
	# 共有ライブラリ不足（CI を壊し得る事象）が見つかったら実行不可とする。
	if [[ "$ldd_out" == *"not found"* ]]; then
		return 1
	fi
	return 0
}

# 実行対象プロジェクトを選択する。
# E2E_PROJECTS の明示値がある場合は指定を厳守（未知値は拒否）。
# 未指定時は firefox/webkit の実行可能性を検出し、両方実行可能なら
# 3 プロジェクト、片方でも不可なら chromium へ警告付きでフォールバックする
# （共有ライブラリ不足環境で CI が壊れるのを防ぐため、既定は chromium 維持）。
e2e_projects_explicit="${E2E_PROJECTS:-}"
firefox_executable=0
webkit_executable=0
if [[ -z "$e2e_projects_explicit" ]]; then
	if detect_browser_executable firefox; then firefox_executable=1; fi
	if detect_browser_executable webkit; then webkit_executable=1; fi
fi
declare -a selected_projects=()
declare warn_fallback=0
if ! select_e2e_projects \
	"$e2e_projects_explicit" "$firefox_executable" "$webkit_executable" \
	selected_projects warn_fallback; then
	exit 1
fi
if ((warn_fallback)); then
	echo "warning: firefox=${firefox_executable} webkit=${webkit_executable};" \
		"chromium へフォールバックします。" >&2
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
# 先に対象ファイルの存在を明示確認する（欠落時に grep が 2 を返す前に fail-fast）。
if [[ ! -f client/dist/index.html ]]; then
	echo "required file is missing: client/dist/index.html" >&2
	exit 1
fi
if [[ ! -f client/input.css ]]; then
	echo "required file is missing: client/input.css" >&2
	exit 1
fi

# grep の終了コードは 0（一致）/1（不一致）/2（エラー）を区別し、意図的に
# 無視するのは 1 だけ。set -euo pipefail 下では `cmd || status=$?` の形で
# 捕捉すれば errexit を発動させずに grep 自身の終了コードが得られる。
# ここでは grep をパイプに包まないため $? は grep の終了コード（PIPESTATUS[0] と
# 同じ値）になる。将来パイプを導入する場合は末尾コマンドの $? ではなく
# PIPESTATUS[0] を読むこと（末尾の成功で 2 が隠れ fail-open になるため）。
assert_no_external_asset_reference() {
	local target_file="$1"
	local grep_status=0
	grep -qE 'cdn\.tailwindcss\.com|fonts\.googleapis\.com|fonts\.gstatic\.com' "$target_file" || grep_status=$?
	case "$grep_status" in
	0)
		echo "external asset reference remains in ${target_file}" >&2
		exit 1
		;;
	1)
		# 不一致は想定どおり。この終了コードだけを意図的に無視する。
		;;
	*)
		echo "failed to scan ${target_file} for external asset references (grep status: ${grep_status})." >&2
		exit 1
		;;
	esac
}

assert_no_external_asset_reference client/dist/index.html

# CSS 側からの外部参照もゲートする（入力 CSS と生成 CSS）。
# 判定ロジック・エラー文言は index.html のゲートと同じものを使用する。
external_css_files=(client/input.css)
# <(find ...) のプロセス置換は find の終了コードを隠して fail-open になるため、
# 一時ファイルへ出力してから明示的に成否を確認する（workdir は EXIT trap で削除）。
generated_css_list="${workdir}/generated-css.list"
if ! find client/dist -type f -name '*.css' -print >"$generated_css_list"; then
	echo "failed to enumerate generated CSS under client/dist." >&2
	exit 1
fi
while IFS= read -r generated_css; do
	external_css_files+=("$generated_css")
done <"$generated_css_list"
for css_file in "${external_css_files[@]}"; do
	assert_no_external_asset_reference "$css_file"
done

echo "==> E2E 依存の導入（channel: chrome のバイナリは取得しない）"
# npm ci の引数はクライアント側（上の `(cd client && npm ci ...)`）と完全一致させ、
# 両者を同一の依存導入契約として扱う（lifecycle script を --ignore-scripts で抑制）。
# ブラウザ導入は下の install_browsers / `npx playwright install` の明示実行であり
# lifecycle script ではないため、--ignore-scripts でも従来どおり動作する。
(cd e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --ignore-scripts --no-audit --no-fund)
if ((${#install_browsers[@]} > 0)); then
	# --with-deps は root 権限が必要なため CI では使わない。
	# firefox/webkit に必要なライブラリは同梱環境で揃っていることを確認済み。
	echo "==> Playwright ブラウザ導入: ${install_browsers[*]}"
	(cd e2e && npx playwright install "${install_browsers[@]}")
fi

echo "==> 型チェック"
(cd e2e && npm run typecheck)

echo "==> Playwright テスト: ${selected_projects[*]}"
# extra_gates では artifact を回収できないため、失敗時は Playwright が
# 生成した error-context をジョブログへ出し、Checks にアノテーションを残す。
report_e2e_failure() {
	local label="$1"
	echo "::error::Playwright E2E failed (${label})"
	find e2e/test-results -name 'error-context.md' -print 2>/dev/null || true
	find e2e/test-results -name 'error-context.md' -exec head -c 20000 {} \; 2>/dev/null || true

	# トレース・スクリーンショット・HTML レポートをタイムスタンプ付きで保存する。
	mkdir -p "$artifact_dir" || true
	archive="${artifact_dir}/e2e-failure-${label}-$(date -u +%Y%m%dT%H%M%SZ).tar.gz"
	if tar -czf "$archive" -C "$repo_root" e2e/test-results e2e/playwright-report 2>/dev/null; then
		echo "E2E failure artifacts: ${archive}"
	else
		echo "warning: failed to archive E2E artifacts" >&2
	fi
	exit 1
}

# 実サーバー結合（real-api）は共有 DB/ポートを使うため、他 spec と同じ
# プロセスで並走するとログインの /auth/verify がタイムアウトする。
# 先に real-api 単独で実行し、成功した場合のみ残りの spec を動的に列挙して実行する。
if ! (cd e2e && npm test -- tests/real-api.spec.ts "${project_args[@]}" --workers=1); then
	report_e2e_failure "real-api"
fi
# 残り spec は固定配列にせず e2e/tests から動的に列挙する。
# doc-numbers（scripts/test-doc-numbers.sh）も e2e/tests/*.spec.ts を動的に
# 走査して内訳を照合しており、新規 spec を追加したときに doc-numbers と
# 同じ集合が CI でも実行されるようにするためである。
# パスに改行や特殊文字が含まれても安全なよう NUL 区切り（-print0 / sort -z /
# read -d ''）を使い、一時ファイルへ出力してから while IFS= read -r で配列へ
# 読み込む。プロセス置換は find の終了コードを隠すため使わない。
# find/sort の失敗は set -o pipefail 下のパイプラインを if ! で判定して
# fail-closed に検出し、0 件ならエラー終了する。順序は LC_ALL=C sort で安定化。
remaining_specs_list="${workdir}/remaining-specs.list"
if ! find e2e/tests -maxdepth 1 -name '*.spec.ts' ! -name 'real-api.spec.ts' -print0 |
	LC_ALL=C sort -z >"$remaining_specs_list"; then
	echo "failed to enumerate remaining E2E specs under e2e/tests." >&2
	exit 1
fi
remaining_specs=()
while IFS= read -r -d '' spec_path; do
	# npm test へ渡すパスは従来どおり tests/<basename> の形式を維持する。
	remaining_specs+=("tests/${spec_path##*/}")
done <"$remaining_specs_list"
if ((${#remaining_specs[@]} == 0)); then
	echo "no remaining E2E specs found under e2e/tests (excluding real-api.spec.ts)." >&2
	exit 1
fi
if ! (cd e2e && npm test -- "${remaining_specs[@]}" "${project_args[@]}"); then
	report_e2e_failure "unit-specs"
fi
