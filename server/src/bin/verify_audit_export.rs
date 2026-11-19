use anyhow::{Context, anyhow};
use common::{SignedAuditSnapshot, verify_audit_snapshot};

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    snapshot: Option<String>,
    public_key: Option<String>,
    expect_root: Option<String>,
    help: bool,
}

fn print_help() {
    eprintln!(
        "Usage: verify_audit_export --snapshot PATH --public-key PATH [--expect-root HEX]\n\
         \n\
         Verifies a signed audit snapshot using the operator's public key DER.\n\
         Version 3 snapshots only; v1/v2 are rejected (fail-closed).\n\
         --expect-root compares the Merkle root against a previously recorded value.\n\
         Exit code: 0 if valid, 1 on error."
    );
}

/// 値取りの共通ヘルパ。オプション風トークン（`-` で始まる値）は値として受理せず、
/// 値やトークンをエコーせずに `{flag} requires a value` を返す。
fn take_value<'a>(it: &mut impl Iterator<Item = &'a str>, flag: &str) -> anyhow::Result<&'a str> {
    match it.next() {
        Some(value) if !value.starts_with('-') => Ok(value),
        _ => Err(anyhow!("{flag} requires a value")),
    }
}

/// 引数を純粋にパースする。必須引数の欠落もここで検出する（`--help` は除外）。
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
            "--snapshot" => {
                args.snapshot = Some(take_value(&mut it, "--snapshot")?.to_string());
            }
            "--public-key" => {
                args.public_key = Some(take_value(&mut it, "--public-key")?.to_string());
            }
            "--expect-root" => {
                args.expect_root = Some(take_value(&mut it, "--expect-root")?.to_string());
            }
            // argv は ps 等で公開される。トークン値は受け付けず、
            // エラーにも値をエコーしない（秘密値を出力しない）。
            "--database-token" => {
                return Err(anyhow!(
                    "--database-token is not accepted; set DATABASE_TOKEN in the environment instead"
                ));
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    if !args.help {
        args.snapshot
            .as_ref()
            .ok_or_else(|| anyhow!("--snapshot is required"))?;
        args.public_key
            .as_ref()
            .ok_or_else(|| anyhow!("--public-key is required"))?;
    }
    Ok(args)
}

fn parse_args() -> anyhow::Result<Args> {
    parse_args_from(std::env::args())
}

/// 検証結果。出力内容と終了コード判定の元になる純粋な値。
#[derive(Debug, PartialEq, Eq)]
enum VerificationOutcome {
    /// 受理。stdout に出力する 1 行を保持する。
    Accepted { report: String },
    /// 拒否。stderr に出力するエラーメッセージを保持する。
    Rejected { message: String },
}

impl VerificationOutcome {
    /// 終了コード判定（純粋関数）。受理=0 / 拒否=1。
    fn exit_code(&self) -> i32 {
        match self {
            Self::Accepted { .. } => 0,
            Self::Rejected { .. } => 1,
        }
    }
}

/// 署名・連鎖・Merkle root・`--expect-root` を検証し、出力内容を純粋に組み立てる。
/// I/O も `std::process::exit` も行わない。
fn verify_export(
    snapshot: &SignedAuditSnapshot,
    public_key: &[u8],
    expect_root: Option<&str>,
) -> VerificationOutcome {
    // verify_audit_snapshot は version 3 のみ受理し、署名・チェーン・Merkle root を検証する。
    if let Err(error) = verify_audit_snapshot(snapshot, public_key) {
        return VerificationOutcome::Rejected {
            message: error.to_string(),
        };
    }
    if let Some(expected) = expect_root
        && !expected.eq_ignore_ascii_case(&snapshot.merkle_root)
    {
        return VerificationOutcome::Rejected {
            message: format!(
                "audit root mismatch: expected={expected}, computed={}",
                snapshot.merkle_root
            ),
        };
    }
    VerificationOutcome::Accepted {
        report: format!(
            "audit_export_verification: OK version={} entries={} governance={} root={}",
            snapshot.version,
            snapshot.entry_count(),
            snapshot.governance_entries.len(),
            snapshot.merkle_root
        ),
    }
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    if args.help {
        print_help();
        return Ok(());
    }
    let snapshot_path = args
        .snapshot
        .ok_or_else(|| anyhow!("--snapshot is required"))?;
    let key_path = args
        .public_key
        .ok_or_else(|| anyhow!("--public-key is required"))?;
    let snapshot: SignedAuditSnapshot = serde_json::from_str(
        &std::fs::read_to_string(&snapshot_path).context("failed to read snapshot")?,
    )
    .context("failed to parse snapshot")?;
    let public_key = std::fs::read(&key_path).context("failed to read public key")?;

    let outcome = verify_export(&snapshot, &public_key, args.expect_root.as_deref());
    match &outcome {
        VerificationOutcome::Accepted { report } => println!("{report}"),
        VerificationOutcome::Rejected { message } => eprintln!("Error: {message}"),
    }
    let code = outcome.exit_code();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
        AUDIT_SNAPSHOT_VERSION, AuditLogEntry, AuditSnapshotEntry, GovernanceAuditEntry,
        compute_audit_chain_digest, mask_reason, public_key_der_from_private_pem,
        sign_audit_snapshot,
    };
    use rsa::pkcs8::EncodePrivateKey;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 秘密情報を含む理由本文のマーカー。出力に漏れないことを検証する。
    const SECRET_MARKER: &str = "TOP-SECRET-INVESTIGATION-NOTE-DO-NOT-LEAK";

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    /// 一意な一時ディレクトリを表し、Drop で確実に削除するテストヘルパ。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "verify-audit-export-test-{}-{nanos}-{seq}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("create temp dir");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                    .expect("set temp dir mode");
            }
            Self(path)
        }

        fn child(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// テスト専用の決定的 RNG（common / server の既存テストと同じ方式）。
    struct TestRng(u64);

    impl TestRng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    impl rsa::rand_core::TryRng for TestRng {
        type Error = rsa::rand_core::Infallible;

        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            Ok((self.next() >> 32) as u32)
        }

        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            Ok(self.next())
        }

        fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
            for chunk in dst.chunks_mut(8) {
                let value = self.next().to_le_bytes();
                chunk.copy_from_slice(&value[..chunk.len()]);
            }
            Ok(())
        }
    }

    impl rsa::rand_core::TryCryptoRng for TestRng {}

    /// 決定論的な RSA 鍵ペアを一度だけ生成して共有する。
    fn test_key_pair() -> &'static (String, Vec<u8>) {
        static KEYS: OnceLock<(String, Vec<u8>)> = OnceLock::new();
        KEYS.get_or_init(|| {
            let mut rng = TestRng(0x9E37_79B9_7F4A_7C15);
            let key = rsa::PrivateKey::new(&mut rng, 2048).expect("generate test RSA key");
            let private_pem = key
                .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
                .expect("encode test RSA key")
                .to_string();
            let public_der =
                public_key_der_from_private_pem(&private_pem).expect("derive public key DER");
            (private_pem, public_der)
        })
    }

    /// 有効な v3 スナップショットを構築して署名する。
    /// `mask = true` なら既定のマスク版、`false` なら平文 reason 版。
    fn build_snapshot(private_pem: &str, mask: bool) -> SignedAuditSnapshot {
        let mut entries = Vec::new();
        let mut previous = "GENESIS_HASH".to_string();
        for id in 1..=2_i64 {
            let plain = format!("{SECRET_MARKER} entry {id}");
            let reason = if mask {
                mask_reason(&plain, false)
            } else {
                plain
            };
            let log = AuditLogEntry {
                log_id: Some(id),
                cid: format!("QmReasonTest{id}"),
                old_status: "Approved".into(),
                new_status: "Reported".into(),
                changed_by: "reporter".into(),
                reason,
                previous_log_hash: previous.clone(),
            };
            let entry_hash = log.calculate_hash_v2();
            entries.push(AuditSnapshotEntry {
                log_id: id,
                cid: log.cid.clone(),
                old_status: log.old_status.clone(),
                new_status: log.new_status.clone(),
                changed_by: log.changed_by.clone(),
                reason: log.reason.clone(),
                previous_log_hash: log.previous_log_hash.clone(),
                entry_hash: entry_hash.clone(),
            });
            previous = entry_hash;
        }

        let merkle_root = compute_audit_chain_digest(
            &entries
                .iter()
                .map(|entry| (entry.log_id, entry.entry_hash.clone()))
                .collect::<Vec<_>>(),
        )
        .expect("compute merkle root")
        .root_hash;

        let gov_reason = format!("{SECRET_MARKER} governance");
        let gov_details = format!("{SECRET_MARKER} details");
        let mut snapshot = SignedAuditSnapshot {
            version: AUDIT_SNAPSHOT_VERSION,
            reasons_masked: mask,
            entries,
            governance_entries: vec![GovernanceAuditEntry {
                log_id: 1,
                action: "BAN".into(),
                target: "QmTarget".into(),
                actor_hash: "actor-hash".into(),
                reason: if mask {
                    mask_reason(&gov_reason, false)
                } else {
                    gov_reason
                },
                details: Some(if mask {
                    mask_reason(&gov_details, false)
                } else {
                    gov_details
                }),
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
            merkle_root,
            signature: String::new(),
        };
        snapshot.signature = sign_audit_snapshot(&snapshot, private_pem).expect("sign snapshot");
        snapshot
    }

    /// テスト用の鍵・一時ディレクトリをまとめたフィクスチャ。Drop でtemp を削除する。
    struct TestFixture {
        dir: TempDir,
        private_pem: String,
        public_der: Vec<u8>,
    }

    impl TestFixture {
        fn new() -> Self {
            let (private_pem, public_der) = test_key_pair();
            Self {
                dir: TempDir::new(),
                private_pem: private_pem.clone(),
                public_der: public_der.clone(),
            }
        }

        fn masked_snapshot(&self) -> SignedAuditSnapshot {
            build_snapshot(&self.private_pem, true)
        }

        fn plaintext_snapshot(&self) -> SignedAuditSnapshot {
            build_snapshot(&self.private_pem, false)
        }

        /// スナップショット JSON と公開鍵 DER を一時ファイルへ書き、パスを返す。
        fn write_files(&self, snapshot: &SignedAuditSnapshot) -> (PathBuf, PathBuf) {
            let snapshot_path = self.dir.child("snapshot.json");
            let key_path = self.dir.child("public_key.der");
            std::fs::write(
                &snapshot_path,
                serde_json::to_string_pretty(snapshot).expect("serialize snapshot"),
            )
            .expect("write snapshot");
            std::fs::write(&key_path, &self.public_der).expect("write public key");
            (snapshot_path, key_path)
        }
    }

    fn assert_rejected(outcome: &VerificationOutcome, fragment: &str) {
        assert_eq!(outcome.exit_code(), 1, "expected rejection: {outcome:?}");
        match outcome {
            VerificationOutcome::Rejected { message } => assert!(
                message.contains(fragment),
                "expected message containing {fragment:?}, got {message:?}"
            ),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    /// 出力に秘密値・理由本文が含まれないことを検証する。
    fn assert_no_secret_leak(outcome: &VerificationOutcome) {
        let rendered = match outcome {
            VerificationOutcome::Accepted { report } => report.clone(),
            VerificationOutcome::Rejected { message } => message.clone(),
        };
        assert!(
            !rendered.contains(SECRET_MARKER),
            "secret marker leaked into output: {rendered}"
        );
        assert!(
            !rendered.contains("[redacted:"),
            "masked reason body leaked into output: {rendered}"
        );
    }

    // --- parse_args_from: 引数パース ---

    #[test]
    fn parse_args_accepts_all_flags() {
        let args = parse_args_from(
            [
                "verify_audit_export",
                "--snapshot",
                "snapshot.json",
                "--public-key",
                "public.der",
                "--expect-root",
                "deadbeef",
            ]
            .map(String::from),
        )
        .expect("valid arguments must parse");
        assert_eq!(args.snapshot.as_deref(), Some("snapshot.json"));
        assert_eq!(args.public_key.as_deref(), Some("public.der"));
        assert_eq!(args.expect_root.as_deref(), Some("deadbeef"));
        assert!(!args.help);
    }

    #[test]
    fn parse_args_without_expect_root_is_valid() {
        let args = parse_args_from(
            [
                "verify_audit_export",
                "--snapshot",
                "snapshot.json",
                "--public-key",
                "public.der",
            ]
            .map(String::from),
        )
        .expect("optional --expect-root must not be required");
        assert_eq!(args.expect_root, None);
    }

    #[test]
    fn parse_args_accepts_help_without_required_args() {
        for flag in ["--help", "-h"] {
            let args = parse_args_from(["verify_audit_export", flag].map(String::from))
                .unwrap_or_else(|e| panic!("{flag} must parse without required args: {e:#}"));
            assert!(args.help, "{flag} must set help");
        }
    }

    #[test]
    fn parse_args_rejects_unknown_argument() {
        let error = parse_args_from(
            [
                "verify_audit_export",
                "--snapshot",
                "s",
                "--public-key",
                "k",
                "--bogus",
            ]
            .map(String::from),
        )
        .expect_err("unknown argument must be rejected");
        assert!(
            error.to_string().contains("unknown argument: --bogus"),
            "{error:#}"
        );
    }

    #[test]
    fn database_token_argument_is_rejected_without_leaking_value() {
        const SECRET: &str = "SECRET_VALUE_MUST_NOT_APPEAR";
        let error =
            parse_args_from(["verify_audit_export", "--database-token", SECRET].map(String::from))
                .expect_err("--database-token must never be accepted on argv");
        let message = error.to_string();
        assert!(message.contains("DATABASE_TOKEN"), "{message}");
        assert!(!message.contains(SECRET), "token value leaked: {message}");
    }

    #[test]
    fn parse_args_rejects_missing_values() {
        for flag in ["--snapshot", "--public-key", "--expect-root"] {
            let error = parse_args_from(["verify_audit_export", flag].map(String::from))
                .expect_err("a value-less flag must be rejected");
            assert!(
                error
                    .to_string()
                    .contains(&format!("{flag} requires a value")),
                "{error:#}"
            );
        }
    }

    /// オプション風トークン（`-` 始まり）は値として受理しない。
    /// `--database-token` を値として消費せず `requires a value` で拒否し、
    /// ダミー値もトークン名もエラーにエコーしないことを検証する。
    #[test]
    fn option_like_token_is_rejected_as_value() {
        const SECRET: &str = "SECRET_VALUE_MUST_NOT_APPEAR";
        let error = parse_args_from(
            [
                "verify_audit_export",
                "--snapshot",
                "--database-token",
                SECRET,
            ]
            .map(String::from),
        )
        .expect_err("--snapshot must reject an option-like token as its value");
        let message = error.to_string();
        assert!(
            message.contains("--snapshot requires a value"),
            "must reject with a value-required error: {message}"
        );
        assert!(!message.contains(SECRET), "dummy value leaked: {message}");
        assert!(
            !message.contains("--database-token"),
            "consumed option token leaked: {message}"
        );
    }

    #[test]
    fn parse_args_rejects_missing_required_arguments() {
        let missing_snapshot =
            parse_args_from(["verify_audit_export"].map(String::from)).expect_err("must reject");
        assert!(
            missing_snapshot
                .to_string()
                .contains("--snapshot is required"),
            "{missing_snapshot:#}"
        );
        let missing_key =
            parse_args_from(["verify_audit_export", "--snapshot", "s.json"].map(String::from))
                .expect_err("must reject");
        assert!(
            missing_key.to_string().contains("--public-key is required"),
            "{missing_key:#}"
        );
    }

    // --- verify_export: 受理 ---

    #[test]
    fn valid_v3_snapshot_from_temp_files_is_accepted() {
        let fixture = TestFixture::new();
        let snapshot = fixture.masked_snapshot();
        let (snapshot_path, key_path) = fixture.write_files(&snapshot);

        // main と同じ経路で一時ファイルから読み直す。
        let loaded: SignedAuditSnapshot = serde_json::from_str(
            &std::fs::read_to_string(&snapshot_path).expect("read snapshot temp file"),
        )
        .expect("parse snapshot temp file");
        let public_key = std::fs::read(&key_path).expect("read public key temp file");
        assert_eq!(
            loaded, snapshot,
            "JSON round-trip must preserve the snapshot"
        );

        let outcome = verify_export(&loaded, &public_key, None);
        assert_eq!(outcome.exit_code(), 0);
        match &outcome {
            VerificationOutcome::Accepted { report } => {
                assert!(
                    report.starts_with("audit_export_verification: OK version=3 "),
                    "{report}"
                );
                assert!(report.contains("entries=2"), "{report}");
                assert!(report.contains("governance=1"), "{report}");
                assert!(
                    report.contains(&format!("root={}", snapshot.merkle_root)),
                    "{report}"
                );
            }
            other => panic!("expected Accepted, got {other:?}"),
        }
        assert_no_secret_leak(&outcome);
    }

    #[test]
    fn matching_expect_root_is_accepted_case_insensitively() {
        let fixture = TestFixture::new();
        let snapshot = fixture.masked_snapshot();
        let upper = snapshot.merkle_root.to_uppercase();
        let outcome = verify_export(&snapshot, &fixture.public_der, Some(&upper));
        assert_eq!(outcome.exit_code(), 0);
        assert!(
            matches!(outcome, VerificationOutcome::Accepted { .. }),
            "case-insensitive root match must be accepted: {outcome:?}"
        );
    }

    // --- verify_export: 拒否 ---

    #[test]
    fn mismatched_expect_root_is_rejected() {
        let fixture = TestFixture::new();
        let snapshot = fixture.masked_snapshot();
        let outcome = verify_export(&snapshot, &fixture.public_der, Some("deadbeef"));
        assert_rejected(&outcome, "audit root mismatch");
        match &outcome {
            VerificationOutcome::Rejected { message } => {
                assert!(message.contains("expected=deadbeef"), "{message}");
                assert!(
                    message.contains(&format!("computed={}", snapshot.merkle_root)),
                    "{message}"
                );
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn tampered_entry_is_rejected() {
        let fixture = TestFixture::new();
        let mut snapshot = fixture.masked_snapshot();
        snapshot.entries[0].cid = "QmTampered".into();
        let outcome = verify_export(&snapshot, &fixture.public_der, None);
        assert_rejected(&outcome, "Invalid signature");
    }

    #[test]
    fn tampered_merkle_root_is_rejected() {
        let fixture = TestFixture::new();
        let mut snapshot = fixture.masked_snapshot();
        snapshot.merkle_root = "00".repeat(32);
        let outcome = verify_export(&snapshot, &fixture.public_der, None);
        assert_rejected(&outcome, "Invalid signature");
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let fixture = TestFixture::new();
        let mut snapshot = fixture.masked_snapshot();
        // 先頭文字を反転させて base64 は有効なまま署名バイトを壊す。
        let mut signature = snapshot.signature.into_bytes();
        signature[0] = if signature[0] == b'A' { b'B' } else { b'A' };
        snapshot.signature = String::from_utf8(signature).expect("valid utf8");
        let outcome = verify_export(&snapshot, &fixture.public_der, None);
        assert_rejected(&outcome, "Invalid signature");
    }

    #[test]
    fn unsupported_versions_1_and_2_are_rejected() {
        let fixture = TestFixture::new();
        let snapshot = fixture.masked_snapshot();
        for version in [1_u32, 2_u32] {
            let mut snapshot = snapshot.clone();
            snapshot.version = version;
            let outcome = verify_export(&snapshot, &fixture.public_der, None);
            assert_rejected(
                &outcome,
                &format!("unsupported audit snapshot version: {version}"),
            );
        }
    }

    // --- 既定マスク: 秘密値・理由本文が出力されない ---

    #[test]
    fn masked_snapshot_output_has_no_secret_or_reason_body() {
        let fixture = TestFixture::new();
        let snapshot = fixture.masked_snapshot();
        assert!(
            snapshot
                .entries
                .iter()
                .all(|entry| entry.reason.starts_with("[redacted:")),
            "fixture must use the default mask"
        );
        let outcome = verify_export(&snapshot, &fixture.public_der, None);
        assert_eq!(outcome.exit_code(), 0);
        assert_no_secret_leak(&outcome);
    }

    #[test]
    fn plaintext_reason_snapshot_output_still_hides_reasons() {
        let fixture = TestFixture::new();
        let snapshot = fixture.plaintext_snapshot();
        assert!(
            snapshot.entries[0].reason.contains(SECRET_MARKER),
            "fixture must carry a plaintext reason"
        );
        let outcome = verify_export(&snapshot, &fixture.public_der, None);
        assert_eq!(
            outcome.exit_code(),
            0,
            "plaintext snapshot is still a valid v3"
        );
        assert_no_secret_leak(&outcome);
    }

    #[test]
    fn rejected_output_has_no_secret_or_reason_body() {
        let fixture = TestFixture::new();
        let mut snapshot = fixture.plaintext_snapshot();
        snapshot.entries[0].reason = format!("{SECRET_MARKER} injected after signing");
        let outcome = verify_export(&snapshot, &fixture.public_der, None);
        assert_rejected(&outcome, "Invalid signature");
        assert_no_secret_leak(&outcome);
    }

    // --- 一時ファイルのライフサイクル ---

    #[test]
    fn temp_files_are_removed_on_drop() {
        let paths = {
            let fixture = TestFixture::new();
            let snapshot = fixture.masked_snapshot();
            let paths = fixture.write_files(&snapshot);
            assert!(paths.0.exists(), "snapshot temp file must exist");
            assert!(paths.1.exists(), "key temp file must exist");
            paths
        };
        assert!(
            !paths.0.exists(),
            "snapshot temp file must be removed on Drop"
        );
        assert!(!paths.1.exists(), "key temp file must be removed on Drop");
    }
}
