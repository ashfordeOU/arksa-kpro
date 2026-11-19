// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The network client for the hosted assistance generator.
//!
//! Generating a long-validity GNSS ephemeris and clock prediction with an integrity overbound
//! is the high-value method (research 13 G-B places it in the hosted service). Under the X3
//! rule it lives only in the hosted verdict service and is reached over the network; this
//! module holds the request contract and the checks on the answer, nothing of the method.
//!
//! The service answers with Kshana Pro's signed verdict envelope
//! ([`kshana_pro::remote_verdict::SignedVerdict`]); the verdict body carries the package under
//! `"package"`. The client checks, in order: the envelope's Ed25519 signature against the
//! trusted service keys, that the verdict is for [`METHOD`], that the package is a valid
//! `ks-lunar-gnss-assist/1` package, and that it answers the request (the satellites asked
//! for, a validity window covering the one asked for, an integrity risk no larger than asked,
//! and the lunar time definition and registry version asked for).
//!
//! **Status.** The service method is not built yet (follow-up in the hosted service). The
//! client is tested against a local stand-in that signs answers with a test key.

use kshana_pro::remote_verdict::{verify, SignedVerdict, VerdictTransport};
use serde::{Deserialize, Serialize};

use super::assist::{epoch_diff_s, parse_epoch, AssistPackage};

/// The service method name (path `/verdict/lunar-assist`).
pub const METHOD: &str = "lunar-assist";

/// A request for a package.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssistRequest {
    /// Satellites (`G01`, `E11`, ...).
    pub satellites: Vec<String>,
    /// Start of validity (ISO, GPST).
    pub validity_start: String,
    pub validity_hours: f64,
    /// Node spacing wanted (s).
    pub step_s: u32,
    /// Largest acceptable integrity risk per independent sample.
    pub integrity_risk: f64,
    /// The lunar time definition the package should reference (registry id, version and
    /// definition id), if any.
    #[serde(default)]
    pub lunar_time: Option<LunarTimeWanted>,
}

/// The registry definition a request names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LunarTimeWanted {
    pub registry_id: String,
    pub registry_version: u32,
    pub definition_id: String,
}

impl AssistRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.satellites.is_empty() {
            return Err("a request names at least one satellite".into());
        }
        parse_epoch(&self.validity_start)?;
        if !(self.validity_hours.is_finite() && self.validity_hours > 0.0) {
            return Err("validity_hours must be positive".into());
        }
        if self.step_s == 0 {
            return Err("step_s must be positive".into());
        }
        if !(self.integrity_risk > 0.0 && self.integrity_risk < 0.5) {
            return Err("integrity_risk must lie in (0, 0.5)".into());
        }
        Ok(())
    }
}

/// Check that `pkg` answers `req`. Returns the problems (empty: it does).
pub fn answers(pkg: &AssistPackage, req: &AssistRequest) -> Vec<String> {
    let mut p = Vec::new();
    if let Err(e) = pkg.validate() {
        p.push(e);
        return p;
    }
    for s in &req.satellites {
        if !pkg.satellites.iter().any(|x| &x.sat == s) {
            p.push(format!(
                "satellite {s} was asked for but is not in the package"
            ));
        }
    }
    match (parse_epoch(&pkg.start), parse_epoch(&req.validity_start)) {
        (Ok(ps), Ok(rs)) => {
            let want_start = epoch_diff_s(&ps, &rs);
            let want_end = want_start + req.validity_hours * 3600.0;
            if (pkg.validity_start_s as f64) > want_start + 1e-6
                || (pkg.validity_end_s as f64) < want_end - 1e-6
            {
                p.push(format!(
                    "validity [{}, {}] s does not cover the requested [{want_start}, {want_end}] s",
                    pkg.validity_start_s, pkg.validity_end_s
                ));
            }
        }
        (Err(e), _) | (_, Err(e)) => p.push(e),
    }
    if pkg.step_s > req.step_s {
        p.push(format!(
            "node spacing {} s is coarser than the requested {} s",
            pkg.step_s, req.step_s
        ));
    }
    for s in &pkg.satellites {
        if s.bound.integrity_risk > req.integrity_risk {
            p.push(format!(
                "{}: integrity risk {:e} is larger than the requested {:e}",
                s.sat, s.bound.integrity_risk, req.integrity_risk
            ));
        }
    }
    if let Some(w) = &req.lunar_time {
        match &pkg.lunar_time {
            None => p.push("a lunar time reference was asked for but is missing".into()),
            Some(l) => {
                if l.registry.registry_id != w.registry_id
                    || l.registry.registry_version != w.registry_version
                    || l.registry.definition_id != w.definition_id
                {
                    p.push(format!(
                        "lunar time reference is {} v{} '{}', not the requested {} v{} '{}'",
                        l.registry.registry_id,
                        l.registry.registry_version,
                        l.registry.definition_id,
                        w.registry_id,
                        w.registry_version,
                        w.definition_id
                    ));
                }
            }
        }
    }
    p
}

/// Ask the hosted service for a package and check the answer (see the module docs).
/// `trusted_keys_hex`: the service's public key(s), 64 hex each, comma separated.
pub fn request_package(
    t: &dyn VerdictTransport,
    bearer: &str,
    req: &AssistRequest,
    trusted_keys_hex: &str,
) -> Result<(SignedVerdict, AssistPackage), String> {
    req.validate()?;
    if bearer.trim().is_empty() {
        return Err("a licence token is required (--license)".into());
    }
    let body = serde_json::to_string(req).map_err(|e| e.to_string())?;
    let resp = t.post(&format!("/verdict/{METHOD}"), bearer.trim(), &body)?;
    let v: SignedVerdict =
        serde_json::from_str(&resp).map_err(|e| format!("answer is not a signed verdict: {e}"))?;
    let problems = verify(&v, trusted_keys_hex);
    if !problems.is_empty() {
        return Err(format!("verdict refused: {}", problems.join("; ")));
    }
    if v.method != METHOD {
        return Err(format!(
            "answer is a verdict for '{}', not '{METHOD}'",
            v.method
        ));
    }
    let pkg: AssistPackage = serde_json::from_value(
        v.verdict
            .get("package")
            .cloned()
            .ok_or("the verdict carries no package")?,
    )
    .map_err(|e| {
        format!(
            "the verdict's package is not {}: {e}",
            super::assist::FORMAT
        )
    })?;
    let problems = answers(&pkg, req);
    if !problems.is_empty() {
        return Err(format!(
            "the package does not answer the request: {}",
            problems.join("; ")
        ));
    }
    Ok((v, pkg))
}
