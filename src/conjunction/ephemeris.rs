// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Ephemeris ingest for the covariance-realism tests and object classes.
//!
//! * OEM (CCSDS 502.0-B-3, KVN): the states through the open engine's reader
//!   ([`kshana::oem::parse_oem`]), and every `COVARIANCE_START … COVARIANCE_STOP` block of each
//!   segment here (the open reader skips covariance; one block may hold many matrices, each
//!   with its own `EPOCH` and optional `COV_REF_FRAME`). Units: km and km/s to m and m/s;
//!   km², km²/s and km²/s² to m², m²/s and m²/s².
//! * OMM (KVN or XML, through [`kshana::odm::OmmMessage`]): the object id and its orbit class
//!   from the mean elements.

use kshana::api::KshanaError;

use super::{bad, M6, MU_EARTH};

/// One object's ephemeris segment with its covariances.
#[derive(Debug, Clone)]
pub struct Ephemeris {
    pub object_id: String,
    pub object_name: String,
    pub ref_frame: String,
    pub time_system: String,
    /// `(t, [x, y, z, vx, vy, vz])`, t as calendar seconds from 2000-01-01 (m, m/s).
    pub states: Vec<(f64, [f64; 6])>,
    /// `(t, frame, covariance)` (m-based units).
    pub covariances: Vec<(f64, String, M6)>,
}

fn epoch_s(e: &kshana::rinex::EpochUtc) -> f64 {
    super::parse_utc(&format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:00",
        e.year, e.month, e.day, e.hour, e.minute
    ))
    .unwrap_or(f64::NAN)
        + e.second
}

/// Parse every segment of an OEM.
pub fn parse_oem(text: &str) -> Result<Vec<Ephemeris>, KshanaError> {
    let f = kshana::oem::parse_oem(text).map_err(|e| bad(format!("OEM: {e}")))?;
    let covs = covariance_blocks(text)?;
    if covs.len() != f.segments.len() {
        return Err(bad(format!(
            "OEM: {} segments but {} metadata blocks",
            f.segments.len(),
            covs.len()
        )));
    }
    Ok(f.segments
        .iter()
        .zip(covs)
        .map(|(s, c)| Ephemeris {
            object_id: s.meta.object_id.clone(),
            object_name: s.meta.object_name.clone(),
            ref_frame: s.meta.ref_frame.clone(),
            time_system: s.meta.time_system.clone(),
            states: s
                .states
                .iter()
                .map(|l| {
                    let p = l.pos_km;
                    let v = l.vel_km_s;
                    (
                        epoch_s(&l.epoch),
                        [
                            p[0] * 1e3,
                            p[1] * 1e3,
                            p[2] * 1e3,
                            v[0] * 1e3,
                            v[1] * 1e3,
                            v[2] * 1e3,
                        ],
                    )
                })
                .collect(),
            covariances: c
                .into_iter()
                .map(|(t, fr, m)| (t, fr.unwrap_or_else(|| s.meta.ref_frame.clone()), m))
                .collect(),
        })
        .collect())
}

type CovBlock = Vec<(f64, Option<String>, M6)>;

/// The covariance matrices of each segment, in segment order.
fn covariance_blocks(text: &str) -> Result<Vec<CovBlock>, KshanaError> {
    let mut segs: Vec<CovBlock> = Vec::new();
    let mut in_cov = false;
    let mut cur: Option<(f64, Option<String>, Vec<f64>)> = None;
    let finish = |cur: &mut Option<(f64, Option<String>, Vec<f64>)>,
                  segs: &mut Vec<CovBlock>|
     -> Result<(), KshanaError> {
        if let Some((t, fr, v)) = cur.take() {
            if v.len() != 21 {
                return Err(bad(format!(
                    "OEM covariance at {}: {} values, expected 21",
                    super::fmt_utc(t),
                    v.len()
                )));
            }
            let mut m = [[0.0; 6]; 6];
            let mut k = 0;
            for i in 0..6 {
                for j in 0..=i {
                    m[i][j] = v[k] * 1e6;
                    m[j][i] = v[k] * 1e6;
                    k += 1;
                }
            }
            segs.last_mut()
                .ok_or_else(|| bad("OEM covariance before any segment"))?
                .push((t, fr, m));
        }
        Ok(())
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line == "META_START" {
            segs.push(Vec::new());
            continue;
        }
        if line == "COVARIANCE_START" {
            in_cov = true;
            continue;
        }
        if line == "COVARIANCE_STOP" {
            finish(&mut cur, &mut segs)?;
            in_cov = false;
            continue;
        }
        if !in_cov || line.is_empty() || line.starts_with("COMMENT") {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "EPOCH" => {
                    finish(&mut cur, &mut segs)?;
                    cur = Some((super::parse_utc(v.trim())?, None, Vec::new()));
                }
                "COV_REF_FRAME" => {
                    if let Some(c) = cur.as_mut() {
                        c.1 = Some(v.trim().to_string());
                    }
                }
                _ => {}
            }
            continue;
        }
        let c = cur
            .as_mut()
            .ok_or_else(|| bad("OEM covariance values before EPOCH"))?;
        for tok in line.split_whitespace() {
            c.2.push(
                tok.parse()
                    .map_err(|_| bad(format!("OEM covariance value '{tok}'")))?,
            );
        }
    }
    Ok(segs)
}

impl Ephemeris {
    /// Lagrange interpolation of the state at `t` over the `order + 1` nearest points
    /// (`None` outside the span).
    pub fn state_at(&self, t: f64, order: usize) -> Option<[f64; 6]> {
        let s = &self.states;
        if s.is_empty() || t < s[0].0 - 1e-6 || t > s[s.len() - 1].0 + 1e-6 {
            return None;
        }
        if let Some(x) = s.iter().find(|x| (x.0 - t).abs() < 1e-6) {
            return Some(x.1);
        }
        let n = (order + 1).min(s.len());
        let i = s.partition_point(|x| x.0 < t);
        let lo = i.saturating_sub(n / 2).min(s.len() - n);
        let pts = &s[lo..lo + n];
        let mut out = [0.0; 6];
        for (j, pj) in pts.iter().enumerate() {
            let mut w = 1.0;
            for (m, pm) in pts.iter().enumerate() {
                if m != j {
                    w *= (t - pm.0) / (pj.0 - pm.0);
                }
            }
            for k in 0..6 {
                out[k] += w * pj.1[k];
            }
        }
        Some(out)
    }
}

/// An OMM's object id (`NORAD_CAT_ID` when given, else `OBJECT_ID`) and an equivalent
/// inertial state at perigee (for the orbit class; not a propagated state).
pub fn omm_class_state(text: &str) -> Result<(String, [f64; 6]), KshanaError> {
    let m = kshana::odm::OmmMessage::parse(text).map_err(|e| bad(format!("OMM: {e}")))?;
    let me = &m.mean_elements;
    let n = |k: &str| me.num(k).map_err(|e| bad(format!("OMM: {e}")));
    let e = n("ECCENTRICITY")?.ok_or_else(|| bad("OMM has no ECCENTRICITY"))?;
    let inc = n("INCLINATION")?.unwrap_or(0.0).to_radians();
    let a_m = match (n("SEMI_MAJOR_AXIS")?, n("MEAN_MOTION")?) {
        (Some(a), _) => a * 1e3,
        (None, Some(rev_day)) => {
            let w = rev_day * 2.0 * std::f64::consts::PI / 86_400.0;
            (MU_EARTH / (w * w)).cbrt()
        }
        _ => return Err(bad("OMM has neither SEMI_MAJOR_AXIS nor MEAN_MOTION")),
    };
    let rp = a_m * (1.0 - e);
    let vp = (MU_EARTH * (2.0 / rp - 1.0 / a_m)).sqrt();
    let id = m
        .tle
        .as_ref()
        .and_then(|t| t.get("NORAD_CAT_ID"))
        .or_else(|| m.metadata.get("OBJECT_ID"))
        .map(super::norm_id)
        .unwrap_or_default();
    Ok((id, [rp, 0.0, 0.0, 0.0, vp * inc.cos(), vp * inc.sin()]))
}
