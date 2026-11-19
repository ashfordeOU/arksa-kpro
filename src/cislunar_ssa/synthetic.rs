// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Synthetic angles-only campaigns and their scoring.
//!
//! Truth orbits come from the open engine's differential-corrected families (NRHO, L2 halo,
//! L1 halo, L1 Lyapunov, DRO) or from Moon-centred elements (ELFO, low lunar orbit), or are
//! given as synodic states. Observers track for stated windows at a stated cadence; stations
//! keep only observations above their elevation mask (no daylight, weather or
//! magnitude limits are modelled); Gaussian angle noise comes from a seeded generator, so a
//! campaign is reproducible byte for byte. Scoring compares the catalogue with the truth it
//! was generated from: an internal-consistency check, not performance on real data.

use kshana::api::KshanaError;
use kshana::cr3bp::{differential_correct_halo, Cr3bpState};
use kshana::cr3bp_family::{correct_member, CorrectorSettings, Hold};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::Deserialize;
use serde_json::{json, Value};

use super::catalogue::Catalogue;
use super::{bad, from_cr3bp, propagate, r9, AngleObs, Observer, Problem, ARCSEC, L_KM, MU, S6};

/// A truth object of a synthetic campaign.
#[derive(Debug, Clone, Deserialize)]
pub struct TruthConfig {
    pub id: String,
    /// A family preset ([`preset_state`]) or an explicit synodic state at the epoch.
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub state: Option<S6>,
    /// Orbit phase (fraction of the preset's period) at the epoch.
    #[serde(default)]
    pub phase_frac: f64,
}

fn d_cadence() -> f64 {
    10.0
}

/// One tracking window of one observer.
#[derive(Debug, Clone, Deserialize)]
pub struct TrackConfig {
    pub observer: String,
    pub start_utc: String,
    pub hours: f64,
    #[serde(default = "d_cadence")]
    pub cadence_min: f64,
    /// Restrict the track to these truth ids (default: every object).
    #[serde(default)]
    pub objects: Option<Vec<String>>,
}

fn d_sigma() -> f64 {
    1.0
}
fn d_seed() -> u64 {
    1
}

/// A synthetic campaign.
#[derive(Debug, Clone, Deserialize)]
pub struct SyntheticConfig {
    pub epoch_utc: String,
    pub objects: Vec<TruthConfig>,
    pub tracks: Vec<TrackConfig>,
    /// Angle noise, one sigma per axis (arcsec).
    #[serde(default = "d_sigma")]
    pub sigma_arcsec: f64,
    #[serde(default = "d_seed")]
    pub seed: u64,
}

/// A generated truth object.
#[derive(Debug, Clone)]
pub struct Truth {
    pub id: String,
    /// Synodic state at the scenario epoch.
    pub state: S6,
    pub tracklets: Vec<String>,
}

/// Moon-centred elements (a in km, angles in degrees, mean anomaly in degrees) to a synodic
/// state, with the inclination measured from the Earth–Moon orbital plane.
pub fn from_lunar_elements(
    a_km: f64,
    e: f64,
    i_deg: f64,
    raan_deg: f64,
    argp_deg: f64,
    m_deg: f64,
) -> S6 {
    let a = a_km / L_KM;
    let gm = MU;
    let m = m_deg.to_radians();
    let ecc_anom = kshana::ephem::solve_kepler(m, e);
    let (se, ce) = ecc_anom.sin_cos();
    let nu = ((1.0 - e * e).sqrt() * se).atan2(ce - e);
    let r = a * (1.0 - e * ce);
    let p = a * (1.0 - e * e);
    let (sn, cn) = nu.sin_cos();
    let rp = [r * cn, r * sn, 0.0];
    let k = (gm / p).sqrt();
    let vp = [-k * sn, k * (e + cn), 0.0];
    let (so, co) = raan_deg.to_radians().sin_cos();
    let (si, ci) = i_deg.to_radians().sin_cos();
    let (sw, cw) = argp_deg.to_radians().sin_cos();
    let rot = [
        [co * cw - so * sw * ci, -co * sw - so * cw * ci, so * si],
        [so * cw + co * sw * ci, -so * sw + co * cw * ci, -co * si],
        [sw * si, cw * si, ci],
    ];
    let mv = |x: [f64; 3]| {
        [
            rot[0][0] * x[0] + rot[0][1] * x[1] + rot[0][2] * x[2],
            rot[1][0] * x[0] + rot[1][1] * x[1] + rot[1][2] * x[2],
            rot[2][0] * x[0] + rot[2][1] * x[1] + rot[2][2] * x[2],
        ]
    };
    let ri = mv(rp);
    let vi = mv(vp);
    // Rotating-frame velocity: v_inertial − ẑ × r.
    [
        ri[0] + (1.0 - MU),
        ri[1],
        ri[2],
        vi[0] + ri[1],
        vi[1] - ri[0],
        vi[2],
    ]
}

fn corrected(guess: S6, planar: bool) -> Result<(S6, f64), KshanaError> {
    let g = Cr3bpState {
        r: [guess[0], guess[1], guess[2]],
        v: [guess[3], guess[4], guess[5]],
    };
    let m = correct_member(&g, MU, Hold::X0, planar, &CorrectorSettings::default())
        .ok_or_else(|| bad("family corrector did not converge for a preset"))?;
    Ok((from_cr3bp(&m.ic), m.period))
}

/// A preset's initial condition and period (nondimensional).
pub fn preset(name: &str) -> Result<(S6, f64), KshanaError> {
    match name.to_ascii_lowercase().as_str() {
        "nrho" => {
            // The open engine's published L2 southern 9:2 NRHO seed.
            let g = Cr3bpState {
                r: [1.0220, 0.0, -0.1800],
                v: [0.0, -0.1020, 0.0],
            };
            let o = differential_correct_halo(&g, MU, 1e-11, 80)
                .ok_or_else(|| bad("NRHO correction did not converge"))?;
            Ok((from_cr3bp(&o.ic), o.period))
        }
        "halo-l2" => corrected([1.12, 0.0, -0.05, 0.0, -0.17, 0.0], false),
        "halo-l1" => corrected([0.8234, 0.0, 0.0224, 0.0, 0.1340, 0.0], false),
        "lyapunov-l1" => corrected([0.82, 0.0, 0.0, 0.0, 0.13, 0.0], true),
        "dro" => {
            let d = kshana::dro::seed_dro(40_000.0)
                .ok_or_else(|| bad("DRO seeding did not converge"))?;
            Ok(([d.ic[0], d.ic[1], 0.0, d.ic[2], d.ic[3], 0.0], d.period))
        }
        "elfo" => {
            let a = 6_142.4;
            let period = std::f64::consts::TAU * ((a / L_KM).powi(3) / MU).sqrt();
            Ok((
                from_lunar_elements(a, 0.6, 54.856, 0.0, 90.0, 180.0),
                period,
            ))
        }
        "llo" => {
            let a = super::MOON_RADIUS_KM + 100.0;
            let period = std::f64::consts::TAU * ((a / L_KM).powi(3) / MU).sqrt();
            Ok((from_lunar_elements(a, 0.001, 90.0, 0.0, 0.0, 0.0), period))
        }
        other => Err(bad(format!(
            "unknown preset '{other}' (nrho, halo-l2, halo-l1, lyapunov-l1, dro, elfo, llo)"
        ))),
    }
}

/// A preset propagated to a phase (fraction of its period).
pub fn preset_state(name: &str, phase_frac: f64) -> Result<S6, KshanaError> {
    let (s, period) = preset(name)?;
    propagate(&s, phase_frac.rem_euclid(1.0) * period)
        .ok_or_else(|| bad(format!("preset '{name}' could not be propagated")))
}

/// A short description of a campaign for the result document.
pub fn describe(c: &SyntheticConfig) -> Value {
    json!({
        "epoch_utc": c.epoch_utc,
        "objects": c.objects.iter().map(|o| json!({"id": o.id, "preset": o.preset, "phase_frac": o.phase_frac, "explicit_state": o.state.is_some()})).collect::<Vec<_>>(),
        "tracks": c.tracks.len(),
        "sigma_arcsec": c.sigma_arcsec,
        "seed": c.seed,
    })
}

fn gauss(rng: &mut ChaCha8Rng) -> f64 {
    let u1: f64 = rng.gen::<f64>().max(1e-300);
    let u2: f64 = rng.gen::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// Generate a campaign's observations and truth.
pub fn generate(
    c: &SyntheticConfig,
    epoch: f64,
    observers: &[(String, Observer)],
) -> Result<(Vec<AngleObs>, Vec<Truth>), KshanaError> {
    let sigma = c.sigma_arcsec * ARCSEC;
    if !(sigma.is_finite() && sigma > 0.0) {
        return Err(bad("synthetic sigma_arcsec must be positive"));
    }
    let mut truth: Vec<Truth> = Vec::new();
    for o in &c.objects {
        let state = match (&o.state, &o.preset) {
            (Some(s), _) => *s,
            (None, Some(p)) => preset_state(p, o.phase_frac)?,
            (None, None) => {
                return Err(bad(format!(
                    "synthetic object {}: give `preset` or `state`",
                    o.id
                )))
            }
        };
        truth.push(Truth {
            id: o.id.clone(),
            state,
            tracklets: Vec::new(),
        });
    }
    let mut rng = ChaCha8Rng::seed_from_u64(c.seed);
    let mut obs = Vec::new();
    let mut n_trk = 0usize;
    let t_unit = super::t_unit_s();
    for tr in &c.tracks {
        let ob = observers
            .iter()
            .find(|(n, _)| *n == tr.observer)
            .map(|(_, o)| o)
            .ok_or_else(|| {
                bad(format!(
                    "track observer '{}' is not configured",
                    tr.observer
                ))
            })?;
        let start = crate::conjunction::parse_utc(&tr.start_utc)?;
        if !(tr.hours > 0.0 && tr.cadence_min > 0.0) {
            return Err(bad("track hours and cadence_min must be positive"));
        }
        let n = (tr.hours * 60.0 / tr.cadence_min).floor() as usize + 1;
        for t in truth.iter_mut() {
            if let Some(only) = &tr.objects {
                if !only.contains(&t.id) {
                    continue;
                }
            }
            let mut batch = Vec::new();
            let mut s = propagate(&t.state, (start - epoch) / t_unit)
                .ok_or_else(|| bad(format!("truth {} could not be propagated", t.id)))?;
            let mut prev = start;
            for k in 0..n {
                let utc = start + k as f64 * tr.cadence_min * 60.0;
                s = match propagate(&s, (utc - prev) / t_unit) {
                    Some(x) => x,
                    None => break,
                };
                prev = utc;
                let (op, up) = ob
                    .position(utc, epoch)
                    .ok_or_else(|| bad("observer could not be propagated"))?;
                let frame = super::EmFrame::at(utc);
                let d = super::sub3(frame.syn_to_geo(&s), op);
                if let (Some(up), Observer::Station { mask_rad, .. }) = (up, ob) {
                    let el = (super::dot3(super::unit3(d), super::unit3(up))).asin();
                    if el < *mask_rad {
                        continue;
                    }
                }
                let (ra, dec) = super::radec(d);
                batch.push((utc, ra, dec));
            }
            if batch.len() < 3 {
                continue;
            }
            n_trk += 1;
            let id = format!("TRK-{n_trk:04}");
            for (utc, ra, dec) in batch {
                let dra = gauss(&mut rng) * sigma / dec.cos().max(1e-6);
                let ddec = gauss(&mut rng) * sigma;
                obs.push(AngleObs {
                    utc_s: utc,
                    observer: tr.observer.clone(),
                    tracklet: id.clone(),
                    ra: (ra + dra).rem_euclid(std::f64::consts::TAU),
                    dec: dec + ddec,
                    sigma,
                });
            }
            t.tracklets.push(id);
        }
    }
    Ok((obs, truth))
}

/// Score a catalogue against the campaign truth.
pub fn score(_p: &Problem, cat: &Catalogue, truth: &[Truth]) -> Value {
    let owner = |trk: &str| {
        truth
            .iter()
            .find(|t| t.tracklets.iter().any(|x| x == trk))
            .map(|t| t.id.as_str())
    };
    let mut rows = Vec::new();
    let mut matched: Vec<&str> = Vec::new();
    let mut duplicates = 0usize;
    let mut correct_assoc = 0usize;
    let mut total_assoc = 0usize;
    for o in &cat.objects {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for trk in &o.tracklets {
            if let Some(id) = owner(trk) {
                match counts.iter_mut().find(|(x, _)| *x == id) {
                    Some((_, c)) => *c += 1,
                    None => counts.push((id, 1)),
                }
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let Some(&(major, nmaj)) = counts.first() else {
            continue;
        };
        total_assoc += o.tracklets.len();
        correct_assoc += nmaj;
        if matched.contains(&major) {
            duplicates += 1;
        } else {
            matched.push(major);
        }
        let t = truth.iter().find(|t| t.id == major).expect("owner exists");
        let ts = propagate(&t.state, o.fit.tau_ref);
        let (pos_err, vel_err, nees) = match ts {
            Some(ts) => {
                let d: Vec<f64> = (0..6).map(|k| o.fit.x[k] - ts[k]).collect();
                let pe = super::norm3([d[0], d[1], d[2]]) * L_KM;
                let ve = super::norm3([d[3], d[4], d[5]]) * super::v_unit_kms() * 1e3;
                let nees = o.fit.cov.and_then(|c| {
                    let a: Vec<Vec<f64>> = c.iter().map(|r| r.to_vec()).collect();
                    let x = super::solve(&a, &d)?;
                    Some(d.iter().zip(&x).map(|(u, v)| u * v).sum::<f64>())
                });
                (pe, ve, nees.unwrap_or(f64::NAN))
            }
            None => (f64::NAN, f64::NAN, f64::NAN),
        };
        rows.push(json!({
            "object": o.id,
            "truth": major,
            "purity": r9(nmaj as f64 / o.tracklets.len().max(1) as f64),
            "position_error_km": r9(pos_err),
            "velocity_error_mps": r9(vel_err),
            "nees6": r9(nees),
        }));
    }
    let observed: Vec<&Truth> = truth.iter().filter(|t| !t.tracklets.is_empty()).collect();
    json!({
        "note": "internal consistency against the generating truth; not performance on real data",
        "truth_objects_observed": observed.len(),
        "truth_objects_catalogued": matched.len(),
        "completeness": r9(matched.len() as f64 / observed.len().max(1) as f64),
        "duplicate_objects": duplicates,
        "association_purity": r9(correct_assoc as f64 / total_assoc.max(1) as f64),
        "objects": rows,
        "truth_tracklets": truth.iter().map(|t| json!({"truth": t.id, "tracklets": t.tracklets})).collect::<Vec<_>>(),
    })
}
