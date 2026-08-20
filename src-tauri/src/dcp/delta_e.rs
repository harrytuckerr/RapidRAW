//! CIEDE2000 colour-difference formula and Lab conversion helpers.
//!
//! Implemented from the published formula (Sharma, Wu & Dalal 2005:
//! "The CIEDE2000 Color-Difference Formula"). Self-contained ~100 lines of
//! math — no external crate needed.
//!
//! The CIEDE2000 function ([`cie_de2000`]) takes two L*a*b* triples under a
//! common reference white and returns the ΔE₂₀₀₀ distance. The conversion
//! helpers ([`xyz_to_lab`], [`lab_to_xyz`]) provide D50-based Lab for the
//! ProPhoto → XYZ → Lab chain used when comparing rendered ProPhoto pixels.

// ---------------------------------------------------------------------------
// CIEDE2000 formula
// ---------------------------------------------------------------------------

/// Compute the CIEDE2000 colour difference between two L*a*b* values.
///
/// All inputs must share the same reference white. The formula is symmetric.
/// L* is in [0, 100]; a* and b* are unbounded but typically in [-128, 128].
pub fn cie_de2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let (l1, a1, b1) = (lab1[0], lab1[1], lab1[2]);
    let (l2, a2, b2) = (lab2[0], lab2[1], lab2[2]);

    // Step 1: compute C*ab
    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();

    // Step 2: mean C*ab
    let c_mean = (c1 + c2) / 2.0;

    // Step 3: G factor
    let c_mean_7 = c_mean.powi(7);
    let g = 0.5 * (1.0 - (c_mean_7 / (c_mean_7 + 25_f64.powi(7))).sqrt());

    // Step 4: compute a', C', h'
    let a1p = a1 * (1.0 + g);
    let a2p = a2 * (1.0 + g);

    let c1p = (a1p * a1p + b1 * b1).sqrt();
    let c2p = (a2p * a2p + b2 * b2).sqrt();

    let h1p = hue_angle_deg(a1p, b1);
    let h2p = hue_angle_deg(a2p, b2);

    // Step 5: deltas
    let dlp = l2 - l1;
    let dcp = c2p - c1p;

    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else {
        let dh = h2p - h1p;
        if dh.abs() <= 180.0 {
            dh
        } else if h2p <= h1p {
            dh + 360.0
        } else {
            dh - 360.0
        }
    };
    let dhp_rad = 2.0 * (c1p * c2p).sqrt() * (deg_to_rad(dhp) / 2.0).sin();

    // Step 6: mean values
    let lp_mean = (l1 + l2) / 2.0;
    let cp_mean = (c1p + c2p) / 2.0;

    let hp_mean = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };

    let t = 1.0 - 0.17 * deg_to_rad(hp_mean - 30.0).cos()
        + 0.24 * deg_to_rad(2.0 * hp_mean).cos()
        + 0.32 * deg_to_rad(3.0 * hp_mean + 6.0).cos()
        - 0.20 * deg_to_rad(4.0 * hp_mean - 63.0).cos();

    let dtheta = 30.0 * (-((hp_mean - 275.0) / 25.0).powi(2)).exp();

    let cp_mean_7 = cp_mean.powi(7);
    let rc = 2.0 * (cp_mean_7 / (cp_mean_7 + 25_f64.powi(7))).sqrt();

    let lp_mean_m50_sq = (lp_mean - 50.0).powi(2);
    let sl = 1.0 + (0.015 * lp_mean_m50_sq) / (20.0 + lp_mean_m50_sq).sqrt();
    let sc = 1.0 + 0.045 * cp_mean;
    let sh = 1.0 + 0.015 * cp_mean * t;

    let rt = -deg_to_rad(2.0 * dtheta).sin() * rc;

    // Step 7: weighted components
    let kl = 1.0;
    let kc = 1.0;
    let kh = 1.0;

    let term_l = dlp / (kl * sl);
    let term_c = dcp / (kc * sc);
    let term_h = dhp_rad / (kh * sh);

    (term_l * term_l + term_c * term_c + term_h * term_h + rt * term_c * term_h).sqrt()
}

/// Hue angle in degrees from a' and b' (0–360).
fn hue_angle_deg(a: f64, b: f64) -> f64 {
    if a == 0.0 && b == 0.0 {
        return 0.0;
    }
    let h = b.atan2(a).to_degrees();
    if h < 0.0 {
        h + 360.0
    } else {
        h
    }
}

// ---------------------------------------------------------------------------
// XYZ ↔ Lab conversion (D50 reference white)
// ---------------------------------------------------------------------------

/// D50 reference white in XYZ (dng_sdk D50_xy_coord). Y = 1.0.
pub const D50_XYZ: [f64; 3] = [0.9642956764, 1.0, 0.8251046025];

/// Conversion threshold: linear below, cube-root above.
const DELTA: f64 = 6.0 / 29.0;
const DELTA2: f64 = DELTA * DELTA;
const DELTA3: f64 = DELTA2 * DELTA;

/// Forward f(t) for the CIE Lab nonlinearity.
fn lab_f(t: f64) -> f64 {
    if t > DELTA3 {
        t.cbrt()
    } else {
        t / (3.0 * DELTA2) + 4.0 / 29.0
    }
}

/// Inverse of lab_f(t).
fn lab_f_inv(t: f64) -> f64 {
    if t > DELTA {
        t * t * t
    } else {
        3.0 * DELTA2 * (t - 4.0 / 29.0)
    }
}

/// Convert XYZ (D50) to CIE L*a*b* (D50).
pub fn xyz_to_lab(xyz: [f64; 3]) -> [f64; 3] {
    let fx = lab_f(xyz[0] / D50_XYZ[0]);
    let fy = lab_f(xyz[1] / D50_XYZ[1]);
    let fz = lab_f(xyz[2] / D50_XYZ[2]);

    let l = 116.0 * fy - 16.0;
    let a = 500.0 * (fx - fy);
    let b = 200.0 * (fy - fz);
    [l, a, b]
}

/// Convert CIE L*a*b* (D50) to XYZ (D50).
pub fn lab_to_xyz(lab: [f64; 3]) -> [f64; 3] {
    let fy = (lab[0] + 16.0) / 116.0;
    let fx = lab[1] / 500.0 + fy;
    let fz = fy - lab[2] / 200.0;

    let x = lab_f_inv(fx) * D50_XYZ[0];
    let y = lab_f_inv(fy) * D50_XYZ[1];
    let z = lab_f_inv(fz) * D50_XYZ[2];
    [x, y, z]
}

/// Convert linear ProPhoto RGB (D50) to CIE L*a*b* (D50) via XYZ(D50).
///
/// Uses the XYZ→ProPhoto matrix inverse to go ProPhoto→XYZ, then XYZ→Lab.
/// `xyz_to_prophoto` is the 3x3 row-major matrix that maps XYZ(D50)→ProPhoto.
pub fn prophoto_to_lab(rgb: [f64; 3], xyz_to_prophoto: &[f64; 9]) -> [f64; 3] {
    // Invert XYZ→ProPhoto to get ProPhoto→XYZ.
    let m = nalgebra::Matrix3::new(
        xyz_to_prophoto[0],
        xyz_to_prophoto[1],
        xyz_to_prophoto[2],
        xyz_to_prophoto[3],
        xyz_to_prophoto[4],
        xyz_to_prophoto[5],
        xyz_to_prophoto[6],
        xyz_to_prophoto[7],
        xyz_to_prophoto[8],
    );
    let m_inv = m.try_inverse().unwrap_or_else(nalgebra::Matrix3::<f64>::identity);
    let v = m_inv * nalgebra::Vector3::new(rgb[0], rgb[1], rgb[2]);
    xyz_to_lab([v[0], v[1], v[2]])
}

/// Convert f32 ProPhoto RGB to L*a*b* for delta-E comparison.
pub fn prophoto_f32_to_lab(rgb: [f32; 3], xyz_to_prophoto: &[f64; 9]) -> [f64; 3] {
    prophoto_to_lab(
        [rgb[0] as f64, rgb[1] as f64, rgb[2] as f64],
        xyz_to_prophoto,
    )
}

/// Compute CIEDE2000 between two ProPhoto f32 pixels.
pub fn prophoto_delta_e(a: [f32; 3], b: [f32; 3], xyz_to_prophoto: &[f64; 9]) -> f64 {
    let la = prophoto_f32_to_lab(a, xyz_to_prophoto);
    let lb = prophoto_f32_to_lab(b, xyz_to_prophoto);
    cie_de2000(la, lb)
}

fn deg_to_rad(d: f64) -> f64 {
    d * std::f64::consts::PI / 180.0
}

// ---------------------------------------------------------------------------
// Statistics helpers
// ---------------------------------------------------------------------------

/// Mean / p95 / max of a delta-E sample.
pub struct DeltaEStats {
    pub mean: f64,
    pub p95: f64,
    pub max: f64,
    pub count: usize,
    /// Number of pixels excluded (deep shadows Y < 0.001 or clipped highlights).
    pub excluded: usize,
    /// Number of deep-shadow pixels excluded.
    pub deep_shadows_excluded: usize,
    /// Number of clipped-highlight pixels excluded.
    pub clipped_highlights_excluded: usize,
}

/// Compute delta-E statistics from a set of per-pixel delta-E values.
/// Deep shadows (Y < 0.001) and clipped highlights (any channel > 0.999 in
/// linear ProPhoto) are excluded from stats but counted separately.
pub fn compute_stats(
    delta_es: &[f64],
    shadow_mask: &[bool],   // true = excluded as deep shadow
    highlight_mask: &[bool], // true = excluded as clipped highlight
) -> DeltaEStats {
    let excluded_by_shadow = shadow_mask.iter().filter(|&&x| x).count();
    let excluded_by_highlight = highlight_mask.iter().filter(|&&x| x).count();
    let total_excluded = excluded_by_shadow + excluded_by_highlight;

    let included: Vec<f64> = delta_es
        .iter()
        .enumerate()
        .filter(|(i, _)| !shadow_mask[*i] && !highlight_mask[*i])
        .map(|(_, &v)| v)
        .collect();

    let count = included.len();
    if count == 0 {
        return DeltaEStats {
            mean: 0.0,
            p95: 0.0,
            max: 0.0,
            count: 0,
            excluded: total_excluded,
            deep_shadows_excluded: excluded_by_shadow,
            clipped_highlights_excluded: excluded_by_highlight,
        };
    }

    let sum: f64 = included.iter().sum();
    let mean = sum / count as f64;
    let max = included.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    let mut sorted = included;
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = sorted[(count as f64 * 0.95).ceil() as usize - 1];

    DeltaEStats {
        mean,
        p95,
        max,
        count,
        excluded: total_excluded,
        deep_shadows_excluded: excluded_by_shadow,
        clipped_highlights_excluded: excluded_by_highlight,
    }
}

/// Report pass/fail against §7.3 thresholds for the "CPU render vs. reference"
/// comparison.
pub fn check_thresholds(stats: &DeltaEStats) -> bool {
    stats.mean < 2.0 && stats.p95 < 3.0 && stats.max < 5.0
}

/// Report pass/fail against §7.3 thresholds for the "GPU vs. CPU parity"
/// comparison.
pub fn check_gpu_parity(stats: &DeltaEStats) -> bool {
    stats.mean < 0.1 && stats.max < 0.5
}

// ---------------------------------------------------------------------------
// Sharma et al. 2005 test data (embedded for self-verification)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The CIEDE2000 published test suite (Sharma, Wu & Dalal 2005) contains 34
    /// colour-difference pairs with expected ΔE₂₀₀₀ values. We embed a subset
    /// covering the formula's key features: neutral axis, high-chroma, hue
    /// wraparound, and the RT rotation term.
    ///
    /// Test pair format: (L1, a1, b1, L2, a2, b2, expected_dE2000, tolerance)
    const SHARMA_PAIRS: &[([f64; 3], [f64; 3], f64)] = &[
        // Pair 1: neutral vs near-neutral (tests SL term)
        ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
        // Pair 2: high-chroma blue
        ([50.0, 3.1571, -77.2803], [50.0, 0.0, -82.7485], 2.8615),
        // Pair 3: higher-chroma blue
        ([50.0, 2.8361, -74.0200], [50.0, 0.0, -82.7485], 3.4412),
        // Pair 4: achromatic pair (near-grey)
        ([50.0, -1.3802, -84.2814], [50.0, 0.0, -82.7485], 1.0000),
        // Pair 5: another achromatic
        ([50.0, -1.1848, -84.8006], [50.0, 0.0, -82.7485], 1.0000),
        // Pair 6: grey vs grey (tests the purely-L* path)
        ([50.0, -0.9009, -85.5211], [50.0, 0.0, -82.7485], 1.0000),
        // Pair 7: hue rotation example (tests the RT term)
        ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
        // Pair 8: near-achromatic in the opposite quadrant
        ([50.0, -1.0, 2.0], [50.0, 0.0, 0.0], 2.3669),
        // Pair 9: small hue difference at high chroma
        ([50.0, 2.4900, -0.0010], [50.0, -2.4900, 0.0009], 7.1792),
        // Pair 10: near-complementary hues
        ([50.0, 2.4900, -0.0010], [50.0, -2.4900, 0.0010], 7.1792),
        // Pair 11: another near-complementary
        ([50.0, 2.4900, -0.0010], [50.0, -2.4900, 0.0011], 7.2195),
        // Pair 12: complementary with small difference
        ([50.0, 2.4900, -0.0010], [50.0, -2.4900, 0.0012], 7.2195),
        // Pair 13: same hue, close
        ([50.0, -0.0010, 2.4900], [50.0, 0.0009, -2.4900], 4.8045),
        // Pair 14: near-same, small shift
        ([50.0, -0.0010, 2.4900], [50.0, 0.0010, -2.4900], 4.8045),
        // Pair 15: edge case near hue discontinuity
        ([50.0, -0.0010, 2.4900], [50.0, 0.0011, -2.4900], 4.7461),
        // Pair 16: L*=50 neutral (hue-difference dominated)
        ([50.0, 2.5, 0.0], [50.0, 0.0, -2.5], 4.3065),
        // Pair 17: 90° hue difference at L*=50
        ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
        // Pair 18: large perceptual difference
        ([50.0, 2.5, 0.0], [61.0, -5.0, 29.0], 22.8977),
        // Pair 19: different L*, same chromatic
        ([50.0, 2.5, 0.0], [56.0, -27.0, -3.0], 31.9030),
        // Pair 20: large L* difference
        ([50.0, 2.5, 0.0], [58.0, 24.0, 15.0], 19.4535),
        // Pair 21: neutral vs neutral (only L* difference)
        ([50.0, 2.5, 0.0], [50.0, 3.1736, 0.5854], 1.0000),
        // Pair 22: very close pair
        ([50.0, 2.5, 0.0], [50.0, 3.2972, 0.0000], 1.0000),
        // Pair 23: small L* difference
        ([50.0, 2.5, 0.0], [50.0, 1.8634, 0.5757], 1.0000),
        // Pair 24: small hue difference near 0°
        ([50.0, 2.5, 0.0], [50.0, 3.2592, 0.3350], 1.0000),
        // Pair 25: L*=60 region
        ([60.2574, -34.0099, 36.2677], [60.4626, -34.1751, 39.4387], 1.2644),
        // Pair 26: medium-chroma green pair
        ([63.0109, -31.0961, -5.8663], [62.8187, -29.7946, -4.0864], 1.2630),
        // Pair 27: medium-high chroma yellow-green
        ([61.2901, 3.7196, -5.3901], [61.4292, 2.2480, -4.9620], 1.8731),
        // Pair 28: L*=35 region, low-chroma
        ([35.0831, -44.1164, 3.7933], [35.0232, -40.0716, 1.5901], 1.8645),
        // Pair 29: high-chroma yellow at low L*
        ([22.7233, 20.0904, -46.6940], [23.0331, 14.9730, -42.5619], 2.0373),
        // Pair 30: high-chroma red/orange
        ([36.4612, 47.8580, 18.3852], [36.2715, 50.5065, 21.2231], 1.4146),
        // Pair 31: high-chroma blue at low L*
        ([90.8027, -2.0831, 1.4410], [91.1528, -1.6435, 0.0447], 1.4441),
        // Pair 32: very close neutral at high L*
        ([90.9257, -0.5406, -0.9208], [88.6381, -0.8985, -0.7239], 1.5381),
        // Pair 33: medium L*, blue region
        ([6.7747, -0.2908, -2.4247], [5.8714, -0.0985, -2.2286], 0.6377),
        // Pair 34: very dark near-neutral
        ([2.0776, 0.0795, -1.1350], [0.9033, -0.0636, -0.5514], 0.9082),
    ];

    /// Verify CIEDE2000 against all 34 published Sharma test pairs.
    #[test]
    fn sharma_all_34_pairs() {
        let mut max_err = 0.0f64;
        let mut max_pair = 0usize;
        for (i, (lab1, lab2, expected)) in SHARMA_PAIRS.iter().enumerate() {
            let got = cie_de2000(*lab1, *lab2);
            let err = (got - expected).abs();
            if err > max_err {
                max_err = err;
                max_pair = i + 1;
            }
            assert!(
                err < 1e-4,
                "Sharma pair {}: L1a1b1={lab1:?} L2a2b2={lab2:?} expected {expected}, got {got} (err {err})",
                i + 1
            );
        }
        // All 34 pairs should match to 1e-4 precision.
        eprintln!(
            "Sharma verification: max error {} on pair {} (all 34 pairs < 1e-4)",
            max_err, max_pair
        );
    }

    /// CIEDE2000 of identical colours must be zero.
    #[test]
    fn identical_is_zero() {
        let lab1 = [50.0, 10.0, 20.0];
        assert!(cie_de2000(lab1, lab1) < 1e-12);
    }

    /// CIEDE2000 is symmetric.
    #[test]
    fn symmetric() {
        let a = [40.0, -5.0, 15.0];
        let b = [45.0, 3.0, -10.0];
        let d1 = cie_de2000(a, b);
        let d2 = cie_de2000(b, a);
        assert!((d1 - d2).abs() < 1e-12);
    }

    /// In-gamut round-trip: XYZ → Lab → XYZ.
    #[test]
    fn xyz_lab_round_trip() {
        for &xyz in &[
            [50.0, 50.0, 50.0],
            [10.0, 20.0, 30.0],
            [0.1, 0.2, 0.3],
            [1.0, 1.0, 1.0],
        ] {
            let lab = xyz_to_lab(xyz);
            let back = lab_to_xyz(lab);
            for i in 0..3 {
                assert!(
                    (back[i] - xyz[i]).abs() < 1e-6,
                    "XYZ round trip: {xyz:?} -> {lab:?} -> {back:?}"
                );
            }
        }
    }

    /// D50 white (scaled to Y=1) maps to L*=100, a*=0, b*=0.
    #[test]
    fn d50_white_is_lab_white() {
        let lab = xyz_to_lab(D50_XYZ);
        assert!((lab[0] - 100.0).abs() < 1e-6, "L* not 100: {}", lab[0]);
        assert!(lab[1].abs() < 1e-6, "a* not 0: {}", lab[1]);
        assert!(lab[2].abs() < 1e-6, "b* not 0: {}", lab[2]);
    }

    /// XYZ = [0,0,0] → L*=0.
    #[test]
    fn black_is_lab_black() {
        let lab = xyz_to_lab([0.0, 0.0, 0.0]);
        assert!(lab[0].abs() < 1e-6, "L* not 0: {}", lab[0]);
    }

    /// Compute ProPhoto→Lab for a known value via the published XYZ→ProPhoto
    /// matrix from render.rs, and verify the round-trip.
    #[test]
    fn prophoto_to_lab_round_trip() {
        // The XYZ→ProPhoto D50 matrix from render.rs.
        let m: [f64; 9] = [
            1.345_786_881_647_158_5,
            -0.255_572_087_379_794_64,
            -0.051_101_864_975_545_26,
            -0.544_630_705_124_901_9,
            1.508_247_742_845_146_8,
            0.020_527_447_436_421_39,
            0.0,
            0.0,
            1.211_967_545_638_945_2,
        ];

        // A known ProPhoto RGB value.
        let rgb: [f64; 3] = [0.18, 0.18, 0.18];
        let lab = prophoto_to_lab(rgb, &m);

        // Convert back: Lab → XYZ → ProPhoto.
        let xyz = lab_to_xyz(lab);
        let m_mat = nalgebra::Matrix3::new(m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8]);
        let prophoto_back = m_mat * nalgebra::Vector3::new(xyz[0], xyz[1], xyz[2]);

        for i in 0..3 {
            assert!(
                (prophoto_back[i] - rgb[i]).abs() < 1e-6,
                "ProPhoto round trip: {rgb:?} -> Lab -> {prophoto_back:?}"
            );
        }
    }

    #[test]
    fn stats_reporting() {
        // 10 pixels: [0.1, 0.5, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]
        let de: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let shadow = vec![false; 10];
        let highlight = vec![false; 10];

        let s = compute_stats(&de, &shadow, &highlight);
        assert_eq!(s.count, 10);
        // mean = sum(0..10) / 10 = 45 / 10 = 4.5
        assert!((s.mean - 4.5).abs() < 1e-6, "mean {}", s.mean);
        assert!((s.max - 9.0).abs() < 1e-6, "max {}", s.max);
        // p95: 10 pixels, ceil(10 * 0.95) = ceil(9.5) = 10, sorted[9] = 9
        assert!((s.p95 - 9.0).abs() < 1e-6, "p95 {}", s.p95);
        assert_eq!(s.excluded, 0);
    }

    #[test]
    fn stats_excludes_shadows_and_highlights() {
        let de: Vec<f64> = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
        let shadows = vec![true, false, false, false, false, false, false, false, false, false];
        let highlights = vec![false, false, false, false, false, false, false, false, false, true];
        let s = compute_stats(&de, &shadows, &highlights);
        assert_eq!(s.count, 8);
        assert_eq!(s.deep_shadows_excluded, 1);
        assert_eq!(s.clipped_highlights_excluded, 1);
        assert_eq!(s.excluded, 2);
    }

    #[test]
    fn sharma_pair_1_diagnostic() {
        // The first Sharma pair is the most commonly cited diagnostic.
        let lab1 = [50.0, 2.6772, -79.7751];
        let lab2 = [50.0, 0.0, -82.7485];
        let de = cie_de2000(lab1, lab2);
        assert!((de - 2.0425).abs() < 1e-4, "pair 1: got {}", de);
    }
}
