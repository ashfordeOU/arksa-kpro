// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! `arksa-kpro landing-nav <library|scenarios|verify>`, under the existing
//! `pro-mission-dossier` feature (a token with the `arksa` claim).

use std::path::Path;
use std::process::ExitCode;

use kshana_pro::cli::{flag, has_flag, now_unix, resolve_entitlement};
use kshana_pro::extension::ExtensionCommand;

use super::{campaign::Campaign, err};
use kshana::api::KshanaError;

/// Usage text for `arksa-kpro landing-nav`.
pub const USAGE: &str = "arksa-kpro landing-nav — independent verification of landing-navigation fault handling (MODELLED)\n\
\n\
  arksa-kpro landing-nav library [--json]\n\
      The descent-sensor fault library and the public incidents it stands for\n\
  arksa-kpro landing-nav scenarios <campaign.toml> --out <dir> [--sign-key-file <file>] [--signer <name>]\n\
      Pre-register a campaign: campaign.toml, preregistration.json (campaign hash, pass bars, stream\n\
      hashes), streams/<id>.csv to feed the filter under test and truth/<id>.csv; signed with a key\n\
  arksa-kpro landing-nav verify <campaign.toml> --logs <dir> --out <dir> [--prereg <preregistration.json>] [--sign-key-file <file>] [--signer <name>]\n\
      Check the filter's logs (<dir>/<scenario id>.csv) against the truth and the bars: verdict.json,\n\
      verdict.md, the inputs; signed with a key. Exit 0 when every scenario passes, 1 otherwise\n\
\n\
Verification only: no navigation algorithm is built or run here. A signing key is a file of 64 hex\n\
characters (or ARKSA_STUDY_SIGNING_KEY); check a signed directory with `arksa-kpro dossier verify`.\n\
Every command takes [--license <token>] or, in a debug build, [--dev]. See docs/LANDING-NAV-VERIFICATION.md.";

/// The command, for [`crate::commands::all`].
pub fn command() -> ExtensionCommand {
    ExtensionCommand {
        name: "landing-nav",
        usage: "arksa-kpro landing-nav <library|scenarios|verify> ... [--license <token>] [--dev]   Pre-registered verification of a lander navigation filter's descent-sensor fault handling, signed (`arksa-kpro landing-nav` for details)",
        run,
    }
}

fn run(args: &[String]) -> ExitCode {
    let args = &args[1..];
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let now = now_unix();
    let ent = match resolve_entitlement(args, now) {
        Ok(e) => e,
        Err(code) => return code,
    };
    if let Err(e) = crate::mission::ensure_entitled(&ent, now) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    match run_inner(args, now) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Positional arguments after the command, skipping flags and their values.
fn positionals(args: &[String]) -> Vec<String> {
    let takes_value = [
        "--out",
        "--logs",
        "--prereg",
        "--sign-key-file",
        "--signer",
        "--license",
    ];
    let mut v = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if takes_value.contains(&a.as_str()) {
            i += 2;
        } else {
            if !a.starts_with("--") {
                v.push(a.clone());
            }
            i += 1;
        }
    }
    v
}

fn read(p: &str) -> Result<Vec<u8>, KshanaError> {
    std::fs::read(p).map_err(|e| err(format!("{p}: {e}")))
}

fn load_campaign(args: &[String]) -> Result<(Campaign, Vec<u8>), KshanaError> {
    let path = positionals(args)
        .into_iter()
        .next()
        .ok_or_else(|| err(format!("{} needs a campaign file\n\n{USAGE}", args[0])))?;
    let bytes = read(&path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| err(format!("{path} is not UTF-8")))?;
    Ok((Campaign::parse(text)?, bytes))
}

fn signing_seed(args: &[String]) -> Result<Option<[u8; 32]>, KshanaError> {
    let key = match flag(args, "--sign-key-file") {
        Some(f) => Some(std::fs::read_to_string(&f).map_err(|e| err(format!("{f}: {e}")))?),
        None => std::env::var("ARKSA_STUDY_SIGNING_KEY").ok(),
    };
    key.filter(|k| !k.trim().is_empty())
        .map(|k| kshana_pro::license::parse_key_hex(k.trim()))
        .transpose()
}

fn report_signed(p: Option<std::path::PathBuf>) {
    match p {
        Some(p) => println!("signed: {}", p.display()),
        None => println!("not signed: no signing key given"),
    }
}

/// Run a `landing-nav` subcommand (`args[0]`); returns the exit code.
pub fn run_inner(args: &[String], now_unix: i64) -> Result<u8, KshanaError> {
    let seed = signing_seed(args)?;
    let signer_name = flag(args, "--signer").unwrap_or_else(|| "Arksa".into());
    let utc = crate::decision::time::fmt_utc(now_unix);
    let signer = seed
        .as_ref()
        .map(|s| (s, signer_name.as_str(), utc.as_str()));
    match args[0].as_str() {
        "library" => {
            if has_flag(args, "--json") {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&super::faults::library_json())
                        .expect("serialises")
                );
            } else {
                print!("{}", super::faults::library_markdown());
            }
            Ok(0)
        }
        "scenarios" => {
            let (c, bytes) = load_campaign(args)?;
            let out = flag(args, "--out").ok_or_else(|| err("scenarios needs --out <dir>"))?;
            let (p, signed) = super::write_preregistration(&c, &bytes, Path::new(&out), signer)?;
            println!(
                "pre-registered {} scenarios of '{}' (campaign sha256 {}); touchdown at {:.2} s; streams in {out}/streams",
                p.scenarios.len(),
                p.campaign_title,
                p.campaign_sha256,
                p.touchdown_s
            );
            report_signed(signed);
            Ok(0)
        }
        "verify" => {
            let (c, bytes) = load_campaign(args)?;
            let logs_dir = flag(args, "--logs").ok_or_else(|| err("verify needs --logs <dir>"))?;
            let out = flag(args, "--out").ok_or_else(|| err("verify needs --out <dir>"))?;
            let prereg = flag(args, "--prereg").map(|p| read(&p)).transpose()?;
            let status = prereg
                .as_deref()
                .map(|p| super::check_preregistration(&c, &bytes, p))
                .transpose()?;
            let logs = super::read_logs(&c, Path::new(&logs_dir))?;
            let v = super::verify::verify(
                &c,
                &crate::mbse::evidence::sha256_hex(&bytes),
                &logs,
                status,
            );
            let signed = super::write_verdict(
                &v,
                &bytes,
                &logs,
                prereg.as_deref(),
                Path::new(&out),
                signer,
            )?;
            println!("{}", v.summary());
            if prereg.is_none() {
                println!("no pre-registration given: the verdict records that the bars were not shown to be fixed in advance");
            }
            report_signed(signed);
            Ok(if v.body.verdict == super::verify::Verdict::Pass {
                0
            } else {
                1
            })
        }
        other => {
            println!("{USAGE}");
            Err(err(format!("unknown landing-nav command '{other}'")))
        }
    }
}
