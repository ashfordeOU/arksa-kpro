// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! `arksa-kpro rideshare-id <identify|catalogue|norad|synth>`, under the mission feature
//! (`pro-mission-dossier`, a token with the `arksa` claim).

use std::path::Path;
use std::process::ExitCode;

use kshana::api::KshanaError;
use kshana_pro::cli::{flag, now_unix, resolve_entitlement};
use kshana_pro::extension::ExtensionCommand;

use super::{bad, catalogue, norad, synthetic};
use crate::packs::rideshare_id_pack as pack;

/// Usage text for `arksa-kpro rideshare-id`.
pub const USAGE: &str = "arksa-kpro rideshare-id — which catalogued object of a launch is ours? (MODELLED)\n\
\n\
  arksa-kpro rideshare-id identify <scenario.toml> --out <dir> [--sign-key-file <file>] [--signer <name>]\n\
      Rank the candidate element sets (TLE with Alpha-5, OMM KVN/XML/JSON) against the observation\n\
      CSV the scenario names: verdict.json, candidates.csv, residuals.csv, report.md; signed with a\n\
      key. Exit 0 when IDENTIFIED, 1 for any other verdict (AMBIGUOUS, NO_CONSISTENT_CANDIDATE, ...)\n\
  arksa-kpro rideshare-id catalogue <file>...\n\
      Read TLE/OMM files and list the objects: catalogue number, TLE field (Alpha-5) or OMM only, epoch\n\
  arksa-kpro rideshare-id norad <encode <number>|decode <field>>\n\
      Catalogue number to the five-column TLE field (Alpha-5 above 99999) and back\n\
  arksa-kpro rideshare-id synth --out <dir> [--spec <deployment.toml>]\n\
      Write a synthetic deployment (candidates.tle, candidates.omm, observations.csv, scenario.toml)\n\
      with a known answer (answer.json)\n\
\n\
Observation CSV columns: time_utc,station,lat_deg,lon_deg,alt_m,type,value,sigma[,carrier_hz][,pass];\n\
types doppler_hz, range_m, range_rate_m_s, az_deg, el_deg. A signing key is a file of 64 hex\n\
characters (or ARKSA_STUDY_SIGNING_KEY); check a signed directory with `arksa-kpro dossier verify`.\n\
Every command takes [--license <token>] or, in a debug build, [--dev]. See docs/RIDESHARE-ID.md.";

/// The command, for [`crate::commands::all`].
pub fn command() -> ExtensionCommand {
    ExtensionCommand {
        name: "rideshare-id",
        usage: "arksa-kpro rideshare-id <identify|catalogue|norad|synth> ... [--license <token>] [--dev]   Which object of a launch is ours: rank TLE/OMM candidates (six-digit catalogue numbers, Alpha-5) against ground-station passes, signed verdict (`arksa-kpro rideshare-id` for details)",
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
    if let Err(e) = crate::entitlement::ensure_rideshare_id_entitled(&ent, now) {
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

/// Positional arguments after the subcommand, skipping flags and their values.
fn positionals(args: &[String]) -> Vec<String> {
    let takes_value = [
        "--out",
        "--spec",
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

fn signing_seed(args: &[String]) -> Result<Option<[u8; 32]>, KshanaError> {
    let key = match flag(args, "--sign-key-file") {
        Some(f) => Some(std::fs::read_to_string(&f).map_err(|e| bad(format!("{f}: {e}")))?),
        None => std::env::var("ARKSA_STUDY_SIGNING_KEY").ok(),
    };
    key.filter(|k| !k.trim().is_empty())
        .map(|k| kshana_pro::license::parse_key_hex(k.trim()))
        .transpose()
}

/// Run a `rideshare-id` subcommand (`args[0]`); returns the exit code.
pub fn run_inner(args: &[String], now_unix: i64) -> Result<u8, KshanaError> {
    let pos = positionals(args);
    match args[0].as_str() {
        "identify" => {
            let path = pos
                .first()
                .ok_or_else(|| bad(format!("identify needs a scenario file\n\n{USAGE}")))?;
            let out = flag(args, "--out").ok_or_else(|| bad("identify needs --out <dir>"))?;
            let p = Path::new(path);
            let text = std::fs::read_to_string(p).map_err(|e| bad(format!("{path}: {e}")))?;
            let mut s: pack::RideshareIdScenario =
                toml::from_str(&text).map_err(|e| bad(format!("invalid scenario: {e}")))?;
            s.resolve_files(p.parent().unwrap_or(Path::new(".")))?;
            let st = pack::run_study(&s)?;
            let seed = signing_seed(args)?;
            let signer = flag(args, "--signer").unwrap_or_else(|| "Arksa".into());
            let utc = crate::decision::time::fmt_utc(now_unix);
            let signed = pack::write_study(
                &st,
                Path::new(&out),
                seed.as_ref().map(|s| (s, signer.as_str(), utc.as_str())),
            )?;
            print!("{}", pack::summary(&st.doc));
            match signed {
                Some(p) => println!("signed: {}", p.display()),
                None => println!("not signed: no signing key given"),
            }
            Ok(
                if st.identification.verdict == super::identify::Verdict::Identified {
                    0
                } else {
                    1
                },
            )
        }
        "catalogue" => {
            if pos.is_empty() {
                return Err(bad("catalogue needs at least one TLE or OMM file"));
            }
            let mut all = Vec::new();
            for f in &pos {
                let t = std::fs::read_to_string(f).map_err(|e| bad(format!("{f}: {e}")))?;
                all.extend(catalogue::from_text(&t, f).map_err(bad)?);
            }
            let (c, warnings) = catalogue::merge(all);
            println!("catalogue_number,tle_field,name,object_id,source,epoch_utc,checksum_ok");
            for x in &c {
                let i = x.info();
                println!(
                    "{},{},{},{},{},{},{}",
                    i.catalogue_number,
                    i.tle_field.unwrap_or_else(|| "OMM only".into()),
                    i.name.replace(',', " "),
                    i.object_id,
                    i.source,
                    i.epoch_utc,
                    i.checksum_ok
                );
            }
            for w in warnings {
                eprintln!("warning: {w}");
            }
            Ok(0)
        }
        "norad" => match (pos.first().map(String::as_str), pos.get(1)) {
            (Some("encode"), Some(n)) => {
                let n: u32 = n
                    .parse()
                    .map_err(|_| bad(format!("'{n}' is not a catalogue number")))?;
                println!("{}", norad::encode(n).map_err(bad)?);
                Ok(0)
            }
            (Some("decode"), Some(f)) => {
                println!("{}", norad::decode(f).map_err(bad)?);
                Ok(0)
            }
            _ => Err(bad(
                "usage: rideshare-id norad <encode <number>|decode <field>>",
            )),
        },
        "synth" => {
            let out = flag(args, "--out").ok_or_else(|| bad("synth needs --out <dir>"))?;
            let spec: synthetic::DeploymentSpec = match flag(args, "--spec") {
                Some(f) => {
                    let t = std::fs::read_to_string(&f).map_err(|e| bad(format!("{f}: {e}")))?;
                    toml::from_str(&t).map_err(|e| bad(format!("{f}: {e}")))?
                }
                None => synthetic::DeploymentSpec::default(),
            };
            let d = synthetic::generate(&spec).map_err(bad)?;
            write_synthetic(&d, &spec, Path::new(&out))?;
            println!(
                "synthetic deployment of {} objects ({} OMM only) with {} measurements of object {} in {out}",
                d.catalogue_numbers.len(),
                d.omm_only.len(),
                d.observations.obs.len(),
                d.target_catalogue_number
            );
            Ok(0)
        }
        other => {
            println!("{USAGE}");
            Err(bad(format!("unknown rideshare-id command '{other}'")))
        }
    }
}

/// Write a synthetic deployment as files a user would have, plus the answer.
pub fn write_synthetic(
    d: &synthetic::Deployment,
    spec: &synthetic::DeploymentSpec,
    dir: &Path,
) -> Result<(), KshanaError> {
    std::fs::create_dir_all(dir).map_err(|e| bad(format!("{}: {e}", dir.display())))?;
    let w = |name: &str, text: &str| {
        std::fs::write(dir.join(name), text).map_err(|e| bad(format!("{name}: {e}")))
    };
    let mut files = Vec::new();
    if !d.tle_text.is_empty() {
        w("candidates.tle", &d.tle_text)?;
        files.push("\"candidates.tle\"");
    }
    w("candidates.omm", &d.omm_text)?;
    files.push("\"candidates.omm\"");
    w("observations.csv", &d.observations_csv)?;
    w(
        "scenario.toml",
        &format!(
            "kind = \"pro-rideshare-id\"\ntitle = \"Synthetic deployment of {} objects\"\nobservations_file = \"observations.csv\"\ncandidate_files = [{}]\n",
            d.catalogue_numbers.len(),
            files.join(", ")
        ),
    )?;
    let answer = serde_json::json!({
        "synthetic": true,
        "target_catalogue_number": d.target_catalogue_number,
        "catalogue_numbers": d.catalogue_numbers,
        "omm_only": d.omm_only,
        "spec": spec,
    });
    w(
        "answer.json",
        &(serde_json::to_string_pretty(&answer).unwrap_or_default() + "\n"),
    )
}
