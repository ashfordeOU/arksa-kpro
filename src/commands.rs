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
