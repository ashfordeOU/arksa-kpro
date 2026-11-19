// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The campaign file: the system under test, a synthetic vertical descent, the descent
//! sensors, the pre-registered pass bars and the fault scenarios.
//!
//! The descent is one-dimensional on purpose. The harness verifies how a navigation filter
//! handles its altitude sensors (detection, false rejection, recovery, the consistency of
//! its stated uncertainty); it does not model the lander's guidance, its attitude or the
//! terrain map, and it contains no navigation algorithm. Terrain appears only as a scalar
//! height under the sensor footprint over time (a crater rim, a ridge), which changes
//! what a healthy altimeter reads.

use serde::{Deserialize, Serialize};

use super::err;
use super::faults::{Expect, FaultType};
use kshana::api::KshanaError;

/// Identifies the campaign file.
pub const SCHEMA: &str = "arksa-landing-nav-campaign/1";

/// The whole campaign file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Campaign {
    pub schema: String,
    pub title: String,
    /// Seeds every scenario's measurement noise (with the scenario id), so the streams are
    /// the same on every run.
    pub seed: u64,
    pub system_under_test: SystemUnderTest,
    pub descent: Descent,
    #[serde(rename = "sensor")]
    pub sensors: Vec<Sensor>,
    pub bars: Bars,
    #[serde(rename = "scenario")]
    pub scenarios: Vec<Scenario>,
}

/// What is being verified, as the provider declares it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemUnderTest {
    pub name: String,
    pub version: String,
    /// How the logs were produced: `SIL` (software in the loop), `HIL` (hardware in the
    /// loop) or `replay`. Declared by the provider, not checked.
    pub test_setup: String,
    #[serde(default)]
    pub provider: Option<String>,
}

/// The synthetic descent: constant-deceleration braking from the start state to the
/// terminal state, then a constant-rate terminal descent to touchdown (altitude 0 above the
/// landing-site datum).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descent {
    pub start_alt_m: f64,
    /// Vertical speed at the start, negative downwards.
    pub start_vz_mps: f64,
    pub terminal_alt_m: f64,
    /// Vertical speed of the terminal descent, negative downwards.
    pub terminal_vz_mps: f64,
    /// Sample period of the generated streams.
    pub step_s: f64,
}

/// The kind of descent sensor (informational; both measure the height above the terrain
/// under the sensor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    RadarAltimeter,
    LaserRangefinder,
}

/// One descent sensor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sensor {
    /// Lower-case letters, digits and `_`; names the log columns `<id>_health`, `<id>_used`.
    pub id: String,
    pub kind: SensorKind,
    /// Noise floor (1 sigma).
    pub noise_sigma_m: f64,
    /// Range-proportional noise (1 sigma, fraction of range), root-sum-squared with the floor.
    #[serde(default)]
    pub noise_fraction: f64,
    pub range_min_m: f64,
    pub range_max_m: f64,
    pub rate_hz: f64,
}

impl Sensor {
    /// The 1-sigma noise at height above terrain `r`.
    pub fn sigma(&self, r: f64) -> f64 {
        (self.noise_sigma_m.powi(2) + (self.noise_fraction * r).powi(2)).sqrt()
    }
    /// Whether a healthy sensor returns a measurement at height above terrain `r`.
    pub fn in_range(&self, r: f64) -> bool {
        r >= self.range_min_m && r <= self.range_max_m
    }
}

/// The pass bars, fixed before any log is seen (pre-registered).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bars {
    /// A fault that must be detected must be flagged (sensor declared faulty or no longer
    /// used) within this time of its onset.
    pub detect_latency_s: f64,
    /// After a transient fault ends, the sensor must be used again within this time.
    pub recovery_s: f64,
    /// Total time a truthful sensor may be declared faulty or left unused, per sensor and
    /// scenario. Over this, the filter rejected good data (the false fault declaration of
    /// ispace Mission 1).
    pub false_reject_max_s: f64,
    /// A sensor error at least this large must be detected (bias, drift, stuck,
    /// saturation); the detection clock starts when the error first reaches it.
    pub min_detectable_error_m: f64,
    /// The consistency bound: |altitude error| <= sigma_k x the filter's stated sigma.
    pub sigma_k: f64,
    /// The fraction of epochs that must be inside the consistency bound.
    pub consistency_min_fraction: f64,
    /// Below this true altitude the low-altitude bars apply.
    pub low_alt_m: f64,
    pub low_alt_error_max_m: f64,
    pub low_alt_vz_error_max_mps: f64,
    /// The largest gap between logged epochs (and at the start and touchdown) before the
    /// log is incomplete.
    pub max_gap_s: f64,
}

/// A scalar terrain height under the sensor footprint over a time window (a crater rim, a
/// ridge): a healthy sensor reads `altitude - height_m` while it is active.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerrainStep {
    pub t_start_s: f64,
    pub t_end_s: f64,
    pub height_m: f64,
}

/// One injected sensor fault.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fault {
    pub sensor: String,
    #[serde(rename = "type")]
    pub fault_type: FaultType,
    #[serde(default)]
    pub t_start_s: f64,
    /// End of the fault; none means until touchdown.
    #[serde(default)]
    pub t_end_s: Option<f64>,
    /// Bias (m), drift rate (m/s), spike size (m), extra noise (m) or saturation ceiling
    /// (m), by type.
    #[serde(default)]
    pub value: Option<f64>,
    /// Spike probability per sample.
    #[serde(default)]
    pub probability: Option<f64>,
    /// Override the type's default expectation.
    #[serde(default)]
    pub expect: Option<Expect>,
    /// Override whether the sensor must be used again after the fault ends.
    #[serde(default)]
    pub recover: Option<bool>,
}

impl Fault {
    pub fn expect(&self) -> Expect {
        self.expect.unwrap_or(self.fault_type.default_expect())
    }
    pub fn end(&self, touchdown_s: f64) -> f64 {
        self.t_end_s.unwrap_or(touchdown_s).min(touchdown_s)
    }
    /// Whether the sensor must be used again after the fault (only a fault that ends
    /// before touchdown can require it).
    pub fn recover(&self, touchdown_s: f64) -> bool {
        let ends = matches!(self.t_end_s, Some(e) if e < touchdown_s);
        ends && self.recover.unwrap_or(self.fault_type.default_recover())
    }
    pub fn active(&self, t: f64, touchdown_s: f64) -> bool {
        if self.fault_type == FaultType::NeverEnabled {
            return true;
        }
        t >= self.t_start_s && t < self.end(touchdown_s)
    }
}

/// One scenario: terrain and faults over the common descent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Letters, digits, `-` and `_`; names the stream and log files.
    pub id: String,
    pub title: String,
    /// The public incident this scenario stands for, if any (see `landing-nav library`).
    #[serde(default)]
    pub incident: Option<String>,
    #[serde(default)]
    pub terrain: Vec<TerrainStep>,
    #[serde(default, rename = "fault")]
    pub faults: Vec<Fault>,
}

impl Scenario {
    /// The terrain height under the sensor at `t`.
    pub fn terrain_at(&self, t: f64) -> f64 {
        self.terrain
            .iter()
            .filter(|s| t >= s.t_start_s && t < s.t_end_s)
            .map(|s| s.height_m)
            .sum()
    }
}

/// The true vertical state at one time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Truth {
    pub alt_m: f64,
    pub vz_mps: f64,
    pub az_mps2: f64,
}

impl Descent {
    fn speeds(&self) -> (f64, f64) {
        (-self.start_vz_mps, -self.terminal_vz_mps)
    }
    /// The braking deceleration (m/s^2, upwards).
    pub fn decel(&self) -> f64 {
        let (s0, st) = self.speeds();
        (s0 * s0 - st * st) / (2.0 * (self.start_alt_m - self.terminal_alt_m))
    }
    /// The end of the braking phase.
    pub fn braking_end_s(&self) -> f64 {
        let (s0, st) = self.speeds();
        let d = self.decel();
        if d > 0.0 {
            (s0 - st) / d
        } else {
            (self.start_alt_m - self.terminal_alt_m) / s0
        }
    }
    /// The touchdown time.
    pub fn touchdown_s(&self) -> f64 {
        self.braking_end_s() + self.terminal_alt_m / (-self.terminal_vz_mps)
    }
    /// The true state at `t` (clamped to the descent).
    pub fn truth(&self, t: f64) -> Truth {
        let (s0, st) = self.speeds();
        let t = t.clamp(0.0, self.touchdown_s());
        let t1 = self.braking_end_s();
        if t <= t1 {
            let d = self.decel();
            Truth {
                alt_m: self.start_alt_m - s0 * t + 0.5 * d * t * t,
                vz_mps: -(s0 - d * t),
                az_mps2: d,
            }
        } else {
            Truth {
                alt_m: (self.terminal_alt_m - st * (t - t1)).max(0.0),
                vz_mps: -st,
                az_mps2: 0.0,
            }
        }
    }
}

fn valid_id(s: &str, extra: &[char]) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || extra.contains(&c))
}

fn finite_pos(name: &str, v: f64) -> Result<(), KshanaError> {
    if v.is_finite() && v > 0.0 {
        Ok(())
    } else {
        Err(err(format!("{name} must be a positive number (got {v})")))
    }
}

impl Campaign {
    /// Parse and check a campaign file.
    pub fn parse(text: &str) -> Result<Campaign, KshanaError> {
        let c: Campaign = toml::from_str(text).map_err(|e| err(format!("campaign: {e}")))?;
        c.check()?;
        Ok(c)
    }

    /// The sensor with this id.
    pub fn sensor(&self, id: &str) -> Option<&Sensor> {
        self.sensors.iter().find(|s| s.id == id)
    }

    fn check(&self) -> Result<(), KshanaError> {
        if self.schema != SCHEMA {
            return Err(err(format!(
                "campaign schema must be \"{SCHEMA}\" (got \"{}\")",
                self.schema
            )));
        }
        if !matches!(
            self.system_under_test.test_setup.as_str(),
            "SIL" | "HIL" | "replay"
        ) {
            return Err(err(
                "system_under_test.test_setup must be SIL, HIL or replay",
            ));
        }
        let d = &self.descent;
        finite_pos("descent.start_alt_m", d.start_alt_m)?;
        finite_pos("descent.terminal_alt_m", d.terminal_alt_m)?;
        finite_pos("descent.step_s", d.step_s)?;
        finite_pos("descent.start_vz_mps (downwards, negated)", -d.start_vz_mps)?;
        finite_pos(
            "descent.terminal_vz_mps (downwards, negated)",
            -d.terminal_vz_mps,
        )?;
        if d.terminal_alt_m >= d.start_alt_m {
            return Err(err("descent.terminal_alt_m must be below start_alt_m"));
        }
        if -d.terminal_vz_mps > -d.start_vz_mps {
            return Err(err(
                "descent.terminal_vz_mps must not be faster than start_vz_mps (braking only)",
            ));
        }
        let steps = d.touchdown_s() / d.step_s;
        if steps > 2_000_000.0 {
            return Err(err("descent: more than 2,000,000 samples; raise step_s"));
        }
        if self.sensors.is_empty() {
            return Err(err("the campaign needs at least one [[sensor]]"));
        }
        for (i, s) in self.sensors.iter().enumerate() {
            if !valid_id(&s.id, &[]) {
                return Err(err(format!(
                    "sensor id '{}' must be lower-case letters, digits and _",
                    s.id
                )));
            }
            if self.sensors[..i].iter().any(|o| o.id == s.id) {
                return Err(err(format!("sensor id '{}' is listed twice", s.id)));
            }
            finite_pos(&format!("sensor {} noise_sigma_m", s.id), s.noise_sigma_m)?;
            finite_pos(&format!("sensor {} range_max_m", s.id), s.range_max_m)?;
            finite_pos(&format!("sensor {} rate_hz", s.id), s.rate_hz)?;
            if !(s.noise_fraction.is_finite() && s.noise_fraction >= 0.0) {
                return Err(err(format!("sensor {} noise_fraction must be >= 0", s.id)));
            }
            if !(s.range_min_m >= 0.0 && s.range_min_m < s.range_max_m) {
                return Err(err(format!(
                    "sensor {}: range_min_m must be >= 0 and below range_max_m",
                    s.id
                )));
            }
            let every = 1.0 / (s.rate_hz * d.step_s);
            if every < 1.0 - 1e-9 || (every - every.round()).abs() > 1e-6 {
                return Err(err(format!(
                    "sensor {}: 1 / (rate_hz x step_s) must be a whole number >= 1",
                    s.id
                )));
            }
        }
        let b = &self.bars;
        for (n, v) in [
            ("detect_latency_s", b.detect_latency_s),
            ("recovery_s", b.recovery_s),
            ("min_detectable_error_m", b.min_detectable_error_m),
            ("sigma_k", b.sigma_k),
            ("low_alt_m", b.low_alt_m),
            ("low_alt_error_max_m", b.low_alt_error_max_m),
            ("low_alt_vz_error_max_mps", b.low_alt_vz_error_max_mps),
            ("max_gap_s", b.max_gap_s),
        ] {
            finite_pos(&format!("bars.{n}"), v)?;
        }
        if !(b.false_reject_max_s.is_finite() && b.false_reject_max_s >= 0.0) {
            return Err(err("bars.false_reject_max_s must be >= 0"));
        }
        if !(b.consistency_min_fraction > 0.0 && b.consistency_min_fraction <= 1.0) {
            return Err(err("bars.consistency_min_fraction must be in (0, 1]"));
        }
        if self.scenarios.is_empty() {
            return Err(err("the campaign needs at least one [[scenario]]"));
        }
        let td = d.touchdown_s();
        for (i, sc) in self.scenarios.iter().enumerate() {
            if !valid_id(&sc.id.to_ascii_lowercase(), &['-']) {
                return Err(err(format!(
                    "scenario id '{}' must be letters, digits, - and _",
                    sc.id
                )));
            }
            if self.scenarios[..i]
                .iter()
                .any(|o| o.id.eq_ignore_ascii_case(&sc.id))
            {
                return Err(err(format!("scenario id '{}' is listed twice", sc.id)));
            }
            for t in &sc.terrain {
                if !(t.t_start_s.is_finite()
                    && t.t_end_s.is_finite()
                    && t.height_m.is_finite()
                    && t.t_start_s >= 0.0
                    && t.t_end_s > t.t_start_s)
                {
                    return Err(err(format!(
                        "scenario {}: a terrain step needs 0 <= t_start_s < t_end_s and a finite height_m",
                        sc.id
                    )));
                }
            }
            for f in &sc.faults {
                let at = |m: &str| err(format!("scenario {} fault {:?}: {m}", sc.id, f.fault_type));
                if self.sensor(&f.sensor).is_none() {
                    return Err(at(&format!("no sensor '{}'", f.sensor)));
                }
                if !(f.t_start_s.is_finite() && f.t_start_s >= 0.0 && f.t_start_s < td) {
                    return Err(at("t_start_s must be >= 0 and before touchdown"));
                }
                if let Some(e) = f.t_end_s {
                    if !(e.is_finite() && e > f.t_start_s) {
                        return Err(at("t_end_s must be after t_start_s"));
                    }
                }
                f.fault_type.check_params(f).map_err(|m| at(&m))?;
            }
        }
        Ok(())
    }
}
