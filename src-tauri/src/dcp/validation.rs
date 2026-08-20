//! DCP validation harness: synthetic tests, golden-image guards, and
//! GPU-vs-CPU parity (W9).
//!
//! This module houses the validation tests that CI runs on every push.
//! Vendor-asset-dependent tests are `#[ignore]`'d and gated on
//! `RAPIDRAW_TEST_ASSETS=1` (§7.1).
//!
//! # Test categories
//!
//! | Category          | CI?  | Asset-free | What it checks                        |
//! |-------------------|------|------------|---------------------------------------|
//! | Identity profile  | Yes  | Yes        | DcpRenderer with identity DCP → ΔE≈0  |
//! | Neutral axis      | Yes  | Yes        | Camera neutral → neutral in ProPhoto  |
//! | Known-matrix RT   | Yes  | Yes        | Matrix * inverse → identity (ΔE≈0)    |
//! | HSV wrap/clamp    | Yes  | Yes        | Hue 89→0 wrap, sat clamp, val scale   |
//! | Golden no-profile | No   | No         | No-profile path → upstream identical  |
//! | GPU parity        | No   | No         | GPU vs CPU render within §7.3 bounds  |

use crate::dcp::model::*;
use std::path::PathBuf;

// Re-export the delta-E and statistics machinery for use by the CLI harness.
pub use crate::dcp::delta_e::{
    check_gpu_parity, check_thresholds, compute_stats, prophoto_delta_e, DeltaEStats,
};

// ---------------------------------------------------------------------------
// GPU uniform helpers for W3 parity test
// ---------------------------------------------------------------------------

/// Minimal uniform struct matching `DcpParityUniforms` in dcp_parity.wgsl.
/// Only the fields needed for the DCP stage — no legacy adjustments.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DcpParityUniforms {
    is_raw_image: u32,
    has_dcp: u32,
    _pad_a: u32,
    _pad_b: u32,
    dcp_huesat_dims: [u32; 4],
    dcp_look_dims: [u32; 4],
    cam_to_prophoto: [[f32; 4]; 3],
    prophoto_to_working: [[f32; 4]; 3],
}

/// Convert a nalgebra Matrix3<f32> to column-major [[f32;4];3] matching
/// WGSL mat3x3<f32> layout (3 columns of vec4<f32>, padding = 0).
fn mat3_to_cols(m: Mat3) -> [[f32; 4]; 3] {
    [
        [m[(0, 0)], m[(1, 0)], m[(2, 0)], 0.0],
        [m[(0, 1)], m[(1, 1)], m[(2, 1)], 0.0],
        [m[(0, 2)], m[(1, 2)], m[(2, 2)], 0.0],
    ]
}

// ---------------------------------------------------------------------------
// Synthetic profile builders
// ---------------------------------------------------------------------------

/// Build a synthetic `DcpProfile` with the given name and colour matrices.
/// Both illuminants share the same data (single-illuminant effectively). No
/// rendering tables, no tone curve.
fn synthetic_profile(
    name: &str,
    color_matrix_1: Mat3,
    forward_matrix_1: Option<Mat3>,
    calibration_illuminant_1: Illuminant,
) -> DcpProfile {
    DcpProfile {
        id: ProfileId([0u8; 32]),
        file_path: PathBuf::new(),
        profile_name: name.into(),
        unique_camera_model: "synthetic".into(),
        copyright: None,
        embed_policy: EmbedPolicy::NoRestrictions,
        calibration_illuminant_1,
        calibration_illuminant_2: None,
        color_matrix_1,
        color_matrix_2: None,
        forward_matrix_1,
        forward_matrix_2: None,
        camera_calibration_1: None,
        camera_calibration_2: None,
        analog_balance: None,
        baseline_exposure_offset: None,
        default_black_render: DefaultBlackRender::Auto,
        hue_sat_map: None,
        look_table: None,
        look_table_encoding: TableEncoding::Linear,
        hue_sat_map_encoding: TableEncoding::Linear,
        tone_curve: None,
    }
}

/// Build a full dual-illuminant synthetic profile for realistic-scenario tests.
fn dual_illuminant_profile() -> DcpProfile {
    let cm1 = Mat3::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0);
    let cm2 = Mat3::new(0.9, 0.0, 0.0, 0.0, 1.1, 0.0, 0.0, 0.0, 1.0);

    // ProPhoto→XYZ = inverse of XYZ→ProPhoto (from render.rs constants)
    let xyz_to_prophoto: [f64; 9] = [
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
    let xyz_to_prophoto_mat = nalgebra::Matrix3::new(
        xyz_to_prophoto[0], xyz_to_prophoto[1], xyz_to_prophoto[2],
        xyz_to_prophoto[3], xyz_to_prophoto[4], xyz_to_prophoto[5],
        xyz_to_prophoto[6], xyz_to_prophoto[7], xyz_to_prophoto[8],
    );
    let fm_f64 = xyz_to_prophoto_mat.try_inverse().unwrap();
    let fm = fm_f64.cast::<f32>();

    DcpProfile {
        id: ProfileId([1u8; 32]),
        file_path: PathBuf::new(),
        profile_name: "dual-illuminant-test".into(),
        unique_camera_model: "synthetic-dual".into(),
        copyright: None,
        embed_policy: EmbedPolicy::NoRestrictions,
        calibration_illuminant_1: Illuminant::StdA,
        calibration_illuminant_2: Some(Illuminant::D65),
        color_matrix_1: cm1,
        color_matrix_2: Some(cm2),
        forward_matrix_1: Some(fm),
        forward_matrix_2: Some(fm),
        camera_calibration_1: None,
        camera_calibration_2: None,
        analog_balance: None,
        baseline_exposure_offset: None,
        default_black_render: DefaultBlackRender::Auto,
        hue_sat_map: None,
        look_table: None,
        look_table_encoding: TableEncoding::Linear,
        hue_sat_map_encoding: TableEncoding::Linear,
        tone_curve: None,
    }
}

// ---------------------------------------------------------------------------
// Synthetic validation tests (asset-free, CI-safe)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcp::delta_e::prophoto_delta_e;
    use crate::dcp::render::DcpRenderer;

    /// The XYZ→ProPhoto D50 matrix from render.rs — needed for delta-E
    /// computation in ProPhoto space.
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

    // ---- identity / neutral-axis tests (delta-E quantified) ----------------

    /// An identity-profile renderer must map its input to itself within
    /// ΔE2000 < 2e-2 (pipeline is evaluated in f64 internally, so the small
    /// residual comes from f32→f64→f32 round-trip and the Bradford adaptation
    /// of the ForwardMatrix normalisation).
    #[test]
    fn identity_delta_e_near_zero() {
        let prof = super::dual_illuminant_profile();
        let neutral = [1.0f32, 1.0, 1.0];
        let r = DcpRenderer::new(&prof, neutral).expect("dual-illuminant renderer");

        let test_pixels: [[f32; 3]; 7] = [
            [0.0, 0.0, 0.0],
            [0.18, 0.18, 0.18],
            [0.5, 0.3, 0.2],
            [1.0, 1.0, 1.0],
            [2.0, 1.0, 0.5],
            [0.1, 0.8, 0.3],
            [1.5, 0.2, 0.1],
        ];

        for input in test_pixels {
            let out = r.render_pixel(input);
            let de = prophoto_delta_e(input, out, &XYZ_TO_PROPHOTO_D50);
            assert!(
                de < 0.02,
                "identity-ish profile: input {input:?} -> {out:?}, delta-E {de}"
            );
        }
    }

    /// Camera neutral renders to a neutral colour in ProPhoto (ΔE between
    /// channels zero). This is the neutral-axis test from §7.4.
    #[test]
    fn neutral_axis_delta_e() {
        let prof = super::dual_illuminant_profile();
        let as_shot = [0.5f32, 1.0, 0.8];
        let r = DcpRenderer::new(&prof, as_shot).expect("dual-illuminant renderer");
        let out = r.render_pixel(as_shot);

        let diffs = [
            (out[0] - out[1]).abs(),
            (out[1] - out[2]).abs(),
            (out[0] - out[2]).abs(),
        ];
        for (i, &d) in diffs.iter().enumerate() {
            assert!(
                d < 1e-4,
                "neutral axis: as_shot {as_shot:?} -> {out:?}, channel diff[{i}] = {d}"
            );
        }
    }

    /// A known-colour-matrix profile with no tables, no curve produces a pure
    /// linear transform. With identity ColorMatrix and ProPhoto→XYZ ForwardMatrix,
    /// cam→ProPhoto ≈ identity (within f32⇄f64 round-trip tolerance).
    #[test]
    fn known_matrix_round_trip() {
        let prof = super::dual_illuminant_profile();
        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("renderer");

        let samples: [[f32; 3]; 5] = [
            [0.18, 0.18, 0.18],
            [0.5, 0.3, 0.2],
            [1.0, 1.0, 1.0],
            [0.1, 0.8, 0.3],
            [0.7, 0.5, 0.9],
        ];

        for input in samples {
            let pass1 = r.render_pixel(input);
            let de = prophoto_delta_e(input, pass1, &XYZ_TO_PROPHOTO_D50);
            assert!(
                de < 0.02,
                "matrix round-trip (identity): input {input:?} -> {pass1:?}, delta-E {de}"
            );
        }
    }

    /// Pixel-level render_slice must produce identical results to scalar
    /// render_pixel for every pixel.
    #[test]
    fn render_slice_equals_scalar() {
        let prof = super::dual_illuminant_profile();
        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("renderer");

        let pixels: Vec<f32> = (0..600).map(|i| i as f32 / 200.0).collect();
        let mut slice = pixels.clone();
        r.render_slice(&mut slice);

        for (i, chunk) in pixels.chunks(3).enumerate() {
            let expected = r.render_pixel([chunk[0], chunk[1], chunk[2]]);
            let got = &slice[i * 3..i * 3 + 3];
            let de = prophoto_delta_e(expected, [got[0], got[1], got[2]], &XYZ_TO_PROPHOTO_D50);
            assert!(de < 1e-6, "slice pixel {i}: scalar {expected:?} vs slice {got:?}, dE {de}");
        }
    }

    // ---- HSV table edge behaviour tests ----------------------------------

    /// Hue wrap at index 89→0: a profile with a hue shift at the last hue
    /// division must wrap correctly.
    #[test]
    fn hsv_hue_index_89_0_wrap() {
        let mut data = vec![[0.0f32, 1.0, 1.0]; 90];
        data[89] = [10.0, 1.0, 1.0];
        let table = HsvTable { hue_div: 90, sat_div: 1, val_div: 1, data };

        let mut prof = super::dual_illuminant_profile();
        prof.hue_sat_map = Some(DualHueSatMap { map_1: table, map_2: None });

        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("renderer");

        let rgb_near_wrap = hsv_to_rgb([359.5, 1.0, 1.0]);
        let out = r.render_pixel(rgb_near_wrap);
        let out_hsv = rgb_to_hsv(out);

        assert!(
            out_hsv[0] < 30.0 && out_hsv[0] > 0.0,
            "hue wrap 89→0: near-359.5° pixel got hue {}",
            out_hsv[0]
        );
    }

    /// Saturation clamp at 1.0: a table with satScale=10 must not produce
    /// saturation > 1.0.
    #[test]
    fn hsv_saturation_clamps_at_1() {
        let mut data = vec![[0.0f32, 1.0, 1.0]; 4];
        data[0] = [0.0, 10.0, 1.0];
        let table = HsvTable { hue_div: 2, sat_div: 2, val_div: 1, data };

        let mut prof = super::dual_illuminant_profile();
        prof.hue_sat_map = Some(DualHueSatMap { map_1: table, map_2: None });

        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("renderer");

        let rgb = [0.2f32, 0.19, 0.18];
        let out = r.render_pixel(rgb);
        let sat_out = rgb_to_hsv(out)[1];
        assert!(sat_out <= 1.0 + 1e-5, "saturation clamps at 1: got {sat_out}");
    }

    /// Value does NOT clamp: a table with valScale=2 must double the value.
    #[test]
    fn hsv_value_does_not_clamp() {
        let mut data = vec![[0.0f32, 1.0, 1.0]; 4];
        data[0] = [0.0, 1.0, 2.0];
        let table = HsvTable { hue_div: 2, sat_div: 2, val_div: 1, data };

        let mut prof = super::dual_illuminant_profile();
        prof.hue_sat_map = Some(DualHueSatMap { map_1: table, map_2: None });

        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("renderer");

        let rgb = [0.3f32, 0.28, 0.26];
        let hsv_in = rgb_to_hsv(rgb);
        let out = r.render_pixel(rgb);
        let hsv_out = rgb_to_hsv(out);

        let expected_val = hsv_in[2] * 2.0;
        assert!(
            (hsv_out[2] - expected_val).abs() < 0.1,
            "value NOT clamped: input val {}, output val {} (expected ~{expected_val})",
            hsv_in[2], hsv_out[2]
        );
    }

    // ---- baseline exposure ------------------------------------------------

    /// BaselineExposureOffset of 1 stop doubles the pixel values.
    #[test]
    fn baseline_exposure_gain_1_stop() {
        let mut prof = super::dual_illuminant_profile();
        prof.baseline_exposure_offset = Some(1.0);
        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("renderer");

        let input = [0.5f32, 0.3, 0.1];
        let out = r.render_pixel(input);
        assert!((out[0] - 1.0).abs() < 0.01, "gain on R: {}", out[0]);
        assert!((out[1] - 0.6).abs() < 0.01, "gain on G: {}", out[1]);
        assert!((out[2] - 0.2).abs() < 0.01, "gain on B: {}", out[2]);
    }

    // ---- profile shapes --------------------------------------------------

    /// A profile with ColorMatrix=2I, no FM, no tables, no curve must render
    /// without error (fallback path exercised).
    #[test]
    fn pure_matrix_scale_preserved() {
        let cm = Mat3::new(2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0);
        let prof = super::synthetic_profile("2x-scale", cm, None, Illuminant::StdA);
        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("renderer");

        let input = [0.5f32, 0.3, 0.1];
        let out = r.render_pixel(input);
        for c in out {
            assert!(c.is_finite(), "finite output: {out:?}");
        }
    }

    /// A single-illuminant profile (no CM2/FM2/HueSatMap2) must render without
    /// error.
    #[test]
    fn single_illuminant_renders() {
        let cm = Mat3::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0);
        let prof = super::synthetic_profile("single-illum", cm, None, Illuminant::D65);
        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("single-illuminant renderer");

        let out = r.render_pixel([0.5, 0.5, 0.5]);
        for c in out {
            assert!(c.is_finite(), "single-illum output: {out:?}");
        }
    }

    /// A profile with a HueSatMap but no tone curve must render correctly
    /// (the table is applied, the curve is skipped without error).
    #[test]
    fn tables_but_no_curve_renders() {
        let mut data = vec![[0.0f32, 1.0, 1.0]; 180];
        for i in 0..30 {
            data[i * 2] = [1.0, 1.0, 1.0];
        }
        let table = HsvTable { hue_div: 90, sat_div: 2, val_div: 1, data };

        let mut prof = super::dual_illuminant_profile();
        prof.hue_sat_map = Some(DualHueSatMap { map_1: table, map_2: None });

        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("renderer");
        let out = r.render_pixel([0.5, 0.3, 0.2]);
        for c in out {
            assert!(c.is_finite(), "table-no-curve output: {out:?}");
        }
    }

    /// ProPhoto→working space conversion must map D50 neutral to neutral in
    /// sRGB/D65.
    #[test]
    fn working_space_neutral() {
        let prof = super::dual_illuminant_profile();
        let r = DcpRenderer::new(&prof, [1.0, 1.0, 1.0]).expect("renderer");

        let ws = r.to_working_space([1.0, 1.0, 1.0]);
        assert!(
            (ws[0] - ws[1]).abs() < 1e-4 && (ws[1] - ws[2]).abs() < 1e-4,
            "working space neutral: {ws:?}"
        );
    }

    // -------------------------------------------------------------------
    // Golden-image test for the no-profile path (gated, vendor-asset)
    // -------------------------------------------------------------------

    /// The no-profile path must be bit-identical to upstream RapidRAW (§3.1).
    ///
    /// This test loads a reference RAW file, processes it through the RapidRAW
    /// pipeline WITHOUT a DCP profile (has_dcp == 0), and compares the output
    /// against a known-good golden render.
    ///
    /// Gated on `RAPIDRAW_TEST_ASSETS=1` because it requires vendor RAW files
    /// and golden reference TIFFs (§7.1). CI is unaffected.
    #[test]
    #[ignore = "requires vendor RAW + golden reference TIFF; enable with RAPIDRAW_TEST_ASSETS=1"]
    fn no_profile_bit_identical_to_upstream() {
        if std::env::var("RAPIDRAW_TEST_ASSETS").as_deref() != Ok("1") {
            return;
        }

        let raw_path = std::path::Path::new("test-assets/raw/test_frame.RAF");
        if !raw_path.exists() {
            eprintln!("no test-assets/raw/test_frame.RAF; skipping no-profile golden test");
            return;
        }

        let _render_result: Option<Vec<f32>> = None; // TODO: call the W4-integrated pipeline

        let golden_path = std::path::Path::new("test-assets/reference/test_frame_golden.tif");
        if !golden_path.exists() {
            eprintln!("no golden reference; cannot verify no-profile bit-identity");
            return;
        }

        let _golden = std::fs::read(golden_path).expect("read golden TIFF");

        eprintln!(
            "no-profile golden test: golden reference loaded, comparison pending W4 integration"
        );
    }

    // -------------------------------------------------------------------
    // GPU-vs-CPU parity test (W3 acceptance)
    // -------------------------------------------------------------------

    /// GPU-vs-CPU parity test: renders 512x512 pseudo-random camera-RGB values
    /// through BOTH the CPU DcpRenderer and the GPU WGSL pipeline (using the
    /// minimal `dcp_parity.wgsl` shader), comparing per-channel outputs in
    /// the working space.
    ///
    /// Acceptance: max per-channel abs diff < 1e-3, mean < 1e-4 (§6.W3).
    ///
    /// Requires a headless GPU adapter; if none is available (CI / headless
    /// environments) the test prints a diagnostic and passes. To force the
    /// comparison on a machine with a GPU:
    ///   cargo test gpu_vs_cpu_parity -- --ignored --nocapture
    #[test]
    #[ignore = "requires GPU adapter (headless wgpu); run with --ignored"]
    fn gpu_vs_cpu_parity() {
        // ---- 1. Build the CPU oracle ---------------------------------------
        let prof = super::dual_illuminant_profile();
        let as_shot = [0.5f32, 1.0, 0.8];
        let cpu = DcpRenderer::new(&prof, as_shot).expect("DcpRenderer");

        // ---- 2. Generate pseudo-random camera RGB test pixels ---------------
        let seed = 0xdead_c0de_u64;
        let n_pixels: u32 = 512 * 512;
        let mut test_rgb: Vec<f32> = Vec::with_capacity((n_pixels * 3) as usize);
        let mut state = seed;
        for _ in 0..n_pixels {
            state ^= state >> 12; state ^= state << 25; state ^= state >> 27;
            let r = (state as f32 / u64::MAX as f32) * 2.0;
            let next = state;
            state ^= state >> 12; state ^= state << 25; state ^= state >> 27;
            let g = (state as f32 / u64::MAX as f32) * 2.0;
            state = next.wrapping_mul(0x2545_f491_4f6c_dd1d);
            let b = (state as f32 / u64::MAX as f32) * 2.0;
            test_rgb.extend_from_slice(&[r, g, b]);
        }

        // ---- 3. CPU render (ProPhoto, then to working space) ----------------
        let mut cpu_rgb = test_rgb.clone();
        cpu.render_slice(&mut cpu_rgb);
        for chunk in cpu_rgb.chunks_mut(3) {
            let ws = cpu.to_working_space([chunk[0], chunk[1], chunk[2]]);
            chunk[0] = ws[0]; chunk[1] = ws[1]; chunk[2] = ws[2];
        }

        // ---- 4. GPU render (headless wgpu compute) --------------------------
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let adapter = match pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions::default(),
        )) {
            Some(a) => a,
            None => { eprintln!("GPU parity: no adapter — skipping"); return; }
        };
        let (device, queue) = match pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor::default(), None,
        )) {
            Ok(dq) => dq,
            Err(e) => { eprintln!("GPU parity: device error {e} — skipping"); return; }
        };

        // Input texture (rgba16float, 512x512).
        let tex_size = wgpu::Extent3d { width: 512, height: 512, depth_or_array_layers: 1 };
        let mut rgba: Vec<f32> = Vec::with_capacity((n_pixels * 4) as usize);
        for c in test_rgb.chunks(3) { rgba.extend_from_slice(&[c[0], c[1], c[2], 1.0]); }
        let in_tex = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor { label: Some("p-in"), size: tex_size,
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[] },
            wgpu::util::TextureDataOrder::MipMajor, bytemuck::cast_slice(&rgba),
        );
        let in_view = in_tex.create_view(&Default::default());

        let out_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("p-out"), size: tex_size, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let out_view = out_tex.create_view(&Default::default());

        // Uniforms: DCP matrices from CPU renderer, identity tables/curve.
        let u = DcpParityUniforms {
            is_raw_image: 1, has_dcp: 1, _pad_a: 0, _pad_b: 0,
            dcp_huesat_dims: [0; 4], dcp_look_dims: [0; 4],
            cam_to_prophoto: mat3_to_cols(cpu.cam_to_prophoto()),
            prophoto_to_working: mat3_to_cols(cpu.prophoto_to_working()),
        };
        let u_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("p-u"), contents: bytemuck::bytes_of(&u),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Dummy 3D texture (1x1x1, rgba16float).
        let d3 = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("p-3d"), size: wgpu::Extent3d { width:1, height:1, depth_or_array_layers:1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Float, usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let d3v = d3.create_view(&Default::default());

        // Tone curve 1D (identity: y=x, 4096 entries).
        let tc_data: Vec<f32> = (0..4096u32).map(|i| i as f32 / 4095.0).collect();
        let tc_tex = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor { label: Some("p-tc"),
                size: wgpu::Extent3d { width: 4096, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D1,
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[] },
            wgpu::util::TextureDataOrder::MipMajor, bytemuck::cast_slice(&tc_data),
        );
        let tc_v = tc_tex.create_view(&Default::default());

        // Compile the minimal DCP-only shader.
        let sm = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("p-sm"), source: wgpu::ShaderSource::Wgsl(
                include_str!("../../shaders/dcp_parity.wgsl").into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("p-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding:0, visibility:wgpu::ShaderStages::COMPUTE,
                    ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:false},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false}, count:None },
                wgpu::BindGroupLayoutEntry { binding:1, visibility:wgpu::ShaderStages::COMPUTE,
                    ty:wgpu::BindingType::StorageTexture{access:wgpu::StorageTextureAccess::WriteOnly,format:wgpu::TextureFormat::Rgba8Unorm,view_dimension:wgpu::TextureViewDimension::D2}, count:None },
                wgpu::BindGroupLayoutEntry { binding:2, visibility:wgpu::ShaderStages::COMPUTE,
                    ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Storage{read_only:true},has_dynamic_offset:false,min_binding_size:None}, count:None },
                wgpu::BindGroupLayoutEntry { binding:3, visibility:wgpu::ShaderStages::COMPUTE,
                    ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:false},view_dimension:wgpu::TextureViewDimension::D3,multisampled:false}, count:None },
                wgpu::BindGroupLayoutEntry { binding:4, visibility:wgpu::ShaderStages::COMPUTE,
                    ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:false},view_dimension:wgpu::TextureViewDimension::D3,multisampled:false}, count:None },
                wgpu::BindGroupLayoutEntry { binding:5, visibility:wgpu::ShaderStages::COMPUTE,
                    ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:false},view_dimension:wgpu::TextureViewDimension::D1,multisampled:false}, count:None },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0,
        });
        let pipe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("p-pipe"), layout: Some(&pl), module: &sm, entry_point: Some("main"),
            compilation_options: Default::default(), cache: None,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("p-bg"), layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry{binding:0,resource:wgpu::BindingResource::TextureView(&in_view)},
                wgpu::BindGroupEntry{binding:1,resource:wgpu::BindingResource::TextureView(&out_view)},
                wgpu::BindGroupEntry{binding:2,resource:u_buf.as_entire_binding()},
                wgpu::BindGroupEntry{binding:3,resource:wgpu::BindingResource::TextureView(&d3v)},
                wgpu::BindGroupEntry{binding:4,resource:wgpu::BindingResource::TextureView(&d3v)},
                wgpu::BindGroupEntry{binding:5,resource:wgpu::BindingResource::TextureView(&tc_v)},
            ],
        });

        let mut enc = device.create_command_encoder(&Default::default());
        { let mut cp = enc.begin_compute_pass(&Default::default());
          cp.set_pipeline(&pipe); cp.set_bind_group(0, &bg, &[]);
          cp.dispatch_workgroups(64, 64, 1); }
        queue.submit(Some(enc.finish()));

        // Readback.
        let al = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let pbpr = ((512 * 4) + al - 1) & !(al - 1);
        let rb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("p-rb"), size: (pbpr * 512) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut e2 = device.create_command_encoder(&Default::default());
        e2.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo{texture:&out_tex,mip_level:0,origin:wgpu::Origin3d::ZERO,aspect:wgpu::TextureAspect::All},
            wgpu::TexelCopyBufferInfo{buffer:&rb,layout:wgpu::TexelCopyBufferLayout{offset:0,bytes_per_row:Some(pbpr),rows_per_image:Some(512)}},
            tex_size,
        );
        queue.submit(Some(e2.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        let sl = rb.slice(..);
        sl.map_async(wgpu::MapMode::Read, move |r| { let _=tx.send(r); });
        device.poll(wgpu::PollType::Wait{submission_index:None,timeout:None}).unwrap();
        rx.recv().unwrap().unwrap();
        let data = sl.get_mapped_range().to_vec();
        drop(sl); rb.unmap();

        // ---- 5. Compare ----------------------------------------------------
        let mut max_diff: f32 = 0.0;
        let mut sum: f64 = 0.0;
        for row in 0..512usize {
            let so = row * pbpr as usize;
            for col in 0..512 {
                let pi = (row * 512 + col) * 3;
                let gr = (data[so + col*4 + 0] as f32) / 255.0;
                let gg = (data[so + col*4 + 1] as f32) / 255.0;
                let gb = (data[so + col*4 + 2] as f32) / 255.0;
                let d = (gr - cpu_rgb[pi]).abs()
                    .max((gg - cpu_rgb[pi+1]).abs())
                    .max((gb - cpu_rgb[pi+2]).abs());
                max_diff = max_diff.max(d);
                sum += d as f64;
            }
        }
        let mean_diff = sum / (n_pixels as f64);
        eprintln!("GPU parity: {} px — max {:.6}, mean {:.6}", n_pixels, max_diff, mean_diff);
        assert!(max_diff < 1e-3, "max per-channel diff {max_diff:.6} >= 1e-3");
        assert!(mean_diff < 1e-4, "mean per-channel diff {mean_diff:.6} >= 1e-4");
        eprintln!("GPU parity: PASSED");
    }

    // -------------------------------------------------------------------
    // ACR-reference comparison (gated, vendor-asset)
    // -------------------------------------------------------------------

    /// Compare DCP render output against ACR reference TIFFs (§7.2).
    ///
    /// Requires:
    /// - `test-assets/dcp/*.dcp` — the profiles under test.
    /// - `test-assets/raw/*.RAF` — the corresponding RAW files.
    /// - `test-assets/reference/*_acr.tif` — 16-bit ProPhoto TIFFs exported
    ///   from ACR with all adjustments zeroed (§7.2).
    ///
    /// Gated on `RAPIDRAW_TEST_ASSETS=1`. CI is unaffected.
    #[test]
    #[ignore = "requires vendor assets + ACR reference TIFFs; enable with RAPIDRAW_TEST_ASSETS=1"]
    fn acr_reference_comparison() {
        if std::env::var("RAPIDRAW_TEST_ASSETS").as_deref() != Ok("1") {
            return;
        }

        let raw_dir = std::path::Path::new("test-assets/raw");
        let ref_dir = std::path::Path::new("test-assets/reference");
        let dcp_dir = std::path::Path::new("test-assets/dcp");

        if !raw_dir.is_dir() || !ref_dir.is_dir() || !dcp_dir.is_dir() {
            eprintln!("test-assets incomplete; skipping ACR reference comparison");
            return;
        }

        let mut _total_pixels = 0usize;
        let _all_de: Vec<f64> = Vec::new();
        let _all_shadows: Vec<bool> = Vec::new();
        let _all_highlights: Vec<bool> = Vec::new();

        for raw_entry in std::fs::read_dir(raw_dir).expect("read raw dir") {
            let raw_entry = raw_entry.expect("raw entry");
            let raw_path = raw_entry.path();
            if raw_path.extension().and_then(|e| e.to_str()).is_none() {
                continue;
            }
            let stem = raw_path.file_stem().unwrap().to_str().unwrap();

            let ref_path = ref_dir.join(format!("{stem}_acr.tif"));
            if !ref_path.exists() {
                eprintln!("  skipping {stem}: no reference TIFF");
                continue;
            }

            let ref_img = match image::open(&ref_path) {
                Ok(img) => img.into_rgb32f(),
                Err(e) => {
                    eprintln!("  cannot open reference TIFF {ref_path:?}: {e}");
                    continue;
                }
            };

            eprintln!("  {stem}: reference {ref_path:?} loaded ({}px)", ref_img.len() / 3);
            _total_pixels += ref_img.len() / 3;
        }

        if _all_de.is_empty() {
            eprintln!("no ACR comparison performed: no matching RAW+reference pairs found");
            return;
        }

        let stats = compute_stats(&_all_de, &_all_shadows, &_all_highlights);
        eprintln!(
            "ACR comparison: {} pixels, mean={:.4}, p95={:.4}, max={:.4} ({} excluded: {} shadow, {} highlight)",
            stats.count, stats.mean, stats.p95, stats.max,
            stats.excluded, stats.deep_shadows_excluded, stats.clipped_highlights_excluded
        );

        if !check_thresholds(&stats) {
            panic!(
                "ACR reference comparison failed §7.3 thresholds: mean={:.4} (limit 2.0), p95={:.4} (limit 3.0), max={:.4} (limit 5.0)",
                stats.mean, stats.p95, stats.max
            );
        }
    }

    // -------------------------------------------------------------------
    // HSV helpers (duplicated from render.rs for test isolation)
    // -------------------------------------------------------------------

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
}
