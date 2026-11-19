#!/usr/bin/env bash
# trunk アーカイブの checksum ゲート（ci-e2e.sh / build-gate.spec.ts 共通）。
#
# 環境変数:
#   TRUNK_ARCHIVE     検証対象のアーカイブパス（必須）
#   TRUNK_SHA256      期待する sha256（64 桁の小文字 hex、必須）
#   TRUNK_VERSION     エラー表示用の版（任意、既定 unknown）
#   TRUNK_EXTRACT_DIR 検証成功時に展開する先（任意、未指定なら検証のみ）
#
# キャッシュの有無にかかわらず checksum を検証する。不一致なら壊れた
# アーカイブを削除して exit 1 し、展開には進まない（fail-open の防止）。
# 展開をゲートと同じスクリプトに置くことで、検証をすり抜けたバイト列が
# PATH に載る経路を構造的に無くしている。
set -euo pipefail

archive="${TRUNK_ARCHIVE:-}"
expected="${TRUNK_SHA256:-}"
version="${TRUNK_VERSION:-unknown}"
extract_dir="${TRUNK_EXTRACT_DIR:-}"

if [[ -z "$archive" ]]; then
	echo "TRUNK_ARCHIVE is not set." >&2
	exit 1
fi
if [[ ! "$expected" =~ ^[0-9a-f]{64}$ ]]; then
	echo "TRUNK_SHA256 must be a 64-digit lowercase hex value." >&2
	exit 1
fi
if [[ ! -f "$archive" ]]; then
	echo "trunk archive is missing: ${archive}" >&2
	exit 1
fi

# キャッシュヒット時も必ず検証を通す。不一致なら壊れたアーカイブを削除して失敗。
if ! echo "${expected}  ${archive}" | sha256sum -c -; then
	rm -f "$archive"
	echo "trunk ${version} の sha256 が一致しない。" >&2
	exit 1
fi

# 検証が成功した場合にのみ展開する（TRUNK_EXTRACT_DIR 指定時）。
if [[ -n "$extract_dir" ]]; then
	mkdir -p "$extract_dir"
	tar -xzf "$archive" -C "$extract_dir" trunk
fi
