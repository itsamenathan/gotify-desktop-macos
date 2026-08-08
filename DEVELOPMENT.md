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

### Build for a specific architecture

The plain build above targets the machine you are building on. To build for a specific Mac architecture (both are supported and released), install the Rust target once and pass it to the build:

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin

# Apple silicon
npm run tauri build -- --target aarch64-apple-darwin

# Intel
npm run tauri build -- --target x86_64-apple-darwin
```

Cross-compiling between the two works on either kind of Mac.

When `--target` is passed, bundles move under the target triple:
- `.app`: `src-tauri/target/<target-triple>/release/bundle/macos/`
- `.dmg`: `src-tauri/target/<target-triple>/release/bundle/dmg/`

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
- CI runs verification checks, including a Rust check for both `aarch64-apple-darwin` and `x86_64-apple-darwin`.
- Release builds run on macOS once per architecture (`npm run tauri build -- --target <triple>`).
- Release CI also syncs app version from the tag (`vX.Y.Z` -> `X.Y.Z`) before building, to keep bundle names/version metadata aligned with the tag.
- Artifacts are uploaded to the workflow run (`.dmg` and zipped `.app`) with an architecture suffix:
  - `gotify-desktop-<tag>-macos-apple-silicon.dmg` / `-app.zip`
  - `gotify-desktop-<tag>-macos-intel.dmg` / `-app.zip`
- A GitHub Release is created or updated automatically with generated release notes and all architecture assets attached.
