// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! CCSDS Conjunction Data Message (CCSDS 508.0-B-1) ingest, in KVN (`KEYWORD = value [unit]`)
//! and NDM/XML (`<cdm>`, read with the open engine's NDM/XML reader), into [`Event`].
//!
//! The message is kept as written ([`Cdm`]: the header and relative block, and one keyword
//! map per object), then normalised: TCA to calendar seconds, the state vectors from km and
//! km/s to m and m/s, the 6x6 RTN covariance (`CR_R` to `CNDOT_NDOT`, m², m²/s, m²/s²) as a
//! symmetric matrix. The drag, solar-pressure and thrust rows (`CDRG_*`, `CSRP_*`, `CTHR_*`)
//! are read and kept in the keyword map but not used. A file may hold several messages (KVN
//! messages one after another, or several `<cdm>` elements).
//!
//! A hard-body radius is not a CDM 1.0 keyword; `HBR` (or `HARD_BODY_RADIUS`) is read when a
//! provider adds it, in the relative block (combined) or per object (summed).

use std::collections::BTreeMap;

use kshana::api::KshanaError;

use super::{bad, norm_id, parse_utc, Event, M6};

/// One object's block, keyword to value (units dropped), in message order.
#[derive(Debug, Clone, Default)]
pub struct CdmObject {
    pub items: Vec<(String, String)>,
}

impl CdmObject {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.items
            .iter()
            .rev()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    }
    fn num(&self, k: &str) -> Result<Option<f64>, KshanaError> {
        num_of(k, self.get(k))
    }
}

/// A conjunction data message as written.
#[derive(Debug, Clone, Default)]
pub struct Cdm {
    /// Header and relative metadata/data (everything before `OBJECT = OBJECT1`).
    pub relative: CdmObject,
    pub objects: [CdmObject; 2],
}

fn num_of(k: &str, v: Option<&str>) -> Result<Option<f64>, KshanaError> {
    match v {
        None => Ok(None),
        Some(s) if s.trim().is_empty() || s.eq_ignore_ascii_case("NULL") => Ok(None),
        Some(s) => s
            .trim()
            .parse::<f64>()
            .map(Some)
            .map_err(|_| bad(format!("CDM keyword {k}: '{s}' is not a number"))),
    }
}

/// The 21 RTN covariance keywords in CCSDS order (row by row, lower triangle).
pub const COV_KEYS: [&str; 21] = [
    "CR_R",
    "CT_R",
    "CT_T",
    "CN_R",
    "CN_T",
    "CN_N",
    "CRDOT_R",
    "CRDOT_T",
    "CRDOT_N",
    "CRDOT_RDOT",
    "CTDOT_R",
    "CTDOT_T",
    "CTDOT_N",
    "CTDOT_RDOT",
    "CTDOT_TDOT",
    "CNDOT_R",
    "CNDOT_T",
    "CNDOT_N",
    "CNDOT_RDOT",
    "CNDOT_TDOT",
    "CNDOT_NDOT",
];

/// The keywords a message must carry to be used.
const REQUIRED: &[&str] = &["TCA", "MISS_DISTANCE"];
const REQUIRED_OBJECT: &[&str] = &["OBJECT", "OBJECT_DESIGNATOR"];

impl Cdm {
    /// Assign one keyword in message order: `OBJECT = OBJECT1|OBJECT2` switches block.
    fn push(&mut self, cur: &mut usize, k: &str, v: &str) {
        if k == "COMMENT" {
            return;
        }
        if k == "OBJECT" {
            *cur = if v.trim().eq_ignore_ascii_case("OBJECT2") {
                2
            } else {
                1
            };
        }
        let item = (k.to_string(), v.trim().to_string());
        match *cur {
            0 => self.relative.items.push(item),
            n => self.objects[n - 1].items.push(item),
        }
    }

    /// Check the keywords needed for an [`Event`] are present.
    fn check(&self) -> Result<(), KshanaError> {
        for k in REQUIRED {
            if self.relative.get(k).is_none() {
                return Err(bad(format!("CDM is missing {k}")));
            }
        }
        for (i, o) in self.objects.iter().enumerate() {
            for k in REQUIRED_OBJECT {
                if o.get(k).is_none() {
                    return Err(bad(format!("CDM object {} is missing {k}", i + 1)));
                }
            }
        }
        Ok(())
    }

    /// The normalised event.
    pub fn to_event(&self) -> Result<Event, KshanaError> {
        self.check()?;
        let r = &self.relative;
        let [o1, o2] = &self.objects;
        let tca_text = r.get("TCA").unwrap_or_default();
        let mut e = Event {
            id: r.get("MESSAGE_ID").map(str::to_string).unwrap_or_else(|| {
                format!("{}@{tca_text}", o1.get("OBJECT_DESIGNATOR").unwrap_or(""))
            }),
            obj1: norm_id(o1.get("OBJECT_DESIGNATOR").unwrap_or_default()),
            obj2: norm_id(o2.get("OBJECT_DESIGNATOR").unwrap_or_default()),
            tca_s: parse_utc(tca_text)?,
            miss_m: r.num("MISS_DISTANCE")?.unwrap_or(f64::NAN),
            vrel_m_s: r.num("RELATIVE_SPEED")?,
            pc: r.num("COLLISION_PROBABILITY")?,
            pc_method: r.get("COLLISION_PROBABILITY_METHOD").map(str::to_string),
            ..Default::default()
        };
        if let (Some(a), Some(b), Some(c)) = (
            r.num("RELATIVE_POSITION_R")?,
            r.num("RELATIVE_POSITION_T")?,
            r.num("RELATIVE_POSITION_N")?,
        ) {
            e.rel_rtn_m = Some([a, b, c]);
        }
        e.state1 = state(o1)?;
        e.state2 = state(o2)?;
        e.frame = o1.get("REF_FRAME").map(str::to_string);
        e.cov1_rtn = covariance(o1)?;
        e.cov2_rtn = covariance(o2)?;
        let hbr = |o: &CdmObject| -> Result<Option<f64>, KshanaError> {
            Ok(o.num("HBR")?.or(o.num("HARD_BODY_RADIUS")?))
        };
        e.hbr_m = match (hbr(r)?, hbr(o1)?, hbr(o2)?) {
            (Some(h), _, _) => Some(h),
            (None, Some(a), Some(b)) => Some(a + b),
            (None, Some(a), None) | (None, None, Some(a)) => Some(a),
            _ => None,
        };
        e.ephemeris1 = o1.get("EPHEMERIS_NAME").map(str::to_string);
        e.ephemeris2 = o2.get("EPHEMERIS_NAME").map(str::to_string);
        e.covariance_method1 = o1.get("COVARIANCE_METHOD").map(str::to_string);
        e.covariance_method2 = o2.get("COVARIANCE_METHOD").map(str::to_string);
        let t = |o: &CdmObject, k: &str| o.get(k).map(parse_utc).transpose();
        e.last_obs1_s = t(o1, "TIME_LASTOB_END")?;
        e.last_obs2_s = t(o2, "TIME_LASTOB_END")?;
        e.created_s = t(r, "CREATION_DATE")?;
        Ok(e)
    }
}

fn state(o: &CdmObject) -> Result<Option<[f64; 6]>, KshanaError> {
    let mut s = [0.0; 6];
    for (i, k) in ["X", "Y", "Z", "X_DOT", "Y_DOT", "Z_DOT"]
        .iter()
        .enumerate()
    {
        match o.num(k)? {
            Some(v) => s[i] = v * 1000.0, // km, km/s
            None => return Ok(None),
        }
    }
    Ok(Some(s))
}

/// The RTN covariance: all 21 terms, or the 6 position terms alone (the velocity rows are
/// then zero; the two-dimensional Pc and the position checks use the position block only).
fn covariance(o: &CdmObject) -> Result<Option<M6>, KshanaError> {
    let vals: Vec<Option<f64>> = COV_KEYS
        .iter()
        .map(|k| o.num(k))
        .collect::<Result<_, _>>()?;
    let n = if vals.iter().all(Option::is_some) {
        21
    } else if vals[..6].iter().all(Option::is_some) {
        6
    } else {
        return Ok(None);
    };
    let mut c = [[0.0; 6]; 6];
    let mut k = 0;
    for i in 0..6 {
        for j in 0..=i {
            if k < n {
                let v = vals[k].unwrap_or(0.0);
                c[i][j] = v;
                c[j][i] = v;
            }
            k += 1;
        }
    }
    Ok(Some(c))
}

/// Split `KEYWORD = value [unit]` into keyword and value (unit dropped).
fn kv(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if let Some(rest) = line.strip_prefix("COMMENT") {
        return Some(("COMMENT".into(), rest.trim().into()));
    }
    let (k, v) = line.split_once('=')?;
    let mut v = v.trim();
    if let Some(i) = v.find('[') {
        if v.ends_with(']') {
            v = v[..i].trim();
        }
    }
    Some((k.trim().to_ascii_uppercase(), v.to_string()))
}

/// Parse every message in a KVN or NDM/XML text.
pub fn parse_all(text: &str) -> Result<Vec<Cdm>, KshanaError> {
    if text.trim_start().starts_with('<') {
        return parse_xml(text);
    }
    let mut out: Vec<Cdm> = Vec::new();
    let mut cur = 0usize;
    for (n, line) in text.lines().enumerate() {
        let Some((k, v)) = kv(line) else {
            if line.trim().is_empty() {
                continue;
            }
            return Err(bad(format!("CDM line {}: not KEYWORD = value", n + 1)));
        };
        if k == "CCSDS_CDM_VERS" {
            out.push(Cdm::default());
            cur = 0;
        }
        let Some(m) = out.last_mut() else {
            if k == "COMMENT" {
                continue;
            }
            return Err(bad("a CDM starts with CCSDS_CDM_VERS"));
        };
        m.push(&mut cur, &k, &v);
    }
    if out.is_empty() {
        return Err(bad("no CDM found (CCSDS_CDM_VERS missing)"));
    }
    Ok(out)
}

fn parse_xml(text: &str) -> Result<Vec<Cdm>, KshanaError> {
    use kshana::ndm_xml::Element;
    let root = kshana::ndm_xml::parse(text).map_err(|e| bad(format!("CDM XML: {e}")))?;
    fn find<'a>(e: &'a Element, out: &mut Vec<&'a Element>) {
        if e.name.eq_ignore_ascii_case("cdm") {
            out.push(e);
        } else {
            e.children.iter().for_each(|c| find(c, out));
        }
    }
    fn leaves(e: &Element, m: &mut Cdm, cur: &mut usize) {
        if e.children.is_empty() {
            m.push(cur, &e.name.to_ascii_uppercase(), &e.text);
        } else {
            e.children.iter().for_each(|c| leaves(c, m, cur));
        }
    }
    let mut cdms = Vec::new();
    find(&root, &mut cdms);
    if cdms.is_empty() {
        return Err(bad("no <cdm> element found"));
    }
    Ok(cdms
        .into_iter()
        .map(|c| {
            let mut m = Cdm::default();
            let mut cur = 0usize;
            if let Some(v) = c.attr("version") {
                m.relative.items.push(("CCSDS_CDM_VERS".into(), v.into()));
            }
            leaves(c, &mut m, &mut cur);
            m
        })
        .collect())
}

/// Parse every message in a text into events.
pub fn events(text: &str) -> Result<Vec<Event>, KshanaError> {
    parse_all(text)?.iter().map(Cdm::to_event).collect()
}

/// A short keyword census of a message (which optional groups it carries), for the ingest
/// report.
pub fn census(m: &Cdm) -> BTreeMap<&'static str, bool> {
    let o = &m.objects;
    let mut c = BTreeMap::new();
    c.insert(
        "relative_position",
        m.relative.get("RELATIVE_POSITION_R").is_some(),
    );
    c.insert(
        "collision_probability",
        m.relative.get("COLLISION_PROBABILITY").is_some(),
    );
    c.insert("state_vectors", o.iter().all(|x| x.get("X").is_some()));
    c.insert("rtn_covariance", o.iter().all(|x| x.get("CR_R").is_some()));
    c.insert(
        "velocity_covariance",
        o.iter().all(|x| x.get("CNDOT_NDOT").is_some()),
    );
    c.insert(
        "ephemeris_name",
        o.iter().all(|x| x.get("EPHEMERIS_NAME").is_some()),
    );
    c.insert(
        "od_parameters",
        o.iter().all(|x| x.get("TIME_LASTOB_END").is_some()),
    );
    c.insert(
        "hard_body_radius",
        m.relative.get("HBR").is_some() || o.iter().any(|x| x.get("HBR").is_some()),
    );
    c
}

/// Write an event as a CCSDS CDM 1.0 KVN message (the keywords the event carries; state in
/// km and km/s, covariance in m-based units). `originator` and `creation` (UTC text) go in the
/// header. Used to hand normalised events (for example rows of a results table) to tools that
/// read CDMs.
pub fn write_kvn(e: &Event, originator: &str, creation: &str) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let mut kv = |k: &str, v: String| {
        let _ = writeln!(s, "{k:<36}= {v}");
    };
    kv("CCSDS_CDM_VERS", "1.0".into());
    kv("CREATION_DATE", creation.into());
    kv("ORIGINATOR", originator.into());
    kv("MESSAGE_ID", e.id.clone());
    kv("TCA", super::fmt_utc(e.tca_s).trim_end_matches('Z').into());
    kv("MISS_DISTANCE", format!("{:.6} [m]", e.miss_m));
    if let Some(v) = e.vrel_m_s {
        kv("RELATIVE_SPEED", format!("{v:.6} [m/s]"));
    }
    if let Some(r) = e.rel_rtn_m {
        for (k, x) in ["R", "T", "N"].iter().zip(r) {
            kv(&format!("RELATIVE_POSITION_{k}"), format!("{x:.6} [m]"));
        }
    }
    if let Some(p) = e.pc {
        kv("COLLISION_PROBABILITY", format!("{p:.6e}"));
    }
    if let Some(m) = &e.pc_method {
        kv("COLLISION_PROBABILITY_METHOD", m.clone());
    }
    if let Some(h) = e.hbr_m {
        kv("HBR", format!("{h} [m]"));
    }
    let objs = [
        (
            &e.obj1,
            e.state1,
            e.cov1_rtn,
            &e.ephemeris1,
            &e.covariance_method1,
            e.last_obs1_s,
        ),
        (
            &e.obj2,
            e.state2,
            e.cov2_rtn,
            &e.ephemeris2,
            &e.covariance_method2,
            e.last_obs2_s,
        ),
    ];
    for (n, (id, st, cov, eph, cm, lo)) in objs.into_iter().enumerate() {
        kv("OBJECT", format!("OBJECT{}", n + 1));
        kv("OBJECT_DESIGNATOR", id.clone());
        kv("CATALOG_NAME", "UNSPECIFIED".into());
        kv("OBJECT_NAME", id.clone());
        kv("INTERNATIONAL_DESIGNATOR", "UNKNOWN".into());
        kv(
            "EPHEMERIS_NAME",
            eph.clone().unwrap_or_else(|| "NONE".into()),
        );
        kv(
            "COVARIANCE_METHOD",
            cm.clone().unwrap_or_else(|| "CALCULATED".into()),
        );
        kv("MANEUVERABLE", "N/A".into());
        kv(
            "REF_FRAME",
            e.frame.clone().unwrap_or_else(|| "EME2000".into()),
        );
        if let Some(t) = lo {
            kv(
                "TIME_LASTOB_END",
                super::fmt_utc(t).trim_end_matches('Z').into(),
            );
        }
        if let Some(st) = st {
            for (i, k) in ["X", "Y", "Z", "X_DOT", "Y_DOT", "Z_DOT"]
                .iter()
                .enumerate()
            {
                let u = if i < 3 { "km" } else { "km/s" };
                kv(k, format!("{:.9} [{u}]", st[i] / 1000.0));
            }
        }
        if let Some(c) = cov {
            let mut k = 0;
            for i in 0..6 {
                for j in 0..=i {
                    kv(COV_KEYS[k], format!("{:.12e}", c[i][j]));
                    k += 1;
                }
            }
        }
    }
    s
}
