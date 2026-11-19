// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Operations assurance: test reports from run logs, anomaly-detector scorecards and
//! tracking-gap impact.
//!
//! The inputs in `scenarios/ops/` are synthetic and illustrative. The two `#[ignore]`d
//! tests read the public benchmarks fetched by `scripts/fetch_ad_benchmarks.py`.

use std::path::{Path, PathBuf};
use std::process::Command;

use arksa_kpro::mbse::evidence::sha256_hex;
use arksa_kpro::ops::ad_scorecard::{self, RareEvents};
use arksa_kpro::ops::test_report::{self, CheckStatus, Confirmation, Procedure, TcStatus};
use arksa_kpro::ops::tracking_gap;
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios/ops")
}

fn tmp(tag: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("ops-{tag}"));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_arksa-kpro"))
}

fn procedure_text() -> String {
    read(&fixtures().join("test-report/procedure.toml"))
}

fn report_of(proc_text: &str, log_text: &str) -> Result<test_report::TestReport, String> {
    let p = Procedure::parse(proc_text).map_err(|e| e.to_string())?;
    let l = test_report::parse_log(log_text).map_err(|e| e.to_string())?;
    test_report::build(
        &p,
        &l,
        &sha256_hex(proc_text.as_bytes()),
        &sha256_hex(log_text.as_bytes()),
    )
    .map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ test reports

#[test]
fn a_nominal_run_is_confirmed_step_by_step_with_expected_beside_observed() {
    let r = report_of(
        &procedure_text(),
        &read(&fixtures().join("test-report/run-nominal.csv")),
    )
    .unwrap();
    let b = &r.body;
    assert_eq!(b.status, Confirmation::Confirmed);
    assert!(b.preconditions_met);
    assert_eq!(b.run.evidence_class, "MODELLED");
    assert_eq!(b.run.operator.as_deref(), Some("test conductor A"));
    assert!(b.steps.iter().all(|s| s.status == Confirmation::Confirmed));
    assert!(b.deviations.is_empty(), "{:?}", b.deviations);
    // The current reached its range 12 s into step S1 (the 0.31 A sample before did not).
    let i = &b.steps[0].checks[1];
    assert_eq!(
        (i.parameter.as_str(), i.status),
        ("HTR_A_I", CheckStatus::Met)
    );
    assert_eq!(
        (i.after_s, i.observed.as_deref()),
        (Some(12.0), Some("0.50"))
    );
    // Held throughout S2: the range of the samples is reported.
    assert_eq!(
        b.steps[1].checks[0].observed.as_deref(),
        Some("0.49 to 0.51 over 3 samples")
    );
    let tc = &b.steps[0].telecommands[0];
    assert_eq!(tc.status, TcStatus::Verified);
    assert_eq!(
        tc.stages[1].utc.as_deref(),
        Some("2026-11-18T09:01:02.500Z")
    );
    assert_eq!(b.summary.checks, 9);
    assert_eq!(b.summary.requirements_verified, 3);
    assert!(b
        .requirements
        .iter()
        .any(|q| q.requirement == "REQ-THM-011" && q.verified_by == ["S1", "S3"]));
    let md = r.to_markdown();
    for s in [
        "## 3. Preconditions body",
        "## 4. Main body: steps",
        "## 5. Confirmation body",
        "## 7. Requirement verification matrix",
        "**Result: confirmed**",
        "panel reached setpoint",
        &r.report_sha256,
    ] {
        assert!(md.contains(s), "{s}");
    }
    // The hash covers the body and is reproducible.
    let again = report_of(
        &procedure_text(),
        &read(&fixtures().join("test-report/run-nominal.csv")),
    )
    .unwrap();
    assert_eq!(again.report_sha256, r.report_sha256);
}

#[test]
fn deviations_are_listed_and_the_run_is_not_confirmed() {
    let r = report_of(
        &procedure_text(),
        &read(&fixtures().join("test-report/run-deviations.csv")),
    )
    .unwrap();
    let b = &r.body;
    assert_eq!(b.status, Confirmation::NotConfirmed);
    let st: Vec<Confirmation> = b.steps.iter().map(|s| s.status).collect();
    assert_eq!(
        st,
        [
            Confirmation::Confirmed,
            Confirmation::NotConfirmed,
            Confirmation::NotConfirmed
        ]
    );
    let kinds: Vec<&str> = b.deviations.iter().map(|d| d.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "telemetry out of expectation",
            "telecommand not verified",
            "unplanned telecommand",
            "confirmation"
        ]
    );
    assert_eq!(b.deviations[0].id, "D-1");
    assert!(b.deviations[0].text.contains("observed 0.58"));
    assert_eq!(b.deviations[2].step.as_deref(), Some("S2"));
    let off = &b.steps[2].telecommands[0];
    assert_eq!(off.status, TcStatus::NotVerified);
    assert_eq!(off.stages[1].status, "not reported");
    assert_eq!(b.confirmation[0].status, CheckStatus::NoTelemetry);
    assert_eq!(b.summary.requirements_verified, 0);
    assert!(b.requirements.iter().all(|q| q.status == "not verified"));
}

#[test]
fn aborts_order_unknown_steps_failed_acks_and_hardware_sources() {
    let log = "time_utc,event,step,name,value,detail
2026-11-18T09:00:00Z,run_start,,RUN-9,engineering-model,
2026-11-18T09:00:05Z,tm,,BUS_V,26.0,
2026-11-18T09:00:05Z,tm,,HTR_A_STATUS,OFF,
2026-11-18T09:01:00Z,step_start,S2,,,
2026-11-18T09:01:30Z,step_end,S2,,,
2026-11-18T09:02:00Z,step_start,S1,,,
2026-11-18T09:02:01Z,tc,,HTR_A_ON,,
2026-11-18T09:02:02Z,tc_ack,,HTR_A_ON,acceptance,success
2026-11-18T09:02:03Z,tc_ack,,HTR_A_ON,completion,failure
2026-11-18T09:02:04Z,step_start,S9,,,
2026-11-18T09:02:10Z,abort,,,,heater current out of limits
2026-11-18T09:02:20Z,run_end,,,,
";
    let r = report_of(&procedure_text(), log).unwrap();
    let b = &r.body;
    assert_eq!(b.run.evidence_class, "MEASURED");
    assert_eq!(b.status, Confirmation::Aborted);
    assert!(!b.preconditions_met);
    assert_eq!(b.steps[0].status, Confirmation::Aborted);
    assert_eq!(b.steps[0].telecommands[0].status, TcStatus::Failed);
    assert_eq!(b.steps[1].status, Confirmation::NotConfirmed);
    assert_eq!(b.steps[2].status, Confirmation::NotExecuted);
    // After an abort the confirmation body is not evaluated.
    assert!(b.confirmation.is_empty());
    let kinds: Vec<&str> = b.deviations.iter().map(|d| d.kind.as_str()).collect();
    for k in [
        "unknown step",
        "order",
        "abort",
        "telecommand verification failure",
        "precondition",
    ] {
        assert!(kinds.contains(&k), "{k}: {kinds:?}");
    }
    // A step that never ran after an abort is not a separate deviation.
    assert!(!kinds.contains(&"step not executed"));
    assert!(r
        .to_markdown()
        .contains("the log records a test on hardware or in orbit"));
}

#[test]
fn bad_procedures_and_logs_are_refused_with_the_reason() {
    let p = procedure_text();
    let bad_procs = [
        (
            p.replace(
                "min = 0.45\nmax = 0.55\nwithin_s = 30",
                "min = 0.55\nmax = 0.45\nwithin_s = 30",
            ),
            "min > max",
        ),
        (
            p.replace("equals_text = \"ON\"", "equals_text = \"ON\"\nmin = 1.0"),
            "exactly one of",
        ),
        (
            p.replace(
                "verify = [\"acceptance\", \"completion\"]",
                "verify = [\"executed\"]",
            ),
            "verification stage 'executed'",
        ),
        (p.replace("id = \"S3\"", "id = \"S1\""), "empty or repeated"),
        (
            p.replace("within_s = 600", "within_s = 0"),
            "within_s must be > 0",
        ),
        (
            p.replace("version = \"1.0\"", "version = \"1.0\"\ncolour = \"red\""),
            "unknown field",
        ),
    ];
    let log = read(&fixtures().join("test-report/run-nominal.csv"));
    for (text, why) in &bad_procs {
        let e = report_of(text, &log).unwrap_err();
        assert!(e.contains(why), "{why}: {e}");
    }
    let bad_logs = [
        (
            log.replace("tm,,BUS_V,28.1", "telemetry,,BUS_V,28.1"),
            "event 'telemetry'",
        ),
        (
            log.replace(
                "completion,success\n2026-11-18T09:01:04Z",
                "completion,ok\n2026-11-18T09:01:04Z",
            ),
            "success or failure",
        ),
        (
            log.replace(
                "acceptance,success\n2026-11-18T09:01:02.500Z",
                "accepted,success\n2026-11-18T09:01:02.500Z",
            ),
            "stage 'accepted'",
        ),
        (
            log.replace(
                "2026-11-18T09:01:00Z,step_start",
                "2026-11-18 09:01:00,step_start",
            ),
            "not a UTC time",
        ),
        (
            log.replace("run_start,,RUN-0001,synthetic", "run_start,,RUN-0001,bench"),
            "log source",
        ),
        (
            log.replace("2026-11-18T09:00:00Z,run_start,,RUN-0001,synthetic,\n", ""),
            "no run_start",
        ),
        (
            log.replace("step_start,S1,,,", "step_start,,,,"),
            "needs a step",
        ),
    ];
    for (text, why) in &bad_logs {
        let e = report_of(&p, text).unwrap_err();
        assert!(e.contains(why), "{why}: {e}");
    }
}

#[test]
fn a_signed_report_verifies_and_any_change_is_detected() {
    let dir = tmp("signed-report");
    let (r, pb, lb) = test_report::from_files(
        &fixtures().join("test-report/procedure.toml"),
        &fixtures().join("test-report/run-nominal.csv"),
    )
    .unwrap();
    let seed = [9u8; 32];
    let sig = arksa_kpro::ops::write_dir(
        &dir,
        &[
            ("report.json", (r.to_json() + "\n").into_bytes()),
            ("report.md", r.to_markdown().into_bytes()),
            ("procedure.toml", pb),
            ("run-log.csv", lb),
        ],
        Some((&seed, "Arksa test", "2026-11-19T00:00:00Z")),
    )
    .unwrap()
    .expect("signed");
    assert!(sig.ends_with("signature.json"));
    let pk = arksa_kpro::license::public_key_for(&seed);
    let ok = kshana_pro::signed_dossier::verify(&dir, &[pk]).unwrap();
    assert!(ok.passed(), "{:?}", ok.problems);
    assert_eq!(ok.files, 4);
    // The body hash recomputes; an edit to the body is caught by both checks.
    let mut doc: Value = serde_json::from_str(&read(&dir.join("report.json"))).unwrap();
    assert!(arksa_kpro::ops::verify_body_hash(&doc, "report_sha256").is_ok());
    doc["body"]["status"] = Value::String("confirmed".into());
    doc["body"]["steps"][0]["status"] = Value::String("aborted".into());
    let e = arksa_kpro::ops::verify_body_hash(&doc, "report_sha256").unwrap_err();
    assert!(e.contains("the body hashes to"), "{e}");
    std::fs::write(
        dir.join("report.json"),
        serde_json::to_string_pretty(&doc).unwrap(),
    )
    .unwrap();
    let bad = kshana_pro::signed_dossier::verify(&dir, &[pk]).unwrap();
    assert!(
        bad.problems.iter().any(|p| p == "report.json: changed"),
        "{:?}",
        bad.problems
    );
    // A directory with other files is refused, so a signature never covers stale outputs.
    std::fs::write(dir.join("stale.txt"), "x").unwrap();
    let e = arksa_kpro::ops::write_dir(&dir, &[("report.md", b"x".to_vec())], None).unwrap_err();
    assert!(e.to_string().contains("already holds other files"), "{e}");
}

// ------------------------------------------------------------------ scorecards

fn scorecard(spec: &str) -> ad_scorecard::Scorecard {
    let p = fixtures().join("ad-scorecard").join(spec);
    ad_scorecard::run(&read(&p), p.parent().unwrap()).unwrap().0
}

fn metric(sc: &ad_scorecard::Scorecard, k: &str) -> Option<f64> {
    *sc.body
        .metrics
        .get(k)
        .unwrap_or_else(|| panic!("no metric {k}"))
}

#[test]
fn the_interval_scorecard_matches_the_hand_computed_figures() {
    let sc = scorecard("spec-esa-ad.toml");
    let b = &sc.body;
    assert_eq!(b.evidence_class, "MODELLED");
    // Three anomalies in the window (id_6 lies outside); the rare event and the
    // communication gap are ignored, and so are the alarms on them.
    for (k, v) in [
        ("events", 3),
        ("events_ignored", 2),
        ("events_detected", 2),
        ("alarms", 7),
        ("alarms_false", 2),
        ("alarms_on_ignored_events", 2),
        ("nominal_seconds", 218 * 3600),
        ("alarmed_nominal_seconds", 12_600),
    ] {
        assert_eq!(b.counts[k], v, "{k}");
    }
    let tnr = (218.0 - 3.5) / 218.0;
    let p = 0.5 * tnr;
    let r = 2.0 / 3.0;
    let close = |k: &str, want: f64| {
        let got = metric(&sc, k).unwrap();
        assert!((got - want).abs() < 2e-6, "{k}: {got} vs {want}");
    };
    close("event_recall", r);
    close("event_precision", 0.5);
    close("time_tnr", tnr);
    close("event_precision_corrected", p);
    close("event_f0_5", 1.25 * p * r / (0.25 * p + r));
    close("channel_precision", 0.75);
    close("channel_recall", 0.75);
    close("alarming_precision", 2.0 / 3.0);
    close("false_alarms_per_day", 2.0 / (218.0 / 24.0));
    close("latency_median_s", 0.0);
    close("latency_p90_s", 1800.0);
    assert_eq!(b.missed_events, ["id_5"]);
    assert_eq!(b.recall_by_type["Locality=Local"], (1, 2));
    // Pre-registered bars, judged; a metric the benchmark does not produce is not applicable.
    let v: Vec<(&str, &str)> = b
        .bars
        .iter()
        .map(|x| (x.metric.as_str(), x.verdict.as_str()))
        .collect();
    assert_eq!(
        v,
        [
            ("event_recall", "PASS"),
            ("event_precision_corrected", "FAIL"),
            ("false_alarms_per_day", "PASS"),
            ("f1", "NOT APPLICABLE")
        ]
    );
    assert_eq!(b.verdict, "FAIL");
    assert_eq!(
        b.bars_registered_utc.as_deref(),
        Some("2025-12-20T00:00:00Z")
    );
    assert!(sc.to_markdown().contains("Not an ESA result"));
}

#[test]
fn rare_events_scope_and_changed_bars_change_the_result_or_are_refused() {
    let dir = fixtures().join("ad-scorecard");
    let truth = ad_scorecard::parse_esa_ad(
        &read(&dir.join("labels.csv")),
        &read(&dir.join("anomaly_types.csv")),
    )
    .unwrap();
    let alarms = ad_scorecard::parse_alarms(&read(&dir.join("alarms.csv"))).unwrap();
    let (w0, w1) = (
        arksa_kpro::ops::parse_utc("2026-01-01T00:00:00Z").unwrap(),
        arksa_kpro::ops::parse_utc("2026-01-11T00:00:00Z").unwrap(),
    );
    // Counting rare events: id_3 becomes an event and alarm a4 detects it.
    let s = ad_scorecard::score_intervals(&truth, &alarms, w0, w1, &[], RareEvents::Count).unwrap();
    assert_eq!((s.events, s.tp_events, s.ignored_alarms), (4, 3, 1));
    // Scope to channel_3 only: id_2 is the one event, a2 and a3 the alarms on it.
    let s = ad_scorecard::score_intervals(
        &truth,
        &alarms,
        w0,
        w1,
        &["channel_3".into()],
        RareEvents::Ignore,
    )
    .unwrap();
    assert_eq!((s.events, s.tp_events, s.alarms, s.fp_alarms), (1, 1, 2, 0));
    assert_eq!(s.channel_precision, Some(1.0));

    // Bars changed after their hash was published: refused.
    let spec = read(&dir.join("spec-esa-ad.toml")).replace("19347b1c", "00000000");
    let e = ad_scorecard::run(&spec, &dir).unwrap_err().to_string();
    assert!(e.contains("refusing to score against changed bars"), "{e}");
    // Bad inputs.
    for (spec, why) in [
        (
            read(&dir.join("spec-esa-ad.toml")).replace("format = \"esa-ad\"", "format = \"nab\""),
            "not esa-ad or opssat-ad",
        ),
        (
            read(&dir.join("spec-esa-ad.toml"))
                .replace("window_end_utc = \"2026-01-11T00:00:00Z\"\n", ""),
            "needs window_end_utc",
        ),
        (
            read(&dir.join("spec-esa-ad.toml"))
                .replace("data_origin = \"synthetic\"", "data_origin = \"made-up\""),
            "unknown variant",
        ),
    ] {
        let e = ad_scorecard::run(&spec, &dir).unwrap_err().to_string();
        assert!(e.contains(why), "{why}: {e}");
    }
    let e = ad_scorecard::parse_esa_ad(
        "ID,Channel,StartTime,EndTime\nid_9,c,2026-01-01T00:00:00Z,2026-01-01T01:00:00Z\n",
        "ID,Category\n",
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("no row in anomaly_types.csv"), "{e}");
}

#[test]
fn the_segment_scorecard_matches_the_hand_computed_figures() {
    let sc = scorecard("spec-opssat-ad.toml");
    let b = &sc.body;
    assert_eq!(
        (
            b.counts["segments"],
            b.counts["tp"],
            b.counts["fp"],
            b.counts["tn"],
            b.counts["fn"]
        ),
        (6, 1, 1, 3, 1)
    );
    for (k, want) in [
        ("precision", 0.5),
        ("recall", 0.5),
        ("f1", 0.5),
        ("accuracy", 0.666667),
        ("balanced_accuracy", 0.625),
        ("mcc", 0.25),
        ("roc_auc", 0.875),
        ("average_precision", 0.833333),
        ("false_alarms_per_100_nominal_segments", 25.0),
    ] {
        assert_eq!(metric(&sc, k), Some(want), "{k}");
    }
    assert_eq!(b.verdict, "PASS");
    assert!(b.notes.iter().any(|n| n.contains("no bars_sha256")));

    let dir = fixtures().join("ad-scorecard");
    let truth = ad_scorecard::parse_opssat(&read(&dir.join("dataset.csv"))).unwrap();
    let missing = ad_scorecard::parse_predictions("segment,predicted\n5,0\n").unwrap();
    let e = ad_scorecard::score_segments(&truth, &missing, true)
        .unwrap_err()
        .to_string();
    assert!(e.contains("miss 5 of the 6"), "{e}");
    let unknown = ad_scorecard::parse_predictions("segment,predicted\n99,1\n").unwrap();
    let e = ad_scorecard::score_segments(&truth, &unknown, true)
        .unwrap_err()
        .to_string();
    assert!(e.contains("segment 99"), "{e}");
    assert!(ad_scorecard::parse_predictions("segment,predicted\n5,yes\n").is_err());
}

// ------------------------------------------------------------------ tracking gaps

#[test]
fn the_tracking_gap_study_counts_what_the_outage_costs_and_what_alternates_recover() {
    let p = fixtures().join("tracking-gap/study.toml");
    let (imp, files) = tracking_gap::run(&read(&p), p.parent().unwrap()).unwrap();
    let b = &imp.body;
    assert_eq!(b.evidence_class, "MODELLED");
    assert_eq!(files.len(), 4);
    assert_eq!(b.stations[0].station, "DSS-A");
    assert_eq!(
        (b.stations[0].outage_h, b.stations[0].passes_impacted),
        (33.0, 3)
    );

    let mo = &b.missions[0];
    assert_eq!(mo.mission, "MARS-ORBITER");
    assert_eq!(mo.impacted_passes.len(), 2);
    assert_eq!(
        (
            mo.impacted_passes[0].effect.as_str(),
            mo.impacted_passes[0].lost_min
        ),
        ("lost", 480.0)
    );
    assert_eq!(
        (
            mo.impacted_passes[1].effect.as_str(),
            mo.impacted_passes[1].lost_min
        ),
        ("shortened", 420.0)
    );
    assert_eq!(mo.contact_lost_h, 15.0);
    // 37 h without contact at 0.72 Gbit/h fills the 20 Gbit recorder and overflows 6.64
    // Gbit; the 1 h left of the next pass drains 6.48, and the last 14 h overflow 3.6 more.
    assert!(
        (mo.with_outages.overflow_gbit - 10.24).abs() < 1e-6,
        "{}",
        mo.with_outages.overflow_gbit
    );
    assert_eq!(mo.scheduled.overflow_gbit, 0.0);
    assert!((mo.science_lost_gbit - 10.24).abs() < 1e-6);
    assert!((mo.scheduled.recorder_end_gbit - 10.08).abs() < 1e-6);
    assert_eq!(mo.with_outages.recorder_end_gbit, 20.0);
    assert_eq!(mo.with_outages.longest_gap_h, 37.0);
    assert_eq!(mo.new_gap_violations, 1);
    assert_eq!(
        mo.with_outages.ranging_days_below_minimum,
        ["2026-11-02 (0 of 1)"]
    );
    assert_eq!(mo.recovered_fraction, Some(1.0));
    let alt = mo.with_alternates.as_ref().unwrap();
    assert_eq!((alt.longest_gap_h, alt.gaps_over_limit.len()), (18.0, 0));

    let ll = &b.missions[1];
    assert_eq!(ll.critical_windows_lost, ["Powered descent"]);
    assert!(!ll.with_outages.critical[0].met && ll.with_outages.critical[0].fraction == 0.0);
    assert!(ll.with_alternates.as_ref().unwrap().critical[0].met);
    assert_eq!(ll.new_gap_violations, 0);
    let md = imp.to_markdown();
    assert!(md.contains("critical window 'Powered descent' no longer covered"));
    assert!(md.contains("MODELLED"));
}

#[test]
fn bad_tracking_inputs_are_refused() {
    let dir = fixtures().join("tracking-gap");
    let study = read(&dir.join("study.toml"));
    for (text, why) in [
        (
            study.replace(
                "end_utc = \"2026-11-04T00:00:00Z\"",
                "end_utc = \"2026-10-01T00:00:00Z\"",
            ),
            "not after study.start_utc",
        ),
        (
            study.replace(
                "recorder_initial_gbit = 5.0",
                "recorder_initial_gbit = 50.0",
            ),
            "recorder_initial_gbit <= capacity",
        ),
        (
            study.replace("max_gap_h = 30.0", "max_gap_h = 0.0"),
            "max_gap_h must be > 0",
        ),
        (
            study.replace("id = \"LUNAR-LANDER\"", "id = \"MARS-ORBITER\""),
            "listed twice",
        ),
    ] {
        let e = tracking_gap::run(&text, &dir).unwrap_err().to_string();
        assert!(e.contains(why), "{why}: {e}");
    }
    let sched = "pass_id,mission,station,start_utc,end_utc,downlink_kbps,services\n";
    for (row, why) in [
        (
            "P,M,S,2026-11-01T02:00:00Z,2026-11-01T01:00:00Z,10,TM",
            "not after start_utc",
        ),
        (
            "P,M,S,2026-11-01T00:00:00Z,2026-11-01T01:00:00Z,-1,TM",
            "negative",
        ),
        (
            "P,M,S,2026-11-01T00:00:00Z,2026-11-01T01:00:00Z,10,TM;VIDEO",
            "service 'VIDEO'",
        ),
    ] {
        let e = tracking_gap::parse_passes("schedule", &format!("{sched}{row}\n"))
            .unwrap_err()
            .to_string();
        assert!(e.contains(why), "{why}: {e}");
    }
}

// ------------------------------------------------------------------ the command

#[test]
fn the_ops_command_needs_the_arksa_entitlement_and_writes_what_it_says() {
    let out = tmp("bin-refused");
    let refused = bin()
        .args(["ops", "tracking-gap"])
        .arg(fixtures().join("tracking-gap/study.toml"))
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains(arksa_kpro::license::tiers::NOT_IN_THIS_EDITION));
    assert!(!out.exists());

    let out = tmp("bin-report");
    let ok = bin()
        .args(["ops", "report"])
        .arg(fixtures().join("test-report/procedure.toml"))
        .arg(fixtures().join("test-report/run-deviations.csv"))
        .arg("--out")
        .arg(&out)
        .arg("--dev")
        .env_remove("ARKSA_STUDY_SIGNING_KEY")
        .output()
        .unwrap();
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let stdout = String::from_utf8_lossy(&ok.stdout);
    assert!(
        stdout.contains("NOT CONFIRMED") && stdout.contains("not signed"),
        "{stdout}"
    );
    for f in ["report.json", "report.md", "procedure.toml", "run-log.csv"] {
        assert!(out.join(f).exists(), "{f}");
    }
    let v = bin()
        .args(["ops", "verify"])
        .arg(&out)
        .arg("--dev")
        .output()
        .unwrap();
    assert!(v.status.success() && String::from_utf8_lossy(&v.stdout).starts_with("OK"));

    let key = tmp("key");
    std::fs::create_dir_all(&key).unwrap();
    std::fs::write(key.join("k.hex"), "11".repeat(32)).unwrap();
    for (cmd, input, file) in [
        (
            "ad-score",
            "ad-scorecard/spec-esa-ad.toml",
            "scorecard.json",
        ),
        ("tracking-gap", "tracking-gap/study.toml", "impact.json"),
    ] {
        let out = tmp(&format!("bin-{cmd}"));
        let ok = bin()
            .args(["ops", cmd])
            .arg(fixtures().join(input))
            .arg("--out")
            .arg(&out)
            .arg("--sign-key-file")
            .arg(key.join("k.hex"))
            .arg("--dev")
            .output()
            .unwrap();
        assert!(
            ok.status.success(),
            "{cmd}: {}",
            String::from_utf8_lossy(&ok.stderr)
        );
        assert!(String::from_utf8_lossy(&ok.stdout).contains("signed: "));
        assert!(out.join(file).exists() && out.join("signature.json").exists());
        let pk = arksa_kpro::license::public_key_for(&[0x11; 32]);
        assert!(kshana_pro::signed_dossier::verify(&out, &[pk])
            .unwrap()
            .passed());
    }
    let usage = bin().args(["ops", "nonsense", "--dev"]).output().unwrap();
    assert_eq!(usage.status.code(), Some(2));
}

// ------------------------------------------------------------------ public benchmarks (opt-in)

/// Needs `ARKSA_ESA_AD_DIR` = a folder with ESA-Mission1's `labels.csv` and
/// `anomaly_types.csv` (ESA Anomalies Dataset, doi:10.5281/zenodo.12528696, CC BY 3.0 IGO),
/// fetched by `scripts/fetch_ad_benchmarks.py`.
#[test]
#[ignore = "fetches nothing itself: run scripts/fetch_ad_benchmarks.py and set ARKSA_ESA_AD_DIR"]
fn esa_ad_mission1_annotations_parse_and_score() {
    let dir = PathBuf::from(std::env::var("ARKSA_ESA_AD_DIR").expect("set ARKSA_ESA_AD_DIR"));
    let labels = read(&dir.join("labels.csv"));
    let truth = ad_scorecard::parse_esa_ad(&labels, &read(&dir.join("anomaly_types.csv"))).unwrap();
    assert_eq!(truth.rows.len(), 200);
    let cats = |c: &str| truth.types.values().filter(|t| t["Category"] == c).count();
    assert_eq!(
        (
            cats("Anomaly"),
            cats("Rare Event"),
            cats("Communication Gap")
        ),
        (118, 78, 4)
    );

    // An oracle that raises exactly the annotated anomaly intervals scores perfectly.
    let mut oracle = String::from("alarm_id,channel,start_utc,end_utc\n");
    for (id, rows) in &truth.rows {
        if truth.types[id]["Category"] == "Anomaly" {
            for (c, a, b) in rows {
                oracle += &format!(
                    "{id},{c},{},{}\n",
                    arksa_kpro::ops::fmt_utc_ms(*a),
                    arksa_kpro::ops::fmt_utc_ms(*b)
                );
            }
        }
    }
    let spec = |outputs: &Path| {
        format!(
            "[benchmark]\nformat = \"esa-ad\"\nname = \"ESA-AD Mission1 (full span)\"\ndata_origin = \"public-benchmark\"\nsource_doi = \"10.5281/zenodo.12528696\"\nlicence = \"CC BY 3.0 IGO\"\nlabels = {:?}\nanomaly_types = {:?}\nwindow_start_utc = \"2000-01-01T00:00:00Z\"\nwindow_end_utc = \"2014-01-01T00:00:00Z\"\n[detector]\nname = \"oracle\"\nversion = \"test\"\noutputs = {:?}\n",
            dir.join("labels.csv"),
            dir.join("anomaly_types.csv"),
            outputs
        )
    };
    let t = tmp("esa-ad-real");
    std::fs::create_dir_all(&t).unwrap();
    std::fs::write(t.join("oracle.csv"), &oracle).unwrap();
    let (sc, _) = ad_scorecard::run(&spec(&t.join("oracle.csv")), &t).unwrap();
    assert_eq!(sc.body.evidence_class, "MEASURED");
    assert_eq!(sc.body.counts["events"], 118);
    for k in [
        "event_recall",
        "event_precision_corrected",
        "channel_precision",
        "channel_recall",
    ] {
        assert_eq!(metric(&sc, k), Some(1.0), "{k}");
    }
    // One alarm over the whole span on every channel finds every event, but the time-wise
    // correction takes its precision to zero.
    let chans: std::collections::BTreeSet<&String> =
        truth.rows.values().flatten().map(|r| &r.0).collect();
    let mut always = String::from("alarm_id,channel,start_utc,end_utc\n");
    for c in chans {
        always += &format!("x,{c},2000-01-01T00:00:00Z,2014-01-01T00:00:00Z\n");
    }
    std::fs::write(t.join("always.csv"), &always).unwrap();
    let (sc, _) = ad_scorecard::run(&spec(&t.join("always.csv")), &t).unwrap();
    assert_eq!(metric(&sc, "event_recall"), Some(1.0));
    assert_eq!(metric(&sc, "event_precision"), Some(1.0));
    assert_eq!(metric(&sc, "event_precision_corrected"), Some(0.0));
}

/// Needs `ARKSA_OPSSAT_AD_CSV` = OPSSAT-AD `dataset.csv` (doi:10.5281/zenodo.15108715),
/// fetched by `scripts/fetch_ad_benchmarks.py`.
#[test]
#[ignore = "fetches nothing itself: run scripts/fetch_ad_benchmarks.py and set ARKSA_OPSSAT_AD_CSV"]
fn opssat_ad_dataset_parses_and_scores() {
    let csv = PathBuf::from(std::env::var("ARKSA_OPSSAT_AD_CSV").expect("set ARKSA_OPSSAT_AD_CSV"));
    let truth = ad_scorecard::parse_opssat(&read(&csv)).unwrap();
    assert_eq!(truth.len(), 2123);
    let test: Vec<(&String, bool)> = truth
        .iter()
        .filter(|(_, v)| !v.1)
        .map(|(s, v)| (s, v.0))
        .collect();
    assert_eq!(
        (test.len(), test.iter().filter(|x| x.1).count()),
        (529, 113)
    );
    let mut perfect = String::from("segment,predicted,score\n");
    for (s, a) in &test {
        perfect += &format!("{s},{},{}\n", u8::from(*a), if *a { 0.9 } else { 0.1 });
    }
    let pred = ad_scorecard::parse_predictions(&perfect).unwrap();
    let (m, c, _) = ad_scorecard::score_segments(&truth, &pred, true).unwrap();
    assert_eq!(
        (m["f1"], m["roc_auc"], m["mcc"]),
        (Some(1.0), Some(1.0), Some(1.0))
    );
    assert_eq!(c["segments"], 529);
}
