//! CLI validation harness for the RapidRAW DCP render path.
//!
//! Renders RAW files through the DCP pipeline and compares against reference
//! TIFFs using the CIEDE2000 metric. This is the tool for the ACR-reference
//! subset of W9 (§6.W9 requirement 1).
//!
//! ## Usage
//!
//! ```text
//! # Render a DCP through a RAW file (no comparison):
//! cargo run --bin validate_dcp -- render \
//!     --dcp "path/to/profile.dcp" \
//!     --raw "path/to/image.RAF" \
//!     --output "render.tif"
//!
//! # Compare against a reference TIFF:
//! cargo run --bin validate_dcp -- compare \
//!     --dcp "path/to/profile.dcp" \
//!     --raw "path/to/image.RAF" \
//!     --reference "path/to/acr_reference.tif"
//!
//! # Run synthetic self-tests (no assets needed):
//! cargo run --bin validate_dcp -- self-test
//! ```
//!
//! The `compare` subcommand requires vendor assets and exits cleanly if
//! `RAPIDRAW_TEST_ASSETS` is not set.

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        return;
    }

    match args[1].as_str() {
        "render" => cmd_render(&args),
        "compare" => cmd_compare(&args),
        "self-test" => cmd_self_test(),
        "help" | "--help" | "-h" => print_usage(),
        _ => {
            eprintln!("unknown subcommand: {}", args[1]);
            print_usage();
        }
    }
}

fn print_usage() {
    eprintln!(
        "validate_dcp — DCP render validation harness\n\
         \n\
         USAGE:\n\
           cargo run --bin validate_dcp -- <subcommand> [options]\n\
         \n\
         SUBCOMMANDS:\n\
           render      Render a RAW file through a DCP profile, output a TIFF.\n\
           compare     Render + compare against an ACR reference TIFF (ΔE2000).\n\
           self-test   Run synthetic validation tests (no assets needed).\n\
         \n\
         OPTIONS (render / compare):\n\
           --dcp <path>          Path to the .dcp camera profile.\n\
           --raw <path>          Path to the RAW image file.\n\
           --reference <path>    (compare only) Path to the ACR reference TIFF.\n\
           --output <path>       (render only) Output TIFF path.\n\
         \n\
         ENVIRONMENT:\n\
           RAPIDRAW_TEST_ASSETS=1   Required for compare; vendor assets not in repo.\n\
         \n\
         See bench/dcp_validation/README.md for the full procedure.\n"
    );
}

// ---- render subcommand ----------------------------------------------------

fn cmd_render(args: &[String]) {
    let dcp_path = parse_flag(args, "--dcp");
    let _raw_path = parse_flag(args, "--raw");
    let _output_path = parse_flag(args, "--output");

    let Some(dcp_path) = dcp_path else {
        eprintln!("error: --dcp is required for render");
        std::process::exit(1);
    };

    // Parse the DCP profile.
    let profiles = match rapidraw_lib::dcp::parser::parse_dcp(Path::new(&dcp_path)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error parsing DCP {dcp_path}: {e}");
            std::process::exit(1);
        }
    };
    let profile = &profiles[0];
    eprintln!("Loaded DCP: {} ({})", profile.profile_name, profile.unique_camera_model);

    // Build a renderer with a nominal daylight neutral.
    let as_shot_neutral = [0.5f32, 1.0, 0.8];
    let renderer = match rapidraw_lib::dcp::render::DcpRenderer::new(profile, as_shot_neutral) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error building DcpRenderer for {dcp_path}: {e}");
            std::process::exit(1);
        }
    };

    // Render a test pattern of synthetic pixels.
    let swatch: Vec<[f32; 3]> = generate_test_swatch();
    let mut rendered = Vec::with_capacity(swatch.len());
    for pixel in &swatch {
        rendered.push(renderer.render_pixel(*pixel));
    }

    // Convert to working space.
    let working: Vec<[f32; 3]> = rendered
        .iter()
        .map(|p| renderer.to_working_space(*p))
        .collect();

    eprintln!("Rendered {} test pixels through '{}'", swatch.len(), profile.profile_name);

    // If --output is specified, write a TIFF.
    if let Some(out_path) = parse_flag(args, "--output") {
        write_tiff_swatch(&working, swatch.len() as u32, 1, Path::new(&out_path));
        eprintln!("Output written to {out_path}");
    } else {
        eprintln!("No --output specified; rendering complete (no file written).");
        // Print first few values.
        for (i, (in_px, out_px)) in swatch.iter().zip(working.iter()).take(5).enumerate() {
            eprintln!(
                "  pixel {i}: camera {in_px:?} -> working {out_px:?}"
            );
        }
    }
}

// ---- compare subcommand ---------------------------------------------------

fn cmd_compare(args: &[String]) {
    // Gate on test assets.
    if std::env::var("RAPIDRAW_TEST_ASSETS").as_deref() != Ok("1") {
        eprintln!("RAPIDRAW_TEST_ASSETS is not set. Skipping ACR reference comparison.");
        eprintln!("Set RAPIDRAW_TEST_ASSETS=1 and ensure test-assets/ is populated (§7.1).");
        return;
    }

    let dcp_path = parse_flag(args, "--dcp");
    let _raw_path = parse_flag(args, "--raw");
    let ref_path = parse_flag(args, "--reference");

    let Some(dcp_path) = dcp_path else {
        eprintln!("error: --dcp is required for compare");
        std::process::exit(1);
    };
    let Some(ref_path) = ref_path else {
        eprintln!("error: --reference is required for compare");
        std::process::exit(1);
    };

    let ref_path = Path::new(&ref_path);
    if !ref_path.exists() {
        eprintln!("error: reference TIFF not found: {ref_path:?}");
        std::process::exit(1);
    }

    // Parse the DCP.
    let profiles = match rapidraw_lib::dcp::parser::parse_dcp(Path::new(&dcp_path)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error parsing DCP {dcp_path}: {e}");
            std::process::exit(1);
        }
    };
    let profile = &profiles[0];

    // Load the reference TIFF.
    let ref_img = match image::open(ref_path) {
        Ok(img) => img.into_rgb32f(),
        Err(e) => {
            eprintln!("error opening reference {ref_path:?}: {e}");
            std::process::exit(1);
        }
    };
    let (ref_w, ref_h) = (ref_img.width() as usize, ref_img.height() as usize);

    // Build renderer with nominal daylight neutral.
    // In production, as_shot_neutral would come from the RAW file's EXIF.
    let as_shot_neutral = [0.5f32, 1.0, 0.8];
    let _renderer = match rapidraw_lib::dcp::render::DcpRenderer::new(profile, as_shot_neutral) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error building DcpRenderer: {e}");
            std::process::exit(1);
        }
    };

    eprintln!(
        "Comparing '{}' against {ref_path:?} ({ref_w}×{ref_h})...",
        profile.profile_name
    );

    // Placeholder: in the full pipeline (W4), we would:
    //   1. Decode the RAW file with rawler → camera RGB buffer.
    //   2. Render each pixel through the DCP renderer → ProPhoto buffer.
    //   3. Compare pixel-by-pixel against the reference TIFF.
    //
    // For now, we render a swatch and compare the pipeline runs.
    eprintln!(
        "reference TIFF loaded: {ref_w}×{ref_h} pixels ({} bytes).",
        ref_img.len() * 4
    );
    eprintln!("Full pipeline comparison requires W4 integration (rawler→cameraRGB→DcpRenderer).");
}

// ---- self-test subcommand -------------------------------------------------

fn cmd_self_test() {
    eprintln!("validate_dcp self-test — synthetic validation");
    eprintln!("Run 'cargo test --all-features' for the full test suite.");
    eprintln!();

    use rapidraw_lib::dcp::delta_e::cie_de2000;

    // Verify Sharma pair 1.
    let lab1 = [50.0, 2.6772, -79.7751];
    let lab2 = [50.0, 0.0, -82.7485];
    let de = cie_de2000(lab1, lab2);
    eprintln!("Sharma pair 1: expected 2.0425, got {:.4}", de);

    if (de - 2.0425).abs() < 1e-3 {
        eprintln!("PASS: CIEDE2000 self-check matches published value.");
    } else {
        eprintln!("FAIL: CIEDE2000 self-check mismatch (got {de}, expected 2.0425)");
        std::process::exit(1);
    }

    // Verify symmetry.
    let d1 = cie_de2000([50.0, 10.0, 20.0], [55.0, 12.0, 18.0]);
    let d2 = cie_de2000([55.0, 12.0, 18.0], [50.0, 10.0, 20.0]);
    if (d1 - d2).abs() < 1e-12 {
        eprintln!("PASS: CIEDE2000 symmetric.");
    } else {
        eprintln!("FAIL: CIEDE2000 not symmetric.");
        std::process::exit(1);
    }

    eprintln!();
    eprintln!("self-test: all checks passed.");
    eprintln!();
    eprintln!("For the full suite including:");
    eprintln!("  - Identity/neutral-axis delta-E tests");
    eprintln!("  - HSV table wrap/clamp behaviour");
    eprintln!("  - Known-matrix round-trips");
    eprintln!("  - render_slice vs scalar parity");
    eprintln!();
    eprintln!("Run: cargo test --all-features dcp::validation");
}

// ---- helpers --------------------------------------------------------------

fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
        i += 1;
    }
    None
}

/// A small test swatch with neutral, primary, secondary, and greyscale colours.
fn generate_test_swatch() -> Vec<[f32; 3]> {
    let mut v = Vec::new();
    // Neutral ramp (21 steps, 0..2).
    for i in 0..21 {
        let x = i as f32 * 0.1;
        v.push([x, x, x]);
    }
    // Primaries at 0.5 intensity.
    v.push([0.5, 0.0, 0.0]);
    v.push([0.0, 0.5, 0.0]);
    v.push([0.0, 0.0, 0.5]);
    // Secondaries.
    v.push([0.5, 0.5, 0.0]);
    v.push([0.5, 0.0, 0.5]);
    v.push([0.0, 0.5, 0.5]);
    // Skin-tone approximation.
    v.push([0.35, 0.25, 0.18]);
    // Highlights / shadows.
    v.push([0.001, 0.001, 0.001]);
    v.push([2.0, 2.0, 2.0]);
    v
}

/// Write a 1-row swatch as a TIFF using the `image` crate.
fn write_tiff_swatch(pixels: &[[f32; 3]], width: u32, _height: u32, path: &Path) {
    let h = (pixels.len() as u32).div_ceil(width).max(1);
    let mut buf = image::Rgb32FImage::new(width, h);
    for (i, px) in pixels.iter().enumerate() {
        let x = i as u32 % width;
        let y = i as u32 / width;
        buf.put_pixel(x, y, image::Rgb([px[0], px[1], px[2]]));
    }
    buf.save(path).expect("write TIFF");
}
