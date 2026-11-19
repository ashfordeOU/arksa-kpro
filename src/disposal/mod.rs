// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # Disposal compliance
//!
//! `arksa-kpro mission disposal <disposal.toml> --out <dir>` checks one spacecraft's
//! end-of-life plan against the post-mission disposal, passivation and re-entry casualty
//! rules of three frameworks, each quoted from its primary text ([`rules`]):
//!
//! * FCC: 47 CFR 25.283(e) (disposal no later than five years after end of mission for
//!   uncontrolled re-entry below 2000 km) and 25.283(c) (stored energy discharged);
//! * ESA: ESSB-ST-U-007 (lifetime under 5 years and cumulative collision probability below
//!   10^-3 after end of life, large-constellation disposal apogee below 375 km,
//!   probability of successful disposal and passivation, casualties per re-entry below
//!   10^-4);
//! * EU: the EU Space Act **proposal**, whose numerical limits are left to implementing
//!   acts, so its rows disclose values rather than pass or fail them.
//!
//! The orbital lifetime is taken from the operator's own analysis when supplied
//! (`OPERATOR-SUPPLIED`) and is otherwise estimated by the screening model in
//! [`lifetime`] (`MODELLED`); with both, both are shown and a disagreement is flagged.
//! Probabilities of successful disposal and passivation, the cumulative collision
//! probability and the casualty expectation are the operator's figures as stated
//! (`DECLARED`): nothing here computes them.
//!
//! A verdict is `COMPLIANT` only when the conservative bound meets the limit;
//! `MARGINAL` when the nominal value meets it and the bound does not; `NOT COMPLIANT`
//! when the nominal value fails; `NOT ASSESSED` when an input is missing; `INFO` for a
//! disclosure with no limit. It is an engineering screening, not a licence decision and
//! not an acceptance by any authority.

pub mod lifetime;
pub mod rules;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mbse::evidence::sha256_hex;
use kshana::api::KshanaError;
use lifetime::{ActivityCase, CaseLifetime};
use rules::RuleText;

/// Identifies `disposal.json`.
pub const SCHEMA: &str = "arksa-disposal-compliance/1";
/// The honesty label every assessment carries.
pub const LABEL: &str = "MODELLED - a screening of the stated end-of-life plan against quoted rule texts. A modelled lifetime is a simplified drag estimate; every probability is the operator's figure as stated. Not a licence decision and not acceptance by any authority.";
/// Five years, the FCC and ESA lifetime limit (years).
pub const FIVE_YEARS: f64 = 5.0;

fn err(msg: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(msg.into())
}

/// The input file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisposalInput {
    pub title: String,
    pub spacecraft: Spacecraft,
    pub disposal: DisposalPlan,
    /// The operator's own lifetime analysis, when there is one.
    #[serde(default)]
    pub lifetime: Option<OperatorLifetime>,
    #[serde(default)]
    pub passivation: Passivation,
    #[serde(default)]
    pub casualty: Option<Casualty>,
    /// Activity cases replacing the default low, moderate and high ones.
    #[serde(default, rename = "activity_case")]
    pub activity_cases: Vec<ActivityCaseIn>,
    /// Frameworks to check: any of `FCC`, `ESA`, `EU` (default all three).
    #[serde(default)]
    pub frameworks: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spacecraft {
    pub name: String,
    pub mass_kg: f64,
    /// Mean cross-section facing the flow (m²).
    pub drag_area_m2: f64,
    #[serde(default = "Spacecraft::d_cd")]
    pub drag_coefficient: f64,
    /// Can it perform collision avoidance manoeuvres (a recurrent manoeuvre capability)?
    pub manoeuvrable: bool,
    /// Part of a constellation of at least 100 spacecraft (ESA's large constellation).
    #[serde(default)]
    pub large_constellation: bool,
    #[serde(default)]
    pub has_propulsion: bool,
    #[serde(default = "yes")]
    pub has_batteries: bool,
    #[serde(default)]
    pub has_pressure_vessels: bool,
}

impl Spacecraft {
    fn d_cd() -> f64 {
        2.2
    }
}

fn yes() -> bool {
    true
}

/// How the spacecraft leaves its orbit.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    ControlledReentry,
    SemiControlledReentry,
    UncontrolledReentry,
    NaturalDecay,
    Graveyard,
    RemovalService,
}

impl Strategy {
    fn uncontrolled(self) -> bool {
        matches!(
            self,
            Strategy::UncontrolledReentry
                | Strategy::NaturalDecay
                | Strategy::SemiControlledReentry
        )
    }
    fn as_str(self) -> &'static str {
        match self {
            Strategy::ControlledReentry => "controlled re-entry",
            Strategy::SemiControlledReentry => "semi-controlled re-entry",
            Strategy::UncontrolledReentry => "uncontrolled re-entry",
            Strategy::NaturalDecay => "natural decay",
            Strategy::Graveyard => "graveyard orbit",
            Strategy::RemovalService => "removal service",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisposalPlan {
    pub strategy: Strategy,
    /// The orbit the spacecraft is left in at end of mission (after any lowering).
    pub perigee_km: f64,
    pub apogee_km: f64,
    #[serde(default)]
    pub end_of_mission_utc: Option<String>,
    /// Days from end of mission to the end of passivation (EU disclosure, Annex V 3.4.1).
    #[serde(default)]
    pub passivation_complete_days_after_eom: Option<f64>,
    /// Overall probability of successful disposal (operator's figure).
    #[serde(default)]
    pub probability_successful_disposal: Option<f64>,
    /// Contribution of system reliability to it (operator's figure).
    #[serde(default)]
    pub reliability_contribution: Option<f64>,
    /// Cumulative collision probability with objects > 1 cm from end of life to re-entry.
    #[serde(default)]
    pub cumulative_collision_probability: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OperatorLifetime {
    pub nominal_years: f64,
    /// The conservative bound of the operator's analysis.
    pub upper_years: f64,
    pub tool: String,
    #[serde(default)]
    pub reference: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Passivation {
    #[serde(default)]
    pub probability_successful: Option<f64>,
    #[serde(default, rename = "source")]
    pub sources: Vec<EnergySource>,
}

/// One stored-energy source and how it is passivated.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnergySource {
    /// `propellant`, `battery`, `pressure_vessel`, `momentum_wheel` or `other`.
    pub kind: String,
    pub measure: String,
    pub planned: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Casualty {
    /// Expected number of casualties per re-entry (operator's figure).
    pub expected_casualties: f64,
    pub tool: String,
    #[serde(default)]
    pub reference: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityCaseIn {
    pub name: String,
    pub f107_sfu: f64,
    pub kp: f64,
}

/// The verdict of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Verdict {
    #[serde(rename = "COMPLIANT")]
    Compliant,
    #[serde(rename = "MARGINAL")]
    Marginal,
    #[serde(rename = "NOT COMPLIANT")]
    NotCompliant,
    #[serde(rename = "NOT ASSESSED")]
    NotAssessed,
    #[serde(rename = "NOT APPLICABLE")]
    NotApplicable,
    #[serde(rename = "INFO")]
    Info,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Compliant => "COMPLIANT",
            Verdict::Marginal => "MARGINAL",
            Verdict::NotCompliant => "NOT COMPLIANT",
            Verdict::NotAssessed => "NOT ASSESSED",
            Verdict::NotApplicable => "NOT APPLICABLE",
            Verdict::Info => "INFO",
        }
    }
}

/// One check against one rule.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Check {
    pub rule: String,
    pub framework: String,
    pub citation: String,
    pub verdict: Verdict,
    /// `MODELLED`, `OPERATOR-SUPPLIED`, `DECLARED` or `-`.
    pub basis: String,
    pub value: String,
    pub limit: String,
    pub detail: String,
}

/// The lifetime the checks use.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LifetimeSummary {
    /// `OPERATOR-SUPPLIED` or `MODELLED`; `NONE` when neither is available.
    pub basis: String,
    pub nominal_years: Option<f64>,
    /// The conservative bound; `None` with a nominal value means "more than" the model's
    /// horizon.
    pub upper_years: Option<f64>,
    /// True when the model ran into its horizon in the conservative case.
    pub upper_beyond_horizon: bool,
    pub operator: Option<OperatorLifetime>,
    /// The screening model, per activity case (empty when the orbit is outside its scope).
    pub modelled: Vec<CaseLifetime>,
    pub model_scope_note: String,
    pub ballistic_coefficient_m2_per_kg: f64,
    /// Set when the operator's bound and the model's disagree about the five-year limit.
    pub cross_check: Option<String>,
}

/// Counts of verdicts.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Counts {
    pub compliant: usize,
    pub marginal: usize,
    pub not_compliant: usize,
    pub not_assessed: usize,
    pub not_applicable: usize,
    pub info: usize,
}

/// The assessment body (what `assessment_sha256` covers).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AssessmentBody {
    pub schema: String,
    pub title: String,
    pub spacecraft: String,
    pub strategy: String,
    pub perigee_km: f64,
    pub apogee_km: f64,
    pub end_of_mission_utc: Option<String>,
    pub input_sha256: String,
    pub frameworks: Vec<String>,
    pub lifetime: LifetimeSummary,
    pub passivation: Vec<EnergySource>,
    pub checks: Vec<Check>,
    pub counts: Counts,
    pub rules: Vec<RuleText>,
    pub limitations: Vec<String>,
}

/// A disposal assessment.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Assessment {
    pub label: &'static str,
    pub body: AssessmentBody,
    pub assessment_sha256: String,
}

fn finite(name: &str, x: f64) -> Result<f64, KshanaError> {
    if x.is_finite() {
        Ok(x)
    } else {
        Err(err(format!("{name} is not a finite number")))
    }
}

fn prob(name: &str, x: Option<f64>) -> Result<Option<f64>, KshanaError> {
    match x {
        Some(p) if !(p.is_finite() && (0.0..=1.0).contains(&p)) => {
            Err(err(format!("{name} = {p} is not a probability in [0, 1]")))
        }
        _ => Ok(x),
    }
}

fn fmt_years(y: f64) -> String {
    format!("{:.2} years", y)
}

fn fmt_p(p: f64) -> String {
    if p == 0.0 {
        "0".into()
    } else if p >= 0.01 {
        format!("{p:.3}")
    } else {
        format!("{p:.2e}")
    }
}

impl DisposalInput {
    /// Parse and check an input file's text.
    pub fn parse(text: &str) -> Result<DisposalInput, KshanaError> {
        let d: DisposalInput =
            toml::from_str(text).map_err(|e| err(format!("invalid disposal input: {e}")))?;
        let s = &d.spacecraft;
        if d.title.trim().is_empty() || s.name.trim().is_empty() {
            return Err(err("the title and the spacecraft name may not be empty"));
        }
        for (n, x) in [
            ("spacecraft.mass_kg", s.mass_kg),
            ("spacecraft.drag_area_m2", s.drag_area_m2),
            ("spacecraft.drag_coefficient", s.drag_coefficient),
        ] {
            if finite(n, x)? <= 0.0 {
                return Err(err(format!("{n} must be positive")));
            }
        }
        let p = &d.disposal;
        finite("disposal.perigee_km", p.perigee_km)?;
        finite("disposal.apogee_km", p.apogee_km)?;
        if p.perigee_km < 0.0 || p.apogee_km < p.perigee_km {
            return Err(err(
                "disposal: perigee_km must be non-negative and not above apogee_km",
            ));
        }
        if let Some(t) = &p.end_of_mission_utc {
            crate::decision::time::parse_utc(t).map_err(err)?;
        }
        if let Some(x) = p.passivation_complete_days_after_eom {
            if finite("disposal.passivation_complete_days_after_eom", x)? < 0.0 {
                return Err(err(
                    "disposal.passivation_complete_days_after_eom must be non-negative",
                ));
            }
        }
        prob(
            "disposal.probability_successful_disposal",
            p.probability_successful_disposal,
        )?;
        prob(
            "disposal.reliability_contribution",
            p.reliability_contribution,
        )?;
        prob(
            "disposal.cumulative_collision_probability",
            p.cumulative_collision_probability,
        )?;
        prob(
            "passivation.probability_successful",
            d.passivation.probability_successful,
        )?;
        if let Some(l) = &d.lifetime {
            finite("lifetime.nominal_years", l.nominal_years)?;
            finite("lifetime.upper_years", l.upper_years)?;
            if l.nominal_years < 0.0 || l.upper_years < l.nominal_years {
                return Err(err(
                    "lifetime: nominal_years must be non-negative and not above upper_years",
                ));
            }
            if l.tool.trim().is_empty() {
                return Err(err("lifetime.tool must name the analysis it comes from"));
            }
        }
        if let Some(c) = &d.casualty {
            if finite("casualty.expected_casualties", c.expected_casualties)? < 0.0 {
                return Err(err("casualty.expected_casualties must be non-negative"));
            }
        }
        for c in &d.activity_cases {
            if finite("activity_case.f107_sfu", c.f107_sfu)? <= 0.0
                || !(0.0..=9.0).contains(&finite("activity_case.kp", c.kp)?)
            {
                return Err(err(format!(
                    "activity case '{}': f107_sfu must be positive and kp in [0, 9]",
                    c.name
                )));
            }
        }
        for f in &d.frameworks {
            if !["FCC", "ESA", "EU"].contains(&f.as_str()) {
                return Err(err(format!("unknown framework '{f}': use FCC, ESA or EU")));
            }
        }
        for src in &d.passivation.sources {
            if ![
                "propellant",
                "battery",
                "pressure_vessel",
                "momentum_wheel",
                "other",
            ]
            .contains(&src.kind.as_str())
            {
                return Err(err(format!(
                    "passivation source kind '{}': use propellant, battery, pressure_vessel, momentum_wheel or other",
                    src.kind
                )));
            }
        }
        Ok(d)
    }
}

/// Orbits whose apogee and perigee differ by more than this are outside the circular
/// screening model (km).
pub const MAX_CIRCULAR_SPREAD_KM: f64 = 50.0;

fn lifetime_summary(d: &DisposalInput) -> LifetimeSummary {
    let s = &d.spacecraft;
    let bc = s.drag_coefficient * s.drag_area_m2 / s.mass_kg;
    let p = &d.disposal;
    let cases: Vec<ActivityCase> = if d.activity_cases.is_empty() {
        lifetime::default_cases()
    } else {
        d.activity_cases
            .iter()
            .map(|c| ActivityCase {
                name: c.name.clone(),
                f107_sfu: c.f107_sfu,
                kp: c.kp,
            })
            .collect()
    };
    let in_scope = p.apogee_km - p.perigee_km <= MAX_CIRCULAR_SPREAD_KM;
    let (modelled, note) = if in_scope {
        let mean = 0.5 * (p.perigee_km + p.apogee_km);
        (
            lifetime::lifetimes(mean, bc, &cases),
            format!(
                "Circular orbit at the mean altitude {mean:.1} km; Jacchia 1971 density at constant activity per case; re-entry taken at {} km; horizon {} years.",
                lifetime::REENTRY_ALTITUDE_KM,
                lifetime::MAX_YEARS
            ),
        )
    } else {
        (
            vec![],
            format!(
                "Apogee and perigee differ by more than {MAX_CIRCULAR_SPREAD_KM} km: the orbit is outside the circular screening model, so only an operator-supplied lifetime is used."
            ),
        )
    };
    // The model's nominal is the median case; its bound is the longest (lowest activity).
    let model_nominal = if modelled.is_empty() {
        None
    } else {
        let mut ys: Vec<Option<f64>> = modelled.iter().map(|c| c.years).collect();
        ys.sort_by(|a, b| match (a, b) {
            (Some(x), Some(y)) => x.total_cmp(y),
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        });
        Some(ys[ys.len() / 2])
    };
    let model_upper: Option<Option<f64>> = if modelled.is_empty() {
        None
    } else if modelled.iter().any(|c| c.years.is_none()) {
        Some(None)
    } else {
        modelled
            .iter()
            .filter_map(|c| c.years)
            .reduce(f64::max)
            .map(Some)
    };

    let model_meets = model_upper.map(|u| u.is_some_and(|y| y <= FIVE_YEARS));
    let (basis, nominal, upper, beyond) = match (&d.lifetime, model_nominal, model_upper) {
        (Some(o), _, _) => (
            "OPERATOR-SUPPLIED",
            Some(o.nominal_years),
            Some(o.upper_years),
            false,
        ),
        (None, Some(n), Some(u)) => ("MODELLED", n.or(Some(lifetime::MAX_YEARS)), u, u.is_none()),
        _ => ("NONE", None, None, false),
    };
    let cross_check = match (&d.lifetime, model_meets) {
        (Some(o), Some(m)) if (o.upper_years <= FIVE_YEARS) != m => Some(format!(
            "The operator's bound ({}) and the screening model's conservative case ({}) fall on different sides of the five-year limit: review the density and solar-activity assumptions of both.",
            fmt_years(o.upper_years),
            match model_upper.flatten() {
                Some(y) => fmt_years(y),
                None => format!("more than {} years", lifetime::MAX_YEARS),
            }
        )),
        _ => None,
    };
    LifetimeSummary {
        basis: basis.into(),
        nominal_years: nominal,
        upper_years: upper,
        upper_beyond_horizon: beyond,
        operator: d.lifetime.clone(),
        modelled,
        model_scope_note: note,
        ballistic_coefficient_m2_per_kg: bc,
        cross_check,
    }
}

fn check(
    rule: &str,
    verdict: Verdict,
    basis: &str,
    value: String,
    limit: &str,
    detail: String,
) -> Check {
    let r = rules::rule(rule).expect("every check names a registered rule");
    Check {
        rule: rule.into(),
        framework: r.framework.into(),
        citation: r.citation.into(),
        verdict,
        basis: basis.into(),
        value,
        limit: limit.into(),
        detail,
    }
}

/// Verdict of a lifetime against `limit`: bound meets it, nominal meets it, or neither.
fn lifetime_verdict(l: &LifetimeSummary, limit: f64) -> (Verdict, String) {
    match (l.nominal_years, l.upper_years, l.upper_beyond_horizon) {
        (None, _, _) => (Verdict::NotAssessed, "no lifetime available".into()),
        (Some(n), Some(u), _) => {
            let v = if u <= limit {
                Verdict::Compliant
            } else if n <= limit {
                Verdict::Marginal
            } else {
                Verdict::NotCompliant
            };
            (
                v,
                format!("nominal {}, bound {}", fmt_years(n), fmt_years(u)),
            )
        }
        (Some(n), None, _) => {
            let v = if n <= limit {
                Verdict::Marginal
            } else {
                Verdict::NotCompliant
            };
            let nominal = if n >= lifetime::MAX_YEARS {
                format!("more than {} years", lifetime::MAX_YEARS)
            } else {
                fmt_years(n)
            };
            (
                v,
                format!(
                    "nominal {nominal}, bound more than {} years",
                    lifetime::MAX_YEARS
                ),
            )
        }
    }
}

fn threshold(
    rule: &str,
    x: Option<f64>,
    above: bool,
    limit: f64,
    limit_text: &str,
    what: &str,
) -> Check {
    match x {
        None => check(
            rule,
            Verdict::NotAssessed,
            "-",
            "not stated".into(),
            limit_text,
            format!("The input states no {what}."),
        ),
        Some(v) => {
            let ok = if above { v > limit } else { v < limit };
            check(
                rule,
                if ok {
                    Verdict::Compliant
                } else {
                    Verdict::NotCompliant
                },
                "DECLARED",
                fmt_p(v),
                limit_text,
                format!("The operator's {what}, as stated; not computed here."),
            )
        }
    }
}

/// Assess a parsed input whose file bytes hash to `input_sha256`.
pub fn assess(d: &DisposalInput, input_sha256: &str) -> Assessment {
    let frameworks: Vec<String> = if d.frameworks.is_empty() {
        vec!["FCC".into(), "ESA".into(), "EU".into()]
    } else {
        let mut f = d.frameworks.clone();
        f.sort();
        f.dedup();
        f
    };
    let has = |f: &str| frameworks.iter().any(|x| x == f);
    let s = &d.spacecraft;
    let p = &d.disposal;
    let lt = lifetime_summary(d);
    let in_leo = p.perigee_km < 2000.0;
    let mut checks = Vec::new();
    let lt_basis = lt.basis.as_str();

    if has("FCC") {
        let applies = in_leo && p.strategy.uncontrolled();
        if applies {
            let (v, val) = lifetime_verdict(&lt, FIVE_YEARS);
            checks.push(check(
                "FCC-25.283-e",
                v,
                lt_basis,
                val,
                "no later than 5 years after end of mission",
                format!(
                    "Disposal by {} from below 2000 km. End of mission is when the spacecraft can no longer perform collision avoidance manoeuvres{}.",
                    p.strategy.as_str(),
                    if s.manoeuvrable { "" } else { " (for this spacecraft without that capability: completion of its primary mission)" }
                ),
            ));
        } else {
            checks.push(check(
                "FCC-25.283-e",
                Verdict::NotApplicable,
                "-",
                format!("{}, perigee {:.0} km", p.strategy.as_str(), p.perigee_km),
                "applies to uncontrolled re-entry below 2000 km",
                "The paragraph covers disposal through uncontrolled atmospheric re-entry from below 2000 km.".into(),
            ));
        }
        let mut missing = Vec::new();
        for (needed, kind) in [
            (s.has_propulsion, "propellant"),
            (s.has_batteries, "battery"),
            (s.has_pressure_vessels, "pressure_vessel"),
        ] {
            if needed
                && !d
                    .passivation
                    .sources
                    .iter()
                    .any(|x| x.kind == kind && x.planned)
            {
                missing.push(kind);
            }
        }
        checks.push(check(
            "FCC-25.283-c",
            if missing.is_empty() {
                Verdict::Compliant
            } else {
                Verdict::NotCompliant
            },
            "DECLARED",
            format!("{} sources planned", d.passivation.sources.iter().filter(|x| x.planned).count()),
            "stored energy sources discharged",
            if missing.is_empty() {
                "Every stored-energy source the spacecraft declares (propellant, batteries, pressure vessels) has a planned discharge measure. This checks the plan as stated, not its execution.".into()
            } else {
                format!("No planned discharge measure for: {}.", missing.join(", "))
            },
        ));
    }

    if has("ESA") {
        if in_leo {
            let (v, val) = lifetime_verdict(&lt, FIVE_YEARS);
            let epoch = if s.manoeuvrable {
                "from the end-of-life epoch (recurrent manoeuvre capability)"
            } else {
                "from the orbit injection epoch (no recurrent manoeuvre capability): the lifetime here runs from the stated orbit, so supply the lifetime from injection"
            };
            checks.push(check(
                "ESA-5.4.2.3-a",
                if p.strategy == Strategy::ControlledReentry {
                    Verdict::NotApplicable
                } else {
                    v
                },
                lt_basis,
                val,
                "orbit lifetime less than 5 years",
                format!("Condition 1, counted {epoch}."),
            ));
            checks.push(threshold(
                "ESA-5.4.2.3-a",
                p.cumulative_collision_probability,
                false,
                1e-3,
                "cumulative collision probability below 1e-3",
                "cumulative collision probability with objects larger than 1 cm from end of life to re-entry",
            ));
            if s.large_constellation {
                checks.push(check(
                    "ESA-5.4.2.3-c",
                    if p.apogee_km < 375.0 {
                        Verdict::Compliant
                    } else {
                        Verdict::NotCompliant
                    },
                    "DECLARED",
                    format!("apogee {:.0} km", p.apogee_km),
                    "disposal apogee below 375 km",
                    "Large constellation in the LEO protected region.".into(),
                ));
            }
        }
        checks.push(threshold(
            "ESA-5.4.1.1-a",
            p.probability_successful_disposal,
            true,
            0.9,
            "above 0.9",
            "overall probability of successful disposal",
        ));
        let long_lived = lt.nominal_years.is_some_and(|y| y > 25.0) || lt.upper_beyond_horizon;
        if s.large_constellation || (in_leo && long_lived) {
            checks.push(threshold(
                "ESA-5.4.1.1-b",
                p.reliability_contribution,
                true,
                0.95,
                "above 0.95",
                "system-reliability contribution to the probability of successful disposal",
            ));
        }
        let controlled = p.strategy == Strategy::ControlledReentry;
        let planned = d.passivation.sources.iter().filter(|x| x.planned).count();
        checks.push(check(
            "ESA-5.3.2.2-b",
            if controlled {
                Verdict::NotApplicable
            } else if planned > 0 && d.passivation.sources.iter().all(|x| x.planned) {
                Verdict::Compliant
            } else if d.passivation.sources.is_empty() {
                Verdict::NotAssessed
            } else {
                Verdict::NotCompliant
            },
            if controlled { "-" } else { "DECLARED" },
            format!("{planned} of {} sources planned", d.passivation.sources.len()),
            "passivated before end of life unless controlled re-entry",
            if controlled {
                "Controlled re-entry is planned; the clause then does not require passivation (it still applies if the controlled re-entry fails).".into()
            } else {
                "Passivation plan as stated.".into()
            },
        ));
        let req = if long_lived && in_leo { 0.95 } else { 0.90 };
        checks.push(match d.passivation.probability_successful {
            None => check(
                "ESA-5.3.2.2-c",
                Verdict::NotAssessed,
                "-",
                "not stated".into(),
                if req > 0.9 {
                    "at least 0.95"
                } else {
                    "at least 0.90"
                },
                "The input states no probability of successful passivation.".into(),
            ),
            Some(x) => check(
                "ESA-5.3.2.2-c",
                if x >= req {
                    Verdict::Compliant
                } else {
                    Verdict::NotCompliant
                },
                "DECLARED",
                fmt_p(x),
                if req > 0.9 {
                    "at least 0.95"
                } else {
                    "at least 0.90"
                },
                "The operator's figure, as stated; not computed here.".into(),
            ),
        });
        if p.strategy != Strategy::Graveyard {
            checks.push(threshold(
                "ESA-5.5-b",
                d.casualty.as_ref().map(|c| c.expected_casualties),
                false,
                1e-4,
                "less than 1e-4 per re-entry",
                "expected number of casualties per re-entry",
            ));
        }
    }

    if has("EU") {
        if in_leo {
            let after_pass = match (lt.nominal_years, p.passivation_complete_days_after_eom) {
                (Some(n), Some(days)) => format!(
                    "; after passivation (completed {days:.0} days after end of mission) {}",
                    fmt_years((n - days / 365.25).max(0.0))
                ),
                _ => "; time after passivation not disclosed (no passivation date stated)".into(),
            };
            checks.push(check(
                "EU-AnnexV-3.4",
                if lt.nominal_years.is_some() {
                    Verdict::Info
                } else {
                    Verdict::NotAssessed
                },
                lt_basis,
                match (lt.nominal_years, lt.upper_years) {
                    (Some(n), Some(u)) => format!("after end of mission {} (bound {}){after_pass}", fmt_years(n), fmt_years(u)),
                    (Some(n), None) => format!("after end of mission {} (bound beyond the model horizon){after_pass}", fmt_years(n)),
                    _ => "not available".into(),
                },
                "limit set by a future implementing act",
                "Disclosure of the expected time in orbit; no numerical limit exists yet, so no pass or fail is given.".into(),
            ));
        }
        checks.push(check(
            "EU-AnnexV-3.1",
            if p.probability_successful_disposal.is_some() {
                Verdict::Info
            } else {
                Verdict::NotAssessed
            },
            if p.probability_successful_disposal.is_some() {
                "DECLARED"
            } else {
                "-"
            },
            p.probability_successful_disposal
                .map(fmt_p)
                .unwrap_or_else(|| "not stated".into()),
            "limit set by a future implementing act",
            "Disclosure only: the threshold is left to an implementing act.".into(),
        ));
        if p.strategy != Strategy::Graveyard {
            checks.push(check(
                "EU-AnnexV-3.5.4",
                if d.casualty.is_some() { Verdict::Info } else { Verdict::NotAssessed },
                if d.casualty.is_some() { "DECLARED" } else { "-" },
                d.casualty.as_ref().map(|c| fmt_p(c.expected_casualties)).unwrap_or_else(|| "not stated".into()),
                "limit set by a future implementing act",
                "The proposal asks for the collective risk (at least one victim) including air and maritime traffic; the stated figure is shown as given, whatever risk measure it uses.".into(),
            ));
        }
    }

    let mut counts = Counts::default();
    for c in &checks {
        *match c.verdict {
            Verdict::Compliant => &mut counts.compliant,
            Verdict::Marginal => &mut counts.marginal,
            Verdict::NotCompliant => &mut counts.not_compliant,
            Verdict::NotAssessed => &mut counts.not_assessed,
            Verdict::NotApplicable => &mut counts.not_applicable,
            Verdict::Info => &mut counts.info,
        } += 1;
    }
    let mut ids: Vec<&str> = checks.iter().map(|c| c.rule.as_str()).collect();
    if has("FCC") {
        ids.push("FCC-2026-RO");
    }
    let rules = rules::rules_for(ids);
    let body = AssessmentBody {
        schema: SCHEMA.into(),
        title: d.title.clone(),
        spacecraft: s.name.clone(),
        strategy: p.strategy.as_str().into(),
        perigee_km: p.perigee_km,
        apogee_km: p.apogee_km,
        end_of_mission_utc: p.end_of_mission_utc.clone(),
        input_sha256: input_sha256.into(),
        frameworks,
        lifetime: round_lifetime(lt),
        passivation: d.passivation.sources.clone(),
        checks,
        counts,
        rules,
        limitations: limitations(),
    };
    let assessment_sha256 = sha256_hex(
        crate::mission::compose::canonical_json(&body)
            .expect("the assessment serialises")
            .as_bytes(),
    );
    Assessment {
        label: LABEL,
        body,
        assessment_sha256,
    }
}

/// Lifetimes rounded to 6 significant digits, so the document and its hash do not move
/// with the last bits of a platform's floating point.
fn round_lifetime(mut l: LifetimeSummary) -> LifetimeSummary {
    let r = |x: f64| {
        if x == 0.0 || !x.is_finite() {
            return x;
        }
        let d = 5 - x.abs().log10().floor() as i32;
        let m = 10f64.powi(d);
        (x * m).round() / m
    };
    l.nominal_years = l.nominal_years.map(r);
    l.upper_years = l.upper_years.map(r);
    l.ballistic_coefficient_m2_per_kg = r(l.ballistic_coefficient_m2_per_kg);
    for c in &mut l.modelled {
        c.years = c.years.map(r);
    }
    l
}

fn limitations() -> Vec<String> {
    vec![
        "A modelled lifetime comes from a circular-orbit drag estimate with constant solar activity per case; it is a screening value, not a lifetime analysis for a licence filing.".into(),
        "Probabilities of successful disposal and passivation, the cumulative collision probability and the casualty expectation are the operator's figures as stated; this assessment does not compute or verify them.".into(),
        "The EU Space Act rows quote the Commission's proposal; it is not adopted law and leaves its numerical limits to implementing acts.".into(),
        "Which FCC rules apply to a given authorisation (launch date, grandfathering, any move of the rule into the July 2026 order's new Part) is to be confirmed per application.".into(),
        "Nothing here is a licence decision, a certification or an acceptance by any authority.".into(),
    ]
}

impl Assessment {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("the assessment serialises")
    }

    /// A short Markdown report.
    pub fn to_markdown(&self) -> String {
        let b = &self.body;
        let mut s = format!(
            "# Disposal compliance: {}\n\n**{}**\n\nSpacecraft {}, {} from a {:.0} x {:.0} km orbit. Assessment SHA-256 `{}`.\n\n",
            b.title, self.label, b.spacecraft, b.strategy, b.perigee_km, b.apogee_km, self.assessment_sha256
        );
        let l = &b.lifetime;
        s.push_str(&format!(
            "## Lifetime ({})\n\n{}\n\n",
            l.basis, l.model_scope_note
        ));
        for c in &l.modelled {
            s.push_str(&format!(
                "- {} activity (F10.7 {} sfu, Kp {}): {}\n",
                c.case.name,
                c.case.f107_sfu,
                c.case.kp,
                c.years
                    .map(fmt_years)
                    .unwrap_or_else(|| format!("more than {} years", lifetime::MAX_YEARS))
            ));
        }
        if let Some(o) = &l.operator {
            s.push_str(&format!(
                "- operator: nominal {}, bound {} ({}{})\n",
                fmt_years(o.nominal_years),
                fmt_years(o.upper_years),
                o.tool,
                if o.reference.is_empty() {
                    String::new()
                } else {
                    format!(", {}", o.reference)
                }
            ));
        }
        if let Some(x) = &l.cross_check {
            s.push_str(&format!("\n**Cross-check:** {x}\n"));
        }
        s.push_str("\n## Checks\n\n| Framework | Clause | Verdict | Basis | Value | Limit |\n|---|---|---|---|---|---|\n");
        for c in &b.checks {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                c.framework,
                c.citation,
                c.verdict.as_str(),
                c.basis,
                c.value.replace('|', "/"),
                c.limit
            ));
        }
        s.push_str("\n## Rule texts\n\n");
        for r in &b.rules {
            match r.status {
                rules::RuleStatus::Verified => s.push_str(&format!(
                    "- **{}** ({}, read {}): \"{}\" Source: {} <{}>{}\n",
                    r.citation,
                    r.framework,
                    r.retrieved,
                    r.quote,
                    r.source_title,
                    r.source_url,
                    if r.note.is_empty() {
                        String::new()
                    } else {
                        format!(". {}", r.note)
                    }
                )),
                rules::RuleStatus::ToConfirm => s.push_str(&format!(
                    "- **{}** ({}): TO CONFIRM. {}\n",
                    r.citation, r.framework, r.note
                )),
            }
        }
        s.push_str("\n## Limitations\n\n");
        for x in &b.limitations {
            s.push_str(&format!("- {x}\n"));
        }
        s
    }
}

/// Read, parse and assess an input file.
pub fn assess_file(path: &Path) -> Result<Assessment, KshanaError> {
    let bytes = std::fs::read(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
    let text = String::from_utf8(bytes.clone())
        .map_err(|_| err(format!("{} is not UTF-8", path.display())))?;
    let d = DisposalInput::parse(&text)?;
    Ok(assess(&d, &sha256_hex(&bytes)))
}

/// Write `disposal.json`, `disposal.md` and a copy of the input into `dir`; with a signer
/// (seed, name, UTC), sign the directory as a whole (`signature.json`).
pub fn write(
    a: &Assessment,
    input_bytes: &[u8],
    dir: &Path,
    signer: Option<(&[u8; 32], &str, &str)>,
) -> Result<Option<PathBuf>, KshanaError> {
    std::fs::create_dir_all(dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
    let w = |name: &str, bytes: &[u8]| {
        std::fs::write(dir.join(name), bytes).map_err(|e| err(format!("{name}: {e}")))
    };
    w("disposal.json", (a.to_json() + "\n").as_bytes())?;
    w("disposal.md", a.to_markdown().as_bytes())?;
    w("input.toml", input_bytes)?;
    match signer {
        Some((seed, name, utc)) => Ok(Some(kshana_pro::signed_dossier::sign(
            dir, seed, name, utc,
        )?)),
        None => Ok(None),
    }
}
