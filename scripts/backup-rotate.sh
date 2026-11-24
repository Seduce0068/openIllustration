#!/usr/bin/env bash
# バックアップ取得→検証→古い世代のローテーションを一度に行う。
# cron 等のスケジューラから実行する想定。backup_db は生存 instance.lock を検出すると
# VACUUM INTO を実行せず拒否する（fail-closed）。サーバー停止中（メンテナンス窓）に実行する。
set -euo pipefail
# 生成する一時ファイル・成果物を 0600 に制限する（service の UMask=0077 と同等）。
umask 077

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

backup_dir="${BACKUP_DIR:-${repo_root}/backups}"
# unset 時のみ既定を適用（空文字は下の検証で拒否する）。
prefix="${BACKUP_PREFIX-openillustration}"
# prefix は backup_dir 直下のファイル名一部。パス区切り・".."・glob メタ文字を
# 許すと成果物が backup_dir 外へ作られたり、find -name のパターンへ伝播して
# 他 prefix の世代を誤削除し得る。安全な文字セットへ制限する。
if [[ ! "$prefix" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ || "$prefix" == *'..'* ]]; then
	echo "invalid BACKUP_PREFIX: use letters, digits, '.', '_' or '-' (no '..')" >&2
	exit 1
fi
retention_days="${RETENTION_DAYS:-30}"
min_keep="${MIN_KEEP:-3}"
bin_dir="${BIN_DIR:-${repo_root}/target/debug}"
gpg_recipient="${BACKUP_GPG_RECIPIENT:-}"
gpg_bin="${GPG_BIN:-gpg}"
# ローテーション列挙に使う find（テストで失敗を注入できるように上書き可能）。
find_bin="${FIND_BIN:-find}"
# 平文バックアップは開発専用。明示的なオプトアウトが無い限り拒否する
# （オフサイト保存が設定されている場合はオプトアウトも認めない）。
allow_plaintext="${ALLOW_UNENCRYPTED_BACKUP:-0}"
# オフサイト保存: 実行ファイルのパスと宛先を指定する（eval は使わない）。
offsitecopy="${OFFSITE_COPY_CMD:-}"
offsitedest="${OFFSITE_DEST:-}"
# 失敗通知: 実行ファイルのパス。呼び出しは <メッセージ> の1引数（eval 不使用）。
alert_cmd="${ALERT_CMD:-}"

rotation_list=""
tmp_dir=""
tmp_db=""
heartbeat_tmp=""
# 失敗通知に添える段階名。初期値は setup とし、各検証・公開段階の直前で更新する。
stage="setup"
on_failure() {
	local code=$?
	if [[ -n "$tmp_dir" ]]; then
		rm -rf -- "$tmp_dir" || true
	fi
	if [[ -n "$rotation_list" ]]; then
		rm -f -- "$rotation_list" || true
	fi
	if [[ -n "$heartbeat_tmp" ]]; then
		rm -f -- "$heartbeat_tmp" || true
	fi
	if ((code == 0)); then
		return 0
	fi
	if [[ -n "$alert_cmd" ]]; then
		"$alert_cmd" "openIllustration backup failed at ${stage} (exit ${code}) on $(hostname)" >/dev/null 2>&1 || true
	fi
	return "$code"
}
trap on_failure EXIT
# SIGINT/SIGTERM も exit に変換して EXIT trap（平文 tmp の cleanup）を確実に通す。
# timeout 1800 / TimeoutStartSec=1800 の強制終了で tmp_db が残置されないようにする。
trap "exit 130" INT
trap "exit 143" TERM

# 別経路で保存した監査 Merkle root（hex、非機微）。設定時は verify_audit_log の
# --expect-root へ渡し、監査ログの空化・末尾切詰めを自動検証で検出する。
# 未設定時は内部整合性チェックのみ（既存運用を壊さない後方互換）。
# root 値はエラー文言へエコーしない既存契約を維持する。
audit_expect_root="${AUDIT_EXPECT_ROOT:-}"
# 非空時は Merkle root の hex 形式のみ受理する。空白のみ・先頭ダッシュ等の
# 不正値は fail-closed で拒否し、エラーには設定名のみを出して値はエコーしない。
if [[ -n "$audit_expect_root" && ! "$audit_expect_root" =~ ^[0-9a-fA-F]{64}$ ]]; then
	echo "AUDIT_EXPECT_ROOT must be a 64-character hexadecimal Merkle root" >&2
	exit 1
fi

run_bin() {
	local name="$1"
	shift
	if [[ -x "${bin_dir}/${name}" ]]; then
		"${bin_dir}/${name}" "$@"
	else
		cargo run --quiet -p server --bin "$name" -- "$@"
	fi
}

# 公開先に既存ファイルやシンボリックリンクがあれば上書きしない。
# -e だけではダングリングシンボリックリンクを検出できないため -L も確認する。
assert_publishable() {
	local target="$1"
	if [[ -L "$target" ]]; then
		echo "Refusing to write through a symlink: ${target}" >&2
		return 1
	fi
	if [[ -e "$target" ]]; then
		echo "backup already exists: ${target}" >&2
		return 1
	fi
}

if ! [[ "$retention_days" =~ ^(0|[1-9][0-9]*)$ && "$min_keep" =~ ^(0|[1-9][0-9]*)$ ]]; then
	echo "RETENTION_DAYS and MIN_KEEP must be non-negative integers without leading zeros" >&2
	exit 1
fi
# 上限は cutoff = now - retention_days*86400 が非負になる範囲に制限する。
# 旧上限 36500 日では cutoff が負になり mtime < cutoff が常に false となって
# 古い世代が一切削除されない（fail-open）ため、now/86400 より小さい 20000 日
# （約55年）へ引き下げる。2026 年時点の now/86400 は約 20700 日であり、
# 時刻が進むほど cutoff は大きくなるため将来にわたって安全である。
# 桁数と辞書順で比較し、巨大な整数でも算術オーバーフローなしに拒否する
# （先頭ゼロは上の正規表現で既に拒否済みなので、同桁の辞書順は数値順と一致する）。
max_retention_days=20000
if ((${#retention_days} > ${#max_retention_days})); then
	echo "RETENTION_DAYS must not exceed ${max_retention_days}" >&2
	exit 1
fi
if ((${#retention_days} == ${#max_retention_days})) &&
	[[ "$retention_days" != "$max_retention_days" ]] &&
	printf '%s\n%s\n' "$max_retention_days" "$retention_days" | sort -C; then
	echo "RETENTION_DAYS must not exceed ${max_retention_days}" >&2
	exit 1
fi
# 防御チェック: 上限変更や時刻の異常で cutoff が負になる場合も黙って全保持
# （fail-open）せず拒否する。ローテーションでもこの時刻を使う。
now_epoch="$(date -u +%s)"
if ((now_epoch - retention_days * 86400 <= 0)); then
	echo "RETENTION_DAYS is too large for the current clock: cutoff would be non-positive" >&2
	exit 1
fi
# MIN_KEEP も 64bit 算術のオーバーフローに対して脆弱だった。
# 例えば 18446744073709551617 (2^64+1) は 1 にラップして
# 「1 以上」の検査を通過してしまう（fail-open）。RETENTION_DAYS と同じく
# 桁数と辞書順で比較し、算術を行わずに上限を超える値を拒否する
# （先頭ゼロは上の正規表現で拒否済みなので同桁の辞書順は数値順と一致する）。
max_min_keep=1000
if ((${#min_keep} > ${#max_min_keep})); then
	echo "MIN_KEEP must not exceed ${max_min_keep}" >&2
	exit 1
fi
if ((${#min_keep} == ${#max_min_keep})) &&
	[[ "$min_keep" != "$max_min_keep" ]] &&
	printf '%s\n%s\n' "$max_min_keep" "$min_keep" | sort -C; then
	echo "MIN_KEEP must not exceed ${max_min_keep}" >&2
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
# mktemp の作業ディレクトリ（平文 tmp_db を含む）とローテーション列挙の
# 一時ファイルは、成否にかかわらず EXIT 時に必ず削除する。
# 既存のバックアップ先が 0700 より開いている場合は暗黙に修正せず拒否する（fail-closed）。
if [[ -L "$backup_dir" ]]; then
	# 公開先ファイルと同じく、ディレクトリ自体のシンボリックリンクも拒否する。
	echo "error: backup directory ${backup_dir} must not be a symbolic link" >&2
	exit 1
fi
if [[ -e "$backup_dir" ]]; then
	existing_mode="$(stat -c %a "$backup_dir")"
	if [[ "$existing_mode" != "700" ]]; then
		echo "error: backup directory ${backup_dir} has mode ${existing_mode}; expected 700" >&2
		exit 1
	fi
fi
mkdir -p "$backup_dir"
if ! chmod 700 "$backup_dir"; then
	echo "error: failed to restrict permissions on ${backup_dir}" >&2
	exit 1
fi
# chmod の成否にかかわらず最終的な mode を stat で検証する（特殊フラグ等の取りこぼし防止）。
actual_mode="$(stat -c %a "$backup_dir")"
if [[ "$actual_mode" != "700" ]]; then
	echo "error: backup directory ${backup_dir} has mode ${actual_mode}; expected 700" >&2
	exit 1
fi

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
output="${backup_dir}/${prefix}-${stamp}.db"
encrypted="${output}.gpg"
assert_publishable "$output" || exit 1
if [[ -n "$gpg_recipient" ]]; then
	assert_publishable "$encrypted" || exit 1
fi
# 監査 root は成果物と同じ stamp の別ファイルとして公開する。
# 暗号化の有無にかかわらず同じ世代名（<prefix>-<stamp>.audit-root）を使う。
audit_root_output="${backup_dir}/${prefix}-${stamp}.audit-root"
assert_publishable "$audit_root_output" || exit 1
# 鮮度監視用ハートビート（固定名）。成功時にのみ UTC RFC3339 で上書きする。
# 固定パスなので assert_publishable は使わず、公開時に非通常ファイルを拒否する。
heartbeat_file="${backup_dir}/.last-success"

# 予測可能な最終パスへ直接書き込まず、mktemp の私有ディレクトリで生成して
# 検証後に atomic rename で公開する（VACUUM INTO は既存ファイルへ書けない）。
tmp_dir="$(mktemp -d "${backup_dir}/.${prefix}-${stamp}.XXXXXX")"
tmp_db="${tmp_dir}/${prefix}.db"
tmp_encrypted="${tmp_dir}/${prefix}.db.gpg"

echo "==> backup: ${output}"
stage="backup_db"
run_bin backup_db --output "$tmp_db"
stage="verify_backup"
run_bin verify_backup --input "$tmp_db"
# Runbook と同じく verify_backup / verify_audit_log の両方が成功しなければ
# バックアップとして認めない（fail-closed）。検証対象は生成直後の平文 tmp_db。
# ローカルファイルなのでトークンは不要で、--database-url を渡す。
# AUDIT_EXPECT_ROOT が非空のときだけ --expect-root を追加し、別経路保存の root と照合する。
stage="verify_audit_log"
audit_verify_args=(--database-url "file:${tmp_db}")
if [[ -n "$audit_expect_root" ]]; then
	audit_verify_args+=(--expect-root "$audit_expect_root")
fi
# 成功出力の計算 root を取得して世代保存する。出力全体を捕捉して stdout へ出さない
# ことで、root 値がログや ALERT へエコーされる経路を断つ（既存の非エコー契約を維持）。
audit_verify_output="$(run_bin verify_audit_log "${audit_verify_args[@]}")"
audit_root_line="$(printf '%s\n' "$audit_verify_output" | grep -E '^audit_chain_digest: .* root=[0-9a-fA-F]{64}$' | tail -n 1 || true)"
audit_root="${audit_root_line##* root=}"
if [[ ! "$audit_root" =~ ^[0-9a-fA-F]{64}$ ]]; then
	# 値・生バイトはエコーせず、段階名のみを通知する（fail-closed）。
	echo "verify_audit_log did not report a valid Merkle root" >&2
	exit 1
fi
# tmp_dir は 0600 の umask 下にあり、失敗時は on_failure で必ず削除される。
tmp_audit_root="${tmp_dir}/${prefix}.audit-root"
printf '%s\n' "$audit_root" >"$tmp_audit_root"
chmod 600 "$tmp_audit_root"
stage="publish"
assert_publishable "$output" || exit 1
mv -T -- "$tmp_db" "$output"
assert_publishable "$audit_root_output" || exit 1
mv -T -- "$tmp_audit_root" "$audit_root_output"

if [[ -n "$gpg_recipient" ]]; then
	stage="encrypt"
	echo "==> encrypting backup for ${gpg_recipient}"
	if ! "$gpg_bin" --batch --yes --encrypt --recipient "$gpg_recipient" \
		--output "$tmp_encrypted" "$output"; then
		# 暗号化失敗時は平文を残す（既存の fail-closed）上で途中の暗号文を削除する。
		rm -f "$tmp_encrypted"
		exit 1
	fi
	# ADR-0013 P2: 暗号化成功後に detached 署名を生成する。
	# 署名鍵は暗号化に使う既存の GPG 秘密鍵と同じもの（監査署名鍵とは別）で、
	# --local-user を指定せず鍵リングの既定秘密鍵を使用する。
	# 復元前の署名検証は verify_backup 側の担当外のため、ここでは署名生成と
	# Runbook へ渡す署名ファイル名（.asc）の出力のみを行う。
	# 公開前に署名を作るため、署名失敗時は平文が残り成果物は出ない（fail-closed）。
	stage="sign"
	tmp_signature="${tmp_encrypted}.asc"
	if ! "$gpg_bin" --batch --yes --detach-sign --armor \
		--output "$tmp_signature" "$tmp_encrypted"; then
		rm -f -- "$tmp_signature" "$tmp_encrypted"
		exit 1
	fi
	# 暗号化・署名の成功後は暗号文を先に公開し、その成功後に平文を削除する。
	# assert_publishable / mv が失敗した場合は平文が残り復元できる（fail-closed）。
	stage="publish"
	assert_publishable "$encrypted" || exit 1
	mv -T -- "$tmp_encrypted" "$encrypted"
	assert_publishable "${encrypted}.asc" || exit 1
	mv -T -- "$tmp_signature" "${encrypted}.asc"
	rm -f "$output"
	echo "==> plaintext removed: ${output}"
	echo "==> detached signature: ${encrypted##*/}.asc"
fi
# rmdir 失敗時に実際の処理と一致する段階名を通知する。
stage="finalize"
rmdir "$tmp_dir"

if [[ -n "$offsitecopy" ]]; then
	stage="offsite"
	artifact="$output"
	if [[ -n "$gpg_recipient" ]]; then
		artifact="${output}.gpg"
	fi
	# 監査 root は成果物と世代をなす対として同じ先へ送る。欠けるとオフサイト先での
	# --expect-root 照合（欠落・末尾切詰め検知）が不能になるため必須。
	if [[ ! -f "$audit_root_output" ]]; then
		echo "error: audit root is missing for offsite copy" >&2
		exit 1
	fi
	# ADR-0013 P2: 監査 root → 成果物 → detached 署名 の順で送る。
	# オフサイト先で復元前に gpg --verify を通すため .asc も世代対に必須。
	copies=("$audit_root_output" "$artifact")
	if [[ -n "$gpg_recipient" ]]; then
		if [[ ! -f "${encrypted}.asc" ]]; then
			echo "error: detached signature is missing for offsite copy" >&2
			exit 1
		fi
		copies+=("${encrypted}.asc")
	fi
	echo "==> offsite copy: audit root, artifact, then detached signature"
	for ((copy_index = 0; copy_index < ${#copies[@]}; copy_index++)); do
		if ! "$offsitecopy" "${copies[$copy_index]}" "$offsitedest"; then
			# 途中失敗で一部だけ残る場合は値・パスをエコーせず固定文言で警告する。
			# 不完全世代は手動 cleanup が必要（Runbook 参照）。
			echo "WARNING: offsite copy partially completed: only some generation artifacts were sent; manual cleanup at the offsite destination is required (see Runbook)" >&2
			exit 1
		fi
	done
fi

# 新しい順（ファイル名の UTC タイムスタンプ順）に並べ、直近 min_keep 件は
# 保持期間を超えていても残す（誤設定で全削除されるのを防ぐ）。
stage="rotate"
cutoff=$((now_epoch - retention_days * 86400))
# process substitution の終了ステータスは親シェルに伝わらないため、
# find の出力を一時ファイルへ書き出してから明示的に成否を確認する。
rotation_list="$(mktemp "${backup_dir}/.${prefix}-rotate.XXXXXX")"
if ! "$find_bin" "$backup_dir" -maxdepth 1 -type f \
	\( -name "${prefix}-*.db" -o -name "${prefix}-*.db.gpg" \) -print0 |
	sort -zr >"$rotation_list"; then
	echo "error: failed to enumerate backups in ${backup_dir}" >&2
	exit 1
fi
scanned=0
pruned=0
while IFS= read -r -d '' file; do
	scanned=$((scanned + 1))
	if ((scanned <= min_keep)); then
		continue
	fi
	mtime="$(stat -c %Y "$file")"
	if ((mtime < cutoff)); then
		rm -f "$file"
		# 同一世代の監査 root も合わせて削除し、古い世代の root が誤選択されないようにする。
		# .audit-root はローテーション列挙には含めない（kept/pruned 件数を変えないため）。
		rm -f -- "${file%.db}.audit-root" "${file%.db.gpg}.audit-root"
		# 対応する detached 署名も同時に削除する（列挙対象には入れず kept/pruned 件数は維持）。
		rm -f -- "${file}.asc"
		pruned=$((pruned + 1))
		echo "pruned: ${file}"
	fi
done <"$rotation_list"

# ADR-0013 P2: ここまで全段階が成功したときだけハートビートを更新する。
# 固定パスを mktemp → atomic rename で上書きし、シンボリックリンクや
# 通常ファイル以外は拒否する（公開ファイルと同じ fail-closed 契約）。
stage="heartbeat"
if [[ -L "$heartbeat_file" || (-e "$heartbeat_file" && ! -f "$heartbeat_file") ]]; then
	echo "Refusing to write heartbeat through a non-regular target" >&2
	exit 1
fi
heartbeat_tmp="$(mktemp "${backup_dir}/.${prefix}-heartbeat.XXXXXX")"
# stamp は YYYYmmddTHHMMSSZ（16桁）。最新世代と同じ時刻を RFC3339 で公開する。
heartbeat_ts="${stamp:0:4}-${stamp:4:2}-${stamp:6:2}T${stamp:9:2}:${stamp:11:2}:${stamp:13:2}Z"
printf '%s\n' "$heartbeat_ts" >"$heartbeat_tmp"
chmod 600 "$heartbeat_tmp"
mv -T -- "$heartbeat_tmp" "$heartbeat_file"
heartbeat_tmp=""
echo "==> heartbeat: ${heartbeat_file}"

# kept は走査カウンタ scanned ではなく実際の保持件数 (scanned - pruned) を報告する。
echo "==> complete: kept=$((scanned - pruned)) pruned=${pruned} retention_days=${retention_days}"
