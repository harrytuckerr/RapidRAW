//! Illuminant interpolation and CCT maths for the DCP render chain.
//!
//! Implements `COBALT_DCP_SPEC.md` §4.2: mapping illuminant enums to correlated
//! colour temperature, the iterative CCT solve for the camera neutral,
//! the mireds interpolation weight, and element-wise matrix interpolation.
//!
//! Every function here runs **once per profile** in `DcpRenderer::new`, never
//! per pixel, so clarity is valued over speed.
//!
//! ## Robertson CCT — verified equivalent to dng_sdk
//!
//! The 31-entry isotherm table is the dng_sdk `kTempTable`, indexed by mireds
//! (NOT Kelvin). The (u, v) coordinates are CIE 1960 UCS. The distance formula
//! includes the `sqrt(1+t²)` normalisation, which matches dng_sdk's
//! `dng_temperature::Set_xy_coord` exactly. Max divergence across 8 test
//! chromaticities: 0.000000 K. Reference: W2_REMEDIATION.md Appendix A.

use crate::dcp::DcpError;
use crate::dcp::model::{Illuminant, Mat3};

// ---- Robertson isotherm table (dng_sdk kTempTable) -------------------------

/// Robertson 31-entry isotherm table (Wyszecki & Stiles, Table 1(3.11)).
/// Fields: (reciprocal temperature in MIREDS, u_1960, v_1960, isotherm slope t).
/// Identical to dng_sdk's `kTempTable` in dng_temperature.cpp.
const ROBERTSON: [(f64, f64, f64, f64); 31] = [
    (0.0, 0.18006, 0.26352, -0.24341),
    (10.0, 0.18066, 0.26589, -0.25479),
    (20.0, 0.18133, 0.26846, -0.26876),
    (30.0, 0.18208, 0.27119, -0.28539),
    (40.0, 0.18293, 0.27407, -0.30470),
    (50.0, 0.18388, 0.27709, -0.32675),
    (60.0, 0.18494, 0.28021, -0.35156),
    (70.0, 0.18611, 0.28342, -0.37915),
    (80.0, 0.18740, 0.28668, -0.40955),
    (90.0, 0.18880, 0.28997, -0.44278),
    (100.0, 0.19032, 0.29326, -0.47888),
    (125.0, 0.19462, 0.30141, -0.58204),
    (150.0, 0.19962, 0.30921, -0.70471),
    (175.0, 0.20525, 0.31647, -0.84901),
    (200.0, 0.21142, 0.32312, -1.01820),
    (225.0, 0.21807, 0.32909, -1.21680),
    (250.0, 0.22511, 0.33439, -1.45120),
    (275.0, 0.23247, 0.33904, -1.72980),
    (300.0, 0.24010, 0.34308, -2.06370),
    (325.0, 0.24792, 0.34655, -2.46810),
    (350.0, 0.25591, 0.34951, -2.96410),
    (375.0, 0.26400, 0.35200, -3.58140),
    (400.0, 0.27218, 0.35407, -4.36330),
    (425.0, 0.28039, 0.35577, -5.37620),
    (450.0, 0.28863, 0.35714, -6.72620),
    (475.0, 0.29685, 0.35823, -8.59550),
    (500.0, 0.30505, 0.35907, -11.32400),
    (525.0, 0.31320, 0.35968, -15.62800),
    (550.0, 0.32129, 0.36011, -23.32500),
    (575.0, 0.32931, 0.36038, -40.77000),
    (600.0, 0.33724, 0.36051, -116.45000),
];

// ---- CIE 1960 UCS helpers ---------------------------------------------------

/// CIE 1960 UCS. Note `6y`, NOT `9y` — `9y` is CIE 1976 v prime and produces
/// CCTs wrong by thousands of kelvin.
#[inline]
pub fn xy_to_uv_1960(x: f64, y: f64) -> (f64, f64) {
    let d = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / d, 6.0 * y / d)
}

/// From XYZ directly: u = 4X/(X+15Y+3Z), v = 6Y/(X+15Y+3Z).
#[inline]
pub fn xyz_to_uv_1960(x: f64, y: f64, z: f64) -> (f64, f64) {
    let d = x + 15.0 * y + 3.0 * z;
    (4.0 * x / d, 6.0 * y / d)
}

// ---- CCT (correlated colour temperature) -----------------------------------

/// Correlated colour temperature (kelvin) for a chromaticity.
/// Clamps to the table endpoints (~1667 K .. inf) rather than failing.
pub fn cct_of_xy(x: f64, y: f64) -> f64 {
    let (u, v) = xy_to_uv_1960(x, y);
    let mut prev = 0.0f64;

    for i in 0..ROBERTSON.len() {
        let (mired, ui, vi, t) = ROBERTSON[i];
        // Signed PERPENDICULAR distance to the isotherm through (ui,vi), slope t.
        // The /sqrt(1+t²) is required: without it the per-row scaling differs and
        // the interpolation weight is wrong (small error, but wrong).
        let d = ((v - vi) - t * (u - ui)) / (1.0 + t * t).sqrt();

        if i > 0 && d * prev <= 0.0 {
            let m0 = ROBERTSON[i - 1].0;
            // Guard the degenerate case where both distances are zero.
            let denom = prev - d;
            let f = if denom.abs() < 1e-30 {
                0.0
            } else {
                prev / denom
            };
            let mired_out = m0 + f * (mired - m0);
            // Interpolate in MIREDS, then invert. Never interpolate in kelvin.
            return 1.0e6 / mired_out.max(1e-9);
        }
        prev = d;
    }
    // Past the hot end of the table (mired -> 0): clamp to the first entry.
    if prev > 0.0 {
        1.0e6 / ROBERTSON[0].0.max(1e-9)
    } else {
        1.0e6 / ROBERTSON[30].0
    }
}

/// Correlated colour temperature of the calibration illuminant, in Kelvin.
/// `Illuminant::correlated_temp()` covers the codes the spec mandates (§4.2.1).
pub fn cct_of_illuminant(ill: Illuminant) -> Result<f64, DcpError> {
    ill.correlated_temp().ok_or_else(|| DcpError::InvalidValue {
        field: "CalibrationIlluminant",
        detail: format!(
            "no correlated colour temperature known for code {}",
            ill.code()
        ),
    })
}

// ---- interpolation weight, matrix interpolation, neutral solve -------------

/// Dual-illuminant interpolation weight `g`, in RECIPROCAL temperature (mireds).
/// `g = 1` selects illuminant 1 entirely, `g = 0` selects illuminant 2.
/// REQUIRES `t1 < t2`; sort the illuminants (and their matrices together) at load.
pub fn interpolation_weight(temp: f64, t1: f64, t2: f64) -> f64 {
    if (t1 - t2).abs() < 1e-9 {
        return 1.0;
    }
    if temp <= t1 {
        return 1.0;
    }
    if temp >= t2 {
        return 0.0;
    }
    ((1.0 / temp) - (1.0 / t2)) / ((1.0 / t1) - (1.0 / t2))
}

/// Element-wise interpolation: `g * m1 + (1 - g) * m2` (§4.2.4).
pub fn interpolate_matrix(m1: &Mat3, m2: &Mat3, g: f64) -> Mat3 {
    let g = g as f32;
    m1 * g + m2 * (1.0 - g)
}

/// Solve for the xy chromaticity of a camera neutral. Mirrors dng_sdk NeutralToXY.
///
/// Corrects the spec's claim of "3 iterations is sufficient": dng_sdk runs up to
/// 30 passes against epsilon 1e-7 and averages the last two if it oscillates.
/// Measured convergence with the real Cobalt matrices: StdA 3 passes, D50 1,
/// D65 4, D55 5. A hard 3-iteration cap silently returns wrong temperatures.
pub fn neutral_to_xy(neutral: [f64; 3], cm1: &Mat3, cm2: &Mat3, t1: f64, t2: f64) -> (f64, f64) {
    const MAX_PASSES: usize = 30;
    const EPS: f64 = 1.0e-7;
    let mut last = (0.34567, 0.35850); // D50 start, as dng_sdk does

    for pass in 0..MAX_PASSES {
        let g = interpolation_weight(cct_of_xy(last.0, last.1), t1, t2);
        let m = interpolate_matrix(cm1, cm2, g); // g*CM1 + (1-g)*CM2 (XYZ -> camera)
        let m_inv = m
            .cast::<f64>()
            .try_inverse()
            .unwrap_or_else(nalgebra::Matrix3::<f64>::identity);
        let xyz = m_inv * nalgebra::Vector3::new(neutral[0], neutral[1], neutral[2]);
        let s = xyz.x + xyz.y + xyz.z;
        if s.abs() < 1e-12 {
            return last;
        } // degenerate guard
        let mut next = (xyz.x / s, xyz.y / s);

        if (next.0 - last.0).abs() + (next.1 - last.1).abs() < EPS {
            return next;
        }
        if pass == MAX_PASSES - 1 {
            // oscillating: average, as dng_sdk does
            next = ((last.0 + next.0) * 0.5, (last.1 + next.1) * 0.5);
        }
        last = next;
    }
    last
}

// ---- daylight / blackbody locus helpers ------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcp::model::Mat3;

    fn identity() -> Mat3 {
        Mat3::identity()
    }

    #[test]
    fn cct_of_xy_d65() {
        // D65 (x=0.31272, y=0.32903) should give ~6502.4 K (brief acceptance vector).
        let t = cct_of_xy(0.31272, 0.32903);
        assert!(
            (t - 6502.39).abs() < 15.0,
            "D65 CCT = {t}, expected ~6502.39"
        );
    }

    #[test]
    fn cct_of_xy_std_a() {
        // StdA (x=0.44757, y=0.40745) should give ~2855.8 K.
        let t = cct_of_xy(0.44757, 0.40745);
        assert!(
            (t - 2855.76).abs() < 15.0,
            "StdA CCT = {t}, expected ~2855.76"
        );
    }

    #[test]
    fn cct_of_xy_d50() {
        // D50 (x=0.34567, y=0.35850) should give ~5001.8 K.
        let t = cct_of_xy(0.34567, 0.35850);
        assert!(
            (t - 5001.80).abs() < 15.0,
            "D50 CCT = {t}, expected ~5001.80"
        );
    }

    #[test]
    fn cct_of_xy_d55() {
        // D55 (x=0.33242, y=0.34743) should give ~5501.5 K.
        let t = cct_of_xy(0.33242, 0.34743);
        assert!(
            (t - 5501.50).abs() < 15.0,
            "D55 CCT = {t}, expected ~5501.50"
        );
    }

    #[test]
    fn cct_of_xy_d75() {
        // D75 (x=0.29902, y=0.31485) should give ~7506.0 K.
        let t = cct_of_xy(0.29902, 0.31485);
        assert!(
            (t - 7506.00).abs() < 15.0,
            "D75 CCT = {t}, expected ~7506.00"
        );
    }

    #[test]
    fn interpolation_weight_at_endpoints() {
        // At the illuminant temperatures the weight is exactly 1 and 0.
        assert!((interpolation_weight(2856.0, 2856.0, 6504.0) - 1.0).abs() < 1e-9);
        assert!((interpolation_weight(6504.0, 2856.0, 6504.0) - 0.0).abs() < 1e-9);
        // Midpoint in mireds -> 0.5.
        let mid = 1.0 / (0.5 * (1.0 / 2856.0 + 1.0 / 6504.0));
        assert!((interpolation_weight(mid, 2856.0, 6504.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn interpolation_weight_clamped() {
        // Far outside the range clamps to 0/1 rather than overshooting.
        assert!(interpolation_weight(1000.0, 2856.0, 6504.0) <= 1.0);
        assert!(interpolation_weight(20000.0, 2856.0, 6504.0) >= 0.0);
    }

    #[test]
    fn interpolation_weight_single_illuminant() {
        // Equal temperatures -> 1.0 regardless of t.
        assert_eq!(interpolation_weight(5000.0, 2856.0, 2856.0), 1.0);
    }

    #[test]
    fn interpolate_matrix_midpoint() {
        let a = identity();
        let b = identity() * 3.0;
        let m = interpolate_matrix(&a, &b, 0.5);
        assert!((m[(0, 0)] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn neutral_to_xy_single_illuminant() {
        // A single (or coincident) illuminant: the neutral, passed through the
        // (identity) ColorMatrix and normalised, should round-trip to the
        // illuminant's CCT. Use StdA's XYZ (Y=1) as the camera neutral so the
        // identity matrix preserves it.
        let stda_xyz = [1.0987, 1.0, 0.3556]; // StdA xy=(0.4476,0.4074), Y=1
        let (x, y) = neutral_to_xy(stda_xyz, &identity(), &identity(), 2856.0, 2856.0);
        let t = cct_of_xy(x, y);
        assert!(
            (t - 2856.0).abs() < 50.0,
            "single-illuminant round-trip CCT = {t}"
        );
    }
}
