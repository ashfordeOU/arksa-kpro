// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The same conjunction seen by two providers, and why their answers differ.
//!
//! Events are matched on the unordered pair with TCAs within a fraction of the primary's
//! orbital period (default 6 %, the rule of the EU SST and TraCSS joint study, Borowitz et al.,
//! AMOS 2026), falling back to a fixed gate when no state is given. For each matched event the
//! result gives the differences (TCA, miss distance, reported Pc), each provider's
//! classification under its own threshold policy (a conflict when one calls it high-risk and
//! the other does not), and an explanation:
//!
//! * **one-input swaps**: Pc recomputed ([`super::pc`]) on provider A's inputs, then with one
//!   input taken from provider B (hard-body radius; covariances; states/geometry), each as a
//!   change in log10 Pc; the swaps are not additive, the remainder is reported as interaction;
//! * **input provenance**: different ephemeris sources (`EPHEMERIS_NAME`, e.g. a catalogue
//!   ephemeris against the operator's), covariance methods, orbit-determination data age, a
//!   hard-body radius missing on one side, a TCA difference beyond the objective.
//!
//! The comparison says where the answers come apart; it does not say which provider is right
//! (the joint study reaches the same conclusion: outputs converge when inputs are similar).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::pc::Encounter;
use super::scorecard::dedupe;
use super::{fmt_utc, Event};

/// A threshold policy: an event is high-risk when the Pc and/or miss-distance tests hold.
#[derive(Debug, Clone)]
pub struct Policy {
    pub name: String,
    pub pc_ge: Option<f64>,
    pub miss_le_m: Option<f64>,
    /// `true`: every stated test must hold; `false`: any one.
    pub all: bool,
}

impl Policy {
    /// An example policy (Pc ≥ 1e-4), used when none is given.
    pub fn example() -> Self {
        Self {
            name: "example: Pc >= 1e-4".into(),
            pc_ge: Some(1e-4),
            miss_le_m: None,
            all: false,
        }
    }

    pub fn high_risk(&self, pc: Option<f64>, miss_m: f64) -> Option<bool> {
        let mut tests = Vec::new();
        if let Some(t) = self.pc_ge {
            tests.push(pc.map(|p| p >= t)?);
        }
        if let Some(t) = self.miss_le_m {
            tests.push(miss_m <= t);
        }
        if tests.is_empty() {
            return None;
        }
        Some(if self.all {
            tests.iter().all(|x| *x)
        } else {
            tests.iter().any(|x| *x)
        })
    }

    pub fn describe(&self) -> Value {
        json!({"name": self.name, "pc_ge": self.pc_ge, "miss_le_m": self.miss_le_m,
               "combine": if self.all { "all" } else { "any" }})
    }
}

#[derive(Debug, Clone)]
pub struct CompareConfig {
    pub period_fraction: f64,
    pub fallback_gate_s: f64,
    pub policy_a: Policy,
    pub policy_b: Policy,
    /// Hard-body radius used for the recomputed Pc when an event states none (m).
    pub default_hbr_m: f64,
    /// TCA difference above which a note is made (s).
    pub tca_note_s: f64,
}

impl Default for CompareConfig {
    fn default() -> Self {
        Self {
            period_fraction: 0.06,
            fallback_gate_s: 300.0,
            policy_a: Policy::example(),
            policy_b: Policy::example(),
            default_hbr_m: 5.0,
            tca_note_s: 1.0,
        }
    }
}

const PC_FLOOR: f64 = 1e-30;

fn lg(p: f64) -> f64 {
    p.max(PC_FLOOR).log10()
}

/// `b` with its objects swapped so that its primary is `a`'s primary.
fn aligned(a: &Event, b: &Event) -> Event {
    if a.obj1 == b.obj1 || a.obj1 != b.obj2 {
        return b.clone();
    }
    let mut s = b.clone();
    std::mem::swap(&mut s.obj1, &mut s.obj2);
    std::mem::swap(&mut s.state1, &mut s.state2);
    std::mem::swap(&mut s.cov1_rtn, &mut s.cov2_rtn);
    std::mem::swap(&mut s.ephemeris1, &mut s.ephemeris2);
    std::mem::swap(&mut s.covariance_method1, &mut s.covariance_method2);
    std::mem::swap(&mut s.last_obs1_s, &mut s.last_obs2_s);
    s.rel_rtn_m = None; // in the other object's axes
    s
}

/// Match on pair and TCA within the period-fraction gate.
fn matches(a: &[Event], b: &[Event], cfg: &CompareConfig) -> Vec<(usize, usize, f64)> {
    let mut by: BTreeMap<(String, String), (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    for (i, e) in a.iter().enumerate() {
        by.entry(e.pair()).or_default().0.push(i);
    }
    for (j, e) in b.iter().enumerate() {
        by.entry(e.pair()).or_default().1.push(j);
    }
    let mut out = Vec::new();
    for (ai, bj) in by.values() {
        let mut cand = Vec::new();
        for &i in ai {
            for &j in bj {
                let gate = a[i]
                    .primary_period_s()
                    .or(b[j].primary_period_s())
                    .map(|p| cfg.period_fraction * p)
                    .unwrap_or(cfg.fallback_gate_s);
                let dt = (b[j].tca_s - a[i].tca_s).abs();
                if dt <= gate {
                    cand.push((dt, i, j, gate));
                }
            }
        }
        cand.sort_by(|x, y| x.0.total_cmp(&y.0));
        let (mut ui, mut uj) = (Vec::new(), Vec::new());
        for (_, i, j, g) in cand {
            if !ui.contains(&i) && !uj.contains(&j) {
                ui.push(i);
                uj.push(j);
                out.push((i, j, g));
            }
        }
    }
    out.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.cmp(&y.1)));
    out
}

/// The explanation of one matched event.
fn explain(a: &Event, b: &Event, cfg: &CompareConfig) -> Value {
    let ha = a.hbr_m.unwrap_or(cfg.default_hbr_m);
    let hb = b.hbr_m.unwrap_or(cfg.default_hbr_m);
    let mut notes: Vec<Value> = Vec::new();
    let mut note = |factor: &str, text: String| notes.push(json!({"factor": factor, "note": text}));
    let dt = b.tca_s - a.tca_s;
    if dt.abs() > cfg.tca_note_s {
        note("tca", format!("TCAs differ by {dt:+.3} s"));
    }
    match (a.hbr_m, b.hbr_m) {
        (Some(x), Some(y)) if (x - y).abs() > 1e-9 => {
            note("hbr", format!("hard-body radius differs: A {x} m, B {y} m"))
        }
        (Some(_), None) | (None, Some(_)) => note(
            "hbr",
            format!(
                "only one provider states a hard-body radius; {} m assumed for the other",
                cfg.default_hbr_m
            ),
        ),
        _ => {}
    }
    for (k, x, y) in [
        ("ephemeris_source_obj1", &a.ephemeris1, &b.ephemeris1),
        ("ephemeris_source_obj2", &a.ephemeris2, &b.ephemeris2),
        (
            "covariance_method_obj1",
            &a.covariance_method1,
            &b.covariance_method1,
        ),
        (
            "covariance_method_obj2",
            &a.covariance_method2,
            &b.covariance_method2,
        ),
    ] {
        if let (Some(x), Some(y)) = (x, y) {
            if !x.eq_ignore_ascii_case(y) {
                note(k, format!("A '{x}', B '{y}'"));
            }
        }
    }
    for (k, x, y) in [
        ("od_data_age_obj1", a.last_obs1_s, b.last_obs1_s),
        ("od_data_age_obj2", a.last_obs2_s, b.last_obs2_s),
    ] {
        if let (Some(x), Some(y)) = (x, y) {
            let d = (a.tca_s - x - (b.tca_s - y)) / 3600.0;
            if d.abs() >= 1.0 {
                note(
                    k,
                    format!("last observation to TCA differs by {d:+.1} h (A minus B)"),
                );
            }
        }
    }
    let (ea, eb) = (Encounter::from_event(a, ha), Encounter::from_event(b, hb));
    let swaps = match (ea, eb) {
        (Some(ea), Some(eb)) => {
            let (pa, pb) = (ea.pc(), eb.pc());
            match (pa, pb) {
                (Some(pa), Some(pb)) => {
                    let total = lg(pb) - lg(pa);
                    let mut s = Vec::new();
                    let mut one = |name: &str, e: Encounter| {
                        let p = e.pc();
                        s.push(json!({"input_from_b": name, "pc": p, "d_log10_pc": p.map(|p| lg(p) - lg(pa))}));
                        p.map(|p| lg(p) - lg(pa)).unwrap_or(0.0)
                    };
                    let d_h = one("hard_body_radius", Encounter { hbr_m: hb, ..ea });
                    let d_c = one(
                        "covariances",
                        Encounter {
                            c1: eb.c1,
                            c2: eb.c2,
                            ..ea
                        },
                    );
                    let d_g = one(
                        "states",
                        Encounter {
                            r1: eb.r1,
                            v1: eb.v1,
                            r2: eb.r2,
                            v2: eb.v2,
                            ..ea
                        },
                    );
                    let mut ranked = [
                        ("hard_body_radius", d_h),
                        ("covariances", d_c),
                        ("states", d_g),
                    ];
                    ranked.sort_by(|x, y| y.1.abs().total_cmp(&x.1.abs()));
                    json!({
                        "recomputed_pc_a": pa, "recomputed_pc_b": pb,
                        "d_log10_pc_total": total, "swaps": s,
                        "interaction": total - d_h - d_c - d_g,
                        "dominant_input": if total.abs() < 0.05 { "none (recomputed values agree within 0.05 in log10)" } else { ranked[0].0 },
                        "mahalanobis_a": ea.mahalanobis(), "mahalanobis_b": eb.mahalanobis(),
                    })
                }
                _ => json!({"unavailable": "a projected covariance is not positive definite"}),
            }
        }
        _ => {
            json!({"unavailable": "states and covariances of both objects are needed from both providers"})
        }
    };
    json!({"swaps": swaps, "provenance": notes})
}

/// Compare provider A with provider B: matched events with explanations, and the events only
/// one of them reports.
pub fn compare(a: &[Event], b: &[Event], cfg: &CompareConfig) -> (Value, Vec<Value>) {
    let (a, ad) = dedupe(a, 1.0);
    let (b, bd) = dedupe(b, 1.0);
    let m = matches(&a, &b, cfg);
    let mut ua = vec![false; a.len()];
    let mut ub = vec![false; b.len()];
    let mut rows = Vec::new();
    let mut events = Vec::new();
    let (mut conflicts, mut dominant): (usize, BTreeMap<String, usize>) = (0, BTreeMap::new());
    let mut dlog = Vec::new();
    for &(i, j, gate) in &m {
        ua[i] = true;
        ub[j] = true;
        let ea = &a[i];
        let eb = aligned(ea, &b[j]);
        let ca = cfg.policy_a.high_risk(ea.pc, ea.miss_m);
        let cb = cfg.policy_b.high_risk(eb.pc, eb.miss_m);
        let conflict = matches!((ca, cb), (Some(x), Some(y)) if x != y);
        if conflict {
            conflicts += 1;
        }
        let pl = match (ea.pc, eb.pc) {
            (Some(x), Some(y)) => Some(lg(y) - lg(x)),
            _ => None,
        };
        if let Some(p) = pl {
            dlog.push(p.abs());
        }
        let ex = explain(ea, &eb, cfg);
        if let Some(d) = ex["swaps"]["dominant_input"].as_str() {
            *dominant.entry(d.to_string()).or_default() += 1;
        }
        let row = json!({
            "obj1": ea.obj1, "obj2": ea.obj2, "class": ea.primary_class(),
            "tca_a": fmt_utc(ea.tca_s), "tca_b": fmt_utc(eb.tca_s), "dt_s": eb.tca_s - ea.tca_s,
            "gate_s": gate, "miss_a_m": ea.miss_m, "miss_b_m": eb.miss_m,
            "d_miss_m": eb.miss_m - ea.miss_m, "pc_a": ea.pc, "pc_b": eb.pc,
            "d_log10_pc_reported": pl, "high_risk_a": ca, "high_risk_b": cb,
            "conflict": conflict, "explanation": ex,
        });
        rows.push(json!({
            "outcome": "matched", "obj1": ea.obj1, "obj2": ea.obj2, "tca_a": fmt_utc(ea.tca_s),
            "tca_b": fmt_utc(eb.tca_s), "dt_s": eb.tca_s - ea.tca_s, "miss_a_m": ea.miss_m,
            "miss_b_m": eb.miss_m, "pc_a": ea.pc, "pc_b": eb.pc, "conflict": conflict,
            "dominant_input": ex["swaps"]["dominant_input"],
        }));
        events.push(row);
    }
    let only = |v: &[Event], used: &[bool], who: &str, rows: &mut Vec<Value>| -> Vec<Value> {
        v.iter()
            .zip(used)
            .filter(|(_, u)| !**u)
            .map(|(e, _)| {
                rows.push(json!({"outcome": format!("only_{who}"), "obj1": e.obj1, "obj2": e.obj2,
                                 "tca": fmt_utc(e.tca_s), "miss_m": e.miss_m, "pc": e.pc}));
                json!({"obj1": e.obj1, "obj2": e.obj2, "tca": fmt_utc(e.tca_s), "miss_m": e.miss_m, "pc": e.pc})
            })
            .collect()
    };
    let only_a = only(&a, &ua, "a", &mut rows);
    let only_b = only(&b, &ub, "b", &mut rows);
    let mut sorted = dlog.clone();
    sorted.sort_by(f64::total_cmp);
    let doc = json!({
        "events_a": a.len(), "events_b": b.len(),
        "duplicates_collapsed": [ad, bd],
        "matched": m.len(),
        "only_a": only_a.len(), "only_b": only_b.len(),
        "period_fraction": cfg.period_fraction,
        "policy_a": cfg.policy_a.describe(), "policy_b": cfg.policy_b.describe(),
        "default_hbr_m": cfg.default_hbr_m,
        "classification_conflicts": conflicts,
        "dominant_input_counts": dominant,
        "reported_pc_abs_log10_difference": {
            "n": sorted.len(),
            "median": super::percentile(&sorted, 50.0),
            "max": sorted.last(),
        },
        "matched_events": events,
        "only_in_a": only_a,
        "only_in_b": only_b,
        "note": "Differences are explained by inputs, not ranked by correctness: neither provider is treated as truth.",
    });
    (doc, rows)
}
