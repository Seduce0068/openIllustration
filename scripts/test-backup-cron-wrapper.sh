#!/usr/bin/env bash
# backup-cron-wrapper.sh の回帰テスト。本番パスのハードコードを一時コピーへ
# 差し替え、外部サービスなしで fail-closed・値伝播・env ファイル検証を確認する。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="${repo_root}/scripts/backup-cron-wrapper.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "${work}/bin"

# 本番パス（/etc/... と /opt/...）はスクリプトにハードコードされているため、
# 本体は変更せず、mktemp ディレクトリへ書き換えた隔離コピーを使って検証する。
sed \
	-e "s#/etc/openillustration/backup.env#${work}/backup.env#g" \
	-e "s#/opt/openIllustration/scripts/backup-rotate.sh#${work}/bin/backup-rotate.sh#g" \
	"$script" >"${work}/wrapper.sh"
chmod +x "${work}/wrapper.sh"

# exec 先のダミー。env ファイルから渡った環境変数と exec の引数を記録する。
cat >"${work}/bin/backup-rotate.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
{
	printf 'INVOKED=1\n'
	printf 'RECIPIENT=%s\n' "${BACKUP_GPG_RECIPIENT:-<unset>}"
	printf 'MARKER=%s\n' "${INJECTION_MARKER:-<unset>}"
	printf 'ARGC=%s\n' "$#"
	printf 'ARGS=%s\n' "$*"
} >>"${WRAPPER_CAPTURE}"
EOF
chmod +x "${work}/bin/backup-rotate.sh"

export WRAPPER_CAPTURE="${work}/capture.log"
env_file="${work}/backup.env"
wrapper="${work}/wrapper.sh"

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

assert_capture_absent() {
	local name="$1"
	if [[ -e "${WRAPPER_CAPTURE}" ]]; then
		echo "FAIL ${name}: exec was called"
		failures=$((failures + 1))
	else
		echo "PASS ${name}: exec not called"
	fi
}

assert_capture_line() {
	local name="$1" expected="$2"
	if grep -Fxq "$expected" "${WRAPPER_CAPTURE}"; then
		echo "PASS ${name}: ${expected}"
	else
		echo "FAIL ${name}: missing '${expected}'"
		failures=$((failures + 1))
	fi
}

assert_file_absent() {
	local name="$1" path="$2"
	if [[ -e "$path" ]]; then
		echo "FAIL ${name}: unexpected side effect at ${path}"
		failures=$((failures + 1))
	else
		echo "PASS ${name}: no side effect"
	fi
}

echo "=== ケース1: 環境ファイルが存在しない/読めない場合は fail-closed ==="
rm -f "$env_file" "${WRAPPER_CAPTURE}"
expect_failure "case1 missing env file" env -u BACKUP_ENV_FILE "$wrapper"
assert_capture_absent "case1 missing env file"
if ((EUID == 0)); then
	echo "SKIP case1 unreadable env file (root ignores mode 000)"
else
	printf 'BACKUP_GPG_RECIPIENT=should-not-propagate\n' >"$env_file"
	chmod 000 "$env_file"
	expect_failure "case1 unreadable env file" env -u BACKUP_ENV_FILE "$wrapper"
	assert_capture_absent "case1 unreadable env file"
fi

echo "=== ケース2: パースした値が exec 先へ伝播する ==="
rm -f "$env_file"
printf 'BACKUP_GPG_RECIPIENT=test@example.com\n' >"$env_file"
chmod 600 "$env_file"
rm -f "${WRAPPER_CAPTURE}"
expect_success "case2 propagation" env -u BACKUP_ENV_FILE "$wrapper"
assert_capture_line "case2" "INVOKED=1"
assert_capture_line "case2" "RECIPIENT=test@example.com"
assert_capture_line "case2" "ARGC=0"
assert_capture_line "case2" "ARGS="

echo "=== ケース3: 制限付きパーサではシェル記法が評価されない ==="
# $(...) が評価されれば ${work}/pwned が作られる。リテラル値として保持される
# こと（＝コマンド置換も副作用も発生しないこと）を検証する。
{
	printf 'BACKUP_GPG_RECIPIENT=test@example.com\n'
	# shellcheck disable=SC2016 # コマンド置換として評価されないことを検証するためリテラル。
	printf 'INJECTION_MARKER="$(printf injected > %s/pwned)"\n' "$work"
} >"$env_file"
chmod 600 "$env_file"
rm -f "${WRAPPER_CAPTURE}" "${work}/pwned"
expect_success "case3 restricted parser" env -u BACKUP_ENV_FILE "$wrapper"
# shellcheck disable=SC2016 # 期待値もリテラル文字列として比較する。
printf -v expected_marker 'MARKER="$(printf injected > %s/pwned)"' "$work"
assert_capture_line "case3 literal value" "$expected_marker"
assert_capture_line "case3" "RECIPIENT=test@example.com"
assert_file_absent "case3 command substitution not evaluated" "${work}/pwned"

echo "=== ケース4: シンボリックリンクの環境ファイルは拒否 ==="
ln -sfn "$env_file" "${work}/backup.env.link"
rm -f "${WRAPPER_CAPTURE}"
expect_failure "case4 symlink env file" env BACKUP_ENV_FILE="${work}/backup.env.link" "$wrapper"
assert_capture_absent "case4 symlink env file"

echo "=== ケース5: 600/400 以外の権限は拒否 ==="
chmod 644 "$env_file"
rm -f "${WRAPPER_CAPTURE}"
expect_failure "case5 insecure mode" env -u BACKUP_ENV_FILE "$wrapper"
assert_capture_absent "case5 insecure mode"
chmod 600 "$env_file"

if ((failures > 0)); then
	echo "backup-cron-wrapper tests FAILED (${failures})" >&2
	exit 1
fi
echo "backup-cron-wrapper tests OK"
