// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! F4, spaceborne GNSS receiver interference exposure: the study on public inputs (CelesTrak
//! element sets and the open repository's adsb.lol-derived emitter-region summary), the time
//! modes, an independent check of the margin, the signed study directory, and the committed
//! study output.
//!
//! Regenerate the committed study (only after a deliberate change):
//! `ARKSA_REGEN_F4=1 cargo test --test spaceborne_rx_exposure committed`.

use std::path::{Path, PathBuf};

use arksa_kpro::packs::spaceborne_rx_exposure_pack::{
    parse_tles, passes_csv, report_md, run_study, write_study, SpaceborneRxExposureScenario,
};
use kshana::api::Scenario;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/f4")
}

fn scenario(toml_text: &str) -> SpaceborneRxExposureScenario {
    let mut s: SpaceborneRxExposureScenario = toml::from_str(toml_text).unwrap();
    s.resolve_files(&fixtures()).unwrap();
    s
}

/// The public study: four spacecraft, one day from 15 November 2026, the located sessions of
/// the adsb.lol emitter-region summary (14 to 19 September 2025) as a climatology.
const PUBLIC: &str = r#"
kind = "pro-spaceborne-rx-exposure"
title = "Exposure of four LEO GNSS receivers to the located Sep 2025 northern Baltic emitter-region sessions"
tle_file = "leo-celestrak-2026-11-15.tle"
start_utc = "2026-11-15T00:00Z"
duration_h = 24.0
step_s = 30.0
emitter_region_files = ["emitter-adsb-adsb-lol-2025-09-14_2025-09-19-summary.json"]
"#;

const RESULT_DIR: &str = "papers/spaceborne-rx-exposure";

#[test]
fn public_study_runs_and_is_labelled_honestly() {
    let doc = run_study(&scenario(PUBLIC)).unwrap();
    assert_eq!(doc["evidence_class"], "MODELLED");
    assert!(doc["label"].as_str().unwrap().contains("MEASURED: nothing"));
    // The maps are from 2025, the orbit from 2026: a climatology, and it says so.
    assert_eq!(doc["span"]["time_mode"], "climatology");
    let inp = &doc["inputs"];
    assert_eq!(inp["sessions_located"], 4);
    assert_eq!(inp["sessions_not_located"], 10);
    assert_eq!(inp["emitter_regions"][0]["licence"], "ODbL-1.0");
    let craft = doc["craft"].as_array().unwrap();
    assert_eq!(craft.len(), 4);
    for c in craft {
        let name = c["name"].as_str().unwrap();
        let passes = c["passes"].as_array().unwrap();
        assert!(
            !passes.is_empty(),
            "{name} passes the northern Baltic in a day"
        );
        assert!(c["tle_age_days_at_mid_span"].as_f64().unwrap().abs() < 2.0);
        // A stronger assumed interferer never leaves a larger margin.
        let m: Vec<f64> = c["min_margin_db"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap())
            .collect();
        assert!(m.windows(2).all(|w| w[1] <= w[0]), "{name}: {m:?}");
        for p in passes {
            assert!(p["over_area_s"].as_f64().unwrap() <= p["duration_s"].as_f64().unwrap());
            assert!(p["max_elevation_deg"].as_f64().unwrap() >= 0.0);
        }
    }
    assert!(report_md(&doc).contains("Evidence class: MODELLED"));
    assert!(passes_csv(&doc).lines().count() > 4);
}

#[test]
fn the_same_scenario_gives_the_same_result() {
    let a = run_study(&scenario(PUBLIC)).unwrap();
    let b = run_study(&scenario(PUBLIC)).unwrap();
    assert_eq!(a, b);
}

/// The worst margin of a pass over one stated area equals the anti-jam equation evaluated by
/// hand at the pass's shortest range.
#[test]
fn margin_matches_the_anti_jam_equation_by_hand() {
    let s = scenario(
        r#"
tle_file = "leo-celestrak-2026-11-15.tle"
start_utc = "2026-11-15T00:00Z"
duration_h = 12.0
step_s = 10.0
assumed_eirp_dbw = [30.0]
[[area]]
label = "one"
lat_deg = 60.0
lon_deg = 25.0
radius_km = 50.0
"#,
    );
    let doc = run_study(&s).unwrap();
    let k_t = 10.0 * (1.380_649e-23f64 * 290.0).log10();
    let cn0 = -158.5 - k_t;
    for c in doc["craft"].as_array().unwrap() {
        for p in c["passes"].as_array().unwrap() {
            let d = p["min_range_km"].as_f64().unwrap() * 1000.0;
            let fspl = 20.0 * d.log10()
                + 20.0 * 1_575_420_000f64.log10()
                + 20.0 * (4.0 * std::f64::consts::PI / 299_792_458.0).log10();
            let js = 30.0 - 15.0 - fspl - (-158.5);
            let eff =
                -10.0 * (10f64.powf(-cn0 / 10.0) + 10f64.powf(js / 10.0) / (2.0 * 1.023e6)).log10();
            let m = p["min_margin_db"][0].as_f64().unwrap();
            assert!((m - (eff - 25.0)).abs() < 1e-6, "{m} vs {}", eff - 25.0);
        }
    }
}

/// Coincident mode counts a map cell only on its own day.
#[test]
fn time_modes() {
    let base = r#"
tle = """
ISS (ZARYA)
1 25544U 98067A   26319.20611933  .00010382  00000+0  19362-3 0  9993
2 25544  51.6322 272.7605 0007377  27.6155 332.5225 15.49388394595162
"""
duration_h = 24.0
step_s = 30.0
map_files = ["synthetic-adsb-custom-2026-03-01.geojson"]
"#;
    // The synthetic map (2026-03-01, mid-Atlantic, one degraded cell) is outside the span.
    let clim = run_study(&scenario(base)).unwrap();
    assert_eq!(clim["span"]["time_mode"], "climatology");
    assert_eq!(clim["inputs"]["map_cells_degraded"], 1);
    let co = run_study(&scenario(&format!("{base}time_mode = \"coincident\"\n"))).unwrap();
    assert_eq!(co["span"]["time_mode"], "coincident");
    assert!(co["craft"][0]["passes"].as_array().unwrap().is_empty());
    assert!(!clim["craft"][0]["passes"].as_array().unwrap().is_empty());
    // Wrong mode, no area, broken element sets: refused.
    assert!(run_study(&scenario(&format!("{base}time_mode = \"sometimes\"\n"))).is_err());
    let none = scenario(&base.replace(
        "map_files = [\"synthetic-adsb-custom-2026-03-01.geojson\"]\n",
        "",
    ));
    assert!(run_study(&none).is_err());
    assert!(parse_tles("1 25544U only one line").is_err());
}

#[test]
fn the_study_directory_is_signed_and_tamper_evident() {
    let doc = run_study(&scenario(PUBLIC)).unwrap();
    let dir = std::env::temp_dir().join(format!("arksa-f4-signed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seed = [9u8; 32];
    let sig = write_study(
        &doc,
        &dir,
        Some((&seed, "Arksa test", "2026-11-15T00:00:00Z")),
    )
    .unwrap()
    .unwrap();
    assert!(sig.ends_with("signature.json"));
    let pk = kshana_pro::license::public_key_for(&seed);
    let v = kshana_pro::signed_dossier::verify(&dir, &[pk]).unwrap();
    assert!(v.passed(), "{:?}", v.problems);
    assert_eq!(v.files, 3);
    std::fs::write(dir.join("report.md"), "edited").unwrap();
    assert!(!kshana_pro::signed_dossier::verify(&dir, &[pk])
        .unwrap()
        .passed());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_pack_runs_through_the_scenario_trait() {
    // Relative files resolve against the working directory on this route (here the crate root).
    let src = PUBLIC
        .replace("leo-celestrak", "tests/fixtures/f4/leo-celestrak")
        .replace("\"emitter-adsb", "\"tests/fixtures/f4/emitter-adsb");
    let s: SpaceborneRxExposureScenario = toml::from_str(&src).unwrap();
    let out = s.run().unwrap();
    assert!(out.summary.starts_with("MODELLED"));
    assert!(out.csv.unwrap().starts_with("# MODELLED"));
}

/// The committed study (`papers/spaceborne-rx-exposure/`) is what the public scenario gives.
#[test]
fn committed_study_matches_a_fresh_run() {
    let doc = run_study(&scenario(PUBLIC)).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(RESULT_DIR);
    if std::env::var("ARKSA_REGEN_F4").is_ok() {
        write_study(&doc, &dir, None).unwrap();
        std::fs::write(dir.join("scenario.toml"), PUBLIC.trim_start()).unwrap();
    }
    let committed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("study.json")).unwrap()).unwrap();
    assert_eq!(committed["result_sha256"], doc["result_sha256"]);
    assert_eq!(
        std::fs::read_to_string(dir.join("passes.csv")).unwrap(),
        passes_csv(&doc)
    );
}
