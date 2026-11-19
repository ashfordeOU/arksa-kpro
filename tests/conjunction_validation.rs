// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Conjunction-assessment validation (`pro-conjunction-validation`): CDM (KVN and XML) and
//! results-table ingest, the consistency checks, the scorecard on synthetic events with known
//! answers, the cross-provider comparison and its explanations, the covariance-realism tests
//! on synthetic OEMs with known covariance error, and the signed study directory.
//!
//! The public TraCSS verification dataset (US Office of Space Commerce, CC0-1.0) is not
//! committed: its results tables are 60 to 200 MB compressed and are released on request. The
//! opt-in test `tracss_answer_key_opt_in` reads a local, decompressed copy named by
//! `ARKSA_TRACSS_KEY` (and, optionally, a provider's results in the same format named by
//! `ARKSA_TRACSS_TEST`); without it the test returns at once.

#![allow(clippy::needless_range_loop)]

use std::path::{Path, PathBuf};

use arksa_kpro::conjunction::{
    answer_key, cdm, compare, ephemeris, fmt_utc, parse_utc, pc::Encounter, realism, rtn_matrix,
    scorecard, Event, M6, MU_EARTH, R_EARTH,
};
use arksa_kpro::packs::conjunction_validation_pack::{
    events_csv, report_md, run_study, write_study, ConjunctionValidationScenario,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conjunction")
}

// ── Synthetic events ────────────────────────────────────────────────────────────────────

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sc(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn diag(s: [f64; 3]) -> M6 {
    let mut c = [[0.0; 6]; 6];
    for i in 0..3 {
        c[i][i] = s[i] * s[i];
        c[i + 3][i + 3] = (s[i] * 1e-3).powi(2);
    }
    c
}

/// A conjunction at TCA with exact geometry: a circular primary at `alt_km`, the secondary
/// crossing at `cross_deg` (rotation of the velocity about the radial axis), separated by
/// `dr` radially and `dp` in the encounter plane, so the relative position is normal to the
/// relative velocity (a true closest approach).
#[allow(clippy::too_many_arguments)]
fn ev(
    o1: &str,
    o2: &str,
    tca: f64,
    alt_km: f64,
    inc_deg: f64,
    cross_deg: f64,
    dr: f64,
    dp: f64,
    s1: [f64; 3],
    s2: [f64; 3],
) -> Event {
    let a = R_EARTH + alt_km * 1e3;
    let v = (MU_EARTH / a).sqrt();
    let (i, c) = (inc_deg.to_radians(), cross_deg.to_radians());
    let r1 = [a, 0.0, 0.0];
    let v1 = [0.0, v * i.cos(), v * i.sin()];
    let v2 = [0.0, v * (i + c).cos(), v * (i + c).sin()];
    let dv = [0.0, v2[1] - v1[1], v2[2] - v1[2]];
    let n = {
        let m = (dv[1] * dv[1] + dv[2] * dv[2]).sqrt();
        [0.0, -dv[2] / m, dv[1] / m]
    };
    let rel = add([dr, 0.0, 0.0], sc(n, dp));
    let r2 = add(r1, rel);
    let m = rtn_matrix(r1, v1);
    let rel_rtn = [
        m[0].iter().zip(rel).map(|(x, y)| x * y).sum(),
        m[1].iter().zip(rel).map(|(x, y)| x * y).sum(),
        m[2].iter().zip(rel).map(|(x, y)| x * y).sum(),
    ];
    let mut e = Event {
        id: format!("{o1}-{o2}@{tca}"),
        obj1: o1.into(),
        obj2: o2.into(),
        tca_s: tca,
        miss_m: (dr * dr + dp * dp).sqrt(),
        vrel_m_s: Some((dv[1] * dv[1] + dv[2] * dv[2]).sqrt()),
        rel_rtn_m: Some(rel_rtn),
        state1: Some([r1[0], r1[1], r1[2], v1[0], v1[1], v1[2]]),
        state2: Some([r2[0], r2[1], r2[2], v2[0], v2[1], v2[2]]),
        frame: Some("EME2000".into()),
        cov1_rtn: Some(diag(s1)),
        cov2_rtn: Some(diag(s2)),
        hbr_m: Some(5.0),
        ..Default::default()
    };
    e.pc = Encounter::from_event(&e, 5.0).and_then(|x| x.pc());
    e
}

fn t0() -> f64 {
    parse_utc("2026-11-21T00:00:00Z").unwrap()
}

/// 30 reference events: every tenth very-low relative velocity, every tenth plus one low,
/// the rest non-low; the last one 5 m inside a 10 km sphere.
fn reference_set() -> Vec<Event> {
    (0..30)
        .map(|k| {
            let cross = match k % 10 {
                0 => 0.038, // ~5 m/s
                1 => 0.23,  // ~30 m/s
                _ => 20.0 + 5.0 * k as f64,
            };
            let dp = if k == 29 {
                9_995.0
            } else {
                500.0 + 20.0 * k as f64
            };
            let dr = if k == 29 { 0.0 } else { 100.0 + k as f64 };
            ev(
                &format!("{}", 1000 + k),
                &format!("{}", 2000 + k),
                t0() + k as f64 * 3600.0,
                700.0,
                53.0,
                cross,
                dr,
                dp,
                [30.0, 150.0, 20.0],
                [80.0, 400.0, 60.0],
            )
        })
        .collect()
}

fn swap(e: &Event) -> Event {
    let mut s = e.clone();
    std::mem::swap(&mut s.obj1, &mut s.obj2);
    std::mem::swap(&mut s.state1, &mut s.state2);
    std::mem::swap(&mut s.cov1_rtn, &mut s.cov2_rtn);
    s.rel_rtn_m = None;
    s
}

fn sphere_cfg() -> scorecard::Config {
    scorecard::Config {
        window: Some((t0() - 3600.0, t0() + 31.0 * 3600.0)),
        screening: scorecard::Screening::Sphere { radius_m: 10_000.0 },
        objectives: Default::default(),
    }
}

fn objective<'a>(doc: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    doc["objectives"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["objective"] == name)
        .unwrap_or_else(|| panic!("{name}"))
}

// ── Ingest ──────────────────────────────────────────────────────────────────────────────

#[test]
fn cdm_kvn_is_read_with_units_and_covariance() {
    let text = std::fs::read_to_string(fixtures().join("synthetic.cdm")).unwrap();
    let m = cdm::parse_all(&text).unwrap();
    assert_eq!(m.len(), 1);
    let census = cdm::census(&m[0]);
    assert!(
        census["rtn_covariance"] && census["velocity_covariance"] && census["hard_body_radius"]
    );
    let e = m[0].to_event().unwrap();
    assert_eq!(e.obj1, "99001"); // leading zero dropped
    assert_eq!(e.obj2, "101390"); // six-digit catalogue number kept
    assert_eq!(fmt_utc(e.tca_s), "2026-11-21T14:30:15.250000Z"); // day-of-year form
    assert_eq!(e.miss_m, 715.0);
    assert_eq!(e.vrel_m_s, Some(14_762.0));
    assert_eq!(e.pc, Some(4.835e-5));
    assert_eq!(e.hbr_m, Some(10.0));
    assert_eq!(e.rel_rtn_m, Some([27.4, -70.6, 711.0]));
    let s1 = e.state1.unwrap();
    assert!((s1[0] - 2_570_097.065).abs() < 1e-6 && (s1[5] + 3_526.774_282).abs() < 1e-6);
    let c1 = e.cov1_rtn.unwrap();
    assert_eq!(c1[1][1], 2.533e3);
    assert_eq!(c1[0][1], c1[1][0]);
    assert_eq!(c1[5][3], -3.004e-6);
    assert_eq!(e.ephemeris2.as_deref(), Some("NONE"));
    assert_eq!(
        fmt_utc(e.last_obs1_s.unwrap()),
        "2026-11-20T06:00:00.000000Z"
    );
    // The writer round-trips through the reader.
    let back = cdm::events(&cdm::write_kvn(&e, "ROUNDTRIP", "2026-11-20T09:00:00")).unwrap();
    assert_eq!(back.len(), 1);
    let b = &back[0];
    assert_eq!((b.obj1.as_str(), b.obj2.as_str()), ("99001", "101390"));
    assert!((b.tca_s - e.tca_s).abs() < 1e-6);
    assert_eq!(b.cov2_rtn, e.cov2_rtn);
    assert_eq!(b.hbr_m, e.hbr_m);
    // Missing required keywords are refused.
    assert!(cdm::events(&text.replace("MISS_DISTANCE", "MISS_DISTANCE_X")).is_err());
    assert!(cdm::events("TCA = 2026-01-01T00:00:00").is_err());
}

#[test]
fn cdm_xml_matches_kvn() {
    let x = std::fs::read_to_string(fixtures().join("synthetic_cdm.xml")).unwrap();
    let e = cdm::events(&x).unwrap().remove(0);
    let k = cdm::events(&std::fs::read_to_string(fixtures().join("synthetic.cdm")).unwrap())
        .unwrap()
        .remove(0);
    assert_eq!(e.obj1, k.obj1);
    assert_eq!(e.obj2, k.obj2);
    assert_eq!(e.tca_s, k.tca_s);
    assert_eq!(e.state2, k.state2);
    // XML object 1 carries the position covariance only; object 2 none.
    let c = e.cov1_rtn.unwrap();
    assert_eq!(c[2][2], 7.098e1);
    assert_eq!(c[3][3], 0.0);
    assert!(e.cov2_rtn.is_none());
}

#[test]
fn results_table_round_trips_and_reads_the_documented_columns() {
    let refs = reference_set();
    let mut text = answer_key::header() + "\n";
    for e in &refs {
        text.push_str(&answer_key::row(e, 1));
        text.push('\n');
    }
    assert!(answer_key::looks_like(&text));
    let back =
        answer_key::read(std::io::Cursor::new(&text), answer_key::CovUnit::Km2, None).unwrap();
    assert_eq!(back.len(), refs.len());
    for (a, b) in refs.iter().zip(&back) {
        assert_eq!(a.pair(), b.pair());
        assert!((a.tca_s - b.tca_s).abs() < 1e-5, "{} {}", a.tca_s, b.tca_s);
        assert!((a.miss_m - b.miss_m).abs() < 1e-6);
        let (ca, cb) = (a.cov2_rtn.unwrap(), b.cov2_rtn.unwrap());
        assert!((ca[1][1] - cb[1][1]).abs() < 1e-6 * ca[1][1]);
        assert!((a.state1.unwrap()[4] - b.state1.unwrap()[4]).abs() < 1e-6);
    }
    // A minimal table with only the required columns, the NULL convention and a jdate TCA.
    let min = "obj1,obj2,min_range,prob,jdate\n00005,62461,1.25,NULL,2460677.0\n";
    let e = answer_key::read(std::io::Cursor::new(min), answer_key::CovUnit::Km2, None).unwrap();
    assert_eq!(e[0].obj1, "5");
    assert_eq!(e[0].miss_m, 1250.0);
    assert!(e[0].pc.is_none());
    assert_eq!(fmt_utc(e[0].tca_s), "2025-01-01T12:00:00.000000Z");
    // The pair filter keeps a subset.
    let keep = |a: &str, _: &str| a == "1003";
    let sub = answer_key::read(
        std::io::Cursor::new(&text),
        answer_key::CovUnit::Km2,
        Some(&keep),
    )
    .unwrap();
    assert_eq!(sub.len(), 1);
    assert!(answer_key::read(
        std::io::Cursor::new("a,b\n1,2\n"),
        answer_key::CovUnit::Km2,
        None
    )
    .is_err());
}

// ── Checks ──────────────────────────────────────────────────────────────────────────────

fn status_of(e: &Event, cfg: &scorecard::Config, id: &str) -> scorecard::Status {
    scorecard::check_event(e, cfg)
        .into_iter()
        .find(|c| c.0 == id)
        .unwrap()
        .1
}

#[test]
fn consistency_checks_catch_each_planted_fault() {
    use scorecard::Status::*;
    let cfg = sphere_cfg();
    let good = reference_set().remove(4);
    for (id, _) in scorecard::CHECKS {
        let s = status_of(&good, &cfg, id);
        assert!(s != Flag, "{id} flags a clean event");
    }
    assert_eq!(status_of(&good, &cfg, "tca_is_closest_approach"), Pass);
    assert_eq!(
        status_of(&good, &cfg, "relative_position_matches_states"),
        Pass
    );

    let mut e = good.clone();
    e.tca_s = t0() + 40.0 * 3600.0;
    assert_eq!(status_of(&e, &cfg, "tca_in_window"), Flag);

    let mut e = good.clone();
    e.obj2 = e.obj1.clone();
    assert_eq!(status_of(&e, &cfg, "not_self_on_self"), Flag);

    let mut e = good.clone();
    e.miss_m += 3.0;
    assert_eq!(status_of(&e, &cfg, "miss_equals_relative_position"), Flag);

    // The secondary 2 s further along its relative path: not the closest approach.
    let mut e = good.clone();
    let (a, b) = (e.state1.unwrap(), e.state2.unwrap());
    let dv = [b[3] - a[3], b[4] - a[4], b[5] - a[5]];
    let mut s2 = b;
    for i in 0..3 {
        s2[i] += 2.0 * dv[i];
    }
    e.state2 = Some(s2);
    assert_eq!(status_of(&e, &cfg, "tca_is_closest_approach"), Flag);
    assert_eq!(
        status_of(&e, &cfg, "relative_position_matches_states"),
        Flag
    );

    let mut e = good.clone();
    let mut c = e.cov1_rtn.unwrap();
    c[0][1] = 10.0 * (c[0][0] * c[1][1]).sqrt();
    c[1][0] = c[0][1];
    e.cov1_rtn = Some(c);
    assert_eq!(status_of(&e, &cfg, "covariance1_psd"), Flag);

    let mut e = good.clone();
    e.pc = Some(1.5);
    assert_eq!(status_of(&e, &cfg, "pc_in_range"), Flag);

    let mut e = good.clone();
    e.miss_m = 12_000.0;
    e.rel_rtn_m = None;
    assert_eq!(status_of(&e, &cfg, "inside_screening_volume"), Flag);

    let mut e = good.clone();
    e.last_obs1_s = Some(t0() - 20.0 * 86_400.0);
    assert_eq!(status_of(&e, &cfg, "od_data_age"), Flag);

    // Earth-fixed states: the state-based checks are not evaluated (no false flags).
    let mut e = good.clone();
    e.frame = Some("ITRF".into());
    assert_eq!(status_of(&e, &cfg, "tca_is_closest_approach"), NotEvaluated);
    assert_eq!(
        status_of(&e, &cfg, "relative_speed_matches_states"),
        NotEvaluated
    );

    // SFSH box: a LEO2 primary has a 0.4 km radial half size.
    let sfsh = scorecard::Config {
        screening: scorecard::Screening::Sfsh {
            table: Default::default(),
        },
        ..sphere_cfg()
    };
    let near = ev(
        "1",
        "2",
        t0(),
        700.0,
        53.0,
        60.0,
        300.0,
        1000.0,
        [1.0; 3],
        [1.0; 3],
    );
    let far = ev(
        "1",
        "2",
        t0(),
        700.0,
        53.0,
        60.0,
        500.0,
        1000.0,
        [1.0; 3],
        [1.0; 3],
    );
    assert_eq!(status_of(&near, &sfsh, "inside_screening_volume"), Pass);
    assert_eq!(status_of(&far, &sfsh, "inside_screening_volume"), Flag);
}

// ── Scorecard ───────────────────────────────────────────────────────────────────────────

#[test]
fn a_perfect_provider_meets_every_objective() {
    let refs = reference_set();
    let (doc, rows) = scorecard::scorecard(&refs, &refs, &sphere_cfg());
    assert_eq!(doc["matched"], 30);
    assert_eq!(doc["objectives_flagged"], 0, "{doc:#}");
    for o in doc["objectives"].as_array().unwrap() {
        assert!(o["status"] == "PASS" || o["status"] == "INFO", "{o}");
    }
    assert_eq!(rows.len(), 30);
    let checks = doc["checks_on_test_events"]["checks"].as_array().unwrap();
    assert!(checks.iter().all(|c| c["flag"] == 0), "{checks:#?}");
}

#[test]
fn the_scorecard_finds_each_planted_difference() {
    let refs = reference_set();
    let mut test = Vec::new();
    for (k, e) in refs.iter().enumerate() {
        if k == 28 || k == 29 {
            continue; // 28 missed outright, 29 missed at the boundary
        }
        let mut t = e.clone();
        t.tca_s += match k % 10 {
            0 => 3.0,             // VLRV: within 10 s
            1 => 0.004,           // LRV: within 0.01 s
            _ if k == 5 => 0.004, // NLRV: beyond 0.001 s
            _ => 0.000_4,         // NLRV: within
        };
        if k == 3 {
            t.miss_m += 5.0;
            t.rel_rtn_m = t.rel_rtn_m.map(|r| sc(r, t.miss_m / e.miss_m));
        }
        if k == 4 {
            let mut c = t.cov1_rtn.unwrap();
            c[1][1] *= 4.0; // in-track sigma doubled: Bhattacharyya 0.5 ln 1.25 = 0.112
            t.cov1_rtn = Some(c);
        }
        test.push(t);
    }
    test.push(swap(&refs[2])); // the B–A report of the same event
    let mut out = ev(
        "7001",
        "7002",
        t0() + 32.0 * 3600.0,
        700.0,
        53.0,
        90.0,
        10.0,
        900.0,
        [30.0; 3],
        [30.0; 3],
    );
    out.id = "outside-window".into();
    test.push(out);
    test.push(ev(
        "7003",
        "7004",
        t0() + 1800.0,
        700.0,
        53.0,
        90.0,
        0.0,
        9_998.0,
        [30.0; 3],
        [30.0; 3],
    ));

    let (doc, rows) = scorecard::scorecard(&refs, &test, &sphere_cfg());
    assert_eq!(doc["reference_events"], 30);
    assert_eq!(doc["test_events"], 30);
    assert_eq!(doc["test_duplicates_collapsed"], 1);
    assert_eq!(doc["matched"], 28);
    let r = objective(&doc, "event_recall");
    assert_eq!(
        (r["missed"].as_u64(), r["missed_near_boundary"].as_u64()),
        (Some(2), Some(1))
    );
    assert_eq!(r["status"], "FLAG");
    let p = objective(&doc, "event_precision");
    assert_eq!(
        (p["extra"].as_u64(), p["extra_near_boundary"].as_u64()),
        (Some(2), Some(1))
    );
    assert_eq!(objective(&doc, "tca_vlrv")["status"], "PASS");
    assert_eq!(objective(&doc, "tca_vlrv")["within"], 3);
    assert_eq!(objective(&doc, "tca_lrv")["status"], "PASS");
    let n = objective(&doc, "tca_nlrv");
    assert_eq!(n["status"], "FLAG");
    assert_eq!(n["stats_s"]["n"], 22);
    assert_eq!(n["within"], 21);
    assert_eq!(objective(&doc, "miss_distance")["status"], "FLAG");
    assert_eq!(objective(&doc, "relative_position")["status"], "FLAG");
    let c = objective(&doc, "covariance_at_tca");
    assert_eq!(c["status"], "FLAG");
    assert_eq!(c["above"], 1);
    assert_eq!(doc["objectives_flagged"], 6);
    // The reasons name what happened.
    let reasons: Vec<&str> = rows.iter().filter_map(|r| r["reason"].as_str()).collect();
    assert!(reasons.contains(&"not reported"));
    assert!(
        reasons
            .iter()
            .filter(|r| **r == "near the screening boundary")
            .count()
            == 2
    );
    assert!(reasons.contains(&"TCA outside the screening window"));
    // The window check flags the outside-window extra.
    let checks = doc["checks_on_test_events"]["checks"].as_array().unwrap();
    let w = checks
        .iter()
        .find(|c| c["check"] == "tca_in_window")
        .unwrap();
    assert_eq!(w["flag"], 1);
    // Rows for the CSV: 28 matched, 2 missed, 2 extra.
    assert_eq!(rows.len(), 32);
}

#[test]
fn repeated_conjunctions_of_one_pair_match_one_to_one() {
    // One pair meeting every half orbit; the provider reports them shifted 1 ms.
    let refs: Vec<Event> = (0..6)
        .map(|k| {
            ev(
                "1",
                "2",
                t0() + k as f64 * 2950.0,
                700.0,
                53.0,
                120.0,
                50.0,
                400.0,
                [30.0; 3],
                [30.0; 3],
            )
        })
        .collect();
    let test: Vec<Event> = refs
        .iter()
        .map(|e| Event {
            tca_s: e.tca_s + 0.000_5,
            ..e.clone()
        })
        .collect();
    let m = scorecard::match_events(&refs, &test, 300.0);
    assert_eq!(m, (0..6).map(|i| (i, i)).collect::<Vec<_>>());
}

// ── Cross-provider comparison ───────────────────────────────────────────────────────────

#[test]
fn the_comparison_explains_each_divergence_by_its_input() {
    let base = |o1: &str, k: f64| {
        ev(
            o1,
            "5000",
            t0() + k * 7200.0,
            700.0,
            53.0,
            70.0,
            40.0,
            180.0,
            [30.0, 150.0, 20.0],
            [80.0, 400.0, 60.0],
        )
    };
    let a: Vec<Event> = (0..5)
        .map(|k| base(&format!("{}", 4000 + k), k as f64))
        .collect();
    let mut b = a.clone();
    // 0: B uses a 15 m hard-body radius.
    b[0].hbr_m = Some(15.0);
    // 1: B's covariances are three times larger in sigma.
    let b1 = &mut b[1];
    for c in [&mut b1.cov1_rtn, &mut b1.cov2_rtn] {
        let mut m = c.unwrap();
        m.iter_mut().flatten().for_each(|x| *x *= 9.0);
        *c = Some(m);
    }
    // 2: B places the secondary 300 m further away in the encounter plane.
    b[2] = ev(
        "4002",
        "5000",
        a[2].tca_s,
        700.0,
        53.0,
        70.0,
        40.0,
        480.0,
        [30.0, 150.0, 20.0],
        [80.0, 400.0, 60.0],
    );
    // 3: B's TCA 200 s later (inside 6 % of a ~5,900 s period); 4: 400 s later (outside).
    b[3].tca_s += 200.0;
    b[4].tca_s += 400.0;
    for e in b.iter_mut() {
        e.pc = Encounter::from_event(e, e.hbr_m.unwrap()).and_then(|x| x.pc());
    }
    b.push(base("4999", 9.0)); // only in B
    let (pa, pb) = (a[0].pc.unwrap(), b[0].pc.unwrap());
    let thr = (pa * pb).sqrt();
    let cfg = compare::CompareConfig {
        policy_a: compare::Policy {
            name: "t".into(),
            pc_ge: Some(thr),
            miss_le_m: None,
            all: false,
        },
        policy_b: compare::Policy {
            name: "t".into(),
            pc_ge: Some(thr),
            miss_le_m: None,
            all: false,
        },
        ..Default::default()
    };
    let (doc, rows) = compare::compare(&a, &b, &cfg);
    assert_eq!(doc["matched"], 4);
    assert_eq!(doc["only_a"], 1);
    assert_eq!(doc["only_b"], 2);
    let ev_ = doc["matched_events"].as_array().unwrap();
    let dom = |i: usize| {
        ev_[i]["explanation"]["swaps"]["dominant_input"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(dom(0), "hard_body_radius");
    assert_eq!(dom(1), "covariances");
    assert_eq!(dom(2), "states");
    assert!(dom(3).starts_with("none"));
    // A 3x radius on a small disc: about (15/5)² in Pc.
    let d = ev_[0]["explanation"]["swaps"]["d_log10_pc_total"]
        .as_f64()
        .unwrap();
    assert!((d - 9f64.log10()).abs() < 0.01, "{d}");
    assert!(ev_[0]["explanation"]["provenance"]
        .to_string()
        .contains("hard-body radius differs"));
    assert!(ev_[3]["explanation"]["provenance"]
        .to_string()
        .contains("TCAs differ by +200"));
    // Classification conflict on event 0 (the threshold sits between its two Pc), and on any
    // other event whose two Pc straddle it.
    assert_eq!(ev_[0]["conflict"], true);
    let straddle = (0..4)
        .filter(|&i| (a[i].pc.unwrap() >= thr) != (b[i].pc.unwrap() >= thr))
        .count();
    assert_eq!(doc["classification_conflicts"], straddle);
    assert_eq!(rows.len(), 4 + 1 + 2);
}

// ── Covariance realism ──────────────────────────────────────────────────────────────────

fn normal(rng: &mut ChaCha8Rng) -> f64 {
    let (u1, u2): (f64, f64) = (rng.gen::<f64>().max(1e-300), rng.gen());
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// A reference and a predicted OEM for one object: circular orbit, a covariance every 10 min
/// for 72 h, prediction errors drawn from the true covariance (sigmas growing with age), and
/// the predicted covariance reported as `reported_scale²` times the true one, in RTN or in
/// EME2000.
fn oem_pair(
    id: &str,
    alt_km: f64,
    reported_scale: f64,
    inertial_cov: bool,
    seed: u64,
) -> (String, String) {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let a = R_EARTH + alt_km * 1e3;
    let n = (MU_EARTH / a.powi(3)).sqrt();
    let inc = 0.9_f64;
    let state = |t: f64| {
        let (c, s) = ((n * t).cos(), (n * t).sin());
        let v = a * n;
        [
            a * c,
            a * s * inc.cos(),
            a * s * inc.sin(),
            -v * s,
            v * c * inc.cos(),
            v * c * inc.sin(),
        ]
    };
    let head = |f: &str| {
        format!(
        "CCSDS_OEM_VERS = 2.0\nCREATION_DATE = 2026-11-20T00:00:00\nORIGINATOR = SYNTHETIC-TEST\n\nMETA_START\nOBJECT_NAME = {id}\nOBJECT_ID = {id}\nCENTER_NAME = EARTH\nREF_FRAME = EME2000\nTIME_SYSTEM = UTC\nSTART_TIME = 2026-11-20T00:00:00.000\nSTOP_TIME = 2026-11-23T00:00:00.000\nMETA_STOP\n{f}"
    )
    };
    let t_start = parse_utc("2026-11-20T00:00:00Z").unwrap();
    let (mut rf, mut pr, mut cov) = (
        String::new(),
        String::new(),
        String::from("COVARIANCE_START\n"),
    );
    for k in 0..432 {
        let t = k as f64 * 600.0;
        let ep = fmt_utc(t_start + t).trim_end_matches('Z').to_string();
        let x = state(t);
        let growth = 1.0 + t / 86_400.0;
        let sig = [20.0 * growth, 120.0 * growth, 10.0 * growth];
        let e_rtn = [
            sig[0] * normal(&mut rng),
            sig[1] * normal(&mut rng),
            sig[2] * normal(&mut rng),
        ];
        let m = rtn_matrix([x[0], x[1], x[2]], [x[3], x[4], x[5]]);
        // inertial error = Mᵀ e_rtn; predicted = reference − error
        let mut p = x;
        for i in 0..3 {
            p[i] -= (0..3).map(|j| m[j][i] * e_rtn[j]).sum::<f64>();
        }
        let line = |s: [f64; 6]| {
            format!(
                "{ep} {:.9} {:.9} {:.9} {:.12} {:.12} {:.12}\n",
                s[0] / 1e3,
                s[1] / 1e3,
                s[2] / 1e3,
                s[3] / 1e3,
                s[4] / 1e3,
                s[5] / 1e3
            )
        };
        rf.push_str(&line(x));
        pr.push_str(&line(p));
        let mut c = [[0.0; 6]; 6];
        for i in 0..3 {
            c[i][i] = (sig[i] * reported_scale).powi(2);
            c[i + 3][i + 3] = (sig[i] * reported_scale * 1e-3).powi(2);
        }
        let (frame, c) = if inertial_cov {
            let mut ci = [[0.0; 6]; 6];
            for i in 0..3 {
                for j in 0..3 {
                    ci[i][j] = (0..3).map(|k| m[k][i] * c[k][k] * m[k][j]).sum();
                    ci[i + 3][j + 3] = (0..3).map(|k| m[k][i] * c[k + 3][k + 3] * m[k][j]).sum();
                }
            }
            ("EME2000", ci)
        } else {
            ("RTN", c)
        };
        cov.push_str(&format!("EPOCH = {ep}\nCOV_REF_FRAME = {frame}\n"));
        for i in 0..6 {
            let row: Vec<String> = (0..=i).map(|j| format!("{:.15e}", c[i][j] / 1e6)).collect();
            cov.push_str(&row.join(" "));
            cov.push('\n');
        }
    }
    cov.push_str("COVARIANCE_STOP\n");
    (head(&rf), head(&format!("{pr}\n{cov}")))
}

#[test]
fn realism_recovers_the_planted_covariance_scale() {
    let mut samples = Vec::new();
    for (id, scale, inertial, seed) in [
        ("90001", 1.0, false, 11), // realistic, RTN covariance
        ("90002", 1.0, true, 12),  // realistic, EME2000 covariance (rotated to RTN)
        ("90003", 0.5, false, 13), // reported sigmas half the truth: optimistic, scale ~2
        ("90004", 2.0, true, 14),  // reported sigmas twice the truth: pessimistic, scale ~0.5
    ] {
        let (rf, pr) = oem_pair(id, 650.0, scale, inertial, seed);
        let p = ephemeris::parse_oem(&pr).unwrap().remove(0);
        let r = ephemeris::parse_oem(&rf).unwrap().remove(0);
        assert_eq!(p.covariances.len(), 432);
        assert_eq!(p.covariances[0].1, if inertial { "EME2000" } else { "RTN" });
        let (s, w) = realism::samples_from_oem(&p, &r, Some(id), None).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.len(), 432);
        samples.extend(s);
    }
    let doc = realism::analyse(&samples, &realism::RealismConfig::default());
    let class = |c: &str| {
        doc["by_class"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["class"] == c)
            .unwrap()["all_ages"]
            .clone()
    };
    for id in ["90001", "90002"] {
        let s = class(id);
        let k = s["sigma_scale_factor"].as_f64().unwrap();
        assert!((0.9..1.1).contains(&k), "{id}: {k}");
        assert_eq!(s["verdict"], "CONSISTENT", "{id}: {s}");
        let c = s["containment_95"].as_f64().unwrap();
        assert!((0.92..0.98).contains(&c), "{id}: {c}");
    }
    let o = class("90003");
    assert!(
        (1.8..2.2).contains(&o["sigma_scale_factor"].as_f64().unwrap()),
        "{o}"
    );
    assert_eq!(o["verdict"], "OPTIMISTIC");
    let p = class("90004");
    assert!(
        (0.45..0.55).contains(&p["sigma_scale_factor"].as_f64().unwrap()),
        "{p}"
    );
    assert_eq!(p["verdict"], "PESSIMISTIC");
    // Per-age bins cover 0–72 h.
    let bins = doc["by_class"][0]["by_age"].as_array().unwrap();
    assert_eq!(bins.len(), 5);
    assert!(doc["independence_note"]
        .as_str()
        .unwrap()
        .contains("correlated"));
}

#[test]
fn realism_reads_a_residual_table_and_flags_an_in_track_problem() {
    let mut rng = ChaCha8Rng::seed_from_u64(5);
    let mut t = String::from("object,class,age_h,dr_m,dt_m,dn_m,crr,ctr,ctt,cnr,cnt,cnn\n");
    for k in 0..300 {
        // in-track error three times the stated sigma; radial and normal right
        let (r, it, n) = (
            10.0 * normal(&mut rng),
            3.0 * 50.0 * normal(&mut rng),
            5.0 * normal(&mut rng),
        );
        t.push_str(&format!(
            "A,LEO,{},{r},{it},{n},100,0,2500,0,0,25\n",
            k as f64 * 0.2
        ));
    }
    let s = realism::samples_from_csv(&t).unwrap();
    assert_eq!(s.len(), 300);
    let doc = realism::analyse(&s, &realism::RealismConfig::default());
    let v = &doc["overall"]["normalised_variance_rtn"];
    assert!((0.8..1.25).contains(&v[0].as_f64().unwrap()), "{v}");
    assert!((7.5..10.5).contains(&v[1].as_f64().unwrap()), "{v}");
    assert_eq!(doc["overall"]["verdict"], "OPTIMISTIC");
}

#[test]
fn omm_gives_the_object_class() {
    let omm = "CCSDS_OMM_VERS = 2.0\nCREATION_DATE = 2026-11-20T00:00:00\nORIGINATOR = SYNTHETIC-TEST\nOBJECT_NAME = SYN\nOBJECT_ID = 2026-999A\nCENTER_NAME = EARTH\nREF_FRAME = TEME\nTIME_SYSTEM = UTC\nMEAN_ELEMENT_THEORY = SGP4\nEPOCH = 2026-11-20T00:00:00\nMEAN_MOTION = 15.0\nECCENTRICITY = 0.001\nINCLINATION = 53.0\nRA_OF_ASC_NODE = 10.0\nARG_OF_PERICENTER = 20.0\nMEAN_ANOMALY = 30.0\nEPHEMERIS_TYPE = 0\nCLASSIFICATION_TYPE = U\nNORAD_CAT_ID = 101395\nELEMENT_SET_NO = 999\nREV_AT_EPOCH = 1\nBSTAR = 0.0001\nMEAN_MOTION_DOT = 0.0\nMEAN_MOTION_DDOT = 0.0\n";
    let (id, st) = ephemeris::omm_class_state(omm).unwrap();
    assert_eq!(id, "101395");
    let (r, v) = ([st[0], st[1], st[2]], [st[3], st[4], st[5]]);
    assert_eq!(arksa_kpro::conjunction::object_class(r, v), "LEO");
    // 15 rev/day: perigee ~550 km, the SFSH LEO2 regime.
    assert_eq!(arksa_kpro::conjunction::sfsh_regime(r, v).0, 4);
}

// ── The pack and the signed directory ───────────────────────────────────────────────────

fn scenario_text() -> String {
    let refs = reference_set();
    let mut table = answer_key::header() + "\n";
    for e in &refs {
        table.push_str(&answer_key::row(e, 1));
        table.push('\n');
    }
    let cdms: Vec<String> = refs
        .iter()
        .take(10)
        .map(|e| cdm::write_kvn(e, "PROVIDER-SYNTHETIC", "2026-11-20T00:00:00"))
        .collect();
    let mut resid = String::from("object,class,age_h,dr_m,dt_m,dn_m,crr,ctr,ctt,cnr,cnt,cnn\n");
    let mut rng = ChaCha8Rng::seed_from_u64(9);
    for k in 0..50 {
        resid.push_str(&format!(
            "A,LEO,{},{},{},{},100,0,400,0,0,25\n",
            k as f64,
            10.0 * normal(&mut rng),
            20.0 * normal(&mut rng),
            5.0 * normal(&mut rng)
        ));
    }
    format!(
        r#"kind = "pro-conjunction-validation"
title = "Synthetic provider against a synthetic reference"

[[source]]
name = "reference"
text = ['''{table}''']

[[source]]
name = "provider"
format = "cdm"
text = [{cdms}]

[[check]]
source = "provider"
screening = "sphere"
sphere_radius_km = 10.0

[scorecard]
reference = "reference"
test = "provider"
window_utc = ["2026-11-20T23:00:00Z", "2026-11-22T07:00:00Z"]
screening = "sphere"
sphere_radius_km = 10.0

[[compare]]
a = "reference"
b = "provider"
policy = {{ name = "example", pc_ge = 1e-4 }}

[[realism]]
label = "synthetic residuals"
residuals_text = '''{resid}'''
"#,
        cdms = cdms
            .iter()
            .map(|c| format!("'''{c}'''"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[test]
fn the_pack_runs_every_section_and_labels_honestly() {
    let s: ConjunctionValidationScenario = toml::from_str(&scenario_text()).unwrap();
    let st = run_study(&s).unwrap();
    let d = &st.doc;
    assert_eq!(d["evidence_class"], "MODELLED");
    assert!(d["label"].as_str().unwrap().starts_with("MODELLED"));
    assert!(d["label"]
        .as_str()
        .unwrap()
        .contains("not endorsed by any agency"));
    assert_eq!(d["sources"][0]["events"], 30);
    assert!(d["sources"][0]["results_table_assumptions"].is_string());
    assert_eq!(d["sources"][1]["events"], 10);
    assert_eq!(d["scorecard"]["matched"], 10);
    // Only the first ten reference events were sent; all other objectives agree.
    assert_eq!(d["scorecard"]["objectives"][0]["missed"], 20);
    assert_eq!(d["compare"][0]["matched"], 10);
    assert_eq!(d["compare"][0]["classification_conflicts"], 0);
    assert_eq!(d["realism"]["samples"], 50);
    assert_eq!(d["checks"][0]["result"]["events"], 10);
    assert!(d["result_sha256"].as_str().unwrap().len() == 64);
    let md = report_md(d);
    for h in [
        "## Scorecard",
        "## Cross-provider comparison",
        "## Covariance realism",
        "## Consistency checks",
        "What this does and does not say",
    ] {
        assert!(md.contains(h), "{h}");
    }
    for w in [
        "certified",
        "endorsed by NASA",
        "endorsed by ESA",
        "validated provider",
    ] {
        assert!(
            !md.to_lowercase().contains(&w.to_lowercase()) || md.contains("not"),
            "{w}"
        );
    }
    let csv = events_csv(&st.rows);
    assert!(csv.starts_with("# evidence: MODELLED"));
    assert_eq!(csv.lines().count(), 2 + st.rows.len());
    // Deterministic: the same inputs give the same result hash.
    assert_eq!(
        run_study(&s).unwrap().doc["result_sha256"],
        d["result_sha256"]
    );
    // Unknown keys and empty scenarios are refused.
    assert!(toml::from_str::<ConjunctionValidationScenario>("kind = \"pro-conjunction-validation\"\n[scorecard]\nreference = \"a\"\ntest = \"b\"\nscreenng = \"sphere\"\n").is_err());
    assert!(run_study(&ConjunctionValidationScenario::default()).is_err());
}

#[test]
fn the_study_directory_is_signed_and_tamper_evident() {
    let s: ConjunctionValidationScenario = toml::from_str(&scenario_text()).unwrap();
    let st = run_study(&s).unwrap();
    let dir = std::env::temp_dir().join(format!("arksa-cv-signed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seed = [7u8; 32];
    let sig = write_study(
        &st,
        &dir,
        Some((&seed, "Arksa test", "2026-11-20T00:00:00Z")),
    )
    .unwrap()
    .unwrap();
    assert!(sig.ends_with("signature.json"));
    let pk = kshana_pro::license::public_key_for(&seed);
    let v = kshana_pro::signed_dossier::verify(&dir, &[pk]).unwrap();
    assert!(v.passed(), "{:?}", v.problems);
    assert_eq!(v.files, 3);
    std::fs::write(dir.join("events.csv"), "edited").unwrap();
    assert!(!kshana_pro::signed_dossier::verify(&dir, &[pk])
        .unwrap()
        .passed());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_pack_reads_files_and_directories_relative_to_the_scenario() {
    let dir = std::env::temp_dir().join(format!("arksa-cv-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("cdms")).unwrap();
    std::fs::copy(fixtures().join("synthetic.cdm"), dir.join("cdms/a.cdm")).unwrap();
    std::fs::copy(fixtures().join("synthetic_cdm.xml"), dir.join("cdms/b.xml")).unwrap();
    let mut s: ConjunctionValidationScenario = toml::from_str(
        "kind = \"pro-conjunction-validation\"\n[[source]]\nname = \"p\"\nfiles = [\"cdms\"]\n[[check]]\nsource = \"p\"\n",
    )
    .unwrap();
    s.resolve_files(&dir).unwrap();
    let st = run_study(&s).unwrap();
    assert_eq!(st.doc["sources"][0]["events"], 2);
    assert_eq!(st.doc["sources"][0]["inputs"][0]["file"], "a.cdm");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── The public dataset (opt-in) ─────────────────────────────────────────────────────────

/// Reads a local copy of the TraCSS verification answer key (spherical 10 km results table,
/// decompressed) named by `ARKSA_TRACSS_KEY`, checks it against the user's guide (TCAs inside
/// the 2025-01-01T12:00Z to 2025-01-08T12:00Z window, miss distances up to 10 km) and scores
/// `ARKSA_TRACSS_TEST` (a provider's results in the same format) against it, or the key against
/// itself as a harness check. Prints the summary. Returns at once without the variable.
#[test]
fn tracss_answer_key_opt_in() {
    let Ok(key) = std::env::var("ARKSA_TRACSS_KEY") else {
        eprintln!("ARKSA_TRACSS_KEY not set: skipping the TraCSS answer-key run");
        return;
    };
    let open = |p: &str| {
        std::io::BufReader::new(std::fs::File::open(p).unwrap_or_else(|e| panic!("{p}: {e}")))
    };
    let refs = answer_key::read(open(&key), answer_key::CovUnit::Km2, None).unwrap();
    assert!(!refs.is_empty());
    let (w0, w1) = (
        parse_utc("2025-01-01T12:00:00Z").unwrap(),
        parse_utc("2025-01-08T12:00:00Z").unwrap(),
    );
    let in_window = refs
        .iter()
        .filter(|e| e.tca_s >= w0 && e.tca_s <= w1)
        .count();
    let within_10km = refs.iter().filter(|e| e.miss_m <= 10_000.0 + 1e-3).count();
    eprintln!(
        "answer key: {} rows, {in_window} with TCA in the window, {within_10km} within 10 km",
        refs.len()
    );
    assert_eq!(
        in_window,
        refs.len(),
        "the guide says every TCA is inside the window"
    );
    let test = match std::env::var("ARKSA_TRACSS_TEST") {
        Ok(p) => answer_key::read(open(&p), answer_key::CovUnit::Km2, None).unwrap(),
        Err(_) => refs.clone(),
    };
    let cfg = scorecard::Config {
        window: Some((w0, w1)),
        screening: scorecard::Screening::Sphere { radius_m: 10_000.0 },
        objectives: Default::default(),
    };
    let (doc, _) = scorecard::scorecard(&refs, &test, &cfg);
    let mut summary = doc.clone();
    summary
        .as_object_mut()
        .unwrap()
        .remove("checks_on_test_events");
    eprintln!("{}", serde_json::to_string_pretty(&summary).unwrap());
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&doc["checks_on_test_events"]).unwrap()
    );
    if std::env::var("ARKSA_TRACSS_TEST").is_err() {
        assert_eq!(doc["matched"], doc["reference_events"]);
    }
}
