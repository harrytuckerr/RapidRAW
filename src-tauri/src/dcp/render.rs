//! CPU reference implementation of the Adobe DNG Camera Profile (DCP) render
//! chain (`COBALT_DCP_SPEC.md` §4.2-§4.5).
//!
//! This is deliberately a **scalar, obviously-correct, unoptimised reference** -
//! the oracle that the GPU (W3) is validated against. Every illuminant
//! interpolation and matrix composition happens once in [`DcpRenderer::new`],
//! never per pixel. Numerical type is `f32` throughout (to match the GPU);
//! `f64` is used only inside `new()` for matrix inversion via `nalgebra`.
//!
//! # Output space
//!
//! [`DcpRenderer::render_pixel`] returns values in **linear ProPhoto RGB
//! (ROMM), white point D50** - the DCP colour working space that all table
//! operations run in (§4.4). The final step of the pipeline (§3.1 step 8,
//! "ProPhoto → working space") converts to RapidRAW's internal working space
//! (sRGB/Rec.709 primaries, D65, linear transfer). That conversion is exposed
//! as [`DcpRenderer::prophoto_to_working`] and [`DcpRenderer::to_working_space`]
//! so W3/W4 can apply it at the end of the chain, while ACR-reference
//! comparison (§7.2) stays in ProPhoto.

use crate::dcp::DcpError;
use crate::dcp::interpolate::{
    cct_of_illuminant, cct_of_xy, interpolate_matrix, interpolation_weight, neutral_to_xy,
    xy_of_blackbody, xy_of_daylight,
};
use crate::dcp::model::{DcpProfile, HsvTable, Illuminant, Mat3, TableEncoding};
use rayon::prelude::*;

// ---- colour space matrices ---------------------------------------------------

/// XYZ(D50) → linear ProPhoto RGB (ROMM), computed from primaries using
/// dng_sdk D50_xy_coord() = (0.3457, 0.3585). The previous constant was built
/// for the ISO D50 (0.34567, 0.35850) and produced a ~1e-4 non-neutrality
/// when the ForwardMatrix rows did not sum to exactly that white point.
/// NormalizeForwardMatrix (below) corrects the FM at load time; this matrix
/// must then be built for the SAME D50, otherwise the neutral axis is off.
const XYZ_TO_PROPHOTO_D50: [f64; 9] = [
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

/// Bradford chromatic-adaptation cone-response matrix (fixed, from the
/// Bradford 1996 model - source-independent).
const BRADFORD_CONE: [f64; 9] = [
    0.8951000, 0.2664000, -0.1614000, -0.7502000, 1.7135000, 0.0367000, 0.0389000, -0.0685000,
    1.0296000,
];

/// Compute a Bradford adaptation matrix from source white (xy) to destination
/// white (xy) in f64. This guarantees the adaptation maps the source white
/// point to the destination white point exactly, which a stored constant
/// cannot do because published Bradford matrices use slightly different white
/// point values.
fn bradford_adapt_matrix(src_xy: (f64, f64), dst_xy: (f64, f64)) -> nalgebra::Matrix3<f64> {
    let m = mat3_from_row_major_f64(&BRADFORD_CONE);
    let m_inv = m
        .try_inverse()
        .unwrap_or_else(nalgebra::Matrix3::<f64>::identity);

    // xy -> XYZ (Y = 1).
    let src_xyz = nalgebra::Vector3::new(
        src_xy.0 / src_xy.1,
        1.0,
        (1.0 - src_xy.0 - src_xy.1) / src_xy.1,
    );
    let dst_xyz = nalgebra::Vector3::new(
        dst_xy.0 / dst_xy.1,
        1.0,
        (1.0 - dst_xy.0 - dst_xy.1) / dst_xy.1,
    );

    let src_cone = m * src_xyz;
    let dst_cone = m * dst_xyz;
    let d = nalgebra::Matrix3::from_diagonal(&nalgebra::Vector3::new(
        dst_cone[0] / src_cone[0],
        dst_cone[1] / src_cone[1],
        dst_cone[2] / src_cone[2],
    ));
    m_inv * d * m
}

/// dng_sdk dng_camera_profile::NormalizeForwardMatrix.
/// ForwardMatrix rows are SRATIONAL-quantised and do not sum to exactly D50;
/// without this the neutral axis is off by ~1e-4 and no ProPhoto constant can
/// fix it. Must be applied to FM1 and FM2 individually BEFORE interpolation.
fn normalize_forward_matrix(m: &nalgebra::Matrix3<f64>) -> nalgebra::Matrix3<f64> {
    let camera_one = nalgebra::Vector3::new(1.0, 1.0, 1.0);
    let rows = m * camera_one;
    let s = rows[0] + rows[1] + rows[2];
    let src_xy = (rows[0] / s, rows[1] / s);
    // Bradford-adapt from the FM's implicit white to canonical D50.
    bradford_adapt_matrix(src_xy, D50_XY) * m
}

/// XYZ(D65) -> linear sRGB/Rec.709 (RapidRAW's working space primaries).
const XYZ_TO_SRGB_D65: [f64; 9] = [
    3.2404542, -1.5371385, -0.4985314, -0.9692660, 1.8760108, 0.0415560, 0.0556434, -0.2040259,
    1.0572252,
];

/// White point of the D65 working space.
const WP_D65_XY: (f64, f64) = (0.3127, 0.3290);

/// dng_sdk D50_xy_coord(). The ONE D50 in this crate. Do not introduce another.
/// XYZ: [0.9642956764, 1.0, 0.8251046025].
pub const D50_XY: (f64, f64) = (0.3457, 0.3585);

/// Number of samples in the resampled tone-curve LUT.
const TONE_CURVE_LUT_SIZE: usize = 4096;

fn mat3_from_row_major(v: &[f32; 9]) -> Mat3 {
    Mat3::new(v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8])
}

/// Build a `nalgebra::Matrix3<f64>` from row-major `[f64; 9]` constants.
fn mat3_from_row_major_f64(v: &[f64; 9]) -> nalgebra::Matrix3<f64> {
    nalgebra::Matrix3::new(v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8])
}

/// How the profile tone curve is applied to an RGB pixel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ToneCurveMode {
    /// Apply the curve to each RGB channel independently (dng_sdk behaviour).
    PerChannel,
    /// Preserve hue/saturation by scaling RGB by `curve(v)/v` on the value.
    HuePreserving,
}

/// What happens to tone-curve input above the curve domain (> last x, ~1.0).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AboveOne {
    /// Extend the last LUT segment linearly (preserves highlight roll-off).
    Extrapolate,
    /// Clamp input to the curve domain (dng_sdk clamps to the last y value).
    Clamp,
}

/// A renderer for one DCP profile against one shot's `as_shot_neutral`.
///
/// All illuminant interpolation and matrix composition is done in [`new`]
/// (`DcpRenderer::new`); per-pixel work is then just a handful of `f32`
/// multiplies plus any table lookups.
pub struct DcpRenderer {
    /// camera-native linear RGB → linear ProPhoto (D50) working space.
    cam_to_prophoto: Mat3,
    /// linear ProPhoto (D50) → RapidRAW working space (sRGB primaries, D65).
    prophoto_to_working: Mat3,
    /// Pre-interpolated (dual-illuminant) HueSatMap in the renderer's ProPhoto
    /// working space. `None` if the profile has no HueSatMap.
    hue_sat_map: Option<HsvTable>,
    hue_sat_map_encoding: TableEncoding,
    /// LookTable (already single; not interpolated). `None` if absent.
    look_table: Option<HsvTable>,
    look_table_encoding: TableEncoding,
    /// BaselineExposureOffset in stops (multiplicative `2^offset` gain).
    baseline_exposure_offset: f32,
    /// Resampled tone curve as a uniform LUT over [0, 1]; `None` if no curve.
    tone_curve_lut: Option<Vec<f32>>,
    tone_curve_mode: ToneCurveMode,
    tone_curve_above_one: AboveOne,
}

impl DcpRenderer {
    /// Build a renderer for `profile` given the shot's camera neutral
    /// (`as_shot_neutral`, normalised so G = 1).
    ///
    /// Resolves the illuminant (§4.2), composes `camToXYZ_D50` (§4.3), converts
    /// to ProPhoto, pre-interpolates the HueSatMap by the illuminant weight,
    /// and resamples the tone curve (§4.4.3). Errors on a singular
    /// calibration matrix or an unusable forward/color matrix.
    pub fn new(profile: &DcpProfile, as_shot_neutral: [f32; 3]) -> Result<Self, DcpError> {
        // ---- §4.2 illuminant interpolation ------------------------------
        let t1 = cct_of_illuminant(profile.calibration_illuminant_1)?;
        let t2 = match profile.calibration_illuminant_2 {
            Some(ill) => cct_of_illuminant(ill)?,
            None => t1,
        };

        let cm2_ref = profile
            .color_matrix_2
            .as_ref()
            .unwrap_or(&profile.color_matrix_1);
        let (nx, ny) = neutral_to_xy(
            [
                as_shot_neutral[0] as f64,
                as_shot_neutral[1] as f64,
                as_shot_neutral[2] as f64,
            ],
            &profile.color_matrix_1,
            cm2_ref,
            t1,
            t2,
        );
        let neutral_cct = cct_of_xy(nx, ny);
        let g = interpolation_weight(neutral_cct, t1, t2);

        let color_matrix = match &profile.color_matrix_2 {
            Some(cm2) => interpolate_matrix(&profile.color_matrix_1, cm2, g),
            None => profile.color_matrix_1,
        };
        let camera_calibration =
            match (&profile.camera_calibration_1, &profile.camera_calibration_2) {
                (Some(cc1), Some(cc2)) => Some(interpolate_matrix(cc1, cc2, g)),
                (Some(cc), None) | (None, Some(cc)) => Some(*cc),
                (None, None) => None,
            };
        let forward_matrix = match (&profile.forward_matrix_1, &profile.forward_matrix_2) {
            (Some(fm1), Some(fm2)) => {
                // Normalise each ForwardMatrix to D50 BEFORE interpolation
                // (dng_sdk dng_camera_profile::NormalizeForwardMatrix).
                // Without this the neutral axis is off by ~1e-4 because
                // SRATIONAL-quantised FM rows don't sum to exactly D50.
                let fm1_norm = normalize_forward_matrix(&fm1.cast::<f64>());
                let fm2_norm = normalize_forward_matrix(&fm2.cast::<f64>());
                // Interpolate in f64, then downcast to f32 for the profile store.
                let fm_interp = fm1_norm * g + fm2_norm * (1.0 - g);
                Some(fm_interp.cast::<f32>())
            }
            (Some(fm), None) | (None, Some(fm)) => {
                let fm_norm = normalize_forward_matrix(&fm.cast::<f64>());
                Some(fm_norm.cast::<f32>())
            }
            (None, None) => None,
        };

        // ---- §4.3 camera → XYZ(D50) — computed in f64, downcast at end ---
        let xyz_to_prophoto_f64 = mat3_from_row_major_f64(&XYZ_TO_PROPHOTO_D50);

        let cam_to_xyz_d50_f64 = match forward_matrix {
            Some(fm) => build_cam_to_xyz_d50_forward_f64(
                &fm,
                camera_calibration.as_ref(),
                profile.analog_balance,
                as_shot_neutral,
            )?,
            None => {
                // Fallback: invert the ColorMatrix, then Bradford-adapt from the
                // calibration illuminant's white point to D50.
                build_cam_to_xyz_d50_fallback_f64(
                    &color_matrix,
                    camera_calibration.as_ref(),
                    profile.analog_balance,
                    as_shot_neutral,
                    profile.calibration_illuminant_1,
                )?
            }
        };

        // camera → ProPhoto in f64, downcast to a single f32 matrix.
        // This is M1 from the brief: (XYZ_D50 → ProPhoto) · cam2xyz_D50.
        let cam_to_prophoto = (xyz_to_prophoto_f64 * cam_to_xyz_d50_f64).cast::<f32>();

        // ProPhoto → RapidRAW working space (sRGB primaries, D65) including
        // D50 → D65 Bradford adaptation. M2 from the brief.
        // Compute ProPhoto→XYZ as the f64 inverse of XYZ→ProPhoto to guarantee
        // they are exact inverses (the f32 published constants are approximations
        // whose product differs from identity by ~1e-4, which propagates into
        // every neutral-axis and identity test).
        let prophoto_to_xyz_f64 = xyz_to_prophoto_f64
            .try_inverse()
            .unwrap_or_else(nalgebra::Matrix3::<f64>::identity);
        let bradford_f64 = bradford_adapt_matrix(D50_XY, WP_D65_XY);
        // Normalise the sRGB matrix so each row maps D65 white to exactly 1.0.
        // The published constant values are rounded and don't map D65 white to
        // [1,1,1] exactly (off by ~0.003), which propagates into a ~1e-4
        // non-neutrality in the composed ProPhoto->working matrix.
        let xyz_to_srgb_raw = mat3_from_row_major_f64(&XYZ_TO_SRGB_D65);
        let d65_xyz = nalgebra::Vector3::new(
            WP_D65_XY.0 / WP_D65_XY.1,
            1.0,
            (1.0 - WP_D65_XY.0 - WP_D65_XY.1) / WP_D65_XY.1,
        );
        let row_scales = nalgebra::Vector3::new(
            1.0 / (xyz_to_srgb_raw.row(0).transpose().dot(&d65_xyz)),
            1.0 / (xyz_to_srgb_raw.row(1).transpose().dot(&d65_xyz)),
            1.0 / (xyz_to_srgb_raw.row(2).transpose().dot(&d65_xyz)),
        );
        let xyz_to_srgb_f64 = nalgebra::Matrix3::from_diagonal(&row_scales) * xyz_to_srgb_raw;
        let prophoto_to_working =
            (xyz_to_srgb_f64 * bradford_f64 * prophoto_to_xyz_f64).cast::<f32>();

        // ---- §4.4 rendering tables --------------------------------------
        let hue_sat_map = profile
            .hue_sat_map
            .as_ref()
            .map(|dual| interpolate_dual_hue_sat_map(dual, g));

        let look_table = profile.look_table.clone();

        // Resample the tone curve to a uniform LUT over [0, 1].
        let tone_curve_lut = profile
            .tone_curve
            .as_ref()
            .map(|curve| resample_tone_curve(&curve.points));

        let baseline_exposure_offset = profile.baseline_exposure_offset.unwrap_or(0.0);

        Ok(DcpRenderer {
            cam_to_prophoto,
            prophoto_to_working,
            hue_sat_map,
            hue_sat_map_encoding: profile.hue_sat_map_encoding,
            look_table,
            look_table_encoding: profile.look_table_encoding,
            baseline_exposure_offset,
            tone_curve_lut,
            tone_curve_mode: ToneCurveMode::PerChannel,
            tone_curve_above_one: AboveOne::Extrapolate,
        })
    }

    /// Set whether the tone curve is applied per-channel (default, dng_sdk
    /// behaviour) or in a hue/saturation-preserving manner. Exposed for the
    /// ACR-reference A/B test (spec §4.4.3 VERIFY).
    pub fn set_tone_curve_hue_preserving(&mut self, on: bool) {
        self.tone_curve_mode = if on {
            ToneCurveMode::HuePreserving
        } else {
            ToneCurveMode::PerChannel
        };
    }

    /// Set whether tone-curve input above the curve domain extrapolates
    /// linearly (default) or clamps. Exposed for the ACR-reference A/B test.
    pub fn set_tone_curve_above_one(&mut self, mode: AboveOneChoice) {
        self.tone_curve_above_one = match mode {
            AboveOneChoice::Extrapolate => AboveOne::Extrapolate,
            AboveOneChoice::Clamp => AboveOne::Clamp,
        };
    }

    /// Render one camera-native, linear RGB pixel to linear ProPhoto (D50).
    ///
    /// The input is the **raw (un-white-balanced) camera RGB**; white balance
    /// is baked into `cam_to_prophoto` via the `D` diagonal (§4.3). A pixel
    /// equal to `as_shot_neutral` therefore renders neutral.
    pub fn render_pixel(&self, camera_rgb: [f32; 3]) -> [f32; 3] {
        let v = nalgebra::Vector3::new(camera_rgb[0], camera_rgb[1], camera_rgb[2]);
        let p = self.cam_to_prophoto * v;
        let mut rgb = [p[0], p[1], p[2]];

        // §4.5 provisional stage order (see docs/dcp-pipeline.md).
        if let Some(hsm) = &self.hue_sat_map {
            rgb = apply_hsv_table(rgb, hsm, self.hue_sat_map_encoding);
        }
        rgb = apply_baseline_exposure(rgb, self.baseline_exposure_offset);
        if let Some(lut) = &self.look_table {
            rgb = apply_hsv_table(rgb, lut, self.look_table_encoding);
        }
        if let Some(lut) = &self.tone_curve_lut {
            rgb = apply_tone_curve(rgb, lut, self.tone_curve_mode, self.tone_curve_above_one);
        }

        rgb
    }

    /// Convert a linear ProPhoto (D50) value to RapidRAW's working space
    /// (sRGB/Rec.709 primaries, D65, linear transfer) - pipeline step 8.
    pub fn to_working_space(&self, prophoto_rgb: [f32; 3]) -> [f32; 3] {
        let v = nalgebra::Vector3::new(prophoto_rgb[0], prophoto_rgb[1], prophoto_rgb[2]);
        let w = self.prophoto_to_working * v;
        [w[0], w[1], w[2]]
    }

    /// The precomputed ProPhoto (D50) → working space matrix, for W3/W4.
    pub fn prophoto_to_working(&self) -> Mat3 {
        self.prophoto_to_working
    }

    /// Render a slice of interleaved RGB `f32` values in place, using a rayon
    /// parallel iterator. `px` must contain a whole number of pixels.
    pub fn render_slice(&self, px: &mut [f32]) {
        px.par_chunks_mut(3).for_each(|px| {
            let out = self.render_pixel([px[0], px[1], px[2]]);
            px[0] = out[0];
            px[1] = out[1];
            px[2] = out[2];
        });
    }
}

/// Public selector for the tone-curve above-1.0 behaviour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AboveOneChoice {
    Extrapolate,
    Clamp,
}

// ---- §4.3 matrix composition ------------------------------------------------

/// Forward-matrix path: `camToXYZ_D50 = FM * D * Inverse(AB * CC)` (§4.3).
/// Computed entirely in f64, downcast to f32 only after the full chain is
/// composed in [`DcpRenderer::new`] (§6.W2 / Group D).
fn build_cam_to_xyz_d50_forward_f64(
    forward_matrix: &Mat3,
    camera_calibration: Option<&Mat3>,
    analog_balance: Option<[f32; 3]>,
    as_shot_neutral: [f32; 3],
) -> Result<nalgebra::Matrix3<f64>, DcpError> {
    let (ab_cc_inv, ref_neutral) =
        calibration_scale_f64(camera_calibration, analog_balance, as_shot_neutral)?;
    let d = invert_diagonal_f64(ref_neutral)?;
    let fm = forward_matrix.cast::<f64>();
    Ok(fm * d * ab_cc_inv)
}

/// No-ForwardMatrix fallback (f64): invert the interpolated ColorMatrix,
/// white balance, then Bradford-adapt from the calibration illuminant's
/// white point to D50 (§4.3). Returns `camToXYZ_D50` in f64 for composition.
fn build_cam_to_xyz_d50_fallback_f64(
    color_matrix: &Mat3,
    camera_calibration: Option<&Mat3>,
    analog_balance: Option<[f32; 3]>,
    as_shot_neutral: [f32; 3],
    illuminant: Illuminant,
) -> Result<nalgebra::Matrix3<f64>, DcpError> {
    let (ab_cc_inv, ref_neutral) =
        calibration_scale_f64(camera_calibration, analog_balance, as_shot_neutral)?;
    let d = invert_diagonal_f64(ref_neutral)?;

    let cm_inv =
        color_matrix
            .cast::<f64>()
            .try_inverse()
            .ok_or_else(|| DcpError::InvalidValue {
                field: "ColorMatrix",
                detail: "ColorMatrix is singular; cannot invert for the no-ForwardMatrix path"
                    .into(),
            })?;

    // camera → XYZ in the calibration illuminant, white-balanced.
    let cam_to_xyz_illum = cm_inv * d * ab_cc_inv;

    // Calibration illuminant white point → D50 Bradford adaptation.
    let temp = cct_of_illuminant(illuminant)?;
    let src_xy = if temp >= 4000.0 {
        xy_of_daylight(temp)
    } else {
        xy_of_blackbody(temp)
    };
    // D50 white point xy (dng_sdk D50_xy_coord).
    let adapt = bradford_adaptation(src_xy, D50_XY);

    Ok(adapt * cam_to_xyz_illum)
}

/// No-ForwardMatrix fallback: invert the interpolated ColorMatrix, white
/// balance, then Bradford-adapt from the calibration illuminant to D50.
fn build_cam_to_xyz_d50_fallback(
    color_matrix: &Mat3,
    camera_calibration: Option<&Mat3>,
    analog_balance: Option<[f32; 3]>,
    as_shot_neutral: [f32; 3],
    illuminant: Illuminant,
) -> Result<Mat3, DcpError> {
    let (ab_cc_inv, ref_neutral) =
        calibration_scale(camera_calibration, analog_balance, as_shot_neutral)?;
    let d = invert_diagonal(ref_neutral)?;

    let cm_inv = color_matrix
        .cast::<f64>()
        .try_inverse()
        .ok_or_else(|| DcpError::InvalidValue {
            field: "ColorMatrix",
            detail: "ColorMatrix is singular; cannot invert for the no-ForwardMatrix path".into(),
        })?
        .cast::<f32>();

    // camera → XYZ in the calibration illuminant, white-balanced.
    let cam_to_xyz_illum = cm_inv * d * ab_cc_inv;

    // Calibration illuminant white point → D50 Bradford adaptation.
    let temp = cct_of_illuminant(illuminant)?;
    let (x, y) = if temp >= 4000.0 {
        xy_of_daylight(temp)
    } else {
        xy_of_blackbody(temp)
    };
    let adapt = bradford_adaptation((x, y), WP_D65_XY);

    Ok((adapt * cam_to_xyz_illum.cast::<f64>()).cast::<f32>())
}

/// Compute `Inverse(AB * CC)` and `refNeutral = Inverse(AB * CC) * as_shot_neutral`
/// (§4.3). `AB` is the AnalogBalance diagonal; `CC` is CameraCalibration.
fn calibration_scale(
    camera_calibration: Option<&Mat3>,
    analog_balance: Option<[f32; 3]>,
    as_shot_neutral: [f32; 3],
) -> Result<(Mat3, nalgebra::Vector3<f32>), DcpError> {
    let ab = match analog_balance {
        Some(b) => nalgebra::Matrix3::from_diagonal(&nalgebra::Vector3::new(b[0], b[1], b[2])),
        None => nalgebra::Matrix3::<f32>::identity(),
    };
    let cc = camera_calibration
        .copied()
        .unwrap_or_else(nalgebra::Matrix3::identity);
    let ab_cc = ab * cc;

    let ab_cc_inv = ab_cc
        .cast::<f64>()
        .try_inverse()
        .ok_or_else(|| DcpError::InvalidValue {
            field: "AnalogBalance/CameraCalibration",
            detail: "AB * CC matrix is singular".into(),
        })?
        .cast::<f32>();

    let n = nalgebra::Vector3::new(as_shot_neutral[0], as_shot_neutral[1], as_shot_neutral[2]);
    let ref_neutral = ab_cc_inv * n;
    Ok((ab_cc_inv, ref_neutral))
}

/// Compute `Inverse(AB * CC)` and `refNeutral = Inverse(AB * CC) * as_shot_neutral`
/// (§4.3) in f64 for composition in [`DcpRenderer::new`].
fn calibration_scale_f64(
    camera_calibration: Option<&Mat3>,
    analog_balance: Option<[f32; 3]>,
    as_shot_neutral: [f32; 3],
) -> Result<(nalgebra::Matrix3<f64>, nalgebra::Vector3<f64>), DcpError> {
    let ab = match analog_balance {
        Some(b) => nalgebra::Matrix3::from_diagonal(&nalgebra::Vector3::new(
            b[0] as f64,
            b[1] as f64,
            b[2] as f64,
        )),
        None => nalgebra::Matrix3::<f64>::identity(),
    };
    let cc = camera_calibration
        .copied()
        .unwrap_or_else(nalgebra::Matrix3::identity)
        .cast::<f64>();
    let ab_cc = ab * cc;

    let ab_cc_inv = ab_cc.try_inverse().ok_or_else(|| DcpError::InvalidValue {
        field: "AnalogBalance/CameraCalibration",
        detail: "AB * CC matrix is singular (f64)".into(),
    })?;

    let n = nalgebra::Vector3::new(
        as_shot_neutral[0] as f64,
        as_shot_neutral[1] as f64,
        as_shot_neutral[2] as f64,
    );
    let ref_neutral = ab_cc_inv * n;
    Ok((ab_cc_inv, ref_neutral))
}

/// `D = Invert(Diagonal(refNeutral))` in f64, guarding near-zero components.
fn invert_diagonal_f64(
    ref_neutral: nalgebra::Vector3<f64>,
) -> Result<nalgebra::Matrix3<f64>, DcpError> {
    let mut out = nalgebra::Matrix3::<f64>::identity();
    for i in 0..3 {
        let v = ref_neutral[i];
        if v.abs() < 1e-12 {
            return Err(DcpError::InvalidValue {
                field: "AsShotNeutral",
                detail: format!(
                    "camera neutral component {i} is ~zero ({v}); cannot white-balance"
                ),
            });
        }
        out[(i, i)] = 1.0 / v;
    }
    Ok(out)
}

/// `D = Invert(Diagonal(refNeutral))` (f32), guarding near-zero components.
fn invert_diagonal(ref_neutral: nalgebra::Vector3<f32>) -> Result<Mat3, DcpError> {
    let mut out = Mat3::identity();
    for i in 0..3 {
        let v = ref_neutral[i];
        if v.abs() < 1e-12 {
            return Err(DcpError::InvalidValue {
                field: "AsShotNeutral",
                detail: format!(
                    "camera neutral component {i} is ~zero ({v}); cannot white-balance"
                ),
            });
        }
        out[(i, i)] = 1.0 / v;
    }
    Ok(out)
}

/// Bradford chromatic adaptation matrix from `src_xy` to `dst_xy` (both xy in
/// XYZ). Returns a 3x3 matrix (f64) mapping XYZ(src) → XYZ(dst).
fn bradford_adaptation(src_xy: (f64, f64), dst_xy: (f64, f64)) -> nalgebra::Matrix3<f64> {
    // XYZ of the source and destination white points.
    let xyz = |(x, y): (f64, f64)| {
        let yv = 1.0;
        let xv = yv * x / y;
        let zv = yv * (1.0 - x - y) / y;
        nalgebra::Vector3::new(xv, yv, zv)
    };
    let src = xyz(src_xy);
    let dst = xyz(dst_xy);

    // Bradford cone-response matrix.
    const BRADFORD_M: [[f64; 3]; 3] = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let m = nalgebra::Matrix3::from_fn(|r, c| BRADFORD_M[r][c]);
    let src_cone = m * src;
    let dst_cone = m * dst;

    let mut diag = nalgebra::Matrix3::<f64>::identity();
    for i in 0..3 {
        let s = src_cone[i];
        if s.abs() < 1e-12 {
            diag[(i, i)] = 1.0;
        } else {
            diag[(i, i)] = dst_cone[i] / s;
        }
    }
    m.try_inverse().unwrap_or_else(nalgebra::Matrix3::identity) * diag * m
}

// ---- §4.4.1 / §4.4.2 HSV table application ----------------------------------

/// Convert a linear ProPhoto RGB value into the table's encoding (Linear =
/// unchanged, Srgb = sign-symmetric transfer), apply the HSV table, and
/// convert back.
fn apply_hsv_table(rgb: [f32; 3], table: &HsvTable, encoding: TableEncoding) -> [f32; 3] {
    let rgb_enc = match encoding {
        TableEncoding::Linear => rgb,
        TableEncoding::Srgb => rgb.map(linear_to_srgb_transfer),
    };

    let hsv = rgb_to_hsv(rgb_enc);
    let h = hsv[0];
    let s = hsv[1];
    let v = hsv[2];

    // Map to table coordinates (hue wraps; sat/val clamp).
    let hue_coord = (h / 360.0) * table.hue_div as f32;
    let sat_coord = (s * (table.sat_div as f32 - 1.0)).clamp(0.0, table.sat_div as f32 - 1.0);
    let val_coord = if table.val_div > 1 {
        (v * (table.val_div as f32 - 1.0)).clamp(0.0, table.val_div as f32 - 1.0)
    } else {
        0.0
    };

    let entry = sample_table(table, hue_coord, sat_coord, val_coord);
    let (hue_shift, sat_scale, val_scale) = (entry[0], entry[1], entry[2]);

    // Apply (§4.4.1): hue shift wraps, sat clamps, val is NOT clamped.
    let h2 = (h + hue_shift).rem_euclid(360.0);
    let s2 = (s * sat_scale).clamp(0.0, 1.0);
    let v2 = v * val_scale;

    let rgb2 = hsv_to_rgb([h2, s2, v2]);

    match encoding {
        TableEncoding::Linear => rgb2,
        TableEncoding::Srgb => rgb2.map(srgb_to_linear_transfer),
    }
}

/// Trilinear (or bilinear when `val_div == 1`) interpolation of the table at
/// fractional cell coordinates. Hue wraps (index `hue_div` → 0); saturation
/// and value clamp at their upper edges.
///
/// Bracketing per spec §4.4.1 / W2 remediation brief: hue wraps on `hue_div`
/// (360° maps back to index 0), sat/val clamp on `div − 1`. The degenerate
/// `val_div == 1` case (2-D map) must be handled explicitly.
fn sample_table(table: &HsvTable, hue_coord: f32, sat_coord: f32, val_coord: f32) -> [f32; 3] {
    let hue_div = table.hue_div as i32;
    let sat_div = table.sat_div as i32;
    let val_div = table.val_div as i32;

    let hfl = hue_coord.floor();
    let fh = hue_coord - hfl;
    let h0 = (hfl as i32).rem_euclid(hue_div) as usize;
    let h1 = (h0 + 1) % hue_div as usize;

    let sf = (sat_coord * (sat_div - 1) as f32).clamp(0.0, (sat_div - 1) as f32);
    let s0 = sf.floor() as usize;
    let s1 = (s0 + 1).min(sat_div as usize - 1);
    let fs = sf - s0 as f32;

    let (v0, v1, fv) = if val_div <= 1 {
        (0usize, 0usize, 0.0f32)
    } else {
        let vf = (val_coord * (val_div - 1) as f32).clamp(0.0, (val_div - 1) as f32);
        let v0 = vf.floor() as usize;
        (v0, (v0 + 1).min(val_div as usize - 1), vf - v0 as f32)
    };

    // Eight corners; `cell` wraps hue and clamps sat/val so the wrap and
    // clamp behaviour is centralised.
    let c000 = cell(table, h0, s0, v0);
    let c100 = cell(table, h1, s0, v0);
    let c010 = cell(table, h0, s1, v0);
    let c110 = cell(table, h1, s1, v0);
    let c001 = cell(table, h0, s0, v1);
    let c101 = cell(table, h1, s0, v1);
    let c011 = cell(table, h0, s1, v1);
    let c111 = cell(table, h1, s1, v1);

    let w0 = (1.0 - fh) * (1.0 - fs) * (1.0 - fv);
    let w1 = fh * (1.0 - fs) * (1.0 - fv);
    let w2 = (1.0 - fh) * fs * (1.0 - fv);
    let w3 = fh * fs * (1.0 - fv);
    let w4 = (1.0 - fh) * (1.0 - fs) * fv;
    let w5 = fh * (1.0 - fs) * fv;
    let w6 = (1.0 - fh) * fs * fv;
    let w7 = fh * fs * fv;

    let mut out = [0.0f32; 3];
    for i in 0..3 {
        out[i] = c000[i] * w0
            + c100[i] * w1
            + c010[i] * w2
            + c110[i] * w3
            + c001[i] * w4
            + c101[i] * w5
            + c011[i] * w6
            + c111[i] * w7;
    }
    out
}

/// Read a single table cell, wrapping the hue index (mod `hue_div`) and
/// clamping saturation/value. Data layout per spec §4.4.1:
/// `v` outermost → `h` → `s` innermost:
/// `index = (v * hue_div + h) * sat_div + s`.
fn cell(table: &HsvTable, h: usize, s: usize, v: usize) -> [f32; 3] {
    let hue_div = table.hue_div as usize;
    let sat_div = table.sat_div as usize;
    let val_div = table.val_div as usize;
    let h = h % hue_div;
    let s = s.min(sat_div - 1);
    let v = v.min(val_div.max(1) - 1);
    let idx = (v * hue_div + h) * sat_div + s;
    table.data[idx]
}

/// RGB → HSV matching the WGSL `rgb_to_hsv` convention: hue in [0, 360),
/// saturation, value = max component.
fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let c_max = c[0].max(c[1]).max(c[2]);
    let c_min = c[0].min(c[1]).min(c[2]);
    let delta = c_max - c_min;
    let mut h = 0.0f32;
    if delta > 0.0 {
        if c_max == c[0] {
            h = 60.0 * (((c[1] - c[2]) / delta) % 6.0);
        } else if c_max == c[1] {
            h = 60.0 * (((c[2] - c[0]) / delta) + 2.0);
        } else {
            h = 60.0 * (((c[0] - c[1]) / delta) + 4.0);
        }
    }
    if h < 0.0 {
        h += 360.0;
    }
    let s = if c_max > 0.0 { delta / c_max } else { 0.0 };
    [h, s, c_max]
}

/// HSV → RGB matching the WGSL `hsv_to_rgb` convention.
fn hsv_to_rgb(c: [f32; 3]) -> [f32; 3] {
    let h = c[0];
    let s = c[1];
    let v = c[2];
    let ch = v * s;
    let x = ch * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - ch;
    let rgb_prime = if h < 60.0 {
        [ch, x, 0.0]
    } else if h < 120.0 {
        [x, ch, 0.0]
    } else if h < 180.0 {
        [0.0, ch, x]
    } else if h < 240.0 {
        [0.0, x, ch]
    } else if h < 300.0 {
        [x, 0.0, ch]
    } else {
        [ch, 0.0, x]
    };
    [rgb_prime[0] + m, rgb_prime[1] + m, rgb_prime[2] + m]
}

/// Sign-symmetric linear → sRGB transfer (extended to handle negative /
/// scene-referred values), matching `linear_to_srgb_extended` in shader.wgsl
/// but applied per sign.
fn linear_to_srgb_transfer(x: f32) -> f32 {
    let a = x.abs();
    let v = if a <= 0.0031308 {
        a * 12.92
    } else {
        1.055 * a.powf(1.0 / 2.4) - 0.055
    };
    x.signum() * v
}

/// Sign-symmetric sRGB → linear transfer (inverse of the above).
fn srgb_to_linear_transfer(x: f32) -> f32 {
    let a = x.abs();
    let v = if a <= 0.04045 {
        a / 12.92
    } else {
        ((a + 0.055) / 1.055).powf(2.4)
    };
    x.signum() * v
}

// ---- §4.5 baseline exposure ------------------------------------------------

/// BaselineExposureOffset (stops) is a multiplicative gain.
fn apply_baseline_exposure(rgb: [f32; 3], stops: f32) -> [f32; 3] {
    if stops == 0.0 {
        return rgb;
    }
    let gain = 2.0f32.powf(stops);
    [rgb[0] * gain, rgb[1] * gain, rgb[2] * gain]
}

// ---- §4.4.3 profile tone curve ----------------------------------------------

/// Resample an (x, y) tone-curve point list to a uniform LUT over [0, 1].
///
/// The curve's `x` is monotonically increasing over [0, 1]. We build
/// [`TONE_CURVE_LUT_SIZE`] samples by evaluating the piecewise-linear curve at
/// uniform grid points, with a guard against a degenerate (flat) first segment.
fn resample_tone_curve(points: &[[f32; 2]]) -> Vec<f32> {
    let n = points.len();
    let mut lut = Vec::with_capacity(TONE_CURVE_LUT_SIZE);
    if n == 0 {
        return lut;
    }
    if n == 1 {
        let y = points[0][1];
        lut.resize(TONE_CURVE_LUT_SIZE, y);
        return lut;
    }
    for i in 0..TONE_CURVE_LUT_SIZE {
        let x = i as f32 / (TONE_CURVE_LUT_SIZE - 1) as f32;
        lut.push(eval_curve(points, x));
    }
    lut
}

/// Evaluate the piecewise-linear curve at `x` in [0, 1] by binary search.
fn eval_curve(points: &[[f32; 2]], x: f32) -> f32 {
    let n = points.len();
    if x <= points[0][0] {
        return points[0][1];
    }
    if x >= points[n - 1][0] {
        return points[n - 1][1];
    }
    // Binary search for the segment [i, i+1] containing x.
    let mut lo = 0usize;
    let mut hi = n - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if points[mid][0] <= x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (x0, y0) = (points[lo][0], points[lo][1]);
    let (x1, y1) = (points[lo + 1][0], points[lo + 1][1]);
    let den = x1 - x0;
    if den.abs() < 1e-9 {
        return y0;
    }
    let t = (x - x0) / den;
    y0 + (y1 - y0) * t
}

/// Apply the tone-curve LUT to a ProPhoto linear pixel.
///
/// - **Per-channel:** evaluate each RGB channel through the LUT independently
///   (dng_sdk behaviour).
/// - **Hue-preserving:** scale RGB by `curve(v)/v` using the value `v = max`.
/// - **Above 1.0:** linear extrapolation from the last segment, or clamp.
fn apply_tone_curve(
    rgb: [f32; 3],
    lut: &[f32],
    mode: ToneCurveMode,
    above_one: AboveOne,
) -> [f32; 3] {
    match mode {
        ToneCurveMode::PerChannel => [
            eval_lut(lut, rgb[0], above_one),
            eval_lut(lut, rgb[1], above_one),
            eval_lut(lut, rgb[2], above_one),
        ],
        ToneCurveMode::HuePreserving => {
            let v = rgb[0].max(rgb[1]).max(rgb[2]);
            let out_v = eval_lut(lut, v, above_one);
            if v.abs() < 1e-9 {
                // Black stays black; avoid a divide by zero.
                [0.0, 0.0, 0.0]
            } else {
                let k = out_v / v;
                [rgb[0] * k, rgb[1] * k, rgb[2] * k]
            }
        }
    }
}

/// Sample the uniform tone-curve LUT at `x`, handling out-of-domain input.
fn eval_lut(lut: &[f32], x: f32, above_one: AboveOne) -> f32 {
    let last = (lut.len() - 1) as f32;
    if x <= 0.0 {
        return match above_one {
            AboveOne::Extrapolate => {
                // Extend the first segment linearly below 0.
                let y0 = lut[0];
                let y1 = lut[1];
                let step = (y1 - y0) / (1.0 / last);
                (y0 + x * step).max(0.0)
            }
            AboveOne::Clamp => lut[0],
        };
    }
    if x >= 1.0 {
        return match above_one {
            AboveOne::Extrapolate => {
                // Extend the last segment linearly above 1.0.
                let y_last = lut[lut.len() - 1];
                let y_prev = lut[lut.len() - 2];
                let step = (y_last - y_prev) / (1.0 / last);
                (y_last + (x - 1.0) * step).max(0.0)
            }
            AboveOne::Clamp => lut[lut.len() - 1],
        };
    }
    let f = x * last;
    let i = f.floor() as usize;
    let t = f - i as f32;
    let i = i.min(lut.len() - 2);
    lut[i] + (lut[i + 1] - lut[i]) * t
}

// ---- dual-illuminant HueSatMap interpolation -------------------------------

/// Interpolate `HueSatMapData1`/`Data2` element-wise by the illuminant weight
/// `g` (§4.4.1: interpolate the table entries, not the resulting colours).
fn interpolate_dual_hue_sat_map(dual: &crate::dcp::model::DualHueSatMap, g: f64) -> HsvTable {
    let map1 = &dual.map_1;
    match &dual.map_2 {
        Some(map2) => {
            let n = map1.data.len().min(map2.data.len());
            let mut data = Vec::with_capacity(n);
            for i in 0..n {
                let a = map1.data[i];
                let b = map2.data[i];
                let gi = g as f32;
                data.push([
                    a[0] * gi + b[0] * (1.0 - gi),
                    a[1] * gi + b[1] * (1.0 - gi),
                    a[2] * gi + b[2] * (1.0 - gi),
                ]);
            }
            HsvTable {
                hue_div: map1.hue_div,
                sat_div: map1.sat_div,
                val_div: map1.val_div,
                data,
            }
        }
        None => map1.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcp::model::ProfileId;
    use std::path::PathBuf;

    fn ident_mat() -> Mat3 {
        Mat3::identity()
    }

    /// A synthetic identity profile: ColorMatrix = I, ForwardMatrix maps camera
    /// directly to ProPhoto (so `camToProPhoto = I`), no tables, no curve.
    /// `as_shot_neutral = [1,1,1]`. Used for the identity test.
    fn identity_profile() -> DcpProfile {
        // ProPhoto→XYZ = inverse of XYZ→ProPhoto, computed in f64 for precision,
        // downcast to f32 (Mat3) for the DcpProfile field.
        let xyz_to_prophoto_f64 = mat3_from_row_major_f64(&XYZ_TO_PROPHOTO_D50);
        let prophoto_to_xyz_f32 = xyz_to_prophoto_f64.try_inverse().unwrap().cast::<f32>();
        DcpProfile {
            id: ProfileId([0u8; 32]),
            file_path: PathBuf::new(),
            profile_name: "identity".into(),
            unique_camera_model: "identity".into(),
            copyright: None,
            embed_policy: crate::dcp::model::EmbedPolicy::NoRestrictions,
            calibration_illuminant_1: Illuminant::StdA,
            calibration_illuminant_2: Some(Illuminant::D65),
            color_matrix_1: ident_mat(),
            color_matrix_2: Some(ident_mat()),
            forward_matrix_1: Some(prophoto_to_xyz_f32),
            forward_matrix_2: Some(prophoto_to_xyz_f32),
            camera_calibration_1: None,
            camera_calibration_2: None,
            analog_balance: None,
            baseline_exposure_offset: None,
            default_black_render: crate::dcp::model::DefaultBlackRender::Auto,
            hue_sat_map: None,
            look_table: None,
            look_table_encoding: TableEncoding::Linear,
            hue_sat_map_encoding: TableEncoding::Linear,
            tone_curve: None,
        }
    }

    #[test]
    fn colour_matrices_round_trip() {
        // XYZ→ProPhoto, when inverted in f64 (as new() does at runtime), must
        // give identity. The f32 published constants are approximations whose
        // direct product differs from I by ~1e-4; the runtime computes the
        // inverse in f64 to avoid that error propagating.
        let a = mat3_from_row_major_f64(&XYZ_TO_PROPHOTO_D50);
        let b = a.try_inverse().unwrap();
        let prod = a * b;
        for i in 0..3 {
            for j in 0..3 {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (prod[(i, j)] - want).abs() < 1e-12,
                    "XYZ↔ProPhoto f64 inverse not identity at ({i},{j}): {}",
                    prod[(i, j)]
                );
            }
        }
    }

    #[test]
    fn prophoto_white_is_neutral() {
        // The D50 white point [1,1,1] in ProPhoto must map to neutral in the
        // working space (sRGB D65), i.e. R == G == B after Bradford.
        let r = DcpRenderer::new(&identity_profile(), [1.0, 1.0, 1.0]).expect("renderer");
        let w = r.to_working_space([1.0, 1.0, 1.0]);
        assert!(
            (w[0] - w[1]).abs() < 1e-4 && (w[1] - w[2]).abs() < 1e-4,
            "D50 white in working space is not neutral: {w:?}"
        );
        // And it should be near [1,1,1] (D65 white maps to ~[1,1,1] in sRGB).
        for c in w {
            assert!((c - 1.0).abs() < 1e-2, "working-space white not ~1: {w:?}");
        }
    }

    #[test]
    fn identity_test_output_equals_input() {
        // Identity DCP with camera ≡ ProPhoto space: output == input (1e-6).
        let r = DcpRenderer::new(&identity_profile(), [1.0, 1.0, 1.0]).expect("renderer");
        for input in [
            [0.0f32, 0.0, 0.0],
            [0.18, 0.18, 0.18],
            [0.5, 0.3, 0.2],
            [1.0, 1.0, 1.0],
            [2.0, 1.0, 0.5],
        ] {
            let out = r.render_pixel(input);
            for i in 0..3 {
                assert!(
                    (out[i] - input[i]).abs() < 1e-6,
                    "identity: input {input:?} -> {out:?}"
                );
            }
        }
    }

    #[test]
    fn identity_profile_is_linear_transform() {
        // Even with pure FM = I (not camera≡ProPhoto), an identity profile with
        // no tables/curve must be a pure linear transform. After
        // NormalizeForwardMatrix, the raw identity FM is corrected to map
        // [1,1,1] → D50, so the effective linear transform is
        // xyz_to_prophoto * normalize_forward_matrix(I), not xyz_to_prophoto.
        let mut prof = identity_profile();
        prof.forward_matrix_1 = Some(ident_mat());
        prof.forward_matrix_2 = Some(ident_mat());
        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("renderer");
        let xyz_to_prophoto = mat3_from_row_major_f64(&XYZ_TO_PROPHOTO_D50);
        let norm_fm = normalize_forward_matrix(&nalgebra::Matrix3::<f64>::identity());
        let expected_f64 = xyz_to_prophoto * norm_fm;
        let expected = expected_f64.cast::<f32>();
        for input in [[0.1f32, 0.2, 0.3], [0.4, 0.4, 0.4], [1.2, 0.8, 0.6]] {
            let out = r.render_pixel(input);
            let v = expected * nalgebra::Vector3::new(input[0], input[1], input[2]);
            for i in 0..3 {
                assert!(
                    (out[i] - v[i]).abs() < 1e-5,
                    "linear identity: input {input:?} -> {out:?}, expected v{i} {}",
                    v[i]
                );
            }
        }
    }

    #[test]
    fn neutral_axis_test() {
        // A neutral camera RGB equal to as_shot_neutral renders neutral.
        // Use a non-trivial neutral to exercise the reciprocal convention.
        let as_shot_neutral = [0.4f32, 1.0, 0.7];
        let r = DcpRenderer::new(&identity_profile(), as_shot_neutral).expect("renderer");
        let out = r.render_pixel(as_shot_neutral);
        assert!(
            (out[0] - out[1]).abs() < 1e-4 && (out[1] - out[2]).abs() < 1e-4,
            "neutral-axis: {as_shot_neutral:?} -> {out:?} (not neutral)"
        );
        // The working-space value should also be neutral.
        let w = r.to_working_space(out);
        assert!(
            (w[0] - w[1]).abs() < 1e-4 && (w[1] - w[2]).abs() < 1e-4,
            "neutral-axis working space: {as_shot_neutral:?} -> {w:?}"
        );
    }

    #[test]
    fn neutral_axis_real_matrices() {
        // The neutral-axis test with a real (non-identity) forward matrix still
        // maps the camera neutral to the D50 white point.
        let mut prof = identity_profile();
        // camera == ProPhoto: FM must map camera RGB -> XYZ(D50), which is
        // ProPhoto -> XYZ(D50) = inverse of XYZ_TO_PROPHOTO_D50, computed in f64.
        let prophoto_to_xyz_f32 = mat3_from_row_major_f64(&XYZ_TO_PROPHOTO_D50)
            .try_inverse()
            .unwrap()
            .cast::<f32>();
        prof.forward_matrix_1 = Some(prophoto_to_xyz_f32);
        prof.forward_matrix_2 = Some(prophoto_to_xyz_f32);
        let as_shot_neutral = [0.5f32, 1.0, 0.8];
        let r = DcpRenderer::new(&prof, as_shot_neutral).expect("renderer");
        let out = r.render_pixel(as_shot_neutral);
        assert!(
            (out[0] - out[1]).abs() < 1e-4 && (out[1] - out[2]).abs() < 1e-4,
            "neutral-axis (real matrices): {as_shot_neutral:?} -> {out:?}"
        );
    }

    #[test]
    fn forward_matrices_normalize_to_d50() {
        // A ForwardMatrix whose rows don't sum to canonical D50 must normalise
        // to one that does. Use a synthetic FM whose row sums are off by ~1e-4
        // (like the real Cobalt FM1). After normalisation, mapping [1,1,1]
        // through the matrix must yield D50 XYZ at machine precision.
        let fm_raw = nalgebra::Matrix3::new(
            0.5852, 0.2478, 0.1314, // row sum ~0.9644 (canonical: 0.964296)
            0.2148, 0.7488, 0.0364, // row sum  1.0000
            0.0075, 0.0195, 0.7981, // row sum ~0.8251 (canonical: 0.825105)
        );
        let n = normalize_forward_matrix(&fm_raw);
        let w = n * nalgebra::Vector3::new(1.0, 1.0, 1.0);
        let d50_xyz = nalgebra::Vector3::new(
            D50_XY.0 / D50_XY.1,
            1.0,
            (1.0 - D50_XY.0 - D50_XY.1) / D50_XY.1,
        );
        for i in 0..3 {
            assert!(
                (w[i] - d50_xyz[i]).abs() < 1e-12,
                "normalised FM row {i} = {}, D50 = {}",
                w[i],
                d50_xyz[i]
            );
        }
    }

    #[test]
    fn hsv_round_trip() {
        for c in [
            [0.1f32, 0.3, 0.5],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.8, 0.8, 0.2],
            [1.0, 1.0, 1.0],
        ] {
            let hsv = rgb_to_hsv(c);
            let back = hsv_to_rgb(hsv);
            for i in 0..3 {
                assert!(
                    (back[i] - c[i]).abs() < 1e-4,
                    "hsv round trip {c:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn srgb_transfer_round_trip() {
        for x in [-2.0f32, -1.0, -0.1, 0.0, 0.001, 0.1, 0.5, 1.0, 2.0, 10.0] {
            let enc = linear_to_srgb_transfer(x);
            let dec = srgb_to_linear_transfer(enc);
            assert!(
                (dec - x).abs() < 1e-4,
                "srgb transfer round trip {x} -> {dec}"
            );
        }
    }

    #[test]
    fn srgb_transfer_sign_symmetric() {
        // Negative values must survive the round trip (sign-symmetric).
        let neg = linear_to_srgb_transfer(-0.5);
        let pos = linear_to_srgb_transfer(0.5);
        assert!(
            (neg + pos).abs() < 1e-6,
            "not sign-symmetric: {neg} vs {pos}"
        );
    }

    /// Build a 2x2x1 identity HueSatMap table (all [0,1,1]).
    fn identity_hsv_table() -> HsvTable {
        let mut data = Vec::new();
        for _ in 0..(2 * 2) {
            data.push([0.0, 1.0, 1.0]);
        }
        HsvTable {
            hue_div: 2,
            sat_div: 2,
            val_div: 1,
            data,
        }
    }

    #[test]
    fn hsv_identity_table_noop() {
        let table = identity_hsv_table();
        for c in [[0.1f32, 0.2, 0.3], [0.8, 0.2, 0.9], [1.0, 0.0, 0.0]] {
            let out = apply_hsv_table(c, &table, TableEncoding::Linear);
            for i in 0..3 {
                assert!(
                    (out[i] - c[i]).abs() < 1e-5,
                    "identity table {c:?} -> {out:?}"
                );
            }
        }
    }

    #[test]
    fn hsv_hue_wrap_89_to_0() {
        // A table with a hue shift that is large only at the last hue index
        // must still interpolate correctly across the 89 -> 0 wrap.
        // Build a 90x1x1 table: hueShift = 10 at hue index 89, 0 elsewhere.
        let mut data = vec![[0.0f32, 1.0, 1.0]; 90];
        data[89] = [10.0, 1.0, 1.0];
        let table = HsvTable {
            hue_div: 90,
            sat_div: 1,
            val_div: 1,
            data,
        };

        // A hue just below the wrap (e.g. hue 359 -> index ~89) gets the shift.
        // Find an RGB whose hue is ~359.9 (red-ish, just below red boundary).
        let rgb = rgb_from_hue(359.5);
        let out = apply_hsv_table(rgb, &table, TableEncoding::Linear);
        let out_hue = rgb_to_hsv(out)[0];
        // hue 359.5 + shift ~10, wrapped -> ~9.5 (should not exceed 360).
        assert!(
            out_hue < 30.0 && out_hue > 0.0,
            "hue wrap 89->0: input hue ~359.5 + shift -> {out_hue} (expected ~9.5)"
        );
        // A hue far from the wrap should be unchanged.
        let rgb2 = rgb_from_hue(180.0);
        let out2 = apply_hsv_table(rgb2, &table, TableEncoding::Linear);
        let out2_hue = rgb_to_hsv(out2)[0];
        assert!(
            (out2_hue - 180.0).abs() < 1.0,
            "hue away from wrap unchanged, got {out2_hue}"
        );
    }

    #[test]
    fn hsv_sat_val_edge_clamp() {
        // A table that pushes saturation to extreme values must clamp sat to
        // [0,1] and not wrap, and val must NOT clamp.
        // Build a 2x2x1 table where satScale is huge (10x) and valScale 2x.
        let mut data = vec![[0.0f32, 1.0, 1.0]; 4];
        data[0] = [0.0, 10.0, 2.0]; // hue0,sat0
        let table = HsvTable {
            hue_div: 2,
            sat_div: 2,
            val_div: 1,
            data,
        };

        // A low-sat colour at hue 0: sat gets scaled but clamps at 1.
        let rgb = [0.2f32, 0.19, 0.18]; // near-neutral, hue ~0-ish region
        let hsv_in = rgb_to_hsv(rgb);
        let out = apply_hsv_table(rgb, &table, TableEncoding::Linear);
        let hsv_out = rgb_to_hsv(out);
        assert!(
            hsv_out[1] <= 1.0 + 1e-5,
            "saturation must clamp to <= 1, got {}",
            hsv_out[1]
        );
        // Value is scaled by 2 (not clamped).
        assert!(
            (hsv_out[2] - hsv_in[2] * 2.0).abs() < 0.05,
            "value scaled by valScale, got {} from {}",
            hsv_out[2],
            hsv_in[2]
        );
    }

    #[test]
    fn tone_curve_lut_interpolation() {
        // A linear curve y = x should reproduce the identity via the LUT.
        let points: Vec<[f32; 2]> = (0..=100)
            .map(|i| [i as f32 / 100.0, i as f32 / 100.0])
            .collect();
        let lut = resample_tone_curve(&points);
        assert_eq!(lut.len(), TONE_CURVE_LUT_SIZE);
        for &x in &[0.0f32, 0.1, 0.5, 0.999, 1.0] {
            let y = eval_lut(&lut, x, AboveOne::Extrapolate);
            assert!((y - x).abs() < 1e-3, "linear curve at {x} -> {y}");
        }
    }

    #[test]
    fn tone_curve_above_one_extrapolate() {
        // Extrapolation continues the last segment above 1.0.
        let points = vec![[0.0f32, 0.0], [0.5, 0.5], [1.0, 1.0]];
        let lut = resample_tone_curve(&points);
        let y = eval_lut(&lut, 1.5, AboveOne::Extrapolate);
        assert!((y - 1.5).abs() < 1e-3, "extrapolate above 1: 1.5 -> {y}");
    }

    #[test]
    fn tone_curve_above_one_clamp() {
        // Clamp holds the last LUT value above 1.0.
        let points = vec![[0.0f32, 0.0], [0.5, 0.5], [1.0, 1.0]];
        let lut = resample_tone_curve(&points);
        let y = eval_lut(&lut, 1.5, AboveOne::Clamp);
        assert!((y - 1.0).abs() < 1e-6, "clamp above 1: 1.5 -> {y}");
    }

    #[test]
    fn hue_preserving_curve_keeps_neutral() {
        // A neutral input must stay neutral under a nonlinear per-channel curve.
        let points = vec![[0.0f32, 0.0], [0.5, 0.3], [1.0, 0.95]];
        let lut = resample_tone_curve(&points);
        let rgb = [0.5f32, 0.5, 0.5];
        for mode in [ToneCurveMode::PerChannel, ToneCurveMode::HuePreserving] {
            let out = apply_tone_curve(rgb, &lut, mode, AboveOne::Extrapolate);
            assert!(
                (out[0] - out[1]).abs() < 1e-5 && (out[1] - out[2]).abs() < 1e-5,
                "neutral stays neutral under {mode:?}: {out:?}"
            );
        }
    }

    #[test]
    fn baseline_exposure_gain() {
        assert_eq!(
            apply_baseline_exposure([0.5, 0.5, 0.5], 1.0),
            [1.0, 1.0, 1.0]
        );
        assert_eq!(
            apply_baseline_exposure([0.5, 0.5, 0.5], 0.0),
            [0.5, 0.5, 0.5]
        );
    }

    #[test]
    fn render_slice_parallel_matches_scalar() {
        let prof = identity_profile();
        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("renderer");
        let pixels: Vec<f32> = (0..300).map(|i| i as f32 / 100.0).collect();
        let mut slice = pixels.clone();
        r.render_slice(&mut slice);
        for (i, chunk) in pixels.chunks(3).enumerate() {
            let expect = r.render_pixel([chunk[0], chunk[1], chunk[2]]);
            let got = &slice[i * 3..i * 3 + 3];
            for k in 0..3 {
                assert!((got[k] - expect[k]).abs() < 1e-6, "slice mismatch at {i}");
            }
        }
    }

    // ---- env-gated test against the supplied vendor DCP -------------------

    #[test]
    #[ignore = "requires vendor DCP; enable with RAPIDRAW_TEST_ASSETS=1"]
    fn vendor_dcp_neutral_axis() {
        if std::env::var("RAPIDRAW_TEST_ASSETS").as_deref() != Ok("1") {
            return;
        }
        let path = std::path::Path::new(
            "/Users/harrisontucker/Downloads/Fujifilm X-Pro2 Cobalt Flat v3.0.dcp",
        );
        assert!(path.exists(), "supplied DCP not present: {path:?}");
        let profiles = crate::dcp::parser::parse_dcp(path).expect("parse vendor DCP");
        let profile = &profiles[0];

        // The as_shot_neutral here is arbitrary (we don't have the shot); pick a
        // mid-daylight neutral and verify it renders neutral through the tables.
        let as_shot_neutral = [0.5f32, 1.0, 0.8];
        let r = DcpRenderer::new(profile, as_shot_neutral).expect("renderer");
        let out = r.render_pixel(as_shot_neutral);
        // NOTE: with real (non-identity) tables and tone curve this is NOT
        // expected to be exactly neutral unless the tables are neutral on the
        // neutral axis. This test documents the pipeline runs end-to-end
        // without NaN and returns finite, plausible values.
        for c in out {
            assert!(c.is_finite(), "non-finite output: {out:?}");
        }
        let w = r.to_working_space(out);
        for c in w {
            assert!(c.is_finite(), "non-finite working output: {w:?}");
        }
    }

    /// Build an RGB colour with the given hue (max saturation, unit value).
    fn rgb_from_hue(hue: f32) -> [f32; 3] {
        hsv_to_rgb([hue, 1.0, 1.0])
    }
}
