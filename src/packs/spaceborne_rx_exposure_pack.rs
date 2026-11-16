// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # `pro-spaceborne-rx-exposure` (roadmap 0.42 F4, Arksa Ops pillar)
//!
//! Spaceborne GNSS receiver interference assurance: how much of a spacecraft's orbit is spent
//! with ground interference in radio view, and what that could do to its receiver.
//!
//! Inputs, all public or the user's:
//! * the spacecraft's two-line element sets, propagated with the open engine's SGP4
//!   ([`kshana::sgp4::SgpOrbit`], TEME to ITRS);
//! * interference maps in the open engine's v1 GeoJSON format (`kshana_interference_map`,
//!   degraded or anomalous cells), emitter-region summaries (`kshana_emitter_region`, located
//!   sessions only) and areas the user states.
//!
//! Outputs: per-pass exposure (an interval with at least one affected area above the
//! elevation mask, seen from the area), time with the sub-satellite point over an affected
//! area, and a modelled effective C/N0 margin for each assumed interferer EIRP, from the open
//! engine's anti-jam equation ([`kshana::jamming::effective_cn0_dbhz`]).
//!
//! Evidence class: MODELLED throughout. Nothing here is measured on board: the maps say where
//! aircraft or ships reported degraded navigation (they do not give an emitter power or a
//! position), the interferer EIRP and the antenna gains are assumptions stated in the result,
//! and a map from another period than the orbit is used as a climatology and labelled so.

use std::path::Path;

use kshana::api::{ExternalPack, KshanaError, RunOutput, Scenario, ScenarioMeta};
use kshana::frames::{ecef_to_geodetic, geodetic_to_ecef, Geodetic};
use kshana::jamming::{effective_cn0_dbhz, free_space_path_loss_db, nominal_cn0_dbhz, L1_HZ};
use kshana::jd2::Jd2;
use kshana::sgp4::{teme_to_itrs_matrix, SgpOrbit};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The kind id (also the command feature id).
pub const KIND: &str = "pro-spaceborne-rx-exposure";
/// The result schema.
pub const SCHEMA: &str = "arksa-spaceborne-rx-exposure/1";

/// The honesty label on every result.
pub const LABEL: &str = "MODELLED spaceborne GNSS receiver interference exposure. MEASURED: nothing \
on board; the inputs are published orbital elements (TLEs, propagated with SGP4) and interference maps \
built from aircraft or ship navigation-accuracy reports, which say where reports were degraded, not \
where an emitter was or how strong it was. MODELLED: the interferer EIRP (assumed, several values), the \
receive-antenna gains, free-space propagation, the anti-jam equation and, when the map period differs \
from the orbit period, the use of the map as a climatology. Not a statement about any receiver's \
measured performance.";

const DEG: f64 = std::f64::consts::PI / 180.0;
const KM: f64 = 1000.0;

// ── Scenario ─────────────────────────────────────────────────────────────────────────────

fn d_duration_h() -> f64 {
    24.0
}
fn d_step_s() -> f64 {
    10.0
}
fn d_eirp() -> Vec<f64> {
    vec![0.0, 20.0, 40.0]
}
fn d_mode() -> String {
    "auto".into()
}
fn d_emitter_height() -> f64 {
    10.0
}

/// The receiver assumptions (all MODELLED; defaults stated in the result).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receiver {
    /// Antenna gain toward the ground interferer (dBi). A zenith-pointing patch sees the Earth
    /// in its back lobe; -15 dBi is an assumption, not a pattern.
    #[serde(default = "Receiver::d_gain_ground")]
    pub gain_toward_ground_dbi: f64,
    /// Antenna gain toward the GNSS satellites (dBi).
    #[serde(default)]
    pub gain_toward_gnss_dbi: f64,
    /// Received GNSS signal power, isotropic (dBW). The ground-level minimum; in low orbit the
    /// signal is slightly stronger, so this is conservative.
    #[serde(default = "Receiver::d_signal")]
    pub signal_power_dbw: f64,
    /// System noise temperature (K).
    #[serde(default = "Receiver::d_temp")]
    pub system_noise_temp_k: f64,
    /// Carrier tracking threshold (dB-Hz).
    #[serde(default = "Receiver::d_threshold")]
    pub tracking_threshold_dbhz: f64,
    /// Margin under which tracking is called degraded (dB).
    #[serde(default = "Receiver::d_degraded")]
    pub degraded_margin_db: f64,
    /// Spectral separation coefficient Q (2 for broadband noise, 1 for a carrier-wave tone).
    #[serde(default = "Receiver::d_q")]
    pub q: f64,
}

impl Receiver {
    fn d_gain_ground() -> f64 {
        -15.0
    }
    fn d_signal() -> f64 {
        kshana::jamming::DEFAULT_SIGNAL_POWER_DBW
    }
    fn d_temp() -> f64 {
        kshana::jamming::DEFAULT_TEMP_K
    }
    fn d_threshold() -> f64 {
        kshana::jamming::DEFAULT_TRACKING_THRESHOLD_DBHZ
    }
    fn d_degraded() -> f64 {
        kshana::jamming::DEFAULT_DEGRADED_MARGIN_DB
    }
    fn d_q() -> f64 {
        kshana::jamming::Q_BROADBAND
    }
}

impl Default for Receiver {
    fn default() -> Self {
        toml::from_str("").expect("defaults")
    }
}

/// An affected area the user states.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AreaSpec {
    pub label: String,
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub radius_km: f64,
    /// Optional UTC window `[start, end]` (`YYYY-MM-DDTHH:MM[:SS]Z`).
    #[serde(default)]
    pub window_utc: Option<[String; 2]>,
}

/// A `pro-spaceborne-rx-exposure` scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceborneRxExposureScenario {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Two-line element sets, inline (two-line or three-line form, several allowed).
    #[serde(default)]
    pub tle: Option<String>,
    /// A file of element sets (relative to the scenario's directory on the command line).
    #[serde(default)]
    pub tle_file: Option<String>,
    /// Start of the span (UTC); default the first element set's epoch.
    #[serde(default)]
    pub start_utc: Option<String>,
    #[serde(default = "d_duration_h")]
    pub duration_h: f64,
    #[serde(default = "d_step_s")]
    pub step_s: f64,
    /// Elevation mask at the interferer (deg): the spacecraft is in radio view of an area when
    /// it is at or above this elevation seen from the area's emitter point.
    #[serde(default)]
    pub min_elevation_deg: f64,
    /// Emitter antenna height above the ellipsoid (m).
    #[serde(default = "d_emitter_height")]
    pub emitter_height_m: f64,
    /// Assumed interferer EIRP per affected area (dBW); one margin per value.
    #[serde(default = "d_eirp")]
    pub assumed_eirp_dbw: Vec<f64>,
    /// `auto`, `coincident` (an area counts only inside its own time window) or `climatology`
    /// (every area counts all the time). `auto` is coincident when any area's window overlaps
    /// the span, climatology otherwise.
    #[serde(default = "d_mode")]
    pub time_mode: String,
    /// Interference maps, v1 GeoJSON text.
    #[serde(default)]
    pub map_geojson: Vec<String>,
    #[serde(default)]
    pub map_files: Vec<String>,
    /// Emitter-region summaries, JSON text.
    #[serde(default)]
    pub emitter_region_json: Vec<String>,
    #[serde(default)]
    pub emitter_region_files: Vec<String>,
    #[serde(default)]
    pub area: Vec<AreaSpec>,
    #[serde(default)]
    pub receiver: Receiver,
}

impl SpaceborneRxExposureScenario {
    pub const KIND: &'static str = KIND;

    pub fn meta_static() -> ScenarioMeta {
        ScenarioMeta {
            name: Self::KIND,
            description: "Spaceborne GNSS receiver interference assurance: exposure of a spacecraft's receiver to ground interference along its orbit, from public interference maps and TLEs: per-pass exposure, time over affected areas and a modelled C/N0 margin for assumed interferer powers. MODELLED.",
            required_fields: &[],
            optional_fields: &[
                "title",
                "tle",
                "tle_file",
                "start_utc",
                "duration_h",
                "step_s",
                "min_elevation_deg",
                "emitter_height_m",
                "assumed_eirp_dbw",
                "time_mode",
                "map_geojson",
                "map_files",
                "emitter_region_json",
                "emitter_region_files",
                "area",
                "receiver",
            ],
        }
    }

    /// Read the referenced files (relative to `base`) into the inline fields.
    pub fn resolve_files(&mut self, base: &Path) -> Result<(), KshanaError> {
        let read = |p: &str| {
            std::fs::read_to_string(base.join(p))
                .map_err(|e| KshanaError::InvalidInput(format!("{p}: {e}")))
        };
        if let Some(f) = self.tle_file.take() {
            let t = read(&f)?;
            self.tle = Some(match self.tle.take() {
                Some(s) => format!("{s}\n{t}"),
                None => t,
            });
        }
        for f in std::mem::take(&mut self.map_files) {
            self.map_geojson.push(read(&f)?);
        }
        for f in std::mem::take(&mut self.emitter_region_files) {
            self.emitter_region_json.push(read(&f)?);
        }
        Ok(())
    }
}

fn bad(m: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(format!("{KIND}: {}", m.into()))
}

// ── Time ─────────────────────────────────────────────────────────────────────────────────

/// Parse `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM[:SS[.s]]Z`.
pub fn parse_utc(s: &str) -> Result<Jd2, KshanaError> {
    let t = s.trim().trim_end_matches('Z');
    let (d, hms) = t.split_once('T').unwrap_or((t, "00:00"));
    let dp: Vec<&str> = d.split('-').collect();
    let hp: Vec<&str> = hms.split(':').collect();
    let num = |x: Option<&&str>| x.and_then(|v| v.parse::<f64>().ok());
    let (Some(y), Some(mo), Some(da)) = (num(dp.first()), num(dp.get(1)), num(dp.get(2))) else {
        return Err(bad(format!("'{s}' is not a UTC date")));
    };
    let h = num(hp.first()).unwrap_or(0.0);
    let mi = num(hp.get(1)).unwrap_or(0.0);
    let se = num(hp.get(2)).unwrap_or(0.0);
    if dp.len() != 3 || hp.len() > 3 {
        return Err(bad(format!("'{s}' is not a UTC date")));
    }
    Jd2::from_utc_calendar(y as i32, mo as u32, da as u32, h as u32, mi as u32, se).map_err(bad)
}

/// Format a UTC Julian date as `YYYY-MM-DDTHH:MM:SSZ` (to the second).
pub fn fmt_utc(t: Jd2) -> String {
    let secs = (t.diff_seconds(Jd2::new(2_440_587.5))).round() as i64;
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

// ── Areas ────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Shape {
    Cell { s: f64, w: f64, n: f64, e: f64 },
    Ellipse { a_km: f64, b_km: f64, az_deg: f64 },
    Circle { r_km: f64 },
}

/// One affected area: where interference was reported, with its emitter point (the centre,
/// an assumption: the maps do not locate an emitter) and its time window.
#[derive(Debug, Clone)]
pub struct Area {
    pub label: String,
    pub source: String,
    pub lat_deg: f64,
    pub lon_deg: f64,
    shape: Shape,
    pub window: Option<(Jd2, Jd2)>,
    emitter_ecef: [f64; 3],
    up: [f64; 3],
}

impl Area {
    fn new(
        label: String,
        source: String,
        lat: f64,
        lon: f64,
        shape: Shape,
        window: Option<(Jd2, Jd2)>,
        h_m: f64,
    ) -> Self {
        let g = Geodetic {
            lat_rad: lat * DEG,
            lon_rad: lon * DEG,
            alt_m: h_m,
        };
        let (sp, cp) = (lat * DEG).sin_cos();
        let (sl, cl) = (lon * DEG).sin_cos();
        Area {
            label,
            source,
            lat_deg: lat,
            lon_deg: lon,
            shape,
            window,
            emitter_ecef: geodetic_to_ecef(g),
            up: [cp * cl, cp * sl, sp],
        }
    }

    /// Whether the geodetic point (deg) is inside the area.
    fn contains(&self, lat: f64, lon: f64) -> bool {
        let dlon = (lon - self.lon_deg + 540.0).rem_euclid(360.0) - 180.0;
        match self.shape {
            Shape::Cell { s, w, n, e } => {
                let lon_n = self.lon_deg + dlon;
                lat >= s && lat < n && lon_n >= w && lon_n < e
            }
            Shape::Ellipse { a_km, b_km, az_deg } => {
                let east = dlon * DEG * 6371.0 * (self.lat_deg * DEG).cos();
                let north = (lat - self.lat_deg) * DEG * 6371.0;
                // The azimuth of the semi-major axis, clockwise from north.
                let (sa, ca) = (az_deg * DEG).sin_cos();
                let along = north * ca + east * sa;
                let across = -north * sa + east * ca;
                (along / a_km).powi(2) + (across / b_km).powi(2) <= 1.0
            }
            Shape::Circle { r_km } => {
                let (p1, p2) = (self.lat_deg * DEG, lat * DEG);
                let c = p1.sin() * p2.sin() + p1.cos() * p2.cos() * (dlon * DEG).cos();
                c.clamp(-1.0, 1.0).acos() * 6371.0 <= r_km
            }
        }
    }

    fn active(&self, t: Jd2, coincident: bool) -> bool {
        match (coincident, self.window) {
            (true, Some((a, b))) => t.diff_seconds(a) >= 0.0 && t.diff_seconds(b) <= 0.0,
            (true, None) => true,
            (false, _) => true,
        }
    }
}

/// What the area readers kept and skipped (for the result).
#[derive(Debug, Clone, Default, Serialize)]
pub struct AreaInputs {
    pub maps: Vec<Value>,
    pub emitter_regions: Vec<Value>,
    pub map_cells_degraded: usize,
    pub map_cells_other: usize,
    pub sessions_located: usize,
    pub sessions_not_located: usize,
    pub stated_areas: usize,
}

fn read_map(
    text: &str,
    h: f64,
    out: &mut Vec<Area>,
    info: &mut AreaInputs,
) -> Result<(), KshanaError> {
    // The open engine's reader checks the document (header, date, grid) first.
    let m = kshana::interference_map::route::load_map(text).map_err(|e| bad(e.to_string()))?;
    let v: Value = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
    let day0 = parse_utc(&m.date)?;
    let window = Some((day0, day0.add_seconds(86_400.0)));
    let mut kept = 0usize;
    for f in v["features"].as_array().into_iter().flatten() {
        let p = &f["properties"];
        let (Some(i), Some(j)) = (p["cell_i"].as_i64(), p["cell_j"].as_i64()) else {
            continue;
        };
        let degraded = matches!(p["status"].as_str(), Some("degraded" | "anomalous"));
        if !degraded {
            info.map_cells_other += 1;
            continue;
        }
        let (s, w, n, e) = m.grid.bounds(kshana::interference_map::grid::CellId {
            i: i as i32,
            j: j as i32,
        });
        out.push(Area::new(
            format!("{}-{}-cell-{i}-{j}", m.source_kind, m.date),
            format!("map {} {}", m.source_kind, m.date),
            (s + n) / 2.0,
            (w + e) / 2.0,
            Shape::Cell { s, w, n, e },
            window,
            h,
        ));
        kept += 1;
    }
    info.map_cells_degraded += kept;
    info.maps.push(json!({
        "date": m.date,
        "source_kind": m.source_kind,
        "cell_deg": m.grid.cell_deg,
        "licence": m.licence,
        "attribution": m.attribution,
        "degraded_cells": kept,
        "sha256": hex::encode(Sha256::digest(text.as_bytes())),
    }));
    Ok(())
}

fn read_emitter_region(
    text: &str,
    h: f64,
    out: &mut Vec<Area>,
    info: &mut AreaInputs,
) -> Result<(), KshanaError> {
    let v: Value = serde_json::from_str(text).map_err(|e| bad(format!("emitter region: {e}")))?;
    let m = v
        .get("kshana_emitter_region")
        .ok_or_else(|| bad("not a kshana_emitter_region document"))?;
    let src = m["source_kind"].as_str().unwrap_or("");
    let (mut located, mut not_located) = (0usize, 0usize);
    for s in m["sessions"].as_array().into_iter().flatten() {
        let c = &s["centroid"];
        let lat = s["centroid_lat"].as_f64().or_else(|| c["lat"].as_f64());
        let lon = s["centroid_lon"].as_f64().or_else(|| c["lon"].as_f64());
        let el = if s["uncertainty_ellipse_95"].is_object() {
            &s["uncertainty_ellipse_95"]
        } else {
            &s["ellipse"]
        };
        let a = s["ellipse_semi_major_km"]
            .as_f64()
            .or_else(|| el["semi_major_km"].as_f64());
        let b = s["ellipse_semi_minor_km"]
            .as_f64()
            .or_else(|| el["semi_minor_km"].as_f64());
        let az = s["ellipse_azimuth_deg"]
            .as_f64()
            .or_else(|| el["azimuth_deg"].as_f64())
            .unwrap_or(0.0);
        let (Some(lat), Some(lon), Some(a), Some(b)) = (lat, lon, a, b) else {
            not_located += 1;
            continue;
        };
        let w = match (s["start_utc"].as_str(), s["end_utc"].as_str()) {
            (Some(x), Some(y)) => Some((parse_utc(x)?, parse_utc(y)?)),
            _ => None,
        };
        let id = s["session"].as_i64().unwrap_or(located as i64 + 1);
        out.push(Area::new(
            format!("{src}-session-{id}"),
            format!("emitter region {src}"),
            lat,
            lon,
            Shape::Ellipse {
                a_km: a,
                b_km: b,
                az_deg: az,
            },
            w,
            h,
        ));
        located += 1;
    }
    info.sessions_located += located;
    info.sessions_not_located += not_located;
    info.emitter_regions.push(json!({
        "source_kind": src,
        "first_day": m["first_day"],
        "last_day": m["last_day"],
        "licence": m.pointer("/data/licence"),
        "attribution": m.pointer("/data/attribution"),
        "sessions_located": located,
        "sessions_not_located": not_located,
        "sha256": hex::encode(Sha256::digest(text.as_bytes())),
    }));
    Ok(())
}

/// Every affected area of the scenario.
pub fn areas(s: &SpaceborneRxExposureScenario) -> Result<(Vec<Area>, AreaInputs), KshanaError> {
    let (mut out, mut info) = (Vec::new(), AreaInputs::default());
    let h = s.emitter_height_m;
    for t in &s.map_geojson {
        read_map(t, h, &mut out, &mut info)?;
    }
    for t in &s.emitter_region_json {
        read_emitter_region(t, h, &mut out, &mut info)?;
    }
    for a in &s.area {
        if a.radius_km.is_nan() || a.radius_km <= 0.0 || !(-90.0..=90.0).contains(&a.lat_deg) {
            return Err(bad(format!("area '{}': bad position or radius", a.label)));
        }
        let w = match &a.window_utc {
            Some([x, y]) => Some((parse_utc(x)?, parse_utc(y)?)),
            None => None,
        };
        out.push(Area::new(
            a.label.clone(),
            "stated".into(),
            a.lat_deg,
            a.lon_deg,
            Shape::Circle { r_km: a.radius_km },
            w,
            h,
        ));
        info.stated_areas += 1;
    }
    Ok((out, info))
}

// ── Element sets ─────────────────────────────────────────────────────────────────────────

/// Parse two- or three-line element sets: `(name, line 1, line 2)`.
pub fn parse_tles(text: &str) -> Result<Vec<(String, String, String)>, KshanaError> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (name, l1, l2) = if lines[i].starts_with("1 ") {
            let l2 = lines
                .get(i + 1)
                .ok_or_else(|| bad("element set without line 2"))?;
            i += 2;
            (
                format!("NORAD {}", lines[i - 2][2..7].trim()),
                lines[i - 2],
                *l2,
            )
        } else {
            let l1 = lines
                .get(i + 1)
                .ok_or_else(|| bad("element set without line 1"))?;
            let l2 = lines
                .get(i + 2)
                .ok_or_else(|| bad("element set without line 2"))?;
            i += 3;
            (lines[i - 3].trim().to_string(), *l1, *l2)
        };
        if !l1.starts_with("1 ") || !l2.starts_with("2 ") {
            return Err(bad(format!("'{name}': not a two-line element set")));
        }
        out.push((name, l1.to_string(), l2.to_string()));
    }
    if out.is_empty() {
        return Err(bad("no element set (tle or tle_file)"));
    }
    Ok(out)
}

// ── The run ──────────────────────────────────────────────────────────────────────────────

/// One pass: a maximal run of steps with at least one active affected area in radio view.
#[derive(Debug, Clone, Serialize)]
pub struct Pass {
    pub start_utc: String,
    pub end_utc: String,
    pub duration_s: f64,
    /// Time with the sub-satellite point inside an affected area (s).
    pub over_area_s: f64,
    pub max_elevation_deg: f64,
    pub min_range_km: f64,
    /// Affected areas seen during the pass.
    pub areas_seen: Vec<String>,
    /// Worst modelled effective C/N0 margin over the threshold, per assumed EIRP (dB).
    pub min_margin_db: Vec<f64>,
    /// Time with a modelled margin under zero, per assumed EIRP (s).
    pub below_threshold_s: Vec<f64>,
    /// Time with a modelled margin under the degraded margin, per assumed EIRP (s).
    pub degraded_s: Vec<f64>,
}

/// One spacecraft's result.
#[derive(Debug, Clone, Serialize)]
pub struct Craft {
    pub name: String,
    pub norad_line1: String,
    pub tle_epoch_utc: String,
    /// Days between the element-set epoch and the middle of the span (SGP4 error grows with it).
    pub tle_age_days_at_mid_span: f64,
    pub passes: Vec<Pass>,
    pub exposure_s: f64,
    pub over_area_s: f64,
    pub exposure_share: f64,
    pub min_margin_db: Vec<f64>,
    pub below_threshold_s: Vec<f64>,
    /// Per assumed EIRP: `kept` (margin never under the degraded margin), `degraded` or
    /// `loss-of-tracking-modelled`.
    pub verdict: Vec<String>,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The full study result.
pub fn run_study(s: &SpaceborneRxExposureScenario) -> Result<Value, KshanaError> {
    if !(1.0..=600.0).contains(&s.step_s) {
        return Err(bad("step_s must be in [1, 600]"));
    }
    if s.duration_h.is_nan() || s.duration_h <= 0.0 || s.duration_h > 24.0 * 31.0 {
        return Err(bad("duration_h must be in (0, 744]"));
    }
    if s.assumed_eirp_dbw.is_empty() || s.assumed_eirp_dbw.iter().any(|x| !x.is_finite()) {
        return Err(bad("assumed_eirp_dbw needs at least one finite value"));
    }
    let tles = parse_tles(s.tle.as_deref().unwrap_or(""))?;
    let (areas, inputs) = areas(s)?;
    if areas.is_empty() {
        return Err(bad("no affected area: give maps, emitter regions or areas"));
    }
    let orbits: Vec<SgpOrbit> = tles
        .iter()
        .map(|(_, a, b)| SgpOrbit::from_tle(a, b).map_err(bad))
        .collect::<Result<_, _>>()?;
    let t0 = match &s.start_utc {
        Some(x) => parse_utc(x)?,
        None => orbits[0].set.epoch_utc,
    };
    let span_s = s.duration_h * 3600.0;
    let t1 = t0.add_seconds(span_s);
    let overlaps = areas.iter().any(|a| {
        a.window
            .is_some_and(|(x, y)| x.diff_seconds(t1) <= 0.0 && y.diff_seconds(t0) >= 0.0)
    });
    let coincident = match s.time_mode.as_str() {
        "auto" => overlaps,
        "coincident" => true,
        "climatology" => false,
        o => {
            return Err(bad(format!(
                "time_mode '{o}' (auto|coincident|climatology)"
            )))
        }
    };
    let r = &s.receiver;
    let cn0_nom = nominal_cn0_dbhz(
        r.signal_power_dbw,
        r.gain_toward_gnss_dbi,
        r.system_noise_temp_k,
    );
    let s_rx = r.signal_power_dbw + r.gain_toward_gnss_dbi;
    let sin_mask = (s.min_elevation_deg * DEG).sin();
    let n_e = s.assumed_eirp_dbw.len();
    let n_steps = (span_s / s.step_s).floor() as usize;

    let mut crafts = Vec::new();
    for ((name, l1, _), orb) in tles.iter().zip(&orbits) {
        let mut passes: Vec<Pass> = Vec::new();
        let mut cur: Option<Pass> = None;
        for k in 0..=n_steps {
            let t = t0.add_seconds(k as f64 * s.step_s);
            let (pos, _) = orb
                .itrs_state_with(t, &teme_to_itrs_matrix(t))
                .map_err(|e| bad(format!("{name}: {e}")))?;
            let g = ecef_to_geodetic(pos);
            let (slat, slon) = (g.lat_rad / DEG, g.lon_rad / DEG);
            let mut seen: Vec<usize> = Vec::new();
            let mut max_el = f64::NEG_INFINITY;
            let mut min_rng = f64::INFINITY;
            let mut j_lin = vec![0.0f64; n_e];
            let mut over = false;
            for (ai, a) in areas.iter().enumerate() {
                if !a.active(t, coincident) {
                    continue;
                }
                let los = sub(pos, a.emitter_ecef);
                let d = dot(los, los).sqrt();
                let sin_el = dot(los, a.up) / d;
                if sin_el < sin_mask {
                    continue;
                }
                seen.push(ai);
                max_el = max_el.max(sin_el.clamp(-1.0, 1.0).asin() / DEG);
                min_rng = min_rng.min(d / KM);
                over |= a.contains(slat, slon);
                let path = free_space_path_loss_db(d, L1_HZ);
                for (e, j) in s.assumed_eirp_dbw.iter().zip(j_lin.iter_mut()) {
                    *j += 10f64.powf((e + r.gain_toward_ground_dbi - path) / 10.0);
                }
            }
            if seen.is_empty() {
                if let Some(p) = cur.take() {
                    passes.push(p);
                }
                continue;
            }
            let margins: Vec<f64> = j_lin
                .iter()
                .map(|j| {
                    let js = 10.0 * j.log10() - s_rx;
                    effective_cn0_dbhz(cn0_nom, js, r.q, kshana::jamming::CA_CHIP_RATE_HZ)
                        - r.tracking_threshold_dbhz
                })
                .collect();
            let iso = fmt_utc(t);
            let p = cur.get_or_insert_with(|| Pass {
                start_utc: iso.clone(),
                end_utc: iso.clone(),
                duration_s: 0.0,
                over_area_s: 0.0,
                max_elevation_deg: f64::NEG_INFINITY,
                min_range_km: f64::INFINITY,
                areas_seen: Vec::new(),
                min_margin_db: vec![f64::INFINITY; n_e],
                below_threshold_s: vec![0.0; n_e],
                degraded_s: vec![0.0; n_e],
            });
            // Each step stands for one step of time (the sampled instant and the step after).
            p.end_utc = iso;
            p.duration_s += s.step_s;
            if over {
                p.over_area_s += s.step_s;
            }
            p.max_elevation_deg = p.max_elevation_deg.max(max_el);
            p.min_range_km = p.min_range_km.min(min_rng);
            for ai in seen {
                if !p.areas_seen.contains(&areas[ai].label) {
                    p.areas_seen.push(areas[ai].label.clone());
                }
            }
            for (i, m) in margins.iter().enumerate() {
                p.min_margin_db[i] = p.min_margin_db[i].min(*m);
                if *m < 0.0 {
                    p.below_threshold_s[i] += s.step_s;
                }
                if *m < r.degraded_margin_db {
                    p.degraded_s[i] += s.step_s;
                }
            }
        }
        if let Some(p) = cur.take() {
            passes.push(p);
        }
        let exposure_s: f64 = passes.iter().map(|p| p.duration_s).sum();
        let over_area_s: f64 = passes.iter().map(|p| p.over_area_s).sum();
        let min_margin_db: Vec<f64> = (0..n_e)
            .map(|i| {
                passes
                    .iter()
                    .map(|p| p.min_margin_db[i])
                    .fold(f64::INFINITY, f64::min)
            })
            .collect();
        let below_threshold_s: Vec<f64> = (0..n_e)
            .map(|i| passes.iter().map(|p| p.below_threshold_s[i]).sum())
            .collect();
        let verdict = min_margin_db
            .iter()
            .map(|m| {
                if !m.is_finite() || *m >= r.degraded_margin_db {
                    "kept"
                } else if *m >= 0.0 {
                    "degraded"
                } else {
                    "loss-of-tracking-modelled"
                }
                .to_string()
            })
            .collect();
        let mid = t0.add_seconds(span_s / 2.0);
        crafts.push(Craft {
            name: name.clone(),
            norad_line1: l1.clone(),
            tle_epoch_utc: fmt_utc(orb.set.epoch_utc),
            tle_age_days_at_mid_span: mid.diff_seconds(orb.set.epoch_utc) / 86_400.0,
            passes,
            exposure_s,
            over_area_s,
            exposure_share: exposure_s / ((n_steps + 1) as f64 * s.step_s),
            min_margin_db: min_margin_db
                .into_iter()
                .map(|m| if m.is_finite() { m } else { f64::NAN })
                .collect(),
            below_threshold_s,
            verdict,
        });
    }
    let unswept = if coincident {
        "coincident: each area counts only inside its own time window (a map day, a session)"
    } else {
        "climatology: the areas' time windows do not overlap the span, so every area counts all the time; the result says what the orbit would see if the mapped areas were active, not what happened"
    };
    let mut doc = json!({
        "schema": SCHEMA,
        "kind": KIND,
        "title": s.title.clone().unwrap_or_else(|| "Spaceborne GNSS receiver interference exposure".into()),
        "label": LABEL,
        "evidence_class": "MODELLED",
        "span": {
            "start_utc": fmt_utc(t0),
            "end_utc": fmt_utc(t1),
            "step_s": s.step_s,
            "time_mode": if coincident { "coincident" } else { "climatology" },
            "time_mode_note": unswept,
        },
        "assumptions": {
            "assumed_eirp_dbw": s.assumed_eirp_dbw,
            "emitter_point": "the centre of each affected area, at emitter_height_m; the maps do not locate an emitter",
            "emitter_height_m": s.emitter_height_m,
            "min_elevation_deg": s.min_elevation_deg,
            "propagation": "free space at L1, no atmospheric or body losses, no terrain",
            "receiver": r,
            "nominal_cn0_dbhz": cn0_nom,
            "orbit": "SGP4 from the element sets (open engine), TEME to ITRS; accuracy falls with element-set age",
        },
        "inputs": inputs,
        "affected_areas": areas.len(),
        "craft": crafts,
    });
    // NaN is not JSON: a margin with no exposure is null.
    sanitise(&mut doc);
    let digest = hex::encode(Sha256::digest(
        serde_json::to_vec(&doc).map_err(|e| bad(e.to_string()))?,
    ));
    doc["result_sha256"] = json!(digest);
    Ok(doc)
}

/// NaN and infinities become null; every other non-integer number is rounded to
/// [`SIG_DIGITS`] significant digits, so the document and its hash are the same on every
/// platform (SGP4 and the trigonometry differ in the last bits between maths libraries).
fn sanitise(v: &mut Value) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(sanitise),
        Value::Object(o) => o.values_mut().for_each(sanitise),
        Value::Number(n) if n.is_f64() => match n.as_f64() {
            Some(x) if x.is_finite() => *v = json!(round_sig(x)),
            _ => *v = Value::Null,
        },
        _ => {}
    }
}

/// Significant digits kept in the published study.
const SIG_DIGITS: usize = 9;

fn round_sig(x: f64) -> f64 {
    if x == 0.0 {
        return 0.0;
    }
    format!("{:.*e}", SIG_DIGITS - 1, x).parse().unwrap_or(x)
}

/// One line per spacecraft.
pub fn summary(doc: &Value) -> String {
    let eirp: Vec<String> = doc["assumptions"]["assumed_eirp_dbw"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| format!("{}", x.as_f64().unwrap_or(0.0)))
        .collect();
    let mut out = format!(
        "MODELLED spaceborne interference exposure ({}, {} affected areas; EIRP assumed {} dBW):",
        doc["span"]["time_mode"].as_str().unwrap_or(""),
        doc["affected_areas"],
        eirp.join("/")
    );
    for c in doc["craft"].as_array().into_iter().flatten() {
        let v: Vec<&str> = c["verdict"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        out.push_str(&format!(
            " {}: {} passes, {:.0} min exposed, {:.0} min over areas, verdict {};",
            c["name"].as_str().unwrap_or(""),
            c["passes"].as_array().map_or(0, Vec::len),
            c["exposure_s"].as_f64().unwrap_or(0.0) / 60.0,
            c["over_area_s"].as_f64().unwrap_or(0.0) / 60.0,
            v.join("/")
        ));
    }
    out
}

/// Per-pass CSV.
pub fn passes_csv(doc: &Value) -> String {
    let eirp: Vec<f64> = doc["assumptions"]["assumed_eirp_dbw"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_f64)
        .collect();
    let mut s = String::from(
        "# MODELLED (see label in study.json)\ncraft,start_utc,end_utc,duration_s,over_area_s,max_elevation_deg,min_range_km,areas_seen",
    );
    for e in &eirp {
        s.push_str(&format!(",min_margin_db_eirp{e},below_threshold_s_eirp{e}"));
    }
    s.push('\n');
    let f = |v: &Value| v.as_f64().map_or("".into(), |x| format!("{x:.2}"));
    for c in doc["craft"].as_array().into_iter().flatten() {
        for p in c["passes"].as_array().into_iter().flatten() {
            s.push_str(&format!(
                "{},{},{},{},{},{},{},{}",
                c["name"].as_str().unwrap_or("").replace(',', " "),
                p["start_utc"].as_str().unwrap_or(""),
                p["end_utc"].as_str().unwrap_or(""),
                f(&p["duration_s"]),
                f(&p["over_area_s"]),
                f(&p["max_elevation_deg"]),
                f(&p["min_range_km"]),
                p["areas_seen"].as_array().map_or(0, Vec::len)
            ));
            for i in 0..eirp.len() {
                s.push_str(&format!(
                    ",{},{}",
                    f(&p["min_margin_db"][i]),
                    f(&p["below_threshold_s"][i])
                ));
            }
            s.push('\n');
        }
    }
    s
}

/// The study report (Markdown).
pub fn report_md(doc: &Value) -> String {
    let mut s = format!(
        "# {}\n\n**Evidence class: MODELLED.** {}\n\nSpan {} to {}, step {} s, time mode **{}**: {}\n\n",
        doc["title"].as_str().unwrap_or(""),
        LABEL,
        doc["span"]["start_utc"].as_str().unwrap_or(""),
        doc["span"]["end_utc"].as_str().unwrap_or(""),
        doc["span"]["step_s"],
        doc["span"]["time_mode"].as_str().unwrap_or(""),
        doc["span"]["time_mode_note"].as_str().unwrap_or("")
    );
    s.push_str("## Inputs\n\n");
    for m in doc["inputs"]["maps"].as_array().into_iter().flatten() {
        s.push_str(&format!(
            "- Interference map {} {} ({}; {}): {} degraded cells, SHA-256 {}\n",
            m["source_kind"].as_str().unwrap_or(""),
            m["date"].as_str().unwrap_or(""),
            m["licence"].as_str().unwrap_or(""),
            m["attribution"].as_str().unwrap_or(""),
            m["degraded_cells"],
            m["sha256"].as_str().unwrap_or("")
        ));
    }
    for m in doc["inputs"]["emitter_regions"]
        .as_array()
        .into_iter()
        .flatten()
    {
        s.push_str(&format!(
            "- Emitter-region summary {} {} to {} ({}; {}): {} located sessions used, {} not located (not used), SHA-256 {}\n",
            m["source_kind"].as_str().unwrap_or(""),
            m["first_day"].as_str().unwrap_or(""),
            m["last_day"].as_str().unwrap_or(""),
            m["licence"].as_str().unwrap_or(""),
            m["attribution"].as_str().unwrap_or(""),
            m["sessions_located"],
            m["sessions_not_located"],
            m["sha256"].as_str().unwrap_or("")
        ));
    }
    let n = doc["inputs"]["stated_areas"].as_u64().unwrap_or(0);
    if n > 0 {
        s.push_str(&format!("- {n} areas stated by the user\n"));
    }
    let eirp: Vec<String> = doc["assumptions"]["assumed_eirp_dbw"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| format!("{}", x.as_f64().unwrap_or(0.0)))
        .collect();
    s.push_str(&format!(
        "\nAssumed interferer EIRP per area: {} dBW. Receiver: {}. Nominal C/N0 {:.1} dB-Hz.\n\n## Results per spacecraft\n\n",
        eirp.join(", "),
        doc["assumptions"]["receiver"],
        doc["assumptions"]["nominal_cn0_dbhz"].as_f64().unwrap_or(0.0)
    ));
    s.push_str("| Spacecraft | TLE epoch | TLE age (d) | Passes | Exposed (min) | Over areas (min) | Share | Worst margin (dB) per EIRP | Verdict per EIRP |\n|---|---|---|---|---|---|---|---|---|\n");
    for c in doc["craft"].as_array().into_iter().flatten() {
        let m: Vec<String> = c["min_margin_db"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| x.as_f64().map_or("-".into(), |v| format!("{v:.1}")))
            .collect();
        let v: Vec<&str> = c["verdict"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        s.push_str(&format!(
            "| {} | {} | {:.1} | {} | {:.1} | {:.1} | {:.3} | {} | {} |\n",
            c["name"].as_str().unwrap_or(""),
            c["tle_epoch_utc"].as_str().unwrap_or(""),
            c["tle_age_days_at_mid_span"].as_f64().unwrap_or(0.0),
            c["passes"].as_array().map_or(0, Vec::len),
            c["exposure_s"].as_f64().unwrap_or(0.0) / 60.0,
            c["over_area_s"].as_f64().unwrap_or(0.0) / 60.0,
            c["exposure_share"].as_f64().unwrap_or(0.0),
            m.join(" / "),
            v.join(" / ")
        ));
    }
    s.push_str("\nPer-pass detail: `passes.csv`. Full result: `study.json` (result SHA-256 ");
    s.push_str(doc["result_sha256"].as_str().unwrap_or(""));
    s.push_str(").\n\n## What this does and does not say\n\n- It says how long the orbit spends with mapped affected areas in radio view and, for each assumed interferer power, what the anti-jam equation gives for the receiver's margin.\n- It does not say that any emitter was on air when the spacecraft passed (unless the time mode is coincident, and then only as the map says), how strong it was, or how a real receiver behaved. No on-board measurement is used.\n- A map cell or session marks where aircraft or ships reported degraded navigation; its centre is used as the emitter point, an assumption.\n");
    s
}

impl Scenario for SpaceborneRxExposureScenario {
    fn run(&self) -> Result<RunOutput, KshanaError> {
        let mut s = self.clone();
        s.resolve_files(Path::new("."))?;
        let doc = run_study(&s)?;
        Ok(RunOutput {
            json: serde_json::to_string_pretty(&doc).map_err(|e| bad(e.to_string()))?,
            svg: String::new(),
            summary: summary(&doc),
            csv: Some(passes_csv(&doc)),
        })
    }
}

impl ExternalPack for SpaceborneRxExposureScenario {
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

/// Write the study directory (`study.json`, `passes.csv`, `report.md`) and, with a seed,
/// sign it as a whole with Kshana Pro's dossier signature (`signature.json`).
pub fn write_study(
    doc: &Value,
    dir: &Path,
    signer: Option<(&[u8; 32], &str, &str)>,
) -> Result<Option<std::path::PathBuf>, KshanaError> {
    std::fs::create_dir_all(dir).map_err(|e| bad(format!("{}: {e}", dir.display())))?;
    let w = |name: &str, text: String| {
        std::fs::write(dir.join(name), text).map_err(|e| bad(format!("{name}: {e}")))
    };
    w(
        "study.json",
        serde_json::to_string_pretty(doc).map_err(|e| bad(e.to_string()))? + "\n",
    )?;
    w("passes.csv", passes_csv(doc))?;
    w("report.md", report_md(doc))?;
    match signer {
        Some((seed, name, utc)) => Ok(Some(kshana_pro::signed_dossier::sign(
            dir, seed, name, utc,
        )?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_hides_last_bit_differences() {
        let x = 1_234.567_890_123_456_f64;
        assert_eq!(round_sig(x), round_sig(f64::from_bits(x.to_bits() + 1)));
        assert_eq!(round_sig(x), 1_234.567_89);
        assert_eq!(round_sig(0.0), 0.0);
        let mut v = json!({"a": [f64::NAN, 2.0, 3], "b": -0.000_123_456_789_01});
        sanitise(&mut v);
        assert_eq!(v, json!({"a": [null, 2.0, 3], "b": -0.000_123_456_789}));
    }

    #[test]
    fn time_round_trip() {
        let t = parse_utc("2025-09-15T09:52Z").unwrap();
        assert_eq!(fmt_utc(t), "2025-09-15T09:52:00Z");
        assert_eq!(
            fmt_utc(parse_utc("2026-11-15").unwrap()),
            "2026-11-15T00:00:00Z"
        );
        assert!(parse_utc("15/09/2025").is_err());
    }

    #[test]
    fn shapes() {
        let c = Area::new(
            "c".into(),
            "t".into(),
            60.0,
            25.0,
            Shape::Circle { r_km: 100.0 },
            None,
            0.0,
        );
        assert!(c.contains(60.5, 25.0));
        assert!(!c.contains(61.0, 25.0));
        let e = Area::new(
            "e".into(),
            "t".into(),
            60.0,
            25.0,
            Shape::Ellipse {
                a_km: 300.0,
                b_km: 100.0,
                az_deg: 0.0,
            },
            None,
            0.0,
        );
        assert!(e.contains(62.5, 25.0)); // 278 km north, along the major axis
        assert!(!e.contains(60.0, 27.0)); // 111 km east, across it
    }
}
