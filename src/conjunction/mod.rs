// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # Conjunction-assessment validation (Arksa Ops pillar)
//!
//! Neutral evidence about conjunction-assessment (CA) outputs and the ephemerides behind them:
//!
//! * [`cdm`]: CCSDS Conjunction Data Message (CCSDS 508.0-B-1) ingest, KVN and NDM/XML, into
//!   one normalised [`Event`];
//! * [`answer_key`]: the row format of the US Office of Space Commerce TraCSS "Dataset for
//!   Conjunction Assessment Verification" results files (CC0, published 18 March 2026), as its
//!   user's guide documents it (Table 5);
//! * [`ephemeris`]: CCSDS OEM ingest with every covariance block (the open engine's OEM reader
//!   keeps the states and skips covariance), and OMM for the object class;
//! * [`pc`]: two-dimensional probability of collision (the standard short-encounter
//!   integral), used to recompute and explain a provider's Pc, never to grade it;
//! * [`scorecard`]: a provider's events against a reference set (the answer key, or another
//!   reference), with event matching, per-velocity-class time-of-closest-approach objectives,
//!   position and covariance agreement, and per-event consistency checks;
//! * [`realism`]: covariance-realism tests (Mahalanobis distance against the chi-square law,
//!   by prediction age and object class);
//! * [`compare`]: the same event seen by two providers, with the divergence explained by
//!   swapping one input at a time.
//!
//! Evidence class: MODELLED. Every agreement figure is computed exactly from the files named
//! (by SHA-256) in the result; a reference set is not ground truth, and nothing here is a
//! validation or certification of a provider. No agency endorses this work.

#![allow(clippy::needless_range_loop)] // index loops read as the matrix algebra they implement

pub mod answer_key;
pub mod cdm;
pub mod compare;
pub mod ephemeris;
pub mod pc;
pub mod realism;
pub mod scorecard;

use kshana::api::KshanaError;
use serde_json::Value;

/// The kind id (also the command feature id).
pub const KIND: &str = "pro-conjunction-validation";
/// The result schema.
pub const SCHEMA: &str = "arksa-conjunction-validation/1";

/// The honesty label on every result.
pub const LABEL: &str = "MODELLED conjunction-assessment validation evidence. MEASURED: nothing; \
the inputs are conjunction messages, result tables and ephemerides as supplied, each named by \
SHA-256. Agreement figures are exact comparisons of those files against the reference set the \
user names; a reference set (including a published answer key, which is another tool's output) \
is not ground truth. MODELLED: the recomputed probability of collision (two-dimensional short-\
encounter integral), the hard-body radii where a message gives none, the chi-square law in the \
covariance-realism tests (which assumes independent samples), and the published performance \
objectives used as thresholds. Not a validation, certification or endorsement of any provider, \
tool or dataset, and not endorsed by any agency.";

/// Where the default objectives come from (cited in every result).
pub const REFERENCES: &[&str] = &[
    "CCSDS 508.0-B-1, Conjunction Data Message (CDM), Blue Book (keywords and units)",
    "CCSDS 502.0-B-3, Orbit Data Messages (OEM, OMM)",
    "US Office of Space Commerce, Dataset for Conjunction Assessment Verification, user's guide (Auman, Murphy, George, The Aerospace Corporation, March 2026), CC0-1.0: configuration rules and results-file columns",
    "Auman et al., Validation methodology for TraCSS conjunction assessment, AMOS 2025: TCA objectives by relative velocity (VLRV < 10 m/s ~10 s, LRV < 50 m/s ~0.01 s, NLRV ~0.001 s), 1 m position, 0.1 Bhattacharyya distance covariance",
    "NASA Spacecraft Conjunction Assessment and Collision Avoidance Best Practices Handbook, NASA-SP-20205011318/REV2-VOL1 (Aug 2026): realistic 6x6 covariance at each ephemeris point; validated CA services (its Volume 2 appendices are not reproduced here)",
    "Borowitz et al., EU SST and TraCSS operational information exchange, AMOS 2026: cross-provider event matching within 6% of the orbital period",
];

const SECONDS_PER_DAY: f64 = 86_400.0;

fn bad(m: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(format!("{KIND}: {}", m.into()))
}

// ── Time ────────────────────────────────────────────────────────────────────────────────

/// Days from 1970-01-01 to a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days from 1970-01-01 to 2000-01-01.
const EPOCH_2000_DAYS: i64 = 10_957;

/// A UTC time as calendar seconds from 2000-01-01T00:00:00 (leap seconds are not counted:
/// two times either side of a leap second differ by one second too few, which the result
/// states; CA spans are days and the comparisons are between files on the same scale).
///
/// Accepts `YYYY-MM-DDThh:mm[:ss[.f]][Z]`, the same with a space for `T`, `YYYY-MM-DD`, and the
/// CCSDS day-of-year form `YYYY-DDDThh:mm:ss[.f][Z]`.
pub fn parse_utc(s: &str) -> Result<f64, KshanaError> {
    let t = s.trim().trim_end_matches('Z').trim_end_matches('z');
    let (date, time) = match t.find(['T', 't', ' ']) {
        Some(i) => (&t[..i], t[i + 1..].trim()),
        None => (t, ""),
    };
    let err = || bad(format!("invalid UTC time '{s}'"));
    let parts: Vec<&str> = date.split('-').collect();
    let days = match parts.as_slice() {
        [y, m, d] => {
            let (y, m, d): (i64, i64, i64) = (
                y.parse().map_err(|_| err())?,
                m.parse().map_err(|_| err())?,
                d.parse().map_err(|_| err())?,
            );
            if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
                return Err(err());
            }
            days_from_civil(y, m, d)
        }
        [y, doy] if doy.len() == 3 => {
            let (y, doy): (i64, i64) = (
                y.parse().map_err(|_| err())?,
                doy.parse().map_err(|_| err())?,
            );
            if !(1..=366).contains(&doy) {
                return Err(err());
            }
            days_from_civil(y, 1, 1) + doy - 1
        }
        _ => return Err(err()),
    };
    let mut sod = 0.0;
    if !time.is_empty() {
        let f: Vec<&str> = time.split(':').collect();
        if f.len() < 2 || f.len() > 3 {
            return Err(err());
        }
        let h: f64 = f[0].parse().map_err(|_| err())?;
        let mi: f64 = f[1].parse().map_err(|_| err())?;
        let se: f64 = match f.get(2) {
            Some(x) => x.parse().map_err(|_| err())?,
            None => 0.0,
        };
        if !(0.0..24.0).contains(&h) || !(0.0..60.0).contains(&mi) || !(0.0..61.0).contains(&se) {
            return Err(err());
        }
        sod = h * 3600.0 + mi * 60.0 + se;
    }
    Ok((days - EPOCH_2000_DAYS) as f64 * SECONDS_PER_DAY + sod)
}

/// A Julian date (UTC) as calendar seconds from 2000-01-01T00:00:00.
pub fn jd_to_s(jd: f64) -> f64 {
    (jd - 2_451_544.5) * SECONDS_PER_DAY
}

/// Format calendar seconds from 2000-01-01 as `YYYY-MM-DDThh:mm:ss.ffffffZ`.
pub fn fmt_utc(t: f64) -> String {
    let day = (t / SECONDS_PER_DAY).floor();
    let mut sod = t - day * SECONDS_PER_DAY;
    let mut day = day as i64;
    // Round to microseconds, carrying into the next day.
    let mut us = (sod * 1e6).round() as i64;
    if us >= 86_400_000_000 {
        us -= 86_400_000_000;
        day += 1;
    }
    sod = us as f64 / 1e6;
    let (y, m, d) = civil_from_days(day + EPOCH_2000_DAYS);
    let h = (us / 3_600_000_000) as u32;
    let mi = ((us / 60_000_000) % 60) as u32;
    let se = sod - (h as f64 * 3600.0 + mi as f64 * 60.0);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{se:09.6}Z")
}

// ── Small linear algebra ────────────────────────────────────────────────────────────────

pub type V3 = [f64; 3];
pub type M3 = [[f64; 3]; 3];
pub type M6 = [[f64; 6]; 6];

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn unit(a: V3) -> V3 {
    let n = norm(a);
    if n > 0.0 {
        scale(a, 1.0 / n)
    } else {
        a
    }
}

/// The rotation from inertial axes to the RTN (radial, transverse, normal; also UVW or RIC)
/// axes of a state: rows R, T, N.
pub fn rtn_matrix(r: V3, v: V3) -> M3 {
    let rh = unit(r);
    let nh = unit(cross(r, v));
    let th = cross(nh, rh);
    [rh, th, nh]
}

pub fn mat_vec(m: &M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

pub fn mat_t_vec(m: &M3, v: V3) -> V3 {
    let mut o = [0.0; 3];
    for (i, row) in m.iter().enumerate() {
        for j in 0..3 {
            o[j] += row[j] * v[i];
        }
    }
    o
}

/// The position block (upper-left 3x3) of a 6x6 covariance.
pub fn pos_block(c: &M6) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = c[i][j];
        }
    }
    o
}

/// `Mᵀ C M` for a 3x3 covariance `C` in the axes `M` maps into (rotating an RTN covariance
/// to inertial with `M` the inertial-to-RTN rotation).
pub fn rotate_cov_to_inertial(m: &M3, c: &M3) -> M3 {
    let mut tmp = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            tmp[i][j] = (0..3).map(|k| c[i][k] * m[k][j]).sum();
        }
    }
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = (0..3).map(|k| m[k][i] * tmp[k][j]).sum();
        }
    }
    o
}

/// Cholesky factor of the leading `n x n` block of a symmetric matrix (row-major slices),
/// `None` when it is not positive definite.
pub fn cholesky(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let mut l = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let s: f64 = (0..j).map(|k| l[i][k] * l[j][k]).sum();
            if i == j {
                let d = a[i][i] - s;
                if d.is_nan() || d <= 0.0 {
                    return None;
                }
                l[i][j] = d.sqrt();
            } else {
                l[i][j] = (a[i][j] - s) / l[j][j];
            }
        }
    }
    Some(l)
}

/// `xᵀ A⁻¹ x` through the Cholesky factor `L` of `A`.
pub fn quad_inv(l: &[Vec<f64>], x: &[f64]) -> f64 {
    let n = l.len();
    let mut y = vec![0.0; n];
    for i in 0..n {
        let s: f64 = (0..i).map(|k| l[i][k] * y[k]).sum();
        y[i] = (x[i] - s) / l[i][i];
    }
    y.iter().map(|v| v * v).sum()
}

/// `ln det A` from its Cholesky factor.
pub fn ln_det(l: &[Vec<f64>]) -> f64 {
    2.0 * (0..l.len()).map(|i| l[i][i].ln()).sum::<f64>()
}

pub fn m3_rows(m: &M3) -> Vec<Vec<f64>> {
    m.iter().map(|r| r.to_vec()).collect()
}

/// Whether a symmetric covariance is positive semi-definite within a relative tolerance
/// (a tiny diagonal load scaled to its trace), and symmetric.
pub fn is_psd(rows: &[Vec<f64>]) -> bool {
    let n = rows.len();
    let tr: f64 = (0..n).map(|i| rows[i][i].abs()).sum();
    if !tr.is_finite() || tr == 0.0 {
        return false;
    }
    for i in 0..n {
        for j in 0..i {
            if (rows[i][j] - rows[j][i]).abs() > 1e-9 * tr {
                return false;
            }
        }
    }
    let load = 1e-12 * tr;
    let a: Vec<Vec<f64>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| rows[i][j] + if i == j { load } else { 0.0 })
                .collect()
        })
        .collect();
    cholesky(&a).is_some()
}

/// Bhattacharyya distance between two zero-mean Gaussians (the covariance term only):
/// `½ ln( det Σ / √(det Σ₁ det Σ₂) )` with `Σ = (Σ₁ + Σ₂)/2`. `None` when either is not
/// positive definite.
pub fn bhattacharyya_cov(a: &M3, b: &M3) -> Option<f64> {
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = 0.5 * (a[i][j] + b[i][j]);
        }
    }
    let la = cholesky(&m3_rows(a))?;
    let lb = cholesky(&m3_rows(b))?;
    let lm = cholesky(&m3_rows(&m))?;
    Some(0.5 * (ln_det(&lm) - 0.5 * (ln_det(&la) + ln_det(&lb))))
}

// ── Orbit class ─────────────────────────────────────────────────────────────────────────

/// Earth's gravitational parameter (m³/s², EGM96/WGS 84 value).
pub const MU_EARTH: f64 = 3.986_004_418e14;
/// Equatorial radius (m).
pub const R_EARTH: f64 = 6_378_137.0;

/// Osculating two-body elements of an Earth-centred inertial state (m, m/s): semi-major
/// axis (m, negative when hyperbolic), eccentricity, inclination (deg), period (s, infinite
/// when unbound) and perigee altitude (m).
pub fn elements(r: V3, v: V3) -> (f64, f64, f64, f64, f64) {
    let rn = norm(r);
    let v2 = dot(v, v);
    let a = 1.0 / (2.0 / rn - v2 / MU_EARTH);
    let h = cross(r, v);
    let e_vec = sub(scale(cross(v, h), 1.0 / MU_EARTH), scale(r, 1.0 / rn));
    let e = norm(e_vec);
    let inc = (h[2] / norm(h)).clamp(-1.0, 1.0).acos().to_degrees();
    let period = if a > 0.0 {
        2.0 * std::f64::consts::PI * (a.powi(3) / MU_EARTH).sqrt()
    } else {
        f64::INFINITY
    };
    let rp = if e < 1.0 {
        a * (1.0 - e)
    } else {
        norm(h).powi(2) / MU_EARTH / (1.0 + e)
    };
    (a, e, inc, period, rp - R_EARTH)
}

/// The Space Flight Safety Handbook screening-volume regime of a state, as the TraCSS
/// verification dataset's user's guide tabulates it (Table 3): `(id, label, [U, V, W] half
/// sizes in m)`.
pub fn sfsh_regime(r: V3, v: V3) -> (u8, &'static str, V3) {
    let (_, e, inc, period, hp) = elements(r, v);
    let min = period / 60.0;
    let km = 1000.0;
    if e >= 1.0 {
        return (8, "hyperbolic", [20.0 * km, 50.0 * km, 20.0 * km]);
    }
    if e < 0.25 {
        if min > 1300.0 && min < 1800.0 && inc < 35.0 {
            return (1, "deep space (near-GEO)", [10.0 * km; 3]);
        }
        if hp <= 500.0 * km {
            return (
                5,
                "LEO1 (perigee <= 500 km)",
                [0.4 * km, 44.0 * km, 51.0 * km],
            );
        }
        if hp <= 750.0 * km {
            return (4, "LEO2 (500-750 km)", [0.4 * km, 25.0 * km, 25.0 * km]);
        }
        if hp <= 1200.0 * km {
            return (3, "LEO3 (750-1200 km)", [0.4 * km, 12.0 * km, 12.0 * km]);
        }
        if hp <= 2000.0 * km {
            return (2, "LEO4 (1200-2000 km)", [0.4 * km, 2.0 * km, 2.0 * km]);
        }
    }
    if min > 225.0 {
        (6, "deep space (period > 225 min)", [20.0 * km; 3])
    } else {
        (
            7,
            "near Earth (period < 225 min)",
            [2.0 * km, 25.0 * km, 25.0 * km],
        )
    }
}

/// A coarse object class for grouping results: LEO, MEO, GEO, HEO or hyperbolic.
pub fn object_class(r: V3, v: V3) -> &'static str {
    let (a, e, _, period, hp) = elements(r, v);
    if e >= 1.0 || a <= 0.0 {
        return "hyperbolic";
    }
    if e >= 0.25 {
        return "HEO";
    }
    let min = period / 60.0;
    if (1300.0..1800.0).contains(&min) {
        "GEO"
    } else if hp <= 2_000_000.0 {
        "LEO"
    } else {
        "MEO"
    }
}

// ── Normalised event ────────────────────────────────────────────────────────────────────

/// One conjunction event from any source, in SI units (m, m/s, m², s).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Event {
    /// Message or conjunction id as given.
    pub id: String,
    /// Primary (object 1) and secondary (object 2) ids, normalised ([`norm_id`]).
    pub obj1: String,
    pub obj2: String,
    /// TCA, calendar seconds from 2000-01-01 UTC ([`parse_utc`]).
    pub tca_s: f64,
    pub miss_m: f64,
    pub vrel_m_s: Option<f64>,
    pub pc: Option<f64>,
    pub pc_method: Option<String>,
    /// Secondary relative to primary, in the primary's RTN axes (m).
    pub rel_rtn_m: Option<V3>,
    /// Inertial states at TCA (m, m/s), and their frame as written.
    pub state1: Option<[f64; 6]>,
    pub state2: Option<[f64; 6]>,
    pub frame: Option<String>,
    /// 6x6 covariances at TCA in each object's own RTN axes (m², m²/s, m²/s²).
    pub cov1_rtn: Option<M6>,
    pub cov2_rtn: Option<M6>,
    /// Hard-body radius (m), when the source states one.
    pub hbr_m: Option<f64>,
    pub mahalanobis: Option<f64>,
    pub ephemeris1: Option<String>,
    pub ephemeris2: Option<String>,
    pub covariance_method1: Option<String>,
    pub covariance_method2: Option<String>,
    /// End of the observation span used in each object's orbit determination (s, as tca_s).
    pub last_obs1_s: Option<f64>,
    pub last_obs2_s: Option<f64>,
    /// Message creation time (s, as tca_s).
    pub created_s: Option<f64>,
    /// Which object's screening criteria triggered the event (answer-key rows).
    pub met_criteria: Option<(bool, bool)>,
    /// Source file names as given in the row (answer-key rows).
    pub files: Option<(String, String)>,
}

impl Event {
    /// The pair as an unordered key.
    pub fn pair(&self) -> (String, String) {
        if self.obj1 <= self.obj2 {
            (self.obj1.clone(), self.obj2.clone())
        } else {
            (self.obj2.clone(), self.obj1.clone())
        }
    }

    /// Relative speed: as given, else from the states.
    pub fn vrel(&self) -> Option<f64> {
        self.vrel_m_s.or_else(|| match (self.state1, self.state2) {
            (Some(a), Some(b)) => Some(norm(sub([b[3], b[4], b[5]], [a[3], a[4], a[5]]))),
            _ => None,
        })
    }

    /// Orbital period of the primary (s), from its state.
    pub fn primary_period_s(&self) -> Option<f64> {
        let s = self.state1?;
        let p = elements([s[0], s[1], s[2]], [s[3], s[4], s[5]]).3;
        p.is_finite().then_some(p)
    }

    pub fn primary_class(&self) -> &'static str {
        match self.state1 {
            Some(s) => object_class([s[0], s[1], s[2]], [s[3], s[4], s[5]]),
            None => "unknown",
        }
    }
}

/// Normalise an object id: trimmed, upper case, leading zeros of an all-digit id removed.
pub fn norm_id(s: &str) -> String {
    let t = s.trim().to_ascii_uppercase();
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        let z = t.trim_start_matches('0');
        return if z.is_empty() { "0".into() } else { z.into() };
    }
    t
}

// ── Output hygiene ──────────────────────────────────────────────────────────────────────

/// Significant digits kept in the published result (so it hashes the same everywhere).
const SIG_DIGITS: usize = 9;

pub fn round_sig(x: f64) -> f64 {
    if x == 0.0 {
        return 0.0;
    }
    format!("{:.*e}", SIG_DIGITS - 1, x).parse().unwrap_or(x)
}

/// NaN and infinities become null; every other non-integer is rounded to 9 significant
/// digits.
pub fn sanitise(v: &mut Value) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(sanitise),
        Value::Object(o) => o.values_mut().for_each(sanitise),
        Value::Number(n) if n.is_f64() => match n.as_f64() {
            Some(x) if x.is_finite() => *v = serde_json::json!(round_sig(x)),
            _ => *v = Value::Null,
        },
        _ => {}
    }
}

/// Percentile (nearest-rank on the sorted values), `None` when empty.
pub fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let k = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    Some(sorted[k.clamp(1, sorted.len()) - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_round_trip_and_forms() {
        let t = parse_utc("2025-01-01T12:00:00Z").unwrap();
        assert_eq!(t, 9132.5 * 86_400.0);
        assert_eq!(fmt_utc(t), "2025-01-01T12:00:00.000000Z");
        assert_eq!(parse_utc("2025-001T12:00:00").unwrap(), t);
        assert_eq!(parse_utc("2025-01-01 12:00:00.000").unwrap(), t);
        let d = parse_utc("2024-366T23:59:59.5").unwrap();
        assert_eq!(fmt_utc(d), "2024-12-31T23:59:59.500000Z");
        assert!((jd_to_s(2_460_677.0) - t).abs() < 1e-3);
        assert!(parse_utc("01/01/2025").is_err());
        assert!(parse_utc("2025-13-01").is_err());
    }

    #[test]
    fn rtn_and_class() {
        let r = [7_000_000.0, 0.0, 0.0];
        let v = [0.0, 7_546.0, 0.0];
        let m = rtn_matrix(r, v);
        assert!((m[0][0] - 1.0).abs() < 1e-12 && m[0][1] == 0.0);
        assert!((m[1][1] - 1.0).abs() < 1e-12);
        assert!((m[2][2] - 1.0).abs() < 1e-12);
        assert_eq!(object_class(r, v), "LEO");
        assert_eq!(sfsh_regime(r, v).0, 4); // perigee ~620 km
        let rg = [42_164_000.0, 0.0, 0.0];
        let vg = [0.0, 3_074.7, 0.0];
        assert_eq!(object_class(rg, vg), "GEO");
        assert_eq!(sfsh_regime(rg, vg).0, 1);
        assert_eq!(object_class(r, [0.0, 12_000.0, 0.0]), "hyperbolic");
    }

    #[test]
    fn bhattacharyya_is_zero_for_equal_and_grows() {
        let a = [[4.0, 0.0, 0.0], [0.0, 9.0, 0.0], [0.0, 0.0, 1.0]];
        assert!(bhattacharyya_cov(&a, &a).unwrap().abs() < 1e-12);
        let mut b = a;
        b[1][1] = 36.0; // in-track sigma doubled
        let d = bhattacharyya_cov(&a, &b).unwrap();
        // 1-D closed form: ½ ln((σ₁²+σ₂²)/(2σ₁σ₂)).
        assert!((d - 0.5 * ((9.0f64 + 36.0) / (2.0 * 3.0 * 6.0)).ln()).abs() < 1e-12);
        assert!(!is_psd(&m3_rows(&[
            [1.0, 2.0, 0.0],
            [2.0, 1.0, 0.0],
            [0.0, 0.0, 1.0]
        ])));
        assert!(is_psd(&m3_rows(&a)));
    }

    #[test]
    fn ids() {
        assert_eq!(norm_id(" 00005 "), "5");
        assert_eq!(norm_id("101388"), "101388");
        assert_eq!(norm_id("2025-001a"), "2025-001A");
    }
}
