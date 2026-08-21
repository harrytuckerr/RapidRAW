# Independent Review — Cobalt DCP Build

**Reviewed:** `cobalt/main` @ `a916f4c5` (all 10 workstreams merged) · **Date:** 2026-08-21
**Method:** built and ran the suite locally, validated the WGSL with `naga`, ran the
`#[ignore]`d tests, and staged the real vendor assets to exercise the vendor-gated paths.
All temporary changes reverted — **working tree is clean.**

---

## Verdict

**Not done.** The CPU side is genuinely strong. The **GPU path does not work at all**, and the
two tests guarding the project's most important acceptance criteria (**D2** and **D6**) are
hollow stubs that report `ok` while asserting nothing.

The "36 tests pass, CI green" claim is true and also misleading: CI cannot detect any of these
defects, for a structural reason given in §5.

| Area | Claimed | Actual |
|---|---|---|
| W1 parser | done | **Confirmed solid** — exact-value assertions vs the real DCP, fuzz clean |
| W2 CPU render | done | **Confirmed solid** — machine-precision neutral axis |
| W5 registry/pairing | done | **Confirmed solid** — two-key lookup, alias, negative cases |
| W7 Cobalt Look | done | **Confirmed** — all 17 real Looks parse and pass acceptance |
| **W3 GPU/WGSL** | done | **BROKEN — shader does not compile; math diverges; test unsatisfiable** |
| **W9 validation** | done | **D2 and D6 tests are empty stubs that pass vacuously** |
| W4 / W6 / W8 / W10 | done | **Not verified** — see §6 |

---

## 1. P0 — The production shader does not compile

`src-tauri/src/shaders/shader.wgsl:1746` (and 4 more lines in the same function) calls
`textureLoad` on a `texture_1d<f32>` with a **`vec2<i32>` coordinate**. WGSL requires a scalar
coordinate for 1-D textures. Verified directly with `naga::valid::Validator`:

```
shader.wgsl: INVALID -> Function 'dcp_eval_tone_curve' is invalid
  1746 │ let y0 = textureLoad(lut, vec2<i32>(0, 0), 0).x;
       = Image coordinate type does not match dimension D1
dcp_parity.wgsl: INVALID -> (identical error)
```

**Impact is not limited to DCP.** `shader.wgsl` is *the* render shader for the whole
application. A module that fails validation cannot create a pipeline, so this is a
**total-failure defect: the editor cannot render anything.** It is not gated behind a profile
being selected.

**Fix** — 5 occurrences in `shader.wgsl`, 5 in `dcp_parity.wgsl`:

```wgsl
// wrong
let y0 = textureLoad(lut, vec2<i32>(0, 0), 0).x;
// right
let y0 = textureLoad(lut, 0, 0).x;
```

I applied this locally and re-validated: **both shaders become `VALID`, with no further errors
behind it.** Then reverted.

> **This should have been impossible to merge.** Any manual launch of the app would have caught
> it in seconds. It indicates W3 was never actually run.

## 2. P0 — GPU output diverges from the CPU reference

With the shader patched so it *can* run, the parity test still fails, badly:

```
GPU parity: 262144 px — max 8.101501, mean 3.271054   (threshold 1e-3)
```

Mean error **3.27 against a 0.001 tolerance** — ~3000×. Three independent causes:

**(a) The parity shader is not the production shader.** `dcp_parity.wgsl` is a separate,
divergent copy. Its uniform struct has **no encoding fields at all**, and it hardcodes:

```wgsl
rgb = dcp_apply_hsv_table(rgb, dcp_look_tex, u.dcp_look_dims, 0u);   // encoding forced to Linear
```

The Cobalt DCP has `ProfileLookTableEncoding = 1` (**sRGB**) — this is precisely the
"two tables, two encodings" trap flagged in `COBALT_DCP_SPEC.md` §4.4 and §11. The *production*
shader passes `ad.dcp_look_encoding` correctly; the parity shader does not. **So even a passing
parity test would prove nothing about the shipped path.**

**(b) `BaselineExposureOffset` is missing from both GPU shaders.** CPU `render_pixel` applies
HueSatMap → **baseline exposure** → LookTable → ToneCurve. Both shaders omit the middle stage.
A real math divergence from the documented §4.5 order.

**(c) The test is unsatisfiable by construction.** The GPU renders into an **`rgba8unorm`**
target, and the comparison reads back `data[i] as f32 / 255.0`. 8-bit quantisation alone is
**3.9e-3**, four times the `1e-3` tolerance it asserts. The CPU side compares against
`render_pixel`, which returns **ProPhoto, unclamped**, while the GPU applies
`prophoto_to_working` and clamps to `[0,1]` — different spaces entirely. That is where the
max diff of 8.1 comes from.

**Fix:** render parity to `rgba32float`, compare like-for-like against
`render_pixel` + `to_working_space`, add baseline exposure to both shaders, and — most
importantly — **make the parity path exercise `shader.wgsl` itself** rather than a hand-copied
twin that can drift.

## 3. P0 — D2 and D6 are verified by empty stubs

Both report `ok` under `RAPIDRAW_TEST_ASSETS=1`. Neither asserts anything.

`no_profile_bit_identical_to_upstream` — **D6, the project's most important invariant**:

```rust
let _render_result: Option<Vec<f32>> = None; // TODO: call the W4-integrated pipeline
```

It never renders. Even with every fixture present it does nothing, then returns green.
`acr_reference_comparison` (**D2**) is the same shape — `_all_de: Vec<f64> = Vec::new()`,
never populated.

They also **early-return `ok` when fixtures are absent**. A missing-fixture skip must be
`eprintln!` + a distinct skipped state, never a pass. As written, these two produce *negative*
value: they make an unverified invariant look verified.

**D6 is not verified. D2 is not verified.** Both should be treated as open.

## 4. P1 — `render_pixel` deviates from the spec signature

`COBALT_DCP_SPEC.md` §6.W2 specifies `render_pixel(...) -> [f32; 3]  // → working space`.
The implementation returns **ProPhoto**, with `to_working_space` as a separate call. Defensible
as a design, but it is an undocumented contract change, and it is the direct cause of the
apples-to-oranges comparison in §2(c). Either restore the spec'd contract or update the spec and
audit every call site.

## 5. Why CI could never have caught any of this

```yaml
run: cargo test --all-features --no-fail-fast     # .github/workflows/cobalt-ci.yml:76
```

- `cargo test` **skips `#[ignore]`d tests** — so the one test that instantiates a GPU device
  never runs.
- WGSL is validated **at runtime**, when a device creates the module. `cargo build` succeeds
  with a completely invalid shader. I confirmed the crate builds clean.

So a shader that cannot render, and stub tests that assert nothing, both sail through green CI.

**Recommended CI changes:**
1. Add a **naga validation test** that parses and validates every `.wgsl` under
   `src/shaders/`. It needs no GPU, runs in milliseconds, and would have caught §1 instantly.
   This is the highest-value single change in this report.
2. Add a CI job running `cargo test -- --ignored` on a runner with a software adapter
   (`lavapipe`/WARP), or at minimum gate merges to `cobalt/main` on a manual GPU run.
3. Make fixture-absent tests **fail or explicitly skip** — never silently pass.

## 6. What I did NOT verify

Out of scope for this pass; all still unproven:

- **W4** pipeline integration — editor/export/thumbnail/CLI agreement (**D7**). Cannot be
  assessed while the shader is invalid.
- **W6** desktop UI — requires launching the app, which §1 currently prevents.
- **W8** Android — no device/emulator run.
- **W10** docs accuracy.
- **D2/D3** vs ACR — still blocked on reference renders and the `Cobalt Modular` base pack.

## 7. What is genuinely good

Real quality here, and it should not be lost in the above:

- **Parser (W1):** asserts every tag in §1.3 against the real Cobalt DCP *exactly*; the mutation
  fuzz target passes; `round_trip_additional_vendor_dcps` passes.
- **Colour science (W2):** post-remediation this is correct. `forward_matrices_normalize_to_d50`,
  `neutral_axis_test`, `prophoto_white_is_neutral`, `vendor_dcp_neutral_axis` all pass — the
  `NormalizeForwardMatrix` fix landed properly and hits machine precision.
- **Registry (W5):** two-key lookup, alias matching, dedupe-by-hash, and the **negative** pairing
  cases are all covered.
- **Looks (W7):** all 17 real Cobalt Looks parse and pass acceptance, including the ShortName /
  SortName trap from §1.1.
- **Every spec non-negotiable is honoured:** `wgpu` still pinned at 29.0, zero vendor assets
  committed (`git ls-files` count: 0), no fuzzy matching in `dcp/`, no new runtime dependencies.

**100 CPU-side tests pass, and I believe them** — they assert real values against real files.
The problem is confined to the GPU path and the validation stubs.

---

## 8. Recommended actions, in order

1. **Do not tag or announce this build.** The app cannot render.
2. **Fix §1** (10 lines). Verify by launching the app, not by `cargo build`.
3. **Add the naga shader-validation test** and wire it into CI (§5.1).
4. **Rewrite the parity test** per §2: `rgba32float`, matching colour spaces, and exercising
   `shader.wgsl` itself. Add baseline exposure and correct look-table encoding to the GPU path.
5. **Delete or complete the D2/D6 stubs.** A test that cannot fail is worse than no test — if
   they are not going to be implemented now, mark them `unimplemented!()` so they fail loudly.
6. **Re-open G2.** It was signed off partly on "W3 is green"; W3 is not green.
7. Only then reassess W4/W6/W8.

**Reopen W3 and W9.** W1, W2, W5 and W7 look genuinely complete.
