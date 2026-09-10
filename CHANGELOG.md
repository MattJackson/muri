# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.1] - 2026-09-09

Patch release in the 0.9.x testing cycle: `muda-compat` drop-in parity fixes found
while migrating the first real adopter (usagio) onto the facade.

### Fixed

- **`muda-compat`: `MenuId::new()` constructor** — added `MenuId::new(impl
  Into<String>)` mirroring real muda's constructor, so callers that build ids via
  `MenuId::new(id)` (rather than passing a `&str`/`String`) compile through the
  facade unchanged (#4).
- **`muda-compat`: `compat::tray_icon::menu` module** — `compat::tray_icon` now
  re-exports the facade's muda module as `menu` (mirroring real tray-icon's `pub
  use muda as menu`), so the canonical `tray_icon::menu::{Menu, MenuItem,
  CheckMenuItem, Submenu, PredefinedMenuItem, …}` import paths migrate with a pure
  `use tray_icon` → `use muri::compat::tray_icon` swap (#5).

### Documentation

- README synced to the published 0.9.0: install shows `muri = "0.9"` (crate is on
  crates.io), the Windows backend is described as code-complete, the version is
  `0.9.x`, and the feature-flag table reflects the shipped `muda-compat` (and drops
  the non-existent `serde` flag).

## [0.9.0] - 2026-09-09

First crates.io release: a themeable, custom-drawn tray + popup/menu library — a
drop-in-capable replacement for `muda` + `tray-icon`. This is a **testing
release**; behaviors that require real hardware or screen readers are marked
`DEVICE-VERIFY(0.9.0)` in the source and are the focus of the 0.9.x cycle.

### Added

- **Custom-drawn menu system** — a declarative `Segment → Row → Item → Menu`
  model with a flush-right `Flex`/`Align` layout (no reserved chevron column),
  N-level nested flyouts, a full keyboard-navigation state machine, and complete
  theming: OS-native look by default, or any colors/borders/vibrancy the app
  chooses. `Theme::native()` / `native_dark()` let the OS material show through,
  and `ThemeSource::FollowSystem` tracks the system appearance.
- **In-house CPU renderer** — an owned premultiplied-RGBA `Framebuffer`
  (analytic-SDF anti-aliased rounded rects, device-snapped separators, `over`
  compositing) plus a text stack built directly on `fontdb` + `harfrust` +
  `swash` with a muri-owned font-fallback layer, so cross-script fallback and
  color emoji are preserved. CPU raster means a tiny binary, no GPU warm-up, and
  instant popups with full pixel control.
- **macOS backend** — a hand-rolled non-activating `NSPanel` with an
  `NSVisualEffectView` vibrancy backdrop presented via `CALayer`, a native
  `NSApplication` run loop with a GCD main-thread drain, an `NSStatusItem` tray,
  N-level flyout panels, and per-window VoiceOver through `accesskit_macos`.
- **Windows backend** — a `WS_EX_NOACTIVATE` layered popup (per-pixel alpha via
  `UpdateLayeredWindow`, DWM acrylic backdrop), a Win32 message pump,
  `WH_MOUSE_LL`/`WH_KEYBOARD_LL` hooks for outside-click dismiss and keyboard, a
  `Shell_NotifyIcon` (PNG→`HICON`) tray, and NVDA/Narrator through
  `accesskit_windows`.
- **Linux backend** — a StatusNotifierItem / AppIndicator native menu tray built
  from the `Menu` (pure-Rust `ksni` + `zbus`, no GTK/system-lib build deps;
  AT-SPI via Orca for free), plus a styled X11 override-redirect popup for
  `ContextMenu::open_at`.
- **`ContextMenu::open_at` and `Popup::anchored_to`** — pointer- and
  rect-anchored styled popups over one shared `PopupSession`, real on macOS,
  Windows, and Linux/X11.
- **Unified event model** — a process-global `MenuEvent::receiver()` channel and
  a generic `set_event_handler`, so a migrated muda app's receiver loop works
  unchanged; every muri surface (`Tray`, `ContextMenu`, `Popup`) fires the
  per-surface closure first, then projects the activation onto the channel. A
  `Clone + Send` `TrayHandle` (`Tray::handle()`) drives the running tray
  (`set_menu`/`set_icon`/`set_tooltip`/`set_visible`/`open`/`close`) from any
  thread.
- **Accessibility** — a parallel AccessKit tree (menu / menuitem / checkbox /
  group / separator roles, accessible name, enabled/checked,
  `has_popup`/`expanded`, positional info), bridged to each OS platform adapter.
- **muda / tray-icon drop-in facade** (`muda-compat` feature) —
  `muri::compat::muda` and `compat::tray_icon` mirror those crates' public
  surfaces (items, checkable items, icon items, submenus, accelerators, and the
  global event channels) onto muri's native model, with **no** real
  muda/tray-icon dependencies, so a migrated app changes only its imports.
- **Release + CI infrastructure** — a tag-driven, version-guarded `cargo publish`
  on `vX.Y.Z` tags; a `dev`/`qa`/`main` CI policy (fmt, clippy `-D warnings`,
  build, examples, tests, and docs across a macOS / Ubuntu / Windows matrix); a
  dedicated MSRV job; and a weekly `cargo-deny` advisories/bans/sources audit.
- **Speed-first `[profile.release]`** (`opt-level = 3`, `lto = "fat"`,
  `codegen-units = 1`, `strip`).

### Notes

- **MSRV is 1.87.** The core, macOS, and Windows trees build on 1.85, but the
  Linux SNI tray's modern `zbus` 5 D-Bus stack requires 1.87 — the honest
  crate-wide floor. Clippy/fmt/test run on the latest stable toolchain; only the
  dedicated MSRV job pins 1.87, so lints are never held back.
- **Dependency diet (ADR-0001 / ADR-0002).** muri owns the menu system and thin
  platform glue and *uses* — rather than reinvents — the deep engines:
  `fontdb`/`harfrust`/`swash` for text, `accesskit` for accessibility, and
  `objc2` / `windows-sys` / `x11rb` / `ksni` for OS FFI. `cosmic-text`,
  `tiny-skia`, `winit`, and `softbuffer` were removed. `harfrust` replaces the
  unmaintained `rustybuzz` (RUSTSEC-2026-0206).
- **Wayland.** A tray-*anchored* styled popup is architecturally impossible (a
  client cannot position its own toplevel); `Tray::run` still installs a working
  native SNI menu there, and `ContextMenu::open_at` returns
  `Unsupported::ClientPositioning` on a bare Wayland session. Use the native menu
  or an X11 (incl. XWayland) session for the styled popup.

[Unreleased]: https://github.com/MattJackson/muri/compare/v0.9.1...HEAD
[0.9.1]: https://github.com/MattJackson/muri/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/MattJackson/muri/releases/tag/v0.9.0
