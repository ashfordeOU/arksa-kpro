// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! M9, the space tip-and-cue study: the four pre-registered bars
//! (`docs/evidence/m9-space-tip-cue/PREREGISTRATION-2026-11-15.md`) and the committed result.
//!
//! Regenerate the committed result (only after a deliberate change, never to make a bar pass):
//! `ARKSA_REGEN_M9=1 cargo test --test space_tip_cue_study committed`.

use arksa_kpro::packs::space_tip_cue_study_pack::{
    case, points, run_designs, run_study, table_csv, SpaceTipCueStudyScenario,
};
use kshana::api::{ExternalPack, Scenario};
use kshana::tip_and_cue::{self as tc, Grid};

fn scenario(extra: &str) -> SpaceTipCueStudyScenario {
    toml::from_str(&format!("kind = \"pro-space-tip-cue-study\"\n{extra}")).unwrap()
}

const RESULT_DIR: &str = "papers/space-tip-cue";

#[test]
fn defaults_are_the_preregistered_values() {
    let s = scenario("");
    assert_eq!(s.seed, 9009);
    assert_eq!(s.planes, vec![4, 8, 16]);
    assert_eq!(s.per_plane, vec![1, 2, 4]);
    assert_eq!((s.span_days, s.step_s, s.thin), (3, 20, 2));
    assert_eq!((s.rf_mask_deg, s.optical_off_nadir_deg), (10.0, 45.0));
    assert_eq!((s.mc_cues, s.sigma_fix_m, s.p_turn), (4000, 1000.0, 0.25));
    assert_eq!(s.speed_kn, [16.0, 22.0]);
    assert_eq!(s.turn_deg, [20.0, 90.0]);
    assert_eq!(s.radii_km, vec![2.5, 5.0, 10.0]);
    assert_eq!(s.cost.years, 5.0);
    assert_eq!(s.kind_name(), "pro-space-tip-cue-study");
    // Thinning keeps every second cell of the M2 sea points.
    let n = points(2).len();
    assert!(n > 200 && n < 300, "{n}");
    assert_eq!(points(1).len(), tc::baltic_points().len());
}

/// M9-R1: same scenario and seed, byte-identical result.
#[test]
fn r1_reproduces_from_scenario_and_seed() {
    let s = scenario("");
    let a = s.run().unwrap();
    let b = s.run().unwrap();
    assert_eq!(a.json, b.json);
    assert_eq!(a.csv, b.csv);
    let v: serde_json::Value = serde_json::from_str(&a.json).unwrap();
    assert_eq!(v["result_sha256"].as_str().unwrap().len(), 64);
    assert!(v["label"].as_str().unwrap().starts_with("MODELLED"));
    assert_eq!(v["designs"].as_array().unwrap().len(), 9);
}

/// M9-R2: another seed moves each capture rate by no more than Monte Carlo noise.
#[test]
fn r2_seed_stability() {
    let (a, _) = run_designs(&scenario("")).unwrap();
    let (b, _) = run_designs(&scenario("seed = 9010")).unwrap();
    for (x, y) in a.iter().zip(&b) {
        let p = x.capture.rate[1];
        let n = x.capture.samples as f64;
        let tol = (4.0 * (2.0 * p * (1.0 - p) / n).sqrt()).max(0.01);
        eprintln!(
            "R2 {}x{}: seed 9009 {p:.4}, seed 9010 {:.4}, |d| {:.4}, tol {tol:.4}",
            x.planes,
            x.per_plane,
            y.capture.rate[1],
            (p - y.capture.rate[1]).abs()
        );
        assert!(
            (p - y.capture.rate[1]).abs() <= tol,
            "{}x{}: {} vs {} (tol {tol})",
            x.planes,
            x.per_plane,
            p,
            y.capture.rate[1]
        );
    }
}

/// M9-R3: the pack's R, F and Q are the open M2 planner's on the same points and grid.
#[test]
fn r3_figures_are_the_open_planners() {
    let s = scenario("planes = [8, 16]\nper_plane = [2, 4]");
    let (ds, _) = run_designs(&s).unwrap();
    let pts = points(s.thin);
    let grid = Grid {
        step_s: s.step_s,
        span_s: s.span_days * 86_400,
    };
    let cases: Vec<_> = [(8, 2), (16, 2), (8, 4)]
        .iter()
        .map(|&(p, k)| case(p, k, 10.0, 45.0))
        .collect();
    let open = tc::run(&cases, &pts, grid).unwrap();
    let close = |a: Option<f64>, b: Option<f64>| {
        let (a, b) = (a.unwrap(), b.unwrap());
        (a - b).abs() <= 1e-12 * b.abs().max(1.0)
    };
    for (c, o) in cases.iter().zip(&open) {
        let d = ds
            .iter()
            .find(|d| d.satellites_per_layer == c.rf.total && d.planes == c.rf.planes)
            .unwrap();
        assert!(close(d.revisit_mean_s, o.revisit.value_s), "{} R", c.name);
        assert!(
            close(d.refresh_mean_s, o.full_refresh.value_s),
            "{} F",
            c.name
        );
        assert!(close(d.reaction_mean_s, o.reaction.value_s), "{} Q", c.name);
        assert_eq!(d.cues_unanswered, o.cues_unanswered, "{}", c.name);
    }
}

/// M9-R4: without turns the Monte Carlo agrees with the closed form.
#[test]
fn r4_monte_carlo_against_closed_form() {
    let (ds, _) = run_designs(&scenario("p_turn = 0.0")).unwrap();
    for d in &ds {
        let (mc, cf, sd) = (
            d.capture.rate[1],
            d.capture.closed_form_no_turn[1],
            d.capture.closed_form_sd[1],
        );
        let tol = (4.0 * sd).max(0.005);
        eprintln!(
            "R4 {}x{}: MC {mc:.4}, closed form {cf:.4}, |d| {:.4}, tol {tol:.4}",
            d.planes,
            d.per_plane,
            (mc - cf).abs()
        );
        // Unanswered cues are misses in the Monte Carlo and add nothing to the closed form.
        assert!(
            (mc - cf).abs() <= tol,
            "{}x{}: MC {mc} closed form {cf} (tol {tol})",
            d.planes,
            d.per_plane
        );
    }
}

#[test]
fn bad_scenarios_are_refused() {
    for extra in [
        "planes = []",
        "per_plane = [0]",
        "span_days = 1",
        "p_turn = 1.5",
        "speed_kn = [22.0, 16.0]",
        "nonsense = 1",
    ] {
        let r: Result<SpaceTipCueStudyScenario, _> =
            toml::from_str(&format!("kind = \"pro-space-tip-cue-study\"\n{extra}"));
        match r {
            Ok(s) => assert!(s.run().is_err(), "{extra}"),
            Err(_) => assert_eq!(extra, "nonsense = 1"),
        }
    }
}

/// The committed result (`papers/space-tip-cue/`) is what the pre-registered scenario gives.
#[test]
fn committed_result_matches_a_fresh_run() {
    let doc = run_study(&scenario("")).unwrap();
    let csv = table_csv(&doc);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(RESULT_DIR);
    if std::env::var("ARKSA_REGEN_M9").is_ok() {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("trade.csv"), &csv).unwrap();
        std::fs::write(
            dir.join("result.json"),
            serde_json::to_string_pretty(&doc).unwrap() + "\n",
        )
        .unwrap();
    }
    let committed = std::fs::read_to_string(dir.join("trade.csv")).unwrap();
    assert_eq!(committed, csv);
    let j: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("result.json")).unwrap()).unwrap();
    assert_eq!(j["result_sha256"], doc["result_sha256"]);
}
