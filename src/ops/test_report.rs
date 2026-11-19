// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! An operations validation report generated from a test-run log.
//!
//! Input 1, the **test procedure** (TOML), in the structure of an ECSS-E-ST-70-32C
//! procedure: a preconditions body (telemetry that must hold before the main body), a main
//! body of **steps** (each with the telecommands it sends, the verification stages each
//! must report, and the telemetry expected), and a confirmation body (telemetry that must
//! hold after the last step). Telecommand verification stages are the ECSS-E-ST-70-41C
//! (PUS) ones: acceptance, start, progress, completion. The file is not PLUTO source and
//! no PLUTO parser is provided; the terms are aligned, nothing more.
//!
//! Input 2, the **run log** (CSV) that a test executor writes:
//! `time_utc,event,step,name,value,detail` with events `run_start` (name = run id,
//! value = log source), `run_end`, `step_start`, `step_end`, `tc` (name = telecommand),
//! `tc_ack` (name = telecommand, value = stage, detail = `success` or `failure`), `tm`
//! (name = parameter, value = observed value), `abort` (detail = reason), `operator`
//! (name = who ran it) and `note`.
//!
//! The report states per step whether it is **confirmed**, **not confirmed**, **aborted**
//! or **not executed** (the ECSS-E-ST-70-32C confirmation statuses, plus "not executed"),
//! with every expected value beside the observed one, every telecommand's verification,
//! the deviations (failed acknowledgements, unplanned telecommands, steps out of order,
//! missing telemetry) and the requirement verification matrix. It reports what the log
//! says; it does not check that the log is authentic (that is the signer's and the test
//! facility's responsibility).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::csv::Table;
use super::{body_sha256, err, fmt_utc_ms, round};
use kshana::api::KshanaError;

/// Identifies `report.json`.
pub const SCHEMA: &str = "arksa-ops-test-report/1";

/// The honesty label every report carries.
pub const LABEL: &str = "Generated from the supplied test-run log. Statuses state what the log records against what the procedure expects; the log's authenticity and the test configuration are the test facility's responsibility. Terms aligned with ECSS-E-ST-70-32C; not a certification and not acceptance by any agency.";

/// Log sources, and the evidence class each gives the report.
pub const SOURCES: &[(&str, &str)] = &[
    ("synthetic", "MODELLED"),
    ("simulator", "MODELLED"),
    ("software-validation-facility", "MODELLED"),
    ("engineering-model", "MEASURED"),
    ("qualification-model", "MEASURED"),
    ("flight-model", "MEASURED"),
    ("in-orbit", "MEASURED"),
];

const STAGES: &[&str] = &["acceptance", "start", "progress", "completion"];

fn default_stages() -> Vec<String> {
    vec!["acceptance".into(), "completion".into()]
}

// ------------------------------------------------------------------ procedure

/// Identification of the procedure.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcedureHead {
    pub id: String,
    pub title: String,
    pub version: String,
    #[serde(default)]
    pub system_under_test: Option<String>,
    #[serde(default)]
    pub test_facility: Option<String>,
    /// Requirements the procedure as a whole verifies.
    #[serde(default)]
    pub requirements: Vec<String>,
}

/// How a telemetry expectation is judged over its window.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CheckMode {
    /// At least one sample in the window satisfies the expectation.
    #[default]
    Reach,
    /// Every sample in the window satisfies it (and there is at least one).
    Hold,
}

/// One telemetry expectation: a range (`min`, `max`), a value with a tolerance
/// (`equals`, `tolerance`) or a state (`equals_text`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TmCheck {
    pub parameter: String,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub equals: Option<f64>,
    #[serde(default)]
    pub tolerance: Option<f64>,
    #[serde(default)]
    pub equals_text: Option<String>,
    /// The window, in seconds from the step's start (from the last step's end for the
    /// confirmation body). Without it the window runs to the step's end.
    #[serde(default)]
    pub within_s: Option<f64>,
    #[serde(default)]
    pub mode: CheckMode,
}

/// A telecommand a step sends, and the verification stages it must report as successful.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TcDef {
    pub name: String,
    #[serde(default = "default_stages")]
    pub verify: Vec<String>,
}

/// One step of the main body.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StepDef {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub telecommands: Vec<TcDef>,
    #[serde(default)]
    pub expect: Vec<TmCheck>,
}

/// The procedure file.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Procedure {
    pub procedure: ProcedureHead,
    #[serde(default)]
    pub preconditions: Vec<TmCheck>,
    pub steps: Vec<StepDef>,
    #[serde(default)]
    pub confirmation: Vec<TmCheck>,
}

impl TmCheck {
    fn validate(&self, at: &str) -> Result<(), KshanaError> {
        if self.parameter.trim().is_empty() {
            return Err(err(format!("{at}: a check needs a parameter")));
        }
        let range = self.min.is_some() || self.max.is_some();
        let kinds = [range, self.equals.is_some(), self.equals_text.is_some()]
            .iter()
            .filter(|k| **k)
            .count();
        if kinds != 1 {
            return Err(err(format!(
                "{at} ({}): give exactly one of min/max, equals (with tolerance) or equals_text",
                self.parameter
            )));
        }
        if let (Some(a), Some(b)) = (self.min, self.max) {
            if a > b {
                return Err(err(format!("{at} ({}): min > max", self.parameter)));
            }
        }
        if self.tolerance.is_some_and(|t| t.is_nan() || t < 0.0)
            || (self.tolerance.is_some() && self.equals.is_none())
        {
            return Err(err(format!(
                "{at} ({}): tolerance must be >= 0 and goes with equals",
                self.parameter
            )));
        }
        if self.within_s.is_some_and(|w| w.is_nan() || w <= 0.0) {
            return Err(err(format!(
                "{at} ({}): within_s must be > 0",
                self.parameter
            )));
        }
        Ok(())
    }

    /// The expectation in words.
    pub fn expected(&self) -> String {
        let core = if let Some(t) = &self.equals_text {
            format!("= \"{t}\"")
        } else if let Some(v) = self.equals {
            match self.tolerance {
                Some(t) if t > 0.0 => format!("= {v} ± {t}"),
                _ => format!("= {v}"),
            }
        } else {
            match (self.min, self.max) {
                (Some(a), Some(b)) => format!("in [{a}, {b}]"),
                (Some(a), None) => format!(">= {a}"),
                (None, Some(b)) => format!("<= {b}"),
                _ => unreachable!("validated"),
            }
        };
        match self.mode {
            CheckMode::Reach => core,
            CheckMode::Hold => format!("{core} throughout"),
        }
    }

    fn satisfied(&self, value: &str) -> bool {
        if let Some(t) = &self.equals_text {
            return value.trim() == t.trim();
        }
        let Ok(x) = value.trim().parse::<f64>() else {
            return false;
        };
        if let Some(v) = self.equals {
            return (x - v).abs() <= self.tolerance.unwrap_or(0.0);
        }
        self.min.is_none_or(|a| x >= a) && self.max.is_none_or(|b| x <= b)
    }
}

impl Procedure {
    pub fn parse(text: &str) -> Result<Procedure, KshanaError> {
        let p: Procedure =
            toml::from_str(text).map_err(|e| err(format!("invalid procedure: {e}")))?;
        p.validate()?;
        Ok(p)
    }

    fn validate(&self) -> Result<(), KshanaError> {
        let h = &self.procedure;
        for (k, v) in [("id", &h.id), ("title", &h.title), ("version", &h.version)] {
            if v.trim().is_empty() {
                return Err(err(format!("procedure.{k} is empty")));
            }
        }
        if self.steps.is_empty() {
            return Err(err("the procedure has no steps"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.steps {
            if s.id.trim().is_empty() || !seen.insert(s.id.as_str()) {
                return Err(err(format!("step id '{}' is empty or repeated", s.id)));
            }
            for c in &s.expect {
                c.validate(&format!("step {}", s.id))?;
            }
            let mut tcs = std::collections::BTreeSet::new();
            for t in &s.telecommands {
                if t.name.trim().is_empty() || !tcs.insert(t.name.as_str()) {
                    return Err(err(format!(
                        "step {}: telecommand '{}' is empty or repeated",
                        s.id, t.name
                    )));
                }
                for st in &t.verify {
                    if !STAGES.contains(&st.as_str()) {
                        return Err(err(format!(
                            "step {} telecommand {}: verification stage '{st}' is not one of {}",
                            s.id,
                            t.name,
                            STAGES.join(", ")
                        )));
                    }
                }
            }
        }
        for c in &self.preconditions {
            c.validate("preconditions")?;
        }
        for c in &self.confirmation {
            c.validate("confirmation")?;
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ run log

/// One line of the run log.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEvent {
    pub t: f64,
    pub line: usize,
    pub event: String,
    pub step: String,
    pub name: String,
    pub value: String,
    pub detail: String,
}

const EVENTS: &[&str] = &[
    "run_start",
    "run_end",
    "step_start",
    "step_end",
    "tc",
    "tc_ack",
    "tm",
    "abort",
    "operator",
    "note",
];

/// Parse the run log CSV; events are kept in time order (stable for equal times).
pub fn parse_log(text: &str) -> Result<Vec<LogEvent>, KshanaError> {
    let t = Table::parse("run log", text)?;
    let (ct, ce) = (t.col("time_utc")?, t.col("event")?);
    let opt = |n: &str| t.opt_col(n);
    let (cs, cn, cv, cd) = (opt("step"), opt("name"), opt("value"), opt("detail"));
    let f = |row: &[String], c: Option<usize>| {
        c.map(|c| Table::get(row, c).to_string())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for (line, row) in &t.rows {
        let event = Table::get(row, ce).to_string();
        if !EVENTS.contains(&event.as_str()) {
            return Err(err(format!(
                "run log line {line}: event '{event}' is not one of {}",
                EVENTS.join(", ")
            )));
        }
        let e = LogEvent {
            t: t.time(*line, row, ct)?,
            line: *line,
            event,
            step: f(row, cs),
            name: f(row, cn),
            value: f(row, cv),
            detail: f(row, cd),
        };
        if matches!(e.event.as_str(), "step_start" | "step_end") && e.step.is_empty() {
            return Err(err(format!(
                "run log line {line}: {} needs a step",
                e.event
            )));
        }
        if matches!(e.event.as_str(), "tc" | "tc_ack" | "tm") && e.name.is_empty() {
            return Err(err(format!(
                "run log line {line}: {} needs a name",
                e.event
            )));
        }
        if e.event == "tc_ack" {
            if !STAGES.contains(&e.value.as_str()) {
                return Err(err(format!(
                    "run log line {line}: tc_ack stage '{}' is not one of {}",
                    e.value,
                    STAGES.join(", ")
                )));
            }
            if !matches!(e.detail.as_str(), "success" | "failure") {
                return Err(err(format!(
                    "run log line {line}: tc_ack detail must be success or failure"
                )));
            }
        }
        out.push(e);
    }
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    Ok(out)
}

// ------------------------------------------------------------------ results

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Met,
    NotMet,
    NoTelemetry,
}

impl CheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckStatus::Met => "met",
            CheckStatus::NotMet => "NOT MET",
            CheckStatus::NoTelemetry => "NO TELEMETRY",
        }
    }
}

/// Expected against observed for one telemetry expectation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckResult {
    pub parameter: String,
    pub expected: String,
    pub window: String,
    pub samples: usize,
    pub status: CheckStatus,
    /// The satisfying sample (reach), the range seen (hold), or the sample that fails.
    pub observed: Option<String>,
    pub observed_utc: Option<String>,
    /// Seconds from the window's start to the satisfying sample (reach).
    pub after_s: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TcStatus {
    Verified,
    Failed,
    NotVerified,
    NotSent,
}

impl TcStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TcStatus::Verified => "verified",
            TcStatus::Failed => "FAILED",
            TcStatus::NotVerified => "NOT VERIFIED",
            TcStatus::NotSent => "NOT SENT",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StageResult {
    pub stage: String,
    /// `success`, `failure` or `not reported`.
    pub status: String,
    pub utc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TcResult {
    pub name: String,
    pub sent_utc: Option<String>,
    pub stages: Vec<StageResult>,
    pub status: TcStatus,
}

/// ECSS-E-ST-70-32C confirmation status of a step or the procedure, plus "not executed".
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Confirmation {
    Confirmed,
    NotConfirmed,
    Aborted,
    NotExecuted,
}

impl Confirmation {
    pub fn as_str(self) -> &'static str {
        match self {
            Confirmation::Confirmed => "confirmed",
            Confirmation::NotConfirmed => "NOT CONFIRMED",
            Confirmation::Aborted => "ABORTED",
            Confirmation::NotExecuted => "NOT EXECUTED",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepResult {
    pub id: String,
    pub title: String,
    pub requirements: Vec<String>,
    pub started_utc: Option<String>,
    pub ended_utc: Option<String>,
    pub telecommands: Vec<TcResult>,
    pub checks: Vec<CheckResult>,
    pub status: Confirmation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Deviation {
    pub id: String,
    pub kind: String,
    pub step: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequirementResult {
    pub requirement: String,
    /// The steps that trace to it (`procedure` for the procedure as a whole).
    pub verified_by: Vec<String>,
    /// `verified by test (as logged)` or `not verified`.
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunInfo {
    pub run_id: String,
    pub source: String,
    pub evidence_class: String,
    pub operator: Option<String>,
    pub started_utc: String,
    pub ended_utc: Option<String>,
    pub log_lines: usize,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub steps: usize,
    pub confirmed: usize,
    pub not_confirmed: usize,
    pub aborted: usize,
    pub not_executed: usize,
    pub telecommands_expected: usize,
    pub telecommands_verified: usize,
    pub telecommands_failed: usize,
    pub checks: usize,
    pub checks_met: usize,
    pub deviations: usize,
    pub requirements_verified: usize,
    pub requirements: usize,
}

/// The report body (what `report_sha256` covers).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReportBody {
    pub schema: String,
    pub procedure: ProcedureHead,
    pub procedure_sha256: String,
    pub log_sha256: String,
    pub run: RunInfo,
    pub preconditions: Vec<CheckResult>,
    pub preconditions_met: bool,
    pub steps: Vec<StepResult>,
    pub confirmation: Vec<CheckResult>,
    pub status: Confirmation,
    pub deviations: Vec<Deviation>,
    pub requirements: Vec<RequirementResult>,
    pub summary: Summary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestReport {
    pub label: String,
    pub body: ReportBody,
    pub report_sha256: String,
}

// ------------------------------------------------------------------ evaluation

fn eval_window(c: &TmCheck, log: &[LogEvent], from: f64, to: f64) -> CheckResult {
    let samples: Vec<&LogEvent> = log
        .iter()
        .filter(|e| e.event == "tm" && e.name == c.parameter && e.t >= from && e.t <= to)
        .collect();
    let window = format!("{} to {}", fmt_utc_ms(from), fmt_utc_ms(to));
    let mut r = CheckResult {
        parameter: c.parameter.clone(),
        expected: c.expected(),
        window,
        samples: samples.len(),
        status: CheckStatus::NoTelemetry,
        observed: None,
        observed_utc: None,
        after_s: None,
    };
    if samples.is_empty() {
        return r;
    }
    match c.mode {
        CheckMode::Reach => match samples.iter().find(|e| c.satisfied(&e.value)) {
            Some(e) => {
                r.status = CheckStatus::Met;
                r.observed = Some(e.value.clone());
                r.observed_utc = Some(fmt_utc_ms(e.t));
                r.after_s = Some(round(e.t - from, 3));
            }
            None => {
                let last = samples.last().expect("non-empty");
                r.status = CheckStatus::NotMet;
                r.observed = Some(format!(
                    "{} (last of {} samples)",
                    last.value,
                    samples.len()
                ));
                r.observed_utc = Some(fmt_utc_ms(last.t));
            }
        },
        CheckMode::Hold => match samples.iter().find(|e| !c.satisfied(&e.value)) {
            Some(e) => {
                r.status = CheckStatus::NotMet;
                r.observed = Some(e.value.clone());
                r.observed_utc = Some(fmt_utc_ms(e.t));
            }
            None => {
                r.status = CheckStatus::Met;
                let nums: Vec<f64> = samples
                    .iter()
                    .filter_map(|e| e.value.trim().parse::<f64>().ok())
                    .collect();
                r.observed = Some(if nums.len() == samples.len() {
                    let lo = nums.iter().copied().fold(f64::INFINITY, f64::min);
                    let hi = nums.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    format!("{lo} to {hi} over {} samples", samples.len())
                } else {
                    format!("{} over {} samples", samples[0].value, samples.len())
                });
            }
        },
    }
    r
}

/// The latest sample at or before `at` (preconditions).
fn eval_before(c: &TmCheck, log: &[LogEvent], at: f64) -> CheckResult {
    let last = log
        .iter()
        .rfind(|e| e.event == "tm" && e.name == c.parameter && e.t <= at);
    let mut r = CheckResult {
        parameter: c.parameter.clone(),
        expected: c.expected(),
        window: format!("latest sample at or before {}", fmt_utc_ms(at)),
        samples: usize::from(last.is_some()),
        status: CheckStatus::NoTelemetry,
        observed: None,
        observed_utc: None,
        after_s: None,
    };
    if let Some(e) = last {
        r.status = if c.satisfied(&e.value) {
            CheckStatus::Met
        } else {
            CheckStatus::NotMet
        };
        r.observed = Some(e.value.clone());
        r.observed_utc = Some(fmt_utc_ms(e.t));
    }
    r
}

/// Build the report of a procedure and its run log (the bytes of each hash to the given
/// values).
pub fn build(
    proc_: &Procedure,
    log: &[LogEvent],
    procedure_sha256: &str,
    log_sha256: &str,
) -> Result<TestReport, KshanaError> {
    let start = log
        .iter()
        .find(|e| e.event == "run_start")
        .ok_or_else(|| err("the run log has no run_start event"))?;
    let evidence_class = SOURCES
        .iter()
        .find(|(s, _)| *s == start.value)
        .map(|(_, c)| c.to_string())
        .ok_or_else(|| {
            err(format!(
                "run_start value (the log source) '{}' is not one of {}",
                start.value,
                SOURCES
                    .iter()
                    .map(|(s, _)| *s)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
    let run_end = log.iter().rev().find(|e| e.event == "run_end").map(|e| e.t);
    let horizon = run_end.unwrap_or_else(|| log.last().map(|e| e.t).unwrap_or(start.t));
    let ids: Vec<&str> = proc_.steps.iter().map(|s| s.id.as_str()).collect();
    let mut deviations: Vec<Deviation> = Vec::new();
    let mut dev = |kind: &str, step: Option<&str>, text: String| {
        deviations.push(Deviation {
            id: String::new(),
            kind: kind.into(),
            step: step.map(str::to_string),
            text,
        })
    };

    // Step windows from the log.
    let mut starts: BTreeMap<&str, f64> = BTreeMap::new();
    let mut ends: BTreeMap<&str, f64> = BTreeMap::new();
    for e in log {
        match e.event.as_str() {
            "step_start" | "step_end" if !ids.contains(&e.step.as_str()) => dev(
                "unknown step",
                Some(&e.step),
                format!(
                    "log line {}: step '{}' is not in the procedure",
                    e.line, e.step
                ),
            ),
            "step_start" => {
                if starts.contains_key(e.step.as_str()) {
                    dev(
                        "step repeated",
                        Some(&e.step),
                        format!(
                            "log line {}: step {} started again; the first execution is assessed",
                            e.line, e.step
                        ),
                    );
                } else {
                    starts.insert(e.step.as_str(), e.t);
                }
            }
            "step_end" => {
                ends.entry(e.step.as_str()).or_insert(e.t);
            }
            _ => {}
        }
    }
    // Execution order.
    let mut executed: Vec<(&str, f64)> = starts.iter().map(|(k, v)| (*k, *v)).collect();
    executed.sort_by(|a, b| a.1.total_cmp(&b.1));
    let defined: Vec<&str> = ids
        .iter()
        .copied()
        .filter(|i| starts.contains_key(i))
        .collect();
    if executed.iter().map(|x| x.0).collect::<Vec<_>>() != defined {
        dev(
            "order",
            None,
            format!(
                "steps ran in the order {}, not the procedure's order {}",
                executed.iter().map(|x| x.0).collect::<Vec<_>>().join(", "),
                defined.join(", ")
            ),
        );
    }
    // A step's window: its start to its end, else to the next step's start, else to the
    // end of the run.
    let window_of = |id: &str| -> Option<(f64, f64)> {
        let s = *starts.get(id)?;
        let e = ends.get(id).copied().unwrap_or_else(|| {
            executed
                .iter()
                .map(|x| x.1)
                .find(|t| *t > s)
                .unwrap_or(horizon)
        });
        Some((s, e.max(s)))
    };
    let step_at = |t: f64, hint: &str| -> Option<String> {
        if !hint.is_empty() && ids.contains(&hint) {
            return Some(hint.to_string());
        }
        ids.iter()
            .find(|id| window_of(id).is_some_and(|(a, b)| t >= a && t <= b))
            .map(|s| s.to_string())
    };

    // Aborts.
    let mut aborted: BTreeMap<String, String> = BTreeMap::new();
    let mut abort_time: Option<f64> = None;
    for e in log.iter().filter(|e| e.event == "abort") {
        let s = step_at(e.t, &e.step).unwrap_or_else(|| "procedure".into());
        dev(
            "abort",
            Some(&s),
            format!(
                "log line {}: aborted at {}{}",
                e.line,
                fmt_utc_ms(e.t),
                if e.detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", e.detail)
                }
            ),
        );
        aborted.entry(s).or_insert_with(|| e.detail.clone());
        abort_time.get_or_insert(e.t);
    }

    // Telecommands: planned ones are matched in their step's window; every other is
    // unplanned.
    let mut matched_tc_lines = std::collections::BTreeSet::new();
    let mut steps = Vec::new();
    for s in &proc_.steps {
        let win = window_of(&s.id);
        let mut tcs = Vec::new();
        for t in &s.telecommands {
            let sent = win.and_then(|(a, b)| {
                log.iter().find(|e| {
                    e.event == "tc"
                        && e.name == t.name
                        && e.t >= a
                        && e.t <= b
                        && (e.step.is_empty() || e.step == s.id)
                })
            });
            let Some(sent) = sent else {
                tcs.push(TcResult {
                    name: t.name.clone(),
                    sent_utc: None,
                    stages: Vec::new(),
                    status: TcStatus::NotSent,
                });
                if win.is_some() {
                    dev(
                        "telecommand not sent",
                        Some(&s.id),
                        format!("step {}: {} was not sent", s.id, t.name),
                    );
                }
                continue;
            };
            matched_tc_lines.insert(sent.line);
            let next_send = log
                .iter()
                .find(|e| e.event == "tc" && e.name == t.name && e.t > sent.t)
                .map(|e| e.t)
                .unwrap_or(f64::INFINITY);
            let mut stages = Vec::new();
            let mut failed = false;
            let mut all = true;
            for st in &t.verify {
                let ack = log.iter().find(|e| {
                    e.event == "tc_ack"
                        && e.name == t.name
                        && e.value == *st
                        && e.t >= sent.t
                        && e.t < next_send
                });
                match ack {
                    Some(a) => {
                        if a.detail == "failure" {
                            failed = true;
                            dev(
                                "telecommand verification failure",
                                Some(&s.id),
                                format!(
                                    "step {}: {} reported {} failure at {}",
                                    s.id,
                                    t.name,
                                    st,
                                    fmt_utc_ms(a.t)
                                ),
                            );
                        }
                        stages.push(StageResult {
                            stage: st.clone(),
                            status: a.detail.clone(),
                            utc: Some(fmt_utc_ms(a.t)),
                        });
                    }
                    None => {
                        all = false;
                        stages.push(StageResult {
                            stage: st.clone(),
                            status: "not reported".into(),
                            utc: None,
                        });
                    }
                }
            }
            let status = if failed {
                TcStatus::Failed
            } else if all {
                TcStatus::Verified
            } else {
                dev(
                    "telecommand not verified",
                    Some(&s.id),
                    format!(
                        "step {}: {} did not report every expected verification stage",
                        s.id, t.name
                    ),
                );
                TcStatus::NotVerified
            };
            tcs.push(TcResult {
                name: t.name.clone(),
                sent_utc: Some(fmt_utc_ms(sent.t)),
                stages,
                status,
            });
        }
        let checks: Vec<CheckResult> = match win {
            Some((a, b)) => s
                .expect
                .iter()
                .map(|c| {
                    let to = c.within_s.map(|w| a + w).unwrap_or(b);
                    eval_window(c, log, a, to)
                })
                .collect(),
            None => Vec::new(),
        };
        for c in &checks {
            if c.status != CheckStatus::Met {
                dev(
                    if c.status == CheckStatus::NoTelemetry {
                        "no telemetry"
                    } else {
                        "telemetry out of expectation"
                    },
                    Some(&s.id),
                    format!(
                        "step {}: {} expected {}, observed {} ({})",
                        s.id,
                        c.parameter,
                        c.expected,
                        c.observed.as_deref().unwrap_or("nothing"),
                        c.window
                    ),
                );
            }
        }
        let status = if aborted.contains_key(&s.id) {
            Confirmation::Aborted
        } else if win.is_none() {
            Confirmation::NotExecuted
        } else if tcs.iter().all(|t| t.status == TcStatus::Verified)
            && checks.iter().all(|c| c.status == CheckStatus::Met)
        {
            Confirmation::Confirmed
        } else {
            Confirmation::NotConfirmed
        };
        steps.push(StepResult {
            id: s.id.clone(),
            title: s.title.clone(),
            requirements: s.requirements.clone(),
            started_utc: win.map(|w| fmt_utc_ms(w.0)),
            ended_utc: win.map(|w| fmt_utc_ms(w.1)),
            telecommands: tcs,
            checks,
            status,
        });
    }
    for e in log
        .iter()
        .filter(|e| e.event == "tc" && !matched_tc_lines.contains(&e.line))
    {
        let s = step_at(e.t, &e.step);
        dev(
            "unplanned telecommand",
            s.as_deref(),
            format!(
                "log line {}: {} sent at {} is not planned {}",
                e.line,
                e.name,
                fmt_utc_ms(e.t),
                match &s {
                    Some(s) => format!("in step {s}"),
                    None => "in any step window".into(),
                }
            ),
        );
    }
    for s in steps
        .iter()
        .filter(|s| s.status == Confirmation::NotExecuted)
    {
        if abort_time.is_none() {
            dev(
                "step not executed",
                Some(&s.id),
                format!("step {} has no step_start in the log", s.id),
            );
        }
    }

    // Preconditions and confirmation.
    let first_step = executed.first().map(|x| x.1).unwrap_or(start.t);
    let preconditions: Vec<CheckResult> = proc_
        .preconditions
        .iter()
        .map(|c| eval_before(c, log, first_step))
        .collect();
    let preconditions_met = preconditions.iter().all(|c| c.status == CheckStatus::Met);
    for c in preconditions
        .iter()
        .filter(|c| c.status != CheckStatus::Met)
    {
        dev(
            "precondition",
            None,
            format!(
                "precondition {} expected {}, observed {}",
                c.parameter,
                c.expected,
                c.observed.as_deref().unwrap_or("nothing")
            ),
        );
    }
    let last_end = proc_
        .steps
        .iter()
        .filter_map(|s| window_of(&s.id))
        .map(|w| w.1)
        .fold(f64::NEG_INFINITY, f64::max);
    let confirmation: Vec<CheckResult> = if last_end.is_finite() && abort_time.is_none() {
        proc_
            .confirmation
            .iter()
            .map(|c| {
                let to = c.within_s.map(|w| last_end + w).unwrap_or(horizon);
                eval_window(c, log, last_end, to)
            })
            .collect()
    } else {
        Vec::new()
    };
    for c in confirmation.iter().filter(|c| c.status != CheckStatus::Met) {
        dev(
            "confirmation",
            None,
            format!(
                "confirmation {} expected {}, observed {}",
                c.parameter,
                c.expected,
                c.observed.as_deref().unwrap_or("nothing")
            ),
        );
    }
    let status = if abort_time.is_some() {
        Confirmation::Aborted
    } else if preconditions_met
        && steps.iter().all(|s| s.status == Confirmation::Confirmed)
        && confirmation.len() == proc_.confirmation.len()
        && confirmation.iter().all(|c| c.status == CheckStatus::Met)
    {
        Confirmation::Confirmed
    } else {
        Confirmation::NotConfirmed
    };
    for (i, d) in deviations.iter_mut().enumerate() {
        d.id = format!("D-{}", i + 1);
    }

    // Requirement verification matrix.
    let mut req: BTreeMap<String, (Vec<String>, bool)> = BTreeMap::new();
    for r in &proc_.procedure.requirements {
        let e = req.entry(r.clone()).or_insert((Vec::new(), true));
        e.0.push("procedure".into());
        e.1 &= status == Confirmation::Confirmed;
    }
    for s in &steps {
        for r in &s.requirements {
            let e = req.entry(r.clone()).or_insert((Vec::new(), true));
            e.0.push(s.id.clone());
            e.1 &= s.status == Confirmation::Confirmed;
        }
    }
    let requirements: Vec<RequirementResult> = req
        .into_iter()
        .map(|(r, (by, ok))| RequirementResult {
            requirement: r,
            verified_by: by,
            status: if ok {
                "verified by test (as logged)"
            } else {
                "not verified"
            }
            .into(),
        })
        .collect();

    let count = |c: Confirmation| steps.iter().filter(|s| s.status == c).count();
    let all_tcs: Vec<&TcResult> = steps.iter().flat_map(|s| &s.telecommands).collect();
    let all_checks: Vec<&CheckResult> = preconditions
        .iter()
        .chain(steps.iter().flat_map(|s| &s.checks))
        .chain(confirmation.iter())
        .collect();
    let summary = Summary {
        steps: steps.len(),
        confirmed: count(Confirmation::Confirmed),
        not_confirmed: count(Confirmation::NotConfirmed),
        aborted: count(Confirmation::Aborted),
        not_executed: count(Confirmation::NotExecuted),
        telecommands_expected: all_tcs.len(),
        telecommands_verified: all_tcs
            .iter()
            .filter(|t| t.status == TcStatus::Verified)
            .count(),
        telecommands_failed: all_tcs
            .iter()
            .filter(|t| t.status == TcStatus::Failed)
            .count(),
        checks: all_checks.len(),
        checks_met: all_checks
            .iter()
            .filter(|c| c.status == CheckStatus::Met)
            .count(),
        deviations: deviations.len(),
        requirements_verified: requirements
            .iter()
            .filter(|r| r.status.starts_with("verified"))
            .count(),
        requirements: requirements.len(),
    };
    let run = RunInfo {
        run_id: start.name.clone(),
        source: start.value.clone(),
        evidence_class,
        operator: log
            .iter()
            .find(|e| e.event == "operator")
            .map(|e| e.name.clone()),
        started_utc: fmt_utc_ms(start.t),
        ended_utc: run_end.map(fmt_utc_ms),
        log_lines: log.len(),
        notes: log
            .iter()
            .filter(|e| e.event == "note")
            .map(|e| {
                format!(
                    "{}: {}",
                    fmt_utc_ms(e.t),
                    if e.detail.is_empty() {
                        &e.name
                    } else {
                        &e.detail
                    }
                )
            })
            .collect(),
    };
    let body = ReportBody {
        schema: SCHEMA.into(),
        procedure: proc_.procedure.clone(),
        procedure_sha256: procedure_sha256.into(),
        log_sha256: log_sha256.into(),
        run,
        preconditions,
        preconditions_met,
        steps,
        confirmation,
        status,
        deviations,
        requirements,
        summary,
    };
    let report_sha256 = body_sha256(&body)?;
    Ok(TestReport {
        label: LABEL.into(),
        body,
        report_sha256,
    })
}

// ------------------------------------------------------------------ rendering

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

impl TestReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("the report serialises")
    }

    /// One line for the command line.
    pub fn summary_line(&self) -> String {
        let b = &self.body;
        let s = &b.summary;
        format!(
            "{} v{} run {} ({}, {}): {}; steps {}/{} confirmed, {} not confirmed, {} aborted, {} not executed; telecommands {}/{} verified; checks {}/{} met; {} deviations; requirements {}/{} verified",
            b.procedure.id,
            b.procedure.version,
            b.run.run_id,
            b.run.source,
            b.run.evidence_class,
            b.status.as_str(),
            s.confirmed,
            s.steps,
            s.not_confirmed,
            s.aborted,
            s.not_executed,
            s.telecommands_verified,
            s.telecommands_expected,
            s.checks_met,
            s.checks,
            s.deviations,
            s.requirements_verified,
            s.requirements
        )
    }

    /// The human-readable test report (Markdown).
    pub fn to_markdown(&self) -> String {
        let b = &self.body;
        let p = &b.procedure;
        let r = &b.run;
        let mut s = String::new();
        s += &format!(
            "# Test report: {} {} (v{})\n\n",
            p.id,
            md_cell(&p.title),
            p.version
        );
        s += &format!(
            "**Result: {}** · evidence class **{}**\n\n",
            b.status.as_str(),
            r.evidence_class
        );
        s += &format!("_{}_\n\n", self.label);
        s += &format!("Report SHA-256 `{}`\n\n", self.report_sha256);
        s += "## 1. Identification and test configuration\n\n| Item | Value |\n|---|---|\n";
        let mut row = |k: &str, v: &str| s += &format!("| {k} | {} |\n", md_cell(v));
        row("Procedure", &format!("{} v{}", p.id, p.version));
        row(
            "System under test",
            p.system_under_test.as_deref().unwrap_or("not stated"),
        );
        row(
            "Test facility",
            p.test_facility.as_deref().unwrap_or("not stated"),
        );
        row("Run", &r.run_id);
        row("Log source", &r.source);
        row("Operator", r.operator.as_deref().unwrap_or("not stated"));
        row("Run start", &r.started_utc);
        row(
            "Run end",
            r.ended_utc.as_deref().unwrap_or("no run_end in the log"),
        );
        row("Procedure file SHA-256", &b.procedure_sha256);
        row("Run log SHA-256", &b.log_sha256);
        let m = &b.summary;
        s += &format!(
            "\n## 2. Summary\n\nSteps: {} confirmed, {} not confirmed, {} aborted, {} not executed (of {}). Telecommands: {} of {} verified, {} failed. Telemetry checks: {} of {} met. Deviations: {}. Requirements: {} of {} verified by test (as logged).\n\n",
            m.confirmed, m.not_confirmed, m.aborted, m.not_executed, m.steps, m.telecommands_verified, m.telecommands_expected, m.telecommands_failed, m.checks_met, m.checks, m.deviations, m.requirements_verified, m.requirements
        );
        let checks_table = |s: &mut String, cs: &[CheckResult]| {
            *s += "| Parameter | Expected | Observed | At | Status |\n|---|---|---|---|---|\n";
            for c in cs {
                *s += &format!(
                    "| {} | {} | {} | {} | {} |\n",
                    md_cell(&c.parameter),
                    md_cell(&c.expected),
                    md_cell(c.observed.as_deref().unwrap_or("—")),
                    c.observed_utc.as_deref().unwrap_or("—"),
                    c.status.as_str()
                );
            }
            *s += "\n";
        };
        s += "## 3. Preconditions body\n\n";
        if b.preconditions.is_empty() {
            s += "None stated.\n\n";
        } else {
            checks_table(&mut s, &b.preconditions);
        }
        s += "## 4. Main body: steps\n\n| Step | Title | Start | End | Confirmation status |\n|---|---|---|---|---|\n";
        for st in &b.steps {
            s += &format!(
                "| {} | {} | {} | {} | {} |\n",
                st.id,
                md_cell(&st.title),
                st.started_utc.as_deref().unwrap_or("—"),
                st.ended_utc.as_deref().unwrap_or("—"),
                st.status.as_str()
            );
        }
        s += "\n";
        for st in &b.steps {
            s += &format!(
                "### Step {}: {} ({})\n\n",
                st.id,
                md_cell(&st.title),
                st.status.as_str()
            );
            if !st.telecommands.is_empty() {
                s += "| Telecommand | Sent | Verification stages | Status |\n|---|---|---|---|\n";
                for t in &st.telecommands {
                    let stages = t
                        .stages
                        .iter()
                        .map(|x| format!("{}: {}", x.stage, x.status))
                        .collect::<Vec<_>>()
                        .join("; ");
                    s += &format!(
                        "| {} | {} | {} | {} |\n",
                        md_cell(&t.name),
                        t.sent_utc.as_deref().unwrap_or("—"),
                        if stages.is_empty() {
                            "—".into()
                        } else {
                            stages
                        },
                        t.status.as_str()
                    );
                }
                s += "\n";
            }
            if !st.checks.is_empty() {
                checks_table(&mut s, &st.checks);
            }
        }
        s += "## 5. Confirmation body\n\n";
        if b.confirmation.is_empty() {
            s += if b.status == Confirmation::Aborted {
                "Not evaluated: the procedure was aborted.\n\n"
            } else {
                "None stated.\n\n"
            };
        } else {
            checks_table(&mut s, &b.confirmation);
        }
        s += "## 6. Deviations and non-conformances\n\n";
        if b.deviations.is_empty() {
            s += "None found in the log.\n\n";
        } else {
            s += "| Id | Kind | Step | Description |\n|---|---|---|---|\n";
            for d in &b.deviations {
                s += &format!(
                    "| {} | {} | {} | {} |\n",
                    d.id,
                    md_cell(&d.kind),
                    d.step.as_deref().unwrap_or("—"),
                    md_cell(&d.text)
                );
            }
            s += "\n";
        }
        s += "## 7. Requirement verification matrix\n\n";
        if b.requirements.is_empty() {
            s += "No requirement is traced in the procedure.\n\n";
        } else {
            s += "| Requirement | Verified by | Status |\n|---|---|---|\n";
            for q in &b.requirements {
                s += &format!(
                    "| {} | {} | {} |\n",
                    md_cell(&q.requirement),
                    q.verified_by.join(", "),
                    q.status
                );
            }
            s += "\n";
        }
        if !r.notes.is_empty() {
            s += "## 8. Notes from the log\n\n";
            for n in &r.notes {
                s += &format!("* {}\n", md_cell(n));
            }
            s += "\n";
        }
        s += &format!(
            "## Conclusion\n\nAgainst procedure {} v{}, run {} is **{}** as logged. Evidence class {}: {}.\n",
            p.id,
            p.version,
            r.run_id,
            b.status.as_str(),
            r.evidence_class,
            if r.evidence_class == "MEASURED" {
                "the log records a test on hardware or in orbit"
            } else {
                "the log comes from a simulator or is synthetic, so the result says nothing about flight hardware"
            }
        );
        s
    }
}

/// Read a procedure and a run log, and build the report. Returns the report and the two
/// files' bytes.
pub fn from_files(
    procedure: &std::path::Path,
    log: &std::path::Path,
) -> Result<(TestReport, Vec<u8>, Vec<u8>), KshanaError> {
    use crate::mbse::evidence::sha256_hex;
    let pb = std::fs::read(procedure).map_err(|e| err(format!("{}: {e}", procedure.display())))?;
    let lb = std::fs::read(log).map_err(|e| err(format!("{}: {e}", log.display())))?;
    let p = Procedure::parse(&super::utf8("procedure", &pb)?)?;
    let l = parse_log(&super::utf8("run log", &lb)?)?;
    let r = build(&p, &l, &sha256_hex(&pb), &sha256_hex(&lb))?;
    Ok((r, pb, lb))
}
