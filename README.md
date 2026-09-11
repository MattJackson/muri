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

muri is published on crates.io. It is **pre-1.0** (`0.11.x`), so the public API
may still change between releases while the three backends are device-verified and
the surface stabilizes — pin at least the minor version:

```toml
[dependencies]
muri = "0.11"
# with accessibility:            muri = { version = "0.11", features = ["a11y"] }
# migrating from muda/tray-icon: muri = { version = "0.11", features = ["muda-compat"] }
# bundled OSS UI fonts for forced cross-platform themes:
#                                muri = { version = "0.11", features = ["bundled-fonts"] }
# tray-only Linux, no styled X11 popup (drops the x11rb stack):
#                                muri = { version = "0.11", default-features = false }
# or track the latest unreleased work from git:
# muri = { git = "https://github.com/MattJackson/muri" }
```

> **Status: all three backends implemented; on-device verification ongoing.**
> muri draws its own styled popup on **macOS** and **Windows**. On **Linux** a
> runtime *presenter* seam picks the best surface for the session: muri's own
> styled popup on **X11**, an **experimental** `wlr-layer-shell` styled popup on
> wlroots + KDE/Plasma (a scaffold behind the off-by-default `wayland-styled`
> feature — device-verify pending), and the **native `com.canonical.dbusmenu`**
> tree on GNOME-Wayland (where a client styled menu is impossible by policy).
> `Tray::run` installs the OS status item, opens the menu, opens flyout submenu
> panels, follows dark/light, and supports **keyboard navigation** (arrows /
> Home-End / Right-Left / Enter-Space / Esc / type-ahead) over the same
> hover-stack the mouse drives. muri also publishes a **parallel accessibility
> tree** (`Tray::accessibility_tree`) that maps the menu onto menu/menuitem roles
> with name, checked, enabled, submenu-expanded, and set-position, and (behind the
> `a11y` feature) attaches an AccessKit platform adapter to the popup window so
> that tree is exposed to NSAccessibility / VoiceOver (macOS) and UIA / NVDA /
> Narrator (Windows). A pointer-anchored styled `ContextMenu` is also available on
> X11, and a **headless renderer** (`render_menu_to_png` / `render_menu_to_rgba`)
> rasterizes any menu to pixels on every OS with no display or window.
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
behind opt-in flags. All ship today (the `wayland-styled` renderer is an
experimental scaffold — see its row).

| Flag | Default | Description |
|------|---------|-------------|
| `x11-popup` | **on** | Linux only: the styled, pointer-anchored `ContextMenu::open_at` popup (and the X11 tray presenter), via the pure-Rust `x11rb` X11 client. On by default for back-compat and `muda-compat` parity; a tray-only consumer can `default-features = false` to drop `x11rb` + `x11rb-protocol` + `gethostname` and keep just the SNI tray. Inert on non-Linux targets. |
| `wayland-styled` | off | **Experimental (Linux only).** Scaffolding for the self-drawn, styled tray/context menu on Wayland via `zwlr_layer_shell_v1` (wlroots compositors + KWin/Plasma; GNOME/Mutter refuses layer-shell by policy, so it stays native there). Today this only gates the Wayland scaffold module and pulls **no** external crates — the live layer-shell renderer + registry bind are device-side tasks (`todo!("DEVICE-VERIFY")`), so `cargo build --all-features` stays green on macOS. Inert on non-Linux targets. See [`docs/adr/0003-linux-styled-tray-menu.md`](docs/adr/0003-linux-styled-tray-menu.md). |
| `a11y` | off | AccessKit screen-reader bridge over the raster backend (`accesskit_macos` → NSAccessibility, `accesskit_windows` → UIA; Linux via the native menu / AT-SPI). Ships today; **planned to be on by default at 1.0**. |
| `muda-compat` | off | The **frozen**, pure bidirectional `muda` / `tray-icon` drop-in facade (`compat::muda` / `compat::tray_icon`) so existing callers can migrate with `s/muda/muri/` (and back). It mirrors upstream's API **exactly — no more, no less** — carrying **no** muri-only customization; maps entirely onto muri's native model and does **not** pull in the real `muda` / `tray-icon` crates (see [Migrating from muda](#migrating-from-muda)). |
| `bundled-fonts` | off | Embeds freely-redistributable OSS UI-font substitutes (OFL **Inter** for SF Pro, Microsoft's own OFL **Selawik** for Segoe UI, genuine **Cantarell** for GNOME) so a *forced* cross-platform theme (`ThemeSource::MacOs`/`Windows`/`Gnome`) can render in a metric-compatible face when the real OEM font isn't installed on the host — never silently drawing the wrong-OS host font. OFF by default to keep the crate small; the TTFs are `include_bytes!`-embedded only when on. See [`assets/fonts/README.md`](assets/fonts/README.md). |

## Quick example

```rust
use muri::{Tray, Menu, Row, Segment, Align, Flex, Color, Icon, Font, Weight};

let menu = Menu::new()
    .section_header(Row::label_only("Claude"))
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

let tray = Tray::new(Icon::from_png(include_bytes!("icon.png").as_slice()))
    .tooltip("My App")
    .menu(menu)
    .on_click(|id| handle_click(id.as_str()));

// let m = muri::MainThreadMarker::new().unwrap(); // call on the real main thread
// tray.run(m)?; // installs the tray icon + runs the event loop (styled popup on macOS/Windows; presenter-selected on Linux)
```

See [`examples/usagio_menu.rs`](examples/usagio_menu.rs) for usagio's full real
menu — provider groups, flush-right colored percentages, and account submenus —
rebuilt through the API.

## Native API: the one obvious way

The native surface deliberately has **one canonical call per task** (issue #62),
with alternatives kept only as clearly-labeled sugar or full-control escape
hatches. When in doubt, reach for the canonical path:

| Task | Canonical native call | Escape hatch |
|------|-----------------------|--------------|
| Build a menu | `Menu::new()` + `.row(..)` / `.separator()` / `.section_header(..)` / `.submenu(label, menu)` / `.content(..)` | `Menu::item(..)` with a hand-built `Item` |
| An interactive row | `Row::new(id)` | — |
| A header / label / info row | `Row::label_only(text)` | `Row::default()` + segments |
| Row text | `Row::label(text)` / `Row::label_value(label, value)` | `Row::segments(vec![Segment…])` |
| Bold a row's label | `Row::bold()` | a whole-label `StyleRun` with `Weight::Bold` |
| Color a row's value | `Row::value_color(color)` | per-substring `StyleRun`s via `Segment::run` / `Segment::runs` |
| An icon | `Icon::from_png(bytes)` / `Icon::from_rgba(rgba, w, h)` / `Icon::from_svg(bytes)` | — |
| Choose the look | `MenuOptions` (carrying a `ThemeSource`) | `Tray::theme(..)` / `TrayHandle::set_theme(..)` (derived conveniences that set the `MenuOptions` theme) |

Per-run `StyleRun` styling (attached with `Segment::run` / `Segment::runs`, each
run carrying a `color` and an optional `weight`) is the **one** styling system;
`Row::bold()` / `Row::value_color()` are the ergonomic front door onto it for the
two most common cases and render identically to the equivalent hand-built runs.
`MenuOptions` is the single source of truth for "which look" — `Tray::theme` and
`TrayHandle::set_theme` ultimately set its `theme` field.

```rust
use muri::{Menu, Row, Icon, Color, MenuOptions, ThemeSource, ThemeMode};

let menu = Menu::new()
    .section_header(Row::label_only("Account"))
    .row(Row::new("me").label_value("me@example.com", "47% / 89%").value_color(Color::SystemRed))
    .row(Row::new("prefs").label("Preferences…").bold())
    .separator()
    .row(Row::new("quit").label("Quit"));

let icon = Icon::from_svg(include_bytes!("logo.svg").as_slice());
let options = MenuOptions::default().theme(ThemeSource::MacOs(ThemeMode::Dark));
```

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
immediately**. The compat facade is a **frozen, pure bidirectional drop-in** — it
mirrors upstream `muda` / `tray-icon` *exactly*, so `s/muri/muda/` reverses the
move with equal ease. It carries **no** muri-only customization: to restyle (bold
rows, colored values, forced themes, logos, `MenuOptions`), you adopt the
**native** muri API (`Menu` / `Row` / `Tray` / `ContextMenu` / `MenuOptions`) —
that is where all customization lives.

### How

1. **Swap the dependency** — replace `muda` + `tray-icon` with:
   ```toml
   [dependencies]
   muri = { version = "0.11", features = ["muda-compat"] }
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
   `show_context_menu_for_*` render as muri's **custom** surface with muri's
   native-look theme.
6. **To customize, cross to the native API.** Because the facade is frozen, you
   restyle by building the same menu through native muri instead: a `Flex::Grow`
   label + `Align::Right` colored value (flush-right, no chevron column) with a
   `StyleRun` span, `Row::bold()` / `Row::value_color()`, section headers, logos,
   submenus, and a `MenuOptions` / `ThemeSource` for the look. Ids never change,
   so your handlers keep matching. See [Native API: the one obvious
   way](#native-api-the-one-obvious-way).

### Honest caveats

In *custom* surfaces (tray/context menus), `PredefinedMenuItem` OS actions are
best-effort and inemulable ones render as **visible disabled rows** (never
silently dropped); accelerators are displayed and handled **only while the menu
is open** (no system-wide hotkey — use `global-hotkey` for that); **Linux** has no
styled *tray-anchored* popup (native dbusmenu / presenter fallback + pointer-anchored
`ContextMenu`); and the facade maps entirely onto muri — it does **not** pull in
the real `muda` / `tray-icon` crates. See the migration guide in
[`docs/design/spec/60-migration-guide.md`](docs/design/spec/60-migration-guide.md)
for the full divergence register.

## Platform support (honest matrix)

Legend: ✅ working (automated-tested / live) · 🔬 implemented, on-device
verification pending · 🧪 experimental scaffold (feature-gated, off by default) ·
❌ not offered (by design).

Note the key nuance: **muri draws its own styled menu on macOS and Windows**; on
**Linux** a runtime *presenter* seam picks the surface per session — muri's own
styled popup on **X11**, an **experimental** `wlr-layer-shell` styled popup on
wlroots + KDE/Plasma (the off-by-default `wayland-styled` scaffold), and the
**native `com.canonical.dbusmenu`** tree on **GNOME-Wayland** (host-rendered, so
not muri-themed — a client styled menu is refused there by policy). A
tray-*anchored* popup stays impossible on Linux regardless (no icon geometry
reaches the app); the pointer-anchored `ContextMenu` is muri's portable styled
primitive there. A **headless renderer** draws any menu to pixels on every OS.

| OS      | Tray icon | Styled anchored popup | Context menu (`open_at`) | Screen reader |
|---------|-----------|-----------------------|--------------------------|---------------|
| macOS   | 🔬 `NSStatusItem` | 🔬 non-activating `NSPanel` + vibrancy, N-level flyouts, mouse + keyboard nav | 🔬 `open_at` + `Popup` (shared `PopupSession`) | 🔬 VoiceOver (per-window AccessKit adapters wired) |
| Windows | 🔬 `Shell_NotifyIcon` | 🔬 `WS_EX_NOACTIVATE` layered popup, DWM acrylic, `WH_MOUSE_LL` dismiss | 🔬 `open_at` + `Popup` (reuses the layered popup) | 🔬 NVDA + Narrator (UIA via `accesskit_windows`) |
| Linux   | 🔬 SNI/AppIndicator (presenter picks the menu surface) | ❌ tray-anchored (by design — see below). Presenter: 🔬 X11 styled popup on activate · 🧪 wlroots/KDE `wlr-layer-shell` (`wayland-styled` scaffold) · 🔬 native `dbusmenu` on GNOME | 🔬 X11 override-redirect `open_at` (`x11-popup`; Wayland: `Unsupported::ClientPositioning`) | 🔬 Orca (AT-SPI via the native menu) |

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

So on Linux muri never fabricates a tray anchor. `Tray::run` **does** work — it
installs the SNI/AppIndicator status item and runs its loop — but the tray
*anchor rect* is unavailable, so `Tray::anchor_rect` (and the cross-thread
`TrayHandle::anchor_rect`) reports `Error::Unsupported(Unsupported::TrayAnchor)`.
What the tray shows is chosen by a runtime **presenter** seam
([ADR-0003](docs/adr/0003-linux-styled-tray-menu.md)):

1. **`NativeDbusMenu`** — render the same `Menu` spec through a native
   `com.canonical.dbusmenu` tree (via `ksni`; the SNI/AppIndicator host draws it,
   so it loses custom styling but works everywhere a Linux tray works, and is
   accessible over AT-SPI for free). The universal baseline and the surface on
   **GNOME-Wayland** (which refuses a client styled menu).
2. **`X11Popup`** — on a real X11 session, activating the tray opens **muri's own
   styled popup** at the pointer (override-redirect window; `x11-popup` feature).
3. **`WaylandLayerShell`** — **experimental** styled popup via `wlr-layer-shell`
   on wlroots + KDE/Plasma, behind the off-by-default `wayland-styled` scaffold
   (the live renderer is a device-side task — see the ADR).

Independently, the **pointer-anchored `ContextMenu`** styled surface is available
wherever a pointer coordinate exists (a right-click menu) — X11 today; on Wayland
`open_at` returns `Unsupported::ClientPositioning` (a client cannot self-position
a bare toplevel).

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

## Headless rendering

muri can rasterize a built `Menu` straight to pixels with **no tray, no window,
and no display** — the same layout + paint pass a live popup runs, driven into an
in-memory framebuffer and read back out:

```rust
use muri::{Menu, Row, MenuOptions, ThemeSource, ThemeMode};

let menu = Menu::new().row(Row::new("quit").label("Quit"));

// PNG bytes (scale = device pixels per logical pixel; 2.0 ≈ Retina):
let png = muri::render_menu_to_png(&menu, &MenuOptions::default(), 2.0);

// or raw straight-alpha RGBA8 + dimensions, for diffing / custom encoding:
let (rgba, w, h) = muri::render_menu_to_rgba(&menu, &MenuOptions::default(), 2.0);
assert_eq!(rgba.len(), (w * h * 4) as usize);
```

Both entry points are available on **every** OS (no platform/tray feature, no
`cfg(target_os)`). The concrete `Theme` is resolved internally from
`MenuOptions::theme`, so forcing a cross-OS look
(`ThemeSource::MacOs`/`Windows`/`Gnome`, ideally with `bundled-fonts` on) renders
any OS's OEM menu from a single host — the basis for **CI screenshots** and a
**cross-OS golden-image** suite on one runner. The render is deterministic and
headless: an `Auto` appearance resolves to the *light* look (pass an explicit dark
mode for dark), and no row is highlighted.

## Error handling

muri's fallible operations return a typed `Result<T, Error>` — it **returns
errors, it does not log** (there is no `log`/`tracing` dependency; a consumer
decides what to surface). `Error` is **structured** and `#[non_exhaustive]`, so
the failure sites that carry actionable meaning are their own variants (a consumer
can `match` on the *kind* rather than string-match a message) and a `match` must
include a `_` arm:

- `Error::Unsupported(Unsupported)` — a genuine per-platform impossibility,
  surfaced deliberately: `Unsupported::TrayAnchor` (a styled tray-anchored popup
  on Linux) and `Unsupported::ClientPositioning` (client-side toplevel
  positioning, which Wayland forbids).
- `Error::BadIcon(String)` — the supplied icon bytes could not be decoded.
- `Error::TrayInstall(String)` — the OS tray/status-item install failed
  (`Shell_NotifyIcon(NIM_ADD)` on Windows, or the SNI/`StatusNotifierItem` D-Bus
  registration on Linux). On Windows/Linux this is surfaced **synchronously**
  through the tray-thread install handshake (via `Tray::spawn`'s `Result`), so a
  caller learns the icon never appeared instead of seeing a false `Ok`.
- `Error::MainThread` — a tray/surface that must be created on the main thread was
  requested off it (AppKit's `NSStatusItem`, and the platform's main-thread-only
  install/run paths).
- `Error::ThreadSpawn(String)` — the background `muri-tray` UI thread could not be
  spawned.
- `Error::Platform(String)` — the catch-all for a residual per-OS API failure
  while creating or anchoring a surface that doesn't fit a more specific
  structured variant; the message names the concrete failure site.

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
3. **Linux backend** (done) — a runtime presenter seam over SNI/AppIndicator:
   muri's own styled popup on X11, native `com.canonical.dbusmenu` on
   GNOME-Wayland (and as the universal fallback), plus the pointer-anchored
   `ContextMenu` on X11; AT-SPI via the native menu (Orca validation remains). An
   **experimental** `wlr-layer-shell` styled popup for wlroots + KDE/Plasma is
   scaffolded behind the off-by-default `wayland-styled` feature — the live
   renderer and registry bind are device-side tasks (see
   [ADR-0003](docs/adr/0003-linux-styled-tray-menu.md)).

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
