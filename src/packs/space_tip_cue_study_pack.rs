// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # `pro-space-tip-cue-study` (roadmap row M9; 0.42 F6, moved here as a space-mission job)
//!
//! A constellation trade for a two-layer maritime tip-and-cue system: planes against
//! satellites per plane, with revisit, full-area refresh and RF-to-optical reaction from the
//! open engine's M2 planner ([`kshana::tip_and_cue`]), a seeded Monte Carlo capture rate on
//! top of the M3 assumptions, a cost axis in the B1b convention (`capex + years x opex`), the
//! exact non-dominated front and the cheapest design meeting a target.
//!
//! Pre-registration: `docs/evidence/m9-space-tip-cue/PREREGISTRATION-2026-11-15.md`
//! (committed before this code). MODELLED: geometry, timing and an assumed vessel motion
//! model; illustrative costs; nothing measured.

use kshana::api::{ExternalPack, KshanaError, RunOutput, Scenario, ScenarioMeta};
use kshana::tip_and_cue::{self as tc, Case, Field, Grid, Layer};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The kind id.
pub const KIND: &str = "pro-space-tip-cue-study";
/// The result schema.
pub const SCHEMA: &str = "arksa-space-tip-cue-study/1";

/// The honesty label.
pub const LABEL: &str = "MODELLED constellation trade for maritime tip-and-cue: revisit, refresh and \
RF-to-optical reaction from the open M2 planner (geometry and timing only, J2 secular, spherical-Earth \
access), a seeded Monte Carlo capture rate under an assumed vessel motion and fix-noise model (M3 \
assumptions; M3 itself not run), and illustrative costs. No measured revisit, reaction, track or cost \
data is used. Not VALIDATED.";

const DEG: f64 = std::f64::consts::PI / 180.0;
const KN: f64 = 1852.0 / 3600.0;

fn d_seed() -> u64 {
    9009
}
fn d_planes() -> Vec<usize> {
    vec![4, 8, 16]
}
fn d_per_plane() -> Vec<usize> {
    vec![1, 2, 4]
}
fn d_span_days() -> u32 {
    3
}
fn d_step_s() -> u32 {
    20
}
fn d_thin() -> usize {
    2
}
fn d_rf_mask() -> f64 {
    10.0
}
fn d_off_nadir() -> f64 {
    45.0
}
fn d_n_mc() -> usize {
    4000
}
fn d_sigma_fix() -> f64 {
    1000.0
}
fn d_speed() -> [f64; 2] {
    [16.0, 22.0]
}
fn d_p_turn() -> f64 {
    0.25
}
fn d_turn() -> [f64; 2] {
    [20.0, 90.0]
}
fn d_radii() -> Vec<f64> {
    vec![2.5, 5.0, 10.0]
}
fn d_costs() -> Costs {
    Costs {
        rf_satellite: 2.0,
        optical_satellite: 3.0,
        per_plane_launch: 1.5,
        opex_per_satellite_year: 0.1,
        years: 5.0,
        currency: "M (illustrative)".into(),
    }
}
fn d_target() -> Target {
    Target {
        capture_5km_min: 0.90,
        reaction_max_min: 15.0,
    }
}

/// The cost model (B1b convention, illustrative defaults).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Costs {
    pub rf_satellite: f64,
    pub optical_satellite: f64,
    /// One launch slot per plane per layer.
    pub per_plane_launch: f64,
    pub opex_per_satellite_year: f64,
    pub years: f64,
    pub currency: String,
}

/// The target for the cheapest qualifying design.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub capture_5km_min: f64,
    pub reaction_max_min: f64,
}

/// A `pro-space-tip-cue-study` scenario; every field has the pre-registered default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceTipCueStudyScenario {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default = "d_seed")]
    pub seed: u64,
    #[serde(default = "d_planes")]
    pub planes: Vec<usize>,
    #[serde(default = "d_per_plane")]
    pub per_plane: Vec<usize>,
    #[serde(default = "d_span_days")]
    pub span_days: u32,
    #[serde(default = "d_step_s")]
    pub step_s: u32,
    /// Keep every n-th M2 sea cell in latitude and longitude (1 keeps all 996).
    #[serde(default = "d_thin")]
    pub thin: usize,
    #[serde(default = "d_rf_mask")]
    pub rf_mask_deg: f64,
    #[serde(default = "d_off_nadir")]
    pub optical_off_nadir_deg: f64,
    #[serde(default = "d_n_mc")]
    pub mc_cues: usize,
    #[serde(default = "d_sigma_fix")]
    pub sigma_fix_m: f64,
    #[serde(default = "d_speed")]
    pub speed_kn: [f64; 2],
    #[serde(default = "d_p_turn")]
    pub p_turn: f64,
    #[serde(default = "d_turn")]
    pub turn_deg: [f64; 2],
    #[serde(default = "d_radii")]
    pub radii_km: Vec<f64>,
    #[serde(default = "d_costs")]
    pub cost: Costs,
    #[serde(default = "d_target")]
    pub target: Target,
}

impl SpaceTipCueStudyScenario {
    pub const KIND: &'static str = KIND;

    pub fn meta_static() -> ScenarioMeta {
        ScenarioMeta {
            name: Self::KIND,
            description: "Space tip-and-cue study: a constellation trade (planes against satellites per plane) for an RF survey layer cueing an optical layer over a maritime area: revisit, refresh and reaction from the open planner, a seeded capture rate, a cost axis, the non-dominated front and the cheapest design meeting a target. MODELLED; reproduces from scenario and seed.",
            required_fields: &[],
            optional_fields: &[
                "title",
                "seed",
                "planes",
                "per_plane",
                "span_days",
                "step_s",
                "thin",
                "rf_mask_deg",
                "optical_off_nadir_deg",
                "mc_cues",
                "sigma_fix_m",
                "speed_kn",
                "p_turn",
                "turn_deg",
                "radii_km",
                "cost",
                "target",
            ],
        }
    }
}

fn bad(m: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(format!("{KIND}: {}", m.into()))
}

/// The study points: the M2 sea points, every `thin`-th cell in both directions.
pub fn points(thin: usize) -> Vec<(f64, f64)> {
    let thin = thin.max(1) as i64;
    tc::baltic_points()
        .into_iter()
        .filter(|&(la, lo)| {
            let i = ((la - 53.5) / 0.25 - 0.5).round() as i64;
            let j = ((lo - 9.5) / 0.25 - 0.5).round() as i64;
            i % thin == 0 && j % thin == 0
        })
        .collect()
}

/// The M2-convention case of a design.
pub fn case(p: usize, k: usize, mask: f64, off: f64) -> Case {
    let layer = |node_offset| Layer {
        total: p * k,
        planes: p,
        phasing: 1,
        node_offset,
    };
    Case {
        name: format!("{p}x{k}"),
        rf: layer(0.0),
        rf_mask_deg: mask,
        optical: layer(0.5),
        optical_off_nadir_deg: off,
        optical_min_sun_deg: None,
    }
}

/// One cue: its point, its wait to the first optical look (`None`: unanswered) and the
/// point's mean start-to-start interval between RF accesses (the fix spacing).
#[derive(Debug, Clone, Copy)]
pub struct Cue {
    pub point: usize,
    pub wait_s: Option<f64>,
    pub spacing_s: f64,
}

/// Every cue of a design, per point (the engine's Q rule).
pub fn cues(rf: &[Vec<[u32; 2]>], opt: &[Vec<[u32; 2]>], until_s: u32) -> Vec<Vec<Cue>> {
    rf.iter()
        .zip(opt)
        .enumerate()
        .map(|(i, (r, o))| {
            let s2s: Vec<f64> = r.windows(2).map(|w| f64::from(w[1][0] - w[0][0])).collect();
            let spacing = if s2s.is_empty() {
                f64::from(until_s)
            } else {
                s2s.iter().sum::<f64>() / s2s.len() as f64
            }
            .max(60.0);
            r.iter()
                .filter(|iv| iv[0] < until_s)
                .map(|&[s, _]| {
                    let k = o.partition_point(|iv| iv[1] < s);
                    Cue {
                        point: i,
                        wait_s: o.get(k).map(|&[os, _]| f64::from(os.saturating_sub(s))),
                        spacing_s: spacing,
                    }
                })
                .collect()
        })
        .collect()
}

fn normal(rng: &mut ChaCha8Rng) -> f64 {
    // Box-Muller, one draw.
    let u1: f64 = rng.gen_range(f64::MIN_POSITIVE..1.0);
    let u2: f64 = rng.gen::<f64>();
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// The capture Monte Carlo of one design.
#[derive(Debug, Clone, Serialize)]
pub struct Capture {
    /// Fraction captured per radius.
    pub rate: Vec<f64>,
    /// Mean closed-form capture per radius over the same sampled cues, with no turn
    /// (the reference of bar R4; meaningful against `rate` only when `p_turn` is 0).
    pub closed_form_no_turn: Vec<f64>,
    /// `sqrt(sum P_i (1 - P_i)) / N` of the closed form, per radius.
    pub closed_form_sd: Vec<f64>,
    pub samples: usize,
    pub unanswered_drawn: usize,
}

/// Run the capture Monte Carlo for one design.
pub fn capture(
    s: &SpaceTipCueStudyScenario,
    pts: &[(f64, f64)],
    cues: &[Vec<Cue>],
    stream: u64,
) -> Capture {
    let mut rng = ChaCha8Rng::seed_from_u64(s.seed.wrapping_add(stream));
    let w: Vec<f64> = pts
        .iter()
        .zip(cues)
        .map(|(&(la, _), c)| if c.is_empty() { 0.0 } else { (la * DEG).cos() })
        .collect();
    let wsum: f64 = w.iter().sum();
    let nr = s.radii_km.len();
    let (mut hit, mut cf, mut cfv) = (vec![0usize; nr], vec![0.0; nr], vec![0.0; nr]);
    let mut unanswered = 0usize;
    let n = if wsum > 0.0 { s.mc_cues } else { 0 };
    let sig = s.sigma_fix_m;
    for _ in 0..n {
        // A point by area weight, then one of its cues.
        let mut x = rng.gen::<f64>() * wsum;
        let mut pi = 0;
        for (i, wi) in w.iter().enumerate() {
            if *wi > 0.0 {
                pi = i;
                if x < *wi {
                    break;
                }
                x -= wi;
            }
        }
        let c = cues[pi][rng.gen_range(0..cues[pi].len())];
        // Draw the vessel and noise even for an unanswered cue, so streams stay aligned.
        let v = rng.gen_range(s.speed_kn[0]..=s.speed_kn[1]) * KN;
        let h0 = rng.gen_range(0.0..360.0) * DEG;
        let turn = rng.gen::<f64>() < s.p_turn;
        let d = c.spacing_s;
        let q = c.wait_s.unwrap_or(0.0);
        let t_turn = rng.gen_range(-2.0 * d..=q.max(-2.0 * d + 1e-9));
        let ang = rng.gen_range(s.turn_deg[0]..=s.turn_deg[1])
            * DEG
            * if rng.gen::<bool>() { 1.0 } else { -1.0 };
        let noise: Vec<f64> = (0..6).map(|_| normal(&mut rng) * sig).collect();
        let Some(q) = c.wait_s else {
            unanswered += 1;
            continue;
        };
        // Truth: straight from t = -2d at heading h0, one turn at t_turn when drawn.
        let truth = |t: f64| -> (f64, f64) {
            let leg = |h: f64, dt: f64| (v * dt * h.sin(), v * dt * h.cos());
            if turn && t > t_turn {
                let (e1, n1) = leg(h0, t_turn + 2.0 * d);
                let (e2, n2) = leg(h0 + ang, t - t_turn);
                (e1 + e2, n1 + n2)
            } else {
                leg(h0, t + 2.0 * d)
            }
        };
        let ts = [-2.0 * d, -d, 0.0];
        let fixes: Vec<(f64, f64)> = ts
            .iter()
            .enumerate()
            .map(|(i, &t)| {
                let (e, n) = truth(t);
                (e + noise[2 * i], n + noise[2 * i + 1])
            })
            .collect();
        // Least-squares line through the three fixes (t mean -d, sum of squares 2 d^2).
        let pred = |sel: fn(&(f64, f64)) -> f64| {
            let m = fixes.iter().map(sel).sum::<f64>() / 3.0;
            let slope = ts
                .iter()
                .zip(&fixes)
                .map(|(t, f)| (t + d) * (sel(f) - m))
                .sum::<f64>()
                / (2.0 * d * d);
            m + slope * (q + d)
        };
        let (pe, pn) = (pred(|f| f.0), pred(|f| f.1));
        let (te, tn) = truth(q);
        let err = ((pe - te).powi(2) + (pn - tn).powi(2)).sqrt();
        let s2 = sig * sig * (1.0 / 3.0 + (q + d).powi(2) / (2.0 * d * d));
        for (i, r) in s.radii_km.iter().enumerate() {
            let r = r * 1000.0;
            if err <= r {
                hit[i] += 1;
            }
            let p = 1.0 - (-(r * r) / (2.0 * s2)).exp();
            cf[i] += p;
            cfv[i] += p * (1.0 - p);
        }
    }
    let nn = n.max(1) as f64;
    Capture {
        rate: hit.iter().map(|h| *h as f64 / nn).collect(),
        closed_form_no_turn: cf.iter().map(|x| x / nn).collect(),
        closed_form_sd: cfv.iter().map(|x| x.sqrt() / nn).collect(),
        samples: n,
        unanswered_drawn: unanswered,
    }
}

/// One design's figures.
#[derive(Debug, Clone, Serialize)]
pub struct Design {
    pub planes: usize,
    pub per_plane: usize,
    pub satellites_per_layer: usize,
    pub revisit_mean_s: Option<f64>,
    pub revisit_median_s: Option<f64>,
    pub refresh_mean_s: Option<f64>,
    pub reaction_mean_s: Option<f64>,
    pub reaction_median_s: Option<f64>,
    pub reaction_p95_s: Option<f64>,
    pub cues: usize,
    pub cues_unanswered: usize,
    pub capture: Capture,
    pub cost: f64,
}

/// Run the trade; returns the designs (figures and capture) without the front.
pub fn run_designs(s: &SpaceTipCueStudyScenario) -> Result<(Vec<Design>, usize), KshanaError> {
    if s.planes.is_empty() || s.per_plane.is_empty() {
        return Err(bad("planes and per_plane need at least one value"));
    }
    if s.planes
        .iter()
        .chain(&s.per_plane)
        .any(|&x| x == 0 || x > 64)
    {
        return Err(bad("planes and per_plane must be in 1..=64"));
    }
    if s.span_days < 2 || s.span_days > 7 || s.step_s == 0 || s.step_s > 120 {
        return Err(bad("span_days in 2..=7 and step_s in 1..=120"));
    }
    if !(0.0..=1.0).contains(&s.p_turn) || s.sigma_fix_m <= 0.0 || s.radii_km.is_empty() {
        return Err(bad(
            "p_turn in [0, 1], sigma_fix_m > 0, at least one radius",
        ));
    }
    if s.speed_kn[0] > s.speed_kn[1] || s.turn_deg[0] > s.turn_deg[1] {
        return Err(bad("speed_kn and turn_deg are [low, high]"));
    }
    let pts = points(s.thin);
    let grid = Grid {
        step_s: s.step_s,
        span_s: s.span_days * 86_400,
    };
    let until = grid.span_s - 86_400;
    let mut out = Vec::new();
    let mut stream = 0u64;
    for &p in &s.planes {
        for &k in &s.per_plane {
            let c = case(p, k, s.rf_mask_deg, s.optical_off_nadir_deg);
            let rf = tc::merged_access(
                &c.rf.elements().map_err(bad)?,
                Field::Elevation(c.rf_mask_deg),
                &pts,
                grid,
                None,
            );
            let opt = tc::merged_access(
                &c.optical.elements().map_err(bad)?,
                Field::OffNadir(c.optical_off_nadir_deg),
                &pts,
                grid,
                None,
            );
            let (rev, _, _) = tc::revisit(&pts, &rf);
            let refresh = tc::full_refresh(&rf, 60, until);
            let (reac, unanswered) = tc::reaction(&pts, &rf, &opt, until);
            let cs = cues(&rf, &opt, until);
            let cap = capture(s, &pts, &cs, stream);
            stream += 1;
            let n_sat = (p * k) as f64;
            let cost = n_sat * (s.cost.rf_satellite + s.cost.optical_satellite)
                + 2.0 * p as f64 * s.cost.per_plane_launch
                + s.cost.years * 2.0 * n_sat * s.cost.opex_per_satellite_year;
            out.push(Design {
                planes: p,
                per_plane: k,
                satellites_per_layer: p * k,
                revisit_mean_s: rev.value_s,
                revisit_median_s: rev.median_s,
                refresh_mean_s: refresh.value_s,
                reaction_mean_s: reac.value_s,
                reaction_median_s: reac.median_s,
                reaction_p95_s: reac.p95_s,
                cues: cs.iter().map(Vec::len).sum(),
                cues_unanswered: unanswered,
                capture: cap,
                cost,
            });
        }
    }
    Ok((out, pts.len()))
}

fn primary_capture(d: &Design, radii: &[f64]) -> f64 {
    let i = radii
        .iter()
        .position(|r| (*r - 5.0).abs() < 1e-9)
        .unwrap_or(0);
    d.capture.rate[i]
}

/// Indices of the exact non-dominated designs over (cost lower, capture at 5 km higher,
/// mean reaction lower).
pub fn front(ds: &[Design], radii: &[f64]) -> Vec<usize> {
    let key = |d: &Design| {
        (
            d.cost,
            -primary_capture(d, radii),
            d.reaction_mean_s.unwrap_or(f64::INFINITY),
        )
    };
    (0..ds.len())
        .filter(|&i| {
            let a = key(&ds[i]);
            !ds.iter().any(|o| {
                let b = key(o);
                b.0 <= a.0 && b.1 <= a.1 && b.2 <= a.2 && (b.0 < a.0 || b.1 < a.1 || b.2 < a.2)
            })
        })
        .collect()
}

/// The full study result.
pub fn run_study(s: &SpaceTipCueStudyScenario) -> Result<Value, KshanaError> {
    let (ds, n_pts) = run_designs(s)?;
    let f = front(&ds, &s.radii_km);
    let cheapest = ds
        .iter()
        .enumerate()
        .filter(|(_, d)| {
            primary_capture(d, &s.radii_km) >= s.target.capture_5km_min
                && d.reaction_mean_s
                    .is_some_and(|q| q <= s.target.reaction_max_min * 60.0)
        })
        .min_by(|a, b| a.1.cost.total_cmp(&b.1.cost))
        .map(|(i, _)| i);
    let mut doc = json!({
        "schema": SCHEMA,
        "kind": KIND,
        "title": s.title.clone().unwrap_or_else(|| "Space tip-and-cue constellation trade".into()),
        "label": LABEL,
        "evidence_class": "MODELLED",
        "preregistration": "docs/evidence/m9-space-tip-cue/PREREGISTRATION-2026-11-15.md",
        "scenario": s,
        "points": n_pts,
        "designs": ds,
        "front": f.iter().map(|&i| format!("{}x{}", ds[i].planes, ds[i].per_plane)).collect::<Vec<_>>(),
        "cheapest_meeting_target": cheapest.map(|i| format!("{}x{}", ds[i].planes, ds[i].per_plane)),
    });
    let digest = hex::encode(Sha256::digest(
        serde_json::to_vec(&doc).map_err(|e| bad(e.to_string()))?,
    ));
    doc["result_sha256"] = json!(digest);
    Ok(doc)
}

/// The trade table as CSV.
pub fn table_csv(doc: &Value) -> String {
    let mut s = String::from("# MODELLED (see label)\ndesign,satellites_per_layer,cost,revisit_mean_min,refresh_mean_min,reaction_mean_min,reaction_median_min,reaction_p95_min,cues,cues_unanswered");
    let radii: Vec<f64> = doc["scenario"]["radii_km"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_f64)
        .collect();
    for r in &radii {
        s.push_str(&format!(",capture_{r}km"));
    }
    s.push_str(",on_front\n");
    let front: Vec<&str> = doc["front"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let m = |v: &Value| v.as_f64().map_or("".into(), |x| format!("{:.2}", x / 60.0));
    for d in doc["designs"].as_array().into_iter().flatten() {
        let name = format!("{}x{}", d["planes"], d["per_plane"]);
        s.push_str(&format!(
            "{name},{},{:.1},{},{},{},{},{},{},{}",
            d["satellites_per_layer"],
            d["cost"].as_f64().unwrap_or(0.0),
            m(&d["revisit_mean_s"]),
            m(&d["refresh_mean_s"]),
            m(&d["reaction_mean_s"]),
            m(&d["reaction_median_s"]),
            m(&d["reaction_p95_s"]),
            d["cues"],
            d["cues_unanswered"]
        ));
        for i in 0..radii.len() {
            s.push_str(&format!(
                ",{:.4}",
                d["capture"]["rate"][i].as_f64().unwrap_or(0.0)
            ));
        }
        s.push_str(&format!(",{}\n", front.contains(&name.as_str())));
    }
    s
}

impl Scenario for SpaceTipCueStudyScenario {
    fn run(&self) -> Result<RunOutput, KshanaError> {
        let doc = run_study(self)?;
        let summary = format!(
            "MODELLED tip-and-cue trade: {} designs on {} points; front {}; cheapest meeting target {} (seed {}, result {})",
            doc["designs"].as_array().map_or(0, Vec::len),
            doc["points"],
            doc["front"],
            doc["cheapest_meeting_target"],
            self.seed,
            &doc["result_sha256"].as_str().unwrap_or("")[..12]
        );
        Ok(RunOutput {
            csv: Some(table_csv(&doc)),
            json: serde_json::to_string_pretty(&doc).map_err(|e| bad(e.to_string()))?,
            svg: String::new(),
            summary,
        })
    }
}

impl ExternalPack for SpaceTipCueStudyScenario {
    fn kind_name(&self) -> &'static str {
        Self::KIND
    }
    fn meta(&self) -> ScenarioMeta {
        Self::meta_static()
    }
    fn register_into(reg: &mut kshana::registry::PackRegistry) {
        reg.register(crate::registry::pro_factory::<Self>(Self::KIND));
    }
}
