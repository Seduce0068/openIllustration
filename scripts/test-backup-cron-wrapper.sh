#!/usr/bin/env bash
# backup-cron-wrapper.sh の回帰テスト。本番パスのハードコードを一時コピーへ
# 差し替え、外部サービスなしで fail-closed と値の伝播を検証する。
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

# exec 先のダミー。source 経由で渡った環境変数と exec の引数を記録する。
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

echo "=== ケース2: source した値が exec 先へ伝播する ==="
rm -f "$env_file"
printf 'BACKUP_GPG_RECIPIENT=test@example.com\n' >"$env_file"
chmod 600 "$env_file"
rm -f "${WRAPPER_CAPTURE}"
expect_success "case2 propagation" env -u BACKUP_ENV_FILE "$wrapper"
assert_capture_line "case2" "INVOKED=1"
assert_capture_line "case2" "RECIPIENT=test@example.com"
assert_capture_line "case2" "ARGC=0"
assert_capture_line "case2" "ARGS="

# ケース3: 環境ファイルは source されるため、$(...) や ; といったシェル記法も
# そのまま評価される。したがって 600 権限の信頼できるファイルだけを置くことが
# 運用上の前提であり、追加のサニタイズは実装しない（テストでも期待しない）。
# ここでは害のないコマンド置換で「評価されること」だけを示す。
echo "=== ケース3: 信頼できる 600 権限ファイル前提（サニタイズなし）==="
cat >"$env_file" <<'EOF'
BACKUP_GPG_RECIPIENT=test@example.com
INJECTION_MARKER="$(printf injected-by-source)"
EOF
chmod 600 "$env_file"
rm -f "${WRAPPER_CAPTURE}"
expect_success "case3 source evaluates shell" env -u BACKUP_ENV_FILE "$wrapper"
assert_capture_line "case3" "MARKER=injected-by-source"
assert_capture_line "case3" "RECIPIENT=test@example.com"

if ((failures > 0)); then
	echo "backup-cron-wrapper tests FAILED (${failures})" >&2
	exit 1
fi
echo "backup-cron-wrapper tests OK"
