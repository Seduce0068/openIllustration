#!/usr/bin/env bash
# ドキュメント記載の数値（E2E件数 / doctorチェック数 / migration範囲）が
# 実コードと乖離していないことを検査する回帰スクリプト。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

failures=0

# 期待値（実コードから算出）と記載値（ドキュメントから抽出）を比較する。
# 差分があれば「期待値 vs 記載値」を stderr へ出し、failures を増やす。
assert_numbers_match() {
	local label="$1" expected="$2" documented="$3"
	if [[ "$expected" == "$documented" ]]; then
		echo "PASS ${label}: ${expected}"
	else
		echo "FAIL ${label}: expected=${expected} documented=${documented}" >&2
		failures=$((failures + 1))
	fi
}

# 対象行から "<数> check(s)" の数値だけを抽出する（他形式は無視）。
extract_checks() {
	grep -oE '[0-9]+ check\(s\)' <<<"$1" | grep -oE '[0-9]+' | head -n1 || true
}

# --- 1. E2E 件数 ----------------------------------------------------------
spec="docs/TEST_SPECIFICATION.md"
e2e_total="$(grep -rc '^  test(' e2e/tests/*.spec.ts | awk -F: '{s+=$2} END {print s+0}')"

measured_line="$(grep -m1 '実測は全[0-9][0-9]*件' "$spec" || true)"
measured_n="$(grep -oE '実測は全[0-9]+件' <<<"$measured_line" | grep -oE '[0-9]+' | head -n1 || true)"
assert_numbers_match "e2e total" "$e2e_total" "$measured_n"

# 内訳は「実測は全...件」から「この内訳は...」までの行に限定して抽出する。
breakdown_block="$(awk '/実測は全[0-9]+件/{inside=1} inside{print} /この内訳は/{inside=0}' "$spec")"

# ドキュメント内訳は "<label> <count>"（例: auth 2 / build-gate 1）で書かれる。
# 行の順序に依存せず label -> count の連想配列へパースする。
declare -A documented_breakdown=()
while IFS= read -r pair; do
	[[ -z "$pair" ]] && continue
	documented_breakdown["${pair%% *}"]="${pair##* }"
done < <(grep -oE '[A-Za-z0-9][A-Za-z0-9_-]* [0-9]+' <<<"$breakdown_block" || true)

# ラベルは e2e/tests/*.spec.ts の basename（.spec.ts を除く）から動的生成する。
# 固定配列だと新規 spec 追加時にドキュメント側の新ラベルが照合されず誤 FAIL になる。
mapfile -t e2e_labels < <(
	for spec_file in e2e/tests/*.spec.ts; do
		[[ -f "$spec_file" ]] || continue
		name="${spec_file##*/}"
		printf '%s\n' "${name%.spec.ts}"
	done | sort
)

# ドキュメント側に実 spec が存在しないラベルが残っていないことも照合する。
for label in "${!documented_breakdown[@]}"; do
	if [[ ! -f "e2e/tests/${label}.spec.ts" ]]; then
		echo "FAIL e2e breakdown: documented label '${label}' has no matching spec file" >&2
		failures=$((failures + 1))
	fi
done

breakdown_sum=0
for label in "${e2e_labels[@]}"; do
	documented="${documented_breakdown[$label]:-}"
	actual="$(grep -c '^  test(' "e2e/tests/${label}.spec.ts" || true)"
	assert_numbers_match "e2e ${label}" "$actual" "$documented"
	# 先頭ゼロ付き数値を8進数として解釈しないよう 10# を付ける。
	breakdown_sum=$((breakdown_sum + 10#${documented:-0}))
done
assert_numbers_match "e2e breakdown sum" "$e2e_total" "$breakdown_sum"

# --- 2. doctor チェック数 -------------------------------------------------
# コメント行の "results.push" を数えないよう、行頭（空白 + results.push(）に限定する。
doctor_checks="$(grep -cE '^[[:space:]]*results\.push\(' server/src/bin/doctor.rs || true)"
doctor_lines="$(grep -rnE --include='*.md' '[0-9]+ check\(s\)' docs || true)"
if [[ -z "$doctor_lines" ]]; then
	echo "FAIL doctor check(s): no documented 'N check(s)' line found in docs" >&2
	failures=$((failures + 1))
fi
while IFS= read -r doc_line; do
	[[ -z "$doc_line" ]] && continue
	loc="$(cut -d: -f1,2 <<<"$doc_line")"
	assert_numbers_match "doctor check(s) @ ${loc}" "$doctor_checks" "$(extract_checks "$doc_line")"
done <<<"$doctor_lines"

# --- 3. migration 範囲 ----------------------------------------------------
# shellcheck disable=SC2012
migration_count="$(ls server/migrations/*.sql | wc -l | tr -d '[:space:]')"
design="docs/DETAILED_DESIGN.md"
range_line="$(grep -m1 '番号付きSQL' "$design" || true)"
range_nums="$(grep -oE '[0-9]{3}_[[:alnum:]_]+\.sql' <<<"$range_line" | grep -oE '^[0-9]{3}' || true)"
range_start="$(sort -n <<<"$range_nums" | head -n1)"
range_end="$(sort -n <<<"$range_nums" | tail -n1)"
if [[ -z "$range_start" || -z "$range_end" ]]; then
	echo "FAIL migration range: no 'NNN_name.sql 〜 NNN_name.sql' description in ${design}" >&2
	failures=$((failures + 1))
else
	# 先頭ゼロ付きの番号を8進数として解釈しないよう 10# を付ける
	documented_count=$((10#$range_end - 10#$range_start + 1))
	assert_numbers_match "migration range ${range_start}-${range_end}" "$migration_count" "$documented_count"
fi

# --- 結果 ----------------------------------------------------------------
if ((failures > 0)); then
	echo "doc numbers FAILED (${failures})" >&2
	exit 1
fi
echo "doc numbers OK: e2e=${e2e_total} doctor=${doctor_checks} migrations=${migration_count}"
exit 0
