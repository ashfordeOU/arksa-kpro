// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The observation CSV: the operator's ground-station measurements of its own satellite.
//!
//! One measurement per row; a header row names the columns (any order; unknown columns are
//! refused so a misspelt one is not silently ignored); `#` starts a comment line.
//!
//! | column | required | meaning |
//! |---|---|---|
//! | `time_utc` | yes | UTC time of the measurement, `YYYY-MM-DDThh:mm:ss[.f]Z` (receive time) |
//! | `station` | yes | station name |
//! | `lat_deg`, `lon_deg`, `alt_m` | yes | station position, WGS-84 geodetic (height above the ellipsoid); the same on every row of a station |
//! | `type` | yes | `doppler_hz`, `range_m`, `range_rate_m_s`, `az_deg` or `el_deg` |
//! | `value` | yes | the measurement in the unit its type names |
//! | `sigma` | yes | its one-sigma uncertainty, same unit, > 0 |
//! | `carrier_hz` | for `doppler_hz` | the nominal downlink carrier the Doppler offset is measured from |
//! | `pass` | no | a pass label; empty: passes are split per station at gaps over 15 minutes |
//!
//! Conventions: `doppler_hz` is the received frequency minus the nominal carrier for a one-way
//! downlink, first order: `-carrier_hz * range_rate / c`. `range_m` is the one-way slant range
//! (a two-way ranging system reports half the round trip). `az_deg` is measured from north
//! through east; `el_deg` above the local horizontal of the WGS-84 ellipsoid. Light time,
//! refraction and the transmitter's frequency drift within a pass are not modelled; an unknown
//! constant frequency offset of the satellite's oscillator is estimated per pass (see
//! [`super::identify::Config::doppler_bias_per_pass`]).

use std::collections::BTreeMap;

use kshana::frames::{geodetic_to_ecef, Geodetic};
use kshana::jd2::Jd2;
use serde::Serialize;

/// Measurement type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObsType {
    DopplerHz,
    RangeM,
    RangeRateMS,
    AzDeg,
    ElDeg,
}

impl ObsType {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "doppler_hz" => Self::DopplerHz,
            "range_m" => Self::RangeM,
            "range_rate_m_s" => Self::RangeRateMS,
            "az_deg" => Self::AzDeg,
            "el_deg" => Self::ElDeg,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DopplerHz => "doppler_hz",
            Self::RangeM => "range_m",
            Self::RangeRateMS => "range_rate_m_s",
            Self::AzDeg => "az_deg",
            Self::ElDeg => "el_deg",
        }
    }
    /// The unit of the residuals.
    pub fn unit(self) -> &'static str {
        match self {
            Self::DopplerHz => "Hz",
            Self::RangeM => "m",
            Self::RangeRateMS => "m/s",
            Self::AzDeg | Self::ElDeg => "deg",
        }
    }
    pub const ALL: [ObsType; 5] = [
        Self::DopplerHz,
        Self::RangeM,
        Self::RangeRateMS,
        Self::AzDeg,
        Self::ElDeg,
    ];
}

/// A ground station.
#[derive(Debug, Clone, Serialize)]
pub struct Station {
    pub name: String,
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub alt_m: f64,
    /// ITRS position (m).
    #[serde(skip)]
    pub ecef: [f64; 3],
}

impl Station {
    pub fn new(name: &str, lat_deg: f64, lon_deg: f64, alt_m: f64) -> Self {
        let ecef = geodetic_to_ecef(Geodetic {
            lat_rad: lat_deg.to_radians(),
            lon_rad: lon_deg.to_radians(),
            alt_m,
        });
        Self {
            name: name.into(),
            lat_deg,
            lon_deg,
            alt_m,
            ecef,
        }
    }
}

/// One measurement.
#[derive(Debug, Clone)]
pub struct Observation {
    /// Row number in the file (1-based, header included).
    pub row: usize,
    pub time: Jd2,
    /// The time as written.
    pub time_utc: String,
    /// Index into [`ObservationSet::stations`].
    pub station: usize,
    pub kind: ObsType,
    pub value: f64,
    pub sigma: f64,
    pub carrier_hz: Option<f64>,
    /// Index into [`ObservationSet::passes`].
    pub pass: usize,
}

/// The observations of one file.
#[derive(Debug, Clone)]
pub struct ObservationSet {
    pub stations: Vec<Station>,
    /// Pass labels, `station/label`.
    pub passes: Vec<String>,
    pub obs: Vec<Observation>,
}

/// Gap (s) that starts a new automatic pass.
pub const PASS_GAP_S: f64 = 900.0;

const COLUMNS: &[&str] = &[
    "time_utc",
    "station",
    "lat_deg",
    "lon_deg",
    "alt_m",
    "type",
    "value",
    "sigma",
    "carrier_hz",
    "pass",
];

/// Parse the observation CSV.
pub fn parse_csv(text: &str) -> Result<ObservationSet, String> {
    let mut header: Option<Vec<String>> = None;
    let mut rows: Vec<(usize, Vec<String>)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cells: Vec<String> = line.split(',').map(|c| c.trim().to_string()).collect();
        if header.is_none() {
            for c in &cells {
                if !COLUMNS.contains(&c.as_str()) {
                    return Err(format!(
                        "observations: unknown column '{c}' (columns: {})",
                        COLUMNS.join(", ")
                    ));
                }
            }
            header = Some(cells);
        } else {
            rows.push((i + 1, cells));
        }
    }
    let header = header.ok_or("observations: no header row")?;
    let idx = |name: &str| header.iter().position(|h| h == name);
    for need in &COLUMNS[..8] {
        if idx(need).is_none() {
            return Err(format!("observations: missing column '{need}'"));
        }
    }
    let col = |cells: &Vec<String>, name: &str| -> String {
        idx(name)
            .and_then(|i| cells.get(i))
            .cloned()
            .unwrap_or_default()
    };
    let mut stations: Vec<Station> = Vec::new();
    let mut obs = Vec::new();
    let mut labels: Vec<Option<String>> = Vec::new();
    for (row, cells) in &rows {
        if cells.len() > header.len() {
            return Err(format!("observations row {row}: more cells than columns"));
        }
        let num = |name: &str| -> Result<f64, String> {
            let s = col(cells, name);
            s.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("observations row {row}: {name} '{s}' is not a number"))
        };
        let tstr = col(cells, "time_utc");
        let time = super::parse_utc(&tstr).map_err(|e| format!("observations row {row}: {e}"))?;
        let st_name = col(cells, "station");
        if st_name.is_empty() {
            return Err(format!("observations row {row}: empty station"));
        }
        let (lat, lon, alt) = (num("lat_deg")?, num("lon_deg")?, num("alt_m")?);
        if !(-90.0..=90.0).contains(&lat) || !(-360.0..=360.0).contains(&lon) {
            return Err(format!(
                "observations row {row}: station position out of range"
            ));
        }
        let si = match stations.iter().position(|s| s.name == st_name) {
            Some(i) => {
                let s = &stations[i];
                if (s.lat_deg - lat).abs() > 1e-7
                    || (s.lon_deg - lon).abs() > 1e-7
                    || (s.alt_m - alt).abs() > 1e-3
                {
                    return Err(format!(
                        "observations row {row}: station '{st_name}' has a different position from its earlier rows"
                    ));
                }
                i
            }
            None => {
                stations.push(Station::new(&st_name, lat, lon, alt));
                stations.len() - 1
            }
        };
        let ty = col(cells, "type");
        let kind = ObsType::parse(&ty).ok_or_else(|| {
            format!("observations row {row}: unknown type '{ty}' (doppler_hz, range_m, range_rate_m_s, az_deg, el_deg)")
        })?;
        let value = num("value")?;
        let sigma = num("sigma")?;
        if sigma <= 0.0 {
            return Err(format!("observations row {row}: sigma must be > 0"));
        }
        let carrier_hz = match col(cells, "carrier_hz").as_str() {
            "" => None,
            s => Some(
                s.parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .ok_or_else(|| {
                        format!("observations row {row}: carrier_hz '{s}' is not a positive number")
                    })?,
            ),
        };
        if kind == ObsType::DopplerHz && carrier_hz.is_none() {
            return Err(format!(
                "observations row {row}: a doppler_hz row needs carrier_hz"
            ));
        }
        let label = Some(col(cells, "pass")).filter(|s| !s.is_empty());
        labels.push(label);
        obs.push(Observation {
            row: *row,
            time,
            time_utc: tstr,
            station: si,
            kind,
            value,
            sigma,
            carrier_hz,
            pass: 0,
        });
    }
    if obs.is_empty() {
        return Err("observations: no measurement rows".into());
    }
    // Passes: labelled ones as given; the rest split per station at gaps.
    let mut pass_ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut passes: Vec<String> = Vec::new();
    let mut order: Vec<usize> = (0..obs.len()).collect();
    order.sort_by(|&a, &b| {
        obs[a]
            .station
            .cmp(&obs[b].station)
            .then(
                obs[a]
                    .time
                    .diff_seconds(obs[b].time)
                    .partial_cmp(&0.0)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.cmp(&b))
    });
    let mut auto_no: BTreeMap<usize, usize> = BTreeMap::new();
    let mut last: BTreeMap<usize, Jd2> = BTreeMap::new();
    for &i in &order {
        let st = obs[i].station;
        let key = match &labels[i] {
            Some(l) => format!("{}/{l}", stations[st].name),
            None => {
                let n = auto_no.entry(st).or_insert(0);
                if let Some(t0) = last.get(&st) {
                    if obs[i].time.diff_seconds(*t0) > PASS_GAP_S {
                        *n += 1;
                    }
                }
                last.insert(st, obs[i].time);
                format!("{}/auto-{}", stations[st].name, *n + 1)
            }
        };
        let id = *pass_ids.entry(key.clone()).or_insert_with(|| {
            passes.push(key);
            passes.len() - 1
        });
        obs[i].pass = id;
    }
    Ok(ObservationSet {
        stations,
        passes,
        obs,
    })
}

/// Write observations back as CSV in the documented format (for the synthetic generator).
pub fn to_csv(set: &ObservationSet, comment: &str) -> String {
    let mut s = String::new();
    for l in comment.lines() {
        s.push_str(&format!("# {l}\n"));
    }
    s.push_str("time_utc,station,lat_deg,lon_deg,alt_m,type,value,sigma,carrier_hz,pass\n");
    for o in &set.obs {
        let st = &set.stations[o.station];
        let pass = set.passes[o.pass]
            .split_once('/')
            .map(|(_, p)| p.to_string())
            .unwrap_or_default();
        s.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            o.time_utc,
            st.name,
            st.lat_deg,
            st.lon_deg,
            st.alt_m,
            o.kind.as_str(),
            fmt_num(o.value),
            fmt_num(o.sigma),
            o.carrier_hz.map(fmt_num).unwrap_or_default(),
            pass
        ));
    }
    s
}

fn fmt_num(x: f64) -> String {
    let r = super::round_sig(x);
    format!("{r}")
}
