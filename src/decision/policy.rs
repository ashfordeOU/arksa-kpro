// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The threshold-policy comparator: what a set of conjunction events would have cost and
//! left behind under different manoeuvre thresholds (for example Pc 1e-4 against 3e-7).
//!
//! For each threshold `t`, every event with `pc >= t` is manoeuvred and its probability
//! becomes its stated post-manoeuvre value (`post_pc` in the events file, else the
//! `default_post_pc` of the study, 0 unless set); every other event keeps its `pc`. The
//! sum of the probabilities stands in for the expected number of collisions, which holds
//! for small, independent probabilities. Reported per threshold: manoeuvres (and per year
//! when the period is given), the risk accepted, and the reduction relative to no
//! manoeuvre at all.
//!
//! The reduction is set beside ESA ESSB-ST-U-007 5.3.3.3 b, which asks for the lower of
//! 10^-4 and the threshold that reduces the annual collision probability by at least 90%.
//! That clause's method also counts debris nobody can track; this comparator sees only
//! the listed events, so its 90% figure is an upper bound on what the threshold achieves
//! and is labelled MODELLED.

use serde::Serialize;

use super::time::parse_utc;
use crate::mbse::evidence::sha256_hex;
use kshana::api::KshanaError;

/// Identifies `thresholds.json`.
pub const SCHEMA: &str = "arksa-ca-threshold-comparison/1";
/// The honesty label.
pub const LABEL: &str = "MODELLED - a comparison of manoeuvre thresholds over the listed conjunction events, using each event's probability of collision as the provider gave it. Untracked debris is not counted, so the risk reduction is an upper bound.";
/// The relative reduction ESA's clause asks for.
pub const ESA_REDUCTION: f64 = 0.9;
/// ESA's absolute upper bound per conjunction.
pub const ESA_UPPER_BOUND: f64 = 1e-4;

fn err(msg: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(msg.into())
}

/// One event of the events file.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Event {
    pub event_id: String,
    pub tca_utc: String,
    pub pc: f64,
    pub post_pc: Option<f64>,
}

/// Parse the events CSV: header `event_id,tca_utc,pc` with an optional fourth column
/// `post_pc`. Blank lines and lines starting with `#` are skipped.
pub fn parse_events(text: &str) -> Result<Vec<Event>, KshanaError> {
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let (_, header) = lines
        .next()
        .ok_or_else(|| err("the events file is empty"))?;
    let cols: Vec<String> = header.split(',').map(|c| c.trim().to_lowercase()).collect();
    let with_post =
        match cols
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["event_id", "tca_utc", "pc"] => false,
            ["event_id", "tca_utc", "pc", "post_pc"] => true,
            _ => return Err(err(
                "the events file header must be event_id,tca_utc,pc or event_id,tca_utc,pc,post_pc",
            )),
        };
    let mut out: Vec<Event> = Vec::new();
    for (i, l) in lines {
        let n = i + 1;
        let f: Vec<&str> = l.split(',').map(str::trim).collect();
        if f.len() != cols.len() {
            return Err(err(format!(
                "line {n}: {} fields, the header has {}",
                f.len(),
                cols.len()
            )));
        }
        parse_utc(f[1]).map_err(|m| err(format!("line {n}: {m}")))?;
        let p = |s: &str, what: &str| -> Result<f64, KshanaError> {
            let x: f64 = s
                .parse()
                .map_err(|_| err(format!("line {n}: {what} '{s}' is not a number")))?;
            if x.is_finite() && (0.0..=1.0).contains(&x) {
                Ok(x)
            } else {
                Err(err(format!("line {n}: {what} {x} is not a probability")))
            }
        };
        if f[0].is_empty() || out.iter().any(|e| e.event_id == f[0]) {
            return Err(err(format!(
                "line {n}: event_id '{}' is empty or repeated",
                f[0]
            )));
        }
        let post_pc = if with_post && !f[3].is_empty() {
            Some(p(f[3], "post_pc")?)
        } else {
            None
        };
        out.push(Event {
            event_id: f[0].to_string(),
            tca_utc: f[1].to_string(),
            pc: p(f[2], "pc")?,
            post_pc,
        });
    }
    if out.is_empty() {
        return Err(err("the events file lists no event"));
    }
    Ok(out)
}

/// The study settings.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Study {
    pub thresholds: Vec<f64>,
    /// The period the events cover (days), for manoeuvres per year.
    pub period_days: Option<f64>,
    /// Post-manoeuvre probability for events that state none.
    pub default_post_pc: f64,
}

/// One threshold's outcome.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Row {
    pub threshold: f64,
    pub manoeuvres: usize,
    pub manoeuvres_per_year: Option<f64>,
    /// Sum of probabilities left after the manoeuvres.
    pub risk_accepted: f64,
    /// `1 - risk_accepted / risk_without_manoeuvres` (0 when there is no risk at all).
    pub reduction: f64,
    pub meets_esa_reduction: bool,
    pub within_esa_upper_bound: bool,
}

/// The comparison body (what `comparison_sha256` covers).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ComparisonBody {
    pub schema: String,
    pub events_sha256: String,
    pub events: usize,
    pub study: Study,
    pub risk_without_manoeuvres: f64,
    pub rows: Vec<Row>,
    /// The highest threshold tried that is at most 1e-4 and reaches the 90% reduction.
    pub esa_reference_threshold: Option<f64>,
    pub reading: String,
    pub rule_refs: Vec<String>,
}

/// A threshold comparison.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Comparison {
    pub label: &'static str,
    pub body: ComparisonBody,
    pub comparison_sha256: String,
}

/// Six significant digits, so documents do not move with the last floating-point bits.
fn r6(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let d = 5 - x.abs().log10().floor() as i32;
    let m = 10f64.powi(d);
    (x * m).round() / m
}

/// Compare the thresholds of `study` over `events` (whose file hashes to `events_sha256`).
pub fn compare(
    events: &[Event],
    study: &Study,
    events_sha256: &str,
) -> Result<Comparison, KshanaError> {
    if study.thresholds.is_empty() {
        return Err(err("the study names no threshold"));
    }
    for t in &study.thresholds {
        if !(t.is_finite() && *t > 0.0 && *t <= 1.0) {
            return Err(err(format!("threshold {t} is not a probability above 0")));
        }
    }
    if !(study.default_post_pc.is_finite() && (0.0..=1.0).contains(&study.default_post_pc)) {
        return Err(err("default_post_pc is not a probability"));
    }
    if let Some(p) = study.period_days {
        if !(p.is_finite() && p > 0.0) {
            return Err(err("period_days must be positive"));
        }
    }
    let mut thresholds = study.thresholds.clone();
    thresholds.sort_by(|a, b| b.total_cmp(a));
    thresholds.dedup();
    let total: f64 = events.iter().map(|e| e.pc).sum();
    let rows: Vec<Row> = thresholds
        .iter()
        .map(|&t| {
            let mut manoeuvres = 0;
            let mut accepted = 0.0;
            for e in events {
                if e.pc >= t {
                    manoeuvres += 1;
                    accepted += e.post_pc.unwrap_or(study.default_post_pc).min(e.pc);
                } else {
                    accepted += e.pc;
                }
            }
            let reduction = if total > 0.0 {
                1.0 - accepted / total
            } else {
                0.0
            };
            Row {
                threshold: t,
                manoeuvres,
                manoeuvres_per_year: study
                    .period_days
                    .map(|d| r6(manoeuvres as f64 * 365.25 / d)),
                risk_accepted: r6(accepted),
                reduction: r6(reduction),
                meets_esa_reduction: reduction >= ESA_REDUCTION,
                within_esa_upper_bound: t <= ESA_UPPER_BOUND,
            }
        })
        .collect();
    let esa = rows
        .iter()
        .find(|r| r.within_esa_upper_bound && r.meets_esa_reduction)
        .map(|r| r.threshold);
    let reading = match (esa, rows.first(), rows.last()) {
        (_, Some(hi), Some(lo)) if hi.threshold != lo.threshold => {
            let mut s = format!(
                "Moving the threshold from {:.1e} to {:.1e} changes the manoeuvres from {} to {} and the risk accepted from {:.3e} to {:.3e}.",
                hi.threshold, lo.threshold, hi.manoeuvres, lo.manoeuvres, hi.risk_accepted, lo.risk_accepted
            );
            match esa {
                Some(t) => s.push_str(&format!(" The highest threshold tried that is at most 1e-4 and removes at least 90% of the listed risk is {t:.1e}.")),
                None => s.push_str(" No threshold tried that is at most 1e-4 removes 90% of the listed risk."),
            }
            s
        }
        _ => "One threshold only: nothing to compare.".into(),
    };
    let body = ComparisonBody {
        schema: SCHEMA.into(),
        events_sha256: events_sha256.into(),
        events: events.len(),
        study: Study {
            thresholds,
            ..study.clone()
        },
        risk_without_manoeuvres: r6(total),
        rows,
        esa_reference_threshold: esa,
        reading,
        rule_refs: vec!["ESA-5.3.3.3-a".into(), "ESA-5.3.3.3-b".into()],
    };
    let comparison_sha256 = sha256_hex(crate::mission::compose::canonical_json(&body)?.as_bytes());
    Ok(Comparison {
        label: LABEL,
        body,
        comparison_sha256,
    })
}

impl Comparison {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("the comparison serialises")
    }

    /// The table as CSV.
    pub fn to_csv(&self) -> String {
        let mut s = String::from(
            "threshold,manoeuvres,manoeuvres_per_year,risk_accepted,reduction,meets_esa_90pct,within_1e-4\n",
        );
        for r in &self.body.rows {
            s.push_str(&format!(
                "{:e},{},{},{:e},{},{},{}\n",
                r.threshold,
                r.manoeuvres,
                r.manoeuvres_per_year
                    .map(|x| x.to_string())
                    .unwrap_or_default(),
                r.risk_accepted,
                r.reduction,
                r.meets_esa_reduction,
                r.within_esa_upper_bound
            ));
        }
        s
    }
}

/// Parse a comma-separated threshold list such as `1e-4,1e-5,3e-7`.
pub fn parse_thresholds(s: &str) -> Result<Vec<f64>, KshanaError> {
    s.split(',')
        .filter(|x| !x.trim().is_empty())
        .map(|x| {
            x.trim()
                .parse::<f64>()
                .map_err(|_| err(format!("threshold '{x}' is not a number")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lower_threshold_manoeuvres_more_and_accepts_less() {
        let csv = "event_id,tca_utc,pc\nA,2026-01-01T00:00:00Z,2e-4\nB,2026-01-02T00:00:00Z,5e-5\nC,2026-01-03T00:00:00Z,1e-6\nD,2026-01-04T00:00:00Z,1e-8\n";
        let ev = parse_events(csv).unwrap();
        let c = compare(
            &ev,
            &Study {
                thresholds: vec![3e-7, 1e-4, 1e-5],
                period_days: Some(365.25),
                default_post_pc: 0.0,
            },
            "x",
        )
        .unwrap();
        let rows = &c.body.rows;
        assert_eq!(
            rows.iter().map(|r| r.threshold).collect::<Vec<_>>(),
            vec![1e-4, 1e-5, 3e-7]
        );
        assert_eq!(
            rows.iter().map(|r| r.manoeuvres).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(rows[2].manoeuvres_per_year, Some(3.0));
        let total = 2e-4 + 5e-5 + 1e-6 + 1e-8;
        assert!((rows[0].risk_accepted - (total - 2e-4)).abs() < 1e-12);
        assert!(rows[0].reduction < 0.9 && rows[1].reduction > 0.9);
        assert_eq!(c.body.esa_reference_threshold, Some(1e-5));
    }
}
