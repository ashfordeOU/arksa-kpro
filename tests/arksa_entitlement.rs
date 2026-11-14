// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The `arksa` entitlement through the registration hook (roadmap 0.42 X1 and X3): the
//! half of Kshana Pro's `tests/arksa_entitlement.rs` that needs the Arksa code.
//!
//! * The placeholder issuer key is the test key below.
//! * Registering this crate adds exactly the Arksa-exclusive kinds and features, and Kshana
//!   Pro's signed tier manifest lists every one of them at the `arksa` tier (by hash).
//! * A standalone Pro token, at any licence tier and naming every Arksa id in `features`,
//!   is refused for each of them through the Kshana Pro dispatcher, with the neutral
//!   message; so is Evaluation.
//! * A token carrying the claim, signed by the Arksa issuer, runs each kind through
//!   `kshana_pro::registry::run_gated`; an expired one runs none.
//!
//! The two seeds are TEST keys; they sign nothing that ships.

use arksa_kpro::{ARKSA_FEATURES, ARKSA_KINDS, PLACEHOLDER_ARKSA_ISSUER_PUBKEY};
use kshana_pro::license::tiers::{self, NOT_IN_THIS_EDITION};
use kshana_pro::license::{
    self, issue_arksa, issue_v2, verify_with, Entitlement, Keyring, LicenseClaims, Tier,
    VerifyOptions,
};

const PRO_TEST_SEED: [u8; 32] = [7; 32];
const ARKSA_TEST_SEED: [u8; 32] = *b"arksa-issuer-test-key-not-real!!";

fn ring() -> Keyring {
    let mut r = Keyring::single(license::public_key_for(&PRO_TEST_SEED));
    r.add_arksa_issuer(license::public_key_for(&ARKSA_TEST_SEED));
    r
}

fn claims(tier: &str) -> LicenseClaims {
    LicenseClaims {
        licensee: "ACME Space".into(),
        tier: tier.into(),
        expires_unix: 10_000,
        products: vec!["cli".into(), "service".into()],
        ..Default::default()
    }
}

fn at(now: i64) -> VerifyOptions<'static> {
    VerifyOptions::at(now)
}

/// Small, fast sources for each Arksa-exclusive kind.
fn src(kind: &str) -> &'static str {
    match kind {
        "pro-mbse" => {
            "kind = \"pro-mbse\"\ntitle = \"t\"\n[[requirement]]\nid = \"SYS-001\"\ntext = \"mass <= 12 kg\"\ntarget = 12.0\nachieved = 10.5\nlower_is_better = true\n"
        }
        "pro-design-optimise" => {
            r#"kind = "pro-design-optimise"
title = "t"
seed = 1
base_scenario = """
kind = "link-budget"
band = "x"
eirp_dbw = 55.0
g_over_t_db = 53.0
range_km = 2000.0
data_rate_bps = 1.0e6
other_losses_db = 3.0
required_eb_n0_db = 4.5
"""

[[variable]]
name = "eirp_dbw"
key = "eirp_dbw"
type = "real"
min = 40.0
max = 50.0
step = 5.0

[[objective]]
name = "margin_db"
sense = "max"
path = "margin_db"

[[objective]]
name = "eirp_dbw"
sense = "min"
variable = "eirp_dbw"
"#
        }
        "pro-spectrum-coexistence" => {
            r#"kind = "pro-spectrum-coexistence"
title = "t"
max_degradation_db = 1.0
limit_source = "test limit"
victims = ["gps-l5"]

[grid]
grid_step_deg = 45.0
duration_s = 600.0
step_s = 300.0

[[plan]]
label = "one"
signal = "generic-l"
mask_deg = 10.0

[[plan.satellite]]
altitude_km = 1000.0
inclination_deg = 53.0
"#
        }
        "pro-lunar-signal-security" => "kind = \"pro-lunar-signal-security\"\n",
        "pro-beacon-placement" => {
            "kind = \"pro-beacon-placement\"\nuser_n_lat = 1\nuser_n_lon = 1\ncand_n_lat = 2\ncand_n_lon = 2\nn_epochs = 1\nk_beacons = 1\n"
        }
        "pro-lunar-time-budget-study" => {
            "kind = \"pro-lunar-time-budget-study\"\nclock = \"passive-h-maser\"\ntau_min_s = 1.0\ntau_max_s = 1000.0\npoints_per_decade = 2\n"
        }
        "pro-cislunar-observability-study" => {
            "kind = \"pro-cislunar-observability-study\"\nperilune_km = [20000.0, 32000.0]\nref_phase_frac = [0.0, 0.25]\narc = 0.05\nepochs = 4\nsteps = 800\nwith_range_rate = true\n"
        }
        "pro-optical-rf-hybrid-study" => "kind = \"pro-optical-rf-hybrid-study\"\n",
        "pro-realtime-frame-eop-study" => {
            "kind = \"pro-realtime-frame-eop-study\"\nhorizons_days = [1]\n"
        }
        "pro-beacon-dop" => {
            "kind = \"pro-beacon-dop\"\nlat_min_deg = -85.0\nlat_max_deg = -85.0\nlat_step_deg = 1.0\nn_fleet = 4\nheadline_beacons = 3\nmax_beacons = 4\nn_epochs = 1\nbootstrap_resamples = 8\n"
        }
        "pro-surface-beacon-dop-study" => {
            "kind = \"pro-surface-beacon-dop-study\"\nn_min = 3\nn_max = 4\nmap_n_relays = 3\nmap_n_lat = 2\nmap_n_lon = 2\nn_epochs = 1\n"
        }
        other => panic!("no source for {other}"),
    }
}

/// Assert `kind` is refused for `ent` with the neutral message, through the hook.
fn assert_refused_neutrally(ent: &Entitlement, kind: &str) {
    assert!(!ent.allows(kind, 1), "{kind}");
    let e = license::check(ent, kind, 1).unwrap_err().to_string();
    assert!(e.contains(NOT_IN_THIS_EDITION), "{kind}: {e}");
    assert!(!e.contains(kind), "the refusal names the kind: {e}");
    let source = if ARKSA_KINDS.contains(&kind) {
        src(kind).to_string()
    } else {
        format!("kind = \"{kind}\"\n")
    };
    let e = kshana_pro::registry::run_gated(&source, ent, 1)
        .unwrap_err()
        .to_string();
    assert!(e.contains(NOT_IN_THIS_EDITION), "{kind}: {e}");
    assert!(!e.contains(kind), "the refusal names the kind: {e}");
}

#[test]
fn placeholder_issuer_key_is_the_test_key() {
    assert_eq!(
        hex::encode(license::public_key_for(&ARKSA_TEST_SEED)),
        PLACEHOLDER_ARKSA_ISSUER_PUBKEY
    );
    // A debug build with no issuer key set trusts the placeholder.
    if option_env!("KSHANA_PRO_ARKSA_ISSUER_PUBKEY").is_none() && cfg!(debug_assertions) {
        assert_eq!(
            arksa_kpro::issuer_keys().unwrap(),
            vec![license::public_key_for(&ARKSA_TEST_SEED)]
        );
    }
}

#[test]
fn the_hook_adds_exactly_the_arksa_exclusive_ids_and_the_manifest_lists_them() {
    arksa_kpro::ensure_registered();
    let mut kinds = ARKSA_KINDS.to_vec();
    kinds.sort();
    assert_eq!(
        kinds,
        [
            "pro-beacon-dop",
            "pro-beacon-placement",
            "pro-cislunar-observability-study",
            "pro-design-optimise",
            "pro-lunar-signal-security",
            "pro-lunar-time-budget-study",
            "pro-mbse",
            "pro-optical-rf-hybrid-study",
            "pro-realtime-frame-eop-study",
            "pro-spectrum-coexistence",
            "pro-surface-beacon-dop-study",
        ]
    );
    for k in ARKSA_KINDS {
        assert!(kshana_pro::registry::is_pro_kind(k), "{k} is registered");
        assert_eq!(tiers::edition_of(k), Some(tiers::Edition::Arksa), "{k}");
        assert_eq!(
            tiers::compiled_edition(k),
            Some(tiers::Edition::Arksa),
            "{k}"
        );
        let id = kshana::registry::ScenarioId::from_static(k);
        assert!(kshana_pro::registry::pro_registry().contains(&id), "{k}");
    }
    for f in ARKSA_FEATURES {
        assert_eq!(tiers::edition_of(f), Some(tiers::Edition::Arksa), "{f}");
        assert!(license::pro_command_features().contains(f), "{f}");
    }
    // The manifest's hidden tier is exactly these ids (kinds and features, by hash).
    let m = tiers::manifest().unwrap();
    assert_eq!(
        m.count_in(tiers::Edition::Arksa),
        ARKSA_KINDS.len() + ARKSA_FEATURES.len()
    );
    tiers::startup_check().expect("the combined build agrees with the signed manifest");
    // `kinds --json` reports each at the arksa edition.
    let doc: serde_json::Value = serde_json::from_str(&kshana_pro::registry::kinds_json()).unwrap();
    for e in doc["pro"].as_array().unwrap() {
        let name = e["name"].as_str().unwrap();
        if ARKSA_KINDS.contains(&name) {
            assert_eq!(e["edition"], "arksa", "{name}");
        }
    }
}

#[test]
fn a_second_registration_is_harmless_and_another_extension_is_refused() {
    arksa_kpro::ensure_registered();
    arksa_kpro::register().expect("registering again is a no-op");
    let other = kshana_pro::extension::Extension {
        name: "other",
        ..Default::default()
    };
    assert!(kshana_pro::extension::register(other).is_err());
}

#[test]
fn a_standalone_pro_token_is_refused_for_each_arksa_exclusive_id() {
    arksa_kpro::ensure_registered();
    let ids: Vec<&str> = ARKSA_KINDS.iter().chain(ARKSA_FEATURES).copied().collect();
    let t = issue_v2(
        &PRO_TEST_SEED,
        &LicenseClaims {
            features: ids.iter().map(|s| s.to_string()).collect(),
            ..claims("enterprise")
        },
    )
    .unwrap();
    let standalone = verify_with(&ring(), &t, &at(1)).unwrap().entitlement;
    assert!(!standalone.arksa);
    assert_eq!(standalone.tier, Tier::Enterprise);
    for id in &ids {
        assert_refused_neutrally(&standalone, id);
        assert_refused_neutrally(&Entitlement::evaluation(), id);
    }
    // The command gates refuse it too, with the same message.
    let e = arksa_kpro::entitlement::ensure_mbse_entitled(&standalone, 1)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains(NOT_IN_THIS_EDITION) && !e.contains("mbse"),
        "{e}"
    );
    let e = arksa_kpro::mission::ensure_entitled(&standalone, 1)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains(NOT_IN_THIS_EDITION) && !e.contains("mission"),
        "{e}"
    );
    // Pro kinds are unchanged for the same token.
    assert!(standalone.allows("pro-twin", 1));
    assert!(standalone.allows("pro-batch", 1));
}

#[test]
fn a_pro_key_cannot_grant_the_claim() {
    let mut c = claims("enterprise");
    c.arksa = true;
    let forged = issue_v2(&PRO_TEST_SEED, &c).unwrap();
    assert!(verify_with(&ring(), &forged, &at(1)).is_err());
}

#[test]
fn an_arksa_entitled_token_runs_each_arksa_exclusive_kind_through_the_hook() {
    arksa_kpro::ensure_registered();
    let t = issue_arksa(&ARKSA_TEST_SEED, &claims("pro")).unwrap();
    let ent = verify_with(&ring(), &t, &at(1)).unwrap().entitlement;
    assert!(ent.arksa);
    for k in ARKSA_KINDS {
        assert!(ent.allows(k, 1), "{k}");
        let out = kshana_pro::registry::run_gated(src(k), &ent, 1)
            .unwrap_or_else(|e| panic!("{k} must run under an Arksa token: {e}"));
        assert!(!out.summary.is_empty(), "{k}");
        let v: serde_json::Value = serde_json::from_str(&out.json).unwrap();
        assert!(
            v["label"]
                .as_str()
                .is_some_and(|l| l.starts_with("MODELLED")),
            "{k}: the result is labelled MODELLED"
        );
    }
    for f in ARKSA_FEATURES {
        assert!(license::check(&ent, f, 1).is_ok(), "{f}");
    }
    assert!(arksa_kpro::entitlement::ensure_mbse_entitled(&ent, 1).is_ok());
    assert!(arksa_kpro::mission::ensure_entitled(&ent, 1).is_ok());
}

#[test]
fn an_expired_arksa_token_runs_nothing_exclusive() {
    arksa_kpro::ensure_registered();
    let t = issue_arksa(&ARKSA_TEST_SEED, &claims("pro")).unwrap();
    let mut o = at(20_000);
    o.allow_expired = true; // the grace path hands the caller an expired entitlement
    let ent = verify_with(&ring(), &t, &o).unwrap().entitlement;
    assert!(ent.arksa);
    for k in ARKSA_KINDS {
        assert!(!ent.allows(k, 20_000), "{k}");
        assert!(kshana_pro::registry::run_gated(src(k), &ent, 20_000).is_err());
    }
}

/// The server binary: Evaluation refuses an Arksa kind neutrally; `--dev` (debug builds)
/// runs it; the usage lists the Arksa commands.
#[test]
fn the_server_binary_registers_the_hook() {
    let bin = env!("CARGO_BIN_EXE_arksa-kpro");
    let dir = std::env::temp_dir().join(format!("arksa-kpro-bin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let scn = dir.join("lss.toml");
    std::fs::write(&scn, src("pro-lunar-signal-security")).unwrap();
    let home = dir.join("home");
    let run = |args: &[&str]| {
        std::process::Command::new(bin)
            .args(args)
            .env("KSHANA_PRO_HOME", &home)
            .output()
            .expect("the binary runs")
    };
    let out = run(&["run", scn.to_str().unwrap()]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(err.contains(NOT_IN_THIS_EDITION), "{err}");
    assert!(!err.contains("lunar"), "{err}");

    let out = run(&["run", scn.to_str().unwrap(), "--dev"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = run(&["kinds", "--json"]);
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = doc["pro"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    for k in ARKSA_KINDS {
        assert!(names.contains(k), "{k}");
    }

    let out = run(&["help"]);
    let usage = String::from_utf8_lossy(&out.stdout);
    for c in [
        "mbse",
        "design",
        "coexist",
        "mission",
        "navdt",
        "spacestream",
        "mrd",
    ] {
        assert!(usage.contains(&format!("arksa-kpro {c}")), "{c}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
