use anyhow::{Context, anyhow};
use common::env::read_env;
use common::{AuditLogEntry, compute_audit_chain_digest, mask_reason};
use libsql::Builder;
use server::resolve_database_url;

#[derive(Debug, Default)]
struct Args {
    database_url: Option<String>,
    cid: Option<String>,
    expect_root: Option<String>,
    include_reasons: bool,
    help: bool,
}

const HELP: &str = "Usage: verify_audit_log [--database-url URL] [--cid CID] [--expect-root HEX] [--include-reasons]\n\
     \n\
     Verifies content_audit_log hash-chain integrity (read-only).\n\
     Accepts v2 (length-prefixed) and legacy v1 entry hashes.\n\
     Scope: this v1/v2 tolerance applies to DB audit log entries only. Signed\n\
     audit snapshots are verified by verify_audit_export and require version 3;\n\
     v1/v2 snapshots are rejected there. `legacy_v1=true` in this tool's output\n\
     is a DB-entry classification, not a claim about snapshot compatibility.\n\
     Also checks contiguous log_id and prints a Merkle root over entry hashes.\n\
     --expect-root compares the root against a previously recorded value to detect\n\
     truncation or omission (the root must have been published out of band).\n\
     Reasons are redacted by default; --include-reasons exposes them (opt-in).\n\
     The remote libSQL token is read only from the DATABASE_TOKEN environment\n\
     variable (argv is public, so secrets are never accepted there).\n\
     DATABASE_URL is required: pass --database-url or set DATABASE_URL; no default is used.\n\
     DATABASE_TOKEN is read only from the environment (or left empty).\n\
     Exit code: 0 if valid, 1 if invalid or on error.";

fn print_help() {
    eprintln!("{HELP}");
}

/// 値取りの共通ヘルパ。オプション風トークン（`-` で始まる値）は値として受理せず、
/// 値やトークンをエコーせずに `{flag} requires a value` を返す。
fn take_value<'a>(it: &mut impl Iterator<Item = &'a str>, flag: &str) -> anyhow::Result<&'a str> {
    match it.next() {
        Some(value) if !value.starts_with('-') => Ok(value),
        _ => Err(anyhow!("{flag} requires a value")),
    }
}

fn parse_args_from<I>(items: I) -> anyhow::Result<Args>
where
    I: IntoIterator<Item = String>,
{
    // 所有権付き引数を保持してから借用し、値取りヘルパへ &str を渡す。
    let owned: Vec<String> = items.into_iter().collect();
    parse_str_args(&owned)
}

fn parse_str_args(items: &[String]) -> anyhow::Result<Args> {
    let mut args = Args::default();
    let mut it = items.iter().skip(1).map(String::as_str);
    while let Some(arg) = it.next() {
        match arg {
            "-h" | "--help" => args.help = true,
            "--database-url" => {
                args.database_url = Some(take_value(&mut it, "--database-url")?.to_string());
            }
            // argv は ps 等で公開される。トークン値は受け付けず、
            // エラーにも値をエコーしない（秘密値を出力しない）。
            "--database-token" => {
                return Err(anyhow!(
                    "--database-token is not accepted; set DATABASE_TOKEN in the environment instead"
                ));
            }
            "--cid" => {
                args.cid = Some(take_value(&mut it, "--cid")?.to_string());
            }
            "--expect-root" => {
                args.expect_root = Some(take_value(&mut it, "--expect-root")?.to_string());
            }
            "--include-reasons" => args.include_reasons = true,
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

fn parse_args() -> anyhow::Result<Args> {
    parse_args_from(std::env::args())
}

/// `--expect-root` の照合結果（I/O なし・終了コード判定は呼び出し側が行う）。
#[derive(Debug, PartialEq, Eq)]
enum ExpectRootOutcome {
    /// `--expect-root` 未指定（照合しない）。
    NotChecked,
    /// 大小文字を区別せず一致。
    Matched,
    /// 不一致。root 値を含まない固定文言のエラーを保持する。
    Mismatched(String),
}

/// 記録済み root と計算出的 root を照合する。既存の `eq_ignore_ascii_case` 比較は不変。
/// 不一致文言は `common::AUDIT_ROOT_MISMATCH` を参照する（両 CLI で共有）。
fn check_expect_root(expected: Option<&str>, computed: &str) -> ExpectRootOutcome {
    match expected {
        None => ExpectRootOutcome::NotChecked,
        Some(expected) if expected.eq_ignore_ascii_case(computed) => ExpectRootOutcome::Matched,
        Some(_) => ExpectRootOutcome::Mismatched(common::AUDIT_ROOT_MISMATCH.to_string()),
    }
}

async fn open_db(
    database_url: String,
    database_token: String,
) -> anyhow::Result<libsql::Connection> {
    let conn = if database_url.starts_with("file:") || !database_url.contains("://") {
        let path = database_url.strip_prefix("file:").unwrap_or(&database_url);
        Builder::new_local(path).build().await?.connect()?
    } else {
        Builder::new_remote(database_url, database_token)
            .build()
            .await?
            .connect()?
    };
    Ok(conn)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    if args.help {
        print_help();
        return Ok(());
    }

    // 暗黙の file:local.db 既定は撤廃。明示設定がなければ起動しない。
    let database_url = resolve_database_url(args.database_url, read_env("DATABASE_URL"))
        .map_err(|error| anyhow!(error.to_string()))?;
    // argv はプロセス一覧に露出するため、トークンは環境変数からのみ読む。
    let database_token = std::env::var("DATABASE_TOKEN").unwrap_or_default();

    let db = open_db(database_url, database_token)
        .await
        .context("failed to open database")?;

    let mut rows = db
        .query(
            "SELECT log_id, cid, old_status, new_status, changed_by, reason, previous_log_hash, entry_hash \
             FROM content_audit_log ORDER BY log_id ASC",
            (),
        )
        .await
        .context("failed to query content_audit_log (is the database migrated?)")?;

    let mut expected_prev = "GENESIS_HASH".to_string();
    let mut total: usize = 0;
    let mut shown: usize = 0;
    let mut errors: Vec<String> = Vec::new();
    let mut chain_entries: Vec<(i64, String)> = Vec::new();

    while let Some(row) = rows.next().await.context("failed to read audit log row")? {
        let log_id: i64 = row.get(0).context("log_id")?;
        let cid: String = row.get(1).context("cid")?;
        let old_status: String = row.get(2).context("old_status")?;
        let new_status: String = row.get(3).context("new_status")?;
        let changed_by: String = row.get(4).context("changed_by")?;
        let reason: String = row.get(5).context("reason")?;
        let previous_log_hash: String = row.get(6).context("previous_log_hash")?;
        let entry_hash: String = row.get(7).context("entry_hash")?;

        total += 1;

        let entry = AuditLogEntry {
            log_id: Some(log_id),
            cid: cid.clone(),
            old_status: old_status.clone(),
            new_status: new_status.clone(),
            changed_by: changed_by.clone(),
            reason: reason.clone(),
            previous_log_hash: previous_log_hash.clone(),
        };
        let computed_v1 = entry.calculate_hash();
        let computed_v2 = entry.calculate_hash_v2();
        let hash_ok = computed_v2 == entry_hash;
        // DB エントリのハッシュが旧 v1 形式かどうかの判定。
        // 署名付きスナップショットの形式互換を示すものではない
        // （common::verify_audit_snapshot は version 3 のみ受理する）。
        let legacy_v1 = computed_v1 == entry_hash;
        let chain_ok = previous_log_hash == expected_prev;

        if !hash_ok && !legacy_v1 {
            errors.push(format!(
                "log_id={log_id} cid={cid}: entry_hash mismatch (stored={entry_hash}, v2={computed_v2}, v1={computed_v1})"
            ));
        }
        if !chain_ok {
            errors.push(format!(
                "log_id={log_id} cid={cid}: previous_log_hash mismatch (stored={previous_log_hash}, expected={expected_prev})"
            ));
        }

        let include = args.cid.as_ref().map(|c| c == &cid).unwrap_or(true);
        if include {
            shown += 1;
            let display_reason = mask_reason(&reason, args.include_reasons);
            println!(
                "log_id={log_id} cid={cid} {old_status}->{new_status} by={changed_by} reason={display_reason} hash_ok={hash_ok} legacy_v1={legacy_v1} chain_ok={chain_ok} entry_hash={entry_hash}"
            );
        }

        expected_prev = entry_hash.clone();
        chain_entries.push((log_id, entry_hash));
    }

    if args.cid.is_some() {
        println!("shown={shown} total={total}");
    } else {
        println!("total={total}");
    }

    match compute_audit_chain_digest(&chain_entries) {
        Ok(digest) => {
            println!(
                "audit_chain_digest: count={} first={:?} last={:?} root={}",
                digest.count, digest.first_log_id, digest.last_log_id, digest.root_hash
            );
            match check_expect_root(args.expect_root.as_deref(), &digest.root_hash) {
                ExpectRootOutcome::NotChecked => {}
                ExpectRootOutcome::Matched => println!("audit_root_verification: OK"),
                ExpectRootOutcome::Mismatched(message) => errors.push(message),
            }
        }
        Err(error) => errors.push(format!("audit chain digest error: {error}")),
    }

    if errors.is_empty() {
        println!("audit_log_verification: OK");
        Ok(())
    } else {
        for e in &errors {
            eprintln!("ERROR: {e}");
        }
        eprintln!("audit_log_verification: FAILED ({} error(s))", errors.len());
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::env::EnvValue;
    use server::DatabaseUrlError;

    // --- parse_args_from: 認証トークンを argv で受け付けない ---

    #[test]
    fn database_token_argument_is_rejected_without_leaking_value() {
        const SECRET: &str = "TOP_SECRET_DATABASE_TOKEN_VALUE";
        let error =
            parse_args_from(["verify_audit_log", "--database-token", SECRET].map(String::from))
                .expect_err("--database-token must never be accepted on argv");
        let message = error.to_string();
        assert!(message.contains("DATABASE_TOKEN"), "{message}");
        assert!(!message.contains(SECRET), "token value leaked: {message}");
    }

    #[test]
    fn database_token_argument_without_value_is_also_rejected() {
        let error = parse_args_from(["verify_audit_log", "--database-token"].map(String::from))
            .expect_err("--database-token must be rejected even without a value");
        assert!(error.to_string().contains("DATABASE_TOKEN"), "{error:#}");
    }

    #[test]
    fn option_like_value_is_rejected_without_echo() {
        const DUMMY: &str = "SECRET_VALUE_MUST_NOT_APPEAR";
        for flag in ["--database-url", "--cid", "--expect-root"] {
            let argv = ["verify_audit_log", flag, "--database-token", DUMMY];
            let error = parse_args_from(argv.map(String::from))
                .expect_err("option-like token must be rejected as a value");
            let message = error.to_string();
            let expected = format!("{flag} requires a value");
            assert!(message.contains(expected.as_str()), "{message}");
            assert!(!message.contains(DUMMY), "dummy value leaked: {message}");
            assert!(
                !message.contains("--database-token"),
                "token flag leaked: {message}"
            );
        }
    }

    #[test]
    fn database_url_and_cid_are_still_parsed() {
        let args = parse_args_from(
            [
                "verify_audit_log",
                "--database-url",
                "file:app.db",
                "--cid",
                "bafy-test",
                "--expect-root",
                "deadbeef",
            ]
            .map(String::from),
        )
        .expect("documented flags must parse");
        assert_eq!(args.database_url.as_deref(), Some("file:app.db"));
        assert_eq!(args.cid.as_deref(), Some("bafy-test"));
        assert_eq!(args.expect_root.as_deref(), Some("deadbeef"));
        assert!(!args.help);
    }

    #[test]
    fn help_text_does_not_advertise_database_token() {
        assert!(!HELP.contains("--database-token"), "{HELP}");
        assert!(HELP.contains("DATABASE_TOKEN"), "{HELP}");
    }

    #[test]
    fn reasons_are_masked_by_default() {
        let args = parse_args_from(["verify_audit_log"].map(String::from)).unwrap();
        assert!(!args.include_reasons);
    }

    #[test]
    fn include_reasons_is_opt_in() {
        let args =
            parse_args_from(["verify_audit_log", "--include-reasons"].map(String::from)).unwrap();
        assert!(args.include_reasons);
    }

    #[test]
    fn help_flag_is_parsed() {
        let args = parse_args_from(["verify_audit_log", "--help"].map(String::from)).unwrap();
        assert!(args.help);
    }

    // --- check_expect_root: 不一致エラーの非エコー契約 ---

    #[test]
    fn mismatched_expect_root_error_does_not_echo_roots() {
        const EXPECTED_ROOT: &str =
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        const COMPUTED_ROOT: &str =
            "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

        // 既存の eq_ignore_ascii_case 照合・未指定時の挙動は不変。
        let upper = EXPECTED_ROOT.to_uppercase();
        assert_eq!(
            check_expect_root(Some(upper.as_str()), EXPECTED_ROOT),
            ExpectRootOutcome::Matched
        );
        assert_eq!(
            check_expect_root(None, COMPUTED_ROOT),
            ExpectRootOutcome::NotChecked
        );

        let outcome = check_expect_root(Some(EXPECTED_ROOT), COMPUTED_ROOT);
        match &outcome {
            ExpectRootOutcome::Mismatched(message) => {
                assert!(message.contains("audit root mismatch"), "{message}");
                assert!(
                    !message.contains(EXPECTED_ROOT),
                    "expected root leaked: {message}"
                );
                assert!(
                    !message.contains(COMPUTED_ROOT),
                    "computed root leaked: {message}"
                );
                // main が実際に出す stderr 行でも非エコーであることを確認する。
                let rendered = format!("ERROR: {message}");
                assert!(!rendered.contains(EXPECTED_ROOT), "{rendered}");
                assert!(!rendered.contains(COMPUTED_ROOT), "{rendered}");
            }
            other => panic!("expected Mismatched, got {other:?}"),
        }
    }

    // --- resolve_database_url: 暗黙既定値の撤廃 ---

    #[test]
    fn database_url_is_required_when_no_source_is_set() {
        let error = resolve_database_url(None, EnvValue::Missing)
            .expect_err("no implicit database URL may be used");
        assert_eq!(error.to_string(), DatabaseUrlError::Required.to_string());
    }

    #[test]
    fn database_url_from_cli_takes_priority_over_environment() {
        let url = resolve_database_url(
            Some("libsql://cli.example.invalid/app.db".to_string()),
            EnvValue::Utf8("libsql://env.example.invalid/app.db".to_string()),
        )
        .expect("CLI value must take priority over the environment");
        assert_eq!(url, "libsql://cli.example.invalid/app.db");
    }

    #[test]
    fn database_url_from_environment_is_accepted_without_cli() {
        let url = resolve_database_url(
            None,
            EnvValue::Utf8("libsql://env.example.invalid/app.db".to_string()),
        )
        .expect("environment value must be accepted");
        assert_eq!(url, "libsql://env.example.invalid/app.db");
    }

    #[test]
    fn blank_database_url_is_rejected() {
        for (cli, env) in [
            (Some(String::new()), EnvValue::Utf8("  \t\n ".to_string())),
            (Some("  \t\n ".to_string()), EnvValue::Missing),
            (None, EnvValue::Utf8(String::new())),
        ] {
            let error =
                resolve_database_url(cli, env).expect_err("blank database URL must be rejected");
            assert_eq!(error.to_string(), DatabaseUrlError::Required.to_string());
        }
    }

    #[test]
    fn database_url_error_does_not_echo_values() {
        const CLI_BLANK: &str = "  \t ";
        const ENV_BLANK: &str = "\n  ";
        let from_none = resolve_database_url(None, EnvValue::Missing)
            .expect_err("missing URL must be rejected")
            .to_string();
        let from_blank =
            resolve_database_url(Some(CLI_BLANK.into()), EnvValue::Utf8(ENV_BLANK.into()))
                .expect_err("blank URLs must be rejected")
                .to_string();
        // 入力値の有無にかかわらず同じ固定文言で、値や旧既定値をエコーしない。
        assert_eq!(from_blank, from_none);
        assert_eq!(from_none, DatabaseUrlError::Required.to_string());
        assert!(!from_none.contains("file:local.db"), "{from_none}");
        assert!(!from_none.contains("libsql://"), "{from_none}");
    }
}
