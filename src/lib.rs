// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # arksa-kpro
//!
//! The Arksa-exclusive Kshana Pro kinds and commands (roadmap 0.42, X3). This crate depends
//! on `kshana-pro` and on the open `kshana` engine; `kshana-pro` never depends on it.
//!
//! It runs only on Arksa servers or in a contracted Arksa Sovereign install (README,
//! rule 1). Its kinds and commands reach the Kshana Pro dispatcher, service and command
//! line through one hook: [`register`] hands them to [`kshana_pro::extension::register`].
//! Every one of them is listed in Kshana Pro's signed tier manifest at the `arksa` tier, by
//! hash, so each run still goes through [`kshana_pro::license::check`] and needs a token
//! carrying the `arksa` claim, signed by an Arksa issuer key ([`issuer_keys`]).

// The open engine and the Kshana Pro modules the moved code builds on, under the paths it
// was written against (`crate::registry`, `crate::license`, ...).
pub use kshana;
pub use kshana_pro::{
    benefits, brand_pdf, criterion, license, open_engine_kinds, pdf_text, registry, report,
    report_brand, report_pdf, study_ledger, units, util, version,
};

pub mod cislunar_ssa;
pub mod commands;
pub mod conjunction;
pub mod connectors;
pub mod decision;
pub mod design;
pub mod disposal;
pub mod entitlement;
pub mod lunar_oed;
pub mod lunar_time_assist;
pub mod mbse;
pub mod mission;
pub mod mrd;
pub mod ops;
pub mod packs;
pub mod simcal;
pub mod spectrum_coexistence;
pub mod suite;
pub mod twin;

use kshana::api::KshanaError;
use kshana_pro::extension::{parse_pack, Extension, ExtensionFeature, ExtensionKind};
use kshana_pro::license::{Tier, KEY_LEN};

use packs::beacon_dop_pack::{BeaconDopScenario, SurfaceBeaconDopStudyScenario};
use packs::beacon_placement_pack::BeaconPlacementScenario;
use packs::cislunar_observability_study_pack::CislunarObservabilityStudyScenario;
use packs::conjunction_validation_pack::ConjunctionValidationScenario;
use packs::design_optimise_pack::DesignOptimiseScenario;
use packs::lunar_signal_security_pack::LunarSignalSecurityScenario;
use packs::lunar_time_budget_study_pack::LunarTimeBudgetStudyScenario;
use packs::mbse_pack::MbseScenario;
use packs::optical_rf_hybrid_study_pack::OpticalRfHybridStudyScenario;
use packs::realtime_frame_eop_study_pack::RealtimeFrameEopStudyScenario;
use packs::space_tip_cue_study_pack::SpaceTipCueStudyScenario;
use packs::spaceborne_rx_exposure_pack::SpaceborneRxExposureScenario;
use packs::spectrum_coexistence_pack::SpectrumCoexistenceScenario;

/// The Arksa-exclusive scenario kinds (roadmap 0.42 section 1a, and the owner's decision of
/// 14 November 2026 adding the two lunar surface beacon studies).
pub const ARKSA_KINDS: &[&str] = &[
    MbseScenario::KIND,
    DesignOptimiseScenario::KIND,
    SpectrumCoexistenceScenario::KIND,
    LunarSignalSecurityScenario::KIND,
    BeaconPlacementScenario::KIND,
    LunarTimeBudgetStudyScenario::KIND,
    CislunarObservabilityStudyScenario::KIND,
    OpticalRfHybridStudyScenario::KIND,
    RealtimeFrameEopStudyScenario::KIND,
    BeaconDopScenario::KIND,
    SurfaceBeaconDopStudyScenario::KIND,
];

/// The Arksa-exclusive command features. The commands `mbse`, `design` and `coexist` share
/// the feature id of their scenario kind.
pub const ARKSA_FEATURES: &[&str] = &[
    MbseScenario::KIND,
    DesignOptimiseScenario::KIND,
    SpectrumCoexistenceScenario::KIND,
    mission::FEATURE,
    "pro-navdt",
    "pro-spacestream",
    "pro-mrd",
];

/// Arksa-exclusive kinds added on 15 November 2026 (roadmap 0.42 F4 and M9). They register
/// through the same hook and the same `arksa` claim as [`ARKSA_KINDS`], but only once the
/// pinned Kshana Pro's signed tier manifest lists them at the `arksa` tier (by hash):
/// registering an id the manifest does not list would fail Kshana Pro's start-up check for
/// every kind. Until the pin moves to a Kshana Pro commit whose manifest lists them, they are
/// built and tested here as packs but are not reachable through the dispatcher (and so are
/// refused like any unknown kind).
///
/// `pro-conjunction-validation` (conjunction-assessment validation, research 06 G1/G2) was
/// added on 20 November 2026 and waits for the same manifest step.
pub const PENDING_MANIFEST_KINDS: &[&str] = &[
    SpaceborneRxExposureScenario::KIND,
    SpaceTipCueStudyScenario::KIND,
    ConjunctionValidationScenario::KIND,
];

/// The command features of [`commands::rx_exposure_command`] (F4) and
/// [`commands::conjunction_command`], each shared with its kind id; pending the manifest like
/// [`PENDING_MANIFEST_KINDS`].
pub const PENDING_MANIFEST_FEATURES: &[&str] = &[
    SpaceborneRxExposureScenario::KIND,
    ConjunctionValidationScenario::KIND,
];

/// Whether the pinned Kshana Pro's signed tier manifest lists `id` at the `arksa` tier with
/// entry type `ty` (`kind` or `feature`).
pub fn manifest_lists(id: &str, ty: &str) -> bool {
    let h = kshana_pro::license::tiers::name_hash(id);
    kshana_pro::license::tiers::ARKSA_ENTRIES
        .iter()
        .any(|(eh, et)| *eh == h && *et == ty)
}

/// The Arksa-exclusive kinds this build registers: [`ARKSA_KINDS`] and the pending kinds the
/// manifest already lists.
pub fn active_kinds() -> Vec<&'static str> {
    ARKSA_KINDS
        .iter()
        .chain(
            PENDING_MANIFEST_KINDS
                .iter()
                .filter(|k| manifest_lists(k, "kind")),
        )
        .copied()
        .collect()
}

/// The Arksa-exclusive command features this build registers.
pub fn active_features() -> Vec<&'static str> {
    ARKSA_FEATURES
        .iter()
        .chain(
            PENDING_MANIFEST_FEATURES
                .iter()
                .filter(|f| manifest_lists(f, "feature")),
        )
        .copied()
        .collect()
}

/// Public half of the placeholder Arksa issuer key: a TEST key whose private half exists
/// only in this crate's tests. A debug build with no `KSHANA_PRO_ARKSA_ISSUER_PUBKEY` set
/// trusts it, for local runs; a release build never does.
pub const PLACEHOLDER_ARKSA_ISSUER_PUBKEY: &str =
    "da2d8a4d79a5b8a2faefad9f4283f8e299f1450aca0ea20492f3e4dbbc6030bb";

/// The Arksa issuer public keys this build trusts: `KSHANA_PRO_ARKSA_ISSUER_PUBKEY` at build
/// time (64 hex characters, or several separated by commas); with none set, a debug build
/// trusts the placeholder test key and a release build trusts none (every Arksa token is
/// then refused).
pub fn issuer_keys() -> Result<Vec<[u8; KEY_LEN]>, KshanaError> {
    let set: Vec<[u8; KEY_LEN]> = option_env!("KSHANA_PRO_ARKSA_ISSUER_PUBKEY")
        .unwrap_or("")
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(kshana_pro::license::parse_key_hex)
        .collect::<Result<_, _>>()?;
    if !set.is_empty() || !cfg!(debug_assertions) {
        return Ok(set);
    }
    Ok(vec![kshana_pro::license::parse_key_hex(
        PLACEHOLDER_ARKSA_ISSUER_PUBKEY,
    )?])
}

macro_rules! kind {
    ($t:ty, $tier:expr) => {
        ExtensionKind {
            meta: <$t>::meta_static(),
            min_tier: $tier,
            build: parse_pack::<$t>,
            register_into: <$t as kshana::api::ExternalPack>::register_into,
        }
    };
}

/// Everything this crate adds to Kshana Pro: the kinds, the command features, the commands
/// ([`commands::all`]) and the issuer keys.
pub fn extension() -> Result<Extension, KshanaError> {
    let mut kinds = vec![
        kind!(MbseScenario, Tier::Enterprise),
        kind!(DesignOptimiseScenario, Tier::Enterprise),
        kind!(SpectrumCoexistenceScenario, Tier::Enterprise),
        kind!(LunarSignalSecurityScenario, Tier::Pro),
        kind!(BeaconPlacementScenario, Tier::Pro),
        kind!(LunarTimeBudgetStudyScenario, Tier::Pro),
        kind!(CislunarObservabilityStudyScenario, Tier::Pro),
        kind!(OpticalRfHybridStudyScenario, Tier::Pro),
        kind!(RealtimeFrameEopStudyScenario, Tier::Pro),
        kind!(BeaconDopScenario, Tier::Pro),
        kind!(SurfaceBeaconDopStudyScenario, Tier::Pro),
    ];
    if manifest_lists(SpaceborneRxExposureScenario::KIND, "kind") {
        kinds.push(kind!(SpaceborneRxExposureScenario, Tier::Pro));
    }
    if manifest_lists(SpaceTipCueStudyScenario::KIND, "kind") {
        kinds.push(kind!(SpaceTipCueStudyScenario, Tier::Pro));
    }
    if manifest_lists(ConjunctionValidationScenario::KIND, "kind") {
        kinds.push(kind!(ConjunctionValidationScenario, Tier::Pro));
    }
    let feature = |id: &'static str, min_tier| ExtensionFeature { id, min_tier };
    let mut features = vec![
        feature(MbseScenario::KIND, Tier::Enterprise),
        feature(DesignOptimiseScenario::KIND, Tier::Enterprise),
        feature(SpectrumCoexistenceScenario::KIND, Tier::Enterprise),
        feature(mission::FEATURE, Tier::Enterprise),
        feature("pro-navdt", Tier::Pro),
        feature("pro-spacestream", Tier::Pro),
        feature("pro-mrd", Tier::Pro),
    ];
    let mut commands = commands::all();
    if manifest_lists(SpaceborneRxExposureScenario::KIND, "feature") {
        features.push(feature(SpaceborneRxExposureScenario::KIND, Tier::Pro));
        commands.push(commands::rx_exposure_command());
    }
    if manifest_lists(ConjunctionValidationScenario::KIND, "feature") {
        features.push(feature(ConjunctionValidationScenario::KIND, Tier::Pro));
        commands.push(commands::conjunction_command());
    }
    Ok(Extension {
        name: "arksa-kpro",
        kinds,
        features,
        commands,
        issuer_keys: issuer_keys()?,
    })
}

/// Register this crate with Kshana Pro, then check the signed tier manifest against the
/// combined build (every Arksa id must be listed at the `arksa` tier). Safe to call more
/// than once in a process: a second call checks that this crate is the one registered.
pub fn register() -> Result<(), KshanaError> {
    match kshana_pro::extension::get() {
        Some(e) if e.name == "arksa-kpro" => {}
        Some(e) => {
            return Err(KshanaError::InvalidInput(format!(
                "another extension ({}) is already registered",
                e.name
            )))
        }
        None => {
            if let Err(e) = kshana_pro::extension::register(extension()?) {
                // Another thread may have registered it first.
                if kshana_pro::extension::get().map(|x| x.name) != Some("arksa-kpro") {
                    return Err(e);
                }
            }
        }
    }
    kshana_pro::license::tiers::startup_check()
}

/// [`register`], panicking on failure. For tests and for code that cannot run without
/// the Arksa kinds.
pub fn ensure_registered() {
    register().expect("arksa-kpro registers with Kshana Pro");
}
