// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The verification: each scenario's filter log against the known truth and the
//! pre-registered bars.
//!
//! Per scenario it checks:
//!
//! * **coverage**: the log spans the descent with no gap over `max_gap_s` (else the
//!   scenario is INCOMPLETE);
//! * **detection**: every fault that must be detected is flagged (sensor declared faulty
//!   or no longer used) within `detect_latency_s` of its onset; for a bias, drift, stuck or
//!   saturation fault the onset is when the sensor's error first reaches
//!   `min_detectable_error_m` while it reports;
//! * **faulty data re-accepted**: after detection, the time the filter took the faulty
//!   sensor back while its error was still at least `min_detectable_error_m`;
//! * **recovery**: after a transient fault, the sensor is used again within `recovery_s`;
//! * **false rejection**: the time a truthful sensor (in range, no fault, past any recovery
//!   window) was declared faulty or left unused, against `false_reject_max_s` (the ispace
//!   Mission 1 failure mode, stressed by terrain steps);
//! * **configuration**: a sensor that should be reporting but is `off` for the whole
//!   descent (the IM-1 failure mode, when not injected on purpose);
//! * **consistency**: the share of epochs with |altitude error| <= `sigma_k` x the
//!   filter's stated sigma;
//! * **low altitude**: the largest altitude and vertical-speed errors below `low_alt_m`.

use serde::Serialize;

use super::campaign::{Bars, Campaign, Fault, Scenario, Sensor};
use super::faults::{first_in_range, hat, sample_times, systematic_error, Expect, FaultType};
use super::log::{self, FilterLog, Health};
use super::{LABEL, VERDICT_SCHEMA};
use crate::mbse::evidence::sha256_hex;

/// A check's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Status {
    Pass,
    Fail,
    NotApplicable,
}

/// A scenario's or the campaign's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Pass,
    Fail,
    Incomplete,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Incomplete => "INCOMPLETE",
        }
    }
}

/// One check.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub check: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fault: Option<&'static str>,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bar: Option<f64>,
    pub unit: &'static str,
    pub note: String,
}

/// Summary figures of one scenario.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Metrics {
    pub epochs_used: usize,
    pub alt_error_rms_m: f64,
    pub alt_error_max_abs_m: f64,
    pub consistency_fraction: f64,
}

/// One scenario's result.
#[derive(Debug, Clone, Serialize)]
pub struct ScenarioResult {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incident: Option<String>,
    pub verdict: Verdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_sha256: Option<String>,
    pub touchdown_s: f64,
    pub metrics: Metrics,
    pub checks: Vec<Check>,
    pub problems: Vec<String>,
    pub ignored_columns: Vec<String>,
}

/// How the run relates to a pre-registration.
#[derive(Debug, Clone, Serialize)]
pub struct PreregStatus {
    pub preregistration_sha256: String,
    /// Always true: a campaign that differs from its pre-registration is refused.
    pub campaign_matches: bool,
    /// Whether the regenerated streams hash as pre-registered (a difference means another
    /// generator build; the truth, and so the verdict, does not depend on the noise).
    pub streams_match: bool,
}

/// A log handed to the verification.
#[derive(Debug, Clone)]
pub struct LogInput {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// Counts by verdict.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub scenarios: usize,
    pub pass: usize,
    pub fail: usize,
    pub incomplete: usize,
}

/// The signed body.
#[derive(Debug, Clone, Serialize)]
pub struct Body {
    pub schema: &'static str,
    pub evidence_class: &'static str,
    pub label: &'static str,
    pub campaign_title: String,
    pub campaign_sha256: String,
    pub system_under_test: String,
    pub system_version: String,
    pub test_setup: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preregistration: Option<PreregStatus>,
    pub bars: Bars,
    pub summary: Summary,
    pub verdict: Verdict,
    pub scenarios: Vec<ScenarioResult>,
}

/// The verification document (`verdict.json`).
#[derive(Debug, Clone, Serialize)]
pub struct Verification {
    pub body: Body,
    /// SHA-256 of the canonical JSON of `body`.
    pub verdict_sha256: String,
}

const EPS: f64 = 1e-9;

/// The time each epoch's state holds: until the next epoch, the last one for none.
fn held(epochs: &[log::Epoch]) -> Vec<f64> {
    let mut v: Vec<f64> = epochs.windows(2).map(|w| w[1].t_s - w[0].t_s).collect();
    v.push(0.0);
    v
}

/// The verification of one scenario's log.
fn verify_scenario(c: &Campaign, sc: &Scenario, log: &FilterLog) -> (Vec<Check>, Metrics, bool) {
    let b = &c.bars;
    let td = c.descent.touchdown_s();
    let epochs: Vec<log::Epoch> = log
        .epochs
        .iter()
        .filter(|e| e.t_s <= td + EPS)
        .cloned()
        .collect();
    let mut checks = Vec::new();
    let mut complete = true;

    // Coverage.
    let max_gap = epochs
        .windows(2)
        .map(|w| w[1].t_s - w[0].t_s)
        .fold(0.0_f64, f64::max);
    let (first, last) = match (epochs.first(), epochs.last()) {
        (Some(f), Some(l)) => (f.t_s, l.t_s),
        _ => (f64::INFINITY, f64::NEG_INFINITY),
    };
    let worst = max_gap.max(first).max(td - last);
    let covered = worst <= b.max_gap_s + EPS;
    complete &= covered;
    checks.push(Check {
        check: "coverage",
        sensor: None,
        fault: None,
        status: if covered { Status::Pass } else { Status::Fail },
        measured: Some(worst),
        bar: Some(b.max_gap_s),
        unit: "s",
        note: format!(
            "{} epochs from {first:.2} s to {last:.2} s; touchdown at {td:.2} s",
            epochs.len()
        ),
    });
    if epochs.is_empty() {
        return (checks, Metrics::default(), false);
    }
    let dt = held(&epochs);

    for (si, s) in c.sensors.iter().enumerate() {
        let faults: Vec<&Fault> = sc.faults.iter().filter(|f| f.sensor == s.id).collect();
        for f in &faults {
            if f.expect() == Expect::Detect {
                checks.extend(detection_checks(c, sc, s, si, f, &epochs, &dt));
            }
            if f.recover(td) {
                checks.push(recovery_check(c, sc, s, si, f, &faults, &epochs));
            }
        }
        checks.extend(truthful_checks(c, sc, s, si, &faults, &epochs, &dt));
    }

    // Consistency and low altitude.
    let mut inside = 0usize;
    let (mut sq, mut max_abs) = (0.0_f64, 0.0_f64);
    let (mut low_alt, mut low_vz, mut low_n) = (0.0_f64, 0.0_f64, 0usize);
    let (mut vz_inside, mut vz_n) = (0usize, 0usize);
    for e in &epochs {
        let tr = c.descent.truth(e.t_s);
        let ea = e.alt_est_m - tr.alt_m;
        let ev = e.vz_est_mps - tr.vz_mps;
        if ea.abs() <= b.sigma_k * e.alt_sigma_m {
            inside += 1;
        }
        if let Some(vs) = e.vz_sigma_mps {
            vz_n += 1;
            if ev.abs() <= b.sigma_k * vs {
                vz_inside += 1;
            }
        }
        sq += ea * ea;
        max_abs = max_abs.max(ea.abs());
        if tr.alt_m <= b.low_alt_m {
            low_n += 1;
            low_alt = low_alt.max(ea.abs());
            low_vz = low_vz.max(ev.abs());
        }
    }
    let n = epochs.len();
    let frac = inside as f64 / n as f64;
    let pass = |ok: bool| if ok { Status::Pass } else { Status::Fail };
    checks.push(Check {
        check: "consistency",
        sensor: None,
        fault: None,
        status: pass(frac + EPS >= b.consistency_min_fraction),
        measured: Some(frac),
        bar: Some(b.consistency_min_fraction),
        unit: "fraction",
        note: format!(
            "share of epochs with |altitude error| <= {} x stated sigma",
            b.sigma_k
        ),
    });
    if vz_n > 0 {
        let vf = vz_inside as f64 / vz_n as f64;
        checks.push(Check {
            check: "vz_consistency",
            sensor: None,
            fault: None,
            status: pass(vf + EPS >= b.consistency_min_fraction),
            measured: Some(vf),
            bar: Some(b.consistency_min_fraction),
            unit: "fraction",
            note: format!(
                "share of the {vz_n} epochs with a vz_sigma_mps where |vertical-speed error| <= {} x stated sigma",
                b.sigma_k
            ),
        });
    }
    for (check, measured, bar, unit, what) in [
        (
            "low_alt_error",
            low_alt,
            b.low_alt_error_max_m,
            "m",
            "largest |altitude error|",
        ),
        (
            "low_alt_vz_error",
            low_vz,
            b.low_alt_vz_error_max_mps,
            "m/s",
            "largest |vertical-speed error|",
        ),
    ] {
        checks.push(Check {
            check,
            sensor: None,
            fault: None,
            status: if low_n == 0 {
                Status::NotApplicable
            } else {
                pass(measured <= bar + EPS)
            },
            measured: (low_n > 0).then_some(measured),
            bar: Some(bar),
            unit,
            note: format!(
                "{what} over the {low_n} epochs below {} m true altitude",
                b.low_alt_m
            ),
        });
    }
    let metrics = Metrics {
        epochs_used: n,
        alt_error_rms_m: (sq / n as f64).sqrt(),
        alt_error_max_abs_m: max_abs,
        consistency_fraction: frac,
    };
    (checks, metrics, complete)
}

/// When the detection clock of a fault starts, if it does.
pub fn onset(c: &Campaign, sc: &Scenario, s: &Sensor, f: &Fault) -> Option<f64> {
    let td = c.descent.touchdown_s();
    let end = f.end(td);
    match f.fault_type {
        FaultType::NeverEnabled => first_in_range(c, sc, s, 0.0, td),
        FaultType::Bias | FaultType::Drift | FaultType::Stuck | FaultType::Saturation => {
            sample_times(c)
                .into_iter()
                .filter(|&t| t >= f.t_start_s - EPS && t < end)
                .find(|&t| {
                    s.in_range(hat(c, sc, t))
                        && systematic_error(c, sc, f, t).abs() >= c.bars.min_detectable_error_m
                })
        }
        _ => first_in_range(c, sc, s, f.t_start_s, end),
    }
}

fn detection_checks(
    c: &Campaign,
    sc: &Scenario,
    s: &Sensor,
    si: usize,
    f: &Fault,
    epochs: &[log::Epoch],
    dt: &[f64],
) -> Vec<Check> {
    let b = &c.bars;
    let td = c.descent.touchdown_s();
    let end = f.end(td);
    let base = |check, status, measured, bar, note: String| Check {
        check,
        sensor: Some(s.id.clone()),
        fault: Some(f.fault_type.name()),
        status,
        measured,
        bar,
        unit: "s",
        note,
    };
    let Some(t0) = onset(c, sc, s, f) else {
        return vec![base(
            "detection",
            Status::NotApplicable,
            None,
            Some(b.detect_latency_s),
            "the fault never reaches a detectable state while the sensor is in range".into(),
        )];
    };
    let det = epochs.iter().position(|e| {
        // A sensor left unused because it has no return (out of range) or after the
        // fault has ended is not a detection.
        e.t_s >= t0 - EPS
            && e.t_s < end + b.detect_latency_s
            && s.in_range(hat(c, sc, e.t_s))
            && !e.sensors[si].accepted()
    });
    let mut out = Vec::new();
    match det {
        Some(i) => {
            let lat = epochs[i].t_s - t0;
            let declared = epochs[i].sensors[si].health == Health::Fault;
            out.push(base(
                "detection",
                if lat <= b.detect_latency_s + EPS {
                    Status::Pass
                } else {
                    Status::Fail
                },
                Some(lat),
                Some(b.detect_latency_s),
                format!(
                    "onset {t0:.2} s; flagged at {:.2} s ({})",
                    epochs[i].t_s,
                    if declared {
                        "declared faulty"
                    } else {
                        "no longer used"
                    }
                ),
            ));
            if f.fault_type.reports() {
                let mut back = 0.0;
                for (j, e) in epochs.iter().enumerate().skip(i) {
                    if e.t_s >= end {
                        break;
                    }
                    let bad = match f.fault_type {
                        FaultType::Spikes | FaultType::NoiseBurst => true,
                        _ => systematic_error(c, sc, f, e.t_s).abs() >= b.min_detectable_error_m,
                    };
                    if bad && e.sensors[si].accepted() {
                        back += dt[j].min(end - e.t_s);
                    }
                }
                out.push(base(
                    "faulty_reaccepted",
                    if back <= b.detect_latency_s + EPS {
                        Status::Pass
                    } else {
                        Status::Fail
                    },
                    Some(back),
                    Some(b.detect_latency_s),
                    "time the faulty sensor was used again after detection, while its error was still detectable".into(),
                ));
            }
        }
        None => out.push(base(
            "detection",
            Status::Fail,
            None,
            Some(b.detect_latency_s),
            format!(
                "onset {t0:.2} s; never flagged while the fault was active and the sensor in range"
            ),
        )),
    }
    out
}

fn recovery_check(
    c: &Campaign,
    sc: &Scenario,
    s: &Sensor,
    si: usize,
    f: &Fault,
    faults: &[&Fault],
    epochs: &[log::Epoch],
) -> Check {
    let b = &c.bars;
    let td = c.descent.touchdown_s();
    let end = f.end(td);
    let base = |status, measured, note: String| Check {
        check: "recovery",
        sensor: Some(s.id.clone()),
        fault: Some(f.fault_type.name()),
        status,
        measured,
        bar: Some(b.recovery_s),
        unit: "s",
        note,
    };
    let Some(t0) = first_in_range(c, sc, s, end, td) else {
        return base(
            Status::NotApplicable,
            None,
            "the sensor is out of range after the fault".into(),
        );
    };
    if faults
        .iter()
        .any(|o| !std::ptr::eq(*o, f) && o.active(t0, td))
    {
        return base(
            Status::NotApplicable,
            None,
            "another fault on the sensor is active when this one ends".into(),
        );
    }
    match epochs
        .iter()
        .find(|e| e.t_s >= t0 - EPS && e.sensors[si].accepted())
    {
        Some(e) => {
            let lat = e.t_s - t0;
            base(
                if lat <= b.recovery_s + EPS {
                    Status::Pass
                } else {
                    Status::Fail
                },
                Some(lat),
                format!(
                    "truthful again from {t0:.2} s; used again at {:.2} s",
                    e.t_s
                ),
            )
        }
        None => base(
            Status::Fail,
            None,
            format!("truthful again from {t0:.2} s; never used again before touchdown"),
        ),
    }
}

/// Whether sensor `s` is truthful at `t`: in range now and `recovery_s` ago, with no fault
/// active or latched and past every fault's recovery window.
fn truthful(c: &Campaign, sc: &Scenario, s: &Sensor, faults: &[&Fault], t: f64) -> bool {
    let td = c.descent.touchdown_s();
    let g = c.bars.recovery_s;
    if !s.in_range(hat(c, sc, t)) || !s.in_range(hat(c, sc, (t - g).max(0.0))) {
        return false;
    }
    faults.iter().all(|f| {
        if f.active(t, td) {
            return false;
        }
        let end = f.end(td);
        if t < end {
            return true;
        }
        if !f.recover(td) {
            return false;
        }
        let back = first_in_range(c, sc, s, end, td).unwrap_or(end);
        t >= back + g
    })
}

fn truthful_checks(
    c: &Campaign,
    sc: &Scenario,
    s: &Sensor,
    si: usize,
    faults: &[&Fault],
    epochs: &[log::Epoch],
    dt: &[f64],
) -> Vec<Check> {
    let b = &c.bars;
    let (mut good, mut rejected, mut declared, mut off_all) = (0usize, 0.0, 0.0, true);
    for (i, e) in epochs.iter().enumerate() {
        if !truthful(c, sc, s, faults, e.t_s) {
            continue;
        }
        good += 1;
        let st = e.sensors[si];
        off_all &= st.health == Health::Off;
        if !st.accepted() {
            rejected += dt[i];
            if st.health == Health::Fault {
                declared += dt[i];
            }
        }
    }
    let mut out = Vec::new();
    let na = good == 0;
    out.push(Check {
        check: "false_rejection",
        sensor: Some(s.id.clone()),
        fault: None,
        status: if na {
            Status::NotApplicable
        } else if rejected <= b.false_reject_max_s + EPS {
            Status::Pass
        } else {
            Status::Fail
        },
        measured: (!na).then_some(rejected),
        bar: Some(b.false_reject_max_s),
        unit: "s",
        note: if na {
            "the sensor is never truthful in this scenario".into()
        } else {
            format!(
                "time a truthful sensor was rejected over {good} epochs; declared faulty for {declared:.2} s of it{}",
                if sc.terrain.is_empty() { "" } else { " (terrain steps in this scenario)" }
            )
        },
    });
    let injected = faults
        .iter()
        .any(|f| f.fault_type == FaultType::NeverEnabled);
    if !injected {
        out.push(Check {
            check: "configuration",
            sensor: Some(s.id.clone()),
            fault: None,
            status: if na {
                Status::NotApplicable
            } else if off_all {
                Status::Fail
            } else {
                Status::Pass
            },
            measured: None,
            bar: None,
            unit: "",
            note: if na {
                "the sensor is never truthful in this scenario".into()
            } else if off_all {
                "the filter shows the sensor off whenever it should report: never enabled (configuration)".into()
            } else {
                "the sensor is enabled while it should report".into()
            },
        });
    }
    out
}

/// Run the verification over every scenario of the campaign. `logs[i]` is scenario `i`'s
/// log, if one was given.
pub fn verify(
    c: &Campaign,
    campaign_sha256: &str,
    logs: &[Option<LogInput>],
    preregistration: Option<PreregStatus>,
) -> Verification {
    let td = c.descent.touchdown_s();
    let mut scenarios = Vec::new();
    let mut summary = Summary::default();
    for (i, sc) in c.scenarios.iter().enumerate() {
        let mut r = ScenarioResult {
            id: sc.id.clone(),
            title: sc.title.clone(),
            incident: sc.incident.clone(),
            verdict: Verdict::Incomplete,
            log_file: None,
            log_sha256: None,
            touchdown_s: td,
            metrics: Metrics::default(),
            checks: Vec::new(),
            problems: Vec::new(),
            ignored_columns: Vec::new(),
        };
        match logs.get(i).and_then(Option::as_ref) {
            None => r.problems.push(format!("no log {}.csv", sc.id)),
            Some(l) => {
                r.log_file = Some(l.file_name.clone());
                r.log_sha256 = Some(sha256_hex(&l.bytes));
                let parsed = std::str::from_utf8(&l.bytes)
                    .map_err(|_| "the log is not UTF-8".to_string())
                    .and_then(|t| log::parse(t, c).map_err(|e| e.to_string()));
                match parsed {
                    Err(e) => r.problems.push(e),
                    Ok(log) => {
                        let (checks, metrics, complete) = verify_scenario(c, sc, &log);
                        r.verdict = if checks
                            .iter()
                            .any(|k| k.status == Status::Fail && k.check != "coverage")
                        {
                            Verdict::Fail
                        } else if !complete {
                            Verdict::Incomplete
                        } else {
                            Verdict::Pass
                        };
                        r.checks = checks;
                        r.metrics = metrics;
                        r.ignored_columns = log.ignored_columns;
                    }
                }
            }
        }
        summary.scenarios += 1;
        match r.verdict {
            Verdict::Pass => summary.pass += 1,
            Verdict::Fail => summary.fail += 1,
            Verdict::Incomplete => summary.incomplete += 1,
        }
        scenarios.push(r);
    }
    let verdict = if summary.fail > 0 {
        Verdict::Fail
    } else if summary.incomplete > 0 {
        Verdict::Incomplete
    } else {
        Verdict::Pass
    };
    let sut = &c.system_under_test;
    let body = Body {
        schema: VERDICT_SCHEMA,
        evidence_class: "MODELLED",
        label: LABEL,
        campaign_title: c.title.clone(),
        campaign_sha256: campaign_sha256.to_string(),
        system_under_test: sut.name.clone(),
        system_version: sut.version.clone(),
        test_setup: sut.test_setup.clone(),
        provider: sut.provider.clone(),
        preregistration,
        bars: c.bars.clone(),
        summary,
        verdict,
        scenarios,
    };
    let canon = serde_json::to_string(&body).expect("the verdict serialises");
    Verification {
        verdict_sha256: sha256_hex(canon.as_bytes()),
        body,
    }
}

fn fmt_opt(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.3}")).unwrap_or_else(|| "-".into())
}

impl Verification {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("the verdict serialises")
    }

    /// One line per scenario and the campaign verdict.
    pub fn summary(&self) -> String {
        let b = &self.body;
        let mut s = String::new();
        for r in &b.scenarios {
            let failed: Vec<String> = r
                .checks
                .iter()
                .filter(|k| k.status == Status::Fail)
                .map(|k| match &k.sensor {
                    Some(id) => format!("{}({id})", k.check),
                    None => k.check.to_string(),
                })
                .chain(r.problems.iter().cloned())
                .collect();
            s.push_str(&format!(
                "{}: {}{}\n",
                r.id,
                r.verdict.as_str(),
                if failed.is_empty() {
                    String::new()
                } else {
                    format!(" - {}", failed.join(", "))
                }
            ));
        }
        s.push_str(&format!(
            "campaign: {} ({} pass, {} fail, {} incomplete of {}); evidence class {}",
            b.verdict.as_str(),
            b.summary.pass,
            b.summary.fail,
            b.summary.incomplete,
            b.summary.scenarios,
            b.evidence_class
        ));
        s
    }

    /// The verdict as Markdown.
    pub fn to_markdown(&self) -> String {
        let b = &self.body;
        let mut s = format!(
            "# Landing-navigation fault-handling verification\n\n\
             **Verdict: {}** ({} pass, {} fail, {} incomplete of {} scenarios). Evidence class: **{}**.\n\n\
             > {}\n\n\
             * Campaign: {} (sha256 `{}`)\n\
             * System under test: {} {} ({}{})\n\
             * Pre-registration: {}\n\
             * Verdict sha256: `{}`\n\n",
            b.verdict.as_str(),
            b.summary.pass,
            b.summary.fail,
            b.summary.incomplete,
            b.summary.scenarios,
            b.evidence_class,
            b.label,
            b.campaign_title,
            b.campaign_sha256,
            b.system_under_test,
            b.system_version,
            b.test_setup,
            b.provider
                .as_ref()
                .map(|p| format!(", provider {p}"))
                .unwrap_or_default(),
            match &b.preregistration {
                Some(p) => format!(
                    "`{}` (campaign matches; streams {})",
                    p.preregistration_sha256,
                    if p.streams_match {
                        "match"
                    } else {
                        "differ: another generator build"
                    }
                ),
                None => "none given (the bars were not shown to be fixed in advance)".into(),
            },
            self.verdict_sha256
        );
        let bars = &b.bars;
        s.push_str(&format!(
            "Pass bars: detection <= {} s after onset (errors >= {} m); recovery <= {} s; false rejection <= {} s per sensor; \
             >= {:.0}% of epochs within {} sigma; below {} m: |altitude error| <= {} m, |vertical-speed error| <= {} m/s; gaps <= {} s.\n\n",
            bars.detect_latency_s,
            bars.min_detectable_error_m,
            bars.recovery_s,
            bars.false_reject_max_s,
            bars.consistency_min_fraction * 100.0,
            bars.sigma_k,
            bars.low_alt_m,
            bars.low_alt_error_max_m,
            bars.low_alt_vz_error_max_mps,
            bars.max_gap_s
        ));
        for r in &b.scenarios {
            s.push_str(&format!(
                "## {} - {}: {}\n\n",
                r.id,
                r.title,
                r.verdict.as_str()
            ));
            if let Some(i) = &r.incident {
                s.push_str(&format!("Stands for: {i}.\n\n"));
            }
            for p in &r.problems {
                s.push_str(&format!("* Problem: {p}\n"));
            }
            if !r.checks.is_empty() {
                s.push_str(&format!(
                    "Altitude error RMS {:.3} m, largest {:.3} m, {:.1}% within the consistency bound, {} epochs.\n\n\
                     | Check | Sensor | Fault | Status | Measured | Bar | Note |\n|---|---|---|---|---|---|---|\n",
                    r.metrics.alt_error_rms_m,
                    r.metrics.alt_error_max_abs_m,
                    r.metrics.consistency_fraction * 100.0,
                    r.metrics.epochs_used
                ));
                for k in &r.checks {
                    s.push_str(&format!(
                        "| {} | {} | {} | {:?} | {} | {} | {} |\n",
                        k.check,
                        k.sensor.as_deref().unwrap_or("-"),
                        k.fault.unwrap_or("-"),
                        k.status,
                        fmt_opt(k.measured),
                        fmt_opt(k.bar),
                        k.note.replace('|', "\\|")
                    ));
                }
            }
            s.push('\n');
        }
        s
    }
}
