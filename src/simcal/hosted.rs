// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The recalibration request to the hosted self-calibration verdict, and the checks on its
//! answer.
//!
//! The calibration method runs only in the hosted verdict service and is reached over the
//! network with Kshana Pro's verdict client ([`kshana_pro::remote_verdict::request`], method
//! [`SELFCAL`]); this module holds the request contract and the checks on the answer,
//! nothing of the method (X3).
//!
//! Request (JSON): `scenario_toml` (the plugin's simulator scenario, verbatim), `config`
//! (the plugin's `[plugin.calibration]` table), `sites` (its `[[plugin.site]]` tables) and
//! `observations`: one per telemetry sample of the plugin, the observable's `key` table with
//! `value_ns` and `sigma_ns` (the sample's value and standard uncertainty) added.
//!
//! The answer is accepted only when, in order: it is a signed verdict for the method; its
//! Ed25519 signature verifies against a trusted service key; and its outcome has the
//! documented fields (`converged`, `posterior` as `{name, value, std_err}`, `lineage.head`;
//! `stop_reason`, `fidelity_first`, `fidelity_last` and `passes` when present). Outcomes are
//! MODELLED unless the verdict says otherwise.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use kshana_pro::remote_verdict::{request, verify, SignedVerdict, VerdictTransport};

use super::mission::Mission;
use super::plugin::SimPlugin;
use super::sha256_hex;

/// The hosted self-calibration method.
pub const SELFCAL: &str = "selfcal";

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
}

/// The request body for a hosted plugin: its binding and its telemetry.
pub fn request_body(m: &Mission, p: &dyn SimPlugin) -> Result<String, String> {
    let b = p
        .hosted()
        .ok_or_else(|| format!("plugin '{}' has no hosted model", p.id()))?;
    if b.method != SELFCAL {
        return Err(format!(
            "plugin '{}': method '{}' is not the calibration method",
            p.id(),
            b.method
        ));
    }
    let mut observations = Vec::new();
    for o in p.observables() {
        let name = format!("{}.{}", p.id(), o.name);
        let key = b
            .keys
            .get(&o.name)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("{name}: no key"))?;
        if key.contains_key("value_ns") || key.contains_key("sigma_ns") {
            return Err(format!(
                "{name}: `key` may not set value_ns or sigma_ns (they come from the telemetry)"
            ));
        }
        for row in m.telemetry_of(&name) {
            let sigma = row.sigma.ok_or_else(|| {
                format!(
                    "{name} at t = {} s: a hosted observable needs the sample's sigma",
                    row.t_s
                )
            })?;
            let mut ob: Map<String, Value> = key.clone();
            ob.insert("value_ns".into(), json!(row.value));
            ob.insert("sigma_ns".into(), json!(sigma));
            observations.push(Value::Object(ob));
        }
    }
    if observations.is_empty() {
        return Err(format!(
            "plugin '{}': the telemetry holds no sample of its observables",
            p.id()
        ));
    }
    let body = json!({
        "scenario_toml": b.scenario_text,
        "config": b.config,
        "sites": b.sites,
        "observations": observations,
    });
    serde_json::to_string(&body).map_err(|e| e.to_string())
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

/// Read the outcome fields of a selfcal verdict.
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
    Ok(Outcome {
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
    })
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
    let v = request(t, SELFCAL, bearer, &body).map_err(|e| format!("plugin '{}': {e}", p.id()))?;
    let problems = verify(&v, trusted_keys_hex);
    if !problems.is_empty() {
        return Err(format!(
            "plugin '{}': verdict refused: {}",
            p.id(),
            problems.join("; ")
        ));
    }
    let outcome = outcome(&v).map_err(|e| format!("plugin '{}': {e}", p.id()))?;
    Ok(Recalibration {
        request_sha256: sha256_hex(body.as_bytes()),
        request_body: body,
        verdict: v,
        outcome,
    })
}
