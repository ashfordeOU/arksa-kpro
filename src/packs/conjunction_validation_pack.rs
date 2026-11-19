// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # `pro-conjunction-validation` (Arksa Ops pillar; research 06 G1/G2, 06b findings 1–3)
//!
//! One scenario runs any of: per-event consistency checks on a provider's conjunction events,
//! a scorecard of a provider against a reference set (such as the public TraCSS verification
//! answer key), a cross-provider comparison with explained divergence, and covariance-realism
//! tests. The engine is [`crate::conjunction`]; this pack reads the scenario, loads and hashes
//! the inputs, and writes one result document (and, from the command, a signed directory).
//!
//! ```toml
//! kind = "pro-conjunction-validation"
//! title = "Provider X against the TraCSS verification answer key (spherical 10 km)"
//!
//! [[source]]
//! name = "key"
//! files = ["IVV_Releasable_Dataset_Spherical_DefaultHBR.csv"]   # results table (auto-detected)
//!
//! [[source]]
//! name = "provider-x"
//! files = ["cdms/"]          # a directory: every file in it, sorted
//!
//! [scorecard]
//! reference = "key"
//! test = "provider-x"
//! window_utc = ["2025-01-01T12:00:00Z", "2025-01-08T12:00:00Z"]
//! screening = "sphere"
//! sphere_radius_km = 10.0
//! ```
//!
//! Evidence class: MODELLED (see [`crate::conjunction::LABEL`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kshana::api::{ExternalPack, KshanaError, RunOutput, Scenario, ScenarioMeta};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::conjunction::{
    self as cj, answer_key, cdm, compare, ephemeris, fmt_utc, parse_utc, realism, scorecard, Event,
};

pub use crate::conjunction::{KIND, LABEL, SCHEMA};

fn bad(m: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(format!("{KIND}: {}", m.into()))
}

// ── Scenario ─────────────────────────────────────────────────────────────────────────────

/// A named set of conjunction events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    pub name: String,
    /// `cdm` or `results-table`; detected from the content when omitted.
    #[serde(default)]
    pub format: Option<String>,
    /// Files or directories (every regular file in a directory, sorted).
    #[serde(default)]
    pub files: Vec<String>,
    /// Inline texts.
    #[serde(default)]
    pub text: Vec<String>,
    /// Units of a results table's covariance columns: `km2` (default) or `m2`.
    #[serde(default)]
    pub covariance_unit: Option<String>,
    /// Hard-body radius (m) given to events that state none.
    #[serde(default)]
    pub hbr_m: Option<f64>,
}

/// A `[[check]]` or `[scorecard]` section: the sources, the screening configuration the
/// events were produced with, and the thresholds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreeningSpec {
    /// `[[check]]`: the source checked.
    #[serde(default)]
    pub source: Option<String>,
    /// `[scorecard]`: the reference and the test source.
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub test: Option<String>,
    #[serde(default)]
    pub window_utc: Option<[String; 2]>,
    /// `none` (default), `sphere` or `sfsh`.
    #[serde(default)]
    pub screening: Option<String>,
    #[serde(default)]
    pub sphere_radius_km: Option<f64>,
    /// Per-object rectangular half sizes for `sfsh` (km), overriding the regime table.
    #[serde(default)]
    pub volume: Vec<VolumeSpec>,
    #[serde(default)]
    pub gate_s: Option<f64>,
    /// TCA objectives (s) for VLRV, LRV and NLRV.
    #[serde(default)]
    pub tca_objectives_s: Option<[f64; 3]>,
    #[serde(default)]
    pub position_objective_m: Option<f64>,
    #[serde(default)]
    pub bhattacharyya_objective: Option<f64>,
    #[serde(default)]
    pub boundary_m: Option<f64>,
    #[serde(default)]
    pub od_age_days: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeSpec {
    pub object: String,
    pub u_km: f64,
    pub v_km: f64,
    pub w_km: f64,
}

/// `[scorecard]`: needs `reference` and `test`.
pub type ScorecardSpec = ScreeningSpec;
/// `[[check]]`: needs `source`.
pub type CheckSpec = ScreeningSpec;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySpec {
    pub name: String,
    #[serde(default)]
    pub pc_ge: Option<f64>,
    #[serde(default)]
    pub miss_le_m: Option<f64>,
    /// `any` (default) or `all`.
    #[serde(default)]
    pub combine: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompareSpec {
    pub a: String,
    pub b: String,
    #[serde(default)]
    pub period_fraction: Option<f64>,
    #[serde(default)]
    pub fallback_gate_s: Option<f64>,
    #[serde(default)]
    pub default_hbr_m: Option<f64>,
    /// One policy for both, or one each.
    #[serde(default)]
    pub policy: Option<PolicySpec>,
    #[serde(default)]
    pub policy_a: Option<PolicySpec>,
    #[serde(default)]
    pub policy_b: Option<PolicySpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealismSpec {
    #[serde(default)]
    pub label: Option<String>,
    /// Object class (default: from the predicted state).
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub predicted_file: Option<String>,
    #[serde(default)]
    pub predicted_text: Option<String>,
    #[serde(default)]
    pub reference_file: Option<String>,
    #[serde(default)]
    pub reference_text: Option<String>,
    /// Use only this OEM segment (`OBJECT_ID`) of each file.
    #[serde(default)]
    pub object_id: Option<String>,
    #[serde(default)]
    pub prediction_epoch_utc: Option<String>,
    #[serde(default)]
    pub residuals_file: Option<String>,
    #[serde(default)]
    pub residuals_text: Option<String>,
}

/// A `pro-conjunction-validation` scenario.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConjunctionValidationScenario {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub source: Vec<SourceSpec>,
    #[serde(default)]
    pub check: Vec<CheckSpec>,
    #[serde(default)]
    pub scorecard: Option<ScorecardSpec>,
    #[serde(default)]
    pub compare: Vec<CompareSpec>,
    #[serde(default)]
    pub realism: Vec<RealismSpec>,
    #[serde(default)]
    pub realism_age_bins_h: Option<Vec<f64>>,
    #[serde(default)]
    pub realism_min_samples: Option<usize>,
    /// OMM files or texts: object classes and SFSH volumes for the objects they describe.
    #[serde(default)]
    pub omm_files: Vec<String>,
    #[serde(default)]
    pub omm_text: Vec<String>,
    /// Directory relative paths resolve against (set by [`Self::resolve_files`]).
    #[serde(skip)]
    pub base: Option<PathBuf>,
}

impl ConjunctionValidationScenario {
    pub const KIND: &'static str = KIND;

    pub fn meta_static() -> ScenarioMeta {
        ScenarioMeta {
            name: Self::KIND,
            description: "Conjunction-assessment validation: CCSDS CDM, OEM and OMM ingest; consistency checks; a scorecard of a provider's conjunction events against a reference set such as the public TraCSS verification answer key; covariance-realism tests by prediction age and object class; cross-provider comparison of the same event with the divergence explained. Signed evidence. MODELLED.",
            required_fields: &[],
            optional_fields: &[
                "title",
                "source",
                "check",
                "scorecard",
                "compare",
                "realism",
                "realism_age_bins_h",
                "realism_min_samples",
                "omm_files",
                "omm_text",
            ],
        }
    }

    /// Resolve relative file names against `base` (the scenario's directory).
    pub fn resolve_files(&mut self, base: &Path) -> Result<(), KshanaError> {
        self.base = Some(base.to_path_buf());
        Ok(())
    }

    fn path(&self, p: &str) -> PathBuf {
        match &self.base {
            Some(b) => b.join(p),
            None => PathBuf::from(p),
        }
    }
}

// ── Inputs ──────────────────────────────────────────────────────────────────────────────

fn sha_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn read(path: &Path) -> Result<String, KshanaError> {
    std::fs::read_to_string(path).map_err(|e| bad(format!("{}: {e}", path.display())))
}

/// Expand files and directories into a sorted list of files.
fn expand(
    s: &ConjunctionValidationScenario,
    names: &[String],
) -> Result<Vec<PathBuf>, KshanaError> {
    let mut out = Vec::new();
    for n in names {
        let p = s.path(n);
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                .map_err(|e| bad(format!("{}: {e}", p.display())))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file())
                .collect();
            v.sort();
            out.extend(v);
        } else {
            out.push(p);
        }
    }
    Ok(out)
}

fn events_of(
    text: &str,
    format: Option<&str>,
    unit: answer_key::CovUnit,
) -> Result<Vec<Event>, KshanaError> {
    let fmt = match format {
        Some(f) => f.to_ascii_lowercase(),
        None if answer_key::looks_like(text) => "results-table".into(),
        None => "cdm".into(),
    };
    match fmt.as_str() {
        "cdm" => cdm::events(text),
        "results-table" | "csv" => answer_key::read(std::io::Cursor::new(text), unit, None),
        other => Err(bad(format!(
            "unknown source format '{other}' (cdm or results-table)"
        ))),
    }
}

/// Load one source: its events and an input record (names, SHA-256, counts).
pub fn load_source(
    s: &ConjunctionValidationScenario,
    src: &SourceSpec,
) -> Result<(Vec<Event>, Value), KshanaError> {
    let unit = match src.covariance_unit.as_deref() {
        None | Some("km2") => answer_key::CovUnit::Km2,
        Some("m2") => answer_key::CovUnit::M2,
        Some(o) => return Err(bad(format!("covariance_unit '{o}' (km2 or m2)"))),
    };
    let mut events = Vec::new();
    let mut files = Vec::new();
    let mut table = false;
    for p in expand(s, &src.files)? {
        let t = read(&p)?;
        table |= answer_key::looks_like(&t);
        let ev = events_of(&t, src.format.as_deref(), unit)
            .map_err(|e| bad(format!("{}: {e}", p.display())))?;
        files.push(json!({
            "file": p.file_name().map(|f| f.to_string_lossy().to_string()),
            "sha256": sha_hex(t.as_bytes()), "events": ev.len(),
        }));
        events.extend(ev);
    }
    for (i, t) in src.text.iter().enumerate() {
        table |= answer_key::looks_like(t);
        let ev = events_of(t, src.format.as_deref(), unit)?;
        files.push(json!({"inline": i, "sha256": sha_hex(t.as_bytes()), "events": ev.len()}));
        events.extend(ev);
    }
    if files.is_empty() {
        return Err(bad(format!("source '{}' has no files or text", src.name)));
    }
    let mut hbr_filled = 0usize;
    if let Some(h) = src.hbr_m {
        for e in events.iter_mut().filter(|e| e.hbr_m.is_none()) {
            e.hbr_m = Some(h);
            hbr_filled += 1;
        }
    }
    let rec = json!({
        "name": src.name, "inputs": files, "events": events.len(),
        "hbr_m_given_to_events_without_one": src.hbr_m.map(|h| json!({"hbr_m": h, "events": hbr_filled})),
        "results_table_assumptions": table.then_some(answer_key::ASSUMPTIONS),
        "results_table_covariance_unit": table.then(|| src.covariance_unit.clone().unwrap_or_else(|| "km2".into())),
    });
    Ok((events, rec))
}

fn config(
    sp: &ScreeningSpec,
    omm_volumes: &BTreeMap<String, cj::V3>,
) -> Result<scorecard::Config, KshanaError> {
    let window = match &sp.window_utc {
        Some([a, b]) => {
            let (a, b) = (parse_utc(a)?, parse_utc(b)?);
            if b <= a {
                return Err(bad("window_utc: the end is not after the start"));
            }
            Some((a, b))
        }
        None => None,
    };
    let screening = match sp.screening.as_deref().unwrap_or("none") {
        "none" => scorecard::Screening::None,
        "sphere" => scorecard::Screening::Sphere {
            radius_m: sp.sphere_radius_km.unwrap_or(10.0) * 1000.0,
        },
        "sfsh" => {
            let mut table = omm_volumes.clone();
            for v in &sp.volume {
                table.insert(
                    cj::norm_id(&v.object),
                    [v.u_km * 1e3, v.v_km * 1e3, v.w_km * 1e3],
                );
            }
            scorecard::Screening::Sfsh { table }
        }
        o => return Err(bad(format!("screening '{o}' (none, sphere or sfsh)"))),
    };
    let mut o = scorecard::Objectives::default();
    if let Some(x) = sp.gate_s {
        o.gate_s = x;
    }
    if let Some(x) = sp.tca_objectives_s {
        o.tca_s = x;
    }
    if let Some(x) = sp.position_objective_m {
        o.position_m = x;
    }
    if let Some(x) = sp.bhattacharyya_objective {
        o.bhattacharyya = x;
    }
    if let Some(x) = sp.boundary_m {
        o.boundary_m = x;
    }
    if let Some(x) = sp.od_age_days {
        o.od_age_days = x;
    }
    Ok(scorecard::Config {
        window,
        screening,
        objectives: o,
    })
}

fn describe_config(c: &scorecard::Config) -> Value {
    let o = &c.objectives;
    json!({
        "window": c.window.map(|(a, b)| json!([fmt_utc(a), fmt_utc(b)])),
        "screening": match &c.screening {
            scorecard::Screening::None => json!("none"),
            scorecard::Screening::Sphere { radius_m } => json!({"sphere_radius_m": radius_m}),
            scorecard::Screening::Sfsh { table } => json!({"sfsh": "Space Flight Safety Handbook regime table (TraCSS dataset guide, Table 3)", "explicit_volumes": table.len()}),
        },
        "objectives": {
            "tca_s": {"vlrv": o.tca_s[0], "lrv": o.tca_s[1], "nlrv": o.tca_s[2]},
            "velocity_classes_m_s": {"vlrv_below": o.vlrv_below_m_s, "lrv_below": o.lrv_below_m_s},
            "position_m": o.position_m, "bhattacharyya": o.bhattacharyya,
            "boundary_m": o.boundary_m, "od_age_days": o.od_age_days, "match_gate_s": o.gate_s,
        },
    })
}

fn policy(p: &Option<PolicySpec>) -> Result<Option<compare::Policy>, KshanaError> {
    p.as_ref()
        .map(|p| {
            Ok(compare::Policy {
                name: p.name.clone(),
                pc_ge: p.pc_ge,
                miss_le_m: p.miss_le_m,
                all: match p.combine.as_deref() {
                    None | Some("any") => false,
                    Some("all") => true,
                    Some(o) => return Err(bad(format!("policy combine '{o}' (any or all)"))),
                },
            })
        })
        .transpose()
}

// ── The study ───────────────────────────────────────────────────────────────────────────

/// A study result: the document and the per-event rows (scorecard and comparisons).
pub struct Study {
    pub doc: Value,
    pub rows: Vec<Value>,
}

/// Rows kept inline in the document (the CSV always has them all).
const INLINE_ROWS: usize = 2000;

/// Run the scenario.
pub fn run_study(s: &ConjunctionValidationScenario) -> Result<Study, KshanaError> {
    if s.check.is_empty() && s.scorecard.is_none() && s.compare.is_empty() && s.realism.is_empty() {
        return Err(bad(
            "nothing to do: give [[check]], [scorecard], [[compare]] or [[realism]]",
        ));
    }
    // OMM object classes.
    let mut omm = Vec::new();
    let mut omm_volumes = BTreeMap::new();
    let mut omm_texts: Vec<(String, String)> = Vec::new();
    for p in expand(s, &s.omm_files)? {
        omm_texts.push((p.display().to_string(), read(&p)?));
    }
    for (i, t) in s.omm_text.iter().enumerate() {
        omm_texts.push((format!("inline {i}"), t.clone()));
    }
    for (name, t) in &omm_texts {
        let (id, st) = ephemeris::omm_class_state(t).map_err(|e| bad(format!("{name}: {e}")))?;
        let (r, v) = ([st[0], st[1], st[2]], [st[3], st[4], st[5]]);
        let (rid, rlabel, vol) = cj::sfsh_regime(r, v);
        omm_volumes.insert(id.clone(), vol);
        omm.push(
            json!({"object": id, "class": cj::object_class(r, v), "sfsh_regime": rid,
                        "sfsh_label": rlabel, "sha256": sha_hex(t.as_bytes())}),
        );
    }
    // Sources.
    let mut sources: BTreeMap<String, Vec<Event>> = BTreeMap::new();
    let mut records = Vec::new();
    for src in &s.source {
        if sources.contains_key(&src.name) {
            return Err(bad(format!("source '{}' is given twice", src.name)));
        }
        let (ev, rec) = load_source(s, src)?;
        sources.insert(src.name.clone(), ev);
        records.push(rec);
    }
    let get = |n: &str| {
        sources
            .get(n)
            .ok_or_else(|| bad(format!("no source named '{n}'")))
    };
    let mut rows = Vec::new();
    let mut checks = Vec::new();
    for c in &s.check {
        let src = c
            .source
            .as_deref()
            .ok_or_else(|| bad("[[check]] needs source"))?;
        let cfg = config(c, &omm_volumes)?;
        checks.push(json!({"source": src, "config": describe_config(&cfg),
                           "result": scorecard::check_all(get(src)?, &cfg)}));
    }
    let score = match &s.scorecard {
        Some(sc) => {
            let (Some(rn), Some(tn)) = (sc.reference.as_deref(), sc.test.as_deref()) else {
                return Err(bad("[scorecard] needs reference and test"));
            };
            let cfg = config(sc, &omm_volumes)?;
            let (mut doc, r) = scorecard::scorecard(get(rn)?, get(tn)?, &cfg);
            doc["reference"] = json!(rn);
            doc["test"] = json!(tn);
            doc["config"] = describe_config(&cfg);
            rows.extend(r.into_iter().map(|mut x| {
                x["section"] = json!("scorecard");
                x
            }));
            Some(doc)
        }
        None => None,
    };
    let mut comps = Vec::new();
    for c in &s.compare {
        let mut cfg = compare::CompareConfig::default();
        if let Some(x) = c.period_fraction {
            cfg.period_fraction = x;
        }
        if let Some(x) = c.fallback_gate_s {
            cfg.fallback_gate_s = x;
        }
        if let Some(x) = c.default_hbr_m {
            cfg.default_hbr_m = x;
        }
        if let Some(p) = policy(&c.policy)? {
            cfg.policy_a = p.clone();
            cfg.policy_b = p;
        }
        if let Some(p) = policy(&c.policy_a)? {
            cfg.policy_a = p;
        }
        if let Some(p) = policy(&c.policy_b)? {
            cfg.policy_b = p;
        }
        let (mut doc, r) = compare::compare(get(&c.a)?, get(&c.b)?, &cfg);
        doc["a"] = json!(c.a);
        doc["b"] = json!(c.b);
        rows.extend(r.into_iter().map(|mut x| {
            x["section"] = json!(format!("compare {} vs {}", c.a, c.b));
            x
        }));
        comps.push(doc);
    }
    let realism_doc = if s.realism.is_empty() {
        None
    } else {
        let mut samples = Vec::new();
        let mut inputs = Vec::new();
        let mut warnings = Vec::new();
        for (i, r) in s.realism.iter().enumerate() {
            let text =
                |f: &Option<String>, t: &Option<String>| -> Result<Option<String>, KshanaError> {
                    match (f, t) {
                        (Some(f), _) => Ok(Some(read(&s.path(f))?)),
                        (None, Some(t)) => Ok(Some(t.clone())),
                        _ => Ok(None),
                    }
                };
            let label = r.label.clone().unwrap_or_else(|| format!("realism {i}"));
            if let Some(t) = text(&r.residuals_file, &r.residuals_text)? {
                let mut ss = realism::samples_from_csv(&t)?;
                if let Some(c) = &r.class {
                    ss.iter_mut().for_each(|x| x.class = c.clone());
                }
                inputs.push(json!({"label": label, "residuals_sha256": sha_hex(t.as_bytes()), "samples": ss.len()}));
                samples.extend(ss);
                continue;
            }
            let (Some(pt), Some(rt)) = (
                text(&r.predicted_file, &r.predicted_text)?,
                text(&r.reference_file, &r.reference_text)?,
            ) else {
                return Err(bad(format!(
                    "{label}: give residuals, or a predicted and a reference ephemeris"
                )));
            };
            let pick =
                |v: Vec<ephemeris::Ephemeris>| -> Result<Vec<ephemeris::Ephemeris>, KshanaError> {
                    match &r.object_id {
                        Some(id) => {
                            let v: Vec<_> = v.into_iter().filter(|e| &e.object_id == id).collect();
                            if v.is_empty() {
                                return Err(bad(format!("{label}: no OEM segment for {id}")));
                            }
                            Ok(v)
                        }
                        None => Ok(v),
                    }
                };
            let preds = pick(ephemeris::parse_oem(&pt)?)?;
            let refs = pick(ephemeris::parse_oem(&rt)?)?;
            let t0 = r
                .prediction_epoch_utc
                .as_deref()
                .map(parse_utc)
                .transpose()?;
            let mut n = 0;
            for p in &preds {
                let Some(rf) = refs.iter().find(|x| x.object_id == p.object_id) else {
                    warnings.push(format!("{label}: no reference segment for {}", p.object_id));
                    continue;
                };
                let (ss, w) = realism::samples_from_oem(p, rf, r.class.as_deref(), t0)?;
                n += ss.len();
                samples.extend(ss);
                warnings.extend(w);
            }
            inputs.push(
                json!({"label": label, "predicted_sha256": sha_hex(pt.as_bytes()),
                               "reference_sha256": sha_hex(rt.as_bytes()), "samples": n}),
            );
        }
        let mut cfg = realism::RealismConfig::default();
        if let Some(b) = &s.realism_age_bins_h {
            if b.len() < 2 || b.windows(2).any(|w| w[1] <= w[0]) {
                return Err(bad(
                    "realism_age_bins_h must be increasing, at least two edges",
                ));
            }
            cfg.age_bins_h = b.clone();
        }
        if let Some(n) = s.realism_min_samples {
            cfg.min_samples = n;
        }
        let mut d = realism::analyse(&samples, &cfg);
        d["inputs"] = json!(inputs);
        d["warnings"] = json!(warnings);
        Some(d)
    };
    let mut doc = json!({
        "schema": SCHEMA,
        "kind": KIND,
        "title": s.title.clone().unwrap_or_else(|| "Conjunction-assessment validation".into()),
        "label": LABEL,
        "evidence_class": "MODELLED",
        "time_scale": "UTC as calendar seconds; leap seconds not counted between two times",
        "references": cj::REFERENCES,
        "endorsement": "No agency, provider or dataset publisher endorses this result. A public dataset is used as published under its licence.",
        "sources": records,
        "objects_from_omm": omm,
        "checks": checks,
        "scorecard": score,
        "compare": comps,
        "realism": realism_doc,
        "event_rows": rows.len(),
    });
    if rows.len() <= INLINE_ROWS {
        doc["rows"] = json!(rows);
    }
    cj::sanitise(&mut doc);
    let digest = sha_hex(&serde_json::to_vec(&doc).map_err(|e| bad(e.to_string()))?);
    doc["result_sha256"] = json!(digest);
    Ok(Study { doc, rows })
}

// ── Output ──────────────────────────────────────────────────────────────────────────────

fn f(v: &Value) -> String {
    match v {
        Value::Null => "–".into(),
        Value::Number(n) => {
            let x = n.as_f64().unwrap_or(0.0);
            if n.is_i64() || n.is_u64() {
                n.to_string()
            } else if x != 0.0 && (x.abs() < 1e-3 || x.abs() >= 1e6) {
                format!("{x:.3e}")
            } else {
                format!("{x:.4}")
            }
        }
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A short text summary.
pub fn summary(doc: &Value) -> String {
    let mut s = format!(
        "{} ({})\n",
        doc["title"].as_str().unwrap_or(""),
        doc["evidence_class"].as_str().unwrap_or("")
    );
    for c in doc["checks"].as_array().into_iter().flatten() {
        let flagged = c["result"]["checks"]
            .as_array()
            .map(|v| v.iter().filter(|x| x["status"] == "FLAG").count())
            .unwrap_or(0);
        s.push_str(&format!(
            "checks on '{}': {} events, {flagged} checks flagged\n",
            f(&c["source"]),
            f(&c["result"]["events"])
        ));
    }
    let sc = &doc["scorecard"];
    if !sc.is_null() {
        s.push_str(&format!(
            "scorecard '{}' vs '{}': {} matched of {} reference / {} test events; {} objectives flagged\n",
            f(&sc["test"]),
            f(&sc["reference"]),
            f(&sc["matched"]),
            f(&sc["reference_events"]),
            f(&sc["test_events"]),
            f(&sc["objectives_flagged"])
        ));
    }
    for c in doc["compare"].as_array().into_iter().flatten() {
        s.push_str(&format!(
            "compare '{}' vs '{}': {} matched, {} only in A, {} only in B, {} classification conflicts\n",
            f(&c["a"]),
            f(&c["b"]),
            f(&c["matched"]),
            f(&c["only_a"]),
            f(&c["only_b"]),
            f(&c["classification_conflicts"])
        ));
    }
    let r = &doc["realism"];
    if !r.is_null() {
        s.push_str(&format!(
            "covariance realism: {} samples, overall scale factor {} ({})\n",
            f(&r["samples"]),
            f(&r["overall"]["sigma_scale_factor"]),
            f(&r["overall"]["verdict"])
        ));
    }
    s
}

/// The report (Markdown).
pub fn report_md(doc: &Value) -> String {
    let mut s = format!("# {}\n\n", doc["title"].as_str().unwrap_or(""));
    s.push_str(&format!(
        "Evidence class: {}. {}\n\n",
        f(&doc["evidence_class"]),
        f(&doc["label"])
    ));
    s.push_str("## Inputs\n\n| Source | Events | Inputs (SHA-256) |\n|---|---|---|\n");
    for r in doc["sources"].as_array().into_iter().flatten() {
        let ins: Vec<String> = r["inputs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|i| {
                format!(
                    "{} `{}`",
                    i["file"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("inline {}", f(&i["inline"]))),
                    &i["sha256"].as_str().unwrap_or("")
                        [..12.min(i["sha256"].as_str().unwrap_or("").len())]
                )
            })
            .collect();
        s.push_str(&format!(
            "| {} | {} | {} |\n",
            f(&r["name"]),
            f(&r["events"]),
            ins.join("; ")
        ));
        if let Some(a) = r["results_table_assumptions"].as_str() {
            s.push_str(&format!("\nAssumption ({}): {a}\n\n", f(&r["name"])));
        }
    }
    let check_table = |s: &mut String, c: &Value| {
        s.push_str("| Check | Pass | Flag | Not evaluated | Status |\n|---|---|---|---|---|\n");
        for x in c["checks"].as_array().into_iter().flatten() {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                f(&x["what"]),
                f(&x["pass"]),
                f(&x["flag"]),
                f(&x["not_evaluated"]),
                f(&x["status"])
            ));
        }
    };
    for c in doc["checks"].as_array().into_iter().flatten() {
        s.push_str(&format!("\n## Consistency checks: {}\n\n", f(&c["source"])));
        check_table(&mut s, &c["result"]);
    }
    let sc = &doc["scorecard"];
    if !sc.is_null() {
        s.push_str(&format!(
            "\n## Scorecard: {} against {}\n\n{} reference events, {} test events ({} and {} duplicate reports collapsed), {} matched (gate {} s).\n\n",
            f(&sc["test"]), f(&sc["reference"]), f(&sc["reference_events"]), f(&sc["test_events"]),
            f(&sc["reference_duplicates_collapsed"]), f(&sc["test_duplicates_collapsed"]),
            f(&sc["matched"]), f(&sc["match_gate_s"])
        ));
        s.push_str("| Objective | Detail | Status |\n|---|---|---|\n");
        for o in sc["objectives"].as_array().into_iter().flatten() {
            let st = if !o["stats_s"].is_null() {
                &o["stats_s"]
            } else if !o["stats_m"].is_null() {
                &o["stats_m"]
            } else {
                &o["stats"]
            };
            let detail = if !o["value"].is_null() {
                format!(
                    "{} (unexplained {}; boundary {})",
                    f(&o["value"]),
                    o["missed"].as_u64().or(o["extra"].as_u64()).unwrap_or(0)
                        - o["missed_near_boundary"]
                            .as_u64()
                            .or(o["extra_near_boundary"].as_u64())
                            .unwrap_or(0),
                    f(if o["missed_near_boundary"].is_null() {
                        &o["extra_near_boundary"]
                    } else {
                        &o["missed_near_boundary"]
                    })
                )
            } else {
                format!(
                    "n {}, median {}, p95 {}, max {}",
                    f(&st["n"]),
                    f(&st["median"]),
                    f(&st["p95"]),
                    f(&st["max"])
                )
            };
            s.push_str(&format!(
                "| {} | {} | {} |\n",
                f(&o["what"]),
                detail,
                f(&o["status"])
            ));
        }
        s.push_str(&format!(
            "\nVerdict: {}.\n\nChecks on the test events:\n\n",
            f(&sc["verdict"])
        ));
        check_table(&mut s, &sc["checks_on_test_events"]);
    }
    for c in doc["compare"].as_array().into_iter().flatten() {
        s.push_str(&format!(
            "\n## Cross-provider comparison: {} (A) and {} (B)\n\n{} matched, {} only in A, {} only in B; {} classification conflicts (A policy: {}; B policy: {}).\n\n",
            f(&c["a"]), f(&c["b"]), f(&c["matched"]), f(&c["only_a"]), f(&c["only_b"]),
            f(&c["classification_conflicts"]), f(&c["policy_a"]["name"]), f(&c["policy_b"]["name"])
        ));
        s.push_str("| Pair | TCA (A) | Δt s | Miss A / B m | Pc A / B | Conflict | Dominant input | Notes |\n|---|---|---|---|---|---|---|---|\n");
        for e in c["matched_events"]
            .as_array()
            .into_iter()
            .flatten()
            .take(50)
        {
            let notes: Vec<String> = e["explanation"]["provenance"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|n| f(&n["note"]))
                .collect();
            s.push_str(&format!(
                "| {}–{} | {} | {} | {} / {} | {} / {} | {} | {} | {} |\n",
                f(&e["obj1"]),
                f(&e["obj2"]),
                f(&e["tca_a"]),
                f(&e["dt_s"]),
                f(&e["miss_a_m"]),
                f(&e["miss_b_m"]),
                f(&e["pc_a"]),
                f(&e["pc_b"]),
                if e["conflict"] == true { "yes" } else { "no" },
                f(&e["explanation"]["swaps"]["dominant_input"]),
                notes.join("; ")
            ));
        }
        s.push_str(&format!("\n{}\n", f(&c["note"])));
    }
    let r = &doc["realism"];
    if !r.is_null() {
        s.push_str(&format!(
            "\n## Covariance realism\n\n{} samples. Overall: scale factor {}, mean d²/3 {}, 95 % containment {}, {}.\n\n",
            f(&r["samples"]), f(&r["overall"]["sigma_scale_factor"]), f(&r["overall"]["mean_d2_over_dof"]),
            f(&r["overall"]["containment_95"]), f(&r["overall"]["verdict"])
        ));
        s.push_str("| Class | Age (h) | n | Scale factor | 95 % containment | p (mean) | KS p | Var R/T/N | Verdict |\n|---|---|---|---|---|---|---|---|---|\n");
        for c in r["by_class"].as_array().into_iter().flatten() {
            for b in c["by_age"].as_array().into_iter().flatten() {
                let st = &b["stats"];
                let var: Vec<String> = st["normalised_variance_rtn"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(f)
                    .collect();
                s.push_str(&format!(
                    "| {} | {}–{} | {} | {} | {} | {} | {} | {} | {} |\n",
                    f(&c["class"]),
                    f(&b["age_h"][0]),
                    f(&b["age_h"][1]),
                    f(&st["n"]),
                    f(&st["sigma_scale_factor"]),
                    f(&st["containment_95"]),
                    f(&st["p_value_mean"]),
                    f(&st["p_value_ks"]),
                    var.join(" / "),
                    f(&st["verdict"])
                ));
            }
        }
        s.push_str(&format!("\n{}\n", f(&r["independence_note"])));
    }
    s.push_str(&format!(
        "\nPer-event rows: `events.csv` ({} rows). Full result: `study.json` (result SHA-256 {}).\n",
        f(&doc["event_rows"]),
        doc["result_sha256"].as_str().unwrap_or("")
    ));
    s.push_str("\n## What this does and does not say\n\n- It says how closely a provider's events agree with the named reference set, whether each event is internally consistent, how the same event differs between two providers and which input moves the recomputed Pc most, and whether predicted covariances matched the errors the predictions had.\n- It does not say that a reference set is ground truth (a published answer key is another tool's output), that a provider is validated or certified, or which of two providers is right.\n- The objectives are published values used as thresholds; the recomputed Pc is the two-dimensional short-encounter integral and may differ from a provider's method.\n- No agency, provider or dataset publisher endorses this result.\n\n## References\n\n");
    for r in doc["references"].as_array().into_iter().flatten() {
        s.push_str(&format!("- {}\n", f(r)));
    }
    s
}

/// The per-event rows as CSV.
pub fn events_csv(rows: &[Value]) -> String {
    const COLS: &[&str] = &[
        "section",
        "outcome",
        "obj1",
        "obj2",
        "class",
        "tca_reference",
        "tca_test",
        "tca_a",
        "tca_b",
        "tca",
        "dt_s",
        "vrel_m_s",
        "velocity_class",
        "tca_within_objective",
        "miss_reference_m",
        "miss_test_m",
        "miss_a_m",
        "miss_b_m",
        "miss_m",
        "d_miss_m",
        "d_rel_position_m",
        "bhattacharyya_obj1",
        "bhattacharyya_obj2",
        "pc_reference",
        "pc_test",
        "pc_a",
        "pc_b",
        "pc",
        "log10_pc_ratio",
        "conflict",
        "dominant_input",
        "reason",
    ];
    let mut s = String::from(
        "# evidence: MODELLED (agreement with the named reference only; not ground truth)\n",
    );
    s.push_str(&COLS.join(","));
    s.push('\n');
    for r in rows {
        let line: Vec<String> = COLS
            .iter()
            .map(|c| match &r[*c] {
                Value::Null => String::new(),
                Value::String(x) => {
                    if x.contains(',') || x.contains('"') {
                        format!("\"{}\"", x.replace('"', "\"\""))
                    } else {
                        x.clone()
                    }
                }
                v => v.to_string(),
            })
            .collect();
        s.push_str(&line.join(","));
        s.push('\n');
    }
    s
}

/// Write the study directory (`study.json`, `events.csv`, `report.md`) and, with a seed, sign
/// it as a whole with Kshana Pro's dossier signature (`signature.json`).
pub fn write_study(
    st: &Study,
    dir: &Path,
    signer: Option<(&[u8; 32], &str, &str)>,
) -> Result<Option<PathBuf>, KshanaError> {
    std::fs::create_dir_all(dir).map_err(|e| bad(format!("{}: {e}", dir.display())))?;
    let w = |name: &str, text: String| {
        std::fs::write(dir.join(name), text).map_err(|e| bad(format!("{name}: {e}")))
    };
    w(
        "study.json",
        serde_json::to_string_pretty(&st.doc).map_err(|e| bad(e.to_string()))? + "\n",
    )?;
    w("events.csv", events_csv(&st.rows))?;
    w("report.md", report_md(&st.doc))?;
    match signer {
        Some((seed, name, utc)) => Ok(Some(kshana_pro::signed_dossier::sign(
            dir, seed, name, utc,
        )?)),
        None => Ok(None),
    }
}

impl Scenario for ConjunctionValidationScenario {
    fn run(&self) -> Result<RunOutput, KshanaError> {
        let mut s = self.clone();
        if s.base.is_none() {
            s.resolve_files(Path::new("."))?;
        }
        let st = run_study(&s)?;
        Ok(RunOutput {
            json: serde_json::to_string_pretty(&st.doc).map_err(|e| bad(e.to_string()))?,
            svg: String::new(),
            summary: summary(&st.doc),
            csv: Some(events_csv(&st.rows)),
        })
    }
}

impl ExternalPack for ConjunctionValidationScenario {
    fn kind_name(&self) -> &'static str {
        Self::KIND
    }
    fn meta(&self) -> ScenarioMeta {
        Self::meta_static()
    }
    fn register_into(reg: &mut kshana::registry::PackRegistry) {
        reg.register(crate::registry::pro_factory::<Self>(Self::KIND));
    }
}
