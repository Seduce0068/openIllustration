#!/usr/bin/env bash
# CI 用の Playwright E2E 実行スクリプト。
# 共通ワークフロー rust-gates.yml の extra_gates から呼ばれ、Rust の
# ビルド成果物が温まった状態で実行されることを前提とする。
# ローカル実行は `cd e2e && npm test`（docs/TEST_SPECIFICATION.md）。
set -euo pipefail

# e2e_stage_fail の定義より前に呼ばれる検査（major version 等）用の失敗ヘルパ。
# 素の診断行を残しつつ ::error:: を 1 本だけ出し、重複を防ぐ。
E2E_ERROR_REPORTED="${E2E_ERROR_REPORTED:-0}"
E2E_STAGE="${E2E_STAGE:-startup}"
e2e_stage_fail_early() {
	local message="$1"
	echo "$message" >&2
	if ((E2E_ERROR_REPORTED)); then
		exit 1
	fi
	E2E_ERROR_REPORTED=1
	printf '::error::E2E setup failed at stage "%s": %s\n' "$E2E_STAGE" "$message" >&2
	exit 1
}

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
		e2e_stage_fail_early "${label} is empty: expected a numeric major version."
	fi
	# 先頭ゼロは算術解釈を 8 進数化して fail-open を招くため拒否する。
	if [[ ! "$value" =~ ^(0|[1-9][0-9]*)$ ]]; then
		e2e_stage_fail_early "${label} is not numeric: '${value}'."
	fi
	# 巨大整数は算術で 2^64+1 → 1 のようにラップし得るため、backup-rotate.sh と
	# 同じ「桁数＋辞書順」方式で上限を拒否する（算術は使わない）。先頭ゼロは
	# 上の正規表現で拒否済みなので、同桁の辞書順は数値順と一致する。
	# major version は実用上 3 桁程度であり、9 桁 (999999999) を上限とする。
	# 現在の max は全桁9のため同桁辞書順の fail 分岐は到達不能だが、
	# 将来上限を下げた際の比較向き誤りを防ぐため残す（桁数分岐は到達可能）。
	local max_major="999999999"
	if ((${#value} > ${#max_major})); then
		e2e_stage_fail_early "${label} exceeds the supported major version range."
	fi
	if ((${#value} == ${#max_major})) &&
		[[ "$value" != "$max_major" ]] &&
		printf '%s\n%s\n' "$max_major" "$value" | sort -C; then
		e2e_stage_fail_early "${label} exceeds the supported major version range."
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
# shellcheck disable=SC2034
select_e2e_projects() {
	local explicit="${1:-}"
	local firefox_ok="${2:-0}"
	local webkit_ok="${3:-0}"
	local -n out_projects="$4"
	local -n out_warning="$5"
	local -a all_projects=(chromium firefox webkit)
	# iphone / pixel は E2E_MOBILE=1 のオプトイン専用で、自動選択の既定には入れない。
	local -a allowed_projects=(chromium firefox webkit iphone pixel)
	local -a parsed=()
	local project

	out_projects=()
	out_warning=0

	if [[ -n "$explicit" ]]; then
		IFS=',' read -r -a parsed <<<"$explicit"
		for project in "${parsed[@]}"; do
			case "$project" in
			chromium | firefox | webkit | iphone | pixel) ;;
			*)
				echo "unknown E2E project: ${project} (allowed: ${allowed_projects[*]})" >&2
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

# ブラウザ選択の可視化データを生成する純粋な formatter（副作用なし）。
# E2E_LIB_ONLY=1 で source しても安全に呼べるよう、判定や外部コマンドを伴わない。
# 入力:
#   $1 = 選択プロジェクト配列の名前（nameref。呼び出し側の配列は変更しない）
#   $2 = firefox 実行可否 (1/0)
#   $3 = webkit 実行可否 (1/0)
#   $4 = 警告フラグ (1=chromium へフォールバック)
# 出力（nameref）:
#   $5 = annotation 文字列（フォールバック時だけ ::warning:: 行、それ以外は空）
#   $6 = summary markdown 文字列（warn_fallback に関わらず選択プロジェクトを記録）
# 固定文言は printf の書式リテラルとして持ち、入力はすべて %s で埋め込む。
# E2E_PROJECTS の生文字列を書式として解釈させず、メッセージへ外部入力を混入させない。
# shellcheck disable=SC2034
format_e2e_browser_selection() {
	local -n in_projects="$1"
	local firefox_ok="$2"
	local webkit_ok="$3"
	local warn="$4"
	local -n out_annotation="$5"
	local -n out_summary="$6"
	local projects_joined=""

	printf -v projects_joined '%s ' "${in_projects[@]}"
	projects_joined="${projects_joined% }"

	out_annotation=""
	if ((warn)); then
		printf -v out_annotation \
			'::warning::E2E browser fallback: firefox=%s webkit=%s; running %s' \
			"$firefox_ok" "$webkit_ok" "$projects_joined"
	fi

	printf -v out_summary \
		'### E2E browser selection\n\n- Firefox executable: %s\n- WebKit executable: %s\n- Fallback to chromium: %s\n- Running projects: %s\n' \
		"$firefox_ok" "$webkit_ok" "$warn" "$projects_joined"
}

# formatter の出力を GitHub Actions の annotation / step summary へ反映する writer。
# GITHUB_ACTIONS / GITHUB_STEP_SUMMARY が未設定なら何もせず成功扱い（no-op）。
# 設定されていても要約ファイルが存在しない・書き込み不可なら return 0 とし、
# set -euo pipefail 下でスクリプトが落ちないようにする。
# 出力は printf のみ。本文は formatter が生成した固定文言で、ここでは外部入力を
# 書式として解釈しない（%s として渡すだけ）。
write_e2e_browser_selection() {
	local annotation="${1:-}"
	local summary="${2:-}"

	[[ -n "${GITHUB_ACTIONS:-}" && -n "${GITHUB_STEP_SUMMARY:-}" ]] || return 0
	local summary_path="${GITHUB_STEP_SUMMARY}"
	[[ -f "$summary_path" && -w "$summary_path" ]] || return 0

	if [[ -n "$annotation" ]]; then
		printf '%s\n' "$annotation" || return 0
	fi
	printf '%s\n' "$summary" >>"$summary_path" || return 0
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

# セットアップ/ビルド段階の失敗へ段階名付き ::error:: を付ける ERR trap。
# テスト失敗時の report_e2e_failure と同じ ::error:: 経路を、ビルド前の
# 失敗にも適用するのが目的（後段の report_e2e_failure とフラグを共有する）。
# E2E_LIB_ONLY の source ではここへ到達しないため、テストが source する純粋
# 関数（assert_major_numeric 等）には一切影響しない。
# E2E_ERROR_REPORTED は明示的に ::error:: を出した経路との重複を防ぐ。
E2E_STAGE="bootstrap"
E2E_ERROR_REPORTED=0
e2e_stage_fail() {
	local message="$1"
	if ((E2E_ERROR_REPORTED)); then
		echo "$message" >&2
		exit 1
	fi
	E2E_ERROR_REPORTED=1
	printf '::error::E2E setup failed at stage "%s": %s\n' "$E2E_STAGE" "$message" >&2
	exit 1
}
e2e_err_trap() {
	local status="$1"
	if ((E2E_ERROR_REPORTED)); then
		exit "$status"
	fi
	E2E_ERROR_REPORTED=1
	printf '::error::E2E setup failed at stage "%s" (exit %d)\n' \
		"$E2E_STAGE" "$status" >&2
	exit "$status"
}
trap 'e2e_err_trap $?' ERR

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
E2E_STAGE="trunk-install"

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
E2E_STAGE="version-check"
min_node_major="${MIN_NODE_MAJOR:-22}"
assert_major_numeric "MIN_NODE_MAJOR" "$min_node_major" 1
node_major="$(node --version | sed -E 's/^v([0-9]+).*/\1/')"
assert_major_numeric "Node major version" "$node_major"
if ((10#$node_major < 10#$min_node_major)); then
	e2e_stage_fail "Node ${node_major} is older than the required major ${min_node_major}."
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
	e2e_stage_fail "Google Chrome が見つからない。channel: chrome を使う E2E には必須。"
fi
chrome_version="$($chrome_bin --version)"
echo "$chrome_version"
min_chrome_major="${MIN_CHROME_MAJOR:-120}"
assert_major_numeric "MIN_CHROME_MAJOR" "$min_chrome_major" 1
chrome_major="$(echo "$chrome_version" | grep -oE '[0-9]+' | head -1)"
assert_major_numeric "Chrome major version" "$chrome_major"
if ((10#$chrome_major < 10#$min_chrome_major)); then
	e2e_stage_fail "Chrome ${chrome_major} is older than the required major ${min_chrome_major}."
fi
# Chrome の上限は CI で明示する（MIN_CHROME_MAJOR=120 と合わせた互換範囲）。
# ci.yml の env_vars で MAX_CHROME_MAJOR を注入する。未注入の CI では 140 を既定とし、
# ランナーの Chrome を更新した場合は ci.yml の値を検証済み major へ追随させる。
# 上限超過は警告ではなく失敗（fail-closed）。
max_chrome_major="${MAX_CHROME_MAJOR:-}"
if [[ -n "${CI:-}" && -z "$max_chrome_major" ]]; then
	max_chrome_major="140"
fi
assert_major_numeric "MAX_CHROME_MAJOR" "$max_chrome_major" 1
if [[ -n "$max_chrome_major" ]] && ((10#$chrome_major > 10#$max_chrome_major)); then
	e2e_stage_fail "Chrome ${chrome_major} is newer than the supported major ${max_chrome_major}."
fi

# 失敗時の調査用アーカイブの出力先（既定はリポジトリ直下の e2e-artifacts/）。
artifact_dir="${E2E_ARTIFACT_DIR:-${repo_root}/e2e-artifacts}"

# E2E 用 JWT シークレットと一時 DB は実行ごとに生成し、既知の固定値や
# リポジトリ内/前回実行の状態を CI で再利用しない（ローカルの固定既定値は
# e2e/playwright.config.ts を参照）。
E2E_JWT_SECRET="$(openssl rand -hex 32)"
export E2E_JWT_SECRET
# 共有 DB/ポートの再利用を禁止し、毎回クリーンなサーバーを起動する。
# 既存プロセスの有無に依存せず、CI とローカルで同じ挙動にする（ADR-0013 P3）。
export E2E_REUSE=0
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
# CI では未指定でも chromium,firefox,webkit を明示指定として扱い、
# firefox/webkit の導入と実行を必須化する。キャッシュ走査による実行可能性判定が
# npm ci / playwright install より前に行われると、コールド CI ではバイナリが
# 未導入のまま chromium 単一へ黙ってフォールバックするためである。
# ローカルの未指定時は従来どおり検出結果に基づき選択する。
e2e_projects_explicit="${E2E_PROJECTS:-}"
if [[ -n "${CI:-}" && -z "$e2e_projects_explicit" ]]; then
	e2e_projects_explicit="chromium,firefox,webkit"
fi
# モバイルはオプトイン（E2E_MOBILE=1）時だけ選択へ追加する（既定の chromium は維持）。
# playwright.config.ts と同じプロジェクト名・同じ契約。
if [[ "${E2E_MOBILE:-0}" == "1" ]]; then
	if [[ -z "$e2e_projects_explicit" ]]; then
		e2e_projects_explicit="chromium"
	fi
	# 既に iphone / pixel が明示されている場合は重複して渡さない。
	mobile_names=()
	IFS=',' read -ra mobile_names <<<"$e2e_projects_explicit"
	merged=()
	seen_mobile=0
	for existing in "${mobile_names[@]}"; do
		[[ -z "$existing" ]] && continue
		merged+=("$existing")
	done
	for existing in iphone pixel; do
		seen_mobile=0
		for item in "${merged[@]}"; do
			[[ "$item" == "$existing" ]] && seen_mobile=1 && break
		done
		((seen_mobile)) || merged+=("$existing")
	done
	e2e_projects_explicit="$(
		IFS=','
		echo "${merged[*]}"
	)"
fi
firefox_executable=0
webkit_executable=0
if [[ -z "$e2e_projects_explicit" ]]; then
	if detect_browser_executable firefox; then firefox_executable=1; fi
	if detect_browser_executable webkit; then webkit_executable=1; fi
else
	# 明示指定時は実行可能性に依存せず指定を厳守する。未知値の拒否は
	# select_e2e_projects のルールをそのまま使う（判定ロジックは変更しない）。
	firefox_executable=1
	webkit_executable=1
fi
declare -a selected_projects=()
declare warn_fallback=0
if ! select_e2e_projects \
	"$e2e_projects_explicit" "$firefox_executable" "$webkit_executable" \
	selected_projects warn_fallback; then
	e2e_stage_fail_early "E2E project selection failed"
fi
# CI で chromium 単一へのフォールバックが発生した場合は fail-closed で失敗させる
# （SKIP 扱いにせず、選択が導入より先行する回帰を検知できるようにする）。
if ((warn_fallback)) && [[ -n "${CI:-}" ]]; then
	E2E_ERROR_REPORTED=1
	echo "::error::E2E browser fallback to chromium is not allowed in CI (E2E_PROJECTS=${e2e_projects_explicit})" >&2
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

# playwright install の再試行ラッパ（有限回数・指数バックオフ）。
# 一時的なネットワーク障害で CI 全体が落ちないよう最大 3 回だけ試行し、
# 最終失敗時は非 0 を返して呼び出し側が ::error:: を残す（fail-closed）。
PLAYWRIGHT_INSTALL_MAX_ATTEMPTS=3
install_playwright_browsers() {
	local -a browsers=("$@")
	local attempt=1
	local delay="${PLAYWRIGHT_INSTALL_BACKOFF_SECONDS:-2}"
	while ((attempt <= PLAYWRIGHT_INSTALL_MAX_ATTEMPTS)); do
		if (cd e2e && npx playwright install "${browsers[@]}"); then
			return 0
		fi
		if ((attempt == PLAYWRIGHT_INSTALL_MAX_ATTEMPTS)); then
			return 1
		fi
		echo "warning: playwright install failed (attempt ${attempt}/${PLAYWRIGHT_INSTALL_MAX_ATTEMPTS}); retrying in ${delay}s." >&2
		sleep "$delay"
		delay=$((delay * 2))
		attempt=$((attempt + 1))
	done
	return 1
}

# ブラウザ導入は選定の後・実行の前に行う（CI では firefox/webkit を必ず導入）。
E2E_STAGE="npm-ci"
echo "==> E2E 依存の導入（channel: chrome のバイナリは取得しない）"
# npm ci の引数はクライアント側（下の `(cd client && npm ci ...)`）と完全一致させ、
# 両者を同一の依存導入契約として扱う（lifecycle script を --ignore-scripts で抑制）。
# ブラウザ導入は下の install_browsers / `npx playwright install` の明示実行であり
# lifecycle script ではないため、--ignore-scripts でも従来どおり動作する。
(cd e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --ignore-scripts --no-audit --no-fund)
if ((${#install_browsers[@]} > 0)); then
	E2E_STAGE="browser-install"
	# --with-deps は root 権限が必要なため CI では使わない。
	# firefox/webkit に必要なライブラリは同梱環境で揃っていることを確認済み。
	echo "==> Playwright ブラウザ導入: ${install_browsers[*]}"
	if ! install_playwright_browsers "${install_browsers[@]}"; then
		E2E_ERROR_REPORTED=1
		printf '::error::E2E setup failed at stage "browser-install" after %d attempts\n' \
			"$PLAYWRIGHT_INSTALL_MAX_ATTEMPTS" >&2
		exit 1
	fi
fi

# 導入後に実行可能性を再検出し、annotation / step summary へ可視化する。
# ここでの再検出は表示用であり、選択（selected_projects）は変更しない。
if [[ -n "$e2e_projects_explicit" ]]; then
	firefox_executable=0
	webkit_executable=0
	if detect_browser_executable firefox; then firefox_executable=1; fi
	if detect_browser_executable webkit; then webkit_executable=1; fi
fi
declare annotation_e2e_browser=""
declare summary_e2e_browser=""
format_e2e_browser_selection \
	selected_projects "$firefox_executable" "$webkit_executable" "$warn_fallback" \
	annotation_e2e_browser summary_e2e_browser
write_e2e_browser_selection "$annotation_e2e_browser" "$summary_e2e_browser"

echo "==> サーバーとクライアントを事前ビルド"
E2E_STAGE="server-build"
cargo build -p server --locked
# Tailwind はビルド時 CSS のみを使う（実行時 CDN は禁止）。
E2E_STAGE="npm-ci"
(cd client && npm ci --ignore-scripts --no-audit --no-fund && npm run build-css)
E2E_STAGE="trunk-build"
(cd client && trunk build)
# 外部 CDN / フォントへの参照が残っていないことをゲートする。
# 先に対象ファイルの存在を明示確認する（欠落時に grep が 2 を返す前に fail-fast）。
E2E_STAGE="external-asset-gate"
if [[ ! -f client/dist/index.html ]]; then
	e2e_stage_fail "required file is missing: client/dist/index.html"
fi
if [[ ! -f client/input.css ]]; then
	e2e_stage_fail "required file is missing: client/input.css"
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
		e2e_stage_fail "external asset reference remains in ${target_file}"
		;;
	1)
		# 不一致は想定どおり。この終了コードだけを意図的に無視する。
		;;
	*)
		e2e_stage_fail "failed to scan ${target_file} for external asset references (grep status: ${grep_status})."
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
	e2e_stage_fail "failed to enumerate generated CSS under client/dist."
fi
while IFS= read -r generated_css; do
	external_css_files+=("$generated_css")
done <"$generated_css_list"
for css_file in "${external_css_files[@]}"; do
	assert_no_external_asset_reference "$css_file"
done

echo "==> 型チェック"
E2E_STAGE="typecheck"
(cd e2e && npm run typecheck)

# 失敗アーカイブのパスを GITHUB_OUTPUT / GITHUB_STEP_SUMMARY へ渡す準備。
# artifact の公開は ci.yml 側（担当外）が独立ジョブで行うため、ここではパスの
# 受け渡しのみ行う。GitHub Actions 外・要約ファイル欠落・書込不可では no-op。
write_e2e_artifact_output() {
	local archive_path="$1"
	[[ -n "${GITHUB_ACTIONS:-}" ]] || return 0
	if [[ -n "${GITHUB_OUTPUT:-}" && -f "${GITHUB_OUTPUT}" && -w "${GITHUB_OUTPUT}" ]]; then
		printf 'e2e_failure_archive=%s\n' "$archive_path" >>"${GITHUB_OUTPUT}" || true
	fi
	if [[ -n "${GITHUB_STEP_SUMMARY:-}" && -f "${GITHUB_STEP_SUMMARY}" && -w "${GITHUB_STEP_SUMMARY}" ]]; then
		printf '### E2E failure artifact\n\n- archive: %s\n' "$archive_path" >>"${GITHUB_STEP_SUMMARY}" || true
	fi
	return 0
}

echo "==> Playwright テスト: ${selected_projects[*]}"
# extra_gates では artifact を回収できないため、失敗時は Playwright が
# 生成した error-context をジョブログへ出し、Checks にアノテーションを残す。
report_e2e_failure() {
	local label="$1"
	# ERR trap が段階名付き ::error:: を出す前に明示的なアノテーションを
	# 出した経路なので、二重出力を避けるためフラグを立てる。
	if ((E2E_ERROR_REPORTED == 0)); then
		E2E_ERROR_REPORTED=1
		echo "::error::Playwright E2E failed (${label})"
	fi
	find e2e/test-results -name 'error-context.md' -print 2>/dev/null || true
	find e2e/test-results -name 'error-context.md' -exec head -c 20000 {} \; 2>/dev/null || true

	# トレース・スクリーンショット・HTML レポートをタイムスタンプ付きで保存する。
	mkdir -p "$artifact_dir" || true
	archive="${artifact_dir}/e2e-failure-${label}-$(date -u +%Y%m%dT%H%M%SZ).tar.gz"
	if tar -czf "$archive" -C "$repo_root" e2e/test-results e2e/playwright-report 2>/dev/null; then
		echo "E2E failure artifacts: ${archive}"
		write_e2e_artifact_output "$archive"
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
E2E_STAGE="test-discovery"
remaining_specs_list="${workdir}/remaining-specs.list"
if ! find e2e/tests -maxdepth 1 -name '*.spec.ts' ! -name 'real-api.spec.ts' -print0 |
	LC_ALL=C sort -z >"$remaining_specs_list"; then
	e2e_stage_fail "failed to enumerate remaining E2E specs under e2e/tests."
fi
remaining_specs=()
while IFS= read -r -d '' spec_path; do
	# npm test へ渡すパスは従来どおり tests/<basename> の形式を維持する。
	remaining_specs+=("tests/${spec_path##*/}")
done <"$remaining_specs_list"
if ((${#remaining_specs[@]} == 0)); then
	e2e_stage_fail "no remaining E2E specs found under e2e/tests (excluding real-api.spec.ts)."
fi
if ! (cd e2e && npm test -- "${remaining_specs[@]}" "${project_args[@]}"); then
	report_e2e_failure "unit-specs"
fi
