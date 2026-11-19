// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Simulator-versus-telemetry residuals.
//!
//! For every observable a plugin predicts, each telemetry sample inside the simulated span
//! gives a residual `r = telemetry − simulated`. The summary per observable:
//!
//! - `bias` (mean residual) and its standard uncertainty `s / √n` (GUM type A, `s` the
//!   sample standard deviation of the residuals);
//! - `rms`, the largest |r|, and how many samples lie outside ±tolerance;
//! - `chi2_reduced = Σ (r/σ)² / n` when every compared sample carries a standard
//!   uncertainty σ;
//! - the verdict: `drifted` when |bias| or RMS exceeds the observable's tolerance, else
//!   `consistent`; `no-comparison` when no sample fell inside the simulated span; `hosted`
//!   when the model runs in the hosted service (its residuals are formed there).
//!
//! A plugin is `drifted` if any of its observables is, `hosted` if its model is hosted.

use serde::{Deserialize, Serialize};

use super::mission::Mission;
use super::plugin::SimPlugin;

/// Identifies a residual report.
pub const FORMAT: &str = "ks-simcal-residuals/1";

/// The verdict on one observable or plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Consistent,
    Drifted,
    NoComparison,
    Hosted,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Consistent => "consistent",
            Verdict::Drifted => "drifted",
            Verdict::NoComparison => "no-comparison",
            Verdict::Hosted => "hosted",
        }
    }
}

/// The residual summary of one observable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservableResiduals {
    /// `<plugin>.<name>`.
    pub observable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub tolerance: f64,
    /// Telemetry samples of this observable.
    pub samples: usize,
    /// Samples compared (inside the simulated span).
    pub compared: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bias: Option<f64>,
    /// Standard uncertainty of the bias (type A).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bias_u: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_abs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chi2_reduced: Option<f64>,
    pub outside_tolerance: usize,
    pub verdict: Verdict,
}

/// The residuals of one plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginResiduals {
    pub plugin: String,
    pub kind: String,
    pub verdict: Verdict,
    pub observables: Vec<ObservableResiduals>,
}

/// One residual sample (for the CSV).
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub observable: String,
    pub t_s: f64,
    pub telemetry: f64,
    pub simulated: f64,
    pub residual: f64,
}

/// The residual report of a mission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub format: String,
    pub mission: String,
    pub mission_sha256: String,
    pub telemetry_sha256: String,
    pub plugins: Vec<PluginResiduals>,
    /// Evidence class of the simulated values.
    pub evidence_class: String,
    pub note: String,
}

/// The residual summary of a list of residuals against a tolerance.
pub fn summarise(
    observable: &str,
    unit: Option<String>,
    tolerance: f64,
    samples: usize,
    residuals: &[(f64, Option<f64>)],
) -> ObservableResiduals {
    let n = residuals.len();
    let mut out = ObservableResiduals {
        observable: observable.to_string(),
        unit,
        tolerance,
        samples,
        compared: n,
        bias: None,
        bias_u: None,
        rms: None,
        max_abs: None,
        chi2_reduced: None,
        outside_tolerance: 0,
        verdict: Verdict::NoComparison,
    };
    if n == 0 {
        return out;
    }
    let nf = n as f64;
    let mean = residuals.iter().map(|(r, _)| r).sum::<f64>() / nf;
    let rms = (residuals.iter().map(|(r, _)| r * r).sum::<f64>() / nf).sqrt();
    let max_abs = residuals.iter().map(|(r, _)| r.abs()).fold(0.0, f64::max);
    let bias_u = if n > 1 {
        let var = residuals
            .iter()
            .map(|(r, _)| (r - mean).powi(2))
            .sum::<f64>()
            / (nf - 1.0);
        Some((var / nf).sqrt())
    } else {
        None
    };
    let chi2 = residuals
        .iter()
        .map(|(r, s)| s.map(|s| (r / s).powi(2)))
        .sum::<Option<f64>>()
        .map(|c| c / nf);
    out.bias = Some(mean);
    out.bias_u = bias_u;
    out.rms = Some(rms);
    out.max_abs = Some(max_abs);
    out.chi2_reduced = chi2;
    out.outside_tolerance = residuals
        .iter()
        .filter(|(r, _)| r.abs() > tolerance)
        .count();
    out.verdict = if mean.abs() > tolerance || rms > tolerance {
        Verdict::Drifted
    } else {
        Verdict::Consistent
    };
    out
}

/// The residuals of one plugin against the mission's telemetry, and the samples.
pub fn plugin_residuals(m: &Mission, p: &dyn SimPlugin) -> (PluginResiduals, Vec<Sample>) {
    let mut obs = Vec::new();
    let mut samples = Vec::new();
    let hosted = p.hosted().is_some();
    for o in p.observables() {
        let name = format!("{}.{}", p.id(), o.name);
        let tel = m.telemetry_of(&name);
        if hosted {
            obs.push(ObservableResiduals {
                verdict: Verdict::Hosted,
                ..summarise(&name, o.unit.clone(), o.tolerance, tel.len(), &[])
            });
            continue;
        }
        let mut rs = Vec::new();
        for row in &tel {
            if let Some(sim) = p.predict(&o.name, row.t_s) {
                let r = row.value - sim;
                rs.push((r, row.sigma));
                samples.push(Sample {
                    observable: name.clone(),
                    t_s: row.t_s,
                    telemetry: row.value,
                    simulated: sim,
                    residual: r,
                });
            }
        }
        obs.push(summarise(
            &name,
            o.unit.clone(),
            o.tolerance,
            tel.len(),
            &rs,
        ));
    }
    let verdict = if hosted {
        Verdict::Hosted
    } else if obs.iter().any(|o| o.verdict == Verdict::Drifted) {
        Verdict::Drifted
    } else if obs.iter().all(|o| o.verdict == Verdict::NoComparison) {
        Verdict::NoComparison
    } else {
        Verdict::Consistent
    };
    (
        PluginResiduals {
            plugin: p.id().to_string(),
            kind: p.kind().to_string(),
            verdict,
            observables: obs,
        },
        samples,
    )
}

/// The residual report of a mission and every residual sample.
pub fn report(m: &Mission, plugins: &[Box<dyn SimPlugin>]) -> (Report, Vec<Sample>) {
    let mut out = Vec::new();
    let mut all = Vec::new();
    for p in plugins {
        let (r, s) = plugin_residuals(m, p.as_ref());
        out.push(r);
        all.extend(s);
    }
    (
        Report {
            format: FORMAT.into(),
            mission: m.file.mission.clone(),
            mission_sha256: m.mission_sha256.clone(),
            telemetry_sha256: m.telemetry_sha256.clone(),
            plugins: out,
            evidence_class: super::MODELLED.into(),
            note: "Residuals are telemetry minus simulated value, measured against the telemetry supplied; the simulated values are MODELLED.".into(),
        },
        all,
    )
}

/// The residual samples as CSV.
pub fn samples_csv(samples: &[Sample]) -> String {
    let mut s = String::from("observable,t_s,telemetry,simulated,residual\n");
    for x in samples {
        s.push_str(&format!(
            "{},{},{},{},{}\n",
            x.observable, x.t_s, x.telemetry, x.simulated, x.residual
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_statistics() {
        let r = [(1.0, Some(1.0)), (3.0, Some(1.0))];
        let s = summarise("p.x", None, 5.0, 3, &r);
        assert_eq!(s.compared, 2);
        assert_eq!(s.samples, 3);
        assert_eq!(s.bias, Some(2.0));
        assert!((s.rms.unwrap() - 5.0f64.sqrt()).abs() < 1e-12);
        assert_eq!(s.max_abs, Some(3.0));
        // s = √2, u = √2/√2 = 1.
        assert!((s.bias_u.unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(s.chi2_reduced, Some(5.0));
        assert_eq!(s.verdict, Verdict::Consistent);
        assert_eq!(s.outside_tolerance, 0);
        let d = summarise("p.x", None, 1.5, 2, &r);
        assert_eq!(d.verdict, Verdict::Drifted);
        assert_eq!(d.outside_tolerance, 1);
    }

    #[test]
    fn chi_square_needs_every_sigma_and_one_sample_has_no_uncertainty() {
        let s = summarise("p.x", None, 5.0, 2, &[(1.0, Some(1.0)), (1.0, None)]);
        assert_eq!(s.chi2_reduced, None);
        let one = summarise("p.x", None, 5.0, 1, &[(1.0, None)]);
        assert_eq!(one.bias_u, None);
        let none = summarise("p.x", None, 5.0, 4, &[]);
        assert_eq!(none.verdict, Verdict::NoComparison);
        assert_eq!(none.bias, None);
    }
}
