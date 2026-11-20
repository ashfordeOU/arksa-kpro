// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # `pro-rideshare-id` (Arksa Ops pillar; research 06 G3, 06b finding 4)
//!
//! "Which object is mine?" after a rideshare. The scenario names the operator's observation CSV
//! and the candidate element sets of the launch (TLE with Alpha-5, or OMM in KVN, XML or JSON);
//! the study ranks the candidates and writes one verdict document with the evidence used. The
//! engine is [`crate::rideshare`]; this pack reads the scenario, loads and hashes the inputs,
//! and writes the verdict (and, from the command, a signed directory).
//!
//! ```toml
//! kind = "pro-rideshare-id"
//! title = "Which Transporter object is ours?"
//! observations_file = "passes.csv"
//! candidate_files = ["launch.tle", "launch-omm.xml"]
//!
//! [identify]                 # all optional; defaults shown
//! doppler_bias_per_pass = true
//! range_bias_per_pass = false
//! max_reduced_chi2 = 4.0
//! decision_delta_chi2 = 25.0
//! min_probability = 0.99
//! min_observations = 10
//! horizon_deg = 0.0
//! ```
//!
//! Evidence class: MODELLED (see [`crate::rideshare::LABEL`]).

use std::path::{Path, PathBuf};

use kshana::api::{ExternalPack, KshanaError, RunOutput, Scenario, ScenarioMeta};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::rideshare::identify::{self, Identification};
use crate::rideshare::{self as rs, catalogue, observations, round_sig, sanitise, sha256_hex};

pub use crate::rideshare::{KIND, LABEL, SCHEMA};

fn bad(m: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(format!("{KIND}: {}", m.into()))
}

/// A `pro-rideshare-id` scenario.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RideshareIdScenario {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// The observation CSV ([`crate::rideshare::observations`]).
    #[serde(default)]
    pub observations_file: Option<String>,
    #[serde(default)]
    pub observations_text: Option<String>,
    /// Candidate element-set files (TLE or OMM, detected from the content).
    #[serde(default)]
    pub candidate_files: Vec<String>,
    #[serde(default)]
    pub candidate_text: Vec<String>,
    #[serde(default)]
    pub identify: identify::Config,
    /// Directory relative paths resolve against (set by [`Self::resolve_files`]).
    #[serde(skip)]
    pub base: Option<PathBuf>,
}

impl RideshareIdScenario {
    pub const KIND: &'static str = KIND;

    pub fn meta_static() -> ScenarioMeta {
        ScenarioMeta {
            name: Self::KIND,
            description: "Rideshare object identification: ranks the catalogued objects of one launch (TLE with Alpha-5 or OMM, catalogue numbers above 99,999 end to end) by how well each explains the operator's ground-station Doppler, ranging or angle observations; residual statistics per candidate, the margin between the best and the second, an explicit AMBIGUOUS verdict when the data cannot separate them. Signed verdict. MODELLED.",
            required_fields: &[],
            optional_fields: &[
                "title",
                "observations_file",
                "observations_text",
                "candidate_files",
                "candidate_text",
                "identify",
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

fn read(path: &Path) -> Result<String, KshanaError> {
    std::fs::read_to_string(path).map_err(|e| bad(format!("{}: {e}", path.display())))
}

/// The result: the verdict document and the two tables.
#[derive(Debug, Clone)]
pub struct Study {
    pub doc: Value,
    pub identification: Identification,
    pub candidates_csv: String,
    pub residuals_csv: String,
}

/// Run the study.
pub fn run_study(s: &RideshareIdScenario) -> Result<Study, KshanaError> {
    let mut inputs = Vec::new();
    let obs_text = match (&s.observations_file, &s.observations_text) {
        (Some(f), None) => {
            let t = read(&s.path(f))?;
            inputs.push(json!({"role": "observations", "name": f, "bytes": t.len(), "sha256": sha256_hex(t.as_bytes())}));
            t
        }
        (None, Some(t)) => {
            inputs.push(json!({"role": "observations", "name": "inline", "bytes": t.len(), "sha256": sha256_hex(t.as_bytes())}));
            t.clone()
        }
        _ => {
            return Err(bad(
                "give exactly one of observations_file and observations_text",
            ))
        }
    };
    let set = observations::parse_csv(&obs_text).map_err(bad)?;
    let mut all = Vec::new();
    for f in &s.candidate_files {
        let t = read(&s.path(f))?;
        let c = catalogue::from_text(&t, f).map_err(bad)?;
        inputs.push(json!({"role": "candidates", "name": f, "bytes": t.len(), "sha256": sha256_hex(t.as_bytes()), "element_sets": c.len()}));
        all.extend(c);
    }
    for (k, t) in s.candidate_text.iter().enumerate() {
        let name = format!("inline-{}", k + 1);
        let c = catalogue::from_text(t, &name).map_err(bad)?;
        inputs.push(json!({"role": "candidates", "name": name, "bytes": t.len(), "sha256": sha256_hex(t.as_bytes()), "element_sets": c.len()}));
        all.extend(c);
    }
    if all.is_empty() {
        return Err(bad(
            "no candidate element sets (candidate_files or candidate_text)",
        ));
    }
    let (cands, warnings) = catalogue::merge(all);
    let id = identify::identify(&cands, &set, &s.identify).map_err(bad)?;
    let identified = id.identified.and_then(|n| id.fit_of(n)).map(|f| {
        json!({
            "catalogue_number": f.candidate.catalogue_number,
            "tle_field": f.candidate.tle_field,
            "alpha5": f.candidate.alpha5,
            "name": f.candidate.name,
            "object_id": f.candidate.object_id,
            "probability": f.probability,
        })
    });
    let six_digit = cands
        .iter()
        .filter(|c| c.catalogue_number > crate::rideshare::norad::FIVE_DIGIT_MAX)
        .count();
    let mut doc = json!({
        "schema": SCHEMA,
        "kind": KIND,
        "title": s.title.clone().unwrap_or_else(|| "Rideshare object identification".into()),
        "label": LABEL,
        "evidence_class": "MODELLED",
        "verdict": id.verdict.as_str(),
        "identified": identified,
        "not_excluded": id.not_excluded,
        "reasons": id.reasons,
        "separation": id.separation,
        "variance_factor": id.variance_factor,
        "config": id.config,
        "observations": id.observations,
        "candidates": id.fits,
        "catalogue": {
            "candidates": cands.len(),
            "above_99999": six_digit,
            "omm_only_above_339999": cands.iter().filter(|c| c.tle_field.is_none()).count(),
            "catalogue_number_helper": "local Alpha-5 helper (arksa-kpro rideshare::norad), same rules as the open engine's norad module; to be replaced by kshana::norad once a kshana release contains it",
        },
        "method": "SGP4 (open engine 0.41.0) per candidate, TEME to ITRS, station geometry on WGS-84; residual = observed - predicted; per-pass constant biases by weighted least squares where configured; chi-square ranking; scaled delta chi-square and posterior probability under a uniform prior",
        "not_modelled": ["light time", "tropospheric and ionospheric refraction", "transmitter frequency drift within a pass", "element-set covariance"],
        "time_scale": "UTC",
        "references": rs::REFERENCES,
        "inputs": inputs,
        "warnings": warnings,
        "endorsement": "No catalogue authority, launch provider or agency endorses this result.",
    });
    sanitise(&mut doc);
    let digest = sha256_hex(&serde_json::to_vec(&doc).map_err(|e| bad(e.to_string()))?);
    doc["result_sha256"] = json!(digest);
    let candidates_csv = candidates_csv(&id);
    let residuals_csv = residuals_csv(&id, &set);
    Ok(Study {
        doc,
        identification: id,
        candidates_csv,
        residuals_csv,
    })
}

/// A number for the CSV tables: rounded like the verdict, in exponent form when very small or
/// very large.
fn num(x: f64) -> String {
    if !x.is_finite() {
        return String::new();
    }
    let r = round_sig(x);
    if r != 0.0 && (r.abs() < 1e-4 || r.abs() >= 1e12) {
        format!("{r:e}")
    } else {
        format!("{r}")
    }
}

/// One row per candidate, ranked.
pub fn candidates_csv(id: &Identification) -> String {
    let mut s = String::from("# evidence: MODELLED (fit of each candidate's SGP4 orbit to the operator's observations)\n");
    s.push_str("rank,catalogue_number,tle_field,name,object_id,source,epoch_utc,status,n,dof,chi2,reduced_chi2,delta_chi2,probability,consistent,below_horizon,element_age_days_first\n");
    for f in &id.fits {
        let c = &f.candidate;
        s.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            f.rank,
            c.catalogue_number,
            c.tle_field.clone().unwrap_or_default(),
            c.name.replace(',', " "),
            c.object_id,
            c.source,
            c.epoch_utc,
            f.status,
            f.n,
            f.dof,
            num(f.chi2),
            num(f.reduced_chi2),
            num(f.delta_chi2),
            num(f.probability),
            f.consistent,
            f.below_horizon,
            num(f.element_age_days[0]),
        ));
    }
    s
}

/// One row per measurement, with the two leading candidates' predictions and residuals.
pub fn residuals_csv(id: &Identification, set: &observations::ObservationSet) -> String {
    let fitted: Vec<&identify::Fit> = id
        .fits
        .iter()
        .filter(|f| f.status == "fitted")
        .take(2)
        .collect();
    let mut s = String::from(
        "# evidence: MODELLED (residuals after per-pass biases; best and second-best candidate)\n",
    );
    s.push_str("row,time_utc,station,pass,type,value,sigma");
    for (k, f) in fitted.iter().enumerate() {
        let n = f.candidate.catalogue_number;
        let tag = if k == 0 { "best" } else { "second" };
        s.push_str(&format!(
            ",{tag}_{n}_predicted,{tag}_{n}_residual,{tag}_{n}_normalised,{tag}_{n}_elevation_deg"
        ));
    }
    s.push('\n');
    for (i, o) in set.obs.iter().enumerate() {
        s.push_str(&format!(
            "{},{},{},{},{},{},{}",
            o.row,
            o.time_utc,
            set.stations[o.station].name,
            set.passes[o.pass],
            o.kind.as_str(),
            num(o.value),
            num(o.sigma)
        ));
        for f in &fitted {
            s.push_str(&format!(
                ",{},{},{},{}",
                num(f.predicted[i]),
                num(f.residual[i]),
                num(f.residual[i] / o.sigma),
                num(f.elevation_deg[i])
            ));
        }
        s.push('\n');
    }
    s
}

fn f(v: &Value) -> String {
    match v {
        Value::Null => "–".into(),
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                return n.to_string();
            }
            let x = n.as_f64().unwrap_or(0.0);
            if x != 0.0 && (x.abs() < 1e-3 || x.abs() >= 1e6) {
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
        "{} ({})\nverdict: {}",
        f(&doc["title"]),
        f(&doc["evidence_class"]),
        f(&doc["verdict"])
    );
    if let Some(o) = doc["identified"].as_object() {
        s.push_str(&format!(
            "; object {}{} (p = {})",
            f(&o["catalogue_number"]),
            match o["tle_field"].as_str() {
                Some(t) if o["alpha5"].as_bool() == Some(true) => format!(", TLE {t}"),
                None => ", OMM only".into(),
                _ => String::new(),
            },
            f(&o["probability"])
        ));
    }
    if let Some(ne) = doc["not_excluded"].as_array() {
        if doc["verdict"] == "AMBIGUOUS" {
            s.push_str(&format!(
                "; not excluded: {}",
                ne.iter().map(f).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    s.push('\n');
    for r in doc["reasons"].as_array().into_iter().flatten() {
        s.push_str(&format!("  - {}\n", f(r)));
    }
    s
}

/// The Markdown report.
pub fn report_md(doc: &Value) -> String {
    let mut s = format!(
        "# {}\n\nEvidence class: **{}**. {}\n\n## Verdict: {}\n\n",
        f(&doc["title"]),
        f(&doc["evidence_class"]),
        f(&doc["label"]),
        f(&doc["verdict"])
    );
    for r in doc["reasons"].as_array().into_iter().flatten() {
        s.push_str(&format!("- {}\n", f(r)));
    }
    let o = &doc["observations"];
    s.push_str(&format!(
        "\n## Observations\n\n{} measurements, {} passes, {} to {} ({} h).\n\n",
        f(&o["measurements"]),
        o["passes"].as_array().map(|a| a.len()).unwrap_or(0),
        f(&o["first_utc"]),
        f(&o["last_utc"]),
        f(&o["span_h"])
    ));
    if let Some(sep) = doc["separation"].as_object() {
        s.push_str("## Best against second best\n\n| quantity | value |\n|---|---|\n");
        for k in [
            "best",
            "second",
            "delta_chi2",
            "log10_likelihood_ratio",
            "predicted_difference_sigma",
            "distance_km_min",
            "distance_km_median",
            "distance_km_max",
        ] {
            s.push_str(&format!("| {k} | {} |\n", f(&sep[k])));
        }
        s.push('\n');
    }
    s.push_str("## Candidates (ranked)\n\n| rank | catalogue number | TLE field | source | reduced chi2 | delta chi2 | probability | below horizon | status |\n|---|---|---|---|---|---|---|---|---|\n");
    for c in doc["candidates"].as_array().into_iter().flatten() {
        let ci = &c["candidate"];
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            f(&c["rank"]),
            f(&ci["catalogue_number"]),
            match ci["tle_field"].as_str() {
                Some(t) => t.to_string(),
                None => "OMM only".into(),
            },
            f(&ci["source"]),
            f(&c["reduced_chi2"]),
            f(&c["delta_chi2"]),
            f(&c["probability"]),
            f(&c["below_horizon"]),
            f(&c["status"])
        ));
    }
    s.push_str(&format!(
        "\nMethod: {}. Not modelled: {}.\n\n{}\n\nResult SHA-256 (numbers rounded to 9 significant digits): `{}`\n",
        f(&doc["method"]),
        doc["not_modelled"]
            .as_array()
            .map(|a| a.iter().map(f).collect::<Vec<_>>().join(", "))
            .unwrap_or_default(),
        f(&doc["endorsement"]),
        f(&doc["result_sha256"])
    ));
    s
}

/// Write the verdict directory (`verdict.json`, `candidates.csv`, `residuals.csv`,
/// `report.md`) and, with a seed, sign it as a whole with Kshana Pro's dossier signature
/// (`signature.json`).
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
        "verdict.json",
        serde_json::to_string_pretty(&st.doc).map_err(|e| bad(e.to_string()))? + "\n",
    )?;
    w("candidates.csv", st.candidates_csv.clone())?;
    w("residuals.csv", st.residuals_csv.clone())?;
    w("report.md", report_md(&st.doc))?;
    match signer {
        Some((seed, name, utc)) => Ok(Some(kshana_pro::signed_dossier::sign(
            dir, seed, name, utc,
        )?)),
        None => Ok(None),
    }
}

impl Scenario for RideshareIdScenario {
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
            csv: Some(st.candidates_csv),
        })
    }
}

impl ExternalPack for RideshareIdScenario {
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
