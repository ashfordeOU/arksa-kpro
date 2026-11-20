// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The recalibration requests to the hosted calibration verdicts, and the checks on their
//! answers.
//!
//! The calibration methods run only in the hosted verdict service and are reached over the
//! network with Kshana Pro's verdict client ([`kshana_pro::remote_verdict::request`],
//! methods [`SELFCAL`] and [`CALIBRATE`]); this module holds the request contracts and the
//! checks on the answers, nothing of the methods (X3).
//!
//! `selfcal` request (JSON): `scenario_toml` (the plugin's simulator scenario, verbatim),
//! `config` (the plugin's `[plugin.calibration]` table), `sites` (its `[[plugin.site]]`
//! tables) and `observations`: one per telemetry sample of the plugin, the observable's `key`
//! table with `value_ns` and `sigma_ns` (the sample's value and standard uncertainty) added.
//!
//! `calibrate` request (JSON): `subject` (the plugin's `calibration.subject`, else
//! `<mission>/<plugin>`), `parameters` (its `[[plugin.parameter]]` tables), `model`
//! (`{"kind": "expression", "observables": {name: expression}}` or `{"kind": "design",
//! "runs": [{"params", "predictions": {name: [[t_s, value], ...]}}]}`), `observations`
//! (`{observable, t_s, value, sigma}`, one per telemetry sample, the observable by its name
//! within the plugin), `tolerances` (each observable's tolerance) and, when set, `options`
//! (`{max_iter}`).
//!
//! An answer is accepted only when, in order: it is a signed verdict for the method; its
//! Ed25519 signature verifies against a trusted service key; and its outcome has the
//! documented fields (`converged`, `posterior` as `{name, value, std_err}`, `lineage.head`;
//! `stop_reason`, `fidelity_first`, `fidelity_last` and `passes` when present). A
//! `calibrate` outcome must also state `at_bound` and `not_constrained` (lists of the
//! posterior's parameter names; `chi2_reduced_first`, `chi2_reduced_last`, `n_obs`,
//! `correlation` and `surface` are read when present), and its posterior must name exactly
//! the parameters requested. Outcomes are MODELLED unless the verdict says otherwise.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use kshana_pro::remote_verdict::{request, verify, SignedVerdict, VerdictTransport};

use super::mission::Mission;
use super::plugin::{CalibrateModel, HostedModel, SimPlugin};
use super::sha256_hex;

/// The hosted self-calibration method (one model family: radio timing reference simulators).
pub const SELFCAL: &str = "selfcal";

/// The hosted general parametric calibration method (any simulator with named parameters).
pub const CALIBRATE: &str = "calibrate";

/// The hosted calibration methods this crate calls.
pub const METHODS: &[&str] = &[SELFCAL, CALIBRATE];

/// A calibrated parameter as the verdict states it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibratedParameter {
    pub name: String,
    pub value: f64,
    /// Standard uncertainty.
    pub std_err: f64,
}

/// The outcome of a recalibration, from a verified verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub converged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passes: Option<u64>,
    /// Observable fidelity before and after (the service's score).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fidelity_before: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fidelity_after: Option<f64>,
    pub parameters: Vec<CalibratedParameter>,
    /// The service's own calibration-lineage head for this run.
    pub service_lineage_head: String,
    /// `calibrate`: reduced chi-square before and after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chi2_reduced_first: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chi2_reduced_last: Option<f64>,
    /// `calibrate`: the number of observations the service used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_obs: Option<u64>,
    /// `calibrate`: the parameter correlation matrix, in posterior order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation: Option<Vec<Vec<f64>>>,
    /// `calibrate`: parameters that ended on a bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_bound: Option<Vec<String>>,
    /// `calibrate`: parameters the telemetry did not constrain (posterior not narrower than
    /// 90 % of the prior).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_constrained: Option<Vec<String>>,
    /// `calibrate` with a design: the response surface the service used, as stated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<Value>,
}

/// The request body for a hosted plugin: its binding and its telemetry.
pub fn request_body(m: &Mission, p: &dyn SimPlugin) -> Result<String, String> {
    let b = p
        .hosted()
        .ok_or_else(|| format!("plugin '{}' has no hosted model", p.id()))?;
    let body = match (&b.model, b.method.as_str()) {
        (
            HostedModel::Selfcal {
                scenario_text,
                config,
                sites,
                keys,
            },
            SELFCAL,
        ) => {
            let mut observations = Vec::new();
            for o in p.observables() {
                let name = format!("{}.{}", p.id(), o.name);
                let key = keys
                    .get(&o.name)
                    .and_then(Value::as_object)
                    .ok_or_else(|| format!("{name}: no key"))?;
                if key.contains_key("value_ns") || key.contains_key("sigma_ns") {
                    return Err(format!(
                        "{name}: `key` may not set value_ns or sigma_ns (they come from the telemetry)"
                    ));
                }
                for row in m.telemetry_of(&name) {
                    let sigma = sample_sigma(&name, row)?;
                    let mut ob: Map<String, Value> = key.clone();
                    ob.insert("value_ns".into(), json!(row.value));
                    ob.insert("sigma_ns".into(), json!(sigma));
                    observations.push(Value::Object(ob));
                }
            }
            no_samples(p, &observations)?;
            json!({
                "scenario_toml": scenario_text,
                "config": config,
                "sites": sites,
                "observations": observations,
            })
        }
        (HostedModel::Calibrate(c), CALIBRATE) => {
            let mut observations = Vec::new();
            let mut tolerances = Map::new();
            for o in p.observables() {
                let name = format!("{}.{}", p.id(), o.name);
                for row in m.telemetry_of(&name) {
                    let sigma = sample_sigma(&name, row)?;
                    observations.push(json!({
                        "observable": o.name,
                        "t_s": row.t_s,
                        "value": row.value,
                        "sigma": sigma,
                    }));
                }
                tolerances.insert(o.name.clone(), json!(o.tolerance));
            }
            no_samples(p, &observations)?;
            let model = match &c.model {
                CalibrateModel::Expression(e) => json!({"kind": "expression", "observables": e}),
                CalibrateModel::Design(runs) => json!({
                    "kind": "design",
                    "runs": runs
                        .iter()
                        .map(|r| {
                            let predictions: Map<String, Value> = r
                                .predictions
                                .iter()
                                .map(|(k, s)| {
                                    (k.clone(), json!(s.iter().map(|(t, v)| [*t, *v]).collect::<Vec<_>>()))
                                })
                                .collect();
                            json!({"params": r.params, "predictions": predictions})
                        })
                        .collect::<Vec<_>>(),
                }),
            };
            let mut body = Map::new();
            body.insert(
                "subject".into(),
                json!(c.subject.clone().unwrap_or_else(|| format!(
                    "{}/{}",
                    m.file.mission,
                    p.id()
                ))),
            );
            body.insert("parameters".into(), json!(c.parameters));
            body.insert("model".into(), model);
            body.insert("observations".into(), Value::Array(observations));
            body.insert("tolerances".into(), Value::Object(tolerances));
            if let Some(n) = c.max_iter {
                body.insert("options".into(), json!({"max_iter": n}));
            }
            Value::Object(body)
        }
        _ => {
            return Err(format!(
                "plugin '{}': method '{}' is not a calibration method",
                p.id(),
                b.method
            ))
        }
    };
    serde_json::to_string(&body).map_err(|e| e.to_string())
}

fn sample_sigma(name: &str, row: &super::mission::TelemetryRow) -> Result<f64, String> {
    row.sigma.ok_or_else(|| {
        format!(
            "{name} at t = {} s: a hosted observable needs the sample's sigma",
            row.t_s
        )
    })
}

fn no_samples(p: &dyn SimPlugin, observations: &[Value]) -> Result<(), String> {
    if observations.is_empty() {
        return Err(format!(
            "plugin '{}': the telemetry holds no sample of its observables",
            p.id()
        ));
    }
    Ok(())
}

fn names(v: &Value, k: &str) -> Result<Vec<String>, String> {
    v.get(k)
        .ok_or_else(|| format!("the verdict states no `{k}`"))
        .and_then(|x| {
            serde_json::from_value::<Vec<String>>(x.clone())
                .map_err(|_| format!("the verdict's `{k}` is not a list of parameter names"))
        })
}

fn opt_f64(v: &Value, k: &str) -> Result<Option<f64>, String> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(x) => x
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("outcome field `{k}` is not a number")),
    }
}

/// Read the outcome fields of a `selfcal` or `calibrate` verdict (by the verdict's method).
pub fn outcome(v: &SignedVerdict) -> Result<Outcome, String> {
    let o = &v.verdict;
    let converged = o
        .get("converged")
        .and_then(Value::as_bool)
        .ok_or("the verdict states no `converged`")?;
    let parameters: Vec<CalibratedParameter> = serde_json::from_value(
        o.get("posterior")
            .cloned()
            .ok_or("the verdict states no `posterior`")?,
    )
    .map_err(|e| {
        format!("the verdict's `posterior` is not a list of {{name, value, std_err}}: {e}")
    })?;
    if parameters
        .iter()
        .any(|p| !(p.value.is_finite() && p.std_err.is_finite() && p.std_err >= 0.0))
    {
        return Err("the verdict's `posterior` holds a value that is not finite".into());
    }
    let head = o
        .get("lineage")
        .and_then(|l| l.get("head"))
        .and_then(Value::as_str)
        .filter(|h| !h.is_empty())
        .ok_or("the verdict states no `lineage.head`")?;
    let mut out = Outcome {
        converged,
        stop_reason: o
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        passes: o.get("passes").and_then(Value::as_u64),
        fidelity_before: opt_f64(o, "fidelity_first")?,
        fidelity_after: opt_f64(o, "fidelity_last")?,
        parameters,
        service_lineage_head: head.to_string(),
        chi2_reduced_first: None,
        chi2_reduced_last: None,
        n_obs: None,
        correlation: None,
        at_bound: None,
        not_constrained: None,
        surface: None,
    };
    if v.method == CALIBRATE {
        let known = |n: &String| out.parameters.iter().any(|p| p.name == *n);
        let at_bound = names(o, "at_bound")?;
        let not_constrained = names(o, "not_constrained")?;
        if let Some(n) = at_bound.iter().chain(&not_constrained).find(|n| !known(n)) {
            return Err(format!(
                "the verdict flags '{n}', which is not in its `posterior`"
            ));
        }
        out.at_bound = Some(at_bound);
        out.not_constrained = Some(not_constrained);
        out.chi2_reduced_first = opt_f64(o, "chi2_reduced_first")?;
        out.chi2_reduced_last = opt_f64(o, "chi2_reduced_last")?;
        out.n_obs = match o.get("n_obs") {
            None | Some(Value::Null) => None,
            Some(x) => Some(x.as_u64().ok_or("outcome field `n_obs` is not a count")?),
        };
        out.correlation = match o.get("correlation") {
            None | Some(Value::Null) => None,
            Some(x) => {
                let c: Vec<Vec<f64>> = serde_json::from_value(x.clone())
                    .map_err(|_| "the verdict's `correlation` is not a matrix of numbers")?;
                let n = out.parameters.len();
                if c.len() != n || c.iter().any(|r| r.len() != n) {
                    return Err(format!(
                        "the verdict's `correlation` is not {n} x {n} (one row per posterior parameter)"
                    ));
                }
                Some(c)
            }
        };
        out.surface = o.get("surface").filter(|s| !s.is_null()).cloned();
    }
    Ok(out)
}

/// A recalibration answer: the request's hash, the verified verdict and its outcome.
#[derive(Debug, Clone)]
pub struct Recalibration {
    pub request_body: String,
    pub request_sha256: String,
    pub verdict: SignedVerdict,
    pub outcome: Outcome,
}

/// Send a hosted plugin's request to the service and check the answer (module docs).
/// `trusted_keys_hex`: the service public key(s), 64 hex each, comma separated.
pub fn recalibrate(
    t: &dyn VerdictTransport,
    bearer: &str,
    m: &Mission,
    p: &dyn SimPlugin,
    trusted_keys_hex: &str,
) -> Result<Recalibration, String> {
    let body = request_body(m, p)?;
    let method = p.hosted().map(|b| b.method.as_str()).unwrap_or(SELFCAL);
    let v = request(t, method, bearer, &body).map_err(|e| format!("plugin '{}': {e}", p.id()))?;
    let problems = verify(&v, trusted_keys_hex);
    if !problems.is_empty() {
        return Err(format!(
            "plugin '{}': verdict refused: {}",
            p.id(),
            problems.join("; ")
        ));
    }
    let outcome = outcome(&v).map_err(|e| format!("plugin '{}': {e}", p.id()))?;
    if let Some(HostedModel::Calibrate(c)) = p.hosted().map(|b| &b.model) {
        let mut want: Vec<&str> = c.parameters.iter().map(|q| q.name.as_str()).collect();
        let mut got: Vec<&str> = outcome.parameters.iter().map(|q| q.name.as_str()).collect();
        want.sort_unstable();
        got.sort_unstable();
        if want != got {
            return Err(format!(
                "plugin '{}': verdict refused: its posterior names {} but the request asked for {}",
                p.id(),
                got.join(", "),
                want.join(", ")
            ));
        }
    }
    Ok(Recalibration {
        request_sha256: sha256_hex(body.as_bytes()),
        request_body: body,
        verdict: v,
        outcome,
    })
}
