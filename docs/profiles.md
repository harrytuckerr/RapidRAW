# Camera profiles in RapidRAW (user guide)

Camera profiles are the foundation of how a RAW file looks — they sit underneath every other adjustment and define the base rendering. This guide covers the profile feature added by the Cobalt DCP project.

## What a camera profile is

A camera profile (Adobe DCP — DNG Camera Profile) is a mathematical recipe that tells RapidRAW how to turn a flat, greyish RAW sensor capture into a colour image. It handles the translation from camera sensor RGB to a standard working colour space. Think of it as the "film stock" for your digital camera.

RapidRAW now gives you a first-class **Profile slot** — the same concept Lightroom and Adobe Camera Raw expose as `Adobe Color`, `Adobe Landscape`, `Camera Standard`, or a Cobalt profile. This slot lives at the very start of the editing pipeline. Your presets, masks, brightness, contrast, colour grading and creative LUTs all stack on top, unchanged.

If you select no profile at all, RapidRAW renders exactly as it did before this feature — output is bit-identical to the upstream version. The profile slot is strictly additive and off by default.

A profile is **not** a preset and **not** a creative LUT. A preset is a bundle of adjustments. A LUT is a late-stage creative colour grade. A profile is a fundamental colour transform that defines how your camera's RAW data becomes colour — it must be applied first.

## How profiles work: the base-DCP + Look pairing

Some profiles stand alone: you select a base DCP like `Cobalt Flat` and it renders your camera's RAW data to colour. That's it.

Cobalt's Looks add a second layer. A Cobalt Look XMP (such as `Pentax 645D Portrait`, part of the "CCD Fever" collection) is a creative colour grade that sits **on top of** a specific base DCP. The Look declares what base it needs via its `CameraProfile` field (for example `"Cobalt Modular"`).

When you select a Look:

1. RapidRAW checks whether you have the required base DCP installed **for your camera body**. A base DCP is per-camera — `Cobalt Modular` for a Fujifilm X-Pro2 is a different file from `Cobalt Modular` for a Pentax 645D.
2. If the matching base DCP is installed, the Look is **satisfied** — RapidRAW renders the base DCP first, then the Look on top. Together they occupy the Profile slot.
3. If the base DCP is **missing**, the Look is **disabled** (greyed out) with an explanatory message telling you exactly which base profile is needed for which camera body.

This two-key lookup — `(camera model, profile name)` — is enforced strictly. RapidRAW will never silently substitute a different base DCP. Rendering a Cobalt Look over the wrong base produces confidently wrong colour with no visible error, so the correct behaviour is to refuse.

## Installing profiles

### Desktop

RapidRAW auto-discovers Adobe's standard Camera Raw directories on macOS and Windows, so if you already have Cobalt or other profiles installed for Lightroom/ACR, they appear automatically:

- **macOS:** `~/Library/Application Support/Adobe/CameraRaw/Settings/` (recursive)
- **Windows:** `%PROGRAMDATA%\Adobe\CameraRaw\Settings\` and `%APPDATA%\Adobe\CameraRaw\Settings\` (recursive)
- **Linux:** no standard Adobe directory; use the importer.

To install new profiles, use the **Import** button in the profile browser. This opens a native file picker filtered to `.dcp` (base profiles) and `.xmp` (Looks). Files are copied into RapidRAW's managed profile directory (`{app_data}/profiles/` with `dcp/` and `looks/` subdirectories). You can import multiple files at once.

Discovery runs on a background task and never blocks app startup. A rescan button forces a fresh index if you add files manually to the managed directory.

### Android

Profiles must be imported through the system file picker (SAF — Storage Access Framework). Android has no general filesystem access, so auto-discovery of Adobe directories does not apply. The import flow mirrors how LUTs are imported: pick a `.dcp` or `.xmp` from any content source, and RapidRAW copies it into the app's managed profile directory.

**Android is in progress** (W8 not yet merged). The SAF import path is partially wired but not yet verified on physical devices. Built-in GLES GPU fallback for 3-D texture sampling is not yet tested.

## Why a Look may be greyed out

A Look is disabled when:

- **Missing base profile.** The Look requires a specific base DCP (for example `"Cobalt Modular"`) for your camera body, and that base DCP is not installed. The tooltip tells you: *"'Pentax 645D Portrait' requires the base profile 'Cobalt Modular' for Fujifilm X-Pro2, which is not installed."* — plus a list of base profiles that *are* installed for that body. This is the most common case, and it's not a bug — it means you need to purchase and install the Cobalt base pack for your camera.
- **Wrong camera.** The Look's `CameraModelRestriction` field (if non-empty) restricts it to a specific body. None of the supplied Looks set this restriction, so this is uncommon.
- **No profiles at all for this camera.** You have not installed any base DCPs matching the opened RAW file's camera model.

Disabled Looks are **shown** in the browser, not hidden. Hiding them would make you wonder where they went. Showing them greyed out teaches you what you own and what you need.

## The Amount slider

When a Look is active, an **Amount** slider appears (range 0–200, default 100, snap-to-100 on double-click). This scales the Look layer only — the base DCP is always applied at full strength.

- **0:** the Look is bypassed entirely (identity). You see the base DCP rendering.
- **100:** the Look is applied at full strength.
- **Above 100:** the Look's effect is exaggerated (useful for dramatic grades).

Amount is implemented as an exponential scaling of the table's saturation and value multipliers (`satScale^k`, `valScale^k` where `k = amount/100`) with a linear hue shift, matching how ACR behaves beyond 100 %. A naïve pixel lerp would desaturate, so don't expect sliders-at-50 % to mean "half as strong" in a simple arithmetic sense.

Only Looks that declare `SupportsAmount="True"` show the slider. All 17 supplied Cobalt Looks support it.

## Profile Capture workflow (Route B)

If a Look's embedded table cannot be decoded natively — for example, a future Cobalt Look version with an unsupported table format — you can capture it via a round-trip through Lightroom or Adobe Camera Raw. RapidRAW already has the machinery for this.

**The workflow:**

1. **Export the identity HALD.** In RapidRAW, select the Look in the profile browser and choose "Capture profile…". RapidRAW writes a 16-bit TIFF identity HALD image. The default grid size is 33 (fast, ~36k pixels). Size 64 gives finer interpolation (~262k pixels, larger file).
2. **Process in ACR/Lightroom.** Open the exported TIFF in Lightroom or ACR. Set **Profile = the Cobalt Look you want to capture** at Amount 100. Zero every other control — exposure 0, contrast 0, tone curve linear, no sharpening, no noise reduction, no lens corrections, no vignetting. Export 16-bit TIFF, ProPhoto RGB, full resolution, no output sharpening.
3. **Import the processed HALD.** Back in RapidRAW, select the same Look and choose "Import captured LUT…". Point to the TIFF from step 2. RapidRAW converts it to a `.cube` 3-D LUT, saves it linked to the Look's UUID, and upgrades the Look from "table unavailable" to "captured".
4. **The capture persists.** The `.cube` file lives in `{app_data}/profiles/looks/captured/<uuid>.cube` and is picked up automatically on next launch. You only need to capture once per Look.

**Caveat — highlight roll-off.** A 3-D RGB LUT with domain [0,1]³ cannot capture behaviour that depends on scene-referred data outside [0,1]. ACR's pipeline can apply the Look to values above 1.0 (scene-linear highlights) and then roll them off. The captured LUT only sees post-clip [0,1] inputs, so highlight roll-off above clipping may differ slightly from ACR — particularly in specular highlights and bright skies. Capture at size 64 to reduce interpolation error near the top end; use a log encoding pre-process on the HALD if highlight fidelity is critical.

**Route A (native decode) is preferred** and works for all 17 supplied Looks. Route B is the fallback. See `docs/cobalt-look.md` for the technical details of the native decoder.

## The no-profile default

With no profile selected in the slot, RapidRAW's output is **bit-identical** to upstream RapidRAW. This is a hard invariant, regression-tested. Every existing sidecar file loads and renders identically. The profile slot does nothing unless you actively install profiles and choose one.

## Presets and profiles

Applying a preset does **not** change your selected profile unless the preset was explicitly saved with one (an opt-in checkbox, default **off**). This matches Lightroom behaviour: presets and profiles are independent layers.

When copying/pasting settings between images, the profile travels only if the destination image's camera also has the required base profile installed. If not, the profile is skipped, everything else is applied, and RapidRAW reports the skip.

## Known limitations

- **ACR delta-E validation is deferred.** The render pipeline is dng_sdk-faithful (stage order, illuminant interpolation, matrix paths) but has not been measured against Adobe Camera Raw reference renders. The CIEDE2000 validation harness is built and ready; reference TIFFs need to be produced on a machine with ACR/Lightroom installed. See `docs/dcp-pipeline.md` for the four resolved VERIFY items and their evidence.
- **Android is in progress.** Desktop profiles work. Android SAF import and GPU fallback are not yet fully verified.
- **Cobalt Look native decode** (Route A) works for all 17 supplied "CCD Fever v3.0" Looks. The decoder handles the `dng_big_table` / `dng_rgb_table` format. Future Look versions with different table formats may require Route B capture.

## Normative source

`COBALT_DCP_SPEC.md` section 2 (the pairing model) and section 2.5 (the profile browser) define the contract this document elaborates.
