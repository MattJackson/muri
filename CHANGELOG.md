# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.6] - 2026-09-10

Hardening from a second full-codebase audit (10 lenses, fresh eyes). The audit
found **no** correctness, panic, or security defects in 0.10.5; these are the
robustness, performance, and honesty items it surfaced.

### Fixed

- **Linux tray leak on platform drop.** `LinuxPlatform` now implements `Drop`,
  shutting down a standalone `install_tray` SNI service on teardown. `ksni`'s
  blocking `Handle` has no unregistering `Drop`, so without this the D-Bus service,
  its thread, and the tray icon leaked until process exit (the re-install path
  already guarded this; the final drop did not).
- **macOS `Tray::run` now returns on shutdown.** When muri owns the blocking
  `NSApplication::run` loop, `TrayCommand::Shutdown` now stops it (gated so a
  spawned tray on the host's loop is never stopped) — matching the Windows
  (`WM_QUIT`) and Linux backends, so dropping the last `TrayHandle` ends `run()` as
  documented (#47). (Device-verify pending, like the rest of the macOS backend.)

### Performance

- **Primary-face resolution is memoized.** `resolve_face` ran a `fontdb` database
  query on every `measure_text`/`draw_text` call (once per segment per row, every
  repaint); it now caches per `(family, weight)` like the shaping/coverage caches,
  so a hover repaint no longer re-scans the system-font database.

### Docs / tests

- Corrected the compat context-menu trait docs (a `None` position falls back to the
  **screen origin**, not the cursor — muri has no cursor-query helper yet), the
  `TrayIconEvent` docs (icon-level pointer events are **not yet emitted** by the
  backend), `TrayIconBuilder::build`'s infallible-degrade behavior, the
  raster-module blend doc (gamma-correct, not the old sRGB `over`), and a wrong
  `png` type reference in `decode_png`.
- Strengthened the `open_custom_with_options` compat test to observe the options
  actually reaching the surface (it previously only checked the surface was routed
  Custom, which held regardless of the options).
- De-duplicated the forced-vs-native drawer construction across the macOS/Windows/
  X11 backends behind `RasterDrawer::for_menu_options` (drift guard).

## [0.10.5] - 2026-09-10

A large correctness + API-ergonomics wave: gamma-correct text rendering, genuine
per-OS forced themes (with the target OS font), a rich-content-row layout
primitive, runtime theme swapping, per-surface event correlation, and a broad set
of compat-facade and native-API ergonomics fixes.

### Added

- **#44 — rich content rows.** New additive layout primitive — `Stack` (with
  `Axis`, `Content`, `TextContent`, `Spacer`) and `Item::Content(Stack)` /
  `Menu::content` — for visually rich, still-OEM menus (an Apple-Weather-extra
  hourly strip, dashboards). Default text rows are unchanged; rich rows are
  non-interactive display stacks with a recursive measure/paint pass.
- **#54 — forced themes render the TARGET OS font.** `ThemeSource::MacOs/Windows/
  Gnome` now resolve their target UI family (Segoe UI / SF Pro / Cantarell), with
  a free metric-compatible fallback, and **never** silently use the host's own UI
  font. Proprietary faces still need to be installed for pixel parity (documented).
- **#45 — compat escape hatch + runtime theme swap.** `Menu::as_native`,
  `Menu::open_custom_with_options`, `TrayIconBuilder::with_options/with_theme`, and
  live `TrayHandle::set_theme`/`set_options` (+ compat `TrayIcon::set_theme/
  set_options`) — enables an in-menu "Preview theme" switcher and single-machine
  cross-OS theme preview.
- **#46 — `MainThreadMarker`.** `Tray::run`/`spawn` now take a `!Send` main-thread
  marker, turning the macOS `NSStatusItem` affinity from a doc note into a
  compile-time contract.
- **#47 — `TrayHandle` auto-removal.** `TrayHandle` now implements `Drop`, posting
  `Shutdown` when the last handle of a family drops (matching `tray-icon`).
- **#48 — anchor-rect accessors.** `Tray::anchor_rect` and best-effort cross-thread
  `TrayHandle::anchor_rect`; compat `TrayIcon::rect()` now returns the real native
  rect (was `None` on every platform).
- **#49/#50 — row-build ergonomics.** `Segment::grow/trailing_value/run`,
  `Row::label_value`, `StyleRun::from_byte_range`, and `Row::label_only` (plus
  doc notes that a submenu/section-header label's id & checked state are ignored).
- **#51 — per-surface event correlation.** `MenuEvent` gains a `source: SurfaceId`;
  `Tray`/`ContextMenu`/`Popup` expose `surface_id()` so multi-surface consumers can
  tell a tray activation from a context-menu one on the global channel.
- **#52 — `MenuOptions` builder.** `.theme()/.min_width()/.max_width()/.gutter()`
  with NaN/negative/`min>max` clamping.
- **#53 — compat ergonomics.** `Icon::data() -> IconData` (PNG-backed icons no
  longer report a zero-size RGBA), and a `MuriRowExt` trait grouping the
  muri-extension row setters.
- **#55 — cross-OS theme conformity goldens.** Host-independent golden images pin
  that each forced theme renders its target-OS metrics on any CI runner.

### Fixed

- **#42 — gamma-correct text rendering.** Glyph/edge compositing now blends in
  linear light instead of directly on sRGB-encoded bytes. Naive sRGB blending left
  anti-aliased edges too dark, so dark-on-light menu text read noticeably heavier
  than a native CoreText/Quartz menu; text now matches the native weight. (A minor
  size-dependent tracking difference from CoreText remains and is documented.)
- **#33 — Windows nested popup sessions no longer conflate events.** Each popup
  session gets a unique id (packed into `GWLP_USERDATA`) and drains only its own
  tagged events, so a tray handler that opens a nested context menu can't steal the
  outer popup's hover/click events. (Device-verify pending on real Windows.)
- **#35 (carried), #38 — Linux** popup device-coordinate clamping and tray
  re-install cleanup (from prior waves, verified in this suite).

### Hardening (pre-release audit)

- **Check-column reservation** now fires for any *checkable* row
  (`checked.is_some()`), matching the documented "`Some(true/false)` shows a check
  column" contract — a menu whose checkable rows were all currently unchecked
  previously reserved no gutter, so its text jumped right the instant one toggled on.
- **`TrayHandle` shutdown-on-last-drop is now race-free.** The "last clone" signal
  moved from an `Arc::strong_count == 1` check (which two final clones dropping
  concurrently could both skip, leaking the tray) to an `Arc<HandleFamily>` drop
  guard that fires exactly once.
- **Gamma-correct blend uses a lookup table** instead of a per-pixel `powf`, with
  an exact fast path for fully-opaque blits — removing the hot-path cost the #42
  change would otherwise have added to every popup open.

### Changed

- **#21 — X11 popup path is feature-gated.** The `x11-popup` feature (default-on)
  guards the `x11rb` dependency, so a tray-only Linux consumer can build
  `default-features = false` and drop `x11rb` + `x11rb-protocol` + `gethostname`.
- **#20 — `ksni`** builds with `default-features = false` + `async-io`, dropping an
  unused `tokio` runtime.
- **#39 — dependency-skew notes** for the `a11y` Windows `windows`/objc2 0.5 stack.
- **#41 — macOS theme metrics** nudged closer to native `NSMenu` (row height 22,
  padding, column gap, 13pt system font).
- **#43 — configurable gutter policy** (`GutterPolicy::Auto/Always/Never`) via
  `MenuOptions.gutter`.
- **#25–#32 — compat-facade correctness** (per-span value colors, bold/checkmark
  decoupling, submenu leading icons, `CheckMenuItem` extensions, `init_for_*` /
  `show_context_menu_for_*` fixes, GTK signatures) — from the compat wave.

## [0.10.4] - 2026-09-10

### Fixed

- **#36 — `render_menu` no longer panics** when a consumer sets `min_width >
  max_width` (`f32::clamp` requires `min <= max`); the floor now wins.
- **#37 — focus-loss dismissal defeated while a submenu was open.** `open_flyout`
  inserted a synthetic entry into the focus set that was never cleared (flyouts
  use `orderFrontRegardless` and never take key), so `focused` stayed non-empty
  after the popup resigned key — defeating the #17 dismissal. The focus set now
  tracks only the real key window (the popup).
- **OEM row alignment** — when a menu has checkmarks, muri now reserves a shared
  leading gutter so **checked and unchecked rows align their text** (native
  `NSMenu` look), instead of a checkmark indenting only its own row. Menus with
  only a section-header icon (no checkmarks) stay per-row inline (#16 preserved).
  macOS theme spacing nudged toward the native menu (taller rows, more padding).

### Performance

- **#22 — the system-font database is scanned once, not per popup open.**
  `load_system_fonts` (the dominant open cost) now runs once per thread and the
  scanned `Database` is cloned (a cheap metadata copy — fontdb `Arc`s the font
  data) for each drawer.
- **#23 — the menu is shaped once per open, not twice.** The offscreen measuring
  drawer is now reused as the panel's drawer (macOS + Windows; Linux already did),
  so its warm shaping/glyph caches carry into the first paint instead of a second
  cold drawer re-shaping every row.

## [0.10.3] - 2026-09-10

### Added (muda-compat)

- **#18 — active-row marking.** `MenuItem`, `IconMenuItem`, and `Submenu` gain a
  muri-only `set_active(bool)`: an active row renders **bold** with a leading
  **checkmark** (the 0.5.x active-account look). muda's `Submenu` has no checked
  state, so this is the compat channel for expressing it. Bold is applied as a
  whole-segment `StyleRun` weight, preserving the OS point size.
- **#19 — per-span value color.** The same three types gain `set_value_color(
  Option<Color>)`, which tints the trailing `\t` value segment (severity coloring,
  e.g. `Color::SystemRed`/`SystemOrange` for high usage). `Color` is re-exported
  from `muri::compat::muda`. Combined with `set_active`, the value renders bold +
  colored.

Both map onto muri's native painter (checkmark + bold + per-segment color); they
are additive muri extensions, not part of muda's API, so existing callers are
unaffected.

## [0.10.2] - 2026-09-10

### Fixed

- **macOS hover latency (laggy highlight)** — the popup swapped its `CALayer`
  contents with no `CATransaction`, so Core Animation ran its default ~0.25s
  implicit cross-fade on every hover, making the highlight feel mushy/laggy. The
  present path now wraps the contents update in a `CATransaction` with actions
  disabled, so highlight changes are instant (matching the native menu). **The
  headline fix.**
- **macOS text-cursor over the menu (#issue)** — a borderless popup inherited the
  I-beam cursor. `MuriView` now sets the arrow cursor (via `resetCursorRects` and
  on `mouseMoved:`).
- **Submenu didn't collapse when the pointer left the menu (onto the desktop)** —
  added `mouseExited:` handling with a global-cursor geometry guard: leaving all
  panels collapses open flyouts and clears the highlight (the popup stays open),
  while crossing into a child flyout is preserved.
- **#16 — no global leading gutter.** A single icon row no longer indents every
  other row: the leading icon/checkmark is **per-row inline content** drawn at the
  shared left x, so icon-less rows are not pushed right. (A shared checkmark
  column becomes an explicit opt-in, not implicit.)
- **#17 — mutual exclusion / focus-loss dismissal (macOS).** The non-activating
  popup now becomes key (`needsPanelToBecomeKey`) **without activating the app**,
  reviving `windowDidResignKey`-driven dismissal — the menu closes when focus is
  lost to Spotlight, an in-app search field, another app, or another (OEM) menu.
  Flyouts use `orderFrontRegardless` (never take key), so opening a submenu does
  not resign the popup.

### Added

- **Per-OS themes + a richer [`ThemeSource`]** — three genuinely distinct native
  looks (`Theme::macos` / `windows` / `gnome`, each dark/light with its own
  metrics + palette) selectable independent of the host, plus built-in
  [`Preset`]s. `ThemeSource` is now `System(ThemeMode)` (match the host OS — the
  only source that receives live OS injection), `MacOs`/`Windows`/`Gnome(ThemeMode)`
  (force a platform look on any host — a macOS app can render a Windows menu),
  `Preset(Preset)` (`OldSchoolTerminal` / `HighContrast` / `Solarized` / `Nord`),
  and `Custom(Box<Theme>)`. `ThemeMode` is `Auto` (follow OS light/dark) / `Light`
  / `Dark`.
  - **BREAKING:** the old `ThemeSource::{FollowSystem, Light, Dark}` /
    `Custom(Theme)` variants are replaced. Migrate: `FollowSystem` →
    `System(ThemeMode::Auto)` (the new `Default`), `Dark` →
    `System(ThemeMode::Dark)`, `Light` → `System(ThemeMode::Light)`,
    `Custom(t)` → `Custom(Box::new(t))`.
- **Live macOS menu colors** — `System(..)` on macOS reads `labelColor` /
  `secondaryLabelColor` / `separatorColor` from `NSColor` under the current
  appearance (`Platform::system_palette`), on top of the live accent.
- **Real macOS system font (SF)** — the system menu font is resolved to its
  actual on-disk file via CoreText's `kCTFontURLAttribute`, so fontdb loads the
  genuine SF face instead of the bundled fallback; the OS menu **point size** is
  applied to the theme fonts.
- **OS transparency awareness** — `Platform::transparency_enabled` (macOS
  Reduce-Transparency accessibility flag; Windows `EnableTransparency`); when the
  user disables transparency, a `System(..)` theme drops its translucent
  background to a solid fill (`Theme::make_opaque`).

### Performance

- The raster framebuffer is **reused** across frames when the popup size is
  unchanged (the common per-hover repaint) instead of reallocating a fresh buffer
  each frame (`Framebuffer::reset`).

## [0.10.1] - 2026-09-10

### Documentation

- **Linux SNI/AppIndicator native-menu layout boundary (#15)** — documented that
  the Linux tray menu is the `com.canonical.dbusmenu` menu **rendered by the host
  (GNOME Shell / KDE)**, not muri's painter: it has no right-aligned tab-stop
  column (a `label\tvalue` row is flattened via `Row::accessible_name`) and the
  host decides which side a leading icon draws on. muda/tray-icon share this host
  menu on Linux and the same limitation. Callers needing muri's exact OEM row
  layout on Linux should use `ContextMenu::open_at`, which renders through muri's
  X11 styled popup and honors every `Segment`/`Flex`/`Align` — identical to the
  macOS/Windows tray popup.

### Tests

- Regression tests locking the #15 cases at the layers muri actually renders: the
  muda→muri conversion produces a **leading** `Icon::Png` for a disabled
  `IconMenuItem` and `[Grow, Right]` segments for a `label\tvalue` `Submenu`, and
  the painter draws the icon in the left gutter and right-aligns both `\t` values
  to one shared column. No runtime behavior change.

## [0.10.0] - 2026-09-10

### Added

- **`Icon::Svg` now renders** — a minimal, dependency-light SVG rasterizer for a
  restricted subset (paths `M/L/H/V/C/S/Q/T/A/Z`, `rect`/`circle`/`ellipse`/
  `line`/`polygon`/`polyline`, `fill`/`stroke`/`fill-rule`/`opacity`/`transform`,
  named + hex + `rgb()` colors), built on **`zeno`** — the pure-Rust AA path
  rasterizer already in the tree via `swash`, so **zero net-new crates** (no
  `tiny-skia`, no `resvg`, no second PNG codec). Wired into the shared icon path
  (`decode_icon_bytes` = PNG-then-SVG) so SVG works for menu-row leading icons and
  the tray icon on all three backends (Linux SNI ARGB32, Windows `HICON`, macOS
  `NSImage` via a PNG re-encode). Filters, `<text>`, gradients, clips/masks are
  out of the subset — ship those as PNG. MSRV stays 1.87; MIT.

## [0.9.7] - 2026-09-10

### Fixed

- **OS accent on Windows and Linux (#14)** — `Color::Accent` (selection,
  checkmarks) now follows the system accent on Windows (`DwmGetColorizationColor`)
  and Linux/GNOME (gsettings `accent-color`, GNOME 47+ named accents), matching
  the macOS path. muri already tracked dark/light and injected the macOS accent +
  vibrancy by default (`ThemeSource::FollowSystem`), and the OS menu font landed
  in 0.9.6 (#10), so this rounds out the "read the live OS appearance" parity.
  Deferred refinements: applying the OS menu **point size** and the OS
  label/separator colors to the theme. On-device appearance is
  `DEVICE-VERIFY(0.9.7)`.

## [0.9.6] - 2026-09-10

Native-menu OEM parity on macOS: the menu renders in the real OS UI font and
dismisses like a native menu. On-device appearance/interaction is
`DEVICE-VERIFY(0.9.6)`.

### Added

- **`FontFamily::System` resolves to the OS native menu font (#10)** — a new
  per-OS `Platform::system_menu_font()` supplies the host UI face (Segoe UI on
  Windows via `SPI_GETNONCLIENTMETRICS`; the GNOME `font-name` on Linux;
  `NSFont menuFontOfSize:0` on macOS) which the renderer registers into `fontdb`
  and pins `FontFamily::System` to (`RasterDrawer::new_native`). Falls back to the
  previous installed-font discovery when unavailable, so it never regresses.
  Windows/Linux resolve the real UI family by name; macOS is best-effort (SF Pro's
  protected file still needs CoreText URL loading — future work).
- **macOS: native-menu dismissal (#11)** — the popup now closes on a click in
  another app / the desktop (a global `NSEvent` monitor, which a non-activating
  panel's `resignKey` never delivered) and when any other menu opens or the app
  resigns active (`NSMenuDidBeginTracking` / `NSApplicationDidResignActive`
  observers) — one menu open at a time. Watchers are unregistered on close (no
  leaks). Windows/Linux already dismissed on outside click.

## [0.9.5] - 2026-09-09

macOS menu-parity + performance: menu clicks now work on macOS, the menu paints
~7× faster on hover, the native two-column tab-stop layout is preserved, plus a
second-round code-audit sweep.

### Fixed

- **macOS: menu clicks now reach every `muda-compat` consumer (#13)** — the
  macOS tray dispatch ran only the per-surface `on_click` and never projected the
  activation onto the global `MenuEvent` channel. The facade sets no `on_click`
  (consumers poll `MenuEvent::receiver()`), so every facade menu item was inert
  on macOS (Quit didn't quit). The dispatch now emits after the handler, matching
  the Windows pump.
- **Menu repaint is ~7× faster (hover no longer lags)** — a hover-highlight
  re-ran the full text pipeline: recompiling harfrust `ShaperData` per run, and —
  the dominant cost — re-scanning the entire system font DB for fallback glyphs
  and re-parsing each font's cmap, on every frame. Now the compiled shaper data,
  per-`(face,char)` coverage, per-`(char,weight)` fallback decision, and shaped
  runs are all cached across frames. Measured on a heavy menu: warm repaint
  187 ms → 27 ms.
- **`muda-compat`: native two-column tab-stop layout + active checkmark (#12)** —
  a `label\tvalue` label now splits into a grow-left segment plus a right-aligned
  trailing column (muda's NSMenu tab stop), and a checked `CheckMenuItem` renders
  the leading checkmark.
- **`Icon::from_rgba` rejects zero dimensions** (was accepted, then silently
  dropped by the encoder — a success that never drew).
- **`StyleRun` span check uses saturating add** (public `start`/`len` fields
  could overflow `usize` and panic under debug overflow checks).

### Performance

- The `muda-compat` RGBA→PNG icon encode is cached (`compat::encode_rgba_cached`)
  so an unchanged logo encodes once and returns a stable `Arc`, restoring the
  render layer's decode-cache hits across `set_menu` ticks.

### Internal

- macOS `Drop` delegates to `remove()`; the Linux dead `Shutdown` match arm gains
  a `debug_assert`; two stale "D6" doc references corrected.

## [0.9.4] - 2026-09-09

The last menu-parity fix for the first adopter, plus release-gate code-audit
fixes (a 10-lens audit of the 0.9.x tray/facade work).

### Added

- **`Tray::shutdown` via `TrayHandle::shutdown()` + `TrayCommand::Shutdown`** — a
  best-effort teardown that removes the OS status item and ends the backend run
  loop (and the spawned `muri-tray` thread).

### Fixed

- **`muda-compat`: `IconMenuItem` raw-RGBA leading icons now render (#9)** — a
  provider logo on a menu row (supplied via `Icon::from_rgba`, the only compat
  icon constructor) was dropped in the custom-surface translation, so header rows
  showed text only. The RGBA is now encoded to PNG (`render::encode_rgba_png`) and
  carried as an `Icon::Png` leading image — the same bridge the tray icon uses,
  extended to menu items. (`NativeIcon` still maps to `Icon::Symbol`.)

- **`muda-compat`: dropping a `TrayIcon` now removes the OS tray icon** — the
  facade spawned a background tray thread and discarded it with no `Drop`, so
  dropping a `TrayIcon` leaked the thread and the live `Shell_NotifyIcon` / SNI
  item until process exit (and diverged from `tray-icon`, which removes its icon
  on drop). `TrayIcon` now has a `Drop` that posts `Shutdown`; Windows quits its
  pump (→ `NIM_DELETE`), Linux calls ksni `Handle::shutdown()` (dropping the
  handle alone unregisters nothing), macOS removes the `NSStatusItem` explicitly
  (its `AppState` is parked in a thread-local, so `Drop` never fired).
- **`Icon::from_rgba` no longer panics on overflowing dimensions** — `width *
  height * 4` used unchecked `usize` arithmetic, panicking under the default debug
  overflow checks (and wrapping to a wrong bound in release) for pathological
  sizes from untrusted metadata. Now checked; overflow is a rejected icon.
- **Windows: the tray waker is cleared when the pump exits** — a `TrayHandle`
  outliving `run_event_loop` could `PostMessageW` a destroyed owner window.

### Documentation

- **`Icon::Svg` is documented as not-yet-rendered.** muri ships no SVG rasterizer
  (minimal-deps ADR), so `Icon::Svg` silently drew nothing on every backend while
  the README/API advertised "SVG rasterized per-DPI." The docs (README,
  `Icon`/`from_svg_bytes`) now say SVG isn't wired up yet — supply PNG — and the
  backends treat `Icon::Svg` uniformly as "no image."

### Internal

- `render::encode_rgba_png` is now `pub(crate)` (was accidentally public).
- Shared `platform::spawn_tray_thread` helper de-duplicates the Windows/Linux
  `spawn_tray` bodies. Corrected doc/changelog references that mislabeled the
  facade tray-RGBA fix as spec divergence "D6" (D6 is `NativeIcon`/`Icon::Symbol`
  rendering). Added regression tests for `TrayHandle`/facade command posting.

## [0.9.3] - 2026-09-09

Patch release in the 0.9.x testing cycle: a **text-title status item** so apps
that show live menu-bar text (e.g. usagio's "45%") can migrate macOS off
`tray-icon` — the remaining part of #8 (its parts 1 and 2 shipped in 0.9.2). The
macOS menu-bar render is `DEVICE-VERIFY(0.9.3)`.

### Added

- **`Tray` text title** — `Tray::title()` (builder), `Tray::set_title()` /
  `Tray::title_text()`, plus `TrayHandle::set_title()` (and a `TrayCommand::SetTitle`)
  for live updates. On macOS it is drawn on the `NSStatusItem` button as menu-bar
  text; on Windows/Linux the notification area has no free-text label, so it is
  retained but not drawn.

### Fixed

- **macOS status item now renders icon + title coherently (#8)** — the button
  rendering is unified (`set_status`): a valid PNG/SVG is the image, a non-empty
  title is the button text (composing with the image), and the bullet placeholder
  shows only when there is neither — so a **text-only** status item (a live "45%"
  with no icon) is no longer overwritten by the placeholder `●`.
- **`muda-compat`: `with_title` / `set_title` reach the drawn tray** — previously
  the facade only stored the title string; it now flows to the live tray via the
  handle.

## [0.9.2] - 2026-09-09

Patch release in the 0.9.x testing cycle: the `muda-compat` tray facade now
installs a **live** OS tray, so the icon actually appears — the root cause behind
the first adopter's Linux (#6) and Windows (#7) "tray icon does not appear"
reports. The on-device appearance is `DEVICE-VERIFY(0.9.2)` (verified on real
Windows and GNOME desktops as part of the 0.9.x cycle).

### Fixed

- **`muda-compat`: the facade never drove a platform tray (#6, #7)** —
  `TrayIconBuilder::build()` built a muri `Tray` but never ran any backend, so no
  OS tray icon was ever registered on any platform. `build()` now installs a live
  tray without blocking, via a new non-blocking `Tray::spawn()` (a
  `Platform::spawn_tray` seam): Windows and Linux run the native pump on a
  dedicated background thread; macOS installs the status item on the main thread
  and relies on the host's `NSApplication` run loop (best-effort). The facade
  drives it through a `TrayHandle` (`set_icon`/`set_menu`/`set_tooltip`/
  `set_visible` post to the running tray).
- **`muda-compat`: the tray icon's RGBA never reached the drawn tray** — the
  facade carried the icon as raw RGBA but built the tray with a placeholder
  symbol, so the Linux SNI `icon_pixmap` / Windows `HICON` were empty and hosts
  dropped the item. The RGBA is now encoded to PNG (`render::encode_rgba_png`) and
  handed to the tray as `Icon::Png`, so the facade's icon reaches the drawn tray.
  (This was a facade-specific gap, distinct from the spec's `NativeIcon`/
  `Icon::Symbol` divergence D6, which concerns menu-item glyph rendering.)

### Added

- **`Tray::spawn()`** — a non-blocking counterpart to `Tray::run()` that installs
  the tray and returns a `TrayHandle`, for hosts that own their own event loop.
- **`render::encode_rgba_png()`** — encode straight-alpha RGBA8 pixels to PNG
  bytes (the bridge from a raw-RGBA icon to muri's encoded-bytes `Icon`).

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
