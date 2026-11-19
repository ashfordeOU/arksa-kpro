// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Angles-only initial orbit determination (IOD) and least-squares refinement in cislunar
//! space.
//!
//! **IOD.** Angles fix the direction to the object, not its range. Over a short tracklet the
//! classical two-body methods (Gauss, Laplace) do not apply: the object moves in the joint
//! field of the Earth and the Moon. Following the admissible-region idea, the range at the
//! first and at the last observation of the tracklet is hypothesised on a logarithmic grid;
//! each pair of hypothesised positions is joined by a **three-body boundary-value solve**
//! (single shooting on the CR3BP flow with the state-transition matrix, a Lambert problem in
//! the CR3BP), and the resulting orbit is scored against every observation of the tracklet.
//! The best distinct cells are each refined by least squares, and the lowest-cost solution is
//! kept; another converged solution within the ambiguity margin flags the IOD as ambiguous.
//!
//! **Refinement.** Levenberg–Marquardt on the whitened angle residuals with a central
//! finite-difference Jacobian of the composed forward model (CR3BP flow, ICRF mapping,
//! observer geometry); the formal covariance is `(HᵀWH)⁻¹` at the solution, valid only as far
//! as the stated angle noise is.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{propagate_stm, r9, radec_unit, sigmas, spd_inverse, Problem, M6, S6};

fn d_rho_min() -> f64 {
    30_000.0
}
fn d_rho_max() -> f64 {
    1_500_000.0
}
fn d_grid() -> usize {
    20
}
fn d_keep() -> usize {
    6
}
fn d_amb() -> f64 {
    9.0
}

/// IOD settings.
#[derive(Debug, Clone, Deserialize)]
pub struct IodConfig {
    /// Smallest and largest hypothesised topocentric range (km).
    #[serde(default = "d_rho_min")]
    pub rho_min_km: f64,
    #[serde(default = "d_rho_max")]
    pub rho_max_km: f64,
    /// Grid points per range axis (log-spaced).
    #[serde(default = "d_grid")]
    pub grid: usize,
    /// Distinct grid cells refined by least squares.
    #[serde(default = "d_keep")]
    pub keep: usize,
    /// Chi-square margin within which a second, distinct solution makes the IOD ambiguous.
    #[serde(default = "d_amb")]
    pub ambiguity_dchi2: f64,
}

impl Default for IodConfig {
    fn default() -> Self {
        Self {
            rho_min_km: d_rho_min(),
            rho_max_km: d_rho_max(),
            grid: d_grid(),
            keep: d_keep(),
            ambiguity_dchi2: d_amb(),
        }
    }
}

/// A least-squares fit of a synodic state at nondimensional time `tau_ref`.
#[derive(Debug, Clone)]
pub struct Fit {
    pub tau_ref: f64,
    pub x: S6,
    /// Formal covariance (nondimensional), if the normal matrix is positive definite.
    pub cov: Option<M6>,
    /// Sum of squared whitened residuals.
    pub chi2: f64,
    /// Number of scalar measurements (two per observation).
    pub m: usize,
    pub iterations: usize,
    pub converged: bool,
}

impl Fit {
    /// Chi-square per degree of freedom.
    pub fn reduced_chi2(&self) -> f64 {
        if self.m > 6 {
            self.chi2 / (self.m - 6) as f64
        } else {
            f64::NAN
        }
    }

    /// RMS of the whitened residuals.
    pub fn rms_whitened(&self) -> f64 {
        (self.chi2 / self.m.max(1) as f64).sqrt()
    }

    pub fn to_json(&self) -> Value {
        let (sp, sv) = self
            .cov
            .as_ref()
            .map(sigmas)
            .unwrap_or((f64::NAN, f64::NAN));
        json!({
            "chi2": r9(self.chi2),
            "measurements": self.m,
            "reduced_chi2": r9(self.reduced_chi2()),
            "sigma_pos_km": r9(sp),
            "sigma_vel_mps": r9(sv),
            "iterations": self.iterations,
            "converged": self.converged,
        })
    }
}

/// Central finite-difference Jacobian of the whitened residuals (the cross-check of the
/// analytic Jacobian, [`Problem::residuals_jac`]).
pub fn fd_jacobian(p: &Problem, idx: &[usize], tau_ref: f64, x: &S6) -> Option<Vec<[f64; 6]>> {
    let mut cols: Vec<Vec<f64>> = Vec::with_capacity(6);
    for j in 0..6 {
        let h = 1e-7;
        let mut xp = *x;
        let mut xm = *x;
        xp[j] += h;
        xm[j] -= h;
        let fp = p.residuals(idx, tau_ref, &xp)?;
        let fm = p.residuals(idx, tau_ref, &xm)?;
        cols.push(
            fp.iter()
                .zip(&fm)
                .map(|(a, b)| (a - b) / (2.0 * h))
                .collect(),
        );
    }
    let m = cols[0].len();
    Some(
        (0..m)
            .map(|i| {
                [
                    cols[0][i], cols[1][i], cols[2][i], cols[3][i], cols[4][i], cols[5][i],
                ]
            })
            .collect(),
    )
}

fn normal(jac: &[[f64; 6]], f: &[f64]) -> (Vec<Vec<f64>>, Vec<f64>) {
    let mut a = vec![vec![0.0; 6]; 6];
    let mut g = vec![0.0; 6];
    for (row, fi) in jac.iter().zip(f) {
        for p in 0..6 {
            g[p] += row[p] * fi;
            for q in 0..6 {
                a[p][q] += row[p] * row[q];
            }
        }
    }
    (a, g)
}

fn to_m6(a: &[Vec<f64>]) -> M6 {
    let mut m = [[0.0; 6]; 6];
    for i in 0..6 {
        for j in 0..6 {
            m[i][j] = a[i][j];
        }
    }
    m
}

/// Formal covariance of a state given the observations `idx`.
pub fn covariance(p: &Problem, idx: &[usize], tau_ref: f64, x: &S6) -> Option<M6> {
    let (f, jac) = p.residuals_jac(idx, tau_ref, x)?;
    let (a, _) = normal(&jac, &f);
    spd_inverse(&a).map(|c| to_m6(&c))
}

/// Levenberg–Marquardt refinement of a synodic state at `tau_ref` on the observations `idx`
/// (ascending indices). `None` if the forward model fails at the start.
pub fn refine(p: &Problem, idx: &[usize], tau_ref: f64, x0: &S6, max_iter: usize) -> Option<Fit> {
    let cost = |x: &S6| -> Option<f64> {
        p.residuals(idx, tau_ref, x)
            .map(|f| f.iter().map(|v| v * v).sum())
    };
    let mut x = *x0;
    let mut c = cost(&x)?;
    let mut lambda = 1e-3;
    let mut converged = false;
    let mut iterations = 0;
    for it in 0..max_iter {
        iterations = it + 1;
        let Some((f, jac)) = p.residuals_jac(idx, tau_ref, &x) else {
            break;
        };
        let (a, g) = normal(&jac, &f);
        let mut improved = false;
        for _ in 0..12 {
            let mut d = a.clone();
            for k in 0..6 {
                d[k][k] += lambda * a[k][k].max(1e-12);
            }
            let rhs: Vec<f64> = g.iter().map(|v| -v).collect();
            let Some(dx) = super::solve(&d, &rhs) else {
                lambda *= 10.0;
                continue;
            };
            let mut xn = x;
            for k in 0..6 {
                xn[k] += dx[k];
            }
            match cost(&xn) {
                Some(cn) if cn < c => {
                    let rel = (c - cn) / c.max(1e-300);
                    let step = dx.iter().map(|v| v * v).sum::<f64>().sqrt();
                    x = xn;
                    c = cn;
                    lambda = (lambda / 5.0).max(1e-12);
                    improved = true;
                    if rel < 1e-10 || step < 1e-13 {
                        converged = true;
                    }
                    break;
                }
                _ => lambda *= 8.0,
            }
        }
        if !improved {
            // No descent at any damping: a (local) minimum to working precision.
            converged = true;
            break;
        }
        if converged {
            break;
        }
    }
    Some(Fit {
        tau_ref,
        x,
        cov: covariance(p, idx, tau_ref, &x),
        chi2: c,
        m: 2 * idx.len(),
        iterations,
        converged,
    })
}

/// Solve the CR3BP two-point boundary-value problem: the velocity at `r0` that reaches `r1`
/// after `dt` (nondimensional), by single shooting with the state-transition matrix.
pub fn lambert_cr3bp(r0: [f64; 3], r1: [f64; 3], dt: f64) -> Option<S6> {
    if dt.abs() < 1e-9 {
        return None;
    }
    // The rotating-frame chord as the first guess.
    let mut v = [
        (r1[0] - r0[0]) / dt,
        (r1[1] - r0[1]) / dt,
        (r1[2] - r0[2]) / dt,
    ];
    if v.iter().map(|x| x * x).sum::<f64>().sqrt() > 6.0 {
        return None; // a chord faster than about 6 km/s is no bound cislunar arc
    }
    for _ in 0..12 {
        let s = [r0[0], r0[1], r0[2], v[0], v[1], v[2]];
        let (e, phi) = propagate_stm(&s, dt)?;
        let err = [e[0] - r1[0], e[1] - r1[1], e[2] - r1[2]];
        let n = (err[0] * err[0] + err[1] * err[1] + err[2] * err[2]).sqrt();
        if n < 1e-11 {
            return Some(s);
        }
        let a: Vec<Vec<f64>> = (0..3)
            .map(|i| vec![phi[i][3], phi[i][4], phi[i][5]])
            .collect();
        let dv = super::solve(&a, &[-err[0], -err[1], -err[2]])?;
        let scale = {
            let m = (dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2]).sqrt();
            if m > 0.5 {
                0.5 / m
            } else {
                1.0
            }
        };
        for k in 0..3 {
            v[k] += scale * dv[k];
        }
        if v.iter().any(|x| !x.is_finite() || x.abs() > 10.0) {
            return None;
        }
    }
    None
}

/// One IOD outcome.
#[derive(Debug, Clone)]
pub struct Iod {
    pub fit: Fit,
    /// Grid cells whose boundary-value solve converged.
    pub cells_solved: usize,
    pub cells_total: usize,
    /// Distinct converged least-squares solutions (chi2, distance from the best in km).
    pub alternatives: Vec<(f64, f64)>,
    pub ambiguous: bool,
}

impl Iod {
    pub fn to_json(&self) -> Value {
        json!({
            "method": "range-hypothesis grid (log-spaced, first and last observation) with a CR3BP boundary-value solve per cell, best distinct cells refined by Levenberg-Marquardt",
            "cells_solved": self.cells_solved,
            "cells_total": self.cells_total,
            "ambiguous": self.ambiguous,
            "alternatives": self.alternatives.iter().map(|(c, d)| json!({"chi2": r9(*c), "distance_from_best_km": r9(*d)})).collect::<Vec<_>>(),
            "fit": self.fit.to_json(),
        })
    }
}

/// Angles-only IOD on the observations `idx` (ascending, at least three observations spread
/// over a non-zero arc). The fitted state is at the time of the first observation.
pub fn iod(p: &Problem, idx: &[usize], cfg: &IodConfig) -> Result<Iod, String> {
    if idx.len() < 3 {
        return Err(format!("{} observations (IOD needs at least 3)", idx.len()));
    }
    let (a, b) = (idx[0], idx[idx.len() - 1]);
    let (ga, gb) = (&p.geo[a], &p.geo[b]);
    let dt = gb.tau - ga.tau;
    if dt * super::t_unit_s() < 600.0 {
        return Err("tracklet arc shorter than 10 minutes".into());
    }
    if !(cfg.rho_min_km > 0.0 && cfg.rho_max_km > cfg.rho_min_km && cfg.grid >= 2) {
        return Err("invalid IOD range grid".into());
    }
    let ua = radec_unit(p.obs[a].ra, p.obs[a].dec);
    let ub = radec_unit(p.obs[b].ra, p.obs[b].dec);
    let n = cfg.grid;
    let rho = |k: usize| {
        let f = k as f64 / (n - 1) as f64;
        cfg.rho_min_km * (cfg.rho_max_km / cfg.rho_min_km).powf(f) * 1e3
    };
    let mut cells: Vec<(f64, S6)> = Vec::new();
    for i in 0..n {
        for j in 0..n {
            let (ra_m, rb_m) = (rho(i), rho(j));
            let pa = [
                ga.obs_m[0] + ra_m * ua[0],
                ga.obs_m[1] + ra_m * ua[1],
                ga.obs_m[2] + ra_m * ua[2],
            ];
            let pb = [
                gb.obs_m[0] + rb_m * ub[0],
                gb.obs_m[1] + rb_m * ub[1],
                gb.obs_m[2] + rb_m * ub[2],
            ];
            let (sa, sb) = (ga.frame.geo_to_syn(pa), gb.frame.geo_to_syn(pb));
            if super::moon_dist_km(&sa) < super::MOON_RADIUS_KM
                || super::earth_dist_km(&sa) < super::EARTH_RADIUS_KM
            {
                continue;
            }
            let Some(x) = lambert_cr3bp(sa, sb, dt) else {
                continue;
            };
            if let Some(f) = p.residuals(idx, ga.tau, &x) {
                cells.push((f.iter().map(|v| v * v).sum(), x));
            }
        }
    }
    let cells_solved = cells.len();
    if cells.is_empty() {
        return Err("no range-hypothesis cell produced a boundary-value solution".into());
    }
    cells.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut seeds: Vec<S6> = Vec::new();
    for (_, x) in &cells {
        let distinct = seeds.iter().all(|s| {
            let d = ((s[0] - x[0]).powi(2) + (s[1] - x[1]).powi(2) + (s[2] - x[2]).powi(2)).sqrt();
            d > 0.01 * super::norm3([x[0] - (1.0 - super::MU), x[1], x[2]]).max(1e-3)
        });
        if distinct {
            seeds.push(*x);
        }
        if seeds.len() >= cfg.keep.max(1) {
            break;
        }
    }
    let mut fits: Vec<Fit> = seeds
        .iter()
        .filter_map(|s| refine(p, idx, ga.tau, s, 60))
        .filter(|f| f.chi2.is_finite())
        .collect();
    if fits.is_empty() {
        return Err("least-squares refinement failed for every IOD candidate".into());
    }
    fits.sort_by(|x, y| x.chi2.total_cmp(&y.chi2));
    let best = fits[0].clone();
    let mut alternatives: Vec<(f64, f64)> = Vec::new();
    for f in &fits[1..] {
        let d = super::norm3([f.x[0] - best.x[0], f.x[1] - best.x[1], f.x[2] - best.x[2]])
            * super::L_KM;
        if d > 1.0 && alternatives.iter().all(|(_, d0)| (d0 - d).abs() > 1.0) {
            alternatives.push((f.chi2, d));
        }
    }
    let ambiguous = alternatives
        .iter()
        .any(|(c, d)| *c <= best.chi2 + cfg.ambiguity_dchi2 && *d > 100.0);
    Ok(Iod {
        fit: best,
        cells_solved,
        cells_total: n * n,
        alternatives,
        ambiguous,
    })
}

/// Propagate a fitted state and its covariance to another time (linear mapping `Φ P Φᵀ`).
pub fn propagate_fit(f: &Fit, tau: f64) -> Option<(S6, Option<M6>)> {
    let (x, phi) = propagate_stm(&f.x, tau - f.tau_ref)?;
    let cov = f.cov.map(|p| {
        let mut t = [[0.0; 6]; 6];
        for i in 0..6 {
            for j in 0..6 {
                t[i][j] = (0..6).map(|k| phi[i][k] * p[k][j]).sum();
            }
        }
        let mut o = [[0.0; 6]; 6];
        for i in 0..6 {
            for j in 0..6 {
                o[i][j] = (0..6).map(|k| t[i][k] * phi[j][k]).sum();
            }
        }
        o
    });
    Some((x, cov))
}

#[cfg(test)]
mod tests {
    use super::super::propagate;
    use super::*;

    #[test]
    fn cr3bp_lambert_recovers_a_known_arc() {
        let s0: S6 = [1.02, 0.01, -0.17, 0.003, -0.09, 0.01];
        let dt = 0.3;
        let s1 = propagate(&s0, dt).unwrap();
        let v = lambert_cr3bp([s0[0], s0[1], s0[2]], [s1[0], s1[1], s1[2]], dt).unwrap();
        for k in 3..6 {
            assert!((v[k] - s0[k]).abs() < 1e-8, "{v:?} vs {s0:?}");
        }
    }

    #[test]
    fn backward_propagation_inverts_forward() {
        let s0: S6 = [0.85, 0.02, 0.05, 0.01, 0.15, -0.02];
        let s1 = propagate(&s0, 0.4).unwrap();
        let back = propagate(&s1, -0.4).unwrap();
        for k in 0..6 {
            assert!((back[k] - s0[k]).abs() < 1e-9);
        }
        // The backward STM is the inverse of the forward one.
        let (_, f) = propagate_stm(&s0, 0.4).unwrap();
        let (_, b) = propagate_stm(&s1, -0.4).unwrap();
        for i in 0..6 {
            for j in 0..6 {
                let v: f64 = (0..6).map(|k| b[i][k] * f[k][j]).sum();
                let e = if i == j { 1.0 } else { 0.0 };
                assert!((v - e).abs() < 1e-6, "({i},{j}) {v}");
            }
        }
    }
}
