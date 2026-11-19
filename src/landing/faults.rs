// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The descent-sensor fault library and the generator of the synthetic sensor streams a
//! provider feeds to its navigation filter.
//!
//! Every fault acts on the measurement a sensor reports (or on whether it reports at
//! all). Nothing here estimates a lander's state: the generator only corrupts a known true
//! height above terrain, and the verification ([`super::verify`]) compares the filter's
//! logged outputs with that truth.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::campaign::{Campaign, Fault, Scenario, Sensor};

/// The injected fault types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultType {
    /// No measurement at all over the window (power loss, receiver drop, loss of lock).
    Dropout,
    /// The sensor never reports during the descent (not armed, safety switch not enabled).
    NeverEnabled,
    /// A constant offset (`value`, m).
    Bias,
    /// An offset that grows from zero at `value` m/s.
    Drift,
    /// The reading freezes at its value at the fault start and stays flagged valid.
    Stuck,
    /// The reading is clipped at a ceiling (`value`, m) while still flagged valid.
    Saturation,
    /// Occasional outliers of size `value` m with `probability` per sample.
    Spikes,
    /// Extra noise of `value` m (1 sigma), e.g. dust or plume returns near touchdown.
    NoiseBurst,
}

/// What the filter must do about a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// Declare the sensor faulty or stop using it within the detection bar.
    Detect,
    /// No detection is required; the estimate must stay consistent and accurate.
    Either,
}

impl FaultType {
    pub const ALL: [FaultType; 8] = [
        FaultType::Dropout,
        FaultType::NeverEnabled,
        FaultType::Bias,
        FaultType::Drift,
        FaultType::Stuck,
        FaultType::Saturation,
        FaultType::Spikes,
        FaultType::NoiseBurst,
    ];

    pub fn name(self) -> &'static str {
        match self {
            FaultType::Dropout => "dropout",
            FaultType::NeverEnabled => "never_enabled",
            FaultType::Bias => "bias",
            FaultType::Drift => "drift",
            FaultType::Stuck => "stuck",
            FaultType::Saturation => "saturation",
            FaultType::Spikes => "spikes",
            FaultType::NoiseBurst => "noise_burst",
        }
    }

    pub fn default_expect(self) -> Expect {
        match self {
            FaultType::Spikes | FaultType::NoiseBurst => Expect::Either,
            _ => Expect::Detect,
        }
    }

    /// Whether, by default, the sensor must be used again after the fault ends.
    pub fn default_recover(self) -> bool {
        matches!(
            self,
            FaultType::Dropout | FaultType::Spikes | FaultType::NoiseBurst
        )
    }

    /// Whether the fault leaves the sensor reporting (so its error can be measured).
    pub fn reports(self) -> bool {
        !matches!(self, FaultType::Dropout | FaultType::NeverEnabled)
    }

    /// One line on what the fault is.
    pub fn describe(self) -> &'static str {
        match self {
            FaultType::Dropout => "no measurement over the window (power loss, receiver drop, loss of lock); the sensor reports invalid",
            FaultType::NeverEnabled => "the sensor never reports during the descent (not armed, enable flag or safety switch left off)",
            FaultType::Bias => "a constant offset of `value` m, reported as valid",
            FaultType::Drift => "an offset growing at `value` m/s from the fault start, reported as valid",
            FaultType::Stuck => "the reading freezes at its value at the fault start and stays flagged valid",
            FaultType::Saturation => "readings above a ceiling of `value` m are clipped to it and flagged valid",
            FaultType::Spikes => "outliers of +/- `value` m with `probability` per sample",
            FaultType::NoiseBurst => "extra noise of `value` m (1 sigma), as from dust or plume returns near touchdown",
        }
    }

    pub(super) fn check_params(self, f: &Fault) -> Result<(), String> {
        let v = f.value;
        let need = |what: &str| -> Result<f64, String> {
            match v {
                Some(x) if x.is_finite() => Ok(x),
                _ => Err(format!("needs a finite `value` ({what})")),
            }
        };
        match self {
            FaultType::Dropout | FaultType::Stuck => {
                if v.is_some() {
                    return Err("takes no `value`".into());
                }
            }
            FaultType::NeverEnabled => {
                if v.is_some() || f.t_start_s != 0.0 || f.t_end_s.is_some() {
                    return Err(
                        "covers the whole descent; give no value, t_start_s or t_end_s".into(),
                    );
                }
            }
            FaultType::Bias => {
                if need("bias, m")? == 0.0 {
                    return Err("a bias of 0 is no fault".into());
                }
            }
            FaultType::Drift => {
                if need("drift rate, m/s")? == 0.0 {
                    return Err("a drift of 0 is no fault".into());
                }
            }
            FaultType::Saturation => {
                if need("ceiling, m")? <= 0.0 {
                    return Err("the ceiling must be positive".into());
                }
            }
            FaultType::Spikes => {
                if need("spike size, m")? <= 0.0 {
                    return Err("the spike size must be positive".into());
                }
                match f.probability {
                    Some(p) if p > 0.0 && p <= 1.0 => {}
                    _ => return Err("needs a `probability` in (0, 1]".into()),
                }
            }
            FaultType::NoiseBurst => {
                if need("extra noise, m")? <= 0.0 {
                    return Err("the extra noise must be positive".into());
                }
            }
        }
        if self != FaultType::Spikes && f.probability.is_some() {
            return Err("only spikes take a `probability`".into());
        }
        Ok(())
    }
}

/// A public landing incident the library's faults stand for.
#[derive(Debug, Clone, Serialize)]
pub struct Incident {
    pub id: &'static str,
    pub mission: &'static str,
    pub date: &'static str,
    pub summary: &'static str,
    pub maps_to: &'static [&'static str],
    pub source: &'static str,
}

/// The public incidents (research note 13, finding 1; summaries of public reporting, not
/// of any accident investigation report).
pub const INCIDENTS: &[Incident] = &[
    Incident {
        id: "ispace-m1",
        mission: "ispace HAKUTO-R Mission 1",
        date: "April 2023",
        summary: "The onboard computer incorrectly presumed the radar altimeter was faulty after a large jump in measured altitude over terrain, and the lander ran out of propellant while hovering.",
        maps_to: &["terrain step with a truthful sensor (false fault declaration)"],
        source: "https://en.wikipedia.org/wiki/Hakuto-R_Mission_1 ; https://ispace-inc.com/news-en/?p=4655",
    },
    Incident {
        id: "im-1",
        mission: "Intuitive Machines IM-1",
        date: "February 2024",
        summary: "The primary laser rangefinders' safety switch was never enabled before launch; after a late switch to NASA's NDL the lander touched down faster than planned on a slope of about 12 degrees against a 10-degree design limit.",
        maps_to: &["never_enabled", "configuration check"],
        source: "https://en.wikipedia.org/wiki/IM-1",
    },
    Incident {
        id: "im-2",
        mission: "Intuitive Machines IM-2",
        date: "March 2025",
        summary: "The altimeter failed during landing, as it had on IM-1; the lander came to rest on its side in a crater.",
        maps_to: &["dropout", "stuck", "bias"],
        source: "https://en.wikipedia.org/wiki/IM-2",
    },
    Incident {
        id: "ispace-m2",
        mission: "ispace HAKUTO-R Mission 2",
        date: "June 2025",
        summary: "ispace's own analysis attributes the loss to a laser range finder anomaly.",
        maps_to: &["dropout", "drift", "saturation"],
        source: "https://en.wikipedia.org/wiki/Hakuto-R_Mission_2 ; https://arstechnica.com/space/2025/06/a-japanese-lander-crashed-on-the-moon-after-losing-track-of-its-location/",
    },
];

/// The library as Markdown (`landing-nav library`).
pub fn library_markdown() -> String {
    let mut s = String::from(
        "# Descent-sensor fault library\n\n\
         Faults act on what an altimeter or rangefinder reports. Each scenario may also add\n\
         terrain steps (a scalar height under the sensor, such as a crater rim), over which a\n\
         healthy sensor reads differently and must not be declared faulty.\n\n\
         | Type | What it is | Filter must, by default |\n|---|---|---|\n",
    );
    for t in FaultType::ALL {
        let must = match (t.default_expect(), t.default_recover()) {
            (Expect::Detect, true) => "detect, then use the sensor again after it ends",
            (Expect::Detect, false) => "detect",
            (Expect::Either, _) => {
                "keep the estimate consistent; use the sensor again after it ends"
            }
        };
        s.push_str(&format!(
            "| `{}` | {} | {} |\n",
            t.name(),
            t.describe(),
            must
        ));
    }
    s.push_str("\n## Public incidents the library stands for\n\n");
    for i in INCIDENTS {
        s.push_str(&format!(
            "* **{}** ({}, `{}`): {} Scenarios: {}. Source: {}\n",
            i.mission,
            i.date,
            i.id,
            i.summary,
            i.maps_to.join(", "),
            i.source
        ));
    }
    s.push_str(
        "\nSummaries of public reporting, not of accident investigation reports. The library is\n\
         generic: it names no lander's hardware and models no specific sensor.\n",
    );
    s
}

/// The library as JSON.
pub fn library_json() -> serde_json::Value {
    serde_json::json!({
        "fault_types": FaultType::ALL.iter().map(|t| serde_json::json!({
            "type": t.name(),
            "describe": t.describe(),
            "default_expect": t.default_expect(),
            "default_recover": t.default_recover(),
        })).collect::<Vec<_>>(),
        "incidents": INCIDENTS,
    })
}

/// The deterministic noise source of one scenario.
fn rng(seed: u64, scenario: &str) -> ChaCha8Rng {
    let h = Sha256::digest(scenario.as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&h[..8]);
    ChaCha8Rng::seed_from_u64(seed ^ u64::from_le_bytes(b))
}

/// A standard normal draw (Box-Muller).
fn normal(r: &mut ChaCha8Rng) -> f64 {
    let u1: f64 = 1.0 - r.gen::<f64>();
    let u2: f64 = r.gen::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// The sample times of the streams (0 to touchdown).
pub fn sample_times(c: &Campaign) -> Vec<f64> {
    let td = c.descent.touchdown_s();
    let n = (td / c.descent.step_s).floor() as usize;
    (0..=n).map(|k| k as f64 * c.descent.step_s).collect()
}

/// The true height above terrain under the sensor at `t`.
pub fn hat(c: &Campaign, sc: &Scenario, t: f64) -> f64 {
    c.descent.truth(t).alt_m - sc.terrain_at(t)
}

/// The first sample time at or after `from` at which `s` is in range, if any before `to`.
pub fn first_in_range(c: &Campaign, sc: &Scenario, s: &Sensor, from: f64, to: f64) -> Option<f64> {
    sample_times(c)
        .into_iter()
        .filter(|&t| t >= from - 1e-9 && t < to)
        .find(|&t| s.in_range(hat(c, sc, t)))
}

/// The systematic (noise-free) error a reporting fault adds at `t`, while it is active.
pub fn systematic_error(c: &Campaign, sc: &Scenario, f: &Fault, t: f64) -> f64 {
    let h = hat(c, sc, t);
    match f.fault_type {
        FaultType::Bias => f.value.unwrap_or(0.0),
        FaultType::Drift => f.value.unwrap_or(0.0) * (t - f.t_start_s).max(0.0),
        FaultType::Stuck => {
            let s = c.sensor(&f.sensor).expect("checked");
            let td = c.descent.touchdown_s();
            let t0 = first_in_range(c, sc, s, f.t_start_s, td).unwrap_or(f.t_start_s);
            hat(c, sc, t0) - h
        }
        FaultType::Saturation => {
            let ceil = f.value.unwrap_or(f64::INFINITY);
            if h > ceil {
                ceil - h
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

/// One generated scenario: stream and truth CSV text.
#[derive(Debug, Clone)]
pub struct Generated {
    pub stream_csv: String,
    pub truth_csv: String,
    pub samples: usize,
}

/// Generate a scenario's sensor stream (what the filter is fed) and its truth.
///
/// Stream columns: `t_s,accel_z_mps2,<id>_m,<id>_valid,...` (an empty `<id>_m` means no
/// sample at that time or no return). Truth columns: `t_s,alt_m,vz_mps,terrain_m`.
pub fn generate(c: &Campaign, sc: &Scenario) -> Generated {
    let mut r = rng(c.seed, &sc.id);
    let td = c.descent.touchdown_s();
    let times = sample_times(c);
    let mut stream = String::from("t_s,accel_z_mps2");
    for s in &c.sensors {
        stream.push_str(&format!(",{0}_m,{0}_valid", s.id));
    }
    stream.push('\n');
    let mut truth = String::from("t_s,alt_m,vz_mps,terrain_m\n");
    let every: Vec<usize> = c
        .sensors
        .iter()
        .map(|s| (1.0 / (s.rate_hz * c.descent.step_s)).round() as usize)
        .collect();
    // The frozen reading of each stuck fault (by fault index).
    let mut frozen: Vec<Option<f64>> = vec![None; sc.faults.len()];
    for (k, &t) in times.iter().enumerate() {
        let tr = c.descent.truth(t);
        let terrain = sc.terrain_at(t);
        truth.push_str(&format!(
            "{t:.3},{:.4},{:.4},{:.4}\n",
            tr.alt_m, tr.vz_mps, terrain
        ));
        stream.push_str(&format!("{t:.3},{:.5}", tr.az_mps2));
        let h = tr.alt_m - terrain;
        for (si, s) in c.sensors.iter().enumerate() {
            // Draw the noise every sample, so one fault never shifts another's noise.
            let n0 = normal(&mut r);
            let n1 = normal(&mut r);
            let u: f64 = r.gen();
            let sign = if r.gen::<bool>() { 1.0 } else { -1.0 };
            if k % every[si] != 0 {
                stream.push_str(",,");
                continue;
            }
            let mut valid = s.in_range(h);
            let mut m = h + s.sigma(h.max(0.0)) * n0;
            for (fi, f) in sc.faults.iter().enumerate() {
                if f.sensor != s.id || !f.active(t, td) {
                    continue;
                }
                match f.fault_type {
                    FaultType::Dropout | FaultType::NeverEnabled => valid = false,
                    FaultType::Bias | FaultType::Drift => m += systematic_error(c, sc, f, t),
                    FaultType::Saturation => {
                        if valid {
                            m = m.min(f.value.unwrap_or(f64::INFINITY));
                        }
                    }
                    FaultType::Stuck => {
                        if valid || frozen[fi].is_some() {
                            m = *frozen[fi].get_or_insert(m);
                            valid = true;
                        }
                    }
                    FaultType::Spikes => {
                        if u < f.probability.unwrap_or(0.0) {
                            m += sign * f.value.unwrap_or(0.0);
                        }
                    }
                    FaultType::NoiseBurst => m += f.value.unwrap_or(0.0) * n1,
                }
            }
            if valid {
                stream.push_str(&format!(",{:.4},1", m.max(0.0)));
            } else {
                stream.push_str(",,0");
            }
        }
        stream.push('\n');
    }
    Generated {
        stream_csv: stream,
        truth_csv: truth,
        samples: times.len(),
    }
}
