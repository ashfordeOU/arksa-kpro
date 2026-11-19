// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! A versioned, explicitly provisional registry of lunar time-scale definitions, and the
//! conversions and convention-change impact computed through it.
//!
//! The physics (TCB/TCG/TT/TDB, the TCL secular rate and periodic terms, the `W0_L` scaling to
//! TL, the GUM budgets) is Kshana Pro's [`kshana_pro::lunar_pro::time`], used through the
//! dependency. What this module adds is the layer a lunar navigation service provider needs
//! while the convention is undecided (research 13 finding 4, gap N9):
//!
//! * a **registry** of candidate definitions (TCL, TL on several `W0_L` surfaces, and the LTC
//!   policy name as a placeholder), loaded from a data file with a version number and a
//!   SHA-256, so the definition set can be replaced when CGPM/IAU/IAG decide without a code
//!   change;
//! * rules that keep it honest: a provisional registry must carry both the TCL and the TL
//!   families, may not mark any definition adopted, and has **no default** (every conversion
//!   names its definition); an adopted registry must cite its decision;
//! * **LTC as an envelope**: LTC has no definition, so its conversion is the equal-weight
//!   envelope of the candidate realisations, with the half-spread carried as a rectangular
//!   "definition undecided" uncertainty term;
//! * the **change impact** between two registry versions at chosen epochs: which definitions
//!   changed, by how much each offset moves, and whether that exceeds a re-qualification
//!   tolerance. This is the regression evidence an updatable time layer needs.
//!
//! Evidence class: MODELLED (as the underlying lunar time model).

use std::collections::BTreeMap;

use kshana::timescales::TwoPartJd;
use kshana_pro::lunar_pro::budget::{Budget, Term};
use kshana_pro::lunar_pro::time::{self as lt, Model, RateSource, Scale, W0Candidate};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The built-in registry file (version 1, provisional).
pub const BUILTIN_V1_TOML: &str = include_str!("registry_v1.toml");

/// GPS time runs 19 s behind TAI (constant by definition).
pub const TAI_MINUS_GPST_S: f64 = 19.0;

/// Registry status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistryStatus {
    /// No international decision: every definition is a candidate or placeholder.
    Provisional,
    /// A decision has been taken and is cited in `decision`.
    Adopted,
}

/// The definition family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    /// Unscaled Lunar Coordinate Time.
    Tcl,
    /// Lunar Time scaled on a lunar equipotential `W0_L`.
    Tl,
    /// Coordinated Lunar Time (a policy name; realised here as an envelope of candidates).
    Ltc,
}

/// A definition's status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefStatus {
    Candidate,
    Placeholder,
    Adopted,
    Withdrawn,
}

/// One lunar time-scale definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Definition {
    pub id: String,
    pub family: Family,
    pub status: DefStatus,
    pub name: String,
    /// `lte440` or `first-principles` (TCL and TL).
    #[serde(default)]
    pub rate_source: Option<String>,
    /// TL only: a built-in `W0_L` candidate name, or `conventional:<m2/s2>`.
    #[serde(default)]
    pub w0: Option<String>,
    /// LTC only: the candidate definitions whose envelope realises it.
    #[serde(default)]
    pub envelope_of: Vec<String>,
    /// Positions stated by agencies or authors (informational; not a ranking).
    #[serde(default)]
    pub stated_preferences: Vec<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub notes: String,
}

/// A versioned registry of lunar time-scale definitions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    pub registry_id: String,
    pub version: u32,
    pub issued: String,
    pub status: RegistryStatus,
    /// The decision an adopted registry rests on (resolution citation). Required when adopted.
    #[serde(default)]
    pub decision: Option<String>,
    pub notice: String,
    #[serde(default)]
    pub basis: Vec<String>,
    pub definitions: Vec<Definition>,
}

/// What every output that uses a registry carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistryRef {
    pub registry_id: String,
    pub registry_version: u32,
    pub registry_sha256: String,
    pub registry_status: RegistryStatus,
    pub definition_id: String,
}

/// The reference (Earth) time scales an offset can be taken against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefScale {
    Utc,
    Tai,
    Tt,
    Tcg,
    Tdb,
    Tcb,
    Gpst,
}

impl RefScale {
    pub fn parse(s: &str) -> Result<Self, String> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "utc" => RefScale::Utc,
            "tai" => RefScale::Tai,
            "tt" => RefScale::Tt,
            "tcg" => RefScale::Tcg,
            "tdb" => RefScale::Tdb,
            "tcb" => RefScale::Tcb,
            "gpst" | "gps" => RefScale::Gpst,
            o => {
                return Err(format!(
                    "unknown reference scale '{o}' (utc|tai|tt|tcg|tdb|tcb|gpst)"
                ))
            }
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            RefScale::Utc => "UTC",
            RefScale::Tai => "TAI",
            RefScale::Tt => "TT",
            RefScale::Tcg => "TCG",
            RefScale::Tdb => "TDB",
            RefScale::Tcb => "TCB",
            RefScale::Gpst => "GPST",
        }
    }
    /// The Kshana Pro scale the instant is read in (GPST is read as TAI shifted by 19 s), and
    /// the constant `scale − lunar-pro scale` (s).
    fn lunar_pro_scale(self) -> (Scale, f64) {
        match self {
            RefScale::Utc => (Scale::Utc, 0.0),
            RefScale::Tai => (Scale::Tai, 0.0),
            RefScale::Tt => (Scale::Tt, 0.0),
            RefScale::Tcg => (Scale::Tcg, 0.0),
            RefScale::Tdb => (Scale::Tdb, 0.0),
            RefScale::Tcb => (Scale::Tcb, 0.0),
            RefScale::Gpst => (Scale::Tai, -TAI_MINUS_GPST_S),
        }
    }
}

/// An offset `lunar − reference` (s) at one instant, under one registry definition.
#[derive(Clone, Debug, Serialize)]
pub struct LunarOffset {
    pub registry: RegistryRef,
    pub family: Family,
    pub reference: RefScale,
    /// The instant, as given (ISO, read in `reference`).
    pub at: String,
    /// `lunar − reference` (s).
    pub offset_s: f64,
    /// Combined standard uncertainty (s).
    pub u_c_s: f64,
    /// Expanded uncertainty, k = 2 (s).
    pub expanded_k2_s: f64,
    pub budget: Budget,
    pub notes: Vec<String>,
}

/// A secular rate `d(lunar)/d(TT) − 1` under one definition.
#[derive(Clone, Debug, Serialize)]
pub struct LunarRate {
    pub definition_id: String,
    pub family: Family,
    pub rate: f64,
    pub u_rate: f64,
    pub us_per_day: f64,
    pub u_ns_per_day: f64,
}

fn bad(s: impl Into<String>) -> String {
    s.into()
}

impl Registry {
    /// The built-in registry (version 1, provisional).
    pub fn builtin() -> Registry {
        Self::from_toml_str(BUILTIN_V1_TOML).expect("the built-in registry is valid")
    }

    /// Parse and validate a registry file.
    pub fn from_toml_str(text: &str) -> Result<Registry, String> {
        let r: Registry = toml::from_str(text).map_err(|e| format!("registry: {e}"))?;
        r.validate()?;
        Ok(r)
    }

    /// Read and validate a registry file.
    pub fn from_file(path: &std::path::Path) -> Result<Registry, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_toml_str(&text)
    }

    /// SHA-256 (hex) over the registry's canonical JSON (field order fixed by the type).
    pub fn sha256(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        hex::encode(Sha256::digest(&bytes))
    }

    /// The reference every output carries.
    pub fn reference(&self, definition_id: &str) -> RegistryRef {
        RegistryRef {
            registry_id: self.registry_id.clone(),
            registry_version: self.version,
            registry_sha256: self.sha256(),
            registry_status: self.status,
            definition_id: definition_id.to_string(),
        }
    }

    /// The definition named `id`.
    pub fn get(&self, id: &str) -> Result<&Definition, String> {
        self.definitions.iter().find(|d| d.id == id).ok_or_else(|| {
            let ids: Vec<&str> = self.definitions.iter().map(|d| d.id.as_str()).collect();
            format!(
                "registry {} v{} has no definition '{id}' (there is no default; name one of: {})",
                self.registry_id,
                self.version,
                ids.join(", ")
            )
        })
    }

    /// The registry's rules. See the module documentation.
    pub fn validate(&self) -> Result<(), String> {
        if self.registry_id.trim().is_empty() {
            return Err(bad("registry_id is empty"));
        }
        if self.version == 0 {
            return Err(bad("version starts at 1"));
        }
        if self.notice.trim().is_empty() {
            return Err(bad("a registry must carry a notice stating its status"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for d in &self.definitions {
            if d.id.trim().is_empty() || !seen.insert(d.id.as_str()) {
                return Err(format!("definition id '{}' is empty or repeated", d.id));
            }
        }
        match self.status {
            RegistryStatus::Provisional => {
                if self.decision.is_some() {
                    return Err(bad(
                        "a provisional registry cites no decision (set status = \"adopted\" with the decision)",
                    ));
                }
                if let Some(d) = self
                    .definitions
                    .iter()
                    .find(|d| d.status == DefStatus::Adopted)
                {
                    return Err(format!(
                        "definition '{}' is marked adopted in a provisional registry",
                        d.id
                    ));
                }
                let live = |f: Family| {
                    self.definitions
                        .iter()
                        .any(|d| d.family == f && d.status != DefStatus::Withdrawn)
                };
                if !(live(Family::Tcl) && live(Family::Tl)) {
                    return Err(bad(
                        "a provisional registry must carry both the TCL and the TL families: \
                         while the choice is open, one option alone would read as the standard",
                    ));
                }
                if !self.notice.to_ascii_uppercase().contains("PROVISIONAL") {
                    return Err(bad("a provisional registry's notice must say PROVISIONAL"));
                }
            }
            RegistryStatus::Adopted => {
                if self
                    .decision
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("")
                    .is_empty()
                {
                    return Err(bad(
                        "an adopted registry must cite the decision (resolution) it rests on",
                    ));
                }
            }
        }
        for d in &self.definitions {
            self.validate_definition(d)?;
        }
        Ok(())
    }

    fn validate_definition(&self, d: &Definition) -> Result<(), String> {
        let ctx = |m: &str| format!("definition '{}': {m}", d.id);
        match d.family {
            Family::Tcl | Family::Tl => {
                if !d.envelope_of.is_empty() {
                    return Err(ctx("only an LTC definition has envelope_of"));
                }
                RateSource::parse(d.rate_source.as_deref().unwrap_or("")).map_err(|e| ctx(&e))?;
                match (d.family, &d.w0) {
                    (Family::Tcl, Some(_)) => {
                        return Err(ctx("TCL is unscaled: it takes no w0"));
                    }
                    (Family::Tl, None) => return Err(ctx("TL needs a w0")),
                    (Family::Tl, Some(w)) => {
                        W0Candidate::parse(w).map_err(|e| ctx(&e))?;
                    }
                    _ => {}
                }
                if d.status == DefStatus::Placeholder {
                    return Err(ctx("only an LTC definition may be a placeholder"));
                }
            }
            Family::Ltc => {
                if d.rate_source.is_some() || d.w0.is_some() {
                    return Err(ctx(
                        "an LTC definition is realised by envelope_of, not its own model",
                    ));
                }
                if d.envelope_of.is_empty() {
                    return Err(ctx("envelope_of is empty"));
                }
                for m in &d.envelope_of {
                    let md = self.get(m).map_err(|e| ctx(&e))?;
                    if md.family == Family::Ltc {
                        return Err(ctx("an envelope member cannot itself be LTC"));
                    }
                    if md.status == DefStatus::Withdrawn {
                        return Err(ctx(&format!("envelope member '{m}' is withdrawn")));
                    }
                }
            }
        }
        Ok(())
    }

    /// The Kshana Pro model and lunar scale of a TCL or TL definition.
    fn model_of(&self, d: &Definition) -> Result<(Model, Scale), String> {
        let rs = RateSource::parse(d.rate_source.as_deref().unwrap_or(""))?;
        match d.family {
            Family::Tcl => Ok((Model::new(rs, W0Candidate::kshana_equatorial()), Scale::Tcl)),
            Family::Tl => {
                let w0 = W0Candidate::parse(d.w0.as_deref().unwrap_or(""))?;
                Ok((Model::new(rs, w0), Scale::Tl))
            }
            Family::Ltc => Err(format!("'{}' is LTC (an envelope)", d.id)),
        }
    }

    /// The models of the definitions `ids` (built once: a first-principles model averages over
    /// the 18.6-year nodal cycle).
    pub fn evaluator(&self) -> Evaluator<'_> {
        Evaluator {
            registry: self,
            models: BTreeMap::new(),
        }
    }

    /// `lunar − reference` (s) under definition `id` at the instant `at` (ISO, read in
    /// `reference`).
    pub fn lunar_offset(
        &self,
        id: &str,
        reference: RefScale,
        at: &str,
    ) -> Result<LunarOffset, String> {
        self.evaluator().lunar_offset(id, reference, at)
    }
}

/// Converts through a registry, caching each definition's model.
pub struct Evaluator<'a> {
    registry: &'a Registry,
    models: BTreeMap<String, (Model, Scale)>,
}

impl Evaluator<'_> {
    fn model(&mut self, d: &Definition) -> Result<(Model, Scale), String> {
        if let Some(m) = self.models.get(&d.id) {
            return Ok(m.clone());
        }
        let m = self.registry.model_of(d)?;
        self.models.insert(d.id.clone(), m.clone());
        Ok(m)
    }

    /// The budget of `lunar − reference` (s) at the reference-scale instant `jd`.
    pub fn offset_budget(
        &mut self,
        id: &str,
        reference: RefScale,
        jd: TwoPartJd,
    ) -> Result<(Budget, Vec<String>), String> {
        let d = self.registry.get(id)?.clone();
        if d.status == DefStatus::Withdrawn {
            return Err(format!("definition '{id}' is withdrawn"));
        }
        match d.family {
            Family::Tcl | Family::Tl => {
                let (model, scale) = self.model(&d)?;
                let (from, shift) = reference.lunar_pro_scale();
                // The instant read in GPST is the TAI instant 19 s later.
                let jd_from = jd.add_seconds(-shift);
                let conv = model.convert(from, jd_from, scale);
                let mut b = conv.offset;
                if shift != 0.0 {
                    // lunar − GPST = (lunar − TAI) + (TAI − GPST).
                    b.push(Term::exact(
                        "TAI - GPST",
                        -shift,
                        "19 s, constant by definition (IS-GPS-200)",
                    ));
                }
                Ok((b, conv.notes))
            }
            Family::Ltc => {
                let mut vals = Vec::new();
                let mut max_u: f64 = 0.0;
                let mut notes = Vec::new();
                for m in &d.envelope_of {
                    let (b, n) = self.offset_budget(m, reference, jd)?;
                    vals.push(b.value());
                    max_u = max_u.max(b.u_c());
                    for x in n {
                        if !notes.contains(&x) {
                            notes.push(x);
                        }
                    }
                }
                let lo = vals.iter().copied().fold(f64::INFINITY, f64::min);
                let hi = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let mid = 0.5 * (lo + hi);
                notes.push(format!(
                    "LTC has no adopted definition: envelope of {} candidate realisations ({}); \
                     the midpoint is not a recommendation",
                    d.envelope_of.len(),
                    d.envelope_of.join(", ")
                ));
                Ok((
                    Budget::new(vec![
                        Term::exact(
                            "LTC envelope midpoint",
                            mid,
                            "equal-weight midpoint of the candidate realisations",
                        ),
                        Term::rectangular(
                            "LTC definition undecided (half-spread of candidates)",
                            0.0,
                            0.5 * (hi - lo),
                            "spread of the candidate realisations; removed only by an adopted definition",
                        ),
                        Term::normal(
                            "LTC largest candidate model uncertainty",
                            0.0,
                            max_u,
                            kshana_pro::lunar_pro::budget::TermKind::B,
                            "the largest combined uncertainty among the candidates (conservative)",
                        ),
                    ]),
                    notes,
                ))
            }
        }
    }

    /// `lunar − reference` (s) under definition `id` at the instant `at`.
    pub fn lunar_offset(
        &mut self,
        id: &str,
        reference: RefScale,
        at: &str,
    ) -> Result<LunarOffset, String> {
        let jd = lt::parse_iso(at)?;
        let d = self.registry.get(id)?.clone();
        let (budget, mut notes) = self.offset_budget(id, reference, jd)?;
        if self.registry.status == RegistryStatus::Provisional {
            notes.insert(0, self.registry.notice.clone());
        }
        Ok(LunarOffset {
            registry: self.registry.reference(id),
            family: d.family,
            reference,
            at: at.to_string(),
            offset_s: budget.value(),
            u_c_s: budget.u_c(),
            expanded_k2_s: budget.expanded(2.0),
            budget,
            notes,
        })
    }

    /// The secular rate `d(lunar)/d(TT) − 1` under definition `id`. For LTC: the envelope
    /// midpoint, with the half-spread and the largest candidate uncertainty in quadrature.
    pub fn rate(&mut self, id: &str) -> Result<LunarRate, String> {
        let d = self.registry.get(id)?.clone();
        let (rate, u) = match d.family {
            Family::Tcl | Family::Tl => {
                let (model, scale) = self.model(&d)?;
                let b = model.rate_vs_tt(scale);
                (b.value(), b.u_c())
            }
            Family::Ltc => {
                let mut rs = Vec::new();
                let mut max_u: f64 = 0.0;
                for m in &d.envelope_of {
                    let r = self.rate(m)?;
                    rs.push(r.rate);
                    max_u = max_u.max(r.u_rate);
                }
                let lo = rs.iter().copied().fold(f64::INFINITY, f64::min);
                let hi = rs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let half = 0.5 * (hi - lo);
                (
                    0.5 * (lo + hi),
                    ((half / 3f64.sqrt()).powi(2) + max_u * max_u).sqrt(),
                )
            }
        };
        Ok(LunarRate {
            definition_id: id.to_string(),
            family: d.family,
            rate,
            u_rate: u,
            us_per_day: rate * 86_400.0 * 1e6,
            u_ns_per_day: u * 86_400.0 * 1e9,
        })
    }
}

/// How one definition changed between two registry versions.
#[derive(Clone, Debug, Serialize)]
pub struct DefinitionChange {
    pub id: String,
    /// `unchanged`, `parameters-changed`, `status-changed`, `added`, `removed`.
    pub change: String,
    pub old_status: Option<DefStatus>,
    pub new_status: Option<DefStatus>,
    /// Per epoch: `(epoch, old offset, new offset, new − old)` of `lunar − UTC` (s).
    pub offsets: Vec<OffsetDelta>,
    pub max_abs_delta_s: f64,
    /// True when the parameters changed or an offset moved by more than the tolerance.
    pub requalify: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OffsetDelta {
    pub epoch_utc: String,
    pub old_s: f64,
    pub new_s: f64,
    pub delta_s: f64,
    pub old_u_c_s: f64,
    pub new_u_c_s: f64,
}

/// The impact of moving from registry `old` to registry `new`.
#[derive(Clone, Debug, Serialize)]
pub struct ChangeImpact {
    pub old: RegistryRef,
    pub new: RegistryRef,
    pub status_change: Option<(RegistryStatus, RegistryStatus)>,
    pub tolerance_s: f64,
    pub epochs_utc: Vec<String>,
    pub definitions: Vec<DefinitionChange>,
    /// The definitions whose users must re-qualify.
    pub requalify: Vec<String>,
    pub evidence_class: &'static str,
}

/// Compare two registry versions at the given UTC epochs. A definition that changed
/// parameters, or whose `lunar − UTC` moves by more than `tolerance_s` at any epoch, is
/// flagged for re-qualification. Definitions added or removed are listed (a removed
/// definition always needs action from its users).
pub fn change_impact(
    old: &Registry,
    new: &Registry,
    epochs_utc: &[String],
    tolerance_s: f64,
) -> Result<ChangeImpact, String> {
    if old.registry_id != new.registry_id {
        return Err(format!(
            "different registries ({} vs {})",
            old.registry_id, new.registry_id
        ));
    }
    if new.version <= old.version {
        return Err(format!(
            "the new registry's version ({}) must be greater than the old one's ({})",
            new.version, old.version
        ));
    }
    if !(tolerance_s.is_finite() && tolerance_s >= 0.0) {
        return Err("tolerance must be finite and non-negative".into());
    }
    let mut ids: Vec<&str> = old.definitions.iter().map(|d| d.id.as_str()).collect();
    for d in &new.definitions {
        if !ids.contains(&d.id.as_str()) {
            ids.push(d.id.as_str());
        }
    }
    let mut eo = old.evaluator();
    let mut en = new.evaluator();
    let mut defs = Vec::new();
    for id in ids {
        let o = old.get(id).ok();
        let n = new.get(id).ok();
        let (change, offsets) = match (o, n) {
            (Some(_), None) => ("removed".to_string(), Vec::new()),
            (None, Some(_)) => ("added".to_string(), Vec::new()),
            (Some(od), Some(nd)) => {
                let params_same = od.family == nd.family
                    && od.rate_source == nd.rate_source
                    && od.w0 == nd.w0
                    && od.envelope_of == nd.envelope_of;
                let mut offs = Vec::new();
                let live = od.status != DefStatus::Withdrawn && nd.status != DefStatus::Withdrawn;
                if live {
                    for e in epochs_utc {
                        let a = eo.lunar_offset(id, RefScale::Utc, e)?;
                        let b = en.lunar_offset(id, RefScale::Utc, e)?;
                        offs.push(OffsetDelta {
                            epoch_utc: e.clone(),
                            old_s: a.offset_s,
                            new_s: b.offset_s,
                            delta_s: b.offset_s - a.offset_s,
                            old_u_c_s: a.u_c_s,
                            new_u_c_s: b.u_c_s,
                        });
                    }
                }
                let c = if !params_same {
                    "parameters-changed"
                } else if od.status != nd.status {
                    "status-changed"
                } else {
                    "unchanged"
                };
                (c.to_string(), offs)
            }
            (None, None) => unreachable!("id comes from one of the registries"),
        };
        let max_abs = offsets
            .iter()
            .map(|x| x.delta_s.abs())
            .fold(0.0_f64, f64::max);
        let requalify =
            matches!(change.as_str(), "parameters-changed" | "removed") || max_abs > tolerance_s;
        defs.push(DefinitionChange {
            id: id.to_string(),
            change,
            old_status: o.map(|d| d.status),
            new_status: n.map(|d| d.status),
            offsets,
            max_abs_delta_s: max_abs,
            requalify,
        });
    }
    let requalify = defs
        .iter()
        .filter(|d| d.requalify)
        .map(|d| d.id.clone())
        .collect();
    Ok(ChangeImpact {
        old: old.reference("*"),
        new: new.reference("*"),
        status_change: (old.status != new.status).then_some((old.status, new.status)),
        tolerance_s,
        epochs_utc: epochs_utc.to_vec(),
        definitions: defs,
        requalify,
        evidence_class: "MODELLED",
    })
}

/// One row of the cross-definition comparison at an epoch.
#[derive(Clone, Debug, Serialize)]
pub struct SpreadRow {
    pub definition_id: String,
    pub family: Family,
    pub status: DefStatus,
    pub lunar_minus_utc_s: f64,
    pub u_c_s: f64,
    pub rate_us_per_day: f64,
    pub u_rate_ns_per_day: f64,
}

/// Every live definition's `lunar − UTC` and rate at `epoch_utc`, and the spread across the
/// TCL/TL candidates (s): how much the undecided choice matters at that epoch.
pub fn cross_definition_spread(
    reg: &Registry,
    epoch_utc: &str,
) -> Result<(Vec<SpreadRow>, f64), String> {
    let mut ev = reg.evaluator();
    let mut rows = Vec::new();
    for d in &reg.definitions {
        if d.status == DefStatus::Withdrawn {
            continue;
        }
        let o = ev.lunar_offset(&d.id, RefScale::Utc, epoch_utc)?;
        let r = ev.rate(&d.id)?;
        rows.push(SpreadRow {
            definition_id: d.id.clone(),
            family: d.family,
            status: d.status,
            lunar_minus_utc_s: o.offset_s,
            u_c_s: o.u_c_s,
            rate_us_per_day: r.us_per_day,
            u_rate_ns_per_day: r.u_ns_per_day,
        });
    }
    let cand: Vec<f64> = rows
        .iter()
        .filter(|r| r.family != Family::Ltc)
        .map(|r| r.lunar_minus_utc_s)
        .collect();
    let spread = cand.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - cand.iter().copied().fold(f64::INFINITY, f64::min);
    Ok((rows, spread))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_is_provisional_with_both_families_and_no_adopted_entry() {
        let r = Registry::builtin();
        assert_eq!(r.status, RegistryStatus::Provisional);
        assert!(r.decision.is_none());
        assert!(r.definitions.iter().any(|d| d.family == Family::Tcl));
        assert!(r.definitions.iter().any(|d| d.family == Family::Tl));
        assert!(r.definitions.iter().all(|d| d.status != DefStatus::Adopted));
        assert!(r.notice.contains("PROVISIONAL"));
        assert_eq!(r.sha256().len(), 64);
    }

    #[test]
    fn gpst_offset_is_tai_offset_plus_19_s() {
        let r = Registry::builtin();
        let a = r
            .lunar_offset("tcl-iau2024-lte440", RefScale::Tai, "2027-01-01T00:00:00")
            .unwrap();
        // The same physical instant read in GPST is 19 s earlier on the GPST clock.
        let b = r
            .lunar_offset("tcl-iau2024-lte440", RefScale::Gpst, "2026-12-31T23:59:41")
            .unwrap();
        assert!(
            (b.offset_s - (a.offset_s + 19.0)).abs() < 1e-9,
            "{} vs {}",
            b.offset_s,
            a.offset_s
        );
    }
}
