# DCP render pipeline (developer reference)

> Status: W2 complete — 36 tests pass, 0 fail, 3 ignored. Four VERIFY items documented below as provisionally resolved; ACR-reference delta-E validation is **blocked** (see §ACR gap).

## Scope

The Adobe DNG Camera Profile (DCP) render chain added to RapidRAW by the Cobalt DCP project. Covers illuminant interpolation (§4.2), ForwardMatrix to XYZ(D50) (§4.3), ProPhoto linear working space (§4.4), HueSatMap / LookTable / ProfileToneCurve (§4.4.1-§4.4.3), and the return to RapidRAW's working space (sRGB/Rec.709 primaries, D65, linear).

## The four VERIFY items — provisionally resolved

All four items are resolved in code per the dng_sdk authority (`dng_render.cpp`, `dng_camera_profile.cpp`). Each is verified against the dng_sdk source and the neutral-axis / identity / forward-matrix-normalisation test suite. **ACR-reference A/B comparison with measured dE2000 is PENDING** — see the ACR gap section below.

### 1. Stage order (§4.5)

**Resolved:** `HueSatMap → BaselineExposureOffset → LookTable → ProfileToneCurve`. This matches `dng_sdk dng_render.cpp` line-for-line and is the order in `render_pixel()` (render.rs ~line 310). Not user-configurable at this stage; W4 may expose an A/B toggle.

**Authority:** dng_sdk `dng_render.cpp`, `dng_host::ProcessStage2()`.
**ACR-reference delta-E validation:** PENDING. Blocked on reference TIFFs (see §ACR gap).

### 2. wb_coeffs → AsShotNeutral reciprocal (§4.1)

**Resolved:** DNG `AsShotNeutral` is the **reciprocal** of the multiplicative white-balance coefficients. The renderer receives `as_shot_neutral` already as the reciprocal (normalised so G = 1). The `D` diagonal in §4.3 (`D = Inv(Diag(refNeutral))`, where `refNeutral = Inv(AB·CC) · as_shot_neutral`) applies the white balance as a reciprocal-of-reciprocal, which is the multiplicative gain. No conversion is needed inside the renderer — the caller is responsible for providing the DNG `AsShotNeutral` value as-is.

**Authority:** DNG Specification v1.7.0.1 §6, "AsShotNeutral"; dng_sdk `dng_negative::SetAsShotNeutral`.
**ACR-reference delta-E validation:** PENDING.

### 3. Tone-curve hue-preservation (§4.4.3)

**Resolved:** Default is **per-channel** (dng_sdk behaviour). A hue-preserving mode (`ToneCurveMode::HuePreserving`) is implemented and toggleable via `set_tone_curve_hue_preserving()`. The hue-preserving mode scales RGB by `curve(v)/v` where `v = max(R,G,B)`. Default `PerChannel` matches dng_sdk; the hue-preserving option is an ACR-match candidate pending reference renders.

**Authority:** dng_sdk applies the tone curve per-channel by default; no hue-preserving mode exists in dng_sdk. This is a research point.
**ACR-reference delta-E validation:** PENDING.

### 4. Tone-curve above 1.0 (§4.4.3)

**Resolved:** Default is **extrapolate** (linearly extend the last segment above 1.0). Clamp mode (`AboveOne::Clamp`) is implemented and toggleable via `set_tone_curve_above_one()`. Extrapolate preserves highlight roll-off; Clamp matches some ACR versions. The correct mode for ACR-matching is an open question pending reference renders.

**Authority:** dng_sdk `dng_host::ProcessStage2` extrapolates; some ACR builds clamp. To be resolved empirically.
**ACR-reference delta-E validation:** PENDING.

## NormalizeForwardMatrix (W2 Appendix B fix)

ForwardMatrix tags are stored as SRATIONAL with finite precision, so their rows do not sum to exactly D50 (the Cobalt FM1 rows sum to `[0.964400, 1.000000, 0.825100]` instead of canonical D50 `[0.964296, 1.0, 0.825105]` — off by 1.04e-4 in X). This is normal and expected.

dng_sdk corrects this at load time via `dng_camera_profile::NormalizeForwardMatrix`. The W2 CPU renderer replicates this:

- `normalize_forward_matrix()` applies Bradford chromatic adaptation from the FM's implicit white point to dng_sdk canonical D50 (`D50_XY = (0.3457, 0.3585)`).
- Applied to FM1 and FM2 **individually, before** dual-illuminant interpolation by `g`.
- After normalisation: neutral axis at machine precision (error ~2.2e-16).

Without this function, the ~1e-4 row-sum error propagates through the entire chain, producing a ~2.7e-4 non-neutrality that no choice of `XYZ_TO_PROPHOTO_D50` can fix.

## ACR gap — G2 blocker

There is **no ACR/Lightroom on the build machine and no reference TIFFs.** The §7.3 acceptance criterion (delta-E vs ACR, mean < 2.0) cannot be met, and the four VERIFY items above cannot be resolved in their A/B-comparison form. This blocks G2 and therefore W3.

**Three options, not decided:**

- **(a)** Produce reference renders on a machine with Lightroom/ACR per §7.2, place them in `test-assets/reference/` locally (NOT committed, §8.1). This is the strongly preferred path — the whole point of D2 is matching Adobe, and without a reference there is no way to know whether the renderer is right, only that it is self-consistent.
- **(b)** Let W2 land against synthetic + neutral-axis + identity + GPU-parity tests only, deferring ACR-matching and the four VERIFY items to a follow-up before G2 closes. This unblocks W3 but means the stage order stays **provisional and unverified** into GPU code — accepting rework risk if it turns out wrong.
- **(c)** Hold W2 until (a) is possible.

## Stage 7 (W7 RGB LUT output)

W7 will add a final RGB LUT (3D cube or 1D channel) as the last stage of the render chain, implemented in WGSL on the GPU path. The CPU reference path will include the same LUT for parity. Not yet implemented; documented here so the stage order in §4.5 is understood to include a placeholder for it after ProfileToneCurve.

## Normative source

See `COBALT_DCP_SPEC.md` section 4 (colour science) and section 7 (testing and validation) for the normative contract this document elaborates.
