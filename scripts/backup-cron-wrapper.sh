#!/usr/bin/env bash
# cron から呼び出すラッパー。600 権限の環境ファイルを読み込んで本体を実行する。
# systemd を使う場合は不要（service の EnvironmentFile を利用する）。
# 環境ファイルは source せず、KEY=VALUE だけを解釈する制限付きパーサで読む。
set -euo pipefail

env_file="${BACKUP_ENV_FILE:-/etc/openillustration/backup.env}"

# -f はシンボリックリンクの参照先も通過するため、-L で明示的に拒否する。
if [[ ! -f "$env_file" || -L "$env_file" ]]; then
	echo "Backup env file must be a regular non-symlink file: ${env_file}" >&2
	exit 1
fi

# 所有者は現在の UID または root に限定する。
if ! meta="$(stat -c '%u %a' -- "$env_file" 2>/dev/null)"; then
	echo "Backup env file is not accessible: ${env_file}" >&2
	exit 1
fi
read -r owner mode <<<"$meta"
if [[ "$owner" != "$EUID" && "$owner" != "0" ]]; then
	echo "Backup env file has an unexpected owner (uid=${owner}): ${env_file}" >&2
	exit 1
fi

# 通常は 600 / 400 のみ許可する。root 実行時は mode 000 も reader として通るが、
# 通常ファイル・所有者の検証は必須（stat %a は 000 を "0" と出力する）。
case "$mode" in
600 | 400) ;;
0 | 000)
	if ((EUID != 0)); then
		echo "Backup env file must be mode 600 or 400: ${env_file}" >&2
		exit 1
	fi
	;;
*)
	echo "Backup env file must be mode 600 or 400: ${env_file}" >&2
	exit 1
	;;
esac

if [[ ! -r "$env_file" ]]; then
	echo "Backup env file is not readable: ${env_file}" >&2
	exit 1
fi

# 検証した内容をそのまま読むため、ファイルを FD で開いてから解析する。
exec {env_fd}<"$env_file"
# KEY=VALUE のみを解釈する制限付きパーサ。source / eval は使わず、クォートや
# コマンド置換、シェル記号はすべてリテラルな値として扱う。
# 行番号は空行・コメント行も含めて採番する。エラーには番号のみを出し、
# 行内容（キー名・値）は cron のメール等へ漏らさない。
line_number=0
while IFS= read -r line <&"$env_fd" || [[ -n "$line" ]]; do
	line_number=$((line_number + 1))
	# 空行・コメント行は無視する。
	[[ -z "$line" || "$line" == \#* ]] && continue
	if [[ "$line" != *=* ]]; then
		echo "Invalid line #${line_number} in backup env file (expected KEY=VALUE)" >&2
		exit 1
	fi
	key="${line%%=*}"
	value="${line#*=}"
	if [[ ! "$key" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
		echo "Invalid key #${line_number} in backup env file" >&2
		exit 1
	fi
	export "$key=$value"
done
exec {env_fd}<&-

exec /opt/openIllustration/scripts/backup-rotate.sh
