# Camera profiles in RapidRAW (user guide)

> Status: stub. Filled by workstreams W6 (desktop UI) and W10 (documentation).

This document is the user guide for the camera-profile feature added by the Cobalt DCP project.

## Scope

To be filled by W6 / W10. It will cover:

- What a camera profile is and how it differs from a preset or a creative LUT. A profile sits underneath every other adjustment and defines the base rendering of a RAW file.
- The base-DCP + Look pairing model: why a Cobalt Look needs its matching base DCP installed for your camera body, and what `Cobalt Modular` vs `Cobalt Flat` means.
- How to install profiles: file picker on desktop, system file picker (SAF) on Android. Where RapidRAW auto-discovers Adobe's CameraRaw directories on desktop.
- Why a Look may be greyed out (its required base profile is not installed for your camera body) and what to do about it.
- The Amount slider (0 to 200, default 100), which scales only the Look layer.
- The Profile Capture workflow (if W7 Route B ships): export an identity HALD from RapidRAW, apply the Cobalt Look in Lightroom/ACR at Amount 100 with all other controls zeroed, re-import the result to capture the Look as a LUT.
- The no-profile default: with nothing selected, output is identical to upstream RapidRAW.

## Normative source

See `COBALT_DCP_SPEC.md` section 2 (the pairing model) and section 2.5 (what a user sees in the profile browser) for the normative contract this document elaborates.
