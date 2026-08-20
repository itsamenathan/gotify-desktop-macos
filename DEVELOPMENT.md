# Development Guide

## Prerequisites

- macOS (this project is macOS-focused)
- Node.js 20+ and npm
- Rust stable toolchain (`rustup`)
- Xcode Command Line Tools (`xcode-select --install`)

## Install dependencies

```bash
npm install
```

## Run in development mode

Use this mode while coding. It launches the Tauri desktop shell and hot-reloading frontend.

```bash
npm run tauri dev
```

Notes:
- Tauri automatically starts Vite using `npm run dev` (configured in `src-tauri/tauri.conf.json`).
- On first run, Rust crates may take a while to compile.

## Debug logs

When running in development mode (`npm run tauri dev`), backend debug logs are written to:
- `/tmp/gotify-desktop.log`

You can watch logs live with:

```bash
tail -f /tmp/gotify-desktop.log
```

Frontend debug output is also available in the WebView devtools console (`[gotify-ui]` log lines).

Important:
- Rust `debug_log(...)` output is only enabled in debug builds.
- Production/release builds do not emit these debug logs.

## Backend module map

The Rust backend is split by responsibility under `src-tauri/src/`:

- `main.rs` - app bootstrap, Tauri command registration, tray/setup wiring
- `stream.rs` - websocket lifecycle, reconnect/backoff, connection state updates
- `messages.rs` - message parsing, cache management, app metadata fetch/sync
- `notifications.rs` - notification gating and macOS notification delivery
- `pause.rs` - pause/resume state, tray pause menu state and related events
- `preview.rs` - URL preview fetch with redirect handling and SSRF protections
- `diagnostics.rs` - runtime diagnostics snapshot + emit helpers
- `settings.rs` - settings load/save and token/base URL helpers
- `ui_shell.rs` - main/quick window visibility and positioning behavior
- `core.rs` - shared file/logging/time helpers
- `consts.rs` - shared backend constants
- `model.rs` - shared backend structs/types

## Frontend/backend contract

The app-wide interaction contract is documented in:

- [`docs/frontend-backend-standard.md`](docs/frontend-backend-standard.md)

Static contract enforcement:

```bash
npm run standard:check
```

## Build for production

Create a release build and installable bundles:

```bash
npm run tauri build
```

Build output is generated under:
- `src-tauri/target/release/`

Common bundle locations:
- `.app`: `src-tauri/target/release/bundle/macos/`
- `.dmg`: `src-tauri/target/release/bundle/dmg/`

This produces a build for your own machine's architecture only.

### Universal (Apple Silicon + Intel) build

Release CI ships a universal binary so one download works on both Apple Silicon and Intel Macs. To reproduce that locally you need both Rust targets (requires a `rustup`-managed toolchain):

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run tauri build -- --target universal-apple-darwin
```

Bundles then land under `src-tauri/target/universal-apple-darwin/release/bundle/`.

Confirm both architecture slices are present:

```bash
lipo -info "src-tauri/target/universal-apple-darwin/release/bundle/macos/Gotify Desktop.app/Contents/MacOS/gotify-desktop"
```

## Run the production build locally

After building, either:
- open the `.app` from Finder, or
- run the release binary directly:

```bash
./src-tauri/target/release/gotify-desktop
```

## Frontend-only commands (optional)

If you only need the web UI without the desktop shell:

```bash
npm run dev
```

To create only the frontend static build:

```bash
npm run build
```

## Releasing

GitHub Actions is configured to build release artifacts when you push a version tag matching `v*` (for example `v0.2.0`).

Typical release flow:

```bash
npm run release:tag -- 0.2.0
```

Equivalent script path:

```bash
bash scripts/release.sh 0.2.0
```

What happens on tag push:
- CI runs verification checks.
- A release build runs on macOS (`npm run tauri build -- --target universal-apple-darwin`), producing a universal Apple Silicon + Intel bundle.
- CI fails the build if either architecture slice is missing from the `.app`.
- Release CI also syncs app version from the tag (`vX.Y.Z` -> `X.Y.Z`) before building, to keep bundle names/version metadata aligned with the tag.
- Artifacts are uploaded to the workflow run (`.dmg` and zipped `.app`).
- A GitHub Release is created or updated automatically with generated release notes and attached assets.
