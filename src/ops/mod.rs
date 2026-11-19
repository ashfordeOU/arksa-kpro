// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! # Operations assurance
//!
//! Three signed products for the operate phase of a mission:
//!
//! * [`test_report`]: an operations validation report generated from a test-run log
//!   (procedure steps, telecommands and their verification, expected against observed
//!   telemetry), in the terms of ECSS-E-ST-70-32C (preconditions, main body, confirmation,
//!   steps, confirmation status). ESA's own words in tender 1-12979: "test reports are
//!   typically written manually".
//! * [`ad_scorecard`]: a scorecard for telemetry anomaly detectors against a public
//!   benchmark's annotations (the ESA Anomalies Dataset of ESA-ADB, interval labels; the
//!   OPSSAT-AD dataset, segment labels), with pre-registered bars and an operator
//!   false-alarm budget. The data is never committed here; it is fetched on demand
//!   (`scripts/fetch_ad_benchmarks.py`).
//! * [`tracking_gap`]: what a ground-station schedule loses to station outages (contact
//!   time, downlinked data, recorder overflow, gaps beyond a mission's limit, navigation
//!   passes, critical windows uncovered) and how much other stations' windows recover.
//!
//! All three run as `arksa-kpro ops report|ad-score|tracking-gap`, under the existing
//! `pro-mission-dossier` feature (an Arksa-tier feature, so a token with the `arksa`
//! claim). Each writes a directory that can be signed as a whole with Kshana Pro's dossier
//! signature. See docs/OPS-ASSURANCE.md.

pub mod ad_scorecard;
pub mod cli;
pub mod csv;
pub mod test_report;
pub mod tracking_gap;

use std::path::{Path, PathBuf};

use kshana::api::KshanaError;

pub use crate::decision::time::{fmt_utc, parse_utc};

/// The command feature the `ops` commands require: the mission dossier's.
pub const FEATURE: &str = crate::mission::FEATURE;

pub(crate) fn err(msg: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(msg.into())
}

/// Parse a UTC stamp, naming the field in the error.
pub(crate) fn utc(field: &str, s: &str) -> Result<f64, KshanaError> {
    parse_utc(s).map_err(|e| err(format!("{field}: {e}")))
}

/// Unix seconds (possibly fractional) as `YYYY-MM-DDTHH:MM:SS[.fff]Z`, keeping
/// milliseconds when there are any.
pub fn fmt_utc_ms(t: f64) -> String {
    let ms = (t * 1000.0).round() as i64;
    let whole = fmt_utc(ms.div_euclid(1000));
    match ms.rem_euclid(1000) {
        0 => whole,
        r => format!("{}.{r:03}Z", whole.trim_end_matches('Z')),
    }
}

/// Read a file whose path is relative to `base` (or absolute).
pub(crate) fn read_rel(base: &Path, rel: &str) -> Result<(PathBuf, Vec<u8>), KshanaError> {
    let p = if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        base.join(rel)
    };
    let bytes = std::fs::read(&p).map_err(|e| err(format!("{}: {e}", p.display())))?;
    Ok((p, bytes))
}

pub(crate) fn utf8(name: &str, bytes: &[u8]) -> Result<String, KshanaError> {
    String::from_utf8(bytes.to_vec()).map_err(|_| err(format!("{name} is not UTF-8")))
}

/// Round to `d` decimals (for stable, readable JSON).
pub(crate) fn round(x: f64, d: i32) -> f64 {
    let f = 10f64.powi(d);
    (x * f).round() / f
}

/// A signer: seed, name, UTC stamp of the signature.
pub type Signer<'a> = (&'a [u8; 32], &'a str, &'a str);

/// Write `files` into `dir`; with a signer, sign the directory as a whole
/// (`signature.json`, Kshana Pro's dossier signature, checked with
/// `arksa-kpro dossier verify`). Refuses a directory that already holds other files, so a
/// signature never covers stale outputs.
pub fn write_dir(
    dir: &Path,
    files: &[(&str, Vec<u8>)],
    signer: Option<Signer>,
) -> Result<Option<PathBuf>, KshanaError> {
    if dir.exists() {
        let names: Vec<String> = std::fs::read_dir(dir)
            .map_err(|e| err(format!("{}: {e}", dir.display())))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !files.iter().any(|(f, _)| f == n) && n != "signature.json")
            .collect();
        if !names.is_empty() {
            return Err(err(format!(
                "{} already holds other files ({}); give an empty or new directory",
                dir.display(),
                names.join(", ")
            )));
        }
        let _ = std::fs::remove_file(dir.join("signature.json"));
    }
    std::fs::create_dir_all(dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
    for (name, bytes) in files {
        std::fs::write(dir.join(name), bytes).map_err(|e| err(format!("{name}: {e}")))?;
    }
    match signer {
        Some((seed, name, utc)) => Ok(Some(kshana_pro::signed_dossier::sign(
            dir, seed, name, utc,
        )?)),
        None => Ok(None),
    }
}

/// Canonical SHA-256 of a serialisable body (the same canonical JSON as the dossier).
pub(crate) fn body_sha256<T: serde::Serialize>(body: &T) -> Result<String, KshanaError> {
    Ok(crate::mbse::evidence::sha256_hex(
        crate::mission::compose::canonical_json(body)?.as_bytes(),
    ))
}

/// Recompute the body hash of a written product (`report.json`, `scorecard.json`,
/// `impact.json`): the value under `hash_key` must equal the canonical SHA-256 of `body`.
pub fn verify_body_hash(doc: &serde_json::Value, hash_key: &str) -> Result<(), String> {
    let body = doc.get("body").ok_or("no body")?;
    let recorded = doc
        .get(hash_key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("no {hash_key}"))?;
    let actual = body_sha256(body).map_err(|e| e.to_string())?;
    if actual == recorded {
        Ok(())
    } else {
        Err(format!(
            "the body hashes to {actual}, but {hash_key} says {recorded}"
        ))
    }
}

/// Union length (seconds) of intervals.
pub(crate) fn union_len(iv: &[(f64, f64)]) -> f64 {
    merge(iv).iter().map(|(a, b)| b - a).sum()
}

/// Merge intervals into a sorted, disjoint list (touching intervals join).
pub(crate) fn merge(iv: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = iv.iter().copied().filter(|(a, b)| b > a).collect();
    v.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Clip intervals to `[lo, hi]`.
pub(crate) fn clip(iv: &[(f64, f64)], lo: f64, hi: f64) -> Vec<(f64, f64)> {
    iv.iter()
        .map(|&(a, b)| (a.max(lo), b.min(hi)))
        .filter(|(a, b)| b > a)
        .collect()
}

/// `a` minus the union of `cut`, as disjoint pieces.
pub(crate) fn subtract(a: (f64, f64), cut: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut pieces = vec![a];
    for &(c0, c1) in &merge(cut) {
        let mut next = Vec::new();
        for (p0, p1) in pieces {
            if c1 <= p0 || c0 >= p1 {
                next.push((p0, p1));
                continue;
            }
            if c0 > p0 {
                next.push((p0, c0));
            }
            if c1 < p1 {
                next.push((c1, p1));
            }
        }
        pieces = next;
    }
    pieces
}

/// Percentile (nearest-rank, `p` in 0..=100) of unsorted values; `None` when empty.
pub(crate) fn percentile(v: &[f64], p: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * s.len() as f64).ceil().max(1.0) as usize;
    Some(s[rank.min(s.len()) - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_helpers() {
        assert_eq!(
            merge(&[(3.0, 5.0), (0.0, 1.0), (1.0, 2.0)]),
            vec![(0.0, 2.0), (3.0, 5.0)]
        );
        assert_eq!(union_len(&[(0.0, 10.0), (5.0, 12.0), (20.0, 21.0)]), 13.0);
        assert_eq!(
            subtract((0.0, 10.0), &[(2.0, 3.0), (8.0, 12.0)]),
            vec![(0.0, 2.0), (3.0, 8.0)]
        );
        assert_eq!(subtract((0.0, 10.0), &[(-1.0, 11.0)]), vec![]);
        assert_eq!(
            clip(&[(-5.0, 5.0), (20.0, 30.0)], 0.0, 10.0),
            vec![(0.0, 5.0)]
        );
        assert_eq!(percentile(&[5.0, 1.0, 3.0, 2.0, 4.0], 50.0), Some(3.0));
        assert_eq!(percentile(&[5.0, 1.0, 3.0, 2.0, 4.0], 90.0), Some(5.0));
        assert_eq!(fmt_utc_ms(1.5), "1970-01-01T00:00:01.500Z");
        assert_eq!(fmt_utc_ms(60.0), "1970-01-01T00:01:00Z");
    }
}
