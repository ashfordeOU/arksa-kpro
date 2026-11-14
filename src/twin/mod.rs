// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Kshana Pro's L1 twin framework, plus the Arksa-exclusive NAV-DT twin bundle.

pub use kshana_pro::twin::*;

/// NAV-DT performance/resilience twin pipeline (A10): the canonical bundle output.
pub mod navdt;
