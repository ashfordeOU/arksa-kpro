// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! NORAD catalogue numbers above 99,999: the Alpha-5 TLE field and the OMM `NORAD_CAT_ID`.
//!
//! **Local stop-gap, to be replaced by `kshana::norad`.** The open engine gained a `norad`
//! module on its main branch (kshana PR #114, commit c86040bd), but this crate builds on the
//! published kshana 0.41.0, which does not contain it (0.41.0's `tle::parse_tle_identity` reads
//! the catalogue field as five digits only). The functions here follow the same rules, word for
//! word, and `tests/rideshare_id.rs` checks them against the open engine's conformance vectors
//! (`data/conformance/time/norad-catalogue.json`, copied into
//! `tests/fixtures/rideshare/norad-catalogue.json`). When a kshana release with `norad` is
//! pinned, delete this file and call `kshana::norad` instead; the vector test then guards the
//! swap.
//!
//! Alpha-5: the first of the five columns becomes a letter standing for the two leading
//! digits, `A` = 10 to `Z` = 33, skipping `I` and `O`; so `A0000` = 100,000 and `Z9999` =
//! 339,999. Above that a number cannot be written in a TLE at all and only OMM can carry it
//! (`NORAD_CAT_ID`, up to nine digits). The TLE checksum is unchanged: a letter counts as 0.

use kshana::odm::OmmMessage;
use kshana::tle::Tle;

/// Largest catalogue number a plain five-digit TLE field holds.
pub const FIVE_DIGIT_MAX: u32 = 99_999;
/// Largest catalogue number Alpha-5 holds (`Z9999`).
pub const ALPHA5_MAX: u32 = 339_999;
/// Largest catalogue number an OMM `NORAD_CAT_ID` may carry (nine digits).
pub const OMM_MAX: u32 = 999_999_999;

/// The Alpha-5 letters in order; the letter at index `k` stands for `10 + k`.
const LETTERS: &[u8; 24] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";

/// The catalogue number in a five-column TLE field: five digits (leading blanks or zeros
/// allowed) or an Alpha-5 letter followed by four digits. Surrounding blanks are ignored. A
/// lower-case letter, `I`, `O`, a sign or any other character is an error.
pub fn decode(field: &str) -> Result<u32, String> {
    let f = field.trim();
    let b = f.as_bytes();
    let bad = || format!("bad catalogue-number field {field:?}");
    if b.is_empty() || b.len() > 5 {
        return Err(bad());
    }
    let digits = |s: &[u8]| -> Option<u32> {
        if s.iter().all(u8::is_ascii_digit) {
            Some(s.iter().fold(0u32, |a, &d| a * 10 + u32::from(d - b'0')))
        } else {
            None
        }
    };
    if b[0].is_ascii_digit() {
        return digits(b).ok_or_else(bad);
    }
    if b.len() != 5 {
        return Err(bad());
    }
    let k = LETTERS.iter().position(|&l| l == b[0]).ok_or_else(bad)? as u32;
    let rest = digits(&b[1..]).ok_or_else(bad)?;
    Ok((10 + k) * 10_000 + rest)
}

/// The five-column TLE field for catalogue number `n`: zero-padded digits up to 99,999,
/// Alpha-5 from 100,000 to 339,999; above that an error saying to use OMM.
pub fn encode(n: u32) -> Result<String, String> {
    if n <= FIVE_DIGIT_MAX {
        return Ok(format!("{n:05}"));
    }
    if n <= ALPHA5_MAX {
        let letter = LETTERS[(n / 10_000 - 10) as usize] as char;
        return Ok(format!("{letter}{:04}", n % 10_000));
    }
    Err(format!(
        "catalogue number {n} is above {ALPHA5_MAX} and cannot be written in a TLE (Alpha-5); use OMM"
    ))
}

/// True if `field` is written in Alpha-5 (a letter in its first column).
pub fn is_alpha5(field: &str) -> bool {
    field
        .trim()
        .as_bytes()
        .first()
        .is_some_and(|c| c.is_ascii_alphabetic())
}

/// The `NORAD_CAT_ID` of an OMM, from its TLE-parameters block or, failing that, its
/// metadata: an unsigned integer of up to nine digits, never limited to five.
pub fn catalogue_number_from_omm(m: &OmmMessage) -> Result<u32, String> {
    let v = m
        .tle
        .as_ref()
        .and_then(|s| s.get("NORAD_CAT_ID"))
        .or_else(|| m.metadata.get("NORAD_CAT_ID"))
        .ok_or_else(|| "OMM has no NORAD_CAT_ID".to_string())?
        .trim();
    let bad = || format!("OMM NORAD_CAT_ID {v:?} is not an integer of 1 to 9 digits");
    if v.is_empty() || v.len() > 9 || !v.bytes().all(|c| c.is_ascii_digit()) {
        return Err(bad());
    }
    v.parse::<u32>().map_err(|_| bad())
}

/// Days from 1950-01-00.0 (SGP4's epoch origin) to the start of `year`.
fn days_1950_to_year(year: i64) -> i64 {
    let days_to = |y: i64| 365 * y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400);
    days_to(year - 1) - days_to(1949)
}

/// An epoch `YYYY-MM-DDThh:mm:ss[.f]` or `YYYY-DDDThh:mm:ss[.f]` (optional trailing `Z`) as
/// days since 1950 Jan 0.0 in the epoch's own time scale (the SGP4 epoch count).
pub fn epoch_days_1950(s: &str) -> Result<f64, String> {
    let bad = || format!("bad epoch {s:?}");
    let s2 = s.trim().trim_end_matches('Z');
    if !s2.is_ascii() {
        return Err(bad());
    }
    let (date, time) = s2.split_once('T').unwrap_or((s2, "00:00:00"));
    let parts: Vec<&str> = date.split('-').collect();
    let int = |t: &str, max: i64| -> Result<i64, String> {
        if t.is_empty() || t.len() > 4 || !t.bytes().all(|c| c.is_ascii_digit()) {
            return Err(bad());
        }
        let v: i64 = t.parse().map_err(|_| bad())?;
        if v > max {
            return Err(bad());
        }
        Ok(v)
    };
    let year = int(parts.first().copied().unwrap_or(""), 9999)?;
    if !(1950..=2100).contains(&year) {
        return Err(bad());
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let doy = match parts.len() {
        2 => {
            let d = int(parts[1], 366)?;
            if d < 1 || (d == 366 && !leap) {
                return Err(bad());
            }
            d
        }
        3 => {
            let m = int(parts[1], 12)?;
            let d = int(parts[2], 31)?;
            const CUM: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
            const LEN: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
            if m < 1 {
                return Err(bad());
            }
            let len = LEN[(m - 1) as usize] + i64::from(leap && m == 2);
            if d < 1 || d > len {
                return Err(bad());
            }
            CUM[(m - 1) as usize] + d + i64::from(leap && m > 2)
        }
        _ => return Err(bad()),
    };
    let tp: Vec<&str> = time.split(':').collect();
    if tp.len() != 3 {
        return Err(bad());
    }
    let hh = int(tp[0], 23)?;
    let mm = int(tp[1], 59)?;
    if tp[2].is_empty() || !tp[2].bytes().all(|c| c.is_ascii_digit() || c == b'.') {
        return Err(bad());
    }
    let ss: f64 = tp[2].parse().map_err(|_| bad())?;
    if !(0.0..61.0).contains(&ss) {
        return Err(bad());
    }
    Ok(days_1950_to_year(year) as f64
        + doy as f64
        + (hh as f64 * 3600.0 + mm as f64 * 60.0 + ss) / 86_400.0)
}

/// SGP4 elements from an OMM's mean elements and TLE parameters (`EPOCH`, `MEAN_MOTION`
/// rev/day, `ECCENTRICITY`, `INCLINATION`, `RA_OF_ASC_NODE`, `ARG_OF_PERICENTER`,
/// `MEAN_ANOMALY` in degrees, `BSTAR` in 1/earth-radii, 0 when absent), with the catalogue
/// number. The same range checks as a TLE apply; nothing limits the number to five digits.
pub fn tle_from_omm(m: &OmmMessage) -> Result<(u32, Tle), String> {
    let id = catalogue_number_from_omm(m)?;
    let me = &m.mean_elements;
    let need = |k: &str| -> Result<f64, String> {
        match me.num(k)? {
            Some(v) if v.is_finite() => Ok(v),
            _ => Err(format!("OMM has no usable {k}")),
        }
    };
    let epoch = me
        .get("EPOCH")
        .ok_or_else(|| "OMM has no EPOCH".to_string())?;
    let epoch_days_1950 = epoch_days_1950(epoch)?;
    let n = need("MEAN_MOTION")?;
    let e = need("ECCENTRICITY")?;
    let i = need("INCLINATION")?;
    let raan = need("RA_OF_ASC_NODE")?;
    let argp = need("ARG_OF_PERICENTER")?;
    let ma = need("MEAN_ANOMALY")?;
    let bstar = match m
        .tle
        .as_ref()
        .map(|s| s.num("BSTAR"))
        .transpose()?
        .flatten()
    {
        Some(b) if b.is_finite() => b,
        Some(_) => return Err("OMM has no usable BSTAR".into()),
        None => 0.0,
    };
    if !(0.0..=180.0).contains(&i) || !(0.0..1.0).contains(&e) || n <= 0.0 {
        return Err("OMM mean elements out of range".into());
    }
    Ok((
        id,
        Tle {
            epoch_days_1950,
            bstar,
            ecco: e,
            argpo_rad: argp.to_radians(),
            inclo_rad: i.to_radians(),
            mo_rad: ma.to_radians(),
            no_kozai_rad_min: n * std::f64::consts::TAU / 1440.0,
            nodeo_rad: raan.to_radians(),
        },
    ))
}

/// Identity on a TLE line 1 with the catalogue field read as five digits or Alpha-5 (the
/// open engine 0.41.0's `parse_tle_identity` reads five digits only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TleIdentity {
    /// Catalogue number (decoded from columns 3-7).
    pub norad_cat_id: u32,
    /// The five-column field as written.
    pub field: String,
    /// COSPAR international designator as `YYYY-NNNP` (empty when the columns are blank).
    pub intl_designator: String,
    /// Epoch as a CCSDS day-of-year timestamp `YYYY-DDDThh:mm:ss.ssssss` (UTC).
    pub epoch_iso: String,
}

/// Read the [`TleIdentity`] of a TLE line 1 (five-digit or Alpha-5 catalogue field).
pub fn parse_tle_identity(line1: &str) -> Result<TleIdentity, String> {
    if !line1.is_ascii() || !line1.starts_with("1 ") || line1.len() < 63 {
        return Err(format!("not a TLE line 1: {line1:?}"));
    }
    let field = line1[2..7].to_string();
    let norad_cat_id = decode(&field)?;
    let des_yy = line1[9..11].trim();
    let intl_designator = if des_yy.is_empty() {
        String::new()
    } else {
        let yy: i64 = des_yy
            .parse()
            .map_err(|_| format!("bad designator year {des_yy:?}"))?;
        let y = if yy < 57 { 2000 + yy } else { 1900 + yy };
        format!("{y:04}-{}{}", line1[11..14].trim(), line1[14..17].trim())
    };
    let yy: i64 = line1[18..20]
        .trim()
        .parse()
        .map_err(|_| "bad TLE epoch year".to_string())?;
    let year = if yy < 57 { 2000 + yy } else { 1900 + yy };
    let epochdays: f64 = line1[20..32]
        .trim()
        .parse()
        .map_err(|_| "bad TLE epoch day".to_string())?;
    let doy = epochdays.floor();
    let sec = (epochdays - doy) * 86_400.0;
    let hh = (sec / 3600.0).floor();
    let mm = ((sec - hh * 3600.0) / 60.0).floor();
    let ss = sec - hh * 3600.0 - mm * 60.0;
    Ok(TleIdentity {
        norad_cat_id,
        field,
        intl_designator,
        epoch_iso: format!(
            "{year:04}-{:03}T{:02}:{:02}:{ss:09.6}",
            doy as u32, hh as u32, mm as u32
        ),
    })
}
