# Frontend/Backend Interaction Standard (Tauri v2)

This document defines the mandatory contract for frontend/backend interaction in this project.
All new code and refactors must follow this standard.

## Core Rules

1. Commands are authoritative for reads and writes.
2. Every mutable domain has a monotonic `revision`.
3. Mutating commands return a full `DomainSnapshot<T>`:
   `{"revision": number, "updated_at_ms": number, "data": T}`.
4. Ordered/high-frequency updates are delivered through a Tauri `Channel`.
5. Low-frequency UI events must be targeted (`emit_to`) and are never used as canonical state transport.
6. Frontend reducers must drop stale updates where `incoming.revision <= currentRevision`.
7. One logical transition is one payload object (no split multi-event state).
8. `app.emit(...)` and `window.emit(...)` are disallowed for state synchronization.
9. Settings and message cache persistence must be serialized (no concurrent read-modify-write clobber).
10. Gotify server communication remains aligned with the v2.9.0 API spec.

## App Contract

### Startup

1. Frontend calls `bootstrap_state`.
2. Frontend applies bootstrap snapshots with revision guards.
3. Frontend subscribes to ordered updates via `subscribe_app_updates(channel)`.
4. Frontend renders live-dependent state after the above sequence.

### Canonical Commands

- `bootstrap_state() -> BootstrapState`
- `save_settings(...) -> DomainSnapshot<SettingsResponse>`
- `set_pause(input) -> DomainSnapshot<PauseStateData>`
- `resume_pause() -> DomainSnapshot<PauseStateData>`
- `delete_message(...) -> DomainSnapshot<CachedMessage[]>`
- `recover_stream() -> DomainSnapshot<RuntimeDiagnostics>`
- `restart_stream() -> DomainSnapshot<RuntimeDiagnostics>`

### Ordered Channel Update Types

- `settings.updated`
- `pause.updated`
- `messages.replace`
- `messages.upsert`
- `messages.remove`
- `connection.updated`
- `runtime.updated`
- `stream.error`

## Legacy Contracts Removed

The following legacy event contracts are removed and must not be reintroduced:

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

## Enforcement

Static enforcement runs through:

- `npm run standard:check`

CI runs this check on every verify build.
