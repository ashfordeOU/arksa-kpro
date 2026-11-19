// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! A screening estimate of the orbital lifetime of a near-circular low orbit under drag.
//!
//! The semi-major axis of a circular orbit decays as `da/dt = -B·ρ(h)·sqrt(μ·a)`, with
//! `B = C_D·A/m` the ballistic coefficient and `ρ` the neutral density of the open
//! engine's Jacchia 1971 model at the diurnal mean exospheric temperature for a constant
//! space-weather state ([`kshana::space_weather::SpaceWeather::density`]). The lifetime is
//! `∫ dh / |dh/dt|` from the start altitude down to [`REENTRY_ALTITUDE_KM`], taken in
//! [`STEP_KM`] steps with the midpoint rule.
//!
//! It is MODELLED and for screening only: the solar cycle is replaced by three constant
//! activity cases whose spread stands in for the uncertainty; the orbit is taken as
//! circular; attitude, lift, solar radiation pressure, third bodies and the Earth's
//! oblateness are left out. An operator's own lifetime analysis (for example from ESA's
//! DRAMA/OSCAR, which this crate does not reproduce) takes precedence when it is supplied.

use kshana::space_weather::SpaceWeather;
use serde::Serialize;

/// Earth's gravitational parameter (m³/s²), the EGM96/WGS 84 value.
pub const MU_EARTH: f64 = 3.986_004_418e14;
/// Equatorial radius of the spherical Earth of the ESA standard's protected region (m).
pub const EARTH_RADIUS_M: f64 = 6_378_000.0;
/// The altitude taken as re-entry (km).
pub const REENTRY_ALTITUDE_KM: f64 = 120.0;
/// Altitude step of the integration (km).
pub const STEP_KM: f64 = 0.25;
/// A lifetime longer than this is reported as "more than" it (years).
pub const MAX_YEARS: f64 = 200.0;
const SECONDS_PER_YEAR: f64 = 365.25 * 86_400.0;

/// One constant space-weather case.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ActivityCase {
    pub name: String,
    /// Daily and 81-day mean F10.7 solar flux (solar flux units), held constant.
    pub f107_sfu: f64,
    /// Planetary Kp index, held constant.
    pub kp: f64,
}

/// The three default cases: low, moderate and high solar activity.
pub fn default_cases() -> Vec<ActivityCase> {
    [
        ("low", 70.0, 1.0),
        ("moderate", 140.0, 2.0),
        ("high", 220.0, 3.0),
    ]
    .iter()
    .map(|(n, f, k)| ActivityCase {
        name: n.to_string(),
        f107_sfu: *f,
        kp: *k,
    })
    .collect()
}

/// The lifetime of one case.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CaseLifetime {
    pub case: ActivityCase,
    /// Years from the start altitude to re-entry; `None` when longer than [`MAX_YEARS`].
    pub years: Option<f64>,
}

/// Altitude rate (m/s, negative) of a circular orbit at altitude `h_m` under drag.
fn altitude_rate(h_m: f64, bc_m2_per_kg: f64, sw: &SpaceWeather) -> f64 {
    let a = EARTH_RADIUS_M + h_m;
    -bc_m2_per_kg * sw.density(h_m) * (MU_EARTH * a).sqrt()
}

/// Years for a circular orbit at `alt_km` to decay to [`REENTRY_ALTITUDE_KM`], for the
/// ballistic coefficient `bc_m2_per_kg` (`C_D·A/m`) and one activity case. `None` when the
/// lifetime exceeds [`MAX_YEARS`].
pub fn decay_years(alt_km: f64, bc_m2_per_kg: f64, case: &ActivityCase) -> Option<f64> {
    if alt_km <= REENTRY_ALTITUDE_KM {
        return Some(0.0);
    }
    let sw = SpaceWeather {
        f107: case.f107_sfu,
        f107a: case.f107_sfu,
        kp: case.kp,
    };
    let max_s = MAX_YEARS * SECONDS_PER_YEAR;
    let mut t = 0.0;
    let mut lo = REENTRY_ALTITUDE_KM;
    while lo < alt_km {
        let hi = (lo + STEP_KM).min(alt_km);
        let mid_m = 0.5 * (lo + hi) * 1000.0;
        let rate = altitude_rate(mid_m, bc_m2_per_kg, &sw);
        if rate >= 0.0 || !rate.is_finite() {
            return None;
        }
        t += (hi - lo) * 1000.0 / -rate;
        if t > max_s {
            return None;
        }
        lo = hi;
    }
    Some(t / SECONDS_PER_YEAR)
}

/// The lifetime at `alt_km` for every case.
pub fn lifetimes(alt_km: f64, bc_m2_per_kg: f64, cases: &[ActivityCase]) -> Vec<CaseLifetime> {
    cases
        .iter()
        .map(|c| CaseLifetime {
            case: c.clone(),
            years: decay_years(alt_km, bc_m2_per_kg, c),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moderate() -> ActivityCase {
        default_cases().remove(1)
    }

    #[test]
    fn lifetime_rises_with_altitude_and_falls_with_activity_and_area() {
        let bc = 2.2 * 1.0 / 100.0; // 0.022 m²/kg
        let c = default_cases();
        let mut last = 0.0;
        for alt in [250.0, 350.0, 450.0, 550.0, 650.0] {
            let y = decay_years(alt, bc, &c[1]).unwrap();
            assert!(y > last, "{alt} km: {y} not above {last}");
            last = y;
        }
        let at = |case: &ActivityCase, bc: f64| decay_years(500.0, bc, case).unwrap();
        assert!(at(&c[0], bc) > at(&c[1], bc) && at(&c[1], bc) > at(&c[2], bc));
        assert!(at(&c[1], bc / 2.0) > at(&c[1], bc));
        assert_eq!(decay_years(110.0, bc, &c[1]), Some(0.0));
    }

    #[test]
    fn orders_of_magnitude_are_those_of_the_textbook_cases() {
        // A 0.022 m²/kg spacecraft: months at 300 km, years around 500 km, and well past
        // 25 years at 800 km. These are screening ranges, not a validation.
        let bc = 0.022;
        let y300 = decay_years(300.0, bc, &moderate()).unwrap();
        assert!((0.01..1.0).contains(&y300), "{y300}");
        let y500 = decay_years(500.0, bc, &moderate()).unwrap();
        assert!((0.5..15.0).contains(&y500), "{y500}");
        let y800 = decay_years(800.0, bc, &moderate());
        assert!(y800.is_none_or(|y| y > 25.0), "{y800:?}");
        assert_eq!(decay_years(1800.0, bc, &moderate()), None);
    }
}
