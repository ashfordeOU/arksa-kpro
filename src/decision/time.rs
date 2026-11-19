// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! UTC time stamps of the decision records and the disposal assessment: the ISO 8601 form
//! `YYYY-MM-DDTHH:MM:SS[.fff]Z` only, parsed to and written from Unix seconds. No clock
//! is read here.

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff]Z` to Unix seconds. Anything else is refused with
/// the reason.
pub fn parse_utc(s: &str) -> Result<f64, String> {
    let bad = || format!("'{s}' is not a UTC time of the form YYYY-MM-DDTHH:MM:SSZ");
    let t = s.trim();
    let body = t.strip_suffix('Z').ok_or_else(bad)?;
    let (date, time) = body.split_once('T').ok_or_else(bad)?;
    let dp: Vec<&str> = date.split('-').collect();
    let tp: Vec<&str> = time.split(':').collect();
    if dp.len() != 3 || tp.len() != 3 || dp[0].len() != 4 || dp[1].len() != 2 || dp[2].len() != 2 {
        return Err(bad());
    }
    let int = |x: &str| -> Result<i64, String> {
        if x.len() == 2 || x.len() == 4 {
            x.parse::<i64>().map_err(|_| bad())
        } else {
            Err(bad())
        }
    };
    let (y, mo, d) = (int(dp[0])?, int(dp[1])?, int(dp[2])?);
    let (h, mi) = (int(tp[0])?, int(tp[1])?);
    let sec: f64 = tp[2].parse().map_err(|_| bad())?;
    if !(1..=12).contains(&mo)
        || d < 1
        || d > days_in_month(y, mo)
        || !(0..24).contains(&h)
        || !(0..60).contains(&mi)
        || !(0.0..60.0).contains(&sec)
        || tp[2].len() < 2
    {
        return Err(bad());
    }
    Ok(days_from_civil(y, mo, d) as f64 * 86_400.0 + (h * 3600 + mi * 60) as f64 + sec)
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` (whole seconds, truncated).
pub fn fmt_utc(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let s = unix.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        (s % 3600) / 60,
        s % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_refuses_bad_stamps() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z").unwrap(), 0.0);
        assert_eq!(parse_utc("2000-03-01T00:00:00Z").unwrap(), 951_868_800.0);
        assert_eq!(
            parse_utc("2026-11-19T17:32:03.5Z").unwrap(),
            1_795_109_523.5
        );
        for t in [0_i64, 951_868_800, 1_795_109_523, 4_102_444_800] {
            assert_eq!(parse_utc(&fmt_utc(t)).unwrap(), t as f64);
        }
        for bad in [
            "2026-11-19 17:32:03Z",
            "2026-11-19T17:32:03",
            "2026-02-30T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-11-19T24:00:00Z",
            "26-11-19T00:00:00Z",
            "",
        ] {
            assert!(parse_utc(bad).is_err(), "{bad}");
        }
    }
}
