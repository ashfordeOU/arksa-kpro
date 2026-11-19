// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Tracking-gap impact: what a ground-station schedule loses to station outages, and how
//! much other stations' windows recover.
//!
//! Inputs (a study TOML naming three CSV files):
//!
//! * the **schedule**: `pass_id,mission,station,start_utc,end_utc,downlink_kbps,services`
//!   (services separated by `;`, from `TM`, `TC`, `RNG`);
//! * the **outages**: `station,start_utc,end_utc,reason`;
//! * optionally **alternates**: candidate windows on other stations or networks, in the
//!   schedule's layout (outages apply to them too).
//!
//! Per mission, three cases are computed over the study period: as scheduled, with the
//! outages, and with the outages plus the alternates. For each: contact hours, contacts
//! with telecommand, downlinked data and recorder overflow (science data lost) from a
//! constant-rate onboard data model with a finite recorder, the longest gap without
//! contact and every gap beyond the mission's limit, ranging passes per UTC day against the
//! mission's minimum, and the coverage of each critical window (a manoeuvre, an entry, a
//! landing). The impact is the outage case against the schedule; the recovery is the
//! alternates case against the outage case. Everything here is MODELLED: the schedule and
//! the outages are given, and the data model is a constant rate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::csv::Table;
use super::{body_sha256, clip, err, fmt_utc_ms, merge, read_rel, round, subtract, union_len, utc};
use crate::mbse::evidence::sha256_hex;
use kshana::api::KshanaError;

/// Identifies `impact.json`.
pub const SCHEMA: &str = "arksa-ops-tracking-gap/1";

/// The honesty label every impact study carries.
pub const LABEL: &str = "MODELLED - computed from the supplied schedule and outages with a constant-rate onboard data model and a finite recorder. Not a prediction of any network's performance and not a statement by any agency or station operator.";

const SERVICES: &[&str] = &["TM", "TC", "RNG"];

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StudyHead {
    pub name: String,
    pub start_utc: String,
    pub end_utc: String,
    pub schedule: String,
    pub outages: String,
    #[serde(default)]
    pub alternates: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CriticalWindow {
    pub name: String,
    pub start_utc: String,
    pub end_utc: String,
    /// Fraction of the window that must be in contact (default 1).
    #[serde(default)]
    pub min_coverage: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MissionNeeds {
    pub id: String,
    /// Onboard data generated for downlink, kbit/s.
    pub data_generation_kbps: f64,
    pub recorder_capacity_gbit: f64,
    #[serde(default)]
    pub recorder_initial_gbit: f64,
    /// The longest acceptable time without contact, hours.
    #[serde(default)]
    pub max_gap_h: Option<f64>,
    #[serde(default)]
    pub min_ranging_passes_per_day: Option<u32>,
    #[serde(default)]
    pub critical: Vec<CriticalWindow>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Study {
    pub study: StudyHead,
    pub missions: Vec<MissionNeeds>,
}

/// A contact window.
#[derive(Debug, Clone, PartialEq)]
pub struct Pass {
    pub id: String,
    pub mission: String,
    pub station: String,
    pub start: f64,
    pub end: f64,
    pub kbps: f64,
    pub services: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outage {
    pub station: String,
    pub start: f64,
    pub end: f64,
    pub reason: String,
}

pub fn parse_passes(name: &str, text: &str) -> Result<Vec<Pass>, KshanaError> {
    let t = Table::parse(name, text)?;
    let (cm, cs, ca, cb, cr, cv) = (
        t.col("mission")?,
        t.col("station")?,
        t.col("start_utc")?,
        t.col("end_utc")?,
        t.col("downlink_kbps")?,
        t.col("services")?,
    );
    let cid = t.opt_col("pass_id");
    let mut out = Vec::new();
    for (line, r) in &t.rows {
        let (a, b) = (t.time(*line, r, ca)?, t.time(*line, r, cb)?);
        if b <= a {
            return Err(err(format!(
                "{name} line {line}: end_utc is not after start_utc"
            )));
        }
        let kbps = t.num(*line, r, cr)?;
        if kbps < 0.0 {
            return Err(err(format!(
                "{name} line {line}: downlink_kbps is negative"
            )));
        }
        let services: BTreeSet<String> = Table::get(r, cv)
            .split([';', '|', ' '])
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_uppercase())
            .collect();
        if let Some(bad) = services.iter().find(|s| !SERVICES.contains(&s.as_str())) {
            return Err(err(format!(
                "{name} line {line}: service '{bad}' is not one of {}",
                SERVICES.join(", ")
            )));
        }
        out.push(Pass {
            id: cid
                .map(|c| Table::get(r, c).to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{name}:{line}")),
            mission: Table::get(r, cm).to_string(),
            station: Table::get(r, cs).to_string(),
            start: a,
            end: b,
            kbps,
            services,
        });
    }
    Ok(out)
}

pub fn parse_outages(text: &str) -> Result<Vec<Outage>, KshanaError> {
    let t = Table::parse("outages", text)?;
    let (cs, ca, cb) = (t.col("station")?, t.col("start_utc")?, t.col("end_utc")?);
    let cr = t.opt_col("reason");
    let mut out = Vec::new();
    for (line, r) in &t.rows {
        let (a, b) = (t.time(*line, r, ca)?, t.time(*line, r, cb)?);
        if b <= a {
            return Err(err(format!(
                "outages line {line}: end_utc is not after start_utc"
            )));
        }
        out.push(Outage {
            station: Table::get(r, cs).to_string(),
            start: a,
            end: b,
            reason: cr.map(|c| Table::get(r, c).to_string()).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// Apply outages to passes: each pass keeps the pieces outside its station's outages.
fn apply(passes: &[Pass], outages: &[Outage]) -> Vec<Pass> {
    let mut out = Vec::new();
    for p in passes {
        let cut: Vec<(f64, f64)> = outages
            .iter()
            .filter(|o| o.station == p.station)
            .map(|o| (o.start, o.end))
            .collect();
        for (a, b) in subtract((p.start, p.end), &cut) {
            out.push(Pass {
                start: a,
                end: b,
                ..p.clone()
            });
        }
    }
    out
}

/// Recorder model: data generated at `gen_kbps`, downlinked at the highest rate of the
/// contacts in progress, recorder of `cap` Gbit starting at `init`. Returns (downlinked,
/// overflow, end fill) in Gbit.
pub fn simulate(
    contacts: &[(f64, f64, f64)],
    gen_kbps: f64,
    cap: f64,
    init: f64,
    t0: f64,
    t1: f64,
) -> (f64, f64, f64) {
    let mut pts: Vec<f64> = vec![t0, t1];
    for &(a, b, _) in contacts {
        if b > t0 && a < t1 {
            pts.push(a.max(t0));
            pts.push(b.min(t1));
        }
    }
    pts.sort_by(f64::total_cmp);
    pts.dedup();
    let g = gen_kbps * 1e-6; // Gbit/s
    let (mut fill, mut down, mut over) = (init.min(cap), 0.0, 0.0);
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let dt = b - a;
        if dt <= 0.0 {
            continue;
        }
        let mid = 0.5 * (a + b);
        let d = contacts
            .iter()
            .filter(|c| c.0 <= mid && mid < c.1)
            .map(|c| c.2)
            .fold(0.0, f64::max)
            * 1e-6;
        let net = g - d;
        if net >= 0.0 {
            // Downlink at full rate while filling; overflow once full.
            down += d * dt;
            let room = cap - fill;
            let rise = net * dt;
            if rise > room {
                over += rise - room;
                fill = cap;
            } else {
                fill += rise;
            }
        } else {
            // Draining: full rate until empty, then only what is generated.
            let t_empty = fill / -net;
            if t_empty >= dt {
                down += d * dt;
                fill += net * dt;
            } else {
                down += d * t_empty + g * (dt - t_empty);
                fill = 0.0;
            }
        }
    }
    (down, over, fill)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gap {
    pub from_utc: String,
    pub to_utc: String,
    pub hours: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Coverage {
    pub name: String,
    pub fraction: f64,
    pub required: f64,
    pub met: bool,
}

/// One mission under one case.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaseResult {
    pub contact_h: f64,
    pub contacts: usize,
    pub contacts_with_tc: usize,
    pub downlinked_gbit: f64,
    pub overflow_gbit: f64,
    pub recorder_end_gbit: f64,
    pub longest_gap_h: f64,
    pub gaps_over_limit: Vec<Gap>,
    pub ranging_days_below_minimum: Vec<String>,
    pub critical: Vec<Coverage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactedPass {
    pub pass_id: String,
    pub station: String,
    pub scheduled_utc: String,
    pub lost_min: f64,
    /// `lost` (no time left) or `shortened`.
    pub effect: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MissionImpact {
    pub mission: String,
    pub scheduled: CaseResult,
    pub with_outages: CaseResult,
    pub with_alternates: Option<CaseResult>,
    /// Passes hit by an outage ("impacted N times").
    pub impacted_passes: Vec<ImpactedPass>,
    pub contact_lost_h: f64,
    /// Overflow with the outages minus overflow as scheduled.
    pub science_lost_gbit: f64,
    /// Recorder backlog at the end, with the outages minus as scheduled.
    pub backlog_added_gbit: f64,
    pub new_gap_violations: usize,
    pub critical_windows_lost: Vec<String>,
    /// Share of the science loss the alternates recover (when any loss and alternates).
    pub recovered_fraction: Option<f64>,
    pub findings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StationSummary {
    pub station: String,
    pub outage_h: f64,
    pub passes_impacted: usize,
    pub missions_impacted: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactBody {
    pub schema: String,
    pub study: StudyHead,
    pub missions_needs: Vec<MissionNeeds>,
    pub inputs: BTreeMap<String, String>,
    pub missions: Vec<MissionImpact>,
    pub stations: Vec<StationSummary>,
    pub evidence_class: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Impact {
    pub label: String,
    pub body: ImpactBody,
    pub impact_sha256: String,
}

fn case(m: &MissionNeeds, passes: &[&Pass], t0: f64, t1: f64) -> Result<CaseResult, KshanaError> {
    let iv: Vec<(f64, f64)> = clip(
        &passes.iter().map(|p| (p.start, p.end)).collect::<Vec<_>>(),
        t0,
        t1,
    );
    let union = merge(&iv);
    let contacts: Vec<(f64, f64, f64)> = passes.iter().map(|p| (p.start, p.end, p.kbps)).collect();
    let (down, over, end) = simulate(
        &contacts,
        m.data_generation_kbps,
        m.recorder_capacity_gbit,
        m.recorder_initial_gbit,
        t0,
        t1,
    );
    // Gaps between contacts, including from the study start and to its end.
    let mut gaps = Vec::new();
    let mut cursor = t0;
    for &(a, b) in &union {
        if a > cursor {
            gaps.push((cursor, a));
        }
        cursor = cursor.max(b);
    }
    if t1 > cursor {
        gaps.push((cursor, t1));
    }
    let longest = gaps.iter().map(|(a, b)| b - a).fold(0.0, f64::max) / 3600.0;
    let gaps_over_limit = match m.max_gap_h {
        Some(lim) => gaps
            .iter()
            .filter(|(a, b)| (b - a) / 3600.0 > lim)
            .map(|&(a, b)| Gap {
                from_utc: fmt_utc_ms(a),
                to_utc: fmt_utc_ms(b),
                hours: round((b - a) / 3600.0, 3),
            })
            .collect(),
        None => Vec::new(),
    };
    let mut ranging_days_below_minimum = Vec::new();
    if let Some(min) = m.min_ranging_passes_per_day {
        let mut day = (t0 / 86_400.0).floor() * 86_400.0;
        while day < t1 {
            let (d0, d1) = (day.max(t0), (day + 86_400.0).min(t1));
            // Only whole days are judged.
            if d1 - d0 >= 86_400.0 {
                let n = passes
                    .iter()
                    .filter(|p| p.services.contains("RNG") && p.start >= d0 && p.start < d1)
                    .count();
                if (n as u32) < min {
                    ranging_days_below_minimum
                        .push(format!("{} ({n} of {min})", &fmt_utc_ms(d0)[..10]));
                }
            }
            day += 86_400.0;
        }
    }
    let mut critical = Vec::new();
    for c in &m.critical {
        let (a, b) = (
            utc("critical start_utc", &c.start_utc)?,
            utc("critical end_utc", &c.end_utc)?,
        );
        let covered = union_len(&clip(&union, a, b));
        let fraction = if b > a { covered / (b - a) } else { 0.0 };
        let required = c.min_coverage.unwrap_or(1.0);
        critical.push(Coverage {
            name: c.name.clone(),
            fraction: round(fraction, 4),
            required,
            met: fraction + 1e-9 >= required,
        });
    }
    Ok(CaseResult {
        contact_h: round(union_len(&union) / 3600.0, 4),
        contacts: passes.iter().filter(|p| p.end > t0 && p.start < t1).count(),
        contacts_with_tc: passes
            .iter()
            .filter(|p| p.end > t0 && p.start < t1 && p.services.contains("TC"))
            .count(),
        downlinked_gbit: round(down, 6),
        overflow_gbit: round(over, 6),
        recorder_end_gbit: round(end, 6),
        longest_gap_h: round(longest, 3),
        gaps_over_limit,
        ranging_days_below_minimum,
        critical,
    })
}

/// Compute the impact study.
pub fn assess(
    study: &Study,
    schedule: &[Pass],
    outages: &[Outage],
    alternates: Option<&[Pass]>,
    inputs: BTreeMap<String, String>,
) -> Result<Impact, KshanaError> {
    let t0 = utc("study.start_utc", &study.study.start_utc)?;
    let t1 = utc("study.end_utc", &study.study.end_utc)?;
    if t1 <= t0 {
        return Err(err("study.end_utc is not after study.start_utc"));
    }
    if study.missions.is_empty() {
        return Err(err("the study has no [[missions]]"));
    }
    let mut ids = BTreeSet::new();
    for m in &study.missions {
        if !ids.insert(m.id.as_str()) {
            return Err(err(format!("mission {} is listed twice", m.id)));
        }
        let bad = |x: f64, lo: f64, strict: bool| !x.is_finite() || x < lo || (strict && x == lo);
        if bad(m.data_generation_kbps, 0.0, false)
            || bad(m.recorder_capacity_gbit, 0.0, true)
            || bad(m.recorder_initial_gbit, 0.0, false)
            || m.recorder_initial_gbit > m.recorder_capacity_gbit
        {
            return Err(err(format!(
                "mission {}: data_generation_kbps >= 0, recorder_capacity_gbit > 0 and 0 <= recorder_initial_gbit <= capacity are required",
                m.id
            )));
        }
        if m.max_gap_h.is_some_and(|g| g.is_nan() || g <= 0.0) {
            return Err(err(format!("mission {}: max_gap_h must be > 0", m.id)));
        }
    }
    let mut notes = Vec::new();
    let unknown: BTreeSet<&str> = schedule
        .iter()
        .chain(alternates.unwrap_or(&[]).iter())
        .map(|p| p.mission.as_str())
        .filter(|m| !ids.contains(m))
        .collect();
    if !unknown.is_empty() {
        notes.push(format!(
            "passes for missions not in the study were ignored: {}",
            unknown.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    let actual = apply(schedule, outages);
    let alt_actual = alternates.map(|a| apply(a, outages));
    let mut missions = Vec::new();
    for m in &study.missions {
        let sched: Vec<&Pass> = schedule.iter().filter(|p| p.mission == m.id).collect();
        let act: Vec<&Pass> = actual.iter().filter(|p| p.mission == m.id).collect();
        let s = case(m, &sched, t0, t1)?;
        let a = case(m, &act, t0, t1)?;
        let r = match &alt_actual {
            Some(alt) => {
                let mut both = act.clone();
                both.extend(alt.iter().filter(|p| p.mission == m.id));
                Some(case(m, &both, t0, t1)?)
            }
            None => None,
        };
        let mut impacted = Vec::new();
        for p in &sched {
            if p.end <= t0 || p.start >= t1 {
                continue;
            }
            let hit: Vec<&Outage> = outages
                .iter()
                .filter(|o| o.station == p.station && o.start < p.end && p.start < o.end)
                .collect();
            if hit.is_empty() {
                continue;
            }
            let cut: Vec<(f64, f64)> = hit.iter().map(|o| (o.start, o.end)).collect();
            let left: f64 = subtract((p.start, p.end), &cut)
                .iter()
                .map(|(x, y)| y - x)
                .sum();
            impacted.push(ImpactedPass {
                pass_id: p.id.clone(),
                station: p.station.clone(),
                scheduled_utc: format!("{} to {}", fmt_utc_ms(p.start), fmt_utc_ms(p.end)),
                lost_min: round((p.end - p.start - left) / 60.0, 2),
                effect: if left <= 0.0 { "lost" } else { "shortened" }.into(),
                reasons: hit
                    .iter()
                    .map(|o| o.reason.clone())
                    .filter(|x| !x.is_empty())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            });
        }
        let science_lost = round((a.overflow_gbit - s.overflow_gbit).max(0.0), 6);
        let backlog = round(a.recorder_end_gbit - s.recorder_end_gbit, 6);
        let new_gaps = a
            .gaps_over_limit
            .len()
            .saturating_sub(s.gaps_over_limit.len());
        let crit_lost: Vec<String> = a
            .critical
            .iter()
            .zip(&s.critical)
            .filter(|(x, y)| y.met && !x.met)
            .map(|(x, _)| x.name.clone())
            .collect();
        let recovered_fraction = match &r {
            Some(r) if science_lost > 0.0 => Some(round(
                ((a.overflow_gbit - r.overflow_gbit) / (a.overflow_gbit - s.overflow_gbit))
                    .clamp(0.0, 1.0),
                4,
            )),
            _ => None,
        };
        let mut findings = Vec::new();
        if !impacted.is_empty() {
            findings.push(format!(
                "{} scheduled passes hit by outages ({} lost, {} shortened); {:.2} h of contact lost",
                impacted.len(),
                impacted.iter().filter(|p| p.effect == "lost").count(),
                impacted.iter().filter(|p| p.effect == "shortened").count(),
                s.contact_h - a.contact_h
            ));
        }
        if s.overflow_gbit > 0.0 {
            findings.push(format!(
                "the schedule itself overflows the recorder by {:.3} Gbit before any outage",
                s.overflow_gbit
            ));
        }
        if science_lost > 0.0 {
            findings.push(format!("{science_lost:.3} Gbit of onboard data lost to recorder overflow because of the outages"));
        }
        if backlog > 0.0 {
            findings.push(format!(
                "{backlog:.3} Gbit more data waiting on board at the end of the period"
            ));
        }
        if new_gaps > 0 {
            findings.push(format!(
                "{new_gaps} more gaps beyond the {} h limit (longest {:.1} h, was {:.1} h)",
                m.max_gap_h.unwrap_or_default(),
                a.longest_gap_h,
                s.longest_gap_h
            ));
        }
        let rng_added = a
            .ranging_days_below_minimum
            .len()
            .saturating_sub(s.ranging_days_below_minimum.len());
        if rng_added > 0 {
            findings.push(format!("{rng_added} more days below the ranging minimum"));
        }
        for c in &crit_lost {
            findings.push(format!(
                "critical window '{c}' no longer covered as required"
            ));
        }
        if let (Some(r), Some(f)) = (&r, recovered_fraction) {
            findings.push(format!(
                "the alternates recover {:.0}% of the data loss (overflow {:.3} Gbit with them)",
                f * 100.0,
                r.overflow_gbit
            ));
        }
        if findings.is_empty() {
            findings.push("no impact on this mission".into());
        }
        missions.push(MissionImpact {
            mission: m.id.clone(),
            contact_lost_h: round(s.contact_h - a.contact_h, 4),
            scheduled: s,
            with_outages: a,
            with_alternates: r,
            impacted_passes: impacted,
            science_lost_gbit: science_lost,
            backlog_added_gbit: backlog,
            new_gap_violations: new_gaps,
            critical_windows_lost: crit_lost,
            recovered_fraction,
            findings,
        });
    }
    let mut stations: BTreeMap<String, StationSummary> = BTreeMap::new();
    for o in outages {
        let e = stations.entry(o.station.clone()).or_insert(StationSummary {
            station: o.station.clone(),
            outage_h: 0.0,
            passes_impacted: 0,
            missions_impacted: Vec::new(),
        });
        e.outage_h += (o.end.min(t1) - o.start.max(t0)).max(0.0) / 3600.0;
    }
    for m in &missions {
        for p in &m.impacted_passes {
            if let Some(e) = stations.get_mut(&p.station) {
                e.passes_impacted += 1;
                if !e.missions_impacted.contains(&m.mission) {
                    e.missions_impacted.push(m.mission.clone());
                }
            }
        }
    }
    let stations = stations
        .into_values()
        .map(|mut s| {
            s.outage_h = round(s.outage_h, 3);
            s
        })
        .collect();
    let body = ImpactBody {
        schema: SCHEMA.into(),
        study: study.study.clone(),
        missions_needs: study.missions.clone(),
        inputs,
        missions,
        stations,
        evidence_class: "MODELLED".into(),
        notes,
    };
    let impact_sha256 = body_sha256(&body)?;
    Ok(Impact {
        label: LABEL.into(),
        body,
        impact_sha256,
    })
}

/// Input files kept beside a study (name, bytes).
pub type InputFiles = Vec<(String, Vec<u8>)>;

/// Read a study and its files (relative to `base`). Returns the impact and every input
/// file to keep beside it (name, bytes).
pub fn run(study_text: &str, base: &Path) -> Result<(Impact, InputFiles), KshanaError> {
    let study: Study =
        toml::from_str(study_text).map_err(|e| err(format!("invalid study: {e}")))?;
    let mut inputs =
        BTreeMap::from([("study.toml".to_string(), sha256_hex(study_text.as_bytes()))]);
    let mut files = vec![("study.toml".to_string(), study_text.as_bytes().to_vec())];
    let mut load = |key: &str, rel: &str| -> Result<String, KshanaError> {
        let (_, bytes) = read_rel(base, rel)?;
        inputs.insert(format!("{key}.csv"), sha256_hex(&bytes));
        files.push((format!("{key}.csv"), bytes.clone()));
        super::utf8(rel, &bytes)
    };
    let schedule = parse_passes("schedule", &load("schedule", &study.study.schedule)?)?;
    let outages = parse_outages(&load("outages", &study.study.outages)?)?;
    let alternates = match &study.study.alternates {
        Some(a) => Some(parse_passes("alternates", &load("alternates", a)?)?),
        None => None,
    };
    let impact = assess(&study, &schedule, &outages, alternates.as_deref(), inputs)?;
    Ok((impact, files))
}

/// A table cell renderer.
type Cell = Box<dyn Fn(&CaseResult) -> String>;

impl Impact {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("the impact serialises")
    }

    pub fn summary_line(&self) -> String {
        let b = &self.body;
        let lost: f64 = b.missions.iter().map(|m| m.science_lost_gbit).sum();
        let hit: usize = b.missions.iter().map(|m| m.impacted_passes.len()).sum();
        let crit: usize = b
            .missions
            .iter()
            .map(|m| m.critical_windows_lost.len())
            .sum();
        format!(
            "{}: {} missions, {} stations with outages; {hit} passes impacted; {:.3} Gbit lost to recorder overflow; {crit} critical windows lost (MODELLED)",
            b.study.name,
            b.missions.len(),
            b.stations.len(),
            lost
        )
    }

    pub fn to_markdown(&self) -> String {
        let b = &self.body;
        let mut s = format!(
            "# Tracking-gap impact: {}\n\n_{}_\n\nImpact SHA-256 `{}`\n\nPeriod {} to {}.\n\n",
            b.study.name, self.label, self.impact_sha256, b.study.start_utc, b.study.end_utc
        );
        s += "## Stations\n\n| Station | Outage h | Passes impacted | Missions |\n|---|---|---|---|\n";
        for st in &b.stations {
            s += &format!(
                "| {} | {} | {} | {} |\n",
                st.station,
                st.outage_h,
                st.passes_impacted,
                st.missions_impacted.join(", ")
            );
        }
        for m in &b.missions {
            s += &format!("\n## Mission {}\n\n", m.mission);
            for f in &m.findings {
                s += &format!("* {f}\n");
            }
            s +=
                "\n| Figure | As scheduled | With outages | With alternates |\n|---|---|---|---|\n";
            let alt = |f: &dyn Fn(&CaseResult) -> String| {
                m.with_alternates
                    .as_ref()
                    .map(f)
                    .unwrap_or_else(|| "—".into())
            };
            let rows: Vec<(&str, Cell)> = vec![
                (
                    "Contact h",
                    Box::new(|c: &CaseResult| format!("{:.2}", c.contact_h)),
                ),
                (
                    "Contacts (with TC)",
                    Box::new(|c: &CaseResult| format!("{} ({})", c.contacts, c.contacts_with_tc)),
                ),
                (
                    "Downlinked Gbit",
                    Box::new(|c: &CaseResult| format!("{:.3}", c.downlinked_gbit)),
                ),
                (
                    "Recorder overflow Gbit",
                    Box::new(|c: &CaseResult| format!("{:.3}", c.overflow_gbit)),
                ),
                (
                    "On board at end Gbit",
                    Box::new(|c: &CaseResult| format!("{:.3}", c.recorder_end_gbit)),
                ),
                (
                    "Longest gap h",
                    Box::new(|c: &CaseResult| format!("{:.2}", c.longest_gap_h)),
                ),
                (
                    "Gaps over limit",
                    Box::new(|c: &CaseResult| c.gaps_over_limit.len().to_string()),
                ),
                (
                    "Days below ranging minimum",
                    Box::new(|c: &CaseResult| c.ranging_days_below_minimum.len().to_string()),
                ),
                (
                    "Critical windows met",
                    Box::new(|c: &CaseResult| {
                        format!(
                            "{} of {}",
                            c.critical.iter().filter(|x| x.met).count(),
                            c.critical.len()
                        )
                    }),
                ),
            ];
            for (k, f) in &rows {
                s += &format!(
                    "| {k} | {} | {} | {} |\n",
                    f(&m.scheduled),
                    f(&m.with_outages),
                    alt(f.as_ref())
                );
            }
            if !m.impacted_passes.is_empty() {
                s += "\n| Pass | Station | Scheduled | Lost min | Effect | Reason |\n|---|---|---|---|---|---|\n";
                for p in &m.impacted_passes {
                    s += &format!(
                        "| {} | {} | {} | {} | {} | {} |\n",
                        p.pass_id,
                        p.station,
                        p.scheduled_utc,
                        p.lost_min,
                        p.effect,
                        p.reasons.join("; ")
                    );
                }
            }
            if !m.with_outages.gaps_over_limit.is_empty() {
                s += "\nGaps beyond the limit with the outages:\n\n";
                for g in &m.with_outages.gaps_over_limit {
                    s += &format!("* {} to {} ({} h)\n", g.from_utc, g.to_utc, g.hours);
                }
            }
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
    fn recorder_model_fills_drains_and_overflows() {
        // 1 Mbit/s generated for 1000 s, no contact, 0.5 Gbit recorder: 0.5 Gbit overflow.
        let (d, o, e) = simulate(&[], 1000.0, 0.5, 0.0, 0.0, 1000.0);
        assert_eq!((d, round(o, 9), e), (0.0, 0.5, 0.5));
        // A 2 Mbit/s contact over the whole period keeps it empty and downlinks what is made.
        let (d, o, e) = simulate(&[(0.0, 1000.0, 2000.0)], 1000.0, 0.5, 0.0, 0.0, 1000.0);
        assert_eq!((round(d, 9), o, e), (1.0, 0.0, 0.0));
        // Start full (0.5), drain at net 1 Mbit/s for 500 s then empty: down 1.0 + 0.5.
        let (d, o, e) = simulate(&[(0.0, 1000.0, 2000.0)], 1000.0, 0.5, 0.5, 0.0, 1000.0);
        assert_eq!((round(d, 9), o, round(e, 9)), (1.5, 0.0, 0.0));
        // Overlapping contacts downlink at the higher rate only.
        let (d, _, _) = simulate(
            &[(0.0, 100.0, 500.0), (0.0, 100.0, 800.0)],
            0.0,
            1.0,
            1.0,
            0.0,
            100.0,
        );
        assert_eq!(round(d, 9), 0.08);
    }
}
