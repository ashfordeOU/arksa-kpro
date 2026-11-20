// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Rideshare object identification (`pro-rideshare-id`, `arksa-kpro rideshare-id`):
//!
//! * catalogue numbers above 99,999: the local Alpha-5 helper against the open engine's
//!   conformance vectors (`tests/fixtures/rideshare/norad-catalogue.json`, copied from kshana
//!   main c86040bd `data/conformance/time/norad-catalogue.json`), TLE and OMM (KVN, XML, JSON)
//!   end to end, numbers beyond Alpha-5 in OMM only;
//! * synthetic multi-object deployments: the operator's object ranked first with a clear margin
//!   (Doppler only; angles and ranging from two stations), an ambiguous pair reported as
//!   ambiguous, a set without the object reported as such, too few measurements;
//! * the observation CSV's checks, the signed verdict directory, reproducibility of the
//!   result hash, the command, and the entitlement gate.

use std::path::{Path, PathBuf};

use arksa_kpro::packs::rideshare_id_pack::{
    report_md, run_study, summary, write_study, RideshareIdScenario,
};
use arksa_kpro::rideshare::catalogue::{self, Candidate};
use arksa_kpro::rideshare::identify::{identify, Config, Verdict};
use arksa_kpro::rideshare::observations::parse_csv;
use arksa_kpro::rideshare::synthetic::{generate, omm_xml, DeploymentSpec, StationSpec};
use arksa_kpro::rideshare::{norad, round_sig};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arksa-rideshare-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn candidates_of(d: &arksa_kpro::rideshare::synthetic::Deployment) -> Vec<Candidate> {
    let mut all = Vec::new();
    if !d.tle_text.is_empty() {
        all.extend(catalogue::from_tle_text(&d.tle_text, "c.tle").unwrap());
    }
    all.extend(catalogue::from_omm_text(&d.omm_text, "c.omm").unwrap());
    catalogue::merge(all).0
}

// ── Catalogue numbers ───────────────────────────────────────────────────────────────────

#[test]
fn alpha5_helper_agrees_with_the_open_engine_conformance_vectors() {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rideshare/norad-catalogue.json"),
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["set"], "norad-catalogue");
    let mut counted = std::collections::BTreeMap::<String, usize>::new();
    for t in v["vectors"].as_array().unwrap() {
        let id = t["id"].as_str().unwrap();
        let op = t["op"].as_str().unwrap();
        let input = &t["input"];
        let expect = &t["expect"];
        let err = expect["error"].as_bool() == Some(true);
        *counted.entry(op.to_string()).or_default() += 1;
        match op {
            "norad_encode" => {
                let r = norad::encode(input["number"].as_u64().unwrap() as u32);
                if err {
                    assert!(r.is_err(), "{id}");
                } else {
                    assert_eq!(r.unwrap(), expect["field"].as_str().unwrap(), "{id}");
                }
            }
            "norad_decode" => {
                let r = norad::decode(input["field"].as_str().unwrap());
                if err {
                    assert!(r.is_err(), "{id}");
                } else {
                    assert_eq!(
                        r.unwrap() as u64,
                        expect["number"].as_u64().unwrap(),
                        "{id}"
                    );
                }
            }
            "tle_checksum" => {
                assert_eq!(
                    kshana::tle::tle_checksum(input["line"].as_str().unwrap()) as u64,
                    expect["checksum"].as_u64().unwrap(),
                    "{id}"
                );
            }
            "tle_identity" => {
                let r = norad::parse_tle_identity(input["line1"].as_str().unwrap()).unwrap();
                assert_eq!(
                    r.norad_cat_id as u64,
                    expect["number"].as_u64().unwrap(),
                    "{id}"
                );
                assert_eq!(
                    r.intl_designator,
                    expect["designator"].as_str().unwrap(),
                    "{id}"
                );
                if let Some(e) = expect["epoch"].as_str() {
                    assert_eq!(r.epoch_iso, e, "{id}");
                }
            }
            "omm_catalogue" => {
                let m = kshana::odm::OmmMessage::parse(input["omm"].as_str().unwrap()).unwrap();
                let (n, tle) = norad::tle_from_omm(&m).unwrap();
                assert_eq!(n as u64, expect["number"].as_u64().unwrap(), "{id}");
                assert_eq!(
                    tle.epoch_days_1950,
                    expect["epoch_days_1950"].as_f64().unwrap(),
                    "{id}"
                );
                let a5 = expect["alpha5"].as_str().unwrap();
                match norad::encode(n) {
                    Ok(f) => assert_eq!(f, a5, "{id}"),
                    Err(_) => assert_eq!(a5, "", "{id}"),
                }
            }
            other => panic!("{id}: unknown op {other}"),
        }
    }
    // Every operation of the vector set is exercised.
    for op in [
        "norad_encode",
        "norad_decode",
        "tle_checksum",
        "tle_identity",
        "omm_catalogue",
    ] {
        assert!(counted.get(op).copied().unwrap_or(0) >= 2, "{op}");
    }
}

#[test]
fn alpha5_round_trips_over_its_whole_range_and_refuses_bad_fields() {
    for n in (0..=norad::ALPHA5_MAX).step_by(7) {
        let f = norad::encode(n).unwrap();
        assert_eq!(f.len(), 5);
        assert_eq!(norad::decode(&f).unwrap(), n, "{f}");
        assert_eq!(norad::is_alpha5(&f), n > norad::FIVE_DIGIT_MAX);
    }
    for (n, f) in [
        (100_000, "A0000"),
        (101_388, "A1388"),
        (179_999, "H9999"),
        (180_000, "J0000"),
        (230_000, "P0000"),
        (339_999, "Z9999"),
    ] {
        assert_eq!(norad::encode(n).unwrap(), f);
        assert_eq!(norad::decode(f).unwrap(), n);
    }
    let e = norad::encode(340_000).unwrap_err();
    assert!(e.contains("OMM"), "{e}");
    for bad in [
        "I0000", "O1234", "a0000", "A000", "A00000", "-1234", "", "A12B4",
    ] {
        assert!(norad::decode(bad).is_err(), "{bad}");
    }
}

/// The published engine this crate builds on (kshana 0.41.0) reads the catalogue field as five
/// digits only; that is why the local helper exists. When a kshana release with `norad` is
/// pinned this test fails on purpose: replace `rideshare::norad` with `kshana::norad` then.
#[test]
fn the_pinned_engine_still_lacks_alpha5_so_the_local_helper_is_needed() {
    let l1 = "1 T0002U 26900A   26318.50000000  .00001000  00000-0  10000-3 0  9999";
    assert!(kshana::tle::parse_tle_identity(l1).is_err());
    assert_eq!(norad::parse_tle_identity(l1).unwrap().norad_cat_id, 270_002);
}

#[test]
fn six_digit_numbers_travel_end_to_end_through_tle_and_omm() {
    let el = kshana::tle::parse_tle(
        "1 25544U 98067A   26319.20611933  .00010382  00000+0  19362-3 0  9993",
        "2 25544  51.6322 272.7605 0007377  27.6155 332.5225 15.49388394595162",
    )
    .unwrap();
    // TLE writer and reader: Alpha-5 field, correct checksums, same elements.
    let (l1, l2) = catalogue::format_tle(270_002, "2026-900A", &el).unwrap();
    assert_eq!(&l1[2..7], "T0002");
    assert_eq!(&l2[2..7], "T0002");
    assert_eq!(l1.len(), 69);
    assert_eq!(l2.len(), 69);
    kshana::tle::verify_checksum(&l1, "1").unwrap();
    kshana::tle::verify_checksum(&l2, "2").unwrap();
    let c = catalogue::from_tle_text(&format!("0 MY SAT\n{l1}\n{l2}\n"), "t").unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].catalogue_number, 270_002);
    assert_eq!(c[0].tle_field.as_deref(), Some("T0002"));
    assert_eq!(c[0].name, "MY SAT");
    assert_eq!(c[0].object_id, "2026-900A");
    assert!(c[0].checksum_ok);
    assert!((c[0].elements.epoch_days_1950 - el.epoch_days_1950).abs() < 1e-8);
    assert!((c[0].elements.bstar - el.bstar).abs() < 1e-9);
    assert!(c[0].info().alpha5);
    // A line pair whose two catalogue fields differ is refused.
    let l2b = format!("2 T0003{}", &l2[7..]);
    assert!(catalogue::from_tle_text(&format!("{l1}\n{l2b}\n"), "t").is_err());
    // Beyond Alpha-5: no TLE, OMM only, with the full number.
    assert!(catalogue::format_tle(340_000, "", &el).is_err());
    for n in [512_345u32, 123_456_789] {
        let omm =
            catalogue::format_omm_kvn(n, "BIG", "2026-900B", &el, "2026-11-15T00:00:00").unwrap();
        let c = catalogue::from_omm_text(&omm, "o").unwrap();
        assert_eq!(c[0].catalogue_number, n);
        assert_eq!(c[0].tle_field, None);
        assert!(!c[0].info().alpha5);
        assert_eq!(c[0].source, "omm-kvn");
    }
    // Ten digits do not fit an OMM NORAD_CAT_ID.
    let omm = catalogue::format_omm_kvn(1, "X", "", &el, "2026-11-15T00:00:00")
        .unwrap()
        .replace("NORAD_CAT_ID = 1\n", "NORAD_CAT_ID = 1234567890\n");
    assert!(catalogue::from_omm_text(&omm, "o").is_err());
}

#[test]
fn omm_kvn_xml_and_json_give_the_same_candidates() {
    let spec = DeploymentSpec {
        objects: 4,
        target: 1,
        catalogue_numbers: Some(vec![99_999, 100_000, 339_999, 512_345]),
        ..DeploymentSpec::default()
    };
    let d = generate(&spec).unwrap();
    assert_eq!(d.omm_only, vec![512_345]);
    assert!(d.tle_text.contains("\n1 99999U") || d.tle_text.contains("1 99999U"));
    assert!(d.tle_text.contains("1 A0000U"));
    assert!(d.tle_text.contains("1 Z9999U"));
    let kvn = catalogue::from_omm_text(&d.omm_text, "k").unwrap();
    let xml_text = omm_xml(&d.omm_text).unwrap();
    assert!(xml_text.contains("<ndm>"));
    let xml = catalogue::from_text(&xml_text, "x").unwrap();
    // JSON as catalogue services publish it, with extra keys that are not OMM keywords.
    let json: Vec<serde_json::Value> = d
        .omm_text
        .split("CCSDS_OMM_VERS")
        .filter(|c| !c.trim().is_empty())
        .map(|c| {
            let mut o = serde_json::Map::new();
            for line in format!("CCSDS_OMM_VERS{c}").lines() {
                if let Some((k, v)) = line.split_once('=') {
                    o.insert(k.trim().into(), serde_json::Value::String(v.trim().into()));
                }
            }
            o.insert("DECAY_DATE".into(), serde_json::Value::Null);
            o.insert("TLE_LINE1".into(), "ignored".into());
            serde_json::Value::Object(o)
        })
        .collect();
    let js = catalogue::from_text(&serde_json::to_string(&json).unwrap(), "j").unwrap();
    assert_eq!(kvn.len(), 4);
    for set in [&xml, &js] {
        assert_eq!(set.len(), 4);
        for (a, b) in kvn.iter().zip(set.iter()) {
            assert_eq!(a.catalogue_number, b.catalogue_number);
            assert!((a.elements.mo_rad - b.elements.mo_rad).abs() < 1e-12);
            assert!((a.elements.no_kozai_rad_min - b.elements.no_kozai_rad_min).abs() < 1e-15);
            assert!((a.elements.epoch_days_1950 - b.elements.epoch_days_1950).abs() < 1e-12);
        }
    }
    assert_eq!(xml[0].source, "omm-xml");
    assert_eq!(js[0].source, "omm-json");
    // TLE and OMM of the same objects merge to one candidate each.
    let (m, w) = catalogue::merge(
        catalogue::from_tle_text(&d.tle_text, "t")
            .unwrap()
            .into_iter()
            .chain(kvn)
            .collect(),
    );
    assert_eq!(m.len(), 4);
    assert_eq!(w.len(), 3);
    assert_eq!(
        m.iter().map(|c| c.catalogue_number).collect::<Vec<_>>(),
        vec![99_999, 100_000, 339_999, 512_345]
    );
}

// ── Identification on synthetic deployments ─────────────────────────────────────────────

#[test]
fn twelve_objects_doppler_only_the_operators_object_ranks_first_with_a_clear_margin() {
    let spec = DeploymentSpec::default();
    assert_eq!(spec.objects, 12);
    let d = generate(&spec).unwrap();
    // Three short passes over one station: about 85 Doppler measurements.
    assert!(
        d.observations.obs.len() >= 60,
        "{}",
        d.observations.obs.len()
    );
    assert_eq!(d.observations.passes.len(), 3);
    let cands = candidates_of(&d);
    assert_eq!(cands.len(), 12);
    assert!(cands.iter().all(|c| c.catalogue_number > 99_999));
    let id = identify(&cands, &d.observations, &Config::default()).unwrap();
    assert_eq!(id.verdict, Verdict::Identified, "{:?}", id.reasons);
    assert_eq!(id.identified, Some(d.target_catalogue_number));
    let best = id.best().unwrap();
    assert_eq!(best.candidate.catalogue_number, 101_393);
    assert_eq!(best.candidate.tle_field.as_deref(), Some("A1393"));
    assert!(best.reduced_chi2 < 2.0, "{}", best.reduced_chi2);
    assert!(best.probability > 0.999_999);
    // The per-pass oscillator offset is recovered.
    for b in &best.biases {
        assert!((b.value - 1_250.0).abs() < 30.0, "{b:?}");
    }
    let s = id.separation.as_ref().unwrap();
    assert!(s.delta_chi2 > 100.0, "{s:?}");
    assert!(s.predicted_difference_sigma > 10.0, "{s:?}");
    assert!(
        s.distance_km_min > 1.0 && s.distance_km_max < 200.0,
        "{s:?}"
    );
    assert_eq!(id.not_excluded, vec![d.target_catalogue_number]);
    // Every other candidate is far worse.
    for f in id.fits.iter().skip(1) {
        assert!(
            f.delta_chi2 >= 25.0,
            "{} {}",
            f.candidate.catalogue_number,
            f.delta_chi2
        );
    }
}

#[test]
fn twenty_objects_angles_and_ranging_from_two_stations_with_an_omm_only_target() {
    let mut cats: Vec<u32> = (0..20).map(|k| 339_990 + k).collect();
    // Objects 10.. are above Alpha-5 and exist in OMM only; the target is one of them.
    cats.sort_unstable();
    let spec = DeploymentSpec {
        objects: 20,
        target: 13,
        catalogue_numbers: Some(cats),
        spacing_km: 3.0,
        drift_km_per_rev: 0.2,
        truth_offset_km: 0.1,
        stations: vec![
            StationSpec {
                name: "NORTH".into(),
                lat_deg: 59.4,
                lon_deg: 24.7,
                alt_m: 40.0,
            },
            StationSpec {
                name: "SOUTH".into(),
                lat_deg: -33.9,
                lon_deg: 18.4,
                alt_m: 10.0,
            },
        ],
        types: vec!["az_deg".into(), "el_deg".into(), "range_m".into()],
        sigma_angle_deg: 0.05,
        // The sigma covers the element sets' along-track error (100 m), not only the noise.
        sigma_range_m: 100.0,
        max_passes: 4,
        step_s: 20.0,
        seed: 7,
        ..DeploymentSpec::default()
    };
    let d = generate(&spec).unwrap();
    assert_eq!(d.target_catalogue_number, 340_003);
    assert_eq!(d.omm_only.len(), 10);
    let cands = candidates_of(&d);
    assert_eq!(cands.len(), 20);
    let id = identify(&cands, &d.observations, &Config::default()).unwrap();
    assert_eq!(id.verdict, Verdict::Identified, "{:?}", id.reasons);
    assert_eq!(id.identified, Some(340_003));
    let best = id.best().unwrap();
    assert_eq!(best.candidate.tle_field, None);
    assert_eq!(best.candidate.source, "omm-kvn");
    assert!(best.reduced_chi2 < 2.0, "{}", best.reduced_chi2);
    assert!(best.biases.is_empty());
    let types: Vec<&str> = best.per_type.iter().map(|t| t.kind).collect();
    assert_eq!(types, vec!["range_m", "az_deg", "el_deg"]);
    assert!(id.separation.as_ref().unwrap().delta_chi2 > 25.0);
}

#[test]
fn two_objects_still_side_by_side_are_reported_ambiguous_not_guessed() {
    // Objects 4 and 5 are 50 m apart with the same drift; the others spaced as usual.
    let mut along: Vec<f64> = (0..10).map(|k| k as f64 * 8.0).collect();
    along[5] = along[4] + 0.05;
    let spec = DeploymentSpec {
        objects: 10,
        target: 5,
        along_track_km: Some(along),
        drift_km_per_rev: 0.0,
        truth_offset_km: 0.0,
        ..DeploymentSpec::default()
    };
    let d = generate(&spec).unwrap();
    let cands = candidates_of(&d);
    let id = identify(&cands, &d.observations, &Config::default()).unwrap();
    assert_eq!(id.verdict, Verdict::Ambiguous, "{:?}", id.reasons);
    assert_eq!(id.identified, None);
    let mut ne = id.not_excluded.clone();
    ne.sort_unstable();
    assert_eq!(ne, vec![101_392, 101_393]);
    let s = id.separation.as_ref().unwrap();
    assert!(s.delta_chi2 < 25.0, "{s:?}");
    assert!(s.distance_km_max < 0.5, "{s:?}");
    assert!(id.reasons.iter().any(|r| r.contains("not excluded")));
    // The two together carry nearly all the probability; the rest is excluded.
    let p: f64 = id
        .fits
        .iter()
        .filter(|f| ne.contains(&f.candidate.catalogue_number))
        .map(|f| f.probability)
        .sum();
    assert!(p > 0.999, "{p}");
}

#[test]
fn a_candidate_set_without_the_object_is_not_forced_onto_a_neighbour() {
    let d = generate(&DeploymentSpec {
        spacing_km: 20.0,
        ..DeploymentSpec::default()
    })
    .unwrap();
    let cands: Vec<Candidate> = candidates_of(&d)
        .into_iter()
        .filter(|c| c.catalogue_number != d.target_catalogue_number)
        .collect();
    let id = identify(&cands, &d.observations, &Config::default()).unwrap();
    assert_eq!(
        id.verdict,
        Verdict::NoConsistentCandidate,
        "{:?}",
        id.reasons
    );
    assert_eq!(id.identified, None);
    assert!(id.not_excluded.is_empty());
}

#[test]
fn too_few_measurements_and_a_single_candidate_are_said_plainly() {
    let d = generate(&DeploymentSpec::default()).unwrap();
    let cands = candidates_of(&d);
    let cfg = Config {
        min_observations: d.observations.obs.len() + 1,
        ..Config::default()
    };
    let id = identify(&cands, &d.observations, &cfg).unwrap();
    assert_eq!(id.verdict, Verdict::InsufficientData);
    assert_eq!(id.identified, None);
    let only: Vec<Candidate> = cands
        .into_iter()
        .filter(|c| c.catalogue_number == d.target_catalogue_number)
        .collect();
    let id = identify(&only, &d.observations, &Config::default()).unwrap();
    assert_eq!(id.verdict, Verdict::ConsistentOnlyCandidate);
    assert!(id.reasons[0].contains("nothing was discriminated"));
    assert!(identify(&[], &d.observations, &Config::default()).is_err());
    let bad = Config {
        min_probability: 1.0,
        ..Config::default()
    };
    assert!(identify(&only, &d.observations, &bad).is_err());
}

// ── Observation CSV ─────────────────────────────────────────────────────────────────────

#[test]
fn the_observation_csv_is_checked() {
    let head = "time_utc,station,lat_deg,lon_deg,alt_m,type,value,sigma,carrier_hz,pass\n";
    let ok = format!("# c\n{head}2026-11-15T10:00:00Z,A,59.4,24.7,40,doppler_hz,100,20,437500000,\n2026-11-15T10:00:10.5Z,A,59.4,24.7,40,el_deg,12.5,0.1,,\n2026-11-15T11:40:00Z,A,59.4,24.7,40,el_deg,13,0.1,,\n2026-11-15T10:00:00Z,B,-33.9,18.4,10,range_m,1e6,30,,x\n");
    let s = parse_csv(&ok).unwrap();
    assert_eq!(s.obs.len(), 4);
    assert_eq!(s.stations.len(), 2);
    assert_eq!(s.passes, vec!["A/auto-1", "A/auto-2", "B/x"]);
    assert_eq!(s.obs[0].pass, s.obs[1].pass);
    assert_ne!(s.obs[1].pass, s.obs[2].pass);
    for (bad, why) in [
        (format!("{head}2026-11-15T10:00:00Z,A,59.4,24.7,40,doppler_hz,100,20,,\n"), "carrier_hz"),
        ("time_utc,station,lat_deg,lon_deg,alt_m,type,value,sigma,colour\n".to_string(), "unknown column"),
        ("time_utc,station,lat_deg,lon_deg,alt_m,type,value\n".to_string(), "missing column"),
        (format!("{head}2026-11-15T10:00:00Z,A,59.4,24.7,40,el_deg,10,0,,\n"), "sigma"),
        (format!("{head}2026-11-15T10:00:00Z,A,59.4,24.7,40,elev,10,1,,\n"), "unknown type"),
        (format!("{head}2026-11-15 10:00:00,A,59.4,24.7,40,el_deg,10,1,,\n"), "UTC"),
        (format!("{head}2026-11-15T10:00:00Z,A,59.4,24.7,40,el_deg,10,1,,\n2026-11-15T10:00:10Z,A,59.5,24.7,40,el_deg,10,1,,\n"), "different position"),
        (head.to_string(), "no measurement"),
    ] {
        let e = parse_csv(&bad).unwrap_err();
        assert!(e.contains(why), "{why}: {e}");
    }
}

// ── Pack, signed verdict, command ───────────────────────────────────────────────────────

fn write_deployment(
    dir: &Path,
    spec: &DeploymentSpec,
) -> arksa_kpro::rideshare::synthetic::Deployment {
    let d = generate(spec).unwrap();
    arksa_kpro::rideshare::cli::write_synthetic(&d, spec, dir).unwrap();
    d
}

#[test]
fn the_pack_writes_a_signed_reproducible_modelled_verdict() {
    let dir = tmp("pack");
    let d = write_deployment(&dir, &DeploymentSpec::default());
    let text = std::fs::read_to_string(dir.join("scenario.toml")).unwrap();
    let mut s: RideshareIdScenario = toml::from_str(&text).unwrap();
    s.resolve_files(&dir).unwrap();
    let st = run_study(&s).unwrap();
    let doc = &st.doc;
    assert_eq!(doc["schema"], "arksa-rideshare-id/1");
    assert_eq!(doc["evidence_class"], "MODELLED");
    assert!(doc["label"].as_str().unwrap().starts_with("MODELLED"));
    assert_eq!(doc["verdict"], "IDENTIFIED");
    assert_eq!(
        doc["identified"]["catalogue_number"],
        d.target_catalogue_number
    );
    assert_eq!(doc["identified"]["tle_field"], "A1393");
    assert_eq!(doc["catalogue"]["above_99999"], 12);
    assert_eq!(doc["inputs"].as_array().unwrap().len(), 3);
    assert_eq!(doc["candidates"].as_array().unwrap().len(), 12);
    // TLE and OMM name the same 12 objects: 12 duplicates merged with a warning each.
    assert_eq!(doc["warnings"].as_array().unwrap().len(), 12);
    let text = serde_json::to_string(doc).unwrap();
    assert!(!text.to_ascii_lowercase().contains("validated"));
    // Every float has at most 9 significant digits.
    fn walk(v: &serde_json::Value) {
        match v {
            serde_json::Value::Number(n) if n.is_f64() => {
                let x = n.as_f64().unwrap();
                assert_eq!(x, round_sig(x));
            }
            serde_json::Value::Array(a) => a.iter().for_each(walk),
            serde_json::Value::Object(o) => o.values().for_each(walk),
            _ => {}
        }
    }
    walk(doc);
    // Reproducible: the same inputs give the same hash.
    let again = run_study(&s).unwrap();
    assert_eq!(again.doc["result_sha256"], doc["result_sha256"]);
    assert_eq!(doc["result_sha256"].as_str().unwrap().len(), 64);
    let sm = summary(doc);
    assert!(
        sm.contains("IDENTIFIED") && sm.contains("TLE A1393"),
        "{sm}"
    );
    let md = report_md(doc);
    assert!(
        md.contains("## Verdict: IDENTIFIED") && md.contains("| 1 | 101393 | A1393 |"),
        "{md}"
    );
    assert!(st
        .residuals_csv
        .lines()
        .nth(1)
        .unwrap()
        .contains("best_101393_residual"));
    // Signed directory.
    let out = dir.join("verdict");
    let seed = [9u8; 32];
    let sig = write_study(
        &st,
        &out,
        Some((&seed, "Arksa test", "2026-11-20T00:00:00Z")),
    )
    .unwrap()
    .unwrap();
    assert!(sig.ends_with("signature.json"));
    let pk = kshana_pro::license::public_key_for(&seed);
    let v = kshana_pro::signed_dossier::verify(&out, &[pk]).unwrap();
    assert!(v.passed(), "{:?}", v.problems);
    assert_eq!(v.files, 4);
    std::fs::write(out.join("candidates.csv"), "edited").unwrap();
    assert!(!kshana_pro::signed_dossier::verify(&out, &[pk])
        .unwrap()
        .passed());
    // The pack also runs through the open engine's Scenario interface.
    use kshana::api::Scenario;
    let r = s.run().unwrap();
    assert!(r.summary.contains("IDENTIFIED"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_command_writes_the_synthetic_inputs_and_identifies() {
    let dir = tmp("cli");
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let synth = dir.join("in");
    let code = arksa_kpro::rideshare::cli::run_inner(
        &a(&["synth", "--out", synth.to_str().unwrap()]),
        1_763_600_000,
    )
    .unwrap();
    assert_eq!(code, 0);
    for f in [
        "candidates.tle",
        "candidates.omm",
        "observations.csv",
        "scenario.toml",
        "answer.json",
    ] {
        assert!(synth.join(f).exists(), "{f}");
    }
    let key = dir.join("key.hex");
    std::fs::write(&key, hex::encode([5u8; 32])).unwrap();
    let out = dir.join("out");
    let code = arksa_kpro::rideshare::cli::run_inner(
        &a(&[
            "identify",
            synth.join("scenario.toml").to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--sign-key-file",
            key.to_str().unwrap(),
        ]),
        1_763_600_000,
    )
    .unwrap();
    assert_eq!(code, 0);
    assert!(out.join("signature.json").exists());
    let pk = kshana_pro::license::public_key_for(&[5u8; 32]);
    assert!(kshana_pro::signed_dossier::verify(&out, &[pk])
        .unwrap()
        .passed());
    // An ambiguous case exits 1.
    let mut along: Vec<f64> = (0..6).map(|k| k as f64 * 8.0).collect();
    along[3] = along[2] + 0.05;
    let spec = DeploymentSpec {
        objects: 6,
        target: 3,
        along_track_km: Some(along),
        drift_km_per_rev: 0.0,
        truth_offset_km: 0.0,
        ..DeploymentSpec::default()
    };
    let amb = dir.join("amb");
    write_deployment(&amb, &spec);
    let code = arksa_kpro::rideshare::cli::run_inner(
        &a(&[
            "identify",
            amb.join("scenario.toml").to_str().unwrap(),
            "--out",
            dir.join("amb-out").to_str().unwrap(),
        ]),
        1_763_600_000,
    )
    .unwrap();
    assert_eq!(code, 1);
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("amb-out/verdict.json")).unwrap())
            .unwrap();
    assert_eq!(v["verdict"], "AMBIGUOUS");
    assert!(v["identified"].is_null());
    assert!(arksa_kpro::rideshare::cli::run_inner(&a(&["norad", "encode", "270002"]), 0).is_ok());
    assert!(arksa_kpro::rideshare::cli::run_inner(&a(&["norad", "encode", "340000"]), 0).is_err());
    assert!(arksa_kpro::rideshare::cli::run_inner(
        &a(&["catalogue", synth.join("candidates.tle").to_str().unwrap()]),
        0
    )
    .is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_command_needs_the_arksa_entitlement() {
    use kshana_pro::license::{self, issue_arksa, issue_v2, verify_with, Keyring, LicenseClaims};
    const PRO: [u8; 32] = [7; 32];
    const ARK: [u8; 32] = *b"arksa-issuer-test-key-not-real!!";
    arksa_kpro::ensure_registered();
    let mut ring = Keyring::single(license::public_key_for(&PRO));
    ring.add_arksa_issuer(license::public_key_for(&ARK));
    let claims = |tier: &str| LicenseClaims {
        licensee: "ACME Space".into(),
        tier: tier.into(),
        expires_unix: 10_000,
        products: vec!["cli".into(), "service".into()],
        ..Default::default()
    };
    let opts = license::VerifyOptions::at(1);
    let pro = verify_with(
        &ring,
        &issue_v2(&PRO, &claims("enterprise")).unwrap(),
        &opts,
    )
    .unwrap()
    .entitlement;
    let ark = verify_with(&ring, &issue_arksa(&ARK, &claims("pro")).unwrap(), &opts)
        .unwrap()
        .entitlement;
    assert!(arksa_kpro::entitlement::ensure_rideshare_id_entitled(&pro, 1).is_err());
    assert!(arksa_kpro::entitlement::ensure_rideshare_id_entitled(&ark, 1).is_ok());
    assert!(arksa_kpro::entitlement::ensure_rideshare_id_entitled(&ark, 20_000).is_err());
    // The command is registered.
    assert!(arksa_kpro::commands::all()
        .iter()
        .any(|c| c.name == "rideshare-id" && c.usage.contains("arksa-kpro rideshare-id")));
}

#[test]
fn every_object_of_the_deployment_is_found_whichever_is_ours_and_whatever_the_noise() {
    for target in 0..12 {
        let spec = DeploymentSpec {
            target,
            seed: 100 + target as u64,
            ..DeploymentSpec::default()
        };
        let d = generate(&spec).unwrap();
        let id = identify(&candidates_of(&d), &d.observations, &Config::default()).unwrap();
        assert_eq!(
            id.verdict,
            Verdict::Identified,
            "target {target}: {:?}",
            id.reasons
        );
        assert_eq!(
            id.identified,
            Some(101_388 + target as u32),
            "target {target}"
        );
    }
}

#[test]
fn the_side_by_side_pair_stays_ambiguous_whatever_the_noise() {
    for seed in 1..=8 {
        let mut along: Vec<f64> = (0..8).map(|k| k as f64 * 8.0).collect();
        along[3] = along[2] + 0.05;
        let spec = DeploymentSpec {
            objects: 8,
            target: 3,
            along_track_km: Some(along),
            drift_km_per_rev: 0.0,
            truth_offset_km: 0.0,
            seed,
            ..DeploymentSpec::default()
        };
        let d = generate(&spec).unwrap();
        let id = identify(&candidates_of(&d), &d.observations, &Config::default()).unwrap();
        assert_eq!(
            id.verdict,
            Verdict::Ambiguous,
            "seed {seed}: {:?}",
            id.reasons
        );
        assert!(id.not_excluded.contains(&101_391), "seed {seed}");
    }
}

/// The committed worked example (`docs/evidence/rideshare-id/`) re-runs to the same files; the
/// numbers are rounded to 9 significant digits, so this holds on macOS and Linux alike.
#[test]
fn the_committed_worked_example_reproduces_exactly() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/evidence/rideshare-id");
    let text = std::fs::read_to_string(dir.join("scenario.toml")).unwrap();
    let mut s: RideshareIdScenario = toml::from_str(&text).unwrap();
    s.resolve_files(&dir).unwrap();
    let st = run_study(&s).unwrap();
    let committed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("verdict/verdict.json")).unwrap())
            .unwrap();
    assert_eq!(st.doc["verdict"], "IDENTIFIED");
    assert_eq!(st.doc["identified"]["catalogue_number"], 101_393);
    assert_eq!(st.doc["result_sha256"], committed["result_sha256"]);
    assert_eq!(st.doc, committed);
    let out = tmp("example");
    write_study(&st, &out, None).unwrap();
    for f in [
        "verdict.json",
        "candidates.csv",
        "residuals.csv",
        "report.md",
    ] {
        assert_eq!(
            std::fs::read_to_string(out.join(f)).unwrap(),
            std::fs::read_to_string(dir.join("verdict").join(f)).unwrap(),
            "{f}"
        );
    }
    let answer: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("answer.json")).unwrap()).unwrap();
    assert_eq!(answer["target_catalogue_number"], 101_393);
    let _ = std::fs::remove_dir_all(&out);
}
