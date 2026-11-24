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
		if [[ "${DUMMY_METADATA_EMPTY:-0}" == "1" ]]; then
			# 依存 0 件: ライセンス本文が 1 件も公開されないため fail-closed。
			jq -n '{ packages: [] }'
		elif [[ "${DUMMY_METADATA_MISSING_LICENSE:-0}" == "1" ]]; then
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

# 配布物が非機微な公開物として適切な権限（dir 755 / file 644）で配置されたことを検証する。
# backup-rotate の秘密データ（0600）とは権限契約が異なる点に注意。
assert_public_permissions() {
	local dir="$1" name="$2"
	local mode path txt_count=0

	mode="$(stat -c '%a' "$dir")"
	if [[ "$mode" == "755" ]]; then
		echo "PASS ${name}: out_dir mode 755"
	else
		echo "FAIL ${name}: out_dir mode ${mode} (expected 755)"
		failures=$((failures + 1))
	fi

	for path in "$dir/LICENSE" "$dir/NOTICE" "$dir/DEPENDENCIES.tsv"; do
		mode="$(stat -c '%a' "$path")"
		if [[ "$mode" == "644" ]]; then
			echo "PASS ${name}: ${path##*/} mode 644"
		else
			echo "FAIL ${name}: ${path##*/} mode ${mode} (expected 644)"
			failures=$((failures + 1))
		fi
	done

	mode="$(stat -c '%a' "${dir}/THIRD_PARTY_LICENSES")"
	if [[ "$mode" == "755" ]]; then
		echo "PASS ${name}: THIRD_PARTY_LICENSES dir mode 755"
	else
		echo "FAIL ${name}: THIRD_PARTY_LICENSES dir mode ${mode} (expected 755)"
		failures=$((failures + 1))
	fi

	while IFS= read -r -d '' path; do
		txt_count=$((txt_count + 1))
		mode="$(stat -c '%a' "$path")"
		if [[ "$mode" == "644" ]]; then
			echo "PASS ${name}: ${path##*/} mode 644"
		else
			echo "FAIL ${name}: ${path##*/} mode ${mode} (expected 644)"
			failures=$((failures + 1))
		fi
	done < <(find "${dir}/THIRD_PARTY_LICENSES" -maxdepth 1 -type f -name '*.txt' -print0)
	if ((txt_count > 0)); then
		echo "PASS ${name}: checked ${txt_count} third-party license text(s)"
	else
		echo "FAIL ${name}: no third-party license text generated"
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

echo "=== ケース7: 配布物の公開権限（dir 755 / file 644、非機微） ==="
perm_out="${work}/out-perms"
# 実行ビット付きのライセンス本文をソースにすると、chmod -R a+rX の大文字 X が
# 実行ビットを残して 755 になる。find -exec chmod 644 で必ず 644 へ正規化されることを検証する。
exec_source="${fixture_dir}/crate-a/LICENSE"
chmod 755 "$exec_source"
expect_success "case7 generate" bash "$script" "$perm_out"
assert_public_permissions "$perm_out" "case7 permissions"

# 実行ビット付き fixture 由来の .txt が 644 に正規化されたことを stat -c %a で明示検証する。
perm_txt="${perm_out}/THIRD_PARTY_LICENSES/fixture-crate-a-1.2.3.txt"
perm_mode="$(stat -c '%a' "$perm_txt")"
if [[ "$perm_mode" == "644" ]]; then
	echo "PASS case7: executable-source license normalized to 644"
else
	echo "FAIL case7: executable-source license mode ${perm_mode} (expected 644)"
	failures=$((failures + 1))
fi

echo "=== ケース8〜12: 出力先検証と rollback（実 repo_root はwork配下のコピーで保護） ==="
# prepare-distribution.sh は BASH_SOURCE から repo_root を解決するため、
# スクリプトと LICENSE を work 配下へコピーして実行する（実 repo_root は変更しない）。
repo_copy="${work}/repo-copy"
mkdir -p "${repo_copy}/scripts"
cp "$script" "${repo_copy}/scripts/prepare-distribution.sh"
cp "${repo_root}/LICENSE" "${repo_copy}/LICENSE"
copy_script="${repo_copy}/scripts/prepare-distribution.sh"

echo "=== ケース8: '..' 成分の拒否 ==="
expect_failure "case8 dotdot suffix rejected" bash "$copy_script" "${repo_copy}/dist/../escape"
expect_failure "case8 dotdot prefix rejected" bash "$copy_script" "../escape"
if [[ -e "${work}/escape" || -e "${repo_copy}/escape" || -e "${repo_copy}/dist" ]]; then
	echo "FAIL case8: path escaped via '..'"
	failures=$((failures + 1))
else
	echo "PASS case8: '..' component refused"
fi

echo "=== ケース9: repo_root / 祖先の置換拒否 ==="
expect_failure "case9 repo_root rejected" bash "$copy_script" "$repo_copy"
expect_failure "case9 ancestor rejected" bash "$copy_script" "$work"
if [[ ! -f "${repo_copy}/LICENSE" ]]; then
	echo "FAIL case9: repo_root copy was disturbed"
	failures=$((failures + 1))
else
	echo "PASS case9: repo_root / ancestor refused and copy intact"
fi

echo "=== ケース10: jq / cargo 未検出の fail-closed ==="
no_tools_bin="${work}/no-tools-bin"
mkdir -p "$no_tools_bin"
# repo_root 解決に使う dirname だけを残し、jq / cargo は見えないようにする。
dirname_path="$(command -v dirname)"
ln -s "$dirname_path" "${no_tools_bin}/dirname"
expect_failure "case10 jq not found" env PATH="$no_tools_bin" /bin/bash "$copy_script" "${work}/out-no-jq"
# jq は見えるが cargo が無い状態を作り、cargo 側のチェックも検証する。
no_cargo_bin="${work}/no-cargo-bin"
mkdir -p "$no_cargo_bin"
jq_path="$(command -v jq)"
ln -s "$dirname_path" "${no_cargo_bin}/dirname"
ln -s "$jq_path" "${no_cargo_bin}/jq"
expect_failure "case10 cargo not found" env PATH="$no_cargo_bin" /bin/bash "$copy_script" "${work}/out-no-cargo"
if [[ ! -e "${work}/out-no-jq" && ! -e "${work}/out-no-cargo" ]]; then
	echo "PASS case10: no artifact created without required tools"
else
	echo "FAIL case10: artifact created without required tools"
	failures=$((failures + 1))
fi

echo "=== ケース11: INT / TERM で rollback が通ること ==="
signal_bin="${work}/signal-bin"
mkdir -p "$signal_bin"
cp "${work}/bin/cargo" "${signal_bin}/cargo-real"
cat >"${signal_bin}/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
# deny list の時点で親スクリプトへシグナルを送り、EXIT trap（rollback）を強制する。
if [[ "${2:-}" == "list" && -n "${SIGNAL_NAME:-}" ]]; then
	kill -"${SIGNAL_NAME}" "$PPID"
fi
exec "${DUMMY_CARGO_REAL}" "$@"
EOF
chmod +x "${signal_bin}/cargo"
signal_out="${repo_copy}/out-signal"
mkdir -p "$signal_out"
printf 'KEEP ME\n' >"${signal_out}/marker"
signal_snapshot="${work}/signal-snapshot"
cp -a "$signal_out" "$signal_snapshot"
for signal_name in INT TERM; do
	expect_failure "case11 ${signal_name} triggers rollback" env \
		PATH="${signal_bin}:${work}/bin:${PATH}" \
		DUMMY_CARGO_REAL="${signal_bin}/cargo-real" \
		SIGNAL_NAME="$signal_name" \
		bash "$copy_script" "$signal_out"
	if diff -r -q "$signal_snapshot" "$signal_out" >"${work}/case11-${signal_name}.diff" 2>&1; then
		echo "PASS case11 ${signal_name}: existing artifact preserved by rollback"
	else
		echo "FAIL case11 ${signal_name}: existing artifact changed"
		cat "${work}/case11-${signal_name}.diff" >&2
		failures=$((failures + 1))
	fi
done
if [[ -z "$(find "$work" -type d -name '*.tmp.*' -print -quit 2>/dev/null || true)" ]]; then
	echo "PASS case11: no .tmp.* leftovers"
else
	echo "FAIL case11: .tmp.* leftover"
	failures=$((failures + 1))
fi

echo "=== ケース12: 旧成果物削除失敗時に published rollback が働くこと ==="
rm_fail_bin="${work}/rm-fail-bin"
mkdir -p "$rm_fail_bin"
cat >"${rm_fail_bin}/rm" <<'EOF'
#!/usr/bin/env bash
for arg in "$@"; do
	if [[ "$arg" == *.old.* ]]; then
		echo "simulated old artifact deletion failure" >&2
		exit 14
	fi
done
exec /bin/rm "$@"
EOF
chmod +x "${rm_fail_bin}/rm"
delete_fail_out="${repo_copy}/out-delete-fail"
mkdir -p "$delete_fail_out"
printf 'OLD MARKER\n' >"${delete_fail_out}/marker"
expect_failure "case12 old artifact deletion failure" env \
	PATH="${rm_fail_bin}:${work}/bin:${PATH}" \
	bash "$copy_script" "$delete_fail_out"
if [[ -f "${delete_fail_out}/marker" ]] && [[ "$(cat "${delete_fail_out}/marker")" == "OLD MARKER" ]]; then
	echo "PASS case12: old artifact restored after deletion failure"
else
	echo "FAIL case12: old artifact not restored"
	failures=$((failures + 1))
fi
if compgen -G "${delete_fail_out}.old.*" >/dev/null; then
	echo "FAIL case12: stashed directory left behind"
	failures=$((failures + 1))
else
	echo "PASS case12: stashed directory not left behind"
fi

echo "=== ケース13: 依存 0 件の配布物を fail-closed で拒否 ==="
empty_out="${repo_copy}/out-empty-metadata"
expect_failure "case13 zero dependency metadata" env DUMMY_METADATA_EMPTY=1 bash "$script" "$empty_out"
assert_absent "$empty_out" "case13 artifacts"

if ((failures > 0)); then
	echo "prepare-distribution tests FAILED (${failures})" >&2
	exit 1
fi
echo "prepare-distribution tests OK"
