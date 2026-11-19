// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! A provider's conjunction events against a reference set, and per-event consistency checks.
//!
//! **Matching.** Events are matched on the unordered object pair and the nearest TCA within a
//! gate (default 300 s; repeated conjunctions of one pair are about half an orbit apart).
//! Reports of the same pair within one second of each other (A–B and B–A, which the
//! verification dataset's guide asks for under a spherical volume) are collapsed first and
//! counted.
//!
//! **Objectives** (defaults from Auman et al., AMOS 2025, the methodology the TraCSS
//! verification dataset points to): TCA within about 10 s for very-low relative velocity
//! (< 10 m/s), 0.01 s for low (< 50 m/s) and 0.001 s above; relative position within 1 m;
//! covariance at TCA within 0.1 Bhattacharyya distance. A missed or extra event whose miss
//! distance lies within `boundary_m` of the screening boundary is reported as a boundary case
//! (the paper attributes such differences to rounding at the threshold), separately from
//! unexplained ones. Pc is compared for information only: its value depends on the method and
//! the hard-body radius.
//!
//! **Checks** on each event (no reference needed): TCA inside the screening window; no
//! self-on-self conjunction; inside the screening volume; miss distance equal to the length of
//! the relative position; relative position and speed consistent with the two states; TCA a
//! closest approach (the range rate implied by the states, as a time offset, within the TCA
//! objective of its velocity class); both covariances positive semi-definite; Pc in [0, 1];
//! orbit-determination data no older than the age limit at the window start.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{
    dot, fmt_utc, is_psd, m3_rows, mat_vec, norm, percentile, pos_block, rtn_matrix, sfsh_regime,
    sub, Event, V3,
};

/// The screening volume the events were produced with.
#[derive(Debug, Clone, Default)]
pub enum Screening {
    /// Not checked.
    #[default]
    None,
    /// One sphere for every object (radius m).
    Sphere { radius_m: f64 },
    /// Rectangular RTN half sizes per primary (m): the explicit table first, else the Space
    /// Flight Safety Handbook regime of the primary's state ([`sfsh_regime`]).
    Sfsh { table: BTreeMap<String, V3> },
}

/// Thresholds (defaults: the published objectives; see the module notes).
#[derive(Debug, Clone)]
pub struct Objectives {
    pub vlrv_below_m_s: f64,
    pub lrv_below_m_s: f64,
    /// TCA objectives (s) for VLRV, LRV and NLRV.
    pub tca_s: [f64; 3],
    pub position_m: f64,
    pub bhattacharyya: f64,
    pub boundary_m: f64,
    pub od_age_days: f64,
    pub gate_s: f64,
}

impl Default for Objectives {
    fn default() -> Self {
        Self {
            vlrv_below_m_s: 10.0,
            lrv_below_m_s: 50.0,
            tca_s: [10.0, 0.01, 0.001],
            position_m: 1.0,
            bhattacharyya: 0.1,
            boundary_m: 10.0,
            od_age_days: 14.0,
            gate_s: 300.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Screening window `(start, end)`, calendar seconds.
    pub window: Option<(f64, f64)>,
    pub screening: Screening,
    pub objectives: Objectives,
}

const CLASSES: [&str; 3] = ["VLRV", "LRV", "NLRV"];

impl Objectives {
    fn class(&self, vrel: Option<f64>) -> Option<usize> {
        let v = vrel?;
        Some(if v < self.vlrv_below_m_s {
            0
        } else if v < self.lrv_below_m_s {
            1
        } else {
            2
        })
    }
}

fn inertial(frame: &Option<String>) -> bool {
    match frame {
        None => true,
        Some(f) => {
            let f = f.to_ascii_uppercase();
            ["EME2000", "J2000", "GCRF", "ICRF", "TEME", "MOD", "TOD"]
                .iter()
                .any(|x| f.contains(x))
        }
    }
}

/// Relative position of the secondary in the primary's RTN axes, from the states.
fn rel_from_states(e: &Event) -> Option<V3> {
    let (a, b) = (e.state1?, e.state2?);
    let (r1, v1) = ([a[0], a[1], a[2]], [a[3], a[4], a[5]]);
    let r2 = [b[0], b[1], b[2]];
    Some(mat_vec(&rtn_matrix(r1, v1), sub(r2, r1)))
}

/// The screening half sizes of the event's primary.
fn volume_of(e: &Event, s: &Screening) -> Option<V3> {
    match s {
        Screening::None => None,
        Screening::Sphere { radius_m } => Some([*radius_m; 3]),
        Screening::Sfsh { table } => table.get(&e.obj1).copied().or_else(|| {
            let a = e.state1?;
            Some(sfsh_regime([a[0], a[1], a[2]], [a[3], a[4], a[5]]).2)
        }),
    }
}

/// Distance from the screening boundary (m, positive inside), when it can be computed.
fn boundary_margin(e: &Event, s: &Screening) -> Option<f64> {
    match s {
        Screening::None => None,
        Screening::Sphere { radius_m } => Some(radius_m - e.miss_m),
        Screening::Sfsh { .. } => {
            let v = volume_of(e, s)?;
            let r = e.rel_rtn_m.or_else(|| rel_from_states(e))?;
            Some(
                (0..3)
                    .map(|i| v[i] - r[i].abs())
                    .fold(f64::INFINITY, f64::min),
            )
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Flag,
    NotEvaluated,
}

/// The ids of the per-event checks, in report order.
pub const CHECKS: &[(&str, &str)] = &[
    ("tca_in_window", "TCA inside the screening window"),
    ("not_self_on_self", "primary and secondary are different objects"),
    ("inside_screening_volume", "relative position inside the primary's screening volume"),
    ("miss_equals_relative_position", "miss distance equals the length of the relative position (within the position objective)"),
    ("relative_position_matches_states", "relative position equals the one computed from the two states (within the position objective; inertial states only)"),
    ("relative_speed_matches_states", "relative speed equals the one computed from the two states (within 0.01 m/s; inertial states only)"),
    ("tca_is_closest_approach", "range rate at TCA, as a time offset, within the TCA objective of the event's velocity class"),
    ("covariance1_psd", "object 1 covariance positive semi-definite"),
    ("covariance2_psd", "object 2 covariance positive semi-definite"),
    ("pc_in_range", "reported Pc in [0, 1]"),
    ("od_data_age", "last observation no older than the age limit at the window start"),
];

/// Run every check on one event: `(check id, status, detail)`.
pub fn check_event(e: &Event, cfg: &Config) -> Vec<(&'static str, Status, String)> {
    use Status::*;
    let o = &cfg.objectives;
    let mut out = Vec::new();
    let mut put = |id: &'static str, s: Status, d: String| out.push((id, s, d));
    match cfg.window {
        Some((a, b)) => put(
            "tca_in_window",
            if e.tca_s >= a && e.tca_s <= b {
                Pass
            } else {
                Flag
            },
            fmt_utc(e.tca_s),
        ),
        None => put("tca_in_window", NotEvaluated, "no window".into()),
    }
    put(
        "not_self_on_self",
        if e.obj1 != e.obj2 { Pass } else { Flag },
        e.obj1.clone(),
    );
    match boundary_margin(e, &cfg.screening) {
        Some(m) => put(
            "inside_screening_volume",
            if m >= -1e-6 { Pass } else { Flag },
            format!("margin {m:.3} m"),
        ),
        None => put("inside_screening_volume", NotEvaluated, String::new()),
    }
    match e.rel_rtn_m {
        Some(r) => {
            let d = (norm(r) - e.miss_m).abs();
            put(
                "miss_equals_relative_position",
                if d <= o.position_m { Pass } else { Flag },
                format!("{d:.3} m"),
            )
        }
        None => put("miss_equals_relative_position", NotEvaluated, String::new()),
    }
    let inert = inertial(&e.frame);
    match (e.rel_rtn_m, rel_from_states(e)) {
        (Some(a), Some(b)) if inert => {
            let d = norm(sub(a, b));
            put(
                "relative_position_matches_states",
                if d <= o.position_m { Pass } else { Flag },
                format!("{d:.3} m"),
            )
        }
        _ => put(
            "relative_position_matches_states",
            NotEvaluated,
            String::new(),
        ),
    }
    let from_states = match (e.state1, e.state2) {
        (Some(a), Some(b)) if inert => Some((
            sub([b[0], b[1], b[2]], [a[0], a[1], a[2]]),
            sub([b[3], b[4], b[5]], [a[3], a[4], a[5]]),
        )),
        _ => None,
    };
    match (e.vrel_m_s, from_states) {
        (Some(v), Some((_, dv))) => {
            let d = (norm(dv) - v).abs();
            put(
                "relative_speed_matches_states",
                if d <= 0.01 { Pass } else { Flag },
                format!("{d:.4} m/s"),
            )
        }
        _ => put("relative_speed_matches_states", NotEvaluated, String::new()),
    }
    match from_states {
        Some((dr, dv)) if norm(dv) > 0.0 => {
            let dt = -dot(dr, dv) / dot(dv, dv);
            let c = o.class(Some(norm(dv))).unwrap_or(2);
            put(
                "tca_is_closest_approach",
                if dt.abs() <= o.tca_s[c] { Pass } else { Flag },
                format!("{dt:.6} s ({})", CLASSES[c]),
            )
        }
        _ => put("tca_is_closest_approach", NotEvaluated, String::new()),
    }
    for (id, c) in [
        ("covariance1_psd", e.cov1_rtn),
        ("covariance2_psd", e.cov2_rtn),
    ] {
        match c {
            Some(c) => {
                let full = c[3][3] != 0.0 || c[5][5] != 0.0;
                let rows: Vec<Vec<f64>> = if full {
                    c.iter().map(|r| r.to_vec()).collect()
                } else {
                    m3_rows(&pos_block(&c))
                };
                put(id, if is_psd(&rows) { Pass } else { Flag }, String::new())
            }
            None => put(id, NotEvaluated, String::new()),
        }
    }
    match e.pc {
        Some(p) => put(
            "pc_in_range",
            if (0.0..=1.0).contains(&p) { Pass } else { Flag },
            format!("{p:e}"),
        ),
        None => put("pc_in_range", NotEvaluated, String::new()),
    }
    let start = cfg.window.map(|w| w.0).unwrap_or(e.tca_s);
    let ages: Vec<f64> = [e.last_obs1_s, e.last_obs2_s]
        .iter()
        .flatten()
        .map(|t| (start - t) / 86_400.0)
        .collect();
    if ages.is_empty() {
        put("od_data_age", NotEvaluated, String::new());
    } else {
        let worst = ages.iter().cloned().fold(f64::MIN, f64::max);
        put(
            "od_data_age",
            if worst <= o.od_age_days { Pass } else { Flag },
            format!("{worst:.2} d"),
        );
    }
    out
}

/// The checks over a set of events: per check, counts and the first flagged events.
pub fn check_all(events: &[Event], cfg: &Config) -> Value {
    let mut agg: BTreeMap<&str, (usize, usize, usize, Vec<Value>)> = BTreeMap::new();
    for e in events {
        for (id, s, d) in check_event(e, cfg) {
            let a = agg.entry(id).or_default();
            match s {
                Status::Pass => a.0 += 1,
                Status::Flag => {
                    a.1 += 1;
                    if a.3.len() < 5 {
                        a.3.push(json!({"event": e.id, "pair": [e.obj1, e.obj2], "tca": fmt_utc(e.tca_s), "detail": d}));
                    }
                }
                Status::NotEvaluated => a.2 += 1,
            }
        }
    }
    let rows: Vec<Value> = CHECKS
        .iter()
        .map(|(id, text)| {
            let (p, f, n, ex) = agg.remove(id).unwrap_or_default();
            json!({
                "check": id, "what": text, "pass": p, "flag": f, "not_evaluated": n,
                "status": if f > 0 { "FLAG" } else if p > 0 { "PASS" } else { "NOT_EVALUATED" },
                "first_flagged": ex,
            })
        })
        .collect();
    json!({"events": events.len(), "checks": rows})
}

/// Collapse reports of the same unordered pair within `tol_s` (keeping the first in input
/// order). Returns the kept events and the number collapsed.
pub fn dedupe(events: &[Event], tol_s: f64) -> (Vec<Event>, usize) {
    let mut by: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, e) in events.iter().enumerate() {
        by.entry(e.pair()).or_default().push(i);
    }
    let mut keep = vec![false; events.len()];
    let mut n = 0;
    for idx in by.values() {
        let mut v = idx.clone();
        v.sort_by(|a, b| events[*a].tca_s.total_cmp(&events[*b].tca_s));
        // Runs of reports closer than `tol_s` to the previous one; keep the first in input
        // order of each run.
        let mut run: Vec<usize> = Vec::new();
        let mut flush = |run: &mut Vec<usize>| {
            if let Some(&k) = run.iter().min() {
                keep[k] = true;
                n += run.len() - 1;
            }
            run.clear();
        };
        for &i in &v {
            if let Some(&last) = run.last() {
                if events[i].tca_s - events[last].tca_s > tol_s {
                    flush(&mut run);
                }
            }
            run.push(i);
        }
        flush(&mut run);
    }
    (
        events
            .iter()
            .zip(&keep)
            .filter(|(_, k)| **k)
            .map(|(e, _)| e.clone())
            .collect(),
        n,
    )
}

/// Match `test` to `reference` (unordered pair, nearest TCA within the gate, greedy by time
/// difference). Returns `(reference index, test index)` pairs.
pub fn match_events(reference: &[Event], test: &[Event], gate_s: f64) -> Vec<(usize, usize)> {
    let mut by: BTreeMap<(String, String), (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    for (i, e) in reference.iter().enumerate() {
        by.entry(e.pair()).or_default().0.push(i);
    }
    for (j, e) in test.iter().enumerate() {
        by.entry(e.pair()).or_default().1.push(j);
    }
    let mut out = Vec::new();
    for (rs, ts) in by.values() {
        let mut cand: Vec<(f64, usize, usize)> = Vec::new();
        for &i in rs {
            for &j in ts {
                let dt = (test[j].tca_s - reference[i].tca_s).abs();
                if dt <= gate_s {
                    cand.push((dt, i, j));
                }
            }
        }
        cand.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut ui, mut uj) = (Vec::new(), Vec::new());
        for (_, i, j) in cand {
            if !ui.contains(&i) && !uj.contains(&j) {
                ui.push(i);
                uj.push(j);
                out.push((i, j));
            }
        }
    }
    out.sort();
    out
}

fn stats(mut v: Vec<f64>) -> Value {
    v.sort_by(f64::total_cmp);
    json!({
        "n": v.len(),
        "max": v.last(),
        "p95": percentile(&v, 95.0),
        "median": percentile(&v, 50.0),
    })
}

/// The scorecard of `test` against `reference`, and its per-event rows (for the CSV).
pub fn scorecard(reference: &[Event], test: &[Event], cfg: &Config) -> (Value, Vec<Value>) {
    let o = &cfg.objectives;
    let (refs, ref_dups) = dedupe(reference, 1.0);
    let (tests, test_dups) = dedupe(test, 1.0);
    let m = match_events(&refs, &tests, o.gate_s);
    let mut matched_r = vec![false; refs.len()];
    let mut matched_t = vec![false; tests.len()];
    let mut rows = Vec::new();
    let mut tca: [Vec<f64>; 3] = Default::default();
    let mut tca_ok = [0usize; 3];
    let mut no_class = 0usize;
    let (mut dmiss, mut drel, mut bd, mut pclog) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut bd_bad = 0usize;
    for &(i, j) in &m {
        matched_r[i] = true;
        matched_t[j] = true;
        let (r, t) = (&refs[i], &tests[j]);
        let dt = t.tca_s - r.tca_s;
        let class = o.class(r.vrel().or(t.vrel()));
        let ok = class.map(|c| dt.abs() <= o.tca_s[c]);
        match class {
            Some(c) => {
                tca[c].push(dt.abs());
                if ok == Some(true) {
                    tca_ok[c] += 1;
                }
            }
            None => no_class += 1,
        }
        let dm = t.miss_m - r.miss_m;
        dmiss.push(dm.abs());
        let same_order = t.obj1 == r.obj1;
        let dr = match (r.rel_rtn_m, t.rel_rtn_m) {
            (Some(a), Some(b)) if same_order => Some(norm(sub(a, b))),
            _ => None,
        };
        if let Some(d) = dr {
            drel.push(d);
        }
        // Covariances matched by object, whatever the order.
        let (tc1, tc2) = if same_order {
            (t.cov1_rtn, t.cov2_rtn)
        } else {
            (t.cov2_rtn, t.cov1_rtn)
        };
        let b = |a: Option<super::M6>, b: Option<super::M6>| match (a, b) {
            (Some(a), Some(b)) => super::bhattacharyya_cov(&pos_block(&a), &pos_block(&b)),
            _ => None,
        };
        let (b1, b2) = (b(r.cov1_rtn, tc1), b(r.cov2_rtn, tc2));
        for x in [b1, b2].into_iter().flatten() {
            if x > o.bhattacharyya {
                bd_bad += 1;
            }
            bd.push(x);
        }
        let pl = match (r.pc, t.pc) {
            (Some(a), Some(b)) if a > 0.0 && b > 0.0 => Some((b / a).log10()),
            _ => None,
        };
        if let Some(p) = pl {
            pclog.push(p.abs());
        }
        rows.push(json!({
            "outcome": "matched", "obj1": r.obj1, "obj2": r.obj2, "class": r.primary_class(),
            "tca_reference": fmt_utc(r.tca_s), "tca_test": fmt_utc(t.tca_s), "dt_s": dt,
            "vrel_m_s": r.vrel().or(t.vrel()), "velocity_class": class.map(|c| CLASSES[c]),
            "tca_within_objective": ok, "miss_reference_m": r.miss_m, "miss_test_m": t.miss_m,
            "d_miss_m": dm, "d_rel_position_m": dr, "bhattacharyya_obj1": b1,
            "bhattacharyya_obj2": b2, "pc_reference": r.pc, "pc_test": t.pc,
            "log10_pc_ratio": pl, "reason": Value::Null,
        }));
    }
    // Missed reference events, with the likely reason.
    let mut pair_tests: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    for t in &tests {
        pair_tests.entry(t.pair()).or_default().push(t.tca_s);
    }
    let near = |e: &Event| {
        boundary_margin(e, &cfg.screening)
            .map(|mg| mg.abs() <= o.boundary_m)
            .unwrap_or(false)
    };
    let (mut missed, mut missed_boundary) = (0usize, 0usize);
    let mut class_tot: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (i, r) in refs.iter().enumerate() {
        let c = class_tot.entry(r.primary_class()).or_default();
        c.0 += 1;
        if matched_r[i] {
            c.1 += 1;
            continue;
        }
        missed += 1;
        let reason = if near(r) {
            missed_boundary += 1;
            "near the screening boundary".to_string()
        } else if let Some(ts) = pair_tests.get(&r.pair()) {
            let d = ts
                .iter()
                .map(|t| t - r.tca_s)
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
                .unwrap_or(f64::NAN);
            format!("same pair reported at a different TCA ({d:+.3} s)")
        } else {
            "not reported".to_string()
        };
        rows.push(json!({
            "outcome": "missed", "obj1": r.obj1, "obj2": r.obj2, "class": r.primary_class(),
            "tca_reference": fmt_utc(r.tca_s), "miss_reference_m": r.miss_m,
            "vrel_m_s": r.vrel(), "pc_reference": r.pc, "reason": reason,
        }));
    }
    let (mut extra, mut extra_boundary) = (0usize, 0usize);
    for (j, t) in tests.iter().enumerate() {
        if matched_t[j] {
            continue;
        }
        extra += 1;
        let outside_window = cfg
            .window
            .map(|(a, b)| t.tca_s < a || t.tca_s > b)
            .unwrap_or(false);
        let reason = if t.obj1 == t.obj2 {
            "self-on-self".to_string()
        } else if outside_window {
            "TCA outside the screening window".to_string()
        } else if near(t) {
            extra_boundary += 1;
            "near the screening boundary".to_string()
        } else if boundary_margin(t, &cfg.screening)
            .map(|m| m < 0.0)
            .unwrap_or(false)
        {
            "outside the screening volume".to_string()
        } else {
            "not in the reference".to_string()
        };
        rows.push(json!({
            "outcome": "extra", "obj1": t.obj1, "obj2": t.obj2, "class": t.primary_class(),
            "tca_test": fmt_utc(t.tca_s), "miss_test_m": t.miss_m, "vrel_m_s": t.vrel(),
            "pc_test": t.pc, "reason": reason,
        }));
    }
    let status = |ok: bool, n: usize| {
        if n == 0 {
            "NOT_EVALUATED"
        } else if ok {
            "PASS"
        } else {
            "FLAG"
        }
    };
    let nm = m.len();
    let mut objectives = vec![
        json!({
            "objective": "event_recall",
            "what": "every reference event found (boundary cases reported apart)",
            "value": if refs.is_empty() { None } else { Some(nm as f64 / refs.len() as f64) },
            "missed": missed, "missed_near_boundary": missed_boundary,
            "status": status(missed == missed_boundary, refs.len()),
        }),
        json!({
            "objective": "event_precision",
            "what": "no events beyond the reference (boundary cases reported apart)",
            "value": if tests.is_empty() { None } else { Some(nm as f64 / tests.len() as f64) },
            "extra": extra, "extra_near_boundary": extra_boundary,
            "status": status(extra == extra_boundary, tests.len()),
        }),
    ];
    for c in 0..3 {
        let n = tca[c].len();
        objectives.push(json!({
            "objective": format!("tca_{}", CLASSES[c].to_ascii_lowercase()),
            "what": format!("|ΔTCA| ≤ {} s for {} events", o.tca_s[c], CLASSES[c]),
            "within": tca_ok[c], "stats_s": stats(tca[c].clone()),
            "status": status(tca_ok[c] == n, n),
        }));
    }
    objectives.push(json!({
        "objective": "miss_distance",
        "what": format!("|Δ miss distance| ≤ {} m", o.position_m),
        "stats_m": stats(dmiss.clone()),
        "status": status(dmiss.iter().all(|d| *d <= o.position_m), dmiss.len()),
    }));
    objectives.push(json!({
        "objective": "relative_position",
        "what": format!("|Δ relative position (RTN)| ≤ {} m", o.position_m),
        "stats_m": stats(drel.clone()),
        "status": status(drel.iter().all(|d| *d <= o.position_m), drel.len()),
    }));
    objectives.push(json!({
        "objective": "covariance_at_tca",
        "what": format!("Bhattacharyya distance of each object's position covariance ≤ {}", o.bhattacharyya),
        "stats": stats(bd.clone()), "above": bd_bad,
        "status": status(bd_bad == 0, bd.len()),
    }));
    objectives.push(json!({
        "objective": "pc_information_only",
        "what": "|log10 Pc ratio| where both report a Pc; not graded (method and hard-body radius differ)",
        "stats": stats(pclog), "status": "INFO",
    }));
    let flagged = objectives.iter().filter(|x| x["status"] == "FLAG").count();
    let by_class: Vec<Value> = class_tot
        .iter()
        .map(|(k, (n, f))| json!({"class": k, "reference_events": n, "matched": f, "recall": *f as f64 / *n as f64}))
        .collect();
    let doc = json!({
        "reference_events": refs.len(),
        "reference_duplicates_collapsed": ref_dups,
        "test_events": tests.len(),
        "test_duplicates_collapsed": test_dups,
        "matched": nm,
        "matched_without_velocity_class": no_class,
        "match_gate_s": o.gate_s,
        "window": cfg.window.map(|(a, b)| json!([fmt_utc(a), fmt_utc(b)])),
        "objectives": objectives,
        "objectives_flagged": flagged,
        "verdict": if flagged == 0 { "all evaluated objectives met (agreement with the reference only)" } else { "objectives flagged: see the list" },
        "by_primary_class": by_class,
        "checks_on_test_events": check_all(&tests, cfg),
    });
    (doc, rows)
}
