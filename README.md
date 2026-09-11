# muri

[![CI](https://github.com/MattJackson/muri/actions/workflows/ci.yml/badge.svg)](https://github.com/MattJackson/muri/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/muri.svg)](https://crates.io/crates/muri)
[![docs.rs](https://img.shields.io/docsrs/muri)](https://docs.rs/muri)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.87-blue.svg)](https://www.rust-lang.org)

**Menu Utilities for Rust Interfaces** — a cross-platform, fully-styleable
tray-icon + popup-menu system for Rust. Think "a better `muda` + `tray-icon`":
muri owns the tray icon, the styled popup, *and* the anchoring, and draws **one
consistent custom appearance on every OS** instead of delegating to native
menus.

## Install

muri is published on crates.io. It is **pre-1.0** (`0.10.x`), so the public API
may still change between releases while the three backends are device-verified and
the surface stabilizes — pin at least the minor version:

```toml
[dependencies]
muri = "0.10"
# with accessibility:            muri = { version = "0.10", features = ["a11y"] }
# migrating from muda/tray-icon: muri = { version = "0.10", features = ["muda-compat"] }
# bundled OSS UI fonts for forced cross-platform themes:
#                                muri = { version = "0.10", features = ["bundled-fonts"] }
# tray-only Linux, no styled X11 popup (drops the x11rb stack):
#                                muri = { version = "0.10", default-features = false }
# or track the latest unreleased work from git:
# muri = { git = "https://github.com/MattJackson/muri" }
```

> **Status: all three backends implemented; on-device verification ongoing.**
> muri draws its own styled popup on **macOS** and **Windows**, and installs a
> native menu on **Linux**. `Tray::run` installs the OS status item, opens the
> menu, opens flyout submenu panels, follows dark/light, and supports **keyboard
> navigation** (arrows / Home-End / Right-Left / Enter-Space / Esc / type-ahead)
> over the same hover-stack the mouse drives. muri also publishes a **parallel
> accessibility tree** (`Tray::accessibility_tree`) that maps the menu onto
> menu/menuitem roles with name, checked, enabled, submenu-expanded, and
> set-position, and (behind the `a11y` feature) attaches an AccessKit platform
> adapter to the popup window so that tree is exposed to NSAccessibility /
> VoiceOver (macOS) and UIA / NVDA / Narrator (Windows). The **Linux** tray is a
> native `com.canonical.dbusmenu` menu (host-rendered — see below), with a
> pointer-anchored styled `ContextMenu` available on X11.
>
> Every backend compiles + is clippy-clean on its target in CI, and the pure
> cross-platform core is unit-tested green on every OS; what remains is exercising
> each backend, and the screen readers, on real hardware. See the [platform
> matrix](#platform-support-honest-matrix) for exactly what is verified vs.
> pending.

## Why muri exists

Native menus (`NSMenu`, Win32 `HMENU`, GTK `MenuItem`) draw their own pixels, so
you cannot restyle them: no custom fonts/colors, no true multi-column alignment,
and always a reserved chevron/submenu column that prevents values from sitting
flush at the right edge. `muda` is a thin data-model sync over those native
objects — there is no drawable layer to fork.

muri instead draws the menu itself, on a CPU raster surface, identically on every
platform. That single owned surface is what makes these possible:

- **True left / center / right alignment** and multi-column `label … value` rows.
- **Flush right-aligned values with no reserved chevron column.**
- **Arbitrary colors, fonts, and embedded logos** (PNG or a restricted **SVG**
  subset — paths/shapes/solid fills, rasterized by muri's own zeno-backed layer).
- **Styled nested submenus** (flyout panels beside the row).
- **One consistent look** that still **follows OS dark/light + accent** by
  default, and is fully overridable via a theme API.

## Design decisions (fixed)

- **Name:** muri. **License:** MIT.
- **One consistent custom-drawn appearance** across all OSes — not native-per-OS
  chrome.
- **Rendering stack:** text is shaped and rasterized with
  [`fontdb`](https://docs.rs/fontdb) (font discovery) +
  [`harfrust`](https://docs.rs/harfrust) (HarfBuzz-project shaper) +
  [`swash`](https://docs.rs/swash) (glyph rendering); the restricted `Icon::Svg`
  subset is filled with [`zeno`](https://docs.rs/zeno); everything is composited
  by muri's own in-house CPU `Framebuffer` blitter, with [`png`](https://docs.rs/png)
  the only image codec. No general-purpose 2D crate, no GPU. Windowing is native
  per-OS (see below). Minimal deps, tiny binary, no GPU warm-up, instant popups,
  full pixel control.
- **muri owns the whole stack:** the tray icon, the styled popup, and the
  anchoring (a full `muda` + `tray-icon` replacement).
- **Accessibility is first-class:** native a11y APIs are wired over the
  custom-drawn widget tree via AccessKit — NSAccessibility (macOS), UIA (Windows),
  AT-SPI (Linux, via the native menu) — behind the `a11y` feature, targeted to be
  on by default at 1.0.
- **v1 OS order:** macOS → Windows → Linux (all three now implemented).

## Feature flags

muri's default surface is `default = ["x11-popup"]`; the other integrations sit
behind opt-in flags. All ship today.

| Flag | Default | Description |
|------|---------|-------------|
| `x11-popup` | **on** | Linux only: the styled, pointer-anchored `ContextMenu::open_at` popup, via the pure-Rust `x11rb` X11 client. On by default for back-compat and `muda-compat` parity; a tray-only consumer can `default-features = false` to drop `x11rb` + `x11rb-protocol` + `gethostname` and keep just the SNI tray. Inert on non-Linux targets. |
| `a11y` | off | AccessKit screen-reader bridge over the raster backend (`accesskit_macos` → NSAccessibility, `accesskit_windows` → UIA; Linux via the native menu / AT-SPI). Ships today; **planned to be on by default at 1.0**. |
| `muda-compat` | off | A `muda` / `tray-icon` drop-in compatibility facade (`compat::muda` / `compat::tray_icon`) so existing callers can migrate with minimal churn. Maps entirely onto muri's native model — it does **not** pull in the real `muda` / `tray-icon` crates (see [Migrating from muda](#migrating-from-muda)). |
| `bundled-fonts` | off | Embeds freely-redistributable OSS UI-font substitutes (OFL **Inter** for SF Pro, Microsoft's own OFL **Selawik** for Segoe UI, genuine **Cantarell** for GNOME) so a *forced* cross-platform theme (`ThemeSource::MacOs`/`Windows`/`Gnome`) can render in a metric-compatible face when the real OEM font isn't installed on the host — never silently drawing the wrong-OS host font. OFF by default to keep the crate small; the TTFs are `include_bytes!`-embedded only when on. See [`assets/fonts/README.md`](assets/fonts/README.md). |

## Quick example

```rust
use muri::{Tray, Menu, Row, Segment, Align, Flex, Color, Icon, Font, Weight};

let menu = Menu::new()
    .section_header(Row::info().label("Claude"))
    .row(
        Row::new("switch:claude:me@x.com")
            .checked(true)
            .leading(Icon::Checkmark)
            .segments(vec![
                // label Grows to eat the gap; value is flush-right with no chevron column
                Segment::new("me@example.com").flex(Flex::Grow).font(Font::system(13.0, Weight::Bold)),
                Segment::new("47% / 89%").align(Align::Right).color(Color::SystemRed),
            ]),
    )
    .separator()
    .row(Row::new("quit").label("Quit"));

let tray = Tray::new(Icon::from_png_bytes(include_bytes!("icon.png").as_slice()))
    .tooltip("My App")
    .menu(menu)
    .on_click(|id| handle_click(id.as_str()));

// let m = muri::MainThreadMarker::new().unwrap(); // call on the real main thread
// tray.run(m)?; // installs the tray icon + runs the event loop (styled popup on macOS/Windows; native menu on Linux)
```

See [`examples/usagio_menu.rs`](examples/usagio_menu.rs) for usagio's full real
menu — provider groups, flush-right colored percentages, and account submenus —
rebuilt through the API.

## Migrating from muda

### Why

[`muda`](https://crates.io/crates/muda) + [`tray-icon`](https://crates.io/crates/tray-icon)
**sync a data model onto native OS menu objects** (`NSMenu`, Win32 `HMENU`, GTK
menus), so you inherit the OS look and **cannot restyle** — no custom
fonts/colors, no true multi-column alignment, and always a reserved
chevron/submenu column that stops values sitting flush at the right edge. muri
**draws the menu itself** on a CPU raster surface, identically on every OS, so you
get full styling (flush-right values, colors, fonts, embedded logos, nested
flyouts) while still following the OS dark/light + accent by default.

The migration is designed so `s/muda/muri/` **compiles, runs, and looks native
immediately** — then lets you restyle at your own pace, with no cliff.

### How

1. **Swap the dependency** — replace `muda` + `tray-icon` with:
   ```toml
   [dependencies]
   muri = { version = "0.10", features = ["muda-compat"] }
   ```
2. **Redirect imports** — your menu-building code compiles unchanged:
   ```rust
   use muri::compat::muda as muda;
   use muri::compat::tray_icon as tray_icon;
   ```
   `Menu`, `Submenu`, `MenuItem`, `CheckMenuItem`, `PredefinedMenuItem`, and
   `MenuId` are all mirrored; `MenuId` is structurally identical, so
   `event.id.0` still matches your existing handlers.
3. **Keep your event reader** — `MenuEvent::receiver()` works as-is; muri fires
   the same process-global channel (closure first, then the channel).
4. **The one real code change — the event loop.** muda/tray-icon are *passive*
   (they push to a global channel while *your* loop runs); muri's `Tray` *owns* a
   loop (`Tray::run(self)`, main thread on macOS). Either let muri own it
   (`tray.run()` blocks; read `MenuEvent::receiver()` off-thread) or drive it from
   your own loop. For runtime menu updates, grab a `TrayHandle` **before** `run()`
   and call `set_menu` / `set_icon` / `set_tooltip` from any thread.
5. **Routing** — `Menu::init_for_nsapp/hwnd/gtk_window` passes through to the
   **native** menu bar; `TrayIconBuilder…with_menu` and
   `show_context_menu_for_*` render as muri's **custom** surface with
   `Theme::native()`.
6. **Then restyle progressively** — Step 1: swap the theme. Step 2: make a row a
   `Flex::Grow` label + `Align::Right` colored value (flush-right, no chevron
   column) with a `StyleRun` span. Step 3: add section headers, logos, and
   submenus. Ids never change, so your handlers keep matching.

### Honest caveats

In *custom* surfaces (tray/context menus), `PredefinedMenuItem` OS actions are
best-effort and inemulable ones render as **visible disabled rows** (never
silently dropped); accelerators are displayed and handled **only while the menu
is open** (no system-wide hotkey — use `global-hotkey` for that); **Linux** has no
styled *tray-anchored* popup (native-menu fallback + pointer-anchored
`ContextMenu`); and the facade maps entirely onto muri — it does **not** pull in
the real `muda` / `tray-icon` crates. See the migration guide in
[`docs/design/spec/60-migration-guide.md`](docs/design/spec/60-migration-guide.md)
for the full divergence register.

## Platform support (honest matrix)

Legend: ✅ working (automated-tested / live) · 🔬 implemented, on-device
verification pending · ❌ not offered (by design).

Note the key nuance: **muri draws its own styled menu on macOS and Windows**; the
**Linux tray menu is a native `com.canonical.dbusmenu`** — the SNI/AppIndicator
*host* renders it, so it does **not** follow muri's theme or fonts. muri's own
styled popup on Linux is the pointer-anchored `ContextMenu` (X11 only today;
Wayland styled positioning returns `Unsupported`).

| OS      | Tray icon | Styled anchored popup | Context menu (`open_at`) | Screen reader |
|---------|-----------|-----------------------|--------------------------|---------------|
| macOS   | 🔬 `NSStatusItem` | 🔬 non-activating `NSPanel` + vibrancy, N-level flyouts, mouse + keyboard nav | 🔬 `open_at` + `Popup` (shared `PopupSession`) | 🔬 VoiceOver (per-window AccessKit adapters wired) |
| Windows | 🔬 `Shell_NotifyIcon` | 🔬 `WS_EX_NOACTIVATE` layered popup, DWM acrylic, `WH_MOUSE_LL` dismiss | 🔬 `open_at` + `Popup` (reuses the layered popup) | 🔬 NVDA + Narrator (UIA via `accesskit_windows`) |
| Linux   | 🔬 SNI/AppIndicator **native `dbusmenu`** (host-rendered; not muri-themed) | ❌ tray-anchored (by design — see below); use pointer `ContextMenu` | 🔬 X11 override-redirect `open_at` (`x11-popup` feature; Wayland: `Unsupported::ClientPositioning`) | 🔬 Orca (AT-SPI via the native menu) |

**All-green yet? No — by design.** The pure cross-platform core (menu model,
`Flex`/`Align` layout, flyout/anchor math, keyboard-nav state machine, theme
resolution, a11y tree) is unit-tested green on every OS, and **every backend
compiles + is clippy-clean on its target in CI**. The 🔬 cells are implemented
muri code that *builds* but hasn't been exercised on real hardware yet — verifying
them, and the four screen readers, is the remaining pre-1.0 work; each flips to ✅
as it's confirmed on the road to 1.0 (the all-four-screen-readers pass is a hard
1.0 gate). The Linux tray-anchored styled popup stays ❌ permanently.

### The Linux caveat (read this)

A tray-**anchored** styled popup is **architecturally impossible** on
Linux/Wayland, and muri will not pretend otherwise:

- The modern Linux tray is **StatusNotifierItem / AppIndicator over D-Bus**. The
  *host* (a GNOME extension, KDE plasmoid, or XEmbed shim) owns and draws the
  icon in its own process and renders the menu from a `com.canonical.dbusmenu`
  description. The app is **never told the icon's on-screen rectangle and never
  receives the click coordinate** (`tray-icon` documents both as *Unsupported*
  on Linux).
- **Wayland forbids a client from positioning its own toplevel** by protocol —
  `set_outer_position` is a documented no-op.

So on Linux muri offers a **fallback** instead of a broken promise:

1. **Native-menu fallback** — render the same `Menu` spec through a native
   `com.canonical.dbusmenu` tree (via `ksni`, the SNI/AppIndicator host renders
   it, so it loses custom styling but works everywhere a Linux tray works). This
   is the recommended default.
2. **Pointer-anchored `ContextMenu`** — the styled surface *is* available where a
   pointer coordinate exists (a right-click menu), just not anchored to the tray
   icon.

`Tray::run` returns `Err(Error::Unsupported(Unsupported::TrayAnchor))` on Linux
so callers fall back deliberately.

## Theming

A `ThemeSource` chooses the look; it resolves to a concrete `Theme` (colors,
fonts, spacing, corner radius, row height, column gap) that you can fully
override:

- **`ThemeSource::System(ThemeMode)`** — the default (`System(ThemeMode::Auto)`).
  Matches the **host** OS look and is the *only* source that receives live OS
  injection: the real accent, menu colors (`NSColor` / `GetSysColor` /
  Adwaita), and the system UI font + point size.
- **`ThemeSource::MacOs` / `Windows` / `Gnome` (each `(ThemeMode)`)** — *force* a
  specific platform look on **any** host (a macOS app can render a Windows menu),
  drawn as-authored with that OS's native metrics (macOS ~22pt rows / 6pt radius
  + SF tracking, Win11 acrylic / 8pt radius / Segoe UI, Adwaita flat / 12pt
  radius / Cantarell).
- **`ThemeSource::Preset(Preset)`** — a built-in non-OS skin, rendered as-authored
  on every platform: `OldSchoolTerminal`, `HighContrast`, `Solarized`, `Nord`.
- **`ThemeSource::Custom(Box<Theme>)`** — a fully hand-built `Theme`.

`ThemeMode` is `Auto` (follow the OS light/dark setting — the default), `Light`,
or `Dark`. Colors on a row are either literal `Color::Rgba(..)` or **semantic**
(`Color::Label`, `SecondaryLabel`, `Accent`, `Separator`,
`SystemRed`/`Orange`/`Green`/`Yellow`) and resolve against the active theme.
`Theme::native()` / `native_dark()` use a translucent background so the OS
vibrancy material (`NSVisualEffectView` on macOS, DWM acrylic on Windows) shows
through; GNOME is a flat opaque fill (Linux/X11 has no vibrancy).

**Forced-theme font resolution.** A forced platform look wants the *target* OS's
UI font, so muri resolves fonts in tiers and never silently substitutes the
host's own UI face (which would draw the wrong OS's font): (1) the real OEM family
(Segoe UI / SF Pro / Cantarell) if it happens to be installed; (2) with the
opt-in **`bundled-fonts`** feature, the vendored OSS substitute (Selawik / Inter /
Cantarell); (3) a free, broadly-available fallback (DejaVu Sans, Liberation Sans,
…). Pixel-perfect parity therefore requires the real target face to be installed —
muri's contract is to never lie about it.

## Error handling

muri's fallible operations return a typed `Result<T, Error>` — it **returns
errors, it does not log** (there is no `log`/`tracing` dependency; a consumer
decides what to surface):

- `Error::Unsupported(Unsupported)` — a genuine per-platform impossibility,
  surfaced deliberately: `Unsupported::TrayAnchor` (a styled tray-anchored popup
  on Linux) and `Unsupported::ClientPositioning` (client-side toplevel
  positioning, which Wayland forbids).
- `Error::BadIcon(String)` — the supplied icon bytes could not be decoded.
- `Error::Platform(String)` — a platform API call failed while creating,
  installing, or anchoring the tray/surface; the message names the concrete
  failure site (e.g. a failed `Shell_NotifyIcon(NIM_ADD)` / SNI registration, or a
  main-thread requirement). A Windows/Linux tray-install failure is surfaced
  synchronously through the tray-thread install handshake (via
  `TrayIconBuilder::build_result`) rather than returning a false `Ok`.

## Performance

muri is built for **instant** popup open — a menu that appears the frame you click
it. That drives two choices:

- **CPU raster, not GPU.** The popup is drawn on the CPU (`swash` glyphs + muri's
  own AA blitter) and blitted to the window. There is **no GPU warm-up** (adapter
  init / shader compilation, ~hundreds of ms cold), which a transient popup can't
  hide. Menus are tiny, so the CPU draw is sub-millisecond, and vibrancy blur is
  the OS compositor's GPU work behind our transparent surface — we get it for free.
- **Event-driven, cached.** The popup repaints only on state change (no idle
  redraw loop) and reuses a per-frame glyph cache (zero re-rasterization on
  repaint).

**Recommended release profile for an embedding app** (muri is a library, so its
own profile doesn't apply to your binary — mirror this):

```toml
[profile.release]
opt-level = 3        # speed, not size — size-tuning hurts open latency
lto = "fat"
codegen-units = 1
strip = true
```

## Roadmap

All three backends are **implemented**; the remaining pre-1.0 work is on-device
verification and screen-reader validation.

1. **macOS backend** (done) — `NSStatusItem`-anchored custom-drawn panel, flush
   alignment, flyout submenus, transient dismiss, dark mode, keyboard navigation,
   a parallel accessibility tree, and an attached AccessKit platform adapter (a
   live VoiceOver validation pass on a device remains).
2. **Windows backend** (done) — `WS_EX_NOACTIVATE` layered window anchored via
   `Shell_NotifyIconGetRect`, DWM acrylic, theme-follow, `WH_MOUSE_LL`/`WH_KEYBOARD_LL`
   dismiss + keyboard, UIA via `accesskit_windows` (NVDA + Narrator validation
   remains).
3. **Linux backend** (done) — SNI/AppIndicator native `dbusmenu` + pointer-anchored
   `ContextMenu` on X11; AT-SPI via the native menu (Orca validation remains). A
   `wlr-layer-shell` anchored backend is a possible future community opt-in
   (wlroots/KWin only).

## Minimum supported Rust version

muri's MSRV is **1.87**, verified in CI by a dedicated job that runs `cargo check`
on 1.87. Lint, format, and tests run on the **latest stable** toolchain (so
Clippy always uses current lints) — only the MSRV *build* is pinned.

The floor is set by the **Linux** SNI tray's `zbus` D-Bus stack, which requires
1.87; the macOS/Windows/core trees build on 1.85, but the crate-wide contract is
the higher of the two, since a current, maintained D-Bus crate is the right
dependency for the Linux tray. Any MSRV change is a documented, deliberate
decision, not an accident of a transitive bump.

## Contributing

Contributions are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for the
build/test workflow and the `fmt` + `clippy` + `test` + `doc` quality gate, and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) for community expectations. CI runs on
the `dev` → `qa` → `main` flow, so a change is validated before it reaches
`main`.

## Security

To report a vulnerability, follow the private disclosure process in
[SECURITY.md](SECURITY.md) (GitHub security advisories, or the email listed
there). Please do not open a public issue for security reports.

## Changelog

Notable changes are recorded in [CHANGELOG.md](CHANGELOG.md).

## License

Licensed under the [MIT License](LICENSE). © Matthew Jackson

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in muri by you shall be licensed as above, without any additional
terms or conditions.
