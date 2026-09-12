# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.12.12] - 2026-09-12

### Fixed

- **Dark glass menu density re-calibrated to the correct native target (#72).** An
  on-device re-measure of a real `NSMenu` (Time Machine) glass gave **lum 51**, not
  the 56 used before, so the flat-overlay alpha is bumped from 0.19 to
  `(69-51)/69 ≈ 0.26`. Verified on live over two backdrops: the menu now lands at
  lum 51 (matching native) and stays neutral (51,51,51, "blackish") even over a
  saturated warm wallpaper instead of letting the desktop bleed through. Live path
  only; no goldens.

## [0.12.11] - 2026-09-12

### Fixed

- **No more visible placeholder-menu flash on tray start (#76).** The 0.12.1 live
  native-menu metrics read popped a real `NSMenu` and tried to hide it
  (`alphaValue = 0` + off-screen) before it painted; on Tahoe the hide raced the
  first paint and flashed a "Row One/Row Two/Row Three" menu near the top-left on
  every process start. The `popUp` path is removed entirely — the metrics now come
  only from their accurate no-popup sources (version-gated corner radius 12pt
  Tahoe / 6pt pre-Tahoe, preset leading inset 14pt, and a `NSMenu.size` row pitch
  that is never displayed), so nothing is ever shown to measure. This also removes
  the last of the async use-after-free hazard behind #75 and drops the `dispatch2`
  dependency.

## [0.12.10] - 2026-09-11

### Diagnostic

- **Env-gated per-run text trace to diagnose the live-vs-offscreen bold/tracking
  divergence (#65/#66).** The live on-screen popup reportedly renders bold and SF
  tracking differently than the offscreen `render_menu_to_rgba` path, which no
  static trace explains. `draw_text` now prints each run's text, requested weight,
  letter-spacing, resolved face, and embolden decision when `MURI_DEBUG_TEXT` is
  set (inert otherwise) — to capture ground truth on the real live tray. Will be
  removed once the divergence is understood.

## [0.12.9] - 2026-09-11

### Fixed

- **Live System menu bold no longer downgrades to regular (#65).** On the live
  macOS System path, a variable menu font (SFNS) whose resolved face `fontdb`
  registered at a mid/heavy default weight (>= the light-weight floor) had its
  wght-axis instance dropped: `face_embolden` treated the registered *default*
  instance as "already bold" and returned `Embolden::None`, so a `Weight::Bold`
  row rendered identical to regular (measured ink ratio 1.00 vs the ~1.2–1.4×
  expected). `face_embolden` now checks the `wght` axis **first** — a variable
  face always instances up the axis for a bold request, regardless of its
  registered default weight; only genuinely static faces use the
  downgrade-detection path. Regression-tested with a heavy-default variable
  fixture.

## [0.12.8] - 2026-09-11

### Fixed

- **Dark menu density now matches native via a measured flat overlay (#72).**
  Research (WWDC25 "Meet Liquid Glass") confirmed a native `NSMenu` uses the
  *private* `NSGlassView`, which has no public equivalent, and that
  `NSGlassEffectView.tintColor` is a content-adaptive tone-map — not a darkener
  (a near-black tint made it *lighter*). So the dark menu keeps the
  `NSVisualEffectView(Material::Menu)` + `emphasized` base and adds a **flat black
  `CALayer` overlay** (the only linearly predictable darkness lever), composited
  normally on top of the vibrancy and under the raster. The overlay alpha is
  solved from measurement, not guessed: base `.menu`+emphasized reads ~lum 69 over
  a gray-128 desktop vs native's ~56, so alpha `= (69-56)/(69-0) ≈ 0.19` lands the
  menu at native density. Only for a dark menu on a Liquid Glass (Tahoe) host.
  (Live path only; no goldens; `DEVICE-VERIFY` — target ~lum 56 over gray-128.)

## [0.12.7] - 2026-09-11

### Fixed

- **Dark Tahoe menu now uses the vibrancy material, not glass, for native density
  (#72).** On-device measurement showed `NSGlassEffectView.tintColor` is only a
  color wash, not a darkness lever — driving it near-black actually *lightened* the
  glass (lum 89), and the public `NSGlassEffectView` can't reach the private
  `NSGlassView`'s dark-menu density. So a **dark** live menu now falls back to
  `NSVisualEffectView(Material::Menu)` + `setEmphasized(true)`, which measures
  closer to a native dark `NSMenu`; a **light** menu keeps the closer-matching
  glass. The dead tint lever is removed. (Live path only; no goldens; still
  `DEVICE-VERIFY` — target ~lum 56 over gray-128.)

## [0.12.6] - 2026-09-11

### Fixed

- **Dark glass menu density much closer to native (#72 follow-up).** An on-device
  re-measure over a gray-128 desktop (median of ~25k interior pixels) showed the
  0.12.4 tint (RGB 40,40,41 @ 0.55) still left the glass at lum ~80 vs native's
  ~56 — `NSGlassEffectView.tintColor` is a subtle wash, so a light half-alpha tint
  barely darkens it. The dark-menu tint is now driven much harder (near-black
  RGB 14,14,15 @ 0.9) to actually reach native `NSMenu` density. (Live path only;
  no goldens; tint still `DEVICE-VERIFY` — target ~lum 56 over gray-128.)

## [0.12.5] - 2026-09-11

### Fixed

- **CRITICAL: fixed a use-after-free crash in the 0.12.1 invisible metrics-read
  (#75).** The one-shot native-menu chrome read scheduled a main-queue block that
  captured the throwaway `NSMenu` as a raw pointer, then called `cancelTracking`
  on it. When that block ran *after* `popUp` had already returned and freed the
  menu, the message landed on the freed (and address-reused) object — crashing the
  host tray app on menu open with `-[NSISUnrestrictedVariable cancelTracking]:
  unrecognized selector`. The block now holds an **owning retain** of the menu for
  its whole lifetime (transferred via `Retained::into_raw`/`from_raw`), so the
  object is always live when messaged. Affected 0.12.1–0.12.4; upgrade is
  strongly recommended.

## [0.12.4] - 2026-09-11

### Fixed

- **Dark glass menu tint is now neutral, not blue (#72 follow-up).** A controlled
  re-measurement over a neutral desktop showed a native dark `NSMenu` is neutral
  gray (~sRGB 44,44,45, lum 44) and muri's untinted glass ~lum 52 — ~8 lum too
  light but *neutral*; the earlier "blue" reading was a dark-blue desktop bleeding
  through the translucent glass. The 0.12.2 dark-glass tint (blue-leaning
  31,34,40) is corrected to a neutral gray tint toward the measured native color.
  The washed light-mode text (#73) is the same-cause light symptom and stays
  handled by the opaque-text flatten. Comments corrected to drop the "blue"
  framing. (Live path only; no goldens.)

## [0.12.3] - 2026-09-11

### Added

- **Live macOS menu honors the OS "Increase Contrast" accessibility setting
  (#74).** The live System path now reads
  `NSWorkspace.accessibilityDisplayShouldIncreaseContrast` (analogous to the
  existing Reduce-Transparency read) and, when on, renders like a native `NSMenu`
  under Increase Contrast: an opaque background (the bulk fill covers the glass),
  max-contrast label text (pure white/black), and full-strength separators. The
  forced preset is unchanged. (A 1px panel border — a further native tell — is
  deferred until `Theme` grows a border field.)

## [0.12.2] - 2026-09-11

macOS live-menu text/glass fidelity, measured against a native `NSMenu` on Tahoe.

### Fixed

- **Light-on-dark menu text no longer renders too heavy (#71).** Gamma-correct
  linear-light AA compositing is polarity-*symmetric*, but macOS font smoothing is
  not — so light text on a dark menu overshot and adjacent letters' anti-aliased
  ink merged. Glyph coverage is now thinned by a font-smoothing curve **only** for
  light-on-dark pixels (per-pixel polarity: foreground lighter than the pixel
  beneath it); dark-on-light keeps the linear blend it already matched native
  with. Solid fills, separators, and icons are unaffected.
- **Light-appearance menu text is crisp again, not washed-out/blue (#73).**
  `NSColor.labelColor` carries alpha 0.85 (secondary 0.55); drawn straight over
  the see-through Clear glass, the desktop bled through the glyph ink. The live
  System path now flattens the text colors over the menu material and draws them
  opaque, so the backdrop only shows *between* glyphs like a native `NSMenu`.
- **Live glass menu density closer to native (#72).** On a Liquid Glass system the
  popup's `NSGlassEffectView` read lighter than a native `NSMenu`'s private
  `NSGlassView`; a dark menu now gets an appearance-aware tint toward the measured
  native menu color (the glass analogue of the vibrancy path's `setEmphasized`).

Note: the exact font-smoothing gamma (#71), glass tint alpha (#72), and text
substrate (#73) are `DEVICE-VERIFY` — tuned against Tahoe captures and refined
on-device. The forced `Theme::macos` preset is unchanged; only offscreen text
snapshots move, from the polarity-aware coverage curve.

## [0.12.1] - 2026-09-11

### Changed

- **macOS live menu chrome is now read live from a real `NSMenu`, not hardcoded
  (#67/#68).** On the live System path the corner radius (12pt on Tahoe), leading
  text inset (14pt), and row pitch (24pt) are read once from an actual `NSMenu` —
  the exact values AppKit lays it out at — then cached for the process, so muri
  auto-tracks whatever the running OS uses (e.g. a future patch that moves Tahoe's
  corner) with no version table to maintain. The read is genuinely invisible: it
  pops a real menu but hides its window (`alphaValue = 0` and off-screen) on the
  first event-tracking tick before it paints, reads `_cornerRadius` /
  `_NSMenuItemTextField` / `NSTableRowView` off the live tree, then cancels
  tracking. The version-gated constants remain as the fallback, so behavior can
  never regress if the live read is unavailable. The forced `Theme::macos` preset
  and its offscreen goldens are unchanged.

## [0.12.0] - 2026-09-11

First breaking release: the compat `tray_icon::Rect.size` type change (below) is
a major bump per SemVer. Also a batch of macOS live-popup fidelity fixes measured
against a real `NSMenu` on Tahoe (macOS 26), and the bundled OSS UI fonts are now
on by default.

### Breaking

- **`compat::tray_icon::Rect.size` is now a `PhysicalSize`, not a `(f64, f64)`
  tuple (F4).** It mirrors `tray-icon`'s `dpi::PhysicalSize` (like the existing
  `PhysicalPosition`), so the drop-in facade matches upstream's shape. Update
  `rect.size.0` / `.1` to `rect.size.width` / `.height`.
- **`bundled-fonts` is now a default feature.** A forced foreign look (e.g. a
  macOS menu on Windows) renders in its close OSS typeface out of the box. This
  embeds ~1.4 MB of TTFs; drop it with `default-features = false` (re-add
  `x11-popup` if you want the Linux styled popup).

### Fixed

- **Live macOS row pitch is now measured from a real `NSMenu` (#67).** The live
  System path reads AppKit's own computed `NSMenu.size` per-row pitch instead of a
  magic `1.82` ratio (kept only as a fallback). Matches the measured native 24pt
  rows on Tahoe.
- **Live macOS popup corner radius is version-aware (#67).** Tahoe (macOS 26+)
  enlarged the menu corner to 12pt (measured via `_cornerRadius` on a live
  `NSPopupMenuWindow`); pre-Tahoe keeps ~6pt. There is no stable public API for
  the live value, so it is a version-gated constant. The forced preset and its
  goldens are unchanged.
- **Live macOS menu no longer over-tightens letters (#66).** The negative SF
  tracking that made adjacent letters touch is dropped on the live System path (a
  native `NSMenu` adds no extra tracking beyond the font metrics). The forced
  preset keeps its frozen value.
- **macOS menu backdrop follows the OS material (#68).** On a Liquid Glass system
  (detected by `NSGlassEffectView` existing at runtime, not a hardcoded version)
  the popup is drawn on glass like a native Tahoe `NSMenu`; on earlier systems it
  keeps the `NSVisualEffectView(Material::Menu)` vibrancy, now emphasized to match
  native density.
- **macOS popup dismisses on a Space switch (#69).** A three-finger swipe /
  Mission Control Space change now closes the popup like a native `NSMenu`, via an
  `NSWorkspaceActiveSpaceDidChange` observer.
- **macOS popup shows the arrow cursor, not the I-beam (#70).** The arrow is now
  pushed/popped across the pointer entering and leaving the panel, so it stays
  authoritative instead of a transient `set()` the I-beam reasserted between
  moves.
- **Variable system-font bold no longer risks downgrading (#65).** The macOS
  system-font path detects a variable (`wght`-axis) regular and registers it as a
  single face, letting the render layer instance bold via the axis (#63) rather
  than searching for a discrete bold file. Covered by a new hermetic
  variable-font test.

### Internal

- Relocated macOS-only helpers (`pack_dual_face`, the live row-pitch helper) into
  `src/platform/mac.rs` per ADR-0002, clearing all cross-target dead-code
  warnings (verified via a Linux cross-check).

## [0.11.3] - 2026-09-11

### Fixed

- **Live macOS popup background now shows the `Material::Menu` vibrancy (#64).** A
  native `NSMenu` paints no bulk background over its vibrancy backdrop, but muri's
  live System-theme popup filled its whole rounded rect with the preset's
  ~0.80-alpha background *on top of* the `NSVisualEffectView(Material::Menu)`,
  masking ~80% of the blur and reading as flat gray. On the live System path with
  transparency enabled, the bulk fill is now fully transparent so the rounded
  vibrancy is the surface — matching native. The offscreen renderer and
  forced/preset/custom themes keep their translucent fill (no vibrancy backdrop
  exists there), so goldens are unchanged; the reduce-transparency path still goes
  opaque.

## [0.11.2] - 2026-09-11

Audit-hardening release before wider testing. A 10-lens code audit of the
0.10.6→0.11.1 change set (19 confirmed findings) — all non-breaking.

### Fixed

- **Unbounded font caches.** The process-lived, thread-local `FontStore`'s
  glyph-coverage, fallback-face, embolden, and primary-face caches had no cap and
  grew forever over a long-running tray session with varied menu text; each now
  has a documented cap with clear-on-overflow, matching the glyph/shaped caches.
- **Variable-font bold over-clamp.** For a variable face whose `wght` axis maxes
  out below the bold floor, the instanced weight could be pushed *above* the
  font's own axis max; the axis-max clamp is now applied last.
- **Compat checkbox identity.** An *unchecked* `CheckMenuItem` on the muda-compat
  facade produced a muri `Row` with `checked == None` (indistinguishable from a
  plain item — losing the checkmark-gutter reservation and the AccessKit
  `MenuItemCheckBox` role); it is now `Some(false)`.
- **Linux SNI leading SVG icons.** The native `com.canonical.dbusmenu` path (the
  GNOME default) silently dropped `Icon::Svg` leading menu-item icons; they are
  now rasterized to PNG, and the match is exhaustive.
- **Linux SNI shutdown ordering.** The tray drain loop discarded commands queued
  after a `Shutdown` in the same batch; it now applies the whole batch before
  tearing down.
- **Tray-install handshake ordering.** The Windows/Linux tray thread now installs
  its command-waker *before* signalling install success, closing a window where a
  command posted by a pre-obtained handle could go undelivered.

### Performance

- The text-measurement cache no longer allocates an owned key on a cache hit
  (hash-then-verify, matching the shaping cache), and the variable-font bold
  `ShaperInstance` is now memoized per (face, weight) instead of rebuilt per
  shape miss. Behavior unchanged.

### Internal / tests / docs

- Added regression tests for the checkbox identity, cache-cap eviction, wght
  clamp, struct-literal `min_width > max_width` (no panic), and a `compile_fail`
  guard pinning the frozen-compat surface (native-only styling must not resolve
  on the facade). Documented deliberate error swallows on the void ksni
  activation callback; assorted doc/comment corrections.

## [0.11.1] - 2026-09-11

Live-macOS OEM parity + the real Linux Wayland styled menu. Non-breaking.

### Fixed

- **Live macOS popup renders bold (#63/#56).** On modern macOS the system UI
  font is a single SF *variable* file, so `Weight::Bold` silently downgraded to
  regular on the live `ThemeSource::System` path (the offscreen forced-`MacOs`
  path was unaffected). muri now **instances the `wght` axis** (700) through both
  shaping (harfrust) and rasterization (swash) for a variable face, and falls
  back to **synthetic emboldening** for a static face with no weight axis — so
  `Row::bold()` renders bold in the live popup, matching the offscreen renderer.
- **Live macOS row metrics (#63).** The live System-theme popup was ~10% tighter
  than native `NSMenu`; the System path now scales its row height to the native
  pitch (only the live path — the forced-`MacOs` offscreen goldens are unchanged).
  `DEVICE-VERIFY(0.11.1)`: on-device pixel confirmation of the row pitch.
- **Arrow cursor over the popup.** The borderless popup could show the text
  I-beam when opened under a stationary pointer (the arrow was only enforced via
  key-window cursor rects + `mouseMoved:`). Added a `cursorUpdate:` handler +
  `NSTrackingAreaOptions::CursorUpdate`, which fires independent of key state and
  movement.

### Added

- **Real Linux Wayland styled menu (`wayland-styled`, experimental).** The
  scaffold is now a working `wlr-layer-shell` overlay presenter (full flyout
  stack, hover/keyboard/dismiss, `wl_shm` ARGB8888 blit) for wlroots + KDE; X11
  tray activation is delivered to muri's own popup (const-generic
  `MENU_ON_ACTIVATE` split); and the custom X11/Wayland popups expose their
  AccessKit tree over AT-SPI via `accesskit_unix` (behind `a11y`). GNOME-Wayland
  stays native dbusmenu. New deps are target-gated to non-macOS. Live-compositor
  behavior is `DEVICE-VERIFY` (verify on a real Wayland/X11 session).

## [0.11.0] - 2026-09-11

The API-shape release: the compat facade is frozen to a pure muda/tray-icon
drop-in, the native API converges on "one obvious way," errors become
structured, and Linux gains a runtime menu-presenter seam. **Breaking** — hence
the minor bump; a migration guide is in `README.md` / the migration doc.

### Breaking

- **`muda-compat` is now a frozen, pure bidirectional `muda`/`tray-icon` drop-in
  (#61).** All muri-only extensions were **removed** from the compat surface —
  `set_active`/`set_bold`/`set_value_color`/`set_value_runs`, `MuriRowExt`,
  `RowStyle`, the `StyleRun`/`Color`/`Weight` re-exports, `Submenu::set_icon`,
  `Icon::from_png`/`Icon::data`/`IconData`, `Menu::as_native`/
  `open_custom_with_options`, `TrayIconBuilder::with_options`/`with_theme`,
  `TrayIcon::set_theme`/`set_options`/`is_live`, and `TrayIconBuilder::build_result`.
  The facade now mirrors upstream exactly (both `s/muda/muri/` and `s/muri/muda/`
  hold). **All customization moves to the native API.** Compat `Icon` now exposes
  muda's own `from_rgba`/`from_path`.
- **Structured `Error` (EH-2).** `Error` is now `#[non_exhaustive]` with
  `TrayInstall`, `MainThread`, and `ThreadSpawn` alongside `Unsupported`/`BadIcon`/
  `Platform`, so consumers can `match` the failure kind (a `match` on `Error` now
  needs a `_` arm). The tray-install handshake surfaces Windows/Linux install
  failures as `Error::TrayInstall` via `Tray::spawn`.
- **Native API convergence (#62).** Renamed `Icon::from_png_bytes` → `from_png`
  and `from_svg_bytes` → `from_svg`; `Row::info()` is deprecated in favor of
  `Row::label_only`. One canonical path is now documented per task (see below).

### Added

- **`Row::bold()` / `Row::value_color()`** — ergonomic native styling for the
  common cases (the native home for the removed compat `set_bold`/`set_value_color`);
  per-run `StyleRun` remains the full-control escape hatch (#62).
- **"Native API: the one obvious way"** doc section (crate root) — a canonical
  path per task: build a menu, put content in a row, style a row, make an icon,
  pick a theme (#62).
- **`TrailingGutterPolicy` on `MenuOptions`** (`Auto`/`Always`/`Never`, mirroring
  the leading `GutterPolicy`) — reserve the trailing chevron column only when a
  submenu is present, so right-aligned content reaches the edge otherwise (#60).
- **Linux menu-presenter seam** — a runtime choice between muri's own styled X11
  popup, an **experimental** `wlr-layer-shell` styled popup on wlroots + KDE (the
  new off-by-default `wayland-styled` feature; scaffold with device-verify
  markers), and the native `dbusmenu` on GNOME-Wayland and as the universal
  fallback (ADR-0003).
- **macOS status-item icon + title** render side-by-side, icon leading (#58,
  shipped 0.10.8) — and OEM-fidelity golden regression tests (bold #56, macOS
  metrics #57) via the headless renderer.

### Notes

- The Linux Wayland layer-shell renderer and AT-SPI accessibility for the custom
  popups are scaffolded (`todo!("DEVICE-VERIFY")`) — they land in a follow-up
  once verified on a real Wayland session.

## [0.10.9] - 2026-09-11

Display-free rendering for CI. Non-breaking (additive).

### Added

- **Headless render-to-pixels API (#59).** `muri::render_menu_to_png(&Menu,
  &MenuOptions, scale) -> Vec<u8>` and `muri::render_menu_to_rgba(&Menu,
  &MenuOptions, scale) -> (Vec<u8>, u32, u32)` render a built menu straight to
  pixels **without** a tray, window, display, or TCC/Accessibility — for CI
  screenshots and cross-OS golden tests. The `Theme` is resolved from the
  `MenuOptions` `ThemeSource` internally (so forced `ThemeSource::{MacOs,Windows,
  Gnome}` produce the target-OS look from any host), routing through the same
  drawer/font path a live popup uses. Ships on default features, all OSes.

## [0.10.8] - 2026-09-11

macOS native fidelity, a paint-layer overhaul against the "attribute set but
silently not drawn" bug class, optional bundled OSS fonts, and render/raster
performance — all non-breaking (public API unchanged since 0.10.7).

### Added

- **Bundled OSS UI fonts (`bundled-fonts` feature, opt-in, off by default).**
  Vendors OFL metric/shape substitutes — **Inter** (→ San Francisco), **Selawik**
  (Microsoft's own OFL Segoe UI replacement), and **Cantarell** (GNOME) — wired as
  a fallback tier for forced cross-platform themes: the real OEM font is used when
  installed, else the bundled substitute, else a free host fallback (never the
  wrong-platform UI font). The proprietary originals are never redistributed.
- **macOS bold system face (#56).** The macOS backend now resolves the real
  **bold** San Francisco face (via `NSFontManager`) and registers regular + bold
  together, so `Weight::Bold` / per-run bold rows draw at bold weight instead of
  silently downgrading to regular. A cross-platform weight-downgrade detector
  guards against the whole silent-downgrade class.
- **macOS status item icon + title together (#58).** When both a drawable image
  and a non-empty title are set, the `NSStatusItem` button renders them
  side-by-side, **icon leading, title trailing** (`NSImageLeft`), matching native
  menu-bar extras.

### Changed / Fixed

- **macOS menu metrics tightened to NSMenu (#57).** Row height, insets, font size,
  and corner radius are pinned to documented native references, and SF UI
  **tracking now travels with the theme** — a forced macOS theme carries its
  letter-spacing on any host (Windows/GNOME presets carry their documented ~0),
  reconciled so the live-System path never double-applies.
- **Paint-layer overhaul.** All icon drawing funnels through one exhaustive
  `draw_icon` (no `_` wildcard — a new `Icon` variant is now a compile error);
  fixes a class of "set on the model, silently not rendered" bugs: leading `Svg`
  icons, trailing-icon column, row background fills, overlapping style runs
  (later wins), disabled rows now dim icon + checkmark (not just text), a NaN
  layout guard, and the `row_highlight` theme field is now actually read.
- **Structured tray-install errors surfaced (EH-1).** A synchronous tray-install
  handshake makes `TrayIconBuilder::build_result()` / `is_live()` reflect a real
  Windows/Linux (`Shell_NotifyIcon` / SNI) install failure instead of a false
  `Ok`; the failure is returned as an `Error::Platform` message naming the site.

### Performance

- **FontStore shared across popup opens.** The font database + shaping/glyph
  caches are built once per configuration and reused, instead of being rebuilt on
  every popup/hover open.
- **Fewer per-call allocations** on the shape/measure hot path (`segment_faces`
  scratch reuse; the measurement cache no longer allocates an owned key on a hit).
- **Opaque-destination fast path** in `blend_pixel` (skips the unpremultiply
  divide when the destination is already opaque; proven bit-identical to the
  general path, so rendered output is unchanged).

### Notes

- Machine-matchable structured `Error` variants were prepared but **deferred to a
  future intentional minor (0.11)** to keep 0.10.8 non-breaking
  (`cargo-semver-checks` green).
- Several macOS changes (bold face, metrics, status-item layout) carry
  `DEVICE-VERIFY(0.10.8)` markers for on-device pixel confirmation.

## [0.10.7] - 2026-09-10

OEM fidelity + ergonomics. Native-parity tracking, cursor-anchored context menus,
and a way to detect a failed tray install.

### Added

- **Font tracking (`Font::letter_spacing`).** Per-glyph tracking (letter-spacing)
  in logical points, threaded through shaping and metrics. The macOS `System`
  theme now applies a small **negative** tracking to San Francisco (a single tuned
  constant, `sf_ui_tracking`) so menu text matches native `NSMenu` tightness
  instead of reading slightly looser (#42). Default `0.0` — existing rendering is
  byte-identical. (Exact factor is device-verify.)
- **Cursor-anchored context menus (#1).** `Platform::cursor_position()` on all
  backends (macOS `NSEvent::mouseLocation`, Windows `GetCursorPos` + per-monitor
  DPI, X11 `QueryPointer`; `None` on Wayland). The muda-compat context-menu facade
  now opens a `None`-position menu **at the cursor** like muda, falling back to the
  screen origin only where the platform can't report the pointer.
- **Tray-install failure detection (#3).** `TrayIcon::is_live()` and
  `TrayIconBuilder::build_result()` — `build()` stays infallible (tray-icon
  parity), but a consumer can now detect (or get the error for) a tray that failed
  to install instead of a silent passive facade.

### Changed / documented

- **OEM menu mutual-exclusion.** Confirmed the *forward* direction (muri's popup
  dismisses when any native/OEM menu opens) is wired via the existing
  `NSMenuDidBeginTracking` observer. Documented that the *reverse* (force-closing an
  already-open foreign-app native menu when muri opens) is an inherent macOS
  limitation — muri's non-activating panel can't cancel another process's menu
  tracking without stealing focus, which would defeat its design.
- Named the Windows DPI/points magic numbers (`BASE_DPI` = 96, `POINTS_PER_INCH`
  = 72).
- `MeasureKey`/shaped-run cache keys include `letter_spacing` so tracked and
  untracked measurements of the same text can't collide.

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
