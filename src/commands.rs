// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The Arksa-exclusive commands, added to Kshana Pro's command line through the
//! registration hook ([`kshana_pro::extension::ExtensionCommand`]). Each resolves the
//! entitlement as every Pro command does (`--dev` in a debug build; else `--license`; else
//! the activated licence or Evaluation) and requires its Arksa-exclusive feature through
//! [`kshana_pro::license::check`] before it runs.

use std::process::ExitCode;

use kshana_pro::cli::{flag, has_flag, now_unix, require_feature, resolve_entitlement};
use kshana_pro::extension::ExtensionCommand;

/// Every Arksa-exclusive command, with its usage line.
pub fn all() -> Vec<ExtensionCommand> {
    vec![
        ExtensionCommand {
            name: "mrd",
            usage: "arksa-kpro mrd                                                Emit the machine-checked D1.4 MRD FoM-mapping (Markdown)",
            run: mrd_cmd,
        },
        ExtensionCommand {
            name: "navdt",
            usage: "arksa-kpro navdt <scenario.toml> [--subject <s>]              NAV-DT deterministic twin bundle, JSON (A10)",
            run: navdt_cmd,
        },
        ExtensionCommand {
            name: "spacestream",
            usage: "arksa-kpro spacestream [--schema]                             SpaceStream codec goldens / ICD schema (A09)",
            run: spacestream_cmd,
        },
        ExtensionCommand {
            name: "mbse",
            usage: "arksa-kpro mbse <import|trace|vcrm|sysml|impact|pack> ... [--license <token>] [--dev]   Requirements, traceability, VCRM, SysML v2, change impact, evidence pack (`arksa-kpro mbse` for details)",
            run: mbse_cmd,
        },
        ExtensionCommand {
            name: "design",
            usage: "arksa-kpro design <study.toml> --out <dir> [--threads <n>] [--license <token>] [--dev]   Pareto front of a design space over any open kind (`arksa-kpro design` for details)",
            run: design_cmd,
        },
        ExtensionCommand {
            name: "coexist",
            usage: "arksa-kpro coexist <study.toml> --out <dir> [--license <token>] [--dev]   Whole signal plans and constellations against the open global navigation satellite system (GNSS) signals, both directions (`arksa-kpro coexist` for details)",
            run: coexist_cmd,
        },
        ExtensionCommand {
            name: "mission",
            usage: "arksa-kpro mission <build|check> ... [--license <token>] [--dev]   Requirements to runs to verification matrix to a PDF dossier, and its re-check (`arksa-kpro mission` for details)",
            run: mission_cmd,
        },
    ]
}

/// Print a library result as the command's exit code.
fn finish(r: Result<u8, impl std::fmt::Display>) -> ExitCode {
    match r {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn wants_help(args: &[String]) -> bool {
    args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h")
}

/// `mrd`: the D1.4 Mission Requirements Document FoM mapping, rendered from the
/// machine-checked table (A03). `tests/mrd_deliverable_in_sync.rs` keeps the committed
/// document equal to this output.
fn mrd_cmd(args: &[String]) -> ExitCode {
    if let Err(code) = require_feature(args, "pro-mrd") {
        return code;
    }
    print!("{}", crate::mrd::to_markdown());
    ExitCode::SUCCESS
}

/// `navdt <scenario.toml>`: the NAV-DT deterministic twin bundle over a scenario (A10).
fn navdt_cmd(args: &[String]) -> ExitCode {
    let path = match args.get(1) {
        Some(p) if !p.starts_with("--") => p,
        _ => {
            eprintln!("error: `navdt` needs a scenario file");
            return ExitCode::FAILURE;
        }
    };
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(code) = require_feature(args, "pro-navdt") {
        return code;
    }
    let subject = flag(args, "--subject").unwrap_or_else(|| "quantum".to_string());
    match crate::twin::navdt::run_navdt_bundle(&src, &subject) {
        Ok(b) => {
            println!("{}", b.to_json());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `spacestream [--schema]`: the codec's golden messages, or the ICD JSON schema (A09).
fn spacestream_cmd(args: &[String]) -> ExitCode {
    use crate::connectors::spacestream;
    if let Err(code) = require_feature(args, "pro-spacestream") {
        return code;
    }
    if has_flag(args, "--schema") {
        println!("{}", spacestream::json_schema());
    } else {
        let msgs: Vec<serde_json::Value> = spacestream::golden_messages()
            .into_iter()
            .map(|(name, m)| {
                serde_json::json!({
                    "name": name,
                    "message": serde_json::from_str::<serde_json::Value>(&m.to_json())
                        .unwrap_or(serde_json::Value::Null),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&msgs).unwrap_or_default()
        );
    }
    ExitCode::SUCCESS
}

/// `mbse <command>`: requirements, traceability, VCRM, SysML v2, change impact and the
/// evidence pack (feature `pro-mbse`).
fn mbse_cmd(args: &[String]) -> ExitCode {
    let args = &args[1..];
    if wants_help(args) {
        println!("{}", crate::mbse::cli::USAGE);
        return ExitCode::SUCCESS;
    }
    let now = now_unix();
    let ent = match resolve_entitlement(args, now) {
        Ok(e) => e,
        Err(code) => return code,
    };
    if let Err(e) = crate::entitlement::ensure_mbse_entitled(&ent, now) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    finish(crate::mbse::cli::run(args))
}

/// `design <study.toml> --out <dir>`: run a design study and write its result, chart,
/// report and the front's scenario files (feature `pro-design-optimise`).
fn design_cmd(args: &[String]) -> ExitCode {
    let args = &args[1..];
    if wants_help(args) {
        println!("{}", crate::design::cli::USAGE);
        return ExitCode::SUCCESS;
    }
    if let Err(code) = require_feature(args, crate::design::KIND) {
        return code;
    }
    finish(crate::design::cli::run(args))
}

/// `coexist <study.toml> --out <dir>`: whole signal plans and constellations against the
/// open GNSS signals (feature `pro-spectrum-coexistence`).
fn coexist_cmd(args: &[String]) -> ExitCode {
    let args = &args[1..];
    if wants_help(args) {
        println!("{}", crate::spectrum_coexistence::cli::USAGE);
        return ExitCode::SUCCESS;
    }
    if let Err(code) = require_feature(args, crate::spectrum_coexistence::KIND) {
        return code;
    }
    finish(crate::spectrum_coexistence::cli::run(args))
}

/// `mission <build|check>`: requirements to runs to verification matrix to a PDF dossier,
/// and its re-check (feature `pro-mission-dossier`). A Pro or Arksa kind inside a
/// programme still needs its own entitlement.
fn mission_cmd(args: &[String]) -> ExitCode {
    let args = &args[1..];
    if wants_help(args) {
        println!("{}", crate::mission::cli::USAGE);
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
    finish(crate::mission::cli::run(args, &ent, now))
}

/// The `rx-exposure` command (roadmap 0.42 F4). Registered only when the signed tier
/// manifest lists its feature ([`crate::PENDING_MANIFEST_FEATURES`]).
pub fn rx_exposure_command() -> ExtensionCommand {
    ExtensionCommand {
        name: "rx-exposure",
        usage: "arksa-kpro rx-exposure <scenario.toml> --out <dir> [--sign-key-file <hex seed file>] [--signer <name>] [--license <token>] [--dev]   Spaceborne GNSS receiver interference exposure study, signed",
        run: rx_exposure_cmd,
    }
}

const RX_EXPOSURE_USAGE: &str =
    "arksa-kpro rx-exposure <scenario.toml> --out <dir> [--sign-key-file <file>] [--signer <name>]

Runs a pro-spaceborne-rx-exposure scenario (files it names are read relative to the scenario's
directory) and writes study.json, passes.csv and report.md to <dir>. With a signing key (a file
holding 64 hex characters, or ARKSA_STUDY_SIGNING_KEY) the directory is signed as a whole
(signature.json, Kshana Pro dossier signature; check it with `arksa-kpro dossier verify`).
Without one the study is written unsigned and the command says so.";

fn rx_exposure_cmd(args: &[String]) -> ExitCode {
    use crate::packs::spaceborne_rx_exposure_pack as rx;
    let args = &args[1..];
    if wants_help(args) {
        println!("{RX_EXPOSURE_USAGE}");
        return ExitCode::SUCCESS;
    }
    if let Err(code) = require_feature(args, rx::KIND) {
        return code;
    }
    let run = || -> Result<u8, String> {
        let path = std::path::Path::new(&args[0]);
        let out = flag(args, "--out").ok_or("--out <dir> is required")?;
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut s: rx::SpaceborneRxExposureScenario =
            toml::from_str(&text).map_err(|e| format!("invalid scenario: {e}"))?;
        s.resolve_files(path.parent().unwrap_or(std::path::Path::new(".")))
            .map_err(|e| e.to_string())?;
        let doc = rx::run_study(&s).map_err(|e| e.to_string())?;
        let key = match flag(args, "--sign-key-file") {
            Some(f) => Some(std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?),
            None => std::env::var("ARKSA_STUDY_SIGNING_KEY").ok(),
        };
        let seed = key
            .filter(|k| !k.trim().is_empty())
            .map(|k| kshana_pro::license::parse_key_hex(k.trim()).map_err(|e| e.to_string()))
            .transpose()?;
        let signer = flag(args, "--signer").unwrap_or_else(|| "Arksa".into());
        let utc = rx::fmt_utc(kshana::jd2::Jd2::new(
            2_440_587.5 + now_unix() as f64 / 86_400.0,
        ));
        let signed = rx::write_study(
            &doc,
            std::path::Path::new(&out),
            seed.as_ref().map(|s| (s, signer.as_str(), utc.as_str())),
        )
        .map_err(|e| e.to_string())?;
        println!("{}", rx::summary(&doc));
        match signed {
            Some(p) => println!("signed: {}", p.display()),
            None => println!("not signed: no signing key given"),
        }
        Ok(0)
    };
    finish(run())
}

/// The `conjunction` command (conjunction-assessment validation). Registered only when the
/// signed tier manifest lists its feature ([`crate::PENDING_MANIFEST_FEATURES`]).
pub fn conjunction_command() -> ExtensionCommand {
    ExtensionCommand {
        name: "conjunction",
        usage: "arksa-kpro conjunction <scenario.toml> --out <dir> [--sign-key-file <hex seed file>] [--signer <name>] [--license <token>] [--dev]   Conjunction-assessment validation (checks, scorecard against a reference set, cross-provider comparison, covariance realism), signed (`arksa-kpro conjunction` for details)",
        run: conjunction_cmd,
    }
}

const CONJUNCTION_USAGE: &str =
    "arksa-kpro conjunction <scenario.toml> --out <dir> [--sign-key-file <file>] [--signer <name>]
arksa-kpro conjunction ingest <file>... [--format cdm|results-table] [--covariance-unit km2|m2]

The first form runs a pro-conjunction-validation scenario (files it names are read relative to
the scenario's directory) and writes study.json, events.csv and report.md to <dir>. With a signing
key (a file holding 64 hex characters, or ARKSA_STUDY_SIGNING_KEY) the directory is signed as a
whole (signature.json, Kshana Pro dossier signature; check it with `arksa-kpro dossier verify`).
Without one the study is written unsigned and the command says so.

`ingest` reads CCSDS CDMs (KVN or XML) or a conjunction results table (the TraCSS verification
dataset's documented columns) and prints the normalised events as JSON.

Evidence class: MODELLED. A reference set is not ground truth; nothing here validates or certifies
a provider.";

fn conjunction_cmd(args: &[String]) -> ExitCode {
    use crate::packs::conjunction_validation_pack as cv;
    let args = &args[1..];
    if wants_help(args) {
        println!("{CONJUNCTION_USAGE}");
        return ExitCode::SUCCESS;
    }
    if let Err(code) = require_feature(args, cv::KIND) {
        return code;
    }
    if args[0] == "ingest" {
        let run = || -> Result<u8, String> {
            let unit = match flag(args, "--covariance-unit").as_deref() {
                None | Some("km2") => crate::conjunction::answer_key::CovUnit::Km2,
                Some("m2") => crate::conjunction::answer_key::CovUnit::M2,
                Some(o) => return Err(format!("--covariance-unit {o}: km2 or m2")),
            };
            let format = flag(args, "--format");
            let mut skip = false;
            let mut out = Vec::new();
            for a in &args[1..] {
                if skip {
                    skip = false;
                    continue;
                }
                if a.starts_with("--") {
                    skip = matches!(a.as_str(), "--format" | "--covariance-unit" | "--license");
                    continue;
                }
                let text = std::fs::read_to_string(a).map_err(|e| format!("{a}: {e}"))?;
                let is_table = match format.as_deref() {
                    Some("results-table") | Some("csv") => true,
                    Some("cdm") => false,
                    Some(o) => return Err(format!("--format {o}: cdm or results-table")),
                    None => crate::conjunction::answer_key::looks_like(&text),
                };
                let ev = if is_table {
                    crate::conjunction::answer_key::read(std::io::Cursor::new(text), unit, None)
                } else {
                    crate::conjunction::cdm::events(&text)
                }
                .map_err(|e| format!("{a}: {e}"))?;
                out.push(serde_json::json!({"file": a, "events": ev}));
            }
            if out.is_empty() {
                return Err("`conjunction ingest` needs at least one file".into());
            }
            let mut v = serde_json::json!(out);
            crate::conjunction::sanitise(&mut v);
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            Ok(0)
        };
        return finish(run());
    }
    let run = || -> Result<u8, String> {
        let path = std::path::Path::new(&args[0]);
        let out = flag(args, "--out").ok_or("--out <dir> is required")?;
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut s: cv::ConjunctionValidationScenario =
            toml::from_str(&text).map_err(|e| format!("invalid scenario: {e}"))?;
        s.resolve_files(path.parent().unwrap_or(std::path::Path::new(".")))
            .map_err(|e| e.to_string())?;
        let study = cv::run_study(&s).map_err(|e| e.to_string())?;
        let key = match flag(args, "--sign-key-file") {
            Some(f) => Some(std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?),
            None => std::env::var("ARKSA_STUDY_SIGNING_KEY").ok(),
        };
        let seed = key
            .filter(|k| !k.trim().is_empty())
            .map(|k| kshana_pro::license::parse_key_hex(k.trim()).map_err(|e| e.to_string()))
            .transpose()?;
        let signer = flag(args, "--signer").unwrap_or_else(|| "Arksa".into());
        let utc = crate::conjunction::fmt_utc(now_unix() as f64 - 946_684_800.0);
        let signed = cv::write_study(
            &study,
            std::path::Path::new(&out),
            seed.as_ref().map(|s| (s, signer.as_str(), utc.as_str())),
        )
        .map_err(|e| e.to_string())?;
        println!("{}", cv::summary(&study.doc));
        match signed {
            Some(p) => println!("signed: {}", p.display()),
            None => println!("not signed: no signing key given"),
        }
        Ok(0)
    };
    finish(run())
}
