// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Two-dimensional probability of collision (the short-encounter integral).
//!
//! At TCA the relative position `r = r₂ − r₁` and velocity `v = v₂ − v₁` define the encounter
//! plane (normal to `v`). The combined position covariance `C = C₁ + C₂` (each object's RTN
//! covariance rotated to inertial axes) is projected onto that plane, and the Gaussian with
//! mean at the miss vector is integrated over the disc of the hard-body radius `R` centred on
//! the primary:
//!
//! ```text
//!   Pc = ∫_{-R}^{R} φ((x − mₓ)/σₓ)/σₓ · [Φ((√(R²−x²) − m_y)/σ_y) − Φ((−√(R²−x²) − m_y)/σ_y)] dx
//! ```
//!
//! in the principal axes of the projected covariance. This is the textbook method (rectilinear
//! relative motion, valid for non-low relative velocity); it is used here to recompute and to
//! explain a provider's figure (swapping one input at a time), never to grade a provider's own
//! Pc, whose method may differ (the TraCSS verification dataset's guide says the same).

use super::{
    cross, dot, mat_vec, norm, pos_block, rotate_cov_to_inertial, rtn_matrix, scale, sub, unit,
    Event, M3, V3,
};

/// The inputs of one Pc evaluation (inertial, m, m/s, m²).
#[derive(Debug, Clone, Copy)]
pub struct Encounter {
    pub r1: V3,
    pub v1: V3,
    pub r2: V3,
    pub v2: V3,
    /// Position covariances in inertial axes.
    pub c1: M3,
    pub c2: M3,
    pub hbr_m: f64,
}

impl Encounter {
    /// From an event with both states and both RTN covariances.
    pub fn from_event(e: &Event, hbr_m: f64) -> Option<Self> {
        let (s1, s2) = (e.state1?, e.state2?);
        let (r1, v1) = ([s1[0], s1[1], s1[2]], [s1[3], s1[4], s1[5]]);
        let (r2, v2) = ([s2[0], s2[1], s2[2]], [s2[3], s2[4], s2[5]]);
        let c1 = rotate_cov_to_inertial(&rtn_matrix(r1, v1), &pos_block(&e.cov1_rtn?));
        let c2 = rotate_cov_to_inertial(&rtn_matrix(r2, v2), &pos_block(&e.cov2_rtn?));
        Some(Self {
            r1,
            v1,
            r2,
            v2,
            c1,
            c2,
            hbr_m,
        })
    }

    /// The encounter-plane miss vector (m) and projected 2x2 covariance (m²).
    pub fn plane(&self) -> Option<([f64; 2], [[f64; 2]; 2])> {
        let r = sub(self.r2, self.r1);
        let v = sub(self.v2, self.v1);
        if norm(v) == 0.0 {
            return None;
        }
        let vh = unit(v);
        let rp = sub(r, scale(vh, dot(r, vh)));
        let e1 = if norm(rp) > 1e-9 {
            unit(rp)
        } else {
            // Head-on through the primary: any axis normal to v.
            let a = if vh[0].abs() < 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            unit(cross(vh, a))
        };
        let e2 = cross(vh, e1);
        let mut c = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                c[i][j] = self.c1[i][j] + self.c2[i][j];
            }
        }
        let p = |a: V3, b: V3| dot(a, mat_vec(&c, b));
        Some((
            [dot(r, e1), dot(r, e2)],
            [[p(e1, e1), p(e1, e2)], [p(e2, e1), p(e2, e2)]],
        ))
    }

    /// The two-dimensional Pc; `None` when the projected covariance is not positive definite.
    pub fn pc(&self) -> Option<f64> {
        let (m, p) = self.plane()?;
        pc_2d(m, p, self.hbr_m)
    }

    /// The squared Mahalanobis distance of the miss vector in the encounter plane.
    pub fn mahalanobis(&self) -> Option<f64> {
        let (m, p) = self.plane()?;
        let det = p[0][0] * p[1][1] - p[0][1] * p[1][0];
        if det.is_nan() || det <= 0.0 || p[0][0].is_nan() || p[0][0] <= 0.0 {
            return None;
        }
        let d2 =
            (p[1][1] * m[0] * m[0] - 2.0 * p[0][1] * m[0] * m[1] + p[0][0] * m[1] * m[1]) / det;
        Some(d2.sqrt())
    }
}

fn q_tail(x: f64) -> f64 {
    0.5 * kshana::lambda::erfc_accurate(x / std::f64::consts::SQRT_2)
}

/// `Φ(b) − Φ(a)` for `a ≤ b` without cancellation in either tail.
fn phi_diff(a: f64, b: f64) -> f64 {
    if a >= 0.0 {
        q_tail(a) - q_tail(b)
    } else if b <= 0.0 {
        q_tail(-b) - q_tail(-a)
    } else {
        1.0 - q_tail(b) - q_tail(-a)
    }
}

/// Gauss–Legendre nodes and weights on [-1, 1] (16 points).
const GL16: [(f64, f64); 8] = [
    (0.095_012_509_837_637_44, 0.189_450_610_455_068_5),
    (0.281_603_550_779_258_9, 0.182_603_415_044_923_6),
    (0.458_016_777_657_227_4, 0.169_156_519_395_002_5),
    (0.617_876_244_402_643_7, 0.149_595_988_816_576_7),
    (0.755_404_408_355_003, 0.124_628_971_255_533_9),
    (0.865_631_202_387_831_7, 0.095_158_511_682_492_78),
    (0.944_575_023_073_232_6, 0.062_253_523_938_647_89),
    (0.989_400_934_991_649_9, 0.027_152_459_411_754_09),
];

/// The disc integral of a bivariate Gaussian with mean `m` and covariance `p` over radius `r`
/// about the origin. `None` when `p` is not positive definite or `r` is not positive.
pub fn pc_2d(m: [f64; 2], p: [[f64; 2]; 2], r: f64) -> Option<f64> {
    if r.is_nan() || r <= 0.0 {
        return None;
    }
    let (a, b, c) = (p[0][0], 0.5 * (p[0][1] + p[1][0]), p[1][1]);
    let tr = a + c;
    let det = a * c - b * b;
    if det.is_nan() || det <= 0.0 || tr.is_nan() || tr <= 0.0 {
        return None;
    }
    // Principal axes.
    let disc = ((a - c) * (a - c) / 4.0 + b * b).sqrt();
    let l1 = tr / 2.0 + disc;
    let l2 = det / l1;
    let th = 0.5 * (2.0 * b).atan2(a - c);
    let (ct, st) = (th.cos(), th.sin());
    let mx = ct * m[0] + st * m[1];
    let my = -st * m[0] + ct * m[1];
    let (sx, sy) = (l1.sqrt(), l2.sqrt());
    // x = r sin(u), u ∈ [-π/2, π/2]: removes the square-root end points.
    let f = |u: f64| {
        let x = r * u.sin();
        let h = r * u.cos();
        let zx = (x - mx) / sx;
        let gx = (-0.5 * zx * zx).exp() / (sx * (2.0 * std::f64::consts::PI).sqrt());
        gx * phi_diff((-h - my) / sy, (h - my) / sy) * h
    };
    // Composite Gauss–Legendre, panels concentrated where the Gaussian sits.
    let panels = 64;
    let lo = -std::f64::consts::FRAC_PI_2;
    let w = std::f64::consts::PI / panels as f64;
    let mut sum = 0.0;
    for k in 0..panels {
        let c0 = lo + (k as f64 + 0.5) * w;
        for &(x, wt) in &GL16 {
            sum += wt * (f(c0 + 0.5 * w * x) + f(c0 - 0.5 * w * x));
        }
    }
    Some((sum * 0.5 * w).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centred_isotropic_matches_closed_form() {
        for (s, r) in [(100.0, 10.0), (10.0, 20.0), (1000.0, 5.0)] {
            let pc = pc_2d([0.0, 0.0], [[s * s, 0.0], [0.0, s * s]], r).unwrap();
            let exact = 1.0 - (-(r * r) / (2.0 * s * s)).exp();
            assert!((pc / exact - 1.0).abs() < 1e-9, "{s} {r}: {pc} vs {exact}");
        }
    }

    #[test]
    fn small_disc_offset_matches_density_times_area() {
        // R ≪ σ: Pc ≈ πR² · density at the origin.
        let (sx, sy, r) = (200.0_f64, 50.0_f64, 0.5_f64);
        let m = [150.0, -40.0];
        let pc = pc_2d(m, [[sx * sx, 0.0], [0.0, sy * sy]], r).unwrap();
        let dens = (-0.5 * ((m[0] / sx).powi(2) + (m[1] / sy).powi(2))).exp()
            / (2.0 * std::f64::consts::PI * sx * sy);
        let approx = std::f64::consts::PI * r * r * dens;
        assert!((pc / approx - 1.0).abs() < 1e-4, "{pc} vs {approx}");
    }

    #[test]
    fn rotation_invariant_and_tail_accurate() {
        let p = [[400.0, 0.0], [0.0, 25.0]];
        let a = pc_2d([300.0, 0.0], p, 10.0).unwrap();
        let th = 0.7_f64;
        let (c, s) = (th.cos(), th.sin());
        let pr = [
            [c * c * 400.0 + s * s * 25.0, c * s * (400.0 - 25.0)],
            [c * s * (400.0 - 25.0), s * s * 400.0 + c * c * 25.0],
        ];
        let b = pc_2d([c * 300.0, s * 300.0], pr, 10.0).unwrap();
        assert!(a > 0.0 && a < 1e-30, "a far tail: {a}");
        assert!((a / b - 1.0).abs() < 1e-6, "{a} vs {b}");
        assert!(pc_2d([0.0, 0.0], [[1.0, 2.0], [2.0, 1.0]], 1.0).is_none());
    }
}
