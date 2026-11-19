// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Covariance-realism tests: is a predicted covariance the right size for the errors the
//! prediction actually had?
//!
//! Each sample is a prediction error `e` (reference minus predicted position, in the predicted
//! object's RTN axes) with the predicted 3x3 position covariance `C` at the same time. If `C`
//! is realistic and the error Gaussian, `d² = eᵀ C⁻¹ e` follows the chi-square law with 3
//! degrees of freedom. Per object class and per prediction-age bin the result gives:
//!
//! * the mean of `d²/3` (1 when realistic) and the scale factor `√(mean d²/3)` by which the
//!   covariance's standard deviations would have to grow (> 1: optimistic covariance);
//! * a two-sided test of `Σd²` against χ²(3n), a Kolmogorov–Smirnov distance from the χ²₃ law
//!   with its asymptotic p-value, and the containment of the 95 % ellipsoid;
//! * per-axis normalised error variances (radial, transverse, normal), to show which axis is
//!   off (in-track is the usual one).
//!
//! The tests assume independent samples. Samples along one arc are correlated, so the p-values
//! are optimistic (too small) and the result says so; the scale factors do not depend on that
//! assumption.
//!
//! Samples come from a predicted OEM with covariance and a reference OEM (a later definitive
//! or precise ephemeris), or directly from a residual table
//! (`object,class,age_h,dr_m,dt_m,dn_m,crr,ctr,ctt,cnr,cnt,cnn`, metres and m²).

use std::collections::BTreeMap;

use kshana::api::KshanaError;
use serde_json::{json, Value};

use super::ephemeris::Ephemeris;
use super::{
    bad, cholesky, m3_rows, mat_vec, object_class, pos_block, quad_inv, rtn_matrix, sub, M3, V3,
};

/// One prediction error with its predicted covariance.
#[derive(Debug, Clone)]
pub struct Sample {
    pub object: String,
    pub class: String,
    /// Time since the prediction epoch (s).
    pub age_s: f64,
    /// Reference minus predicted position, RTN (m).
    pub err_rtn_m: V3,
    /// Predicted position covariance, RTN (m²).
    pub cov_rtn_m2: M3,
}

fn rtn_frame(f: &str) -> bool {
    matches!(
        f.trim().to_ascii_uppercase().as_str(),
        "RTN" | "RSW" | "RIC" | "UVW" | "QSW"
    )
}

fn inertial_frame(f: &str) -> bool {
    let f = f.to_ascii_uppercase();
    ["EME2000", "J2000", "GCRF", "ICRF", "TEME", "MOD", "TOD"]
        .iter()
        .any(|x| f.contains(x))
}

/// Samples from a predicted ephemeris with covariance and a reference ephemeris of the same
/// object (same inertial frame). `prediction_epoch`: the age origin (default the predicted
/// ephemeris's first state). Returns the samples and warnings.
pub fn samples_from_oem(
    pred: &Ephemeris,
    reference: &Ephemeris,
    class: Option<&str>,
    prediction_epoch: Option<f64>,
) -> Result<(Vec<Sample>, Vec<String>), KshanaError> {
    if !inertial_frame(&pred.ref_frame) || !inertial_frame(&reference.ref_frame) {
        return Err(bad(format!(
            "realism needs inertial ephemerides (got {} and {})",
            pred.ref_frame, reference.ref_frame
        )));
    }
    if !pred.ref_frame.eq_ignore_ascii_case(&reference.ref_frame) {
        return Err(bad(format!(
            "predicted ({}) and reference ({}) ephemerides are in different frames",
            pred.ref_frame, reference.ref_frame
        )));
    }
    let t0 = prediction_epoch
        .or(pred.states.first().map(|s| s.0))
        .ok_or_else(|| bad("predicted ephemeris has no states"))?;
    let mut warn = Vec::new();
    let (mut outside, mut unknown_frame) = (0usize, 0usize);
    let mut out = Vec::new();
    for (t, frame, c) in &pred.covariances {
        let (Some(xp), Some(xr)) = (pred.state_at(*t, 8), reference.state_at(*t, 8)) else {
            outside += 1;
            continue;
        };
        let (rp, vp) = ([xp[0], xp[1], xp[2]], [xp[3], xp[4], xp[5]]);
        let m = rtn_matrix(rp, vp);
        let cpos = pos_block(c);
        let cov = if rtn_frame(frame) {
            cpos
        } else if inertial_frame(frame) {
            // C_rtn = M C Mᵀ
            let mut tmp = [[0.0; 3]; 3];
            for i in 0..3 {
                for j in 0..3 {
                    tmp[i][j] = (0..3).map(|k| m[i][k] * cpos[k][j]).sum();
                }
            }
            let mut o = [[0.0; 3]; 3];
            for i in 0..3 {
                for j in 0..3 {
                    o[i][j] = (0..3).map(|k| tmp[i][k] * m[j][k]).sum();
                }
            }
            o
        } else {
            unknown_frame += 1;
            continue;
        };
        let cls = class
            .map(str::to_string)
            .unwrap_or_else(|| object_class(rp, vp).to_string());
        out.push(Sample {
            object: pred.object_id.clone(),
            class: cls,
            age_s: t - t0,
            err_rtn_m: mat_vec(&m, sub([xr[0], xr[1], xr[2]], rp)),
            cov_rtn_m2: cov,
        });
    }
    if outside > 0 {
        warn.push(format!(
            "{}: {outside} covariance epochs outside one of the ephemerides were skipped",
            pred.object_id
        ));
    }
    if unknown_frame > 0 {
        warn.push(format!(
            "{}: {unknown_frame} covariances in a frame other than RTN or inertial were skipped",
            pred.object_id
        ));
    }
    Ok((out, warn))
}

/// Samples from a residual table (see the module notes).
pub fn samples_from_csv(text: &str) -> Result<Vec<Sample>, KshanaError> {
    let mut lines = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'));
    let header: Vec<String> = lines
        .next()
        .ok_or_else(|| bad("residual table is empty"))?
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();
    let col = |n: &str| {
        header
            .iter()
            .position(|h| h == n)
            .ok_or_else(|| bad(format!("residual table has no '{n}' column")))
    };
    let names = [
        "age_h", "dr_m", "dt_m", "dn_m", "crr", "ctr", "ctt", "cnr", "cnt", "cnn",
    ];
    let idx: Vec<usize> = names.iter().map(|n| col(n)).collect::<Result<_, _>>()?;
    let (io, ic) = (col("object")?, col("class").ok());
    let mut out = Vec::new();
    for (n, l) in lines.enumerate() {
        let f: Vec<&str> = l.split(',').map(str::trim).collect();
        let v: Vec<f64> = idx
            .iter()
            .map(|&i| {
                f.get(i)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| bad(format!("residual table row {}: bad number", n + 2)))
            })
            .collect::<Result<_, _>>()?;
        out.push(Sample {
            object: f.get(io).unwrap_or(&"").to_string(),
            class: ic
                .and_then(|i| f.get(i))
                .filter(|s| !s.is_empty())
                .unwrap_or(&"unspecified")
                .to_string(),
            age_s: v[0] * 3600.0,
            err_rtn_m: [v[1], v[2], v[3]],
            cov_rtn_m2: [[v[4], v[5], v[7]], [v[5], v[6], v[8]], [v[7], v[8], v[9]]],
        });
    }
    Ok(out)
}

/// Settings of the tests.
#[derive(Debug, Clone)]
pub struct RealismConfig {
    /// Bin edges in hours (each bin is `[edge_i, edge_{i+1})`).
    pub age_bins_h: Vec<f64>,
    pub min_samples: usize,
    pub alpha: f64,
}

impl Default for RealismConfig {
    fn default() -> Self {
        Self {
            age_bins_h: vec![0.0, 6.0, 12.0, 24.0, 48.0, 72.0, 168.0],
            min_samples: 10,
            alpha: 0.05,
        }
    }
}

/// χ²₃ 95 % quantile.
const CHI2_3_95: f64 = 7.814_727_903_251_178;

/// Kolmogorov distribution tail `P(K > x)` (asymptotic KS p-value with `x = √n D`).
fn ks_p(x: f64) -> f64 {
    if x < 0.27 {
        return 1.0;
    }
    let mut s = 0.0;
    for k in 1..=100 {
        let kf = k as f64;
        let t = 2.0 * (-2.0 * kf * kf * x * x).exp();
        s += if k % 2 == 1 { t } else { -t };
        if t < 1e-16 {
            break;
        }
    }
    s.clamp(0.0, 1.0)
}

/// The statistics of one group of samples.
pub fn stats(samples: &[&Sample], cfg: &RealismConfig) -> Value {
    let mut d2 = Vec::new();
    let mut z2 = [0.0f64; 3];
    let mut not_pd = 0usize;
    for s in samples {
        let rows = m3_rows(&s.cov_rtn_m2);
        let Some(l) = cholesky(&rows) else {
            not_pd += 1;
            continue;
        };
        d2.push(quad_inv(&l, &s.err_rtn_m));
        for i in 0..3 {
            z2[i] += s.err_rtn_m[i] * s.err_rtn_m[i] / s.cov_rtn_m2[i][i];
        }
    }
    let n = d2.len();
    if n == 0 {
        return json!({"n": 0, "not_positive_definite": not_pd, "verdict": "INSUFFICIENT"});
    }
    let sum: f64 = d2.iter().sum();
    let mean = sum / n as f64 / 3.0;
    let f = kshana::raim::chi2_cdf(sum, 3.0 * n as f64);
    let p_mean = (2.0 * f.min(1.0 - f)).min(1.0);
    let mut sorted = d2.clone();
    sorted.sort_by(f64::total_cmp);
    let mut dks = 0.0f64;
    for (i, x) in sorted.iter().enumerate() {
        let c = kshana::raim::chi2_cdf(*x, 3.0);
        dks = dks
            .max((c - i as f64 / n as f64).abs())
            .max(((i + 1) as f64 / n as f64 - c).abs());
    }
    let p_ks = ks_p((n as f64).sqrt() * dks);
    let contained = d2.iter().filter(|x| **x <= CHI2_3_95).count() as f64 / n as f64;
    let verdict = if n < cfg.min_samples {
        "INSUFFICIENT"
    } else if p_mean < cfg.alpha {
        if mean > 1.0 {
            "OPTIMISTIC"
        } else {
            "PESSIMISTIC"
        }
    } else {
        "CONSISTENT"
    };
    json!({
        "n": n,
        "not_positive_definite": not_pd,
        "mean_d2_over_dof": mean,
        "sigma_scale_factor": mean.sqrt(),
        "p_value_mean": p_mean,
        "ks_distance": dks,
        "p_value_ks": p_ks,
        "containment_95": contained,
        "normalised_variance_rtn": [z2[0] / n as f64, z2[1] / n as f64, z2[2] / n as f64],
        "verdict": verdict,
    })
}

/// Spearman rank correlation of two equal-length series (no tie correction).
fn spearman(x: &[f64], y: &[f64]) -> Option<f64> {
    let n = x.len();
    if n < 3 {
        return None;
    }
    let rank = |v: &[f64]| {
        let mut i: Vec<usize> = (0..n).collect();
        i.sort_by(|a, b| v[*a].total_cmp(&v[*b]));
        let mut r = vec![0.0; n];
        for (k, &j) in i.iter().enumerate() {
            r[j] = k as f64;
        }
        r
    };
    let (rx, ry) = (rank(x), rank(y));
    let d2: f64 = rx.iter().zip(&ry).map(|(a, b)| (a - b).powi(2)).sum();
    let nf = n as f64;
    Some(1.0 - 6.0 * d2 / (nf * (nf * nf - 1.0)))
}

/// The realism result over all samples: by object class and age bin, by object, and overall.
pub fn analyse(samples: &[Sample], cfg: &RealismConfig) -> Value {
    let mut classes: BTreeMap<&str, Vec<&Sample>> = BTreeMap::new();
    let mut objects: BTreeMap<&str, Vec<&Sample>> = BTreeMap::new();
    for s in samples {
        classes.entry(s.class.as_str()).or_default().push(s);
        objects.entry(s.object.as_str()).or_default().push(s);
    }
    let edges = &cfg.age_bins_h;
    let by_class: Vec<Value> = classes
        .iter()
        .map(|(c, ss)| {
            let mut bins = Vec::new();
            let (mut mids, mut scales) = (Vec::new(), Vec::new());
            for w in edges.windows(2) {
                let sel: Vec<&Sample> = ss
                    .iter()
                    .copied()
                    .filter(|s| s.age_s >= w[0] * 3600.0 && s.age_s < w[1] * 3600.0)
                    .collect();
                if sel.is_empty() {
                    continue;
                }
                let st = stats(&sel, cfg);
                if let Some(k) = st["sigma_scale_factor"].as_f64() {
                    mids.push(0.5 * (w[0] + w[1]));
                    scales.push(k);
                }
                bins.push(json!({"age_h": [w[0], w[1]], "stats": st}));
            }
            json!({
                "class": c,
                "objects": ss.iter().map(|s| s.object.as_str()).collect::<std::collections::BTreeSet<_>>(),
                "all_ages": stats(ss, cfg),
                "by_age": bins,
                "scale_factor_trend_spearman": spearman(&mids, &scales),
            })
        })
        .collect();
    let by_object: Vec<Value> = objects
        .iter()
        .map(|(o, ss)| json!({"object": o, "class": ss[0].class, "stats": stats(ss, cfg)}))
        .collect();
    let all: Vec<&Sample> = samples.iter().collect();
    json!({
        "samples": samples.len(),
        "age_bins_h": edges,
        "dof": 3,
        "overall": stats(&all, cfg),
        "by_class": by_class,
        "by_object": by_object,
        "independence_note": "The chi-square and KS p-values assume independent samples; samples along one arc are correlated, so the p-values are optimistic. The scale factors do not depend on that assumption.",
    })
}
