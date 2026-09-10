# ADR-0002 — One platform module per OS, behind a single `Platform` trait

- **Status:** Accepted — 2026-09-09
- **Relates to:** ADR-0001 (dependency diet / native per-OS windowing)

## Context

muri's OS-specific code (tray install, popup windowing, event loop, input,
focus/dismiss, a11y adapter, OS colors/DPI) is growing as the native-windowing
rewrite (ADR-0001) removes `winit` and drives AppKit / Win32 / Wayland directly.
Left unmanaged, `#[cfg(target_os = "...")]` branches scatter across the crate,
making the engine code hard to read, test, and port. Today only tray-anchoring
is abstracted (a `TrayAnchor` trait + `src/tray/{macos,windows,linux}.rs`) while
the entire macOS event loop, render-present, and a11y wiring sit inline in
`src/tray/macos.rs`, and stray `cfg(target_os)` gates exist elsewhere (e.g.
`lib.rs`).

## Decision

**All OS-specific code lives in exactly one module per OS, behind one common
trait, selected once. No `#[cfg(target_os)]` appears anywhere outside
`src/platform/`.**

- `src/platform/mac.rs`, `src/platform/windows.rs`, `src/platform/linux.rs` —
  each provides the full platform implementation.
- `src/platform/mod.rs` is the **single** place a target cfg appears: it selects
  exactly one implementation via `#[cfg(target_os = ...)]` and re-exports it as
  `PlatformImpl` (with a `platform::current() -> PlatformImpl` constructor).
- A common trait — `Platform` — captures the whole platform surface the engine
  needs:
  - tray: install icon, tooltip, `anchor_rect`, teardown;
  - popup/flyout windows: create (non-activating), show, move, close, present a
    `tiny-skia`/in-house pixmap;
  - event loop: run / pump, deliver a unified `PlatformEvent` (pointer, key,
    focus, redraw, a11y, tray-click, command) on the UI thread;
  - focus/dismiss tracking across popup + flyout windows;
  - a11y: attach/update/teardown the native adapter (accesskit_macos /
    _windows / _unix);
  - environment: OS accent color, appearance (dark/light), DPI/scale, work area
    for the anchor's own monitor.
- **All engine/app/menu/render/keynav/a11y code is platform-agnostic** and talks
  only through `Platform` (or pure functions). It contains zero target cfgs.
- **Enforcement:** `tests/strict_cfg.rs` fails the build if any
  `#[cfg(target_os = ...)]` appears outside `src/platform/*` (ported from
  usagio's guard, with comment/doc-string skipping so example text isn't a false
  positive).

## Consequences

- **Pros:** engine code reads as one portable implementation; each OS backend is
  a single self-contained file; adding/replacing a backend is local; the "no
  scattered cfg" invariant is machine-checked; matches ADR-0001's native-per-OS
  windowing cleanly (each `Platform` impl owns its run loop).
- **Cons / effort:** a one-time refactor to hoist the inline macOS code out of
  `src/tray/macos.rs` into `src/platform/mac.rs` behind the trait, and to route
  the existing `lib.rs` surfaces (`Tray`, `ContextMenu`, `Popup`) through
  `platform::current()` instead of cfg branches. The trait must be wide enough to
  cover all three OSes without leaking OS types into the engine (unified event +
  handle abstractions).

## Alternatives considered

- **Keep `TrayAnchor` + inline per-file backends** (status quo): rejected — only
  abstracts anchoring; the event loop / render / a11y stay OS-specific and inline,
  and cfg gates leak into `lib.rs` and elsewhere.
- **Feature flags per OS instead of `cfg(target_os)`:** rejected — target
  detection is automatic and correct; features would let a consumer mis-select a
  backend.
