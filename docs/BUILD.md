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

### Prerequisites

- JDK 17
- Android SDK (API level 34+)
- Android NDK r26d
- Rust Android targets: `aarch64-linux-android`, `armv7-linux-androideabi`, `x86_64-linux-android`, `i686-linux-android`

Install Rust targets:

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
```

### Build commands

```sh
# From the repo root
cd src-tauri/gen/android

# Debug APK
./gradlew :app:assembleDebug

# Release APK
./gradlew :app:assembleRelease
```

The APK lands in `src-tauri/gen/android/app/build/outputs/apk/`.

### Android limitations (W8 not yet merged)

The Android build for the Cobalt DCP feature is **in progress** (workstream W8 has not been merged into `cobalt/main`):

- **SAF import** for `.dcp`/`.xmp` files is partially wired (mirrors the existing LUT import pattern). The import command in `commands.rs` handles Android content URIs, but the manifest has not been updated with intent filters for `.dcp`/`.xmp` MIME types.
- **GPU compatibility:** 3-D `rgba32float` texture sampling on GLES 3.0 has not been verified on real devices. A `rgba16float` fallback is spec'd but not implemented.
- **Auto-discovery:** does not apply on Android (no Adobe directory). Profiles must be imported via SAF.
- **Memory:** DCP tables (~1.3 MB per active profile) need to be lazily loaded and dropped on background. Current desktop code keeps the active renderer in memory throughout the session.

If you are building the Android APK from `cobalt/main` today, the core RapidRAW app will build and run, but **camera profiles will not be importable or usable on Android.** The desktop path is fully functional.

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
