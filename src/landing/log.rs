// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The documented interface for a navigation filter's logged outputs: one CSV per scenario,
//! named `<scenario id>.csv`.
//!
//! ```text
//! # comment lines start with '#'
//! t_s,alt_est_m,alt_sigma_m,vz_est_mps,vz_sigma_mps,alt_health,alt_used,lrf_health,lrf_used
//! 0.0,2000.1,5.0,-60.0,0.5,off,0,ok,1
//! ```
//!
//! * `t_s`: seconds from the start of the stream (strictly increasing);
//! * `alt_est_m`: the filter's altitude above the landing-site datum; `alt_sigma_m` its
//!   stated 1-sigma uncertainty (> 0);
//! * `vz_est_mps`: its vertical speed, negative downwards; `vz_sigma_mps` is optional;
//! * for every campaign sensor `<id>`: `<id>_health`, the filter's health state for it
//!   (`ok`, `fault` or `off`), and `<id>_used`, whether its latest measurement is used in
//!   the estimate (`1` or `0`).
//!
//! Other columns are ignored (and listed in the verdict), so a provider can log more.

use super::campaign::Campaign;
use super::err;
use kshana::api::KshanaError;

/// The filter's health state for one sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Ok,
    Fault,
    Off,
}

/// One sensor's state at one epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorState {
    pub health: Health,
    pub used: bool,
}

impl SensorState {
    /// Whether the filter treats the sensor as good and uses it.
    pub fn accepted(&self) -> bool {
        self.health == Health::Ok && self.used
    }
}

/// One logged epoch.
#[derive(Debug, Clone)]
pub struct Epoch {
    pub t_s: f64,
    pub alt_est_m: f64,
    pub alt_sigma_m: f64,
    pub vz_est_mps: f64,
    pub vz_sigma_mps: Option<f64>,
    /// In campaign sensor order.
    pub sensors: Vec<SensorState>,
}

/// A parsed log.
#[derive(Debug, Clone)]
pub struct FilterLog {
    pub epochs: Vec<Epoch>,
    pub ignored_columns: Vec<String>,
}

/// Parse a filter log against the campaign's sensors. Any malformed row is refused with
/// its line number.
pub fn parse(text: &str, c: &Campaign) -> Result<FilterLog, KshanaError> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'));
    let (_, header) = lines.next().ok_or_else(|| err("the log is empty"))?;
    let cols: Vec<&str> = header.split(',').map(str::trim).collect();
    let find = |name: &str| cols.iter().position(|c| *c == name);
    let need =
        |name: &str| find(name).ok_or_else(|| err(format!("the log has no '{name}' column")));
    let (it, ia, isig, iv) = (
        need("t_s")?,
        need("alt_est_m")?,
        need("alt_sigma_m")?,
        need("vz_est_mps")?,
    );
    let ivs = find("vz_sigma_mps");
    let mut sensor_cols = Vec::new();
    for s in &c.sensors {
        sensor_cols.push((
            need(&format!("{}_health", s.id))?,
            need(&format!("{}_used", s.id))?,
        ));
    }
    let mut known: Vec<usize> = vec![it, ia, isig, iv];
    known.extend(ivs);
    for (h, u) in &sensor_cols {
        known.extend([*h, *u]);
    }
    let ignored_columns = cols
        .iter()
        .enumerate()
        .filter(|(i, _)| !known.contains(i))
        .map(|(_, c)| c.to_string())
        .collect();
    let mut epochs: Vec<Epoch> = Vec::new();
    for (n, line) in lines {
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() != cols.len() {
            return Err(err(format!(
                "log line {n}: {} fields, the header has {}",
                f.len(),
                cols.len()
            )));
        }
        let num = |i: usize, what: &str| -> Result<f64, KshanaError> {
            f[i].parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| {
                    err(format!(
                        "log line {n}: {what} '{}' is not a finite number",
                        f[i]
                    ))
                })
        };
        let t_s = num(it, "t_s")?;
        if let Some(p) = epochs.last() {
            if t_s <= p.t_s {
                return Err(err(format!("log line {n}: t_s must increase")));
            }
        }
        let alt_sigma_m = num(isig, "alt_sigma_m")?;
        if alt_sigma_m <= 0.0 {
            return Err(err(format!("log line {n}: alt_sigma_m must be positive")));
        }
        let vz_sigma_mps = match ivs {
            Some(i) if !f[i].is_empty() => Some(num(i, "vz_sigma_mps")?),
            _ => None,
        };
        let mut sensors = Vec::with_capacity(sensor_cols.len());
        for (si, (h, u)) in sensor_cols.iter().enumerate() {
            let id = &c.sensors[si].id;
            let health = match f[*h].to_ascii_lowercase().as_str() {
                "ok" => Health::Ok,
                "fault" => Health::Fault,
                "off" => Health::Off,
                v => {
                    return Err(err(format!(
                        "log line {n}: {id}_health '{v}' must be ok, fault or off"
                    )))
                }
            };
            let used = match f[*u] {
                "1" => true,
                "0" => false,
                v => return Err(err(format!("log line {n}: {id}_used '{v}' must be 1 or 0"))),
            };
            sensors.push(SensorState { health, used });
        }
        epochs.push(Epoch {
            t_s,
            alt_est_m: num(ia, "alt_est_m")?,
            alt_sigma_m,
            vz_est_mps: num(iv, "vz_est_mps")?,
            vz_sigma_mps,
            sensors,
        });
    }
    if epochs.is_empty() {
        return Err(err("the log has a header but no epochs"));
    }
    Ok(FilterLog {
        epochs,
        ignored_columns,
    })
}
