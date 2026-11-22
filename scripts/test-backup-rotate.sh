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

# verify_audit_log スタブが成功時に出力する計算 root（非機微な hex）。
# backup-rotate.sh の .audit-root 世代保存オラクルの期待値としても使う。
stub_root="aaaabbbbccccddddeeeeffff00001111aaaabbbbccccddddeeeeffff00001111"
export STUB_AUDIT_ROOT="$stub_root"

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
# verify_audit_log は呼び出し引数をログへ残す（自動統合のオラクル用）。
# ローカル DB のみを検証するためトークンは渡さない。
cat >"${work}/bin/verify_audit_log" <<'EOF'
#!/usr/bin/env bash
if [[ -n "${AUDIT_VERIFY_LOG:-}" ]]; then
  printf '%s\n' "$*" >> "$AUDIT_VERIFY_LOG"
fi
# 実 CLI の成功出力と同じ形式で計算 root を出す（root 値はスクリプト側で捕捉される）。
echo "audit_chain_digest: count=1 first=Some(1) last=Some(1) root=${STUB_AUDIT_ROOT}"
echo "audit_log_verification: OK"
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
chmod +x "${work}/bin/backup_db" "${work}/bin/verify_backup" "${work}/bin/verify_audit_log" "${work}/bin/gpg"

export BIN_DIR="${work}/bin"
export GPG_BIN="${work}/bin/gpg"
export BACKUP_DIR="${work}/backups"
export DATABASE_URL="file:${work}/unused.db"
export JWT_SECRET="test_secret_for_backup_rotate_script_123"
export ALLOW_MOCK_AUTH=1
export AUDIT_VERIFY_LOG="${work}/audit-verify.log"

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

# === verify_audit_log の自動統合 ===
# (a) 正常系: 生成直後の平文 tmp_db に対して verify_audit_log が呼ばれること。
#     公開先ではなく mktemp の作業ディレクトリ配下の .db であることを引数で確認する。
#     テスト実行環境の AUDIT_EXPECT_ROOT に汚されないよう env -u で明示的に外す。
: >"$AUDIT_VERIFY_LOG"
audit_ok_dir="${work}/audit-verify-ok"
new_backup_dir "$audit_ok_dir"
expect_success "verify_audit_log invoked on success" env -u AUDIT_EXPECT_ROOT BACKUP_DIR="$audit_ok_dir" BACKUP_GPG_RECIPIENT=test "$script"
if grep -Fq -- "--database-url file:${audit_ok_dir}" "$AUDIT_VERIFY_LOG" &&
	grep -Eq -- '--database-url file:.*/\.openillustration-[^ ]+/openillustration\.db$' "$AUDIT_VERIFY_LOG"; then
	echo "PASS verify_audit_log invoked on plaintext tmp_db before publish"
else
	echo "FAIL verify_audit_log not invoked on plaintext tmp_db"
	cat "$AUDIT_VERIFY_LOG" >&2
	failures=$((failures + 1))
fi

# AUDIT_EXPECT_ROOT の受け渡し（受け入れ条件 a/b）。
# Merkle root は非機微な hex のため、既存の引数ログオラクルへ残してよい。
# (a') 設定時: verify_audit_log スタブへ --expect-root <値> が渡ること。
expect_root="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
: >"$AUDIT_VERIFY_LOG"
expect_root_dir="${work}/audit-expect-root"
new_backup_dir "$expect_root_dir"
expect_success "AUDIT_EXPECT_ROOT forwarded to verify_audit_log" env BACKUP_DIR="$expect_root_dir" BACKUP_GPG_RECIPIENT=test AUDIT_EXPECT_ROOT="$expect_root" "$script"
if grep -Fq -- "--expect-root ${expect_root}" "$AUDIT_VERIFY_LOG"; then
	echo "PASS AUDIT_EXPECT_ROOT forwarded as --expect-root"
else
	echo "FAIL AUDIT_EXPECT_ROOT not forwarded as --expect-root"
	cat "$AUDIT_VERIFY_LOG" >&2
	failures=$((failures + 1))
fi

# (a'') 成功時にverify_audit_logの計算rootが、成果物と同じstampの
#       .audit-root として0600で公開され、root値がstdout/stderrへ出ないこと。
#       expect_* の呼び出し件数は増やさず、既存ケース内へオラクルのみ追加する。
mapfile -d '' audit_root_files < <(find "$expect_root_dir" -maxdepth 1 -type f -name 'openillustration-*.audit-root' -print0)
# このケースは暗号化を有効にしているため最終成果物は .db.gpg（平文 .db は削除済み）。
mapfile -d '' audit_db_files < <(find "$expect_root_dir" -maxdepth 1 -type f \
	\( -name 'openillustration-*.db' -o -name 'openillustration-*.db.gpg' \) -print0)
if ((${#audit_root_files[@]} == 1)) && ((${#audit_db_files[@]} == 1)); then
	root_name="${audit_root_files[0]##*/}"
	db_name="${audit_db_files[0]##*/}"
	root_stamp="${root_name#openillustration-}"
	root_stamp="${root_stamp%.audit-root}"
	db_stamp="${db_name#openillustration-}"
	db_stamp="${db_stamp%.db.gpg}"
	db_stamp="${db_stamp%.db}"
	if [[ "$root_stamp" == "$db_stamp" ]]; then
		echo "PASS .audit-root published with the same stamp as the artifact"
	else
		echo "FAIL .audit-root stamp differs from artifact stamp"
		failures=$((failures + 1))
	fi
	if [[ "$(stat -c %a "${audit_root_files[0]}")" == "600" ]]; then
		echo "PASS .audit-root mode is 600"
	else
		echo "FAIL .audit-root mode is not 600"
		failures=$((failures + 1))
	fi
	if [[ "$(cat "${audit_root_files[0]}")" == "$stub_root" ]]; then
		echo "PASS .audit-root contains the freshly computed root"
	else
		echo "FAIL .audit-root content mismatch"
		failures=$((failures + 1))
	fi
	if grep -Fq -- "$stub_root" "${work}/out.log"; then
		echo "FAIL computed root leaked into script stdout/stderr"
		failures=$((failures + 1))
	else
		echo "PASS computed root not echoed to stdout/stderr"
	fi
else
	echo "FAIL .audit-root / artifact count mismatch"
	failures=$((failures + 1))
fi

# (b') 未設定時: 既存運用を壊さず --expect-root を渡さないこと。
# 実行環境側で AUDIT_EXPECT_ROOT が設定されていても env -u で明示的に外す。
: >"$AUDIT_VERIFY_LOG"
no_expect_dir="${work}/audit-no-expect-root"
new_backup_dir "$no_expect_dir"
expect_success "no --expect-root when AUDIT_EXPECT_ROOT unset" env -u AUDIT_EXPECT_ROOT BACKUP_DIR="$no_expect_dir" BACKUP_GPG_RECIPIENT=test "$script"
if grep -Eq -- '--expect-root' "$AUDIT_VERIFY_LOG"; then
	echo "FAIL --expect-root passed while AUDIT_EXPECT_ROOT unset"
	cat "$AUDIT_VERIFY_LOG" >&2
	failures=$((failures + 1))
else
	echo "PASS no --expect-root when AUDIT_EXPECT_ROOT unset"
fi

# (d') AUDIT_EXPECT_ROOT に不一致値（有効な hex だが記録済み root と異なる）を設定した
#     とき、その値が ALERT ログ／スクリプト stderr へエコーされないこと。
#     実 verify_audit_log の不一致エラーは固定文言（root 値なし）なので、
#     同じ文言を出すスタブで fail-closed にし、backup-rotate.sh 側の非エコーを検証する。
#     既存の引数ログオラクル（値が --expect-root として渡ること）も同時に維持する。
mismatch_root="fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
mismatch_bin="${work}/mismatch-audit-bin"
mkdir -p "$mismatch_bin"
cp "${work}/bin/backup_db" "${mismatch_bin}/backup_db"
cp "${work}/bin/verify_backup" "${mismatch_bin}/verify_backup"
cat >"${mismatch_bin}/verify_audit_log" <<'EOF'
#!/usr/bin/env bash
if [[ -n "${AUDIT_VERIFY_LOG:-}" ]]; then
  printf 'MISMATCHED_ROOT %s\n' "$*" >> "$AUDIT_VERIFY_LOG"
fi
echo "ERROR: audit root mismatch (expected and computed roots withheld)" >&2
exit 1
EOF
chmod +x "${mismatch_bin}/verify_audit_log"
mismatch_dir="${work}/audit-root-mismatch"
new_backup_dir "$mismatch_dir"
: >"${ALERT_LOG}"
: >"${work}/mismatch-audit.log"
expect_failure "mismatched AUDIT_EXPECT_ROOT is fail-closed" env BIN_DIR="$mismatch_bin" BACKUP_DIR="$mismatch_dir" BACKUP_GPG_RECIPIENT=test ALERT_CMD="${work}/alert.sh" AUDIT_VERIFY_LOG="${work}/mismatch-audit.log" AUDIT_EXPECT_ROOT="$mismatch_root" "$script"
nonleak_bad=0
if grep -Fq -- "$mismatch_root" "${ALERT_LOG}"; then
	echo "FAIL mismatched root leaked into ALERT log"
	cat "${ALERT_LOG}" >&2
	nonleak_bad=1
fi
if grep -Fq -- "$mismatch_root" "${work}/out.log"; then
	echo "FAIL mismatched root leaked into script stderr/stdout"
	cat "${work}/out.log" >&2
	nonleak_bad=1
fi
if grep -Fq -- "--expect-root ${mismatch_root}" "${work}/mismatch-audit.log"; then
	echo "PASS AUDIT_EXPECT_ROOT mismatch value still forwarded (arg-log oracle intact)"
else
	echo "FAIL AUDIT_EXPECT_ROOT mismatch value not forwarded"
	cat "${work}/mismatch-audit.log" >&2
	nonleak_bad=1
fi
if grep -Fq 'openIllustration backup failed at verify_audit_log' "${ALERT_LOG}"; then
	echo "PASS alert invoked at verify_audit_log on root mismatch"
else
	echo "FAIL alert missing failing stage on root mismatch"
	cat "${ALERT_LOG}" >&2
	nonleak_bad=1
fi
if ((nonleak_bad == 0)); then
	echo "PASS mismatched AUDIT_EXPECT_ROOT value not echoed to ALERT/stderr"
else
	failures=$((failures + 1))
fi

# (b) verify_audit_log 失敗: Runbook の「両方成功しなければ認めない」に従い、
#     公開前に失敗して成果物を出さず、一時平文を清掃し、ALERT_CMD が通知する。
mkdir -p "${work}/fail-audit-bin"
cp "${work}/bin/backup_db" "${work}/fail-audit-bin/backup_db"
cp "${work}/bin/verify_backup" "${work}/fail-audit-bin/verify_backup"
cat >"${work}/fail-audit-bin/verify_audit_log" <<'EOF'
#!/usr/bin/env bash
if [[ -n "${AUDIT_VERIFY_LOG:-}" ]]; then
  printf 'FAILED_AUDIT %s\n' "$*" >> "$AUDIT_VERIFY_LOG"
fi
echo "simulated audit verify failure" >&2
exit 10
EOF
chmod +x "${work}/fail-audit-bin/verify_audit_log"
cleanup_dir="${work}/cleanup-audit"
new_backup_dir "$cleanup_dir"
: >"${ALERT_LOG}"
expect_failure "verify_audit_log failure is fail-closed" env BACKUP_DIR="$cleanup_dir" BIN_DIR="${work}/fail-audit-bin" BACKUP_GPG_RECIPIENT=test ALERT_CMD="${work}/alert.sh" AUDIT_VERIFY_LOG="${work}/audit-fail.log" "$script"
cleanup_no_leftovers "verify_audit_log failure" "$cleanup_dir"
if grep -q '^FAILED_AUDIT ' "${work}/audit-fail.log" 2>/dev/null; then
	echo "PASS verify_audit_log failure was the cause (marker recorded)"
else
	echo "FAIL verify_audit_log failure marker missing"
	failures=$((failures + 1))
fi
if compgen -G "${cleanup_dir}/openillustration-*.db" >/dev/null ||
	compgen -G "${cleanup_dir}/openillustration-*.db.gpg" >/dev/null; then
	echo "FAIL verify_audit_log failure: backup artifact was published"
	failures=$((failures + 1))
else
	echo "PASS verify_audit_log failure: no backup artifact published"
fi
if [[ -s "${ALERT_LOG}" ]]; then
	echo "PASS alert invoked on verify_audit_log failure"
else
	echo "FAIL alert not invoked on verify_audit_log failure"
	failures=$((failures + 1))
fi
# (c') 通知本文に失敗段階名が含まれること。接頭辞 `openIllustration backup failed` は維持。
# 直前に ALERT_LOG をクリアしているため、この1行だけを検証すればよい。
if grep -Fq 'openIllustration backup failed at verify_audit_log' "${ALERT_LOG}"; then
	echo "PASS alert message includes failing stage (verify_audit_log)"
else
	echo "FAIL alert message does not include failing stage"
	cat "${ALERT_LOG}" >&2
	failures=$((failures + 1))
fi

assert_plaintext_retained() {
	local name="$1" dir="$2"
	local -a plains=()
	mapfile -d '' plains < <(find "$dir" -maxdepth 1 -type f -name 'openillustration-*.db' -print0)
	if ((${#plains[@]} == 1)) && [[ "$(cat "${plains[0]}")" == "dummy backup" ]]; then
		echo "PASS ${name}: plaintext retained and restorable"
	else
		echo "FAIL ${name}: plaintext lost on ciphertext publish failure"
		failures=$((failures + 1))
	fi
}

# 暗号文が公開先へ atomic rename 済みであることを検証する。
assert_ciphertext_published() {
	local name="$1" dir="$2"
	local -a ciphers=()
	mapfile -d '' ciphers < <(find "$dir" -maxdepth 1 -type f -name 'openillustration-*.db.gpg' -print0)
	if ((${#ciphers[@]} == 1)) && [[ "$(cat "${ciphers[0]}")" == "dummy encrypted" ]]; then
		echo "PASS ${name}: ciphertext published"
	else
		echo "FAIL ${name}: ciphertext not published before plaintext removal"
		failures=$((failures + 1))
	fi
}

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
assert_plaintext_retained "gpg failure" "$cleanup_dir"

# 暗号文公開失敗時に平文が残り復元可能であること（完全喪失の回帰防止）。
# gpg 暗号化は成功させた上で、公開直前の assert_publishable / mv を失敗させる。

# (a) assert_publishable 失敗: gpg スタブが公開先にダングリング symlink を置く。
cat >"${work}/publish-assert-fail-gpg" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
src="${!#}"
while (($#)); do
  if [[ "$1" == --output ]]; then
    printf 'dummy encrypted' > "$2"
    break
  fi
  shift
done
# 平文最終パス + .gpg にダングリング symlink を置き、公開直前の
# assert_publishable を失敗させる（gpg 暗号化自体は成功）。
ln -s "${src}.missing" "${src}.gpg"
EOF
chmod +x "${work}/publish-assert-fail-gpg"
publish_dir="${work}/publish-assert-fail"
new_backup_dir "$publish_dir"
expect_failure "assert_publishable failure on ciphertext publish" env BACKUP_DIR="$publish_dir" GPG_BIN="${work}/publish-assert-fail-gpg" BACKUP_GPG_RECIPIENT=test "$script"
assert_plaintext_retained "assert_publishable failure" "$publish_dir"

# (b) mv 失敗: .gpg を含む rename だけを失敗させ、それ以外の mv は委譲する。
mkdir -p "${work}/mv-fail-bin"
cat >"${work}/mv-fail-bin/mv" <<'EOF'
#!/usr/bin/env bash
for arg in "$@"; do
  if [[ "$arg" == *.gpg ]]; then
    echo "simulated mv failure" >&2
    exit 12
  fi
done
exec /bin/mv "$@"
EOF
chmod +x "${work}/mv-fail-bin/mv"
publish_dir="${work}/publish-mv-fail"
new_backup_dir "$publish_dir"
expect_failure "mv failure on ciphertext publish" env PATH="${work}/mv-fail-bin:${PATH}" BACKUP_DIR="$publish_dir" BACKUP_GPG_RECIPIENT=test "$script"
assert_plaintext_retained "mv failure" "$publish_dir"

# (c) rm 失敗: 暗号文公開後の平文削除（rm -f "$output"）だけを失敗させる。
# backup-rotate.sh は先に暗号文を公開してから平文を削除するため、rm が
# 失敗しても暗号文は公開済み・平文は残存し、alert_cmd が呼ばれる必要がある。
# Trap の後片付け（rm -rf -- "$tmp_dir" / rm -f -- "$rotation_list"）は
# .db でないため委譲され、残留物は残らない。
mkdir -p "${work}/rm-fail-bin"
cat >"${work}/rm-fail-bin/rm" <<'EOF'
#!/usr/bin/env bash
for arg in "$@"; do
  if [[ "$arg" == *.db ]]; then
    echo "simulated rm failure" >&2
    exit 13
  fi
done
exec /bin/rm "$@"
EOF
chmod +x "${work}/rm-fail-bin/rm"
: >"${ALERT_LOG}" # 直前のテストの通知をクリアし、今回の呼び出しだけを検証する。
publish_dir="${work}/publish-rm-fail"
new_backup_dir "$publish_dir"
expect_failure "rm failure on plaintext removal" env PATH="${work}/rm-fail-bin:${PATH}" BACKUP_DIR="$publish_dir" BACKUP_GPG_RECIPIENT=test ALERT_CMD="${work}/alert.sh" "$script"
assert_ciphertext_published "rm failure" "$publish_dir"
assert_plaintext_retained "rm failure" "$publish_dir"
if [[ -s "${ALERT_LOG}" ]]; then
	echo "PASS alert invoked on rm failure"
else
	echo "FAIL alert not invoked on rm failure"
	failures=$((failures + 1))
fi
cleanup_no_leftovers "rm failure" "$publish_dir"

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

echo "=== ケース8: 実 gpg による暗号化→復号→バイト一致 ==="
# gpg 未インストール／使い捨て鍵の生成失敗時はローカルでは SKIP（PASS とは区別する
# 専用出力）として扱う。CI（CI 環境変数が設定されている時）は gpg の導入を前提とし、
# 前提条件を満たせない場合は黙ってスキップせず FAIL として失敗させる（黙スキップ防止）。
# 判定は command -v gpg と鍵生成コマンドの終了コードのみに基づく。
# expect_failure / expect_success の件数は増やさない（専用出力は独立したオラクル）。
case8_skip() {
	local reason="$1"
	if [[ -n "${CI:-}" ]]; then
		echo "FAIL case 8: required precondition missing in CI (${reason})" >&2
		failures=$((failures + 1))
	else
		echo "SKIP case 8: ${reason}"
	fi
}
real_gpg="$(command -v gpg || true)"
if [[ -z "$real_gpg" ]]; then
	case8_skip "gpg not found via command -v"
else
	# 一時 GNUPGHOME は work 配下に作り、既存の EXIT trap で削除する（リポジトリへ鍵を置かない）。
	gnupg_home="$(mktemp -d "${work}/gnupg.XXXXXX")"
	chmod 700 "$gnupg_home"
	cat >"${gnupg_home}/gpg.conf" <<'EOF'
trust-model always
no-tty
EOF
	cat >"${work}/case8-key.params" <<'EOF'
%echo generating disposable backup-rotate test key
Key-Type: RSA
Key-Length: 2048
Key-Usage: sign
Subkey-Type: RSA
Subkey-Length: 2048
Subkey-Usage: encrypt
Name-Real: Backup Rotate Test
Name-Email: backup-rotate-test@example.invalid
Expire-Date: 0
%no-protection
%commit
%echo done
EOF
	if ! GNUPGHOME="$gnupg_home" "$real_gpg" --batch --generate-key \
		"${work}/case8-key.params" >"${work}/case8-keygen.log" 2>&1; then
		case8_skip "disposable GPG key generation failed"
	else
		recipient="$(
			GNUPGHOME="$gnupg_home" "$real_gpg" --batch --list-keys --with-colons \
				backup-rotate-test@example.invalid 2>/dev/null |
				awk -F: '$1 == "fpr" { print $10; exit }' || true
		)"
		if [[ -z "$recipient" ]]; then
			case8_skip "generated key fingerprint unavailable"
		else
			gpg_backup_dir="${work}/gpg-roundtrip-backups"
			new_backup_dir "$gpg_backup_dir"
			# GPG_BIN はスタブではなく実 gpg（command -v で解決）。GNUPGHOME を env で渡す。
			expect_success "case 8: real gpg encrypted backup" env \
				GNUPGHOME="$gnupg_home" \
				GPG_BIN="$real_gpg" \
				BACKUP_GPG_RECIPIENT="$recipient" \
				BACKUP_DIR="$gpg_backup_dir" \
				"$script"
			# 公開された .db.gpg を同じ一時 GNUPGHOME で復号し、元の平文とバイト一致を検証する。
			mapfile -d '' case8_ciphers < <(find "$gpg_backup_dir" -maxdepth 1 -type f \
				-name 'openillustration-*.db.gpg' -print0)
			if ((${#case8_ciphers[@]} != 1)); then
				echo "FAIL case 8: expected exactly one published .db.gpg artifact"
				failures=$((failures + 1))
			elif ! GNUPGHOME="$gnupg_home" "$real_gpg" --batch --decrypt \
				"${case8_ciphers[0]}" >"${work}/case8-plaintext.bin" 2>"${work}/case8-decrypt.log"; then
				echo "FAIL case 8: gpg --decrypt failed"
				cat "${work}/case8-decrypt.log" >&2
				failures=$((failures + 1))
			else
				printf 'dummy backup' >"${work}/case8-expected.bin"
				if cmp -s "${work}/case8-expected.bin" "${work}/case8-plaintext.bin"; then
					echo "PASS case 8: decrypt matches original plaintext byte-for-byte"
				else
					echo "FAIL case 8: decrypted plaintext differs from original"
					failures=$((failures + 1))
				fi
			fi
		fi
	fi
fi

echo "=== ケース9: バックアップ窓 unit の停止→backup→起動（独立オラクル） ==="
# 既存の expect_failure / expect_success 件数は増やさない。unit 定義を直接読み取り、
# oneshot の実行順（ExecStartPre → ExecStart → ExecStopPost）を stub systemctl で再生する。
# ExecStopPost は ExecStart 成否にかかわらず必ず実行される systemd 契約を前提とし、
# バックアップ失敗時もアプリ unit 起動が呼ばれることを検証する。
window_unit="${repo_root}/deploy/openillustration-backup-window.service"
if [[ ! -f "$window_unit" ]]; then
	echo "FAIL case 9: backup window unit missing"
	failures=$((failures + 1))
else
	# 指定キーの先頭 1 行の値を返す（eval 不使用・パイプの SIGPIPE も避ける）。
	read_unit_kv() {
		local key="$1=" line
		while IFS= read -r line || [[ -n "$line" ]]; do
			if [[ "$line" == "$key"* ]]; then
				printf '%s' "${line#"$key"}"
				return 0
			fi
		done <"$window_unit"
		return 1
	}
	app_unit_value="$(read_unit_kv Environment=APP_UNIT || true)"
	pre_cmd="$(read_unit_kv ExecStartPre || true)"
	start_cmd="$(read_unit_kv ExecStart || true)"
	post_cmd="$(read_unit_kv ExecStopPost || true)"
	if [[ -z "$app_unit_value" || -z "$pre_cmd" || -z "$start_cmd" || -z "$post_cmd" ]]; then
		echo "FAIL case 9: APP_UNIT / Exec* not configured in window unit"
		failures=$((failures + 1))
	else
		stub_bin="${work}/case9-bin"
		backup_abs="/opt/openIllustration/scripts/backup-rotate.sh"
		systemctl_log="${work}/case9-systemctl.log"
		mkdir -p "$stub_bin"
		case9_backup_dir="${work}/case9-backups"
		new_backup_dir "$case9_backup_dir"
		export BACKUP_DIR="$case9_backup_dir"
		export SYSTEMCTL_LOG="$systemctl_log"
		export BACKUP_SCRIPT="$script"

		cat >"${stub_bin}/systemctl" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "systemctl $*" >> "$SYSTEMCTL_LOG"
EOF
		# バックアップ段をログに残して本体へ委譲する。CASE9_FAIL_BACKUP=1 のときは
		# 暗号化設定を外して本体を失敗させ、起動が依然呼ばれることを検証する。
		cat >"${stub_bin}/backup-step" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "backup" >> "$SYSTEMCTL_LOG"
if [[ "${CASE9_FAIL_BACKUP:-0}" == "1" ]]; then
  exec env -u BACKUP_GPG_RECIPIENT -u ALLOW_UNENCRYPTED_BACKUP "$BACKUP_SCRIPT" "$@"
fi
exec env BACKUP_GPG_RECIPIENT=test "$BACKUP_SCRIPT" "$@"
EOF
		chmod +x "${stub_bin}/systemctl" "${stub_bin}/backup-step"

		run_unit_cmd() {
			# shellcheck disable=SC2016
			local cmd="$1" token='${APP_UNIT}'
			local -a argv=()
			cmd="${cmd//"$token"/$app_unit_value}"
			# root 特権接頭辞 '+' を外し、systemctl / backup-rotate.sh をスタブへ差し替える。
			cmd="${cmd#+}"
			if [[ "$cmd" =~ ^/usr/bin/systemctl(.*)$ ]]; then
				cmd="${stub_bin}/systemctl${BASH_REMATCH[1]}"
			fi
			if [[ "$cmd" =~ ^"${backup_abs}"(.*)$ ]]; then
				cmd="${stub_bin}/backup-step${BASH_REMATCH[1]}"
			fi
			read -r -a argv <<<"$cmd"
			"${argv[@]}"
		}

		replay_window() {
			local label="$1" pre_rc=0 start_rc=0 post_rc=0
			local expected actual
			: >"$SYSTEMCTL_LOG"
			run_unit_cmd "$pre_cmd" || pre_rc=$?
			# ExecStartPre 失敗時は systemd が ExecStart をスキップする。
			if ((pre_rc == 0)); then
				run_unit_cmd "$start_cmd" || start_rc=$?
			fi
			# ExecStopPost は ExecStartPre / ExecStart の成否にかかわらず実行される。
			run_unit_cmd "$post_cmd" || post_rc=$?
			expected="$(printf 'systemctl stop %s\nbackup\nsystemctl start %s' \
				"$app_unit_value" "$app_unit_value")"
			actual="$(cat "$SYSTEMCTL_LOG")"
			if [[ "$actual" == "$expected" ]]; then
				echo "PASS case 9 (${label}): stop -> backup -> start order"
			else
				echo "FAIL case 9 (${label}): unexpected order"
				printf 'expected:\n%s\nactual:\n%s\n' "$expected" "$actual" >&2
				failures=$((failures + 1))
			fi
			if ((post_rc != 0)); then
				echo "FAIL case 9 (${label}): start command failed"
				failures=$((failures + 1))
			fi
			# 失敗シナリオでは backup 段が実際に失敗していることを確認する。
			if [[ "$label" == "backup failure" ]] && ((start_rc == 0)); then
				echo "FAIL case 9: backup failure injection did not fail"
				failures=$((failures + 1))
			fi
		}

		CASE9_FAIL_BACKUP=0
		export CASE9_FAIL_BACKUP
		replay_window "backup success"
		sleep 1 # タイムスタンプ付きファイル名の衝突を避ける
		CASE9_FAIL_BACKUP=1
		replay_window "backup failure"
	fi
fi

if ((failures > 0)); then
	echo "backup-rotate tests FAILED (${failures})" >&2
	exit 1
fi
echo "backup-rotate tests OK"
