# muri

[![CI](https://github.com/MattJackson/muri/actions/workflows/ci.yml/badge.svg)](https://github.com/MattJackson/muri/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/muri.svg)](https://crates.io/crates/muri)
[![docs.rs](https://img.shields.io/docsrs/muri)](https://docs.rs/muri)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Menu Utilities for Rust Interfaces** — a cross-platform, fully-styleable
tray-icon + popup-menu system for Rust. Think "a better `muda` + `tray-icon`":
muri owns the tray icon, the styled popup, *and* the anchoring, and draws **one
consistent custom appearance on every OS** instead of delegating to native
menus.

> **Status: early / macOS-first WIP.** This crate currently ships the **public
> API surface only** — the types, the builder, and the documented architecture.
> Nothing is rendered yet; methods that need a live renderer are `todo!()`. See
> [`docs/design/muri.md` in the usagio repo](https://github.com/MattJackson/usagio)
> for the full design and roadmap.

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

// tray.run()?; // installs the tray icon + runs the event loop (not implemented yet)
```

See [`examples/usagio_menu.rs`](examples/usagio_menu.rs) for usagio's full real
menu — provider groups, flush-right colored percentages, and account submenus —
rebuilt through the API.

## Platform support (honest matrix)

| OS      | Tray icon | Styled anchored popup | Anchoring mechanism | Screen-reader a11y |
|---------|-----------|-----------------------|---------------------|--------------------|
| macOS   | ✅        | ✅                    | `NSStatusItem` button rect | NSAccessibility |
| Windows | ✅        | ✅                    | `Shell_NotifyIconGetRect`  | UIA |
| Linux   | ✅        | ❌ (see below)        | —                   | AT-SPI (fallback path) |

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
active theme (and to the matching `NSColor` on macOS).

## Roadmap

1. **macOS backend** — `NSStatusItem`-anchored custom-drawn panel, flush
   alignment, submenus, transient dismiss, NSAccessibility, dark mode.
2. **Windows backend** — `WS_EX_NOACTIVATE` layered window anchored via
   `Shell_NotifyIconGetRect`, theme-follow, outside-click dismiss, UIA.
3. **Linux** — native-menu fallback + pointer-anchored `ContextMenu`; AT-SPI via
   AccessKit. (A `wlr-layer-shell` anchored backend is a possible future
   community opt-in, wlroots/KWin only.)

## Contributing

Contributions are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for the
build/test workflow and the `fmt` + `clippy` + `test` quality gate, and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) for community expectations. Security
issues should be reported privately per [SECURITY.md](SECURITY.md). Notable
changes are tracked in [CHANGELOG.md](CHANGELOG.md).

## License

Licensed under the [MIT License](LICENSE). © Matthew Jackson

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in muri by you shall be licensed as above, without any additional
terms or conditions.
