// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! A synthetic rideshare deployment: N objects released close together on one orbit, their
//! catalogue element sets (TLE with Alpha-5 fields for numbers up to 339,999, and OMM for
//! every object, so a number above 339,999 travels in OMM only), and noisy ground-station
//! observations of one of them. Used by the tests and by `arksa-kpro rideshare-id synth` to
//! show the method on data whose answer is known. Everything here is MODELLED: the "truth" is
//! the target's own catalogue elements shifted along the track by `truth_offset_km`, propagated
//! with the same SGP4, so the result shows the method's logic, not its accuracy on real data.

use kshana::odm::OmmMessage;
use kshana::sgp4::teme_to_itrs_matrix;
use kshana::tle::Tle;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use super::catalogue::{self, Candidate};
use super::identify::{geometry, predicted, Geo};
use super::norad;
use super::observations::{self, ObsType, Observation, ObservationSet, Station};

/// Earth's gravitational parameter used to turn mean motion into a semi-major axis (WGS-72,
/// SGP4's constants), m^3/s^2.
const MU: f64 = 3.986_008e14;

/// A ground station of the synthetic campaign.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationSpec {
    pub name: String,
    pub lat_deg: f64,
    pub lon_deg: f64,
    #[serde(default)]
    pub alt_m: f64,
}

/// The deployment and the observation campaign.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DeploymentSpec {
    /// Number of objects released.
    pub objects: usize,
    /// Index (0-based) of the operator's object.
    pub target: usize,
    /// Catalogue number of object 0; the others follow in order.
    pub first_catalogue_number: u32,
    /// Explicit catalogue numbers (overrides `first_catalogue_number`).
    pub catalogue_numbers: Option<Vec<u32>>,
    /// International designator of the launch (`YYYY-NNN`; pieces A, B, ... are appended).
    pub launch: String,
    /// Element epoch (UTC).
    pub epoch_utc: String,
    pub inclination_deg: f64,
    pub mean_motion_rev_day: f64,
    pub eccentricity: f64,
    pub raan_deg: f64,
    pub arg_perigee_deg: f64,
    pub mean_anomaly_deg: f64,
    pub bstar: f64,
    /// Along-track spacing between consecutive objects at the epoch (km).
    pub spacing_km: f64,
    /// Along-track drift between consecutive objects per revolution (km), from the
    /// deployment velocity differences.
    pub drift_km_per_rev: f64,
    /// Explicit along-track offsets (km) per object (overrides `spacing_km`).
    pub along_track_km: Option<Vec<f64>>,
    /// Along-track error of the target's catalogue elements relative to the truth (km).
    pub truth_offset_km: f64,
    pub stations: Vec<StationSpec>,
    /// Observation window after the epoch (h).
    pub start_h: f64,
    pub duration_h: f64,
    pub step_s: f64,
    pub min_elevation_deg: f64,
    /// At most this many passes (all stations together).
    pub max_passes: usize,
    pub types: Vec<String>,
    pub carrier_hz: f64,
    /// Constant frequency offset of the target's oscillator (Hz), unknown to the matcher.
    pub oscillator_offset_hz: f64,
    pub sigma_doppler_hz: f64,
    pub sigma_range_m: f64,
    pub sigma_range_rate_m_s: f64,
    pub sigma_angle_deg: f64,
    pub seed: u64,
}

impl Default for DeploymentSpec {
    fn default() -> Self {
        Self {
            objects: 12,
            target: 5,
            first_catalogue_number: 101_388,
            catalogue_numbers: None,
            launch: "2026-900".into(),
            epoch_utc: "2026-11-15T06:00:00Z".into(),
            inclination_deg: 97.45,
            mean_motion_rev_day: 15.2,
            eccentricity: 0.0012,
            raan_deg: 40.0,
            arg_perigee_deg: 90.0,
            mean_anomaly_deg: 10.0,
            bstar: 1.0e-4,
            spacing_km: 8.0,
            drift_km_per_rev: 0.5,
            along_track_km: None,
            truth_offset_km: 0.3,
            stations: vec![StationSpec {
                name: "GS-NORTH".into(),
                lat_deg: 59.4,
                lon_deg: 24.7,
                alt_m: 40.0,
            }],
            start_h: 1.0,
            duration_h: 14.0,
            step_s: 10.0,
            min_elevation_deg: 10.0,
            max_passes: 3,
            types: vec!["doppler_hz".into()],
            carrier_hz: 437.5e6,
            oscillator_offset_hz: 1_250.0,
            sigma_doppler_hz: 20.0,
            sigma_range_m: 50.0,
            sigma_range_rate_m_s: 1.0,
            sigma_angle_deg: 0.1,
            seed: 1,
        }
    }
}

/// The generated deployment.
#[derive(Debug, Clone)]
pub struct Deployment {
    /// TLE text (objects up to 339,999; Alpha-5 above 99,999).
    pub tle_text: String,
    /// OMM KVN text (every object).
    pub omm_text: String,
    /// The objects that could not be written as TLE (above 339,999).
    pub omm_only: Vec<u32>,
    pub catalogue_numbers: Vec<u32>,
    pub target_catalogue_number: u32,
    /// Observation CSV (the documented format).
    pub observations_csv: String,
    pub observations: ObservationSet,
}

fn gauss(rng: &mut ChaCha8Rng) -> f64 {
    let u1: f64 = rng.gen_range(f64::MIN_POSITIVE..1.0);
    let u2: f64 = rng.gen::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

fn piece(k: usize) -> String {
    // A..Z without I and O, then AA, AB, ...
    const L: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    if k < L.len() {
        (L[k] as char).to_string()
    } else {
        format!(
            "{}{}",
            L[(k / L.len() - 1) % L.len()] as char,
            L[k % L.len()] as char
        )
    }
}

/// Generate the deployment.
pub fn generate(spec: &DeploymentSpec) -> Result<Deployment, String> {
    if spec.objects == 0 || spec.target >= spec.objects {
        return Err("synthetic: target must be one of the objects".into());
    }
    if spec.stations.is_empty() || spec.step_s <= 0.0 || spec.duration_h <= 0.0 {
        return Err("synthetic: needs a station, a positive step and a positive duration".into());
    }
    let cats: Vec<u32> = match &spec.catalogue_numbers {
        Some(v) if v.len() == spec.objects => v.clone(),
        Some(_) => return Err("synthetic: catalogue_numbers must list every object".into()),
        None => (0..spec.objects as u32)
            .map(|k| spec.first_catalogue_number + k)
            .collect(),
    };
    let epoch = super::parse_utc(&spec.epoch_utc).map_err(|e| e.to_string())?;
    let epoch_days_1950 =
        epoch.diff_seconds(kshana::jd2::Jd2::from_parts(2_433_281.0, 0.5)) / 86_400.0;
    let n_rad_s = spec.mean_motion_rev_day * std::f64::consts::TAU / 86_400.0;
    let a = (MU / (n_rad_s * n_rad_s)).cbrt();
    let offsets: Vec<f64> = match &spec.along_track_km {
        Some(v) if v.len() == spec.objects => v.clone(),
        Some(_) => return Err("synthetic: along_track_km must list every object".into()),
        None => (0..spec.objects)
            .map(|k| k as f64 * spec.spacing_km)
            .collect(),
    };
    let mid = (spec.objects as f64 - 1.0) / 2.0;
    let mut tle_text = String::new();
    let mut omm_text = String::new();
    let mut omm_only = Vec::new();
    for k in 0..spec.objects {
        // Drift per revolution d (m) = a * 2 pi * dn / n.
        let dn_rev_day = (k as f64 - mid) * spec.drift_km_per_rev * 1e3
            / (a * std::f64::consts::TAU)
            * spec.mean_motion_rev_day;
        let el = Tle {
            epoch_days_1950,
            bstar: spec.bstar,
            ecco: spec.eccentricity,
            argpo_rad: spec.arg_perigee_deg.to_radians(),
            inclo_rad: spec.inclination_deg.to_radians(),
            mo_rad: (spec.mean_anomaly_deg.to_radians() + offsets[k] * 1e3 / a)
                .rem_euclid(std::f64::consts::TAU),
            no_kozai_rad_min: (spec.mean_motion_rev_day + dn_rev_day) * std::f64::consts::TAU
                / 1440.0,
            nodeo_rad: spec.raan_deg.to_radians(),
        };
        let name = format!("RIDESHARE OBJ {}", piece(k));
        let des = format!("{}{}", spec.launch, piece(k));
        if cats[k] <= norad::ALPHA5_MAX {
            let (l1, l2) = catalogue::format_tle(cats[k], &des, &el)?;
            tle_text.push_str(&format!("{name}\n{l1}\n{l2}\n"));
        } else {
            omm_only.push(cats[k]);
        }
        omm_text.push_str(&catalogue::format_omm_kvn(
            cats[k],
            &name,
            &des,
            &el,
            "2026-11-15T00:00:00",
        )?);
    }
    // The candidates as an operator reads them; the truth is the target's.
    let mut all = Vec::new();
    if !tle_text.is_empty() {
        all.extend(catalogue::from_tle_text(&tle_text, "synthetic.tle")?);
    }
    all.extend(catalogue::from_omm_text(&omm_text, "synthetic.omm")?);
    let (cands, _) = catalogue::merge(all);
    let target = cands
        .iter()
        .find(|c| c.catalogue_number == cats[spec.target])
        .ok_or("synthetic: target missing")?;
    let mut truth_el = target.elements;
    truth_el.mo_rad =
        (truth_el.mo_rad + spec.truth_offset_km * 1e3 / a).rem_euclid(std::f64::consts::TAU);
    let truth = Candidate::new(
        0,
        "TRUTH".into(),
        String::new(),
        "tle",
        "truth".into(),
        true,
        truth_el,
    );
    let types: Vec<ObsType> = spec
        .types
        .iter()
        .map(|t| ObsType::parse(t).ok_or_else(|| format!("synthetic: unknown type '{t}'")))
        .collect::<Result<_, _>>()?;
    let stations: Vec<Station> = spec
        .stations
        .iter()
        .map(|s| Station::new(&s.name, s.lat_deg, s.lon_deg, s.alt_m))
        .collect();
    let mut rng = ChaCha8Rng::seed_from_u64(spec.seed);
    let mut obs: Vec<Observation> = Vec::new();
    let mut passes: Vec<String> = Vec::new();
    let mut in_pass: Vec<Option<usize>> = vec![None; stations.len()];
    let mut pass_no = vec![0usize; stations.len()];
    let steps = (spec.duration_h * 3600.0 / spec.step_s).floor() as usize;
    'time: for s in 0..=steps {
        let t = epoch.add_seconds(spec.start_h * 3600.0 + s as f64 * spec.step_s);
        let m = teme_to_itrs_matrix(t);
        let (r, v) = truth.orbit.itrs_state_with(t, &m)?;
        for (si, st) in stations.iter().enumerate() {
            let g: Geo = geometry(r, v, st);
            if g.el_deg < spec.min_elevation_deg {
                in_pass[si] = None;
                continue;
            }
            let pid = match in_pass[si] {
                Some(p) => p,
                None => {
                    if passes.len() >= spec.max_passes {
                        break 'time;
                    }
                    pass_no[si] += 1;
                    passes.push(format!("{}/p{}", st.name, pass_no[si]));
                    in_pass[si] = Some(passes.len() - 1);
                    passes.len() - 1
                }
            };
            for &kind in &types {
                let (sigma, carrier) = match kind {
                    ObsType::DopplerHz => (spec.sigma_doppler_hz, Some(spec.carrier_hz)),
                    ObsType::RangeM => (spec.sigma_range_m, None),
                    ObsType::RangeRateMS => (spec.sigma_range_rate_m_s, None),
                    ObsType::AzDeg | ObsType::ElDeg => (spec.sigma_angle_deg, None),
                };
                let mut o = Observation {
                    row: 0,
                    time: t,
                    time_utc: super::fmt_utc(t),
                    station: si,
                    kind,
                    value: 0.0,
                    sigma,
                    carrier_hz: carrier,
                    pass: pid,
                };
                let mut val = predicted(&o, &g) + sigma * gauss(&mut rng);
                if kind == ObsType::DopplerHz {
                    val += spec.oscillator_offset_hz;
                }
                if kind == ObsType::AzDeg {
                    val = val.rem_euclid(360.0);
                }
                o.value = val;
                obs.push(o);
            }
        }
    }
    if obs.is_empty() {
        return Err("synthetic: the target is never above the elevation mask in the window".into());
    }
    let set = ObservationSet {
        stations,
        passes,
        obs,
    };
    let csv = observations::to_csv(
        &set,
        &format!(
            "SYNTHETIC observations of object {} (MODELLED; generated by arksa-kpro rideshare-id synth, seed {})",
            cats[spec.target], spec.seed
        ),
    );
    // Re-read, so the set is exactly what a user's file gives.
    let observations = observations::parse_csv(&csv)?;
    Ok(Deployment {
        tle_text,
        omm_text,
        omm_only,
        catalogue_numbers: cats.clone(),
        target_catalogue_number: cats[spec.target],
        observations_csv: csv,
        observations,
    })
}

/// The OMM of a deployment as NDM/XML messages (to exercise the XML path).
pub fn omm_xml(omm_kvn: &str) -> Result<String, String> {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ndm>\n");
    for chunk in omm_kvn
        .split("CCSDS_OMM_VERS")
        .filter(|c| !c.trim().is_empty())
    {
        let m = OmmMessage::parse(&format!("CCSDS_OMM_VERS{chunk}"))?;
        let x = m.to_xml()?;
        let body = x
            .trim_start()
            .strip_prefix("<?xml version=\"1.0\" encoding=\"UTF-8\"?>")
            .unwrap_or(&x)
            .trim();
        out.push_str(body);
        out.push('\n');
    }
    out.push_str("</ndm>\n");
    Ok(out)
}
