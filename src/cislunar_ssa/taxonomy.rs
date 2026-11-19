// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Cislunar orbit-family taxonomy.
//!
//! A state is propagated over a window (default 21 days: at least one revolution of the libration families, short enough that the error growth of an unstable orbit stays moderate) in the CR3BP and reduced to a few
//! features: distances to the Moon and the Earth, out-of-plane amplitude, the net winding
//! angle about the Moon in the rotating frame, the mean `x`, and (for a Moon-bound orbit) the
//! Moon-centred osculating elements over the window. Documented rules, applied in order,
//! assign the family:
//!
//! 1. **lunar-impact** (closest Moon distance below its radius, or the flow fails near it),
//!    **earth-reentry** (below 6,500 km geocentric), **escape** (beyond 1.5 million km);
//! 2. Moon-bound (farthest Moon distance below 30,000 km): **llo** (apolune altitude below
//!    2,000 km and mean eccentricity below 0.1), **elfo** (lunar frozen, ELFO-class: mean
//!    eccentricity at least 0.3 and the argument of perilune within 30° of 90° or 270° over
//!    the whole window), else **lunar-orbit**;
//! 3. **earth-orbit**: never inside 66,000 km of the Moon (the lunar Hill radius is about
//!    61,500 km) and never beyond 0.85 Earth–Moon distances from the Earth;
//! 4. planar (out-of-plane amplitude below 2,000 km): **dro** (at least 0.9 of a full
//!    retrograde turn about the Moon), **dpo** (prograde), else **lyapunov-l1** /
//!    **lyapunov-l2** by the side of the Moon the orbit librates on;
//! 5. spatial and librating (farthest Moon distance below 0.5 Earth–Moon distances):
//!    **nrho** (closest Moon distance at most 17,500 km, after Zimovan-Spreen et al. 2020)
//!    else **halo**, each tagged L1 or L2 by the mean `x`;
//! 6. anything else: **transfer** (unclassified: chaotic, transfer or quasi-periodic motion).
//!
//! Inclinations and arguments of perilune are measured from the Earth–Moon orbital plane,
//! not the lunar equator (about 6.7° apart). The thresholds are stated modelling choices,
//! and the result is a MODELLED label from CR3BP dynamics; [`classify`] is checked against
//! differential-corrected members of the open engine's DRO, halo and NRHO families.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{earth_dist_km, moon_dist_km, propagate, r9, L_KM, M6, MOON_RADIUS_KM, MU, S6};

fn d_window() -> f64 {
    21.0
}
fn d_samples() -> usize {
    3000
}

/// Taxonomy settings.
#[derive(Debug, Clone, Deserialize)]
pub struct TaxonomyConfig {
    /// Classification window (days).
    #[serde(default = "d_window")]
    pub window_days: f64,
    /// Samples over the window.
    #[serde(default = "d_samples")]
    pub samples: usize,
}

impl Default for TaxonomyConfig {
    fn default() -> Self {
        Self {
            window_days: d_window(),
            samples: d_samples(),
        }
    }
}

/// The features a classification rests on.
#[derive(Debug, Clone, Default)]
pub struct Features {
    pub window_days: f64,
    pub moon_min_km: f64,
    pub moon_max_km: f64,
    pub earth_min_km: f64,
    pub earth_max_km: f64,
    pub z_max_km: f64,
    /// Net winding angle about the Moon in the rotating frame (turns; negative = retrograde).
    pub moon_turns: f64,
    pub x_mean: f64,
    pub jacobi: f64,
    /// Moon-centred mean osculating eccentricity, inclination (deg), argument of perilune
    /// range (deg, min and max over the window), semi-major axis (km): Moon-bound orbits only.
    pub lunar_elements: Option<(f64, f64, (f64, f64), f64)>,
    pub flow_failed: bool,
}

/// A classification.
#[derive(Debug, Clone)]
pub struct Classification {
    pub family: &'static str,
    pub label: String,
    pub rule: String,
    pub features: Features,
    /// Fraction of the twelve one-sigma points of the state covariance (± each Cholesky
    /// column) that receive the same family; `None` without a covariance.
    pub covariance_agreement: Option<f64>,
}

impl Classification {
    pub fn to_json(&self) -> Value {
        let f = &self.features;
        json!({
            "family": self.family,
            "label": self.label,
            "rule": self.rule,
            "evidence_class": "MODELLED",
            "covariance_agreement": self.covariance_agreement.map(r9),
            "stable_under_covariance": self.covariance_agreement.map(|a| a >= 1.0),
            "features": {
                "window_days": r9(f.window_days),
                "moon_min_km": r9(f.moon_min_km),
                "moon_max_km": r9(f.moon_max_km),
                "earth_min_km": r9(f.earth_min_km),
                "earth_max_km": r9(f.earth_max_km),
                "z_max_km": r9(f.z_max_km),
                "moon_turns": r9(f.moon_turns),
                "x_mean": r9(f.x_mean),
                "jacobi": r9(f.jacobi),
                "flow_failed": f.flow_failed,
                "lunar_elements": f.lunar_elements.map(|(e, i, (w0, w1), a)| json!({
                    "e_mean": r9(e), "i_deg_mean": r9(i), "argp_deg_min": r9(w0), "argp_deg_max": r9(w1), "a_km_mean": r9(a)
                })),
            }
        })
    }
}

/// Moon-centred osculating elements of a synodic state: (a [nd], e, i [rad], ω [rad]).
pub fn lunar_elements(s: &S6) -> (f64, f64, f64, f64) {
    let r = [s[0] - (1.0 - MU), s[1], s[2]];
    // Inertial velocity in synodic axes: v + ẑ × r.
    let v = [s[3] - r[1], s[4] + r[0], s[5]];
    let gm = MU;
    let rn = super::norm3(r);
    let v2 = super::dot3(v, v);
    let h = super::cross3(r, v);
    let hn = super::norm3(h);
    let vxh = super::cross3(v, h);
    let ev = [
        vxh[0] / gm - r[0] / rn,
        vxh[1] / gm - r[1] / rn,
        vxh[2] / gm - r[2] / rn,
    ];
    let e = super::norm3(ev);
    let a = 1.0 / (2.0 / rn - v2 / gm);
    let inc = (h[2] / hn).clamp(-1.0, 1.0).acos();
    let node = [-h[1], h[0], 0.0];
    let nn = super::norm3(node);
    let w = if nn < 1e-12 || e < 1e-9 {
        0.0
    } else {
        let c = (super::dot3(node, ev) / (nn * e)).clamp(-1.0, 1.0);
        let w = c.acos();
        if ev[2] < 0.0 {
            std::f64::consts::TAU - w
        } else {
            w
        }
    };
    (a, e, inc, w)
}

/// Extract the features of a state over a window.
pub fn features(s0: &S6, cfg: &TaxonomyConfig) -> Features {
    let window = cfg.window_days.max(1.0) * 86_400.0 / super::t_unit_s();
    let n = cfg.samples.max(200);
    let dt = window / n as f64;
    let mut f = Features {
        window_days: cfg.window_days,
        moon_min_km: f64::INFINITY,
        earth_min_km: f64::INFINITY,
        jacobi: kshana::cr3bp::jacobi_constant(&super::to_cr3bp(s0), MU),
        ..Default::default()
    };
    let mut s = *s0;
    let mut prev_angle = (s[1]).atan2(s[0] - (1.0 - MU));
    let mut winding = 0.0;
    let mut xsum = 0.0;
    let mut count = 0usize;
    let mut moon_bound = true;
    let mut elems: Vec<(f64, f64, f64, f64)> = Vec::new();
    for k in 0..=n {
        let dm = moon_dist_km(&s);
        let de = earth_dist_km(&s);
        f.moon_min_km = f.moon_min_km.min(dm);
        f.moon_max_km = f.moon_max_km.max(dm);
        f.earth_min_km = f.earth_min_km.min(de);
        f.earth_max_km = f.earth_max_km.max(de);
        f.z_max_km = f.z_max_km.max(s[2].abs() * L_KM);
        let ang = s[1].atan2(s[0] - (1.0 - MU));
        winding += super::wrap_pi(ang - prev_angle);
        prev_angle = ang;
        xsum += s[0];
        count += 1;
        if dm < 30_000.0 && moon_bound {
            elems.push(lunar_elements(&s));
        } else {
            moon_bound = false;
        }
        if dm < MOON_RADIUS_KM || !(6_500.0..=1.6e6).contains(&de) {
            break;
        }
        if k == n {
            break;
        }
        match propagate(&s, dt) {
            Some(x) => s = x,
            None => {
                f.flow_failed = true;
                break;
            }
        }
    }
    f.moon_turns = winding / std::f64::consts::TAU;
    f.x_mean = xsum / count.max(1) as f64;
    if moon_bound && !elems.is_empty() {
        let m = elems.len() as f64;
        let e = elems.iter().map(|x| x.1).sum::<f64>() / m;
        let i = elems.iter().map(|x| x.2.to_degrees()).sum::<f64>() / m;
        let a = elems.iter().map(|x| x.0 * L_KM).sum::<f64>() / m;
        let (w0, w1) = elems
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| {
                let w = x.3.to_degrees();
                (lo.min(w), hi.max(w))
            });
        f.lunar_elements = Some((e, i, (w0, w1), a));
    }
    f
}

/// Classify a state from its features.
pub fn classify(s0: &S6, cfg: &TaxonomyConfig) -> Classification {
    let f = features(s0, cfg);
    let side = if f.x_mean < 1.0 - MU { "L1" } else { "L2" };
    let mk = |family: &'static str, label: String, rule: &str, f: Features| Classification {
        family,
        label,
        rule: rule.to_string(),
        features: f,
        covariance_agreement: None,
    };
    if f.moon_min_km < MOON_RADIUS_KM || (f.flow_failed && f.moon_min_km < 5_000.0) {
        return mk(
            "lunar-impact",
            "lunar impact within the window".into(),
            "closest Moon distance below the lunar radius",
            f,
        );
    }
    if f.earth_min_km < 6_500.0 {
        return mk(
            "earth-reentry",
            "Earth re-entry within the window".into(),
            "closest Earth distance below 6,500 km",
            f,
        );
    }
    if f.earth_max_km > 1.5e6 {
        return mk(
            "escape",
            "escape from the Earth-Moon system".into(),
            "farthest Earth distance beyond 1.5 million km",
            f,
        );
    }
    if f.moon_max_km < 30_000.0 {
        if let Some((e, _i, (w0, w1), _a)) = f.lunar_elements {
            if f.moon_max_km - MOON_RADIUS_KM < 2_000.0 && e < 0.1 {
                return mk(
                    "llo",
                    "low lunar orbit".into(),
                    "Moon-bound; apolune altitude below 2,000 km and mean eccentricity below 0.1",
                    f,
                );
            }
            let near = |c: f64| w0 >= c - 30.0 && w1 <= c + 30.0;
            if e >= 0.3 && (near(90.0) || near(270.0)) {
                return mk(
                    "elfo",
                    "lunar frozen orbit (ELFO-class)".into(),
                    "Moon-bound; mean eccentricity at least 0.3 and argument of perilune within 30 deg of 90 or 270 deg over the window",
                    f,
                );
            }
        }
        return mk(
            "lunar-orbit",
            "lunar orbit (not frozen in the window)".into(),
            "Moon-bound (farthest Moon distance below 30,000 km); no llo/elfo rule met",
            f,
        );
    }
    if f.moon_min_km > 66_000.0 && f.earth_max_km < 0.85 * L_KM {
        let label = if f.earth_max_km > 50_000.0 {
            "Earth orbit (cislunar, beyond GEO)"
        } else {
            "Earth orbit (GEO and below)"
        };
        return mk(
            "earth-orbit",
            label.into(),
            "never within 66,000 km of the Moon and never beyond 0.85 Earth-Moon distances",
            f,
        );
    }
    if f.z_max_km < 2_000.0 {
        if f.moon_turns <= -0.9 && f.moon_max_km < 0.5 * L_KM {
            return mk(
                "dro",
                "distant retrograde orbit".into(),
                "planar; at least 0.9 of a retrograde turn about the Moon",
                f,
            );
        }
        if f.moon_turns >= 0.9 && f.moon_max_km < 0.5 * L_KM {
            return mk(
                "dpo",
                "distant prograde orbit".into(),
                "planar; at least 0.9 of a prograde turn about the Moon",
                f,
            );
        }
        if f.moon_max_km < 0.5 * L_KM {
            let fam = if side == "L1" {
                "lyapunov-l1"
            } else {
                "lyapunov-l2"
            };
            return mk(
                fam,
                format!("planar Lyapunov orbit ({side})"),
                "planar; librating without a full turn about the Moon",
                f,
            );
        }
    } else if f.moon_max_km < 0.5 * L_KM {
        if f.moon_min_km <= 17_500.0 {
            return mk(
                "nrho",
                format!("{side} near-rectilinear halo orbit"),
                "spatial, librating; closest Moon distance at most 17,500 km (NRHO range)",
                f,
            );
        }
        return mk(
            "halo",
            format!("{side} halo orbit"),
            "spatial, librating; closest Moon distance above 17,500 km",
            f,
        );
    }
    mk(
        "transfer",
        "transfer or chaotic motion (unclassified)".into(),
        "no family rule met",
        f,
    )
}

/// [`classify`] for a catalogued state with its covariance: the state is classified, then
/// each of the twelve one-sigma points `x ± Lⱼ` (`P = L Lᵀ`), and the agreement fraction
/// says whether the label survives the state uncertainty.
pub fn classify_at(s: &S6, cov: Option<&M6>, cfg: &TaxonomyConfig) -> Classification {
    let mut c = classify(s, cfg);
    if let Some(p) = cov {
        let rows: Vec<Vec<f64>> = p.iter().map(|r| r.to_vec()).collect();
        if let Some(l) = crate::conjunction::cholesky(&rows) {
            let mut same = 0usize;
            for j in 0..6 {
                for sign in [1.0, -1.0] {
                    let mut x = *s;
                    for i in 0..6 {
                        x[i] += sign * l[i][j];
                    }
                    if classify(&x, cfg).family == c.family {
                        same += 1;
                    }
                }
            }
            c.covariance_agreement = Some(same as f64 / 12.0);
        }
    }
    c
}
