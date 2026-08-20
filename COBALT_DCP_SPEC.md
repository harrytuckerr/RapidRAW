# RapidRAW — Adobe DCP Camera-Profile Pipeline + Cobalt Image Support
## Master Specification & Agent Orchestration Brief

**Document version:** 1.0
**Date:** 2026-08-20
**Fork:** `https://github.com/harrytuckerr/RapidRAW` (fork of `CyberTimon/RapidRAW`)
**Upstream license:** AGPL-3.0 — **this fork must remain AGPL-3.0.**
**Target platforms:** Windows, macOS, Linux (Tauri desktop) **and** Android (same repo, `src-tauri/gen/android`)

---

## 0. THE PROJECT

### 0.1 What we are building, and why

**The problem.** Cobalt Image profiles — and Adobe DCP camera profiles generally — only work inside Lightroom / Adobe Camera Raw. The user has purchased Cobalt profiles and wants to use them in RapidRAW, their preferred editor, on both desktop and Android. Today that is impossible, because **RapidRAW has no concept of a camera profile at all.** It has Presets and it has creative LUTs, but it has no *Profile* slot — the thing Lightroom exposes as `Adobe Color` / `Adobe Landscape` / `Camera Standard`, which sits underneath every other adjustment and defines the base rendering of a RAW file.

**What we are building.** Three things, in this order of importance:

1. **A profile slot in RapidRAW** — a new first-class pipeline stage holding `(base DCP, optional Look, amount)`, positioned *before* every existing adjustment, with presets, masks and LUTs continuing to stack on top exactly as they do now. This is the architectural core; everything else hangs off it.
2. **A complete Adobe DCP renderer** — dual-illuminant matrix interpolation, forward matrices, HueSatMap, profile tone curve and look table — running on both CPU (reference) and GPU (production), on desktop and Android.
3. **Cobalt-specific support** — parsing Cobalt Look XMPs, enforcing the correct per-camera base-profile pairing, and acquiring Look tables either natively or via a capture round-trip.

**What we are explicitly *not* building:** a Lightroom clone, a profile *authoring* tool, a profile converter/exporter, or anything that lets Cobalt profiles be redistributed to people who have not bought them. Read-only interoperability for files the user already owns.

### 0.2 End goal — definition of done

The project is complete when **all** of the following are true and demonstrated:

> **The user story.** A photographer opens a RAW file in their RapidRAW fork — on Windows, macOS, Linux **or** Android — picks a Cobalt base profile for that camera body from a Profile dropdown, optionally layers a Cobalt Look on top and dials it with an Amount slider, and gets a render matching Adobe Camera Raw. Their existing presets, masks and LUTs still work, on top, unchanged.

| # | Done when | Verified by |
|---|---|---|
| D1 | A profile slot exists in the UI, above Basic, on desktop **and** Android | W6, W8 |
| D2 | Selecting a Cobalt base DCP renders within **ΔE2000 mean < 2.0** of ACR | §7.3, W9 |
| D3 | A Cobalt Look layers on its correct base, with a working Amount slider | W7, W6 |
| D4 | A Look whose base profile is missing is **visibly disabled with a specific reason** — never silently mis-rendered | §2.3, W5 |
| D5 | Presets, masks and LUTs still stack on top and are unaffected by the profile | §5.3, W6 |
| D6 | With **no** profile selected, output is **bit-identical** to upstream RapidRAW | §3.1, W9 |
| D7 | Editor preview, export, thumbnail and CLI all agree within ΔE mean < 1.0 | W4 |
| D8 | Profiles import via file picker on desktop and via SAF on Android | W5, W8 |
| D9 | The whole §7.4 test matrix passes | W9 |
| D10 | No vendor assets in the repo; `EmbedNever` honoured; AGPL intact | §8, W0 |

**Minimum shippable outcome.** If W7 (Cobalt Look tables) fails outright, **D1, D2, D4–D10 still ship** and the product is genuinely useful: a full DCP camera-profile pipeline with correct Cobalt base-profile support. Only D3 is at risk. This is deliberate — see §6.W7.

### 0.3 Repository, branches, and where the work happens

| | |
|---|---|
| **Working fork (all work happens here)** | `https://github.com/harrytuckerr/RapidRAW` |
| **Upstream (read-only reference)** | `https://github.com/CyberTimon/RapidRAW` — default branch `main`, AGPL-3.0 |
| **Local working directory** | `/Users/harrisontucker/Documents/CoWork/RapidRaw` |
| **Integration branch** | `cobalt/main` — every PR targets this, never `main` |
| **Feature branches** | `cobalt/w1-dcp-parser`, `cobalt/w2-cpu-render`, … one per workstream |
| **Android** | **Same repository** — `src-tauri/gen/android/`. There is no separate Android repo to fork. |

**`main` is kept clean and trackable against upstream** so upstream changes can be merged without conflict archaeology. All project work lands on `cobalt/main`.

**⚠️ The local directory is NOT empty** — it already contains `COBALT_DCP_SPEC.md` and `tools/`. `git clone` refuses a non-empty target, so W0 must use the init-and-fetch sequence in §6.W0, not a plain clone. These files are part of the project and must be committed onto `cobalt/main`, not deleted.

**Vendor assets live outside the repo** and are never committed (§8.1):
- `/Users/harrisontucker/Downloads/Cobalt_CCD_Fever_3/` — 17 Look XMPs + manual
- `/Users/harrisontucker/Downloads/Fujifilm X-Pro2 Cobalt Flat v3.0.dcp` — the base DCP

---

## 0.4 HOW TO USE THIS DOCUMENT (orchestrator agent)

You are the **orchestrator**. You will dispatch sub-agents against the workstreams in §6. Rules:

1. **Do not skip §1–§5.** Every sub-agent prompt you write MUST embed the relevant subsections verbatim. Sub-agents start cold and cannot re-derive this.
2. **Respect the dependency graph in §6.0.** Do not dispatch a workstream whose dependencies are unmet.
3. **Decision gates (§9) are blocking.** When a gate is reached, stop and surface the result to the human. Do not choose for them.
4. **Ground truth beats assumption.** Every factual claim in §1 was verified against the actual files and the actual upstream source on 2026-08-20. Claims marked ⚠️ **UNVERIFIED** are explicitly *not* established and must be proven by the responsible workstream before code depends on them.
5. **No silent scope changes.** If a workstream turns out to be infeasible, report it; do not substitute an approximation without a decision gate.

### 0.5 Definition of terms (use these exactly; do not invent synonyms)

| Term | Meaning |
|---|---|
| **DCP** | Adobe DNG Camera Profile. Binary TIFF-like file, magic `IIRC`. Contains colour matrices + rendering tables. Per-camera. |
| **Look / Look XMP** | An `.xmp` file with `crs:PresetType="Look"` containing an embedded RGB look table. Camera-agnostic. Sits *on top of* a DCP. |
| **Base profile** | The DCP that a Look declares via `crs:CameraProfile`. Must match the photo's camera. |
| **Profile slot** | The new first-class pipeline stage this project adds. Holds `(base DCP, optional Look, amount)`. Analogous to Lightroom's "Profile" dropdown (`Adobe Color`, `Adobe Landscape`, …). |
| **Preset** | RapidRAW's existing adjustment bundle. **Unchanged by this project.** Stacks *on top of* the profile slot. |
| **LUT** | RapidRAW's existing `.cube`/`.3dl`/HALD support. **Unchanged.** A late-stage creative stage, distinct from the profile slot. |
| **Working space** | RapidRAW's internal linear RGB space (sRGB/Rec.709 primaries, linear transfer). |

---

## 1. GROUND TRUTH — WHAT THESE FILES ACTUALLY ARE

> ⚠️ **The single most important correction in this document:** the Cobalt "CCD Fever" package the user supplied contains **no DCP files at all.** They are XMP Looks. The DCP is a *separate* product. Any agent that assumes "Cobalt profile == .dcp file" will build the wrong thing.

### 1.1 Verified facts — the Look XMPs

Source: `/Users/harrisontucker/Downloads/Cobalt_CCD_Fever_3/` — 17 `.xmp` files + 1 PDF manual.

All 17 files share this exact structure (verified across all 17):

```xml
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF>
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Look"
   crs:Cluster="Cobalt-Image"
   crs:UUID="369CEFB650E07246A4C925B6449A0E01"     <!-- unique per file -->
   crs:SupportsAmount="True"
   crs:SupportsColor="True"
   crs:SupportsMonochrome="False"
   crs:SupportsHighDynamicRange="True"
   crs:SupportsNormalDynamicRange="True"
   crs:SupportsSceneReferred="True"
   crs:SupportsOutputReferred="False"
   crs:RequiresRGBTables="False"
   crs:CameraModelRestriction=""                    <!-- EMPTY on all 17 -->
   crs:Copyright="© Cobalt-Image 2024"
   crs:ContactInfo="info@cobalt-image.com"
   crs:Version="16.1.1"                             <!-- or 16.2 -->
   crs:ProcessVersion="15.4"
   crs:ConvertToGrayscale="False"                   <!-- False on all 17, incl. B&W looks -->
   crs:CameraProfile="Cobalt Modular"               <!-- IDENTICAL on all 17 -->
   crs:RGBTable="B83D0CA5772B34B5B26CE93ABEA395F6"  <!-- unique per file -->
   crs:Table_B83D0CA5772B34B5B26CE93ABEA395F6="Lir00((5jOIEn+/=bvT:…"  <!-- ~220–245 KB -->
   crs:HasSettings="True">
   <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Pentax 645D Portrait</rdf:li></rdf:Alt></crs:Name>
   <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Cobalt CCD fever v3.0</rdf:li></rdf:Alt></crs:Group>
   <crs:ShortName/> <crs:SortName/> <crs:Description/>   <!-- all empty -->
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
```

> **⚠️ XML parsing trap — this bug has already been hit once during spec preparation.**
> `ShortName`, `SortName` and `Description` are empty, and empty values are written as a
> **self-closing** `<rdf:li xml:lang="x-default"/>`. A lazy regex such as
> `<crs:ShortName>.*?<rdf:li[^>]*>([^<]*)</rdf:li>` **skips straight past the self-closing tag
> and captures the next element's text** — silently yielding `ShortName = "Cobalt CCD fever v3.0"`
> (the `Group` value). The failure is invisible: you get a plausible-looking string, not an error.
> **Requirement:** scope the match to the closing tag (`<crs:X>(.*?)</crs:X>`) *before* looking for
> `rdf:li`, or use a real XML parser (`quick-xml` is already a dependency). Add a unit test
> asserting `ShortName == ""` and `SortName == ""` for all 17 Looks — a test that would fail
> under the naive regex. Note that upstream `preset_converter.rs::extract_xmp_name` uses this
> regex style; it is bounded by `</crs:Name>` so it happens to be safe, but do not copy the
> pattern unbounded.

**The 17 Looks** (all `Group = "Cobalt CCD fever v3.0"`, all `CameraProfile = "Cobalt Modular"`):

`Fuji S5Pro F3a` · `Fuji S5Pro F3b` · `Fuji S5Pro F3c` · `Fuji S5Pro FujiChrome` · `Fuji S5Pro ProNegative` · `Fuji S5Pro Standard` · `Leica M8` · `Leica M9 BW Medium` · `Leica M9 Standard Sat` · `Nikon D200 Mode I` · `Pentax 645D Bright` · `Pentax 645D Landscape` · `Pentax 645D Miyabi` · `Pentax 645D Monochrome` · `Pentax 645D Natural` · `Pentax 645D Portrait` · `Pentax 645D Satobi`

**Critical semantic point:** the camera name in a Look's title is the **emulated** camera, *not* a constraint on the source camera. "Pentax 645D Portrait" applied to a Fujifilm X-Pro2 file is the intended use — the entire premise of "CCD Fever" is making modern sensors render like old CCD bodies. `CameraModelRestriction=""` confirms Adobe applies no source-camera restriction.

### 1.2 Verified facts — the embedded look table blob

- Attribute name: `crs:Table_<RGBTable-UUID>`; value length **220,047 – 245,249 characters**.
- Character set: **exactly 85 distinct characters**, `ord` range 33–125, excluding `"` `&` `,` `;` `<` `>` `\` `_` — i.e. an **XML-attribute-safe base-85 alphabet**.
- **Every one of the 17 blobs begins with the literal 5-character magic `Lir00`.** Character 6 varies per file.
- ⚠️ **UNVERIFIED / OPEN:** the exact decode. A naïve "ASCII-order alphabet, 5 chars → `u32`" Ascii85 decode **fails**: ~3.15 % of 5-char groups exceed `2^32`, which precisely matches the statistical expectation for *arbitrary* data under a wrong alphabet/packing (`(85^5 − 2^32) / 85^5 = 3.20 %`). Conclusion: **the classic Ascii85 uint32 grouping does not hold.** The format is undocumented by Adobe.

> **This blob is the single largest technical risk in the project.** It is quarantined into workstream **W7**, which has a hard decision gate and a fully-specified fallback that does not require decoding it at all (§6.W7).

### 1.3 Verified facts — the DCP

Source: `/Users/harrisontucker/Downloads/Fujifilm X-Pro2 Cobalt Flat v3.0.dcp` (1,102,942 bytes).

Header: `49 49 52 43` = `II` + magic `0x4352`, IFD at offset 8, **18 entries**. Full tag dump (verified):

| Tag | Hex | Name | Type | Count | Value |
|---|---|---|---|---|---|
| 50708 | C614 | UniqueCameraModel | ASCII | 16 | `Fujifilm X-Pro2` |
| 50721 | C621 | ColorMatrix1 | SRATIONAL | 9 | `[1.339, −0.731, 0.0216, −0.3983, 1.1994, 0.2238, −0.0435, 0.1035, 0.6328]` |
| 50722 | C622 | ColorMatrix2 | SRATIONAL | 9 | `[1.1434, −0.4948, −0.121, −0.3746, 1.2042, 0.1903, −0.0666, 0.1479, 0.5235]` |
| 50778 | C65A | CalibrationIlluminant1 | SHORT | 1 | `17` → **Standard Illuminant A** |
| 50779 | C65B | CalibrationIlluminant2 | SHORT | 1 | `21` → **D65** |
| 50932 | C6F4 | ProfileCalibrationSignature | ASCII | 10 | `com.adobe` |
| 50936 | C6F8 | ProfileName | ASCII | 12 | `Cobalt Flat` |
| 50937 | C6F9 | ProfileHueSatMapDims | LONG | 3 | `[90, 30, 1]` (hue, sat, val) |
| 50938 | C6FA | ProfileHueSatMapData1 | FLOAT | 8100 | 2700 triplets |
| 50939 | C6FB | ProfileHueSatMapData2 | FLOAT | 8100 | 2700 triplets |
| 50940 | C6FC | ProfileToneCurve | FLOAT | 16384 | 8192 (x,y) pairs |
| 50941 | C6FD | ProfileEmbedPolicy | LONG | 1 | `2` |
| 50942 | C6FE | ProfileCopyright | ASCII | 21 | `(c)Cobalt-Image 2024` |
| 50964 | C714 | ForwardMatrix1 | SRATIONAL | 9 | `[0.5852, 0.2478, 0.1314, 0.2148, 0.7488, 0.0364, 0.0075, 0.0195, 0.7981]` |
| 50965 | C715 | ForwardMatrix2 | SRATIONAL | 9 | `[0.525, 0.2687, 0.1706, 0.198, 0.752, 0.05, 0.0001, 0.0111, 0.8139]` |
| 50981 | C725 | ProfileLookTableDims | LONG | 3 | `[90, 30, 30]` |
| 50982 | C726 | ProfileLookTableData | FLOAT | 243000 | 81000 triplets |
| 51108 | C7A4 | ProfileLookTableEncoding | LONG | 1 | `1` |

Notes the implementer must not miss:

- **Dual-illuminant** profile. Both `ColorMatrix1/2` and `ForwardMatrix1/2` present → the full interpolation path in §4.2 is mandatory, not optional.
- `ProfileHueSatMapEncoding` (51107) is **absent** → defaults to `0` (Linear). `ProfileLookTableEncoding` is present and `= 1` (**sRGB**). These two tables therefore operate in **different encodings**. Getting this wrong is a common and highly visible bug.
- `ProfileEmbedPolicy = 2`. Per the DNG spec this is **"embed never."** → **The exporter MUST NOT embed this profile into exported DNG/TIFF files.** See §8.2.
- No `CameraCalibration1/2`, no `AnalogBalance`, no `BaselineExposureOffset`, no `DefaultBlackRender` → treat as identity / zero defaults, but the parser must still handle them when present in other Cobalt profiles.
- This DCP is `ProfileName = "Cobalt Flat"`, **not** `"Cobalt Modular"`. See the pairing problem in §2.3.

### 1.4 Verified facts — the vendor's own documentation

From `Cobalt CCD fever modular package.pdf` (pages 2–4), quoted:

> "A basic DNG package for your camera is required in order to use this feature."

Install locations Cobalt documents for ACR:
- **macOS:** `~/Library/Application Support/Adobe/CameraRaw/Settings/Cobalt/`
- **Windows:** `C:\ProgramData\Adobe\CameraRaw\Settings\Adobe\Profiles\Cobalt\`

This confirms: **CCD Fever is a Looks-only add-on. The per-camera base DCP packs are sold separately.** The user must own the base pack for each camera body they shoot.

---

## 2. THE PAIRING MODEL (the thing most likely to be built wrong)

### 2.1 The Lightroom mental model — replicate this exactly

1. Cobalt **base DCP profiles are installed per camera body.**
2. A Cobalt **Look XMP is applied to an image whose camera has the Cobalt base pack installed.**
3. Together they occupy the **Profile** slot — the same slot as `Adobe Color`, `Adobe Landscape`, `Camera Standard`. They are *image profiles*, **not presets**.
4. **Lightroom presets stack on top, independently.** Changing the profile does not disturb the preset, and vice-versa.

RapidRAW today has **Presets** and **LUTs** but **no profile slot whatsoever**. Creating that slot is the architectural core of this project.

### 2.2 The resolution rule (implement exactly)

Given a photo `P` and a user-selected Look `L`:

```
camera        := UniqueCameraModel derived from P's RAW metadata     (§4.1)
required_base := L.crs:CameraProfile                                  ("Cobalt Modular")
base_dcp      := registry.lookup(camera_model = camera,
                                 profile_name = required_base)
```

- If `base_dcp` resolves → **satisfied.** Render `base_dcp` → then `L`.
- If it does **not** resolve → **unsatisfiable.** The Look MUST be blocked. It must **not** be applied over a different base, over the camera's stock matrix, or over `Adobe Color`-equivalent. Doing so produces confidently wrong colour with no visible error.

**Two-key lookup.** A profile is identified by the pair `(UniqueCameraModel, ProfileName)` — never by filename, never by `ProfileName` alone. `"Cobalt Flat"` for the X-Pro2 and `"Cobalt Flat"` for a Pentax 645D are different files with different matrices.

### 2.3 The known live mismatch — do not "fix" it by loosening the rule

The supplied assets currently **do not** pair:

| | |
|---|---|
| All 17 Looks require | `CameraProfile = "Cobalt Modular"` |
| Supplied DCP provides | `ProfileName = "Cobalt Flat"`, `UniqueCameraModel = "Fujifilm X-Pro2"` |

So with only these files installed, **every Look is unsatisfiable.** This is expected: the user owns the `Cobalt Flat` DCP but the Looks need the `Cobalt Modular` DCP for their camera.

**The correct product behaviour is to say so, clearly.** The UI must surface: *"'Pentax 645D Portrait' requires the base profile 'Cobalt Modular' for Fujifilm X-Pro2, which is not installed."* — plus a list of which base profiles *are* installed for that body.

> **Instruction to all agents:** do **not** implement a fuzzy/nearest-match fallback that quietly substitutes a different base DCP. Do not match on `Cluster`. Do not fall back to the camera's embedded matrix while leaving the Look applied. If the pair is unsatisfiable, the Look is disabled with an explanatory message. This is a hard requirement.

### 2.4 Amount / intensity

`crs:SupportsAmount="True"` on all 17 Looks → the profile slot exposes an **Amount** slider (ACR shows this for Looks). Semantics in §4.5. The base DCP itself has **no** amount — it is always applied at full strength when selected. Only the Look layer is scalable.

### 2.5 What a user sees in the profile browser

One flat, grouped browser (mirroring ACR):

```
Profile ▾
├─ Adobe-equivalent / RapidRAW built-ins
│   └─ (existing default rendering — must remain the default)
├─ Camera profiles for this body            ← DCPs whose UniqueCameraModel matches
│   ├─ Cobalt Flat                          ← selectable on its own (base only)
│   └─ Cobalt Modular
└─ Cobalt CCD fever v3.0                    ← from crs:Group; Looks
    ├─ Fuji S5Pro Standard        [Amount ▭▭▭▭▭▭▭░░░ 100]
    ├─ Pentax 645D Portrait                              ← greyed if base unsatisfiable
    └─ …
```

Selecting a **Look** implicitly activates its required base DCP. Selecting a **base DCP** clears any Look.

---

## 3. TARGET ARCHITECTURE

### 3.1 Where the profile slot sits in the pipeline

Verified current behaviour (`src-tauri/src/raw_processing.rs`): RapidRAW builds a `rawler::RawDevelop` and **removes `ProcessingStep::SRgb`**, keeping `Calibrate`. `Calibrate` (verified in `rawler/src/imgop/develop.rs`) builds `xyz2cam` from `rawimage.color_matrix` — preferring the `D65` entry — inverts it, and converts camera → XYZ → sRGB primaries. **That step is exactly what a DCP replaces.**

```
                            ┌─────────── NEW: PROFILE SLOT ───────────┐
RAW file
  → rawler: Rescale, Demosaic, CropActiveArea, WhiteBalance, CropDefault
  → [Calibrate REMOVED when a DCP profile is active]
  →                        │  camera-native, white-balanced linear RGB │
  →                        │    1. illuminant interpolation            │
  →                        │    2. ForwardMatrix → XYZ(D50)            │
  →                        │    3. XYZ(D50) → ProPhoto linear          │
  →                        │    4. HueSatMap  (encoding 0 = linear)    │
  →                        │    5. LookTable  (encoding 1 = sRGB)      │
  →                        │    6. ProfileToneCurve                    │
  →                        │    7. Cobalt Look table  × Amount   (W7)  │
  →                        │    8. ProPhoto → working space            │
                            └──────────────────────────────────────────┘
  → EXISTING RapidRAW global adjustments (exposure, contrast, curves, colour grading…)
  → EXISTING masks
  → EXISTING creative LUT stage
  → tone-mapping / display transform / export
```

**Invariant (must hold, and must be regression-tested):** with **no** profile selected, output is **bit-identical** to current upstream RapidRAW. The profile slot is strictly additive and off by default.

**Invariant:** presets, masks and LUTs are untouched and continue to stack on top. A preset must never implicitly change the profile unless the preset explicitly captured one (§5.3).

### 3.2 New / modified files

**New Rust module — `src-tauri/src/dcp/`:**

| File | Responsibility |
|---|---|
| `mod.rs` | Public API, re-exports, `DcpError` |
| `parser.rs` | Binary DCP (`IIRC`) reader → `DcpProfile` (W1) |
| `model.rs` | `DcpProfile`, `HueSatMap`, `ToneCurve`, `LookTable`, `Illuminant` types |
| `render.rs` | CPU reference implementation of the render chain (W2) |
| `interpolate.rs` | Illuminant weighting, matrix interpolation, HSV mapping |
| `registry.rs` | Discovery, indexing, two-key `(camera, profile_name)` lookup, pairing state (W5) |
| `look_xmp.rs` | Cobalt Look XMP parsing + table acquisition (W7) |
| `commands.rs` | `#[tauri::command]` surface |

**Modified:**

| File | Change |
|---|---|
| `src-tauri/src/raw_processing.rs` | Conditionally strip `ProcessingStep::Calibrate`; expose camera-native RGB + `wb_coeffs` + `color_matrix` |
| `src-tauri/src/image_processing.rs` | Thread profile state into `RenderRequest` |
| `src-tauri/src/gpu_processing.rs` | Bind DCP textures/uniforms |
| `src-tauri/src/shaders/shader.wgsl` | New DCP stage (W3) |
| `src-tauri/src/lib.rs` | Register `dcp` module + commands |
| `src-tauri/src/file_management.rs` | Extend `Preset` to optionally carry a profile ref |
| `src-tauri/src/android_integration.rs` | Content-URI import for `.dcp`/`.xmp` (W8) |
| `src-tauri/Cargo.toml` | (Likely **no new deps** — see §3.3) |
| `src/components/panel/right/ControlsPanel.tsx` | Mount profile panel above Basic |
| `src/utils/adjustments.ts` | Add profile fields + defaults |

**New frontend:**

| File | Responsibility |
|---|---|
| `src/components/panel/right/ProfilePanel.tsx` | Profile slot UI, grouped browser, Amount slider |
| `src/components/ui/ProfileBrowser.tsx` | Grouped/searchable list, unsatisfiable-state rendering |
| `src/hooks/useProfiles.ts` | Fetch/cache registry, invalidation on import |

### 3.3 Dependencies — prefer zero new crates

Everything needed is already in `src-tauri/Cargo.toml` (verified): `quick-xml 0.41`, `regex`, `nalgebra 0.35`, `glam 0.33`, `bytemuck`, `half`, `memmap2`, `image`, `wgpu 29`, `serde`. **Write the DCP IFD reader by hand** — it is ~200 lines and avoids pulling a TIFF crate that would fight the `IIRC` magic. **Do not add the Adobe `dng_sdk`** (C++, would break the Android build and complicate AGPL compliance).

> `wgpu` is pinned to 29.0 with the comment *"Downgraded to prevent P3 color shifts on Apple devices."* **Do not bump it.**

---

## 4. COLOUR SCIENCE — THE RENDER CHAIN (normative)

> This section is the contract for W2/W3. Implement it literally. Where a step is marked ⚠️ **VERIFY**, the workstream must prove the behaviour empirically (§7) before it is considered done — do not guess and move on.

### 4.1 Input to the profile stage

- **Pixels:** white-balanced, demosaiced, **camera-native** linear RGB. Obtained by removing `ProcessingStep::Calibrate` **and** `ProcessingStep::SRgb` from `RawDevelop::steps`, keeping `WhiteBalance`.
- **`as_shot_neutral`:** derived from `rawler` `RawImage::wb_coeffs: [f32; 4]`. DNG `AsShotNeutral` is the **reciprocal** of the multiplicative WB coefficients — normalise so G = 1, then invert. ⚠️ **VERIFY** the exact convention `rawler` uses before trusting it; a reciprocal error here inverts the entire colour cast and is easy to miss.
- **Camera identity:** `RawImage::clean_model` / `make` / `model` → the string matched against DCP `UniqueCameraModel`. Matching rules in §5.2.

### 4.2 Illuminant interpolation

`CalibrationIlluminant1 = 17 (StdA ≈ 2856 K)`, `CalibrationIlluminant2 = 21 (D65 ≈ 6504 K)`.

1. Map illuminant enums → correlated colour temperature. Support at minimum: `1` Daylight 5500, `2` Fluorescent 4200, `3`/`17` Tungsten/StdA 2856, `4`/`10` Flash 5500, `18` StdB 4874, `19` StdC 6774, `20` D55 5500, `21` D65 6504, `22` D75 7504, `23` D50 5003, `24` ISO Studio Tungsten 3200.
2. Solve for the CCT of `as_shot_neutral`. **This is iterative** (dng_sdk `dng_color_spec::FindXYZtoCamera`): guess a temperature, build the interpolated `ColorMatrix`, map the neutral to xy, compute its CCT, repeat. **3 iterations is sufficient and is what dng_sdk uses.** Use the Robertson method for xy → CCT.
3. Interpolation weight in **reciprocal temperature (mireds)**, never linear temperature:
   ```
   g = (1/T − 1/T2) / (1/T1 − 1/T2),  clamped to [0, 1]
   ```
   where `T1` = illuminant 1 CCT, `T2` = illuminant 2 CCT. If `T1 == T2`, or only one illuminant is present, `g = 1`.
4. Interpolate **element-wise**: `M = g·M1 + (1−g)·M2` for `ColorMatrix`, `ForwardMatrix`, and `CameraCalibration` when present.

### 4.3 Camera → XYZ(D50)

`ForwardMatrix` is present, so use the **forward-matrix path** (higher quality; the `ColorMatrix`-inverse path is only a fallback for profiles lacking FM):

```
AB  = AnalogBalance      (diagonal; identity if absent — absent here)
CC  = CameraCalibration  (identity if absent — absent here)
refNeutral  = Inverse(AB · CC) · as_shot_neutral
D           = Invert(Diagonal(refNeutral))
camToXYZ_D50 = ForwardMatrix · D · Inverse(AB · CC)
```

`ForwardMatrix` is **defined to map to XYZ with a D50 white point**, so no chromatic adaptation follows. If `ForwardMatrix` is absent: `camToXYZ = Inverse(ColorMatrix)`, then Bradford-adapt from the calibration illuminant's white point to D50.

### 4.4 The rendering tables

**Working space for all table operations: linear ProPhoto RGB (ROMM), white point D50.** XYZ(D50) → ProPhoto matrix is standard and requires no adaptation.

#### 4.4.1 HueSatMap — `Dims = [90, 30, 1]`, encoding **0 (Linear)**

- Layout: `hueDivisions=90`, `satDivisions=30`, `valDivisions=1`. Index order is **`v` outermost → `h` → `s` innermost**; each entry is `(hueShift°, satScale, valScale)`.
- `valDivisions == 1` → the map is 2-D (hue × sat); value is ignored. Handle `valDivisions > 1` too, for other Cobalt profiles.
- Convert ProPhoto **linear** RGB → HSV, apply, convert back:
  ```
  h' = h + hueShift        (wrap modulo 360)
  s' = clamp(s · satScale, 0, 1)
  v' =       v · valScale   (do NOT clamp v — scene-referred data exceeds 1.0)
  ```
- **Hue interpolation must wrap** (index 89 → 0). Saturation and value **must clamp** at the edges, not wrap.
- Interpolation: trilinear (bilinear when `valDivisions == 1`).
- **Dual-illuminant:** interpolate between `HueSatMapData1` and `HueSatMapData2` using the **same `g`** from §4.2. Interpolate the *table entries*, not the resulting colours.

#### 4.4.2 LookTable — `Dims = [90, 30, 30]`, encoding **1 (sRGB)**

Identical HSV mechanism, but because `ProfileLookTableEncoding = 1`, the RGB values must be **sRGB-transfer-encoded before** the HSV conversion and **linearised after**:

```
rgb_enc = linear_to_srgb_transfer(rgb_prophoto_linear)   // transfer curve only; primaries unchanged
hsv     = rgb_to_hsv(rgb_enc)
hsv'    = apply_look_table(hsv)
rgb_enc'= hsv_to_rgb(hsv')
rgb     = srgb_transfer_to_linear(rgb_enc')
```

> ⚠️ The HueSatMap (encoding 0) does **not** get this treatment. Two tables, two encodings, in one pipeline. Assert on the parsed encoding values rather than hard-coding.
> Negative/HDR values: the sRGB transfer must be applied **sign-symmetrically** (`sign(x)·f(|x|)`) so scene-referred data survives the round trip. `shader.wgsl` already contains `linear_to_srgb_extended` — reuse that convention.

#### 4.4.3 ProfileToneCurve — 8192 (x, y) pairs

- Stored as `FLOAT count=16384`, interleaved `x0,y0,x1,y1,…`, x monotonically increasing over [0,1].
- Resample to a **uniform 1-D LUT** (recommend 4096 entries) at load; do not binary-search per pixel.
- Applied in ProPhoto space. Values > 1.0 must be handled: clamp input to the curve domain but preserve the highlight ratio, or extrapolate linearly from the last segment. ⚠️ **VERIFY** against ACR on a clipped-highlight test file.
- dng_sdk applies the curve in a **hue- and saturation-preserving** manner rather than naïvely per-channel. ⚠️ **VERIFY** — this materially changes saturated highlight rendering. W2 must test both and pick the one matching ACR.

### 4.5 Stage order and Amount

⚠️ **VERIFY — HIGHEST-RISK ITEM IN THE PROJECT.** The relative order of HueSatMap, LookTable and ProfileToneCurve is **not** reliably documented in a form worth trusting from memory. The DNG spec and `dng_sdk`'s `dng_render.cpp` are the authorities.

**Provisional order to implement first:**
```
HueSatMap → BaselineExposureOffset → LookTable → ProfileToneCurve
```

**W2 must resolve this empirically** and record the outcome in `docs/dcp-pipeline.md` with evidence. The validation harness (§7.2) is the arbiter: swapping tone curve and look table produces a large, obvious difference on saturated test patches. **Do not ship until this is settled by measurement, not argument.**

**Amount** (`a` ∈ [0, 200], default 100, UI-scaled to `0.0–2.0`):
- `a` scales **only the Look layer** (§6.W7), never the base DCP.
- Implement as interpolation of the **table's effect**, not of the final pixel:
  `hueShift·k`, `satScale^k`, `valScale^k` where `k = a/100`.
  Exponential for the multiplicative terms, linear for the additive hue shift. This matches how ACR's Amount behaves beyond 100 % and avoids the desaturation artefacts of naïve pixel lerp.
- `a = 0` must be **exactly** identity (bypass the stage entirely).

### 4.6 Monochrome looks

`Leica M9 BW Medium`, `Pentax 645D Monochrome` are B&W looks, yet **all 17 files set `crs:ConvertToGrayscale="False"` and `SupportsMonochrome="False"`.** The desaturation is baked into the look table (via `satScale ≈ 0`), not signalled by a flag. **Do not special-case monochrome.** If the pipeline is correct, B&W falls out of the table automatically. If a "monochrome" look renders in colour, the bug is in the table application, not in a missing flag.

---

## 5. DATA MODEL & PERSISTENCE

### 5.1 Rust types (normative shape)

```rust
// dcp/model.rs
pub struct DcpProfile {
    pub id: ProfileId,                    // blake3 of file bytes — stable, dedupes
    pub file_path: PathBuf,
    pub profile_name: String,             // tag 50936; falls back to file stem
    pub unique_camera_model: String,      // tag 50708
    pub copyright: Option<String>,
    pub embed_policy: EmbedPolicy,        // tag 50941
    pub calibration_illuminant_1: Illuminant,
    pub calibration_illuminant_2: Option<Illuminant>,
    pub color_matrix_1: Mat3,
    pub color_matrix_2: Option<Mat3>,
    pub forward_matrix_1: Option<Mat3>,
    pub forward_matrix_2: Option<Mat3>,
    pub camera_calibration_1: Option<Mat3>,
    pub camera_calibration_2: Option<Mat3>,
    pub analog_balance: Option<[f32; 3]>,
    pub baseline_exposure_offset: Option<f32>,
    pub default_black_render: DefaultBlackRender,
    pub hue_sat_map: Option<DualHueSatMap>,
    pub look_table: Option<HsvTable>,
    pub look_table_encoding: TableEncoding, // Linear | Srgb
    pub hue_sat_map_encoding: TableEncoding,
    pub tone_curve: Option<ToneCurve>,
}

pub struct HsvTable { pub hue_div: u32, pub sat_div: u32, pub val_div: u32, pub data: Vec<[f32; 3]> }
pub struct DualHueSatMap { pub map_1: HsvTable, pub map_2: Option<HsvTable> }
pub enum TableEncoding { Linear, Srgb }
pub enum EmbedPolicy { AllowCopying, EmbedIfUsed, EmbedNever, NoRestrictions }

// dcp/look_xmp.rs
pub struct CobaltLook {
    pub id: LookId,
    pub file_path: PathBuf,
    pub uuid: String,                     // crs:UUID
    pub name: String,                     // crs:Name
    pub group: String,                    // crs:Group  → UI grouping
    pub cluster: String,                  // crs:Cluster
    pub required_base_profile: String,    // crs:CameraProfile → "Cobalt Modular"
    pub camera_model_restriction: Option<String>, // empty → None
    pub supports_amount: bool,
    pub table_uuid: String,               // crs:RGBTable
    pub table: LookTableSource,           // see W7
    pub copyright: Option<String>,
    pub process_version: String,
}

// The profile slot as persisted in adjustments
pub struct ProfileSelection {
    pub base: Option<ProfileId>,
    pub look: Option<LookId>,
    pub amount: f32,                      // 0.0 – 2.0, default 1.0
}
```

### 5.2 Camera-model matching (exact rules — do not improvise)

DCP `UniqueCameraModel` is `"Fujifilm X-Pro2"`. `rawler` exposes `make`, `model`, `clean_model`. Match in this order, stopping at the first hit:

1. Exact, case-sensitive: `unique_camera_model == clean_model`.
2. Exact, case-**insensitive**, after collapsing internal whitespace runs to one space and trimming.
3. Case-insensitive against `format!("{} {}", make, model)`, same normalisation.
4. Case-insensitive with manufacturer aliases applied to **both** sides: `Fujifilm ≡ FUJIFILM ≡ Fuji`, `Nikon ≡ NIKON CORPORATION`, `Canon ≡ Canon Inc.`, `Panasonic ≡ Lumix`, `Olympus ≡ OM Digital Solutions`, `Pentax ≡ RICOH IMAGING ≡ PENTAX`, `Leica ≡ Leica Camera AG`, `Sony ≡ SONY`, `Hasselblad`, `Phase One`.

**No fuzzy/edit-distance matching.** `fuzzy-matcher` is in `Cargo.toml` for other features — **do not use it here.** A wrong match silently produces wrong colour, which is worse than no match. If no rule hits, the profile is simply not offered for that body.

Maintain the alias table in one place: `src-tauri/src/dcp/camera_aliases.rs`, with a unit test per alias.

### 5.3 Persistence & schema migration

Extend the per-image adjustments JSON:

```json
{
  "profile": {
    "base": "b3:9f2a…",
    "look": "b3:41c7…",
    "amount": 1.0
  }
}
```

Rules:
- Absent `profile` key → no profile → **current upstream rendering.** All existing sidecars must continue to load and render identically. Add a regression test that asserts this.
- Store the **content hash**, plus `profile_name` + `unique_camera_model` as human-readable recovery hints, so a moved/reinstalled file can be re-resolved.
- If a referenced profile is missing at load: render **without** it, and surface a non-blocking banner — *"This image references the profile 'Cobalt Modular' (Fujifilm X-Pro2), which is not installed."* **Never silently drop the reference from the sidecar** — the user may reinstall.
- **Presets:** capturing a profile into a preset is **opt-in**, via a checkbox in `ConfigurePresetModal.tsx` (default **off**). A preset without a captured profile must leave the profile slot untouched when applied. This preserves the Lightroom behaviour the user described.
- Batch/copy-paste settings: profile travels only if the source and destination images share a camera model, or the destination's camera also has the required base installed. Otherwise skip the profile, apply everything else, and report the skip.

### 5.4 Install locations

Ship a first-class importer; also auto-discover Adobe's directories so existing installs work with zero effort.

**RapidRAW-managed (writable, primary):**
- Desktop: `{app_data_dir}/profiles/` — `dcp/` and `looks/` subdirs.
- Android: `{app_data_dir}/profiles/` via the same code path; import through SAF content URIs (§6.W8).

**Auto-discovered (read-only, best-effort, must degrade silently if absent):**
- macOS: `~/Library/Application Support/Adobe/CameraRaw/Settings/` (recursive) and `.../CameraProfiles/` — plus the `Cobalt/` subfolder the manual names.
- Windows: `%PROGRAMDATA%\Adobe\CameraRaw\Settings\` and `%APPDATA%\Adobe\CameraRaw\Settings\` (recursive).
- Linux: none standard; RapidRAW-managed only.

Discovery is recursive, extension-filtered (`.dcp`, `.xmp`), and must **never** block app start — index on a background task and emit a Tauri event when ready.

---

## 6. WORKSTREAMS

### 6.0 Dependency graph & parallelisation

```
W0 (repo/CI)
 ├─→ W1 (DCP parser) ──┬─→ W2 (CPU render) ─┬─→ W3 (GPU/WGSL) ─→ W4 (pipeline integration) ─┐
 │                     │                    └─→ W9 (validation harness) ──────────────────┤
 │                     └─→ W5 (registry/pairing) ─→ W6 (desktop UI) ─────────────────────┤
 ├─→ W7 (Cobalt Look — RESEARCH SPIKE, runs in parallel from day 1) ─────────────────────┤
 └─→ W8 (Android) ── depends on W4 + W6 ─────────────────────────────────────────────────┤
                                                                                          └─→ W10 (docs/release)
```

**Dispatch immediately in parallel:** W0, W1, W7 (spike).
**After W1:** W2, W5.
**After W2:** W3, W9.
**After W3 + W5:** W4, W6.
**After W4 + W6:** W8.

Every workstream must land on its own branch and merge via PR into `cobalt/main`. Never commit to `main`.

---

### W0 — Fork setup, branching, CI, licensing

**Agent:** infrastructure · **Depends on:** nothing · **Parallel:** yes

**Deliverables**
1. **Set up the working directory. It is NOT empty** — it already contains `COBALT_DCP_SPEC.md` and `tools/`, which are project files and must be preserved and committed. `git clone` refuses a non-empty target, so use init-and-fetch:

   ```bash
   cd /Users/harrisontucker/Documents/CoWork/RapidRaw
   git init -b main
   git remote add origin   https://github.com/harrytuckerr/RapidRAW.git
   git remote add upstream https://github.com/CyberTimon/RapidRAW.git
   git fetch origin
   git checkout -f main            # populates the tree; spec + tools/ remain as untracked
   git fetch upstream              # verify upstream reachable
   git checkout -b cobalt/main
   git add COBALT_DCP_SPEC.md tools/
   git commit -m "docs: add Cobalt DCP pipeline specification and reference tooling"
   ```

   Verify afterwards: `git status` shows a clean tree, `git remote -v` lists both remotes, and `COBALT_DCP_SPEC.md` + `tools/` are tracked on `cobalt/main`. **If `git checkout -f main` would overwrite the spec or `tools/`, stop** — those paths must not exist upstream; investigate rather than forcing.
2. `cobalt/main` (created in step 1) is the long-lived integration branch. **Every** feature branch (`cobalt/w1-dcp-parser`, `cobalt/w2-cpu-render`, …) branches from it and PRs back into it. Keep `main` clean and tracking `upstream/main` so upstream can be merged later without conflict archaeology. Protect `main` against direct pushes if the fork's settings allow.
3. Verify a clean baseline build **before any changes**: `npm install && npm run tauri build` (desktop). Record toolchain versions (Rust ≥ 1.96, `edition = "2024"`, Node) in `docs/BUILD.md`. **If the baseline does not build, stop and report — do not begin feature work on a broken baseline.**
4. Add `docs/` with `dcp-pipeline.md`, `profiles.md`, `BUILD.md` stubs.
5. Create `test-assets/` with a `README` (see §7.1 — **no vendor files committed**).
6. Add `.github/workflows/cobalt-ci.yml`: build + `cargo test` + `cargo clippy -- -D warnings` + `cargo fmt --check` + `npm run lint`, on PRs to `cobalt/main`. Model it on the existing `ci.yml`/`pr-ci.yml`.
7. **Licensing:** confirm `LICENSE` (AGPL-3.0) is unmodified. Add `NOTICE.md` recording that this fork adds a DCP pipeline, that DCP/DNG is an Adobe specification, and that no Adobe or Cobalt-Image code or assets are redistributed.

**Acceptance**
- `git log --oneline upstream/main..cobalt/main` shows only this project's commits.
- Clean desktop build from a fresh clone, documented.
- CI green on an empty PR.

**Out of scope:** any pipeline code.

---

### W1 — DCP binary parser

**Agent:** Rust/systems · **Depends on:** W0 · **Parallel with:** W5, W7

**Deliverables:** `src-tauri/src/dcp/parser.rs`, `model.rs`, `mod.rs`.

**Requirements**
1. Hand-written IFD reader. Validate header: bytes `II`, magic `0x4352`, IFD offset. **Reject** big-endian (`MM`) with a clear error — DCP is little-endian by definition; do not silently attempt to parse it.
2. Parse all 18 tags in §1.3 plus the optional ones in §5.1. Support TIFF types `1 BYTE, 2 ASCII, 3 SHORT, 4 LONG, 5 RATIONAL, 7 UNDEFINED, 8 SSHORT, 9 SLONG, 10 SRATIONAL, 11 FLOAT, 12 DOUBLE`. Honour the ≤ 4-byte inline-value rule.
3. **Robustness — this parses untrusted user files:**
   - No panics. No `unwrap()`/`expect()` on parsed data. Return `Result<_, DcpError>`.
   - Bounds-check every offset+length against file size **before** slicing.
   - Reject `count` values implying > 64 MB for a single tag.
   - Reject files > 256 MB.
   - Guard `RATIONAL` denominator = 0 → `0.0`, no divide-by-zero.
   - **Add a `cargo fuzz` target** (or a property test over mutated bytes) proving no panic on corrupted input. **This is a hard requirement, not a nice-to-have** — profiles are user-supplied files from the internet.
4. Handle `ExtraCameraProfiles` (50933): a DCP may contain multiple sub-profiles as nested IFDs. Parse them into separate `DcpProfile` entries. If unsupported initially, **detect and report** rather than silently reading only the first.
5. Compute `ProfileId` as `blake3` of the full file bytes (`blake3` already a dependency).
6. Memory: use `memmap2` (already a dependency) for files > 1 MB. The supplied DCP is 1.05 MB and `ProfileLookTableData` alone is 972 KB.

**Acceptance**
- Unit test parses `Fujifilm X-Pro2 Cobalt Flat v3.0.dcp` and asserts **every value in the §1.3 table exactly** (matrices to 1e-6, all counts, both illuminants, `embed_policy == EmbedNever`, `look_table_encoding == Srgb`, `hue_sat_map_encoding == Linear`).
- Asserts `hue_sat_map.map_1.data.len() == 2700` and `look_table.data.len() == 81000`.
- Fuzz/property test: ≥ 10,000 mutated inputs, zero panics.
- Round-trip test on ≥ 3 additional DCPs from other vendors (Adobe standard profiles shipped with any DNG, or freely-licensed ones) to prove generality.

**Out of scope:** rendering, XMP, UI.

---

### W2 — CPU reference render implementation

**Agent:** colour-science/Rust · **Depends on:** W1

**Deliverables:** `src-tauri/src/dcp/render.rs`, `interpolate.rs`, and `docs/dcp-pipeline.md`.

**Requirements**
1. Implement §4.2 – §4.5 exactly, as a **scalar, obviously-correct, unoptimised reference.** Clarity over speed — this is the oracle the GPU implementation is validated against.
2. Public API:
   ```rust
   pub struct DcpRenderer { /* precomputed matrices + resampled curve */ }
   impl DcpRenderer {
       pub fn new(profile: &DcpProfile, as_shot_neutral: [f32; 3]) -> Result<Self, DcpError>;
       pub fn render_pixel(&self, camera_rgb: [f32; 3]) -> [f32; 3];  // → working space
       pub fn render_slice(&self, px: &mut [f32]);                    // rayon-parallel
   }
   ```
   All illuminant interpolation and matrix composition happens **once** in `new()`, never per pixel.
3. **Resolve the ⚠️ VERIFY items** and document each with evidence in `docs/dcp-pipeline.md`:
   - stage order (§4.5) — **blocking**;
   - `wb_coeffs` → `AsShotNeutral` reciprocal convention (§4.1);
   - tone-curve hue-preservation behaviour (§4.4.3);
   - tone-curve behaviour above 1.0.
   Method: implement both candidates behind a flag, render the §7.1 targets, compare against the ACR reference renders, keep the winner, delete the loser, record ΔE numbers.
4. Numerical: `f32` throughout to match GPU; use `f64` only inside `new()` for matrix inversion (`nalgebra`). Never `NaN`-propagate — clamp denormals and guard inversions of near-singular matrices with a descriptive error.

**Acceptance**
- Neutral-axis test: a perfectly neutral camera RGB matching `as_shot_neutral` renders to a neutral working-space value, `|R−G|, |G−B| < 1e-4`. **This is the single best smoke test** — if it fails, matrices or the reciprocal convention are wrong.
- Identity test: a synthetic DCP with identity matrices, no tables, linear tone curve → output == input within 1e-6.
- All four ⚠️ VERIFY items resolved and documented with measurements.
- ΔE2000 vs. ACR reference renders (§7.2) — **mean < 2.0, max < 5.0** across the test set.

**Out of scope:** GPU, UI, Looks.

---

### W3 — GPU / WGSL implementation

**Agent:** graphics/WGSL · **Depends on:** W2

**Deliverables:** changes to `src-tauri/src/shaders/shader.wgsl`, `src-tauri/src/gpu_processing.rs`.

**Requirements**
1. Port W2's chain to WGSL as a stage that runs **before** all existing adjustments (§3.1).
2. **Resource layout.** The shader already uses `@group(0) @binding(4) lut_texture: texture_3d<f32>` and `binding(5) lut_sampler` for the creative LUT. **Do not reuse those bindings.** Add new ones:
   - `dcp_huesat_tex: texture_3d<f32>` — `rgba32float`, dims `(hue, sat, val)`, xyz = `(hueShift, satScale, valScale)`. Pre-interpolate the dual maps by `g` **on the CPU at upload time** and upload one texture. Do not upload both and blend in-shader.
   - `dcp_look_tex: texture_3d<f32>` — same layout, dims `[90, 30, 30]` = 81000 texels ≈ 1.3 MB as rgba32f. Acceptable.
   - `dcp_tone_curve_tex: texture_1d<f32>` — `r32float`, 4096 entries.
   - Extend the `GlobalAdjustments` uniform: `has_dcp: u32`, `dcp_amount: f32`, `dcp_look_encoding: u32`, `dcp_huesat_encoding: u32`, `dcp_huesat_dims: vec3<u32>`, `dcp_look_dims: vec3<u32>`, `cam_to_prophoto: mat3x3<f32>`, `prophoto_to_working: mat3x3<f32>`.
   - **Respect the existing struct padding discipline** — the struct already carries explicit `_pad_lut3/4/5` fields. WGSL `mat3x3` is 48 bytes with 16-byte column alignment; `bytemuck` layout on the Rust side must match exactly. Add a `static_assert`-style size test.
3. **Hue wrapping cannot use hardware sampling.** `texture_3d` sampling clamps or repeats on *all* axes uniformly; we need wrap on hue, clamp on sat/val. Implement **manual trilinear fetch** with `textureLoad` and explicit index arithmetic. Do not attempt to solve this with sampler address modes.
4. `has_dcp == 0u` → early-out before any DCP work; zero cost and bit-identical to current output.
5. Bind dummy 1×1×1 textures when no profile is active — WGSL requires all bindings present.

**Acceptance**
- **GPU-vs-CPU parity:** render a 512×512 image of pseudo-random camera-RGB values through both W2 and W3; **max per-channel abs diff < 1e-3**, mean < 1e-4. Automated test.
- `has_dcp == 0` output is **bit-identical** to upstream (hash comparison against a pre-change render).
- Frame time increase with a profile active < 2 ms at 24 MP on a mid-range GPU. Measure and record; use `bench/`.
- Runs on Vulkan, Metal, DX12, **and GLES 3.0 / Android** (§6.W8). Note GLES lacks some 3-D texture features — verify `rgba32float` 3-D sampling support and fall back to `rgba16float` if needed, documenting the precision impact.

---

### W4 — RAW pipeline integration

**Agent:** Rust/pipeline · **Depends on:** W3, W5

**Deliverables:** `raw_processing.rs`, `image_processing.rs`, `export_processing.rs`, `dcp/commands.rs`.

**Requirements**
1. In `develop_internal` (`raw_processing.rs` ~line 106), when a DCP profile is active, additionally strip `ProcessingStep::Calibrate`. **The existing conditional already has the shape for this** (`apply_calibration || step != ProcessingStep::Calibrate`) — extend, don't rewrite.
2. Surface `wb_coeffs`, `color_matrix`, `clean_model`, `make`, `model` from `RawImage` up to the profile stage. Currently `raw_image` is dropped early (`drop(raw_image)`); capture what's needed **before** that.
3. Thread `ProfileSelection` through `RenderRequest` → `get_all_adjustments_from_json` → GPU uniforms.
4. **Non-RAW files (JPEG/TIFF/PNG):** there is no camera-native space, so DCP is meaningless. The profile slot must be **hidden/disabled** for these, not silently ignored. Decide and document behaviour for DNG (linear DNG has `RawPhotometricInterpretation::LinearRaw` — the existing code already branches on this; DCP *is* applicable to linear DNG but the WB/calibration state differs — handle explicitly).
5. Ensure export (`export_processing.rs`), thumbnails, and the headless CLI all route through the same profile-aware path. **A profile visible in the editor but missing from the export is the most likely integration bug** — add an explicit test.
6. **`ProfileEmbedPolicy = EmbedNever` must be honoured** in any DNG/TIFF export path that would otherwise embed profile data (§8.2).
7. Caching: profile identity must participate in `calculate_transform_hash` (`cache_utils.rs`) or stale thumbnails will render with the wrong profile.

**Acceptance**
- Editor preview, exported JPEG, thumbnail, and CLI export of the same image with the same profile agree within ΔE mean < 1.0.
- No-profile path bit-identical to upstream (regression test).
- Cache invalidates on profile/amount change.

---

### W5 — Profile registry & pairing

**Agent:** Rust/application · **Depends on:** W1 · **Parallel with:** W2

**Deliverables:** `src-tauri/src/dcp/registry.rs`, `camera_aliases.rs`, `commands.rs`.

**Requirements**
1. Index all discovered `.dcp` and `.xmp` files (§5.4) into an in-memory registry keyed by `(unique_camera_model_normalised, profile_name)`. Persist the index to `{app_data_dir}/profiles/index.json` with file mtime+size for fast invalidation.
2. Implement §2.2 resolution and §5.2 matching **exactly**. Expose:
   ```rust
   pub enum PairingState {
       Satisfied { base: ProfileId },
       MissingBase { required_name: String, camera: String, installed_for_camera: Vec<String> },
       NoProfilesForCamera { camera: String },
   }
   pub fn resolve_look(&self, look: &CobaltLook, camera: &str) -> PairingState;
   ```
   `installed_for_camera` drives the UI's "here's what you *do* have" message.
3. Tauri commands: `list_profiles_for_image(path) -> ProfileBrowserModel`, `import_profiles(paths) -> ImportResult`, `remove_profile(id)`, `rescan_profiles()`. Model the import/remove ergonomics on the existing `import_luts`/`remove_lut` in `lut_processing.rs` for consistency.
4. Import validation: parse before accepting; reject unparseable files with a specific reason. **Never copy an invalid file into the managed directory.**
5. Duplicate handling: same `ProfileId` (content hash) from two paths → one registry entry. Same `(camera, name)` with *different* content → keep both, disambiguate in the UI by source directory.
6. Background rescan with a Tauri event on completion; never block startup.

**Acceptance**
- Unit tests for every §5.2 matching rule and every alias.
- Test asserting the §2.3 mismatch produces `MissingBase { required_name: "Cobalt Modular", camera: "Fujifilm X-Pro2", installed_for_camera: ["Cobalt Flat"] }`.
- **Negative test:** a Look is *never* returned as applicable when its base is missing.
- 500-profile registry indexes in < 2 s and lookups are O(1).

---

### W6 — Desktop UI: the profile slot

**Agent:** React/TypeScript · **Depends on:** W5

**Deliverables:** `src/components/panel/right/ProfilePanel.tsx`, `src/components/ui/ProfileBrowser.tsx`, `src/hooks/useProfiles.ts`; edits to `ControlsPanel.tsx`, `adjustments.ts`, `ConfigurePresetModal.tsx`.

**Requirements**
1. Profile selector at the **top of the adjustments column, above Basic** — matching Lightroom/ACR placement. Study `src/components/panel/right/ControlsPanel.tsx` and `src/components/adjustments/Basic.tsx` and **match the existing component idiom, styling and state patterns exactly.** Do not introduce a new styling approach or state library.
2. Browser UI per §2.5: grouped by `crs:Group` for Looks and by "camera profiles for this body" for DCPs; searchable (reuse the existing search idiom); shows the active selection.
3. **Unsatisfiable Looks are shown but disabled**, with the §2.3 message on hover/click. **Do not hide them** — the user needs to learn that they own a Look but lack its base pack. This is a key UX requirement, not a detail.
4. **Amount slider**, shown only when a Look is active and `supports_amount`. Range 0–200, default 100, snap-to-100 on double-click. Reuse the existing slider component.
5. Import button → native file dialog (`tauri-plugin-dialog`, already a dependency) filtered to `.dcp`/`.xmp`; multi-select; progress + per-file success/failure summary.
6. Reset control returns the slot to "no profile" (upstream rendering).
7. **Preset interaction:** add the opt-in "Include profile" checkbox to `ConfigurePresetModal.tsx`, default **off** (§5.3).
8. i18n: the repo has `i18next.config.ts` — **add all new strings to the locale files**, no hard-coded English.
9. Accessibility: keyboard-navigable list, focus states, disabled items announced with their reason.

**Acceptance**
- Selecting a base DCP, then a Look, then adjusting Amount updates the preview live with no stutter.
- Unsatisfiable Look shows the exact §2.3 message naming the required base and the camera.
- Applying a preset does **not** change the profile slot unless the preset captured one.
- No new console errors/warnings; lint clean; all strings localised.

---

### W7 — Cobalt Look XMP support ⚠️ RESEARCH SPIKE WITH DECISION GATE

**Agent:** reverse-engineering / format research · **Depends on:** W0 · **Start immediately, in parallel**

> This is the highest-uncertainty workstream. It is deliberately isolated so that **W1–W6 deliver a complete, useful, shippable DCP pipeline even if W7 fails entirely.**

**Phase 1 — Metadata parsing (low risk, do this first, unconditionally)**

Parse everything in §1.1 *except* the table blob: `Name`, `Group`, `Cluster`, `UUID`, `CameraProfile`, `CameraModelRestriction`, `SupportsAmount`, `RGBTable`, `ProcessVersion`, `Copyright`. Use `quick-xml` (already a dependency). This alone enables the entire browser UI, grouping, and pairing logic (W5/W6).

*Acceptance:* all 17 supplied Looks parse; asserts `required_base_profile == "Cobalt Modular"` and `camera_model_restriction == None` for all 17; **and asserts `short_name == ""` and `sort_name == ""` for all 17** (this catches the §1.1 self-closing-`rdf:li` regex trap — a naive parser returns `"Cobalt CCD fever v3.0"` here and passes every other check).

**Phase 2 — Table acquisition. Evaluate these three routes in order.**

**Route A — Decode the embedded blob** *(preferred if it works; time-boxed)*

Known: 85-char XML-safe alphabet (§1.2); constant 5-char magic `Lir00`; naïve Ascii85 uint32 grouping **disproven**. Investigate: `Lir00` as a header (version/length/dims); non-uint32 bit-packing (`log2(85) ≈ 6.409` bits/char); a permuted alphabet; whether the decoded payload is zlib/deflate (look for `0x78` headers), or half-float triplets.
Sanity check for a correct decode: the result should be an HSV-triplet table whose **leading entries are near-identity** (`hueShift ≈ 0, satScale ≈ 1, valScale ≈ 1`), exactly as the DCP's own `ProfileLookTableData` begins `[0.0, 1.0, 1.0, …]`. Table sizes should be plausible (e.g. 36×8×16, 90×30×30).
**Time-box: 2 agent-days.** If not decoded, stop and go to Route B. **Do not let this run indefinitely.**

**Route B — Profile Capture via round-trip** ⭐ *recommended fallback; fully specified; no reverse-engineering*

RapidRAW **already contains both halves of this**, verified in `src-tauri/src/lut_processing.rs`:
- `generate_identity_lut_image(size: u32) -> DynamicImage` (line ~430)
- `convert_image_to_cube_lut(image: &DynamicImage, size: u32) -> Result<Vec<u8>, String>` (line ~450)

Workflow to build:
1. RapidRAW exports an identity HALD image (16-bit TIFF, size 33 or 64) — "Capture a profile…".
2. User opens it in Lightroom/ACR (which they already own — the profiles are installed there), sets **Profile = the Cobalt Look at Amount 100**, all other settings zeroed, and exports 16-bit TIFF.
3. User imports the result; RapidRAW runs `convert_image_to_cube_lut` and stores it as the Look's table, linked to the Look's `UUID` and `RGBTable` id.

This is a **legitimate, robust, vendor-neutral** path: it captures the user's own licensed rendering from their own licensed software, needs no undocumented format, and works for *any* ACR profile, not just Cobalt. It also degrades gracefully — it is plain LUT capture.
*Caveat to document honestly:* a 3-D RGB LUT cannot capture behaviour that depends on scene-referred data outside [0,1], so highlight roll-off above clipping may differ slightly from ACR. Mitigate by capturing in a wide, log-ish encoding and documenting the limitation.

**Route C — Ship DCP-only.** Looks appear in the browser with a "table unavailable" state. Base DCP profiles still work fully. This is the floor, and it is still a genuinely useful product.

**🚦 DECISION GATE G1 — human sign-off required.** At the end of the Route A time-box, report: what was learned, whether A succeeded, and the recommendation. **Do not proceed past this gate autonomously.**

**Hard constraints for this workstream**
- Operate **only** on files the user already owns and has licensed. This is interoperability for the user's own data.
- **Never** commit any Cobalt table data, decoded or encoded, to the repository. Not in tests, not in fixtures, not in comments.
- **Never** implement anything that bypasses a licence check, generates profiles the user has not bought, or lets one user's profiles be redistributed to another.
- If Route A succeeds, ship a **reader only**. No writer, no re-encoder, no export of Cobalt tables in any form.

---

### W8 — Android

**Agent:** Android/Rust cross-platform · **Depends on:** W4, W6

> **Correction for the orchestrator:** RapidRAW's Android app is **not a separate repository.** It is the same codebase — verified: `src-tauri/gen/android/` (Gradle/Kotlin), `src-tauri/src/android_integration.rs`, `src/hooks/useAndroidBackHandler.ts`, and `[target.'cfg(target_os = "android")'.dependencies]` in `Cargo.toml`. **There is nothing to fork separately.** Building the Android target from this same fork gives you the Android app with DCP support.

**Requirements**
1. **File import via SAF.** Android has no general filesystem access. `lut_processing.rs` already solves this exact problem — study `is_android_content_uri`, `read_android_content_uri`, `resolve_android_content_uri_name`, `get_android_cached_lut_path`, and `import_android_lut`, and **mirror that pattern** for `.dcp`/`.xmp`. Do not invent a new mechanism.
2. Register `.dcp` and `.xmp` MIME/extension intent filters in `src-tauri/gen/android/app/src/main/AndroidManifest.xml` so profiles can be opened/shared into the app.
3. **Auto-discovery (§5.4) does not apply** — there is no Adobe directory. Managed import only. Ensure discovery code is `#[cfg]`-gated off, not merely failing at runtime.
4. **GPU constraints.** Verify 3-D `rgba32float` sampling on GLES 3.0 / Vulkan on real devices. If unsupported, fall back to `rgba16float` and document the ΔE impact. **Test on at least one low-end and one recent device.**
5. **Memory.** A parsed DCP holds ~1.3 MB of tables; Android processes are memory-constrained and RapidRAW already does AI/masking work. Lazily load table data; keep only the active profile's tables resident; drop on background.
6. **UI.** The profile browser must work on a phone-width viewport. Check `PanelSwitcher.tsx` / `SidePanelArea.tsx` for the existing mobile layout idiom and follow it. Touch targets ≥ 44 dp.
7. Optionally bundle nothing by default — **do not ship Cobalt files in the APK** (§8.1). `include_dir` is used for other assets; do not extend it to profiles.

**Acceptance**
- APK builds from `cobalt/main` via the existing Gradle setup; document exact commands in `docs/BUILD.md`.
- Import a `.dcp` and a `.xmp` via the system file picker on a physical device.
- Same image + same profile renders within ΔE mean < 1.0 of the desktop build.
- No OOM in a 200-image session with a profile active.

---

### W9 — Validation harness

**Agent:** testing/tooling · **Depends on:** W2 · **Parallel with:** W3

**Deliverables:** `bench/dcp_validation/` (the repo already has `bench/`), plus CI wiring.

**Requirements**
1. A CLI harness that renders a set of RAW files through the RapidRAW DCP path and compares against reference renders, reporting **ΔE2000 mean / p95 / max** per image and a pass/fail against §7.3 thresholds.
2. Synthetic tests that need no vendor assets: identity profile, neutral-axis, known-matrix round-trips, HSV table wrap/clamp behaviour at hue index 89→0 and at sat/val edges.
3. Golden-image tests for the **no-profile** path to protect the §3.1 bit-identical invariant.
4. GPU-vs-CPU parity test (drives W3's acceptance).
5. Wire the synthetic subset into CI. **The vendor-asset subset must be skipped-by-default and never break CI for contributors who lack the files** (§7.1).

---

### W10 — Documentation & release

**Agent:** technical writing · **Depends on:** all

**Deliverables**
- `docs/profiles.md` — user guide: what a camera profile is, the base-DCP + Look pairing (§2), how to install, why a Look may be greyed out, the Amount slider, the Profile-Capture workflow if W7-B ships.
- `docs/dcp-pipeline.md` — developer reference: the render chain, the resolved ⚠️ VERIFY decisions **with their evidence**, GPU resource layout, known limitations.
- `docs/BUILD.md` — desktop + Android build instructions with verified commands.
- README section describing the fork's additions and linking upstream.
- `NOTICE.md` per W0.7.
- Release notes stating plainly what is and is not supported — including, honestly, whether Cobalt Look tables are supported natively (Route A), via capture (Route B), or not at all (Route C).

---

## 7. TESTING & VALIDATION

### 7.1 Test assets — licensing-safe handling

**Never commit vendor profiles, decoded tables, or copyrighted RAW files to the repository.** The supplied DCP carries `ProfileCopyright = "(c)Cobalt-Image 2024"` and `ProfileEmbedPolicy = EmbedNever`.

`test-assets/README.md` documents the expected local layout; the directory is `.gitignore`d apart from that README:

```
test-assets/                       # gitignored
  dcp/Fujifilm X-Pro2 Cobalt Flat v3.0.dcp
  looks/Cobalt *.xmp               (17 files)
  raw/xpro2_*.RAF                  (user-supplied test frames)
  reference/xpro2_*_acr.tif        (ACR reference renders)
```

Tests requiring these must be `#[ignore]`d by default and enabled with `RAPIDRAW_TEST_ASSETS=1`. **CI must stay green without them.** Synthetic tests (W9.2) carry the CI signal.

### 7.2 Producing ACR reference renders (the oracle)

Because the goal is *matching Adobe*, references must come from Adobe. Document this procedure precisely in `docs/dcp-pipeline.md`:

1. In ACR/Lightroom, open the test RAW.
2. Set profile to the Cobalt DCP under test. **Zero every other control** — exposure 0, contrast 0, all tone/presence/colour sliders 0, no lens corrections, no sharpening, no noise reduction, no vignetting. Set the tone curve to Linear.
3. Set white balance to **As Shot** and record the resulting temperature/tint.
4. Export **16-bit TIFF, ProPhoto RGB**, full resolution, no output sharpening.
5. Record ACR version + Process Version (the Looks declare `ProcessVersion = 15.4`, `Version = 16.1.1 / 16.2`).

Compare in ProPhoto to avoid gamut clipping in the comparison itself. RapidRAW must be configured to match: no adjustments, no LUT, profile only.

### 7.3 Thresholds

| Comparison | Mean ΔE2000 | p95 | Max |
|---|---|---|---|
| CPU render vs. ACR reference | < 2.0 | < 3.0 | < 5.0 |
| GPU vs. CPU (parity) | < 0.1 | — | < 0.5 |
| Android vs. desktop | < 1.0 | — | < 2.0 |
| Editor vs. export vs. thumbnail | < 1.0 | — | < 2.0 |
| No-profile vs. upstream | **bit-identical** | | |

Deep shadows (Y < 0.001) and clipped highlights may be excluded from ΔE statistics, but **must be reported separately** rather than silently dropped.

### 7.4 Mandatory test matrix

| Dimension | Cases |
|---|---|
| Illuminant | tungsten (~2856 K), daylight (~6504 K), and an intermediate (~4000 K) forcing genuine interpolation |
| Content | ColorChecker, saturated primaries, skin tones, neutral ramp, clipped specular highlights, deep shadows |
| Sensor | Bayer **and** X-Trans (the supplied DCP is X-Pro2 — X-Trans; do not test Bayer only) |
| Profile shape | dual-illuminant + FM (supplied), single-illuminant, no-FM, no-tables, tables-but-no-curve |
| Pairing | satisfied · missing base · no profiles for camera · profile deleted after being referenced |
| Platform | Windows, macOS, Linux, Android |

---

## 8. LICENSING, LEGAL & ETHICS

### 8.1 Distribution

- The fork **remains AGPL-3.0**. AGPL obligations apply to any network-served deployment.
- **Do not redistribute Cobalt-Image profiles** in the repository, in installers, or in the APK. They are commercial licensed products. Users install their own purchased files.
- **Do not commit ACR reference renders** derived from copyrighted RAW files unless the user owns and licenses them for that use.

### 8.2 `ProfileEmbedPolicy`

The supplied DCP sets `EmbedNever (2)`. Honour it: **no export path may embed this profile's data into an output file.** This is a licence term expressed in the file format, and it is straightforward to comply with — implement the check in W4 and test it.

### 8.3 Reverse engineering (W7 Route A)

Scoped as **interoperability for files the user has lawfully purchased** — the same basis on which open-source RAW developers read DCP, and it stays within that boundary only if the constraints hold: read-only, no writer, no redistribution of tables, no licence-check circumvention, nothing that lets a non-purchaser obtain a profile. If Route A ever drifts toward enabling redistribution, **stop and escalate**. Route B (Profile Capture) avoids the question entirely and is preferred for that reason as well as its robustness.

### 8.4 Attribution

`NOTICE.md` must state: this fork adds an Adobe DNG Camera Profile pipeline; DNG/DCP is an Adobe specification; Cobalt Image® is a trademark of its owner; no Adobe or Cobalt-Image code or assets are redistributed; upstream RapidRAW is © Timon Käch, AGPL-3.0.

---

## 9. RISK REGISTER & DECISION GATES

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Cobalt look-table blob is not decodable | **High** | Medium | W7 is isolated; Routes B and C both ship a useful product. Gate **G1**. |
| R2 | Stage order (§4.5) implemented wrong | **High** | **High** | Empirical resolution is a blocking acceptance criterion for W2; harness is the arbiter. |
| R3 | `wb_coeffs` reciprocal convention inverted | Medium | High | Neutral-axis test catches it immediately (W2 acceptance). |
| R4 | Uniform-struct padding mismatch (Rust ↔ WGSL) | Medium | High | Size/offset assertion test; existing `_pad_lut*` fields show the established convention. |
| R5 | GLES/Android lacks `rgba32float` 3-D sampling | Medium | Medium | `rgba16float` fallback, documented precision cost (W8.4). |
| R6 | Profile applied in preview but not export | Medium | High | Explicit cross-path ΔE test (W4 acceptance). |
| R7 | Regression in the no-profile path | Low | **Critical** | Bit-identical golden test in CI (W9.3). |
| R8 | Fuzzy camera matching produces wrong colour silently | Medium | High | §5.2 forbids fuzzy matching; negative tests required. |
| R9 | Silent substitution of a mismatched base DCP | Medium | **High** | §2.3 forbids it; explicit negative test in W5. |
| R10 | `wgpu` bumped, reintroducing Apple P3 colour shift | Low | High | Pin documented in §3.3; CI check on `Cargo.toml` diff. |

### Decision gates (human sign-off required; do not pass autonomously)

- **G0 — after W0:** baseline builds clean on desktop; Android toolchain confirmed. *If the baseline is broken, stop.*
- **G1 — after W7 Phase 2 Route A time-box:** choose Route A / B / C.
- **G2 — after W2:** the four ⚠️ VERIFY items are resolved with evidence, and ΔE vs. ACR is within §7.3. *Do not build GPU code on an unvalidated CPU reference.*
- **G3 — before Android work (W8):** desktop path is complete and validated.
- **G4 — before release:** full §7.4 matrix passed; licensing review (§8) signed off.

---

## 10. SUB-AGENT PROMPT TEMPLATE

Use this shape for every dispatch. Fill the braces; embed the named sections **verbatim** — do not paraphrase them.

```
You are implementing workstream {Wn} of the RapidRAW Cobalt DCP project.

REPOSITORY: /Users/harrisontucker/Documents/CoWork/RapidRaw
BRANCH: create `cobalt/{wn}-{slug}` off `cobalt/main`. Never commit to `main`.

CONTEXT YOU MUST READ FIRST (verbatim, do not paraphrase):
{paste §0.1, §0.2, §0.3, §0.5, §1, §2, and §3.1 of COBALT_DCP_SPEC.md}

YOUR WORKSTREAM (verbatim):
{paste §6.Wn}

{if the workstream touches colour: paste §4 in full}
{if it touches persistence or matching: paste §5 in full}

GROUND RULES:
- Facts in §1 are verified. Items marked ⚠️ UNVERIFIED are NOT established —
  prove them before writing code that depends on them.
- Match the surrounding code's idiom, naming, error handling and comment density.
  Read neighbouring files before writing new ones.
- Do NOT add crates without justifying why nothing in Cargo.toml suffices (§3.3).
- Do NOT bump wgpu from 29.0 (§3.3).
- Do NOT commit vendor profiles or decoded tables (§8.1).
- The no-profile path must stay bit-identical to upstream (§3.1).
- If a §9 decision gate is reached, STOP and report. Do not decide for the human.
- If the spec is genuinely ambiguous or turns out to be wrong, STOP and report
  it. Do not paper over it with an approximation.

DELIVERABLES: {exact file paths from §6.Wn}
ACCEPTANCE: every criterion in §6.Wn's Acceptance block, demonstrated by a
  passing test. Report honestly: if a criterion is unmet, say so with the output.

Open a PR into `cobalt/main` describing what you did, what you verified, and
what remains uncertain.
```

---

## 11. QUICK REFERENCE

**What the user has**
- 17 Cobalt Look XMPs ("CCD fever v3.0"), all requiring base profile `"Cobalt Modular"`.
- 1 Cobalt DCP: `Cobalt Flat` for `Fujifilm X-Pro2`.
- **These do not currently pair** (§2.3) — the `Cobalt Modular` base pack for their camera is needed. This is a *product/purchase* gap, not a bug to code around.

**What is being built**
1. A **profile slot** in RapidRAW — the missing Lightroom-equivalent concept — holding `(base DCP, optional Look, amount)`, sitting *before* all existing adjustments, with presets/masks/LUTs stacking on top unchanged.
2. A full **Adobe DCP renderer** (dual-illuminant, forward matrices, HueSatMap, tone curve, look table) on CPU + GPU.
3. A **registry** enforcing correct per-camera pairing, failing loudly rather than silently mis-rendering.
4. **Cobalt Look** support — natively if the blob is decodable, otherwise via Profile Capture round-trip.
5. The same on **Android**, from the same repository.

**Three things most likely to be got wrong**
1. Assuming Cobalt profiles are `.dcp` files. **17 of the 18 supplied files are XMP Looks** (§1.1).
2. Getting the §4.5 stage order wrong, or missing that HueSatMap and LookTable use **different encodings** (§4.4).
3. Silently substituting a base profile when the pair is unsatisfiable, instead of disabling the Look (§2.3).

**Non-negotiables**
- No profile selected → **bit-identical** to upstream.
- No fuzzy camera matching.
- No silent base-profile substitution.
- No vendor assets in the repo.
- `wgpu` stays at 29.0.

---

*End of specification. Verified against the supplied Cobalt assets and upstream `CyberTimon/RapidRAW@main` on 2026-08-20.*
