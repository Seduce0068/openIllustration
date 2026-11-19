#!/usr/bin/env bash
# backup-rotate.sh の設定分岐を、外部サービスなしで検証する。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="${repo_root}/scripts/backup-rotate.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "${work}/bin" "${work}/backups" "${work}/offsite"
# backup-rotate.sh は既存のバックアップ先が 0700 でない場合に拒否するため、
# テスト用ディレクトリも本番と同じ権限に揃える（umask 022 環境での誤失敗を防ぐ）。
chmod 700 "${work}/backups"

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
expect_failure "octal retention (08)" env BACKUP_GPG_RECIPIENT=test RETENTION_DAYS=08 "$script"
# RETENTION_DAYS の上限境界（36500 = 100年）。同桁は辞書順、桁超過は長さで拒否される。
expect_failure "retention above max (36501)" env BACKUP_GPG_RECIPIENT=test RETENTION_DAYS=36501 "$script"
expect_failure "retention far above max (100000)" env BACKUP_GPG_RECIPIENT=test RETENTION_DAYS=100000 "$script"
expect_failure "octal min_keep (08)" env BACKUP_GPG_RECIPIENT=test MIN_KEEP=08 "$script"
expect_failure "zero min_keep" env BACKUP_GPG_RECIPIENT=test MIN_KEEP=0 "$script"
# MIN_KEEP の上限境界（1000）。同桁は辞書順、桁超過は長さで拒否される。
# 2^64+1 は算術演算で 1 にラップするため、オーバーフロー検証としても扱う。
expect_failure "min_keep above max (1001)" env BACKUP_GPG_RECIPIENT=test MIN_KEEP=1001 "$script"
expect_failure "min_keep far above max (100000)" env BACKUP_GPG_RECIPIENT=test MIN_KEEP=100000 "$script"
expect_failure "min_keep overflow wrap (2^64+1)" env BACKUP_GPG_RECIPIENT=test MIN_KEEP=18446744073709551617 "$script"
expect_failure "alert without dest" env BACKUP_GPG_RECIPIENT=test OFFSITE_COPY_CMD=/bin/true "$script"
expect_failure "missing gpg" env BACKUP_GPG_RECIPIENT=test GPG_BIN=/no/such/gpg "$script"
expect_failure "missing alert cmd" env BACKUP_GPG_RECIPIENT=test ALERT_CMD=/no/such/cmd "$script"

echo "=== バックアップ先ディレクトリの権限 fail-closed ==="
loose_dir="${work}/backups-loose"
mkdir -p "$loose_dir"
chmod 755 "$loose_dir"
expect_failure "loose (0755) backup dir rejected" env BACKUP_GPG_RECIPIENT=test BACKUP_DIR="$loose_dir" "$script"
if compgen -G "${loose_dir}/openillustration-*" >/dev/null; then
	echo "FAIL artifact written to loosely-permissioned directory"
	failures=$((failures + 1))
else
	echo "PASS no artifact written to loosely-permissioned directory"
fi

echo "=== 正常系 ==="
expect_success "plain backup (opt-out)" env -u BACKUP_GPG_RECIPIENT ALLOW_UNENCRYPTED_BACKUP=1 "$script"
sleep 1 # タイムスタンプ付きファイル名の衝突を避ける
expect_success "encrypted backup" env BACKUP_GPG_RECIPIENT=test "$script"
sleep 1 # タイムスタンプ付きファイル名の衝突を避ける
# デフォルト値と同じ 30/3 を明示指定しても受け付けられること（先頭ゼロ除去の回帰防止）。
expect_success "explicit retention/min_keep (30/3)" env BACKUP_GPG_RECIPIENT=test RETENTION_DAYS=30 MIN_KEEP=3 "$script"
sleep 1 # タイムスタンプ付きファイル名の衝突を避ける
# 上限値そのもの（36500）は受理されること（上限比較の off-by-one 回帰防止）。
expect_success "max retention (36500)" env BACKUP_GPG_RECIPIENT=test RETENTION_DAYS=36500 "$script"
sleep 1 # タイムスタンプ付きファイル名の衝突を避ける
# MIN_KEEP の上限値そのもの（1000）も受理されること（同値除外の回帰防止）。
expect_success "max min_keep (1000)" env BACKUP_GPG_RECIPIENT=test MIN_KEEP=1000 "$script"

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
sleep 1 # タイムスタンプ付きファイル名の衝突を避ける
expect_failure "offsite failure propagates" env BACKUP_GPG_RECIPIENT=test OFFSITE_COPY_CMD="${work}/fail.sh" OFFSITE_DEST="${work}/offsite" "$script"

# 失敗通知が呼ばれること。
cat >"${work}/alert.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$1" >> "$ALERT_LOG"
EOF
chmod +x "${work}/alert.sh"
export ALERT_LOG="${work}/alerts.log"
sleep 1 # タイムスタンプ付きファイル名の衝突を避ける
expect_failure "failing run with alert" env BACKUP_GPG_RECIPIENT=test ALERT_CMD="${work}/alert.sh" OFFSITE_COPY_CMD="${work}/fail.sh" OFFSITE_DEST="${work}/offsite" "$script"
if [[ -s "${ALERT_LOG}" ]]; then
	echo "PASS alert invoked"
else
	echo "FAIL alert not invoked"
	failures=$((failures + 1))
fi

echo "=== find 失敗時の fail-closed ==="
# backup-rotate.sh は BACKUP_DIR を mkdir -p してから列挙するため、
# 存在しないディレクトリでは find まで到達しない。FIND_BIN に失敗する
# 実行ファイルを渡し、列挙失敗が process substitution で飲み込まれないことを検証する。
cat >"${work}/failing-find" <<'EOF'
#!/usr/bin/env bash
echo "simulated find failure" >&2
exit 42
EOF
chmod +x "${work}/failing-find"
: >"${ALERT_LOG}"
sleep 1 # タイムスタンプ付きファイル名の衝突を避ける
expect_failure "find failure propagates" env BACKUP_GPG_RECIPIENT=test FIND_BIN="${work}/failing-find" ALERT_CMD="${work}/alert.sh" "$script"
if [[ -s "${ALERT_LOG}" ]]; then
	echo "PASS alert invoked on find failure"
else
	echo "FAIL alert not invoked on find failure"
	failures=$((failures + 1))
fi
if grep -q "==> complete:" "${work}/out.log"; then
	echo "FAIL complete reported despite find failure"
	failures=$((failures + 1))
else
	echo "PASS no complete on find failure"
fi
if compgen -G "${work}/backups/.openillustration-rotate.*" >/dev/null; then
	echo "FAIL leftover rotation temp file"
	failures=$((failures + 1))
else
	echo "PASS rotation temp file cleaned up"
fi

echo "=== 失敗時の一時ディレクトリ・平文 DB の後片付け ==="
# 失敗注入ごとに専用のバックアップ先を用意し、残留物を確実に検出する。
new_backup_dir() {
	local dir="$1"
	mkdir -p "$dir"
	chmod 700 "$dir"
}
# 作業ディレクトリ（.openillustration-*）と平文 tmp_db（openillustration.db）が残っていないことを検証する。
cleanup_no_leftovers() {
	local name="$1" dir="$2"
	if compgen -G "${dir}/.openillustration-*" >/dev/null; then
		echo "FAIL ${name}: leftover temp dir/file"
		failures=$((failures + 1))
	elif [[ -n "$(find "$dir" -maxdepth 2 -type f -name 'openillustration.db' -print -quit)" ]]; then
		echo "FAIL ${name}: leftover plaintext temp DB"
		failures=$((failures + 1))
	else
		echo "PASS ${name}: no temp dir / plaintext temp DB left"
	fi
}

# verify_backup 失敗: 平文 tmp_db が作業ディレクトリに残っていた問題の回帰テスト。
mkdir -p "${work}/fail-verify-bin"
cp "${work}/bin/backup_db" "${work}/fail-verify-bin/backup_db"
cat >"${work}/fail-verify-bin/verify_backup" <<'EOF'
#!/usr/bin/env bash
echo "simulated verify failure" >&2
exit 9
EOF
chmod +x "${work}/fail-verify-bin/verify_backup"
cleanup_dir="${work}/cleanup-verify"
new_backup_dir "$cleanup_dir"
expect_failure "cleanup on verify_backup failure" env BACKUP_DIR="$cleanup_dir" BIN_DIR="${work}/fail-verify-bin" BACKUP_GPG_RECIPIENT=test "$script"
cleanup_no_leftovers "verify_backup failure" "$cleanup_dir"

# gpg 失敗: 暗号化失敗時も作業ディレクトリは残さない（最終パスの平文は fail-closed で維持）。
cat >"${work}/fail-gpg" <<'EOF'
#!/usr/bin/env bash
echo "simulated gpg failure" >&2
exit 8
EOF
chmod +x "${work}/fail-gpg"
cleanup_dir="${work}/cleanup-gpg"
new_backup_dir "$cleanup_dir"
expect_failure "cleanup on gpg failure" env BACKUP_DIR="$cleanup_dir" GPG_BIN="${work}/fail-gpg" BACKUP_GPG_RECIPIENT=test "$script"
cleanup_no_leftovers "gpg failure" "$cleanup_dir"

# オフサイト失敗。
cleanup_dir="${work}/cleanup-offsite"
new_backup_dir "$cleanup_dir"
expect_failure "cleanup on offsite failure" env BACKUP_DIR="$cleanup_dir" BACKUP_GPG_RECIPIENT=test OFFSITE_COPY_CMD="${work}/fail.sh" OFFSITE_DEST="${work}/offsite" "$script"
cleanup_no_leftovers "offsite failure" "$cleanup_dir"

# find 失敗。
cleanup_dir="${work}/cleanup-find"
new_backup_dir "$cleanup_dir"
expect_failure "cleanup on find failure" env BACKUP_DIR="$cleanup_dir" BACKUP_GPG_RECIPIENT=test FIND_BIN="${work}/failing-find" "$script"
cleanup_no_leftovers "find failure" "$cleanup_dir"

echo "=== 一時ファイルの権限と後片付け ==="
mapfile -d '' artifacts < <(find "${work}/backups" -maxdepth 1 -type f \
	-name 'openillustration-*.db.gpg' -print0)
if ((${#artifacts[@]} == 0)); then
	echo "FAIL no encrypted artifact to check"
	failures=$((failures + 1))
else
	bad_mode=0
	for artifact in "${artifacts[@]}"; do
		[[ "$(stat -c %a "$artifact")" == "600" ]] || bad_mode=1
	done
	if ((bad_mode == 0)); then
		echo "PASS artifacts are mode 600 (umask 077)"
	else
		echo "FAIL artifact mode is not 600"
		failures=$((failures + 1))
	fi
fi
mapfile -d '' temps < <(find "${work}/backups" -maxdepth 1 -type d \
	-name '.openillustration-*' -print0)
if ((${#temps[@]} == 0)); then
	echo "PASS no leftover temp directories"
else
	echo "FAIL leftover temp directories: ${temps[*]}"
	failures=$((failures + 1))
fi

echo "=== 公開先のシンボリックリンクは拒否 ==="
# スクリプト内の UTC タイムスタンプと一致するよう、前後 5 秒の名前を置く。
now="$(date -u +%s)"
for offset in {-5..5}; do
	stamp="$(date -u -d "@$((now + offset))" +%Y%m%dT%H%M%SZ)"
	ln -sfn "${work}/nowhere" "${work}/backups/linktest-${stamp}.db"
done
expect_failure "symlink target refused" env BACKUP_GPG_RECIPIENT=test BACKUP_PREFIX=linktest "$script"
if compgen -G "${work}/backups/linktest-*.db.gpg" >/dev/null; then
	echo "FAIL artifact published despite symlink target"
	failures=$((failures + 1))
else
	echo "PASS no artifact published at symlink target"
fi
rm -f "${work}/backups/linktest-"*.db

if ((failures > 0)); then
	echo "backup-rotate tests FAILED (${failures})" >&2
	exit 1
fi
echo "backup-rotate tests OK"
