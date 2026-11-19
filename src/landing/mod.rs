// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # Landing-navigation fault-handling verification
//!
//! Independent, pre-registered verification of how a lander's navigation filter handles
//! its descent sensors (radar altimeters, laser rangefinders): research note 13, finding 1
//! (four of the last five commercial lunar landing attempts had trouble traced to altitude
//! sensing or its handling).
//!
//! 1. [`campaign`]: the campaign file (system under test, a synthetic vertical descent,
//!    the sensors, the pass bars and the fault scenarios);
//! 2. [`faults`]: the fault library and the generator of the sensor streams the provider
//!    feeds to its filter, with the truth;
//! 3. `landing-nav scenarios` writes the streams and a pre-registration (campaign hash,
//!    bars, stream hashes), signed, before any log exists;
//! 4. [`log`]: the documented interface for the filter's logged outputs;
//! 5. [`verify`]: the checks and the signed verdict.
//!
//! **Verification only.** Nothing here estimates a lander's state or navigates: there is
//! no terrain-relative or map-matching navigation, no guidance and no sensor fusion. The
//! harness corrupts a known truth and compares the provider's logged outputs with it.
//! Every verdict is **MODELLED**: the descent and the sensor faults are synthetic, and the
//! filter's behaviour is as the provider logged it.

pub mod campaign;
pub mod cli;
pub mod faults;
pub mod log;
pub mod verify;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mbse::evidence::sha256_hex;
use kshana::api::KshanaError;

/// Identifies `verdict.json`.
pub const VERDICT_SCHEMA: &str = "arksa-landing-nav-verdict/1";
/// Identifies `preregistration.json`.
pub const PREREG_SCHEMA: &str = "arksa-landing-nav-preregistration/1";
/// The honesty label of every output.
pub const LABEL: &str = "MODELLED - a synthetic descent with injected descent-sensor faults; verdicts on the navigation filter's outputs as the provider logged them (test setup as declared). Not flight qualification, not a certification and not acceptance by NASA, ESA or any authority.";

pub(crate) fn err(msg: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(msg.into())
}

/// One scenario in the pre-registration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreregScenario {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub incident: Option<String>,
    pub samples: usize,
    pub stream_file: String,
    pub stream_sha256: String,
    pub truth_file: String,
    pub truth_sha256: String,
}

/// `preregistration.json`: what is fixed before any log is seen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preregistration {
    pub schema: String,
    pub label: String,
    pub campaign_title: String,
    pub campaign_sha256: String,
    pub system_under_test: String,
    pub system_version: String,
    pub touchdown_s: f64,
    pub bars: campaign::Bars,
    pub generator: String,
    pub scenarios: Vec<PreregScenario>,
}

/// The streams and truth of every scenario, and the pre-registration over them.
pub fn prepare(
    c: &campaign::Campaign,
    campaign_bytes: &[u8],
) -> (Preregistration, Vec<faults::Generated>) {
    let mut gens = Vec::new();
    let mut scenarios = Vec::new();
    for sc in &c.scenarios {
        let g = faults::generate(c, sc);
        scenarios.push(PreregScenario {
            id: sc.id.clone(),
            title: sc.title.clone(),
            incident: sc.incident.clone(),
            samples: g.samples,
            stream_file: format!("streams/{}.csv", sc.id),
            stream_sha256: sha256_hex(g.stream_csv.as_bytes()),
            truth_file: format!("truth/{}.csv", sc.id),
            truth_sha256: sha256_hex(g.truth_csv.as_bytes()),
        });
        gens.push(g);
    }
    let p = Preregistration {
        schema: PREREG_SCHEMA.into(),
        label: LABEL.into(),
        campaign_title: c.title.clone(),
        campaign_sha256: sha256_hex(campaign_bytes),
        system_under_test: c.system_under_test.name.clone(),
        system_version: c.system_under_test.version.clone(),
        touchdown_s: c.descent.touchdown_s(),
        bars: c.bars.clone(),
        generator: format!("arksa-kpro {}", env!("CARGO_PKG_VERSION")),
        scenarios,
    };
    (p, gens)
}

/// A signer: seed, name, UTC stamp.
pub type Signer<'a> = Option<(&'a [u8; 32], &'a str, &'a str)>;

fn write(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), KshanaError> {
    let p = dir.join(name);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| err(format!("{}: {e}", parent.display())))?;
    }
    std::fs::write(&p, bytes).map_err(|e| err(format!("{}: {e}", p.display())))
}

/// Write the pre-registration package (`campaign.toml`, `preregistration.json`,
/// `streams/`, `truth/`) to `dir`, signed as a whole with a signer.
pub fn write_preregistration(
    c: &campaign::Campaign,
    campaign_bytes: &[u8],
    dir: &Path,
    signer: Signer,
) -> Result<(Preregistration, Option<PathBuf>), KshanaError> {
    let (p, gens) = prepare(c, campaign_bytes);
    std::fs::create_dir_all(dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
    write(dir, "campaign.toml", campaign_bytes)?;
    let json = serde_json::to_string_pretty(&p).expect("serialises") + "\n";
    write(dir, "preregistration.json", json.as_bytes())?;
    for (s, g) in p.scenarios.iter().zip(&gens) {
        write(dir, &s.stream_file, g.stream_csv.as_bytes())?;
        write(dir, &s.truth_file, g.truth_csv.as_bytes())?;
    }
    let signed = match signer {
        Some((seed, name, utc)) => Some(kshana_pro::signed_dossier::sign(dir, seed, name, utc)?),
        None => None,
    };
    Ok((p, signed))
}

/// Compare a campaign with a pre-registration: refused when the campaign differs (the
/// bars or scenarios changed after registration); otherwise whether the regenerated
/// streams hash as registered.
pub fn check_preregistration(
    c: &campaign::Campaign,
    campaign_bytes: &[u8],
    prereg_bytes: &[u8],
) -> Result<verify::PreregStatus, KshanaError> {
    let p: Preregistration = serde_json::from_slice(prereg_bytes)
        .map_err(|e| err(format!("preregistration.json: {e}")))?;
    if p.schema != PREREG_SCHEMA {
        return Err(err(format!(
            "preregistration.json: schema must be {PREREG_SCHEMA}"
        )));
    }
    let sha = sha256_hex(campaign_bytes);
    if p.campaign_sha256 != sha {
        return Err(err(format!(
            "the campaign (sha256 {sha}) is not the one pre-registered ({}): the bars or scenarios changed after registration",
            p.campaign_sha256
        )));
    }
    let (now, _) = prepare(c, campaign_bytes);
    let streams_match =
        now.scenarios.len() == p.scenarios.len()
            && now.scenarios.iter().zip(&p.scenarios).all(|(a, b)| {
                a.stream_sha256 == b.stream_sha256 && a.truth_sha256 == b.truth_sha256
            });
    Ok(verify::PreregStatus {
        preregistration_sha256: sha256_hex(prereg_bytes),
        campaign_matches: true,
        streams_match,
    })
}

/// Read every scenario's log `<id>.csv` from `logs_dir` (a missing file is `None`).
pub fn read_logs(
    c: &campaign::Campaign,
    logs_dir: &Path,
) -> Result<Vec<Option<verify::LogInput>>, KshanaError> {
    if !logs_dir.is_dir() {
        return Err(err(format!("{} is not a directory", logs_dir.display())));
    }
    c.scenarios
        .iter()
        .map(|sc| {
            let name = format!("{}.csv", sc.id);
            let p = logs_dir.join(&name);
            if !p.exists() {
                return Ok(None);
            }
            let bytes = std::fs::read(&p).map_err(|e| err(format!("{}: {e}", p.display())))?;
            Ok(Some(verify::LogInput {
                file_name: name,
                bytes,
            }))
        })
        .collect()
}

/// Write `verdict.json`, `verdict.md`, `campaign.toml` and the logs (`logs/`) to `dir`,
/// signed as a whole with a signer.
pub fn write_verdict(
    v: &verify::Verification,
    campaign_bytes: &[u8],
    logs: &[Option<verify::LogInput>],
    prereg_bytes: Option<&[u8]>,
    dir: &Path,
    signer: Signer,
) -> Result<Option<PathBuf>, KshanaError> {
    std::fs::create_dir_all(dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
    write(dir, "verdict.json", (v.to_json() + "\n").as_bytes())?;
    write(dir, "verdict.md", v.to_markdown().as_bytes())?;
    write(dir, "campaign.toml", campaign_bytes)?;
    if let Some(p) = prereg_bytes {
        write(dir, "preregistration.json", p)?;
    }
    for l in logs.iter().flatten() {
        write(dir, &format!("logs/{}", l.file_name), &l.bytes)?;
    }
    match signer {
        Some((seed, name, utc)) => Ok(Some(kshana_pro::signed_dossier::sign(
            dir, seed, name, utc,
        )?)),
        None => Ok(None),
    }
}
