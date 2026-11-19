// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! A simple debris-environment risk estimate along a cislunar trajectory.
//!
//! Three populations, all scenario inputs (MODELLED, not a validated environment model):
//!
//! * **catalogued objects** (from the catalogue build-up), each with its formal covariance
//!   propagated by the state-transition matrix to the encounter;
//! * **break-up clouds**: a parent orbit fragments a stated number of days before the
//!   trajectory starts; fragments are sampled with an isotropic Gaussian velocity spread from
//!   a seeded generator, propagated in the CR3BP, and each sample stands for
//!   `fragments_represented / fragments_sampled` real fragments, spread by a Gaussian kernel;
//! * **diffuse background**: spherical shells about the Moon or the Earth with a number
//!   density and an assumed mean relative speed (flux `n · A · v · Δt`).
//!
//! Encounters are found at local minima of the sampled separation, refined by linear relative
//! motion, and given a probability of collision by the two-dimensional short-encounter
//! integral of AK1 ([`crate::conjunction::pc::pc_2d`]) over the combined hard-body radius.
//! The expected collision count is the weighted sum over every source and the probability of
//! at least one collision is `1 − exp(−N)` (independent rare events).

use kshana::api::KshanaError;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::Deserialize;
use serde_json::{json, Value};

use super::catalogue::Catalogue;
use super::iod::propagate_fit;
use super::{bad, propagate, r9, synthetic, Problem, L_KM, MU, S6};

fn d_span() -> f64 {
    10.0
}
fn d_step() -> f64 {
    10.0
}
fn d_hbr() -> f64 {
    10.0
}
fn d_screen() -> f64 {
    1_000.0
}
fn d_true() -> bool {
    true
}
fn d_sigma_km() -> f64 {
    10.0
}

/// The trajectory at risk.
#[derive(Debug, Clone, Deserialize)]
pub struct TrajectoryConfig {
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub state: Option<S6>,
    /// A catalogued object id (for example `C-001`).
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub phase_frac: f64,
}

fn d_frag() -> usize {
    200
}
fn d_rep() -> f64 {
    1_000.0
}
fn d_dv() -> f64 {
    5.0
}
fn d_kernel() -> f64 {
    25.0
}

/// A break-up event.
#[derive(Debug, Clone, Deserialize)]
pub struct BreakupConfig {
    pub name: String,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub state: Option<S6>,
    #[serde(default)]
    pub phase_frac: f64,
    /// Days before the trajectory start at which the parent fragments.
    pub days_before_start: f64,
    #[serde(default = "d_frag")]
    pub fragments_sampled: usize,
    #[serde(default = "d_rep")]
    pub fragments_represented: f64,
    /// One-sigma velocity spread per axis (m/s).
    #[serde(default = "d_dv")]
    pub dv_sigma_mps: f64,
    /// Gaussian kernel (km) each sample is spread over.
    #[serde(default = "d_kernel")]
    pub kernel_sigma_km: f64,
    #[serde(default)]
    pub seed: u64,
}

/// A diffuse background shell.
#[derive(Debug, Clone, Deserialize)]
pub struct BackgroundConfig {
    pub name: String,
    /// `moon` or `earth`.
    pub centre: String,
    pub r_min_km: f64,
    pub r_max_km: f64,
    /// Objects per cubic kilometre.
    pub density_per_km3: f64,
    /// Assumed mean relative speed (km/s).
    pub v_rel_kms: f64,
}

/// The `[ssa.debris]` section.
#[derive(Debug, Clone, Deserialize)]
pub struct DebrisConfig {
    pub trajectory: TrajectoryConfig,
    /// Start of the assessment (UTC); defaults to the scenario epoch.
    #[serde(default)]
    pub start_utc: Option<String>,
    #[serde(default = "d_span")]
    pub span_days: f64,
    #[serde(default = "d_step")]
    pub step_minutes: f64,
    /// Combined hard-body radius (m).
    #[serde(default = "d_hbr")]
    pub hard_body_radius_m: f64,
    /// Separation (km) below which a local minimum is assessed as an encounter.
    #[serde(default = "d_screen")]
    pub screen_km: f64,
    #[serde(default = "d_true")]
    pub include_catalogue: bool,
    /// Position sigma (km) assumed for a catalogued object without a covariance.
    #[serde(default = "d_sigma_km")]
    pub default_sigma_km: f64,
    #[serde(default)]
    pub breakups: Vec<BreakupConfig>,
    #[serde(default)]
    pub background: Vec<BackgroundConfig>,
}

struct Encounter {
    source: String,
    object: String,
    tau: f64,
    miss_km: f64,
    v_rel_kms: f64,
    pc: f64,
    weight: f64,
    /// `short-encounter-2d` or `flux-integral` (slow, long encounters).
    method: &'static str,
}

/// Sample a trajectory on the grid; `None` entries after a flow failure.
fn sample(s0: &S6, n: usize, dt: f64) -> Vec<Option<S6>> {
    let mut out = Vec::with_capacity(n + 1);
    let mut s = Some(*s0);
    for k in 0..=n {
        out.push(s);
        if k < n {
            s = s.and_then(|x| propagate(&x, dt));
        }
    }
    out
}

/// Local-minimum encounters between the trajectory and one object (position covariance in
/// km² supplied per encounter by `cov_km2`).
#[allow(clippy::too_many_arguments)]
fn encounters(
    traj: &[Option<S6>],
    other: &[Option<S6>],
    tau0: f64,
    dt: f64,
    screen_km: f64,
    hbr_m: f64,
    cov_km2: &dyn Fn(f64) -> [[f64; 3]; 3],
    source: &str,
    object: &str,
    weight: f64,
) -> Vec<Encounter> {
    let d: Vec<f64> = traj
        .iter()
        .zip(other)
        .map(|(a, b)| match (a, b) {
            (Some(a), Some(b)) => super::norm3([a[0] - b[0], a[1] - b[1], a[2] - b[2]]) * L_KM,
            _ => f64::INFINITY,
        })
        .collect();
    let mut out = Vec::new();
    let v_unit = super::v_unit_kms();
    for k in 0..d.len() {
        let left = if k == 0 { f64::INFINITY } else { d[k - 1] };
        let right = d.get(k + 1).copied().unwrap_or(f64::INFINITY);
        if !(d[k] <= left && d[k] <= right && d[k] < screen_km) {
            continue;
        }
        let (Some(a), Some(b)) = (traj[k], other[k]) else {
            continue;
        };
        // Relative state (km, km/s); rotating-frame relative velocity plus ω × Δr gives the
        // inertial relative velocity.
        let dr = [
            (b[0] - a[0]) * L_KM,
            (b[1] - a[1]) * L_KM,
            (b[2] - a[2]) * L_KM,
        ];
        let dv = [
            (b[3] - a[3]) * v_unit - dr[1] / super::t_unit_s(),
            (b[4] - a[4]) * v_unit + dr[0] / super::t_unit_s(),
            (b[5] - a[5]) * v_unit,
        ];
        let v2 = super::dot3(dv, dv);
        if v2 <= 0.0 {
            continue;
        }
        let dt_s = dt * super::t_unit_s();
        let t_star = (-super::dot3(dr, dv) / v2).clamp(-dt_s, dt_s);
        let miss = [
            dr[0] + dv[0] * t_star,
            dr[1] + dv[1] * t_star,
            dr[2] + dv[2] * t_star,
        ];
        let miss_km = super::norm3(miss);
        let vhat = super::unit3(dv);
        let e1 = if miss_km > 1e-9 {
            super::unit3(miss)
        } else {
            // Any unit vector perpendicular to the relative velocity.
            let t = if vhat[0].abs() < 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            super::unit3(super::cross3(vhat, t))
        };
        let e2 = super::cross3(vhat, e1);
        let tau = tau0 + k as f64 * dt + t_star / super::t_unit_s();
        let p = cov_km2(tau);
        let proj = |u: [f64; 3], w: [f64; 3]| -> f64 {
            let mut s = 0.0;
            for i in 0..3 {
                for j in 0..3 {
                    s += u[i] * p[i][j] * w[j];
                }
            }
            s * 1e6 // km² → m²
        };
        let floor = 1.0; // m², keeps the 2-D covariance positive definite
        let p2 = [
            [proj(e1, e1) + floor, proj(e1, e2)],
            [proj(e2, e1), proj(e2, e2) + floor],
        ];
        // Encounter duration: six sigma along the relative velocity over the relative speed.
        let sig_along = (proj(vhat, vhat) / 1e6).max(0.0).sqrt();
        let duration_s = 6.0 * sig_along.max(hbr_m / 1e3) / v2.sqrt();
        let (pc, method) = if duration_s <= 4.0 * dt_s {
            (
                crate::conjunction::pc::pc_2d([miss_km * 1e3, 0.0], p2, hbr_m).unwrap_or(0.0),
                "short-encounter-2d",
            )
        } else {
            // Slow encounter: the short-encounter integral does not apply. Integrate the
            // flux of the object's position density through the hard-body cross-section
            // over the samples spanning the encounter, rate = pdf(Δr) · A · |Δv|.
            (
                flux_integral(traj, other, k, duration_s, dt_s, &p, hbr_m),
                "flux-integral",
            )
        };
        out.push(Encounter {
            source: source.to_string(),
            object: object.to_string(),
            tau,
            miss_km,
            v_rel_kms: v2.sqrt(),
            pc: pc.min(1.0),
            weight,
            method,
        });
    }
    out
}

/// Expected crossings of the hard-body cross-section through a Gaussian position density
/// (covariance `p`, km²) over the samples within half the encounter duration of sample `k`.
fn flux_integral(
    traj: &[Option<S6>],
    other: &[Option<S6>],
    k: usize,
    duration_s: f64,
    dt_s: f64,
    p: &[[f64; 3]; 3],
    hbr_m: f64,
) -> f64 {
    let rows: Vec<Vec<f64>> = p.iter().map(|r| r.to_vec()).collect();
    let Some(inv) = super::spd_inverse(&rows) else {
        return 0.0;
    };
    let det = p[0][0] * (p[1][1] * p[2][2] - p[1][2] * p[2][1])
        - p[0][1] * (p[1][0] * p[2][2] - p[1][2] * p[2][0])
        + p[0][2] * (p[1][0] * p[2][1] - p[1][1] * p[2][0]);
    if det.is_nan() || det <= 0.0 {
        return 0.0;
    }
    let norm = 1.0 / ((std::f64::consts::TAU).powf(1.5) * det.sqrt());
    let area = std::f64::consts::PI * (hbr_m / 1e3).powi(2);
    let half = ((0.5 * duration_s / dt_s).ceil() as usize).min(traj.len());
    let v_unit = super::v_unit_kms();
    let mut sum = 0.0;
    for j in k.saturating_sub(half)..(k + half + 1).min(traj.len()) {
        let (Some(a), Some(b)) = (traj[j], other[j]) else {
            continue;
        };
        let d = [
            (b[0] - a[0]) * L_KM,
            (b[1] - a[1]) * L_KM,
            (b[2] - a[2]) * L_KM,
        ];
        let v = [
            (b[3] - a[3]) * v_unit - d[1] / super::t_unit_s(),
            (b[4] - a[4]) * v_unit + d[0] / super::t_unit_s(),
            (b[5] - a[5]) * v_unit,
        ];
        let mut q = 0.0;
        for r in 0..3 {
            for c in 0..3 {
                q += d[r] * inv[r][c] * d[c];
            }
        }
        sum += norm * (-0.5 * q).exp() * area * super::norm3(v) * dt_s;
    }
    sum
}

fn gauss(rng: &mut ChaCha8Rng) -> f64 {
    let u1: f64 = rng.gen::<f64>().max(1e-300);
    let u2: f64 = rng.gen::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

fn state_of(
    preset: &Option<String>,
    state: &Option<S6>,
    phase: f64,
    what: &str,
) -> Result<S6, KshanaError> {
    match (state, preset) {
        (Some(s), _) => Ok(*s),
        (None, Some(p)) => synthetic::preset_state(p, phase),
        (None, None) => Err(bad(format!("{what}: give `preset` or `state`"))),
    }
}

/// Assess the debris environment along the configured trajectory.
pub fn assess(cfg: &DebrisConfig, p: &Problem, cat: &Catalogue) -> Result<Value, KshanaError> {
    if !(cfg.span_days > 0.0 && cfg.step_minutes > 0.0 && cfg.hard_body_radius_m > 0.0) {
        return Err(bad(
            "debris span_days, step_minutes and hard_body_radius_m must be positive",
        ));
    }
    let t_unit = super::t_unit_s();
    let tau0 = match &cfg.start_utc {
        Some(s) => (crate::conjunction::parse_utc(s)? - p.epoch_utc_s) / t_unit,
        None => 0.0,
    };
    let n = ((cfg.span_days * 1440.0) / cfg.step_minutes).ceil() as usize;
    if n > 200_000 {
        return Err(bad("debris grid too fine (more than 200,000 samples)"));
    }
    let dt = cfg.step_minutes * 60.0 / t_unit;
    let tr = &cfg.trajectory;
    let (traj0, own) = match &tr.object {
        Some(id) => {
            let o = cat.objects.iter().find(|o| o.id == *id).ok_or_else(|| {
                bad(format!(
                    "debris trajectory object '{id}' is not in the catalogue"
                ))
            })?;
            let (x, _) = propagate_fit(&o.fit, tau0)
                .ok_or_else(|| bad("trajectory object could not be propagated"))?;
            (x, Some(id.clone()))
        }
        None => {
            let s = state_of(&tr.preset, &tr.state, tr.phase_frac, "debris trajectory")?;
            (
                propagate(&s, tau0).ok_or_else(|| bad("trajectory could not be propagated"))?,
                None,
            )
        }
    };
    let traj = sample(&traj0, n, dt);
    let mut enc: Vec<Encounter> = Vec::new();
    let mut sources: Vec<Value> = Vec::new();

    // Catalogued objects.
    if cfg.include_catalogue {
        let mut count = 0usize;
        for o in &cat.objects {
            if own.as_deref() == Some(o.id.as_str()) {
                continue;
            }
            let Some((x0, _)) = propagate_fit(&o.fit, tau0) else {
                continue;
            };
            count += 1;
            let other = sample(&x0, n, dt);
            let s2 = (cfg.default_sigma_km).powi(2);
            let fit = o.fit.clone();
            let cov = move |tau: f64| -> [[f64; 3]; 3] {
                match propagate_fit(&fit, tau).and_then(|(_, c)| c) {
                    Some(c) => {
                        let k = L_KM * L_KM;
                        [
                            [c[0][0] * k, c[0][1] * k, c[0][2] * k],
                            [c[1][0] * k, c[1][1] * k, c[1][2] * k],
                            [c[2][0] * k, c[2][1] * k, c[2][2] * k],
                        ]
                    }
                    None => [[s2, 0.0, 0.0], [0.0, s2, 0.0], [0.0, 0.0, s2]],
                }
            };
            enc.extend(encounters(
                &traj,
                &other,
                tau0,
                dt,
                cfg.screen_km,
                cfg.hard_body_radius_m,
                &cov,
                "catalogue",
                &o.id,
                1.0,
            ));
        }
        let e: f64 = enc
            .iter()
            .filter(|e| e.source == "catalogue")
            .map(|e| e.pc * e.weight)
            .sum();
        sources.push(json!({"source": "catalogue", "kind": "catalogued objects with propagated formal covariance", "objects": count, "expected_collisions": r9(e)}));
    }

    // Break-up clouds.
    let mut fragments_xy: Vec<Value> = Vec::new();
    for b in &cfg.breakups {
        if b.fragments_sampled == 0 || b.days_before_start.is_nan() || b.days_before_start < 0.0 {
            return Err(bad(format!(
                "breakup {}: fragments_sampled must be positive and days_before_start non-negative",
                b.name
            )));
        }
        let parent = state_of(
            &b.preset,
            &b.state,
            b.phase_frac,
            &format!("breakup {}", b.name),
        )?;
        let tau_b = tau0 - b.days_before_start * 86_400.0 / t_unit;
        let parent_b = propagate(&parent, tau_b).ok_or_else(|| {
            bad(format!(
                "breakup {}: parent could not be propagated",
                b.name
            ))
        })?;
        let mut rng = ChaCha8Rng::seed_from_u64(b.seed);
        let sig = b.dv_sigma_mps / 1e3 / super::v_unit_kms();
        let w = b.fragments_represented / b.fragments_sampled as f64;
        let ks2 = b.kernel_sigma_km.powi(2);
        let kern = move |_: f64| [[ks2, 0.0, 0.0], [0.0, ks2, 0.0], [0.0, 0.0, ks2]];
        let mut lost = 0usize;
        let before = enc.len();
        for f in 0..b.fragments_sampled {
            let mut s = parent_b;
            for k in 3..6 {
                s[k] += gauss(&mut rng) * sig;
            }
            let Some(s0) = propagate(&s, tau0 - tau_b) else {
                lost += 1;
                continue;
            };
            if fragments_xy.len() < 2_000 {
                fragments_xy.push(json!([r9(s0[0]), r9(s0[1])]));
            }
            let other = sample(&s0, n, dt);
            enc.extend(encounters(
                &traj,
                &other,
                tau0,
                dt,
                cfg.screen_km,
                cfg.hard_body_radius_m,
                &kern,
                &b.name,
                &format!("{}#{f}", b.name),
                w,
            ));
        }
        let e: f64 = enc[before..].iter().map(|e| e.pc * e.weight).sum();
        sources.push(json!({"source": b.name, "kind": "break-up cloud (sampled fragments, Gaussian kernel)", "fragments_sampled": b.fragments_sampled, "fragments_represented": r9(b.fragments_represented), "fragments_lost_before_start": lost, "dv_sigma_mps": r9(b.dv_sigma_mps), "kernel_sigma_km": r9(b.kernel_sigma_km), "days_before_start": r9(b.days_before_start), "expected_collisions": r9(e)}));
    }

    // Diffuse background.
    let area_km2 = std::f64::consts::PI * (cfg.hard_body_radius_m / 1e3).powi(2);
    let dt_s = cfg.step_minutes * 60.0;
    for g in &cfg.background {
        let centre = match g.centre.to_ascii_lowercase().as_str() {
            "moon" => 1.0 - MU,
            "earth" => -MU,
            other => {
                return Err(bad(format!(
                    "background {}: unknown centre '{other}'",
                    g.name
                )))
            }
        };
        if !(g.density_per_km3 >= 0.0 && g.v_rel_kms >= 0.0 && g.r_max_km > g.r_min_km) {
            return Err(bad(format!("background {}: invalid shell", g.name)));
        }
        let mut e = 0.0;
        let mut inside = 0usize;
        for s in traj.iter().flatten() {
            let r = super::norm3([s[0] - centre, s[1], s[2]]) * L_KM;
            if r >= g.r_min_km && r < g.r_max_km {
                e += g.density_per_km3 * area_km2 * g.v_rel_kms * dt_s;
                inside += 1;
            }
        }
        sources.push(json!({"source": g.name, "kind": "diffuse background shell (flux n A v dt)", "centre": g.centre, "samples_inside": inside, "expected_collisions": r9(e)}));
    }

    let total: f64 = sources
        .iter()
        .filter_map(|s| s["expected_collisions"].as_f64())
        .sum();
    enc.sort_by(|a, b| (b.pc * b.weight).total_cmp(&(a.pc * a.weight)));
    let top: Vec<Value> = enc
        .iter()
        .take(20)
        .map(|e| {
            json!({
                "source": e.source,
                "object": e.object,
                "tca_utc": crate::conjunction::fmt_utc(p.epoch_utc_s + e.tau * t_unit),
                "miss_km": r9(e.miss_km),
                "v_rel_kms": r9(e.v_rel_kms),
                "pc": r9(e.pc),
                "weight": r9(e.weight),
                "method": e.method,
            })
        })
        .collect();
    let stride = (traj.len() / 200).max(1);
    let trajectory_xy: Vec<Value> = traj
        .iter()
        .step_by(stride)
        .flatten()
        .map(|s| json!([r9(s[0]), r9(s[1])]))
        .collect();
    let flow_ok = traj.iter().all(|s| s.is_some());
    Ok(json!({
        "evidence_class": "MODELLED",
        "method": "local-minimum encounter screening on a common grid, linear relative motion to closest approach, 2-D short-encounter probability of collision (AK1 pc_2d) over the combined hard-body radius when the encounter (six sigma along the relative velocity over the relative speed) lasts at most four steps, otherwise the flux of the Gaussian position density through the hard-body cross-section integrated over the encounter samples; background flux n A v dt; expected count summed, P(>=1) = 1 - exp(-N)",
        "start_utc": crate::conjunction::fmt_utc(p.epoch_utc_s + tau0 * t_unit),
        "span_days": r9(cfg.span_days),
        "step_minutes": r9(cfg.step_minutes),
        "hard_body_radius_m": r9(cfg.hard_body_radius_m),
        "trajectory": {"object": own, "preset": tr.preset, "flow_completed": flow_ok},
        "sources": sources,
        "encounters_assessed": enc.len(),
        "encounters_flux_integral": enc.iter().filter(|e| e.method == "flux-integral").count(),
        "top_encounters": top,
        "expected_collisions": r9(total),
        "probability_any_collision": r9(1.0 - (-total).exp()),
        "trajectory_xy": trajectory_xy,
        "fragments_xy": fragments_xy,
        "assumptions": [
            "CR3BP dynamics for the trajectory, every object and every fragment",
            "fragment velocity spread isotropic Gaussian; no area-to-mass or radiation-pressure effects",
            "background densities and relative speeds are scenario inputs",
            "encounters independent; linear relative motion near closest approach for the 2-D integral; the flux integral for slow encounters is a rare-event expected count sampled at the step",
        ],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// For a fast, rare encounter the flux integral of the position density equals the 2-D
    /// short-encounter probability (both are the expected number of crossings).
    #[test]
    fn flux_integral_matches_the_short_encounter_integral() {
        let v_unit = super::super::v_unit_kms();
        let (miss_km, v_kms, dt_s, sigma_km, hbr_m) = (1.0, 1.0, 0.05, 1.0, 10.0);
        let n = 400usize;
        let k = n / 2;
        let a: S6 = [1.0 - MU + 0.1, 0.0, 0.0, 0.0, 0.0, 0.0];
        let traj: Vec<Option<S6>> = vec![Some(a); n + 1];
        let other: Vec<Option<S6>> = (0..=n)
            .map(|j| {
                let z = (j as f64 - k as f64) * v_kms * dt_s;
                Some([
                    a[0] + miss_km / L_KM,
                    0.0,
                    z / L_KM,
                    0.0,
                    -miss_km / L_KM, // cancels ω × Δr so the relative motion is along z only
                    v_kms / v_unit,
                ])
            })
            .collect();
        let s2 = sigma_km * sigma_km;
        let p = [[s2, 0.0, 0.0], [0.0, s2, 0.0], [0.0, 0.0, s2]];
        let flux = flux_integral(&traj, &other, k, n as f64 * dt_s, dt_s, &p, hbr_m);
        let pc = crate::conjunction::pc::pc_2d(
            [miss_km * 1e3, 0.0],
            [[s2 * 1e6, 0.0], [0.0, s2 * 1e6]],
            hbr_m,
        )
        .unwrap();
        assert!((flux / pc - 1.0).abs() < 1e-3, "flux {flux:e} vs pc {pc:e}");
    }
}
