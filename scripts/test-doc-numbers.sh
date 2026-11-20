#!/usr/bin/env bash
# ドキュメント記載の数値（E2E件数 / doctorチェック数 / migration範囲 / CLI bin テスト数）と
# Rust テスト名が実コードと乖離していないことを検査する回帰スクリプト。
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

# --- 1b. E2E 総数表記の古い件数検出（未注記は fail-closed） ----------------
# 走査対象は TEST_SPECIFICATION.md（$spec）のみ。他ドキュメントは対象外だが、
# 現状は全て注記済み（当時 / 旧 / 履歴 / 当初）のため実害はない。
# フルスイートの E2E 総数を示す文脈（"定義数はN件" / "実測は全N件"）の数値を走査し、
# 実測値（e2e_total / measured_n）と異なる数値には同じ行の注記
# （当時 / 旧 / 履歴 / 当初）を要求する。注記が無ければ FAIL。
# 注記語は古い件数の直前に置かれるものだけを許容し、"現在" は救済語に含めない。
# 個別 spec の内訳件数・テスト名/パス内の数値・セクション6で照合済みの
# 個別 spec 実行例コメント件数は総数文脈ではないため、総数キーワード直後の
# 数値（注記語の長さに上限なし） only を抽出し、注記判定も同じマッチに対して行う。
# 外側の行走査と内側のマッチ抽出は同一パターン変数を共有し、パターンが一致する行の
# 取りこぼしはない。修飾語の長さに上限を設けないため、長い注記付きの行も走査対象になる。
total_count_pattern='(定義数は|実測は全)[^0-9]*[0-9]+[[:space:]]*件'
while IFS= read -r total_entry; do
	[[ -z "$total_entry" ]] && continue
	line_no="${total_entry%%:*}"
	line_body="${total_entry#*:}"
	while IFS= read -r total_match; do
		[[ -z "$total_match" ]] && continue
		# マッチ部分だけから数値を取り、注記も同じマッチ部分で判定する
		# （行全体だと他セルの注記に誤って救われるため）。
		total_n="$(grep -oE '[0-9]+' <<<"$total_match" | head -n1 || true)"
		[[ -z "$total_n" ]] && continue
		[[ "$total_n" == "$e2e_total" || "$total_n" == "$measured_n" ]] && continue
		if grep -qE '当時|旧|履歴|当初' <<<"$total_match"; then
			continue
		fi
		echo "FAIL e2e stale total @ ${spec}:${line_no}: ${total_n} vs ${e2e_total} without annotation" >&2
		failures=$((failures + 1))
	done < <(
		printf '%s\n' "$line_body" |
			grep -oE "$total_count_pattern" ||
			true
	)
done < <(grep -nE "$total_count_pattern" "$spec" || true)

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

# --- 4. Rust テスト名の照合 -----------------------------------------------
# ドキュメント側の `test_<name>` トークンが実コードの `fn test_<name>` に
# 存在するかを照合する。rename された古い名前がドキュメントに残ると FAIL。
# 実コード側は `fn test_` で走査するため `test_http_client` / `test_key_pair`
# 等のヘルパー関数も実在として扱う（誤検知を避けるため意図的に残す）。

# 抽出上の誤検知だけを最小限定で除外する。
# - test_config: `proptest_config(...)` から `test_config` が部分一致する（L529）
# - test_state: `rate_limit_test_state(...)` から `test_state` が部分一致する（L990）
# - test_jwt_verification_keys_json_: `test_jwt_verification_keys_json_*` の
#   ワイルドカード抽出尾（L147）。実コード側の `..._keys_from_env_verification_keys_json_*` とは別物。
declare -A doc_test_exclusions=(
	[test_config]=1
	[test_state]=1
	[test_jwt_verification_keys_json_]=1
)

mapfile -t doc_test_names < <(
	while IFS= read -r name; do
		[[ -z "$name" ]] && continue
		[[ -n "${doc_test_exclusions[$name]:-}" ]] && continue
		printf '%s\n' "$name"
	done < <(grep -oE 'test_[a-z0-9_]+' "$spec" | sort -u || true)
)

# 実在テスト名集合を作る（関数シグネチャの `fn ` を除去）。
# server/src/ 配下には bin/ も含まれるが、各 bin の #[test] fn を明示的にも走査し取りこぼしを防ぐ。
declare -A code_test_names=()
while IFS= read -r name; do
	[[ -z "$name" ]] && continue
	code_test_names["$name"]=1
done < <(
	{
		grep -rhoE 'fn test_[a-z0-9_]+' server/src/ common/src/ client/src/ || true
		for bin_dir in server/src/bin common/src/bin client/src/bin; do
			[[ -d "$bin_dir" ]] || continue
			grep -rhoE 'fn test_[a-z0-9_]+' "$bin_dir" || true
		done
	} | sed -E 's/^fn //' | sort -u
)

for name in "${doc_test_names[@]}"; do
	if [[ -z "${code_test_names[$name]:-}" ]]; then
		echo "FAIL test name: '${name}' is documented in ${spec} but not found in source" >&2
		failures=$((failures + 1))
	fi
done

# --- 5. bin テスト数の照合 -------------------------------------------------
# 7 bin（backup_db / doctor / restore_db / sign_audit_export / verify_audit_export /
# verify_audit_log / verify_backup）の #[test] / #[tokio::test] の実数を grep -c で数え、
# TEST_SPECIFICATION の記載件数（<bin> <N>件 / <bin> <N>テスト）と照合する。
# 一方向のみ: ドキュメント側の記載が実コードとずれていれば FAIL とし、実コード側の
# 全テストがドキュメントへ載っていることは要求しない（履歴表記と衝突しないため）。
# 内訳・増分表記（"当初17件" / "追加4件" / "追加警告 2件" 等）は総数ではないため、
# 抽出後の短い一致ではなく元の行の段階で除外する。総数行の括弧内にある "既存29件" 等の
# 内訳は <bin> 直後の総数だけが抽出されるため照合対象に残る（既存件数自体は誤抽出されない）。
doc_bin_counts() {
	local bin="$1"
	grep -vE '(当初|旧|追加)[^0-9]{0,4}[0-9]+(件|テスト)' "$spec" |
		grep -oE "${bin}[^0-9]*[0-9]+(件|テスト)" |
		grep -oE '[0-9]+' |
		sort -u ||
		true
}

# DETAILED_DESIGN.md の CLI 表から「テスト数」列（3列目）の総数を取り出す。
# 行形式: | `doctor` | ... | 45（既存29＋...） | ... |
# 括弧内の内訳数（既存29 等）は総数ではないため、セルの先頭数値だけを採る。
doc_cli_table_count() {
	local bin="$1"
	awk -F'|' -v target="$bin" '
		{
			name = $2
			gsub(/^[[:space:]`,]+|[[:space:]`,]+$/, "", name)
			if (name != target) next
			cell = $4
			gsub(/^[[:space:]]+|[[:space:]]+$/, "", cell)
			if (match(cell, /^[0-9]+/)) {
				print substr(cell, 1, RLENGTH)
				exit
			}
		}
	' "$design" || true
}

# server/src/bin/*.rs の実ファイルを列挙し、basename（.rs を除く）を bin 名へ変換する。
# 既知 7 bin は従来どおり照合し、新規 bin（既知一覧に無いもの）は未知 bin として
# 既存の unknown bin 分岐を再利用して fail-closed に FAIL させる。
mapfile -t bin_names < <(
	for bin_file in server/src/bin/*.rs; do
		[[ -f "$bin_file" ]] || continue
		name="${bin_file##*/}"
		printf '%s\n' "${name%.rs}"
	done | sort
)

for bin_name in "${bin_names[@]}"; do
	case "$bin_name" in
	backup_db) bin_file="server/src/bin/backup_db.rs" ;;
	doctor) bin_file="server/src/bin/doctor.rs" ;;
	restore_db) bin_file="server/src/bin/restore_db.rs" ;;
	sign_audit_export) bin_file="server/src/bin/sign_audit_export.rs" ;;
	verify_audit_export) bin_file="server/src/bin/verify_audit_export.rs" ;;
	verify_audit_log) bin_file="server/src/bin/verify_audit_log.rs" ;;
	verify_backup) bin_file="server/src/bin/verify_backup.rs" ;;
	*)
		echo "FAIL unknown bin ${bin_name}" >&2
		failures=$((failures + 1))
		continue
		;;
	esac
	actual_tests="$(grep -cE '^[[:space:]]*#\[(tokio::)?test\]' "$bin_file" || true)"

	# 既存どおり TEST_SPECIFICATION の記載件数（<bin> <N>件 / <bin> <N>テスト）を照合。
	documented_tests="$(doc_bin_counts "$bin_name")"
	if [[ -z "$documented_tests" ]]; then
		echo "FAIL ${bin_name} test count: no documented '<${bin_name}> <N>件' line found in ${spec}" >&2
		failures=$((failures + 1))
	else
		while IFS= read -r documented_n; do
			[[ -z "$documented_n" ]] && continue
			assert_numbers_match "${bin_name} test count" "$actual_tests" "$documented_n"
		done <<<"$documented_tests"
	fi

	# 追加: DETAILED_DESIGN.md の CLI テスト数表も実コードの #[test] 実数と照合する。
	design_n="$(doc_cli_table_count "$bin_name")"
	if [[ -z "$design_n" ]]; then
		echo "FAIL ${bin_name} test count: no CLI table count found in ${design}" >&2
		failures=$((failures + 1))
	else
		assert_numbers_match "${bin_name} test count @ ${design}" "$actual_tests" "$design_n"
	fi
done

# --- 6. E2E 個別 spec 実行例の件数照合 -------------------------------------
# TEST_SPECIFICATION の `npm test -- <spec>.spec.ts # ...（N件...）` 実行例から
# 記載件数 N を取り出し、e2e/tests/<spec>.spec.ts の `test(` 定義数と照合する。
# 実行例はあるが件数表記がない spec（search / favorites / export 等）は対象外。
# 件数表記があるのに抽出できない場合は fail-closed で FAIL とする。
spec_example_lines="$(grep -E 'npm test -- [A-Za-z0-9_-]+\.spec\.ts[^#]*#' "$spec" || true)"
if [[ -z "$spec_example_lines" ]]; then
	echo "FAIL e2e spec example count: no 'npm test -- <spec>.spec.ts # ...' line found in ${spec}" >&2
	failures=$((failures + 1))
fi
while IFS= read -r example_line; do
	[[ -z "$example_line" ]] && continue
	spec_name="$(grep -oE 'npm test -- [A-Za-z0-9_-]+\.spec\.ts' <<<"$example_line" | head -n1 | awk '{print $NF}' || true)"
	spec_name="${spec_name%.spec.ts}"
	[[ -n "$spec_name" ]] || continue
	comment="${example_line#*#}"
	# 件数表記がなければ対象外（実行例はあるが件数を書いていない spec）。
	if ! grep -qE '[0-9]+件' <<<"$comment"; then
		continue
	fi
	# 総数を抽出する。複数内訳が並ぶ形（「...3件 + ...3件の計6件」）では
	# 「の計N件」を総数として優先し、それ以外は「（N件」を採る。
	documented_n="$(grep -oE 'の計[0-9]+件' <<<"$comment" | grep -oE '[0-9]+' | head -n1 || true)"
	if [[ -z "$documented_n" ]]; then
		documented_n="$(grep -oE '（[0-9]+件' <<<"$comment" | grep -oE '[0-9]+' | head -n1 || true)"
	fi
	if [[ -z "$documented_n" ]]; then
		echo "FAIL e2e spec example count @ ${spec_name}: count annotation found but no number extracted from '${comment}'" >&2
		failures=$((failures + 1))
		continue
	fi
	spec_file="e2e/tests/${spec_name}.spec.ts"
	if [[ ! -f "$spec_file" ]]; then
		echo "FAIL e2e spec example count @ ${spec_name}: no such file ${spec_file}" >&2
		failures=$((failures + 1))
		continue
	fi
	actual_n="$(grep -c '^  test(' "$spec_file" || true)"
	assert_numbers_match "e2e spec example ${spec_name}" "$actual_n" "$documented_n"
done <<<"$spec_example_lines"

# --- 7. CLI-01 群内訳テーブルの件数合計照合 ---------------------------------
# TEST_SPECIFICATION の CLI-01 群テーブル（| 群 | 件数 | テスト名 |）から件数列を抽出し
# 合計して、doctor.rs の実 #[test] 数と照合する。CLI-02/03 の同名テーブルは対象外
# （CLI-01 見出しから次の CLI 見出しまでに限定）。抽出失敗は fail-closed で FAIL。
doctor_group_sum="$(
	awk -F'|' '
		/^#### CLI-01:/ { in_cli = 1 }
		in_cli && /^#### CLI-0[23]:/ { exit }
		in_cli && /^\| *群 *\|/ { in_table = 1; next }
		in_table && /^\|/ {
			cell = $3
			gsub(/^[[:space:]]+|[[:space:]]+$/, "", cell)
			if (cell ~ /^[0-9]+$/) { sum += cell; n++ }
			next
		}
		in_table && n > 0 { exit }
		END { if (n > 0) print sum }
	' "$spec" || true
)"
if [[ ! "$doctor_group_sum" =~ ^[0-9]+$ ]]; then
	echo "FAIL doctor group breakdown: no valid CLI-01 '| 群 | 件数 |' table in ${spec}" >&2
	failures=$((failures + 1))
else
	doctor_test_total="$(grep -cE '^[[:space:]]*#\[(tokio::)?test\]' server/src/bin/doctor.rs || true)"
	assert_numbers_match "doctor group breakdown sum" "$doctor_test_total" "$doctor_group_sum"
fi

# 群テーブルのテスト名列（バッククォート `name` トークン）と doctor.rs の
# #[test] / #[tokio::test] 付き fn 名を双方向に照合する（CLI bin 用の独立ゲート）。
# セクション4の `fn test_` 走査・除外ロジックは変更せず、ここでは doctor.rs のみを対象にする。
# 重複・欠落・typo・属性の無い同名 fn は fail-closed で FAIL。
cli01_group_name_cells="$(
	awk -F'|' '
		/^#### CLI-01:/ { in_cli = 1 }
		in_cli && /^#### CLI-0[23]:/ { exit }
		in_cli && /^\| *群 *\|/ { in_table = 1; next }
		in_table && /^\|/ {
			cell = $3
			gsub(/^[[:space:]]+|[[:space:]]+$/, "", cell)
			if (cell ~ /^[0-9]+$/) { print $4; printed++ }
			next
		}
		in_table && printed > 0 { exit }
	' "$spec" || true
)"

# 抽出した全名（重複を含む）を配列へ保持し、一意名との比較で重複を検出する。
mapfile -t cli01_all_names < <(
	printf '%s\n' "$cli01_group_name_cells" |
		# バッククォートは 16 進エスケープ (\x60) で表現し shellcheck のトークン解釈を避ける。
		grep -oE $'\x60[A-Za-z0-9_]+\x60' |
		tr -d '`' || true
)

declare -A cli01_doc_test_names=()
for name in "${cli01_all_names[@]}"; do
	[[ -z "$name" ]] && continue
	cli01_doc_test_names["$name"]=1
done

# 同名が 2 回以上載ると一意名が減るため、抽出件数と一意件数を比較して FAIL とする。
if ((${#cli01_all_names[@]} != ${#cli01_doc_test_names[@]})); then
	echo "FAIL doctor group test names: duplicates (extracted ${#cli01_all_names[@]}, unique ${#cli01_doc_test_names[@]})" >&2
	failures=$((failures + 1))
fi

# 一意名の数は群「件数」列の合計（doctor_group_sum）と一致しなければならない（欠落検出）。
if [[ "$doctor_group_sum" =~ ^[0-9]+$ ]] && ((${#cli01_doc_test_names[@]} != doctor_group_sum)); then
	echo "FAIL doctor group test names: ${#cli01_doc_test_names[@]} unique vs sum ${doctor_group_sum} in ${spec}" >&2
	failures=$((failures + 1))
fi

# 実コード側は #[test] / #[tokio::test] が付いた fn 名のみを集合にする。
# 属性行・空行・コメントを飛ばし、直後の fn を awk で拾う（ヘルパー fn は除外）。
declare -A doctor_test_fn_names=()
while IFS= read -r name; do
	[[ -z "$name" ]] && continue
	doctor_test_fn_names["$name"]=1
done < <(
	awk '
		/^[[:space:]]*#\[(tokio::)?test\]/ { pending = 1; next }
		pending && /^[[:space:]]*$/ { next }
		pending && /^[[:space:]]*#\[/ { next }
		pending && /^[[:space:]]*\/\// { next }
		pending && /^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]/ {
			line = $0
			sub(/^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+/, "", line)
			sub(/[^A-Za-z0-9_].*$/, "", line)
			if (line != "") { print line; pending = 0 }
			next
		}
		{ pending = 0 }
	' server/src/bin/doctor.rs | sort -u || true
)

if ((${#cli01_doc_test_names[@]} == 0)); then
	echo "FAIL doctor group test names: no backtick test name found in CLI-01 '| 群 | 件数 | テスト名 |' table in ${spec}" >&2
	failures=$((failures + 1))
fi
if ((${#doctor_test_fn_names[@]} == 0)); then
	echo "FAIL doctor #[test] names: no #[test]/#[tokio::test] fn found in server/src/bin/doctor.rs" >&2
	failures=$((failures + 1))
fi

# 双方向の集合差を検証する（ドキュメント側のみ・実コード側のみの名前は共に FAIL）。
if ((${#cli01_doc_test_names[@]} > 0 && ${#doctor_test_fn_names[@]} > 0)); then
	for name in "${!cli01_doc_test_names[@]}"; do
		if [[ -z "${doctor_test_fn_names[$name]:-}" ]]; then
			echo "FAIL doctor group test name: '${name}' in CLI-01 table but no #[test] fn in doctor.rs" >&2
			failures=$((failures + 1))
		fi
	done
	for name in "${!doctor_test_fn_names[@]}"; do
		if [[ -z "${cli01_doc_test_names[$name]:-}" ]]; then
			echo "FAIL doctor #[test] name: '${name}' in doctor.rs but not in CLI-01 table in ${spec}" >&2
			failures=$((failures + 1))
		fi
	done
fi

# --- 結果 ----------------------------------------------------------------
if ((failures > 0)); then
	echo "doc numbers FAILED (${failures})" >&2
	exit 1
fi
echo "doc numbers OK: e2e=${e2e_total} doctor=${doctor_checks} migrations=${migration_count} tests=${#doc_test_names[@]}"
exit 0
