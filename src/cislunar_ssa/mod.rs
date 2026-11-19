// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # Cislunar space situational awareness: catalogue build-up and debris environment
//!
//! The cislunar extension of the `pro-cislunar-observability-study` kind (an `[ssa]` section
//! in its scenario selects it), aimed at the two ESA cislunar studies open to Estonia
//! (research 10: "Cislunar catalogue build-up demonstration", ESA 198267, and "Debris
//! environment modelling in cis-lunar space", ESA 193155):
//!
//! * [`iod`]: angles-only initial orbit determination in cislunar space (a range-hypothesis
//!   grid with a three-body boundary-value solve per cell) and its least-squares refinement
//!   (Levenberg–Marquardt) with a formal covariance;
//! * [`catalogue`]: catalogue build-up from tracklets: association of each new tracklet to a
//!   catalogued object by a Mahalanobis gate and a joint refit, otherwise a new object from
//!   initial orbit determination, otherwise an uncorrelated track;
//! * [`taxonomy`]: an orbit-family taxonomy (DRO, NRHO, L1/L2 halo, Lyapunov, lunar frozen
//!   ELFO-class, low lunar orbit, Earth orbit, escape, impact) from documented feature rules;
//! * [`debris`]: a simple debris-environment risk estimate along a trajectory: catalogued
//!   objects with their propagated covariance, sampled break-up clouds and diffuse background
//!   densities, combined into an expected collision count;
//! * [`tdm`]: angles (`ANGLE_TYPE = RADEC`) in a CCSDS Tracking Data Message, read through the
//!   open engine's TDM parser, and written back for synthetic campaigns.
//!
//! ## Model fidelity (stated in every result)
//! Dynamics are the open engine's Earth–Moon circular restricted three-body problem
//! ([`kshana::cr3bp`], adaptive flow of [`kshana::cr3bp_family`]); no solar gravity, solar
//! radiation pressure, lunar or Earth gravity harmonics, or eccentric lunar orbit in the
//! dynamics. Observations are mapped to the ICRF on the instantaneous Earth–Moon line of the
//! open engine's analytical Moon series ([`kshana::ephem::moon_icrf`], about 0.05 degrees
//! against JPL Horizons), stations through the open engine's IAU 2006/2000A CIO chain with
//! UT1 = UTC and no polar motion; no light time, aberration or refraction. Evidence class:
//! MODELLED. Synthetic campaigns are scored against the truth they were generated from, which
//! checks the method's internal consistency, not its performance on real data.

#![allow(clippy::needless_range_loop)] // index loops read as the matrix algebra they implement

pub mod catalogue;
pub mod debris;
pub mod iod;
pub mod synthetic;
pub mod taxonomy;
pub mod tdm;

use std::path::{Path, PathBuf};

use kshana::api::KshanaError;
use kshana::cr3bp::{Cr3bpState, EARTH_MOON_DIST_KM, EARTH_MOON_MU, SIDEREAL_MONTH_DAYS};
use kshana::cr3bp_family::{propagate_adaptive, propagate_stm_adaptive, FlowTolerance};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The result schema.
pub const SCHEMA: &str = "arksa-cislunar-ssa/1";

/// The honesty label on every result.
pub const LABEL: &str = "MODELLED cislunar catalogue build-up and debris-environment estimate. \
MEASURED: nothing; observations are the angles supplied (CCSDS TDM files named by SHA-256) or a \
synthetic campaign generated from stated truth orbits. MODELLED: Earth-Moon circular restricted \
three-body dynamics (no solar gravity, radiation pressure, gravity harmonics or lunar \
eccentricity in the dynamics); the ICRF mapping on the instantaneous Earth-Moon line of an \
analytical Moon series (about 0.05 degrees); stations with UT1 = UTC and no polar motion; no \
light time, aberration or refraction; formal covariances that assume the stated angle noise; \
the family taxonomy rules and their thresholds; the debris populations and densities, which are \
scenario inputs, not a validated environment model. Scores on a synthetic campaign check internal \
consistency against the truth it was generated from, not performance on real data. Not a \
validation, certification or operational catalogue, and not endorsed by any agency.";

/// Sources for the methods, units and thresholds used.
pub const REFERENCES: &[&str] = &[
    "CCSDS 503.0-B-2, Tracking Data Message (TDM), Blue Book, June 2020 (ANGLE_1/ANGLE_2, ANGLE_TYPE = RADEC, degrees)",
    "Szebehely, Theory of Orbits: The Restricted Problem of Three Bodies (1967): CR3BP, Jacobi constant, time-reversal symmetry",
    "Milani, Gronchi et al., Orbit determination with very short arcs I: admissible regions, Celestial Mechanics and Dynamical Astronomy 90 (2004): range-hypothesis initial orbit determination",
    "Tapley, Schutz and Born, Statistical Orbit Determination (2004), ch. 4: weighted batch least squares and its formal covariance",
    "Zimovan-Spreen, Howell and Davis, Near rectilinear halo orbits and nearby higher-period dynamical structures, Celestial Mechanics and Dynamical Astronomy 132 (2020): NRHO perilune range (about 1,850 to 17,350 km) used as the NRHO/halo boundary",
    "Ely, Stable constellations of frozen elliptical inclined lunar orbits, Journal of the Astronautical Sciences 53 (2005): ELFO element band (critical argument of perilune 90 or 270 degrees)",
    "Montenbruck and Gill, Satellite Orbits (2000), sec. 3.3.2: analytical Moon series",
    "ESA EXPRO+ 198267 (S3-SD-13 cislunar catalogue build-up demonstration) and 193155 (debris environment modelling in cis-lunar space), intended tenders: scope only, no ESA involvement in this work",
];

/// Earth–Moon mass ratio used throughout.
pub const MU: f64 = EARTH_MOON_MU;
/// Length unit (km): the CR3BP Earth–Moon distance.
pub const L_KM: f64 = EARTH_MOON_DIST_KM;
/// Moon mean radius (km).
pub const MOON_RADIUS_KM: f64 = kshana::cr3bp::MOON_RADIUS_KM;
/// Earth equatorial radius (km).
pub const EARTH_RADIUS_KM: f64 = 6_378.137;
/// TT − UTC (s) since 2017-01-01 (37 leap seconds + 32.184 s); used only to evaluate the
/// analytical Moon series and the CIO chain, both far coarser than a leap-second error.
pub const TT_MINUS_UTC_S: f64 = 69.184;
/// Julian date of 2000-01-01T00:00:00 UTC (the zero of [`crate::conjunction::parse_utc`]).
pub const JD_2000_UTC: f64 = 2_451_544.5;
/// One arcsecond in radians.
pub const ARCSEC: f64 = std::f64::consts::PI / 648_000.0;

/// Time unit (s): one sidereal month over 2π.
pub fn t_unit_s() -> f64 {
    SIDEREAL_MONTH_DAYS * 86_400.0 / std::f64::consts::TAU
}

/// Velocity unit (km/s).
pub fn v_unit_kms() -> f64 {
    L_KM / t_unit_s()
}

/// A synodic (rotating-frame, barycentric, nondimensional) CR3BP state `[x, y, z, ẋ, ẏ, ż]`.
pub type S6 = [f64; 6];
/// A 6×6 matrix.
pub type M6 = [[f64; 6]; 6];
pub type V3 = [f64; 3];

pub(crate) fn bad(m: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(format!("cislunar-ssa: {}", m.into()))
}

pub(crate) fn to_cr3bp(s: &S6) -> Cr3bpState {
    Cr3bpState {
        r: [s[0], s[1], s[2]],
        v: [s[3], s[4], s[5]],
    }
}

pub(crate) fn from_cr3bp(c: &Cr3bpState) -> S6 {
    [c.r[0], c.r[1], c.r[2], c.v[0], c.v[1], c.v[2]]
}

pub(crate) fn flow_tol() -> FlowTolerance {
    FlowTolerance {
        rtol: 1e-12,
        atol: 1e-13,
        h_max: 0.02,
    }
}

/// Time-reversal mirror of the CR3BP: `(x, −y, z, −ẋ, ẏ, −ż)` flows backward in time.
fn mirror(s: &S6) -> S6 {
    [s[0], -s[1], s[2], -s[3], s[4], -s[5]]
}

/// Propagate a synodic state by `dt` (nondimensional, either sign). `None` if the flow fails
/// (for example a collision with a primary).
pub fn propagate(s: &S6, dt: f64) -> Option<S6> {
    if dt == 0.0 {
        return Some(*s);
    }
    let out = if dt > 0.0 {
        from_cr3bp(&propagate_adaptive(&to_cr3bp(s), MU, dt, &flow_tol()))
    } else {
        mirror(&from_cr3bp(&propagate_adaptive(
            &to_cr3bp(&mirror(s)),
            MU,
            -dt,
            &flow_tol(),
        )))
    };
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// Propagate a synodic state and its state-transition matrix by `dt` (either sign).
pub fn propagate_stm(s: &S6, dt: f64) -> Option<(S6, M6)> {
    if dt >= 0.0 {
        let (e, phi) = propagate_stm_adaptive(&to_cr3bp(s), MU, dt, &flow_tol());
        let e = from_cr3bp(&e);
        return e.iter().all(|v| v.is_finite()).then_some((e, phi));
    }
    // Backward: Φ(−t) = D Φ_m(t) D with D = diag(1, −1, 1, −1, 1, −1).
    let (e, phi_m) = propagate_stm_adaptive(&to_cr3bp(&mirror(s)), MU, -dt, &flow_tol());
    let e = mirror(&from_cr3bp(&e));
    if !e.iter().all(|v| v.is_finite()) {
        return None;
    }
    let d = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
    let mut phi = [[0.0; 6]; 6];
    for i in 0..6 {
        for j in 0..6 {
            phi[i][j] = d[i] * phi_m[i][j] * d[j];
        }
    }
    Some((e, phi))
}

/// Distance (km) from the Moon of a synodic position.
pub fn moon_dist_km(s: &[f64]) -> f64 {
    let d = [s[0] - (1.0 - MU), s[1], s[2]];
    norm3(d) * L_KM
}

/// Distance (km) from the Earth of a synodic position.
pub fn earth_dist_km(s: &[f64]) -> f64 {
    let d = [s[0] + MU, s[1], s[2]];
    norm3(d) * L_KM
}

// ── Small vector algebra ────────────────────────────────────────────────────────────────

pub(crate) fn norm3(a: V3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
pub(crate) fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn dot3(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn cross3(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(crate) fn unit3(a: V3) -> V3 {
    let n = norm3(a);
    [a[0] / n, a[1] / n, a[2] / n]
}

/// Solve `A x = b` (n×n) by Gaussian elimination with partial pivoting. `None` if singular.
pub(crate) fn solve(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut m: Vec<Vec<f64>> = a.to_vec();
    let mut x = b.to_vec();
    for k in 0..n {
        let p = (k..n).max_by(|&i, &j| m[i][k].abs().total_cmp(&m[j][k].abs()))?;
        if m[p][k].abs() < 1e-300 || !m[p][k].is_finite() {
            return None;
        }
        m.swap(k, p);
        x.swap(k, p);
        for i in k + 1..n {
            let f = m[i][k] / m[k][k];
            if f != 0.0 {
                for j in k..n {
                    m[i][j] -= f * m[k][j];
                }
                x[i] -= f * x[k];
            }
        }
    }
    for k in (0..n).rev() {
        let s: f64 = (k + 1..n).map(|j| m[k][j] * x[j]).sum();
        x[k] = (x[k] - s) / m[k][k];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Inverse of a symmetric positive-definite matrix through AK1's Cholesky factor.
pub(crate) fn spd_inverse(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    crate::conjunction::cholesky(a)?;
    let mut inv = vec![vec![0.0; n]; n];
    for j in 0..n {
        let mut e = vec![0.0; n];
        e[j] = 1.0;
        let col = solve(a, &e)?;
        for i in 0..n {
            inv[i][j] = col[i];
        }
    }
    // Symmetrise.
    for i in 0..n {
        for j in i + 1..n {
            let m = 0.5 * (inv[i][j] + inv[j][i]);
            inv[i][j] = m;
            inv[j][i] = m;
        }
    }
    Some(inv)
}

// ── The instantaneous Earth–Moon frame ─────────────────────────────────────────────────

/// The Earth–Moon rotating frame at one epoch, tied to the ICRF through the analytical Moon
/// series: `c` holds the frame axes (columns: x̂ toward the Moon, ẑ along the Moon's orbital
/// angular momentum, ŷ completing the triad) in ICRF components and `l_m` the Earth–Moon
/// distance (m) that scales the nondimensional coordinates.
#[derive(Clone, Copy, Debug)]
pub struct EmFrame {
    pub c: [[f64; 3]; 3],
    pub l_m: f64,
}

/// Julian date (TT) of a UTC instant in seconds from 2000-01-01.
pub fn jd_tt(utc_s: f64) -> f64 {
    JD_2000_UTC + (utc_s + TT_MINUS_UTC_S) / 86_400.0
}

/// Julian date (UT1 taken equal to UTC).
pub fn jd_ut1(utc_s: f64) -> f64 {
    JD_2000_UTC + utc_s / 86_400.0
}

impl EmFrame {
    /// The frame at a UTC instant (seconds from 2000-01-01).
    pub fn at(utc_s: f64) -> Self {
        let jd = jd_tt(utc_s);
        let r = kshana::ephem::moon_icrf(jd);
        let v = kshana::ephem::moon_icrf_velocity(jd);
        let x = unit3(r);
        let z = unit3(cross3(r, v));
        let y = cross3(z, x);
        Self {
            c: [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]],
            l_m: norm3(r),
        }
    }

    /// Frame axes applied to a synodic-axes vector (to ICRF axes).
    pub fn to_icrf_axes(&self, a: V3) -> V3 {
        let c = &self.c;
        [
            c[0][0] * a[0] + c[0][1] * a[1] + c[0][2] * a[2],
            c[1][0] * a[0] + c[1][1] * a[1] + c[1][2] * a[2],
            c[2][0] * a[0] + c[2][1] * a[1] + c[2][2] * a[2],
        ]
    }

    /// ICRF-axes vector to synodic axes.
    pub fn to_syn_axes(&self, a: V3) -> V3 {
        let c = &self.c;
        [
            c[0][0] * a[0] + c[1][0] * a[1] + c[2][0] * a[2],
            c[0][1] * a[0] + c[1][1] * a[1] + c[2][1] * a[2],
            c[0][2] * a[0] + c[1][2] * a[1] + c[2][2] * a[2],
        ]
    }

    /// Synodic barycentric nondimensional position to geocentric ICRF metres.
    pub fn syn_to_geo(&self, s: &[f64]) -> V3 {
        let g = self.to_icrf_axes([s[0] + MU, s[1], s[2]]);
        [g[0] * self.l_m, g[1] * self.l_m, g[2] * self.l_m]
    }

    /// Geocentric ICRF metres to a synodic barycentric nondimensional position.
    pub fn geo_to_syn(&self, g: V3) -> V3 {
        let s = self.to_syn_axes([g[0] / self.l_m, g[1] / self.l_m, g[2] / self.l_m]);
        [s[0] - MU, s[1], s[2]]
    }
}

/// Right ascension and declination (rad) of a direction.
pub fn radec(u: V3) -> (f64, f64) {
    let n = norm3(u);
    let ra = u[1].atan2(u[0]).rem_euclid(std::f64::consts::TAU);
    (ra, (u[2] / n).clamp(-1.0, 1.0).asin())
}

/// Unit vector of a right ascension and declination (rad).
pub fn radec_unit(ra: f64, dec: f64) -> V3 {
    [dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()]
}

/// Wrap an angle difference into `(−π, π]`.
pub fn wrap_pi(a: f64) -> f64 {
    let t = std::f64::consts::TAU;
    let r = a.rem_euclid(t);
    if r > std::f64::consts::PI {
        r - t
    } else {
        r
    }
}

// ── Observers ──────────────────────────────────────────────────────────────────────────

fn d_mask() -> f64 {
    10.0
}

/// An observer: the geocentre, a ground station, or a spacecraft on a CR3BP orbit.
#[derive(Debug, Clone, Deserialize)]
pub struct ObserverConfig {
    /// The name a TDM `PARTICIPANT_1` or a synthetic track uses.
    pub name: String,
    /// `geocentre`, `station` or `orbit`.
    pub kind: String,
    /// Station geodetic latitude, longitude (degrees) and height (m, WGS-84).
    #[serde(default)]
    pub lat_deg: f64,
    #[serde(default)]
    pub lon_deg: f64,
    #[serde(default)]
    pub alt_m: f64,
    /// Station elevation mask (degrees) for synthetic campaigns.
    #[serde(default = "d_mask")]
    pub elevation_mask_deg: f64,
    /// Spacecraft observer: synodic state at the scenario epoch, or a preset name
    /// (`synthetic::preset_state`).
    #[serde(default)]
    pub state: Option<S6>,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub phase_frac: f64,
}

/// A resolved observer.
#[derive(Debug, Clone)]
pub enum Observer {
    Geocentre,
    Station {
        ecef_m: V3,
        up_ecef: V3,
        mask_rad: f64,
    },
    Orbit {
        /// Synodic state at the scenario epoch.
        state: S6,
    },
}

impl Observer {
    pub fn from_config(c: &ObserverConfig) -> Result<Self, KshanaError> {
        match c.kind.to_ascii_lowercase().as_str() {
            "geocentre" | "geocenter" => Ok(Observer::Geocentre),
            "station" => {
                if !(-90.0..=90.0).contains(&c.lat_deg) {
                    return Err(bad(format!("observer {}: latitude out of range", c.name)));
                }
                let g = kshana::frames::Geodetic {
                    lat_rad: c.lat_deg.to_radians(),
                    lon_rad: c.lon_deg.to_radians(),
                    alt_m: c.alt_m,
                };
                let (la, lo) = (g.lat_rad, g.lon_rad);
                Ok(Observer::Station {
                    ecef_m: kshana::frames::geodetic_to_ecef(g),
                    up_ecef: [la.cos() * lo.cos(), la.cos() * lo.sin(), la.sin()],
                    mask_rad: c.elevation_mask_deg.to_radians(),
                })
            }
            "orbit" => {
                let state = match (&c.state, &c.preset) {
                    (Some(s), _) => *s,
                    (None, Some(p)) => synthetic::preset_state(p, c.phase_frac)?,
                    (None, None) => {
                        return Err(bad(format!(
                            "observer {}: an orbit observer needs `state` or `preset`",
                            c.name
                        )))
                    }
                };
                Ok(Observer::Orbit { state })
            }
            other => Err(bad(format!(
                "observer {}: unknown kind '{other}' (geocentre, station, orbit)",
                c.name
            ))),
        }
    }

    /// Geocentric ICRF position (m) at `utc_s`, with the scenario epoch `epoch_utc_s` for an
    /// orbit observer, and the local-vertical unit vector (stations only).
    pub fn position(&self, utc_s: f64, epoch_utc_s: f64) -> Option<(V3, Option<V3>)> {
        match self {
            Observer::Geocentre => Some(([0.0; 3], None)),
            Observer::Station {
                ecef_m, up_ecef, ..
            } => {
                let (tt, ut1) = (jd_tt(utc_s), jd_ut1(utc_s));
                let r = kshana::cio::itrs_to_gcrs(*ecef_m, tt, ut1, 0.0, 0.0);
                let up = kshana::cio::itrs_to_gcrs(*up_ecef, tt, ut1, 0.0, 0.0);
                Some((r, Some(up)))
            }
            Observer::Orbit { state } => {
                let s = propagate(state, (utc_s - epoch_utc_s) / t_unit_s())?;
                Some((EmFrame::at(utc_s).syn_to_geo(&s), None))
            }
        }
    }
}

// ── Observations and the measurement model ─────────────────────────────────────────────

/// One angles-only observation (right ascension and declination in the ICRF, radians).
#[derive(Clone, Debug)]
pub struct AngleObs {
    pub utc_s: f64,
    pub observer: String,
    pub tracklet: String,
    pub ra: f64,
    pub dec: f64,
    pub sigma: f64,
}

/// Geometry precomputed for one observation (independent of the target state).
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    /// Nondimensional time from the scenario epoch.
    pub tau: f64,
    pub frame: EmFrame,
    /// Observer geocentric ICRF position (m).
    pub obs_m: V3,
}

/// The observations of a scenario with their geometry, sorted by time.
#[derive(Clone, Debug)]
pub struct Problem {
    pub epoch_utc_s: f64,
    pub obs: Vec<AngleObs>,
    pub geo: Vec<Geometry>,
}

impl Problem {
    /// Sort the observations by time and precompute the geometry.
    pub fn new(
        epoch_utc_s: f64,
        mut obs: Vec<AngleObs>,
        observers: &[(String, Observer)],
    ) -> Result<Self, KshanaError> {
        obs.sort_by(|a, b| {
            a.utc_s
                .total_cmp(&b.utc_s)
                .then(a.tracklet.cmp(&b.tracklet))
        });
        let mut geo = Vec::with_capacity(obs.len());
        for o in &obs {
            let ob = observers
                .iter()
                .find(|(n, _)| *n == o.observer)
                .map(|(_, ob)| ob)
                .ok_or_else(|| bad(format!("no observer named '{}' is configured", o.observer)))?;
            let (obs_m, _) = ob
                .position(o.utc_s, epoch_utc_s)
                .ok_or_else(|| bad(format!("observer '{}' could not be propagated", o.observer)))?;
            geo.push(Geometry {
                tau: (o.utc_s - epoch_utc_s) / t_unit_s(),
                frame: EmFrame::at(o.utc_s),
                obs_m,
            });
        }
        Ok(Self {
            epoch_utc_s,
            obs,
            geo,
        })
    }

    /// Predicted right ascension and declination of a synodic position at observation `i`.
    pub fn predict(&self, i: usize, s: &[f64]) -> (f64, f64) {
        let g = &self.geo[i];
        radec(sub3(g.frame.syn_to_geo(s), g.obs_m))
    }

    /// Whitened residuals `[(Δα cos δ)/σ, Δδ/σ]` (predicted minus observed) of the
    /// observations `idx` (ascending) for a synodic state `x` at nondimensional time `tau_ref`.
    pub fn residuals(&self, idx: &[usize], tau_ref: f64, x: &S6) -> Option<Vec<f64>> {
        let mut out = Vec::with_capacity(2 * idx.len());
        // Propagate outward from tau_ref in both directions.
        let split = idx.partition_point(|&i| self.geo[i].tau < tau_ref);
        let mut slots = vec![[0.0; 2]; idx.len()];
        let mut s = *x;
        let mut t = tau_ref;
        for k in split..idx.len() {
            let i = idx[k];
            s = propagate(&s, self.geo[i].tau - t)?;
            t = self.geo[i].tau;
            slots[k] = self.whiten(i, &s);
        }
        let mut s = *x;
        let mut t = tau_ref;
        for k in (0..split).rev() {
            let i = idx[k];
            s = propagate(&s, self.geo[i].tau - t)?;
            t = self.geo[i].tau;
            slots[k] = self.whiten(i, &s);
        }
        for r in slots {
            out.extend_from_slice(&r);
        }
        out.iter().all(|v| v.is_finite()).then_some(out)
    }

    /// [`Self::residuals`] with the analytic Jacobian of the whitened residuals with respect
    /// to the state at `tau_ref`: the angle partials with respect to the synodic position
    /// (through the frame matrix and the Earth–Moon distance) times the position rows of the
    /// variational state-transition matrix accumulated along the arc.
    pub fn residuals_jac(
        &self,
        idx: &[usize],
        tau_ref: f64,
        x: &S6,
    ) -> Option<(Vec<f64>, Vec<[f64; 6]>)> {
        let split = idx.partition_point(|&i| self.geo[i].tau < tau_ref);
        let mut res = vec![[0.0; 2]; idx.len()];
        let mut jac = vec![[[0.0; 6]; 2]; idx.len()];
        let mut ident = [[0.0; 6]; 6];
        for (k, row) in ident.iter_mut().enumerate() {
            row[k] = 1.0;
        }
        for dir in [true, false] {
            let mut s = *x;
            let mut t = tau_ref;
            let mut phi = ident;
            let order: Vec<usize> = if dir {
                (split..idx.len()).collect()
            } else {
                (0..split).rev().collect()
            };
            for k in order {
                let i = idx[k];
                let (sn, step) = propagate_stm(&s, self.geo[i].tau - t)?;
                s = sn;
                t = self.geo[i].tau;
                let mut acc = [[0.0; 6]; 6];
                for a in 0..6 {
                    for b in 0..6 {
                        acc[a][b] = (0..6).map(|c| step[a][c] * phi[c][b]).sum();
                    }
                }
                phi = acc;
                res[k] = self.whiten(i, &s);
                let g = &self.geo[i];
                let d = sub3(g.frame.syn_to_geo(&s), g.obs_m);
                let (dx, dy, dz) = (d[0], d[1], d[2]);
                let rho2 = dx * dx + dy * dy;
                let r2 = rho2 + dz * dz;
                let rho = rho2.sqrt();
                let dra = [-dy / rho2, dx / rho2, 0.0];
                let ddec = [-dx * dz / (r2 * rho), -dy * dz / (r2 * rho), rho / r2];
                let o = &self.obs[i];
                let w = [o.dec.cos() / o.sigma, 1.0 / o.sigma];
                for (row, (part, wt)) in [(dra, w[0]), (ddec, w[1])].iter().enumerate() {
                    // ∂angle/∂x_syn = ∂angle/∂d · l C.
                    let mut a = [0.0; 3];
                    for j in 0..3 {
                        a[j] = (0..3).map(|q| part[q] * g.frame.c[q][j]).sum::<f64>() * g.frame.l_m;
                    }
                    for b in 0..6 {
                        jac[k][row][b] = wt * (0..3).map(|j| a[j] * phi[j][b]).sum::<f64>();
                    }
                }
            }
        }
        let f: Vec<f64> = res.iter().flat_map(|r| r.iter().copied()).collect();
        let h: Vec<[f64; 6]> = jac.iter().flat_map(|r| r.iter().copied()).collect();
        (f.iter().all(|v| v.is_finite()) && h.iter().all(|r| r.iter().all(|v| v.is_finite())))
            .then_some((f, h))
    }

    fn whiten(&self, i: usize, s: &S6) -> [f64; 2] {
        let o = &self.obs[i];
        let (ra, dec) = self.predict(i, s);
        [
            wrap_pi(ra - o.ra) * o.dec.cos() / o.sigma,
            (dec - o.dec) / o.sigma,
        ]
    }

    /// Indices of the observations of each tracklet, in first-observation order.
    pub fn tracklets(&self) -> Vec<(String, Vec<usize>)> {
        let mut out: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, o) in self.obs.iter().enumerate() {
            match out.iter_mut().find(|(t, _)| *t == o.tracklet) {
                Some((_, v)) => v.push(i),
                None => out.push((o.tracklet.clone(), vec![i])),
            }
        }
        out
    }
}

/// A synodic state as a Moon-centred state in the synodic axes: position (km) and velocity
/// relative to the rotating frame (km/s).
pub fn state_km(s: &S6) -> [f64; 6] {
    let v = v_unit_kms();
    [
        (s[0] - (1.0 - MU)) * L_KM,
        s[1] * L_KM,
        s[2] * L_KM,
        s[3] * v,
        s[4] * v,
        s[5] * v,
    ]
}

/// One-sigma position (km) and velocity (m/s) from a nondimensional covariance (square root
/// of the trace of each block).
pub fn sigmas(p: &M6) -> (f64, f64) {
    let pos = (p[0][0] + p[1][1] + p[2][2]).max(0.0).sqrt() * L_KM;
    let vel = (p[3][3] + p[4][4] + p[5][5]).max(0.0).sqrt() * v_unit_kms() * 1e3;
    (pos, vel)
}

/// Round to 9 significant digits so the result document is identical on every platform.
pub(crate) fn r9(x: f64) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    json!(crate::conjunction::round_sig(x))
}

pub(crate) fn sha256_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

// ── Scenario section ───────────────────────────────────────────────────────────────────

fn d_tdm_sigma() -> f64 {
    1.0
}

/// The `[ssa]` section of a `pro-cislunar-observability-study` scenario.
#[derive(Debug, Clone, Deserialize)]
pub struct SsaConfig {
    /// Scenario epoch (UTC; ISO or CCSDS day-of-year). Defaults to the first observation.
    #[serde(default)]
    pub epoch_utc: Option<String>,
    #[serde(default)]
    pub observers: Vec<ObserverConfig>,
    /// CCSDS TDM files with RADEC angles (read relative to the working directory).
    #[serde(default)]
    pub tdm_files: Vec<String>,
    /// Angle noise (arcsec, one sigma) assigned to TDM angles (the TDM carries none).
    #[serde(default = "d_tdm_sigma")]
    pub tdm_sigma_arcsec: f64,
    /// A synthetic campaign generated from stated truth orbits.
    #[serde(default)]
    pub synthetic: Option<synthetic::SyntheticConfig>,
    #[serde(default)]
    pub iod: iod::IodConfig,
    #[serde(default)]
    pub association: catalogue::AssocConfig,
    #[serde(default)]
    pub taxonomy: taxonomy::TaxonomyConfig,
    /// Debris-environment risk along a trajectory.
    #[serde(default)]
    pub debris: Option<debris::DebrisConfig>,
}

/// A finished study: the result document, the catalogue CSV, the figure and (for a synthetic
/// campaign) the generated observations as a TDM.
#[derive(Debug, Clone)]
pub struct Study {
    pub doc: Value,
    pub csv: String,
    pub svg: String,
    pub tdm: Option<String>,
}

/// Run the `[ssa]` section.
pub fn run_study(cfg: &SsaConfig) -> Result<Study, KshanaError> {
    let mut observers: Vec<(String, Observer)> = Vec::new();
    for c in &cfg.observers {
        if observers.iter().any(|(n, _)| *n == c.name) {
            return Err(bad(format!("observer '{}' is defined twice", c.name)));
        }
        observers.push((c.name.clone(), Observer::from_config(c)?));
    }
    let mut obs: Vec<AngleObs> = Vec::new();
    let mut inputs: Vec<Value> = Vec::new();
    let sigma = cfg.tdm_sigma_arcsec * ARCSEC;
    if !(sigma.is_finite() && sigma > 0.0) {
        return Err(bad("tdm_sigma_arcsec must be positive"));
    }
    for f in &cfg.tdm_files {
        let text = std::fs::read_to_string(f).map_err(|e| bad(format!("{f}: {e}")))?;
        let mut o = tdm::read_angles(&text, sigma)?;
        for a in &mut o {
            a.tracklet = format!("{}#{}", file_stem(f), a.tracklet);
        }
        inputs.push(
            json!({"file": f, "sha256": sha256_hex(text.as_bytes()), "observations": o.len()}),
        );
        obs.extend(o);
    }
    let epoch_cfg = cfg
        .epoch_utc
        .as_deref()
        .map(crate::conjunction::parse_utc)
        .transpose()?;
    let mut truth: Vec<synthetic::Truth> = Vec::new();
    let mut tdm_out = None;
    if let Some(sc) = &cfg.synthetic {
        let epoch = match epoch_cfg {
            Some(e) => e,
            None => crate::conjunction::parse_utc(&sc.epoch_utc)?,
        };
        let (o, t) = synthetic::generate(sc, epoch, &observers)?;
        tdm_out = Some(tdm::write_angles(&o, "ARKSA-SYNTHETIC"));
        inputs.push(json!({"synthetic": synthetic::describe(sc), "observations": o.len()}));
        obs.extend(o);
        truth = t;
    }
    if obs.is_empty() {
        return Err(bad(
            "no observations: give tdm_files or a [ssa.synthetic] campaign",
        ));
    }
    let epoch = match (epoch_cfg, &cfg.synthetic) {
        (Some(e), _) => e,
        (None, Some(sc)) => crate::conjunction::parse_utc(&sc.epoch_utc)?,
        (None, None) => obs.iter().map(|o| o.utc_s).fold(f64::INFINITY, f64::min),
    };
    let problem = Problem::new(epoch, obs, &observers)?;
    let cat = catalogue::build(&problem, &cfg.iod, &cfg.association);
    let mut families = Vec::new();
    for o in &cat.objects {
        families.push(taxonomy::classify_at(
            &o.fit.x,
            o.fit.cov.as_ref(),
            &cfg.taxonomy,
        ));
    }
    let scoring = (!truth.is_empty()).then(|| synthetic::score(&problem, &cat, &truth));
    let debris = match &cfg.debris {
        Some(d) => Some(debris::assess(d, &problem, &cat)?),
        None => None,
    };

    let catalogue_json: Vec<Value> = cat
        .objects
        .iter()
        .zip(&families)
        .map(|(o, f)| {
            let mut v = o.to_json(&problem);
            v["family"] = f.to_json();
            v
        })
        .collect();
    let n_tracklets = problem.tracklets().len();
    let mut doc = json!({
        "schema": SCHEMA,
        "kind": crate::packs::cislunar_observability_study_pack::CislunarObservabilityStudyScenario::KIND,
        "section": "ssa",
        "evidence_class": "MODELLED",
        "label": LABEL,
        "references": REFERENCES,
        "model": {
            "dynamics": format!("Earth-Moon circular restricted three-body problem, mu = {MU}, adaptive Dormand-Prince 5(4) flow (rtol 1e-12)"),
            "frames": "synodic (rotating, barycentric, nondimensional: L = 384,400 km, T = sidereal month / 2 pi) mapped to the ICRF on the instantaneous Earth-Moon line of an analytical Moon series",
            "observations": "right ascension and declination (ICRF) from the observer; no light time, aberration or refraction",
            "stations": "WGS-84 geodetic to ITRS, IAU 2006/2000A CIO chain to GCRS with UT1 = UTC and no polar motion",
            "time": "UTC seconds from 2000-01-01 without leap seconds; TT = UTC + 69.184 s",
        },
        "epoch_utc": crate::conjunction::fmt_utc(epoch),
        "inputs": inputs,
        "observations": problem.obs.len(),
        "tracklets": n_tracklets,
        "catalogue": catalogue_json,
        "uncorrelated": cat.uncorrelated.iter().map(|(t, why)| json!({"tracklet": t, "reason": why})).collect::<Vec<_>>(),
        "log": cat.log,
    });
    if let Some(s) = scoring {
        doc["synthetic_scoring"] = s;
    }
    if let Some(d) = &debris {
        doc["debris"] = d.clone();
    }
    crate::conjunction::sanitise(&mut doc);
    let csv = catalogue_csv(&doc);
    let svg = figure(&problem, &cat, &truth, debris.as_ref());
    doc["summary"] = json!(summary(&doc));
    Ok(Study {
        doc,
        csv,
        svg,
        tdm: tdm_out,
    })
}

fn file_stem(f: &str) -> String {
    Path::new(f)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| f.to_string())
}

/// One line describing the result.
pub fn summary(doc: &Value) -> String {
    let n = doc["catalogue"].as_array().map(|a| a.len()).unwrap_or(0);
    let fams: Vec<String> = doc["catalogue"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| {
                    format!(
                        "{}={}",
                        o["id"].as_str().unwrap_or("?"),
                        o["family"]["family"].as_str().unwrap_or("?")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut s = format!(
        "cislunar-ssa ({}): {} observations in {} tracklets -> {} catalogued object(s) [{}], {} uncorrelated tracklet(s)",
        doc["evidence_class"].as_str().unwrap_or(""),
        doc["observations"],
        doc["tracklets"],
        n,
        fams.join(", "),
        doc["uncorrelated"].as_array().map(|a| a.len()).unwrap_or(0),
    );
    if let Some(p) = doc["debris"]["probability_any_collision"].as_f64() {
        s.push_str(&format!(
            "; debris: expected collisions {:.3e}, P(>=1) {:.3e}",
            doc["debris"]["expected_collisions"].as_f64().unwrap_or(0.0),
            p
        ));
    }
    s
}

/// The catalogue as CSV (one row per object).
pub fn catalogue_csv(doc: &Value) -> String {
    let mut s = String::from("# arksa-kpro cislunar-ssa catalogue (MODELLED); states are Moon-centred in synodic axes at the object epoch\n");
    s.push_str("id,epoch_utc,family,x_km,y_km,z_km,vx_kms,vy_kms,vz_kms,sigma_pos_km,sigma_vel_mps,reduced_chi2,rms_arcsec,observations,tracklets\n");
    let f = |v: &Value| match v.as_f64() {
        Some(x) => format!("{x}"),
        None => String::new(),
    };
    for o in doc["catalogue"].as_array().into_iter().flatten() {
        let st = &o["state_moon_synodic_km_kms"];
        s.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            o["id"].as_str().unwrap_or(""),
            o["epoch_utc"].as_str().unwrap_or(""),
            o["family"]["family"].as_str().unwrap_or(""),
            f(&st[0]),
            f(&st[1]),
            f(&st[2]),
            f(&st[3]),
            f(&st[4]),
            f(&st[5]),
            f(&o["sigma_pos_km"]),
            f(&o["sigma_vel_mps"]),
            f(&o["reduced_chi2"]),
            f(&o["rms_arcsec"]),
            o["observations"],
            o["tracklets"]
                .as_array()
                .map(|a| a
                    .iter()
                    .filter_map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(" "))
                .unwrap_or_default(),
        ));
    }
    s
}

/// A Markdown report of the result.
pub fn report_md(doc: &Value) -> String {
    let mut s = String::from("# Cislunar catalogue build-up and debris environment\n\n");
    s.push_str(&format!(
        "Evidence class: **{}**. Schema `{}`. Epoch {}.\n\n",
        doc["evidence_class"].as_str().unwrap_or(""),
        doc["schema"].as_str().unwrap_or(""),
        doc["epoch_utc"].as_str().unwrap_or("")
    ));
    s.push_str(&format!("{}\n\n", doc["label"].as_str().unwrap_or("")));
    s.push_str("## Catalogue\n\n| id | family | sigma pos (km) | sigma vel (m/s) | reduced chi2 | tracklets |\n|---|---|---|---|---|---|\n");
    for o in doc["catalogue"].as_array().into_iter().flatten() {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            o["id"].as_str().unwrap_or(""),
            o["family"]["label"].as_str().unwrap_or(""),
            o["sigma_pos_km"],
            o["sigma_vel_mps"],
            o["reduced_chi2"],
            o["tracklets"].as_array().map(|a| a.len()).unwrap_or(0)
        ));
    }
    if let Some(u) = doc["uncorrelated"].as_array() {
        s.push_str(&format!("\nUncorrelated tracklets: {}\n", u.len()));
    }
    if doc["synthetic_scoring"].is_object() {
        s.push_str(&format!(
            "\n## Synthetic scoring (internal consistency only)\n\n```\n{}\n```\n",
            serde_json::to_string_pretty(&doc["synthetic_scoring"]).unwrap_or_default()
        ));
    }
    if doc["debris"].is_object() {
        s.push_str(&format!(
            "\n## Debris environment (MODELLED)\n\nExpected collisions {} ; probability of at least one {}.\n",
            doc["debris"]["expected_collisions"], doc["debris"]["probability_any_collision"]
        ));
    }
    s.push_str("\n## References\n\n");
    for r in doc["references"].as_array().into_iter().flatten() {
        s.push_str(&format!("- {}\n", r.as_str().unwrap_or("")));
    }
    s
}

/// Write the study directory (`study.json`, `catalogue.csv`, `report.md`, `figure.svg`, and
/// for a synthetic campaign `observations.tdm`) and, with a seed, sign it as a whole with
/// Kshana Pro's dossier signature (`signature.json`; check with `arksa-kpro dossier verify`).
pub fn write_study(
    st: &Study,
    dir: &Path,
    signer: Option<(&[u8; 32], &str, &str)>,
) -> Result<Option<PathBuf>, KshanaError> {
    std::fs::create_dir_all(dir).map_err(|e| bad(format!("{}: {e}", dir.display())))?;
    let w = |name: &str, text: &str| {
        std::fs::write(dir.join(name), text).map_err(|e| bad(format!("{name}: {e}")))
    };
    w(
        "study.json",
        &(serde_json::to_string_pretty(&st.doc).map_err(|e| bad(e.to_string()))? + "\n"),
    )?;
    w("catalogue.csv", &st.csv)?;
    w("report.md", &report_md(&st.doc))?;
    w("figure.svg", &st.svg)?;
    if let Some(t) = &st.tdm {
        w("observations.tdm", t)?;
    }
    match signer {
        Some((seed, name, utc)) => Ok(Some(kshana_pro::signed_dossier::sign(
            dir, seed, name, utc,
        )?)),
        None => Ok(None),
    }
}

/// A deterministic rotating-frame (x–y) figure: catalogued orbits over the taxonomy window,
/// truth positions (synthetic), the debris trajectory and fragment positions.
fn figure(
    p: &Problem,
    cat: &catalogue::Catalogue,
    truth: &[synthetic::Truth],
    debris: Option<&Value>,
) -> String {
    let (w, h) = (640.0, 480.0);
    let span = 0.35; // nondimensional half-width around the Moon
    let cx = 1.0 - MU;
    let sx = |x: f64| w / 2.0 + (x - cx) / span * (h / 2.0) * 0.95;
    let sy = |y: f64| h / 2.0 - y / span * (h / 2.0) * 0.95;
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\" font-family=\"sans-serif\" font-size=\"11\">\n<rect width=\"{w}\" height=\"{h}\" fill=\"#fbf8f1\"/>\n<text x=\"10\" y=\"16\" fill=\"#1d2b44\">Cislunar catalogue, Earth-Moon rotating frame (x-y), MODELLED</text>\n"
    );
    s.push_str(&format!(
        "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"4\" fill=\"#888\"/><text x=\"{:.2}\" y=\"{:.2}\" fill=\"#555\">Moon</text>\n",
        sx(cx),
        sy(0.0),
        sx(cx) + 6.0,
        sy(0.0) - 6.0
    ));
    let colours = [
        "#1d2b44", "#a07a2c", "#2c7a5a", "#8a2c4a", "#4a5aa0", "#7a6a2c",
    ];
    for (k, o) in cat.objects.iter().enumerate() {
        let col = colours[k % colours.len()];
        let mut pts = Vec::new();
        let mut st = o.fit.x;
        let dt = 4.0 * std::f64::consts::PI / 2.0 / 240.0;
        for _ in 0..=240 {
            pts.push(format!("{:.2},{:.2}", sx(st[0]), sy(st[1])));
            match propagate(&st, dt) {
                Some(n) => st = n,
                None => break,
            }
        }
        s.push_str(&format!(
            "<polyline fill=\"none\" stroke=\"{col}\" stroke-width=\"1.2\" points=\"{}\"/>\n<text x=\"10\" y=\"{}\" fill=\"{col}\">{}</text>\n",
            pts.join(" "),
            34 + 14 * k,
            o.id
        ));
    }
    for t in truth {
        if let Some(st) = propagate(&t.state, 0.0) {
            s.push_str(&format!(
                "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"2.5\" fill=\"none\" stroke=\"#b33\"/>\n",
                sx(st[0]),
                sy(st[1])
            ));
        }
    }
    if let Some(d) = debris {
        if let Some(tr) = d["trajectory_xy"].as_array() {
            let pts: Vec<String> = tr
                .iter()
                .filter_map(|p| {
                    Some(format!(
                        "{:.2},{:.2}",
                        sx(p[0].as_f64()?),
                        sy(p[1].as_f64()?)
                    ))
                })
                .collect();
            s.push_str(&format!(
                "<polyline fill=\"none\" stroke=\"#b33\" stroke-dasharray=\"4 3\" points=\"{}\"/>\n",
                pts.join(" ")
            ));
        }
        for f in d["fragments_xy"].as_array().into_iter().flatten() {
            if let (Some(x), Some(y)) = (f[0].as_f64(), f[1].as_f64()) {
                s.push_str(&format!(
                    "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"1\" fill=\"#999\"/>\n",
                    sx(x),
                    sy(y)
                ));
            }
        }
    }
    s.push_str(&format!(
        "<text x=\"10\" y=\"{}\" fill=\"#555\">{} observations; red circles: synthetic truth at epoch</text>\n</svg>\n",
        h - 10.0,
        p.obs.len()
    ));
    s
}
