// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Catalogue build-up from tracklets: data association, initial orbit determination and
//! refinement.
//!
//! Tracklets are taken in order of their first observation. Each is first tested against
//! every catalogued object: the object's state and formal covariance are mapped to the
//! tracklet's observations, and the Mahalanobis distance of the predicted-minus-observed
//! angles, `rᵀ(H P Hᵀ + I)⁻¹r` on whitened residuals, is gated at a chi-square quantile
//! (Wilson–Hilferty approximation). A tracklet inside the gate is refitted jointly with the
//! object's observations and joins the object only if the joint fit stays consistent
//! (reduced chi-square at most the configured limit). Otherwise the tracklet seeds a new
//! object through [`super::iod`], or, if IOD fails or is ambiguous, it is held and retried
//! against the catalogue once every tracklet has been seen; what is still unexplained is
//! reported as uncorrelated with the reason.

use serde::Deserialize;
use serde_json::{json, Value};

use super::iod::{iod, refine, Fit, Iod, IodConfig};
use super::{r9, sigmas, state_km, Problem};

fn d_gate() -> f64 {
    0.999
}
fn d_max_rchi2() -> f64 {
    4.0
}

/// Association settings.
#[derive(Debug, Clone, Deserialize)]
pub struct AssocConfig {
    /// Gate probability of the chi-square association test.
    #[serde(default = "d_gate")]
    pub gate_probability: f64,
    /// Largest reduced chi-square a fit (new object or joint refit) may have.
    #[serde(default = "d_max_rchi2")]
    pub max_reduced_chi2: f64,
}

impl Default for AssocConfig {
    fn default() -> Self {
        Self {
            gate_probability: d_gate(),
            max_reduced_chi2: d_max_rchi2(),
        }
    }
}

/// A catalogued object.
#[derive(Debug, Clone)]
pub struct CatalogueObject {
    pub id: String,
    pub fit: Fit,
    pub tracklets: Vec<String>,
    /// Observation indices (ascending).
    pub obs: Vec<usize>,
    /// The IOD that created the object.
    pub iod: Iod,
}

impl CatalogueObject {
    pub fn to_json(&self, p: &Problem) -> Value {
        let (sp, sv) = self
            .fit
            .cov
            .as_ref()
            .map(sigmas)
            .unwrap_or((f64::NAN, f64::NAN));
        let epoch = p.epoch_utc_s + self.fit.tau_ref * super::t_unit_s();
        json!({
            "id": self.id,
            "epoch_utc": crate::conjunction::fmt_utc(epoch),
            "state_synodic": self.fit.x.iter().map(|v| r9(*v)).collect::<Vec<_>>(),
            "state_moon_synodic_km_kms": state_km(&self.fit.x).iter().map(|v| r9(*v)).collect::<Vec<_>>(),
            "sigma_pos_km": r9(sp),
            "sigma_vel_mps": r9(sv),
            "reduced_chi2": r9(self.fit.reduced_chi2()),
            "rms_arcsec": r9(self.rms_arcsec(p)),
            "observations": self.obs.len(),
            "tracklets": self.tracklets,
            "arc_days": r9((p.geo[*self.obs.last().unwrap_or(&0)].tau - p.geo[self.obs[0]].tau) * super::t_unit_s() / 86_400.0),
            "fit": self.fit.to_json(),
            "iod": self.iod.to_json(),
        })
    }

    /// Angle residual RMS (arcsec, both axes).
    pub fn rms_arcsec(&self, p: &Problem) -> f64 {
        let f = match p.residuals(&self.obs, self.fit.tau_ref, &self.fit.x) {
            Some(f) => f,
            None => return f64::NAN,
        };
        let mut s = 0.0;
        for (k, &i) in self.obs.iter().enumerate() {
            let sig = p.obs[i].sigma / super::ARCSEC;
            s += (f[2 * k] * sig).powi(2) + (f[2 * k + 1] * sig).powi(2);
        }
        (s / f.len().max(1) as f64).sqrt()
    }
}

/// The catalogue build-up outcome.
#[derive(Debug, Clone, Default)]
pub struct Catalogue {
    pub objects: Vec<CatalogueObject>,
    /// Tracklets left unexplained, with the reason.
    pub uncorrelated: Vec<(String, String)>,
    /// One entry per decision, in order.
    pub log: Vec<Value>,
}

/// Standard-normal quantile (Acklam's rational approximation; |error| < 1.2e-9).
pub fn norm_quantile(p: f64) -> f64 {
    let a = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    let b = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    let c = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    let d = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let pl = 0.02425;
    if p < pl {
        let q = (-2.0 * p.ln()).sqrt();
        (((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5])
            / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0)
    } else if p <= 1.0 - pl {
        let q = p - 0.5;
        let r = q * q;
        (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q
            / (((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1.0)
    } else {
        -norm_quantile(1.0 - p)
    }
}

/// Chi-square quantile with `k` degrees of freedom (Wilson–Hilferty).
pub fn chi2_quantile(p: f64, k: usize) -> f64 {
    let k = k.max(1) as f64;
    let z = norm_quantile(p);
    let t = 1.0 - 2.0 / (9.0 * k) + z * (2.0 / (9.0 * k)).sqrt();
    k * t * t * t
}

/// Mahalanobis distance of a tracklet against a fitted object (whitened residuals, the
/// object's covariance mapped through the finite-difference measurement Jacobian).
pub fn mahalanobis(p: &Problem, fit: &Fit, idx: &[usize]) -> Option<f64> {
    let (r, h) = p.residuals_jac(idx, fit.tau_ref, &fit.x)?;
    let m = r.len();
    let mut s = vec![vec![0.0; m]; m];
    for i in 0..m {
        s[i][i] = 1.0;
    }
    if let Some(cov) = &fit.cov {
        // H P Hᵀ
        let hp: Vec<[f64; 6]> = h
            .iter()
            .map(|row| {
                let mut o = [0.0; 6];
                for q in 0..6 {
                    o[q] = (0..6).map(|k| row[k] * cov[k][q]).sum();
                }
                o
            })
            .collect();
        for i in 0..m {
            for j in 0..m {
                s[i][j] += (0..6).map(|q| hp[i][q] * h[j][q]).sum::<f64>();
            }
        }
    }
    let x = super::solve(&s, &r)?;
    Some(r.iter().zip(&x).map(|(a, b)| a * b).sum())
}

fn merged(a: &[usize], b: &[usize]) -> Vec<usize> {
    let mut v: Vec<usize> = a.iter().chain(b).copied().collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Try to add a tracklet to the catalogue. Returns the object index on success.
fn try_associate(
    p: &Problem,
    objects: &mut [CatalogueObject],
    tracklet: &str,
    idx: &[usize],
    cfg: &AssocConfig,
    log: &mut Vec<Value>,
) -> Option<usize> {
    let gate = chi2_quantile(cfg.gate_probability, 2 * idx.len());
    let mut candidates: Vec<(f64, usize)> = objects
        .iter()
        .enumerate()
        .filter_map(|(k, o)| mahalanobis(p, &o.fit, idx).map(|d| (d, k)))
        .filter(|(d, _)| d.is_finite() && *d <= gate)
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (d2, k) in candidates {
        let all = merged(&objects[k].obs, idx);
        let tau = p.geo[all[0]].tau;
        // Move the reference to the start of the joint arc.
        let x0 = match super::propagate(&objects[k].fit.x, tau - objects[k].fit.tau_ref) {
            Some(x) => x,
            None => continue,
        };
        let Some(fit) = refine(p, &all, tau, &x0, 40) else {
            continue;
        };
        let rc = fit.reduced_chi2();
        log.push(json!({"tracklet": tracklet, "object": objects[k].id, "test": "association", "mahalanobis2": r9(d2), "gate": r9(gate), "joint_reduced_chi2": r9(rc), "accepted": rc <= cfg.max_reduced_chi2}));
        if rc <= cfg.max_reduced_chi2 {
            let o = &mut objects[k];
            o.fit = fit;
            o.obs = all;
            o.tracklets.push(tracklet.to_string());
            return Some(k);
        }
    }
    None
}

/// Build the catalogue from every tracklet of the problem.
pub fn build(p: &Problem, iod_cfg: &IodConfig, cfg: &AssocConfig) -> Catalogue {
    let mut cat = Catalogue::default();
    let mut held: Vec<(String, Vec<usize>, String)> = Vec::new();
    for (t, idx) in p.tracklets() {
        if try_associate(p, &mut cat.objects, &t, &idx, cfg, &mut cat.log).is_some() {
            continue;
        }
        match iod(p, &idx, iod_cfg) {
            Ok(r) if !r.ambiguous && r.fit.reduced_chi2() <= cfg.max_reduced_chi2 => {
                let id = format!("C-{:03}", cat.objects.len() + 1);
                cat.log.push(json!({"tracklet": t, "object": id, "test": "iod", "reduced_chi2": r9(r.fit.reduced_chi2()), "ambiguous": false, "accepted": true}));
                cat.objects.push(CatalogueObject {
                    id,
                    fit: r.fit.clone(),
                    tracklets: vec![t.clone()],
                    obs: idx.clone(),
                    iod: r,
                });
            }
            Ok(r) => {
                let why = if r.ambiguous {
                    "IOD ambiguous (distinct solutions within the chi-square margin)".to_string()
                } else {
                    format!(
                        "IOD reduced chi-square {:.3} above the limit",
                        r.fit.reduced_chi2()
                    )
                };
                cat.log.push(json!({"tracklet": t, "test": "iod", "reduced_chi2": r9(r.fit.reduced_chi2()), "ambiguous": r.ambiguous, "accepted": false}));
                held.push((t, idx, why));
            }
            Err(e) => {
                cat.log
                    .push(json!({"tracklet": t, "test": "iod", "accepted": false, "error": e}));
                held.push((t, idx, format!("IOD failed: {e}")));
            }
        }
    }
    // Second pass: held tracklets against the final catalogue.
    for (t, idx, why) in held {
        if try_associate(p, &mut cat.objects, &t, &idx, cfg, &mut cat.log).is_none() {
            cat.uncorrelated.push((t, why));
        }
    }
    // Final covariance on each object's full arc (refine already set it).
    for o in &mut cat.objects {
        if o.fit.cov.is_none() {
            o.fit.cov = super::iod::covariance(p, &o.obs, o.fit.tau_ref, &o.fit.x);
        }
    }
    cat
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_match_tables() {
        assert!((norm_quantile(0.975) - 1.959_964).abs() < 1e-5);
        assert!((norm_quantile(0.001) + 3.090_232).abs() < 1e-5);
        // chi2(0.999, 20) = 45.315; Wilson-Hilferty is within 0.5 %.
        assert!((chi2_quantile(0.999, 20) - 45.315).abs() / 45.315 < 5e-3);
        assert!((chi2_quantile(0.95, 100) - 124.342).abs() / 124.342 < 1e-3);
    }
}
