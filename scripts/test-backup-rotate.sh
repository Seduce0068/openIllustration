#!/usr/bin/env bash
# backup-rotate.sh の設定分岐を、外部サービスなしで検証する。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="${repo_root}/scripts/backup-rotate.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "${work}/bin" "${work}/backups" "${work}/offsite"

# 実際の DB 操作の代わりに成果物を作るダミーコマンド。
cat >"${work}/bin/backup_db" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
for arg in "$@"; do
  if [[ "$arg" == --output ]]; then
    shift
    printf 'dummy backup' > "$1"
    exit 0
  fi
  shift
done
echo "missing --output" >&2
exit 1
EOF
cat >"${work}/bin/verify_backup" <<'EOF'
#!/usr/bin/env bash
echo "dummy verify"
EOF
# 実際の gpg の代わりにダミー成果物を作る（外部サービス不要）。
cat >"${work}/bin/gpg" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
for arg in "$@"; do
  if [[ "$arg" == --output ]]; then
    shift
    printf 'dummy encrypted' > "$1"
    exit 0
  fi
  shift
done
echo "missing --output" >&2
exit 1
EOF
chmod +x "${work}/bin/backup_db" "${work}/bin/verify_backup" "${work}/bin/gpg"

export BIN_DIR="${work}/bin"
export GPG_BIN="${work}/bin/gpg"
export BACKUP_DIR="${work}/backups"
export DATABASE_URL="file:${work}/unused.db"
export JWT_SECRET="test_secret_for_backup_rotate_script_123"
export ALLOW_MOCK_AUTH=1

failures=0
expect_failure() {
	local name="$1"
	shift
	if "$@" >"${work}/out.log" 2>&1; then
		echo "FAIL ${name}: expected failure but succeeded"
		failures=$((failures + 1))
	else
		echo "PASS ${name}"
	fi
}
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

echo "=== 設定不整合の fail-closed ==="
expect_failure "no recipient" env -u BACKUP_GPG_RECIPIENT -u ALLOW_UNENCRYPTED_BACKUP "$script"
expect_failure "offsite plaintext refused" env -u BACKUP_GPG_RECIPIENT OFFSITE_COPY_CMD=/bin/true OFFSITE_DEST=/tmp "$script"
expect_failure "invalid retention" env BACKUP_GPG_RECIPIENT=test RETENTION_DAYS=abc "$script"
expect_failure "zero min_keep" env BACKUP_GPG_RECIPIENT=test MIN_KEEP=0 "$script"
expect_failure "alert without dest" env BACKUP_GPG_RECIPIENT=test OFFSITE_COPY_CMD=/bin/true "$script"
expect_failure "missing gpg" env BACKUP_GPG_RECIPIENT=test GPG_BIN=/no/such/gpg "$script"
expect_failure "missing alert cmd" env BACKUP_GPG_RECIPIENT=test ALERT_CMD=/no/such/cmd "$script"

echo "=== 正常系 ==="
expect_success "plain backup (opt-out)" env -u BACKUP_GPG_RECIPIENT ALLOW_UNENCRYPTED_BACKUP=1 "$script"
sleep 1 # タイムスタンプ付きファイル名の衝突を避ける
expect_success "encrypted backup" env BACKUP_GPG_RECIPIENT=test "$script"

# オフサイト保存の成功と失敗。
cat >"${work}/copy.sh" <<'EOF'
#!/usr/bin/env bash
cp "$1" "$2/"
EOF
chmod +x "${work}/copy.sh"
sleep 1 # タイムスタンプ付きファイル名の衝突を避ける
expect_success "offsite copy" env BACKUP_GPG_RECIPIENT=test OFFSITE_COPY_CMD="${work}/copy.sh" OFFSITE_DEST="${work}/offsite" "$script"
if [[ ! -e "${work}/offsite" ]] || [[ -z "$(ls -A "${work}/offsite")" ]]; then
	echo "FAIL offsite artifact missing"
	failures=$((failures + 1))
else
	echo "PASS offsite artifact"
fi

cat >"${work}/fail.sh" <<'EOF'
#!/usr/bin/env bash
exit 7
EOF
chmod +x "${work}/fail.sh"
expect_failure "offsite failure propagates" env BACKUP_GPG_RECIPIENT=test OFFSITE_COPY_CMD="${work}/fail.sh" OFFSITE_DEST="${work}/offsite" "$script"

# 失敗通知が呼ばれること。
cat >"${work}/alert.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$1" >> "$ALERT_LOG"
EOF
chmod +x "${work}/alert.sh"
export ALERT_LOG="${work}/alerts.log"
expect_failure "failing run with alert" env BACKUP_GPG_RECIPIENT=test ALERT_CMD="${work}/alert.sh" OFFSITE_COPY_CMD="${work}/fail.sh" OFFSITE_DEST="${work}/offsite" "$script"
if [[ -s "${ALERT_LOG}" ]]; then
	echo "PASS alert invoked"
else
	echo "FAIL alert not invoked"
	failures=$((failures + 1))
fi

if ((failures > 0)); then
	echo "backup-rotate tests FAILED (${failures})" >&2
	exit 1
fi
echo "backup-rotate tests OK"
