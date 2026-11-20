#!/usr/bin/env bash
# prepare-distribution.sh の回帰テスト。外部サービスなしで成果物と fail-closed を検証する。
# cargo は一時 bin のダミーで差し替え、jq は実物を使う。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="${repo_root}/scripts/prepare-distribution.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "${work}/bin"

# ライセンス本文コピーループ用の fixture。ダミー文字列のみを使う
# （実ライセンス全文や秘密情報は使わない）。
fixture_dir="${work}/fixtures"
mkdir -p "${fixture_dir}/crate-a" "${fixture_dir}/crate-b" "${fixture_dir}/missing"
printf 'DUMMY LICENSE TEXT A\nsecond line\n' >"${fixture_dir}/crate-a/LICENSE"
printf 'DUMMY LICENSE TEXT B\n' >"${fixture_dir}/crate-b/LICENSE-MIT"
export DUMMY_FIXTURE_DIR="${fixture_dir}"

# cargo deny / cargo metadata のダミー。固定 JSON を返し、licenses 検査は
# DUMMY_CARGO_DENY_FAIL=1 のときだけ失敗させる（fail-closed の検証用）。
cat >"${work}/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
	metadata)
		# license_file が実在する通常 fixture と、存在しないパスを指す
		# fail-closed 検証用 fixture を DUMMY_METADATA_MISSING_LICENSE で切り替える。
		if [[ "${DUMMY_METADATA_MISSING_LICENSE:-0}" == "1" ]]; then
			# license_file も manifest_dir 配下の候補も存在しないため、
			# prepare-distribution.sh は非 0 で失敗する必要がある。
			jq -n --arg dir "${DUMMY_FIXTURE_DIR:?}" '{
				packages: [
					{
						name: "missing-license-crate",
						version: "0.1.0",
						license_file: ($dir + "/missing/NO_SUCH_LICENSE"),
						manifest_path: ($dir + "/missing/Cargo.toml"),
						source: "registry+https://example.invalid/index"
					}
				]
			}'
		else
			jq -n --arg dir "${DUMMY_FIXTURE_DIR:?}" '{
				packages: [
					{
						name: "fixture-crate-a",
						version: "1.2.3",
						license_file: ($dir + "/crate-a/LICENSE"),
						manifest_path: ($dir + "/crate-a/Cargo.toml"),
						source: "registry+https://example.invalid/index"
					},
					{
						name: "fixture-crate-b",
						version: "4.5.6",
						license_file: ($dir + "/crate-b/LICENSE-MIT"),
						manifest_path: ($dir + "/crate-b/Cargo.toml"),
						source: "registry+https://example.invalid/index"
					}
				]
			}'
		fi
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

# コピー先のライセンス本文が fixture と完全一致することを検証する。
assert_license_text() {
	local dir="$1" name="$2" file="$3" expected="$4"
	if [[ -f "${dir}/THIRD_PARTY_LICENSES/${file}" ]] && cmp -s "${dir}/THIRD_PARTY_LICENSES/${file}" "${expected}"; then
		echo "PASS ${name}: ${file} matches fixture"
	else
		echo "FAIL ${name}: ${file} missing or differs from fixture"
		failures=$((failures + 1))
	fi
}

echo "=== ケース1: 通常生成と4成果物の存在 ==="
expect_success "case1 generate" bash "$script" "$normal_out"
assert_present "$normal_out" "case1 artifacts"
assert_license_text "$normal_out" "case1 license text" "fixture-crate-a-1.2.3.txt" "${fixture_dir}/crate-a/LICENSE"
assert_license_text "$normal_out" "case1 license text" "fixture-crate-b-4.5.6.txt" "${fixture_dir}/crate-b/LICENSE-MIT"

echo "=== ケース2: NOTICE 件数と DEPENDENCIES.tsv 行数の整合（固定値なし） ==="
expect_success "case2 generate" bash "$script" "$consistency_out"
assert_notice_matches_tsv "$consistency_out" "case2 consistency"
assert_license_text "$consistency_out" "case2 license text" "fixture-crate-a-1.2.3.txt" "${fixture_dir}/crate-a/LICENSE"
assert_license_text "$consistency_out" "case2 license text" "fixture-crate-b-4.5.6.txt" "${fixture_dir}/crate-b/LICENSE-MIT"

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
assert_license_text "$existing_out" "case4 license text" "fixture-crate-a-1.2.3.txt" "${fixture_dir}/crate-a/LICENSE"
assert_license_text "$existing_out" "case4 license text" "fixture-crate-b-4.5.6.txt" "${fixture_dir}/crate-b/LICENSE-MIT"
# 上書き時に旧成果物の古いライセンス本文が残っていないことを検証する。
if [[ ! -e "${existing_out}/THIRD_PARTY_LICENSES/stale.txt" ]]; then
	echo "PASS case4: stale.txt removed"
else
	echo "FAIL case4: stale.txt still present"
	failures=$((failures + 1))
fi

echo "=== ケース5: ライセンス本文コピー失敗時の fail-closed ==="
# license_file が存在せずフォールバック候補もない場合、prepare-distribution.sh は
# 非 0 で終了する必要がある（黙ってスキップして成功してはいけない）。
missing_license_out="${work}/out-missing-license"
expect_failure "case5 missing license file" env DUMMY_METADATA_MISSING_LICENSE=1 bash "$script" "$missing_license_out"
assert_absent "$missing_license_out" "case5 artifacts"

echo "=== ケース6: ライセンス本文コピー失敗時に既存の完全な成果物が不変であること ==="
# オラクル: 完全な成果物を用意した出力先へライセンス本文コピー失敗の実行を
# 重ねても、既存の成果物がバイト単位で不変で、中途半端な上書きや空の
# ディレクトリ、.tmp.* の一時ディレクトリが残らないことを検証する。
preserved_out="${work}/out-preserved"
preserved_snapshot="${work}/out-preserved-snapshot"

# 事前に完全な成果物を用意し、実行前のスナップショットを取る。
expect_success "case6 prepare baseline" bash "$script" "$preserved_out"
cp -a "$preserved_out" "$preserved_snapshot"

# license_file もフォールバック候補も存在しないケースを発生させる。
expect_failure "case6 missing license preserves existing" env DUMMY_METADATA_MISSING_LICENSE=1 bash "$script" "$preserved_out"

# ディレクトリツリー全体が実行前と同一であることを検証する（新規/削除ファイル
# や空ディレクトリもここで検出する）。
if diff -r -q "$preserved_snapshot" "$preserved_out" >"${work}/case6.diff" 2>&1; then
	echo "PASS case6: existing artifact tree unchanged"
else
	echo "FAIL case6: existing artifact tree changed"
	cat "${work}/case6.diff" >&2
	failures=$((failures + 1))
fi

# 個別の成果物も cmp で明示的に完全一致を検証する（md5/cmp による確認）。
for artifact in LICENSE DEPENDENCIES.tsv NOTICE; do
	if cmp -s "${preserved_snapshot}/${artifact}" "${preserved_out}/${artifact}"; then
		echo "PASS case6: ${artifact} unchanged"
	else
		echo "FAIL case6: ${artifact} missing or changed"
		failures=$((failures + 1))
	fi
done

# THIRD_PARTY_LICENSES/*.txt が実行前と同一であることを検証する。
while IFS= read -r -d '' snapshot_file; do
	rel="${snapshot_file#"${preserved_snapshot}/"}"
	if [[ -f "${preserved_out}/${rel}" ]] && cmp -s "$snapshot_file" "${preserved_out}/${rel}"; then
		echo "PASS case6: ${rel} unchanged"
	else
		echo "FAIL case6: ${rel} missing or changed"
		failures=$((failures + 1))
	fi
done < <(find "${preserved_snapshot}/THIRD_PARTY_LICENSES" -maxdepth 1 -type f -name '*.txt' -print0 2>/dev/null || true)

# 実行前後でエントリ数が同じこと（空ディレクトリやゴミが残っていないこと）を確認する。
snapshot_count="$(find "$preserved_snapshot" -mindepth 1 | wc -l | tr -d '[:space:]')"
current_count="$(find "$preserved_out" -mindepth 1 | wc -l | tr -d '[:space:]')"
if [[ "$snapshot_count" == "$current_count" ]]; then
	echo "PASS case6: no extra entries (snapshot=${snapshot_count}, current=${current_count})"
else
	echo "FAIL case6: entry count changed (snapshot=${snapshot_count}, current=${current_count})"
	failures=$((failures + 1))
fi

# .tmp.* の一時ディレクトリが残っていないことを検証する。
# ステージング先は出力先直下またはその親に作られる想定で両方を走査する。
tmp_leftovers="$(find "$work" -type d \( -name '.tmp.*' -o -name '*.tmp.*' \) -print 2>/dev/null || true)"
if [[ -z "$tmp_leftovers" ]]; then
	echo "PASS case6: no .tmp.* directories left"
else
	echo "FAIL case6: leftover .tmp.* directories: ${tmp_leftovers}"
	failures=$((failures + 1))
fi

if ((failures > 0)); then
	echo "prepare-distribution tests FAILED (${failures})" >&2
	exit 1
fi
echo "prepare-distribution tests OK"
