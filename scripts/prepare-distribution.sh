#!/usr/bin/env bash
# 配布物へ同梱するライセンス関連ファイルを生成する。
# 新規依存を追加せず、cargo deny / cargo metadata の出力のみを利用する。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

out_dir="${1:-dist-license}"
if [[ -z "$out_dir" ]]; then
	echo "output directory must not be empty" >&2
	exit 1
fi

for tool in jq cargo; do
	if ! command -v "$tool" >/dev/null 2>&1; then
		echo "required tool not found: $tool" >&2
		exit 1
	fi
done

mkdir -p "$out_dir"

echo "==> ライセンス本文を配置"
cp LICENSE "${out_dir}/LICENSE"

echo "==> 依存ライセンス一覧を生成"
cargo deny list --format json |
	jq -r '
      .licenses[]
      | .[0] as $license
      | .[1][]
      | [$license, .] | @tsv
    ' >"${out_dir}/DEPENDENCIES.tsv"

echo "==> NOTICE を生成"
{
	echo "openIllustration / Digital Guild"
	echo "Copyright (c) 2026 koppe"
	echo ""
	echo "This distribution is licensed under AGPL-3.0-or-later."
	echo "See LICENSE for the full text."
	echo ""
	echo "Third-party dependencies and their licenses are listed in"
	echo "DEPENDENCIES.tsv. License texts of bundled crates are copied"
	echo "into THIRD_PARTY_LICENSES/ when available in the local cargo registry."
} >"${out_dir}/NOTICE"

echo "==> クレートのライセンス本文を収集"
metadata="$(cargo metadata --format-version 1)"
mkdir -p "${out_dir}/THIRD_PARTY_LICENSES"
echo "$metadata" | jq -r '
  .packages[]
  | select(.source != null)
  | [.name, .version, (.license_file // "-"), (.manifest_path | sub("/Cargo.toml$"; ""))]
  | @tsv
' | while IFS=$'\t' read -r name version license_file manifest_dir || [[ -n "$name" ]]; do
	if [[ -n "$license_file" && -f "$license_file" ]]; then
		src="$license_file"
	else
		src=""
		for candidate in LICENSE LICENSE.txt LICENSE-MIT LICENSE-APACHE COPYING; do
			if [[ -f "${manifest_dir}/${candidate}" ]]; then
				src="${manifest_dir}/${candidate}"
				break
			fi
		done
	fi
	if [[ -n "$src" ]]; then
		cp "$src" "${out_dir}/THIRD_PARTY_LICENSES/${name}-${version}.txt"
	fi
done

count="$(find "${out_dir}/THIRD_PARTY_LICENSES" -type f | wc -l)"
echo "==> 完了: ${out_dir} (依存ライセンス本文 ${count} 件)"
