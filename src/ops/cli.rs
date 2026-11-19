// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! `arksa-kpro ops <command>`: the command-line front end. The binary resolves the licence
//! and requires the mission dossier feature first (crate::commands), then calls [`run`].

use std::path::Path;

use super::{ad_scorecard, err, test_report, tracking_gap, write_dir};
use kshana::api::KshanaError;

/// Usage text for `arksa-kpro ops`.
pub const USAGE: &str = "arksa-kpro ops — operations assurance (signed reports)\n\
\n\
  arksa-kpro ops report <procedure.toml> <run-log.csv> --out <dir> [--sign-key-file <file>] [--signer <name>]\n\
      Operations validation report from a test-run log: steps, telecommand verification, expected\n\
      against observed telemetry, deviations and the requirement matrix (ECSS-E-ST-70-32C terms).\n\
      Writes report.json, report.md, procedure.toml, run-log.csv\n\
  arksa-kpro ops ad-score <spec.toml> --out <dir> [--sign-key-file <file>] [--signer <name>]\n\
      Score a telemetry anomaly detector on a benchmark (ESA-AD interval labels or OPSSAT-AD\n\
      segments) against pre-registered bars. Writes scorecard.json, scorecard.md, spec.toml, bars.toml\n\
  arksa-kpro ops tracking-gap <study.toml> --out <dir> [--sign-key-file <file>] [--signer <name>]\n\
      What station outages cost each mission (contact, data lost to recorder overflow, gaps over\n\
      limit, ranging days, critical windows) and what alternate windows recover. Writes impact.json,\n\
      impact.md and the inputs\n\
  arksa-kpro ops verify <dir>\n\
      Recompute the body hash of a report, scorecard or impact directory (the signature is checked\n\
      with `arksa-kpro dossier verify <dir> --trust <public key>`)\n\
\n\
Files a spec or study names are read relative to it. A signing key is a file of 64 hex characters\n\
(or ARKSA_STUDY_SIGNING_KEY). Every command takes [--license <token>] or, in a debug build, [--dev].\n\
Feature: pro-mission-dossier (Arksa). See docs/OPS-ASSURANCE.md.";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn positionals(args: &[String]) -> Vec<String> {
    let takes_value = ["--out", "--license", "--sign-key-file", "--signer"];
    let mut out = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if takes_value.contains(&a.as_str()) {
            i += 2;
            continue;
        }
        if !a.starts_with("--") {
            out.push(a.clone());
        }
        i += 1;
    }
    out
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

fn read(p: &str) -> Result<String, KshanaError> {
    std::fs::read_to_string(p).map_err(|e| err(format!("{p}: {e}")))
}

fn base_of(p: &str) -> &Path {
    Path::new(p).parent().unwrap_or(Path::new("."))
}

/// Run an `ops` command (`args[0]` is the command); `now_unix` stamps a signature.
/// Returns the exit code: 0 on success, 1 when `verify` finds a problem, 2 on a usage error.
pub fn run(args: &[String], now_unix: i64) -> Result<u8, KshanaError> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let pos = positionals(args);
    let seed = signing_seed(args)?;
    let signer_name = flag(args, "--signer").unwrap_or_else(|| "Arksa".into());
    let stamp = crate::decision::time::fmt_utc(now_unix);
    let signer = seed
        .as_ref()
        .map(|s| (s, signer_name.as_str(), stamp.as_str()));
    let out = || flag(args, "--out").ok_or_else(|| err(format!("{cmd} needs --out <dir>")));
    let report_signed = |p: Option<std::path::PathBuf>| match p {
        Some(p) => println!("signed: {}", p.display()),
        None => println!("not signed: no signing key given"),
    };
    match cmd {
        "report" => {
            let (Some(proc_), Some(log)) = (pos.first(), pos.get(1)) else {
                return Err(err(format!(
                    "report needs a procedure and a run log\n\n{USAGE}"
                )));
            };
            let dir = out()?;
            let (r, pb, lb) = test_report::from_files(Path::new(proc_), Path::new(log))?;
            let signed = write_dir(
                Path::new(&dir),
                &[
                    ("report.json", (r.to_json() + "\n").into_bytes()),
                    ("report.md", r.to_markdown().into_bytes()),
                    ("procedure.toml", pb),
                    ("run-log.csv", lb),
                ],
                signer,
            )?;
            println!("{}", r.summary_line());
            println!("report_sha256 {}", r.report_sha256);
            report_signed(signed);
            Ok(0)
        }
        "ad-score" => {
            let spec = pos
                .first()
                .ok_or_else(|| err(format!("ad-score needs a spec file\n\n{USAGE}")))?;
            let dir = out()?;
            let text = read(spec)?;
            let (sc, bars) = ad_scorecard::run(&text, base_of(spec))?;
            let mut files = vec![
                ("scorecard.json", (sc.to_json() + "\n").into_bytes()),
                ("scorecard.md", sc.to_markdown().into_bytes()),
                ("spec.toml", text.into_bytes()),
            ];
            if let Some(b) = bars {
                files.push(("bars.toml", b));
            }
            let signed = write_dir(Path::new(&dir), &files, signer)?;
            println!("{}", sc.summary_line());
            println!("scorecard_sha256 {}", sc.scorecard_sha256);
            report_signed(signed);
            Ok(0)
        }
        "tracking-gap" => {
            let study = pos
                .first()
                .ok_or_else(|| err(format!("tracking-gap needs a study file\n\n{USAGE}")))?;
            let dir = out()?;
            let text = read(study)?;
            let (imp, inputs) = tracking_gap::run(&text, base_of(study))?;
            let mut files: Vec<(&str, Vec<u8>)> = vec![
                ("impact.json", (imp.to_json() + "\n").into_bytes()),
                ("impact.md", imp.to_markdown().into_bytes()),
            ];
            for (n, b) in &inputs {
                files.push((n.as_str(), b.clone()));
            }
            let signed = write_dir(Path::new(&dir), &files, signer)?;
            println!("{}", imp.summary_line());
            println!("impact_sha256 {}", imp.impact_sha256);
            report_signed(signed);
            Ok(0)
        }
        "verify" => {
            let dir = pos
                .first()
                .ok_or_else(|| err(format!("verify needs a directory\n\n{USAGE}")))?;
            let found = [
                ("report.json", "report_sha256"),
                ("scorecard.json", "scorecard_sha256"),
                ("impact.json", "impact_sha256"),
            ]
            .into_iter()
            .find(|(f, _)| Path::new(dir).join(f).exists());
            let Some((file, key)) = found else {
                println!("FAIL: {dir} holds no report.json, scorecard.json or impact.json");
                return Ok(1);
            };
            let text = read(&Path::new(dir).join(file).to_string_lossy())?;
            let doc: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| err(format!("{file}: {e}")))?;
            match super::verify_body_hash(&doc, key) {
                Ok(()) => {
                    println!("OK: {file} body hashes to its {key}");
                    Ok(0)
                }
                Err(e) => {
                    println!("FAIL: {file}: {e}");
                    Ok(1)
                }
            }
        }
        _ => {
            println!("{USAGE}");
            Ok(if args.is_empty() { 0 } else { 2 })
        }
    }
}
