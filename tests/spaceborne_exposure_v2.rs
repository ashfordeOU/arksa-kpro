// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! AK4, spaceborne exposure v2: maps of ground interference regions, the per-revolution
//! exposure forecast with the receiver link-margin model and predicted outage windows, the
//! export of matching receiver test scenarios in Kshana Pro's neutral test-as-code format
//! (`kshana-testkit-scenario/1`), the Arksa entitlement gate, and the committed v2 study.
//!
//! Regenerate the committed v2 study (only after a deliberate change):
//! `ARKSA_REGEN_F4=1 cargo test --test spaceborne_exposure_v2 committed`.

use std::path::{Path, PathBuf};

use arksa_kpro::packs::spaceborne_rx_exposure_pack::{
    orbits_csv, outages_csv, passes_csv, report_md, run_study, run_study_entitled,
    run_study_with_scenarios, write_study_with_scenarios, ExportedScenario,
    SpaceborneRxExposureScenario,
};
use kshana_pro::license::tiers::NOT_IN_THIS_EDITION;
use kshana_pro::license::{Entitlement, Tier};
use kshana_pro::testkit::spec::Spec;
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/f4")
}

fn scenario(toml_text: &str) -> SpaceborneRxExposureScenario {
    let mut s: SpaceborneRxExposureScenario = toml::from_str(toml_text).unwrap();
    s.resolve_files(&fixtures()).unwrap();
    s
}

fn ent(arksa: bool) -> Entitlement {
    Entitlement {
        licensee: "test".into(),
        tier: Tier::Enterprise,
        expires_unix: 0,
        features: vec![],
        arksa,
    }
}

const ISS: &str = r#"tle = """
ISS (ZARYA)
1 25544U 98067A   26319.20611933  .00010382  00000+0  19362-3 0  9993
2 25544  51.6322 272.7605 0007377  27.6155 332.5225 15.49388394595162
"""
start_utc = "2026-11-15T00:00Z"
duration_h = 24.0
step_s = 30.0
"#;

/// The synthetic region map (a polygon and a circle with activity 0.5), with the forecast.
fn synthetic() -> String {
    format!(
        "{ISS}assumed_eirp_dbw = [50.0, 60.0]\nregion_files = [\"synthetic-regions.geojson\"]\n[forecast]\n"
    )
}

fn arr(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap_or(f64::NAN))
        .collect()
}

#[test]
fn a_region_map_is_read_and_its_polygon_counts_time_over_the_area() {
    let doc = run_study(&scenario(&synthetic())).unwrap();
    let rm = &doc["inputs"]["region_maps"][0];
    assert_eq!(rm["regions"], 2);
    assert_eq!(rm["evidence"], "SYNTHETIC");
    assert_eq!(doc["affected_areas"], 2);
    assert_eq!(doc["span"]["time_mode"], "climatology");
    let passes = doc["craft"][0]["passes"].as_array().unwrap();
    assert!(passes.iter().any(|p| {
        p["areas_seen"]
            .as_array()
            .unwrap()
            .contains(&"synthetic mid-Atlantic box".into())
    }));
    // The ISS (51.6 deg) crosses the 40-46 deg N box: some time with the ground track inside.
    assert!(doc["craft"][0]["over_area_s"].as_f64().unwrap() > 0.0);

    let bad = |body: &str| {
        let text = format!(
            r#"{{"type":"FeatureCollection","arksa_interference_regions":{{"version":1,"title":"t"}},"features":[{body}]}}"#
        );
        let mut s = scenario(ISS);
        s.region_json = vec![text];
        run_study(&s)
    };
    let point = r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[0,0]},"properties":{"id":"a","radius_km":100}}"#;
    assert!(bad(point).is_ok());
    assert!(
        bad(&point.replace("\"radius_km\":100", "\"radius_km\":100,\"activity\":1.5")).is_err()
    );
    assert!(bad(&point.replace(",\"radius_km\":100", "")).is_err());
    assert!(bad(&point.replace("Point", "LineString")).is_err());
    assert!(bad("").is_err(), "a map without a region is refused");
    let mut s = scenario(ISS);
    s.region_json = vec![r#"{"type":"FeatureCollection","features":[]}"#.into()];
    assert!(run_study(&s).is_err(), "the header is required");
}

#[test]
fn forecast_totals_windows_and_passes_agree() {
    let doc = run_study(&scenario(&synthetic())).unwrap();
    assert!(doc["forecast"]["label"]
        .as_str()
        .unwrap()
        .starts_with("MODELLED"));
    let fc = &doc["forecast"]["craft"][0];
    let craft = &doc["craft"][0];
    // About 15.5 revolutions a day: 16 or 17 (partial) revolutions in the span.
    let n_rev = fc["revolutions"].as_u64().unwrap();
    assert!((16..=17).contains(&n_rev), "{n_rev}");
    let orbits = fc["orbits"].as_array().unwrap();
    assert_eq!(orbits.len() as u64, n_rev);
    let exp: f64 = orbits
        .iter()
        .map(|o| o["exposure_s"].as_f64().unwrap())
        .sum();
    assert_eq!(exp, craft["exposure_s"].as_f64().unwrap());
    assert_eq!(fc["totals"]["exposure_s"], craft["exposure_s"]);
    let mut any_outage = false;
    for o in orbits {
        assert!(
            o["expected_exposure_s"].as_f64().unwrap() <= o["exposure_s"].as_f64().unwrap() + 1e-6
        );
        let (det, ex, p) = (
            arr(&o["outage_s"]),
            arr(&o["expected_outage_s"]),
            arr(&o["p_outage"]),
        );
        for i in 0..2 {
            assert!(ex[i] <= det[i] + 1e-6, "{o}");
            assert!((0.0..=1.0).contains(&p[i]));
            any_outage |= det[i] > 0.0;
        }
        // A stronger interferer never gives less outage.
        assert!(det[1] >= det[0]);
    }
    assert!(
        any_outage,
        "60 dBW on an overhead ISS pass is a modelled outage"
    );
    // Per pass: the windows hold exactly the steps under the threshold, plus reacquisition.
    for p in craft["passes"].as_array().unwrap() {
        let w = p["outage_windows"].as_array().unwrap();
        for (i, below) in arr(&p["below_threshold_s"]).iter().enumerate() {
            let ws = w[i].as_array().unwrap();
            let loss: f64 = ws.iter().map(|x| x["loss_s"].as_f64().unwrap()).sum();
            assert_eq!(loss, *below);
            for x in ws {
                assert!(x["outage_s"].as_f64().unwrap() >= x["loss_s"].as_f64().unwrap() + 20.0);
                assert!(x["worst_margin_db"].as_f64().unwrap() < 0.0);
                assert!(!x["areas"].as_array().unwrap().is_empty());
            }
        }
    }
    assert!(outages_csv(&doc).lines().count() > 2);
    assert_eq!(orbits_csv(&doc).lines().count(), 2 + n_rev as usize);
    assert!(report_md(&doc).contains("## Per-revolution forecast"));
}

/// One stated area active half the time: the exact enumeration halves the outage and the
/// exposure, and the outage probability of a revolution with an outage is one half.
#[test]
fn activity_is_enumerated_exactly() {
    let src = format!(
        "{ISS}assumed_eirp_dbw = [60.0]\n[forecast]\n[[area]]\nlabel = \"half\"\nlat_deg = 45.0\nlon_deg = -30.0\nradius_km = 200.0\nactivity = 0.5\n"
    );
    let doc = run_study(&scenario(&src)).unwrap();
    let mut seen = 0;
    for o in doc["forecast"]["craft"][0]["orbits"].as_array().unwrap() {
        let det = o["outage_s"][0].as_f64().unwrap();
        assert!(
            (o["expected_exposure_s"].as_f64().unwrap() - 0.5 * o["exposure_s"].as_f64().unwrap())
                .abs()
                < 1e-6
        );
        assert!((o["expected_outage_s"][0].as_f64().unwrap() - 0.5 * det).abs() < 1e-6);
        if det > 0.0 {
            seen += 1;
            assert_eq!(o["p_outage"][0], 0.5);
            assert_eq!(o["activity_method"], "exact");
        }
    }
    assert!(seen > 0);
    let t = &doc["forecast"]["craft"][0]["totals"];
    assert!(
        (t["expected_revolutions_with_outage"][0].as_f64().unwrap()
            - 0.5 * t["revolutions_with_outage"][0].as_f64().unwrap())
        .abs()
            < 1e-9
    );
    // Out-of-range activity is refused.
    assert!(run_study(&scenario(&src.replace("activity = 0.5", "activity = -0.1"))).is_err());
}

/// More uncertain areas than the exact enumeration takes: a seeded Monte Carlo draw, the same
/// on every run, between nothing and the all-active outage.
#[test]
fn many_uncertain_areas_use_a_seeded_draw() {
    let mut src =
        format!("{ISS}assumed_eirp_dbw = [55.0]\n[forecast]\nmonte_carlo_samples = 256\n");
    for i in 0..12 {
        src.push_str(&format!(
            "[[area]]\nlabel = \"a{i}\"\nlat_deg = 45.0\nlon_deg = {}\nradius_km = 50.0\nactivity = 0.3\n",
            -40.0 + 1.5 * i as f64
        ));
    }
    let s = scenario(&src);
    let a = run_study(&s).unwrap();
    assert_eq!(a, run_study(&s).unwrap());
    let orbits = a["forecast"]["craft"][0]["orbits"].as_array().unwrap();
    let mc: Vec<&Value> = orbits
        .iter()
        .filter(|o| o["activity_method"] == "monte-carlo")
        .collect();
    assert!(!mc.is_empty());
    for o in mc {
        assert!(o["uncertain_areas"].as_u64().unwrap() > 10);
        let (det, ex, p) = (
            o["outage_s"][0].as_f64().unwrap(),
            o["expected_outage_s"][0].as_f64().unwrap(),
            o["p_outage"][0].as_f64().unwrap(),
        );
        assert!(ex <= det + 1e-6 && (0.0..=1.0).contains(&p));
    }
}

/// The link budget at a pass's worst step is the stated model evaluated by hand: the pattern
/// gain at the angle from boresight, the polarisation loss, free space at the range, the
/// anti-jam equation; and it equals the pass's worst margin.
#[test]
fn link_budget_matches_the_model_by_hand() {
    let src = format!(
        "{ISS}assumed_eirp_dbw = [30.0, 50.0]\n[forecast.link]\npolarisation_loss_db = 2.0\nother_losses_db = 1.0\n[[area]]\nlabel = \"one\"\nlat_deg = 45.0\nlon_deg = -30.0\nradius_km = 100.0\n"
    );
    let doc = run_study(&scenario(&src)).unwrap();
    let pattern = [
        (0.0, 3.0),
        (30.0, 2.0),
        (60.0, -1.0),
        (80.0, -4.0),
        (90.0, -7.0),
        (100.0, -10.0),
        (120.0, -15.0),
        (150.0, -20.0),
        (180.0, -22.0),
    ];
    let gain = |x: f64| {
        let i = pattern.windows(2).position(|w| x <= w[1].0).unwrap();
        let (a, b) = (pattern[i], pattern[i + 1]);
        a.1 + (x - a.0) / (b.0 - a.0) * (b.1 - a.1)
    };
    let k_t = 10.0 * (1.380_649e-23f64 * 290.0).log10();
    let cn0 = -158.5 - k_t;
    let mut n = 0;
    for p in doc["craft"][0]["passes"].as_array().unwrap() {
        let lb = &p["link_budget"];
        let off = lb["off_boresight_deg"].as_f64().unwrap();
        assert!(
            off > 90.0,
            "a ground point is below the spacecraft's horizontal plane"
        );
        assert!((lb["rx_gain_dbi"].as_f64().unwrap() - gain(off)).abs() < 1e-6);
        let d = lb["range_km"].as_f64().unwrap() * 1000.0;
        let fspl = 20.0 * d.log10()
            + 20.0 * 1_575_420_000f64.log10()
            + 20.0 * (4.0 * std::f64::consts::PI / 299_792_458.0).log10();
        for (i, e) in [30.0, 50.0].iter().enumerate() {
            let js = e + gain(off) - 2.0 - 1.0 - fspl + 158.5;
            let eff =
                -10.0 * (10f64.powf(-cn0 / 10.0) + 10f64.powf(js / 10.0) / (2.0 * 1.023e6)).log10();
            let m = lb["per_eirp"][i]["margin_db"].as_f64().unwrap();
            assert!((m - (eff - 25.0)).abs() < 1e-5, "{m} vs {}", eff - 25.0);
            assert!((p["min_margin_db"][i].as_f64().unwrap() - m).abs() < 1e-5);
        }
        n += 1;
    }
    assert!(n > 0);
    assert!(doc["assumptions"]["link_model"]["replaces"]
        .as_str()
        .unwrap()
        .contains("gain_toward_ground_dbi"));
}

fn exported(src: &str) -> (Value, Vec<ExportedScenario>) {
    run_study_with_scenarios(&scenario(src), &ent(true), 1).unwrap()
}

#[test]
fn exported_scenarios_are_valid_neutral_test_scenarios() {
    let src = format!("{}[export]\ninclude = \"degraded\"\n", synthetic());
    let (doc, ex) = exported(&src);
    let idx = doc["test_scenarios"]["scenarios"].as_array().unwrap();
    assert!(!ex.is_empty());
    assert_eq!(idx.len(), ex.len());
    assert_eq!(
        doc["test_scenarios"]["eirp_dbw"], 60.0,
        "default: the largest EIRP"
    );
    let mut ids = std::collections::BTreeSet::new();
    let mut worst_prev = f64::NEG_INFINITY;
    for (x, i) in ex.iter().zip(idx) {
        assert!(ids.insert(x.id.clone()));
        assert_eq!(i["id"], x.id.as_str());
        assert_eq!(i["sha256"], x.sha256.as_str());
        assert_eq!(x.file, format!("scenarios/{}.toml", x.id));
        let w = i["worst_margin_db"].as_f64().unwrap();
        assert!(w >= worst_prev, "worst margin first");
        worst_prev = w;
        let spec = Spec::parse(&x.toml).unwrap_or_else(|e| panic!("{}: {e}", x.id));
        assert_eq!(spec.sha256(), x.sha256);
        assert_eq!(spec.evidence, "MODELLED");
        assert!(spec.tags.contains(&"spacecraft".to_string()));
        // Truth: samples in time order, longitudes on the map, heights in low orbit.
        let tr = &spec.trajectory;
        assert!(!tr.is_static());
        assert!(tr.samples.windows(2).all(|w| w[1].t_s > w[0].t_s));
        assert!(tr
            .samples
            .iter()
            .all(|p| p.lon_deg.abs() <= 180.0 && (3.0e5..5.0e5).contains(&p.height_m)));
        assert!(tr.samples.last().unwrap().t_s <= spec.scene.duration_s + 1e-9);
        // Threats: wideband jamming at L1, quantised, inside the scene, over the floor.
        assert!(!spec.threats.is_empty());
        for t in &spec.threats {
            assert_eq!(t.center_hz, Some(1_575_420_000.0));
            let js = t.js_db.unwrap();
            assert!(js >= 10.0 && (js / 3.0 - (js / 3.0).round()).abs() < 1e-9);
            assert!(t.end_s() <= spec.scene.duration_s);
        }
        let ids: Vec<&str> = spec.criteria.iter().map(|c| c.id.as_str()).collect();
        match i["predicted"].as_str().unwrap() {
            "outage" => assert!(ids.contains(&"detect")),
            "degraded" | "kept" => assert!(ids.contains(&"available")),
            o => panic!("{o}"),
        }
        assert!(ids.contains(&"no-misleading") && ids.contains(&"recover"));
        // Kshana Pro's own validator says what each simulator adapter can express.
        for a in kshana_pro::testkit::compile::ADAPTERS {
            let v = kshana_pro::testkit::compile::validate(&spec, a).unwrap();
            assert_eq!(v.scenario, spec.id);
        }
    }
    // The same scenario gives the same files.
    let (_, again) = exported(&src);
    assert_eq!(
        ex.iter().map(|x| &x.toml).collect::<Vec<_>>(),
        again.iter().map(|x| &x.toml).collect::<Vec<_>>()
    );
    // The selection widens from outage to degraded to all.
    let n = |inc: &str| {
        let (d, _) = exported(&src.replace("\"degraded\"", &format!("\"{inc}\"")));
        d["test_scenarios"]["passes_selected"].as_u64().unwrap()
    };
    let (o, d, a) = (n("outage"), n("degraded"), n("all"));
    assert!(o <= d && d <= a && o > 0, "{o} {d} {a}");
    // Bad options are refused.
    let s = scenario(&format!("{}[export]\neirp_dbw = 55.0\n", synthetic()));
    assert!(run_study(&s).is_err());
    let s = scenario(&format!("{}[export]\ninclude = \"some\"\n", synthetic()));
    assert!(run_study(&s).is_err());
    let s = scenario(&format!("{}[export]\nsample_step_s = 0.0\n", synthetic()));
    assert!(run_study(&s).is_err());
}

#[test]
fn the_study_directory_with_forecast_and_scenarios_is_signed() {
    let src = format!("{}[export]\nmax_scenarios = 2\n", synthetic());
    let (doc, ex) = exported(&src);
    assert_eq!(ex.len(), 2);
    let dir = std::env::temp_dir().join(format!("arksa-ak4-signed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seed = [11u8; 32];
    write_study_with_scenarios(
        &doc,
        &ex,
        &dir,
        Some((&seed, "Arksa test", "2026-11-20T00:00:00Z")),
    )
    .unwrap()
    .unwrap();
    for f in [
        "study.json",
        "passes.csv",
        "report.md",
        "orbits.csv",
        "outages.csv",
    ] {
        assert!(dir.join(f).is_file(), "{f}");
    }
    assert!(dir.join(&ex[0].file).is_file());
    let pk = kshana_pro::license::public_key_for(&seed);
    let v = kshana_pro::signed_dossier::verify(&dir, &[pk]).unwrap();
    assert!(v.passed(), "{:?}", v.problems);
    assert_eq!(v.files, 7);
    std::fs::write(dir.join(&ex[1].file), "edited").unwrap();
    assert!(!kshana_pro::signed_dossier::verify(&dir, &[pk])
        .unwrap()
        .passed());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The entitled entry points need the Arksa claim; the refusal names no kind and no method.
#[test]
fn the_entitled_entry_points_need_the_arksa_claim() {
    arksa_kpro::ensure_registered();
    let s = scenario(&format!("{}[export]\n", synthetic()));
    for e in [ent(false), Entitlement::evaluation()] {
        for msg in [
            run_study_with_scenarios(&s, &e, 1).unwrap_err().to_string(),
            run_study_entitled(&s, &e, 1).unwrap_err().to_string(),
            arksa_kpro::entitlement::ensure_rx_exposure_entitled(&e, 1)
                .unwrap_err()
                .to_string(),
        ] {
            assert!(msg.contains(NOT_IN_THIS_EDITION), "{msg}");
            assert!(
                !msg.contains("spaceborne") && !msg.contains("exposure"),
                "{msg}"
            );
        }
    }
    assert!(run_study_entitled(&s, &ent(true), 1).is_ok());
    assert!(arksa_kpro::entitlement::ensure_rx_exposure_entitled(&ent(true), 1).is_ok());
}

/// Without `[forecast]` and `[export]` the study is the F4 study as before (its committed
/// hash is pinned in `tests/spaceborne_rx_exposure.rs`).
#[test]
fn without_the_new_sections_nothing_new_is_written() {
    let doc = run_study(&scenario(&format!(
        "{ISS}region_files = [\"synthetic-regions.geojson\"]\n"
    )))
    .unwrap();
    assert!(doc.get("forecast").is_none());
    assert!(doc.get("test_scenarios").is_none());
    assert!(doc["assumptions"].get("link_model").is_none());
    for p in doc["craft"][0]["passes"].as_array().unwrap() {
        assert!(p.get("outage_windows").is_none() && p.get("link_budget").is_none());
    }
}

/// The committed v2 study: the public inputs of the F4 study with the forecast and the
/// export (`papers/spaceborne-rx-exposure-v2/`).
const PUBLIC_V2: &str = r#"
kind = "pro-spaceborne-rx-exposure"
title = "Per-revolution exposure forecast of four LEO GNSS receivers to the located Sep 2025 northern Baltic emitter-region sessions, with matching receiver test scenarios"
tle_file = "leo-celestrak-2026-11-15.tle"
start_utc = "2026-11-15T00:00Z"
duration_h = 24.0
step_s = 30.0
assumed_eirp_dbw = [20.0, 40.0, 50.0]
emitter_region_files = ["emitter-adsb-adsb-lol-2025-09-14_2025-09-19-summary.json"]

[forecast]
reacquisition_s = 20.0

[export]
eirp_dbw = 50.0
include = "outage"
max_scenarios = 2
"#;

const RESULT_DIR: &str = "papers/spaceborne-rx-exposure-v2";

#[test]
fn committed_v2_study_matches_a_fresh_run() {
    let (doc, ex) = exported(PUBLIC_V2);
    assert_eq!(doc["evidence_class"], "MODELLED");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(RESULT_DIR);
    if std::env::var("ARKSA_REGEN_F4").is_ok() {
        let _ = std::fs::remove_dir_all(dir.join("scenarios"));
        write_study_with_scenarios(&doc, &ex, &dir, None).unwrap();
        std::fs::write(dir.join("scenario.toml"), PUBLIC_V2.trim_start()).unwrap();
    }
    let committed: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("study.json")).unwrap()).unwrap();
    assert_eq!(committed["result_sha256"], doc["result_sha256"]);
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();
    assert_eq!(read("passes.csv"), passes_csv(&doc));
    assert_eq!(read("orbits.csv"), orbits_csv(&doc));
    assert_eq!(read("outages.csv"), outages_csv(&doc));
    for x in &ex {
        assert_eq!(read(&x.file), x.toml, "{}", x.file);
    }
    assert_eq!(
        std::fs::read_dir(dir.join("scenarios")).unwrap().count(),
        ex.len()
    );
}
