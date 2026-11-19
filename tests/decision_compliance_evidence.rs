// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Collision-avoidance decision records, the threshold-policy comparator, the disposal
//! compliance screening and the dossier's space-safety section.
//!
//! The inputs in `scenarios/space-safety/` are synthetic and illustrative.

use std::path::{Path, PathBuf};
use std::process::Command;

use arksa_kpro::decision::{policy, record};
use arksa_kpro::disposal::{self, rules, Verdict};
use arksa_kpro::license::{Entitlement, Tier};
use arksa_kpro::mbse::evidence::sha256_hex;
use arksa_kpro::mission::check::check;
use arksa_kpro::mission::{build, BuildOptions};
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios/space-safety")
}

fn mission_fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios/pro-power/mission")
}

fn tmp(tag: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("space-safety-{tag}"));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn decision(text: &str) -> Result<record::DecisionRecord, String> {
    record::DecisionInput::parse(text)
        .and_then(|i| record::build(i, &sha256_hex(text.as_bytes())))
        .map_err(|e| e.to_string())
}

fn fixture_record(name: &str) -> record::DecisionRecord {
    let text = read(&fixtures().join("decisions").join(name));
    decision(&text).unwrap()
}

fn as_value(r: &record::DecisionRecord) -> Value {
    serde_json::from_str(&r.to_json()).unwrap()
}

fn disposal_of(text: &str) -> disposal::Assessment {
    let d = disposal::DisposalInput::parse(text).unwrap();
    disposal::assess(&d, &sha256_hex(text.as_bytes()))
}

fn verdicts<'a>(a: &'a disposal::Assessment, rule: &str) -> Vec<&'a disposal::Check> {
    a.body.checks.iter().filter(|c| c.rule == rule).collect()
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_arksa-kpro"))
}

// ------------------------------------------------------------------ decision records

#[test]
fn the_fixture_records_follow_their_policy_and_chain() {
    let a = fixture_record("ev-001.toml");
    let f = &a.body.findings;
    assert_eq!(f.band, record::Band::AboveManoeuvreThreshold);
    assert_eq!(f.consistency, record::Consistency::FollowsPolicy);
    assert!(
        f.mitigation.contains("below the manoeuvre threshold"),
        "{}",
        f.mitigation
    );
    assert!(f.notes.is_empty(), "{:?}", f.notes);
    assert!(
        (f.hours_before_tca - 30.733).abs() < 1e-3,
        "{}",
        f.hours_before_tca
    );

    let b = fixture_record("ev-002.toml");
    assert_eq!(b.body.findings.band, record::Band::MonitoringBand);
    assert_eq!(
        b.body.findings.consistency,
        record::Consistency::FollowsPolicy
    );
    // The second fixture names the first: this keeps the committed hash current.
    assert_eq!(
        b.body.input.prev_record_sha256.as_deref(),
        Some(a.record_sha256.as_str()),
        "update prev_record_sha256 in decisions/ev-002.toml to {}",
        a.record_sha256
    );
    assert!(record::verify_chain(&[as_value(&a), as_value(&b)]).is_empty());
    // Out of order, the chain breaks and says where.
    let p = record::verify_chain(&[as_value(&b), as_value(&a)]);
    assert!(p.iter().any(|x| x.starts_with("record 2:")), "{p:?}");
}

#[test]
fn a_departure_from_the_policy_is_recorded_with_its_rationale_and_flagged() {
    let text = read(&fixtures().join("decisions/ev-001.toml"))
        .replace("chosen_option = \"M1\"", "chosen_option = \"W1\"")
        .replace(
            "ca_provider_informed = true",
            "ca_provider_informed = false",
        );
    let r = decision(&text).unwrap();
    let f = &r.body.findings;
    assert_eq!(f.consistency, record::Consistency::DepartsFromPolicy);
    assert!(f
        .notes
        .iter()
        .any(|n| n.contains("departure from the policy rests on the rationale")));
    assert!(f.notes.iter().any(|n| n.contains("Article 64(5)")));
    assert!(f.rule_refs.iter().any(|x| x == "EU-Art64-5"));

    // A manoeuvre below every threshold is more conservative than the policy.
    let text = read(&fixtures().join("decisions/ev-002.toml"))
        .replace("chosen_option = \"C1\"", "chosen_option = \"M1\"");
    let r = decision(&text).unwrap();
    assert_eq!(
        r.body.findings.consistency,
        record::Consistency::MoreConservativeThanPolicy
    );
    // ... and without stated secondary effects, a review note asks for them.
    assert!(r
        .body
        .findings
        .notes
        .iter()
        .any(|n| n.contains("other conjunctions")));

    // A miss distance below the policy's minimum calls for a manoeuvre even at low Pc.
    let (band, why) = record::band_of(
        1e-7,
        50.0,
        &record::ThresholdPolicy {
            name: "p".into(),
            manoeuvre_pc: 1e-4,
            monitor_pc: None,
            min_miss_distance_m: Some(200.0),
            source: None,
        },
    );
    assert_eq!(band, record::Band::AboveManoeuvreThreshold);
    assert!(why.contains("miss distance"));
}

#[test]
fn a_manoeuvre_that_does_not_reduce_the_risk_is_not_shown_as_mitigation() {
    let text = read(&fixtures().join("decisions/ev-001.toml"))
        .replace("post_pc = 2.0e-7", "post_pc = 5.0e-4");
    let r = decision(&text).unwrap();
    assert!(r.body.findings.mitigation.starts_with("NOT SHOWN"));
    let text = read(&fixtures().join("decisions/ev-001.toml")).replace("post_pc = 2.0e-7\n", "");
    let r = decision(&text).unwrap();
    assert_eq!(r.body.findings.mitigation, "not stated");
    assert!(!r.body.findings.notes.is_empty());
}

#[test]
fn bad_decision_inputs_are_refused_with_the_reason() {
    let base = read(&fixtures().join("decisions/ev-001.toml"));
    let cases = [
        (
            base.replace(
                "decided_utc = \"2026-04-30T20:30:00Z\"",
                "decided_utc = \"2026-05-02T04:00:00Z\"",
            ),
            "not before the time of closest approach",
        ),
        (
            base.replace("chosen_option = \"M1\"", "chosen_option = \"X9\""),
            "'X9' is not one of the options",
        ),
        (base.replace("pc = 3.1e-4", "pc = nan"), "not a probability"),
        (base.replace("pc = 3.1e-4", "pc = 1.5"), "not a probability"),
        (
            base.replace("monitor_pc = 1.0e-5", "monitor_pc = 1.0e-3"),
            "monitor_pc may not be above",
        ),
        (base.replace("id = \"W1\"", "id = \"M1\""), "appears twice"),
        (
            base.replace(
                "tca_utc = \"2026-05-02T03:14:00Z\"",
                "tca_utc = \"2026-05-02 03:14\"",
            ),
            "event.tca_utc",
        ),
        (
            base.replace("rationale = \"Pc is", "rationale = \"\"\nx = \"Pc is"),
            "invalid decision input",
        ),
        (
            format!("prev_record_sha256 = \"abc\"\n{base}"),
            "64 lower-case hexadecimal",
        ),
        (
            format!("{base}\nunknown_key = 1\n"),
            "invalid decision input",
        ),
    ];
    for (text, want) in cases {
        let e = decision(&text).unwrap_err();
        assert!(e.contains(want), "want '{want}' in: {e}");
    }
}

#[test]
fn an_altered_record_fails_the_chain_check_and_a_signed_record_verifies() {
    let a = fixture_record("ev-001.toml");
    let mut v = as_value(&a);
    v["body"]["input"]["decision"]["rationale"] = Value::String("changed later".into());
    let p = record::verify_chain(&[v]);
    assert!(p.iter().any(|x| x.contains("the body hashes to")), "{p:?}");

    // Signed as a whole with Kshana Pro's dossier signature; any change is detected.
    let dir = tmp("signed-decision");
    let seed = [7u8; 32];
    let input = std::fs::read(fixtures().join("decisions/ev-001.toml")).unwrap();
    let sig = arksa_kpro::decision::write_record(
        &a,
        &input,
        &dir,
        Some((&seed, "Arksa test", "2026-11-19T00:00:00Z")),
    )
    .unwrap()
    .expect("signed");
    assert!(sig.ends_with("signature.json"));
    let pk = arksa_kpro::license::public_key_for(&seed);
    let ok = kshana_pro::signed_dossier::verify(&dir, &[pk]).unwrap();
    assert!(ok.passed(), "{:?}", ok.problems);
    assert_eq!(ok.files, 3);
    let md = read(&dir.join("decision.md"));
    assert!(md.contains("MODELLED") && md.contains(&a.record_sha256));
    std::fs::write(dir.join("decision.md"), md.replace("EV-001", "EV-999")).unwrap();
    let bad = kshana_pro::signed_dossier::verify(&dir, &[pk]).unwrap();
    assert!(!bad.passed());
    assert!(
        bad.problems.iter().any(|p| p == "decision.md: changed"),
        "{:?}",
        bad.problems
    );
}

// ------------------------------------------------------------------ thresholds

#[test]
fn the_threshold_comparator_counts_manoeuvres_and_accepted_risk() {
    let text = read(&fixtures().join("events.csv"));
    let events = policy::parse_events(&text).unwrap();
    assert_eq!(events.len(), 10);
    let c = policy::compare(
        &events,
        &policy::Study {
            thresholds: vec![1e-4, 3e-7, 1e-5, 1e-6],
            period_days: Some(180.0),
            default_post_pc: 0.0,
        },
        &sha256_hex(text.as_bytes()),
    )
    .unwrap();
    let rows = &c.body.rows;
    let m: Vec<usize> = rows.iter().map(|r| r.manoeuvres).collect();
    assert_eq!(m, vec![1, 3, 5, 7]);
    for w in rows.windows(2) {
        assert!(w[1].risk_accepted <= w[0].risk_accepted);
        assert!(w[1].reduction >= w[0].reduction);
    }
    // At 1e-4 only EV-001 is manoeuvred, to its stated 2e-7.
    let total: f64 = events.iter().map(|e| e.pc).sum();
    assert!((rows[0].risk_accepted - (total - 3.1e-4 + 2.0e-7)).abs() < 1e-9);
    assert!((rows[0].manoeuvres_per_year.unwrap() - 365.25 / 180.0).abs() < 1e-4);
    assert_eq!(c.body.esa_reference_threshold, Some(1e-5));
    assert!(c.label.starts_with("MODELLED"));
    assert!(c.to_csv().lines().count() == 5);

    for (bad, want) in [
        ("event_id,pc\nA,1e-4\n", "header"),
        (
            "event_id,tca_utc,pc\nA,2026-01-01T00:00:00Z,2\n",
            "not a probability",
        ),
        (
            "event_id,tca_utc,pc\nA,2026-01-01T00:00:00Z,1e-4\nA,2026-01-02T00:00:00Z,1e-5\n",
            "repeated",
        ),
        ("event_id,tca_utc,pc\n", "lists no event"),
    ] {
        let e = policy::parse_events(bad).unwrap_err().to_string();
        assert!(e.contains(want), "{want}: {e}");
    }
}

// ------------------------------------------------------------------ disposal

#[test]
fn the_smallsat_plan_is_marginal_on_lifetime_and_compliant_on_declared_figures() {
    let a = disposal_of(&read(&fixtures().join("disposal-leo-smallsat.toml")));
    let fcc = verdicts(&a, "FCC-25.283-e");
    assert_eq!(fcc.len(), 1);
    // Moderate activity: about 4 years; a constant solar minimum: well past 5.
    assert_eq!(fcc[0].verdict, Verdict::Marginal, "{:?}", fcc[0]);
    assert_eq!(fcc[0].basis, "MODELLED");
    let l = &a.body.lifetime;
    assert_eq!(l.basis, "MODELLED");
    assert!(
        l.nominal_years.unwrap() < 5.0 && l.upper_years.unwrap() > 5.0,
        "{l:?}"
    );
    assert_eq!(verdicts(&a, "FCC-25.283-c")[0].verdict, Verdict::Compliant);
    assert_eq!(verdicts(&a, "ESA-5.5-b")[0].verdict, Verdict::Compliant);
    assert_eq!(verdicts(&a, "ESA-5.4.1.1-a")[0].verdict, Verdict::Compliant);
    assert_eq!(verdicts(&a, "ESA-5.3.2.2-c")[0].verdict, Verdict::Compliant);
    assert!(a.label.starts_with("MODELLED"));
    // Deterministic: the same input gives the same hash.
    let b = disposal_of(&read(&fixtures().join("disposal-leo-smallsat.toml")));
    assert_eq!(a.assessment_sha256, b.assessment_sha256);
}

#[test]
fn a_high_orbit_without_lowering_is_not_compliant_and_missing_inputs_are_not_assessed() {
    let a = disposal_of(&read(&fixtures().join("disposal-high-leo.toml")));
    assert_eq!(
        verdicts(&a, "FCC-25.283-e")[0].verdict,
        Verdict::NotCompliant
    );
    let esa = verdicts(&a, "ESA-5.4.2.3-a");
    assert_eq!(esa[0].verdict, Verdict::NotCompliant);
    assert_eq!(
        esa[1].verdict,
        Verdict::NotAssessed,
        "no collision probability stated"
    );
    // Propulsion declared but no propellant passivation planned.
    let c = verdicts(&a, "FCC-25.283-c")[0];
    assert_eq!(c.verdict, Verdict::NotCompliant);
    assert!(c.detail.contains("propellant"));
    assert_eq!(
        verdicts(&a, "ESA-5.4.1.1-a")[0].verdict,
        Verdict::NotCompliant
    );
    assert_eq!(verdicts(&a, "ESA-5.5-b")[0].verdict, Verdict::NotAssessed);
    // Past 25 years the stricter ESA figures apply.
    assert_eq!(verdicts(&a, "ESA-5.3.2.2-c")[0].limit, "at least 0.95");
    assert_eq!(verdicts(&a, "ESA-5.4.1.1-b").len(), 1);
}

#[test]
fn an_operator_lifetime_takes_precedence_and_a_disagreement_is_flagged() {
    let base = read(&fixtures().join("disposal-leo-smallsat.toml"));
    let with = format!(
        "{base}\n[lifetime]\nnominal_years = 3.0\nupper_years = 4.6\ntool = \"operator's lifetime tool (illustrative)\"\n"
    );
    let a = disposal_of(&with);
    assert_eq!(a.body.lifetime.basis, "OPERATOR-SUPPLIED");
    let fcc = verdicts(&a, "FCC-25.283-e")[0];
    assert_eq!(fcc.verdict, Verdict::Compliant);
    assert_eq!(fcc.basis, "OPERATOR-SUPPLIED");
    // The screening model's conservative case says more than five years.
    let x = a.body.lifetime.cross_check.as_deref().unwrap();
    assert!(x.contains("different sides of the five-year limit"), "{x}");
    assert!(
        !a.body.lifetime.modelled.is_empty(),
        "the model is still shown"
    );
}

#[test]
fn strategy_orbit_shape_and_constellation_change_which_rules_apply() {
    let base = read(&fixtures().join("disposal-leo-smallsat.toml"));
    // Controlled re-entry: the five-year rule for uncontrolled re-entry does not apply.
    let a = disposal_of(&base.replace(
        "strategy = \"natural_decay\"",
        "strategy = \"controlled_reentry\"",
    ));
    assert_eq!(
        verdicts(&a, "FCC-25.283-e")[0].verdict,
        Verdict::NotApplicable
    );
    assert_eq!(
        verdicts(&a, "ESA-5.3.2.2-b")[0].verdict,
        Verdict::NotApplicable
    );
    // An elliptical orbit is outside the circular model: no lifetime, not assessed.
    let a = disposal_of(&base.replace("apogee_km = 525.0", "apogee_km = 1200.0"));
    assert!(a.body.lifetime.modelled.is_empty());
    assert_eq!(a.body.lifetime.basis, "NONE");
    assert_eq!(
        verdicts(&a, "FCC-25.283-e")[0].verdict,
        Verdict::NotAssessed
    );
    // A large constellation adds the 375 km disposal apogee and the 0.95 reliability.
    let a = disposal_of(&base.replace("large_constellation = false", "large_constellation = true"));
    let c = verdicts(&a, "ESA-5.4.2.3-c");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].verdict, Verdict::NotCompliant);
    assert_eq!(
        verdicts(&a, "ESA-5.4.1.1-b")[0].verdict,
        Verdict::NotAssessed
    );
    // One framework only.
    let a = disposal_of(&base.replace(
        "frameworks = [\"FCC\", \"ESA\", \"EU\"]",
        "frameworks = [\"ESA\"]",
    ));
    assert!(a.body.checks.iter().all(|c| c.framework == "ESA"));
}

#[test]
fn bad_disposal_inputs_are_refused() {
    let base = read(&fixtures().join("disposal-leo-smallsat.toml"));
    for (text, want) in [
        (
            base.replace("mass_kg = 150.0", "mass_kg = 0.0"),
            "must be positive",
        ),
        (
            base.replace("apogee_km = 525.0", "apogee_km = 500.0"),
            "not above apogee_km",
        ),
        (
            base.replace(
                "probability_successful_disposal = 0.93",
                "probability_successful_disposal = 1.2",
            ),
            "not a probability",
        ),
        (
            base.replace(
                "frameworks = [\"FCC\", \"ESA\", \"EU\"]",
                "frameworks = [\"ITU\"]",
            ),
            "unknown framework",
        ),
        (
            base.replace("kind = \"battery\"", "kind = \"flywheel\""),
            "passivation source kind",
        ),
        (
            base.replace("strategy = \"natural_decay\"", "strategy = \"hope\""),
            "invalid disposal input",
        ),
    ] {
        let e = disposal::DisposalInput::parse(&text)
            .unwrap_err()
            .to_string();
        assert!(e.contains(want), "{want}: {e}");
    }
}

/// Honesty: every check cites a registered rule; a check that passes or fails a limit
/// cites a verified primary text; the EU proposal's rows never pass or fail anything.
#[test]
fn every_verdict_rests_on_a_verified_rule_and_the_eu_proposal_is_never_passed_or_failed() {
    for f in ["disposal-leo-smallsat.toml", "disposal-high-leo.toml"] {
        let a = disposal_of(&read(&fixtures().join(f)));
        for c in &a.body.checks {
            let r = rules::rule(&c.rule).unwrap_or_else(|| panic!("{} unregistered", c.rule));
            if matches!(
                c.verdict,
                Verdict::Compliant | Verdict::NotCompliant | Verdict::Marginal
            ) {
                assert_eq!(r.status, rules::RuleStatus::Verified, "{}", c.rule);
            }
            if c.framework == "EU" {
                assert!(
                    matches!(c.verdict, Verdict::Info | Verdict::NotAssessed),
                    "{c:?}"
                );
            }
            assert!(
                a.body.rules.iter().any(|x| x.id == c.rule),
                "{} listed",
                c.rule
            );
        }
        // The text never claims an authority's acceptance.
        let md = a.to_markdown().to_lowercase();
        for w in ["certified", "approved by", "validated against"] {
            assert!(!md.contains(w), "{f}: '{w}'");
        }
    }
}

// ------------------------------------------------------------------ dossier

fn enterprise() -> Entitlement {
    Entitlement {
        licensee: "(test)".into(),
        tier: Tier::Enterprise,
        expires_unix: 0,
        features: vec![],
        arksa: true,
    }
}

fn opts() -> BuildOptions {
    BuildOptions {
        issued_utc: Some("2026-11-19T00:00:00Z".into()),
        entitlement: enterprise(),
        now_unix: 1_790_000_000,
    }
}

const PROGRAMME: &str = "title = \"Space safety\"\nrequirements = \"requirements.csv\"\n[[scenario]]\nid = \"RUN-MARS\"\npath = \"mars-orbit-pnt.toml\"\n";
const SPACE_SAFETY: &str = "[space_safety]\ndisposal = [\"disposal-leo-smallsat.toml\"]\ndecision = [\"decisions/ev-001.toml\", \"decisions/ev-002.toml\"]\n[space_safety.thresholds]\nevents = \"events.csv\"\nthresholds = [1e-4, 1e-5, 3e-7]\nperiod_days = 180.0\n";

/// A one-run programme (Mars) in a fresh directory with the space-safety fixtures.
fn programme_dir(tag: &str, programme: &str) -> PathBuf {
    let d = tmp(tag);
    std::fs::create_dir_all(d.join("decisions")).unwrap();
    std::fs::copy(
        mission_fixtures().join("mars-orbit-pnt.toml"),
        d.join("mars-orbit-pnt.toml"),
    )
    .unwrap();
    for f in [
        "disposal-leo-smallsat.toml",
        "events.csv",
        "decisions/ev-001.toml",
        "decisions/ev-002.toml",
    ] {
        std::fs::copy(fixtures().join(f), d.join(f)).unwrap();
    }
    std::fs::write(d.join("programme.toml"), programme).unwrap();
    std::fs::write(
        d.join("requirements.csv"),
        "id,text,method,criterion,runs\nR1,x,analysis,fom.median_pdop <= 6,RUN-MARS\n",
    )
    .unwrap();
    d
}

#[test]
fn the_dossier_renders_the_space_safety_section_and_its_check_covers_the_files() {
    let d = programme_dir("dossier", &format!("{PROGRAMME}{SPACE_SAFETY}"));
    let out = d.join("out");
    let b = build(&d.join("programme.toml"), &out, &opts()).unwrap();
    let ss = b
        .dossier
        .body
        .space_safety
        .as_ref()
        .expect("section present");
    assert_eq!(ss.decisions.len(), 2);
    assert_eq!(ss.disposal.len(), 1);
    assert_eq!(
        ss.decision_chain.as_deref(),
        Some(&[][..]),
        "the fixture chain is intact"
    );
    assert_eq!(ss.thresholds.as_ref().unwrap().rows.len(), 3);
    assert!(ss.label.starts_with("MODELLED"));
    assert!(ss.rules.iter().any(|r| r.id == "FCC-25.283-e"));

    // Every section file is under space-safety/, listed in the body and re-hashed.
    for f in [
        "space-safety/ev-001.toml",
        "space-safety/ev-001.decision.json",
        "space-safety/ev-002.decision.json",
        "space-safety/disposal-leo-smallsat.disposal.json",
        "space-safety/events.thresholds.json",
    ] {
        assert!(out.join(f).exists(), "{f}");
        assert!(
            b.dossier
                .body
                .identification
                .files
                .iter()
                .any(|x| x.path == f),
            "{f} listed"
        );
    }
    assert!(b
        .dossier
        .body
        .identification
        .inputs
        .iter()
        .any(|x| x.path == "decisions/ev-002.toml"));
    let ok = check(&out, None).unwrap();
    assert!(ok.passed(), "{:?}", ok.problems);

    // The three faces carry it.
    let json: Value = serde_json::from_str(&read(&out.join("dossier.json"))).unwrap();
    assert_eq!(
        json["body"]["space_safety"]["decisions"][0]["event_id"],
        "EV-001"
    );
    let html = read(&out.join("dossier.html"));
    assert!(html.contains("id=\"space-safety\"") && html.contains("href=\"#space-safety\""));
    assert!(html.contains("47 CFR 25.283(e)") && html.contains("not adopted law"));
    let pdf = String::from_utf8_lossy(&std::fs::read(out.join("dossier.pdf")).unwrap()).to_string();
    assert!(pdf.contains("Space safety: collision-avoidance decisions and disposal compliance"));
    assert!(pdf.contains("EV-001"));

    // A changed section file fails the check and is named.
    let p = out.join("space-safety/ev-001.decision.json");
    let t = read(&p).replace("Flight dynamics lead", "Someone else");
    std::fs::write(&p, t).unwrap();
    let bad = check(&out, None).unwrap();
    assert!(
        bad.problems
            .iter()
            .any(|x| x.starts_with("space-safety/ev-001.decision.json")),
        "{:?}",
        bad.problems
    );
}

#[test]
fn a_programme_without_space_safety_is_unchanged_and_inputs_enter_the_request_hash() {
    let plain = programme_dir("plain", PROGRAMME);
    let a = build(&plain.join("programme.toml"), &plain.join("out"), &opts()).unwrap();
    assert!(a.dossier.body.space_safety.is_none());
    assert!(!read(&plain.join("out/dossier.json")).contains("space_safety"));
    assert!(!plain.join("out/space-safety").exists());

    let with = programme_dir("with", &format!("{PROGRAMME}{SPACE_SAFETY}"));
    let b = build(&with.join("programme.toml"), &with.join("out"), &opts()).unwrap();
    assert_ne!(
        a.dossier.body.identification.request_sha256,
        b.dossier.body.identification.request_sha256
    );
    // Changing a decision input changes the request hash too.
    let p = with.join("decisions/ev-002.toml");
    std::fs::write(&p, read(&p).replace("Radial burn", "Radial  burn")).unwrap();
    let c = arksa_kpro::mission::programme::Programme::load(&with.join("programme.toml")).unwrap();
    assert_ne!(
        c.request_sha256,
        b.dossier.body.identification.request_sha256
    );
}

#[test]
fn bad_space_safety_inputs_are_refused_before_anything_runs() {
    let go = |tag: &str, ss: &str| {
        let d = programme_dir(tag, &format!("{PROGRAMME}{ss}"));
        let e = build(&d.join("programme.toml"), &d.join("out"), &opts())
            .unwrap_err()
            .to_string();
        assert!(!d.join("out").exists(), "{tag}: nothing is written");
        e
    };
    let e = go("missing", "[space_safety]\ndisposal = [\"nope.toml\"]\n");
    assert!(e.contains("nope.toml"), "{e}");
    let e = go("empty", "[space_safety]\n");
    assert!(e.contains("names no file"), "{e}");
    let e = go("dup", "[space_safety]\ndecision = [\"decisions/ev-001.toml\", \"decisions/../decisions/ev-001.toml\"]\n");
    assert!(e.contains("distinct names"), "{e}");
    let e = go(
        "bad-threshold",
        "[space_safety.thresholds]\nevents = \"events.csv\"\nthresholds = [0.0]\n",
    );
    assert!(e.contains("threshold"), "{e}");
}

#[test]
fn the_commands_need_the_mission_entitlement_and_write_what_they_say() {
    let out = tmp("bin-refused");
    let refused = bin()
        .args(["mission", "disposal"])
        .arg(fixtures().join("disposal-leo-smallsat.toml"))
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains(arksa_kpro::license::tiers::NOT_IN_THIS_EDITION));
    assert!(!out.exists());

    let out = tmp("bin-decision");
    let ok = bin()
        .args(["mission", "decision"])
        .arg(fixtures().join("decisions/ev-001.toml"))
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
        stdout.contains("follows the policy") && stdout.contains("not signed"),
        "{stdout}"
    );
    for f in ["decision.json", "decision.md", "input.toml"] {
        assert!(out.join(f).exists(), "{f}");
    }
    let chain = bin()
        .args(["mission", "decision-chain"])
        .arg(&out)
        .arg("--dev")
        .output()
        .unwrap();
    assert!(chain.status.success());

    let th = bin()
        .args(["mission", "thresholds"])
        .arg(fixtures().join("events.csv"))
        .args(["--thresholds", "1e-4,1e-5", "--dev"])
        .output()
        .unwrap();
    assert!(th.status.success());
    assert!(String::from_utf8_lossy(&th.stdout).starts_with("threshold,manoeuvres"));
}
