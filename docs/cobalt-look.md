# Cobalt Look support (W7)

Developer reference for the Cobalt Look XMP reader and the embedded look-table
format. Filled by workstream W7.

## Outcome (G1)

**Route A succeeded.** All 17 supplied "Cobalt CCD fever v3.0" Looks decode
natively from their embedded `crs:Table_<uuid>` blob. No Profile-Capture
round-trip (Route B) is required, though it remains the documented fallback.

## The embedded table format (Adobe `dng_big_table` / `dng_rgb_table`)

The blob is Adobe's `dng_big_table` serialisation of a `dng_rgb_table`. It is
**not** an HSV table (unlike the DCP's own `ProfileLookTableData`, which is
HSV - see COBALT_DCP_SPEC.md section 4.4.2). Decoding is a three-stage pipe:

1. **base85 text -> binary.** A Z85-like alphabet, not ASCII-order: the value
   of a character is looked up in a 96-entry `KDECODE` table (indexed by
   `ord(c) - 32`; `0xFF` marks the eight XML-unsafe chars that never appear).
   Digits are **least-significant-first** within each 5-char group; each full
   group yields a little-endian `u32`. A trailing partial group of `p` chars
   yields `p - 1` bytes.
2. **zlib inflate.** The first 4 bytes (LE `u32`) are the uncompressed size;
   the remainder is an RFC 1950 zlib stream.
3. **Parse `dng_rgb_table`.** Layout:
   `u32 type(=1)`, `u32 version(=1)`, `u32 dimensions(=3)`, `u32 divisions`,
   then `divisions^3` samples of `u16 r, u16 g, u16 b` stored as **deltas from
   an identity ramp** (`nop[i] = (i*0xFFFF + (N>>1)) / (N-1)`), then
   `u32 primaries`, `u32 transfer`, `u32 gamut`, `f64 min_amount`,
   `f64 max_amount`.

For all 17 supplied Looks the decoded table is: `divisions = 32` (32768
samples), `primaries = ProPhoto`, `transfer = Gamma18` (1.8 gamma), `gamut =
Clip`, `min_amount = 0.2`, `max_amount = 1.5`. The black corner `(0,0,0)` and
white corner `(31,31,31)` are exactly identity; interior samples carry small
deltas that encode the grade.

## Pipeline placement (important for W2/W3)

The Cobalt Look table is a **3-D RGB LUT** applied in **ProPhoto primaries with
a 1.8 gamma transfer**, as stage 7 in the COBALT_DCP_SPEC.md section 3.1
pipeline. This is a different mechanism from the DCP's HSV `ProfileLookTable`
(section 4.4.2) - do not conflate them. W2/W3 must apply the Cobalt Look as
trilinear 3-D LUT interpolation in ProPhoto/1.8-gamma space, then return to
the working space. The `Amount` slider (section 4.5) scales the Look layer
only; for an RGB LUT, Amount is best implemented as a lerp between the identity
ramp and the table output (`out = lerp(identity, table_out, k)`, `k = amount`),
which matches ACR's `min_amount`/`max_amount` range of [0.2, 1.5] surfaced on
these Looks.

## Licensing

The reader is **read-only**. There is no writer, no re-encoder, and no export
of Cobalt tables in any form. No Cobalt table data (decoded or encoded) is
committed to the repository - tests that need the supplied XMPs are
`#[ignore]`d and gated on `RAPIDRAW_TEST_ASSETS=1`, reading from a local path
(`RAPIDRAW_LOOKS_DIR`, defaulting to the user's download location). This is
interoperability for files the user already owns (spec section 8.3).

## Route B (Profile Capture) - documented fallback

Route A (native decode) is preferred and works for all 17 supplied Looks. Route B
is the fallback for future Look versions whose embedded table cannot be decoded,
or for non-Cobalt ACR profiles that carry no embedded table at all. It is a
**full ACR round-trip capture** using machinery RapidRAW already ships:
`generate_identity_lut_image` and `convert_image_to_cube_lut` in
`src-tauri/src/lut_processing.rs` (verified at lines 430 and 450 respectively).

### Workflow for the user

1. **Export the identity HALD.** In RapidRAW, select the Look in the profile
   browser and choose "Capture profile…". RapidRAW writes a 16-bit TIFF
   identity HALD image to the path you choose. The default grid size is 33
   (fast; 35937 pixels). 64 gives finer interpolation at the cost of a larger
   file (262144 pixels).

2. **Process in ACR/Lightroom.** Open the exported TIFF in Lightroom or Adobe
   Camera Raw. Set:
   - **Profile = the Cobalt Look you want to capture**, Amount 100.
   - **Every other control zeroed** — exposure, contrast, tone curve linear,
     no sharpening, no noise reduction, no lens corrections, no vignetting.
   - Export **16-bit TIFF**, ProPhoto RGB, full resolution, no output
     sharpening.

3. **Import the processed HALD.** Back in RapidRAW, select the same Look and
   choose "Import captured LUT…", pointing to the TIFF you exported from ACR.
   RapidRAW converts it to a `.cube` 3-D LUT, saves it linked to the Look's
   UUID, and upgrades the Look from "table unavailable" to "captured".

4. **The capture persists.** The `.cube` file lives in
   `{app_data}/profiles/looks/captured/<uuid>.cube` and is picked up
   automatically on next launch. You only need to capture once per Look.

### Honest caveat — highlight roll-off

A 3-D RGB LUT with domain [0,1]³ cannot capture behaviour that depends on
scene-referred data outside the [0,1] range. ACR's rendering pipeline can
apply the Look to values above 1.0 (scene-linear highlights before tone
mapping) and then roll them off. The captured LUT only sees post-clip [0,1]
inputs, so **highlight roll-off above clipping may differ slightly from ACR**
— particularly in specular highlights and bright skies where the original
Look applies a gentle shoulder.

Mitigations:
- **Capture at size 64** instead of 33 — finer grid spacing reduces
  interpolation error near the top end, where the gradient is steepest.
- **Pre-process the HALD through a wide log encoding** before ACR if you
  need highlight fidelity. The `.cube` file records whatever transform was
  applied to [0,1] samples; a log encoding redistributes precision towards
  shadows where it matters more.
- **Route A has no such limitation** (the native table decoder recovers the
  full scene-referred transform), which is why it is preferred now that it
  works for all supplied Looks.

### Technical notes for W4/W3

The captured LUT is a standard `.cube` 3-D RGB LUT with `DOMAIN_MIN 0.0 0.0 0.0`
and `DOMAIN_MAX 1.0 1.0 1.0`. It is applied at stage 7 of the §3.1 pipeline
(ProPhoto primaries, 1.8 gamma encoding) via trilinear interpolation —
mechanically identical to the existing creative LUT stage, just placed at the
Cobalt Look position in the chain. The Amount slider (§4.5) lerps between
identity and the table output: `out = lerp(identity, table_out, k)` where
`k = amount` (default 1.0, range 0.2–1.5 for these Looks).
