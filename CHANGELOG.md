# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Phase 4 — VoiceOver attach + Windows backend groundwork.**
  - Shared, unit-tested popup-placement math (`anchor` module): `place_popup`
    resolves a popup's top-left corner from an anchor rect, popup size, screen
    work area, and grow-[`Edge`], flipping to the opposite side when it would
    spill and clamping fully on-screen. Every backend obtains the anchor rect
    natively and calls this one function; the macOS backend now uses it in place
    of its inline placement. Exposed as `place_popup`.
  - **AccessKit platform adapter attached on macOS** (behind the `a11y` feature):
    the popup's winit window now hosts an `accesskit_winit::Adapter`, so the
    already-published `a11y` tree is exposed to NSAccessibility / VoiceOver. muri
    creates the window hidden, attaches the adapter, then shows it; forwards every
    popup window event to the adapter; serves the initial tree and pushes a fresh
    `TreeUpdate` (menu tree + focus + expanded flyout) on open, keyboard/mouse
    selection, and flyout open/close; and applies AccessKit `Focus`/`Click` action
    requests back onto the live popup (moving the highlight, opening a flyout, or
    dispatching a row). A new pure helper `a11y::locate` maps an AccessKit node id
    back to a `(top, child)` menu position (unit-tested). The wiring compiles and
    is exercised by the pure tests; a **live VoiceOver pass on a device remains
    the human-verified final hop**, and the flyout — a separate OS window — is not
    yet independently adapted (its items are present as nested nodes in the popup
    window's tree).
  - **Windows backend groundwork:** `WindowsAnchor` now really installs a
    notification-area icon (`Shell_NotifyIcon`/`NIM_ADD` against a message-only
    window it creates and tears down on `Drop`) and reports its on-screen
    rectangle via `Shell_NotifyIconGetRect`, converted to logical coordinates
    through the window's DPI (`GetDpiForWindow`). `WindowsAnchor::popup_origin`
    wraps the shared `place_popup` so the future Windows popup uses the exact
    placement macOS does. Built with `windows-sys` and compile-verified for
    `x86_64-pc-windows-msvc` (`cargo check`/`clippy --target`). The popup event
    loop and PNG→`HICON` decoding remain `todo!()` (the stock application icon is
    shown until then). The `unsafe`-forbid attribute now also exempts the Windows
    build.

- **Phase 3 — accessibility + keyboard navigation.**
  - Pure, tested keyboard-navigation state machine (`keynav` module): `handle_key`
    advances a `MenuFocus` (selected top-level row + optional open-flyout child)
    for a normalized `NavKey` and returns a `NavAction` for the backend. Arrows
    move the highlight (wrapping, skipping separators/headers/disabled/inert
    rows), Home/End jump to the bounds, Right/Enter open the selected submenu's
    flyout (focusing its first child), Left/Esc pop one level (Esc at the top
    dismisses the whole menu), Enter/Space activate a leaf row, and a typed
    character does type-ahead to the next matching row. Exposed as `handle_key`,
    `MenuFocus`, `FlyoutFocus`, `NavKey`, `NavAction`.
  - Live keyboard nav wired into the macOS popup: `Tray::run` translates winit key
    events into `NavKey`s and applies the state machine, keeping the keyboard
    highlight in sync with the mouse hover-stack (same `hovered`/flyout state) and
    opening/closing the real flyout window, dispatching a row's `MenuId`, or
    dismissing accordingly. The pure transitions are unit-tested; driving it on a
    live borderless popup is device-verified interactively.
  - Pure, tested accessibility-tree model (`a11y` module): `build_tree` maps the
    declarative `Menu` into a parallel `AxTree` of `AxNode`s (roles `Menu` /
    `MenuItem` / `MenuItemCheckbox` / `GroupLabel` / `Separator`) carrying
    accessible name, enabled, checked, `has_popup`/`expanded`, and 1-based
    set-position among focusable siblings; `focused_id` maps a `MenuFocus` onto
    the node the screen reader should announce; `set_expanded` keeps a submenu's
    open state truthful; `announcement` renders the spoken string. Exposed as
    `AxTree`, `AxNode`, `AxRole`, `AxId`, `build_tree`, `focused_id`,
    `announcement`, plus `Tray::accessibility_tree` / `ContextMenu::accessibility_tree`.
  - AccessKit bridge behind the `a11y` feature (`a11y::accesskit::tree_update`):
    converts an `AxTree` + focus into an AccessKit `TreeUpdate` (the data a
    consumer feeds to `accesskit_macos` → NSAccessibility / `accesskit_windows` →
    UIA). The pure tree is the design's "publish a parallel accessibility tree";
    attaching the platform adapter to the winit event loop and the VoiceOver/NVDA
    passes remain the device-verified final hop (see the roadmap).
- **Phase 2 — flyout submenus + render polish.**
  - Pure, tested flyout geometry (`flyout` module): `place_flyout` positions a
    child panel flush against its parent's right edge, top-aligned to the hovered
    row, flips to the parent's left when it would spill off the monitor, and
    clamps into the work area; `next_flyout` encodes the hover-stack transitions
    (hover a submenu parent to open/switch, hover a sibling to close, moving into
    the flyout or across the gap keeps it open). Exposed as `place_flyout`,
    `next_flyout`, `FlyoutPlacement`, `FlyoutSide`, `HoverTarget`.
  - macOS live flyout: hovering (or clicking) a submenu row opens a second
    softbuffer popup window beside it, painted by the same scene drawer with the
    same flush-right layout; hovering a different parent switches it, moving into
    the flyout keeps it open, clicking a leaf row fires its `MenuId` and closes
    the whole stack, and focus-loss dismissal is tracked across both windows so
    opening a flyout no longer dismisses the menu.
  - Render polish: bold section headers and bold rows now share the **same
    typeface** as regular rows. Previously `cosmic-text` matched a bold request
    against the generic sans to a different (monospace) face; muri now pins a
    concrete UI family verified to shape both regular and bold in-family.
  - Snapshot proof: `snapshot_flyout_parent_plus_child_is_saved` renders a parent
    panel with an open flyout and composites both panels (child to the right of
    its highlighted parent row) into `target/muri-phase2-flyout.png`.
  - `examples/demo_tray.rs` now exercises flyouts (per-account detail panels and
    a populated Settings submenu).
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
  - Still `todo!()`/follow-up: Windows + Linux backends, native screen-reader
    a11y (Phase 3), keyboard nav, animations.
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
