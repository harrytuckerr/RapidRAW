# Building RapidRAW (desktop + Android)

> Status: stub. Toolchain verified by W0 on 2026-08-20; Android section finalised by W8.

## Toolchain (verified 2026-08-20 on the W0 host)

| Tool | Version | Notes |
|---|---|---|
| Rust | 1.97.1 (stable, aarch64-apple-darwin) | satisfies `rust-version = "1.96"`, `edition = "2024"` |
| Node.js | 22.22.0 | matches CI (`actions/setup-node` node-version 22) |
| npm | 10.9.4 | |
| Host | macOS, Apple Silicon | desktop baseline verified on this host |

Install Rust via [rustup](https://rustup.rs): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile default` (includes `rustfmt` and `clippy`).

## Desktop build

```sh
npm install
npm run tauri build      # release desktop bundle
# or, for development:
npm run start            # tauri dev
```

Quality gates (mirrored by CI on PRs to `cobalt/main`):

```sh
# from src-tauri/
cargo fmt -p RapidRAW -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
# from the repo root:
npm run lint
npm run typecheck
npm run i18n:check
```

## Android build

> Filled by W8. The Android app is built from the **same repository** (`src-tauri/gen/android/`); there is no separate Android repo to fork. Exact commands and toolchain (JDK 17, Android SDK, NDK r26d) will be recorded here once W8 verifies them.

## CI

`.github/workflows/cobalt-ci.yml` runs on pull requests and pushes to `cobalt/main`:

- a `quality` job: `cargo fmt` check, `cargo clippy -D warnings`, `cargo test`, frontend lint, typecheck, and i18n check;
- a `build` job: a representative cross-platform matrix (macOS arm, Linux, Windows) reusing `.github/workflows/build.yml`. The Android build enters CI with W8.

The full release matrix (all platforms, tethering variants) remains in `.github/workflows/ci.yml` for tagged releases, unchanged from upstream.
