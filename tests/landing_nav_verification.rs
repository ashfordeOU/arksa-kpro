// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Landing-navigation fault-handling verification on synthetic descents.
//!
//! The filter logs here are synthesised from the truth with a chosen behaviour (how fast
//! it flags a fault, whether it latches on a terrain step, whether it recovers, how
//! honest its sigma is), so each test knows the verdict the harness must reach. No
//! navigation filter is implemented here or anywhere in the crate.

use std::path::{Path, PathBuf};
use std::process::Command;

use arksa_kpro::landing::campaign::Campaign;
use arksa_kpro::landing::faults::{self, Expect, FaultType};
use arksa_kpro::landing::verify::{self, Check, LogInput, Status, Verdict, Verification};
use arksa_kpro::mbse::evidence::sha256_hex;

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios/landing-nav/campaign.toml")
}

fn fixture_text() -> String {
    std::fs::read_to_string(fixture_path()).unwrap()
}

fn fixture() -> Campaign {
    Campaign::parse(&fixture_text()).unwrap()
}

fn tmp(tag: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("landing-nav-{tag}"));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// How the synthesised filter behaves.
#[derive(Clone)]
struct Behaviour {
    detect_delay_s: f64,
    recover_delay_s: f64,
    /// Declare every sensor faulty, for good, when a terrain step begins (ispace M1).
    latch_on_terrain: bool,
    /// Never flag any fault.
    never_detect: bool,
    /// Take a detected faulty sensor back after this long.
    reaccept_after_s: Option<f64>,
    alt_sigma_m: f64,
    /// This sensor is logged `off` throughout.
    sensor_off: Option<&'static str>,
    /// Stop logging at this time.
    truncate_at_s: Option<f64>,
}

impl Default for Behaviour {
    fn default() -> Self {
        Behaviour {
            detect_delay_s: 0.5,
            recover_delay_s: 0.5,
            latch_on_terrain: false,
            never_detect: false,
            reaccept_after_s: None,
            alt_sigma_m: 1.0,
            sensor_off: None,
            truncate_at_s: None,
        }
    }
}

/// A filter log for scenario `i` with behaviour `b`.
fn synth_log(c: &Campaign, i: usize, b: &Behaviour) -> String {
    let sc = &c.scenarios[i];
    let td = c.descent.touchdown_s();
    let mut s = String::from("# synthesised\nt_s,alt_est_m,alt_sigma_m,vz_est_mps,vz_sigma_mps");
    for sn in &c.sensors {
        s.push_str(&format!(",{0}_health,{0}_used", sn.id));
    }
    s.push_str(",mode\n");
    let terrain_start = sc
        .terrain
        .iter()
        .map(|t| t.t_start_s)
        .fold(f64::INFINITY, f64::min);
    for t in faults::sample_times(c) {
        if matches!(b.truncate_at_s, Some(x) if t > x) {
            break;
        }
        let tr = c.descent.truth(t);
        s.push_str(&format!(
            "{t:.3},{:.4},{:.3},{:.4},0.2",
            tr.alt_m + 0.3 * (0.7 * t).sin(),
            b.alt_sigma_m,
            tr.vz_mps + 0.05 * (0.3 * t).cos()
        ));
        for sn in &c.sensors {
            let mut health = "ok";
            let mut used = sn.in_range(faults::hat(c, sc, t));
            if !b.never_detect {
                for f in sc.faults.iter().filter(|f| f.sensor == sn.id) {
                    if f.expect() != Expect::Detect {
                        continue;
                    }
                    let Some(t0) = verify::onset(c, sc, sn, f) else {
                        continue;
                    };
                    let flag_at = t0 + b.detect_delay_s;
                    let end = f.end(td);
                    let back = if f.recover(td) {
                        faults::first_in_range(c, sc, sn, end, td)
                            .map(|x| x + b.recover_delay_s)
                            .unwrap_or(f64::INFINITY)
                    } else {
                        f64::INFINITY
                    };
                    let back = match b.reaccept_after_s {
                        Some(r) => back.min(flag_at + r),
                        None => back,
                    };
                    if t >= flag_at - 1e-9 && t < back - 1e-9 {
                        health = "fault";
                        used = false;
                    }
                }
            }
            if b.latch_on_terrain && t >= terrain_start {
                health = "fault";
                used = false;
            }
            if b.sensor_off == Some(sn.id.as_str()) {
                health = "off";
                used = false;
            }
            s.push_str(&format!(",{health},{}", used as u8));
        }
        s.push_str(",descent\n");
    }
    s
}

fn logs_for(c: &Campaign, b: &Behaviour) -> Vec<Option<LogInput>> {
    (0..c.scenarios.len())
        .map(|i| {
            Some(LogInput {
                file_name: format!("{}.csv", c.scenarios[i].id),
                bytes: synth_log(c, i, b).into_bytes(),
            })
        })
        .collect()
}

fn run(c: &Campaign, b: &Behaviour) -> Verification {
    verify::verify(c, "0".repeat(64).as_str(), &logs_for(c, b), None)
}

fn scenario<'a>(v: &'a Verification, id: &str) -> &'a verify::ScenarioResult {
    v.body.scenarios.iter().find(|r| r.id == id).unwrap()
}

fn failed(r: &verify::ScenarioResult) -> Vec<&Check> {
    r.checks
        .iter()
        .filter(|k| k.status == Status::Fail)
        .collect()
}

fn has_fail(r: &verify::ScenarioResult, check: &str, sensor: Option<&str>) -> bool {
    failed(r)
        .iter()
        .any(|k| k.check == check && (sensor.is_none() || k.sensor.as_deref() == sensor))
}

// ------------------------------------------------------------------ campaign and library

#[test]
fn the_fixture_campaign_parses_and_its_descent_is_consistent() {
    let c = fixture();
    assert_eq!(c.scenarios.len(), 10);
    let td = c.descent.touchdown_s();
    assert!((td - 84.065).abs() < 0.001, "{td}");
    let t0 = c.descent.truth(0.0);
    assert_eq!((t0.alt_m, t0.vz_mps), (2000.0, -60.0));
    let tb = c.descent.truth(c.descent.braking_end_s());
    assert!((tb.alt_m - 30.0).abs() < 1e-6 && (tb.vz_mps + 1.5).abs() < 1e-9);
    assert!(c.descent.truth(td).alt_m.abs() < 1e-9);
    // Truth is continuous and descending.
    let mut prev = f64::INFINITY;
    for t in faults::sample_times(&c) {
        let a = c.descent.truth(t).alt_m;
        assert!(a <= prev + 1e-9);
        prev = a;
    }
}

#[test]
fn the_library_lists_every_fault_type_and_the_four_incidents() {
    let md = faults::library_markdown();
    for t in FaultType::ALL {
        assert!(md.contains(&format!("`{}`", t.name())), "{}", t.name());
    }
    for id in ["ispace-m1", "im-1", "im-2", "ispace-m2"] {
        assert!(md.contains(id), "{id}");
    }
    let j = faults::library_json();
    assert_eq!(j["fault_types"].as_array().unwrap().len(), 8);
    assert_eq!(j["incidents"].as_array().unwrap().len(), 4);
    for i in faults::INCIDENTS {
        assert!(i.source.starts_with("https://"), "{}", i.id);
    }
}

#[test]
fn bad_campaigns_are_refused_with_the_reason() {
    let base = fixture_text();
    let cases: Vec<(String, &str)> = vec![
        (
            base.replace("arksa-landing-nav-campaign/1", "x/1"),
            "schema",
        ),
        (
            base.replace("test_setup = \"SIL\"", "test_setup = \"flight\""),
            "test_setup",
        ),
        (
            base.replace("terminal_vz_mps = -1.5", "terminal_vz_mps = -70.0"),
            "braking",
        ),
        (
            base.replace("terminal_alt_m = 30.0", "terminal_alt_m = 3000.0"),
            "terminal_alt_m",
        ),
        (
            base.replacen("rate_hz = 10.0", "rate_hz = 3.0", 1),
            "whole number",
        ),
        (base.replacen("id = \"lrf\"", "id = \"alt\"", 1), "twice"),
        (
            base.replace(
                "sensor = \"lrf\"\ntype = \"never_enabled\"",
                "sensor = \"lrf\"\ntype = \"never_enabled\"\nt_start_s = 3.0",
            ),
            "whole descent",
        ),
        (
            base.replace(
                "sensor = \"lrf\"\ntype = \"bias\"",
                "sensor = \"radar\"\ntype = \"bias\"",
            ),
            "no sensor",
        ),
        (base.replace("value = 40.0", "value = 0.0"), "bias of 0"),
        (
            base.replace("probability = 0.05", "probability = 1.5"),
            "probability",
        ),
        (
            base.replace(
                "t_start_s = 50.0\nt_end_s = 60.0",
                "t_start_s = 50.0\nt_end_s = 40.0",
            ),
            "t_end_s",
        ),
        (
            base.replace(
                "consistency_min_fraction = 0.95",
                "consistency_min_fraction = 1.5",
            ),
            "consistency_min_fraction",
        ),
        (
            base.replace("type = \"stuck\"", "type = \"stuck\"\nvalue = 3.0"),
            "no `value`",
        ),
        (
            base.replace("id = \"S01-nominal\"", "id = \"S01 nominal\""),
            "scenario id",
        ),
        (
            base.replace("seed = 20261120", "seed = 1\nsurprise = 2"),
            "unknown field",
        ),
    ];
    for (text, want) in cases {
        assert_ne!(text, base, "the replacement for '{want}' did not apply");
        let e = Campaign::parse(&text).unwrap_err().to_string();
        assert!(e.contains(want), "want '{want}' in: {e}");
    }
}

// ------------------------------------------------------------------ stream generation

fn stream_rows(c: &Campaign, id: &str) -> Vec<Vec<String>> {
    let sc = c.scenarios.iter().find(|s| s.id == id).unwrap();
    faults::generate(c, sc)
        .stream_csv
        .lines()
        .skip(1)
        .map(|l| l.split(',').map(str::to_string).collect())
        .collect()
}

/// Column indexes in the stream: t=0, accel=1, alt_m=2, alt_valid=3, lrf_m=4, lrf_valid=5.
fn value(row: &[String], col: usize) -> Option<f64> {
    row[col].parse().ok()
}

#[test]
fn streams_are_deterministic_and_carry_the_injected_faults() {
    let c = fixture();
    let (p1, _) = arksa_kpro::landing::prepare(&c, fixture_text().as_bytes());
    let (p2, _) = arksa_kpro::landing::prepare(&c, fixture_text().as_bytes());
    assert_eq!(p1, p2);
    assert_eq!(p1.scenarios.len(), 10);
    assert!(p1.scenarios.iter().all(|s| s.samples == 841));

    // Never enabled: no laser rangefinder value at all.
    let rows = stream_rows(&c, "S03-lrf-never-enabled");
    assert!(rows.iter().all(|r| r[4].is_empty() && r[5] == "0"));
    assert!(rows.iter().any(|r| r[3] == "1"));

    // Dropout: no altimeter value from 50 to 60 s; values either side.
    let rows = stream_rows(&c, "S04-alt-dropout");
    let at = |t: f64| {
        rows.iter()
            .find(|r| (value(r, 0).unwrap() - t).abs() < 1e-6)
            .unwrap()
    };
    for t in [50.0, 55.0, 59.9] {
        assert_eq!(at(t)[3], "0", "{t}");
    }
    for t in [49.9, 60.0] {
        assert_eq!(at(t)[3], "1", "{t}");
    }

    // Bias: the mean error after 40 s is 40 m (noise averages out).
    let sc = c.scenarios.iter().find(|s| s.id == "S05-lrf-bias").unwrap();
    let errs: Vec<f64> = stream_rows(&c, "S05-lrf-bias")
        .iter()
        .filter(|r| value(r, 0).unwrap() >= 40.0 && r[5] == "1")
        .map(|r| value(r, 4).unwrap() - faults::hat(&c, sc, value(r, 0).unwrap()))
        .collect();
    let mean = errs.iter().sum::<f64>() / errs.len() as f64;
    assert!(errs.len() > 300 && (mean - 40.0).abs() < 0.5, "{mean}");

    // Stuck: one frozen value from 55 s, still flagged valid.
    let rows = stream_rows(&c, "S07-alt-stuck");
    let frozen: Vec<&String> = rows
        .iter()
        .filter(|r| value(r, 0).unwrap() >= 55.0)
        .map(|r| &r[2])
        .collect();
    assert!(frozen.windows(2).all(|w| w[0] == w[1]));
    assert!(rows
        .iter()
        .filter(|r| value(r, 0).unwrap() >= 55.0)
        .all(|r| r[3] == "1"));

    // Saturation: never above the 600 m ceiling.
    let rows = stream_rows(&c, "S08-alt-saturation");
    assert!(rows.iter().filter_map(|r| value(r, 2)).all(|m| m <= 600.0));
    assert!(rows.iter().filter_map(|r| value(r, 2)).any(|m| m == 600.0));

    // Terrain step: a healthy altimeter reads about 400 m more over the depression.
    let sc = c
        .scenarios
        .iter()
        .find(|s| s.id == "S02-terrain-step")
        .unwrap();
    let rows = stream_rows(&c, "S02-terrain-step");
    let r = rows
        .iter()
        .find(|r| (value(r, 0).unwrap() - 25.0).abs() < 1e-6)
        .unwrap();
    let alt = c.descent.truth(25.0).alt_m;
    assert!((faults::hat(&c, sc, 25.0) - alt - 400.0).abs() < 1e-9);
    assert!((value(r, 2).unwrap() - (alt + 400.0)).abs() < 40.0);
}

// ------------------------------------------------------------------ verdicts

#[test]
fn a_filter_that_handles_every_fault_passes_every_scenario() {
    let c = fixture();
    let v = run(&c, &Behaviour::default());
    for r in &v.body.scenarios {
        assert_eq!(
            r.verdict,
            Verdict::Pass,
            "{}: {:?} {:?}",
            r.id,
            failed(r),
            r.problems
        );
    }
    assert_eq!(v.body.verdict, Verdict::Pass);
    assert_eq!(v.body.evidence_class, "MODELLED");
    assert_eq!(v.body.summary.pass, 10);
    // The checks the scenarios exist for were run, not skipped.
    let s03 = scenario(&v, "S03-lrf-never-enabled");
    assert!(s03
        .checks
        .iter()
        .any(|k| k.check == "detection" && k.status == Status::Pass));
    let s04 = scenario(&v, "S04-alt-dropout");
    assert!(s04
        .checks
        .iter()
        .any(|k| k.check == "recovery" && k.status == Status::Pass));
    let s06 = scenario(&v, "S06-lrf-drift");
    let d = s06.checks.iter().find(|k| k.check == "detection").unwrap();
    assert!(d.note.contains("onset 49.00 s"), "{}", d.note);
    assert_eq!(scenario(&v, "S01-nominal").ignored_columns, vec!["mode"]);
}

#[test]
fn latching_a_truthful_altimeter_as_faulty_over_terrain_fails_the_terrain_scenario() {
    let c = fixture();
    let v = run(
        &c,
        &Behaviour {
            latch_on_terrain: true,
            ..Behaviour::default()
        },
    );
    let r = scenario(&v, "S02-terrain-step");
    assert_eq!(r.verdict, Verdict::Fail);
    assert!(
        has_fail(r, "false_rejection", Some("alt")),
        "{:?}",
        failed(r)
    );
    let k = r
        .checks
        .iter()
        .find(|k| k.check == "false_rejection" && k.sensor.as_deref() == Some("alt"))
        .unwrap();
    assert!(
        k.note.contains("declared faulty") && k.note.contains("terrain"),
        "{}",
        k.note
    );
    assert_eq!(v.body.verdict, Verdict::Fail);
    // Scenarios without terrain are unaffected.
    assert_eq!(scenario(&v, "S01-nominal").verdict, Verdict::Pass);
}

#[test]
fn slow_or_missing_detection_fails_the_fault_scenarios() {
    let c = fixture();
    let slow = run(
        &c,
        &Behaviour {
            detect_delay_s: 5.0,
            ..Behaviour::default()
        },
    );
    for id in [
        "S03-lrf-never-enabled",
        "S04-alt-dropout",
        "S05-lrf-bias",
        "S06-lrf-drift",
        "S07-alt-stuck",
        "S08-alt-saturation",
    ] {
        assert!(has_fail(scenario(&slow, id), "detection", None), "{id}");
    }
    assert_eq!(scenario(&slow, "S01-nominal").verdict, Verdict::Pass);
    assert_eq!(scenario(&slow, "S09-lrf-spikes").verdict, Verdict::Pass);

    let never = run(
        &c,
        &Behaviour {
            never_detect: true,
            ..Behaviour::default()
        },
    );
    let r = scenario(&never, "S05-lrf-bias");
    let d = r.checks.iter().find(|k| k.check == "detection").unwrap();
    assert_eq!(d.status, Status::Fail);
    assert!(d.note.contains("never flagged"), "{}", d.note);
    assert!(has_fail(
        scenario(&never, "S03-lrf-never-enabled"),
        "detection",
        Some("lrf")
    ));
}

#[test]
fn taking_a_faulty_sensor_back_or_never_recovering_a_good_one_fails() {
    let c = fixture();
    let v = run(
        &c,
        &Behaviour {
            reaccept_after_s: Some(3.0),
            ..Behaviour::default()
        },
    );
    let r = scenario(&v, "S05-lrf-bias");
    assert!(
        has_fail(r, "faulty_reaccepted", Some("lrf")),
        "{:?}",
        failed(r)
    );

    let v = run(
        &c,
        &Behaviour {
            recover_delay_s: 1000.0,
            ..Behaviour::default()
        },
    );
    let r = scenario(&v, "S04-alt-dropout");
    let k = r.checks.iter().find(|k| k.check == "recovery").unwrap();
    assert_eq!(k.status, Status::Fail);
    assert!(k.note.contains("never used again"), "{}", k.note);
    assert!(has_fail(r, "false_rejection", Some("alt")));
}

#[test]
fn an_overconfident_sigma_fails_consistency() {
    let c = fixture();
    let v = run(
        &c,
        &Behaviour {
            alt_sigma_m: 0.05,
            ..Behaviour::default()
        },
    );
    for r in &v.body.scenarios {
        assert!(has_fail(r, "consistency", None), "{}", r.id);
        assert!(!has_fail(r, "low_alt_error", None), "{}", r.id);
    }
}

#[test]
fn a_sensor_left_off_is_a_configuration_failure() {
    let c = fixture();
    let v = run(
        &c,
        &Behaviour {
            sensor_off: Some("lrf"),
            ..Behaviour::default()
        },
    );
    let r = scenario(&v, "S01-nominal");
    assert!(has_fail(r, "configuration", Some("lrf")), "{:?}", failed(r));
    assert!(!has_fail(r, "configuration", Some("alt")));
    // When the scenario injects "never enabled" on purpose, it is a detection question.
    let r = scenario(&v, "S03-lrf-never-enabled");
    assert!(!r
        .checks
        .iter()
        .any(|k| k.check == "configuration" && k.sensor.as_deref() == Some("lrf")));
    assert!(r
        .checks
        .iter()
        .any(|k| k.check == "detection" && k.status == Status::Pass));
}

#[test]
fn a_truncated_or_missing_log_is_incomplete_not_passed() {
    let c = fixture();
    let v = run(
        &c,
        &Behaviour {
            truncate_at_s: Some(50.0),
            ..Behaviour::default()
        },
    );
    let r = scenario(&v, "S01-nominal");
    assert_eq!(r.verdict, Verdict::Incomplete, "{:?}", failed(r));
    assert!(has_fail(r, "coverage", None));
    // Scenarios whose fault falls after the cut fail (the fault was never flagged): an
    // observed failure counts even in a short log.
    assert_eq!(scenario(&v, "S07-alt-stuck").verdict, Verdict::Fail);
    assert_eq!(scenario(&v, "S05-lrf-bias").verdict, Verdict::Incomplete);
    assert_eq!(v.body.verdict, Verdict::Fail);

    let mut logs = logs_for(&c, &Behaviour::default());
    logs[0] = None;
    let v = verify::verify(&c, "x", &logs, None);
    let r = scenario(&v, "S01-nominal");
    assert_eq!(r.verdict, Verdict::Incomplete);
    assert!(r.problems[0].contains("S01-nominal.csv"));
    assert_eq!(v.body.summary.incomplete, 1);
    assert_eq!(v.body.verdict, Verdict::Incomplete);
}

#[test]
fn malformed_logs_are_refused_with_the_line() {
    let c = fixture();
    let good = synth_log(&c, 0, &Behaviour::default());
    let cases = [
        (
            good.replacen(",ok,", ",maybe,", 1),
            "must be ok, fault or off",
        ),
        (good.replace("t_s,", "time,"), "no 't_s' column"),
        (good.replace(",lrf_used", ",lrf_in_use"), "no 'lrf_used'"),
        (
            good.replacen("\n0.100,", "\n0.000,", 1),
            "t_s must increase",
        ),
        (
            good.replacen(",1.000,", ",0,", 1),
            "alt_sigma_m must be positive",
        ),
        (good.replacen(",descent\n", ",descent,extra\n", 1), "fields"),
        (good.replacen("2000.", "NaN", 1), "not a finite number"),
        ("# nothing\n".to_string(), "empty"),
    ];
    for (text, want) in cases {
        assert_ne!(text, good, "{want}");
        let e = arksa_kpro::landing::log::parse(&text, &c)
            .unwrap_err()
            .to_string();
        assert!(e.contains(want), "want '{want}' in: {e}");
    }
    // Through the verification, a bad log makes its scenario incomplete with the reason.
    let mut logs = logs_for(&c, &Behaviour::default());
    logs[1] = Some(LogInput {
        file_name: "S02-terrain-step.csv".into(),
        bytes: cases_first_bad(&good),
    });
    let v = verify::verify(&c, "x", &logs, None);
    let r = scenario(&v, "S02-terrain-step");
    assert_eq!(r.verdict, Verdict::Incomplete);
    assert!(r.problems[0].contains("log line"), "{:?}", r.problems);
}

fn cases_first_bad(good: &str) -> Vec<u8> {
    good.replacen(",ok,", ",maybe,", 1).into_bytes()
}

// ------------------------------------------------------------------ pre-registration and files

#[test]
fn preregistration_fixes_the_campaign_and_the_verdict_is_signed() {
    let c = fixture();
    let bytes = fixture_text().into_bytes();
    let dir = tmp("prereg");
    let seed = [42u8; 32];
    let (p, sig) = arksa_kpro::landing::write_preregistration(
        &c,
        &bytes,
        &dir,
        Some((&seed, "Arksa test", "2026-11-20T00:00:00Z")),
    )
    .unwrap();
    assert!(sig.is_some());
    let pk = arksa_kpro::license::public_key_for(&seed);
    let ok = kshana_pro::signed_dossier::verify(&dir, &[pk]).unwrap();
    assert!(ok.passed(), "{:?}", ok.problems);
    assert_eq!(ok.files, 2 + 2 * 10);
    for s in &p.scenarios {
        let stream = std::fs::read(dir.join(&s.stream_file)).unwrap();
        assert_eq!(sha256_hex(&stream), s.stream_sha256);
    }
    let prereg = std::fs::read(dir.join("preregistration.json")).unwrap();
    let st = arksa_kpro::landing::check_preregistration(&c, &bytes, &prereg).unwrap();
    assert!(st.campaign_matches && st.streams_match);

    // Loosening a bar after registration is refused.
    let loose = fixture_text().replace("detect_latency_s = 2.0", "detect_latency_s = 9.0");
    let lc = Campaign::parse(&loose).unwrap();
    let e = arksa_kpro::landing::check_preregistration(&lc, loose.as_bytes(), &prereg)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not the one pre-registered"), "{e}");

    // The verdict directory: signed, re-checkable, tamper-evident.
    let logs = logs_for(&c, &Behaviour::default());
    let v = verify::verify(&c, &sha256_hex(&bytes), &logs, Some(st));
    let out = tmp("verdict");
    arksa_kpro::landing::write_verdict(
        &v,
        &bytes,
        &logs,
        Some(&prereg),
        &out,
        Some((&seed, "Arksa test", "2026-11-20T00:00:00Z")),
    )
    .unwrap()
    .expect("signed");
    let ok = kshana_pro::signed_dossier::verify(&out, &[pk]).unwrap();
    assert!(ok.passed(), "{:?}", ok.problems);
    assert_eq!(ok.files, 4 + 10);
    let md = std::fs::read_to_string(out.join("verdict.md")).unwrap();
    assert!(md.contains("Verdict: PASS") && md.contains(&v.verdict_sha256));
    std::fs::write(out.join("verdict.md"), md.replace("PASS", "PASS!")).unwrap();
    let bad = kshana_pro::signed_dossier::verify(&out, &[pk]).unwrap();
    assert!(!bad.passed());

    // The verdict hash covers the body.
    let j: serde_json::Value = serde_json::from_str(&v.to_json()).unwrap();
    let canon = serde_json::to_string(&v.body).unwrap();
    assert_eq!(sha256_hex(canon.as_bytes()), v.verdict_sha256);
    assert_eq!(j["body"]["preregistration"]["streams_match"], true);
}

#[test]
fn outputs_are_labelled_modelled_and_make_no_overclaims() {
    let c = fixture();
    let v = run(&c, &Behaviour::default());
    let texts = [
        v.to_json(),
        v.to_markdown(),
        faults::library_markdown(),
        arksa_kpro::landing::cli::USAGE.to_string(),
    ];
    assert!(texts[0].contains("\"evidence_class\": \"MODELLED\""));
    assert!(texts[1].contains("MODELLED") && texts[1].contains("Not flight qualification"));
    for t in &texts {
        let l = t.to_lowercase();
        for w in [
            "certified by",
            "flight-proven",
            "flight proven",
            "validated against",
            "nasa-approved",
            "kill",
            "obsolete",
            "dominate",
            "terrain-relative navigation algorithm",
        ] {
            assert!(!l.contains(w), "'{w}'");
        }
    }
    // Without a pre-registration the verdict says so.
    assert!(texts[1].contains("none given"));
}

// ------------------------------------------------------------------ command line

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_arksa-kpro"));
    c.env("KSHANA_PRO_HOME", tmp("home"))
        .env_remove("ARKSA_STUDY_SIGNING_KEY");
    c
}

#[test]
fn the_command_needs_the_entitlement_and_runs_end_to_end() {
    let out = tmp("bin-refused");
    let refused = bin()
        .args(["landing-nav", "scenarios"])
        .arg(fixture_path())
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains(arksa_kpro::license::tiers::NOT_IN_THIS_EDITION));
    assert!(!out.exists());

    let help = bin().args(["help"]).output().unwrap();
    assert!(String::from_utf8_lossy(&help.stdout).contains("arksa-kpro landing-nav"));

    let lib = bin()
        .args(["landing-nav", "library", "--json", "--dev"])
        .output()
        .unwrap();
    assert!(lib.status.success());
    let j: serde_json::Value = serde_json::from_slice(&lib.stdout).unwrap();
    assert_eq!(j["incidents"].as_array().unwrap().len(), 4);

    let pre = tmp("bin-prereg");
    let key = tmp("bin-key");
    std::fs::create_dir_all(&key).unwrap();
    let keyfile = key.join("k.hex");
    std::fs::write(&keyfile, hex::encode([9u8; 32])).unwrap();
    let ok = bin()
        .args(["landing-nav", "scenarios"])
        .arg(fixture_path())
        .arg("--out")
        .arg(&pre)
        .arg("--sign-key-file")
        .arg(&keyfile)
        .arg("--dev")
        .output()
        .unwrap();
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let so = String::from_utf8_lossy(&ok.stdout);
    assert!(
        so.contains("pre-registered 10 scenarios") && so.contains("signed:"),
        "{so}"
    );
    assert!(pre.join("streams/S02-terrain-step.csv").exists());

    // Logs from a filter that latches on terrain: exit 1, verdict FAIL, signed.
    let c = fixture();
    let logs = tmp("bin-logs");
    std::fs::create_dir_all(&logs).unwrap();
    let b = Behaviour {
        latch_on_terrain: true,
        ..Behaviour::default()
    };
    for (i, sc) in c.scenarios.iter().enumerate() {
        std::fs::write(logs.join(format!("{}.csv", sc.id)), synth_log(&c, i, &b)).unwrap();
    }
    let vout = tmp("bin-verdict");
    let r = bin()
        .args(["landing-nav", "verify"])
        .arg(fixture_path())
        .arg("--logs")
        .arg(&logs)
        .arg("--out")
        .arg(&vout)
        .arg("--prereg")
        .arg(pre.join("preregistration.json"))
        .arg("--sign-key-file")
        .arg(&keyfile)
        .arg("--dev")
        .output()
        .unwrap();
    assert_eq!(
        r.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&r.stderr)
    );
    let so = String::from_utf8_lossy(&r.stdout);
    assert!(
        so.contains("S02-terrain-step: FAIL - false_rejection(alt)"),
        "{so}"
    );
    assert!(
        so.contains("campaign: FAIL (9 pass, 1 fail, 0 incomplete of 10)"),
        "{so}"
    );
    let pk = arksa_kpro::license::public_key_for(&[9u8; 32]);
    assert!(kshana_pro::signed_dossier::verify(&vout, &[pk])
        .unwrap()
        .passed());
    let j: serde_json::Value =
        serde_json::from_slice(&std::fs::read(vout.join("verdict.json")).unwrap()).unwrap();
    assert_eq!(j["body"]["verdict"], "FAIL");
    assert_eq!(j["body"]["preregistration"]["campaign_matches"], true);

    // Good logs: exit 0.
    for (i, sc) in c.scenarios.iter().enumerate() {
        std::fs::write(
            logs.join(format!("{}.csv", sc.id)),
            synth_log(&c, i, &Behaviour::default()),
        )
        .unwrap();
    }
    let r = bin()
        .args(["landing-nav", "verify"])
        .arg(fixture_path())
        .arg("--logs")
        .arg(&logs)
        .arg("--out")
        .arg(tmp("bin-verdict-ok"))
        .arg("--dev")
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "{}{}",
        String::from_utf8_lossy(&r.stdout),
        String::from_utf8_lossy(&r.stderr)
    );
    assert!(String::from_utf8_lossy(&r.stdout).contains("no pre-registration given"));
}
