// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Lunar time and lunar GNSS assistance (AK6, research 13 findings 4–6, gaps N7 and N9).
//!
//! * [`registry`]: a versioned, explicitly PROVISIONAL registry of lunar time-scale
//!   definitions (TCL, TL on several `W0_L` candidates, LTC as an envelope), conversions with
//!   GUM uncertainty through Kshana Pro's lunar time model, and the change impact between two
//!   registry versions (re-qualification evidence).
//! * [`assist`]: the `ks-lunar-gnss-assist/1` package (long-validity ephemeris and clocks with an
//!   integrity bound): packing from a predicted SP3, Ed25519 signing, a requirements check and
//!   a scorecard against a reference product.
//! * [`service`]: the network client for the hosted generator (the prediction method itself is
//!   not in this crate).
//!
//! Command: `arksa-kpro lunar-time ...`, gated on the Arksa-exclusive kind
//! `pro-lunar-time-budget-study` (no new manifest entry). Docs: `docs/LUNAR-TIME-ASSIST.md`.
//! Evidence class: MODELLED throughout.

pub mod assist;
pub mod cli;
pub mod registry;
pub mod service;

/// The entitlement the `lunar-time` command requires: the existing Arksa-exclusive lunar time
/// study kind (already listed in Kshana Pro's signed tier manifest at the `arksa` tier).
pub const FEATURE: &str =
    crate::packs::lunar_time_budget_study_pack::LunarTimeBudgetStudyScenario::KIND;
