// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The lunar GNSS assistance package: long-validity GNSS ephemeris and clock data with an
//! integrity bound, packaged, signed, checked and scored.
//!
//! Lunar receivers mostly cannot decode GNSS navigation data at lunar C/N0, so they need a
//! ground-made ephemeris uploaded to them, valid for a day or more (research 13 finding 5:
//! JAXA LNSS wants a long-term ephemeris with "at least 24H as effective age" for more than
//! 60 GPS and Galileo satellites; gap N7). This module is the **packaging and verification**
//! side of that product:
//!
//! * the format `ks-lunar-gnss-assist/1` (a PROPOSAL, not a standard): a common epoch grid,
//!   per-satellite ECEF positions (mm) and clocks (ps), the interpolation rule (the IGS
//!   11-point Lagrange with Earth-rotation correction, and linear clocks, as the open engine's
//!   SP3 reader does), a validity window, a per-satellite **integrity bound** (a Gaussian
//!   overbound whose sigma grows with the age of the prediction), and a reference to the lunar
//!   time-scale registry definition the user maps time with ([`super::registry`]);
//! * packing from a predicted SP3 file (no extrapolation: the window must lie inside the data);
//! * an Ed25519 signature over the canonical package;
//! * a requirements check (validity length, satellite count, constellations, node spacing,
//!   bound at the end of validity, signature);
//! * a **scorecard against a reference product** (for example a later precise SP3): the
//!   conservative range error `|Δr| + c|Δt|` against `K·σ(age)`, error-versus-age curves, and
//!   a Clopper–Pearson statement of whether the bound's integrity risk is demonstrated, not
//!   demonstrated (too few independent samples), or violated.
//!
//! **What is not here.** The prediction itself — propagating GNSS orbits and clocks beyond the
//! available products and deriving the overbound — is the high-value method. It belongs in the
//! hosted verdict service (X3 rule) and is reached over the network
//! ([`super::service`]); this crate never contains it. A package packed here from an SP3 file
//! carries whatever prediction and bound its producer supplied.
//!
//! Evidence class: MODELLED. A scorecard against a reference product says how a package
//! performed against that product; it does not validate or certify a producer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use kshana::rinex::EpochUtc;
use kshana::sp3::{interpolate_clock_linear, Sp3Epoch, Sp3File, Sp3Header, Sp3SatState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::registry::{RefScale, Registry, RegistryRef};

/// The format identifier.
pub const FORMAT: &str = "ks-lunar-gnss-assist/1";
/// The interpolation rule a package declares (and this module applies).
pub const INTERPOLATION: &str =
    "positions: 11-point Lagrange over ECEF nodes with per-node Earth-rotation correction (IGS/RTKLIB practice); clocks: linear between nodes";
/// The error metric the bound applies to.
pub const BOUND_METRIC: &str =
    "conservative range error |dr| + c|dt| (an upper bound on the range error along any line of sight)";
/// Speed of light (m/s).
pub const C_M_S: f64 = 299_792_458.0;
/// Evidence label.
pub const LABEL: &str = "MODELLED. Format ks-lunar-gnss-assist/1 is a proposal, not a standard. \
A scorecard states how a package performed against the named reference product; it does not \
validate or certify a producer. The time-scale reference points to a PROVISIONAL registry: no \
international lunar time definition is adopted.";

// ── Calendar helpers (proleptic Gregorian; days from 1970-01-01) ─────────────

fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

/// Seconds from `a` to `b` (calendar arithmetic, no leap seconds: both read in one scale).
pub fn epoch_diff_s(a: &EpochUtc, b: &EpochUtc) -> f64 {
    let sod = |e: &EpochUtc| e.hour as f64 * 3600.0 + e.minute as f64 * 60.0 + e.second;
    (days_from_civil(b.year, b.month, b.day) - days_from_civil(a.year, a.month, a.day)) as f64
        * 86_400.0
        + (sod(b) - sod(a))
}

/// `e` plus a whole number of seconds.
pub fn epoch_add_s(e: &EpochUtc, s: i64) -> EpochUtc {
    let whole = e.second.floor();
    let frac = e.second - whole;
    let total = days_from_civil(e.year, e.month, e.day) * 86_400
        + e.hour as i64 * 3600
        + e.minute as i64 * 60
        + whole as i64
        + s;
    let days = total.div_euclid(86_400);
    let sod = total.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    EpochUtc {
        year,
        month,
        day,
        hour: (sod / 3600) as u32,
        minute: ((sod % 3600) / 60) as u32,
        second: (sod % 60) as f64 + frac,
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.f]` (no zone).
pub fn parse_epoch(s: &str) -> Result<EpochUtc, String> {
    let bad = || format!("bad epoch '{s}' (YYYY-MM-DDTHH:MM:SS)");
    let s = s.trim().trim_end_matches('Z');
    let (d, t) = s.split_once(['T', ' ']).ok_or_else(bad)?;
    let dp: Vec<&str> = d.split('-').collect();
    let tp: Vec<&str> = t.split(':').collect();
    if dp.len() != 3 || tp.len() != 3 {
        return Err(bad());
    }
    let e = EpochUtc {
        year: dp[0].parse().map_err(|_| bad())?,
        month: dp[1].parse().map_err(|_| bad())?,
        day: dp[2].parse().map_err(|_| bad())?,
        hour: tp[0].parse().map_err(|_| bad())?,
        minute: tp[1].parse().map_err(|_| bad())?,
        second: tp[2].parse().map_err(|_| bad())?,
    };
    if !(1..=12).contains(&e.month)
        || !(1..=31).contains(&e.day)
        || e.hour > 23
        || e.minute > 59
        || !(0.0..60.0).contains(&e.second)
    {
        return Err(bad());
    }
    Ok(e)
}

/// Format an epoch as `YYYY-MM-DDTHH:MM:SS` (fractional seconds kept when present).
pub fn format_epoch(e: &EpochUtc) -> String {
    if e.second.fract() == 0.0 {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            e.year, e.month, e.day, e.hour, e.minute, e.second as u32
        )
    } else {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:09.6}",
            e.year, e.month, e.day, e.hour, e.minute, e.second
        )
    }
}

// ── Statistics helpers ───────────────────────────────────────────────────────

/// The standard normal quantile (Acklam's rational approximation, relative error < 1.2e-9).
pub fn norm_inv(p: f64) -> f64 {
    assert!(p > 0.0 && p < 1.0, "norm_inv needs 0 < p < 1");
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let pl = 0.024_25;
    if p < pl {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - pl {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    }
}

fn ln_gamma(x: f64) -> f64 {
    // Lanczos (g = 7, n = 9).
    const G: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        let pi = std::f64::consts::PI;
        return (pi / (pi * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = G[0];
    let t = x + 7.5;
    for (i, g) in G.iter().enumerate().skip(1) {
        a += g / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// Regularised incomplete beta `I_x(a, b)` (continued fraction, Numerical Recipes `betai`).
fn betai(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let bt = (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        bt * betacf(a, b, x) / a
    } else {
        1.0 - bt * betacf(b, a, 1.0 - x) / b
    }
}

fn betacf(a: f64, b: f64, x: f64) -> f64 {
    let (qab, qap, qam) = (a + b, a + 1.0, a - 1.0);
    let tiny = 1e-300;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < tiny {
        d = tiny;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=10_000 {
        let m = m as f64;
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < tiny {
            d = tiny;
        }
        c = 1.0 + aa / c;
        if c.abs() < tiny {
            c = tiny;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < tiny {
            d = tiny;
        }
        c = 1.0 + aa / c;
        if c.abs() < tiny {
            c = tiny;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-15 {
            break;
        }
    }
    h
}

/// `P(X ≤ k)` for `X ~ Binomial(n, p)`.
pub fn binom_cdf(k: u64, n: u64, p: f64) -> f64 {
    if k >= n {
        return 1.0;
    }
    betai((n - k) as f64, k as f64 + 1.0, 1.0 - p)
}

/// One-sided Clopper–Pearson bounds at confidence `conf` for `k` events in `n` trials:
/// `(lower, upper)`.
pub fn clopper_pearson(k: u64, n: u64, conf: f64) -> (f64, f64) {
    let alpha = 1.0 - conf;
    if n == 0 {
        return (0.0, 1.0);
    }
    let solve = |f: &dyn Fn(f64) -> f64, target: f64| {
        // f is decreasing in p; find f(p) = target.
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            if f(mid) > target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    };
    let upper = if k >= n {
        1.0
    } else {
        solve(&|p| binom_cdf(k, n, p), alpha)
    };
    let lower = if k == 0 {
        0.0
    } else {
        // P(X ≥ k; p) = alpha  ⇔  P(X ≤ k−1; p) = 1 − alpha.
        solve(&|p| binom_cdf(k - 1, n, p), 1.0 - alpha)
            .min(upper)
            .max(0.0)
    };
    (lower, upper)
}

// ── The package ──────────────────────────────────────────────────────────────

/// A per-satellite integrity bound: the error metric ([`BOUND_METRIC`]) is overbounded by a
/// zero-mean Gaussian of sigma `σ(age) = sigma0_m + growth_m_per_h·age + growth2_m_per_h2·age²`
/// (age in hours since the start of validity), with the stated integrity risk per independent
/// sample.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IntegrityBound {
    pub sigma0_m: f64,
    pub growth_m_per_h: f64,
    #[serde(default)]
    pub growth2_m_per_h2: f64,
    /// Probability that the error exceeds `K·σ(age)` at one independent sample.
    pub integrity_risk: f64,
}

impl IntegrityBound {
    pub fn validate(&self) -> Result<(), String> {
        let ok = |v: f64| v.is_finite() && v >= 0.0;
        if !(ok(self.sigma0_m) && self.sigma0_m > 0.0) {
            return Err("sigma0_m must be finite and positive".into());
        }
        if !(ok(self.growth_m_per_h) && ok(self.growth2_m_per_h2)) {
            return Err("bound growth terms must be finite and non-negative".into());
        }
        if !(self.integrity_risk > 0.0 && self.integrity_risk < 0.5) {
            return Err("integrity_risk must lie in (0, 0.5)".into());
        }
        Ok(())
    }
    /// `σ(age)` (m), age in hours.
    pub fn sigma_m(&self, age_h: f64) -> f64 {
        let a = age_h.max(0.0);
        self.sigma0_m + self.growth_m_per_h * a + self.growth2_m_per_h2 * a * a
    }
    /// The two-sided Gaussian multiplier `K = Φ⁻¹(1 − P/2)`.
    pub fn k_factor(&self) -> f64 {
        norm_inv(1.0 - self.integrity_risk / 2.0)
    }
}

/// One satellite's product.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SatProduct {
    /// `G01`, `E11`, ...
    pub sat: String,
    /// ECEF positions on the package grid (mm).
    pub pos_mm: Vec<[i64; 3]>,
    /// Clock offsets on the package grid (ps).
    pub clock_ps: Vec<i64>,
    pub bound: IntegrityBound,
}

/// The lunar time-scale reference a package carries: which registry definition the user maps
/// GNSS time to lunar time with, and the offset at the start of validity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LunarTimeRef {
    pub registry: RegistryRef,
    /// `lunar − GPST` at the start of validity (ps).
    pub lunar_minus_gpst_at_start_ps: i64,
    /// Its combined standard uncertainty (ps).
    pub u_c_ps: i64,
    /// Secular rate `d(lunar)/d(TT) − 1`, in units of 1e-18.
    pub rate_e18: i64,
    pub notice: String,
}

/// The detached-in-place signature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackageSignature {
    pub signer: String,
    pub public_key_hex: String,
    /// SHA-256 (hex) of the canonical package bytes.
    pub sha256: String,
    pub signature_hex: String,
}

/// A lunar GNSS assistance package (`ks-lunar-gnss-assist/1`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssistPackage {
    pub format: String,
    pub package_id: String,
    pub producer: String,
    /// How the prediction was made (free text from the producer).
    pub prediction_basis: String,
    pub evidence_class: String,
    /// Issue time (ISO, read in `time_system`).
    pub issued: String,
    /// The time system of every epoch in the package (`GPST`).
    pub time_system: String,
    /// The terrestrial frame of the positions.
    pub frame: String,
    pub interpolation: String,
    pub bound_metric: String,
    /// First grid epoch (ISO, read in `time_system`).
    pub start: String,
    pub step_s: u32,
    pub n_epochs: u32,
    /// Validity window, seconds from `start`.
    pub validity_start_s: i64,
    pub validity_end_s: i64,
    #[serde(default)]
    pub lunar_time: Option<LunarTimeRef>,
    pub satellites: Vec<SatProduct>,
    #[serde(default)]
    pub signature: Option<PackageSignature>,
}

impl AssistPackage {
    /// Structural checks every package must pass.
    pub fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT {
            return Err(format!("format '{}' is not {FORMAT}", self.format));
        }
        if self.time_system != "GPST" {
            return Err(format!(
                "time system '{}': this version carries GPST epochs only",
                self.time_system
            ));
        }
        parse_epoch(&self.start)?;
        if self.step_s == 0 || self.n_epochs < 11 {
            return Err("a package needs a positive step and at least 11 epochs".into());
        }
        let span = self.step_s as i64 * (self.n_epochs as i64 - 1);
        if !(0 <= self.validity_start_s
            && self.validity_start_s < self.validity_end_s
            && self.validity_end_s <= span)
        {
            return Err(format!(
                "validity [{}, {}] s must lie inside the data span [0, {span}] s (no extrapolation)",
                self.validity_start_s, self.validity_end_s
            ));
        }
        if self.satellites.is_empty() {
            return Err("a package carries at least one satellite".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.satellites {
            if !seen.insert(s.sat.as_str()) {
                return Err(format!("satellite {} appears twice", s.sat));
            }
            if s.pos_mm.len() != self.n_epochs as usize
                || s.clock_ps.len() != self.n_epochs as usize
            {
                return Err(format!("satellite {}: node count is not n_epochs", s.sat));
            }
            s.bound.validate().map_err(|e| format!("{}: {e}", s.sat))?;
        }
        Ok(())
    }

    /// Validity length (h).
    pub fn validity_hours(&self) -> f64 {
        (self.validity_end_s - self.validity_start_s) as f64 / 3600.0
    }

    /// The canonical bytes the signature covers: the package without its signature.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut c = self.clone();
        c.signature = None;
        serde_json::to_vec(&c).unwrap_or_default()
    }

    /// Sign with an Ed25519 seed. Refuses a package whose canonical bytes do not survive a JSON
    /// round trip (so a verifier that re-parses the file gets the same bytes).
    pub fn sign(&mut self, seed: &[u8; 32], signer: &str) -> Result<(), String> {
        self.validate()?;
        let bytes = self.canonical_bytes();
        let back: AssistPackage =
            serde_json::from_slice(&bytes).map_err(|e| format!("round trip: {e}"))?;
        if back.canonical_bytes() != bytes {
            return Err(
                "a number in the package does not survive a JSON round trip; give bound \
                 parameters with at most 15 significant digits"
                    .into(),
            );
        }
        let sk = SigningKey::from_bytes(seed);
        let sig = sk.sign(&bytes);
        self.signature = Some(PackageSignature {
            signer: signer.to_string(),
            public_key_hex: hex::encode(sk.verifying_key().to_bytes()),
            sha256: hex::encode(Sha256::digest(&bytes)),
            signature_hex: hex::encode(sig.to_bytes()),
        });
        Ok(())
    }

    /// Verify the signature. With `trusted` keys (64 hex each), the signing key must be one of
    /// them; without, only integrity is checked and the result says the key is untrusted.
    pub fn verify_signature(&self, trusted: &[String]) -> Result<String, String> {
        let s = self.signature.as_ref().ok_or("the package is not signed")?;
        let bytes = self.canonical_bytes();
        if hex::encode(Sha256::digest(&bytes)) != s.sha256 {
            return Err("package hash does not match its signature block".into());
        }
        let pk: [u8; 32] = hex::decode(&s.public_key_hex)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or("public key is not 64 hex characters")?;
        let vk = VerifyingKey::from_bytes(&pk).map_err(|e| format!("public key: {e}"))?;
        let sig: [u8; 64] = hex::decode(&s.signature_hex)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or("signature is not 128 hex characters")?;
        vk.verify(&bytes, &Signature::from_bytes(&sig))
            .map_err(|_| "signature does not verify".to_string())?;
        if trusted.is_empty() {
            return Ok(format!(
                "signature intact (signer '{}', key {}), but no trusted key was given: the signer is not authenticated",
                s.signer, s.public_key_hex
            ));
        }
        if trusted
            .iter()
            .any(|k| k.trim().eq_ignore_ascii_case(&s.public_key_hex))
        {
            Ok(format!(
                "signature verifies against a trusted key (signer '{}')",
                s.signer
            ))
        } else {
            Err(format!(
                "signature is intact but key {} is not among the trusted keys",
                s.public_key_hex
            ))
        }
    }

    /// The package as an SP3 file (GPST grid), so the open engine's interpolator evaluates it.
    pub fn to_sp3(&self) -> Result<Sp3File, String> {
        let start = parse_epoch(&self.start)?;
        let epochs = (0..self.n_epochs as usize)
            .map(|i| Sp3Epoch {
                time: epoch_add_s(&start, i as i64 * self.step_s as i64),
                sats: self
                    .satellites
                    .iter()
                    .map(|s| Sp3SatState {
                        sat: s.sat.clone(),
                        pos_m: [
                            s.pos_mm[i][0] as f64 * 1e-3,
                            s.pos_mm[i][1] as f64 * 1e-3,
                            s.pos_mm[i][2] as f64 * 1e-3,
                        ],
                        clock_us: s.clock_ps[i] as f64 * 1e-6,
                        vel_m_s: None,
                    })
                    .collect(),
            })
            .collect();
        Ok(Sp3File {
            header: Sp3Header {
                version: 'd',
                has_velocity: false,
                start,
                num_epochs: self.n_epochs as usize,
                sat_ids: self.satellites.iter().map(|s| s.sat.clone()).collect(),
            },
            epochs,
        })
    }

    /// The constellations present (first letter of each satellite id).
    pub fn constellations(&self) -> Vec<char> {
        let mut c: Vec<char> = self
            .satellites
            .iter()
            .filter_map(|s| s.sat.chars().next())
            .collect();
        c.sort_unstable();
        c.dedup();
        c
    }
}

/// Options for packing an SP3 prediction.
#[derive(Clone, Debug, Deserialize)]
pub struct PackOptions {
    pub package_id: String,
    pub producer: String,
    pub prediction_basis: String,
    pub issued: String,
    #[serde(default = "d_frame")]
    pub frame: String,
    /// Start of validity (ISO, GPST).
    pub validity_start: String,
    pub validity_hours: f64,
    /// The satellites to include (all complete ones when empty).
    #[serde(default)]
    pub satellites: Vec<String>,
    /// The default bound.
    pub bound: IntegrityBound,
    /// Per-satellite bound overrides.
    #[serde(default)]
    pub bound_overrides: BTreeMap<String, IntegrityBound>,
    /// The registry definition the lunar time reference names (none: no reference).
    #[serde(default)]
    pub lunar_time_definition: Option<String>,
}

fn d_frame() -> String {
    "as the source SP3 (IGS realisation, ECEF)".into()
}

/// The result of packing.
#[derive(Clone, Debug)]
pub struct Packed {
    pub package: AssistPackage,
    /// Satellites left out, with the reason.
    pub dropped: Vec<(String, String)>,
}

/// Pack an SP3 prediction (GPST epochs) into an unsigned assistance package. The grid must be
/// uniform; satellites with a missing epoch or a bad clock anywhere on the grid are dropped
/// (and listed); the validity window must lie inside the data (no extrapolation).
pub fn pack_from_sp3(
    sp3: &Sp3File,
    opt: &PackOptions,
    registry: Option<&Registry>,
) -> Result<Packed, String> {
    if sp3.epochs.len() < 11 {
        return Err("the SP3 file has fewer than 11 epochs".into());
    }
    let start = sp3.epochs[0].time;
    let step = epoch_diff_s(&start, &sp3.epochs[1].time);
    if !(step > 0.0 && step.fract() == 0.0 && step <= u32::MAX as f64) {
        return Err(format!("SP3 step {step} s is not a positive whole number"));
    }
    for (i, e) in sp3.epochs.iter().enumerate() {
        let want = i as f64 * step;
        if (epoch_diff_s(&start, &e.time) - want).abs() > 1e-6 {
            return Err(format!("SP3 epoch {i} is off the uniform {step} s grid"));
        }
    }
    opt.bound.validate()?;
    let vstart = parse_epoch(&opt.validity_start)?;
    let vs = epoch_diff_s(&start, &vstart);
    if vs.fract() != 0.0 {
        return Err("validity_start must fall on a whole second of the SP3 grid".into());
    }
    if !(opt.validity_hours.is_finite() && opt.validity_hours > 0.0) {
        return Err("validity_hours must be positive".into());
    }
    let ve = vs + (opt.validity_hours * 3600.0).round();
    let wanted: Vec<String> = if opt.satellites.is_empty() {
        sp3.header.sat_ids.clone()
    } else {
        opt.satellites.clone()
    };
    let mut sats = Vec::new();
    let mut dropped = Vec::new();
    'sat: for id in wanted {
        let mut pos = Vec::with_capacity(sp3.epochs.len());
        let mut clk = Vec::with_capacity(sp3.epochs.len());
        for e in &sp3.epochs {
            let Some(st) = e.sats.iter().find(|s| s.sat == id) else {
                dropped.push((id.clone(), "missing at one or more epochs".into()));
                continue 'sat;
            };
            if st.clock_is_bad() || st.pos_m == [0.0, 0.0, 0.0] {
                dropped.push((id.clone(), "bad clock or position sentinel".into()));
                continue 'sat;
            }
            pos.push([
                (st.pos_m[0] * 1e3).round() as i64,
                (st.pos_m[1] * 1e3).round() as i64,
                (st.pos_m[2] * 1e3).round() as i64,
            ]);
            clk.push((st.clock_us * 1e6).round() as i64);
        }
        let bound = opt
            .bound_overrides
            .get(&id)
            .cloned()
            .unwrap_or_else(|| opt.bound.clone());
        bound.validate().map_err(|e| format!("{id}: {e}"))?;
        sats.push(SatProduct {
            sat: id,
            pos_mm: pos,
            clock_ps: clk,
            bound,
        });
    }
    let lunar_time = match (&opt.lunar_time_definition, registry) {
        (None, _) => None,
        (Some(_), None) => return Err("a lunar time definition needs a registry".into()),
        (Some(def), Some(reg)) => {
            let mut ev = reg.evaluator();
            let o = ev.lunar_offset(def, RefScale::Gpst, &format_epoch(&vstart))?;
            let r = ev.rate(def)?;
            Some(LunarTimeRef {
                registry: o.registry.clone(),
                lunar_minus_gpst_at_start_ps: (o.offset_s * 1e12).round() as i64,
                u_c_ps: (o.u_c_s * 1e12).round() as i64,
                rate_e18: (r.rate * 1e18).round() as i64,
                notice: reg.notice.clone(),
            })
        }
    };
    let pkg = AssistPackage {
        format: FORMAT.into(),
        package_id: opt.package_id.clone(),
        producer: opt.producer.clone(),
        prediction_basis: opt.prediction_basis.clone(),
        evidence_class: "MODELLED".into(),
        issued: opt.issued.clone(),
        time_system: "GPST".into(),
        frame: opt.frame.clone(),
        interpolation: INTERPOLATION.into(),
        bound_metric: BOUND_METRIC.into(),
        start: format_epoch(&start),
        step_s: step as u32,
        n_epochs: sp3.epochs.len() as u32,
        validity_start_s: vs as i64,
        validity_end_s: ve as i64,
        lunar_time,
        satellites: sats,
        signature: None,
    };
    pkg.validate()?;
    Ok(Packed {
        package: pkg,
        dropped,
    })
}

// ── Requirements check ───────────────────────────────────────────────────────

/// What a user requires of a package. Defaults follow the published JAXA LNSS statement (≥ 24 h
/// effective age, more than 60 GPS + Galileo satellites); the rest are caller choices.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Requirements {
    #[serde(default = "d_min_validity")]
    pub min_validity_h: f64,
    #[serde(default = "d_min_sats")]
    pub min_satellites: usize,
    #[serde(default = "d_constellations")]
    pub constellations: Vec<String>,
    #[serde(default = "d_max_step")]
    pub max_step_s: u32,
    /// Largest acceptable `K·σ` at the end of validity (m); none: not checked.
    #[serde(default)]
    pub max_protection_at_end_m: Option<f64>,
    #[serde(default = "d_true")]
    pub require_signature: bool,
    #[serde(default)]
    pub require_lunar_time_ref: bool,
}

fn d_min_validity() -> f64 {
    24.0
}
fn d_min_sats() -> usize {
    60
}
fn d_constellations() -> Vec<String> {
    vec!["G".into(), "E".into()]
}
fn d_max_step() -> u32 {
    900
}
fn d_true() -> bool {
    true
}

impl Default for Requirements {
    fn default() -> Self {
        Requirements {
            min_validity_h: d_min_validity(),
            min_satellites: d_min_sats(),
            constellations: d_constellations(),
            max_step_s: d_max_step(),
            max_protection_at_end_m: None,
            require_signature: true,
            require_lunar_time_ref: false,
        }
    }
}

/// One requirement's outcome.
#[derive(Clone, Debug, Serialize)]
pub struct CheckItem {
    pub item: String,
    pub required: String,
    pub actual: String,
    pub pass: bool,
}

/// Check a package against requirements (and, with trusted keys, its signer).
pub fn check(pkg: &AssistPackage, req: &Requirements, trusted: &[String]) -> Vec<CheckItem> {
    let mut out = Vec::new();
    let mut push = |item: &str, required: String, actual: String, pass: bool| {
        out.push(CheckItem {
            item: item.into(),
            required,
            actual,
            pass,
        })
    };
    let structural = pkg.validate();
    push(
        "structure",
        format!("valid {FORMAT}"),
        structural.clone().err().unwrap_or_else(|| "valid".into()),
        structural.is_ok(),
    );
    let vh = pkg.validity_hours();
    push(
        "validity length",
        format!(">= {} h", req.min_validity_h),
        format!("{vh:.2} h"),
        vh + 1e-9 >= req.min_validity_h,
    );
    push(
        "satellite count",
        format!(">= {}", req.min_satellites),
        pkg.satellites.len().to_string(),
        pkg.satellites.len() >= req.min_satellites,
    );
    let have = pkg.constellations();
    let missing: Vec<&String> = req
        .constellations
        .iter()
        .filter(|c| !c.chars().next().map(|x| have.contains(&x)).unwrap_or(false))
        .collect();
    push(
        "constellations",
        req.constellations.join("+"),
        have.iter().collect::<String>(),
        missing.is_empty(),
    );
    push(
        "node spacing",
        format!("<= {} s", req.max_step_s),
        format!("{} s", pkg.step_s),
        pkg.step_s <= req.max_step_s,
    );
    if let Some(maxp) = req.max_protection_at_end_m {
        let worst = pkg
            .satellites
            .iter()
            .map(|s| s.bound.k_factor() * s.bound.sigma_m(vh))
            .fold(0.0_f64, f64::max);
        push(
            "protection at end of validity",
            format!("K*sigma <= {maxp} m"),
            format!("{worst:.3} m (worst satellite)"),
            worst <= maxp,
        );
    }
    if req.require_signature || !trusted.is_empty() {
        let v = pkg.verify_signature(trusted);
        let pass = match &v {
            Ok(_) => !trusted.is_empty() || !req.require_signature || pkg.signature.is_some(),
            Err(_) => false,
        };
        push(
            "signature",
            if trusted.is_empty() {
                "intact".into()
            } else {
                "verifies against a trusted key".into()
            },
            v.unwrap_or_else(|e| e),
            pass,
        );
    }
    if req.require_lunar_time_ref {
        push(
            "lunar time reference",
            "names a registry definition".into(),
            pkg.lunar_time
                .as_ref()
                .map(|l| {
                    format!(
                        "{} v{} '{}' ({:?})",
                        l.registry.registry_id,
                        l.registry.registry_version,
                        l.registry.definition_id,
                        l.registry.registry_status
                    )
                })
                .unwrap_or_else(|| "none".into()),
            pkg.lunar_time.is_some(),
        );
    }
    out
}

// ── Scorecard against a reference product ────────────────────────────────────

/// How to compare clocks of two products.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClockDatum {
    /// Compare clocks as given (conservative).
    Absolute,
    /// Remove the mean clock difference over the common satellites at each epoch (products of
    /// different analysis centres can use different clock references).
    RemoveEpochMean,
}

/// Scoring options.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ScoreOptions {
    /// Name of the reference product (for example an IGS final SP3 file name).
    pub reference_name: String,
    /// Score every n-th reference epoch.
    #[serde(default = "d_one")]
    pub every_nth_epoch: usize,
    /// Minimum spacing (s) between samples counted as independent for the integrity statement.
    #[serde(default = "d_decorrelation")]
    pub decorrelation_s: f64,
    #[serde(default = "d_clock_datum")]
    pub clock_datum: ClockDatum,
    /// Width of the age bins (h).
    #[serde(default = "d_bin")]
    pub age_bin_h: f64,
    /// Confidence of the Clopper–Pearson statement.
    #[serde(default = "d_conf")]
    pub confidence: f64,
}

fn d_one() -> usize {
    1
}
fn d_decorrelation() -> f64 {
    3600.0
}
fn d_clock_datum() -> ClockDatum {
    ClockDatum::Absolute
}
fn d_bin() -> f64 {
    1.0
}
fn d_conf() -> f64 {
    0.95
}

/// One age bin of the error-versus-age curve.
#[derive(Clone, Debug, Serialize)]
pub struct AgeBin {
    pub age_from_h: f64,
    pub age_to_h: f64,
    pub n: usize,
    pub rms_error_m: f64,
    pub max_error_m: f64,
    /// Mean `σ(age)` over the bin's samples (m).
    pub mean_sigma_m: f64,
    /// `rms / mean σ`: above 1 the bound's sigma is optimistic in this bin.
    pub rms_over_sigma: f64,
    pub max_z: f64,
    pub exceedances: usize,
}

/// Per-satellite summary.
#[derive(Clone, Debug, Serialize)]
pub struct SatScore {
    pub sat: String,
    pub n: usize,
    pub rms_error_m: f64,
    pub max_error_m: f64,
    pub max_z: f64,
    pub exceedances: usize,
}

/// The integrity statement.
#[derive(Clone, Debug, Serialize)]
pub struct IntegrityStatement {
    /// Largest allocated integrity risk across the package's satellites.
    pub allocated_risk: f64,
    pub independent_samples: u64,
    pub independent_exceedances: u64,
    pub confidence: f64,
    pub exceedance_rate_lower: f64,
    pub exceedance_rate_upper: f64,
    /// `BOUND-HELD (demonstrated)`, `NOT-DEMONSTRATED`, or `BOUND-VIOLATED`.
    pub verdict: String,
    /// Independent samples with zero exceedances needed to demonstrate the allocation.
    pub samples_needed_with_zero_exceedances: u64,
    pub explanation: String,
}

/// The scorecard.
#[derive(Clone, Debug, Serialize)]
pub struct Scorecard {
    pub kind: &'static str,
    pub label: &'static str,
    pub evidence_class: &'static str,
    pub package_id: String,
    pub package_sha256: String,
    pub producer: String,
    pub reference_name: String,
    pub reference_sha256: String,
    pub options: ScoreOptions,
    pub metric: &'static str,
    pub samples: usize,
    pub satellites_scored: usize,
    pub satellites_without_reference: Vec<String>,
    pub rms_error_m: f64,
    pub max_error_m: f64,
    pub max_z: f64,
    pub exceedances_all_samples: usize,
    pub age_curve: Vec<AgeBin>,
    pub per_satellite: Vec<SatScore>,
    pub integrity: IntegrityStatement,
    pub lunar_time: Option<LunarTimeRef>,
    pub caveats: Vec<String>,
}

struct Sample {
    sat_idx: usize,
    t_s: f64,
    age_h: f64,
    err_m: f64,
    sigma_m: f64,
    k: f64,
}

/// Score a package against a reference product (both GPST, both ECEF in the same frame),
/// at the reference epochs inside the package's validity window.
pub fn score(
    pkg: &AssistPackage,
    reference: &Sp3File,
    reference_text_sha256: &str,
    opt: &ScoreOptions,
) -> Result<Scorecard, String> {
    pkg.validate()?;
    if opt.every_nth_epoch == 0 {
        return Err("every_nth_epoch must be at least 1".into());
    }
    if !(opt.decorrelation_s.is_finite() && opt.decorrelation_s >= 0.0) {
        return Err("decorrelation_s must be non-negative".into());
    }
    if !(opt.age_bin_h.is_finite() && opt.age_bin_h > 0.0) {
        return Err("age_bin_h must be positive".into());
    }
    if !(opt.confidence > 0.5 && opt.confidence < 1.0) {
        return Err("confidence must lie in (0.5, 1)".into());
    }
    let start = parse_epoch(&pkg.start)?;
    let sp3 = pkg.to_sp3()?;
    let interps: Vec<_> = pkg
        .satellites
        .iter()
        .map(|s| sp3.interpolator(&s.sat))
        .collect();
    let clock_nodes: Vec<_> = pkg
        .satellites
        .iter()
        .map(|s| sp3.clock_nodes(&s.sat))
        .collect();
    let max_gap = 2.0 * pkg.step_s as f64;
    let mut samples: Vec<Sample> = Vec::new();
    let mut seen_sat = vec![false; pkg.satellites.len()];
    for (ei, ep) in reference.epochs.iter().enumerate() {
        if ei % opt.every_nth_epoch != 0 {
            continue;
        }
        let t = epoch_diff_s(&start, &ep.time);
        if t < pkg.validity_start_s as f64 - 1e-6 || t > pkg.validity_end_s as f64 + 1e-6 {
            continue;
        }
        // (sat index, position error vector norm, clock difference) for this epoch.
        let mut rows: Vec<(usize, f64, f64)> = Vec::new();
        for (si, s) in pkg.satellites.iter().enumerate() {
            let Some(truth) = ep.sats.iter().find(|x| x.sat == s.sat) else {
                continue;
            };
            if truth.clock_is_bad() || truth.pos_m == [0.0, 0.0, 0.0] {
                continue;
            }
            let Some(ip) = &interps[si] else { continue };
            let Some(clk) = interpolate_clock_linear(&clock_nodes[si], t, max_gap) else {
                continue;
            };
            let p = ip.position_ecef(t);
            let dr = ((p[0] - truth.pos_m[0]).powi(2)
                + (p[1] - truth.pos_m[1]).powi(2)
                + (p[2] - truth.pos_m[2]).powi(2))
            .sqrt();
            rows.push((si, dr, clk - truth.clock_us * 1e-6));
        }
        if rows.is_empty() {
            continue;
        }
        let mean_dt = if opt.clock_datum == ClockDatum::RemoveEpochMean {
            rows.iter().map(|r| r.2).sum::<f64>() / rows.len() as f64
        } else {
            0.0
        };
        let age_h = (t - pkg.validity_start_s as f64) / 3600.0;
        for (si, dr, dt) in rows {
            seen_sat[si] = true;
            let b = &pkg.satellites[si].bound;
            samples.push(Sample {
                sat_idx: si,
                t_s: t,
                age_h,
                err_m: dr + C_M_S * (dt - mean_dt).abs(),
                sigma_m: b.sigma_m(age_h),
                k: b.k_factor(),
            });
        }
    }
    if samples.is_empty() {
        return Err(
            "no reference epoch with a usable satellite falls inside the package's validity window"
                .into(),
        );
    }
    let exceeds = |s: &Sample| s.err_m > s.k * s.sigma_m;
    let rms = |v: &[&Sample]| {
        (v.iter().map(|s| s.err_m * s.err_m).sum::<f64>() / v.len().max(1) as f64).sqrt()
    };
    let all: Vec<&Sample> = samples.iter().collect();
    // Age curve.
    let nbins = (pkg.validity_hours() / opt.age_bin_h).ceil().max(1.0) as usize;
    let mut age_curve = Vec::new();
    for b in 0..nbins {
        let (lo, hi) = (b as f64 * opt.age_bin_h, (b + 1) as f64 * opt.age_bin_h);
        let v: Vec<&Sample> = samples
            .iter()
            .filter(|s| s.age_h >= lo && (s.age_h < hi || (b + 1 == nbins && s.age_h <= hi)))
            .collect();
        if v.is_empty() {
            continue;
        }
        let r = rms(&v);
        let ms = v.iter().map(|s| s.sigma_m).sum::<f64>() / v.len() as f64;
        age_curve.push(AgeBin {
            age_from_h: lo,
            age_to_h: hi,
            n: v.len(),
            rms_error_m: r,
            max_error_m: v.iter().map(|s| s.err_m).fold(0.0, f64::max),
            mean_sigma_m: ms,
            rms_over_sigma: r / ms,
            max_z: v.iter().map(|s| s.err_m / s.sigma_m).fold(0.0, f64::max),
            exceedances: v.iter().filter(|s| exceeds(s)).count(),
        });
    }
    // Per satellite, and the independent subsample (spacing ≥ decorrelation_s per satellite).
    let mut per_satellite = Vec::new();
    let mut indep_n: u64 = 0;
    let mut indep_k: u64 = 0;
    for (si, s) in pkg.satellites.iter().enumerate() {
        let v: Vec<&Sample> = samples.iter().filter(|x| x.sat_idx == si).collect();
        if v.is_empty() {
            continue;
        }
        let mut last = f64::NEG_INFINITY;
        for x in &v {
            if x.t_s - last >= opt.decorrelation_s {
                indep_n += 1;
                if exceeds(x) {
                    indep_k += 1;
                }
                last = x.t_s;
            }
        }
        per_satellite.push(SatScore {
            sat: s.sat.clone(),
            n: v.len(),
            rms_error_m: rms(&v),
            max_error_m: v.iter().map(|x| x.err_m).fold(0.0, f64::max),
            max_z: v.iter().map(|x| x.err_m / x.sigma_m).fold(0.0, f64::max),
            exceedances: v.iter().filter(|x| exceeds(x)).count(),
        });
    }
    let allocated = pkg
        .satellites
        .iter()
        .map(|s| s.bound.integrity_risk)
        .fold(0.0_f64, f64::max);
    let (lo, hi) = clopper_pearson(indep_k, indep_n, opt.confidence);
    let needed = ((1.0 - opt.confidence).ln() / (1.0 - allocated).ln()).ceil() as u64;
    let (verdict, explanation) = if lo > allocated {
        (
            "BOUND-VIOLATED",
            format!(
                "{indep_k} exceedances in {indep_n} independent samples: even the lower {:.0} % \
                 bound on the exceedance rate ({lo:.3e}) is above the allocated risk {allocated:.1e}",
                opt.confidence * 100.0
            ),
        )
    } else if hi <= allocated {
        (
            "BOUND-HELD (demonstrated)",
            format!(
                "{indep_k} exceedances in {indep_n} independent samples: the upper {:.0} % bound \
                 on the exceedance rate ({hi:.3e}) is within the allocated risk {allocated:.1e}",
                opt.confidence * 100.0
            ),
        )
    } else {
        (
            "NOT-DEMONSTRATED",
            format!(
                "{indep_k} exceedances in {indep_n} independent samples: the data neither \
                 demonstrate nor refute the allocated risk {allocated:.1e} (upper {:.0} % bound \
                 {hi:.3e}); about {needed} independent samples with no exceedance would be needed",
                opt.confidence * 100.0
            ),
        )
    };
    let without: Vec<String> = pkg
        .satellites
        .iter()
        .zip(&seen_sat)
        .filter(|(_, seen)| !**seen)
        .map(|(s, _)| s.sat.clone())
        .collect();
    let mut caveats = vec![
        "Samples closer than decorrelation_s on one satellite are not counted as independent; \
         errors of different satellites from one prediction can still be correlated, so the \
         binomial statement is optimistic when they are."
            .to_string(),
        "Both products are read as GPST and ECEF in the same frame; a frame or time-system \
         mismatch shows up as error."
            .to_string(),
        "The reference is another product, not ground truth; its own error is inside the scored error."
            .to_string(),
    ];
    if opt.clock_datum == ClockDatum::RemoveEpochMean {
        caveats.push(
            "The per-epoch mean clock difference was removed: a common clock error of the \
             package is not scored."
                .into(),
        );
    }
    Ok(Scorecard {
        kind: "ks-lunar-gnss-assist-scorecard/1",
        label: LABEL,
        evidence_class: "MODELLED",
        package_id: pkg.package_id.clone(),
        package_sha256: hex::encode(Sha256::digest(pkg.canonical_bytes())),
        producer: pkg.producer.clone(),
        reference_name: opt.reference_name.clone(),
        reference_sha256: reference_text_sha256.to_string(),
        options: opt.clone(),
        metric: BOUND_METRIC,
        samples: samples.len(),
        satellites_scored: per_satellite.len(),
        satellites_without_reference: without,
        rms_error_m: rms(&all),
        max_error_m: samples.iter().map(|s| s.err_m).fold(0.0, f64::max),
        max_z: samples
            .iter()
            .map(|s| s.err_m / s.sigma_m)
            .fold(0.0, f64::max),
        exceedances_all_samples: samples.iter().filter(|s| exceeds(s)).count(),
        age_curve,
        per_satellite,
        integrity: IntegrityStatement {
            allocated_risk: allocated,
            independent_samples: indep_n,
            independent_exceedances: indep_k,
            confidence: opt.confidence,
            exceedance_rate_lower: lo,
            exceedance_rate_upper: hi,
            verdict: verdict.into(),
            samples_needed_with_zero_exceedances: needed,
            explanation,
        },
        lunar_time: pkg.lunar_time.clone(),
        caveats,
    })
}

/// The age curve as CSV.
pub fn age_curve_csv(s: &Scorecard) -> String {
    let mut out = String::from(
        "age_from_h,age_to_h,n,rms_error_m,max_error_m,mean_sigma_m,rms_over_sigma,max_z,exceedances\n",
    );
    for b in &s.age_curve {
        out.push_str(&format!(
            "{:.3},{:.3},{},{:.4},{:.4},{:.4},{:.4},{:.4},{}\n",
            b.age_from_h,
            b.age_to_h,
            b.n,
            b.rms_error_m,
            b.max_error_m,
            b.mean_sigma_m,
            b.rms_over_sigma,
            b.max_z,
            b.exceedances
        ));
    }
    out
}

/// A short Markdown report.
pub fn report_md(s: &Scorecard) -> String {
    let mut r = String::new();
    r.push_str(&format!(
        "# Lunar GNSS assistance package scorecard\n\n**Evidence class: {}.** {}\n\n",
        s.evidence_class, s.label
    ));
    r.push_str(&format!(
        "- Package: `{}` (sha256 `{}`), producer {}\n- Reference: {} (sha256 `{}`)\n- Metric: {}\n- Samples: {} over {} satellites\n- RMS error {:.3} m, max {:.3} m, max z {:.2}, exceedances {}\n\n",
        s.package_id,
        s.package_sha256,
        s.producer,
        s.reference_name,
        s.reference_sha256,
        s.metric,
        s.samples,
        s.satellites_scored,
        s.rms_error_m,
        s.max_error_m,
        s.max_z,
        s.exceedances_all_samples
    ));
    r.push_str(&format!(
        "## Integrity statement: {}\n\n{}\n\n",
        s.integrity.verdict, s.integrity.explanation
    ));
    r.push_str("## Error versus age\n\n| age (h) | n | RMS (m) | max (m) | mean sigma (m) | RMS/sigma | exceedances |\n|---|---|---|---|---|---|---|\n");
    for b in &s.age_curve {
        r.push_str(&format!(
            "| {:.1}–{:.1} | {} | {:.3} | {:.3} | {:.3} | {:.2} | {} |\n",
            b.age_from_h,
            b.age_to_h,
            b.n,
            b.rms_error_m,
            b.max_error_m,
            b.mean_sigma_m,
            b.rms_over_sigma,
            b.exceedances
        ));
    }
    if let Some(l) = &s.lunar_time {
        r.push_str(&format!(
            "\n## Lunar time reference\n\nRegistry {} v{} (sha256 `{}`), definition `{}`, status {:?}. {}\n",
            l.registry.registry_id,
            l.registry.registry_version,
            l.registry.registry_sha256,
            l.registry.definition_id,
            l.registry.registry_status,
            l.notice
        ));
    }
    r.push_str("\n## Caveats\n\n");
    for c in &s.caveats {
        r.push_str(&format!("- {c}\n"));
    }
    r
}

/// Write `score.json`, `age_curve.csv` and `report.md` to `dir`, and sign the directory with
/// the Kshana Pro dossier signature when a signer is given.
pub fn write_score(
    s: &Scorecard,
    dir: &Path,
    signer: Option<(&[u8; 32], &str, &str)>,
) -> Result<Option<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let w = |name: &str, text: String| {
        std::fs::write(dir.join(name), text).map_err(|e| format!("{name}: {e}"))
    };
    w(
        "score.json",
        serde_json::to_string_pretty(s).map_err(|e| e.to_string())? + "\n",
    )?;
    w("age_curve.csv", age_curve_csv(s))?;
    w("report.md", report_md(s))?;
    match signer {
        Some((seed, name, utc)) => kshana_pro::signed_dossier::sign(dir, seed, name, utc)
            .map(Some)
            .map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_round_trips() {
        for z in [-1_000_000_i64, -1, 0, 1, 10_957, 20_819, 1_000_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        let e = parse_epoch("2026-12-31T23:59:30").unwrap();
        let f = epoch_add_s(&e, 45);
        assert_eq!(format_epoch(&f), "2027-01-01T00:00:15");
        assert_eq!(epoch_diff_s(&e, &f), 45.0);
    }

    #[test]
    fn norm_inv_matches_known_quantiles() {
        assert!((norm_inv(0.975) - 1.959_963_985).abs() < 1e-8);
        assert!((norm_inv(1.0 - 5e-8) - 5.326_723_886).abs() < 1e-6);
        assert!((norm_inv(0.5)).abs() < 1e-12);
    }

    #[test]
    fn clopper_pearson_zero_events_is_the_rule_of_three() {
        let (lo, hi) = clopper_pearson(0, 1000, 0.95);
        assert_eq!(lo, 0.0);
        let exact = 1.0 - 0.05f64.powf(1.0 / 1000.0);
        assert!((hi - exact).abs() < 1e-9, "{hi} vs {exact}");
        let (lo, hi) = clopper_pearson(5, 100, 0.95);
        assert!(lo < 0.05 && hi > 0.05 && lo > 0.0);
        // P(X ≤ 5; 100, hi) = 0.05 and P(X ≥ 5; 100, lo) = 0.05 by construction.
        assert!((binom_cdf(5, 100, hi) - 0.05).abs() < 1e-9);
        assert!((1.0 - binom_cdf(4, 100, lo) - 0.05).abs() < 1e-9);
        // Published one-sided 95 % Clopper–Pearson limits for 5/100: 0.0197 and 0.1024.
        assert!(
            (lo - 0.0197).abs() < 5e-4 && (hi - 0.1024).abs() < 5e-4,
            "{lo} {hi}"
        );
    }
}
