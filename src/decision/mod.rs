// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # Collision-avoidance decision evidence
//!
//! * [`record`]: one signed decision record per conjunction event (inputs, the threshold
//!   policy in force, options considered, action, rationale, coordination), chainable;
//! * [`policy`]: the threshold-policy comparator (manoeuvre counts and accepted risk under
//!   several thresholds over one set of events).
//!
//! Both feed the mission dossier's space-safety section ([`crate::mission::space_safety`])
//! and run as `arksa-kpro mission decision|decision-chain|thresholds`. Neither parses a
//! conjunction data message: the event figures are given by the operator (see the TODO on
//! [`record::ConjunctionInput`] for wiring the CCSDS CDM parser of the conjunction
//! validation work once it merges).

pub mod policy;
pub mod record;
pub mod time;

use std::path::{Path, PathBuf};

use crate::mbse::evidence::sha256_hex;
use kshana::api::KshanaError;

fn err(msg: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(msg.into())
}

/// Read and build the record of a decision input file.
pub fn record_file(path: &Path) -> Result<(record::DecisionRecord, Vec<u8>), KshanaError> {
    let bytes = std::fs::read(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
    let text = String::from_utf8(bytes.clone())
        .map_err(|_| err(format!("{} is not UTF-8", path.display())))?;
    let input = record::DecisionInput::parse(&text)?;
    Ok((record::build(input, &sha256_hex(&bytes))?, bytes))
}

/// Write `decision.json`, `decision.md` and a copy of the input into `dir`; with a signer
/// (seed, name, UTC), sign the directory as a whole (`signature.json`, Kshana Pro's dossier
/// signature, checked with `arksa-kpro dossier verify`).
pub fn write_record(
    r: &record::DecisionRecord,
    input_bytes: &[u8],
    dir: &Path,
    signer: Option<(&[u8; 32], &str, &str)>,
) -> Result<Option<PathBuf>, KshanaError> {
    std::fs::create_dir_all(dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
    let w = |name: &str, bytes: &[u8]| {
        std::fs::write(dir.join(name), bytes).map_err(|e| err(format!("{name}: {e}")))
    };
    w("decision.json", (r.to_json() + "\n").as_bytes())?;
    w("decision.md", r.to_markdown().as_bytes())?;
    w("input.toml", input_bytes)?;
    match signer {
        Some((seed, name, utc)) => Ok(Some(kshana_pro::signed_dossier::sign(
            dir, seed, name, utc,
        )?)),
        None => Ok(None),
    }
}
