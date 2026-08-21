# DCP render pipeline (developer reference)

The Adobe DNG Camera Profile (DCP) render chain added by the Cobalt DCP project. Covers the CPU reference renderer (W2), the GPU/WGSL path (W3), pipeline integration (W4), the Cobalt Look decoder (W7), and the validation harness (W9).

## Architecture overview

The DCP stage replaces `rawler`'s `Calibrate` step (§3.1). When a profile is active, `Calibrate` is stripped from `RawDevelop::steps` so the image remains in **camera-native, white-balanced linear RGB** entering the profile stage. With no profile, `Calibrate` stays and the pipeline is bit-identical to upstream.

```
RAW file → rawler (demosaic, WB, crop, NO calibrate when profile active)
  → DCP stage:
      1. Illuminant interpolation + camera → ProPhoto D50 matrix
      2. HueSatMap (encoding 0 = Linear) 
      3. BaselineExposureOffset
      4. LookTable (encoding 1 = sRGB)
      5. ProfileToneCurve
      6. ProPhoto → working space (sRGB primaries, D65)
  → Cobalt Look RGB LUT (stage 7, applied via LUT system)
  → Existing RapidRAW adjustments, masks, creative LUT, tone mapping
```

## Implementation status by workstream

| W | Name | Status | Key files |
|---|------|--------|-----------|
| W1 | DCP binary parser | **Done** | `parser.rs` (1219 lines), `model.rs` (225 lines) |
| W2 | CPU reference render | **Done** | `render.rs` (1470 lines), `interpolate.rs` (336 lines) |
| W3 | GPU/WGSL path | **Done** | `shader.wgsl` (DCP section lines 1627–1823), `dcp_parity.wgsl` |
| W4 | Pipeline integration | **Done** | `raw_processing.rs`, `image_processing.rs`, `export_processing.rs` |
| W5 | Registry & pairing | **Done** | `registry.rs` (1114 lines), `camera_aliases.rs` (244 lines), `commands.rs` (616 lines) |
| W6 | Desktop UI | **Done** | `ProfilePanel.tsx`, `ProfileBrowser.tsx`, `useProfiles.ts` |
| W7 | Cobalt Look support | **Done** (Route A+B) | `look_xmp.rs` (759 lines), `capture.rs` (334 lines) |
| W8 | Android | **In progress** | SAF import partially wired in `commands.rs`; GLES fallback not verified |
| W9 | Validation harness | **Done** | `delta_e.rs` (571 lines), `validation.rs` (1012 lines), `bench/dcp_validation/` |

Total DCP module: ~8000 lines across 12 files.

## CPU reference renderer (W2)

Implemented in `render.rs`. Scalar, obviously-correct, unoptimised — the oracle the GPU path is validated against.

### Public API

```rust
pub struct DcpRenderer { /* precomputed matrices + resampled curve */ }
impl DcpRenderer {
    pub fn new(profile: &DcpProfile, as_shot_neutral: [f32; 3]) -> Result<Self, DcpError>;
    pub fn render_pixel(&self, camera_rgb: [f32; 3]) -> [f32; 3];  // → ProPhoto D50
    pub fn render_slice(&self, px: &mut [f32]);                    // rayon-parallel
    pub fn to_working_space(&self, prophoto_rgb: [f32; 3]) -> [f32; 3];
    pub fn prophoto_to_working(&self) -> Mat3;
    pub fn cam_to_prophoto(&self) -> Mat3;
}
```

All illuminant interpolation and matrix composition happens once in `new()`, never per pixel. Numerical type is `f32` throughout (matches GPU); `f64` used only inside `new()` for matrix inversion via `nalgebra`.

### Actual stage order in render_pixel (lines 349–359)

```rust
// §4.5 stage order — matches dng_sdk dng_render.cpp.
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
```

Order: **HueSatMap → BaselineExposureOffset → LookTable → ProfileToneCurve.**

## GPU/WGSL path (W3)

Implemented in `shader.wgsl` (DCP section at lines 1627–1823). Runs as a compute shader stage **before** all existing adjustments. The `dcp_apply` function is called when `has_dcp != 0`; the early-out path leaves the input unchanged at zero cost.

### GPU stage order (dcp_apply, lines 1773–1785)

```
cam_to_prophoto matrix → HueSatMap → LookTable → ProfileToneCurve
```

**Known divergence from CPU reference:** the GPU path does not apply `BaselineExposureOffset`. This is a gap — the offset is a small gain (typically ~0.1 stops) and its absence produces a slight exposure mismatch between CPU and GPU renders. The `has_dcp`, `dcp_huesat_dims`, `dcp_look_dims` uniforms and all DCP-related bindings are present and wired.

### GPU resource layout

| Binding | Group | Type | Content |
|---------|-------|------|---------|
| 12 | 0 | `texture_3d<f32>` | HueSatMap (pre-interpolated dual-illuminant, one texture uploaded from CPU) |
| 13 | 0 | `texture_3d<f32>` | LookTable (ProfileLookTableData) |
| 14 | 0 | `texture_1d<f32>` | ProfileToneCurve (resampled 4096-entry LUT) |

Uniform struct carries: `has_dcp`, `dcp_amount`, `dcp_huesat_dims`, `dcp_look_dims`, `dcp_huesat_encoding`, `dcp_look_encoding`, `cam_to_prophoto` (mat3x3), `prophoto_to_working` (mat3x3).

Dummy 1×1×1 textures are bound when no profile is active. WGSL requires all bindings present.

Hue wrapping uses manual `textureLoad` with explicit index arithmetic — hardware `texture_3d` sampling cannot express "wrap on hue, clamp on sat/val" across a single sampler. The wrapper fetches 8 corners and trilinearly interpolates manually.

**GLES compatibility:** `rgba32float` 3-D texture sampling on GLES 3.0 is not yet verified on real Android devices. A `rgba16float` fallback is spec'd but not implemented. This is a W8 open item.

### GPU-vs-CPU parity

The W9 validation harness has a stub for GPU-vs-CPU parity testing. The test is `#[ignore]`'d (requires a GPU context) and not run in CI. Parity has not been measured with the BaselineExposureOffset divergence present.

## The four VERIFY items — resolved status

All four items from §4 of the spec are resolved in code against the dng_sdk source. ACR-reference A/B comparison with measured ΔE2000 is **PENDING** — see the ACR gap section below.

### 1. Stage order (§4.5)

**Resolved.** `HueSatMap → BaselineExposureOffset → LookTable → ProfileToneCurve`. Matches `dng_sdk dng_render.cpp` line-for-line (`dng_host::ProcessStage2()`). Not user-configurable.

**Authority:** dng_sdk `dng_render.cpp`. **ACR-reference ΔE:** PENDING.

### 2. wb_coeffs → AsShotNeutral reciprocal (§4.1)

**Resolved.** DNG `AsShotNeutral` is the reciprocal of the multiplicative white-balance coefficients. The renderer receives `as_shot_neutral` already as the reciprocal (normalised so G = 1). The `D` diagonal applies the white balance as a reciprocal-of-reciprocal (i.e. the multiplicative gain). No conversion is needed inside the renderer — the caller is responsible for providing the DNG `AsShotNeutral` value as-is.

**Authority:** DNG Specification v1.7.0.1 §6; dng_sdk `dng_negative::SetAsShotNeutral`. **ACR-reference ΔE:** PENDING.

### 3. Tone-curve hue-preservation (§4.4.3)

**Resolved.** Default is **per-channel** (dng_sdk behaviour). A hue-preserving mode (`ToneCurveMode::HuePreserving`) is implemented and toggleable via `set_tone_curve_hue_preserving()`. The hue-preserving mode scales RGB by `curve(v)/v` where `v = max(R,G,B)`. Default `PerChannel` matches dng_sdk; the hue-preserving option is an ACR-match candidate pending reference renders.

**Authority:** dng_sdk applies the tone curve per-channel by default. No hue-preserving mode exists in dng_sdk. **ACR-reference ΔE:** PENDING.

### 4. Tone-curve above 1.0 (§4.4.3)

**Resolved.** Default is **extrapolate** (linearly extend the last segment above 1.0). Clamp mode (`AboveOne::Clamp`) is implemented and toggleable via `set_tone_curve_above_one()`. Extrapolate preserves highlight roll-off; Clamp matches some ACR versions.

**Authority:** dng_sdk `dng_host::ProcessStage2` extrapolates. **ACR-reference ΔE:** PENDING.

## NormalizeForwardMatrix (W2 Appendix B)

ForwardMatrix tags are stored as SRATIONAL with finite precision, so their rows do not sum to exactly D50. The Cobalt FM1 rows sum to `[0.964400, 1.000000, 0.825100]` instead of canonical D50 `[0.964296, 1.0, 0.825105]` — off by ~1.04e-4 in X.

`normalize_forward_matrix()` in `render.rs` applies Bradford chromatic adaptation from the FM's implicit white point to dng_sdk canonical D50 (`D50_XY = (0.3457, 0.3585)`). Applied to FM1 and FM2 individually, before dual-illuminant interpolation. After normalisation: neutral axis at machine precision (error ~2.2e-16).

Without this function, the ~1e-4 row-sum error propagates through the entire chain, producing a ~2.7e-4 non-neutrality that no choice of `XYZ_TO_PROPHOTO_D50` can fix.

## Colour space matrices

All computed in `f64` inside `DcpRenderer::new()` from first principles, then downcast to `f32`. The published constant matrices (XYZ→ProPhoto, XYZ→sRGB) are rounded approximations whose product with their inverses differs from identity by ~1e-4. The renderer computes:

- **XYZ→ProPhoto:** from dng_sdk D50_xy (0.3457, 0.3585) and ProPhoto primaries.
- **ProPhoto→XYZ:** `f64` inverse of the above (guarantees exact inverse).
- **Bradford adaptation D50→D65:** computed adaptively from source/dest white points, not from a stored constant.
- **XYZ→sRGB (D65):** row-normalised so each row maps D65 white to exactly 1.0, cancelling the ~0.003 offset in the published values.

This computed-matrix approach guarantees neutral-axis precision at machine-epsilon level across the full camera→ProPhoto→working-space chain.

## Illuminant interpolation (interpolate.rs)

Robertson 31-entry isotherm table, indexed by mireds (identical to dng_sdk `kTempTable` in `dng_temperature.cpp`). CCT solve is iterative: guess a temperature, build the interpolated `ColorMatrix`, map the neutral to xy, compute its CCT, repeat. 3 iterations used, matching dng_sdk. Max divergence across 8 test chromaticities: 0.000000 K.

Interpolation weight in reciprocal temperature (mireds):
```
g = (1/T − 1/T2) / (1/T1 − 1/T2), clamped to [0, 1]
```

## Cobalt Look decoder (W7)

### Route A: native dng_big_table decode

**Status: WORKING.** All 17 supplied "CCD Fever v3.0" Looks decode. ~220–245 KB of base85 text → zlib-inflated binary → `dng_rgb_table` (32³ samples, ProPhoto primaries, 1.8 gamma transfer, `min_amount = 0.2`, `max_amount = 1.5`).

The decoder is in `look_xmp.rs`. Three-stage pipe:

1. **Base85→binary.** Z85-like alphabet, 96-entry decode table. 5-char groups → little-endian `u32`. Trailing partial group handled.
2. **zlib inflate.** First 4 bytes = uncompressed size (LE `u32`). Remainder is RFC 1950 zlib stream.
3. **Parse dng_rgb_table.** `u32 type, version, dimensions, divisions`, then `divisions³` samples of `u16` RGB deltas from an identity ramp, then `u32 primaries, transfer, gamut, f64 min_amount, max_amount`.

The reader is **read-only**. No writer, no re-encoder, no export of Cobalt tables. No Cobalt table data (decoded or encoded) is committed to the repository — tests needing the supplied XMPs are `#[ignore]`'d and gated on `RAPIDRAW_TEST_ASSETS=1`.

### Route B: Profile Capture

**Status: WORKING.** The HALD export + import workflow is wired. `capture.rs` wraps the existing `generate_identity_lut_image` and `convert_image_to_cube_lut` from `lut_processing.rs`. The captured `.cube` LUT is persisted in `{app_data}/profiles/looks/captured/<uuid>.cube` and picked up on next launch.

See `docs/profiles.md` for the user-facing workflow. See `docs/cobalt-look.md` for the detailed technical format reference.

### Where the Cobalt Look sits in the pipeline

The Cobalt Look RGB table (3-D, ProPhoto primaries, 1.8 gamma) is stage 7 — after ProfileToneCurve, before ProPhoto→working space conversion. It is applied through RapidRAW's existing LUT system (not inline in the DCP shader), which means it shares the same trilinear interpolation and Amount lerp (`out = lerp(identity, table_out, amount)`) as the creative LUT stage.

## Validation harness (W9)

### CIEDE2000 (delta_e.rs)

Self-contained ~570 lines of math. No external crate. Verified against all 34 published colour-difference pairs from Sharma, Wu & Dalal (2005) — all pairs < 1e-4 error. XYZ↔Lab conversion helpers provided for D50 white point.

### Synthetic tests (validation.rs)

14 asset-free tests, all run in CI on every PR to `cobalt/main`:

- Identity profile → ΔE ≈ 0
- Neutral axis → channel equality < 1e-4
- Known-matrix round-trip → ΔE < 0.02
- `render_slice` == scalar `render_pixel` per-pixel
- HSV hue wrap at 89→0 boundary
- HSV saturation clamps at 1.0
- HSV value does NOT clamp
- BaselineExposureOffset applies correct gain
- Single-illuminant profile renders
- HueSatMap without tone curve renders
- ProPhoto→working space preserves neutrality
- CIEDE2000 symmetry + identity
- XYZ↔Lab D50 round-trip
- 34 Sharma pairs verification

### Gated tests (require `RAPIDRAW_TEST_ASSETS=1`)

- No-profile path bit-identical to upstream (golden image)
- ACR reference comparison (stub — needs reference TIFFs)
- GPU-vs-CPU parity (stub — needs GPU context)

### CLI harness

`src-tauri/src/bin/validate_dcp.rs` — `render`, `compare`, `self-test` subcommands. `self-test` runs the synthetic validation suite standalone. `compare` needs reference TIFFs.

## ACR gap — the honest status

There is **no ACR/Lightroom on the build machine and no reference TIFFs.** The §7.3 acceptance criterion (ΔE2000 vs ACR, mean < 2.0) cannot be met, and the four VERIFY items cannot be resolved in their A/B-comparison form.

**What IS validated:**

- Stage order matches dng_sdk source (`dng_render.cpp`, `dng_camera_profile.cpp`) line-for-line
- Neutral-axis and identity tests confirm matrix paths are self-consistent at machine precision
- Illuminant interpolation matches dng_sdk Robertson table and iterative CCT solve (max divergence 0.000000 K)
- HueSatMap wrap/clamp/extrapolate behaviour matches DNG spec
- ForwardMatrix normalisation corrects the ~1e-4 row-sum error
- Bradley adaptation recomputed from first principles to guarantee exact white-point mapping
- 34-pair Sharma CIEDE2000 verification confirms the metric itself is correct

**What is NOT validated (and cannot be without reference renders):**

- Whether the per-channel tone curve matches ACR's hue-preserving behaviour on saturated highlights
- Whether the extrapolate-vs-clamp choice for values > 1.0 matches ACR
- Whether the stage order (HueSatMap → LookTable → ToneCurve) exactly matches ACR's internal order (dng_sdk says yes, but ACR may have deviated)
- Whether the specific numerical behaviour of `satScale^k` Amount interpolation matches ACR for k > 1.0

The CIEDE2000 harness is built, and the reference render procedure is documented in `test-assets/README.md`. To close the gap: produce reference TIFFs on a machine with ACR/Lightroom per §7.2 of the spec, place them in `test-assets/reference/` locally, and run `RAPIDRAW_TEST_ASSETS=1 cargo run --bin validate_dcp -- compare`.

## Android (W8) — expected state

W8 has not yet been merged into `cobalt/main`. What exists:

- The Android app is built from the **same repository** (`src-tauri/gen/android/`). There is no separate Android repo.
- SAF import for `.dcp`/`.xmp` is partially wired in `commands.rs` (lines 267–272), mirroring the existing LUT import pattern.
- `#[cfg(target_os = "android")]` gates are in place for the import path.
- The Android manifest (`src-tauri/gen/android/app/src/main/AndroidManifest.xml`) has not yet been updated with `.dcp`/`.xmp` intent filters.

What is still needed:

- Verify 3-D `rgba32float` texture sampling on GLES 3.0 on real devices; implement `rgba16float` fallback if unsupported.
- Add `.dcp`/`.xmp` MIME/extension intent filters to the manifest.
- Test on at least one low-end and one recent Android device.
- Document exact build commands in `docs/BUILD.md`.

## Known limitations

1. **BaselineExposureOffset missing from GPU path.** The CPU reference applies it; the GPU shader does not. This produces a slight exposure mismatch (~0.1 stops) between CPU and GPU renders when the profile carries a BaselineExposureOffset tag. The Cobalt Flat DCP does not set one, so the mismatch is zero for the supplied test profile, but it exists for other profiles that do.
2. **GPU-vs-CPU parity not measured.** The W9 stub exists but has not been run. The BaselineExposureOffset divergence means parity cannot pass as-is.
3. **ACR delta-E validation deferred.** See ACR gap section above.
4. **Dual-illuminant HueSatMap interpolation.** Pre-interpolated by `g` on the CPU at upload time; only one texture is uploaded. If a user switches white balance frequently, the CPU must re-upload. This is a perf concern, not a correctness one — the re-upload is ~1-2 ms for the largest table (90×30×30 = 81k texels × 16 bytes = 1.3 MB).
5. **Amount slider on GPU.** The `dcp_amount` uniform exists but is not yet wired into the GPU shader's `dcp_apply` function. The Cobalt Look Amount lerp is handled through the LUT system (stage 7), not in the DCP shader.
6. **Cobalt Look table application.** The Look RGB table is applied via RapidRAW's existing creative LUT system, not inline in the DCP WGSL shader. This means the Look is applied in the working space (post-ProPhoto→sRGB conversion) rather than in ProPhoto as the spec envisions. The ΔE impact of this space difference is unknown and not measured.

## Normative source

`COBALT_DCP_SPEC.md` section 4 (colour science) and section 7 (testing and validation) define the contract this document elaborates.
