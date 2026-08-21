# NOTICE

This is a fork of [RapidRAW](https://github.com/CyberTimon/RapidRAW), (c) Timon Kach, licensed under AGPL-3.0.

## What this fork adds

An Adobe DNG Camera Profile (DCP) pipeline and Cobalt Image profile support, enabling camera-profile-based rendering inside RapidRAW on desktop and Android. The feature adds a first-class "profile slot" to the editing pipeline (the concept Lightroom/ACR exposes as `Adobe Color` / `Camera Standard` / a Cobalt profile), holding a base DCP plus an optional Cobalt Look and an Amount, applied before all existing adjustments.

## Specifications and trademarks

- **DNG / DCP** is an Adobe specification. Adobe is a trademark of Adobe Inc. No Adobe code or assets are redistributed in this repository.
- **Cobalt Image** is a trademark of its owner. No Cobalt-Image code, profiles, look tables, or other assets are redistributed in this repository, in source, in tests, in fixtures, or in built binaries. Users install their own lawfully-purchased profile files.
- This fork implements **read-only interoperability** with profile files the user already owns. It does not author, convert, export, or redistribute profiles, and it does not circumvent any licence check. A profile's `ProfileEmbedPolicy` (e.g. `EmbedNever`) is honoured on export.

## Licensing

This fork remains licensed under AGPL-3.0, identical to upstream. See `LICENSE`. AGPL obligations apply to any network-served deployment.

## Release notes — Cobalt DCP pipeline (2026-08-21)

### What is supported

- **DCP camera profiles.** Dual-illuminant matrix interpolation, ForwardMatrix path, HueSatMap (Linear encoding), LookTable (sRGB encoding), ProfileToneCurve. CPU reference renderer and GPU/WGSL compute-shader path both working on desktop.
- **Cobalt Look tables — Route A (native decode).** All 17 supplied "CCD Fever v3.0" Looks decode natively from their embedded `dng_big_table` / `dng_rgb_table` blob. The decoder handles the base85→zlib→dng_rgb_table format for tables with 32³ divisions, ProPhoto primaries, and 1.8 gamma transfer. Read-only — no writer, no re-encoder, no export of Cobalt tables.
- **Cobalt Look tables — Route B (Profile Capture).** When native decode fails (future Look versions, unsupported formats), the HALD export + ACR round-trip + re-import workflow is wired and functional. The capture persists as a `.cube` LUT linked to the Look's UUID.
- **Profile registry.** Auto-discovers Adobe CameraRaw directories on macOS and Windows. Two-key `(camera model, profile name)` pairing. Unsatisfiable Looks are disabled with an explanatory message — never silently mis-rendered. Exact camera-model matching (no fuzzy fallback).
- **Desktop UI.** Profile dropdown above Basic adjustments. Grouped browser (camera profiles for this body, Looks by group). Amount slider (0–200, default 100) for Looks. Import via file picker. Preset integration: profile capture is opt-in, default off.
- **Pipeline integration.** Editor preview, export, thumbnails, and headless CLI all route through the same profile-aware path. `ProfileEmbedPolicy` (including `EmbedNever`) honoured on export. Cache invalidates on profile/amount change.
- **No-profile path.** With no profile selected, output is bit-identical to upstream RapidRAW. Regression-tested.
- **Validation.** CIEDE2000 formula verified against all 34 Sharma (2005) pairs. 14 synthetic tests run in CI: identity profile, neutral axis, matrix round-trips, HSV wrap/clamp, BaselineExposureOffset, single-illuminant fallback. CLI harness (`validate_dcp`) for render, compare, self-test.

### What is not yet supported

- **ACR-reference delta-E validation (DEFERRED).** The render pipeline is dng_sdk-faithful (stage order, illuminants, matrices) and self-consistent (neutral axis at machine precision). But it has not been measured against Adobe Camera Raw reference renders. No ACR/Lightroom is installed on the build machine, and no reference TIFFs have been produced. The CIEDE2000 harness is built and ready — see `test-assets/README.md` for the procedure to close this gap.
- **Stage order is dng_sdk-faithful, not ACR-verified.** The four VERIFY items (stage order, wb reciprocal convention, tone-curve hue preservation, tone-curve above-1.0 behaviour) are resolved against dng_sdk source, not ACR A/B comparison. The tone-curve defaults (per-channel, extrapolate) match dng_sdk; ACR's behaviour may differ.
- **Android (IN PROGRESS).** W8 has not been merged. The SAF import path for `.dcp`/`.xmp` is partially wired. GLES 3.0 3-D texture compatibility (`rgba32float` sampling) has not been verified on real devices. The Android manifest lacks `.dcp`/`.xmp` intent filters. The core RapidRAW app builds for Android from `cobalt/main` but camera profiles are not functional on Android.
- **GPU-vs-CPU parity not measured.** The GPU path does not apply `BaselineExposureOffset` (the CPU reference does). The W9 parity test stub exists but has not been run. For the Cobalt Flat DCP (which carries no BaselineExposureOffset), this divergence is zero.
- **GLES fallback for 3-D textures.** Spec'd (`rgba16float` instead of `rgba32float`) but not implemented.

### What is explicitly not built

- A profile authoring tool, converter, or exporter.
- A Lightroom clone — the profile slot is additive and bounded.
- Anything that redistributes Cobalt profiles or enables unlicensed use. The decoder is read-only; no table data is committed to the repository; `EmbedNever` is enforced; tests needing vendor assets are gated and `#[ignore]`'d.
- Support for non-RAW files (JPEG/TIFF/PNG) — the profile slot is hidden for these.

## Upstream attribution

Upstream RapidRAW is (c) Timon Kach, AGPL-3.0: https://github.com/CyberTimon/RapidRAW
