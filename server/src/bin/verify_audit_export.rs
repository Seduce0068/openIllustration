use anyhow::{Context, anyhow};
use common::{SignedAuditSnapshot, verify_audit_snapshot};

#[derive(Debug, Default)]
struct Args {
    snapshot: Option<String>,
    public_key: Option<String>,
    help: bool,
}

fn print_help() {
    eprintln!(
        "Usage: verify_audit_export --snapshot PATH --public-key PATH\n\
         \n\
         Verifies a signed audit snapshot using the operator's public key DER.\n\
         Exit code: 0 if valid, 1 on error."
    );
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => args.help = true,
            "--snapshot" => {
                args.snapshot = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("--snapshot requires a value"))?,
                );
            }
            "--public-key" => {
                args.public_key = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("--public-key requires a value"))?,
                );
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    Ok(args)
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

    verify_audit_snapshot(&snapshot, &public_key).map_err(|error| anyhow!("{error}"))?;
    println!(
        "audit_export_verification: OK version={} entries={} root={}",
        snapshot.version, snapshot.entry_count, snapshot.merkle_root
    );
    Ok(())
}
