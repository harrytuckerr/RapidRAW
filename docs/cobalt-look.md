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

If native decoding ever breaks for a future Look version, Route B captures the
Look via an ACR round-trip using machinery RapidRAW already has:
`generate_identity_lut_image` and `convert_image_to_cube_lut` in
`src-tauri/src/lut_processing.rs` (verify line numbers before use). Workflow:
export an identity HALD (16-bit TIFF, size 33 or 64) from RapidRAW; open in
Lightroom/ACR with the Cobalt Look at Amount 100 and all other controls zeroed;
export 16-bit TIFF; re-import into RapidRAW, which runs
`convert_image_to_cube_lut` and stores the result linked to the Look's `UUID`
and `RGBTable` id.

**Honest caveat:** a 3-D RGB LUT cannot capture behaviour that depends on
scene-referred data outside [0,1], so highlight roll-off above clipping may
differ slightly from ACR. Mitigate by capturing in a wide, log-ish encoding and
documenting the limitation. Route A has no such limitation, which is why it is
preferred now that it works.
