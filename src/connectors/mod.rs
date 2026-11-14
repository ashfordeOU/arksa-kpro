// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Kshana Pro's connectors, plus the Arksa-exclusive SpaceStream codec.

pub use kshana_pro::connectors::*;

/// SpaceStream: the versioned, aggregated-metrics-only interface (A09).
pub mod spacestream;
