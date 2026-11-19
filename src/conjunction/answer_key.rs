// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The conjunction-results table of the US Office of Space Commerce TraCSS "Dataset for
//! Conjunction Assessment Verification" (CC0-1.0, published 18 March 2026), as its user's guide
//! documents it (Table 5, "CSieve Conjunction Results Output File Specifications"): one row per
//! conjunction with `run_id, conj_id, obj1, met_criteria1, obj2, met_criteria2, min_range`
//! (km), `Vrel` (km/s), `prob` (Pc, Alfano 2004), `dilution`, `mdistance` (Mahalanobis),
//! `epoch` (TCA, UTC), `jdate`, the J2000 states `x1 … vz1` and `x2 … vz2` (km, km/s), the
//! relative positions `local_x1 … local_z1` (object 2 from object 1 in object 1's UVW axes,
//! km) and `local_x2 …`, the UVW covariances `C1_…` and `C2_…` (upper triangle) and the source
//! file names. `NULL` means not computed.
//!
//! The same table is accepted for a provider's own results, so a provider can be scored in the
//! answer key's own format.
//!
//! Assumptions the guide does not state, recorded in every result that uses this reader:
//! the covariance columns are the upper triangle row by row (21 columns for 6x6, or 6 for the
//! position block), in km-based units (km², km²/s, km²/s²) unless the scenario says `m2`;
//! columns are matched by name, case-insensitively, in any order. The dataset itself is not
//! committed to this repository (its files are 20 GB of ephemerides and 60 to 200 MB of
//! compressed results, available on request from the publisher); tests read a local copy when
//! one is named.

use std::io::BufRead;

use kshana::api::KshanaError;

/// A filter on the normalised pair `(obj1, obj2)`.
pub type PairFilter<'a> = &'a dyn Fn(&str, &str) -> bool;

use super::{bad, jd_to_s, norm_id, parse_utc, Event, M6};

/// The assumptions stated with every result read with this module.
pub const ASSUMPTIONS: &str = "Results-table covariance columns (C1_*, C2_*) read as the upper \
triangle, row by row, of the UVW (RTN) covariance; units km-based unless stated (the user's guide \
gives the column list, not the order or units; unverified against the dataset).";

/// Covariance units of the `C1_`/`C2_` columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CovUnit {
    Km2,
    M2,
}

struct Cols {
    obj1: usize,
    obj2: usize,
    min_range: usize,
    conj_id: Option<usize>,
    vrel: Option<usize>,
    prob: Option<usize>,
    mdist: Option<usize>,
    epoch: Option<usize>,
    jdate: Option<usize>,
    met1: Option<usize>,
    met2: Option<usize>,
    s1: Option<[usize; 6]>,
    s2: Option<[usize; 6]>,
    l1: Option<[usize; 3]>,
    c1: Vec<usize>,
    c2: Vec<usize>,
    f1: Option<usize>,
    f2: Option<usize>,
}

fn cols(header: &[String]) -> Result<Cols, KshanaError> {
    let find = |n: &str| header.iter().position(|h| h.eq_ignore_ascii_case(n));
    let need = |n: &str| find(n).ok_or_else(|| bad(format!("results table has no '{n}' column")));
    let six = |suffix: &str| -> Option<[usize; 6]> {
        let mut o = [0; 6];
        for (i, b) in ["x", "y", "z", "vx", "vy", "vz"].iter().enumerate() {
            o[i] = find(&format!("{b}{suffix}"))?;
        }
        Some(o)
    };
    let three = |suffix: &str| -> Option<[usize; 3]> {
        let mut o = [0; 3];
        for (i, b) in ["local_x", "local_y", "local_z"].iter().enumerate() {
            o[i] = find(&format!("{b}{suffix}"))?;
        }
        Some(o)
    };
    let cov = |p: &str| -> Vec<usize> {
        header
            .iter()
            .enumerate()
            .filter(|(_, h)| h.len() > 3 && h[..3].eq_ignore_ascii_case(p))
            .map(|(i, _)| i)
            .collect()
    };
    if find("epoch").is_none() && find("jdate").is_none() {
        return Err(bad("results table has neither 'epoch' nor 'jdate'"));
    }
    Ok(Cols {
        obj1: need("obj1")?,
        obj2: need("obj2")?,
        min_range: need("min_range")?,
        conj_id: find("conj_id"),
        vrel: find("vrel"),
        prob: find("prob"),
        mdist: find("mdistance"),
        epoch: find("epoch"),
        jdate: find("jdate"),
        met1: find("met_criteria1"),
        met2: find("met_criteria2"),
        s1: six("1"),
        s2: six("2"),
        l1: three("1"),
        c1: cov("C1_"),
        c2: cov("C2_"),
        f1: find("obj1_file"),
        f2: find("obj2_file"),
    })
}

fn split(line: &str) -> Vec<String> {
    line.split(',')
        .map(|f| f.trim().trim_matches('"').to_string())
        .collect()
}

fn num(f: &[String], i: Option<usize>) -> Result<Option<f64>, KshanaError> {
    match i.and_then(|i| f.get(i)) {
        None => Ok(None),
        Some(s)
            if s.is_empty() || s.eq_ignore_ascii_case("NULL") || s.eq_ignore_ascii_case("NaN") =>
        {
            Ok(None)
        }
        Some(s) => s
            .parse::<f64>()
            .map(Some)
            .map_err(|_| bad(format!("'{s}' is not a number"))),
    }
}

fn upper_tri(f: &[String], idx: &[usize], k: f64) -> Result<Option<M6>, KshanaError> {
    let n = match idx.len() {
        21 => 6,
        6 => 3,
        _ => return Ok(None),
    };
    let mut c = [[0.0; 6]; 6];
    let mut p = 0;
    for i in 0..n {
        for j in i..n {
            match num(f, Some(idx[p]))? {
                Some(v) => {
                    c[i][j] = v * k;
                    c[j][i] = v * k;
                }
                None => return Ok(None),
            }
            p += 1;
        }
    }
    Ok(Some(c))
}

/// Read a results table. `keep`: an optional filter on the (normalised) pair, applied before
/// a row is kept (to score a subset of a large table).
pub fn read<R: BufRead>(
    r: R,
    unit: CovUnit,
    keep: Option<PairFilter<'_>>,
) -> Result<Vec<Event>, KshanaError> {
    let k = match unit {
        CovUnit::Km2 => 1e6,
        CovUnit::M2 => 1.0,
    };
    let mut lines = r.lines();
    let header = loop {
        match lines.next() {
            None => return Err(bad("results table is empty")),
            Some(l) => {
                let l = l.map_err(|e| bad(e.to_string()))?;
                if !l.trim().is_empty() && !l.starts_with('#') {
                    break split(&l);
                }
            }
        }
    };
    let c = cols(&header)?;
    let mut out = Vec::new();
    for (n, l) in lines.enumerate() {
        let l = l.map_err(|e| bad(e.to_string()))?;
        if l.trim().is_empty() || l.starts_with('#') {
            continue;
        }
        let f = split(&l);
        let row = || format!("results table row {}", n + 2);
        let o1 = norm_id(f.get(c.obj1).ok_or_else(|| bad(row()))?);
        let o2 = norm_id(f.get(c.obj2).ok_or_else(|| bad(row()))?);
        if let Some(keep) = keep {
            if !keep(&o1, &o2) {
                continue;
            }
        }
        let tca_s = match (c.epoch.and_then(|i| f.get(i)), num(&f, c.jdate)?) {
            (Some(s), _) if !s.is_empty() => parse_utc(s)?,
            (_, Some(jd)) => jd_to_s(jd),
            _ => return Err(bad(format!("{}: no TCA", row()))),
        };
        let state = |s: Option<[usize; 6]>| -> Result<Option<[f64; 6]>, KshanaError> {
            let Some(s) = s else { return Ok(None) };
            let mut o = [0.0; 6];
            for (i, &col) in s.iter().enumerate() {
                match num(&f, Some(col))? {
                    Some(v) => o[i] = v * 1000.0,
                    None => return Ok(None),
                }
            }
            Ok(Some(o))
        };
        let mut e = Event {
            id: c
                .conj_id
                .and_then(|i| f.get(i).cloned())
                .unwrap_or_else(|| format!("row{}", n + 2)),
            obj1: o1,
            obj2: o2,
            tca_s,
            miss_m: num(&f, Some(c.min_range))?
                .ok_or_else(|| bad(format!("{}: no min_range", row())))?
                * 1000.0,
            vrel_m_s: num(&f, c.vrel)?.map(|v| v * 1000.0),
            pc: num(&f, c.prob)?,
            pc_method: c
                .prob
                .map(|_| "ALFANO-2004 (as the user's guide states)".to_string()),
            mahalanobis: num(&f, c.mdist)?,
            state1: state(c.s1)?,
            state2: state(c.s2)?,
            frame: c.s1.map(|_| "EME2000".to_string()),
            cov1_rtn: upper_tri(&f, &c.c1, k)?,
            cov2_rtn: upper_tri(&f, &c.c2, k)?,
            ..Default::default()
        };
        if let Some(l1) = c.l1 {
            if let (Some(a), Some(b), Some(d)) = (
                num(&f, Some(l1[0]))?,
                num(&f, Some(l1[1]))?,
                num(&f, Some(l1[2]))?,
            ) {
                e.rel_rtn_m = Some([a * 1000.0, b * 1000.0, d * 1000.0]);
            }
        }
        if let (Some(a), Some(b)) = (num(&f, c.met1)?, num(&f, c.met2)?) {
            e.met_criteria = Some((a != 0.0, b != 0.0));
        }
        if let (Some(a), Some(b)) = (c.f1.and_then(|i| f.get(i)), c.f2.and_then(|i| f.get(i))) {
            e.files = Some((a.clone(), b.clone()));
        }
        out.push(e);
    }
    Ok(out)
}

/// Whether a text looks like a results table (a header line naming `obj1` and `min_range`).
pub fn looks_like(text: &str) -> bool {
    text.lines()
        .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let h = l.to_ascii_lowercase();
            h.contains("obj1") && h.contains("min_range")
        })
        .unwrap_or(false)
}

/// The header of the table as this module writes it (for provider results and fixtures).
pub fn header() -> String {
    let mut h: Vec<String> = [
        "run_id",
        "conj_id",
        "obj1",
        "met_criteria1",
        "obj2",
        "met_criteria2",
        "min_range",
        "Vrel",
        "prob",
        "dilution",
        "mdistance",
        "epoch",
        "jdate",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for o in ["1", "2"] {
        for b in ["x", "y", "z", "vx", "vy", "vz"] {
            h.push(format!("{b}{o}"));
        }
        for b in ["local_x", "local_y", "local_z"] {
            h.push(format!("{b}{o}"));
        }
        for i in 0..6 {
            for j in i..6 {
                h.push(format!(
                    "C{o}_{}{}",
                    "uvwUVW".as_bytes()[i] as char,
                    "uvwUVW".as_bytes()[j] as char
                ));
            }
        }
    }
    h.push("obj1_file".into());
    h.push("obj2_file".into());
    h.join(",")
}

/// One row in the format of [`header`] (km-based units). Fields the event lacks are `NULL`.
pub fn row(e: &Event, run_id: u32) -> String {
    let n = |x: Option<f64>| {
        x.map(|v| format!("{v:.12e}"))
            .unwrap_or_else(|| "NULL".into())
    };
    let mut f: Vec<String> = vec![
        run_id.to_string(),
        e.id.clone(),
        e.obj1.clone(),
        e.met_criteria
            .map(|m| (m.0 as u8).to_string())
            .unwrap_or_else(|| "1".into()),
        e.obj2.clone(),
        e.met_criteria
            .map(|m| (m.1 as u8).to_string())
            .unwrap_or_else(|| "1".into()),
        n(Some(e.miss_m / 1000.0)),
        n(e.vrel_m_s.map(|v| v / 1000.0)),
        n(e.pc),
        "NULL".into(),
        n(e.mahalanobis),
        super::fmt_utc(e.tca_s)
            .trim_end_matches('Z')
            .replace('T', " "),
        n(Some(e.tca_s / 86_400.0 + 2_451_544.5)),
    ];
    let rel2 = e.rel_rtn_m;
    for (st, rel, cov) in [(e.state1, rel2, e.cov1_rtn), (e.state2, None, e.cov2_rtn)] {
        for i in 0..6 {
            f.push(n(st.map(|s| s[i] / 1000.0)));
        }
        for i in 0..3 {
            f.push(n(rel.map(|r| r[i] / 1000.0)));
        }
        for i in 0..6 {
            for j in i..6 {
                f.push(n(cov.map(|c| c[i][j] / 1e6)));
            }
        }
    }
    let (a, b) = e.files.clone().unwrap_or_default();
    f.push(a);
    f.push(b);
    f.join(",")
}
