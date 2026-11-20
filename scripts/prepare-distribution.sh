#!/usr/bin/env bash
# 配布物へ同梱するライセンス関連ファイルを生成する。
# 新規依存を追加せず、cargo deny / cargo metadata の出力のみを利用する。
set -euo pipefail

umask 077

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

out_dir="${1:-dist-license}"
if [[ -z "$out_dir" ]]; then
	echo "output directory must not be empty" >&2
	exit 1
fi
# 原子置換では out_dir 自身を rename するため、末尾スラッシュを正規化する。
while [[ "${#out_dir}" -gt 1 && "$out_dir" == */ ]]; do
	out_dir="${out_dir%/}"
done
# '..' 成分を含む入力は正規化前に拒否する（dist/.. や ../x のように
# 見かけ上安全な文字列で上位ディレクトリへ逃げることを防ぐ）。
case "/${out_dir}/" in
*/../*)
	echo "refusing unsafe output directory: $out_dir" >&2
	exit 1
	;;
esac
# 絶対正規化する（可能なら realpath -m / readlink -f でリンクも解決）。
if command -v realpath >/dev/null 2>&1; then
	out_dir_abs="$(realpath -m -- "$out_dir")"
elif command -v readlink >/dev/null 2>&1; then
	out_dir_abs="$(readlink -f -- "$out_dir")"
else
	# ツールが無い場合: 相対パスを repo_root 基準で絶対化し、'.' 成分と
	# 重複スラッシュを畳む（'..' は上で拒否済み）。
	if [[ "$out_dir" == /* ]]; then
		out_dir_abs="$out_dir"
	else
		out_dir_abs="${repo_root}/${out_dir}"
	fi
	IFS='/' read -r -a out_dir_parts <<<"$out_dir_abs"
	out_dir_abs=""
	for part in "${out_dir_parts[@]}"; do
		if [[ -n "$part" && "$part" != "." ]]; then
			out_dir_abs+="/${part}"
		fi
	done
	[[ -n "$out_dir_abs" ]] || out_dir_abs="/"
fi
# ルート / repo_root / repo_root の祖先は置換しない（破壊範囲を配布物に限定）。
case "$out_dir_abs" in
/ | "$repo_root")
	echo "refusing unsafe output directory: $out_dir" >&2
	exit 1
	;;
esac
if [[ "$repo_root" == "$out_dir_abs"/* ]]; then
	echo "refusing unsafe output directory: $out_dir" >&2
	exit 1
fi

for tool in jq cargo; do
	if ! command -v "$tool" >/dev/null 2>&1; then
		echo "required tool not found: $tool" >&2
		exit 1
	fi
done

# 配布前にライセンス許容性を検証する（不整合ならここで失敗し、成果物を作らない）。
echo "==> cargo deny check licenses"
cargo deny check licenses

# 生成物はまず出力先と同階層の私有ディレクトリへ作り、全て成功した時だけ公開する。
parent_dir="$(dirname "$out_dir")"
mkdir -p "$parent_dir"
work_dir="$(mktemp -d "${out_dir}.tmp.XXXXXX")"
old_dir=""
# build: 生成中 / stashed: 旧 out_dir 退避済み / published: 新 out_dir 配置済み / done: 完了
phase="build"

rollback() {
	# クリーンアップ前に終了コードを退避し、set +e でも失わないようにする。
	local rc=$?
	trap - EXIT
	set +e
	case "$phase" in
	build)
		# 公開前: 私有ディレクトリを消すだけで out_dir は無傷。
		rm -rf "$work_dir"
		;;
	stashed)
		# 旧 out_dir を退避した後: 旧を戻してから作業ディレクトリを消す。
		if [[ -n "$old_dir" && -d "$old_dir" ]]; then
			mv "$old_dir" "$out_dir"
		fi
		rm -rf "$work_dir"
		;;
	published)
		# 新 out_dir を配置した後: 新を捨てて旧を復元する。
		rm -rf "$out_dir"
		if [[ -n "$old_dir" && -d "$old_dir" ]]; then
			mv "$old_dir" "$out_dir"
		fi
		;;
	done)
		# 正常終了。旧ディレクトリは既に削除済み。
		;;
	esac
	exit "$rc"
}
trap rollback EXIT
# SIGINT/SIGTERM も exit に変換して EXIT trap（rollback）を確実に通す。
trap "exit 130" INT
trap "exit 143" TERM

echo "==> ライセンス本文を配置"
cp LICENSE "${work_dir}/LICENSE"

echo "==> 依存ライセンス一覧を生成"
cargo deny list --format json |
	jq -r '
      .licenses[]
      | .[0] as $license
      | .[1][]
      | [$license, .] | @tsv
    ' >"${work_dir}/DEPENDENCIES.tsv"

dependency_count="$(wc -l <"${work_dir}/DEPENDENCIES.tsv")"
echo "==> 依存ライセンスエントリ: ${dependency_count}"

echo "==> NOTICE を生成"
{
	echo "openIllustration / Digital Guild"
	echo "Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
	echo "License entries: ${dependency_count}"
	echo "Copyright (c) 2026 koppe"
	echo ""
	echo "This distribution is licensed under AGPL-3.0-or-later."
	echo "See LICENSE for the full text."
	echo ""
	echo "Third-party dependencies and their licenses are listed in"
	echo "DEPENDENCIES.tsv. License texts of bundled crates are copied"
	echo "into THIRD_PARTY_LICENSES/ when available in the local cargo registry."
} >"${work_dir}/NOTICE"

echo "==> クレートのライセンス本文を収集"
metadata="$(cargo metadata --format-version 1)"
licenses_dir="${work_dir}/THIRD_PARTY_LICENSES"
mkdir -p "$licenses_dir"
# 作業ディレクトリは新規だが、コピー前に .txt を拭いて件数を実依存数に揃える。
find "$licenses_dir" -maxdepth 1 -type f -name '*.txt' -delete
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
		cp "$src" "${licenses_dir}/${name}-${version}.txt"
	else
		# ライセンス本文が取得できない依存は、不完全な配布物を作らないため fail-closed。
		echo "no license text found for ${name}-${version}" >&2
		exit 1
	fi
done

count="$(find "$licenses_dir" -type f | wc -l)"

# 公開前に成果物同士の件数整合を検証する（不完全な出版物を出さない）。
tsv_lines="$(wc -l <"${work_dir}/DEPENDENCIES.tsv" | tr -d '[:space:]')"
notice_entries="$(awk -F': ' '/^License entries: / {print $2}' "${work_dir}/NOTICE")"
if [[ -z "$notice_entries" || "$notice_entries" != "$tsv_lines" ]]; then
	echo "license entry count mismatch: NOTICE=${notice_entries} DEPENDENCIES.tsv=${tsv_lines}" >&2
	exit 1
fi

# ここまで全て成功したので、旧成果物を退避してから新成果物を atomic に公開する。
if [[ -e "$out_dir" ]]; then
	old_dir="${out_dir}.old.$(date +%Y%m%dT%H%M%S).$$.${RANDOM}"
	while [[ -e "$old_dir" ]]; do
		old_dir="${out_dir}.old.$(date +%Y%m%dT%H%M%S).$$.${RANDOM}"
	done
	mv "$out_dir" "$old_dir"
	phase="stashed"
fi
mv "$work_dir" "$out_dir"
phase="published"

if [[ -n "$old_dir" ]]; then
	if ! rm -rf "$old_dir"; then
		echo "旧成果物の復元に失敗した: ${old_dir}" >&2
		exit 1
	fi
fi
phase="done"

echo "==> 完了: ${out_dir} (依存ライセンス本文 ${count} 件)"
