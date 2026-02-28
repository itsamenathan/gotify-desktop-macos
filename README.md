# Gotify Desktop (macOS)

Desktop client for [Gotify](https://gotify.net/) built with Tauri (Rust) + React/Vite.

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
git checkout main
git pull
npm run version:set -- 0.2.0
git add package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json
git commit -m "chore: bump version to 0.2.0"
git push origin main
git tag v0.2.0
git push origin v0.2.0
```

What happens on tag push:
- CI runs verification checks.
- A release build runs (`npm run tauri build`) on macOS.
- Release CI also syncs app version from the tag (`vX.Y.Z` -> `X.Y.Z`) before building, to keep bundle names/version metadata aligned with the tag.
- Artifacts are uploaded to the workflow run (`.dmg` and zipped `.app`).
- A GitHub Release is created/updated automatically with generated release notes and attached assets.
