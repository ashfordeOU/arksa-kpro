// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! A scorecard for telemetry anomaly detectors on public benchmarks, with pre-registered
//! bars and an operator false-alarm budget.
//!
//! Two benchmark layouts are read in their published form:
//!
//! * **`esa-ad`**: the ESA Anomalies Dataset of the ESA Anomaly Detection Benchmark
//!   (ESA-ADB; Kotowski et al., arXiv:2406.17826; data doi:10.5281/zenodo.12528696, licence
//!   CC BY 3.0 IGO). `labels.csv` (`ID,Channel,StartTime,EndTime`, one row per annotated
//!   channel interval) and `anomaly_types.csv` (`ID,Class,Subclass,Category,Dimensionality,
//!   Locality,Length`; categories `Anomaly`, `Rare Event`, `Communication Gap`). A detector
//!   gives alarms as `alarm_id,channel,start_utc,end_utc` (rows sharing an `alarm_id` are
//!   one multi-channel alarm; without the column each row is its own alarm).
//! * **`opssat-ad`**: the OPSSAT-AD dataset (Ruszczak et al., Scientific Data 2025;
//!   doi:10.5281/zenodo.15108715; record licence CC BY 4.0, MIT licence file inside).
//!   `dataset.csv` (`segment,anomaly,train,channel,...`, one row per telemetry segment). A
//!   detector gives `segment,predicted[,score]`.
//!
//! The interval metrics follow the definitions published with ESA-ADB (event-wise recall,
//! event-wise precision corrected by the time-wise true-negative rate, F0.5,
//! channel-aware precision and recall, alarming precision, detection latency), written
//! here independently from the paper's text. They are not the ESA-ADB TimeEval code, so a
//! figure here can differ from a figure in the ESA-ADB paper for the same detector; the
//! exact definitions used are in docs/OPS-ASSURANCE.md. No result here is an ESA result
//! or endorsed by ESA, the dataset authors or the benchmark's maintainers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::csv::Table;
use super::{body_sha256, err, fmt_utc_ms, merge, percentile, read_rel, round, union_len, utc};
use crate::mbse::evidence::sha256_hex;
use kshana::api::KshanaError;

/// Identifies `scorecard.json`.
pub const SCHEMA: &str = "arksa-ops-ad-scorecard/1";

/// The honesty label every scorecard carries.
pub const LABEL: &str = "Detector outputs scored against the benchmark's published annotations with metrics reimplemented independently from the ESA-ADB definitions; figures can differ from the ESA-ADB TimeEval pipeline. Not an ESA result and not endorsed by ESA, the dataset authors or the benchmark's maintainers. The annotations are the dataset authors'.";

// ------------------------------------------------------------------ spec

/// Whether the scored data are real telemetry (a public benchmark or an operator's own)
/// or synthetic. Only real data give the evidence class MEASURED.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DataOrigin {
    PublicBenchmark,
    Operator,
    Synthetic,
}

/// How annotated rare nominal events are treated (ESA-AD category `Rare Event`).
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RareEvents {
    /// Neither counted as anomalies nor as false alarms when detected.
    #[default]
    Ignore,
    /// Counted as anomalies to detect.
    Count,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Benchmark {
    /// `esa-ad` or `opssat-ad`.
    pub format: String,
    pub name: String,
    pub data_origin: DataOrigin,
    #[serde(default)]
    pub source_doi: Option<String>,
    #[serde(default)]
    pub licence: Option<String>,
    /// esa-ad: `labels.csv`.
    #[serde(default)]
    pub labels: Option<String>,
    /// esa-ad: `anomaly_types.csv`.
    #[serde(default)]
    pub anomaly_types: Option<String>,
    /// esa-ad: the evaluation window (the detector's test period).
    #[serde(default)]
    pub window_start_utc: Option<String>,
    #[serde(default)]
    pub window_end_utc: Option<String>,
    /// esa-ad: channels in scope (empty: every channel in the labels).
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub rare_events: RareEvents,
    /// opssat-ad: `dataset.csv`.
    #[serde(default)]
    pub dataset: Option<String>,
    /// opssat-ad: `test` (rows with train = 0, the default) or `all`.
    #[serde(default)]
    pub split: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Detector {
    pub name: String,
    pub version: String,
    /// The alarms (esa-ad) or predictions (opssat-ad).
    pub outputs: String,
    /// The pre-registered bars file.
    #[serde(default)]
    pub bars: Option<String>,
    /// The bars file's SHA-256 as published before scoring; when given it must match.
    #[serde(default)]
    pub bars_sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub benchmark: Benchmark,
    pub detector: Detector,
}

/// One pre-registered bar on a metric.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Bar {
    pub metric: String,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub why: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Bars {
    pub registered_utc: String,
    #[serde(default)]
    pub registered_by: Option<String>,
    #[serde(default)]
    pub statement: Option<String>,
    #[serde(rename = "bar")]
    pub bars: Vec<Bar>,
}

impl Bars {
    pub fn parse(text: &str) -> Result<Bars, KshanaError> {
        let b: Bars = toml::from_str(text).map_err(|e| err(format!("invalid bars: {e}")))?;
        utc("bars registered_utc", &b.registered_utc)?;
        if b.bars.is_empty() {
            return Err(err("the bars file has no [[bar]]"));
        }
        for x in &b.bars {
            if x.min.is_none() == x.max.is_none() {
                return Err(err(format!(
                    "bar on {}: give exactly one of min or max",
                    x.metric
                )));
            }
        }
        Ok(b)
    }
}

// ------------------------------------------------------------------ results

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BarResult {
    pub metric: String,
    pub bar: String,
    pub value: Option<f64>,
    /// `PASS`, `FAIL` or `NOT APPLICABLE` (the metric is not produced for this benchmark).
    pub verdict: String,
    pub why: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InputHash {
    pub role: String,
    pub file: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScorecardBody {
    pub schema: String,
    pub benchmark: Benchmark,
    pub detector_name: String,
    pub detector_version: String,
    pub evidence_class: String,
    pub inputs: Vec<InputHash>,
    /// Metric name to value (`null` when undefined, e.g. no detected event).
    pub metrics: BTreeMap<String, Option<f64>>,
    /// Counts behind the metrics.
    pub counts: BTreeMap<String, u64>,
    /// esa-ad: recall per anomaly type column value (e.g. `Locality=Local`).
    pub recall_by_type: BTreeMap<String, (u64, u64)>,
    /// esa-ad: the events missed.
    pub missed_events: Vec<String>,
    pub bars_registered_utc: Option<String>,
    pub bars: Vec<BarResult>,
    /// `PASS` (every bar passes), `FAIL`, or `NO BARS`.
    pub verdict: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scorecard {
    pub label: String,
    pub body: ScorecardBody,
    pub scorecard_sha256: String,
}

fn f_beta(p: f64, r: f64, beta: f64) -> Option<f64> {
    let b2 = beta * beta;
    let d = b2 * p + r;
    (d > 0.0).then(|| (1.0 + b2) * p * r / d)
}

fn ratio(a: f64, b: f64) -> Option<f64> {
    (b > 0.0).then(|| a / b)
}

// ------------------------------------------------------------------ esa-ad

/// Interval benchmark inputs, parsed.
#[derive(Debug, Clone)]
pub struct IntervalTruth {
    /// Event id to its per-channel intervals.
    pub rows: BTreeMap<String, Vec<(String, f64, f64)>>,
    /// Event id to its type columns (`Category`, `Dimensionality`, ...).
    pub types: BTreeMap<String, BTreeMap<String, String>>,
}

pub fn parse_esa_ad(labels: &str, types: &str) -> Result<IntervalTruth, KshanaError> {
    let t = Table::parse("labels.csv", labels)?;
    let (ci, cc, cs, ce) = (
        t.col("ID")?,
        t.col("Channel")?,
        t.col("StartTime")?,
        t.col("EndTime")?,
    );
    let mut rows: BTreeMap<String, Vec<(String, f64, f64)>> = BTreeMap::new();
    for (line, r) in &t.rows {
        let (a, b) = (t.time(*line, r, cs)?, t.time(*line, r, ce)?);
        if b < a {
            return Err(err(format!(
                "labels.csv line {line}: EndTime before StartTime"
            )));
        }
        rows.entry(Table::get(r, ci).to_string())
            .or_default()
            .push((Table::get(r, cc).to_string(), a, b));
    }
    let ty = Table::parse("anomaly_types.csv", types)?;
    let tid = ty.col("ID")?;
    ty.col("Category")?;
    let mut types_map = BTreeMap::new();
    for (_, r) in &ty.rows {
        let m: BTreeMap<String, String> = ty
            .header
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != tid)
            .map(|(i, h)| (h.clone(), Table::get(r, i).to_string()))
            .collect();
        types_map.insert(Table::get(r, tid).to_string(), m);
    }
    for id in rows.keys() {
        if !types_map.contains_key(id) {
            return Err(err(format!(
                "labels.csv event {id} has no row in anomaly_types.csv"
            )));
        }
    }
    Ok(IntervalTruth {
        rows,
        types: types_map,
    })
}

/// One alarm: channels and extent.
#[derive(Debug, Clone)]
pub struct Alarm {
    pub id: String,
    pub channels: BTreeSet<String>,
    pub start: f64,
    pub end: f64,
}

pub fn parse_alarms(text: &str) -> Result<Vec<Alarm>, KshanaError> {
    let t = Table::parse("alarms", text)?;
    let (cc, cs, ce) = (t.col("channel")?, t.col("start_utc")?, t.col("end_utc")?);
    let ca = t.opt_col("alarm_id");
    let mut by: BTreeMap<String, Alarm> = BTreeMap::new();
    for (line, r) in &t.rows {
        let (a, b) = (t.time(*line, r, cs)?, t.time(*line, r, ce)?);
        if b < a {
            return Err(err(format!("alarms line {line}: end_utc before start_utc")));
        }
        let id = match ca {
            Some(c) if !Table::get(r, c).is_empty() => Table::get(r, c).to_string(),
            _ => format!("line-{line}"),
        };
        let e = by.entry(id.clone()).or_insert(Alarm {
            id,
            channels: BTreeSet::new(),
            start: a,
            end: b,
        });
        e.channels.insert(Table::get(r, cc).to_string());
        e.start = e.start.min(a);
        e.end = e.end.max(b);
    }
    Ok(by.into_values().collect())
}

struct Event {
    id: String,
    channels: BTreeSet<String>,
    start: f64,
    end: f64,
}

fn overlaps(a0: f64, a1: f64, b0: f64, b1: f64) -> bool {
    a0 <= b1 && b0 <= a1
}

/// Score alarms against interval truth over `[w0, w1]`.
pub fn score_intervals(
    truth: &IntervalTruth,
    alarms: &[Alarm],
    w0: f64,
    w1: f64,
    scope: &[String],
    rare: RareEvents,
) -> Result<IntervalScore, KshanaError> {
    if w1 <= w0 {
        return Err(err("the evaluation window ends before it starts"));
    }
    let in_scope = |c: &str| scope.is_empty() || scope.iter().any(|s| s == c);
    let mut counted = Vec::new();
    let mut ignored = Vec::new();
    for (id, rows) in &truth.rows {
        let cat = truth.types[id]
            .get("Category")
            .map(String::as_str)
            .unwrap_or("");
        let rs: Vec<&(String, f64, f64)> = rows.iter().filter(|r| in_scope(&r.0)).collect();
        if rs.is_empty() {
            continue;
        }
        let start = rs.iter().map(|r| r.1).fold(f64::INFINITY, f64::min).max(w0);
        let end = rs
            .iter()
            .map(|r| r.2)
            .fold(f64::NEG_INFINITY, f64::max)
            .min(w1);
        if end < start {
            continue;
        }
        let ev = Event {
            id: id.clone(),
            channels: rs.iter().map(|r| r.0.clone()).collect(),
            start,
            end,
        };
        match (cat, rare) {
            ("Anomaly", _) | ("Rare Event", RareEvents::Count) => counted.push(ev),
            ("Rare Event", RareEvents::Ignore) | ("Communication Gap", _) => ignored.push(ev),
            (other, _) => {
                return Err(err(format!(
                    "event {id}: category '{other}' is not Anomaly, Rare Event or Communication Gap"
                )))
            }
        }
    }
    let alarms: Vec<Alarm> = alarms
        .iter()
        .filter_map(|a| {
            let ch: BTreeSet<String> = a.channels.iter().filter(|c| in_scope(c)).cloned().collect();
            (!ch.is_empty() && overlaps(a.start, a.end, w0, w1)).then(|| Alarm {
                id: a.id.clone(),
                channels: ch,
                start: a.start.max(w0),
                end: a.end.min(w1),
            })
        })
        .collect();

    let mut tp_events = 0u64;
    let mut latencies = Vec::new();
    let mut ch_p = Vec::new();
    let mut ch_r = Vec::new();
    let mut missed = Vec::new();
    let mut by_type: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for ev in &counted {
        let hits: Vec<&Alarm> = alarms
            .iter()
            .filter(|a| overlaps(a.start, a.end, ev.start, ev.end))
            .collect();
        let detected = !hits.is_empty();
        for (k, v) in &truth.types[&ev.id] {
            if matches!(
                k.as_str(),
                "Dimensionality" | "Locality" | "Length" | "Category"
            ) {
                let e = by_type.entry(format!("{k}={v}")).or_insert((0, 0));
                e.1 += 1;
                e.0 += u64::from(detected);
            }
        }
        if !detected {
            missed.push(ev.id.clone());
            continue;
        }
        tp_events += 1;
        let first = hits.iter().map(|a| a.start).fold(f64::INFINITY, f64::min);
        latencies.push((first - ev.start).max(0.0));
        // Channels flagged by the alarms on this event. A flagged channel that belongs to
        // another annotated event the same alarms overlap is attributed to that event, so
        // concurrent events do not count against each other.
        let flagged: BTreeSet<&String> = hits.iter().flat_map(|a| a.channels.iter()).collect();
        let concurrent: BTreeSet<&String> = counted
            .iter()
            .chain(ignored.iter())
            .filter(|o| {
                o.id != ev.id
                    && hits
                        .iter()
                        .any(|a| overlaps(a.start, a.end, o.start, o.end))
            })
            .flat_map(|o| o.channels.iter())
            .collect();
        let inter = flagged.iter().filter(|c| ev.channels.contains(**c)).count() as f64;
        let judged = flagged
            .iter()
            .filter(|c| ev.channels.contains(**c) || !concurrent.contains(**c))
            .count() as f64;
        ch_p.push(inter / judged);
        ch_r.push(inter / ev.channels.len() as f64);
    }
    let mut fp_alarms = 0u64;
    let mut tp_alarms = 0u64;
    let mut ignored_alarms = 0u64;
    for a in &alarms {
        if counted
            .iter()
            .any(|e| overlaps(a.start, a.end, e.start, e.end))
        {
            tp_alarms += 1;
        } else if ignored
            .iter()
            .any(|e| overlaps(a.start, a.end, e.start, e.end))
        {
            ignored_alarms += 1;
        } else {
            fp_alarms += 1;
        }
    }
    // Time-wise true-negative rate over nominal time (the window minus every annotated
    // event, counted or ignored).
    let annotated: Vec<(f64, f64)> = counted
        .iter()
        .chain(ignored.iter())
        .map(|e| (e.start, e.end))
        .collect();
    let annotated = merge(&annotated);
    let nominal = (w1 - w0) - union_len(&annotated);
    let alarmed: Vec<(f64, f64)> = alarms.iter().map(|a| (a.start, a.end)).collect();
    let mut alarmed_nominal = 0.0;
    for (a0, a1) in merge(&alarmed) {
        alarmed_nominal += super::subtract((a0, a1), &annotated)
            .iter()
            .map(|(x, y)| y - x)
            .sum::<f64>();
    }
    let n = counted.len() as f64;
    let tp = tp_events as f64;
    Ok(IntervalScore {
        events: counted.len() as u64,
        ignored_events: ignored.len() as u64,
        tp_events,
        alarms: alarms.len() as u64,
        tp_alarms,
        fp_alarms,
        ignored_alarms,
        nominal_s: nominal,
        alarmed_nominal_s: alarmed_nominal,
        recall: ratio(tp, n),
        precision: ratio(tp, tp + fp_alarms as f64),
        tnr: ratio(nominal - alarmed_nominal, nominal),
        channel_precision: ratio(ch_p.iter().sum(), ch_p.len() as f64),
        channel_recall: ratio(ch_r.iter().sum(), ch_r.len() as f64),
        latencies,
        missed,
        by_type,
    })
}

/// The interval score's figures.
#[derive(Debug, Clone)]
pub struct IntervalScore {
    pub events: u64,
    pub ignored_events: u64,
    pub tp_events: u64,
    pub alarms: u64,
    pub tp_alarms: u64,
    pub fp_alarms: u64,
    pub ignored_alarms: u64,
    pub nominal_s: f64,
    pub alarmed_nominal_s: f64,
    pub recall: Option<f64>,
    pub precision: Option<f64>,
    pub tnr: Option<f64>,
    pub channel_precision: Option<f64>,
    pub channel_recall: Option<f64>,
    pub latencies: Vec<f64>,
    pub missed: Vec<String>,
    pub by_type: BTreeMap<String, (u64, u64)>,
}

impl IntervalScore {
    /// The named metrics.
    pub fn metrics(&self) -> BTreeMap<String, Option<f64>> {
        let corrected = match (self.precision, self.tnr) {
            (Some(p), Some(t)) => Some(p * t),
            _ => None,
        };
        let f05 = match (corrected, self.recall) {
            (Some(p), Some(r)) => f_beta(p, r, 0.5),
            _ => None,
        };
        let ch_f05 = match (self.channel_precision, self.channel_recall) {
            (Some(p), Some(r)) => f_beta(p, r, 0.5),
            _ => None,
        };
        let days = self.nominal_s / 86_400.0;
        let r6 = |x: Option<f64>| x.map(|v| round(v, 6));
        BTreeMap::from([
            ("event_recall".into(), r6(self.recall)),
            ("event_precision".into(), r6(self.precision)),
            ("time_tnr".into(), r6(self.tnr)),
            ("event_precision_corrected".into(), r6(corrected)),
            ("event_f0_5".into(), r6(f05)),
            ("channel_precision".into(), r6(self.channel_precision)),
            ("channel_recall".into(), r6(self.channel_recall)),
            ("channel_f0_5".into(), r6(ch_f05)),
            (
                "alarming_precision".into(),
                r6(ratio(self.tp_events as f64, self.tp_alarms as f64).map(|x| x.min(1.0))),
            ),
            (
                "false_alarms_per_day".into(),
                r6(ratio(self.fp_alarms as f64, days)),
            ),
            (
                "latency_median_s".into(),
                percentile(&self.latencies, 50.0).map(|v| round(v, 3)),
            ),
            (
                "latency_p90_s".into(),
                percentile(&self.latencies, 90.0).map(|v| round(v, 3)),
            ),
        ])
    }

    pub fn counts(&self) -> BTreeMap<String, u64> {
        BTreeMap::from([
            ("events".into(), self.events),
            ("events_ignored".into(), self.ignored_events),
            ("events_detected".into(), self.tp_events),
            ("alarms".into(), self.alarms),
            ("alarms_on_events".into(), self.tp_alarms),
            ("alarms_false".into(), self.fp_alarms),
            ("alarms_on_ignored_events".into(), self.ignored_alarms),
            ("nominal_seconds".into(), self.nominal_s.round() as u64),
            (
                "alarmed_nominal_seconds".into(),
                self.alarmed_nominal_s.round() as u64,
            ),
        ])
    }
}

// ------------------------------------------------------------------ opssat-ad

/// Segment truth: segment id to (anomaly, train).
pub fn parse_opssat(dataset: &str) -> Result<BTreeMap<String, (bool, bool)>, KshanaError> {
    let t = Table::parse("dataset.csv", dataset)?;
    let (cs, ca, ct) = (t.col("segment")?, t.col("anomaly")?, t.col("train")?);
    let b = |line: usize, r: &[String], c: usize| match Table::get(r, c) {
        "0" => Ok(false),
        "1" => Ok(true),
        x => Err(err(format!(
            "dataset.csv line {line}: {} '{x}' is not 0 or 1",
            t.header[c]
        ))),
    };
    let mut out = BTreeMap::new();
    for (line, r) in &t.rows {
        let seg = Table::get(r, cs).to_string();
        if out
            .insert(seg.clone(), (b(*line, r, ca)?, b(*line, r, ct)?))
            .is_some()
        {
            return Err(err(format!(
                "dataset.csv line {line}: segment {seg} repeated"
            )));
        }
    }
    Ok(out)
}

/// Segment predictions: segment id to (predicted, score).
pub fn parse_predictions(text: &str) -> Result<BTreeMap<String, (bool, Option<f64>)>, KshanaError> {
    let t = Table::parse("predictions", text)?;
    let (cs, cp) = (t.col("segment")?, t.col("predicted")?);
    let csc = t.opt_col("score");
    let mut out = BTreeMap::new();
    for (line, r) in &t.rows {
        let p = match Table::get(r, cp) {
            "0" => false,
            "1" => true,
            x => {
                return Err(err(format!(
                    "predictions line {line}: predicted '{x}' is not 0 or 1"
                )))
            }
        };
        let score = match csc {
            Some(c) if !Table::get(r, c).is_empty() => Some(t.num(*line, r, c)?),
            _ => None,
        };
        let seg = Table::get(r, cs).to_string();
        if out.insert(seg.clone(), (p, score)).is_some() {
            return Err(err(format!(
                "predictions line {line}: segment {seg} repeated"
            )));
        }
    }
    Ok(out)
}

/// Area under the ROC curve (Mann-Whitney, ties count one half).
fn roc_auc(pos: &[f64], neg: &[f64]) -> Option<f64> {
    if pos.is_empty() || neg.is_empty() {
        return None;
    }
    let mut all: Vec<(f64, bool)> = pos
        .iter()
        .map(|&s| (s, true))
        .chain(neg.iter().map(|&s| (s, false)))
        .collect();
    all.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Mid-ranks.
    let mut rank_sum_pos = 0.0;
    let mut i = 0;
    while i < all.len() {
        let mut j = i;
        while j + 1 < all.len() && all[j + 1].0 == all[i].0 {
            j += 1;
        }
        let mid = (i + j) as f64 / 2.0 + 1.0;
        rank_sum_pos += all[i..=j].iter().filter(|x| x.1).count() as f64 * mid;
        i = j + 1;
    }
    let (np, nn) = (pos.len() as f64, neg.len() as f64);
    Some((rank_sum_pos - np * (np + 1.0) / 2.0) / (np * nn))
}

/// Average precision (step-wise area under the precision-recall curve; tied scores are
/// taken together).
fn average_precision(scored: &[(f64, bool)]) -> Option<f64> {
    let npos = scored.iter().filter(|x| x.1).count() as f64;
    if npos == 0.0 {
        return None;
    }
    let mut s = scored.to_vec();
    s.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (mut tp, mut fp, mut ap, mut last_r) = (0.0, 0.0, 0.0, 0.0);
    let mut i = 0;
    while i < s.len() {
        let mut j = i;
        while j + 1 < s.len() && s[j + 1].0 == s[i].0 {
            j += 1;
        }
        for x in &s[i..=j] {
            if x.1 {
                tp += 1.0
            } else {
                fp += 1.0
            }
        }
        let r = tp / npos;
        ap += (r - last_r) * tp / (tp + fp);
        last_r = r;
        i = j + 1;
    }
    Some(ap)
}

/// Metrics, counts and notes of a segment score.
pub type SegmentScore = (
    BTreeMap<String, Option<f64>>,
    BTreeMap<String, u64>,
    Vec<String>,
);

/// Score segment predictions.
pub fn score_segments(
    truth: &BTreeMap<String, (bool, bool)>,
    pred: &BTreeMap<String, (bool, Option<f64>)>,
    test_only: bool,
) -> Result<SegmentScore, KshanaError> {
    let mut notes = Vec::new();
    if let Some(s) = pred.keys().find(|s| !truth.contains_key(*s)) {
        return Err(err(format!(
            "predictions name segment {s}, which the dataset does not have"
        )));
    }
    let eval: Vec<(&String, bool)> = truth
        .iter()
        .filter(|(_, (_, train))| !test_only || !*train)
        .map(|(s, (a, _))| (s, *a))
        .collect();
    let missing: Vec<&String> = eval
        .iter()
        .map(|x| x.0)
        .filter(|s| !pred.contains_key(*s))
        .collect();
    if !missing.is_empty() {
        return Err(err(format!(
            "predictions miss {} of the {} evaluated segments (first: {})",
            missing.len(),
            eval.len(),
            missing[0]
        )));
    }
    let extra = pred.len() - eval.len();
    if extra > 0 {
        notes.push(format!(
            "{extra} predictions for segments outside the evaluated split were ignored"
        ));
    }
    let (mut tp, mut fp, mut tn, mut fn_) = (0u64, 0u64, 0u64, 0u64);
    let mut scored = Vec::new();
    for (s, a) in &eval {
        let (p, sc) = pred[*s];
        match (a, p) {
            (true, true) => tp += 1,
            (false, true) => fp += 1,
            (false, false) => tn += 1,
            (true, false) => fn_ += 1,
        }
        if let Some(x) = sc {
            scored.push((x, *a));
        }
    }
    let (tpf, fpf, tnf, fnf) = (tp as f64, fp as f64, tn as f64, fn_ as f64);
    let precision = ratio(tpf, tpf + fpf);
    let recall = ratio(tpf, tpf + fnf);
    let tnr = ratio(tnf, tnf + fpf);
    let mcc_d = ((tpf + fpf) * (tpf + fnf) * (tnf + fpf) * (tnf + fnf)).sqrt();
    let all_scored = scored.len() == eval.len();
    if !scored.is_empty() && !all_scored {
        notes.push(
            "scores are given for only some segments, so no ranking metric is computed".into(),
        );
    }
    let (auc, ap) = if all_scored && !scored.is_empty() {
        let pos: Vec<f64> = scored.iter().filter(|x| x.1).map(|x| x.0).collect();
        let neg: Vec<f64> = scored.iter().filter(|x| !x.1).map(|x| x.0).collect();
        (roc_auc(&pos, &neg), average_precision(&scored))
    } else {
        (None, None)
    };
    let pr = |x: Option<f64>| x.map(|v| round(v, 6));
    let f1 = match (precision, recall) {
        (Some(p), Some(r)) => f_beta(p, r, 1.0),
        _ => None,
    };
    let f05 = match (precision, recall) {
        (Some(p), Some(r)) => f_beta(p, r, 0.5),
        _ => None,
    };
    let metrics = BTreeMap::from([
        ("precision".into(), pr(precision)),
        ("recall".into(), pr(recall)),
        ("f1".into(), pr(f1)),
        ("f0_5".into(), pr(f05)),
        ("accuracy".into(), pr(ratio(tpf + tnf, eval.len() as f64))),
        (
            "balanced_accuracy".into(),
            pr(match (recall, tnr) {
                (Some(r), Some(t)) => Some((r + t) / 2.0),
                _ => None,
            }),
        ),
        ("mcc".into(), pr(ratio(tpf * tnf - fpf * fnf, mcc_d))),
        ("roc_auc".into(), pr(auc)),
        ("average_precision".into(), pr(ap)),
        (
            "false_alarms_per_100_nominal_segments".into(),
            pr(ratio(100.0 * fpf, tnf + fpf)),
        ),
    ]);
    let counts = BTreeMap::from([
        ("segments".into(), eval.len() as u64),
        ("tp".into(), tp),
        ("fp".into(), fp),
        ("tn".into(), tn),
        ("fn".into(), fn_),
    ]);
    Ok((metrics, counts, notes))
}

// ------------------------------------------------------------------ the scorecard

/// Judge bars against metrics.
pub fn judge(bars: &Bars, metrics: &BTreeMap<String, Option<f64>>) -> (Vec<BarResult>, String) {
    let res: Vec<BarResult> = bars
        .bars
        .iter()
        .map(|b| {
            let value = metrics.get(&b.metric).copied().flatten();
            let (bar, ok) = match (b.min, b.max) {
                (Some(m), _) => (format!(">= {m}"), value.map(|v| v >= m)),
                (_, Some(m)) => (format!("<= {m}"), value.map(|v| v <= m)),
                _ => unreachable!("validated"),
            };
            let verdict = match (metrics.contains_key(&b.metric), ok) {
                (false, _) => "NOT APPLICABLE",
                (true, Some(true)) => "PASS",
                // An undefined metric (no detection at all, say) does not pass a bar.
                (true, _) => "FAIL",
            };
            BarResult {
                metric: b.metric.clone(),
                bar,
                value,
                verdict: verdict.into(),
                why: b.why.clone(),
            }
        })
        .collect();
    let verdict = if res.iter().any(|r| r.verdict == "FAIL") {
        "FAIL"
    } else if res.iter().any(|r| r.verdict == "PASS") {
        "PASS"
    } else {
        "NO BARS"
    };
    (res, verdict.into())
}

/// Run a scorecard spec (files it names are read relative to `base`). Returns the
/// scorecard and the bytes of the spec and bars to keep beside it.
pub fn run(spec_text: &str, base: &Path) -> Result<(Scorecard, Option<Vec<u8>>), KshanaError> {
    let spec: Spec =
        toml::from_str(spec_text).map_err(|e| err(format!("invalid scorecard spec: {e}")))?;
    let b = &spec.benchmark;
    let d = &spec.detector;
    let mut inputs = vec![InputHash {
        role: "spec".into(),
        file: "spec.toml".into(),
        sha256: sha256_hex(spec_text.as_bytes()),
    }];
    let mut load = |role: &str, rel: &str| -> Result<String, KshanaError> {
        let (p, bytes) = read_rel(base, rel)?;
        inputs.push(InputHash {
            role: role.into(),
            file: p
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default(),
            sha256: sha256_hex(&bytes),
        });
        super::utf8(rel, &bytes)
    };
    let need = |v: &Option<String>, k: &str| {
        v.clone()
            .ok_or_else(|| err(format!("benchmark format {} needs {k}", b.format)))
    };
    let mut notes = Vec::new();
    let (metrics, counts, by_type, missed) = match b.format.as_str() {
        "esa-ad" => {
            let truth = parse_esa_ad(
                &load("labels", &need(&b.labels, "labels")?)?,
                &load("anomaly_types", &need(&b.anomaly_types, "anomaly_types")?)?,
            )?;
            let alarms = parse_alarms(&load("detector outputs", &d.outputs)?)?;
            let w0 = utc(
                "window_start_utc",
                &need(&b.window_start_utc, "window_start_utc")?,
            )?;
            let w1 = utc(
                "window_end_utc",
                &need(&b.window_end_utc, "window_end_utc")?,
            )?;
            let s = score_intervals(&truth, &alarms, w0, w1, &b.channels, b.rare_events)?;
            if s.events == 0 {
                notes.push("no annotated event to detect lies in the window and scope".into());
            }
            notes.push(format!(
                "evaluation window {} to {}; {} channels in scope{}",
                fmt_utc_ms(w0),
                fmt_utc_ms(w1),
                if b.channels.is_empty() {
                    "all".to_string()
                } else {
                    b.channels.len().to_string()
                },
                match b.rare_events {
                    RareEvents::Ignore => "; rare nominal events ignored",
                    RareEvents::Count => "; rare nominal events counted as anomalies",
                }
            ));
            (s.metrics(), s.counts(), s.by_type.clone(), s.missed.clone())
        }
        "opssat-ad" => {
            let truth = parse_opssat(&load("dataset", &need(&b.dataset, "dataset")?)?)?;
            let pred = parse_predictions(&load("detector outputs", &d.outputs)?)?;
            let test_only = match b.split.as_deref().unwrap_or("test") {
                "test" => true,
                "all" => false,
                x => return Err(err(format!("split '{x}' is not test or all"))),
            };
            let (m, c, n) = score_segments(&truth, &pred, test_only)?;
            notes.extend(n);
            (m, c, BTreeMap::new(), Vec::new())
        }
        x => {
            return Err(err(format!(
                "benchmark format '{x}' is not esa-ad or opssat-ad"
            )))
        }
    };
    let (bars_bytes, bars_registered_utc, bar_results, verdict) = match &d.bars {
        Some(rel) => {
            let (_, bytes) = read_rel(base, rel)?;
            let h = sha256_hex(&bytes);
            if let Some(expected) = &d.bars_sha256 {
                if !expected.eq_ignore_ascii_case(&h) {
                    return Err(err(format!(
                        "the bars file hashes to {h}, not the pre-registered {expected}; refusing to score against changed bars"
                    )));
                }
            } else {
                notes.push("no bars_sha256 given, so the scorecard cannot show the bars were fixed before scoring".into());
            }
            inputs.push(InputHash {
                role: "bars".into(),
                file: "bars.toml".into(),
                sha256: h,
            });
            let bars = Bars::parse(&super::utf8("bars", &bytes)?)?;
            let (r, v) = judge(&bars, &metrics);
            (Some(bytes), Some(bars.registered_utc), r, v)
        }
        None => (None, None, Vec::new(), "NO BARS".to_string()),
    };
    let evidence_class = match b.data_origin {
        DataOrigin::Synthetic => "MODELLED",
        _ => "MEASURED",
    };
    let body = ScorecardBody {
        schema: SCHEMA.into(),
        benchmark: b.clone(),
        detector_name: d.name.clone(),
        detector_version: d.version.clone(),
        evidence_class: evidence_class.into(),
        inputs,
        metrics,
        counts,
        recall_by_type: by_type,
        missed_events: missed,
        bars_registered_utc,
        bars: bar_results,
        verdict,
        notes,
    };
    let scorecard_sha256 = body_sha256(&body)?;
    Ok((
        Scorecard {
            label: LABEL.into(),
            body,
            scorecard_sha256,
        },
        bars_bytes,
    ))
}

impl Scorecard {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("the scorecard serialises")
    }

    pub fn summary_line(&self) -> String {
        let b = &self.body;
        let pick = |k: &str| {
            b.metrics
                .get(k)
                .copied()
                .flatten()
                .map(|v| format!("{k} {v:.3}"))
        };
        let shown: Vec<String> = [
            "event_recall",
            "event_precision_corrected",
            "event_f0_5",
            "false_alarms_per_day",
            "f1",
            "roc_auc",
        ]
        .iter()
        .filter_map(|k| pick(k))
        .collect();
        format!(
            "{} {} on {} ({}): {}; bars {}",
            b.detector_name,
            b.detector_version,
            b.benchmark.name,
            b.evidence_class,
            shown.join(", "),
            b.verdict
        )
    }

    pub fn to_markdown(&self) -> String {
        let b = &self.body;
        let mut s = format!(
            "# Anomaly-detection scorecard: {} {} on {}\n\n**Bars: {}** · evidence class **{}**\n\n_{}_\n\nScorecard SHA-256 `{}`\n\n",
            b.detector_name, b.detector_version, b.benchmark.name, b.verdict, b.evidence_class, self.label, self.scorecard_sha256
        );
        s += "## Benchmark\n\n| Item | Value |\n|---|---|\n";
        s += &format!("| Format | {} |\n", b.benchmark.format);
        s += &format!("| Data origin | {:?} |\n", b.benchmark.data_origin);
        s += &format!(
            "| Source | {} |\n",
            b.benchmark.source_doi.as_deref().unwrap_or("not stated")
        );
        s += &format!(
            "| Licence | {} |\n",
            b.benchmark.licence.as_deref().unwrap_or("not stated")
        );
        for i in &b.inputs {
            s += &format!("| {} ({}) | `{}` |\n", i.role, i.file, i.sha256);
        }
        s += "\n## Metrics\n\n| Metric | Value |\n|---|---|\n";
        for (k, v) in &b.metrics {
            s += &format!(
                "| {k} | {} |\n",
                v.map(|x| format!("{x}"))
                    .unwrap_or_else(|| "undefined".into())
            );
        }
        s += "\n| Count | Value |\n|---|---|\n";
        for (k, v) in &b.counts {
            s += &format!("| {k} | {v} |\n");
        }
        if !b.recall_by_type.is_empty() {
            s += "\n## Recall by annotated type\n\n| Type | Detected | Events |\n|---|---|---|\n";
            for (k, (d, n)) in &b.recall_by_type {
                s += &format!("| {k} | {d} | {n} |\n");
            }
        }
        s += "\n## Pre-registered bars\n\n";
        if b.bars.is_empty() {
            s += "No bars were given.\n";
        } else {
            s += &format!(
                "Registered {}.\n\n| Metric | Bar | Value | Verdict | Why |\n|---|---|---|---|---|\n",
                b.bars_registered_utc.as_deref().unwrap_or("?")
            );
            for r in &b.bars {
                s += &format!(
                    "| {} | {} | {} | {} | {} |\n",
                    r.metric,
                    r.bar,
                    r.value
                        .map(|x| format!("{x}"))
                        .unwrap_or_else(|| "undefined".into()),
                    r.verdict,
                    r.why.as_deref().unwrap_or("")
                );
            }
        }
        if !b.missed_events.is_empty() {
            s += &format!("\n## Missed events\n\n{}\n", b.missed_events.join(", "));
        }
        if !b.notes.is_empty() {
            s += "\n## Notes\n\n";
            for n in &b.notes {
                s += &format!("* {n}\n");
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_metrics() {
        assert_eq!(roc_auc(&[0.9, 0.8], &[0.1, 0.2]), Some(1.0));
        assert_eq!(roc_auc(&[0.5], &[0.5]), Some(0.5));
        assert_eq!(roc_auc(&[0.1], &[0.9]), Some(0.0));
        assert_eq!(
            average_precision(&[(0.9, true), (0.8, false), (0.7, true)]),
            Some((1.0 + 2.0 / 3.0) / 2.0)
        );
        assert_eq!(f_beta(1.0, 1.0, 0.5), Some(1.0));
        assert_eq!(f_beta(0.0, 0.0, 0.5), None);
    }
}
