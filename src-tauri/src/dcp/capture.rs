//! Route B: Profile Capture via ACR round-trip (W7).
//!
//! When Route A (native decode of the embedded `dng_big_table` blob) fails
//! for a Cobalt Look, the user can capture the table via an ACR round-trip:
//!
//! 1. RapidRAW exports an identity HALD image (16-bit TIFF).
//! 2. The user opens it in Lightroom/ACR, sets Profile = the Cobalt Look at
//!    Amount 100, zeros every other control, and exports 16-bit TIFF.
//! 3. RapidRAW converts the processed HALD to a `.cube` 3-D LUT and links it
//!    to the Look via the `Captured` variant of `LookTableSource`.
//!
//! The two halves of this — identity HALD generation and HALD-to-cube
//! conversion — already live in `crate::lut_processing::` (confirmed at
//! `generate_identity_lut_image` line 430 and `convert_image_to_cube_lut`
//! line 450). This module wraps them with file I/O and lookup plumbing.

use std::path::{Path, PathBuf};

use image::GenericImageView;

/// Export an identity HALD image as a 16-bit TIFF to `path`.
///
/// `size` is the grid size per axis. 33 is recommended (35937 pixels, fast
/// round-trip); 64 gives finer interpolation (262144 pixels, larger files).
/// The image is saved as 16-bit RGB TIFF, which preserves the [0,1] linear
/// values that ACR/Lightroom interprets correctly.
pub fn export_identity_hald(path: &Path, size: u32) -> Result<(), String> {
    let identity = crate::lut_processing::generate_identity_lut_image(size);
    let rgb16 = identity.to_rgb16();
    rgb16.save(path).map_err(|e| {
        format!(
            "failed to save identity HALD TIFF to '{}': {e}",
            path.display()
        )
    })
}

/// Detect the HALD cube size from the N×N² layout used by
/// `generate_identity_lut_image` and `convert_image_to_cube_lut`.
/// Width = N (grid size per axis), height = N².
fn detect_hald_size(width: u32, height: u32) -> Result<u32, String> {
    if height != width * width {
        return Err(format!(
            "HALD must have N×N² layout: got {width}x{height}, expected {width}x{}",
            width * width
        ));
    }
    // width == N, total pixels == N³
    Ok(width)
}

/// Convert a processed HALD TIFF to a `.cube` 3-D LUT and save it.
///
/// Reads the 16-bit TIFF at `tiff_path`, runs `convert_image_to_cube_lut`,
/// and writes the resulting `.cube` text to `cube_path`. The parent
/// directory is created if it does not exist.
pub fn capture_to_cube(tiff_path: &Path, cube_path: &Path) -> Result<PathBuf, String> {
    let img =
        image::open(tiff_path).map_err(|e| format!("failed to open processed HALD TIFF: {e}"))?;

    let (w, h) = img.dimensions();
    let size = detect_hald_size(w, h)?;

    let cube_bytes = crate::lut_processing::convert_image_to_cube_lut(&img, size)?;

    if let Some(parent) = cube_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create directory for captured LUT: {e}"))?;
    }

    std::fs::write(cube_path, &cube_bytes).map_err(|e| {
        format!(
            "failed to write captured .cube LUT to '{}': {e}",
            cube_path.display()
        )
    })?;

    Ok(cube_path.to_path_buf())
}

/// Check whether a captured `.cube` LUT exists for a Look given its UUID.
pub fn find_captured_lut(captured_dir: &Path, look_uuid: &str) -> Option<PathBuf> {
    let path = captured_dir.join(format!("{look_uuid}.cube"));
    if path.exists() { Some(path) } else { None }
}

/// Directory where captured `.cube` LUTs are stored, keyed by Look UUID.
/// Lives under the managed looks directory: `profiles/looks/captured/`.
pub fn captured_luts_dir(app_data_dir: &Path) -> PathBuf {
    crate::dcp::registry::managed_looks_dir(app_data_dir).join("captured")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate an identity HALD, round-trip it through save/load, and verify
    /// the resulting .cube LUT is identity (all samples exactly on the
    /// diagonal). This proves `export_identity_hald` + `capture_to_cube`
    /// produce a mathematically correct identity table.
    #[test]
    fn identity_hald_round_trip_is_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let tiff_path = tmp.path().join("identity.tiff");
        let cube_path = tmp.path().join("identity.cube");

        // Use size 4 for speed (64 pixels, 192 float values in the .cube).
        let size: u32 = 4;
        export_identity_hald(&tiff_path, size).unwrap();

        // Re-open the saved TIFF and convert to .cube.
        let out = capture_to_cube(&tiff_path, &cube_path).unwrap();
        assert_eq!(out, cube_path);

        let cube_text = std::fs::read_to_string(&cube_path).unwrap();
        // First line must declare the size.
        assert!(
            cube_text.contains(&format!("LUT_3D_SIZE {size}")),
            "missing LUT_3D_SIZE header"
        );

        // Parse data lines: skip header comments and size/title/domain lines.
        let data_lines: Vec<&str> = cube_text
            .lines()
            .filter(|l| {
                let t = l.trim();
                !t.is_empty()
                    && !t.starts_with('#')
                    && !t.starts_with("LUT_3D_SIZE")
                    && !t.starts_with("TITLE")
                    && !t.starts_with("DOMAIN_MIN")
                    && !t.starts_with("DOMAIN_MAX")
            })
            .collect();

        let expected = (size as usize).pow(3);
        assert_eq!(
            data_lines.len(),
            expected,
            "cube has {} data lines, expected {}",
            data_lines.len(),
            expected
        );

        // For an identity HALD, each sample should be identity: the HALD
        // encodes the lookup coordinate itself, so converting back should
        // give identity. Because 16-bit TIFF round-trip quantises float
        // [0,1] → [0,65535] → [0,1], there is small round-trip error.
        // We check that every sample is within 1e-4 of identity (the
        // diagonal).
        //
        // HALD pixel layout at (x, z*N+y): Rgb([x/(N-1), y/(N-1), z/(N-1)])
        // .cube line order: z-major, y-intermediate, x-fastest.
        let n = size as usize;
        let epsilon = 2.0e-4_f32; // generous for 16-bit quantisation
        for (idx, line) in data_lines.iter().enumerate() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(
                parts.len(),
                3,
                "line {idx}: expected 3 values, got {}",
                parts.len()
            );
            let r: f32 = parts[0].parse().unwrap();
            let g: f32 = parts[1].parse().unwrap();
            let b: f32 = parts[2].parse().unwrap();

            // .cube iteration: x fastest, y intermediate, z slowest.
            let x = idx % n;
            let y = (idx / n) % n;
            let z = idx / (n * n);

            // HALD pixel (x, z*n+y) stores R=x, G=y, B=z.
            let expected_r = x as f32 / (n - 1) as f32;
            let expected_g = y as f32 / (n - 1) as f32;
            let expected_b = z as f32 / (n - 1) as f32;

            assert!(
                (r - expected_r).abs() < epsilon,
                "sample [{x},{y},{z}] r={r:.6} expected {expected_r:.6}"
            );
            assert!(
                (g - expected_g).abs() < epsilon,
                "sample [{x},{y},{z}] g={g:.6} expected {expected_g:.6}"
            );
            assert!(
                (b - expected_b).abs() < epsilon,
                "sample [{x},{y},{z}] b={b:.6} expected {expected_b:.6}"
            );
        }
    }

    /// `detect_hald_size` only accepts the N×N² HALD layout.
    #[test]
    fn detect_hald_size_accepts_n_by_n_squared() {
        // 4 × 16 = 4³ layout → ok
        assert_eq!(detect_hald_size(4, 16).unwrap(), 4);
        // 33 × 1089 = 33³ layout → ok
        assert_eq!(detect_hald_size(33, 1089).unwrap(), 33);
        // 4 × 4 is not N×N² (would need to be 4×16) → error
        assert!(detect_hald_size(4, 4).is_err());
        // 10 × 10 is not N×N² → error
        assert!(detect_hald_size(10, 10).is_err());
        // 4 × 8 is not N×N² (height=8, width²=16) → error
        assert!(detect_hald_size(4, 8).is_err());
    }

    /// `find_captured_lut` returns None when the file is absent and Some when
    /// it exists.
    #[test]
    fn find_captured_lut_absent_and_present() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let uuid = "369CEFB650E07246A4C925B6449A0E01";

        // Absent.
        assert!(find_captured_lut(dir, uuid).is_none());

        // Create the file.
        let path = dir.join(format!("{uuid}.cube"));
        std::fs::write(&path, b"LUT_3D_SIZE 2\n").unwrap();
        assert_eq!(find_captured_lut(dir, uuid), Some(path));
    }

    /// Generate a synthetic processed HALD (not identity — shift red channel
    /// by +0.1) as a 16-bit TIFF, convert to .cube, and verify the red shift
    /// is preserved. This proves `capture_to_cube` correctly reads 16-bit
    /// RGB TIFF and the .cube output has the right values within 16-bit
    /// quantisation tolerance.
    #[test]
    fn synthetic_processed_hald_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let tiff_path = tmp.path().join("processed.tiff");
        let cube_path = tmp.path().join("processed.cube");

        let size: u32 = 4;
        let w = size;
        let h = size * size;

        // Build a 16-bit RGB image with red boosted by +0.1 (clamped to 1.0).
        let mut buf = vec![0u16; (w * h * 3) as usize];
        let n = size as f32;
        for z in 0..size {
            for y in 0..size {
                for x in 0..size {
                    let r = x as f32 / (n - 1.0);
                    let g = y as f32 / (n - 1.0);
                    let b = z as f32 / (n - 1.0);
                    let idx = ((z * size + y) * size + x) as usize * 3;
                    buf[idx] = ((r + 0.1).clamp(0.0, 1.0) * 65535.0).round() as u16;
                    buf[idx + 1] = (g * 65535.0).round() as u16;
                    buf[idx + 2] = (b * 65535.0).round() as u16;
                }
            }
        }

        image::save_buffer(
            &tiff_path,
            bytemuck::cast_slice(&buf),
            w,
            h,
            image::ColorType::Rgb16,
        )
        .unwrap_or_else(|e| panic!("failed to save synthetic TIFF: {e}"));

        capture_to_cube(&tiff_path, &cube_path).unwrap();

        let cube_text = std::fs::read_to_string(&cube_path).unwrap();
        let data_lines: Vec<&str> = cube_text
            .lines()
            .filter(|l| {
                let t = l.trim();
                !t.is_empty()
                    && !t.starts_with('#')
                    && !t.starts_with("LUT_3D_SIZE")
                    && !t.starts_with("TITLE")
                    && !t.starts_with("DOMAIN_MIN")
                    && !t.starts_with("DOMAIN_MAX")
            })
            .collect();

        let expected = (size as usize).pow(3);
        assert_eq!(data_lines.len(), expected);

        let epsilon = 2.0e-4_f32;
        for (idx, line) in data_lines.iter().enumerate() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let r: f32 = parts[0].parse().unwrap();
            let g: f32 = parts[1].parse().unwrap();
            let b: f32 = parts[2].parse().unwrap();

            let n = size as usize;
            let x = idx % n;
            let y = (idx / n) % n;
            let z = idx / (n * n);

            // The synthetic HALD has red boosted by +0.1 at every pixel.
            // Pixel (x, z*N+y) stores Rgb([r_boosted, g, b]) where
            // r_boosted = clamp(x/(N-1)+0.1), g = y/(N-1), b = z/(N-1).
            let expected_r = (x as f32 / (n as f32 - 1.0) + 0.1).clamp(0.0, 1.0);
            let expected_g = y as f32 / (n as f32 - 1.0);
            let expected_b = z as f32 / (n as f32 - 1.0);

            assert!(
                (r - expected_r).abs() < epsilon,
                "sample [{x},{y},{z}] r={r:.6} expected {expected_r:.6}"
            );
            assert!(
                (g - expected_g).abs() < epsilon,
                "sample [{x},{y},{z}] g={g:.6} expected {expected_g:.6}"
            );
            assert!(
                (b - expected_b).abs() < epsilon,
                "sample [{x},{y},{z}] b={b:.6} expected {expected_b:.6}"
            );
        }
    }

    /// Identity HALD generation produces a square-ish output with correct
    /// dimensions (width = size, height = size^2).
    #[test]
    fn identity_hald_dimensions() {
        for size in [2u32, 4, 8, 33] {
            let img = crate::lut_processing::generate_identity_lut_image(size);
            let (w, h) = img.dimensions();
            assert_eq!(w, size, "width mismatch for size {size}");
            assert_eq!(h, size * size, "height mismatch for size {size}");
        }
    }
}
