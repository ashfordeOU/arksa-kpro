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
//! v2 (AK4, optional sections): maps of ground interference regions
//! (`arksa_interference_regions`; regions, never emitter positions), `[forecast]` (a receiver
//! link-margin model with an antenna pattern, predicted outage windows per pass and a
//! per-revolution exposure forecast over the areas' activity) and `[export]` (matching receiver
//! test scenarios in Kshana Pro's `kshana-testkit-scenario/1` format). The entitled entry
//! points ([`run_study_entitled`], [`run_study_with_scenarios`]) need the Arksa claim.
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

/// The receiver link-margin model of the forecast (all MODELLED; defaults stated in the
/// result). It replaces the constant `receiver.gain_toward_ground_dbi` with a gain pattern
/// over the angle from the antenna's boresight and adds the losses between an interferer and
/// the receiver; the signal side, noise, threshold and Q stay those of `[receiver]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkModel {
    /// Receive gain (dBi) against the angle from boresight (deg, 0 to 180), linear between
    /// points. The boresight is the geocentric zenith (a zenith-pointing antenna; no
    /// attitude). The default is an illustrative zenith patch with about 25 dB front to back,
    /// not any product's measured pattern.
    #[serde(default = "LinkModel::d_pattern")]
    pub antenna_pattern: Vec<[f64; 2]>,
    /// Polarisation mismatch toward the interferer (dB): 3 dB for a linearly polarised
    /// interferer on a right-hand circular antenna (an assumption).
    #[serde(default = "LinkModel::d_pol")]
    pub polarisation_loss_db: f64,
    /// Other losses on the interferer path (atmosphere, cable, filter) (dB).
    #[serde(default)]
    pub other_losses_db: f64,
}

impl LinkModel {
    fn d_pattern() -> Vec<[f64; 2]> {
        vec![
            [0.0, 3.0],
            [30.0, 2.0],
            [60.0, -1.0],
            [80.0, -4.0],
            [90.0, -7.0],
            [100.0, -10.0],
            [120.0, -15.0],
            [150.0, -20.0],
            [180.0, -22.0],
        ]
    }
    fn d_pol() -> f64 {
        3.0
    }

    /// The gain (dBi) at `deg` from boresight.
    pub fn gain_dbi(&self, deg: f64) -> f64 {
        let p = &self.antenna_pattern;
        if deg <= p[0][0] {
            return p[0][1];
        }
        for w in p.windows(2) {
            if deg <= w[1][0] {
                let f = (deg - w[0][0]) / (w[1][0] - w[0][0]);
                return w[0][1] + f * (w[1][1] - w[0][1]);
            }
        }
        p[p.len() - 1][1]
    }

    fn check(&self) -> Result<(), KshanaError> {
        let p = &self.antenna_pattern;
        if p.len() < 2
            || p.iter().any(|x| !x[0].is_finite() || !x[1].is_finite())
            || p.windows(2).any(|w| w[1][0] <= w[0][0])
            || p[0][0] < 0.0
            || p[p.len() - 1][0] > 180.0
        {
            return Err(bad(
                "forecast.link.antenna_pattern: at least two [deg, dBi] points, angles increasing within [0, 180]",
            ));
        }
        if !self.polarisation_loss_db.is_finite() || !self.other_losses_db.is_finite() {
            return Err(bad("forecast.link: losses must be finite"));
        }
        Ok(())
    }
}

impl Default for LinkModel {
    fn default() -> Self {
        toml::from_str("").expect("defaults")
    }
}

fn d_reacq() -> f64 {
    20.0
}
fn d_mc() -> usize {
    1024
}

/// The per-orbit forecast settings (`[forecast]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forecast {
    /// Time after the margin comes back above zero before tracking is predicted back (s):
    /// an assumption for reacquisition, added to the end of every outage window.
    #[serde(default = "d_reacq")]
    pub reacquisition_s: f64,
    /// Samples of the activity draw when an orbit sees more uncertain areas than the exact
    /// enumeration takes ([`EXACT_MAX_UNCERTAIN`]).
    #[serde(default = "d_mc")]
    pub monte_carlo_samples: usize,
    #[serde(default)]
    pub link: LinkModel,
}

fn d_include() -> String {
    "outage".into()
}
fn d_lead() -> f64 {
    120.0
}
fn d_tail() -> f64 {
    300.0
}
fn d_sample_step() -> f64 {
    5.0
}
fn d_js_step() -> f64 {
    3.0
}
fn d_js_floor() -> f64 {
    10.0
}
fn d_bw() -> f64 {
    2.046e6
}
fn d_detect() -> f64 {
    10.0
}
fn d_recover() -> f64 {
    120.0
}
fn d_avail() -> f64 {
    0.95
}
fn d_mis_h() -> f64 {
    100.0
}
fn d_rec_h() -> f64 {
    50.0
}
fn d_max_sc() -> usize {
    50
}
fn d_interp() -> f64 {
    10.0
}

/// Export of matching receiver test scenarios (`[export]`), in Kshana Pro's neutral
/// test-as-code format (`kshana-testkit-scenario/1`): one scenario per selected pass, with
/// the spacecraft's propagated motion as time-tagged samples, the modelled J/S profile of
/// the pass as signal-parameter jamming events, and pass/fail criteria stated before any run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportSpec {
    /// The assumed EIRP (dBW) whose J/S profile is exported; one of `assumed_eirp_dbw`
    /// (default the largest).
    #[serde(default)]
    pub eirp_dbw: Option<f64>,
    /// Which passes: `outage` (a modelled loss of tracking), `degraded` (margin under the
    /// degraded margin, outage included) or `all`.
    #[serde(default = "d_include")]
    pub include: String,
    /// Baseline before the first in-view step (s).
    #[serde(default = "d_lead")]
    pub lead_s: f64,
    /// Time after the pass for recovery (s).
    #[serde(default = "d_tail")]
    pub tail_s: f64,
    /// Spacing of the trajectory samples (s).
    #[serde(default = "d_sample_step")]
    pub sample_step_s: f64,
    /// J/S quantisation of the exported profile (dB); one jamming event per run of equal value.
    #[serde(default = "d_js_step")]
    pub js_step_db: f64,
    /// Runs whose quantised J/S is under this are left out (dB).
    #[serde(default = "d_js_floor")]
    pub js_floor_db: f64,
    /// Bandwidth of the exported wideband jamming (Hz) at L1.
    #[serde(default = "d_bw")]
    pub bandwidth_hz: f64,
    /// Detection bound for a pass with a modelled outage (s).
    #[serde(default = "d_detect")]
    pub detect_bound_s: f64,
    /// Recovery bound after the interference ends (s).
    #[serde(default = "d_recover")]
    pub recovery_bound_s: f64,
    /// Availability bound for a pass with a degraded but kept margin.
    #[serde(default = "d_avail")]
    pub availability_bound: f64,
    /// A valid, unflagged fix farther than this from the truth is misleading (m).
    #[serde(default = "d_mis_h")]
    pub misleading_h_error_m: f64,
    /// Within this of the truth counts as recovered (m).
    #[serde(default = "d_rec_h")]
    pub recover_h_error_m: f64,
    /// At most this many scenarios (the passes with the worst margin first).
    #[serde(default = "d_max_sc")]
    pub max_scenarios: usize,
    /// Samples are bisected (down to 50 ms) until linear interpolation between them is within
    /// this of the propagated sub-satellite point (m); the largest left is stated.
    #[serde(default = "d_interp")]
    pub max_interpolation_error_m: f64,
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
    /// Probability (0 to 1) that the area is active during an orbit, used by the forecast
    /// (`[forecast]`); default 1 (always active).
    #[serde(default)]
    pub activity: Option<f64>,
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
    /// Maps of ground interference regions (`arksa_interference_regions` GeoJSON, version 1),
    /// JSON text. A region is an area on the ground, never an emitter position.
    #[serde(default)]
    pub region_json: Vec<String>,
    #[serde(default)]
    pub region_files: Vec<String>,
    #[serde(default)]
    pub receiver: Receiver,
    /// The per-orbit exposure forecast with the receiver link-margin model and the predicted
    /// outage windows (v2). Absent: the F4 study as before.
    #[serde(default)]
    pub forecast: Option<Forecast>,
    /// Export matching receiver test scenarios in Kshana Pro's neutral test-as-code format.
    #[serde(default)]
    pub export: Option<ExportSpec>,
}

impl SpaceborneRxExposureScenario {
    pub const KIND: &'static str = KIND;

    pub fn meta_static() -> ScenarioMeta {
        ScenarioMeta {
            name: Self::KIND,
            description: "Spaceborne GNSS receiver interference assurance: exposure of a spacecraft's receiver to ground interference along its orbit, from public interference maps and TLEs: per-pass exposure, time over affected areas and a modelled C/N0 margin for assumed interferer powers; optionally a per-revolution forecast with a link-margin model and predicted outage windows, and matching receiver test scenarios. MODELLED.",
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
                "region_json",
                "region_files",
                "receiver",
                "forecast",
                "export",
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
        for f in std::mem::take(&mut self.region_files) {
            self.region_json.push(read(&f)?);
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
    Cell {
        s: f64,
        w: f64,
        n: f64,
        e: f64,
    },
    Ellipse {
        a_km: f64,
        b_km: f64,
        az_deg: f64,
    },
    Circle {
        r_km: f64,
    },
    /// An outer ring of (lat, lon) vertices (deg); holes are not used.
    Polygon {
        ring: Vec<(f64, f64)>,
    },
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
    /// Probability that the area is active during an orbit (forecast only; 1 = always).
    pub activity: f64,
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
            activity: 1.0,
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
            Shape::Polygon { ref ring } => {
                // Even-odd rule in (lat, lon) with longitudes taken relative to the centre.
                let rel = |lo: f64| (lo - self.lon_deg + 540.0).rem_euclid(360.0) - 180.0;
                let (y, x) = (lat, dlon);
                let mut inside = false;
                let n = ring.len();
                for i in 0..n {
                    let (yi, xi) = (ring[i].0, rel(ring[i].1));
                    let (yj, xj) = (ring[(i + n - 1) % n].0, rel(ring[(i + n - 1) % n].1));
                    if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                        inside = !inside;
                    }
                }
                inside
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub region_maps: Vec<Value>,
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

/// The tag of a map of ground interference regions.
pub const REGIONS_TAG: &str = "arksa_interference_regions";

/// Read a map of ground interference regions: a GeoJSON FeatureCollection with an
/// `arksa_interference_regions` header (`version` 1, `title`, `source`, `licence`, optional
/// `valid_from_utc`/`valid_to_utc`) and one feature per region, a `Polygon` (outer ring) or a
/// `Point` with `radius_km`; properties `id`, optional `label`, `activity` (0 to 1) and
/// `window_utc`. A region says where interference is expected on the ground; its interferer
/// point is the region's reference point (the vertex mean or the circle's centre), an
/// assumption, since the map locates no emitter.
fn read_regions(
    text: &str,
    h: f64,
    out: &mut Vec<Area>,
    info: &mut AreaInputs,
) -> Result<(), KshanaError> {
    let v: Value = serde_json::from_str(text).map_err(|e| bad(format!("regions: {e}")))?;
    let m = v
        .get(REGIONS_TAG)
        .ok_or_else(|| bad(format!("not a {REGIONS_TAG} document")))?;
    if m["version"].as_i64() != Some(1) {
        return Err(bad(format!("{REGIONS_TAG}: version 1 expected")));
    }
    let title = m["title"].as_str().unwrap_or("").to_string();
    let map_window = match (m["valid_from_utc"].as_str(), m["valid_to_utc"].as_str()) {
        (Some(a), Some(b)) => Some((parse_utc(a)?, parse_utc(b)?)),
        _ => None,
    };
    let mut kept = 0usize;
    for (n, f) in v["features"].as_array().into_iter().flatten().enumerate() {
        let p = &f["properties"];
        let id = p["id"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("region-{}", n + 1));
        let label = p["label"]
            .as_str()
            .map(str::to_string)
            .unwrap_or(id.clone());
        let activity = p["activity"].as_f64().unwrap_or(1.0);
        if !(0.0..=1.0).contains(&activity) {
            return Err(bad(format!("region '{id}': activity must be in [0, 1]")));
        }
        let window = match p["window_utc"].as_array().map(Vec::as_slice) {
            Some([a, b]) => Some((
                parse_utc(a.as_str().unwrap_or(""))?,
                parse_utc(b.as_str().unwrap_or(""))?,
            )),
            Some(_) => return Err(bad(format!("region '{id}': window_utc is [start, end]"))),
            None => map_window,
        };
        let g = &f["geometry"];
        let pt = |c: &Value| -> Option<(f64, f64)> { Some((c[1].as_f64()?, c[0].as_f64()?)) };
        let (lat, lon, shape) = match g["type"].as_str() {
            Some("Point") => {
                let (lat, lon) =
                    pt(&g["coordinates"]).ok_or_else(|| bad(format!("region '{id}': point")))?;
                let r = p["radius_km"]
                    .as_f64()
                    .filter(|r| *r > 0.0)
                    .ok_or_else(|| bad(format!("region '{id}': a Point needs radius_km > 0")))?;
                (lat, lon, Shape::Circle { r_km: r })
            }
            Some("Polygon") => {
                let ring: Vec<(f64, f64)> = g["coordinates"][0]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(pt)
                    .collect::<Option<_>>()
                    .ok_or_else(|| bad(format!("region '{id}': polygon coordinates")))?;
                let mut ring = ring;
                if ring.len() > 1 && ring.first() == ring.last() {
                    ring.pop();
                }
                if ring.len() < 3 {
                    return Err(bad(format!("region '{id}': a polygon needs 3 vertices")));
                }
                let lat = ring.iter().map(|x| x.0).sum::<f64>() / ring.len() as f64;
                // Vertex mean of the longitudes, taken relative to the first vertex.
                let l0 = ring[0].1;
                let dl = ring
                    .iter()
                    .map(|x| (x.1 - l0 + 540.0).rem_euclid(360.0) - 180.0)
                    .sum::<f64>()
                    / ring.len() as f64;
                let lon = (l0 + dl + 540.0).rem_euclid(360.0) - 180.0;
                (lat, lon, Shape::Polygon { ring })
            }
            o => {
                return Err(bad(format!(
                    "region '{id}': geometry {o:?} (Polygon or Point)"
                )))
            }
        };
        if !(-90.0..=90.0).contains(&lat) || !lon.is_finite() {
            return Err(bad(format!("region '{id}': bad position")));
        }
        let mut a = Area::new(
            label,
            format!("regions {title}"),
            lat,
            lon,
            shape,
            window,
            h,
        );
        a.activity = activity;
        out.push(a);
        kept += 1;
    }
    if kept == 0 {
        return Err(bad(format!("{REGIONS_TAG}: no region")));
    }
    info.region_maps.push(json!({
        "title": title,
        "source": m["source"],
        "licence": m["licence"],
        "evidence": m["evidence"],
        "regions": kept,
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
    for t in &s.region_json {
        read_regions(t, h, &mut out, &mut info)?;
    }
    for a in &s.area {
        if a.radius_km.is_nan() || a.radius_km <= 0.0 || !(-90.0..=90.0).contains(&a.lat_deg) {
            return Err(bad(format!("area '{}': bad position or radius", a.label)));
        }
        let w = match &a.window_utc {
            Some([x, y]) => Some((parse_utc(x)?, parse_utc(y)?)),
            None => None,
        };
        let mut area = Area::new(
            a.label.clone(),
            "stated".into(),
            a.lat_deg,
            a.lon_deg,
            Shape::Circle { r_km: a.radius_km },
            w,
            h,
        );
        if let Some(p) = a.activity {
            if !(0.0..=1.0).contains(&p) {
                return Err(bad(format!(
                    "area '{}': activity must be in [0, 1]",
                    a.label
                )));
            }
            area.activity = p;
        }
        out.push(area);
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
    /// Forecast only: the predicted outage windows of the pass, per assumed EIRP.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outage_windows: Option<Vec<Vec<OutageWindow>>>,
    /// Forecast only: the link budget at the pass's worst step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_budget: Option<Value>,
}

/// A predicted outage window: from the first step with a modelled margin under zero to the
/// last one, plus the assumed reacquisition time; runs closer than that are merged.
#[derive(Debug, Clone, Serialize)]
pub struct OutageWindow {
    pub start_utc: String,
    /// End of the modelled loss of tracking (before reacquisition).
    pub loss_end_utc: String,
    /// End of the window (loss end plus the reacquisition time).
    pub end_utc: String,
    /// Time with a margin under zero inside the window (s).
    pub loss_s: f64,
    /// Length of the window (s).
    pub outage_s: f64,
    pub worst_margin_db: f64,
    /// Areas in radio view at the worst step.
    pub areas: Vec<String>,
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

/// The orbits with more uncertain areas (activity under 1) in view than this use a seeded
/// Monte Carlo draw of the activity instead of the exact enumeration.
pub const EXACT_MAX_UNCERTAIN: usize = 10;

/// One area in radio view at one step: the receive gain less the interferer-path losses
/// (dB), the free-space loss (dB), the range and the angle from the antenna boresight.
#[derive(Debug, Clone)]
struct Term {
    ai: usize,
    g_db: f64,
    path_db: f64,
    range_km: f64,
    off_deg: f64,
}

/// One step with at least one area in view.
#[derive(Debug, Clone)]
struct StepRec {
    k: usize,
    rev: usize,
    terms: Vec<Term>,
    over: bool,
}

/// The margin model: effective C/N0 over the threshold for a received interference power.
struct Margin<'a> {
    cn0_nom: f64,
    s_rx: f64,
    r: &'a Receiver,
}

impl Margin<'_> {
    /// The summed interference power (W) of the included terms for EIRP `e` (dBW).
    fn j_lin(e: f64, terms: &[Term], include: &dyn Fn(usize) -> bool) -> f64 {
        let mut j = 0.0f64;
        for t in terms {
            if include(t.ai) {
                j += 10f64.powf((e + t.g_db - t.path_db) / 10.0);
            }
        }
        j
    }
    /// The margin (dB) for a summed interference power; infinite with none.
    fn of(&self, j: f64) -> f64 {
        if j <= 0.0 {
            return f64::INFINITY;
        }
        let js = 10.0 * j.log10() - self.s_rx;
        effective_cn0_dbhz(self.cn0_nom, js, self.r.q, kshana::jamming::CA_CHIP_RATE_HZ)
            - self.r.tracking_threshold_dbhz
    }
}

/// A run of outage steps (indices into the span), merged across gaps shorter than the
/// reacquisition time.
#[derive(Debug, Clone)]
struct Win {
    k0: usize,
    k1: usize,
    loss_steps: usize,
    worst: f64,
    worst_k: usize,
}

impl Win {
    fn outage_s(&self, step_s: f64, reacq_s: f64) -> f64 {
        (self.k1 + 1 - self.k0) as f64 * step_s + reacq_s
    }
}

/// The outage windows of a sequence `(step index, margin)` sorted by step.
fn outage_windows(seq: &[(usize, f64)], step_s: f64, reacq_s: f64) -> Vec<Win> {
    let mut out: Vec<Win> = Vec::new();
    for &(k, m) in seq {
        if m >= 0.0 {
            continue;
        }
        if let Some(w) = out.last_mut() {
            let end = (w.k1 + 1) as f64 * step_s + reacq_s;
            if k as f64 * step_s <= end + 1e-9 {
                w.k1 = k;
                w.loss_steps += 1;
                if m < w.worst {
                    w.worst = m;
                    w.worst_k = k;
                }
                continue;
            }
        }
        out.push(Win {
            k0: k,
            k1: k,
            loss_steps: 1,
            worst: m,
            worst_k: k,
        });
    }
    out
}

/// The full study result.
pub fn run_study(s: &SpaceborneRxExposureScenario) -> Result<Value, KshanaError> {
    Ok(study(s)?.0)
}

/// [`run_study`] behind the Arksa entitlement ([`crate::entitlement`]).
pub fn run_study_entitled(
    s: &SpaceborneRxExposureScenario,
    ent: &kshana_pro::license::Entitlement,
    now_unix: i64,
) -> Result<Value, KshanaError> {
    crate::entitlement::ensure_rx_exposure_entitled(ent, now_unix)?;
    run_study(s)
}

/// One exported receiver test scenario (`kshana-testkit-scenario/1`).
#[derive(Debug, Clone)]
pub struct ExportedScenario {
    pub id: String,
    /// The path inside the study directory (`scenarios/<id>.toml`).
    pub file: String,
    /// The scenario file text.
    pub toml: String,
    /// The scenario's identity (Kshana Pro's spec SHA-256, carried by every verdict on it).
    pub sha256: String,
}

/// The study and the matching test scenarios (empty without `[export]`), behind the Arksa
/// entitlement. The study lists each scenario's id and SHA-256 under `test_scenarios`.
pub fn run_study_with_scenarios(
    s: &SpaceborneRxExposureScenario,
    ent: &kshana_pro::license::Entitlement,
    now_unix: i64,
) -> Result<(Value, Vec<ExportedScenario>), KshanaError> {
    crate::entitlement::ensure_rx_exposure_entitled(ent, now_unix)?;
    study(s)
}

/// Per pass, what the forecast and the export need beyond [`Pass`].
struct PassSteps {
    steps: Vec<StepRec>,
}

fn study(s: &SpaceborneRxExposureScenario) -> Result<(Value, Vec<ExportedScenario>), KshanaError> {
    if !(1.0..=600.0).contains(&s.step_s) {
        return Err(bad("step_s must be in [1, 600]"));
    }
    if s.duration_h.is_nan() || s.duration_h <= 0.0 || s.duration_h > 24.0 * 31.0 {
        return Err(bad("duration_h must be in (0, 744]"));
    }
    if s.assumed_eirp_dbw.is_empty() || s.assumed_eirp_dbw.iter().any(|x| !x.is_finite()) {
        return Err(bad("assumed_eirp_dbw needs at least one finite value"));
    }
    let fc = s.forecast.as_ref();
    if let Some(f) = fc {
        f.link.check()?;
        if !(f.reacquisition_s.is_finite() && (0.0..=3600.0).contains(&f.reacquisition_s)) {
            return Err(bad("forecast.reacquisition_s must be in [0, 3600]"));
        }
        if !(16..=100_000).contains(&f.monte_carlo_samples) {
            return Err(bad("forecast.monte_carlo_samples must be in [16, 100000]"));
        }
    }
    let export_e = match &s.export {
        Some(x) => Some(check_export(x, &s.assumed_eirp_dbw)?),
        None => None,
    };
    let tles = parse_tles(s.tle.as_deref().unwrap_or(""))?;
    let (areas, inputs) = areas(s)?;
    if areas.is_empty() {
        return Err(bad(
            "no affected area: give maps, emitter regions, region maps or areas",
        ));
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
    let mm = Margin { cn0_nom, s_rx, r };
    let sin_mask = (s.min_elevation_deg * DEG).sin();
    let n_e = s.assumed_eirp_dbw.len();
    let n_steps = (span_s / s.step_s).floor() as usize;
    let reacq_s = fc.map_or(d_reacq(), |f| f.reacquisition_s);
    let all = |_: usize| true;

    let mut crafts = Vec::new();
    let mut forecast_craft = Vec::new();
    // (craft index, pass index, worst margin at the exported EIRP)
    let mut export_cands: Vec<(usize, usize, f64)> = Vec::new();
    let mut per_craft_steps: Vec<Vec<PassSteps>> = Vec::new();
    for (ci, ((name, l1, _), orb)) in tles.iter().zip(&orbits).enumerate() {
        let mut passes: Vec<Pass> = Vec::new();
        let mut psteps: Vec<PassSteps> = Vec::new();
        let mut cur: Option<Pass> = None;
        let mut cur_steps: Vec<StepRec> = Vec::new();
        let mut rev = 0usize;
        let mut rev_starts = vec![0.0f64];
        let mut z_prev: Option<f64> = None;
        for k in 0..=n_steps {
            let t = t0.add_seconds(k as f64 * s.step_s);
            let (pos, _) = orb
                .itrs_state_with(t, &teme_to_itrs_matrix(t))
                .map_err(|e| bad(format!("{name}: {e}")))?;
            // A revolution starts at each ascending node (z crossing upward).
            if let Some(zp) = z_prev {
                if zp < 0.0 && pos[2] >= 0.0 {
                    rev += 1;
                    rev_starts.push(((k - 1) as f64 + (-zp) / (pos[2] - zp)) * s.step_s);
                }
            }
            z_prev = Some(pos[2]);
            let g = ecef_to_geodetic(pos);
            let (slat, slon) = (g.lat_rad / DEG, g.lon_rad / DEG);
            let rn = dot(pos, pos).sqrt();
            let mut seen: Vec<usize> = Vec::new();
            let mut terms: Vec<Term> = Vec::new();
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
                let (gain, off_deg) = match fc {
                    None => (r.gain_toward_ground_dbi, f64::NAN),
                    Some(f) => {
                        // Angle between the zenith boresight and the direction to the area.
                        let c = -dot(los, pos) / (d * rn);
                        let off = c.clamp(-1.0, 1.0).acos() / DEG;
                        (
                            f.link.gain_dbi(off)
                                - f.link.polarisation_loss_db
                                - f.link.other_losses_db,
                            off,
                        )
                    }
                };
                for (e, j) in s.assumed_eirp_dbw.iter().zip(j_lin.iter_mut()) {
                    *j += 10f64.powf((e + gain - path) / 10.0);
                }
                terms.push(Term {
                    ai,
                    g_db: gain,
                    path_db: path,
                    range_km: d / KM,
                    off_deg,
                });
            }
            if seen.is_empty() {
                if let Some(p) = cur.take() {
                    passes.push(p);
                    psteps.push(PassSteps {
                        steps: std::mem::take(&mut cur_steps),
                    });
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
                outage_windows: None,
                link_budget: None,
            });
            cur_steps.push(StepRec {
                k,
                rev,
                terms,
                over,
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
            psteps.push(PassSteps {
                steps: std::mem::take(&mut cur_steps),
            });
        }
        let tk = |k: usize| fmt_utc(t0.add_seconds(k as f64 * s.step_s));
        if let Some(f) = fc {
            for (p, ps) in passes.iter_mut().zip(&psteps) {
                let mut per_e = Vec::with_capacity(n_e);
                for &e in &s.assumed_eirp_dbw {
                    let seq: Vec<(usize, f64)> = ps
                        .steps
                        .iter()
                        .map(|st| (st.k, mm.of(Margin::j_lin(e, &st.terms, &all))))
                        .collect();
                    let wins = outage_windows(&seq, s.step_s, reacq_s);
                    per_e.push(
                        wins.iter()
                            .map(|w| {
                                let worst = ps.steps.iter().find(|st| st.k == w.worst_k);
                                OutageWindow {
                                    start_utc: tk(w.k0),
                                    loss_end_utc: tk(w.k1 + 1),
                                    end_utc: fmt_utc(
                                        t0.add_seconds((w.k1 + 1) as f64 * s.step_s + reacq_s),
                                    ),
                                    loss_s: w.loss_steps as f64 * s.step_s,
                                    outage_s: w.outage_s(s.step_s, reacq_s),
                                    worst_margin_db: w.worst,
                                    areas: worst
                                        .map(|st| {
                                            st.terms
                                                .iter()
                                                .map(|t| areas[t.ai].label.clone())
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                }
                            })
                            .collect(),
                    );
                }
                p.outage_windows = Some(per_e);
                p.link_budget = Some(link_budget(&ps.steps, &areas, f, &mm, s, t0));
            }
            forecast_craft.push(forecast_orbits(
                ci,
                name,
                &psteps,
                &rev_starts,
                &areas,
                &mm,
                s,
                f,
                t0,
                (n_steps + 1) as f64 * s.step_s,
            ));
        }
        if let Some(ie) = export_e {
            for (pi, p) in passes.iter().enumerate() {
                export_cands.push((ci, pi, p.min_margin_db[ie]));
            }
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
        per_craft_steps.push(psteps);
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
    if let Some(f) = fc {
        doc["assumptions"]["link_model"] = json!({
            "antenna_pattern": f.link.antenna_pattern,
            "boresight": "the geocentric zenith (a zenith-pointing antenna; no attitude, no body or limb blockage)",
            "polarisation_loss_db": f.link.polarisation_loss_db,
            "other_losses_db": f.link.other_losses_db,
            "replaces": "receiver.gain_toward_ground_dbi (not used when the forecast is on)",
            "reacquisition_s": f.reacquisition_s,
        });
        doc["forecast"] = json!({
            "label": FORECAST_LABEL,
            "revolution": "a revolution starts at each ascending node (ITRS z crossing upward, interpolated between steps); the first and last revolutions of the span are partial",
            "activity": format!("each area is active for a whole revolution with its activity probability, independently of the others; exact enumeration up to {EXACT_MAX_UNCERTAIN} uncertain areas in view per revolution, else a seeded Monte Carlo draw of {} samples", f.monte_carlo_samples),
            "outage_window": "from the first step with a modelled margin under zero to the last one, plus reacquisition_s; runs closer than that are merged",
            "craft": forecast_craft,
        });
    }
    let mut exported = Vec::new();
    if let (Some(x), Some(ie)) = (&s.export, export_e) {
        let (index, ex) = export_scenarios(
            s,
            x,
            ie,
            &export_cands,
            &doc,
            &tles,
            &orbits,
            &per_craft_steps,
            &mm,
            t0,
            reacq_s,
        )?;
        doc["test_scenarios"] = index;
        exported = ex;
    }
    // NaN is not JSON: a margin with no exposure is null.
    sanitise(&mut doc);
    let digest = hex::encode(Sha256::digest(
        serde_json::to_vec(&doc).map_err(|e| bad(e.to_string()))?,
    ));
    doc["result_sha256"] = json!(digest);
    Ok((doc, exported))
}

/// The label of the forecast section.
pub const FORECAST_LABEL: &str = "MODELLED per-revolution exposure forecast. The areas come \
from the maps and regions given (where interference was reported or is expected on the \
ground; no emitter is located), their activity probabilities are inputs, the interferer EIRP \
is assumed, and the receiver link margin is the stated model. A forecast of what the model \
gives for the orbit, not a prediction that any interferer will be on air.";

/// The link budget at the pass's worst step (the largest summed coupling).
fn link_budget(
    steps: &[StepRec],
    areas: &[Area],
    f: &Forecast,
    mm: &Margin,
    s: &SpaceborneRxExposureScenario,
    t0: Jd2,
) -> Value {
    let coupling = |st: &StepRec| -> f64 {
        st.terms
            .iter()
            .map(|t| 10f64.powf((t.g_db - t.path_db) / 10.0))
            .sum()
    };
    let Some(st) = steps
        .iter()
        .max_by(|a, b| coupling(a).total_cmp(&coupling(b)))
    else {
        return Value::Null;
    };
    let c = coupling(st);
    let dom = st
        .terms
        .iter()
        .max_by(|a, b| (a.g_db - a.path_db).total_cmp(&(b.g_db - b.path_db)))
        .expect("a step in view has a term");
    let per_eirp: Vec<Value> = s
        .assumed_eirp_dbw
        .iter()
        .map(|&e| {
            let j = 10f64.powf(e / 10.0) * c;
            let js = 10.0 * j.log10() - mm.s_rx;
            let m = mm.of(j);
            json!({
                "eirp_dbw": e,
                "interference_dbw": 10.0 * j.log10(),
                "js_db": js,
                "effective_cn0_dbhz": m + mm.r.tracking_threshold_dbhz,
                "margin_db": m,
            })
        })
        .collect();
    json!({
        "utc": fmt_utc(t0.add_seconds(st.k as f64 * s.step_s)),
        "areas_in_view": st.terms.len(),
        "dominant_area": areas[dom.ai].label,
        "range_km": dom.range_km,
        "off_boresight_deg": dom.off_deg,
        "rx_gain_dbi": f.link.gain_dbi(dom.off_deg),
        "polarisation_loss_db": f.link.polarisation_loss_db,
        "other_losses_db": f.link.other_losses_db,
        "path_loss_db": dom.path_db,
        "aggregate_coupling_db": 10.0 * c.log10(),
        "signal_dbw": mm.s_rx,
        "nominal_cn0_dbhz": mm.cn0_nom,
        "tracking_threshold_dbhz": mm.r.tracking_threshold_dbhz,
        "per_eirp": per_eirp,
    })
}

/// The per-revolution forecast of one spacecraft.
#[allow(clippy::too_many_arguments)]
fn forecast_orbits(
    ci: usize,
    name: &str,
    psteps: &[PassSteps],
    rev_starts: &[f64],
    areas: &[Area],
    mm: &Margin,
    s: &SpaceborneRxExposureScenario,
    f: &Forecast,
    t0: Jd2,
    end_s: f64,
) -> Value {
    use rand::{Rng, SeedableRng};
    let n_e = s.assumed_eirp_dbw.len();
    let all_steps: Vec<&StepRec> = psteps.iter().flat_map(|p| p.steps.iter()).collect();
    let n_rev = rev_starts.len();
    let mut orbits = Vec::with_capacity(n_rev);
    let mut tot_exp = 0.0;
    let mut tot_eexp = 0.0;
    let mut tot_out = vec![0.0; n_e];
    let mut tot_eout = vec![0.0; n_e];
    let mut orbits_out = vec![0usize; n_e];
    let mut e_orbits_out = vec![0.0; n_e];
    for rv in 0..n_rev {
        let st: Vec<&StepRec> = all_steps.iter().copied().filter(|x| x.rev == rv).collect();
        let a = rev_starts[rv];
        let b = rev_starts.get(rv + 1).copied().unwrap_or(end_s);
        let exposure = st.len() as f64 * s.step_s;
        let over = st.iter().filter(|x| x.over).count() as f64 * s.step_s;
        let e_exposure: f64 = st
            .iter()
            .map(|x| {
                1.0 - x
                    .terms
                    .iter()
                    .map(|t| 1.0 - areas[t.ai].activity)
                    .product::<f64>()
            })
            .sum::<f64>()
            * s.step_s;
        // Outage time of the revolution for a set of active areas, per EIRP.
        let outage = |inc: &dyn Fn(usize) -> bool| -> Vec<f64> {
            s.assumed_eirp_dbw
                .iter()
                .map(|&e| {
                    let seq: Vec<(usize, f64)> = st
                        .iter()
                        .map(|x| (x.k, mm.of(Margin::j_lin(e, &x.terms, inc))))
                        .collect();
                    outage_windows(&seq, s.step_s, f.reacquisition_s)
                        .iter()
                        .map(|w| w.outage_s(s.step_s, f.reacquisition_s))
                        .sum()
                })
                .collect()
        };
        let min_margin: Vec<f64> = s
            .assumed_eirp_dbw
            .iter()
            .map(|&e| {
                st.iter()
                    .map(|x| mm.of(Margin::j_lin(e, &x.terms, &|_| true)))
                    .fold(f64::INFINITY, f64::min)
            })
            .collect();
        let det = outage(&|_| true);
        let mut unc: Vec<usize> = st
            .iter()
            .flat_map(|x| x.terms.iter().map(|t| t.ai))
            .filter(|&ai| areas[ai].activity < 1.0)
            .collect();
        unc.sort_unstable();
        unc.dedup();
        let (exp_out, p_out, method) = if unc.is_empty() {
            (
                det.clone(),
                det.iter().map(|o| f64::from(u8::from(*o > 0.0))).collect(),
                "certain",
            )
        } else {
            let mut eo = vec![0.0; n_e];
            let mut po = vec![0.0; n_e];
            let mut add = |active: &[bool], w: f64| {
                let inc = |ai: usize| match unc.binary_search(&ai) {
                    Ok(i) => active[i],
                    Err(_) => areas[ai].activity > 0.0,
                };
                let o = outage(&inc);
                for i in 0..n_e {
                    eo[i] += w * o[i];
                    if o[i] > 0.0 {
                        po[i] += w;
                    }
                }
            };
            if unc.len() <= EXACT_MAX_UNCERTAIN {
                for mask in 0u32..(1u32 << unc.len()) {
                    let active: Vec<bool> = (0..unc.len()).map(|i| mask >> i & 1 == 1).collect();
                    let w: f64 = unc
                        .iter()
                        .zip(&active)
                        .map(|(&ai, &on)| {
                            if on {
                                areas[ai].activity
                            } else {
                                1.0 - areas[ai].activity
                            }
                        })
                        .product();
                    if w > 0.0 {
                        add(&active, w);
                    }
                }
                (eo, po, "exact")
            } else {
                let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(
                    0x5eed_0000_0000 ^ ((ci as u64) << 20) ^ rv as u64,
                );
                let n = f.monte_carlo_samples;
                for _ in 0..n {
                    let active: Vec<bool> = unc
                        .iter()
                        .map(|&ai| rng.gen::<f64>() < areas[ai].activity)
                        .collect();
                    add(&active, 1.0 / n as f64);
                }
                (eo, po, "monte-carlo")
            }
        };
        tot_exp += exposure;
        tot_eexp += e_exposure;
        for i in 0..n_e {
            tot_out[i] += det[i];
            tot_eout[i] += exp_out[i];
            if det[i] > 0.0 {
                orbits_out[i] += 1;
            }
            e_orbits_out[i] += p_out[i];
        }
        orbits.push(json!({
            "rev": rv,
            "start_utc": fmt_utc(t0.add_seconds(a)),
            "end_utc": fmt_utc(t0.add_seconds(b)),
            "partial": rv == 0 || rv + 1 == n_rev,
            "exposure_s": exposure,
            "expected_exposure_s": e_exposure,
            "over_area_s": over,
            "min_margin_db": min_margin.iter().map(|m| if m.is_finite() { *m } else { f64::NAN }).collect::<Vec<_>>(),
            "outage_s": det,
            "expected_outage_s": exp_out,
            "p_outage": p_out,
            "uncertain_areas": unc.len(),
            "activity_method": method,
        }));
    }
    json!({
        "name": name,
        "revolutions": n_rev,
        "totals": {
            "exposure_s": tot_exp,
            "expected_exposure_s": tot_eexp,
            "outage_s": tot_out,
            "expected_outage_s": tot_eout,
            "revolutions_with_outage": orbits_out,
            "expected_revolutions_with_outage": e_orbits_out,
        },
        "orbits": orbits,
    })
}

// ── Export of matching test scenarios ────────────────────────────────────────────────────

/// Check `[export]` and return the index of the exported EIRP.
fn check_export(x: &ExportSpec, eirps: &[f64]) -> Result<usize, KshanaError> {
    let ie = match x.eirp_dbw {
        Some(e) => eirps
            .iter()
            .position(|v| (v - e).abs() < 1e-9)
            .ok_or_else(|| {
                bad(format!(
                    "export.eirp_dbw {e} is not one of assumed_eirp_dbw"
                ))
            })?,
        None => eirps
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap_or(0),
    };
    if !matches!(x.include.as_str(), "outage" | "degraded" | "all") {
        return Err(bad(format!(
            "export.include '{}' (outage|degraded|all)",
            x.include
        )));
    }
    let pos = |v: f64| v.is_finite() && v > 0.0;
    let nonneg = |v: f64| v.is_finite() && v >= 0.0;
    if !(nonneg(x.lead_s)
        && nonneg(x.tail_s)
        && (0.1..=60.0).contains(&x.sample_step_s)
        && pos(x.js_step_db)
        && x.js_floor_db.is_finite()
        && pos(x.bandwidth_hz)
        && pos(x.detect_bound_s)
        && pos(x.recovery_bound_s)
        && (0.0..=1.0).contains(&x.availability_bound)
        && pos(x.misleading_h_error_m)
        && pos(x.recover_h_error_m)
        && (1..=1000).contains(&x.max_scenarios)
        && pos(x.max_interpolation_error_m))
    {
        return Err(bad(
            "export: lead_s, tail_s >= 0; sample_step_s in [0.1, 60]; js_step_db, bandwidth_hz, bounds and errors > 0; availability_bound in [0, 1]; max_scenarios in [1, 1000]",
        ));
    }
    Ok(ie)
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn fixed(x: f64, scale: f64) -> f64 {
    let v = (x * scale).round() / scale;
    if v == 0.0 {
        0.0
    } else {
        v
    }
}

/// Spherical surface distance (m) between two (lat, lon) points (deg).
fn surface_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (p1, p2) = (a.0 * DEG, b.0 * DEG);
    let dl = (b.1 - a.1) * DEG;
    let h = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * h.sqrt().clamp(0.0, 1.0).asin()
}

/// A sample of the truth: (t s, lat deg, lon deg, height m).
type Sample = (f64, f64, f64, f64);

/// Sample the track over `[0, dur]` every `step` s with longitudes kept continuous, then
/// bisect every interval whose midpoint is farther than `tol` m (horizontal) from the linear
/// interpolation, down to 50 ms. Returns the samples and the largest horizontal and vertical
/// midpoint errors left.
fn sample_track(
    at: &dyn Fn(f64) -> Result<Sample, KshanaError>,
    dur: f64,
    step: f64,
    tol: f64,
) -> Result<(Vec<Sample>, f64, f64), KshanaError> {
    let unwrap = |prev: Option<&Sample>, mut x: Sample| {
        if let Some(p) = prev {
            x.2 = fixed(p.2 + (x.2 - p.2 + 540.0).rem_euclid(360.0) - 180.0, 1e6);
        }
        x
    };
    let mut grid: Vec<Sample> = Vec::new();
    let n = (dur / step).ceil() as usize;
    for i in 0..=n {
        let t = fixed((i as f64 * step).min(dur), 1e3);
        if grid.last().is_some_and(|p| p.0 >= t) {
            continue;
        }
        let x = unwrap(grid.last(), at(t)?);
        grid.push(x);
    }
    let mut out = vec![grid[0]];
    let (mut max_h, mut max_v) = (0.0f64, 0.0f64);
    for w in grid.windows(2) {
        // Depth-first bisection of one grid interval.
        let mut stack = vec![(w[0], w[1])];
        while let Some((a, b)) = stack.pop() {
            let tm = fixed((a.0 + b.0) / 2.0, 1e3);
            let m = unwrap(Some(&a), at(tm)?);
            let lin = ((a.1 + b.1) / 2.0, (a.2 + b.2) / 2.0, (a.3 + b.3) / 2.0);
            let eh = surface_m((m.1, m.2), (lin.0, lin.1));
            if eh > tol && b.0 - a.0 > 0.1 {
                stack.push((m, b));
                stack.push((a, m));
                continue;
            }
            max_h = max_h.max(eh);
            max_v = max_v.max((m.3 - lin.2).abs());
            out.push(b);
        }
    }
    Ok((out, fixed(max_h, 10.0), fixed(max_v, 10.0)))
}

/// Longitudes back into [-180, 180]; at each antimeridian crossing two samples 1 ms apart
/// at lon +-180, so no interpolated interval spans the map edge for longer than that.
fn wrap_track(track: &[Sample]) -> Vec<Sample> {
    let wrap = |l: f64| fixed((l + 540.0).rem_euclid(360.0) - 180.0, 1e6);
    let mut out: Vec<Sample> = Vec::with_capacity(track.len());
    for w in track.windows(2) {
        let (a, b) = (w[0], w[1]);
        if out.is_empty() {
            out.push((a.0, a.1, wrap(a.2), a.3));
        }
        // The multiple of 180 (odd) between the two, if any: a crossing.
        let k = ((a.2 + 180.0) / 360.0).floor();
        let kb = ((b.2 + 180.0) / 360.0).floor();
        if k != kb {
            let edge = if b.2 > a.2 {
                kb * 360.0 - 180.0
            } else {
                k * 360.0 - 180.0
            };
            let fr = (edge - a.2) / (b.2 - a.2);
            let tc = fixed(a.0 + fr * (b.0 - a.0), 1e3);
            if tc > a.0 && tc + 0.001 < b.0 {
                let lac = fixed(a.1 + fr * (b.1 - a.1), 1e6);
                let hc = fixed(a.3 + fr * (b.3 - a.3), 10.0);
                let (l1, l2) = if b.2 > a.2 {
                    (180.0, -180.0)
                } else {
                    (-180.0, 180.0)
                };
                out.push((tc, lac, l1, hc));
                out.push((fixed(tc + 0.001, 1e3), lac, l2, hc));
            }
        }
        out.push((b.0, b.1, wrap(b.2), b.3));
    }
    if out.is_empty() {
        out.extend(track.iter().map(|a| (a.0, a.1, wrap(a.2), a.3)));
    }
    out
}

/// The exported scenarios and the study's index of them.
#[allow(clippy::too_many_arguments)]
fn export_scenarios(
    s: &SpaceborneRxExposureScenario,
    x: &ExportSpec,
    ie: usize,
    cands: &[(usize, usize, f64)],
    doc: &Value,
    tles: &[(String, String, String)],
    orbits: &[SgpOrbit],
    steps: &[Vec<PassSteps>],
    mm: &Margin,
    t0: Jd2,
    reacq_s: f64,
) -> Result<(Value, Vec<ExportedScenario>), KshanaError> {
    use kshana_pro::testkit::spec::{Spec, FORMAT};
    let e = s.assumed_eirp_dbw[ie];
    let deg_m = s.receiver.degraded_margin_db;
    let mut sel: Vec<(usize, usize, f64)> = cands
        .iter()
        .copied()
        .filter(|c| match x.include.as_str() {
            "outage" => c.2 < 0.0,
            "degraded" => c.2 < deg_m,
            _ => true,
        })
        .collect();
    let considered = sel.len();
    // Worst margin first; ties by spacecraft and pass order (stable).
    sel.sort_by(|a, b| a.2.total_cmp(&b.2));
    sel.truncate(x.max_scenarios);
    let all = |_: usize| true;
    let mut out = Vec::new();
    let mut index = Vec::new();
    let mut skipped = 0usize;
    for (ci, pi, worst) in sel {
        let name = &tles[ci].0;
        let orb = &orbits[ci];
        let ps = &steps[ci][pi].steps;
        let pass = &doc["craft"][ci]["passes"][pi];
        let k0 = ps[0].k;
        let k_end = ps[ps.len() - 1].k + 1;
        let t_pass0 = t0.add_seconds(k0 as f64 * s.step_s);
        let scene_t0 = parse_utc(&fmt_utc(t_pass0.add_seconds(-x.lead_s)))?;
        let off = |k: usize| t0.add_seconds(k as f64 * s.step_s).diff_seconds(scene_t0);
        let dur = fixed(off(k_end) + x.tail_s, 1e3);
        // The modelled J/S of the pass at the exported EIRP, quantised, as jamming events.
        let mut runs: Vec<(usize, usize, f64)> = Vec::new(); // (first k, steps, J/S)
        for st in ps {
            let j = Margin::j_lin(e, &st.terms, &all);
            let js = 10.0 * j.log10() - mm.s_rx;
            let q = fixed((js / x.js_step_db).round() * x.js_step_db, 1e6);
            match runs.last_mut() {
                Some(r) if r.2 == q && r.0 + r.1 == st.k => r.1 += 1,
                _ => runs.push((st.k, 1, q)),
            }
        }
        let runs: Vec<_> = runs.into_iter().filter(|r| r.2 >= x.js_floor_db).collect();
        if runs.is_empty() {
            skipped += 1;
            continue;
        }
        let threats: Vec<Value> = runs
            .iter()
            .enumerate()
            .map(|(i, r)| {
                json!({
                    "id": format!("jam-{:02}", i + 1),
                    "type": "jamming",
                    "waveform": "wideband",
                    "start_s": fixed(off(r.0).max(0.0), 1e3),
                    "duration_s": fixed(r.1 as f64 * s.step_s, 1e3),
                    "center_hz": L1_HZ,
                    "bandwidth_hz": x.bandwidth_hz,
                    "js_db": r.2,
                })
            })
            .collect();
        let seq: Vec<(usize, f64)> = ps
            .iter()
            .map(|st| (st.k, mm.of(Margin::j_lin(e, &st.terms, &all))))
            .collect();
        let wins = outage_windows(&seq, s.step_s, reacq_s);
        let predicted = if !wins.is_empty() {
            "outage"
        } else if worst < deg_m {
            "degraded"
        } else {
            "kept"
        };
        // The truth: the propagated sub-satellite point and height, sampled every
        // `sample_step_s` and bisected where linear interpolation strays from the orbit.
        let at = |t: f64| -> Result<(f64, f64, f64, f64), KshanaError> {
            let tt = scene_t0.add_seconds(t);
            let (pos, _) = orb
                .itrs_state_with(tt, &teme_to_itrs_matrix(tt))
                .map_err(|e| bad(format!("{name}: {e}")))?;
            let g = ecef_to_geodetic(pos);
            Ok((
                t,
                fixed(g.lat_rad / DEG, 1e6),
                fixed(g.lon_rad / DEG, 1e6),
                fixed(g.alt_m, 10.0),
            ))
        };
        let (track, max_h_m, max_v_m) =
            sample_track(&at, dur, x.sample_step_s, x.max_interpolation_error_m)?;
        let samples = wrap_track(&track);
        let first = samples[0];
        let start_utc = fmt_utc(scene_t0);
        let id = format!(
            "rx-exposure-{}-{}-eirp{}",
            slug(name),
            start_utc
                .replace(['-', ':'], "")
                .trim_end_matches('Z')
                .to_ascii_lowercase(),
            format!("{e}").replace('.', "p")
        );
        let loss_threat = wins.first().and_then(|w| {
            runs.iter()
                .position(|r| r.0 <= w.k0 && w.k0 < r.0 + r.1)
                .map(|i| format!("jam-{:02}", i + 1))
        });
        let mut criteria = Vec::new();
        if predicted == "outage" {
            let mut c = json!({"id": "detect", "metric": "detection_latency_s", "op": "<=", "value": x.detect_bound_s,
                "note": "the receiver withdraws its fix or flags interference once the modelled loss of tracking starts"});
            if let Some(t) = &loss_threat {
                c["threat"] = json!(t);
            }
            criteria.push(c);
        } else {
            criteria.push(json!({"id": "available", "metric": "availability", "op": ">=", "value": x.availability_bound,
                "threshold": x.misleading_h_error_m,
                "note": "the modelled margin stays above zero: the fix is expected to stay valid"}));
        }
        criteria.push(
            json!({"id": "no-misleading", "metric": "misleading_epochs", "op": "<=", "value": 0.0}),
        );
        criteria.push(json!({"id": "recover", "metric": "recovery_s", "op": "<=", "value": x.recovery_bound_s}));
        let behaviour = match predicted {
            "outage" => format!(
                "The model predicts a loss of tracking ({} s under the threshold, worst margin {:.1} dB). The receiver withdraws or flags its fix within {} s of the loss, gives no misleading fix, and recovers within {} s after the interference ends.",
                wins.iter().map(|w| w.loss_steps as f64 * s.step_s).sum::<f64>(),
                worst,
                x.detect_bound_s,
                x.recovery_bound_s
            ),
            "degraded" => format!(
                "The model predicts degraded tracking (worst margin {:.1} dB, above zero). The receiver keeps a valid fix for at least {:.0} % of the interference, gives no misleading fix, and recovers within {} s.",
                worst,
                x.availability_bound * 100.0,
                x.recovery_bound_s
            ),
            _ => format!(
                "The model predicts tracking is kept (worst margin {worst:.1} dB). The receiver keeps a valid fix for at least {:.0} % of the interference and gives no misleading fix.",
                x.availability_bound * 100.0
            ),
        };
        let spec_v = json!({
            "format": FORMAT,
            "id": id,
            "version": 1,
            "title": format!("{name}: pass from {} under modelled ground interference ({e} dBW per area)", pass["start_utc"].as_str().unwrap_or("")),
            "description": format!(
                "Spaceborne GNSS receiver test generated from a MODELLED exposure study (study kind {KIND}). The jamming events are the modelled J/S at the receiver for an assumed EIRP of {e} dBW per affected area, quantised to {} dB steps, runs under {} dB left out. The trajectory is the SGP4-propagated orbit (sub-satellite point and height) at least every {} s; samples are added where needed so that linear interpolation between them is within {max_h_m} m horizontally and {max_v_m} m in height of the propagated orbit (checked at interval midpoints). The format has no spacecraft platform: `aircraft` only labels the scenario. A simulator must support orbital dynamics; check before running.",
                x.js_step_db, x.js_floor_db, x.sample_step_s
            ),
            "evidence": "MODELLED",
            "modelled_on": [format!("Arksa spaceborne interference exposure study, {name}, pass from {} (MODELLED: assumed interferer EIRP, link-margin model; not an observation)", pass["start_utc"].as_str().unwrap_or(""))],
            "tags": ["spacecraft", "gnss-interference-exposure", "jamming", predicted],
            "scene": {"start_utc": start_utc, "duration_s": dur},
            "signal": [{"constellation": "gps", "signals": ["L1CA"]}],
            "trajectory": {
                "platform": "aircraft",
                "lat_deg": first.1,
                "lon_deg": first.2,
                "height_m": first.3,
                "sample": samples.iter().map(|p| json!({"t_s": p.0, "lat_deg": p.1, "lon_deg": p.2, "height_m": p.3})).collect::<Vec<_>>(),
            },
            "threat": threats,
            "expected": {
                "behaviour": behaviour,
                "misleading_h_error_m": x.misleading_h_error_m,
                "recover_h_error_m": x.recover_h_error_m,
            },
            "criterion": criteria,
        });
        let spec: Spec =
            serde_json::from_value(spec_v).map_err(|e| bad(format!("export {id}: {e}")))?;
        spec.validate().map_err(|e| bad(format!("export: {e}")))?;
        let sha = spec.sha256();
        let file = format!("scenarios/{id}.toml");
        index.push(json!({
            "id": id,
            "file": file,
            "sha256": sha,
            "craft": name,
            "pass_start_utc": pass["start_utc"],
            "predicted": predicted,
            "worst_margin_db": worst,
            "jamming_events": runs.len(),
            "max_js_db": runs.iter().map(|r| r.2).fold(f64::NEG_INFINITY, f64::max),
            "trajectory_samples": samples.len(),
            "max_interpolation_error_m": max_h_m,
        }));
        out.push(ExportedScenario {
            id,
            file,
            toml: spec.to_toml(),
            sha256: sha,
        });
    }
    Ok((
        json!({
            "format": FORMAT,
            "label": "MODELLED: receiver test scenarios generated from this study; the J/S profiles follow from the assumed EIRP and the link model; the criteria are stated before any run. Not a result of any receiver test.",
            "eirp_dbw": e,
            "include": x.include,
            "passes_selected": considered,
            "passes_without_events": skipped,
            "scenarios": index,
        }),
        out,
    ))
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
    for c in doc["forecast"]["craft"].as_array().into_iter().flatten() {
        let o: Vec<String> = c["totals"]["revolutions_with_outage"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| x.to_string())
            .collect();
        out.push_str(&format!(
            " forecast {}: {} revolutions, with outage {};",
            c["name"].as_str().unwrap_or(""),
            c["revolutions"],
            o.join("/")
        ));
    }
    if let Some(n) = doc["test_scenarios"]["scenarios"].as_array() {
        out.push_str(&format!(" {} test scenarios exported;", n.len()));
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

fn eirps_of(doc: &Value) -> Vec<f64> {
    doc["assumptions"]["assumed_eirp_dbw"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_f64)
        .collect()
}

/// Per-revolution forecast CSV (empty without the forecast).
pub fn orbits_csv(doc: &Value) -> String {
    let eirp = eirps_of(doc);
    let mut s = String::from(
        "# MODELLED (see forecast.label in study.json)\ncraft,rev,start_utc,end_utc,partial,exposure_s,expected_exposure_s,over_area_s",
    );
    for e in &eirp {
        s.push_str(&format!(
            ",min_margin_db_eirp{e},outage_s_eirp{e},expected_outage_s_eirp{e},p_outage_eirp{e}"
        ));
    }
    s.push('\n');
    let f = |v: &Value| v.as_f64().map_or("".into(), |x| format!("{x:.2}"));
    for c in doc["forecast"]["craft"].as_array().into_iter().flatten() {
        for o in c["orbits"].as_array().into_iter().flatten() {
            s.push_str(&format!(
                "{},{},{},{},{},{},{},{}",
                c["name"].as_str().unwrap_or("").replace(',', " "),
                o["rev"],
                o["start_utc"].as_str().unwrap_or(""),
                o["end_utc"].as_str().unwrap_or(""),
                o["partial"],
                f(&o["exposure_s"]),
                f(&o["expected_exposure_s"]),
                f(&o["over_area_s"]),
            ));
            for i in 0..eirp.len() {
                s.push_str(&format!(
                    ",{},{},{},{}",
                    f(&o["min_margin_db"][i]),
                    f(&o["outage_s"][i]),
                    f(&o["expected_outage_s"][i]),
                    o["p_outage"][i]
                        .as_f64()
                        .map_or("".into(), |x| format!("{x:.4}"))
                ));
            }
            s.push('\n');
        }
    }
    s
}

/// Predicted outage windows CSV, one row per window per assumed EIRP (forecast only).
pub fn outages_csv(doc: &Value) -> String {
    let eirp = eirps_of(doc);
    let mut s = String::from(
        "# MODELLED (see label in study.json)\ncraft,pass_start_utc,eirp_dbw,start_utc,loss_end_utc,end_utc,loss_s,outage_s,worst_margin_db,areas\n",
    );
    let f = |v: &Value| v.as_f64().map_or("".into(), |x| format!("{x:.2}"));
    for c in doc["craft"].as_array().into_iter().flatten() {
        for p in c["passes"].as_array().into_iter().flatten() {
            for (i, e) in eirp.iter().enumerate() {
                for w in p["outage_windows"][i].as_array().into_iter().flatten() {
                    s.push_str(&format!(
                        "{},{},{e},{},{},{},{},{},{},{}\n",
                        c["name"].as_str().unwrap_or("").replace(',', " "),
                        p["start_utc"].as_str().unwrap_or(""),
                        w["start_utc"].as_str().unwrap_or(""),
                        w["loss_end_utc"].as_str().unwrap_or(""),
                        w["end_utc"].as_str().unwrap_or(""),
                        f(&w["loss_s"]),
                        f(&w["outage_s"]),
                        f(&w["worst_margin_db"]),
                        w["areas"].as_array().map_or(0, Vec::len)
                    ));
                }
            }
        }
    }
    s
}

/// The forecast and export sections of the report (empty without them).
fn report_v2(doc: &Value) -> String {
    let mut s = String::new();
    let eirp = eirps_of(doc);
    let fc = &doc["forecast"];
    if fc.is_object() {
        s.push_str(&format!(
            "\n## Per-revolution forecast\n\n**{}**\n\nLink model: {}. Revolutions: {} Activity: {} Outage window: {}.\n\n",
            fc["label"].as_str().unwrap_or(""),
            doc["assumptions"]["link_model"],
            fc["revolution"].as_str().unwrap_or(""),
            fc["activity"].as_str().unwrap_or(""),
            fc["outage_window"].as_str().unwrap_or("")
        ));
        s.push_str("| Spacecraft | Revolutions | Exposed (min) | Expected exposed (min) | Outage (min) per EIRP | Expected outage (min) per EIRP | Revolutions with outage per EIRP |\n|---|---|---|---|---|---|---|\n");
        let mins = |v: &Value| -> String {
            v.as_array()
                .into_iter()
                .flatten()
                .map(|x| format!("{:.1}", x.as_f64().unwrap_or(0.0) / 60.0))
                .collect::<Vec<_>>()
                .join(" / ")
        };
        for c in fc["craft"].as_array().into_iter().flatten() {
            let t = &c["totals"];
            s.push_str(&format!(
                "| {} | {} | {:.1} | {:.1} | {} | {} | {} |\n",
                c["name"].as_str().unwrap_or(""),
                c["revolutions"],
                t["exposure_s"].as_f64().unwrap_or(0.0) / 60.0,
                t["expected_exposure_s"].as_f64().unwrap_or(0.0) / 60.0,
                mins(&t["outage_s"]),
                mins(&t["expected_outage_s"]),
                t["revolutions_with_outage"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(" / ")
            ));
        }
        s.push_str(&format!(
            "\nEIRP order: {} dBW. Per revolution: `orbits.csv`. Predicted outage windows per pass: `outages.csv`.\n",
            eirp.iter().map(|e| format!("{e}")).collect::<Vec<_>>().join(", ")
        ));
    }
    let ts = &doc["test_scenarios"];
    if ts.is_object() {
        s.push_str(&format!(
            "\n## Matching receiver test scenarios\n\n{}\n\nFormat `{}`, EIRP {} dBW, passes `{}`: {} selected, {} without jamming events over the floor.\n\n| Scenario | Spacecraft | Predicted | Worst margin (dB) | Events | Max J/S (dB) | SHA-256 |\n|---|---|---|---|---|---|---|\n",
            ts["label"].as_str().unwrap_or(""),
            ts["format"].as_str().unwrap_or(""),
            ts["eirp_dbw"],
            ts["include"].as_str().unwrap_or(""),
            ts["passes_selected"],
            ts["passes_without_events"]
        ));
        for x in ts["scenarios"].as_array().into_iter().flatten() {
            s.push_str(&format!(
                "| `{}` | {} | {} | {:.1} | {} | {} | {} |\n",
                x["file"].as_str().unwrap_or(""),
                x["craft"].as_str().unwrap_or(""),
                x["predicted"].as_str().unwrap_or(""),
                x["worst_margin_db"].as_f64().unwrap_or(0.0),
                x["jamming_events"],
                x["max_js_db"],
                &x["sha256"].as_str().unwrap_or("")
                    [..16.min(x["sha256"].as_str().unwrap_or("").len())]
            ));
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
    s.push_str(&report_v2(doc));
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
    write_study_with_scenarios(doc, &[], dir, signer)
}

/// [`write_study`], plus, with the forecast, `orbits.csv` and `outages.csv`, and the exported
/// test scenarios under `scenarios/`; the signature covers them all.
pub fn write_study_with_scenarios(
    doc: &Value,
    scenarios: &[ExportedScenario],
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
    if doc.get("forecast").is_some() {
        w("orbits.csv", orbits_csv(doc))?;
        w("outages.csv", outages_csv(doc))?;
    }
    if !scenarios.is_empty() {
        let sd = dir.join("scenarios");
        std::fs::create_dir_all(&sd).map_err(|e| bad(format!("{}: {e}", sd.display())))?;
        for x in scenarios {
            w(&x.file, x.toml.clone())?;
        }
    }
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
    fn outage_windows_merge_within_the_reacquisition_time() {
        // Steps of 10 s: loss at 3-4, again at 6 (gap 1 step = 10 s <= 20 s reacq: merged),
        // and at 20 (separate).
        let seq: Vec<(usize, f64)> = (0..25)
            .map(|k| {
                (
                    k,
                    if [3, 4, 6, 20].contains(&k) {
                        -1.0 - k as f64
                    } else {
                        5.0
                    },
                )
            })
            .collect();
        let w = outage_windows(&seq, 10.0, 20.0);
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].k0, w[0].k1, w[0].loss_steps), (3, 6, 3));
        assert_eq!(w[0].worst_k, 6);
        assert_eq!(w[0].outage_s(10.0, 20.0), 60.0);
        assert_eq!((w[1].k0, w[1].k1), (20, 20));
        // Without reacquisition the gap splits the first window.
        assert_eq!(outage_windows(&seq, 10.0, 0.0).len(), 3);
        assert!(outage_windows(&[(0, 0.0), (1, 3.0)], 10.0, 20.0).is_empty());
    }

    #[test]
    fn antenna_pattern_interpolates_and_is_checked() {
        let l = LinkModel::default();
        assert_eq!(l.gain_dbi(0.0), 3.0);
        assert_eq!(l.gain_dbi(85.0), -5.5);
        assert_eq!(l.gain_dbi(180.0), -22.0);
        assert!(l.check().is_ok());
        let bad_order = LinkModel {
            antenna_pattern: vec![[90.0, 0.0], [10.0, 1.0]],
            ..LinkModel::default()
        };
        assert!(bad_order.check().is_err());
        let one = LinkModel {
            antenna_pattern: vec![[0.0, 0.0]],
            ..LinkModel::default()
        };
        assert!(one.check().is_err());
    }

    #[test]
    fn polygon_regions_contain_across_the_antimeridian() {
        let p = Area::new(
            "p".into(),
            "t".into(),
            0.0,
            180.0,
            Shape::Polygon {
                ring: vec![(-5.0, 175.0), (-5.0, -175.0), (5.0, -175.0), (5.0, 175.0)],
            },
            None,
            0.0,
        );
        assert!(p.contains(0.0, 179.0));
        assert!(p.contains(0.0, -178.0));
        assert!(!p.contains(0.0, 170.0));
        assert!(!p.contains(6.0, 180.0));
    }

    #[test]
    fn track_wrapping_marks_the_antimeridian() {
        let t = vec![(0.0, 1.0, 179.0, 5e5), (10.0, 2.0, 181.0, 5e5)];
        let w = wrap_track(&t);
        assert_eq!(w.len(), 4);
        assert_eq!(w[1], (5.0, 1.5, 180.0, 5e5));
        assert_eq!(w[2], (5.001, 1.5, -180.0, 5e5));
        assert_eq!(w[3].2, -179.0);
        let back = vec![(0.0, 0.0, -179.0, 0.0), (10.0, 0.0, -181.0, 0.0)];
        let w = wrap_track(&back);
        assert_eq!((w[1].2, w[2].2, w[3].2), (-180.0, 180.0, 179.0));
        assert_eq!(slug("GRACE-FO 1 (x)"), "grace-fo-1-x");
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
