# DCP Validation Harness (W9)

Validates the RapidRAW DCP render path against the CIEDE2000 metric. Tests are split into two subsets:

| Subset | CI? | Assets needed | What it checks |
|--------|-----|---------------|----------------|
| Synthetic | Yes | None | Identity profile, neutral axis, known-matrix round-trips, HSV wrap/clamp, render_slice parity |
| ACR reference | No | Vendor DCP + RAW + TIFF references | Full pipeline match against Adobe Camera Raw |

## Quick start

```bash
# Run all synthetic tests (CI-safe, no assets needed):
cargo test --all-features dcp::validation
cargo test --all-features dcp::delta_e

# Run the standalone self-test binary:
cargo run --bin validate_dcp -- self-test

# Render a DCP through a synthetic test swatch:
cargo run --bin validate_dcp -- render --dcp "path/to/profile.dcp"

# Compare against an ACR reference (requires test assets):
RAPIDRAW_TEST_ASSETS=1 cargo run --bin validate_dcp -- compare \
    --dcp "test-assets/dcp/Fujifilm X-Pro2 Cobalt Flat v3.0.dcp" \
    --raw "test-assets/raw/xpro2_test.RAF" \
    --reference "test-assets/reference/xpro2_test_acr.tif"
```

All vendor-asset tests are `#[ignore]`'d by default. Set `RAPIDRAW_TEST_ASSETS=1` to enable them.

## Test inventory

### Synthetic (CI — always runs)

| Test | File | What it verifies |
|------|------|-----------------|
| `identity_delta_e_near_zero` | `validation.rs` | Identity-profile render pipeline preserves input within ΔE < 0.02 |
| `neutral_axis_delta_e` | `validation.rs` | Camera neutral → neutral ProPhoto (channel equality) |
| `known_matrix_round_trip` | `validation.rs` | Matrix · inverse ≈ identity (ΔE < 0.02) |
| `render_slice_equals_scalar` | `validation.rs` | Parallel render_slice matches scalar render_pixel per-pixel |
| `hsv_hue_index_89_0_wrap` | `validation.rs` | Hue wraps correctly at the 89→0 boundary |
| `hsv_saturation_clamps_at_1` | `validation.rs` | Saturation clamps to ≤ 1.0 with large satScale |
| `hsv_value_does_not_clamp` | `validation.rs` | Value is NOT clamped (unlike saturation) |
| `baseline_exposure_gain_1_stop` | `validation.rs` | BaselineExposureOffset applies correct gain |
| `single_illuminant_renders` | `validation.rs` | Single-illuminant profile renders without error |
| `tables_but_no_curve_renders` | `validation.rs` | HueSatMap without tone curve renders correctly |
| `working_space_neutral` | `validation.rs` | ProPhoto→working space preserves neutrality |
| Sharma 34-pair verification | `delta_e.rs` | CIEDE2000 matches published test data (all 34 pairs < 1e-4 error) |
| `cie_de2000` symmetry + identity | `delta_e.rs` | ΔE of identical colours = 0, formula is symmetric |
| XYZ↔Lab round-trip | `delta_e.rs` | D50 white maps to L*=100, a*=b*=0 |

### Gated (only when `RAPIDRAW_TEST_ASSETS=1`)

| Test | What it verifies |
|------|-----------------|
| `no_profile_bit_identical_to_upstream` | No-profile path output == upstream RapidRAW (bit-identical) |
| `acr_reference_comparison` | Full DCP pipeline vs ACR reference TIFFs (ΔE2000 thresholds) |
| `gpu_vs_cpu_parity` | Stub for W3 — GPU vs CPU render parity (ΔE mean < 0.1, max < 0.5) |

## CIEDE2000 implementation

The formula is implemented in `src-tauri/src/dcp/delta_e.rs` (~130 lines of math, no crate needed). It is verified against all 34 published colour-difference pairs from Sharma, Wu & Dalal (2005). The conversion helpers (XYZ↔Lab) are provided for the ProPhoto (D50) working space.

## Thresholds (§7.3)

| Comparison | Mean ΔE2000 | p95 | Max |
|---|---|---|---|
| CPU render vs. ACR reference | < 2.0 | < 3.0 | < 5.0 |
| GPU vs. CPU (parity) | < 0.1 | — | < 0.5 |
| Android vs. desktop | < 1.0 | — | < 2.0 |
| Editor vs. export vs. thumbnail | < 1.0 | — | < 2.0 |
| No-profile vs. upstream | bit-identical | | |

Deep shadows (Y < 0.001) and clipped highlights are excluded from ΔE statistics but reported separately.

## Files

```
src-tauri/src/dcp/
  delta_e.rs          CIEDE2000 formula + Lab conversion + statistics + Sharma verification
  validation.rs       Synthetic validation tests + golden-image + GPU parity stubs

src-tauri/src/bin/
  validate_dcp.rs     CLI harness (render, compare, self-test subcommands)

bench/dcp_validation/
  README.md           This file
```

## CI wiring

The `cobalt-ci.yml` quality job already runs `cargo test --all-features --no-fail-fast`, which picks up all `#[test]` functions in `dcp::delta_e` and `dcp::validation`. No additional CI changes are required for the synthetic subset — it runs on every PR to `cobalt/main`.

Vendor-asset tests are `#[ignore]`'d and gated on `RAPIDRAW_TEST_ASSETS=1`, which CI never sets. CI stays green without vendor assets (§7.1).
