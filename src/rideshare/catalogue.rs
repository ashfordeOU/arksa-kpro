// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Candidate element sets of one launch, from TLE (two- or three-line, five-digit or Alpha-5
//! catalogue field) and from OMM (CCSDS 502.0 KVN, NDM/XML, or the JSON array that catalogue
//! services publish, whose keys are the KVN keywords). Every candidate becomes an SGP4 orbit on
//! the open engine's one propagation and frame path ([`kshana::sgp4::SgpOrbit`]).
//!
//! A catalogue number above 339,999 cannot be written in a TLE; such an object can come only
//! from OMM, and [`Candidate::tle_field`] is then `None`.

use kshana::jd2::Jd2;
use kshana::odm::OmmMessage;
use kshana::sgp4::{MeanElementSet, SgpOrbit};
use kshana::tle::Tle;
use serde::Serialize;

use super::norad;

/// One candidate object.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Catalogue number (no five-digit limit).
    pub catalogue_number: u32,
    /// The number as a five-column TLE field (digits or Alpha-5); `None` above 339,999.
    pub tle_field: Option<String>,
    /// Object name (the 3LE name line or `OBJECT_NAME`).
    pub name: String,
    /// International designator (`OBJECT_ID`), when given.
    pub object_id: String,
    /// `tle`, `omm-kvn`, `omm-xml` or `omm-json`.
    pub source: &'static str,
    /// The input it came from (file name or `inline`).
    pub origin: String,
    /// Element epoch (UTC).
    pub epoch: Jd2,
    /// Whether both TLE line checksums matched (always true for OMM).
    pub checksum_ok: bool,
    /// SGP4 elements.
    pub elements: Tle,
    /// The orbit.
    pub orbit: SgpOrbit,
}

/// The public description of a candidate.
#[derive(Debug, Clone, Serialize)]
pub struct CandidateInfo {
    pub catalogue_number: u32,
    pub tle_field: Option<String>,
    pub alpha5: bool,
    pub name: String,
    pub object_id: String,
    pub source: &'static str,
    pub origin: String,
    pub epoch_utc: String,
    pub checksum_ok: bool,
    pub mean_motion_rev_day: f64,
    pub inclination_deg: f64,
    pub eccentricity: f64,
}

impl Candidate {
    /// Build a candidate from SGP4 elements.
    pub fn new(
        catalogue_number: u32,
        name: String,
        object_id: String,
        source: &'static str,
        origin: String,
        checksum_ok: bool,
        elements: Tle,
    ) -> Self {
        let epoch = Jd2::from_parts(2_433_281.0, 0.5 + elements.epoch_days_1950);
        let orbit = SgpOrbit::new(MeanElementSet {
            epoch_utc: epoch,
            no_kozai: elements.no_kozai_rad_min,
            ecco: elements.ecco,
            inclo: elements.inclo_rad,
            nodeo: elements.nodeo_rad,
            argpo: elements.argpo_rad,
            mo: elements.mo_rad,
            bstar: elements.bstar,
        });
        Self {
            catalogue_number,
            tle_field: norad::encode(catalogue_number).ok(),
            name,
            object_id,
            source,
            origin,
            epoch,
            checksum_ok,
            elements,
            orbit,
        }
    }

    /// The public description.
    pub fn info(&self) -> CandidateInfo {
        CandidateInfo {
            catalogue_number: self.catalogue_number,
            tle_field: self.tle_field.clone(),
            alpha5: self.catalogue_number > norad::FIVE_DIGIT_MAX
                && self.catalogue_number <= norad::ALPHA5_MAX,
            name: self.name.clone(),
            object_id: self.object_id.clone(),
            source: self.source,
            origin: self.origin.clone(),
            epoch_utc: super::fmt_utc(self.epoch),
            checksum_ok: self.checksum_ok,
            mean_motion_rev_day: self.elements.no_kozai_rad_min * 1440.0 / std::f64::consts::TAU,
            inclination_deg: self.elements.inclo_rad.to_degrees(),
            eccentricity: self.elements.ecco,
        }
    }
}

/// Candidates from a block of TLEs (two- or three-line; a `0 ` prefix on a name line is
/// dropped). Line 1 and line 2 must carry the same catalogue number; a checksum mismatch is
/// recorded on the candidate, not refused (synthetic and hand-edited sets often carry none).
pub fn from_tle_text(text: &str, origin: &str) -> Result<Vec<Candidate>, String> {
    let mut out = Vec::new();
    let mut name: Option<String> = None;
    let mut l1: Option<String> = None;
    for (no, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with("1 ") && t.len() >= 63 {
            l1 = Some(t.to_string());
        } else if t.starts_with("2 ") && t.len() >= 63 {
            let a = l1
                .take()
                .ok_or_else(|| format!("{origin}:{}: line 2 without a line 1", no + 1))?;
            let id = norad::parse_tle_identity(&a).map_err(|e| format!("{origin}: {e}"))?;
            let n2 = norad::decode(&t[2..7]).map_err(|e| format!("{origin}: line 2: {e}"))?;
            if n2 != id.norad_cat_id {
                return Err(format!(
                    "{origin}:{}: line 1 is object {} but line 2 is object {n2}",
                    no + 1,
                    id.norad_cat_id
                ));
            }
            let el = kshana::tle::parse_tle(&a, t).map_err(|e| format!("{origin}: {e}"))?;
            let ok = kshana::tle::verify_checksum(&a, "line 1").is_ok()
                && kshana::tle::verify_checksum(t, "line 2").is_ok();
            let nm = name
                .take()
                .unwrap_or_else(|| format!("OBJECT {}", id.norad_cat_id));
            out.push(Candidate::new(
                id.norad_cat_id,
                nm,
                id.intl_designator,
                "tle",
                origin.to_string(),
                ok,
                el,
            ));
        } else {
            let nm = t.strip_prefix("0 ").unwrap_or(t).trim();
            name = Some(nm.to_string());
            l1 = None;
        }
    }
    if out.is_empty() {
        return Err(format!("{origin}: no TLE element set found"));
    }
    Ok(out)
}

/// The OMM keywords carried over from a JSON object, in KVN order.
const OMM_HEADER_KEYS: &[&str] = &["CCSDS_OMM_VERS", "CREATION_DATE", "ORIGINATOR"];
const OMM_BODY_KEYS: &[&str] = &[
    "OBJECT_NAME",
    "OBJECT_ID",
    "CENTER_NAME",
    "REF_FRAME",
    "TIME_SYSTEM",
    "MEAN_ELEMENT_THEORY",
    "EPOCH",
    "MEAN_MOTION",
    "ECCENTRICITY",
    "INCLINATION",
    "RA_OF_ASC_NODE",
    "ARG_OF_PERICENTER",
    "MEAN_ANOMALY",
    "EPHEMERIS_TYPE",
    "CLASSIFICATION_TYPE",
    "NORAD_CAT_ID",
    "ELEMENT_SET_NO",
    "REV_AT_EPOCH",
    "BSTAR",
    "MEAN_MOTION_DOT",
    "MEAN_MOTION_DDOT",
];

/// One OMM in KVN from a JSON object with KVN keywords as keys (other keys, such as a
/// catalogue service's `DECAY_DATE` or `TLE_LINE1`, are ignored; missing header keys get
/// neutral values).
fn json_to_kvn(o: &serde_json::Map<String, serde_json::Value>) -> String {
    let get = |k: &str| -> Option<String> {
        match o.get(k) {
            Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Some(s.trim().into()),
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
    };
    let mut s = String::new();
    for (k, default) in OMM_HEADER_KEYS
        .iter()
        .zip(["2.0", "1970-01-01T00:00:00", "UNKNOWN"])
    {
        let v = get(k).unwrap_or_else(|| default.to_string());
        s.push_str(&format!("{k} = {v}\n"));
    }
    for k in OMM_BODY_KEYS {
        if let Some(v) = get(k) {
            let v = if *k == "EPOCH" {
                v.trim_end_matches('Z').to_string()
            } else {
                v
            };
            s.push_str(&format!("{k} = {v}\n"));
        }
    }
    s
}

/// The single messages in an OMM text: KVN messages split at `CCSDS_OMM_VERS`, XML `<omm>`
/// elements (bare or inside an NDM combined-instantiation `<ndm>`), or JSON objects.
fn split_omm(text: &str) -> Result<Vec<(String, &'static str)>, String> {
    let t = text.trim_start_matches('\u{feff}').trim();
    if t.starts_with('[') || t.starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(t).map_err(|e| format!("OMM JSON: {e}"))?;
        let items = match v {
            serde_json::Value::Array(a) => a,
            o => vec![o],
        };
        return items
            .iter()
            .map(|i| {
                i.as_object()
                    .map(|o| (json_to_kvn(o), "omm-json"))
                    .ok_or_else(|| "OMM JSON: every item must be an object".to_string())
            })
            .collect();
    }
    if t.starts_with('<') {
        let mut out = Vec::new();
        let mut rest = t;
        while let Some(start) = find_tag(rest, "<omm") {
            let after = &rest[start..];
            let end = after
                .find("</omm>")
                .ok_or_else(|| "OMM XML: <omm> without </omm>".to_string())?;
            let body = &after[..end + "</omm>".len()];
            out.push((
                format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{body}\n"),
                "omm-xml",
            ));
            rest = &after[end + "</omm>".len()..];
        }
        if out.is_empty() {
            return Err("OMM XML: no <omm> element".into());
        }
        return Ok(out);
    }
    let mut out: Vec<String> = Vec::new();
    for line in t.lines() {
        if line.trim_start().starts_with("CCSDS_OMM_VERS") {
            out.push(String::new());
        }
        match out.last_mut() {
            Some(m) => {
                m.push_str(line);
                m.push('\n');
            }
            None if line.trim().is_empty() || line.trim_start().starts_with("COMMENT") => {}
            None => return Err("OMM KVN: text before the first CCSDS_OMM_VERS".into()),
        }
    }
    if out.is_empty() {
        return Err("OMM: no message found".into());
    }
    Ok(out.into_iter().map(|m| (m, "omm-kvn")).collect())
}

/// The position of an opening tag `<name` followed by whitespace, `>` or `/`.
fn find_tag(s: &str, open: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = s[from..].find(open) {
        let at = from + i;
        match s[at + open.len()..].chars().next() {
            Some(c) if c.is_whitespace() || c == '>' || c == '/' => return Some(at),
            _ => from = at + open.len(),
        }
    }
    None
}

/// Candidates from an OMM text (KVN, XML or JSON; one or many messages).
pub fn from_omm_text(text: &str, origin: &str) -> Result<Vec<Candidate>, String> {
    let mut out = Vec::new();
    for (k, (msg, source)) in split_omm(text)
        .map_err(|e| format!("{origin}: {e}"))?
        .into_iter()
        .enumerate()
    {
        let m = OmmMessage::parse(&msg).map_err(|e| format!("{origin}: message {}: {e}", k + 1))?;
        let (id, el) =
            norad::tle_from_omm(&m).map_err(|e| format!("{origin}: message {}: {e}", k + 1))?;
        let name = m
            .metadata
            .get("OBJECT_NAME")
            .map(str::to_string)
            .unwrap_or_else(|| format!("OBJECT {id}"));
        let object_id = m.metadata.get("OBJECT_ID").unwrap_or("").to_string();
        out.push(Candidate::new(
            id,
            name,
            object_id,
            source,
            origin.to_string(),
            true,
            el,
        ));
    }
    Ok(out)
}

/// Candidates from a text whose format is detected: OMM when it starts with `CCSDS_OMM_VERS`,
/// `<`, `[` or `{`, else TLE.
pub fn from_text(text: &str, origin: &str) -> Result<Vec<Candidate>, String> {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with("CCSDS_OMM_VERS")
        || t.starts_with('<')
        || t.starts_with('[')
        || t.starts_with('{')
        || t.starts_with("COMMENT")
    {
        from_omm_text(text, origin)
    } else {
        from_tle_text(text, origin)
    }
}

/// Merge candidate lists: one entry per catalogue number, the newest epoch kept; returns the
/// warnings (duplicates dropped). The result is sorted by catalogue number.
pub fn merge(all: Vec<Candidate>) -> (Vec<Candidate>, Vec<String>) {
    let mut by: std::collections::BTreeMap<u32, Candidate> = Default::default();
    let mut warnings = Vec::new();
    for c in all {
        match by.get(&c.catalogue_number) {
            Some(old) => {
                let newer = c.epoch.diff_seconds(old.epoch) > 0.0;
                warnings.push(format!(
                    "object {} given more than once ({} and {}); the element set with the newer epoch ({}) is used",
                    c.catalogue_number,
                    old.origin,
                    c.origin,
                    if newer { &c.origin } else { &old.origin }
                ));
                if newer {
                    by.insert(c.catalogue_number, c);
                }
            }
            None => {
                by.insert(c.catalogue_number, c);
            }
        }
    }
    (by.into_values().collect(), warnings)
}

/// A TLE exponent field (`" 12345-3"`: sign, five-digit mantissa with an implied leading
/// point, signed one-digit power of ten).
fn exp_field(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return " 00000-0".into();
    }
    let sign = if x < 0.0 { '-' } else { ' ' };
    let a = x.abs();
    let mut e = a.log10().floor() as i32 + 1;
    let mut m = (a / 10f64.powi(e) * 1e5).round() as i64;
    if m >= 100_000 {
        m /= 10;
        e += 1;
    }
    let e = e.clamp(-9, 9);
    let es = if e < 0 { '-' } else { '+' };
    format!("{sign}{m:05}{es}{}", e.abs())
}

/// Write SGP4 elements as a TLE (line 1, line 2) with the catalogue field in Alpha-5 above
/// 99,999 and correct checksums. Errors above 339,999 (write OMM instead). `designator` is
/// `YYYY-NNNP` or empty; the first and second mean-motion derivatives are written as zero.
pub fn format_tle(cat: u32, designator: &str, el: &Tle) -> Result<(String, String), String> {
    let field = norad::encode(cat)?;
    let des = match designator.split_once('-') {
        Some((y, rest)) if y.len() == 4 && rest.len() >= 3 => {
            format!("{}{}", &y[2..], rest)
        }
        _ => String::new(),
    };
    // Epoch: year and day of year from days since 1950 Jan 0.0.
    let mut year = 1950i64;
    let mut days = el.epoch_days_1950;
    loop {
        let len = if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
            366.0
        } else {
            365.0
        };
        if days <= len + 1.0 - 1e-9 && days >= 1.0 {
            break;
        }
        if days < 1.0 {
            return Err("TLE epoch before 1950".into());
        }
        days -= len;
        year += 1;
    }
    let deg = |r: f64| r.to_degrees().rem_euclid(360.0);
    let ecc = format!("{:07}", (el.ecco * 1e7).round() as i64);
    let l1 = format!(
        "1 {field}U {des:<8} {:02}{:012.8}  .00000000  00000-0 {} 0  999",
        year % 100,
        days,
        exp_field(el.bstar)
    );
    let l2 = format!(
        "2 {field} {:8.4} {:8.4} {ecc} {:8.4} {:8.4} {:11.8}    1",
        el.inclo_rad.to_degrees(),
        deg(el.nodeo_rad),
        deg(el.argpo_rad),
        deg(el.mo_rad),
        el.no_kozai_rad_min * 1440.0 / std::f64::consts::TAU
    );
    let fin = |l: String| {
        let c = kshana::tle::tle_checksum(&l);
        format!("{l}{c}")
    };
    Ok((fin(l1), fin(l2)))
}

/// An OMM (KVN) for SGP4 elements with any catalogue number up to nine digits.
pub fn format_omm_kvn(
    cat: u32,
    name: &str,
    designator: &str,
    el: &Tle,
    creation_utc: &str,
) -> Result<String, String> {
    if cat > norad::OMM_MAX {
        return Err(format!("catalogue number {cat} has more than nine digits"));
    }
    let epoch = Jd2::from_parts(2_433_281.0, 0.5 + el.epoch_days_1950);
    let ep = super::fmt_utc(epoch);
    let ep = ep.trim_end_matches('Z');
    let deg = |r: f64| r.to_degrees().rem_euclid(360.0);
    Ok(format!(
        "CCSDS_OMM_VERS = 3.0\nCREATION_DATE = {creation_utc}\nORIGINATOR = ARKSA\n\
OBJECT_NAME = {name}\nOBJECT_ID = {designator}\nCENTER_NAME = EARTH\nREF_FRAME = TEME\n\
TIME_SYSTEM = UTC\nMEAN_ELEMENT_THEORY = SGP4\nEPOCH = {ep}\nMEAN_MOTION = {:.10}\n\
ECCENTRICITY = {:.9}\nINCLINATION = {:.8}\nRA_OF_ASC_NODE = {:.8}\nARG_OF_PERICENTER = {:.8}\n\
MEAN_ANOMALY = {:.8}\nEPHEMERIS_TYPE = 0\nCLASSIFICATION_TYPE = U\nNORAD_CAT_ID = {cat}\n\
ELEMENT_SET_NO = 999\nREV_AT_EPOCH = 1\nBSTAR = {:e}\nMEAN_MOTION_DOT = 0.0\nMEAN_MOTION_DDOT = 0.0\n",
        el.no_kozai_rad_min * 1440.0 / std::f64::consts::TAU,
        el.ecco,
        el.inclo_rad.to_degrees(),
        deg(el.nodeo_rad),
        deg(el.argpo_rad),
        deg(el.mo_rad),
        el.bstar,
    ))
}
