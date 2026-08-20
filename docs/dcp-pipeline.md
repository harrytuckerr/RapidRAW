# DCP render pipeline (developer reference)

> Status: stub. Filled by workstreams W2 (CPU reference render) and W10 (documentation).

This document is the developer reference for the Adobe DNG Camera Profile (DCP) render chain added to RapidRAW by the Cobalt DCP project.

## Scope

To be filled by W2 / W10. It will cover:

- The render chain: illuminant interpolation, ForwardMatrix to XYZ(D50), ProPhoto linear working space, HueSatMap (encoding 0 / Linear), LookTable (encoding 1 / sRGB), ProfileToneCurve, the Cobalt Look table with Amount, and the return to RapidRAW's working space.
- The resolved `VERIFY` decisions from the spec, each with its evidence and measured `dE2000` numbers:
  - stage order (HueSatMap, BaselineExposureOffset, LookTable, ProfileToneCurve);
  - `wb_coeffs` to `AsShotNeutral` reciprocal convention;
  - tone-curve hue/saturation preservation behaviour;
  - tone-curve behaviour above 1.0.
- The GPU (WGSL) resource layout and the `has_dcp` zero-cost fast path.
- Known limitations, e.g. highlight roll-off above clipping for Route B captured LUTs.
- The ACR reference-render procedure used as the validation oracle (also in `test-assets/README.md`).

## Normative source

See `COBALT_DCP_SPEC.md` section 4 (colour science) and section 7 (testing and validation) for the normative contract this document elaborates.
