// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Rank the candidates of one launch by how well each explains the operator's observations.
//!
//! For each candidate, every measurement is predicted from the candidate's SGP4 orbit (ITRS,
//! through the open engine's TEME to ITRS rotation) and the station position; the residual is
//! observed minus predicted. Optional nuisance parameters are estimated per pass by weighted
//! least squares (a constant: the unknown frequency offset of the satellite's oscillator for
//! Doppler, on by default; a ranging bias, off by default). The fit statistic is the weighted
//! sum of squares `chi2 = sum((residual / sigma)^2)` with `dof = n - (number of biases)`.
//!
//! Ranking and confidence. Candidates are ranked by `chi2`. The best fit's variance factor
//! `s2 = max(1, chi2_best / dof)` scales every difference (so a best fit worse than the stated
//! sigmas does not inflate the confidence), `delta_k = (chi2_k - chi2_best) / s2`, and under a
//! uniform prior over the candidates and Gaussian errors the posterior probability of candidate
//! `k` is `exp(-delta_k / 2) / sum_j exp(-delta_j / 2)`. The probability is conditional on the
//! operator's object being one of the candidates; the fit-quality test below guards that.
//!
//! Verdict (thresholds in [`Config`]):
//! * `INSUFFICIENT_DATA`: fewer than `min_observations` measurements.
//! * `NO_CONSISTENT_CANDIDATE`: even the best candidate's reduced chi-square exceeds
//!   `max_reduced_chi2` (the object may be missing from the set, or the element sets too old).
//! * `CONSISTENT_ONLY_CANDIDATE`: one candidate, consistent; nothing was discriminated.
//! * `IDENTIFIED`: the runner-up is worse by at least `decision_delta_chi2` and the best
//!   candidate's probability is at least `min_probability`.
//! * `AMBIGUOUS`: otherwise; the verdict lists every candidate not excluded (`delta <
//!   decision_delta_chi2`) and never names one of them as the answer.
//!
//! The evidence also says whether the data could have separated the two leading candidates:
//! their predicted measurements differ by `sqrt(sum(((pred_1 - pred_2) / sigma)^2))` sigma over
//! the same measurements (after the same per-pass biases are removed), and their positions are
//! a stated distance apart over the observation times.

use std::collections::HashMap;

use kshana::sgp4::teme_to_itrs_matrix;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::catalogue::{Candidate, CandidateInfo};
use super::observations::{ObsType, Observation, ObservationSet, Station};

/// Speed of light (m/s).
pub const C_M_S: f64 = 299_792_458.0;

/// Thresholds and nuisance parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Estimate a constant Doppler offset per pass (the satellite oscillator's unknown offset).
    pub doppler_bias_per_pass: bool,
    /// Estimate a constant range bias per pass.
    pub range_bias_per_pass: bool,
    /// A candidate whose reduced chi-square exceeds this does not explain the observations.
    pub max_reduced_chi2: f64,
    /// The runner-up must be worse than the best by at least this scaled chi-square.
    pub decision_delta_chi2: f64,
    /// The best candidate's posterior probability needed for `IDENTIFIED`.
    pub min_probability: f64,
    /// Fewer measurements than this give `INSUFFICIENT_DATA`.
    pub min_observations: usize,
    /// Measurements a candidate puts below this elevation (deg) are counted as below the
    /// horizon (evidence against it; they still enter the residuals).
    pub horizon_deg: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            doppler_bias_per_pass: true,
            range_bias_per_pass: false,
            max_reduced_chi2: 4.0,
            decision_delta_chi2: 25.0,
            min_probability: 0.99,
            min_observations: 10,
            horizon_deg: 0.0,
        }
    }
}

impl Config {
    pub fn check(&self) -> Result<(), String> {
        if !(self.max_reduced_chi2.is_finite() && self.max_reduced_chi2 > 0.0) {
            return Err("max_reduced_chi2 must be a positive number".into());
        }
        if !(self.decision_delta_chi2.is_finite() && self.decision_delta_chi2 > 0.0) {
            return Err("decision_delta_chi2 must be a positive number".into());
        }
        if !(self.min_probability > 0.0 && self.min_probability < 1.0) {
            return Err("min_probability must be in (0, 1)".into());
        }
        if !(-90.0..=90.0).contains(&self.horizon_deg) {
            return Err("horizon_deg must be in [-90, 90]".into());
        }
        Ok(())
    }

    fn biased(&self, k: ObsType) -> bool {
        match k {
            ObsType::DopplerHz => self.doppler_bias_per_pass,
            ObsType::RangeM => self.range_bias_per_pass,
            _ => false,
        }
    }
}

/// Look angles, range and range rate of a satellite from a station.
#[derive(Debug, Clone, Copy)]
pub struct Geo {
    pub range_m: f64,
    pub range_rate_m_s: f64,
    pub az_deg: f64,
    pub el_deg: f64,
    /// Satellite ITRS position (m).
    pub pos: [f64; 3],
}

/// Geometry of an ITRS state (m, m/s) seen from a station fixed in ITRS.
pub fn geometry(r: [f64; 3], v: [f64; 3], st: &Station) -> Geo {
    let d = [r[0] - st.ecef[0], r[1] - st.ecef[1], r[2] - st.ecef[2]];
    let rho = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let rr = (d[0] * v[0] + d[1] * v[1] + d[2] * v[2]) / rho;
    let (sl, cl) = st.lat_deg.to_radians().sin_cos();
    let (so, co) = st.lon_deg.to_radians().sin_cos();
    let e = -so * d[0] + co * d[1];
    let n = -sl * co * d[0] - sl * so * d[1] + cl * d[2];
    let u = cl * co * d[0] + cl * so * d[1] + sl * d[2];
    Geo {
        range_m: rho,
        range_rate_m_s: rr,
        az_deg: e.atan2(n).to_degrees().rem_euclid(360.0),
        el_deg: (u / rho).clamp(-1.0, 1.0).asin().to_degrees(),
        pos: r,
    }
}

/// The predicted value of a measurement.
pub fn predicted(o: &Observation, g: &Geo) -> f64 {
    match o.kind {
        ObsType::DopplerHz => -o.carrier_hz.unwrap_or(0.0) * g.range_rate_m_s / C_M_S,
        ObsType::RangeM => g.range_m,
        ObsType::RangeRateMS => g.range_rate_m_s,
        ObsType::AzDeg => g.az_deg,
        ObsType::ElDeg => g.el_deg,
    }
}

/// Observed minus predicted, azimuth wrapped to (-180, 180].
fn raw_residual(o: &Observation, pred: f64) -> f64 {
    let r = o.value - pred;
    if o.kind == ObsType::AzDeg {
        let w = (r + 180.0).rem_euclid(360.0) - 180.0;
        if w == -180.0 {
            180.0
        } else {
            w
        }
    } else {
        r
    }
}

/// ITRS states of `orbit` at the observation times (rotation matrices shared).
pub struct Epochs {
    /// Index of each observation's time in `mats`.
    pub of_obs: Vec<usize>,
    pub times: Vec<kshana::jd2::Jd2>,
    pub mats: Vec<[[f64; 3]; 3]>,
}

impl Epochs {
    pub fn new(set: &ObservationSet) -> Self {
        let mut map: HashMap<(u64, u64), usize> = HashMap::new();
        let mut times = Vec::new();
        let mut mats = Vec::new();
        let of_obs = set
            .obs
            .iter()
            .map(|o| {
                *map.entry((o.time.day.to_bits(), o.time.frac.to_bits()))
                    .or_insert_with(|| {
                        times.push(o.time);
                        mats.push(teme_to_itrs_matrix(o.time));
                        times.len() - 1
                    })
            })
            .collect();
        Self {
            of_obs,
            times,
            mats,
        }
    }
}

/// A bias estimated for one pass and measurement type.
#[derive(Debug, Clone, Serialize)]
pub struct Bias {
    pub pass: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub value: f64,
    pub unit: &'static str,
    pub n: usize,
}

/// Residual statistics of one measurement type.
#[derive(Debug, Clone, Serialize)]
pub struct TypeStats {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub unit: &'static str,
    pub n: usize,
    /// RMS residual after the biases, in the type's unit.
    pub rms: f64,
    /// Mean residual after the biases.
    pub mean: f64,
    /// RMS of residual / sigma.
    pub rms_normalised: f64,
}

/// The fit of one candidate.
#[derive(Debug, Clone, Serialize)]
pub struct Fit {
    pub candidate: CandidateInfo,
    /// `fitted` or `propagation_failed`.
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub rank: usize,
    pub n: usize,
    pub n_bias_parameters: usize,
    pub dof: usize,
    pub chi2: f64,
    pub reduced_chi2: f64,
    pub max_abs_normalised_residual: f64,
    /// Measurements this candidate puts below the horizon.
    pub below_horizon: usize,
    /// Element age at the first and the last observation (days, negative: before the epoch).
    pub element_age_days: [f64; 2],
    /// Whether the candidate alone explains the observations (`reduced_chi2 <= max`).
    pub consistent: bool,
    /// Scaled chi-square above the best candidate.
    pub delta_chi2: f64,
    pub probability: f64,
    pub per_type: Vec<TypeStats>,
    pub biases: Vec<Bias>,
    /// Predicted value and residual (after biases) per measurement.
    #[serde(skip)]
    pub predicted: Vec<f64>,
    #[serde(skip)]
    pub residual: Vec<f64>,
    #[serde(skip)]
    pub elevation_deg: Vec<f64>,
    #[serde(skip)]
    pub positions: Vec<[f64; 3]>,
}

/// The verdict class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Identified,
    Ambiguous,
    NoConsistentCandidate,
    ConsistentOnlyCandidate,
    InsufficientData,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identified => "IDENTIFIED",
            Self::Ambiguous => "AMBIGUOUS",
            Self::NoConsistentCandidate => "NO_CONSISTENT_CANDIDATE",
            Self::ConsistentOnlyCandidate => "CONSISTENT_ONLY_CANDIDATE",
            Self::InsufficientData => "INSUFFICIENT_DATA",
        }
    }
}

/// How far apart the two leading candidates are, and how well the data could tell them apart.
#[derive(Debug, Clone, Serialize)]
pub struct Separation {
    pub best: u32,
    pub second: u32,
    /// Scaled chi-square between them (the decision margin).
    pub delta_chi2: f64,
    /// log10 of the likelihood ratio best : second.
    pub log10_likelihood_ratio: f64,
    /// `sqrt(sum(((pred_best - pred_second) / sigma)^2))`: how many sigma their predictions
    /// differ over these measurements (biases removed alike). Below about 5 the data cannot
    /// separate them, whatever the noise.
    pub predicted_difference_sigma: f64,
    /// Distance between their predicted positions over the observation times (km).
    pub distance_km_min: f64,
    pub distance_km_median: f64,
    pub distance_km_max: f64,
}

/// The ranking and verdict.
#[derive(Debug, Clone, Serialize)]
pub struct Identification {
    pub verdict: Verdict,
    /// The identified (or only consistent) object; `None` unless the verdict is `IDENTIFIED`
    /// or `CONSISTENT_ONLY_CANDIDATE`.
    pub identified: Option<u32>,
    /// Candidates not excluded: within `decision_delta_chi2` of a best candidate that is itself
    /// consistent (empty otherwise).
    pub not_excluded: Vec<u32>,
    pub reasons: Vec<String>,
    pub config: Config,
    pub variance_factor: f64,
    pub separation: Option<Separation>,
    /// Ranked best first; failed propagations last.
    pub fits: Vec<Fit>,
    pub observations: Value,
}

fn wmean(xs: &[(f64, f64)]) -> f64 {
    let (mut s, mut w) = (0.0, 0.0);
    for &(x, sg) in xs {
        let wi = 1.0 / (sg * sg);
        s += wi * x;
        w += wi;
    }
    if w > 0.0 {
        s / w
    } else {
        0.0
    }
}

/// Fit one candidate.
fn fit(c: &Candidate, set: &ObservationSet, ep: &Epochs, cfg: &Config) -> Fit {
    let n = set.obs.len();
    let age = |t: kshana::jd2::Jd2| t.diff_seconds(c.epoch) / 86_400.0;
    let (t0, t1) = set
        .obs
        .iter()
        .fold((set.obs[0].time, set.obs[0].time), |(a, b), o| {
            (
                if o.time.diff_seconds(a) < 0.0 {
                    o.time
                } else {
                    a
                },
                if o.time.diff_seconds(b) > 0.0 {
                    o.time
                } else {
                    b
                },
            )
        });
    let mut f = Fit {
        candidate: c.info(),
        status: "fitted",
        error: None,
        rank: 0,
        n,
        n_bias_parameters: 0,
        dof: 0,
        chi2: f64::INFINITY,
        reduced_chi2: f64::INFINITY,
        max_abs_normalised_residual: 0.0,
        below_horizon: 0,
        element_age_days: [age(t0), age(t1)],
        consistent: false,
        delta_chi2: f64::INFINITY,
        probability: 0.0,
        per_type: vec![],
        biases: vec![],
        predicted: vec![0.0; n],
        residual: vec![0.0; n],
        elevation_deg: vec![0.0; n],
        positions: vec![[0.0; 3]; n],
    };
    let mut states = Vec::with_capacity(ep.times.len());
    for (t, m) in ep.times.iter().zip(&ep.mats) {
        match c.orbit.itrs_state_with(*t, m) {
            Ok(s) => states.push(s),
            Err(e) => {
                f.status = "propagation_failed";
                f.error = Some(format!("SGP4 at {}: {e}", super::fmt_utc(*t)));
                return f;
            }
        }
    }
    for (i, o) in set.obs.iter().enumerate() {
        let (r, v) = states[ep.of_obs[i]];
        let g = geometry(r, v, &set.stations[o.station]);
        let p = predicted(o, &g);
        f.predicted[i] = p;
        f.residual[i] = raw_residual(o, p);
        f.elevation_deg[i] = g.el_deg;
        f.positions[i] = g.pos;
        if g.el_deg < cfg.horizon_deg {
            f.below_horizon += 1;
        }
    }
    // Biases per (pass, type).
    let mut groups: std::collections::BTreeMap<(usize, ObsType), Vec<usize>> = Default::default();
    for (i, o) in set.obs.iter().enumerate() {
        if cfg.biased(o.kind) {
            groups.entry((o.pass, o.kind)).or_default().push(i);
        }
    }
    for ((pass, kind), idx) in &groups {
        let b = wmean(
            &idx.iter()
                .map(|&i| (f.residual[i], set.obs[i].sigma))
                .collect::<Vec<_>>(),
        );
        for &i in idx {
            f.residual[i] -= b;
            f.predicted[i] += b;
        }
        f.biases.push(Bias {
            pass: set.passes[*pass].clone(),
            kind: kind.as_str(),
            value: b,
            unit: kind.unit(),
            n: idx.len(),
        });
    }
    f.n_bias_parameters = groups.len();
    f.dof = n.saturating_sub(f.n_bias_parameters).max(1);
    let mut chi2 = 0.0;
    for (i, o) in set.obs.iter().enumerate() {
        let z = f.residual[i] / o.sigma;
        chi2 += z * z;
        f.max_abs_normalised_residual = f.max_abs_normalised_residual.max(z.abs());
    }
    f.chi2 = chi2;
    f.reduced_chi2 = chi2 / f.dof as f64;
    f.consistent = f.reduced_chi2 <= cfg.max_reduced_chi2;
    for k in ObsType::ALL {
        let idx: Vec<usize> = (0..n).filter(|&i| set.obs[i].kind == k).collect();
        if idx.is_empty() {
            continue;
        }
        let m = idx.len() as f64;
        let mean = idx.iter().map(|&i| f.residual[i]).sum::<f64>() / m;
        let rms = (idx.iter().map(|&i| f.residual[i].powi(2)).sum::<f64>() / m).sqrt();
        let rn = (idx
            .iter()
            .map(|&i| (f.residual[i] / set.obs[i].sigma).powi(2))
            .sum::<f64>()
            / m)
            .sqrt();
        f.per_type.push(TypeStats {
            kind: k.as_str(),
            unit: k.unit(),
            n: idx.len(),
            rms,
            mean,
            rms_normalised: rn,
        });
    }
    f
}

fn separation(a: &Fit, b: &Fit, set: &ObservationSet) -> Separation {
    // Each fit's prediction includes its own per-pass biases, so the difference of the two
    // predictions is the difference of the residuals, with any constant offset per biased
    // group already absorbed.
    let n = set.obs.len();
    let sum: f64 = (0..n)
        .map(|i| {
            let mut d = b.residual[i] - a.residual[i];
            if set.obs[i].kind == ObsType::AzDeg {
                d = (d + 180.0).rem_euclid(360.0) - 180.0;
            }
            (d / set.obs[i].sigma).powi(2)
        })
        .sum();
    let mut dist: Vec<f64> = (0..n)
        .map(|i| {
            let (p, q) = (a.positions[i], b.positions[i]);
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt() / 1e3
        })
        .collect();
    dist.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    Separation {
        best: a.candidate.catalogue_number,
        second: b.candidate.catalogue_number,
        delta_chi2: b.delta_chi2,
        log10_likelihood_ratio: b.delta_chi2 / (2.0 * std::f64::consts::LN_10),
        predicted_difference_sigma: sum.sqrt(),
        distance_km_min: dist.first().copied().unwrap_or(0.0),
        distance_km_median: dist.get(dist.len() / 2).copied().unwrap_or(0.0),
        distance_km_max: dist.last().copied().unwrap_or(0.0),
    }
}

fn label(c: &CandidateInfo) -> String {
    match &c.tle_field {
        Some(f) if c.alpha5 => format!("{} (TLE {f})", c.catalogue_number),
        Some(_) => c.catalogue_number.to_string(),
        None => format!("{} (OMM only)", c.catalogue_number),
    }
}

/// Rank the candidates against the observations and return the verdict.
pub fn identify(
    cands: &[Candidate],
    set: &ObservationSet,
    cfg: &Config,
) -> Result<Identification, String> {
    cfg.check()?;
    if cands.is_empty() {
        return Err("no candidate element sets".into());
    }
    let ep = Epochs::new(set);
    let mut fits: Vec<Fit> = cands.iter().map(|c| fit(c, set, &ep, cfg)).collect();
    fits.sort_by(|a, b| {
        a.chi2
            .partial_cmp(&b.chi2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.candidate
                    .catalogue_number
                    .cmp(&b.candidate.catalogue_number),
            )
    });
    let fitted: Vec<usize> = (0..fits.len())
        .filter(|&i| fits[i].status == "fitted")
        .collect();
    let mut variance_factor = 1.0;
    if let Some(&b) = fitted.first() {
        variance_factor = fits[b].reduced_chi2.max(1.0);
        let best = fits[b].chi2;
        for &i in &fitted {
            fits[i].delta_chi2 = (fits[i].chi2 - best) / variance_factor;
        }
        let z: f64 = fitted
            .iter()
            .map(|&i| (-fits[i].delta_chi2 / 2.0).exp())
            .sum();
        for &i in &fitted {
            fits[i].probability = (-fits[i].delta_chi2 / 2.0).exp() / z;
        }
    }
    for (k, f) in fits.iter_mut().enumerate() {
        f.rank = k + 1;
    }
    let separation = if fitted.len() >= 2 {
        Some(separation(&fits[fitted[0]], &fits[fitted[1]], set))
    } else {
        None
    };
    // Not excluded: within the decision margin of the best (empty when even the best
    // candidate does not explain the observations).
    let not_excluded: Vec<u32> = match fitted.first() {
        Some(&b) if fits[b].consistent => fitted
            .iter()
            .filter(|&&i| fits[i].delta_chi2 < cfg.decision_delta_chi2)
            .map(|&i| fits[i].candidate.catalogue_number)
            .collect(),
        _ => vec![],
    };
    let n = set.obs.len();
    let mut reasons = Vec::new();
    let mut identified = None;
    let verdict = if n < cfg.min_observations {
        reasons.push(format!(
            "{n} measurements, fewer than the {} required",
            cfg.min_observations
        ));
        Verdict::InsufficientData
    } else if fitted.is_empty() {
        reasons.push("no candidate could be propagated to the observation times".into());
        Verdict::NoConsistentCandidate
    } else {
        let b = &fits[fitted[0]];
        if !b.consistent {
            reasons.push(format!(
                "the best candidate, {}, has reduced chi-square {:.3} over {} measurements, above the limit {}: no candidate explains the observations (the object may be missing from the set, the element sets may be too old, or the sigmas too small)",
                label(&b.candidate),
                b.reduced_chi2,
                b.n,
                cfg.max_reduced_chi2
            ));
            Verdict::NoConsistentCandidate
        } else if fitted.len() == 1 {
            reasons.push(format!(
                "only one candidate could be tested ({}); it is consistent (reduced chi-square {:.3}) but nothing was discriminated",
                label(&b.candidate),
                b.reduced_chi2
            ));
            identified = Some(b.candidate.catalogue_number);
            Verdict::ConsistentOnlyCandidate
        } else {
            let s = separation.as_ref().expect("two fitted candidates");
            let second = &fits[fitted[1]];
            reasons.push(format!(
                "best: {} with reduced chi-square {:.3} over {} measurements; probability {:.6} under a uniform prior over {} candidates",
                label(&b.candidate),
                b.reduced_chi2,
                b.n,
                b.probability,
                fitted.len()
            ));
            reasons.push(format!(
                "runner-up: {} worse by delta chi-square {:.3} (scaled by the variance factor {:.3}), likelihood ratio 10^{:.1}",
                label(&second.candidate),
                s.delta_chi2,
                variance_factor,
                s.log10_likelihood_ratio
            ));
            reasons.push(format!(
                "the two leading candidates' predictions differ by {:.1} sigma over these measurements; their positions are {:.3} to {:.3} km apart",
                s.predicted_difference_sigma, s.distance_km_min, s.distance_km_max
            ));
            if s.delta_chi2 >= cfg.decision_delta_chi2 && b.probability >= cfg.min_probability {
                identified = Some(b.candidate.catalogue_number);
                reasons.push(format!(
                    "margin {:.3} >= {} and probability {:.6} >= {}: identified",
                    s.delta_chi2, cfg.decision_delta_chi2, b.probability, cfg.min_probability
                ));
                Verdict::Identified
            } else {
                let mut short = Vec::new();
                if s.delta_chi2 < cfg.decision_delta_chi2 {
                    short.push(format!(
                        "the margin {:.3} is below {}",
                        s.delta_chi2, cfg.decision_delta_chi2
                    ));
                }
                if b.probability < cfg.min_probability {
                    short.push(format!(
                        "the probability {:.6} is below {}",
                        b.probability, cfg.min_probability
                    ));
                }
                reasons.push(format!(
                    "not identified: {}; {} candidates are not excluded ({}); more passes, another measurement type or a pass where they are further apart would separate them",
                    short.join(" and "),
                    not_excluded.len(),
                    not_excluded
                        .iter()
                        .map(|c| c.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                if s.predicted_difference_sigma < 5.0 {
                    reasons.push("the two leading candidates' predictions differ by under 5 sigma over these measurements: even noise-free data of this kind could not separate them reliably".into());
                }
                Verdict::Ambiguous
            }
        }
    };
    let below: Vec<String> = fits
        .iter()
        .filter(|f| {
            f.status == "fitted"
                && f.below_horizon > 0
                && Some(f.candidate.catalogue_number) == identified
        })
        .map(|f| {
            format!(
                "the identified object is below the horizon ({}) at {} of the measurements",
                cfg.horizon_deg, f.below_horizon
            )
        })
        .collect();
    reasons.extend(below);
    if cands.iter().any(|c| !c.checksum_ok) {
        reasons.push("some TLE lines carry a checksum that does not match (recorded per candidate; the elements were read regardless)".into());
    }
    let mut per_type = serde_json::Map::new();
    for k in ObsType::ALL {
        let m = set.obs.iter().filter(|o| o.kind == k).count();
        if m > 0 {
            per_type.insert(k.as_str().into(), json!(m));
        }
    }
    let first = set
        .obs
        .iter()
        .map(|o| o.time)
        .fold(
            set.obs[0].time,
            |a, t| if t.diff_seconds(a) < 0.0 { t } else { a },
        );
    let last = set
        .obs
        .iter()
        .map(|o| o.time)
        .fold(
            set.obs[0].time,
            |a, t| if t.diff_seconds(a) > 0.0 { t } else { a },
        );
    let observations = json!({
        "measurements": n,
        "per_type": per_type,
        "stations": set.stations,
        "passes": set.passes,
        "first_utc": super::fmt_utc(first),
        "last_utc": super::fmt_utc(last),
        "span_h": last.diff_seconds(first) / 3600.0,
    });
    Ok(Identification {
        verdict,
        identified,
        not_excluded,
        reasons,
        config: cfg.clone(),
        variance_factor,
        separation,
        fits,
        observations,
    })
}

impl Identification {
    /// The fit of a catalogue number.
    pub fn fit_of(&self, cat: u32) -> Option<&Fit> {
        self.fits
            .iter()
            .find(|f| f.candidate.catalogue_number == cat)
    }

    /// The best fitted candidate.
    pub fn best(&self) -> Option<&Fit> {
        self.fits.iter().find(|f| f.status == "fitted")
    }
}
