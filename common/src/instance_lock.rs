//! `instance.lock` の解析と所有者生存判定を共有するヘルパ。
//!
//! サーバー起動時の単一ライター検査（`acquire_instance_lock`）と、
//! `backup_db` / `restore_db` の生存ロック検査で同じ判定を使うための単一情報源。
//! `parse_instance_lock` は文字列だけを扱う純粋関数で無条件に公開する。
//! `/proc` に依存する I/O は `#[cfg(not(target_arch = "wasm32"))]` で分離し、
//! wasm クライアントが `common` を依存しても `/proc` アクセスが混入しないようにする。

/// ロックファイルの内容から `(PID, プロセス開始時刻)` を取り出す純粋関数。
///
/// - 新形式: 1行目が PID、2行目が `/proc/<pid>/stat` の field 22 (starttime)。
/// - 旧形式: PID のみ（`starttime` は `None`）。後方互換として扱う。
/// - PID が読めない場合: `(0, None)` を返す。PID 0 は所有者を識別できないため、
///   呼び出し側は stale として回収せず起動を拒否する（fail-closed）。
pub fn parse_instance_lock(contents: &str) -> (u32, Option<u64>) {
    let mut lines = contents.lines();
    let pid = lines
        .next()
        .and_then(|line| line.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let starttime = lines
        .next()
        .and_then(|line| line.trim().parse::<u64>().ok());
    (pid, starttime)
}

/// `/proc/<pid>/stat` の field 22 (starttime: 起動からの clock ticks) を読む純粋関数。
/// `/proc` が無い環境・プロセスが消えた場合・形式が読めない場合は `None`。
#[cfg(not(target_arch = "wasm32"))]
pub fn read_proc_starttime(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm (field 2) は空白や ')' を含み得るため、最後の ')' より後だけを解析する。
    // ')' の直後は field 3 (state) なので field 22 はそこから 19 個先のトークン。
    let after_comm = stat.rfind(')')?;
    stat.get(after_comm + 1..)?
        .split_whitespace()
        .nth(19)?
        .parse::<u64>()
        .ok()
}

/// PID が 0（読解不能／所有者識別不能）かどうか。
/// 生存判定の前にこれを検査し、stale 回収ではなく fail-closed に倒す。
pub fn instance_lock_owner_unknown(pid: u32) -> bool {
    pid == 0
}

/// ロック所有者が生存中かを判定する。
/// `/proc` が読めない環境では判定不能のため「生存」として起動を拒否する（fail-closed）。
#[cfg(not(target_arch = "wasm32"))]
pub fn instance_lock_owner_alive(pid: u32, starttime: Option<u64>) -> bool {
    if read_proc_starttime(std::process::id()).is_none() {
        return true;
    }
    match starttime {
        // PID 再利用を防ぐため、開始時刻が一致したときだけ生存とみなす。
        // /proc/<pid> はあるのに stat が読めない場合は確認不能なので拒否側へ倒す。
        Some(expected) => match read_proc_starttime(pid) {
            Some(actual) => actual == expected,
            None => std::path::Path::new(&format!("/proc/{pid}")).exists(),
        },
        // 旧形式（PID のみ）は後方互換のため PID の存在だけで判定する。
        None => std::path::Path::new(&format!("/proc/{pid}")).exists(),
    }
}
