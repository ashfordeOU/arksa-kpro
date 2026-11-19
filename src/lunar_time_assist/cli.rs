// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! `arksa-kpro lunar-time ...`: the command line of [`super`].

use std::path::Path;

use kshana_pro::cli::{flag, has_flag, now_unix};

use super::assist::{self, AssistPackage, PackOptions, Requirements, ScoreOptions};
use super::registry::{self, RefScale, Registry};
use super::service::{self, AssistRequest};

pub const USAGE: &str = "arksa-kpro lunar-time registry [--registry <file>]
arksa-kpro lunar-time convert --definition <id> --at <YYYY-MM-DDTHH:MM:SS> [--reference utc|tai|tt|tcg|tdb|tcb|gpst] [--registry <file>]
arksa-kpro lunar-time spread --at <YYYY-MM-DDTHH:MM:SS> [--registry <file>]
arksa-kpro lunar-time impact --new <file> [--old <file>] --at <t1>[,<t2>...] [--tolerance-ns <ns>] [--out <dir>] [--sign-key-file <file>] [--signer <name>]
arksa-kpro lunar-time assist pack --sp3 <predicted.sp3> --options <pack.toml> --out <package.json> [--registry <file>] [--sign-key-file <file>] [--signer <name>]
arksa-kpro lunar-time assist check <package.json> [--requirements <req.toml>] [--trusted-key <hex>[,<hex>...]]
arksa-kpro lunar-time assist score <package.json> --reference <reference.sp3> --out <dir> [--options <score.toml>] [--sign-key-file <file>] [--signer <name>]
arksa-kpro lunar-time assist request --service <https://...> --request <request.json> --trusted-key <hex>[,<hex>...] --out <package.json> --license <token>

Lunar time definitions are undecided internationally: CGPM 2026 Resolution 6 left TCL versus a
scaled TL open, and no conventional W0_L exists. The registry (built in: version 1) is
PROVISIONAL, lists TCL and several TL candidates and LTC as an envelope of them, and has no
default: every conversion names its definition. A new registry file (status \"adopted\" with the
decision citation) replaces it without a code change; `impact` gives the re-qualification
evidence between two versions.

`assist` handles the ks-lunar-gnss-assist/1 package (a proposal, not a standard): long-validity
GNSS ephemeris and clocks with a per-satellite integrity bound. `pack` packs a predicted SP3 file
(no extrapolation); `check` checks it against requirements (defaults: >= 24 h validity, >= 60
satellites, GPS + Galileo, signed); `score` scores it against a reference product and signs the
result directory (Kshana Pro dossier signature) when a key is given; `request` asks the hosted
generator (the prediction method is not in this build).

A signing key is a file holding 64 hex characters (or ARKSA_STUDY_SIGNING_KEY).
Evidence class: MODELLED.";

fn load_registry(args: &[String], name: &str) -> Result<Registry, String> {
    match flag(args, name) {
        Some(p) if p != "builtin" => Registry::from_file(Path::new(&p)),
        _ => Ok(Registry::builtin()),
    }
}

fn signing_seed(args: &[String]) -> Result<Option<[u8; 32]>, String> {
    let key = match flag(args, "--sign-key-file") {
        Some(f) => Some(std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?),
        None => std::env::var("ARKSA_STUDY_SIGNING_KEY").ok(),
    };
    key.filter(|k| !k.trim().is_empty())
        .map(|k| kshana_pro::license::parse_key_hex(k.trim()).map_err(|e| e.to_string()))
        .transpose()
}

/// The current UTC as `YYYY-MM-DDTHH:MM:SSZ`.
fn utc_now() -> String {
    let e = assist::epoch_add_s(
        &kshana::rinex::EpochUtc {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0.0,
        },
        now_unix(),
    );
    format!("{}Z", assist::format_epoch(&e))
}

fn print_json<T: serde::Serialize>(v: &T) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(v).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

/// Run `lunar-time <args>` (args without the command name). Returns the exit code.
pub fn run(args: &[String]) -> Result<u8, String> {
    match args.first().map(String::as_str) {
        Some("registry") => {
            let r = load_registry(args, "--registry")?;
            print_json(&serde_json::json!({
                "registry_sha256": r.sha256(),
                "registry": r,
            }))?;
            Ok(0)
        }
        Some("convert") => {
            let r = load_registry(args, "--registry")?;
            let def = flag(args, "--definition")
                .ok_or("--definition <id> is required (there is no default definition)")?;
            let at = flag(args, "--at").ok_or("--at <YYYY-MM-DDTHH:MM:SS> is required")?;
            let reference = RefScale::parse(&flag(args, "--reference").unwrap_or("utc".into()))?;
            print_json(&r.lunar_offset(&def, reference, &at)?)?;
            Ok(0)
        }
        Some("spread") => {
            let r = load_registry(args, "--registry")?;
            let at = flag(args, "--at").ok_or("--at <YYYY-MM-DDTHH:MM:SS> is required")?;
            let (rows, spread) = registry::cross_definition_spread(&r, &at)?;
            print_json(&serde_json::json!({
                "registry": r.reference("*"),
                "notice": r.notice,
                "epoch_utc": at,
                "candidate_spread_s": spread,
                "definitions": rows,
                "evidence_class": "MODELLED",
            }))?;
            Ok(0)
        }
        Some("impact") => {
            let old = load_registry(args, "--old")?;
            let new = match flag(args, "--new") {
                Some(p) => Registry::from_file(Path::new(&p))?,
                None => return Err("--new <registry file> is required".into()),
            };
            let at = flag(args, "--at").ok_or("--at <t1>[,<t2>...] is required")?;
            let epochs: Vec<String> = at.split(',').map(|s| s.trim().to_string()).collect();
            let tol_ns: f64 = flag(args, "--tolerance-ns")
                .unwrap_or("1".into())
                .parse()
                .map_err(|_| "--tolerance-ns: a number")?;
            let imp = registry::change_impact(&old, &new, &epochs, tol_ns * 1e-9)?;
            match flag(args, "--out") {
                None => print_json(&imp)?,
                Some(out) => {
                    let dir = Path::new(&out);
                    std::fs::create_dir_all(dir).map_err(|e| format!("{out}: {e}"))?;
                    std::fs::write(
                        dir.join("impact.json"),
                        serde_json::to_string_pretty(&imp).map_err(|e| e.to_string())? + "\n",
                    )
                    .map_err(|e| e.to_string())?;
                    println!(
                        "impact: {} definition(s) to re-qualify: {}",
                        imp.requalify.len(),
                        imp.requalify.join(", ")
                    );
                    sign_dir(args, dir)?;
                }
            }
            Ok(0)
        }
        Some("assist") => assist_cmd(&args[1..]),
        Some(o) => Err(format!("unknown lunar-time subcommand '{o}'\n{USAGE}")),
        None => Err(USAGE.into()),
    }
}

fn sign_dir(args: &[String], dir: &Path) -> Result<(), String> {
    match signing_seed(args)? {
        Some(seed) => {
            let signer = flag(args, "--signer").unwrap_or_else(|| "Arksa".into());
            let p = kshana_pro::signed_dossier::sign(dir, &seed, &signer, &utc_now())
                .map_err(|e| e.to_string())?;
            println!("signed: {}", p.display());
        }
        None => println!("not signed: no signing key given"),
    }
    Ok(())
}

fn assist_cmd(args: &[String]) -> Result<u8, String> {
    match args.first().map(String::as_str) {
        Some("pack") => {
            let sp3_path = flag(args, "--sp3").ok_or("--sp3 <file> is required")?;
            let opt_path = flag(args, "--options").ok_or("--options <pack.toml> is required")?;
            let out = flag(args, "--out").ok_or("--out <package.json> is required")?;
            let sp3 = kshana::sp3::parse_sp3(&read(&sp3_path)?)
                .map_err(|e| format!("{sp3_path}: {e}"))?;
            let opt: PackOptions =
                toml::from_str(&read(&opt_path)?).map_err(|e| format!("{opt_path}: {e}"))?;
            let reg = match &opt.lunar_time_definition {
                Some(_) => Some(load_registry(args, "--registry")?),
                None => None,
            };
            let mut packed = assist::pack_from_sp3(&sp3, &opt, reg.as_ref())?;
            for (s, why) in &packed.dropped {
                eprintln!("dropped {s}: {why}");
            }
            match signing_seed(args)? {
                Some(seed) => {
                    let signer = flag(args, "--signer").unwrap_or_else(|| "Arksa".into());
                    packed.package.sign(&seed, &signer)?;
                }
                None => eprintln!("not signed: no signing key given"),
            }
            std::fs::write(
                &out,
                serde_json::to_string(&packed.package).map_err(|e| e.to_string())? + "\n",
            )
            .map_err(|e| format!("{out}: {e}"))?;
            println!(
                "package {}: {} satellites, validity {:.2} h, written to {out}",
                packed.package.package_id,
                packed.package.satellites.len(),
                packed.package.validity_hours()
            );
            Ok(0)
        }
        Some("check") => {
            let path = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .ok_or("`assist check` needs a package file")?;
            let pkg: AssistPackage =
                serde_json::from_str(&read(path)?).map_err(|e| format!("{path}: {e}"))?;
            let req: Requirements = match flag(args, "--requirements") {
                Some(p) => toml::from_str(&read(&p)?).map_err(|e| format!("{p}: {e}"))?,
                None => Requirements::default(),
            };
            let trusted: Vec<String> = flag(args, "--trusted-key")
                .map(|k| k.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default();
            let items = assist::check(&pkg, &req, &trusted);
            let pass = items.iter().all(|i| i.pass);
            print_json(&serde_json::json!({
                "package_id": pkg.package_id,
                "requirements": req,
                "items": items,
                "pass": pass,
                "evidence_class": "MODELLED",
            }))?;
            Ok(if pass { 0 } else { 1 })
        }
        Some("score") => {
            let path = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .ok_or("`assist score` needs a package file")?;
            let refp = flag(args, "--reference").ok_or("--reference <sp3> is required")?;
            let out = flag(args, "--out").ok_or("--out <dir> is required")?;
            let pkg: AssistPackage =
                serde_json::from_str(&read(path)?).map_err(|e| format!("{path}: {e}"))?;
            let ref_text = read(&refp)?;
            let reference =
                kshana::sp3::parse_sp3(&ref_text).map_err(|e| format!("{refp}: {e}"))?;
            let opt: ScoreOptions = match flag(args, "--options") {
                Some(p) => toml::from_str(&read(&p)?).map_err(|e| format!("{p}: {e}"))?,
                None => toml::from_str(&format!(
                    "reference_name = {:?}",
                    Path::new(&refp)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or(refp.clone())
                ))
                .map_err(|e| e.to_string())?,
            };
            use sha2::Digest;
            let ref_sha = hex::encode(sha2::Sha256::digest(ref_text.as_bytes()));
            let sc = assist::score(&pkg, &reference, &ref_sha, &opt)?;
            let seed = signing_seed(args)?;
            let signer = flag(args, "--signer").unwrap_or_else(|| "Arksa".into());
            let utc = utc_now();
            let signed = assist::write_score(
                &sc,
                Path::new(&out),
                seed.as_ref().map(|s| (s, signer.as_str(), utc.as_str())),
            )?;
            println!(
                "scorecard {}: {} samples, RMS {:.3} m, max z {:.2}; integrity: {}",
                sc.package_id, sc.samples, sc.rms_error_m, sc.max_z, sc.integrity.verdict
            );
            match signed {
                Some(p) => println!("signed: {}", p.display()),
                None => println!("not signed: no signing key given"),
            }
            Ok(0)
        }
        Some("request") => {
            let url = flag(args, "--service").ok_or("--service <https://...> is required")?;
            let reqp = flag(args, "--request").ok_or("--request <request.json> is required")?;
            let keys = flag(args, "--trusted-key")
                .or_else(|| kshana_pro::remote_verdict::builtin_trusted_keys().map(str::to_string))
                .ok_or("--trusted-key <hex> is required (the service's public key)")?;
            let out = flag(args, "--out").ok_or("--out <package.json> is required")?;
            let bearer = flag(args, "--license").unwrap_or_default();
            let req: AssistRequest =
                serde_json::from_str(&read(&reqp)?).map_err(|e| format!("{reqp}: {e}"))?;
            let t = kshana_pro::remote_verdict::CurlVerdictTransport { base_url: url };
            let (v, pkg) = service::request_package(&t, &bearer, &req, &keys)?;
            std::fs::write(
                &out,
                serde_json::to_string(&pkg).map_err(|e| e.to_string())? + "\n",
            )
            .map_err(|e| format!("{out}: {e}"))?;
            println!(
                "package {} ({} satellites) from {} (verdict {}, evidence class {}) written to {out}",
                pkg.package_id,
                pkg.satellites.len(),
                v.service,
                v.verdict_sha256,
                v.evidence_class
            );
            Ok(0)
        }
        _ if has_flag(args, "--help") => Err(USAGE.into()),
        Some(o) => Err(format!("unknown assist subcommand '{o}'\n{USAGE}")),
        None => Err(USAGE.into()),
    }
}
