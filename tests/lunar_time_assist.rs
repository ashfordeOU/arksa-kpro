// PRIVATE — Arksa-exclusive (arksa-kpro).
//
//! AK6: the provisional lunar time-scale registry, the lunar GNSS assistance package, the
//! hosted-generator client, the `[time_scale]` section of the lunar time budget study and the
//! `lunar-time` command.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use arksa_kpro::lunar_time_assist::assist::{
    self, check, pack_from_sp3, score, AssistPackage, ClockDatum, IntegrityBound, PackOptions,
    Requirements, ScoreOptions,
};
use arksa_kpro::lunar_time_assist::registry::{
    change_impact, cross_definition_spread, DefStatus, Family, RefScale, Registry, RegistryStatus,
};
use arksa_kpro::lunar_time_assist::service::{self, AssistRequest, LunarTimeWanted};
use ed25519_dalek::{Signer, SigningKey};
use kshana::rinex::EpochUtc;
use kshana::sp3::{Sp3Epoch, Sp3File, Sp3Header, Sp3SatState};
use kshana_pro::lunar_pro::time::{LunarOption, Model};
use kshana_pro::remote_verdict::{SignedVerdict, VerdictTransport};
use serde_json::json;
use sha2::{Digest, Sha256};

const T0: &str = "2027-01-01T00:00:00";

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "arksa-ak6-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

// ── Registry ─────────────────────────────────────────────────────────────────

#[test]
fn builtin_registry_is_provisional_balanced_and_has_no_default() {
    let r = Registry::builtin();
    assert_eq!(r.status, RegistryStatus::Provisional);
    assert!(r.notice.contains("PROVISIONAL"));
    let fam = |f| r.definitions.iter().filter(|d| d.family == f).count();
    assert!(fam(Family::Tcl) >= 1 && fam(Family::Tl) >= 2 && fam(Family::Ltc) == 1);
    assert!(r
        .definitions
        .iter()
        .all(|d| matches!(d.status, DefStatus::Candidate | DefStatus::Placeholder)));
    let err = r.lunar_offset("tcl", RefScale::Utc, T0).unwrap_err();
    assert!(err.contains("no default"), "{err}");
    // Every output names the registry version and hash it came from.
    let o = r
        .lunar_offset("tcl-iau2024-lte440", RefScale::Utc, T0)
        .unwrap();
    assert_eq!(o.registry.registry_version, 1);
    assert_eq!(o.registry.registry_sha256, r.sha256());
    assert!(o.notes[0].contains("PROVISIONAL"));
}

fn edit(text: &str, f: impl Fn(&mut toml::Value)) -> String {
    let mut v: toml::Value = toml::from_str(text).unwrap();
    f(&mut v);
    toml::to_string(&v).unwrap()
}

fn defs(v: &mut toml::Value) -> &mut Vec<toml::Value> {
    v.get_mut("definitions").unwrap().as_array_mut().unwrap()
}

#[test]
fn registry_rules_refuse_a_single_option_or_an_uncited_adoption() {
    let base = arksa_kpro::lunar_time_assist::registry::BUILTIN_V1_TOML;
    // A provisional registry with TCL alone would read as the standard.
    let only_tcl = edit(base, |v| {
        defs(v).retain(|d| d["family"].as_str() == Some("tcl"));
    });
    let e = Registry::from_toml_str(&only_tcl).unwrap_err();
    assert!(e.contains("both the TCL and the TL"), "{e}");
    // A provisional registry may not mark a definition adopted.
    let adopted_def = edit(base, |v| {
        defs(v)[0]
            .as_table_mut()
            .unwrap()
            .insert("status".into(), "adopted".into());
    });
    assert!(Registry::from_toml_str(&adopted_def)
        .unwrap_err()
        .contains("provisional registry"));
    // An adopted registry must cite its decision.
    let uncited = edit(base, |v| {
        v.as_table_mut()
            .unwrap()
            .insert("status".into(), "adopted".into());
    });
    assert!(Registry::from_toml_str(&uncited)
        .unwrap_err()
        .contains("cite the decision"));
    // TCL is unscaled; TL needs a W0; LTC members must exist.
    let tcl_w0 = edit(base, |v| {
        defs(v)[0]
            .as_table_mut()
            .unwrap()
            .insert("w0".into(), "turyshev-selenoid".into());
    });
    assert!(Registry::from_toml_str(&tcl_w0)
        .unwrap_err()
        .contains("unscaled"));
    let ltc_bad = edit(base, |v| {
        let d = defs(v);
        let last = d.len() - 1;
        d[last].as_table_mut().unwrap().insert(
            "envelope_of".into(),
            toml::Value::Array(vec!["nope".into()]),
        );
    });
    assert!(Registry::from_toml_str(&ltc_bad)
        .unwrap_err()
        .contains("nope"));
}

#[test]
fn conversions_are_kshana_pro_lunar_time_with_registry_provenance() {
    let r = Registry::builtin();
    let jd = kshana_pro::lunar_pro::time::parse_iso(T0).unwrap();
    let direct = Model::default_model().lunar_minus_utc(LunarOption::Tcl, jd);
    let via = r
        .lunar_offset("tcl-iau2024-lte440", RefScale::Utc, T0)
        .unwrap();
    assert!((via.offset_s - direct.value()).abs() < 1e-12);
    assert!((via.u_c_s - direct.u_c()).abs() < 1e-15);
    // TL on the default surface is Kshana Pro's default TL model.
    let direct_tl = Model::default_model().lunar_minus_utc(LunarOption::Tl, jd);
    let via_tl = r
        .lunar_offset("tl-w0-kshana-equatorial-j2", RefScale::Utc, T0)
        .unwrap();
    assert!((via_tl.offset_s - direct_tl.value()).abs() < 1e-12);
}

#[test]
fn rates_cross_check_against_published_values() {
    let r = Registry::builtin();
    let mut ev = r.evaluator();
    let tcl = ev.rate("tcl-iau2024-lte440").unwrap();
    assert!(
        (tcl.us_per_day - 58.7).abs() < 0.05,
        "OSTP 58.7: {}",
        tcl.us_per_day
    );
    let tl = ev.rate("tl-w0-kshana-equatorial-j2").unwrap();
    assert!(
        (tl.us_per_day - 56.02).abs() < 0.01,
        "Ashby and Patla: {}",
        tl.us_per_day
    );
    let sel = ev.rate("tl-w0-turyshev-selenoid").unwrap();
    assert!(
        ((tcl.us_per_day - sel.us_per_day) - 2.712).abs() < 1e-3,
        "Turyshev selenoid: {}",
        tcl.us_per_day - sel.us_per_day
    );
    let fp = ev.rate("tcl-iau2024-first-principles").unwrap();
    assert!(
        (fp.us_per_day - tcl.us_per_day).abs() * 1e3 < 2.0 * fp.u_ns_per_day,
        "first principles within 2u of LTE440"
    );
}

#[test]
fn ltc_is_an_envelope_not_a_choice() {
    let r = Registry::builtin();
    let ltc = r.get("ltc-placeholder").unwrap().clone();
    let mut ev = r.evaluator();
    let vals: Vec<f64> = ltc
        .envelope_of
        .iter()
        .map(|m| ev.lunar_offset(m, RefScale::Utc, T0).unwrap().offset_s)
        .collect();
    let lo = vals.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let o = ev
        .lunar_offset("ltc-placeholder", RefScale::Utc, T0)
        .unwrap();
    assert!((o.offset_s - 0.5 * (lo + hi)).abs() < 1e-12);
    // The undecided definition is an uncertainty, at least the half-spread / sqrt(3).
    assert!(o.u_c_s >= 0.5 * (hi - lo) / 3f64.sqrt() - 1e-15);
    let t = o
        .budget
        .term("LTC definition undecided (half-spread of candidates)")
        .unwrap();
    assert!((t.u - 0.5 * (hi - lo) / 3f64.sqrt()).abs() < 1e-15);
    assert!(o.notes.iter().any(|n| n.contains("not a recommendation")));
    // TCL and TL differ by microseconds today: the choice matters.
    let (rows, spread) = cross_definition_spread(&r, T0).unwrap();
    assert_eq!(rows.len(), r.definitions.len());
    assert!(spread > 1e-6, "spread {spread}");
}

/// A version 2 as it might look after a decision: adopted, one TL definition pinned to a
/// conventional W0_L, the point-mass candidate withdrawn from the registry.
fn v2_text() -> String {
    edit(
        arksa_kpro::lunar_time_assist::registry::BUILTIN_V1_TOML,
        |v| {
            let t = v.as_table_mut().unwrap();
            t.insert("version".into(), 2.into());
            t.insert("status".into(), "adopted".into());
            t.insert(
                "decision".into(),
                "TEST ONLY: a hypothetical resolution for the impact test".into(),
            );
            t.insert(
                "notice".into(),
                "TEST ONLY: hypothetical adopted registry".into(),
            );
            let d = defs(v);
            for x in d.iter_mut() {
                if x["id"].as_str() == Some("tl-w0-turyshev-selenoid") {
                    let tt = x.as_table_mut().unwrap();
                    tt.insert("w0".into(), "conventional:2824000".into());
                    tt.insert("status".into(), "adopted".into());
                }
                if x["id"].as_str() == Some("ltc-placeholder") {
                    x.as_table_mut().unwrap().insert(
                        "envelope_of".into(),
                        toml::Value::Array(vec![
                            "tcl-iau2024-lte440".into(),
                            "tl-w0-turyshev-selenoid".into(),
                        ]),
                    );
                }
            }
            d.retain(|x| x["id"].as_str() != Some("tl-w0-point-mass"));
        },
    )
}

#[test]
fn change_impact_flags_exactly_the_definitions_that_moved() {
    let v1 = Registry::builtin();
    let v2 = Registry::from_toml_str(&v2_text()).unwrap();
    assert_ne!(v1.sha256(), v2.sha256());
    let epochs = vec![T0.to_string(), "2028-06-01T12:00:00".to_string()];
    let imp = change_impact(&v1, &v2, &epochs, 1e-9).unwrap();
    assert_eq!(
        imp.status_change,
        Some((RegistryStatus::Provisional, RegistryStatus::Adopted))
    );
    let by = |id: &str| imp.definitions.iter().find(|d| d.id == id).unwrap();
    let sel = by("tl-w0-turyshev-selenoid");
    assert_eq!(sel.change, "parameters-changed");
    assert!(sel.requalify && sel.max_abs_delta_s > 0.0);
    assert_eq!(by("tl-w0-point-mass").change, "removed");
    let tcl = by("tcl-iau2024-lte440");
    assert_eq!(tcl.change, "unchanged");
    assert_eq!(tcl.max_abs_delta_s, 0.0);
    assert!(!tcl.requalify);
    assert!(by("ltc-placeholder").requalify);
    assert!(!imp.requalify.contains(&"tcl-iau2024-lte440".to_string()));
    // Versions must move forward within one registry.
    assert!(change_impact(&v2, &v1, &epochs, 1e-9).is_err());
}

// ── Assistance package ───────────────────────────────────────────────────────

const STEP: i64 = 900;
const N_EPOCHS: usize = 121; // 30 h
const OMEGA_E: f64 = 7.292_115_146_7e-5;

fn sat_ids() -> Vec<String> {
    let mut v: Vec<String> = (1..=32).map(|i| format!("G{i:02}")).collect();
    v.extend((1..=30).map(|i| format!("E{i:02}")));
    v
}

/// A circular-orbit ECEF position (m) of satellite `k` at `t` s, plus a clock (s).
fn truth_state(k: usize, t: f64) -> ([f64; 3], f64) {
    let (a, inc, planes, n_sats) = if k < 32 {
        (26_560e3_f64, 55f64.to_radians(), 6, 32)
    } else {
        (29_600e3, 56f64.to_radians(), 3, 30)
    };
    let j = if k < 32 { k } else { k - 32 };
    let raan = (j % planes) as f64 * std::f64::consts::TAU / planes as f64;
    let n = (3.986_004_418e14 / (a * a * a)).sqrt();
    let u = j as f64 * std::f64::consts::TAU / n_sats as f64 + n * t;
    let x = a * (u.cos() * raan.cos() - u.sin() * inc.cos() * raan.sin());
    let y = a * (u.cos() * raan.sin() + u.sin() * inc.cos() * raan.cos());
    let z = a * u.sin() * inc.sin();
    let th = OMEGA_E * t;
    let p = [x * th.cos() + y * th.sin(), -x * th.sin() + y * th.cos(), z];
    let clk = 1e-4 * ((k as f64) / 62.0 - 0.5) + 1e-11 * t;
    (p, clk)
}

fn start() -> EpochUtc {
    EpochUtc {
        year: 2027,
        month: 3,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0.0,
    }
}

/// An SP3 product; `err(age_h)` returns (along-track m, clock s) errors added after validity
/// start (age measured from `vstart_s`).
fn product(vstart_s: f64, err: impl Fn(f64) -> (f64, f64)) -> Sp3File {
    let ids = sat_ids();
    let epochs = (0..N_EPOCHS)
        .map(|i| {
            let t = (i as i64 * STEP) as f64;
            let age = ((t - vstart_s) / 3600.0).max(0.0);
            let (da, dc) = err(age);
            Sp3Epoch {
                time: assist::epoch_add_s(&start(), i as i64 * STEP),
                sats: ids
                    .iter()
                    .enumerate()
                    .map(|(k, id)| {
                        let (p, c) = truth_state(k, t);
                        let (p2, _) = truth_state(k, t + 1.0);
                        let v = [p2[0] - p[0], p2[1] - p[1], p2[2] - p[2]];
                        let vn = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                        Sp3SatState {
                            sat: id.clone(),
                            pos_m: [
                                p[0] + da * v[0] / vn,
                                p[1] + da * v[1] / vn,
                                p[2] + da * v[2] / vn,
                            ],
                            clock_us: (c + dc) * 1e6,
                            vel_m_s: None,
                        }
                    })
                    .collect(),
            }
        })
        .collect();
    let sp3 = Sp3File {
        header: Sp3Header {
            version: 'd',
            has_velocity: false,
            start: start(),
            num_epochs: N_EPOCHS,
            sat_ids: ids,
        },
        epochs,
    };
    // Go through the SP3 text format, as a real file would.
    kshana::sp3::parse_sp3(&sp3.to_sp3_string()).unwrap()
}

fn pack_options(bound: IntegrityBound) -> PackOptions {
    PackOptions {
        package_id: "TEST-PKG-0001".into(),
        producer: "test producer (synthetic)".into(),
        prediction_basis: "synthetic circular orbits with injected along-track and clock error"
            .into(),
        issued: "2027-03-01T00:00:00".into(),
        frame: "synthetic ECEF".into(),
        validity_start: "2027-03-01T01:00:00".into(),
        validity_hours: 24.0,
        satellites: vec![],
        bound,
        bound_overrides: BTreeMap::new(),
        lunar_time_definition: Some("tcl-iau2024-lte440".into()),
    }
}

fn honest_bound() -> IntegrityBound {
    IntegrityBound {
        sigma0_m: 1.0,
        growth_m_per_h: 0.5,
        growth2_m_per_h2: 0.0,
        integrity_risk: 0.01,
    }
}

fn predicted() -> Sp3File {
    product(3600.0, |age| (0.2 + 0.05 * age, (1.0 + 0.5 * age) * 1e-9))
}

const SEED: [u8; 32] = [7u8; 32];

fn signed_package() -> AssistPackage {
    let reg = Registry::builtin();
    let mut p = pack_from_sp3(&predicted(), &pack_options(honest_bound()), Some(&reg))
        .unwrap()
        .package;
    p.sign(&SEED, "test signer").unwrap();
    p
}

fn pubkey() -> String {
    hex::encode(SigningKey::from_bytes(&SEED).verifying_key().to_bytes())
}

#[test]
fn pack_meets_the_published_lnss_shape_and_carries_the_time_reference() {
    let p = signed_package();
    assert_eq!(p.satellites.len(), 62);
    assert_eq!(p.constellations(), vec!['E', 'G']);
    assert_eq!(p.validity_hours(), 24.0);
    assert_eq!(p.step_s, 900);
    let items = check(&p, &Requirements::default(), &[pubkey()]);
    assert!(items.iter().all(|i| i.pass), "{items:#?}");
    // The time reference is the registry's GPST offset at the start of validity.
    let reg = Registry::builtin();
    let lt = p.lunar_time.as_ref().unwrap();
    let o = reg
        .lunar_offset("tcl-iau2024-lte440", RefScale::Gpst, "2027-03-01T01:00:00")
        .unwrap();
    assert_eq!(
        lt.lunar_minus_gpst_at_start_ps,
        (o.offset_s * 1e12).round() as i64
    );
    assert_eq!(lt.registry.registry_status, RegistryStatus::Provisional);
    assert!(lt.notice.contains("PROVISIONAL"));
    // JSON round trip keeps the signature valid.
    let text = serde_json::to_string(&p).unwrap();
    let back: AssistPackage = serde_json::from_str(&text).unwrap();
    assert!(back.verify_signature(&[pubkey()]).is_ok());
}

#[test]
fn requirements_and_signature_failures_are_reported() {
    let p = signed_package();
    let mut few = p.clone();
    few.satellites.truncate(59);
    let items = check(&few, &Requirements::default(), &[]);
    let sat = items.iter().find(|i| i.item == "satellite count").unwrap();
    assert!(!sat.pass);
    // Truncating after signing also breaks the signature.
    assert!(!items.iter().find(|i| i.item == "signature").unwrap().pass);
    let mut tampered = p.clone();
    tampered.satellites[3].pos_mm[50][0] += 1;
    assert!(tampered.verify_signature(&[]).is_err());
    assert!(p
        .verify_signature(&["00".repeat(32)])
        .unwrap_err()
        .contains("not among the trusted keys"));
    let gps_only = Requirements {
        constellations: vec!["G".into(), "E".into(), "C".into()],
        ..Requirements::default()
    };
    assert!(
        !check(&p, &gps_only, &[])
            .iter()
            .find(|i| i.item == "constellations")
            .unwrap()
            .pass
    );
    let tight = Requirements {
        max_protection_at_end_m: Some(5.0),
        ..Requirements::default()
    };
    assert!(
        !check(&p, &tight, &[])
            .iter()
            .find(|i| i.item == "protection at end of validity")
            .unwrap()
            .pass
    );
}

#[test]
fn packing_never_extrapolates() {
    let mut o = pack_options(honest_bound());
    o.validity_hours = 30.0; // 01:00 + 30 h is past the 30 h of data
    let e = pack_from_sp3(&predicted(), &o, Some(&Registry::builtin())).unwrap_err();
    assert!(e.contains("no extrapolation"), "{e}");
    let mut o = pack_options(honest_bound());
    o.lunar_time_definition = Some("tl".into());
    assert!(pack_from_sp3(&predicted(), &o, Some(&Registry::builtin()))
        .unwrap_err()
        .contains("no default"));
}

fn score_opts() -> ScoreOptions {
    ScoreOptions {
        reference_name: "synthetic truth".into(),
        every_nth_epoch: 1,
        decorrelation_s: 3600.0,
        clock_datum: ClockDatum::Absolute,
        age_bin_h: 2.0,
        confidence: 0.95,
    }
}

#[test]
fn a_package_of_the_truth_scores_millimetres() {
    let truth = product(0.0, |_| (0.0, 0.0));
    let p = pack_from_sp3(
        &truth,
        &pack_options(honest_bound()),
        Some(&Registry::builtin()),
    )
    .unwrap()
    .package;
    let s = score(&p, &truth, "x", &score_opts()).unwrap();
    // Grid points are exact; the error is the SP3/package quantisation (1 mm, 1 ps).
    assert!(s.max_error_m < 0.01, "max {}", s.max_error_m);
    assert_eq!(s.exceedances_all_samples, 0);
}

#[test]
fn an_honest_bound_is_demonstrated_and_an_undersized_one_is_violated() {
    let truth = product(0.0, |_| (0.0, 0.0));
    let p = signed_package();
    let s = score(&p, &truth, "x", &score_opts()).unwrap();
    assert_eq!(
        s.samples,
        62 * 97,
        "97 epochs inside 24 h validity, 62 satellites"
    );
    assert_eq!(s.exceedances_all_samples, 0);
    assert_eq!(s.integrity.independent_samples, 62 * 25);
    assert_eq!(s.integrity.verdict, "BOUND-HELD (demonstrated)");
    // The error grows with age, as injected.
    let first = s.age_curve.first().unwrap();
    let last = s.age_curve.last().unwrap();
    assert!(last.rms_error_m > first.rms_error_m * 3.0);
    // Expected error at age a: 0.2 + 0.05a (along-track) + c(1 + 0.5a) ns.
    let want = 0.2 + 0.05 * 24.0 + assist::C_M_S * (1.0 + 0.5 * 24.0) * 1e-9;
    assert!(
        (s.max_error_m - want).abs() < 0.01,
        "{} vs {want}",
        s.max_error_m
    );

    let small = IntegrityBound {
        sigma0_m: 0.1,
        growth_m_per_h: 0.01,
        growth2_m_per_h2: 0.0,
        integrity_risk: 0.01,
    };
    let q = pack_from_sp3(
        &predicted(),
        &pack_options(small),
        Some(&Registry::builtin()),
    )
    .unwrap()
    .package;
    let s = score(&q, &truth, "x", &score_opts()).unwrap();
    assert_eq!(s.integrity.verdict, "BOUND-VIOLATED");
    assert!(s.age_curve.iter().all(|b| b.rms_over_sigma > 1.0));

    // A tiny allocation cannot be demonstrated by a day of data, and the scorecard says so.
    let rare = IntegrityBound {
        integrity_risk: 1e-7,
        ..honest_bound()
    };
    let r = pack_from_sp3(
        &predicted(),
        &pack_options(rare),
        Some(&Registry::builtin()),
    )
    .unwrap()
    .package;
    let s = score(&r, &truth, "x", &score_opts()).unwrap();
    assert_eq!(s.integrity.verdict, "NOT-DEMONSTRATED");
    assert!(s.integrity.samples_needed_with_zero_exceedances > 1_000_000);
}

#[test]
fn removing_the_epoch_mean_clock_hides_a_common_clock_error() {
    let truth = product(0.0, |_| (0.0, 0.0));
    let common = product(3600.0, |_| (0.0, 50e-9)); // 15 m common clock offset
    let p = pack_from_sp3(
        &common,
        &pack_options(honest_bound()),
        Some(&Registry::builtin()),
    )
    .unwrap()
    .package;
    let abs = score(&p, &truth, "x", &score_opts()).unwrap();
    let rel = score(
        &p,
        &truth,
        "x",
        &ScoreOptions {
            clock_datum: ClockDatum::RemoveEpochMean,
            ..score_opts()
        },
    )
    .unwrap();
    assert!(abs.max_error_m > 14.0);
    assert!(rel.max_error_m < 0.01);
    assert!(rel.caveats.iter().any(|c| c.contains("common clock error")));
}

#[test]
fn the_score_directory_is_signed_and_verifies() {
    let truth = product(0.0, |_| (0.0, 0.0));
    let s = score(&signed_package(), &truth, "x", &score_opts()).unwrap();
    let dir = tmp("score");
    let p = assist::write_score(&s, &dir, Some((&SEED, "test", "2027-03-02T00:00:00Z")))
        .unwrap()
        .unwrap();
    assert!(p.exists());
    let pk = kshana_pro::license::public_key_for(&SEED);
    let v = kshana_pro::signed_dossier::verify(&dir, &[pk]).unwrap();
    assert!(v.passed(), "{:?}", v.problems);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("score.json")).unwrap()).unwrap();
    assert_eq!(doc["evidence_class"], "MODELLED");
    assert!(std::fs::read_to_string(dir.join("report.md"))
        .unwrap()
        .contains("not a standard"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ── Hosted generator client ──────────────────────────────────────────────────

const SERVICE_SEED: [u8; 32] = [9u8; 32];

struct StandIn {
    package: serde_json::Value,
    method: &'static str,
    seed: [u8; 32],
    tamper: bool,
}

impl VerdictTransport for StandIn {
    fn post(&self, path: &str, bearer: &str, body: &str) -> Result<String, String> {
        assert_eq!(path, "/verdict/lunar-assist");
        assert!(!bearer.is_empty());
        let _: AssistRequest = serde_json::from_str(body).unwrap();
        let mut v = SignedVerdict {
            service: "stand-in".into(),
            method: self.method.into(),
            evidence_class: "MODELLED".into(),
            utc: "2027-03-01T00:00:00Z".into(),
            key_id: "test".into(),
            verdict: json!({ "package": self.package }),
            verdict_sha256: String::new(),
            signature: String::new(),
        };
        // The same canonical body Kshana Pro's client verifies.
        let canonical = serde_json::to_vec(&json!({
            "service": v.service,
            "method": v.method,
            "evidence_class": v.evidence_class,
            "utc": v.utc,
            "key_id": v.key_id,
            "verdict": v.verdict,
        }))
        .unwrap();
        v.verdict_sha256 = hex::encode(Sha256::digest(&canonical));
        v.signature = hex::encode(
            SigningKey::from_bytes(&self.seed)
                .sign(&canonical)
                .to_bytes(),
        );
        if self.tamper {
            v.verdict["package"]["producer"] = json!("someone else");
        }
        Ok(serde_json::to_string(&v).unwrap())
    }
}

fn request() -> AssistRequest {
    AssistRequest {
        satellites: vec!["G01".into(), "E05".into()],
        validity_start: "2027-03-01T02:00:00".into(),
        validity_hours: 20.0,
        step_s: 900,
        integrity_risk: 0.01,
        lunar_time: Some(LunarTimeWanted {
            registry_id: "ks-lunar-time-definitions".into(),
            registry_version: 1,
            definition_id: "tcl-iau2024-lte440".into(),
        }),
    }
}

#[test]
fn the_client_accepts_only_a_signed_answer_that_answers_the_request() {
    let svc_pk = hex::encode(
        SigningKey::from_bytes(&SERVICE_SEED)
            .verifying_key()
            .to_bytes(),
    );
    let pkg = serde_json::to_value(signed_package()).unwrap();
    let ok = StandIn {
        package: pkg.clone(),
        method: "lunar-assist",
        seed: SERVICE_SEED,
        tamper: false,
    };
    let (v, p) = service::request_package(&ok, "token", &request(), &svc_pk).unwrap();
    assert_eq!(v.method, "lunar-assist");
    assert_eq!(p.satellites.len(), 62);

    let wrong_key = StandIn {
        seed: [1u8; 32],
        ..StandIn {
            package: pkg.clone(),
            method: "lunar-assist",
            seed: SERVICE_SEED,
            tamper: false,
        }
    };
    assert!(
        service::request_package(&wrong_key, "token", &request(), &svc_pk)
            .unwrap_err()
            .contains("verdict refused")
    );
    let tampered = StandIn {
        package: pkg.clone(),
        method: "lunar-assist",
        seed: SERVICE_SEED,
        tamper: true,
    };
    assert!(service::request_package(&tampered, "token", &request(), &svc_pk).is_err());
    let other = StandIn {
        package: pkg.clone(),
        method: "selfcal",
        seed: SERVICE_SEED,
        tamper: false,
    };
    assert!(
        service::request_package(&other, "token", &request(), &svc_pk)
            .unwrap_err()
            .contains("not 'lunar-assist'")
    );
    // The answer must cover the request.
    let mut r = request();
    r.satellites.push("C20".into());
    r.integrity_risk = 0.001;
    r.validity_hours = 30.0;
    let e = service::request_package(&ok, "token", &r, &svc_pk).unwrap_err();
    assert!(e.contains("C20") && e.contains("integrity risk") && e.contains("does not cover"));
    let mut r = request();
    r.lunar_time.as_mut().unwrap().definition_id = "tl-w0-turyshev-selenoid".into();
    assert!(service::request_package(&ok, "token", &r, &svc_pk)
        .unwrap_err()
        .contains("lunar time reference"));
    assert!(service::request_package(&ok, "", &request(), &svc_pk).is_err());
}

// ── The study pack's [time_scale] section ────────────────────────────────────

#[test]
fn the_time_budget_study_reports_candidates_side_by_side_only_when_asked() {
    use arksa_kpro::packs::lunar_time_budget_study_pack::LunarTimeBudgetStudyScenario as S;
    use kshana::api::Scenario;
    let plain: S = toml::from_str("").unwrap();
    let doc: serde_json::Value = serde_json::from_str(&plain.run().unwrap().json).unwrap();
    assert!(doc.get("time_scale").is_none());
    let with: S = toml::from_str(
        "[time_scale]\nepochs_utc = [\"2027-01-01T00:00:00\", \"2030-01-01T00:00:00\"]\n",
    )
    .unwrap();
    let out = with.run().unwrap();
    let doc2: serde_json::Value = serde_json::from_str(&out.json).unwrap();
    let ts = &doc2["time_scale"];
    assert_eq!(ts["no_default_definition"], true);
    assert_eq!(ts["registry"]["registry_status"], "provisional");
    assert_eq!(
        ts["definitions"].as_array().unwrap().len(),
        Registry::builtin().definitions.len()
    );
    assert!(
        ts["spread"][1]["candidate_spread_s"].as_f64().unwrap()
            > ts["spread"][0]["candidate_spread_s"].as_f64().unwrap()
    );
    assert!(out.summary.contains("PROVISIONAL"));
    assert_ne!(doc["scenario_hash"], doc2["scenario_hash"]);
    // Everything else is the plain study.
    assert_eq!(doc["budget"], doc2["budget"]);
}

// ── The command ──────────────────────────────────────────────────────────────

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_arksa-kpro"))
        .args(args)
        .env_remove("ARKSA_STUDY_SIGNING_KEY")
        .output()
        .unwrap()
}

#[test]
fn the_command_is_gated_and_names_its_definition() {
    let no = run(&["lunar-time", "registry"]);
    assert!(!no.status.success());
    assert!(String::from_utf8_lossy(&no.stderr)
        .contains(arksa_kpro::license::tiers::NOT_IN_THIS_EDITION));
    let yes = run(&["lunar-time", "registry", "--dev"]);
    assert!(
        yes.status.success(),
        "{}",
        String::from_utf8_lossy(&yes.stderr)
    );
    assert!(String::from_utf8_lossy(&yes.stdout).contains("PROVISIONAL"));
    let nodef = run(&["lunar-time", "convert", "--at", T0, "--dev"]);
    assert!(!nodef.status.success());
    assert!(String::from_utf8_lossy(&nodef.stderr).contains("no default"));
    let conv = run(&[
        "lunar-time",
        "convert",
        "--definition",
        "ltc-placeholder",
        "--at",
        T0,
        "--dev",
    ]);
    assert!(conv.status.success());
    let v: serde_json::Value = serde_json::from_slice(&conv.stdout).unwrap();
    assert_eq!(v["family"], "ltc");
}

#[test]
fn the_command_packs_checks_scores_and_measures_impact() {
    let dir = tmp("cli");
    let sp3 = dir.join("pred.sp3");
    std::fs::write(&sp3, predicted().to_sp3_string()).unwrap();
    let truth = dir.join("truth.sp3");
    std::fs::write(&truth, product(0.0, |_| (0.0, 0.0)).to_sp3_string()).unwrap();
    let opts = dir.join("pack.toml");
    std::fs::write(
        &opts,
        r#"package_id = "CLI-1"
producer = "test"
prediction_basis = "synthetic"
issued = "2027-03-01T00:00:00"
validity_start = "2027-03-01T01:00:00"
validity_hours = 24.0
lunar_time_definition = "tl-w0-turyshev-south-pole"
[bound]
sigma0_m = 1.0
growth_m_per_h = 0.5
integrity_risk = 0.01
"#,
    )
    .unwrap();
    let key = dir.join("key.hex");
    std::fs::write(&key, hex::encode(SEED)).unwrap();
    let pkg = dir.join("pkg.json");
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();
    let out = run(&[
        "lunar-time",
        "assist",
        "pack",
        "--sp3",
        &s(&sp3),
        "--options",
        &s(&opts),
        "--out",
        &s(&pkg),
        "--sign-key-file",
        &s(&key),
        "--dev",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let chk = run(&[
        "lunar-time",
        "assist",
        "check",
        &s(&pkg),
        "--trusted-key",
        &pubkey(),
        "--dev",
    ]);
    assert!(
        chk.status.success(),
        "{}",
        String::from_utf8_lossy(&chk.stdout)
    );
    let bad = run(&[
        "lunar-time",
        "assist",
        "check",
        &s(&pkg),
        "--trusted-key",
        &"ab".repeat(32),
        "--dev",
    ]);
    assert_eq!(bad.status.code(), Some(1));
    let sdir = dir.join("score");
    let sc = run(&[
        "lunar-time",
        "assist",
        "score",
        &s(&pkg),
        "--reference",
        &s(&truth),
        "--out",
        &s(&sdir),
        "--sign-key-file",
        &s(&key),
        "--dev",
    ]);
    assert!(
        sc.status.success(),
        "{}",
        String::from_utf8_lossy(&sc.stderr)
    );
    assert!(String::from_utf8_lossy(&sc.stdout).contains("BOUND-HELD"));
    assert!(sdir.join("score.json").exists());
    let v2 = dir.join("v2.toml");
    std::fs::write(&v2, v2_text()).unwrap();
    let idir = dir.join("impact");
    let imp = run(&[
        "lunar-time",
        "impact",
        "--new",
        &s(&v2),
        "--at",
        T0,
        "--out",
        &s(&idir),
        "--sign-key-file",
        &s(&key),
        "--dev",
    ]);
    assert!(
        imp.status.success(),
        "{}",
        String::from_utf8_lossy(&imp.stderr)
    );
    assert!(String::from_utf8_lossy(&imp.stdout).contains("tl-w0-turyshev-selenoid"));
    let pk = kshana_pro::license::public_key_for(&SEED);
    assert!(kshana_pro::signed_dossier::verify(&idir, &[pk])
        .unwrap()
        .passed());
    let _ = std::fs::remove_dir_all(&dir);
}
