// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Kshana Pro's L3 trade-study suite, plus the Arksa-exclusive assurance argument (GSN)
//! and Verification Control Document (VCD) built on a trade report.

pub use kshana_pro::suite::*;

pub mod gsn;
pub mod vcd;
