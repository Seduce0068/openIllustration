#!/usr/bin/env bash
# バックアップ取得→検証→古い世代のローテーションを一度に行う。
# cron 等のスケジューラから実行する想定（サーバーは起動中でも可: VACUUM INTO を使用）。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

backup_dir="${BACKUP_DIR:-${repo_root}/backups}"
prefix="${BACKUP_PREFIX:-openillustration}"
retention_days="${RETENTION_DAYS:-30}"
min_keep="${MIN_KEEP:-3}"
bin_dir="${BIN_DIR:-${repo_root}/target/debug}"

run_bin() {
	local name="$1"
	shift
	if [[ -x "${bin_dir}/${name}" ]]; then
		"${bin_dir}/${name}" "$@"
	else
		cargo run --quiet -p server --bin "$name" -- "$@"
	fi
}

if ! [[ "$retention_days" =~ ^[0-9]+$ && "$min_keep" =~ ^[0-9]+$ ]]; then
	echo "RETENTION_DAYS and MIN_KEEP must be non-negative integers" >&2
	exit 1
fi
if ((min_keep < 1)); then
	echo "MIN_KEEP must be at least 1" >&2
	exit 1
fi

mkdir -p "$backup_dir"
chmod 700 "$backup_dir" || echo "warning: failed to restrict permissions on ${backup_dir}" >&2

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
output="${backup_dir}/${prefix}-${stamp}.db"
if [[ -e "$output" ]]; then
	echo "backup already exists: ${output}" >&2
	exit 1
fi

echo "==> backup: ${output}"
run_bin backup_db --output "$output"
run_bin verify_backup --input "$output"

# 新しい順（ファイル名の UTC タイムスタンプ順）に並べ、直近 min_keep 件は
# 保持期間を超えていても残す（誤設定で全削除されるのを防ぐ）。
cutoff=$(($(date -u +%s) - retention_days * 86400))
kept=0
pruned=0
while IFS= read -r -d '' file; do
	kept=$((kept + 1))
	if ((kept <= min_keep)); then
		continue
	fi
	mtime="$(stat -c %Y "$file")"
	if ((mtime < cutoff)); then
		rm -f "$file"
		pruned=$((pruned + 1))
		echo "pruned: ${file}"
	fi
done < <(find "$backup_dir" -maxdepth 1 -type f -name "${prefix}-*.db" -print0 | sort -zr)

echo "==> complete: kept=${kept} pruned=${pruned} retention_days=${retention_days}"
