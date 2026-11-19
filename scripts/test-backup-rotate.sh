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
# RETENTION_DAYS の上限境界（20000 = 約55年）。同桁は辞書順、桁超過は長さで拒否される。
expect_failure "retention above max (20001)" env BACKUP_GPG_RECIPIENT=test RETENTION_DAYS=20001 "$script"
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
# 上限値そのもの（20000）は受理されること（上限比較の off-by-one 回帰防止）。
expect_success "max retention (20000)" env BACKUP_GPG_RECIPIENT=test RETENTION_DAYS=20000 "$script"
sleep 1 # タイムスタンプ付きファイル名の衝突を避ける
# 上限値でも cutoff = now - 20000*86400 が正のままであることを実証する。
# mtime を 1970 年代（@1）に設定した古い世代を置き、実際に prune されること
# （cutoff 負値で mtime < cutoff が常に false になる fail-open の回帰防止）。
prune_dir="${work}/prune-max-retention"
mkdir -p "$prune_dir"
chmod 700 "$prune_dir"
old_artifact="${prune_dir}/openillustration-19900101T000000Z.db.gpg"
printf 'old backup' >"$old_artifact"
touch -d '@1' "$old_artifact"
expect_success "prune old generation at max retention (20000)" env BACKUP_DIR="$prune_dir" BACKUP_GPG_RECIPIENT=test RETENTION_DAYS=20000 MIN_KEEP=1 "$script"
if [[ -e "$old_artifact" ]]; then
	echo "FAIL old generation not pruned at max retention (cutoff may be non-positive)"
	failures=$((failures + 1))
else
	echo "PASS old generation pruned at max retention"
fi
# MIN_KEEP の上限値そのもの（1000）も受理されること（同値除外の回帰防止）。
expect_success "max min_keep (1000)" env BACKUP_GPG_RECIPIENT=test MIN_KEEP=1000 "$script"

# MIN_KEEP の中核契約: 保持期間を超えていても直近 MIN_KEEP 件は残す。
# 古い mtime(@1) の世代を MIN_KEEP+3 件用意し、ファイル名が新しい順の上位
# MIN_KEEP 件だけが残り、残りが prune されることをファイル存在で検証する。
# 上限値 prune テスト（MIN_KEEP=1 の単一案）では MIN_KEEP 判定を削除しても
# 通ってしまうため、このオラクルで契約を直接担保する。
min_keep_dir="${work}/min-keep-protection"
mkdir -p "$min_keep_dir"
chmod 700 "$min_keep_dir"
mk_expect_kept=()
mk_expect_pruned=()
for day in 1 2 3 4 5 6; do
	# 未来のファイル名にして、スクリプトが新規作成する世代より新しく並べる。
	f="${min_keep_dir}/openillustration-2099010${day}T000000Z.db.gpg"
	printf 'old backup' >"$f"
	touch -d '@1' "$f"
	if ((day > 3)); then
		mk_expect_kept+=("$f") # 新しい順 1〜3 件目（MIN_KEEP で保持）
	else
		mk_expect_pruned+=("$f") # 古い 3 件（保持期間超過で prune）
	fi
done
expect_success "MIN_KEEP protects newest generations" env BACKUP_DIR="$min_keep_dir" BACKUP_GPG_RECIPIENT=test RETENTION_DAYS=30 MIN_KEEP=3 "$script"
mk_bad=0
for f in "${mk_expect_kept[@]}"; do
	if [[ ! -e "$f" ]]; then
		echo "FAIL expected kept generation missing: $f"
		mk_bad=1
	fi
done
for f in "${mk_expect_pruned[@]}"; do
	if [[ -e "$f" ]]; then
		echo "FAIL expected pruned generation still present: $f"
		mk_bad=1
	fi
done
if ((mk_bad == 0)); then
	echo "PASS MIN_KEEP kept newest 3 and pruned older 3 generations"
else
	failures=$((failures + 1))
fi

# 完了ログの kept は走査カウンタではなく実際の保持件数 (N-M) であること。
# 既存テストは "==> complete:" の存在しか見ておらず kept= の値を検証していない。
# 古い世代 3 件 + スクリプトの新規 1 件 = 走査 4、MIN_KEEP=2 により最古 1 件を
# prune するため、kept は 3 (=4-1) でなければならない（バグ露呈時は 4 になる）。
kept_dir="${work}/kept-log"
mkdir -p "$kept_dir"
chmod 700 "$kept_dir"
for day in 1 2 3; do
	f="${kept_dir}/openillustration-2099010${day}T000000Z.db.gpg"
	printf 'old backup' >"$f"
	touch -d '@1' "$f"
done
expect_success "kept log reflects actual kept count" env BACKUP_DIR="$kept_dir" BACKUP_GPG_RECIPIENT=test RETENTION_DAYS=30 MIN_KEEP=2 "$script"
if grep -q '==> complete: kept=3 pruned=1 retention_days=30' "${work}/out.log"; then
	echo "PASS kept log reports kept=3 (= scanned 4 - pruned 1)"
else
	echo "FAIL kept log does not report actual kept count"
	cat "${work}/out.log" >&2
	failures=$((failures + 1))
fi

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
