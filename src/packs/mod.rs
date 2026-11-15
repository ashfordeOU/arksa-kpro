// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The Arksa-exclusive scenario packs. Each is a [`kshana::api::ExternalPack`]; they
//! reach the Kshana Pro dispatcher only through the registration hook
//! ([`crate::register`], [`kshana_pro::extension`]).

/// Kshana Pro's branded study PDF, shared by the study packs.
pub use kshana_pro::packs::study_pdf;

pub mod beacon_dop_pack;
pub mod beacon_placement_pack;
pub mod cislunar_observability_study_pack;
pub mod design_optimise_pack;
pub mod lunar_signal_security_pack;
pub mod lunar_time_budget_study_pack;
pub mod mbse_pack;
pub mod optical_rf_hybrid_study_pack;
pub mod realtime_frame_eop_study_pack;
pub mod space_tip_cue_study_pack;
pub mod spaceborne_rx_exposure_pack;
pub mod spectrum_coexistence_pack;
