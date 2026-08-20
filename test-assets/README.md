# Test assets

This directory holds vendor-supplied and copyrighted test files that must **never** be committed to the repository. The directory is `.gitignore`d except for this README.

## Expected local layout

```
test-assets/
  dcp/Fujifilm X-Pro2 Cobalt Flat v3.0.dcp   # the supplied base DCP (Cobalt Flat, X-Pro2)
  looks/Cobalt *.xmp                          # 17 Cobalt Look XMPs (CCD fever v3.0)
  raw/xpro2_*.RAF                             # user-supplied RAW test frames (include X-Trans + Bayer)
  reference/xpro2_*_acr.tif                   # ACR reference renders (16-bit ProPhoto, controls zeroed)
```

## Enabling asset-dependent tests

Tests that require these files are `#[ignore]`d by default and run only when the environment variable `RAPIDRAW_TEST_ASSETS=1` is set. CI never sets this flag, so the synthetic test suite carries the CI signal and CI stays green without these files.

## Producing ACR reference renders (the oracle)

See `docs/dcp-pipeline.md` for the full procedure. In short:

1. Open the test RAW in ACR / Lightroom (the profiles are installed there).
2. Set the profile to the Cobalt DCP under test.
3. Zero every other control: exposure 0, contrast 0, all tone/presence/colour sliders 0, no lens corrections, no sharpening, no noise reduction, no vignetting, tone curve = Linear.
4. White balance = As Shot; record the resulting temperature/tint.
5. Export 16-bit TIFF, ProPhoto RGB, full resolution, no output sharpening.
6. Record ACR version + Process Version (the Looks declare `ProcessVersion = 15.4`, `Version = 16.1.1 / 16.2`).

Compare in ProPhoto to avoid gamut clipping in the comparison itself.

## Licensing

Files here are commercial licensed products (Cobalt-Image profiles) or copyrighted RAW files. They are the user's own lawfully-acquired copies, used only for local testing of interoperability. Do not commit them. Do not redistribute them.
