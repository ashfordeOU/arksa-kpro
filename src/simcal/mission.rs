// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The mission file and the telemetry file.
//!
//! ```toml
//! format = "ks-simcal-mission/1"
//! mission = "TOY-1"
//! title = "Toy LEO mission, end-to-end performance simulator"   # optional
//! telemetry = "telemetry.csv"
//!
//! [[plugin]]
//! id = "eps"
//! kind = "table"                  # predictions of an external simulator
//! predictions = "eps-sim.csv"
//! [[plugin.observable]]
//! name = "bus_power_w"
//! unit = "W"
//! tolerance = 4.0                 # |bias| or RMS above this: drifted
//!
//! [[plugin]]
//! id = "clock"
//! kind = "engine"                 # a scenario of an open kind, run on the embedded engine
//! scenario = "clock-holdover.toml"
//! [[plugin.observable]]
//! name = "time_error_ns"
//! tolerance = 50.0
//! series = "classical.series"     # an array of objects ...
//! x = "t"                         # ... its time field (s) ...
//! y = "error_ns"                  # ... and its value field
//! # or: path = "global.availability_pct" for a constant prediction
//!
//! [[plugin]]
//! id = "timing"
//! kind = "hosted"                 # the model runs in the hosted service
//! method = "selfcal"
//! scenario = "timing-scenario.toml"
//! [plugin.calibration]            # passed to the method as its configuration
//! # ...
//! [[plugin.site]]                 # passed to the method as its reference sites
//! # ...
//! [[plugin.observable]]
//! name = "toa_s1_m"
//! unit = "ns"
//! tolerance = 20.0
//! [plugin.observable.key]         # what identifies this observable to the method
//! # ...
//! ```
//!
//! Telemetry (`t_s,observable,value,sigma`; `#` starts a comment; `sigma`, the standard
//! uncertainty of the value, may be empty except for a hosted plugin's observables):
//!
//! ```text
//! t_s,observable,value,sigma
//! 0,eps.bus_power_w,412.0,2.0
//! ```
//!
//! An observable is named `<plugin id>.<observable name>`. Paths are relative to the mission
//! file. Every file is read once as bytes, and those bytes are what is hashed and used.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::sha256_hex;

/// Identifies a mission file.
pub const FORMAT: &str = "ks-simcal-mission/1";

/// The mission file, as written.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissionFile {
    pub format: String,
    pub mission: String,
    #[serde(default)]
    pub title: Option<String>,
    /// The tenant/mission tag sent to the hosted verdict service for cost tracking
    /// ([`crate::verdict_service`]); default: `mission`.
    #[serde(default)]
    pub tenant: Option<String>,
    pub telemetry: String,
    #[serde(rename = "plugin", default)]
    pub plugins: Vec<PluginSpec>,
}

/// One simulator plugin of the mission. Which fields apply depends on `kind`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSpec {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub description: Option<String>,
    /// `table`: the predictions CSV (`t_s,observable,value`, observable = its name).
    #[serde(default)]
    pub predictions: Option<String>,
    /// `engine`, `hosted`: the simulator scenario.
    #[serde(default)]
    pub scenario: Option<String>,
    /// `hosted`: the hosted method.
    #[serde(default)]
    pub method: Option<String>,
    /// `hosted`: the method's configuration, passed through.
    #[serde(default)]
    pub calibration: Option<toml::Value>,
    /// `hosted`: the method's reference sites, passed through.
    #[serde(rename = "site", default)]
    pub sites: Vec<toml::Value>,
    #[serde(rename = "observable", default)]
    pub observables: Vec<ObservableSpec>,
}

/// One observable a plugin predicts.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ObservableSpec {
    pub name: String,
    #[serde(default)]
    pub unit: Option<String>,
    /// Acceptance bound on |bias| and RMS of the residuals, in the observable's unit.
    pub tolerance: f64,
    /// `engine`: a number at this dot path (a constant prediction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `engine`: an array of objects at this dot path, with time field `x` and value `y`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    /// `hosted`: the fields that identify this observable to the hosted method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<toml::Value>,
}

/// One telemetry sample.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TelemetryRow {
    pub t_s: f64,
    /// `<plugin id>.<observable name>`.
    pub observable: String,
    pub value: f64,
    /// Standard uncertainty of `value`, if given.
    pub sigma: Option<f64>,
}

/// A file the mission was built from, with the SHA-256 of its exact bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceRef {
    /// The path as the mission file spells it.
    pub path: String,
    pub sha256: String,
}

/// A loaded mission: the file, its directory, the telemetry, and the hashes.
#[derive(Debug, Clone)]
pub struct Mission {
    pub file: MissionFile,
    pub dir: PathBuf,
    pub mission_sha256: String,
    pub telemetry: Vec<TelemetryRow>,
    pub telemetry_sha256: String,
}

/// Read `rel` (relative to `dir`) as bytes and as text, with its [`SourceRef`].
pub fn read_source(dir: &Path, rel: &str) -> Result<(String, SourceRef), String> {
    let p = dir.join(rel);
    let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let text = String::from_utf8(bytes.clone()).map_err(|_| format!("{rel}: not UTF-8 text"))?;
    Ok((
        text,
        SourceRef {
            path: rel.to_string(),
            sha256: sha256_hex(&bytes),
        },
    ))
}

impl Mission {
    /// Load and check a mission file and its telemetry.
    pub fn load(path: &Path) -> Result<Mission, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let text = String::from_utf8(bytes.clone())
            .map_err(|_| format!("{}: not UTF-8 text", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        Mission::from_text(&text, &dir).map(|mut m| {
            m.mission_sha256 = sha256_hex(&bytes);
            m
        })
    }

    /// Parse a mission file's text; files it names are read relative to `dir`.
    pub fn from_text(text: &str, dir: &Path) -> Result<Mission, String> {
        let file: MissionFile =
            toml::from_str(text).map_err(|e| format!("invalid mission file: {e}"))?;
        file.validate()?;
        let (tel_text, tel_ref) = read_source(dir, &file.telemetry)?;
        let telemetry = parse_telemetry(&tel_text)?;
        let known: BTreeSet<String> = file
            .plugins
            .iter()
            .flat_map(|p| {
                p.observables
                    .iter()
                    .map(move |o| format!("{}.{}", p.id, o.name))
            })
            .collect();
        if let Some(r) = telemetry.iter().find(|r| !known.contains(&r.observable)) {
            return Err(format!(
                "telemetry names observable '{}', which no plugin declares",
                r.observable
            ));
        }
        Ok(Mission {
            file,
            dir: dir.to_path_buf(),
            mission_sha256: sha256_hex(text.as_bytes()),
            telemetry,
            telemetry_sha256: tel_ref.sha256,
        })
    }

    /// The telemetry of one observable (`<plugin>.<name>`), in file order.
    pub fn telemetry_of(&self, observable: &str) -> Vec<&TelemetryRow> {
        self.telemetry
            .iter()
            .filter(|r| r.observable == observable)
            .collect()
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl MissionFile {
    /// Structural checks that do not depend on the plugin kind.
    pub fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT {
            return Err(format!("format is '{}', expected '{FORMAT}'", self.format));
        }
        if self.mission.trim().is_empty() {
            return Err("mission must not be empty".into());
        }
        if self.plugins.is_empty() {
            return Err("a mission names at least one [[plugin]]".into());
        }
        let mut ids = BTreeSet::new();
        for p in &self.plugins {
            if !valid_id(&p.id) {
                return Err(format!(
                    "plugin id '{}' must be letters, digits, '_' or '-'",
                    p.id
                ));
            }
            if !ids.insert(p.id.as_str()) {
                return Err(format!("plugin id '{}' is used twice", p.id));
            }
            if p.observables.is_empty() {
                return Err(format!("plugin '{}' declares no observable", p.id));
            }
            let mut names = BTreeSet::new();
            for o in &p.observables {
                if !valid_id(&o.name) {
                    return Err(format!(
                        "plugin '{}': observable name '{}' must be letters, digits, '_' or '-'",
                        p.id, o.name
                    ));
                }
                if !names.insert(o.name.as_str()) {
                    return Err(format!(
                        "plugin '{}': observable '{}' is declared twice",
                        p.id, o.name
                    ));
                }
                if !(o.tolerance.is_finite() && o.tolerance > 0.0) {
                    return Err(format!(
                        "{}.{}: tolerance must be a positive number",
                        p.id, o.name
                    ));
                }
            }
        }
        Ok(())
    }
}

fn number(field: &str, what: &str, line: usize) -> Result<f64, String> {
    let v: f64 = field
        .trim()
        .parse()
        .map_err(|_| format!("line {line}: {what} '{}' is not a number", field.trim()))?;
    if !v.is_finite() {
        return Err(format!("line {line}: {what} is not finite"));
    }
    Ok(v)
}

/// Parse telemetry CSV text (header `t_s,observable,value,sigma`; `sigma` optional).
pub fn parse_telemetry(text: &str) -> Result<Vec<TelemetryRow>, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'));
    let (_, header) = lines.next().ok_or("telemetry is empty")?;
    let cols: Vec<&str> = header.split(',').map(str::trim).collect();
    if cols.len() < 3 || cols[..3] != ["t_s", "observable", "value"] {
        return Err("telemetry header must be t_s,observable,value[,sigma]".into());
    }
    let has_sigma = cols.get(3) == Some(&"sigma");
    let mut rows = Vec::new();
    for (n, l) in lines {
        let f: Vec<&str> = l.split(',').collect();
        if f.len() < 3 {
            return Err(format!("line {n}: expected t_s,observable,value[,sigma]"));
        }
        let sigma = match (has_sigma, f.get(3).map(|s| s.trim())) {
            (true, Some(s)) if !s.is_empty() => {
                let s = number(s, "sigma", n)?;
                if s <= 0.0 {
                    return Err(format!("line {n}: sigma must be positive"));
                }
                Some(s)
            }
            _ => None,
        };
        rows.push(TelemetryRow {
            t_s: number(f[0], "t_s", n)?,
            observable: f[1].trim().to_string(),
            value: number(f[2], "value", n)?,
            sigma,
        });
    }
    if rows.is_empty() {
        return Err("telemetry holds no samples".into());
    }
    Ok(rows)
}

/// Parse a predictions CSV (`t_s,observable,value`) into rows.
pub fn parse_predictions(text: &str) -> Result<Vec<(f64, String, f64)>, String> {
    let rows = parse_telemetry(text).map_err(|e| format!("predictions: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|r| (r.t_s, r.observable, r.value))
        .collect())
}
