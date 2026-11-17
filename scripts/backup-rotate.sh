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
gpg_recipient="${BACKUP_GPG_RECIPIENT:-}"
gpg_bin="${GPG_BIN:-gpg}"
# 平文バックアップは開発専用。明示的なオプトアウトが無い限り拒否する
# （オフサイト保存が設定されている場合はオプトアウトも認めない）。
allow_plaintext="${ALLOW_UNENCRYPTED_BACKUP:-0}"
# オフサイト保存: 実行ファイルのパスと宛先を指定する（eval は使わない）。
offsitecopy="${OFFSITE_COPY_CMD:-}"
offsitedest="${OFFSITE_DEST:-}"
# 失敗通知: 実行ファイルのパス。呼び出しは <メッセージ> の1引数（eval 不使用）。
alert_cmd="${ALERT_CMD:-}"

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
if [[ -n "$gpg_recipient" ]] && ! command -v "$gpg_bin" >/dev/null 2>&1; then
	echo "BACKUP_GPG_RECIPIENT is set but ${gpg_bin} is not available" >&2
	exit 1
fi
if [[ -z "$gpg_recipient" ]]; then
	if [[ -n "$offsitecopy" || -n "$offsitedest" ]]; then
		echo "Refusing to copy an unencrypted backup offsite; set BACKUP_GPG_RECIPIENT" >&2
		exit 1
	fi
	if [[ "$allow_plaintext" != "1" ]]; then
		echo "BACKUP_GPG_RECIPIENT is not set; set ALLOW_UNENCRYPTED_BACKUP=1 for local development only" >&2
		exit 1
	fi
	echo "WARNING: producing an unencrypted backup (ALLOW_UNENCRYPTED_BACKUP=1)" >&2
fi
if [[ -n "$offsitecopy" && -z "$offsitedest" ]] || [[ -z "$offsitecopy" && -n "$offsitedest" ]]; then
	echo "OFFSITE_COPY_CMD and OFFSITE_DEST must be set together" >&2
	exit 1
fi
if [[ -n "$offsitecopy" && ! -x "$offsitecopy" ]]; then
	echo "OFFSITE_COPY_CMD is not executable: ${offsitecopy}" >&2
	exit 1
fi
if [[ -n "$alert_cmd" && ! -x "$alert_cmd" ]]; then
	echo "ALERT_CMD is not executable: ${alert_cmd}" >&2
	exit 1
fi

# 失敗時に通知する。通知自体が失敗しても元の終了コードを維持する。
on_failure() {
	local code=$?
	if ((code == 0)); then
		return 0
	fi
	if [[ -n "$alert_cmd" ]]; then
		"$alert_cmd" "openIllustration backup failed (exit ${code}) on $(hostname)" >/dev/null 2>&1 || true
	fi
	return "$code"
}
trap on_failure EXIT

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

if [[ -n "$gpg_recipient" ]]; then
	echo "==> encrypting backup for ${gpg_recipient}"
	"$gpg_bin" --batch --yes --encrypt --recipient "$gpg_recipient" \
		--output "${output}.gpg" "$output"
	# 暗号化が成功した場合のみ平文を削除（失敗時は平文を残して復旧可能にする）。
	rm -f "$output"
	echo "==> plaintext removed: ${output}"
fi

if [[ -n "$offsitecopy" ]]; then
	artifact="$output"
	if [[ -n "$gpg_recipient" ]]; then
		artifact="${output}.gpg"
	fi
	echo "==> offsite copy: ${artifact} -> ${offsitedest}"
	"$offsitecopy" "$artifact" "$offsitedest"
fi

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
done < <(find "$backup_dir" -maxdepth 1 -type f \
	\( -name "${prefix}-*.db" -o -name "${prefix}-*.db.gpg" \) -print0 | sort -zr)

echo "==> complete: kept=${kept} pruned=${pruned} retention_days=${retention_days}"
