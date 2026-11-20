// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # Rideshare and LEOP object identification ("which object is mine?")
//!
//! Research 06 G3 and 06b finding 4: after a rideshare, dozens of objects are catalogued with
//! near-identical element sets, and an operator can spend days to weeks working out which one
//! is its own satellite; satellites have been lost because the operator was talking to the
//! wrong object. In November 2026 the five-digit catalogue number ran out, so new objects carry
//! six-digit numbers, written in a TLE only as Alpha-5 and in full only in OMM.
//!
//! This module matches the operator's observed passes (Doppler, range, range rate or angles
//! from its ground-station logs, [`observations`]) against the candidate element sets of one
//! launch (TLE with Alpha-5, or OMM in KVN, XML or JSON, [`catalogue`]), ranks the candidates
//! by how well each explains the observations and returns a verdict with the evidence used
//! ([`identify`]): residual statistics per candidate, the separation between the best and the
//! second-best candidate, and whether the data could have told them apart at all. An
//! ambiguous result is reported as ambiguous, never as a guess.
//!
//! * [`norad`]: catalogue numbers above 99,999 (Alpha-5 encode/decode, OMM `NORAD_CAT_ID`).
//!   A local helper, to be replaced by `kshana::norad` once a kshana release contains it.
//! * [`catalogue`]: candidate element sets from TLE and OMM, SGP4 through the open engine.
//! * [`observations`]: the documented observation CSV.
//! * [`identify`]: predicted measurements, residuals, ranking, verdict.
//! * [`synthetic`]: a synthetic multi-object deployment with noisy observations of one object,
//!   for tests and demonstrations.
//! * [`cli`]: `arksa-kpro rideshare-id`.
//!
//! The pack `pro-rideshare-id` ([`crate::packs::rideshare_id_pack`]) runs the same study from a
//! scenario file and writes the signed verdict directory. Evidence class: MODELLED.

pub mod catalogue;
pub mod cli;
pub mod identify;
pub mod norad;
pub mod observations;
pub mod synthetic;

use kshana::api::KshanaError;
use kshana::jd2::Jd2;
use kshana_pro::license::Entitlement;
use serde_json::{json, Value};

/// The kind id of the scenario pack.
pub const KIND: &str = "pro-rideshare-id";

/// Schema of the verdict document.
pub const SCHEMA: &str = "arksa-rideshare-id/1";

/// The command feature: the mission feature (`pro-mission-dossier`, Arksa tier), as for the
/// other operations commands (`ops`, `simcal`, `landing-nav`), so the command needs the Arksa
/// entitlement and works with the signed tier manifest as it stands.
pub const FEATURE: &str = crate::mission::FEATURE;

/// The evidence label carried by every verdict.
pub const LABEL: &str = "MODELLED rideshare object identification: the candidate orbits are \
propagated with SGP4 from the element sets given; the observations are the operator's own; \
the probabilities assume Gaussian measurement errors (scaled by the best fit's variance factor) \
and that the operator's object is one of the candidates. Not an identification by any \
catalogue authority. MEASURED: only the observations supplied.";

/// Sources the method and formats rest on.
pub const REFERENCES: &[&str] = &[
    "CCSDS 502.0-B-3, Orbit Data Messages (OMM: NORAD_CAT_ID is an integer of up to nine digits)",
    "Space-Track.org documentation: Alpha-5 catalogue numbers in TLE/3LE (letters A-Z without I and O for the leading two digits, 100000 to 339999) as a stop-gap; OMM recommended beyond",
    "Hoots and Roehrich, Spacetrack Report No. 3 (1980); Vallado et al., Revisiting Spacetrack Report #3, AIAA 2006-6753 (SGP4 as implemented in the open engine)",
];

/// The command and kind refuse without the Arksa entitlement.
pub fn ensure_entitled(ent: &Entitlement, now_unix: i64) -> Result<(), KshanaError> {
    crate::license::check(ent, FEATURE, now_unix)
}

pub(crate) fn bad(m: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(format!("{KIND}: {}", m.into()))
}

/// Parse `YYYY-MM-DDThh:mm:ss[.fff][Z]` (UTC) into a two-part Julian date.
pub fn parse_utc(s: &str) -> Result<Jd2, KshanaError> {
    let t = s.trim().trim_end_matches('Z');
    let err = || {
        bad(format!(
            "'{s}' is not a UTC time (YYYY-MM-DDThh:mm:ss[.f]Z)"
        ))
    };
    let (d, hms) = t.split_once('T').ok_or_else(err)?;
    let dp: Vec<&str> = d.split('-').collect();
    let hp: Vec<&str> = hms.split(':').collect();
    if dp.len() != 3 || hp.len() != 3 {
        return Err(err());
    }
    let int = |x: &str| x.parse::<i64>().map_err(|_| err());
    let (y, mo, da) = (int(dp[0])?, int(dp[1])?, int(dp[2])?);
    let (h, mi) = (int(hp[0])?, int(hp[1])?);
    let se: f64 = hp[2].parse().map_err(|_| err())?;
    if !(1960..=2200).contains(&y) || mo < 0 || da < 0 || h < 0 || mi < 0 {
        return Err(err());
    }
    Jd2::from_utc_calendar(y as i32, mo as u32, da as u32, h as u32, mi as u32, se)
        .map_err(|e| bad(format!("'{s}': {e}")))
}

/// Format a UTC two-part Julian date as `YYYY-MM-DDThh:mm:ss.fffZ` (milliseconds).
pub fn fmt_utc(t: Jd2) -> String {
    let ms = (t.diff_seconds(Jd2::new(2_440_587.5)) * 1000.0).round() as i64;
    let secs = ms.div_euclid(1000);
    let msr = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{msr:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// Civil date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
pub(crate) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Significant digits kept in the published verdict.
pub const SIG_DIGITS: usize = 9;

/// `x` rounded to [`SIG_DIGITS`] significant digits, so the verdict and its hash are the same
/// on every platform (SGP4 and the trigonometry differ in the last bits between maths
/// libraries).
pub fn round_sig(x: f64) -> f64 {
    if x == 0.0 {
        return 0.0;
    }
    format!("{:.*e}", SIG_DIGITS - 1, x).parse().unwrap_or(x)
}

/// NaN and infinities become null; every other non-integer number is rounded with
/// [`round_sig`].
pub fn sanitise(v: &mut Value) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(sanitise),
        Value::Object(o) => o.values_mut().for_each(sanitise),
        Value::Number(n) if n.is_f64() => match n.as_f64() {
            Some(x) if x.is_finite() => *v = json!(round_sig(x)),
            _ => *v = Value::Null,
        },
        _ => {}
    }
}

/// SHA-256 of bytes, hex.
pub fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(b))
}
