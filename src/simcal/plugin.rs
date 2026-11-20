// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Simulator plugins: one interface for every model family of a mission performance
//! simulator.
//!
//! A plugin declares its observables and predicts each one at a time, or says that its
//! model runs in a hosted service ([`SimPlugin::hosted`]); then the residuals are formed by
//! that service and the plugin hands over what the service needs ([`HostedBinding`]).
//!
//! | kind | simulator | predictions |
//! |---|---|---|
//! | `table` | any external simulator (its output CSV) | linear interpolation inside the simulated span |
//! | `engine` | a scenario of any open kind, run on the embedded engine | a series (array of objects) or a number at a dot path |
//! | `hosted` | a model that runs in the hosted service (`selfcal`: a scenario it runs; `calibrate`: named parameters with an expression per observable or a design of the plugin's own simulator runs) | formed by the service |
//!
//! Adding a model family: implement [`SimPlugin`] and add one entry to [`KINDS`].

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use super::mission::{read_source, ObservableSpec, ParameterSpec, PluginSpec, SourceRef};
use crate::license::Entitlement;

/// What a plugin is built with.
pub struct Context<'a> {
    /// The mission file's directory (plugin files are relative to it).
    pub dir: &'a Path,
    /// The entitlement an `engine` scenario of a Pro kind is checked against.
    pub entitlement: &'a Entitlement,
    pub now_unix: i64,
}

/// What a hosted model family needs from the plugin, besides its telemetry.
#[derive(Debug, Clone)]
pub struct HostedBinding {
    /// The hosted method (one of Kshana Pro's verdict-client methods:
    /// [`super::hosted::SELFCAL`] or [`super::hosted::CALIBRATE`]).
    pub method: String,
    /// What the method needs.
    pub model: HostedModel,
}

/// The method-specific part of a hosted binding.
#[derive(Debug, Clone)]
pub enum HostedModel {
    /// `selfcal`: a simulator scenario the service runs.
    Selfcal {
        /// The simulator scenario, verbatim.
        scenario_text: String,
        /// The method's configuration.
        config: Value,
        /// The method's reference sites.
        sites: Vec<Value>,
        /// Per observable name, the fields that identify it to the method.
        keys: BTreeMap<String, Value>,
    },
    /// `calibrate`: named parameters and a model of the observables.
    Calibrate(CalibrateBinding),
}

/// A `calibrate` binding: the parameters, the model and the request options.
#[derive(Debug, Clone)]
pub struct CalibrateBinding {
    /// Echoed by the service (default `<mission>/<plugin>`).
    pub subject: Option<String>,
    pub max_iter: Option<u64>,
    pub parameters: Vec<ParameterSpec>,
    pub model: CalibrateModel,
}

/// The model of a `calibrate` binding.
#[derive(Debug, Clone, PartialEq)]
pub enum CalibrateModel {
    /// A closed-form expression per observable name.
    Expression(BTreeMap<String, String>),
    /// Runs of the plugin's own simulator at chosen parameter sets.
    Design(Vec<DesignRun>),
}

/// One design run: the parameter values and, per observable name, the predictions
/// `(t_s, value)` ascending in time.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignRun {
    pub params: BTreeMap<String, f64>,
    pub predictions: BTreeMap<String, Vec<(f64, f64)>>,
}

/// At most this many parameters in one `calibrate` request (the method's limit).
pub const MAX_CALIBRATE_PARAMETERS: usize = 32;

/// One model family of a mission performance simulator.
pub trait SimPlugin {
    fn id(&self) -> &str;
    /// The plugin kind (`table`, `engine`, `hosted`, ...).
    fn kind(&self) -> &'static str;
    fn observables(&self) -> &[ObservableSpec];
    /// The predicted value of `observable` at `t_s`, or `None` where the simulator makes no
    /// prediction (outside its span, or a hosted model).
    fn predict(&self, observable: &str, t_s: f64) -> Option<f64>;
    /// The files the plugin was built from, with their SHA-256.
    fn sources(&self) -> Vec<SourceRef>;
    /// The hosted calibration binding, when the model runs in the hosted service.
    fn hosted(&self) -> Option<&HostedBinding> {
        None
    }
}

/// Builds a plugin from its mission-file entry.
pub type Factory = fn(&PluginSpec, &Context) -> Result<Box<dyn SimPlugin>, String>;

/// Every plugin kind this build knows.
pub const KINDS: &[(&str, Factory)] = &[
    ("table", TablePlugin::build),
    ("engine", EnginePlugin::build),
    ("hosted", HostedPlugin::build),
];

/// Build the plugin of a mission-file entry.
pub fn build(spec: &PluginSpec, ctx: &Context) -> Result<Box<dyn SimPlugin>, String> {
    let Some((_, f)) = KINDS.iter().find(|(k, _)| *k == spec.kind) else {
        let known: Vec<_> = KINDS.iter().map(|(k, _)| *k).collect();
        return Err(format!(
            "plugin '{}': unknown kind '{}' (known: {})",
            spec.id,
            spec.kind,
            known.join(", ")
        ));
    };
    f(spec, ctx)
}

fn refuse_fields(spec: &PluginSpec, fields: &[(&str, bool)]) -> Result<(), String> {
    for (name, present) in fields {
        if *present {
            return Err(format!(
                "plugin '{}': `{name}` does not apply to kind '{}'",
                spec.id, spec.kind
            ));
        }
    }
    Ok(())
}

fn require<'a>(spec: &'a PluginSpec, v: &'a Option<String>, name: &str) -> Result<&'a str, String> {
    v.as_deref()
        .ok_or_else(|| format!("plugin '{}' ({}) needs `{name}`", spec.id, spec.kind))
}

/// A local plugin's observables carry no hosted fields.
fn local_observables_only(spec: &PluginSpec) -> Result<(), String> {
    for o in &spec.observables {
        if o.key.is_some() || o.expression.is_some() {
            return Err(format!(
                "plugin '{}': {}: `key` and `expression` apply to kind 'hosted' only",
                spec.id, o.name
            ));
        }
        if o.path.is_some() || o.series.is_some() || o.x.is_some() || o.y.is_some() {
            return Err(format!(
                "plugin '{}': {}: `path`/`series` apply to kind 'engine' only",
                spec.id, o.name
            ));
        }
    }
    Ok(())
}

/// Linear interpolation in a series sorted by time; `None` outside its span.
pub fn interpolate(series: &[(f64, f64)], t: f64) -> Option<f64> {
    let first = series.first()?;
    let last = series.last()?;
    if t < first.0 || t > last.0 {
        return None;
    }
    let i = series.partition_point(|(x, _)| *x < t);
    if i < series.len() && series[i].0 == t {
        return Some(series[i].1);
    }
    let (x0, y0) = series[i - 1];
    let (x1, y1) = series[i];
    Some(y0 + (y1 - y0) * (t - x0) / (x1 - x0))
}

/// Sort a series by time and refuse repeated times.
fn sorted(mut s: Vec<(f64, f64)>, what: &str) -> Result<Vec<(f64, f64)>, String> {
    s.sort_by(|a, b| a.0.total_cmp(&b.0));
    if s.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err(format!("{what}: a time appears twice"));
    }
    if s.is_empty() {
        return Err(format!("{what}: no samples"));
    }
    Ok(s)
}

// ─────────────────────────────── table ───────────────────────────────

/// The predictions of an external simulator, as a CSV `t_s,observable,value`.
pub struct TablePlugin {
    id: String,
    observables: Vec<ObservableSpec>,
    series: BTreeMap<String, Vec<(f64, f64)>>,
    source: SourceRef,
}

impl TablePlugin {
    fn build(spec: &PluginSpec, ctx: &Context) -> Result<Box<dyn SimPlugin>, String> {
        refuse_fields(
            spec,
            &[
                ("scenario", spec.scenario.is_some()),
                ("method", spec.method.is_some()),
                ("calibration", spec.calibration.is_some()),
                ("site", !spec.sites.is_empty()),
                ("parameter", !spec.parameters.is_empty()),
                ("run", !spec.runs.is_empty()),
            ],
        )?;
        local_observables_only(spec)?;
        let path = require(spec, &spec.predictions, "predictions")?;
        let (text, source) = read_source(ctx.dir, path)?;
        let rows = super::mission::parse_predictions(&text)
            .map_err(|e| format!("plugin '{}': {e}", spec.id))?;
        let mut series = BTreeMap::new();
        for o in &spec.observables {
            let s: Vec<(f64, f64)> = rows
                .iter()
                .filter(|(_, n, _)| *n == o.name)
                .map(|(t, _, v)| (*t, *v))
                .collect();
            let s = sorted(
                s,
                &format!("plugin '{}': predictions of {}", spec.id, o.name),
            )?;
            series.insert(o.name.clone(), s);
        }
        Ok(Box::new(TablePlugin {
            id: spec.id.clone(),
            observables: spec.observables.clone(),
            series,
            source,
        }))
    }
}

impl SimPlugin for TablePlugin {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &'static str {
        "table"
    }
    fn observables(&self) -> &[ObservableSpec] {
        &self.observables
    }
    fn predict(&self, observable: &str, t_s: f64) -> Option<f64> {
        interpolate(self.series.get(observable)?, t_s)
    }
    fn sources(&self) -> Vec<SourceRef> {
        vec![self.source.clone()]
    }
}

// ─────────────────────────────── engine ───────────────────────────────

enum EnginePrediction {
    Constant(f64),
    Series(Vec<(f64, f64)>),
}

/// A scenario of any open kind (or a Pro kind the entitlement carries), run once on the
/// embedded engine; observables are read from its result document.
pub struct EnginePlugin {
    id: String,
    observables: Vec<ObservableSpec>,
    predictions: BTreeMap<String, EnginePrediction>,
    source: SourceRef,
}

fn at_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(root, |v, k| match v {
        Value::Object(m) => m.get(k),
        Value::Array(a) => k.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => None,
    })
}

impl EnginePlugin {
    fn build(spec: &PluginSpec, ctx: &Context) -> Result<Box<dyn SimPlugin>, String> {
        refuse_fields(
            spec,
            &[
                ("predictions", spec.predictions.is_some()),
                ("method", spec.method.is_some()),
                ("calibration", spec.calibration.is_some()),
                ("site", !spec.sites.is_empty()),
                ("parameter", !spec.parameters.is_empty()),
                ("run", !spec.runs.is_empty()),
            ],
        )?;
        let path = require(spec, &spec.scenario, "scenario")?;
        let (src, source) = read_source(ctx.dir, path)?;
        let run = crate::registry::run_gated(&src, ctx.entitlement, ctx.now_unix)
            .map_err(|e| format!("plugin '{}': scenario {path} did not run: {e}", spec.id))?;
        let doc: Value = serde_json::from_str(&run.json)
            .map_err(|e| format!("plugin '{}': the result is not JSON: {e}", spec.id))?;
        let mut predictions = BTreeMap::new();
        for o in &spec.observables {
            let what = format!("plugin '{}': {}", spec.id, o.name);
            let p = match (&o.path, &o.series, &o.x, &o.y) {
                (Some(p), None, None, None) => EnginePrediction::Constant(
                    at_path(&doc, p)
                        .and_then(Value::as_f64)
                        .ok_or_else(|| format!("{what}: no number at `{p}` in the result"))?,
                ),
                (None, Some(s), Some(x), Some(y)) => {
                    let arr = at_path(&doc, s)
                        .and_then(Value::as_array)
                        .ok_or_else(|| format!("{what}: no array at `{s}` in the result"))?;
                    let pts = arr
                        .iter()
                        .filter_map(|e| Some((e.get(x)?.as_f64()?, e.get(y)?.as_f64()?)))
                        .collect::<Vec<_>>();
                    if pts.len() != arr.len() {
                        return Err(format!(
                            "{what}: not every element of `{s}` has numbers `{x}` and `{y}`"
                        ));
                    }
                    EnginePrediction::Series(sorted(pts, &what)?)
                }
                _ => {
                    return Err(format!(
                        "{what}: give either `path`, or `series` with `x` and `y`"
                    ))
                }
            };
            if o.key.is_some() || o.expression.is_some() {
                return Err(format!(
                    "{what}: `key` and `expression` apply to kind 'hosted' only"
                ));
            }
            predictions.insert(o.name.clone(), p);
        }
        Ok(Box::new(EnginePlugin {
            id: spec.id.clone(),
            observables: spec.observables.clone(),
            predictions,
            source,
        }))
    }
}

impl SimPlugin for EnginePlugin {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &'static str {
        "engine"
    }
    fn observables(&self) -> &[ObservableSpec] {
        &self.observables
    }
    fn predict(&self, observable: &str, t_s: f64) -> Option<f64> {
        match self.predictions.get(observable)? {
            EnginePrediction::Constant(v) => Some(*v),
            EnginePrediction::Series(s) => interpolate(s, t_s),
        }
    }
    fn sources(&self) -> Vec<SourceRef> {
        vec![self.source.clone()]
    }
}

// ─────────────────────────────── hosted ───────────────────────────────

/// A simulator whose model runs in the hosted service: no prediction here; the scenario,
/// configuration, sites and observable keys go to the hosted method with the telemetry.
pub struct HostedPlugin {
    id: String,
    observables: Vec<ObservableSpec>,
    binding: HostedBinding,
    sources: Vec<SourceRef>,
}

fn to_json(v: &toml::Value, what: &str) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| format!("{what}: {e}"))
}

impl HostedPlugin {
    fn build(spec: &PluginSpec, ctx: &Context) -> Result<Box<dyn SimPlugin>, String> {
        refuse_fields(spec, &[("predictions", spec.predictions.is_some())])?;
        let method = require(spec, &spec.method, "method")?;
        if !kshana_pro::remote_verdict::METHODS
            .iter()
            .any(|(m, _)| *m == method)
        {
            return Err(format!(
                "plugin '{}': '{method}' is not a hosted method this build can call",
                spec.id
            ));
        }
        for o in &spec.observables {
            if o.path.is_some() || o.series.is_some() || o.x.is_some() || o.y.is_some() {
                return Err(format!(
                    "plugin '{}': {}: `path`/`series` apply to kind 'engine' only",
                    spec.id, o.name
                ));
            }
        }
        let (model, sources) = match method {
            super::hosted::SELFCAL => Self::selfcal(spec, ctx)?,
            super::hosted::CALIBRATE => Self::calibrate(spec, ctx)?,
            _ => {
                return Err(format!(
                    "plugin '{}': method '{method}' is not a calibration method (use '{}' or '{}')",
                    spec.id,
                    super::hosted::SELFCAL,
                    super::hosted::CALIBRATE
                ))
            }
        };
        Ok(Box::new(HostedPlugin {
            id: spec.id.clone(),
            observables: spec.observables.clone(),
            binding: HostedBinding {
                method: method.to_string(),
                model,
            },
            sources,
        }))
    }

    /// `selfcal`: the scenario, `[plugin.calibration]`, the sites and a key per observable.
    fn selfcal(spec: &PluginSpec, ctx: &Context) -> Result<(HostedModel, Vec<SourceRef>), String> {
        refuse_fields(
            spec,
            &[
                ("parameter", !spec.parameters.is_empty()),
                ("run", !spec.runs.is_empty()),
            ],
        )?;
        let path = require(spec, &spec.scenario, "scenario")?;
        let (scenario_text, source) = read_source(ctx.dir, path)?;
        let config = to_json(
            spec.calibration.as_ref().ok_or_else(|| {
                format!("plugin '{}' (hosted) needs [plugin.calibration]", spec.id)
            })?,
            "calibration",
        )?;
        if spec.sites.is_empty() {
            return Err(format!(
                "plugin '{}' (hosted) needs at least one [[plugin.site]]",
                spec.id
            ));
        }
        let sites = spec
            .sites
            .iter()
            .map(|s| to_json(s, "site"))
            .collect::<Result<Vec<_>, _>>()?;
        let mut keys = BTreeMap::new();
        for o in &spec.observables {
            if o.expression.is_some() {
                return Err(format!(
                    "plugin '{}': {}: `expression` applies to method 'calibrate' only",
                    spec.id, o.name
                ));
            }
            let key = o
                .key
                .as_ref()
                .ok_or_else(|| format!("plugin '{}': {} needs a `key` table", spec.id, o.name))?;
            let key = to_json(key, "key")?;
            if !key.is_object() {
                return Err(format!(
                    "plugin '{}': {}: `key` must be a table",
                    spec.id, o.name
                ));
            }
            keys.insert(o.name.clone(), key);
        }
        Ok((
            HostedModel::Selfcal {
                scenario_text,
                config,
                sites,
                keys,
            },
            vec![source],
        ))
    }

    /// `calibrate`: `[[plugin.parameter]]`, then either an `expression` on every observable
    /// or `[[plugin.run]]` design runs; `[plugin.calibration]` may set `subject` and
    /// `max_iter`.
    fn calibrate(
        spec: &PluginSpec,
        ctx: &Context,
    ) -> Result<(HostedModel, Vec<SourceRef>), String> {
        let id = &spec.id;
        refuse_fields(
            spec,
            &[
                ("scenario", spec.scenario.is_some()),
                ("site", !spec.sites.is_empty()),
            ],
        )?;
        for o in &spec.observables {
            if o.key.is_some() {
                return Err(format!(
                    "plugin '{id}': {}: `key` applies to method 'selfcal' only",
                    o.name
                ));
            }
        }
        // Options.
        let (mut subject, mut max_iter) = (None, None);
        if let Some(c) = &spec.calibration {
            let t = c
                .as_table()
                .ok_or_else(|| format!("plugin '{id}': [plugin.calibration] must be a table"))?;
            for (k, v) in t {
                match k.as_str() {
                    "subject" => {
                        subject = Some(
                            v.as_str()
                                .ok_or_else(|| {
                                    format!("plugin '{id}': calibration.subject must be text")
                                })?
                                .to_string(),
                        )
                    }
                    "max_iter" => {
                        max_iter = Some(
                            v.as_integer()
                                .filter(|n| (1..=100).contains(n))
                                .ok_or_else(|| {
                                    format!(
                                        "plugin '{id}': calibration.max_iter must be an integer in 1..=100"
                                    )
                                })? as u64,
                        )
                    }
                    other => {
                        return Err(format!(
                            "plugin '{id}': calibration.{other} does not apply to method 'calibrate' (subject, max_iter)"
                        ))
                    }
                }
            }
        }
        // Parameters.
        if spec.parameters.is_empty() {
            return Err(format!(
                "plugin '{id}' (calibrate) needs at least one [[plugin.parameter]]"
            ));
        }
        if spec.parameters.len() > MAX_CALIBRATE_PARAMETERS {
            return Err(format!(
                "plugin '{id}': at most {MAX_CALIBRATE_PARAMETERS} parameters"
            ));
        }
        let mut names = std::collections::BTreeSet::new();
        for q in &spec.parameters {
            if !parameter_name(&q.name) {
                return Err(format!(
                    "plugin '{id}': parameter name '{}' must be letters, digits and '_', not starting with a digit, and not t or pi",
                    q.name
                ));
            }
            if !names.insert(q.name.as_str()) {
                return Err(format!(
                    "plugin '{id}': parameter '{}' is declared twice",
                    q.name
                ));
            }
            let finite = [q.prior, q.prior_sigma, q.lower, q.upper, q.initial]
                .iter()
                .all(|v| v.is_none_or(f64::is_finite));
            if !finite {
                return Err(format!(
                    "plugin '{id}': parameter '{}': a value is not finite",
                    q.name
                ));
            }
            if q.prior_sigma.is_some_and(|s| s <= 0.0) {
                return Err(format!(
                    "plugin '{id}': parameter '{}': prior_sigma must be positive",
                    q.name
                ));
            }
            if q.prior_sigma.is_some() && q.prior.is_none() {
                return Err(format!(
                    "plugin '{id}': parameter '{}': prior_sigma needs a prior",
                    q.name
                ));
            }
            if let (Some(l), Some(h)) = (q.lower, q.upper) {
                if l >= h {
                    return Err(format!(
                        "plugin '{id}': parameter '{}': lower must be below upper",
                        q.name
                    ));
                }
            }
            if q.prior.is_none() && q.initial.is_none() && (q.lower.is_none() || q.upper.is_none())
            {
                return Err(format!(
                    "plugin '{id}': parameter '{}': give a prior, an initial value or both bounds",
                    q.name
                ));
            }
        }
        // Model: an expression on every observable, or design runs.
        let with_expr = spec
            .observables
            .iter()
            .filter(|o| o.expression.is_some())
            .count();
        let mut sources = Vec::new();
        let model = match (with_expr, spec.runs.is_empty()) {
            (n, true) if n == spec.observables.len() => {
                let mut m = BTreeMap::new();
                for o in &spec.observables {
                    let e = o.expression.as_deref().unwrap_or_default();
                    if e.trim().is_empty() {
                        return Err(format!("plugin '{id}': {}: empty expression", o.name));
                    }
                    m.insert(o.name.clone(), e.to_string());
                }
                CalibrateModel::Expression(m)
            }
            (0, false) => {
                let mut runs = Vec::new();
                for (k, r) in spec.runs.iter().enumerate() {
                    let what = format!("plugin '{id}': run {}", k + 1);
                    let have: std::collections::BTreeSet<&str> =
                        r.params.keys().map(String::as_str).collect();
                    if have != names {
                        return Err(format!(
                            "{what}: `params` must give every parameter ({}) and nothing else",
                            names.iter().copied().collect::<Vec<_>>().join(", ")
                        ));
                    }
                    if r.params.values().any(|v| !v.is_finite()) {
                        return Err(format!("{what}: a parameter value is not finite"));
                    }
                    let (text, source) = read_source(ctx.dir, &r.predictions)?;
                    let rows = super::mission::parse_predictions(&text)
                        .map_err(|e| format!("{what}: {e}"))?;
                    if let Some((_, n, _)) = rows
                        .iter()
                        .find(|(_, n, _)| !spec.observables.iter().any(|o| o.name == *n))
                    {
                        return Err(format!(
                            "{what}: predicts '{n}', which the plugin does not declare"
                        ));
                    }
                    let mut predictions = BTreeMap::new();
                    for o in &spec.observables {
                        let s: Vec<(f64, f64)> = rows
                            .iter()
                            .filter(|(_, n, _)| *n == o.name)
                            .map(|(t, _, v)| (*t, *v))
                            .collect();
                        let s = sorted(s, &format!("{what}: predictions of {}", o.name))?;
                        predictions.insert(o.name.clone(), s);
                    }
                    sources.push(source);
                    runs.push(DesignRun {
                        params: r.params.clone(),
                        predictions,
                    });
                }
                CalibrateModel::Design(runs)
            }
            (0, true) => {
                return Err(format!(
                    "plugin '{id}' (calibrate) needs an `expression` on every observable, or [[plugin.run]] design runs"
                ))
            }
            (_, false) => {
                return Err(format!(
                    "plugin '{id}' (calibrate): give either expressions or design runs, not both"
                ))
            }
            (_, true) => {
                return Err(format!(
                    "plugin '{id}' (calibrate): every observable needs an `expression` (or use design runs)"
                ))
            }
        };
        Ok((
            HostedModel::Calibrate(CalibrateBinding {
                subject,
                max_iter,
                parameters: spec.parameters.clone(),
                model,
            }),
            sources,
        ))
    }
}

/// A `calibrate` parameter name: a plain identifier that is not `t` or `pi`.
fn parameter_name(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s != "t"
        && s != "pi"
}

impl SimPlugin for HostedPlugin {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &'static str {
        "hosted"
    }
    fn observables(&self) -> &[ObservableSpec] {
        &self.observables
    }
    fn predict(&self, _observable: &str, _t_s: f64) -> Option<f64> {
        None
    }
    fn sources(&self) -> Vec<SourceRef> {
        self.sources.clone()
    }
    fn hosted(&self) -> Option<&HostedBinding> {
        Some(&self.binding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_stays_inside_the_span() {
        let s = vec![(0.0, 0.0), (10.0, 10.0), (20.0, 0.0)];
        assert_eq!(interpolate(&s, 5.0), Some(5.0));
        assert_eq!(interpolate(&s, 10.0), Some(10.0));
        assert_eq!(interpolate(&s, 15.0), Some(5.0));
        assert_eq!(interpolate(&s, 0.0), Some(0.0));
        assert_eq!(interpolate(&s, 20.0), Some(0.0));
        assert_eq!(interpolate(&s, -0.1), None);
        assert_eq!(interpolate(&s, 20.1), None);
        assert_eq!(interpolate(&[(3.0, 7.0)], 3.0), Some(7.0));
        assert_eq!(interpolate(&[], 3.0), None);
    }

    #[test]
    fn repeated_times_are_refused() {
        assert!(sorted(vec![(1.0, 1.0), (1.0, 2.0)], "x").is_err());
        assert_eq!(
            sorted(vec![(2.0, 1.0), (1.0, 2.0)], "x").unwrap(),
            vec![(1.0, 2.0), (2.0, 1.0)]
        );
    }

    #[test]
    fn dot_paths_reach_objects_and_arrays() {
        let v = serde_json::json!({"a": {"b": [1.0, {"c": 2.5}]}});
        assert_eq!(at_path(&v, "a.b.1.c").and_then(Value::as_f64), Some(2.5));
        assert!(at_path(&v, "a.x").is_none());
    }
}
