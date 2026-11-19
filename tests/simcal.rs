// PRIVATE — Arksa-exclusive (arksa-kpro).
//
//! AK8: simulator plugins against telemetry, recalibration through the hosted
//! self-calibration verdict (a local stand-in service that signs its answers with a test
//! key), and the signed calibration lineage. Toy mission TOY-1 (synthetic data).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use arksa_kpro::license::{Entitlement, Tier};
use arksa_kpro::simcal::hosted::{self, SELFCAL};
use arksa_kpro::simcal::lineage::{self, RecalOptions, Status, GENESIS};
use arksa_kpro::simcal::mission::{parse_telemetry, Mission};
use arksa_kpro::simcal::plugin::{self, Context, SimPlugin};
use arksa_kpro::simcal::residuals::{self, Verdict};
use kshana_pro::license::signer::LicenceSigner;
use kshana_pro::remote_verdict::{SignedVerdict, VerdictTransport};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const SERVICE_SEED: [u8; 32] = *b"ak8-simcal-stand-in-service-key!";
const ARKSA_SEED: [u8; 32] = [11; 32];
const UTC: &str = "2026-11-20T09:00:00Z";
/// Offset (ns) of the committed clock telemetry from the engine's prediction.
const CLOCK_OFFSET_NS: f64 = 1.25;

fn enterprise() -> Entitlement {
    Entitlement {
        licensee: "(test)".into(),
        tier: Tier::Enterprise,
        expires_unix: 0,
        features: vec![],
        arksa: true,
    }
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/simcal/toy-1")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "arksa-ak8-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A copy of the toy mission in a fresh directory.
fn toy_copy(tag: &str) -> PathBuf {
    let d = tmp(tag);
    for e in std::fs::read_dir(fixture()).unwrap() {
        let p = e.unwrap().path();
        std::fs::copy(&p, d.join(p.file_name().unwrap())).unwrap();
    }
    d
}

fn load(dir: &Path) -> (Mission, Vec<Box<dyn SimPlugin>>) {
    let ent = enterprise();
    let m = Mission::load(&dir.join("mission.toml")).unwrap();
    let ctx = Context {
        dir: &m.dir,
        entitlement: &ent,
        now_unix: 1_790_000_000,
    };
    let p = m
        .file
        .plugins
        .iter()
        .map(|s| plugin::build(s, &ctx).unwrap())
        .collect();
    (m, p)
}

fn service_pk() -> String {
    hex::encode(SERVICE_SEED.public_key().unwrap())
}

fn arksa_pk() -> [u8; 32] {
    ARKSA_SEED.public_key().unwrap()
}

/// A verdict signed exactly as the hosted service signs one (canonical body: the envelope
/// fields above `signature`, in the client's order).
fn sign(method: &str, verdict: Value, seed: &[u8; 32]) -> SignedVerdict {
    let body = json!({
        "service": "stand-in/1",
        "method": method,
        "evidence_class": "MODELLED",
        "utc": "2026-11-20T08:59:00Z",
        "key_id": "test-k1",
        "verdict": verdict,
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    SignedVerdict {
        service: "stand-in/1".into(),
        method: method.into(),
        evidence_class: "MODELLED".into(),
        utc: "2026-11-20T08:59:00Z".into(),
        key_id: "test-k1".into(),
        verdict: body["verdict"].clone(),
        verdict_sha256: hex::encode(Sha256::digest(&bytes)),
        signature: hex::encode(seed.sign_bytes(&bytes).unwrap()),
    }
}

/// The stand-in's outcome for a request: outcome fields only, as the service documents.
fn outcome_for(req: &Value, converged: bool) -> Value {
    let n = req["observations"].as_array().map(|a| a.len()).unwrap_or(0);
    let head = hex::encode(Sha256::digest(serde_json::to_vec(req).unwrap()));
    json!({
        "converged": converged,
        "stop_reason": if converged { "cost_converged" } else { "max_iter" },
        "passes": 4,
        "regenerations": 4,
        "fidelity_first": 61.0,
        "fidelity_last": 93.5,
        "posterior": [
            {"name": "all", "value": -2.41, "std_err": 0.08},
            {"name": "obs_count", "value": n as f64, "std_err": 0.0},
        ],
        "lineage": {"head": head, "entries": 5, "scenario_hash": "ab12", "recording_digest": "cd34", "key_id": "lin-1"},
    })
}

/// What the stand-in received.
#[derive(Default, Clone)]
struct Seen {
    paths: Vec<String>,
    bearers: Vec<String>,
    bodies: Vec<Value>,
}

/// A loopback stand-in for the hosted verdict service: answers `POST /verdict/selfcal` with
/// a verdict signed by `seed`. Returns its base URL and what it saw.
fn stand_in(seed: [u8; 32], converged: bool, answers: usize) -> (String, Arc<Mutex<Seen>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for stream in l.incoming().take(answers) {
            let mut stream = stream.unwrap();
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
            let (mut len, mut bearer, mut expect) = (0usize, String::new(), false);
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                let h = h.trim_end();
                if h.is_empty() {
                    break;
                }
                let (k, v) = h.split_once(':').unwrap();
                match k.to_ascii_lowercase().as_str() {
                    "content-length" => len = v.trim().parse().unwrap(),
                    "authorization" => bearer = v.trim().trim_start_matches("Bearer ").into(),
                    "expect" => expect = true,
                    _ => {}
                }
            }
            if expect {
                stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").unwrap();
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            {
                let mut s = s2.lock().unwrap();
                s.paths.push(path.clone());
                s.bearers.push(bearer.clone());
                s.bodies.push(req.clone());
            }
            let (code, text) = if path != "/verdict/selfcal" {
                ("404 Not Found", r#"{"error":"no such method"}"#.to_string())
            } else if bearer.is_empty() {
                (
                    "401 Unauthorized",
                    r#"{"error":"licence required"}"#.to_string(),
                )
            } else {
                let v = sign(SELFCAL, outcome_for(&req, converged), &seed);
                ("200 OK", serde_json::to_string(&v).unwrap())
            };
            let resp = format!(
                "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
            stream.write_all(resp.as_bytes()).unwrap();
        }
    });
    (url, seen)
}

/// An in-process transport answering with a fixed text.
struct Fixed(Result<String, String>);
impl VerdictTransport for Fixed {
    fn post(&self, _p: &str, _b: &str, _body: &str) -> Result<String, String> {
        self.0.clone()
    }
}

fn opts<'a>(t: &'a dyn VerdictTransport, keys: &'a str) -> RecalOptions<'a> {
    RecalOptions {
        transport: t,
        bearer: "licence-token",
        trusted_verdict_keys: keys,
        utc: UTC,
        signer: Some((&ARKSA_SEED, "Arksa test")),
    }
}

// ------------------------------------------------------------------ fixture telemetry

/// The committed telemetry is the base file plus the clock rows: the engine's prediction
/// at every 600 s, offset by CLOCK_OFFSET_NS. `REGENERATE_SIMCAL_FIXTURE=1` rewrites it.
#[test]
fn committed_telemetry_is_base_plus_engine_clock_rows() {
    let base = std::fs::read_to_string(fixture().join("telemetry-base.csv")).unwrap();
    // The base alone names every observable of the mission but the clock.
    let ent = enterprise();
    let m_text = std::fs::read_to_string(fixture().join("mission.toml")).unwrap();
    let file: arksa_kpro::simcal::mission::MissionFile = toml::from_str(&m_text).unwrap();
    let clock = file.plugins.iter().find(|p| p.id == "clock").unwrap();
    let ctx = Context {
        dir: &fixture(),
        entitlement: &ent,
        now_unix: 1_790_000_000,
    };
    let p = plugin::build(clock, &ctx).unwrap();
    let mut rows = String::new();
    for k in 0..=12 {
        let t = 600.0 * k as f64;
        let v = p.predict("time_error_ns", t).unwrap() + CLOCK_OFFSET_NS;
        rows.push_str(&format!("{t},clock.time_error_ns,{v:.6},0.5\n"));
    }
    let want = format!("{base}{rows}");
    if std::env::var("REGENERATE_SIMCAL_FIXTURE").is_ok() {
        std::fs::write(fixture().join("telemetry.csv"), &want).unwrap();
    }
    let got = std::fs::read_to_string(fixture().join("telemetry.csv")).unwrap();
    assert_eq!(got, want, "telemetry.csv is stale; regenerate it");
}

// ------------------------------------------------------------------ residuals

#[test]
fn toy_mission_residuals_find_the_drifted_power_model() {
    let (m, plugins) = load(&fixture());
    assert_eq!(m.file.mission, "TOY-1");
    assert_eq!(plugins.len(), 3);
    let (rep, samples) = residuals::report(&m, &plugins);
    assert_eq!(rep.format, "ks-simcal-residuals/1");
    assert_eq!(rep.evidence_class, "MODELLED");
    let by = |id: &str| rep.plugins.iter().find(|p| p.plugin == id).unwrap();
    let eps = by("eps");
    let power = &eps.observables[0];
    let temp = &eps.observables[1];
    // The array is 3 % weaker than the simulator assumes: drifted, negative bias.
    assert_eq!(power.verdict, Verdict::Drifted, "{power:?}");
    assert!(power.bias.unwrap() < -4.0, "{power:?}");
    assert_eq!(power.compared, 19);
    assert!(power.chi2_reduced.unwrap() > 10.0);
    // Temperature is sampled between the simulator's steps: interpolated and consistent.
    assert_eq!(temp.verdict, Verdict::Consistent, "{temp:?}");
    assert_eq!(temp.compared, 18);
    assert!(temp.bias.unwrap().abs() < 0.3);
    assert!(temp.bias_u.unwrap() > 0.0);
    assert_eq!(eps.verdict, Verdict::Drifted);
    // The open-engine clock: offset by CLOCK_OFFSET_NS, within its 5 ns tolerance.
    let clock = &by("clock").observables[0];
    assert_eq!(clock.verdict, Verdict::Consistent, "{clock:?}");
    assert_eq!(clock.compared, 13);
    assert!((clock.bias.unwrap() - CLOCK_OFFSET_NS).abs() < 1e-5);
    assert!(
        clock.bias_u.unwrap() < 1e-5,
        "a constant offset has no spread"
    );
    // The hosted model forms its residuals in the service.
    let timing = by("timing");
    assert_eq!(timing.verdict, Verdict::Hosted);
    assert!(timing
        .observables
        .iter()
        .all(|o| o.samples == 3 && o.compared == 0 && o.bias.is_none()));
    // The CSV holds every compared sample.
    assert_eq!(samples.len(), 19 + 18 + 13);
    let csv = residuals::samples_csv(&samples);
    assert!(csv.starts_with("observable,t_s,telemetry,simulated,residual\n"));
    assert_eq!(csv.lines().count(), 1 + samples.len());
}

#[test]
fn telemetry_outside_the_simulated_span_is_not_compared() {
    let d = toy_copy("span");
    let mut t = std::fs::read_to_string(d.join("telemetry.csv")).unwrap();
    t.push_str("9999,eps.battery_temp_c,18.0,0.5\n-5,eps.battery_temp_c,18.0,0.5\n");
    std::fs::write(d.join("telemetry.csv"), t).unwrap();
    let (m, plugins) = load(&d);
    let (rep, _) = residuals::report(&m, &plugins);
    let temp = &rep.plugins[0].observables[1];
    assert_eq!(temp.samples, 20);
    assert_eq!(temp.compared, 18);
}

#[test]
fn bad_missions_and_telemetry_are_refused_with_reasons() {
    let d = toy_copy("bad");
    let good = std::fs::read_to_string(d.join("mission.toml")).unwrap();
    let ent = enterprise();
    let try_mission = |text: &str| -> String {
        std::fs::write(d.join("m.toml"), text).unwrap();
        match Mission::load(&d.join("m.toml")) {
            Err(e) => e,
            Ok(m) => {
                let ctx = Context {
                    dir: &m.dir,
                    entitlement: &ent,
                    now_unix: 1_790_000_000,
                };
                match m
                    .file
                    .plugins
                    .iter()
                    .map(|s| plugin::build(s, &ctx))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Err(e) => e,
                    Ok(_) => "ok".into(),
                }
            }
        }
    };
    assert_eq!(try_mission(&good), "ok");
    let cases: &[(&str, &str, &str)] = &[
        ("ks-simcal-mission/1", "ks-simcal-mission/9", "format"),
        ("kind = \"table\"", "kind = \"spreadsheet\"", "unknown kind"),
        ("tolerance = 4.0", "tolerance = -1.0", "tolerance"),
        (
            "method = \"selfcal\"",
            "method = \"survey\"",
            "not a calibration method",
        ),
        (
            "method = \"selfcal\"",
            "method = \"nope\"",
            "not a hosted method",
        ),
        ("id = \"clock\"", "id = \"eps\"", "used twice"),
        (
            "y = \"error_ns\"",
            "y = \"no_such_field\"",
            "not every element",
        ),
        (
            "predictions = \"eps-sim.csv\"",
            "predictions = \"eps-sim.csv\"\nscenario = \"x.toml\"",
            "does not apply",
        ),
    ];
    for (from, to, want) in cases {
        let e = try_mission(&good.replacen(from, to, 1));
        assert!(e.contains(want), "{from} -> {to}: {e}");
    }
    // A hosted observable without a key.
    let e = try_mission(&good.replacen(
        "key = { site = \"s1\", chain = \"c\", station = \"M\", kind = \"ground_toa\" }",
        "",
        1,
    ));
    assert!(e.contains("needs a `key`"), "{e}");
    // Telemetry naming an observable no plugin declares.
    let mut t = std::fs::read_to_string(d.join("telemetry.csv")).unwrap();
    t.push_str("0,eps.solar_flux,1.0,\n");
    std::fs::write(d.join("telemetry.csv"), t).unwrap();
    assert!(try_mission(&good).contains("no plugin declares"));
    // Telemetry parsing.
    assert!(parse_telemetry("a,b,c\n").unwrap_err().contains("header"));
    assert!(parse_telemetry("t_s,observable,value\n0,x,abc\n")
        .unwrap_err()
        .contains("not a number"));
    assert!(parse_telemetry("t_s,observable,value,sigma\n0,x,1,0\n")
        .unwrap_err()
        .contains("positive"));
    assert!(parse_telemetry("t_s,observable,value\n")
        .unwrap_err()
        .contains("no samples"));
    let rows = parse_telemetry("# c\nt_s,observable,value,sigma\n1,x,2,\n3,y,4,0.5\n").unwrap();
    assert_eq!(rows[0].sigma, None);
    assert_eq!(rows[1].sigma, Some(0.5));
}

// ------------------------------------------------------------------ hosted request

#[test]
fn the_hosted_request_carries_scenario_config_sites_and_telemetry_only() {
    let (m, plugins) = load(&fixture());
    let timing = plugins.iter().find(|p| p.id() == "timing").unwrap();
    let body: Value =
        serde_json::from_str(&hosted::request_body(&m, timing.as_ref()).unwrap()).unwrap();
    let keys: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys.len(), 4, "{keys:?}");
    assert_eq!(
        body["scenario_toml"].as_str().unwrap(),
        std::fs::read_to_string(fixture().join("timing-scenario.toml")).unwrap()
    );
    assert_eq!(body["config"]["subject_id"], "toy-1-timing");
    assert_eq!(body["config"]["sections"][0]["name"], "all");
    assert_eq!(body["sites"].as_array().unwrap().len(), 2);
    assert_eq!(body["sites"][1]["position"]["lon_deg"], 2.0);
    let obs = body["observations"].as_array().unwrap();
    assert_eq!(obs.len(), 12);
    assert_eq!(
        obs[0],
        json!({"site": "s1", "chain": "c", "station": "M", "kind": "ground_toa", "value_ns": 3991.5, "sigma_ns": 20.0})
    );
    // Nothing of the other plugins goes to the service.
    let text = body.to_string();
    assert!(!text.contains("bus_power") && !text.contains("time_error"));
    // A local plugin has no hosted request.
    let eps = plugins.iter().find(|p| p.id() == "eps").unwrap();
    assert!(hosted::request_body(&m, eps.as_ref()).is_err());
}

#[test]
fn a_hosted_sample_without_sigma_is_refused() {
    let d = toy_copy("nosigma");
    let t = std::fs::read_to_string(d.join("telemetry.csv"))
        .unwrap()
        .replacen(
            "0,timing.toa_s1_m,3991.5,20.0",
            "0,timing.toa_s1_m,3991.5,",
            1,
        );
    std::fs::write(d.join("telemetry.csv"), t).unwrap();
    let (m, plugins) = load(&d);
    let e = hosted::request_body(&m, plugins[2].as_ref()).unwrap_err();
    assert!(e.contains("sigma"), "{e}");
}

// ------------------------------------------------------------------ recalibration over the network

#[test]
fn recalibration_through_the_stand_in_service_records_a_signed_lineage() {
    let (m, plugins) = load(&fixture());
    let (url, seen) = stand_in(SERVICE_SEED, true, 2);
    let t = kshana_pro::remote_verdict::CurlVerdictTransport { base_url: url };
    let dir = tmp("lineage");
    let pk = service_pk();
    let e = lineage::recalibrate_mission(&m, &plugins, &dir, &opts(&t, &pk)).unwrap();

    // The stand-in saw one selfcal request with the licence token and the request body.
    {
        let s = seen.lock().unwrap();
        assert_eq!(s.paths, vec!["/verdict/selfcal"]);
        assert_eq!(s.bearers, vec!["licence-token"]);
        assert_eq!(s.bodies[0]["observations"].as_array().unwrap().len(), 12);
    }
    assert_eq!(e.seq, 1);
    assert_eq!(e.prev_entry_sha256, GENESIS);
    assert_eq!(e.entry_sha256, e.body_sha256());
    assert_eq!(e.evidence_class, "MODELLED");
    let st = |id: &str| e.plugins.iter().find(|p| p.plugin == id).unwrap();
    assert_eq!(st("eps").status, Status::RecalibrationNeeded);
    assert!(st("eps")
        .note
        .contains("No hosted calibration method serves kind 'table'"));
    assert_eq!(st("clock").status, Status::Consistent);
    let timing = st("timing");
    assert_eq!(timing.status, Status::Calibrated);
    let h = timing.hosted.as_ref().unwrap();
    assert_eq!(h.method, "selfcal");
    assert_eq!(h.evidence_class, "MODELLED");
    assert!(h.outcome.converged);
    assert_eq!(h.outcome.fidelity_before, Some(61.0));
    assert_eq!(h.outcome.fidelity_after, Some(93.5));
    assert_eq!(h.outcome.parameters[0].name, "all");
    assert_eq!(h.outcome.parameters[1].value, 12.0);
    // The service's lineage head is the stand-in's hash of exactly the request recorded.
    let req = std::fs::read(dir.join(&h.request_file)).unwrap();
    assert_eq!(hex::encode(Sha256::digest(&req)), h.request_sha256);
    let req: Value = serde_json::from_slice(&req).unwrap();
    assert_eq!(
        h.outcome.service_lineage_head,
        hex::encode(Sha256::digest(serde_json::to_vec(&req).unwrap()))
    );
    // Sources are recorded by hash.
    assert_eq!(st("eps").sources[0].path, "eps-sim.csv");

    // The lineage checks, with the service key and the Arksa signer pinned.
    let c = lineage::verify_dir(&dir, &pk, &[arksa_pk()]).unwrap();
    assert!(c.passed(), "{:?}", c.problems);
    assert!(c.signed);
    assert_eq!(c.entries, 1);

    // A second recalibration chains to the first.
    let e2 = lineage::recalibrate_mission(&m, &plugins, &dir, &opts(&t, &pk)).unwrap();
    assert_eq!(e2.seq, 2);
    assert_eq!(e2.prev_entry_sha256, e.entry_sha256);
    let c = lineage::verify_dir(&dir, &pk, &[arksa_pk()]).unwrap();
    assert!(c.passed(), "{:?}", c.problems);
    assert_eq!(c.entries, 2);
    assert_eq!(c.head.as_deref(), Some(e2.entry_sha256.as_str()));
    let md = lineage::summary_md(&e2);
    assert!(md.contains("| timing | hosted | calibrated |"), "{md}");
    assert!(md.contains("MODELLED"));
}

#[test]
fn a_verdict_not_signed_by_a_trusted_key_is_refused_and_nothing_is_written() {
    let (m, plugins) = load(&fixture());
    let (url, _) = stand_in([3; 32], true, 1);
    let t = kshana_pro::remote_verdict::CurlVerdictTransport { base_url: url };
    let dir = tmp("foreign");
    let pk = service_pk();
    let e = lineage::recalibrate_mission(&m, &plugins, &dir, &opts(&t, &pk)).unwrap_err();
    assert!(e.contains("verdict refused"), "{e}");
    assert!(lineage::read_entries(&dir).unwrap().is_empty());
    assert!(!dir.join("verdicts").exists());
}

#[test]
fn service_errors_and_malformed_answers_are_refused() {
    let (m, plugins) = load(&fixture());
    let pk = service_pk();
    let timing = plugins[2].as_ref();
    // No licence token: the client refuses before sending.
    let (url, seen) = stand_in(SERVICE_SEED, true, 1);
    let t = kshana_pro::remote_verdict::CurlVerdictTransport { base_url: url };
    let e = hosted::recalibrate(&t, "", &m, timing, &pk).unwrap_err();
    assert!(e.contains("licence token"), "{e}");
    assert!(seen.lock().unwrap().paths.is_empty());
    // Unreachable service.
    let e = hosted::recalibrate(&Fixed(Err("offline".into())), "tok", &m, timing, &pk).unwrap_err();
    assert!(e.contains("offline"), "{e}");
    // A verdict for another method.
    let other = serde_json::to_string(&sign("survey-plan", json!({}), &SERVICE_SEED)).unwrap();
    let e = hosted::recalibrate(&Fixed(Ok(other)), "tok", &m, timing, &pk).unwrap_err();
    assert!(e.contains("not 'selfcal'"), "{e}");
    // Signed, but without the documented outcome fields.
    for (verdict, want) in [
        (
            json!({"posterior": [], "lineage": {"head": "h"}}),
            "converged",
        ),
        (
            json!({"converged": true, "lineage": {"head": "h"}}),
            "posterior",
        ),
        (
            json!({"converged": true, "posterior": [{"name": "a"}]}),
            "posterior",
        ),
        (json!({"converged": true, "posterior": []}), "lineage.head"),
    ] {
        let v = serde_json::to_string(&sign(SELFCAL, verdict, &SERVICE_SEED)).unwrap();
        let e = hosted::recalibrate(&Fixed(Ok(v)), "tok", &m, timing, &pk).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
    // A tampered outcome fails the signature.
    let mut v = sign(SELFCAL, outcome_for(&json!({}), true), &SERVICE_SEED);
    v.verdict["posterior"][0]["value"] = json!(0.0);
    let e = hosted::recalibrate(
        &Fixed(Ok(serde_json::to_string(&v).unwrap())),
        "tok",
        &m,
        timing,
        &pk,
    )
    .unwrap_err();
    assert!(e.contains("verdict refused"), "{e}");
}

#[test]
fn a_calibration_that_did_not_converge_is_recorded_as_such() {
    let (m, plugins) = load(&fixture());
    let (url, _) = stand_in(SERVICE_SEED, false, 1);
    let t = kshana_pro::remote_verdict::CurlVerdictTransport { base_url: url };
    let dir = tmp("noconv");
    let pk = service_pk();
    let e = lineage::recalibrate_mission(&m, &plugins, &dir, &opts(&t, &pk)).unwrap();
    let timing = e.plugins.iter().find(|p| p.plugin == "timing").unwrap();
    assert_eq!(timing.status, Status::NotConverged);
    assert!(timing.note.contains("nothing is taken as calibrated"));
    assert!(lineage::verify_dir(&dir, &pk, &[]).unwrap().passed());
}

// ------------------------------------------------------------------ lineage tampering

fn recorded_lineage(tag: &str) -> (PathBuf, String) {
    let (m, plugins) = load(&fixture());
    let pk = service_pk();
    let dir = tmp(tag);
    for _ in 0..2 {
        let v = sign(SELFCAL, outcome_for(&json!({"x": 1}), true), &SERVICE_SEED);
        let t = Fixed(Ok(serde_json::to_string(&v).unwrap()));
        lineage::recalibrate_mission(&m, &plugins, &dir, &opts(&t, &pk)).unwrap();
    }
    (dir, pk)
}

#[test]
fn edits_to_the_lineage_are_detected() {
    let (dir, pk) = recorded_lineage("tamper");
    assert!(lineage::verify_dir(&dir, &pk, &[arksa_pk()])
        .unwrap()
        .passed());

    // A wrong signer key or a wrong service key.
    let c = lineage::verify_dir(&dir, &pk, &[[9; 32]]).unwrap();
    assert!(
        c.problems.iter().any(|p| p.contains("not a trusted key")),
        "{:?}",
        c.problems
    );
    let other = hex::encode([4u8; 32].public_key().unwrap());
    let c = lineage::verify_dir(&dir, &other, &[]).unwrap();
    assert!(
        c.problems.iter().any(|p| p.contains("verdict:")),
        "{:?}",
        c.problems
    );
    let c = lineage::verify_dir(&dir, "", &[]).unwrap();
    assert!(c
        .problems
        .iter()
        .any(|p| p.contains("no trusted service key")));

    let entries = std::fs::read_to_string(dir.join("entries.jsonl")).unwrap();
    let restore = || std::fs::write(dir.join("entries.jsonl"), &entries).unwrap();
    let lines: Vec<&str> = entries.lines().collect();

    // Removing the first entry breaks the chain (and the signature).
    std::fs::write(dir.join("entries.jsonl"), format!("{}\n", lines[1])).unwrap();
    let c = lineage::verify_dir(&dir, &pk, &[]).unwrap();
    assert!(
        c.problems.iter().any(|p| p.contains("sequence number")),
        "{:?}",
        c.problems
    );
    assert!(c
        .problems
        .iter()
        .any(|p| p.contains("does not name the entry before it")));
    assert!(c.problems.iter().any(|p| p.starts_with("signature:")));
    restore();

    // Reordering.
    std::fs::write(
        dir.join("entries.jsonl"),
        format!("{}\n{}\n", lines[1], lines[0]),
    )
    .unwrap();
    assert!(!lineage::verify_dir(&dir, &pk, &[]).unwrap().passed());
    restore();

    // Editing an entry's status, even with its own hash recomputed.
    let mut e: lineage::Entry = serde_json::from_str(lines[0]).unwrap();
    e.plugins[0].status = Status::Consistent;
    e.entry_sha256 = e.body_sha256();
    let edited = format!("{}\n{}\n", serde_json::to_string(&e).unwrap(), lines[1]);
    std::fs::write(dir.join("entries.jsonl"), edited).unwrap();
    let c = lineage::verify_dir(&dir, &pk, &[]).unwrap();
    assert!(
        c.problems
            .iter()
            .any(|p| p.contains("does not name the entry before it")),
        "{:?}",
        c.problems
    );
    assert!(c.problems.iter().any(|p| p.starts_with("signature:")));
    restore();

    // Claiming a hosted calibration converged when the verdict says otherwise.
    let mut e: lineage::Entry = serde_json::from_str(lines[1]).unwrap();
    let t = e.plugins.iter_mut().find(|p| p.plugin == "timing").unwrap();
    t.hosted.as_mut().unwrap().outcome.parameters[0].value = 1.0;
    e.entry_sha256 = e.body_sha256();
    std::fs::write(
        dir.join("entries.jsonl"),
        format!("{}\n{}\n", lines[0], serde_json::to_string(&e).unwrap()),
    )
    .unwrap();
    let c = lineage::verify_dir(&dir, &pk, &[]).unwrap();
    assert!(
        c.problems.iter().any(|p| p.contains("outcome differs")),
        "{:?}",
        c.problems
    );
    restore();

    // Editing a stored verdict file.
    let vf = dir.join("verdicts/0001-timing.json");
    let original = std::fs::read_to_string(&vf).unwrap();
    std::fs::write(&vf, original.replace("93.5", "99.5")).unwrap();
    let c = lineage::verify_dir(&dir, &pk, &[]).unwrap();
    assert!(
        c.problems
            .iter()
            .any(|p| p.contains("verdicts/0001-timing.json changed")),
        "{:?}",
        c.problems
    );
    std::fs::write(&vf, &original).unwrap();
    assert!(lineage::verify_dir(&dir, &pk, &[arksa_pk()])
        .unwrap()
        .passed());

    // An extra file is not covered by the signature.
    std::fs::write(dir.join("extra.txt"), "x").unwrap();
    let c = lineage::verify_dir(&dir, &pk, &[]).unwrap();
    assert!(
        c.problems.iter().any(|p| p.contains("not covered")),
        "{:?}",
        c.problems
    );
}

#[test]
fn an_altered_lineage_is_not_extended() {
    let (dir, pk) = recorded_lineage("noextend");
    let vf = dir.join("requests/0002-timing.json");
    std::fs::write(&vf, "{}").unwrap();
    let (m, plugins) = load(&fixture());
    let v = sign(SELFCAL, outcome_for(&json!({}), true), &SERVICE_SEED);
    let t = Fixed(Ok(serde_json::to_string(&v).unwrap()));
    let e = lineage::recalibrate_mission(&m, &plugins, &dir, &opts(&t, &pk)).unwrap_err();
    assert!(e.contains("does not check"), "{e}");
    assert_eq!(lineage::read_entries(&dir).unwrap().len(), 2);
}

// ------------------------------------------------------------------ command line

fn bin(args: &[&str], home: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_arksa-kpro"))
        .args(args)
        .env("KSHANA_PRO_HOME", home)
        .env_remove("KSHANA_VERDICT_URL")
        .env_remove("KSHANA_VERDICT_KEY")
        .env_remove("ARKSA_STUDY_SIGNING_KEY")
        .output()
        .expect("the binary runs")
}

#[test]
fn the_command_is_gated_and_runs_end_to_end() {
    let work = tmp("cli");
    let home = work.join("home");
    let mission = fixture().join("mission.toml");
    let m = mission.to_str().unwrap();

    // Without an Arksa entitlement the command is refused.
    let out = bin(&["simcal", "residuals", m], &home);
    assert!(!out.status.success());

    let out = bin(&["simcal", "plugins", "--dev"], &home);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "table\nengine\nhosted\n"
    );

    let res = work.join("res");
    let out = bin(
        &[
            "simcal",
            "residuals",
            m,
            "--out",
            res.to_str().unwrap(),
            "--dev",
        ],
        &home,
    );
    let so = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(so.contains("eps (table): drifted"), "{so}");
    assert!(so.contains("clock (engine): consistent"), "{so}");
    assert!(so.contains("timing (hosted): hosted"), "{so}");
    let rep: Value =
        serde_json::from_str(&std::fs::read_to_string(res.join("residuals.json")).unwrap())
            .unwrap();
    assert_eq!(rep["mission"], "TOY-1");
    assert!(res.join("residuals.csv").exists());

    // Recalibrate against the stand-in, signed; then verify with both keys pinned.
    let (url, _) = stand_in(SERVICE_SEED, true, 1);
    let keyf = work.join("arksa.key");
    std::fs::write(&keyf, hex::encode(ARKSA_SEED)).unwrap();
    let lin = work.join("lineage");
    let pk = service_pk();
    let out = bin(
        &[
            "simcal",
            "recalibrate",
            m,
            "--lineage",
            lin.to_str().unwrap(),
            "--service",
            &url,
            "--trusted-key",
            &pk,
            "--verdict-license",
            "tok",
            "--sign-key-file",
            keyf.to_str().unwrap(),
            "--utc",
            UTC,
            "--dev",
        ],
        &home,
    );
    let so = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{so}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(so.contains("timing (hosted): calibrated"), "{so}");
    assert!(so.contains("eps (table): recalibration-needed"), "{so}");
    assert!(so.contains("lineage entry 1"), "{so}");

    let arksa = hex::encode(arksa_pk());
    let out = bin(
        &[
            "simcal",
            "verify",
            lin.to_str().unwrap(),
            "--trusted-key",
            &pk,
            "--trust",
            &arksa,
            "--dev",
        ],
        &home,
    );
    let so = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{so}");
    assert!(so.contains("lineage checks"), "{so}");
    assert!(so.contains("signed by Arksa"), "{so}");

    // A wrong service key fails verification with exit code 1.
    let out = bin(
        &[
            "simcal",
            "verify",
            lin.to_str().unwrap(),
            "--trusted-key",
            &hex::encode([1u8; 32]),
            "--dev",
        ],
        &home,
    );
    assert_eq!(out.status.code(), Some(1));

    // Missing service: a clear error.
    let out = bin(
        &[
            "simcal",
            "recalibrate",
            m,
            "--lineage",
            lin.to_str().unwrap(),
            "--trusted-key",
            &pk,
            "--dev",
        ],
        &home,
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--service"));
}

// ------------------------------------------------------------------ honesty and X3

#[test]
fn no_method_internals_and_no_overclaims_in_this_track() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("docs/SIM-RECALIBRATION.md")];
    for e in std::fs::read_dir(root.join("src/simcal")).unwrap() {
        files.push(e.unwrap().path());
    }
    // Words that would describe how the hosted calibration works inside.
    let internals = [
        "gauss",
        "newton",
        "levenberg",
        "jacobian",
        "millington",
        "conductivity",
        "kshana_eloran",
        "kshana-eloran",
        "run_selfcal",
    ];
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap().to_lowercase();
        for w in internals {
            assert!(!text.contains(w), "{} mentions '{w}'", f.display());
        }
        for w in [
            "validated against flight",
            "certified",
            "flight-proven",
            "flight proven",
        ] {
            assert!(!text.contains(w), "{} claims '{w}'", f.display());
        }
    }
    // The crate does not link the hosted service or the gated method code.
    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(!cargo.contains("unfiled-methods"));
}
