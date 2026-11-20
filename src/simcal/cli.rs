// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! `arksa-kpro simcal <command>`: the command-line front end. The binary resolves the
//! licence and checks the mission feature first, then calls [`run`].

use std::path::Path;

use kshana_pro::cli::flag;

use super::lineage::{self, RecalOptions};
use super::mission::Mission;
use super::plugin::{self, Context, SimPlugin};
use super::residuals;
use crate::license::Entitlement;

/// Usage text for `arksa-kpro simcal`.
pub const USAGE: &str = "arksa-kpro simcal — mission performance simulator, compared with telemetry and recalibrated\n\
\n\
  arksa-kpro simcal plugins\n\
      The simulator plugin kinds this build knows\n\
  arksa-kpro simcal residuals <mission.toml> [--out <dir>]\n\
      Simulator-versus-telemetry residuals per observable (bias with its uncertainty, RMS,\n\
      reduced chi-square, verdict against tolerance); with --out, residuals.json and residuals.csv\n\
  arksa-kpro simcal recalibrate <mission.toml> --lineage <dir> [--service <https://...>]\n\
                    [--trusted-key <hex>[,<hex>]] [--verdict-license <token>]\n\
                    [--sign-key-file <file>] [--signer <name>] [--utc <iso>]\n\
      Send each hosted plugin's model and telemetry to its hosted calibration method (selfcal:\n\
      the plugin's scenario; calibrate: named parameters with an expression per observable or\n\
      design runs of the plugin's own simulator), accept only a verdict signed by a trusted\n\
      service key for that method, and append a hash-chained entry to the calibration lineage\n\
      in <dir> (signed with a key)\n\
  arksa-kpro simcal verify <lineage-dir> [--trusted-key <hex>[,<hex>]] [--trust <signer public key>]\n\
      Re-check the lineage: chain, hashes, every verdict's signature and outcome, the directory signature\n\
\n\
The service is --service or KSHANA_VERDICT_URL; its key --trusted-key, KSHANA_VERDICT_KEY or the key\n\
built into the release. The verdict licence token is --verdict-license or KSHANA_VERDICT_LICENSE.\n\
A signing key is a file of 64 hex characters (or ARKSA_STUDY_SIGNING_KEY).\n\
\n\
Every command takes [--license <token>] or, in a debug build, [--dev]. See docs/SIM-RECALIBRATION.md.";

fn opt_or_env(args: &[String], name: &str, env: &str) -> Option<String> {
    flag(args, name)
        .or_else(|| std::env::var(env).ok())
        .filter(|s| !s.trim().is_empty())
}

fn trusted_keys(args: &[String]) -> Option<String> {
    opt_or_env(args, "--trusted-key", "KSHANA_VERDICT_KEY")
        .or_else(|| kshana_pro::remote_verdict::builtin_trusted_keys().map(str::to_string))
}

fn utc_now(now_unix: i64) -> String {
    crate::packs::spaceborne_rx_exposure_pack::fmt_utc(kshana::jd2::Jd2::new(
        2_440_587.5 + now_unix as f64 / 86_400.0,
    ))
}

/// Load a mission and build its plugins.
pub fn load(
    path: &Path,
    ent: &Entitlement,
    now_unix: i64,
) -> Result<(Mission, Vec<Box<dyn SimPlugin>>), String> {
    let m = Mission::load(path)?;
    let ctx = Context {
        dir: &m.dir,
        entitlement: ent,
        now_unix,
    };
    let plugins = m
        .file
        .plugins
        .iter()
        .map(|s| plugin::build(s, &ctx))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((m, plugins))
}

fn positional(args: &[String], what: &str) -> Result<String, String> {
    match args.get(1) {
        Some(p) if !p.starts_with("--") => Ok(p.clone()),
        _ => Err(format!("{what} is required\n{USAGE}")),
    }
}

/// Run `arksa-kpro simcal <args>` (the first argument is the subcommand) with an
/// entitlement already checked for the feature. Returns the exit code.
pub fn run(args: &[String], ent: &Entitlement, now_unix: i64) -> Result<u8, String> {
    match args.first().map(String::as_str) {
        Some("plugins") => {
            for (k, _) in plugin::KINDS {
                println!("{k}");
            }
            Ok(0)
        }
        Some("residuals") => {
            let path = positional(args, "<mission.toml>")?;
            let (m, plugins) = load(Path::new(&path), ent, now_unix)?;
            let (rep, samples) = residuals::report(&m, &plugins);
            for p in &rep.plugins {
                println!("{} ({}): {}", p.plugin, p.kind, p.verdict.as_str());
                for o in &p.observables {
                    match (o.bias, o.rms) {
                        (Some(b), Some(r)) => println!(
                            "  {}: {} of {} compared, bias {:.6} (u {}), RMS {:.6}, tolerance {} -> {}",
                            o.observable,
                            o.compared,
                            o.samples,
                            b,
                            o.bias_u.map(|u| format!("{u:.6}")).unwrap_or_else(|| "n/a".into()),
                            r,
                            o.tolerance,
                            o.verdict.as_str()
                        ),
                        _ => println!(
                            "  {}: {} samples -> {}",
                            o.observable,
                            o.samples,
                            o.verdict.as_str()
                        ),
                    }
                }
            }
            if let Some(out) = flag(args, "--out") {
                let out = Path::new(&out);
                std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
                let json = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())? + "\n";
                std::fs::write(out.join("residuals.json"), json)
                    .map_err(|e| format!("residuals.json: {e}"))?;
                std::fs::write(out.join("residuals.csv"), residuals::samples_csv(&samples))
                    .map_err(|e| format!("residuals.csv: {e}"))?;
                println!("written: {}", out.display());
            }
            Ok(0)
        }
        Some("recalibrate") => {
            let path = positional(args, "<mission.toml>")?;
            let dir = flag(args, "--lineage").ok_or("--lineage <dir> is required")?;
            let url = opt_or_env(args, "--service", "KSHANA_VERDICT_URL")
                .ok_or("--service <https://...> (or KSHANA_VERDICT_URL) is required")?;
            let keys = trusted_keys(args).ok_or(
                "--trusted-key <hex> (or KSHANA_VERDICT_KEY) is required: the service's public key",
            )?;
            let bearer =
                opt_or_env(args, "--verdict-license", "KSHANA_VERDICT_LICENSE").unwrap_or_default();
            let key = match flag(args, "--sign-key-file") {
                Some(f) => Some(std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?),
                None => std::env::var("ARKSA_STUDY_SIGNING_KEY").ok(),
            };
            let seed = key
                .filter(|k| !k.trim().is_empty())
                .map(|k| kshana_pro::license::parse_key_hex(k.trim()).map_err(|e| e.to_string()))
                .transpose()?;
            let signer = flag(args, "--signer").unwrap_or_else(|| "Arksa".into());
            let utc = flag(args, "--utc").unwrap_or_else(|| utc_now(now_unix));
            let (m, plugins) = load(Path::new(&path), ent, now_unix)?;
            let t = kshana_pro::remote_verdict::CurlVerdictTransport { base_url: url };
            let e = lineage::recalibrate_mission(
                &m,
                &plugins,
                Path::new(&dir),
                &RecalOptions {
                    transport: &t,
                    bearer: &bearer,
                    trusted_verdict_keys: &keys,
                    utc: &utc,
                    signer: seed.as_ref().map(|s| (s, signer.as_str())),
                },
            )?;
            for r in &e.plugins {
                println!("{} ({}): {}", r.plugin, r.kind, r.status.as_str());
                if let Some(h) = &r.hosted {
                    for q in &h.outcome.parameters {
                        println!(
                            "  {} = {} (standard uncertainty {}, {})",
                            q.name, q.value, q.std_err, h.evidence_class
                        );
                    }
                    let flags = lineage::flags_note(&h.outcome);
                    if !flags.is_empty() {
                        println!("  {}", flags.trim());
                    }
                }
            }
            println!(
                "lineage entry {} ({}) appended to {dir}",
                e.seq, e.entry_sha256
            );
            if seed.is_none() {
                println!("not signed: no signing key given");
            }
            Ok(0)
        }
        Some("verify") => {
            let dir = positional(args, "<lineage-dir>")?;
            let keys = trusted_keys(args).unwrap_or_default();
            let trust = match flag(args, "--trust") {
                Some(k) => {
                    vec![kshana_pro::license::parse_key_hex(k.trim()).map_err(|e| e.to_string())?]
                }
                None => vec![],
            };
            let c = lineage::verify_dir(Path::new(&dir), &keys, &trust)?;
            println!(
                "{} entries, head {}, {}",
                c.entries,
                c.head.as_deref().unwrap_or("-"),
                match &c.signer {
                    Some(s) => format!("signed by {s}"),
                    None => "not signed".into(),
                }
            );
            if c.passed() {
                println!("lineage checks");
                Ok(0)
            } else {
                for p in &c.problems {
                    println!("problem: {p}");
                }
                Ok(1)
            }
        }
        Some(o) => Err(format!("unknown simcal subcommand '{o}'\n{USAGE}")),
        None => Err(USAGE.into()),
    }
}
