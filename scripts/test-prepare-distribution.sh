#!/usr/bin/env bash
# prepare-distribution.sh の回帰テスト。外部サービスなしで成果物と fail-closed を検証する。
# cargo は一時 bin のダミーで差し替え、jq は実物を使う。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="${repo_root}/scripts/prepare-distribution.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "${work}/bin"

# cargo deny / cargo metadata のダミー。固定 JSON を返し、licenses 検査は
# DUMMY_CARGO_DENY_FAIL=1 のときだけ失敗させる（fail-closed の検証用）。
cat >"${work}/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
	metadata)
		printf '%s\n' '{"packages":[]}'
		exit 0
		;;
	deny)
		;;
	*)
		echo "dummy cargo: unsupported command: ${1:-}" >&2
		exit 1
		;;
esac

case "${2:-}" in
	check)
		if [[ "${3:-}" != "licenses" ]]; then
			echo "dummy cargo: unsupported deny check target: ${3:-}" >&2
			exit 1
		fi
		if [[ "${DUMMY_CARGO_DENY_FAIL:-0}" == "1" ]]; then
			echo "dummy cargo: deny check licenses failed" >&2
			exit 1
		fi
		exit 0
		;;
	list)
		if [[ "${3:-}" != "--format" || "${4:-}" != "json" ]]; then
			echo "dummy cargo: unsupported deny list args: $*" >&2
			exit 1
		fi
		cat <<'JSON'
{
	"licenses": [
		["MIT", ["crate-mit-a", "crate-mit-b"]],
		["Apache-2.0", ["crate-apache"]]
	]
}
JSON
		exit 0
		;;
	*)
		echo "dummy cargo: unsupported deny subcommand: ${2:-}" >&2
		exit 1
		;;
esac
EOF
chmod +x "${work}/bin/cargo"

# ダミー cargo を PATH 先頭に置く。jq は ${work}/bin に無いため実物が使われる。
export PATH="${work}/bin:${PATH}"

normal_out="${work}/out-normal"
consistency_out="${work}/out-consistency"
fail_out="${work}/out-fail"
existing_out="${work}/out-existing"

artifacts=(LICENSE DEPENDENCIES.tsv NOTICE THIRD_PARTY_LICENSES)

failures=0
expect_success() {
	local name="$1"
	shift
	if "$@" >"${work}/out.log" 2>&1; then
		echo "PASS ${name}"
	else
		echo "FAIL ${name}: expected success but failed"
		cat "${work}/out.log" >&2
		failures=$((failures + 1))
	fi
}
expect_failure() {
	local name="$1"
	shift
	if "$@" >"${work}/out.log" 2>&1; then
		echo "FAIL ${name}: expected failure but succeeded"
		cat "${work}/out.log" >&2
		failures=$((failures + 1))
	else
		echo "PASS ${name}"
	fi
}

assert_present() {
	local dir="$1" name="$2" artifact
	for artifact in "${artifacts[@]}"; do
		if [[ -e "${dir}/${artifact}" ]]; then
			echo "PASS ${name}: ${artifact}"
		else
			echo "FAIL ${name}: ${artifact} missing"
			failures=$((failures + 1))
		fi
	done
}

assert_absent() {
	local dir="$1" name="$2" artifact
	for artifact in "${artifacts[@]}"; do
		if [[ -e "${dir}/${artifact}" ]]; then
			echo "FAIL ${name}: ${artifact} unexpectedly generated"
			failures=$((failures + 1))
		else
			echo "PASS ${name}: ${artifact} not generated"
		fi
	done
}

assert_notice_matches_tsv() {
	local dir="$1" name="$2"
	local notice_count tsv_count
	notice_count="$(awk -F': ' '/^License entries: / {print $2}' "${dir}/NOTICE" 2>/dev/null || true)"
	tsv_count="$(wc -l <"${dir}/DEPENDENCIES.tsv" 2>/dev/null | tr -d '[:space:]' || true)"
	if [[ -n "$notice_count" && "$notice_count" == "$tsv_count" ]]; then
		echo "PASS ${name}: NOTICE=${notice_count} == DEPENDENCIES.tsv lines=${tsv_count}"
	else
		echo "FAIL ${name}: NOTICE='${notice_count}' != DEPENDENCIES.tsv lines='${tsv_count}'"
		failures=$((failures + 1))
	fi
}

echo "=== ケース1: 通常生成と4成果物の存在 ==="
expect_success "case1 generate" bash "$script" "$normal_out"
assert_present "$normal_out" "case1 artifacts"

echo "=== ケース2: NOTICE 件数と DEPENDENCIES.tsv 行数の整合（固定値なし） ==="
expect_success "case2 generate" bash "$script" "$consistency_out"
assert_notice_matches_tsv "$consistency_out" "case2 consistency"

echo "=== ケース3: deny 失敗時の fail-closed（成果物なし） ==="
expect_failure "case3 deny failure" env DUMMY_CARGO_DENY_FAIL=1 bash "$script" "$fail_out"
assert_absent "$fail_out" "case3 artifacts"

echo "=== ケース4: 既存成果物の上書き ==="
mkdir -p "${existing_out}/THIRD_PARTY_LICENSES"
printf 'OLD LICENSE MARKER\n' >"${existing_out}/LICENSE"
printf 'OLD\toldcrate\n' >"${existing_out}/DEPENDENCIES.tsv"
printf 'License entries: 999\n' >"${existing_out}/NOTICE"
printf 'stale license\n' >"${existing_out}/THIRD_PARTY_LICENSES/stale.txt"
expect_success "case4 generate over existing" bash "$script" "$existing_out"
assert_present "$existing_out" "case4 artifacts"
if cmp -s "${existing_out}/LICENSE" "${repo_root}/LICENSE"; then
	echo "PASS case4: LICENSE overwritten"
else
	echo "FAIL case4: LICENSE not overwritten"
	failures=$((failures + 1))
fi
if grep -q 'crate-mit-a' "${existing_out}/DEPENDENCIES.tsv" && ! grep -q 'oldcrate' "${existing_out}/DEPENDENCIES.tsv"; then
	echo "PASS case4: DEPENDENCIES.tsv overwritten"
else
	echo "FAIL case4: DEPENDENCIES.tsv not overwritten"
	failures=$((failures + 1))
fi
if ! grep -q '999' "${existing_out}/NOTICE"; then
	echo "PASS case4: NOTICE overwritten"
else
	echo "FAIL case4: NOTICE not overwritten"
	failures=$((failures + 1))
fi
assert_notice_matches_tsv "$existing_out" "case4 consistency"

if ((failures > 0)); then
	echo "prepare-distribution tests FAILED (${failures})" >&2
	exit 1
fi
echo "prepare-distribution tests OK"
