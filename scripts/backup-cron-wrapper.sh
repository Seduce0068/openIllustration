#!/usr/bin/env bash
# cron から呼び出すラッパー。600 権限の環境ファイルを読み込んで本体を実行する。
# systemd を使う場合は不要（service の EnvironmentFile を利用する）。
# 環境ファイルのパスは実行時に決まるため SC1090 は無効化する。
# shellcheck disable=SC1090
set -euo pipefail

env_file="${BACKUP_ENV_FILE:-/etc/openillustration/backup.env}"
if [[ ! -r "$env_file" ]]; then
	echo "Backup env file is not readable: ${env_file}" >&2
	exit 1
fi

# 環境ファイルは KEY=VALUE 形式のみを想定し、eval は使わない。
set -a
source "$env_file"
set +a

exec /opt/openIllustration/scripts/backup-rotate.sh
