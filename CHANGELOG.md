# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Phase 1 — macOS backend (a menu you can actually see).**
  - Real CPU-raster scene drawer: `RasterDrawer` implements `SceneDrawer` with
    `tiny-skia` (rounded-rect panel, row highlight, hairline separators, bitmap
    icon blit) and `cosmic-text` (system-font measurement, shaping, glyph
    blitting with per-`StyleRun` colors and weights).
  - `render::paint::render_menu`: the single platform-agnostic layout + draw pass
    that resolves the `Flex`/`Align` layout into a **truly flush-right value with
    no reserved chevron column**, draws section headers, separators, checkmarks,
    PNG logos, severity color spans, submenu `›` affordances, and returns a
    `LaidMenu` hit-test map.
  - macOS tray + popup: `tray::macos` creates the `NSStatusItem` (objc2), reports
    the button's screen frame for anchoring, and runs a winit/softbuffer event
    loop that opens a borderless popup under the icon, highlights the hovered
    row, dispatches a row's `MenuId` on click then closes, and dismisses on focus
    loss. `Tray::run` is now implemented on macOS. Theme follows
    `NSApp effectiveAppearance` (dark/light).
  - `examples/demo_tray.rs`: a runnable tray + popup demo
    (`cargo run --example demo_tray`).
  - Headless render snapshot test writing `target/muri-phase1-render{,-light}.png`.
  - New deps: `tiny-skia`, `cosmic-text` (portable), and macOS-gated `winit`,
    `softbuffer`, `raw-window-handle`, `objc2`/`objc2-app-kit`/`objc2-foundation`.
  - Still `todo!()`/follow-up: flyout submenus (Phase 2), Windows + Linux
    backends, native screen-reader a11y (Phase 3), keyboard nav, animations.
- Initial crate scaffold (Phase 0): the complete public API surface as compiling
  types with `todo!()` renderer bodies — `Tray`, `ContextMenu`, `Menu`, `Item`,
  `Row`, `Segment`, `StyleRun`, `Align`, `Flex`, `Color`, `Font`/`FontFamily`/
  `Weight`, `Icon`, `Theme`/`ThemeSource`, `MenuOptions`, `Insets`, `Edge`,
  `LogicalPoint`, `MenuId`, `MenuEvent`, `Error`/`Unsupported`.
- Module structure toward the macOS-first implementation: a pure `menu` data
  model, `theme` (with semantic-color resolution), `layout` (flex/alignment
  resolution), `geometry`, `error`, a `render` scene-drawer trait, and a `tray`
  per-OS anchoring trait with macOS / Windows / Linux shim stubs.
- Unit tests for the pure data-model layer: menu building, `Flex`/`Align`
  resolution, semantic color/theme resolution, and the usagio `RowStyle` → muri
  mapping.
- `examples/usagio_menu.rs` rebuilds usagio's real tray menu through the API
  (provider headers with logos, flush-right colored percentages with no chevron
  column, per-account detail submenus, icons, and a greyed version tail).
- Standard open-source project files: `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`,
  `SECURITY.md`, issue/PR templates, and Dependabot config.
- GitHub Actions CI running `cargo fmt --check`, `cargo clippy -D warnings`, and
  `cargo build` on macOS.

[Unreleased]: https://github.com/MattJackson/muri/commits/main
