//! Illuminant interpolation and CCT maths for the DCP render chain.
//!
//! Implements `COBALT_DCP_SPEC.md` §4.2: mapping illuminant enums to correlated
//! colour temperature, the iterative CCT solve for the camera neutral,
//! the mireds interpolation weight, and element-wise matrix interpolation.
//!
//! Every function here runs **once per profile** in `DcpRenderer::new`, never
//! per pixel, so clarity is valued over speed.

use crate::dcp::DcpError;
use crate::dcp::model::{Illuminant, Mat3};

/// Robertson (1968) Planckian-locus table. Each entry is the `(u, v)` of a
/// point on the blackbody locus plus the reciprocal slope `m` of its
/// isotemperature line. The temperatures live in [`ROBERTSON_TEMP`].
///
/// This is the standard published Robertson dataset (also used by
/// colour-science and many open-source RAW developers). Interpolating between
/// the bracketing isotemperature lines yields the correlated colour
/// temperature for an arbitrary chromaticity.
const ROBERTSON_UV: [(f64, f64, f64); 31] = [
    (0.180064, 0.263215, -0.24341),
    (0.180637, 0.265276, -0.25480),
    (0.181268, 0.267255, -0.26876),
    (0.181939, 0.269233, -0.28539),
    (0.182614, 0.271246, -0.30470),
    (0.183341, 0.273291, -0.32675),
    (0.184130, 0.275382, -0.35156),
    (0.184945, 0.277510, -0.37915),
    (0.185800, 0.279681, -0.40955),
    (0.186718, 0.281910, -0.44278),
    (0.187680, 0.284213, -0.47888),
    (0.189563, 0.286958, -0.58204),
    (0.191571, 0.289684, -0.70471),
    (0.193689, 0.292382, -0.84901),
    (0.195893, 0.295053, -1.01820),
    (0.198152, 0.297693, -1.21680),
    (0.200438, 0.300310, -1.45120),
    (0.202729, 0.302926, -1.72980),
    (0.205010, 0.305582, -2.06370),
    (0.207254, 0.308259, -2.46810),
    (0.209448, 0.310952, -2.96410),
    (0.211583, 0.313650, -3.58140),
    (0.213648, 0.316342, -4.36330),
    (0.215628, 0.319036, -5.37620),
    (0.217517, 0.321735, -6.72620),
    (0.219311, 0.324444, -8.59550),
    (0.221008, 0.327156, -11.32400),
    (0.222606, 0.329871, -15.62800),
    (0.224107, 0.332584, -23.32500),
    (0.225500, 0.335298, -40.77000),
    (0.226787, 0.338006, -116.45000),
];

/// Temperatures (K) of the [`ROBERTSON_UV`] locus points, in order.
const ROBERTSON_TEMP: [f64; 31] = [
    1000.0, 1100.0, 1200.0, 1300.0, 1400.0, 1500.0, 1600.0, 1700.0, 1800.0, 1900.0, 2000.0,
    2500.0, 3000.0, 3500.0, 4000.0, 4500.0, 5000.0, 5500.0, 6000.0, 6500.0, 7000.0, 7500.0,
    8000.0, 8500.0, 9000.0, 9500.0, 10000.0, 15000.0, 20000.0, 30000.0, 50000.0,
];

/// Correlated colour temperature of the calibration illuminant, in Kelvin.
/// `Illuminant::correlated_temp()` covers the codes the spec mandates (§4.2.1).
pub fn cct_of_illuminant(ill: Illuminant) -> Result<f64, DcpError> {
    ill.correlated_temp().ok_or_else(|| DcpError::InvalidValue {
        field: "CalibrationIlluminant",
        detail: format!("no correlated colour temperature known for code {}", ill.code()),
    })
}

/// Robertson (1968) correlated colour temperature (K) of an xy chromaticity.
///
/// Converts xy to CIE 1960 UCS (u, v), finds the two bracketing isotemperature
/// lines in the Planckian-locus table, and interpolates in reciprocal
/// temperature.
pub fn cct_of_xy(x: f64, y: f64) -> f64 {
    // xy -> CIE 1960 UCS u,v.
    let denom = -2.0 * x + 12.0 * y + 3.0;
    if denom.abs() < 1e-12 {
        return 6500.0;
    }
    let u = 4.0 * x / denom;
    let v = 6.0 * y / denom;

    // Signed distance to each isotemperature line: d = (u - ui)*m - (v - vi).
    // Find the last index whose distance is negative (the lower bracket).
    let mut idx = 0usize;
    for i in 0..ROBERTSON_UV.len() {
        let (ui, vi, mi) = ROBERTSON_UV[i];
        if (u - ui) * mi - (v - vi) < 0.0 {
            idx = i;
        }
    }

    let t = &ROBERTSON_TEMP;
    let (ui, vi, mi) = ROBERTSON_UV[idx];
    let d = (u - ui) * mi - (v - vi);

    if idx == ROBERTSON_UV.len() - 1 {
        // Past the end of the table; interpolate back towards the previous row.
        let (u2, v2, m2) = ROBERTSON_UV[idx - 1];
        let d_prev = (u - u2) * m2 - (v - v2);
        // Guard against a degenerate divisor.
        let den = d - d_prev;
        if den.abs() < 1e-12 {
            return t[idx];
        }
        1.0 / (1.0 / t[idx] + d / den * (1.0 / t[idx - 1] - 1.0 / t[idx]))
    } else {
        let (u2, v2, m2) = ROBERTSON_UV[idx + 1];
        let d_next = (u - u2) * m2 - (v - v2);
        let den = d - d_next;
        if den.abs() < 1e-12 {
            return t[idx];
        }
        1.0 / (1.0 / t[idx] + d / den * (1.0 / t[idx + 1] - 1.0 / t[idx]))
    }
}

/// CIE daylight-locus xy for a temperature `t >= 4000 K` (CIE 15 / D-series).
///
/// Used only by the no-ForwardMatrix fallback (§4.3) for the calibration
/// illuminant's white point. For temperatures below 4000 K the daylight locus
/// is not defined; callers pass the blackbody approximation from
/// [`xy_of_blackbody`].
pub fn xy_of_daylight(t: f64) -> (f64, f64) {
    let t2 = t * t;
    let t3 = t2 * t;
    let x = if (4000.0..=7000.0).contains(&t) {
        -4.6070e9 / t3 + 2.9678e6 / t2 + 0.09911e3 / t + 0.244063
    } else if t > 7000.0 && t <= 25000.0 {
        -2.0064e9 / t3 + 1.9018e6 / t2 + 0.24748e3 / t + 0.237040
    } else {
        0.3127
    };
    let y = -3.000 * x * x + 2.870 * x - 0.275;
    (x, y)
}

/// Blackbody (Planckian) locus xy for a temperature, via the standard piecewise
/// polynomial fits to the CIE 1931 blackbody locus (Kim et al. 2002).
///
/// Used only by the no-ForwardMatrix fallback (§4.3) to get the calibration
/// illuminant's white point below the daylight locus range (< 4000 K).
pub fn xy_of_blackbody(t: f64) -> (f64, f64) {
    if t >= 4000.0 {
        return xy_of_daylight(t);
    }
    if t >= 2222.0 {
        // 2222-4000 K
        let x = -0.2661239e9 / t.powi(3) - 0.2343589e6 / t.powi(2) + 0.8776956e3 / t + 0.179910;
        let y = -1.1063814 * x.powi(3) - 1.34811020 * x.powi(2) + 2.18555832 * x - 0.20219683;
        (x, y)
    } else {
        // 1667-2222 K
        let x = -0.29502e9 / t.powi(3) - 0.4106640e6 / t.powi(2) + 2.711160e3 / t + 0.188220;
        let y = -0.402532e0 * x.powi(3) + 1.45992e0 * x.powi(2) - 0.93522e0 * x + 0.27766;
        (x, y)
    }
}

/// Interpolation weight in reciprocal temperature (mireds) between illuminant 1
/// (`t1`) and illuminant 2 (`t2`) for a camera-neutral temperature `t`.
///
/// ```
/// g = (1/T - 1/T2) / (1/T1 - 1/T2)
/// ```
/// Clamped to `[0, 1]`. Returns `1.0` when the illuminants coincide (or only
/// one is present), per §4.2.3.
pub fn mireds_weight(t: f64, t1: f64, t2: f64) -> f64 {
    if (t1 - t2).abs() < 1e-6 {
        return 1.0;
    }
    let inv = 1.0 / t;
    let inv1 = 1.0 / t1;
    let inv2 = 1.0 / t2;
    ((inv - inv2) / (inv1 - inv2)).clamp(0.0, 1.0)
}

/// Element-wise interpolation: `g * m1 + (1 - g) * m2` (§4.2.4).
pub fn interpolate_matrix(m1: &Mat3, m2: &Mat3, g: f64) -> Mat3 {
    let g = g as f32;
    m1 * g + m2 * (1.0 - g)
}

/// Solve for the correlated colour temperature of the camera neutral by
/// iterating, matching dng_sdk's `dng_color_spec::FindXYZtoCamera`.
///
/// Each iteration: build the interpolated `ColorMatrix` for a temperature
/// guess, invert it to map the camera neutral to XYZ, reduce to xy, and
/// compute its CCT (Robertson). Three iterations is what dng_sdk uses and is
/// sufficient (spec §4.2.2).
pub fn solve_neutral_cct(
    color_matrix_1: &Mat3,
    color_matrix_2: Option<&Mat3>,
    t1: f64,
    t2: f64,
    as_shot_neutral: [f32; 3],
) -> f64 {
    let cm2 = color_matrix_2.unwrap_or(color_matrix_1);
    // Initial guess: the mireds midpoint between the two illuminants.
    let mut temp = if (t1 - t2).abs() < 1e-6 {
        t1
    } else {
        1.0 / (0.5 * (1.0 / t1 + 1.0 / t2))
    };

    for _ in 0..3 {
        let g = mireds_weight(temp, t1, t2);
        let cm = interpolate_matrix(color_matrix_1, cm2, g);
        // Invert to map camera -> XYZ (f64; the illuminate solve is not the
        // hot path).
        let cm_inv = cm
            .cast::<f64>()
            .try_inverse()
            .unwrap_or_else(nalgebra::Matrix3::<f64>::identity);
        let n = nalgebra::Vector3::new(
            as_shot_neutral[0] as f64,
            as_shot_neutral[1] as f64,
            as_shot_neutral[2] as f64,
        );
        let xyz = cm_inv * n;
        let sum = xyz.x + xyz.y + xyz.z;
        let (x, y) = if sum > 1e-12 {
            (xyz.x / sum, xyz.y / sum)
        } else {
            (0.3127, 0.3290) // D65 white fallback
        };
        temp = cct_of_xy(x, y);
    }
    temp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcp::model::Mat3;

    fn identity() -> Mat3 {
        Mat3::identity()
    }

    #[test]
    fn cct_of_xy_d65() {
        // D65 (x=0.3127, y=0.3290) should give ~6504 K.
        let t = cct_of_xy(0.3127, 0.3290);
        assert!(
            (t - 6504.0).abs() < 60.0,
            "D65 CCT = {t}, expected ~6504"
        );
    }

    #[test]
    fn cct_of_xy_std_a() {
        // StdA (x=0.4476, y=0.4074) should give ~2856 K.
        let t = cct_of_xy(0.4476, 0.4074);
        assert!((t - 2856.0).abs() < 120.0, "StdA CCT = {t}, expected ~2856");
    }

    #[test]
    fn cct_of_xy_d50() {
        // D50 (x=0.3457, y=0.3585) should give ~5003 K.
        let t = cct_of_xy(0.3457, 0.3585);
        assert!((t - 5003.0).abs() < 100.0, "D50 CCT = {t}, expected ~5003");
    }

    #[test]
    fn mireds_weight_at_endpoints() {
        // At the illuminant temperatures the weight is exactly 1 and 0.
        assert!((mireds_weight(2856.0, 2856.0, 6504.0) - 1.0).abs() < 1e-9);
        assert!((mireds_weight(6504.0, 2856.0, 6504.0) - 0.0).abs() < 1e-9);
        // Midpoint in mireds -> 0.5.
        let mid = 1.0 / (0.5 * (1.0 / 2856.0 + 1.0 / 6504.0));
        assert!((mireds_weight(mid, 2856.0, 6504.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn mireds_weight_clamped() {
        // Far outside the range clamps to 0/1 rather than overshooting.
        assert!(mireds_weight(1000.0, 2856.0, 6504.0) <= 1.0);
        assert!(mireds_weight(20000.0, 2856.0, 6504.0) >= 0.0);
    }

    #[test]
    fn mireds_weight_single_illuminant() {
        // Equal temperatures -> 1.0 regardless of t.
        assert_eq!(mireds_weight(5000.0, 2856.0, 2856.0), 1.0);
    }

    #[test]
    fn interpolate_matrix_midpoint() {
        let a = identity();
        let b = identity() * 3.0;
        let m = interpolate_matrix(&a, &b, 0.5);
        assert!((m[(0, 0)] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn solve_neutral_cct_single_illuminant() {
        // A single (or coincident) illuminant collapses to that temperature.
        let t = solve_neutral_cct(&identity(), Some(&identity()), 2856.0, 2856.0, [0.5, 1.0, 0.5]);
        assert!((t - 2856.0).abs() < 1.0, "single-illuminant CCT = {t}");
    }
}
