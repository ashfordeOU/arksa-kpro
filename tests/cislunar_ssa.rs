// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Cislunar space situational awareness (AK9): orbit-family taxonomy, angles-only IOD and
//! refinement, catalogue build-up, CCSDS TDM ingest, debris-environment risk, the signed
//! study directory and the Arksa gate, all on synthetic data (MODELLED; the synthetic scores
//! are internal consistency against the generating truth).

#![allow(clippy::needless_range_loop)] // index loops read as the matrix algebra they implement

use std::sync::OnceLock;

use arksa_kpro::cislunar_ssa::catalogue::{chi2_quantile, Catalogue};
use arksa_kpro::cislunar_ssa::debris::{assess, DebrisConfig};
use arksa_kpro::cislunar_ssa::synthetic::{generate, preset_state};
use arksa_kpro::cislunar_ssa::taxonomy::{classify, TaxonomyConfig};
use arksa_kpro::cislunar_ssa::{
    iod, run_study, write_study, Observer, Problem, SsaConfig, Study, L_KM, MU,
};
use arksa_kpro::conjunction::parse_utc;
use serde_json::Value;

const STATIONS: &str = r#"
[[observers]]
name = "TORAVERE"
kind = "station"
lat_deg = 58.2649
lon_deg = 26.4661
alt_m = 70.0
[[observers]]
name = "TEIDE"
kind = "station"
lat_deg = 28.3009
lon_deg = -16.5097
alt_m = 2390.0
"#;

/// Two objects tracked by two stations on two days, and a third seen in one track only.
const CAMPAIGN: &str = r#"
[synthetic]
epoch_utc = "2026-12-01T00:00:00"
sigma_arcsec = 1.0
seed = 7
objects = [
  { id = "NRHO-A", preset = "nrho", phase_frac = 0.0 },
  { id = "DRO-B", preset = "dro", phase_frac = 0.3 },
  { id = "HALO-C", preset = "halo-l1", phase_frac = 0.5 },
]
tracks = [
  { observer = "TORAVERE", start_utc = "2026-12-01T00:00:00", hours = 24.0, cadence_min = 15.0, objects = ["NRHO-A", "DRO-B"] },
  { observer = "TEIDE", start_utc = "2026-12-01T00:00:00", hours = 24.0, cadence_min = 15.0, objects = ["NRHO-A", "DRO-B"] },
  { observer = "TORAVERE", start_utc = "2026-12-02T00:00:00", hours = 24.0, cadence_min = 15.0, objects = ["NRHO-A", "DRO-B"] },
  { observer = "TEIDE", start_utc = "2026-12-02T00:00:00", hours = 24.0, cadence_min = 15.0 },
]
"#;

fn campaign() -> &'static (SsaConfig, Study) {
    static S: OnceLock<(SsaConfig, Study)> = OnceLock::new();
    S.get_or_init(|| {
        let cfg: SsaConfig = toml::from_str(&format!("{STATIONS}{CAMPAIGN}")).unwrap();
        let st = run_study(&cfg).unwrap();
        (cfg, st)
    })
}

fn observers(cfg: &SsaConfig) -> Vec<(String, Observer)> {
    cfg.observers
        .iter()
        .map(|c| (c.name.clone(), Observer::from_config(c).unwrap()))
        .collect()
}

// ── Taxonomy ──────────────────────────────────────────────────────────────────────────

#[test]
fn taxonomy_labels_the_corrected_family_members() {
    let cfg = TaxonomyConfig::default();
    for (preset, family, label_part) in [
        ("nrho", "nrho", "L2"),
        ("halo-l2", "halo", "L2"),
        ("halo-l1", "halo", "L1"),
        ("lyapunov-l1", "lyapunov-l1", "L1"),
        ("dro", "dro", "retrograde"),
        ("elfo", "elfo", "ELFO"),
        ("llo", "llo", "low lunar"),
    ] {
        let s = preset_state(preset, 0.25).unwrap();
        let c = classify(&s, &cfg);
        assert_eq!(c.family, family, "{preset}: {} ({:?})", c.rule, c.features);
        assert!(c.label.contains(label_part), "{preset}: {}", c.label);
        assert_eq!(c.to_json()["evidence_class"], "MODELLED");
    }
}

#[test]
fn taxonomy_labels_earth_orbits_escapes_and_impacts() {
    let cfg = TaxonomyConfig::default();
    let v_unit = L_KM / (27.321_661 * 86_400.0 / std::f64::consts::TAU);
    // A circular geocentric orbit of 150,000 km radius, on the far side from the Moon.
    let r = 150_000.0 / L_KM;
    let v_in = (1.0 - MU) / r; // v^2 = GM/r with GM_earth = 1 - mu (nondimensional)
    let v_in = v_in.sqrt();
    let earth = [-MU - r, 0.0, 0.0, 0.0, -v_in + r, 0.0];
    assert_eq!(classify(&earth, &cfg).family, "earth-orbit");
    // 3 km/s outbound from the far side: leaves the Earth-Moon system.
    let esc = [-MU - r, 0.0, 0.0, -3.0 / v_unit, -2.0 / v_unit + r, 0.0];
    assert_eq!(classify(&esc, &cfg).family, "escape");
    // Falling straight at the Moon from 20,000 km.
    let imp = [1.0 - MU - 20_000.0 / L_KM, 0.0, 0.0, 1.5 / v_unit, 0.0, 0.0];
    assert_eq!(classify(&imp, &cfg).family, "lunar-impact");
}

// ── Measurement model and IOD ─────────────────────────────────────────────────────────

fn single_object_problem(seed: u64) -> (SsaConfig, Problem, [f64; 6]) {
    let text = format!(
        "{STATIONS}
[synthetic]
epoch_utc = \"2026-12-01T00:00:00\"
seed = {seed}
objects = [ {{ id = \"X\", preset = \"dro\", phase_frac = 0.1 }} ]
tracks = [ {{ observer = \"TEIDE\", start_utc = \"2026-12-01T00:00:00\", hours = 24.0, cadence_min = 20.0 }} ]
"
    );
    let cfg: SsaConfig = toml::from_str(&text).unwrap();
    let obs = observers(&cfg);
    let sc = cfg.synthetic.as_ref().unwrap();
    let ep = parse_utc(&sc.epoch_utc).unwrap();
    let (o, truth) = generate(sc, ep, &obs).unwrap();
    let p = Problem::new(ep, o, &obs).unwrap();
    (cfg, p, truth[0].state)
}

#[test]
fn analytic_jacobian_matches_finite_differences() {
    let (_, p, truth) = single_object_problem(3);
    let idx: Vec<usize> = (0..p.obs.len()).collect();
    assert!(idx.len() >= 10, "the track must see the object");
    let tau = p.geo[idx[idx.len() / 2]].tau; // reference mid-arc: both directions
    let x = arksa_kpro::cislunar_ssa::propagate(&truth, tau).unwrap();
    let (_, ha) = p.residuals_jac(&idx, tau, &x).unwrap();
    let hf = iod::fd_jacobian(&p, &idx, tau, &x).unwrap();
    let mut worst: f64 = 0.0;
    for (a, f) in ha.iter().zip(&hf) {
        for k in 0..6 {
            let scale = f[k].abs().max(1.0);
            worst = worst.max((a[k] - f[k]).abs() / scale);
        }
    }
    assert!(
        worst < 1e-4,
        "analytic vs finite-difference Jacobian: {worst:e}"
    );
}

#[test]
fn iod_from_one_station_tracklet_recovers_the_truth_consistently() {
    let (cfg, p, truth) = single_object_problem(11);
    let idx: Vec<usize> = (0..p.obs.len()).collect();
    let r = iod::iod(&p, &idx, &cfg.iod).unwrap();
    assert!(!r.ambiguous);
    assert!(r.fit.converged);
    let rc = r.fit.reduced_chi2();
    assert!((0.5..2.0).contains(&rc), "reduced chi2 {rc}");
    let t = arksa_kpro::cislunar_ssa::propagate(&truth, r.fit.tau_ref).unwrap();
    let d: Vec<f64> = (0..6).map(|k| r.fit.x[k] - t[k]).collect();
    let cov = r.fit.cov.expect("covariance");
    let a: Vec<Vec<f64>> = cov.iter().map(|row| row.to_vec()).collect();
    // NEES via a solve of P y = d.
    let y = solve_dense(&a, &d);
    let nees: f64 = d.iter().zip(&y).map(|(u, v)| u * v).sum();
    assert!(nees < chi2_quantile(0.999, 6), "NEES {nees}");
    // One station, one day: the range is the weak direction; the error stays a small
    // fraction of the distance and inside the formal covariance (NEES above).
    let pos_err_km = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() * L_KM;
    assert!(
        pos_err_km < 0.01 * 384_400.0,
        "position error {pos_err_km} km"
    );
}

fn solve_dense(a: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
    let n = b.len();
    let mut m: Vec<Vec<f64>> = a.to_vec();
    let mut x = b.to_vec();
    for k in 0..n {
        let p = (k..n)
            .max_by(|&i, &j| m[i][k].abs().total_cmp(&m[j][k].abs()))
            .unwrap();
        m.swap(k, p);
        x.swap(k, p);
        for i in k + 1..n {
            let f = m[i][k] / m[k][k];
            for j in k..n {
                m[i][j] -= f * m[k][j];
            }
            x[i] -= f * x[k];
        }
    }
    for k in (0..n).rev() {
        let s: f64 = (k + 1..n).map(|j| m[k][j] * x[j]).sum();
        x[k] = (x[k] - s) / m[k][k];
    }
    x
}

// ── Catalogue build-up ────────────────────────────────────────────────────────────────

#[test]
fn catalogue_buildup_associates_every_tracklet_to_the_right_object() {
    let (_, st) = campaign();
    let d = &st.doc;
    assert_eq!(d["schema"], "arksa-cislunar-ssa/1");
    assert_eq!(d["evidence_class"], "MODELLED");
    let sc = &d["synthetic_scoring"];
    assert_eq!(sc["association_purity"].as_f64(), Some(1.0), "{sc:#}");
    assert_eq!(sc["duplicate_objects"].as_u64(), Some(0), "{sc:#}");
    // NRHO-A and DRO-B are catalogued; HALO-C (one track) is catalogued or reported
    // uncorrelated, never merged into another object.
    let caught: Vec<&str> = sc["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["truth"].as_str().unwrap())
        .collect();
    assert!(
        caught.contains(&"NRHO-A") && caught.contains(&"DRO-B"),
        "{caught:?}"
    );
    let gate = chi2_quantile(0.999, 6);
    for o in sc["objects"].as_array().unwrap() {
        let nees = o["nees6"].as_f64().unwrap();
        assert!(nees < gate, "{o}: covariance inconsistent with the error");
    }
    for o in d["catalogue"].as_array().unwrap() {
        let rc = o["reduced_chi2"].as_f64().unwrap();
        assert!(rc < 2.0, "{o}");
        let truth = sc["objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["object"] == o["id"])
            .unwrap()["truth"]
            .as_str()
            .unwrap();
        let want = match truth {
            "NRHO-A" => "nrho",
            "DRO-B" => "dro",
            _ => "halo",
        };
        if truth == "HALO-C" {
            // One tracklet only: the label may not survive the state uncertainty, and the
            // result must then say so.
            assert!(
                o["family"]["family"] == want
                    || o["family"]["covariance_agreement"].as_f64().unwrap() < 1.0,
                "{o:#}"
            );
        } else {
            assert_eq!(o["family"]["family"], want, "{truth}");
            assert_eq!(o["family"]["stable_under_covariance"], true, "{truth}");
        }
    }
    // Multi-day objects: four tracklets each.
    for o in d["catalogue"].as_array().unwrap() {
        let id = o["id"].as_str().unwrap();
        let truth = sc["objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["object"] == id)
            .unwrap()["truth"]
            .as_str()
            .unwrap();
        if truth != "HALO-C" {
            assert_eq!(o["tracklets"].as_array().unwrap().len(), 4, "{id}");
            assert!(o["arc_days"].as_f64().unwrap() > 1.0);
        }
    }
    assert!(st.csv.lines().count() >= 4);
    assert!(st.svg.starts_with("<svg"));
    assert!(d["summary"].as_str().unwrap().contains("catalogued object"));
}

#[test]
fn tdm_written_by_the_campaign_reads_back_into_the_same_catalogue() {
    let (cfg, st) = campaign();
    let dir = std::env::temp_dir().join(format!("ak9-tdm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("campaign.tdm");
    std::fs::write(&path, st.tdm.as_ref().unwrap()).unwrap();
    let mut c2 = cfg.clone();
    c2.synthetic = None;
    c2.epoch_utc = Some("2026-12-01T00:00:00".into());
    c2.tdm_files = vec![path.to_string_lossy().into_owned()];
    let st2 = run_study(&c2).unwrap();
    assert_eq!(st2.doc["observations"], st.doc["observations"]);
    assert_eq!(st2.doc["tracklets"], st.doc["tracklets"]);
    let n = |d: &Value| d["catalogue"].as_array().unwrap().len();
    assert_eq!(n(&st2.doc), n(&st.doc));
    for (a, b) in st.doc["catalogue"]
        .as_array()
        .unwrap()
        .iter()
        .zip(st2.doc["catalogue"].as_array().unwrap())
    {
        assert_eq!(a["family"]["family"], b["family"]["family"]);
        let ea = a["state_synodic"].as_array().unwrap();
        let eb = b["state_synodic"].as_array().unwrap();
        for k in 0..3 {
            let d = (ea[k].as_f64().unwrap() - eb[k].as_f64().unwrap()).abs() * L_KM;
            assert!(d < 1.0, "TDM round trip moved the solution by {d} km");
        }
    }
    let inputs = st2.doc["inputs"].as_array().unwrap();
    assert_eq!(inputs[0]["sha256"].as_str().unwrap().len(), 64);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn signed_study_directory_verifies_and_detects_tampering() {
    let (_, st) = campaign();
    let dir = std::env::temp_dir().join(format!("ak9-signed-{}", std::process::id()));
    let seed = [9u8; 32];
    let sig = write_study(
        st,
        &dir,
        Some((&seed, "Arksa test", "2026-12-01T00:00:00Z")),
    )
    .unwrap()
    .unwrap();
    assert!(sig.exists());
    for f in [
        "study.json",
        "catalogue.csv",
        "report.md",
        "figure.svg",
        "observations.tdm",
    ] {
        assert!(dir.join(f).exists(), "{f}");
    }
    let key = kshana_pro::license::public_key_for(&seed);
    assert!(kshana_pro::signed_dossier::verify(&dir, &[key])
        .unwrap()
        .passed());
    std::fs::write(dir.join("catalogue.csv"), "tampered\n").unwrap();
    let v = kshana_pro::signed_dossier::verify(&dir, &[key]);
    assert!(v.map(|v| !v.passed()).unwrap_or(true));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_result_is_labelled_modelled_and_claims_no_endorsement() {
    let (_, st) = campaign();
    let label = st.doc["label"].as_str().unwrap();
    assert!(label.starts_with("MODELLED"));
    assert!(label.contains("not endorsed by any agency"));
    assert!(label.contains("not performance on real data"));
    let report = arksa_kpro::cislunar_ssa::report_md(&st.doc);
    for w in [
        "certified",
        "validated against",
        "kill",
        "obsolete",
        "dominate",
    ] {
        assert!(!report.to_lowercase().contains(w), "{w}");
    }
}

// ── Debris environment ────────────────────────────────────────────────────────────────

fn debris(extra: &str) -> Value {
    let text = format!(
        "trajectory = {{ preset = \"nrho\", phase_frac = 0.0 }}
span_days = 3.0
step_minutes = 10.0
hard_body_radius_m = 10.0
screen_km = 2000.0
{extra}"
    );
    let cfg: DebrisConfig = toml::from_str(&text).unwrap();
    let p = Problem {
        epoch_utc_s: parse_utc("2026-12-01T00:00:00").unwrap(),
        obs: vec![],
        geo: vec![],
    };
    assess(&cfg, &p, &Catalogue::default()).unwrap()
}

const BREAKUP: &str = "
[[breakups]]
name = \"frag\"
preset = \"nrho\"
phase_frac = 0.995
days_before_start = 0.5
fragments_sampled = 40
fragments_represented = REP
dv_sigma_mps = 3.0
kernel_sigma_km = 50.0
seed = 4
";

#[test]
fn debris_risk_scales_with_the_population_and_combines_sources() {
    let one = debris(&BREAKUP.replace("REP", "1000.0"));
    let two = debris(&BREAKUP.replace("REP", "2000.0"));
    let e1 = one["expected_collisions"].as_f64().unwrap();
    let e2 = two["expected_collisions"].as_f64().unwrap();
    assert!(
        e1 > 0.0,
        "a cloud released on the trajectory's own orbit carries risk: {one:#}"
    );
    assert!((e2 / e1 - 2.0).abs() < 1e-6, "{e1} {e2}");
    let p = one["probability_any_collision"].as_f64().unwrap();
    assert!((p - (1.0 - (-e1).exp())).abs() < 1e-12);
    assert_eq!(one["evidence_class"], "MODELLED");
    assert!(!one["top_encounters"].as_array().unwrap().is_empty());

    // A background shell around the Moon adds n A v dt per sample inside it.
    let bg = debris(
        "[[background]]
name = \"shell\"
centre = \"moon\"
r_min_km = 0.0
r_max_km = 100000.0
density_per_km3 = 1e-12
v_rel_kms = 1.0
",
    );
    let src = bg["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["source"] == "shell")
        .unwrap();
    let inside = src["samples_inside"].as_u64().unwrap() as f64;
    let want = 1e-12 * std::f64::consts::PI * (0.01f64).powi(2) * 1.0 * 600.0 * inside;
    let got = bg["expected_collisions"].as_f64().unwrap();
    assert!(
        inside > 0.0 && (got - want).abs() <= 1e-9 * want,
        "{got} {want}"
    );
}

// ── The Arksa gate ────────────────────────────────────────────────────────────────────

const GATED: &str = r#"kind = "pro-cislunar-observability-study"
[ssa]
[[ssa.observers]]
name = "TEIDE"
kind = "station"
lat_deg = 28.3009
lon_deg = -16.5097
alt_m = 2390.0
[ssa.synthetic]
epoch_utc = "2026-12-01T00:00:00"
objects = [ { id = "X", preset = "dro", phase_frac = 0.1 } ]
tracks = [ { observer = "TEIDE", start_utc = "2026-12-01T00:00:00", hours = 24.0, cadence_min = 30.0 } ]
"#;

#[test]
fn the_ssa_section_runs_only_under_the_arksa_entitlement() {
    use kshana_pro::license::{
        self, issue_arksa, issue_v2, verify_with, Keyring, LicenseClaims, VerifyOptions,
    };
    arksa_kpro::ensure_registered();
    let pro_seed = [7u8; 32];
    let arksa_seed: [u8; 32] = *b"arksa-issuer-test-key-not-real!!";
    let mut ring = Keyring::single(license::public_key_for(&pro_seed));
    ring.add_arksa_issuer(license::public_key_for(&arksa_seed));
    let claims = LicenseClaims {
        licensee: "ACME Space".into(),
        tier: "enterprise".into(),
        expires_unix: 10_000,
        products: vec!["cli".into(), "service".into()],
        ..Default::default()
    };
    // A standalone Pro token is refused.
    let pro = issue_v2(&pro_seed, &claims).unwrap();
    let ent = verify_with(&ring, &pro, &VerifyOptions::at(1))
        .unwrap()
        .entitlement;
    assert!(kshana_pro::registry::run_gated(GATED, &ent, 1).is_err());
    // The Arksa-signed token runs it and the result is the SSA document.
    let t = issue_arksa(&arksa_seed, &claims).unwrap();
    let ent = verify_with(&ring, &t, &VerifyOptions::at(1))
        .unwrap()
        .entitlement;
    let out = kshana_pro::registry::run_gated(GATED, &ent, 1).unwrap();
    let v: Value = serde_json::from_str(&out.json).unwrap();
    assert_eq!(v["schema"], "arksa-cislunar-ssa/1");
    assert!(v["label"].as_str().unwrap().starts_with("MODELLED"));
    assert!(out.summary.contains("cislunar-ssa"));
}

#[test]
fn an_ssa_run_is_deterministic() {
    let text = GATED
        .replace("kind = \"pro-cislunar-observability-study\"\n[ssa]\n", "")
        .replace("ssa.", "");
    let cfg: SsaConfig = toml::from_str(&text).unwrap();
    let a = run_study(&cfg).unwrap();
    let b = run_study(&cfg).unwrap();
    assert_eq!(a.doc, b.doc);
    assert_eq!(a.csv, b.csv);
    assert_eq!(a.svg, b.svg);
    assert_eq!(a.tdm, b.tdm);
}

#[test]
fn the_worked_scenario_parses_into_the_kind() {
    use arksa_kpro::packs::cislunar_observability_study_pack::CislunarObservabilityStudyScenario;
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios/cislunar-ssa/demo.toml"),
    )
    .unwrap();
    let s: CislunarObservabilityStudyScenario = toml::from_str(&text).unwrap();
    let ssa = s.ssa.expect("the [ssa] section");
    assert_eq!(ssa.observers.len(), 2);
    assert_eq!(ssa.synthetic.as_ref().unwrap().objects.len(), 3);
    let d = ssa.debris.expect("the debris section");
    assert_eq!(d.trajectory.object.as_deref(), Some("C-001"));
    assert_eq!(d.breakups.len(), 1);
    assert_eq!(d.background.len(), 1);
}
