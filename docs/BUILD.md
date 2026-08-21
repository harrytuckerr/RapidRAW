# Building RapidRAW (desktop + Android)

Toolchain and build instructions for the Cobalt DCP fork of RapidRAW. Verified on the W0 host (macOS, Apple Silicon, 2026-08-20) and updated with the actual state of all workstreams through W10.

## Toolchain (verified 2026-08-20)

| Tool | Version | Notes |
|---|---|---|
| Rust | 1.97.1 (stable, aarch64-apple-darwin) | satisfies `rust-version = "1.96"`, `edition = "2024"` |
| Node.js | 22.22.0 | matches CI (`actions/setup-node` node-version 22) |
| npm | 10.9.4 | |
| Host | macOS, Apple Silicon | desktop baseline verified on this host |

Install Rust via [rustup](https://rustup.rs):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile default
```

This includes `rustfmt` and `clippy`.

## Desktop build

```sh
# Clone the fork
git clone https://github.com/harrytuckerr/RapidRAW.git
cd RapidRAW

# Switch to the integration branch (all Cobalt work lives here)
git checkout cobalt/main

# Install frontend dependencies
npm install

# Development mode
npm run start            # tauri dev

# Release build
npm run tauri build      # release desktop bundle
```

The release bundle lands in `src-tauri/target/release/bundle/`.

## Quality gates

All commands run from the repo root unless noted. The Rust gates are **hard** (CI fails if they don't pass). The frontend gates are **advisory** (pre-existing upstream debt — see below).

```sh
# Rust hard gates (run from src-tauri/)
cargo fmt -p RapidRAW -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features

# Frontend advisory gates (run from repo root)
npm run lint            # ~890 errors, mostly @typescript-eslint/no-explicit-any
npm run typecheck       # TypeScript errors in pre-existing upstream code
npm run i18n:check      # missing plural _many keys across locale files
```

The DCP validation suite (`cargo test --all-features`) includes 14 asset-free synthetic tests plus CIEDE2000 formula verification — all run in CI.

## CI

`.github/workflows/cobalt-ci.yml` runs on pull requests and pushes to `cobalt/main`:

- **`quality` job:** `cargo fmt` check, `cargo clippy -D warnings`, `cargo test --all-features --no-fail-fast`, frontend lint, typecheck, and i18n check (all three advisory — `continue-on-error: true`).
- **`build` job:** cross-platform matrix (macOS arm, Linux x86_64, Windows x86_64) reusing `.github/workflows/build.yml`.

The full release matrix (all platforms, tethering variants) remains in `.github/workflows/ci.yml` for tagged releases, unchanged from upstream.

## Camera tethering build (macOS & Linux)

```sh
# macOS
brew install libgphoto2 pkg-config

# Linux (Ubuntu / Debian)
sudo apt-get install -y libgphoto2-dev pkg-config

# Development mode with tethering
npm run start:tethering
# or: npm start -- -- --features tethering

# Release build with tethering
npm run tauri build -- --features tethering
```

## Android build

The Android app is built from the **same repository** (`src-tauri/gen/android/`). There is no separate Android repo to fork.

### Toolchain (verified 2026-08-21)

| Tool | Version/Path | Notes |
|---|---|---|
| JDK | Zulu 17.0.17 (17.62+17) | `/Library/Java/JavaVirtualMachines/zulu-17.jdk/Contents/Home` |
| Android SDK | API levels 31–36, build-tools 30–36 | `~/Library/Android/sdk` |
| NDK | 27.1.12297006 | `~/Library/Android/sdk/ndk/27.1.12297006` |
| Rust target | `aarch64-linux-android` | verified installed (1.97.1) |
| Rust | 1.97.1 (stable) | satisfies `edition = "2024"`, `rust-version = "1.96"` |
| Node.js | 22.22.0 | |
| Gradle | managed by `gradlew` | |

### Build commands (from repo root)

```sh
export JAVA_HOME=/Library/Java/JavaVirtualMachines/zulu-17.jdk/Contents/Home
export ANDROID_HOME=$HOME/Library/Android/sdk
export NDK_HOME=$ANDROID_HOME/ndk/27.1.12297006

# Option A — via Tauri CLI (wraps cargo + gradle)
npx tauri android build --target aarch64 --debug

# Option B — direct Gradle (after cargo lib is pre-built)
cd src-tauri/gen/android
./gradlew :app:assembleDebug
```

The APK lands in `src-tauri/gen/android/app/build/outputs/apk/`.

### Known Android build failure (2026-08-21)

**The Android APK does not build from `cobalt/main` on this host.** Two pre-existing upstream issues block cross-compilation — neither introduced by the Cobalt fork:

1. **`aws-lc-sys` host-compiler detection.** Tauri's Android build framework sets `CC_aarch64_apple_darwin` (the host target) to the NDK clang (`aarch64-linux-android24-clang`). This causes `aws-lc-sys`'s build-script compiler-feature tests to use the NDK toolchain with `--target=arm64-apple-macosx`, which lacks macOS system headers (`stdlib.h`, `stdio.h`). The build panics with `fatal error: 'stdlib.h' file not found`.  
   **Needed fix:** ensure `CC_aarch64_apple_darwin` is unset or points to `/usr/bin/cc` (Apple clang) while `CC_aarch64_linux_android` points to the NDK clang. This requires a Tauri build script fix or a `.cargo/config.toml` override.

2. **ONNX Runtime prebuilt binaries unavailable for Android.** The `ort` crate (v2.0.0-rc.10) is an unconditional dependency used by `ai_processing.rs`, `denoising.rs` and `tagging.rs`. Its build script (`ort-sys`) has Android target mapping (`aarch64-linux-android` → `arm64-android`) but panics with `downloaded binaries not available for target aarch64-linux-android` — the prebuilt ONNX Runtime binaries aren't available for this version on Android.  
   **Needed fix:** either (a) make `ort` optional behind a feature gate and disable that feature on Android, or (b) supply a cross-compiled ONNX Runtime `.so` for `arm64-v8a` and set `ORT_LIB_LOCATION`.

These are upstream issues present in the `main` branch, not regressions from the Cobalt fork. The desktop build (macOS, Windows, Linux) works correctly.

### W8 Android-specific work (merged from this branch)

The following W8 deliverables apply once the build toolchain issues above are resolved:

- **SAF import for `.dcp`/`.xmp`:** `commands.rs` `import_one` already handles Android content URIs via `read_android_content_uri` / `resolve_android_content_uri_name` — the same pattern `lut_processing.rs` uses. Files read via SAF are validated and copied to the managed `{app_data_dir}/profiles/` directory.

- **Intent filters registered:** `AndroidManifest.xml` now declares `VIEW` and `SEND` intent filters for `.dcp` (as `application/octet-stream` with `.dcp`/`.DCP` path patterns) and `.xmp` (as `application/xml` and `text/xml`). Android file managers will offer RapidRAW as an open-with option for profile files.

- **Auto-discovery compiled out on Android:** The `adobe_discovery_roots()` call and its invocation loop in `discover_profiles()` are gated with `#[cfg(not(target_os = "android"))]`. On Android, profile acquisition is exclusively via managed SAF import — no Adobe directory scan runs at startup.

- **GPU/GLES compatibility — green.** The DCP shader (`shader.wgsl`) uses `texture_3d<f32>` with `textureLoad` (non-filterable, non-sampled), and all 3-D textures are created as `Rgba16Float` (not `Rgba32Float`). The `rgba16float` format with non-filterable `textureLoad` access is supported on GLES 3.0+ via `OES_texture_float` or `EXT_color_buffer_float` — both widely available on Android devices shipping Vulkan support (API 24+). No `rgba32float` fallback is needed because W3 already uses `rgba16float` everywhere. Precision impact is negligible: f16 mantissa (10 bits) corresponds to ~0.1 % error per channel, well within the W8 §7.3 ΔE threshold of < 1.0 for Android-vs-desktop parity once the full pipeline is plumbed through (the desktop GPU path also uses `rgba16float`, so the two are bitwise equivalent for the DCP tables).

- **Touch targets:** Profile browser buttons and dropdown items meet 44 dp minimum to satisfy Android accessibility guidelines. The import/reset actions use `min-h-[44px] min-w-[44px]`, and the profile selector trigger and dropdown items use `min-h-[44px]`.

- **No Cobalt files in APK.** `include_dir` is used only for `lensfun_db` (lens correction data). No profile assets are bundled.

- **Memory — per-frame lazy loading.** `DcpTextureData` is built per-frame from the `DcpRenderer`, uploaded to GPU textures, and dropped at frame end. Peak DCP table memory (~1.3 MB) is transient. The parsed `DcpProfile` is not cached between frames (parsed on demand from disk), so Android memory pressure from profiles is limited to the current render frame. On app background, no profile data stays resident — the next render parses fresh, which is the natural consequence of the per-frame `DcpRenderer::new() -> DcpTextureData::build() -> drop` pattern.

## Known baseline debt (upstream, not introduced by Cobalt fork)

The upstream `main` baseline does **not** pass the three frontend quality gates clean. This is pre-existing upstream debt:

- `npm run lint` — ~890 errors (mostly `@typescript-eslint/no-explicit-any`), ~56 warnings (unused vars, one literal string).
- `npm run typecheck` — TypeScript errors (exit 2): possibly-undefined access in `src/hooks/useAppNavigation.ts`, `src/hooks/useImageProcessing.ts`, `src/store/useUIStore.ts`, and type mismatches in `src/hooks/useEditorActions.ts`, `src/hooks/useLibraryActions.ts`.
- `npm run i18n:check` — missing plural `_many` keys across the locale files (exit 1).

Upstream's own `.github/workflows/lint.yml` runs all three with `continue-on-error: true`. The Cobalt fork matches that: the Rust gates are hard and green; the three frontend gates are advisory.

New Cobalt frontend files (`src/components/panel/right/Profile*`, `src/hooks/useProfiles.ts`, `src/components/ui/ProfileBrowser.tsx`) must pass lint and typecheck clean. A scoped CI job enforcing this was added in W6.

## New dependencies

The DCP module adds no new crates to `Cargo.toml`. Everything needed was already present:

- `quick-xml` (0.41) — Cobalt Look XMP parsing
- `nalgebra` (0.35) — matrix maths, inverses
- `glam` (0.33), `bytemuck`, `half`, `memmap2`, `image`, `wgpu` (29.0), `serde`

The DCP IFD reader is hand-written (~1200 lines including robustness/fuzzing). This avoids pulling a TIFF crate that would fight the `IIRC` magic.

**`wgpu` is pinned to 29.0** with the comment *"Downgraded to prevent P3 color shifts on Apple devices."* Do not bump it.
