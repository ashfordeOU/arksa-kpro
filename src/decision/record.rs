// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! One signed decision record per conjunction event: the inputs the operator had, the
//! threshold policy in force, the options considered, the action chosen and why.
//!
//! The record states; it does not decide. The probability of collision and the miss
//! distance are the conjunction assessment provider's figures as given (not recomputed),
//! and the checks below compare the stated decision with the stated policy:
//!
//! * the band the event falls in under the policy (above the manoeuvre threshold, in the
//!   monitoring band, or below it);
//! * whether the chosen option follows the policy, is more conservative than it, or
//!   departs from it (a departure is recorded with its rationale, never refused: the
//!   operator decides);
//! * whether a chosen manoeuvre's stated post-manoeuvre probability is below the
//!   manoeuvre threshold, in the spirit of the EU Space Act proposal's Annex IV
//!   1.4(c)(v) ("checking that mitigation actions decrease the risk level");
//! * the timing (the decision precedes the time of closest approach) and the
//!   coordination stated.
//!
//! Records can be chained: each may carry the `record_sha256` of the operator's previous
//! record, and [`verify_chain`] checks an ordered list.

use serde::{Deserialize, Serialize};

use super::time::parse_utc;
use crate::mbse::evidence::sha256_hex;
use kshana::api::KshanaError;

/// Identifies `decision.json`.
pub const SCHEMA: &str = "arksa-ca-decision-record/1";
/// The honesty label every record carries.
pub const LABEL: &str = "MODELLED - a record of a collision-avoidance decision as the operator stated it. The probability of collision and the miss distance are the provider's model outputs as given, not recomputed or validated here. Not a regulatory filing and not acceptance by any authority.";

fn err(msg: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(msg.into())
}

/// The conjunction as the operator received it.
///
/// TODO(AK1): once `ak/conjunction-validation` merges, fill this from its CCSDS CDM
/// (508.0) parser instead of by hand, and carry the CDM's message identifier and
/// originator into `cdm_ids` and `data_sources`. This record deliberately parses no CDM.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConjunctionInput {
    pub event_id: String,
    /// The operator's object (name or catalogue number).
    pub primary: String,
    pub secondary: String,
    pub tca_utc: String,
    /// Probability of collision, as the provider gives it.
    pub pc: f64,
    pub miss_distance_m: f64,
    #[serde(default)]
    pub radial_miss_m: Option<f64>,
    #[serde(default)]
    pub pc_method: Option<String>,
    #[serde(default)]
    pub hard_body_radius_m: Option<f64>,
    /// Who produced the figures (provider names, products).
    #[serde(default)]
    pub data_sources: Vec<String>,
    /// Identifiers of the conjunction data messages the figures come from.
    #[serde(default)]
    pub cdm_ids: Vec<String>,
    /// When the figures were produced.
    #[serde(default)]
    pub assessed_utc: Option<String>,
}

/// The threshold policy in force for the event.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThresholdPolicy {
    pub name: String,
    /// At or above this probability the policy calls for a manoeuvre.
    pub manoeuvre_pc: f64,
    /// At or above this probability (and below `manoeuvre_pc`) the event is monitored.
    #[serde(default)]
    pub monitor_pc: Option<f64>,
    /// A miss distance below this also calls for a manoeuvre (m).
    #[serde(default)]
    pub min_miss_distance_m: Option<f64>,
    /// Where the policy comes from (operations manual, licence condition, provider).
    #[serde(default)]
    pub source: Option<String>,
}

/// What an option is.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OptionKind {
    Manoeuvre,
    Monitor,
    NoAction,
    Coordinate,
}

impl OptionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OptionKind::Manoeuvre => "manoeuvre",
            OptionKind::Monitor => "monitor",
            OptionKind::NoAction => "no action",
            OptionKind::Coordinate => "coordinate with the other operator",
        }
    }
}

/// One option the operator considered.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OptionConsidered {
    pub id: String,
    pub kind: OptionKind,
    pub description: String,
    #[serde(default)]
    pub delta_v_mps: Option<f64>,
    /// Probability of collision after the option, as assessed.
    #[serde(default)]
    pub post_pc: Option<f64>,
    #[serde(default)]
    pub post_miss_m: Option<f64>,
    /// Effects on other conjunctions, mission or fuel.
    #[serde(default)]
    pub secondary_effects: Option<String>,
}

/// The decision.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DecisionMade {
    pub chosen_option: String,
    pub rationale: String,
    pub decided_by: String,
    pub role: String,
    pub decided_utc: String,
    #[serde(default)]
    pub executed_utc: Option<String>,
}

/// The coordination stated.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Coordination {
    #[serde(default)]
    pub other_operator_contacted: bool,
    #[serde(default)]
    pub ca_provider_informed: bool,
    #[serde(default)]
    pub notes: String,
}

/// The input file.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DecisionInput {
    pub event: ConjunctionInput,
    pub policy: ThresholdPolicy,
    #[serde(rename = "option")]
    pub options: Vec<OptionConsidered>,
    pub decision: DecisionMade,
    #[serde(default)]
    pub coordination: Coordination,
    /// `record_sha256` of the operator's previous record, to chain records.
    #[serde(default)]
    pub prev_record_sha256: Option<String>,
}

/// The policy band of an event.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Band {
    AboveManoeuvreThreshold,
    MonitoringBand,
    BelowThresholds,
}

impl Band {
    pub fn as_str(self) -> &'static str {
        match self {
            Band::AboveManoeuvreThreshold => "above the manoeuvre threshold",
            Band::MonitoringBand => "in the monitoring band",
            Band::BelowThresholds => "below the thresholds",
        }
    }
}

/// How the decision relates to the policy.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Consistency {
    FollowsPolicy,
    MoreConservativeThanPolicy,
    DepartsFromPolicy,
}

impl Consistency {
    pub fn as_str(self) -> &'static str {
        match self {
            Consistency::FollowsPolicy => "follows the policy",
            Consistency::MoreConservativeThanPolicy => "more conservative than the policy",
            Consistency::DepartsFromPolicy => "DEPARTS FROM THE POLICY",
        }
    }
}

/// What the record finds.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Findings {
    pub band: Band,
    /// Why the event is in that band.
    pub band_reason: String,
    pub chosen_kind: OptionKind,
    pub consistency: Consistency,
    /// The chosen manoeuvre's stated post-manoeuvre probability against the threshold.
    pub mitigation: String,
    pub hours_before_tca: f64,
    pub options_considered: usize,
    /// Points a reviewer should look at (empty when there are none).
    pub notes: Vec<String>,
    /// The rule texts the findings refer to ([`crate::disposal::rules`]).
    pub rule_refs: Vec<String>,
}

/// The record body (what `record_sha256` covers).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RecordBody {
    pub schema: String,
    pub input: DecisionInput,
    pub input_sha256: String,
    pub findings: Findings,
}

/// A decision record.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DecisionRecord {
    pub label: &'static str,
    pub body: RecordBody,
    pub record_sha256: String,
}

fn check_prob(name: &str, p: f64) -> Result<(), KshanaError> {
    if p.is_finite() && (0.0..=1.0).contains(&p) {
        Ok(())
    } else {
        Err(err(format!("{name} = {p} is not a probability in [0, 1]")))
    }
}

fn check_nonneg(name: &str, x: f64) -> Result<(), KshanaError> {
    if x.is_finite() && x >= 0.0 {
        Ok(())
    } else {
        Err(err(format!(
            "{name} = {x} must be a non-negative finite number"
        )))
    }
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl DecisionInput {
    /// Parse and check an input file's text.
    pub fn parse(text: &str) -> Result<DecisionInput, KshanaError> {
        let d: DecisionInput =
            toml::from_str(text).map_err(|e| err(format!("invalid decision input: {e}")))?;
        d.validate()?;
        Ok(d)
    }

    fn validate(&self) -> Result<(), KshanaError> {
        let e = &self.event;
        for (n, s) in [
            ("event.event_id", &e.event_id),
            ("event.primary", &e.primary),
            ("event.secondary", &e.secondary),
            ("policy.name", &self.policy.name),
            ("decision.rationale", &self.decision.rationale),
            ("decision.decided_by", &self.decision.decided_by),
            ("decision.role", &self.decision.role),
        ] {
            if s.trim().is_empty() {
                return Err(err(format!("{n} may not be empty")));
            }
        }
        check_prob("event.pc", e.pc)?;
        check_nonneg("event.miss_distance_m", e.miss_distance_m)?;
        if let Some(r) = e.radial_miss_m {
            if !r.is_finite() {
                return Err(err("event.radial_miss_m is not a finite number"));
            }
        }
        if let Some(r) = e.hard_body_radius_m {
            check_nonneg("event.hard_body_radius_m", r)?;
        }
        let tca = parse_utc(&e.tca_utc).map_err(|m| err(format!("event.tca_utc: {m}")))?;
        if let Some(t) = &e.assessed_utc {
            parse_utc(t).map_err(|m| err(format!("event.assessed_utc: {m}")))?;
        }
        let p = &self.policy;
        check_prob("policy.manoeuvre_pc", p.manoeuvre_pc)?;
        if p.manoeuvre_pc == 0.0 {
            return Err(err("policy.manoeuvre_pc must be above 0"));
        }
        if let Some(m) = p.monitor_pc {
            check_prob("policy.monitor_pc", m)?;
            if m > p.manoeuvre_pc {
                return Err(err(
                    "policy.monitor_pc may not be above policy.manoeuvre_pc",
                ));
            }
        }
        if let Some(m) = p.min_miss_distance_m {
            check_nonneg("policy.min_miss_distance_m", m)?;
        }
        if self.options.is_empty() {
            return Err(err("the record names no [[option]]"));
        }
        for (i, o) in self.options.iter().enumerate() {
            if o.id.trim().is_empty() {
                return Err(err("an [[option]] has an empty id"));
            }
            if self.options[..i].iter().any(|x| x.id == o.id) {
                return Err(err(format!("option id '{}' appears twice", o.id)));
            }
            if let Some(x) = o.post_pc {
                check_prob(&format!("option '{}' post_pc", o.id), x)?;
            }
            if let Some(x) = o.delta_v_mps {
                check_nonneg(&format!("option '{}' delta_v_mps", o.id), x)?;
            }
            if let Some(x) = o.post_miss_m {
                check_nonneg(&format!("option '{}' post_miss_m", o.id), x)?;
            }
        }
        if !self
            .options
            .iter()
            .any(|o| o.id == self.decision.chosen_option)
        {
            return Err(err(format!(
                "decision.chosen_option '{}' is not one of the options",
                self.decision.chosen_option
            )));
        }
        let decided = parse_utc(&self.decision.decided_utc)
            .map_err(|m| err(format!("decision.decided_utc: {m}")))?;
        if decided >= tca {
            return Err(err(format!(
                "decision.decided_utc {} is not before the time of closest approach {}: a record is made for a decision taken before the event",
                self.decision.decided_utc, e.tca_utc
            )));
        }
        if let Some(x) = &self.decision.executed_utc {
            let t = parse_utc(x).map_err(|m| err(format!("decision.executed_utc: {m}")))?;
            if t < decided {
                return Err(err("decision.executed_utc is before decision.decided_utc"));
            }
        }
        if let Some(h) = &self.prev_record_sha256 {
            if !is_hex64(h) {
                return Err(err(
                    "prev_record_sha256 must be 64 lower-case hexadecimal characters",
                ));
            }
        }
        Ok(())
    }
}

/// The band of an event under a policy, with the reason.
pub fn band_of(pc: f64, miss_m: f64, p: &ThresholdPolicy) -> (Band, String) {
    if pc >= p.manoeuvre_pc {
        return (
            Band::AboveManoeuvreThreshold,
            format!(
                "Pc {pc:.2e} is at or above the manoeuvre threshold {:.2e}",
                p.manoeuvre_pc
            ),
        );
    }
    if let Some(m) = p.min_miss_distance_m {
        if miss_m < m {
            return (
                Band::AboveManoeuvreThreshold,
                format!("miss distance {miss_m:.0} m is below the policy's {m:.0} m"),
            );
        }
    }
    match p.monitor_pc {
        Some(m) if pc >= m => (
            Band::MonitoringBand,
            format!(
                "Pc {pc:.2e} is at or above the monitoring threshold {m:.2e} and below the manoeuvre threshold {:.2e}",
                p.manoeuvre_pc
            ),
        ),
        _ => (
            Band::BelowThresholds,
            format!("Pc {pc:.2e} is below the policy's thresholds"),
        ),
    }
}

/// Build the record of an input whose file bytes hash to `input_sha256`.
pub fn build(input: DecisionInput, input_sha256: &str) -> Result<DecisionRecord, KshanaError> {
    input.validate()?;
    let e = &input.event;
    let p = &input.policy;
    let chosen = input
        .options
        .iter()
        .find(|o| o.id == input.decision.chosen_option)
        .expect("validated");
    let (band, band_reason) = band_of(e.pc, e.miss_distance_m, p);
    let acts = matches!(chosen.kind, OptionKind::Manoeuvre);
    let consistency = match (band, acts) {
        (Band::AboveManoeuvreThreshold, true) => Consistency::FollowsPolicy,
        (Band::AboveManoeuvreThreshold, false) => Consistency::DepartsFromPolicy,
        (_, true) => Consistency::MoreConservativeThanPolicy,
        (_, false) => Consistency::FollowsPolicy,
    };
    let mut notes = Vec::new();
    let mut rule_refs = vec!["EU-AnnexIV-1.4-c".to_string()];
    let mitigation = if acts {
        match chosen.post_pc {
            Some(x) if x < p.manoeuvre_pc && x < e.pc => format!(
                "the chosen manoeuvre's stated Pc after the manoeuvre, {x:.2e}, is below the manoeuvre threshold and below the Pc before it"
            ),
            Some(x) => {
                notes.push(format!(
                    "The chosen manoeuvre's stated Pc after the manoeuvre ({x:.2e}) is not below both the manoeuvre threshold ({:.2e}) and the Pc before it ({:.2e}).",
                    p.manoeuvre_pc, e.pc
                ));
                format!("NOT SHOWN: stated Pc after the manoeuvre is {x:.2e}")
            }
            None => {
                notes.push("The chosen manoeuvre states no Pc after the manoeuvre, so the record cannot show that it reduces the risk.".into());
                "not stated".into()
            }
        }
    } else {
        "no manoeuvre chosen".into()
    };
    if acts
        && chosen
            .secondary_effects
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
    {
        notes.push("The chosen manoeuvre states no effect on other conjunctions; the EU Space Act proposal (Annex IV 1.4(c)(v)) asks that mitigation actions \"do not unduly increase the risk level of other conjunctions\".".into());
    }
    if consistency == Consistency::DepartsFromPolicy {
        notes.push(format!(
            "The event is {} and the chosen option is '{}': the departure from the policy rests on the rationale recorded.",
            band.as_str(),
            chosen.kind.as_str()
        ));
    }
    if input.options.len() < 2 {
        notes.push("Only one option is recorded.".into());
    }
    if e.data_sources.is_empty() {
        notes.push("No data source is named for the conjunction figures.".into());
    }
    if band == Band::AboveManoeuvreThreshold && !input.coordination.ca_provider_informed {
        notes.push("The record does not state that the collision avoidance provider was informed of the action; under the EU Space Act proposal (Article 64(5)) a Union operator would inform it after a high interest event alert.".into());
        rule_refs.push("EU-Art64-5".into());
    }
    rule_refs.push("EU-AnnexIV-2.7-a".into());
    let tca = parse_utc(&e.tca_utc).expect("validated");
    let decided = parse_utc(&input.decision.decided_utc).expect("validated");
    let hours_before_tca = ((tca - decided) / 3600.0 * 1000.0).round() / 1000.0;
    let findings = Findings {
        band,
        band_reason,
        chosen_kind: chosen.kind,
        consistency,
        mitigation,
        hours_before_tca,
        options_considered: input.options.len(),
        notes,
        rule_refs,
    };
    let body = RecordBody {
        schema: SCHEMA.into(),
        input,
        input_sha256: input_sha256.into(),
        findings,
    };
    let record_sha256 = sha256_hex(crate::mission::compose::canonical_json(&body)?.as_bytes());
    Ok(DecisionRecord {
        label: LABEL,
        body,
        record_sha256,
    })
}

impl DecisionRecord {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("the record serialises")
    }

    /// A short Markdown rendering.
    pub fn to_markdown(&self) -> String {
        let b = &self.body;
        let i = &b.input;
        let f = &b.findings;
        let mut s = format!(
            "# Collision-avoidance decision record: {}\n\n**{}**\n\nRecord SHA-256 `{}`{}.\n\n",
            i.event.event_id,
            self.label,
            self.record_sha256,
            i.prev_record_sha256
                .as_ref()
                .map(|h| format!(", previous record `{h}`"))
                .unwrap_or_default()
        );
        s.push_str(&format!(
            "## Event\n\n{} and {}, time of closest approach {}. Pc {:.2e}{}, miss distance {:.0} m{}. Sources: {}. Conjunction data messages: {}.\n\n",
            i.event.primary,
            i.event.secondary,
            i.event.tca_utc,
            i.event.pc,
            i.event.pc_method.as_ref().map(|m| format!(" ({m})")).unwrap_or_default(),
            i.event.miss_distance_m,
            i.event.radial_miss_m.map(|r| format!(", radial {r:.0} m")).unwrap_or_default(),
            if i.event.data_sources.is_empty() { "not named".into() } else { i.event.data_sources.join(", ") },
            if i.event.cdm_ids.is_empty() { "none listed".into() } else { i.event.cdm_ids.join(", ") },
        ));
        s.push_str(&format!(
            "## Policy\n\n{}: manoeuvre at Pc >= {:.2e}{}{}{}.\n\nThe event is {} ({}).\n\n",
            i.policy.name,
            i.policy.manoeuvre_pc,
            i.policy
                .monitor_pc
                .map(|m| format!(", monitor at Pc >= {m:.2e}"))
                .unwrap_or_default(),
            i.policy
                .min_miss_distance_m
                .map(|m| format!(", or miss distance below {m:.0} m"))
                .unwrap_or_default(),
            i.policy
                .source
                .as_ref()
                .map(|x| format!("; source: {x}"))
                .unwrap_or_default(),
            f.band.as_str(),
            f.band_reason
        ));
        s.push_str("## Options considered\n\n| Id | Kind | Description | Delta-v (m/s) | Pc after | Miss after (m) |\n|---|---|---|---|---|---|\n");
        for o in &i.options {
            s.push_str(&format!(
                "| {}{} | {} | {} | {} | {} | {} |\n",
                o.id,
                if o.id == i.decision.chosen_option {
                    " (chosen)"
                } else {
                    ""
                },
                o.kind.as_str(),
                o.description.replace('|', "/"),
                o.delta_v_mps
                    .map(|x| format!("{x:.3}"))
                    .unwrap_or_else(|| "-".into()),
                o.post_pc
                    .map(|x| format!("{x:.2e}"))
                    .unwrap_or_else(|| "-".into()),
                o.post_miss_m
                    .map(|x| format!("{x:.0}"))
                    .unwrap_or_else(|| "-".into()),
            ));
        }
        s.push_str(&format!(
            "\n## Decision\n\n{} ({}) decided at {} ({} h before closest approach){}: **{}**, which {}.\n\nRationale: {}\n\nMitigation: {}.\n\nCoordination: other operator {}contacted; collision avoidance provider {}informed. {}\n",
            i.decision.decided_by,
            i.decision.role,
            i.decision.decided_utc,
            f.hours_before_tca,
            i.decision.executed_utc.as_ref().map(|x| format!(", executed {x}")).unwrap_or_default(),
            i.decision.chosen_option,
            f.consistency.as_str(),
            i.decision.rationale,
            f.mitigation,
            if i.coordination.other_operator_contacted { "" } else { "not " },
            if i.coordination.ca_provider_informed { "" } else { "not " },
            i.coordination.notes
        ));
        if !f.notes.is_empty() {
            s.push_str("\n## Review notes\n\n");
            for n in &f.notes {
                s.push_str(&format!("- {n}\n"));
            }
        }
        s.push_str("\n## Rule texts referred to\n\n");
        for r in crate::disposal::rules::rules_for(f.rule_refs.iter().map(String::as_str)) {
            s.push_str(&format!(
                "- **{}** (read {}): \"{}\" Source: {} <{}>. {}\n",
                r.citation, r.retrieved, r.quote, r.source_title, r.source_url, r.note
            ));
        }
        s
    }
}

/// Check an ordered list of records (each `decision.json` parsed as JSON): every
/// `record_sha256` recomputes from its body, and every record after the first names the
/// one before it as `prev_record_sha256`. Returns the problems found (empty: intact).
pub fn verify_chain(records: &[serde_json::Value]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut prev: Option<String> = None;
    for (i, r) in records.iter().enumerate() {
        let n = i + 1;
        let recorded = r
            .get("record_sha256")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        match r.get("body") {
            Some(body) if body.get("schema").and_then(|s| s.as_str()) == Some(SCHEMA) => {
                let actual = serde_json::to_string(body)
                    .map(|c| sha256_hex(c.as_bytes()))
                    .unwrap_or_default();
                if actual != recorded {
                    problems.push(format!(
                        "record {n}: the body hashes to {actual}, but record_sha256 says {recorded}"
                    ));
                }
                let stated = body
                    .get("input")
                    .and_then(|x| x.get("prev_record_sha256"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                if let Some(p) = &prev {
                    if stated.as_deref() != Some(p.as_str()) {
                        problems.push(format!(
                            "record {n}: prev_record_sha256 is {}, but the record before it is {p}",
                            stated.as_deref().unwrap_or("absent")
                        ));
                    }
                }
            }
            _ => problems.push(format!("record {n}: not a decision record ({SCHEMA})")),
        }
        prev = Some(recorded);
    }
    problems
}
