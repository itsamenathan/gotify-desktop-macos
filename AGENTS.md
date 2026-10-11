# AGENTS.md

Agent guide for `gotify-desktop-osx`.

## Project Overview

- Build a macOS desktop client for [Gotify](https://gotify.net/) using Tauri + React.
- Backend: Rust (`src-tauri/src`)
- Frontend: React/TypeScript (`src`)
- App behavior is tray-first with main + quick windows.

## Non-Negotiable API Constraint

- Always follow Gotify server API spec:
  `https://raw.githubusercontent.com/gotify/server/v2.9.0/docs/spec.json`
- Do not invent unsupported Gotify fields or endpoints.

## Setup Commands

- Install dependencies: `npm install`
- Run app in dev mode: `npm run tauri dev`
- Frontend-only dev server: `npm run dev`
- Frontend build: `npm run build`
- Rust check: `cd src-tauri && cargo check --locked`

## Required Validation Before Finishing

- Run interaction standard check: `npm run standard:check`
- Run frontend build/typecheck: `npm run build`
- Run Rust check: `cd src-tauri && cargo check --locked`

## Architecture Map

- `src-tauri/src/main.rs`: app bootstrap + Tauri command registration
- `src-tauri/src/contract.rs`: canonical frontend/backend contract (snapshots + app updates)
- `src-tauri/src/stream.rs`: websocket lifecycle + reconnect behavior
- `src-tauri/src/messages.rs`: message cache, sync, persistence
- `src-tauri/src/pause.rs`: pause/resume domain logic
- `src-tauri/src/settings.rs`: settings load/save and validation
- `src/App.tsx`: frontend bootstrap and app update reducer handling
- `src/types.ts`: shared frontend contract types
- `docs/frontend-backend-standard.md`: full contract standard

## Frontend/Backend Interaction Standard (Tauri v2)

Follow `docs/frontend-backend-standard.md` exactly.

### Required rules

1. Commands (`invoke`) are authoritative for reads/writes.
2. Mutable-domain command responses return `DomainSnapshot<T>`.
3. Ordered/high-frequency backend updates use Tauri `Channel`.
4. Low-frequency UI notifications use targeted events (`emit_to`) only.
5. Frontend drops stale payloads: `incoming.revision <= currentRevision`.
6. One logical transition = one payload (no split multi-event state).
7. Do not use `app.emit(...)` or `window.emit(...)` for canonical state sync.
8. Serialize settings/message persistence writes to avoid race clobbering.

### Legacy contracts that must not return

- `notifications-pause-state`
- `notifications-pause-mode`
- `notifications-paused-until`
- `notifications-resumed`
- `messages-synced`
- `messages-updated`
- `message-received`
- `connection-state`
- `runtime-diagnostics`
- `connection-error`

## Coding Guidelines

- Make minimal, local, maintainable changes.
- Keep TypeScript strict-compatible.
- Keep Rust changes `cargo fmt` clean.
- Prefer explicit contracts/types over implicit behavior.
- Update docs when behavior/contract changes.

## CI and Release Notes

- CI file: `.github/workflows/ci.yml`
- Verify job includes `npm run standard:check`, frontend build, and Rust check.
- Release builds are tag-driven (`v*`) and use Tauri build on macOS.

## Version Changes

Always bump the version with `npm run version:set -- <version>` (e.g. `0.2.10`). Do not edit version numbers by hand. The script updates every file that carries the app version:

- `package.json`
- `package-lock.json` (top-level `version` and `packages[""].version`)
- `src-tauri/tauri.conf.json` (the version shown in the built app)
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock` (the `gotify-desktop` package entry)

If a new file starts carrying the version, add it to `scripts/set-version.mjs` and this list.

Release steps:

1. Add user-facing changes to `CHANGELOG.md` under `## Unreleased` as they land.
2. Rename `## Unreleased` to `## <version>` (commit: `chore: mark changelog <version>`).
3. Run `npm run version:set -- <version>` and commit all changed files (commit: `chore: bump version to <version>`).
4. Run the required validation above, then tag `v<version>` on `main`. CI re-runs `version:set` from the tag, so the tag must match the committed version.
