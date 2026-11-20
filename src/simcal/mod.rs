// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # Simulator recalibration from telemetry (AK8)
//!
//! A mission-agnostic core for mission performance simulators, on the mission model of
//! [`crate::mission`]. One mission file (`ks-simcal-mission/1`, [`mission`]) names the
//! telemetry file and the simulator **plugins** of the mission ([`plugin`]); each plugin
//! predicts some observables. The core then:
//!
//! 1. **compares** every plugin's predictions with the telemetry ([`residuals`]): per
//!    observable the bias with its standard uncertainty (GUM type A), RMS, largest residual,
//!    reduced chi-square where the telemetry carries a standard uncertainty, and a verdict
//!    against the observable's stated tolerance (`consistent` or `drifted`);
//! 2. **recalibrates** ([`hosted`]): a hosted plugin sends its model and its telemetry to
//!    a hosted calibration method over the network (Kshana Pro's verdict client,
//!    [`kshana_pro::remote_verdict`]): `selfcal` for radio timing reference simulators (the
//!    plugin's scenario), `calibrate` for any simulator with named parameters (an expression
//!    per observable, or design runs of the plugin's own simulator). The answer is accepted
//!    only as a signed verdict for that method that verifies against a trusted service
//!    key. No calibration method runs here: the hosted method's internals stay server-side
//!    (X3), and this crate holds only the request contract and the checks on the answer;
//! 3. **records the calibration lineage** ([`lineage`]): one hash-chained entry per
//!    recalibration (the mission, telemetry and plugin files by SHA-256, every plugin's
//!    residuals, each hosted verdict kept verbatim and its outcome), and the lineage
//!    directory signed as a whole (Kshana Pro's signed dossier, Arksa signer key).
//!
//! Built-in plugins: `table` (the predictions of any external simulator, as a CSV),
//! `engine` (a scenario of any open kind, run on the embedded engine, read at dot paths) and
//! `hosted` (a simulator whose model runs in the hosted service). A new model family is one
//! type implementing [`plugin::SimPlugin`] and one entry in [`plugin::KINDS`].
//!
//! Gated by the Arksa entitlement through the mission feature ([`FEATURE`]), the same feature
//! as `arksa-kpro mission`: the recalibration is part of the mission model.
//!
//! **Evidence.** Simulator predictions and calibrated parameters are MODELLED. The residuals
//! are measured against the telemetry supplied, nothing more; nothing here has been run on a
//! flight mission's telemetry yet. See `docs/SIM-RECALIBRATION.md`.

pub mod cli;
pub mod hosted;
pub mod lineage;
pub mod mission;
pub mod plugin;
pub mod residuals;

use sha2::{Digest, Sha256};

use crate::license::Entitlement;
use kshana::api::KshanaError;

/// The licence feature the command needs (the mission feature, Arksa claim).
pub const FEATURE: &str = crate::mission::FEATURE;

/// Evidence class of everything a simulator predicts or a calibration returns.
pub const MODELLED: &str = "MODELLED";

/// Gate the recalibration: the mission feature, behind the Arksa entitlement. The refusal
/// names no method ([`crate::license::check`]).
pub fn ensure_entitled(ent: &Entitlement, now_unix: i64) -> Result<(), KshanaError> {
    crate::license::check(ent, FEATURE, now_unix)
}

/// SHA-256 (hex) of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
