// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Angles-only observations in a CCSDS Tracking Data Message (CCSDS 503.0-B-2, KVN), read
//! and written through the open engine's TDM parser ([`kshana::ccsds_tdm`]).
//!
//! Accepted: segments with `ANGLE_TYPE = RADEC`, `REFERENCE_FRAME` EME2000, ICRF or GCRF
//! (treated alike: they differ by tens of milliarcseconds), `TIME_SYSTEM` UTC, TAI or TT, and
//! `ANGLE_1` (right ascension) / `ANGLE_2` (declination) in degrees paired by epoch.
//! `PARTICIPANT_1` names the observer (it must match a configured observer) and
//! `PARTICIPANT_2` the track (each segment is one tracklet). A segment with angles of any
//! other type is refused, never reinterpreted.

use kshana::api::KshanaError;
use kshana::ccsds_tdm::{TdmFile, TdmMeta, TdmObs, TdmSegment};

use super::{bad, AngleObs};

fn meta_get<'a>(m: &'a TdmMeta, key: &str) -> Option<&'a str> {
    m.other
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// Read the RADEC angle pairs of a TDM. `sigma` (rad) is assigned to every angle.
pub fn read_angles(text: &str, sigma: f64) -> Result<Vec<AngleObs>, KshanaError> {
    let f = TdmFile::parse(text).map_err(|e| bad(format!("TDM: {e}")))?;
    let mut out = Vec::new();
    for (k, seg) in f.segments.iter().enumerate() {
        let has_angles = seg
            .data
            .iter()
            .any(|o| o.key == "ANGLE_1" || o.key == "ANGLE_2");
        if !has_angles {
            continue;
        }
        let at = meta_get(&seg.meta, "ANGLE_TYPE").unwrap_or("");
        if !at.eq_ignore_ascii_case("RADEC") {
            return Err(bad(format!(
                "TDM segment {}: ANGLE_TYPE '{at}' is not supported (RADEC only)",
                k + 1
            )));
        }
        let frame = meta_get(&seg.meta, "REFERENCE_FRAME").unwrap_or("EME2000");
        if !["EME2000", "ICRF", "GCRF"]
            .iter()
            .any(|f| frame.eq_ignore_ascii_case(f))
        {
            return Err(bad(format!(
                "TDM segment {}: REFERENCE_FRAME '{frame}' is not supported (EME2000, ICRF, GCRF)",
                k + 1
            )));
        }
        let offset = match seg.meta.time_system.to_ascii_uppercase().as_str() {
            "UTC" => 0.0,
            "TAI" => -37.0,
            "TT" => -super::TT_MINUS_UTC_S,
            other => {
                return Err(bad(format!(
                    "TDM segment {}: TIME_SYSTEM '{other}' is not supported (UTC, TAI, TT)",
                    k + 1
                )))
            }
        };
        let observer = seg
            .meta
            .participants
            .first()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| bad(format!("TDM segment {}: no PARTICIPANT_1", k + 1)))?
            .clone();
        let track = seg
            .meta
            .participants
            .get(1)
            .filter(|p| !p.is_empty())
            .cloned()
            .unwrap_or_else(|| format!("SEG-{}", k + 1));
        let mut pending: Vec<(String, Option<f64>, Option<f64>)> = Vec::new();
        for o in &seg.data {
            let slot = match pending.iter_mut().find(|(e, _, _)| *e == o.epoch) {
                Some(s) => s,
                None => {
                    pending.push((o.epoch.clone(), None, None));
                    pending.last_mut().expect("just pushed")
                }
            };
            match o.key.as_str() {
                "ANGLE_1" => slot.1 = Some(o.value),
                "ANGLE_2" => slot.2 = Some(o.value),
                _ => {}
            }
        }
        for (epoch, a1, a2) in pending {
            let (Some(ra), Some(dec)) = (a1, a2) else {
                continue;
            };
            if !(-90.0..=90.0).contains(&dec) {
                return Err(bad(format!(
                    "TDM segment {}: declination {dec} out of range",
                    k + 1
                )));
            }
            out.push(AngleObs {
                utc_s: crate::conjunction::parse_utc(&epoch)? + offset,
                observer: observer.clone(),
                tracklet: track.clone(),
                ra: ra.to_radians().rem_euclid(std::f64::consts::TAU),
                dec: dec.to_radians(),
                sigma,
            });
        }
    }
    Ok(out)
}

/// Write angle observations as a TDM (one segment per tracklet, UTC, EME2000 RADEC).
pub fn write_angles(obs: &[AngleObs], originator: &str) -> String {
    let mut segs: Vec<TdmSegment> = Vec::new();
    let mut keys: Vec<(String, String)> = Vec::new();
    for o in obs {
        let key = (o.tracklet.clone(), o.observer.clone());
        let k = match keys.iter().position(|x| *x == key) {
            Some(k) => k,
            None => {
                keys.push(key);
                segs.push(TdmSegment {
                    meta: TdmMeta {
                        time_system: "UTC".into(),
                        participants: vec![o.observer.clone(), o.tracklet.clone()],
                        mode: "SEQUENTIAL".into(),
                        path: "2,1".into(),
                        other: vec![
                            ("ANGLE_TYPE".into(), "RADEC".into()),
                            ("REFERENCE_FRAME".into(), "EME2000".into()),
                        ],
                        ..Default::default()
                    },
                    data: Vec::new(),
                });
                segs.len() - 1
            }
        };
        let epoch = crate::conjunction::fmt_utc(o.utc_s)
            .trim_end_matches('Z')
            .to_string();
        segs[k].data.push(TdmObs {
            epoch: epoch.clone(),
            key: "ANGLE_1".into(),
            value: o.ra.to_degrees(),
        });
        segs[k].data.push(TdmObs {
            epoch,
            key: "ANGLE_2".into(),
            value: o.dec.to_degrees(),
        });
    }
    TdmFile {
        version: "2.0".into(),
        creation_date: "2026-01-01T00:00:00".into(),
        originator: originator.into(),
        segments: segs,
    }
    .to_tdm_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_refusals() {
        let obs = vec![
            AngleObs {
                utc_s: 846_000_000.0,
                observer: "ST1".into(),
                tracklet: "T1".into(),
                ra: 1.234_567_890_123,
                dec: -0.321_098_765_432,
                sigma: 1e-6,
            },
            AngleObs {
                utc_s: 846_000_600.0,
                observer: "ST1".into(),
                tracklet: "T1".into(),
                ra: 1.234_6,
                dec: -0.321,
                sigma: 1e-6,
            },
        ];
        let text = write_angles(&obs, "TEST");
        let back = read_angles(&text, 1e-6).unwrap();
        assert_eq!(back.len(), 2);
        assert!((back[0].ra - obs[0].ra).abs() < 1e-12);
        assert!((back[0].dec - obs[0].dec).abs() < 1e-12);
        assert!((back[1].utc_s - obs[1].utc_s).abs() < 1e-6);
        assert_eq!(back[0].observer, "ST1");
        assert_eq!(back[0].tracklet, "T1");
        let azel = text.replace("ANGLE_TYPE = RADEC", "ANGLE_TYPE = AZEL");
        assert!(read_angles(&azel, 1e-6).is_err());
        let tdb = text.replace("TIME_SYSTEM = UTC", "TIME_SYSTEM = TDB");
        assert!(read_angles(&tdb, 1e-6).is_err());
    }
}
