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

muri is published on crates.io. It is **pre-1.0** (`0.9.x`), so the public API may
still change between releases until the Windows backend is device-verified and the
surface stabilizes — pin an exact version:

```toml
[dependencies]
muri = "0.9"
# with accessibility:            muri = { version = "0.9", features = ["a11y"] }
# migrating from muda/tray-icon: muri = { version = "0.9", features = ["muda-compat"] }
# or track the latest unreleased work from git:
# muri = { git = "https://github.com/MattJackson/muri" }
```

> **Status: macOS-first WIP.** On **macOS** the crate draws a real styled popup:
> `Tray::run` installs the `NSStatusItem`, opens the custom-drawn menu anchored to
> it, opens flyout submenu panels, follows dark/light, and now supports
> **keyboard navigation** (arrows / Home-End / Right-Left / Enter-Space / Esc /
> type-ahead) over the same hover-stack the mouse drives. muri also publishes a
> **parallel accessibility tree** (`Tray::accessibility_tree`) that maps the menu
> onto menu/menuitem roles with name, checked, enabled, submenu-expanded, and
> set-position, and (behind the `a11y` feature) attaches an `accesskit_winit`
> adapter to the popup window so that tree is exposed to NSAccessibility /
> VoiceOver (a live screen-reader validation pass is the remaining human hop). The
> **Windows** backend is **code-complete** as of 0.9.0 — `Shell_NotifyIcon` tray, a
> `WS_EX_NOACTIVATE` layered popup with DWM acrylic, a Win32 message pump,
> `WH_MOUSE_LL`/`WH_KEYBOARD_LL` hooks for outside-click dismiss + keyboard, and
> NVDA/Narrator via `accesskit_windows` (compile-verified for
> `x86_64-pc-windows-msvc`; on-device verification is the 0.9.x cycle's job). The
> **Linux** tray/anchor path is a deliberate fallback (see below), not a styled
> tray popup.
>
> **Pre-1.0:** muri is `0.9.x`. The public API may change between releases until
> the Windows backend is device-verified and the surface stabilizes; pin an exact
> version.

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
- **Arbitrary colors, fonts, and embedded logos** (PNG or SVG bytes).
- **Styled nested submenus** (flyout panels beside the row).
- **One consistent look** that still **follows OS dark/light + accent** by
  default, and is fully overridable via a theme API.

## Design decisions (fixed)

- **Name:** muri. **License:** MIT.
- **One consistent custom-drawn appearance** across all OSes — not native-per-OS
  chrome.
- **Rendering stack:** [`winit`](https://docs.rs/winit) (windowing) +
  `softbuffer`/`tiny-skia` (CPU 2D raster) + `cosmic-text` (text/fonts). Minimal
  deps, tiny binary, no GPU warm-up, instant popups, full pixel control.
- **muri owns the whole stack:** the tray icon, the styled popup, and the
  anchoring (a full `muda` + `tray-icon` replacement).
- **Accessibility is first-class:** native a11y APIs are wired over the
  custom-drawn widget tree — NSAccessibility (macOS), UIA (Windows), AT-SPI
  (Linux fallback) — targeted for v1.
- **v1 OS order:** macOS → Windows → Linux.

## Feature flags

muri keeps its default surface minimal (`default = []`); optional integrations sit
behind flags. Both flags below ship today.

| Flag | Default | Description |
|------|---------|-------------|
| `a11y` | off | AccessKit screen-reader bridge over the raster backend (NSAccessibility / UIA / AT-SPI). Ships today; **planned to be on-by-default at 1.0**. |
| `muda-compat` | off | A `muda` / `tray-icon` drop-in compatibility facade (`compat::muda` / `compat::tray_icon`) so existing callers can migrate with minimal churn. Maps entirely onto muri's native model — it does **not** pull in the real `muda` / `tray-icon` crates. Ships today (see [Migrating from muda](#migrating-from-muda)). |

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

// tray.run()?; // installs the tray icon + runs the event loop (live on macOS; Windows code-complete/device-verify pending; Linux uses the fallback)
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
   muri = { version = "0.9", features = ["muda-compat"] }
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

Legend: ✅ working (automated-tested / live) · 🔬 code-complete, on-device
verification pending (this is what the 0.9.0 testing release is for) ·
🚧 planned before 1.0 · ❌ not offered (by design).

| OS      | Tray icon | Styled anchored popup | Context menu (`open_at`) | Screen reader |
|---------|-----------|-----------------------|--------------------------|---------------|
| macOS   | 🔬 `NSStatusItem` | 🔬 non-activating `NSPanel` + vibrancy, N-level flyouts, mouse + keyboard nav | 🔬 `open_at` + `Popup` (shared `PopupSession`) | 🔬 VoiceOver (per-window AccessKit adapters wired) |
| Windows | 🔬 `Shell_NotifyIcon` | 🔬 `WS_EX_NOACTIVATE` layered popup, DWM acrylic, `WH_MOUSE_LL` dismiss | 🔬 `open_at` + `Popup` (reuses the layered popup) | 🔬 NVDA + Narrator (UIA via `accesskit_windows`) |
| Linux   | 🔬 SNI/AppIndicator native menu | ❌ tray-anchored (by design — see below); use pointer `ContextMenu` | 🔬 X11 override-redirect `open_at` (Wayland: `Unsupported::ClientPositioning`) | 🔬 Orca (AT-SPI via the native menu) |

**Will 0.9.0 be all-green? No — by design.** The pure cross-platform core (menu
model, `Flex`/`Align` layout, flyout/anchor math, keyboard-nav state machine,
theme resolution, a11y tree) is unit-tested green on every OS, and **every backend
compiles + is clippy-clean on its target in CI**. The 🔬 cells are muri code that
*builds* but hasn't been exercised on real hardware yet — verifying them, and the
four screen readers, is exactly what the 0.9.0 testing release is for; each flips
to ✅ as it's confirmed on the road to 1.0 (the all-four-screen-readers pass is a
hard 1.0 gate). The Linux tray-anchored styled popup stays ❌ permanently.

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
   `muda` / `dbusmenu` tree (loses custom styling, but works everywhere a Linux
   tray works). This is the recommended default.
2. **Pointer-anchored `ContextMenu`** — the styled surface *is* available where a
   pointer coordinate exists (a right-click menu), just not anchored to the tray
   icon.

`Tray::run` returns `Err(Error::Unsupported(Unsupported::TrayAnchor))` on Linux
so callers fall back deliberately.

## Theming

muri follows OS dark/light and accent by default (`ThemeSource::FollowSystem`)
and exposes a full `Theme` (colors, fonts, spacing, corner radius, row height,
column gap) for consumer overrides via `ThemeSource::Custom(Theme)`. Colors are
either literal `Color::Rgba(..)` or **semantic** (`Color::Label`,
`SecondaryLabel`, `Accent`, `SystemRed`/`Orange`/…) which resolve against the
active theme (and to the matching `NSColor` on macOS). `Theme::native()` uses a
translucent background so the OS vibrancy material (`NSVisualEffectView` on macOS,
DWM acrylic on Windows) shows through.

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

1. **macOS backend** — `NSStatusItem`-anchored custom-drawn panel, flush
   alignment, flyout submenus, transient dismiss, dark mode, keyboard navigation,
   a parallel accessibility tree, and an attached AccessKit platform adapter
   (**done**; a live VoiceOver validation pass on a device remains).
2. **Windows backend** — `WS_EX_NOACTIVATE` layered window anchored via
   `Shell_NotifyIconGetRect`, theme-follow, outside-click dismiss, UIA.
3. **Linux** — native-menu fallback + pointer-anchored `ContextMenu`; AT-SPI via
   AccessKit. (A `wlr-layer-shell` anchored backend is a possible future
   community opt-in, wlroots/KWin only.)

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
