# 00 — Overview: mission, decisions, architecture

Status of this document: **foundational spec.** Cross-references:
[`01-api-contract.md`](01-api-contract.md), [`02-muda-compat.md`](02-muda-compat.md),
[`03-threading-events-versioning.md`](03-threading-events-versioning.md).

---

## 1. Mission

muri (**M**enu **U**tilities for **R**ust **I**nterfaces) is a cross-platform,
fully-styleable **tray-icon + popup-menu** crate. It is a custom-drawn replacement
for the `muda` (menu model) + `tray-icon` (status icon) pairing.

Native menu crates sync a data model onto OS-owned menu objects — `NSMenu`
(macOS), Win32 `HMENU` / `TrackPopupMenu` (Windows), GTK menus / `dbusmenu`
(Linux). The OS draws those pixels, so you cannot restyle them: no custom fonts or
colors, no true multi-column alignment, and always a reserved chevron/submenu
column that stops a value from sitting flush at the right edge.

muri draws the menu **itself** on a portable CPU raster surface, identically on
every OS. That single owned surface is the whole point: it is what makes true
left/center/right alignment, arbitrary color and font, embedded logos, and
**flush-right values with no reserved chevron column** actually possible.

## 2. The vision (north star)

> "Anyone on `muda` can move to muri and have their product look the **same** with
> minimal code changes, but suddenly gain massive flexibility to change the look
> however they want — vs 'this is what it is'."

Concretely: `s/muda/muri/` (change the import path) should compile, and the app
should look native by default. Then the consumer can *progressively* restyle —
first a `Theme`, then per-row `Segment` / `Flex` / `Align` / `Color` — without a
rewrite. This is a two-layer product:

- a **compatibility layer** (`muri::compat::muda`) that mirrors muda 1:1 for a
  zero-thought migration (see [`02-muda-compat.md`](02-muda-compat.md)); and
- muri's **own native API** (see [`01-api-contract.md`](01-api-contract.md)) that
  the consumer reaches into once they want the flexibility the facade hides.

## 3. Goals and non-goals

### Goals (1.0)

- **One consistent, fully custom-drawn look** on macOS, Windows, and Linux for the
  surfaces muri owns (tray, context menu, dropdown/popup, flyout submenus).
- **muda drop-in compatibility** (locked decision #1): the facade, `Theme::native()`,
  and the global `MenuEvent` channel.
- **Hybrid menu bar** (locked decision #2): custom surfaces are muri; the OS
  application menu bar / system menu passes through to native.
- **Full presentation control**: per-segment alignment and multi-column rows;
  literal or semantic colors; fonts (family / size / weight); leading & trailing
  icons/logos (PNG or SVG bytes); enabled / checked state; separators; section
  headers; nested submenus as flyout panels; a full `Theme` override surface.
- **Follows OS dark/light + accent by default**, HiDPI-crisp, opens with no
  perceptible delay (CPU raster, no GPU warm-up).
- **Accessibility as a first-class 1.0 gate**: a published parallel a11y tree
  bridged through AccessKit to NSAccessibility / UIA / AT-SPI, plus muri-owned
  keyboard navigation, verified against **all** target screen readers
  (locked decision #4).

### Non-goals (1.0 and beyond)

- **Not a general GUI framework.** No arbitrary widget trees, forms, text inputs,
  tabs, or scroll-views. muri draws *menus* — lists of rows, possibly nested.
- **Not a replacement for application windows / dialogs.** Text entry (e.g.
  paste-an-API-key) belongs in a normal window the host owns; a non-activating
  popup deliberately cannot host a first-responder text field.
- **Not a fork of muda.** muda is a data-model sync over native menu objects;
  there is no drawable layer to fork. Custom drawing means bypassing native menus
  entirely — which is exactly what muri does. (The facade *wraps* muda for
  passthrough surfaces; see decision #2.)
- **Not a global-hotkey provider.** Accelerator *text* is displayed and in-menu
  mnemonics are handled, but system-wide hotkeys are out of scope (that is the
  `global-hotkey` crate's job). See [`40-input-interaction.md`](40-input-interaction.md).
- **Not a tray-anchored styled popup on Linux.** Architecturally impossible on
  SNI/AppIndicator + Wayland; muri says so honestly (decision #3, §6 below).

## 4. The four locked decisions and their rationale

These are fixed for 1.0. Design *to* them.

### Decision 1 — muda drop-in compatibility is a hard requirement

**What:** 1.0 ships `muri::compat::muda`, a facade with muda's type names,
builders, `MenuId`, and the **global `MenuEvent::receiver()` channel** semantics,
plus a `Theme::native()` default. An existing muda app migrates by changing the
import and looks the same.

**Rationale:** adoption. The addressable market is every app already on muda /
tao / tauri-style tray menus. A migration that is "change one import, ship, looks
identical, then restyle at your leisure" is a categorically easier sell than "port
your menu code." It also *forces* muri's native API to be expressive enough to
express everything muda can — a useful design constraint.

**Tension it creates:** muda's event model is a **global crossbeam channel**
(`MenuEvent::receiver()`); muri's shipped native model is a **per-surface closure**
(`on_click: Box<dyn Fn(&MenuId)>`). The facade must present the global channel
while muri's core prefers closures. Resolved in
[`03-threading-events-versioning.md`](03-threading-events-versioning.md): 1.0 adds a
process-global `muri::MenuEvent` channel alongside the closure, and the facade
wires surfaces to forward into it.

### Decision 2 — Hybrid menu bar (custom surfaces muri, menu bar passthrough)

**What:** muri custom-draws the tray icon menu, context menus, and
dropdown/popup menus. The **application menu bar** (macOS global menu bar; a
Windows window menu; a GTK menu bar) and the macOS **system/status menus** pass
through to the *real native menu*. The facade routes `init_for_nsapp` /
`init_for_hwnd` / `init_for_gtk_window` (menu-bar installs) to native muda, and
`show_context_menu_for_*` (transient menus) to muri's custom renderer.

**Rationale:** a custom renderer fundamentally cannot own the macOS menu bar — it
is drawn by the window server at the top of the screen and is not a surface an app
can repaint. Fighting that is a losing battle and would break platform
conventions (Apple HIG, keyboard access, Services). The pragmatic, honest split is:
own what we *can* draw (transient, app-anchored surfaces) and defer what the OS
owns. This is the single biggest correctness seam in the facade; see the
adversarial notes in [`02-muda-compat.md`](02-muda-compat.md) §6.

### Decision 3 — All three platforms ship in 1.0

**What:** macOS, Windows, and Linux are all 1.0 targets. Linux ships via the
**pointer-anchored context-menu path** and the **native-menu fallback**, because
tray-anchoring is architecturally impossible there (§6).

**Rationale:** credibility of a "cross-platform" claim. Linux support is *honest
degradation*, not a lie: `Tray::run` returns `Unsupported::TrayAnchor` on Linux,
and the same `Menu` spec still renders — styled at a pointer (`ContextMenu`) or
native in the tray (fallback). A crate that silently no-ops on Linux would be
worse than one that returns a typed error and offers a documented alternative.

### Decision 4 — All screen readers verified in 1.0

**What:** VoiceOver (macOS), NVDA and Narrator (Windows), and AT-SPI / Orca
(Linux) are each verified before 1.0 — not merely "AccessKit is wired."

**Rationale:** accessibility is the one thing native menus give for free and a
custom-drawn popup risks losing entirely (a `tiny-skia` pixmap is an opaque
rectangle to a screen reader). AccessKit's *fidelity* varies per platform and per
AT, so a compiled bridge is necessary but not sufficient; the gate is a real human
pass on each. This is the second-hardest part of the project after Linux; see
[`30-accessibility.md`](30-accessibility.md).

## 5. Target platforms and rendering stack

| OS | Tray icon | Styled anchored popup | Context menu (`open_at`) | Menu bar | Screen reader |
|---|---|---|---|---|---|
| macOS | yes (`NSStatusItem`) | yes (button rect) | yes | native passthrough | VoiceOver |
| Windows | yes (`Shell_NotifyIcon`) | yes (`Shell_NotifyIconGetRect`) | yes | native passthrough | NVDA + Narrator |
| Linux | yes (SNI/AppIndicator) | **no** (`Unsupported::TrayAnchor`) | yes (pointer) | native passthrough | AT-SPI / Orca |

Rendering stack (all permissive-licensed, no GPU dependency):

- **`winit` 0.30** — windowing / event loop.
- **`softbuffer` 0.4** — a CPU framebuffer for the window.
- **`tiny-skia` 0.11** — 2D raster (rounded rects, hairlines, glyph/icon blit).
- **`cosmic-text` 0.12** — system-font lookup, shaping, layout.
- **`accesskit` 0.17** (+ `accesskit_winit` 0.23, platform adapters) — the
  screen-reader bridge, behind the `a11y` feature.

Why CPU raster and not a GPU toolkit: a popup must appear the instant the icon is
clicked and cannot absorb a GPU device/surface warm-up (~0.5s cold). CPU raster
has no such cost, produces a tiny binary, and gives full pixel control. Native
per-OS drawing was rejected because it cannot deliver one consistent custom look
and is ~3× the code. Full rationale in the roadmap; the *choice* is locked.

## 6. The Linux carve-out (honest verdict)

The modern Linux tray is **StatusNotifierItem / AppIndicator over D-Bus**. The
*host* (a GNOME extension, KDE plasmoid, or XEmbed shim) owns and draws the icon
in its own process and renders the menu from a `com.canonical.dbusmenu`
description the app exports. The application is **never told the icon's on-screen
rectangle and never receives the click coordinate** (`tray-icon` documents both
as *Unsupported* on Linux). Compounding it, **Wayland forbids a client from
positioning its own toplevel** by protocol (`set_outer_position` is a no-op).

So a tray-*anchored* styled popup is impossible on Linux/Wayland — for muri or
anyone. muri does not pretend otherwise. `LinuxAnchor::anchor_rect` returns
`Err(Unsupported::TrayAnchor)`; `Tray::run` surfaces it. The 1.0 Linux story is:

1. **Native-menu fallback** (recommended default): the same `Menu` spec rendered
   through a native `dbusmenu` / GTK tree. Loses the custom look, works everywhere
   a Linux tray works, and is accessible via AT-SPI for free.
2. **Pointer-anchored `ContextMenu::open_at(point, edge)`**: the styled surface
   *is* available wherever a pointer coordinate exists (a right-click menu), via
   `xdg_positioner` relative to the caller's own surface.

See [`22-platform-linux.md`](22-platform-linux.md).

## 7. The accessibility bar

1.0 must, for every target screen reader:

- expose the popup as a menu container and each row as a menu item / checkable
  menu item with a correct **name** (concatenated segment text), **enabled**,
  **checked**, **has-popup / expanded**, and **set-position** (`N of M` over
  *focusable* siblings only);
- move AT focus in lockstep with muri-owned keyboard navigation;
- apply an AT-initiated action (VoiceOver/NVDA activating or focusing an item)
  onto the live popup (move highlight, open flyout, or dispatch the row);
- be verified by a **human pass** per AT (decision #4).

The known-hard, currently-unresolved problem: a flyout submenu is a **separate OS
window** whose items are exposed as *nested nodes in the parent window's tree*,
and the flyout window is not independently adapted. Whether a screen reader
follows focus cleanly across that OS-window boundary is unverified and is a 1.0
gate item. See [`30-accessibility.md`](30-accessibility.md).

## 8. Incremental adoption — usable before 1.0

1.0 is the *full* target the owner will build. But **early embedders adopt muri as
soon as it is usable, not gated on 1.0.** usagio is the first embedder and already
has a `custom-popup` feature-flag seam so it can build against muri's macOS
surface today. The milestone ladder below defines "usable" checkpoints; the
pre-1.0 API-stability contract that lets an embedder build safely against a moving
crate is in
[`03-threading-events-versioning.md` §5](03-threading-events-versioning.md).

### Milestone ladder

| Milestone | Scope | Usable by whom | Status |
|---|---|---|---|
| **M0 — API + pure logic** | Full public data model as compiling types; pure, unit-tested `layout` / `theme` / `anchor` / `flyout` / `keynav` / `a11y`. | Nobody yet (no live surface). | **Shipped** |
| **M1 — macOS tray usable** | macOS `NSStatusItem` tray + styled popup + flyout submenus + mouse & keyboard nav + dark/light follow. muri's **native** API only (`Tray` / `Menu` / `ContextMenu` builders). No facade. | usagio behind its `custom-popup` flag, macOS only. | **Shipped** (VoiceOver device pass pending) |
| **M2 — macOS a11y + context menu** | VoiceOver pass on macOS; `ContextMenu::open_at` implemented; the flyout-a11y model resolved. | usagio ships the styled macOS popup as default; keeps native muda one release as a kill-switch. | 1.0-new |
| **M3 — muda facade + `Theme::native()`** | `muri::compat::muda` facade + global `MenuEvent` channel + `Theme::native()`, so a muda app drops in on macOS and looks native. | New muda-based adopters on macOS. | 1.0-new |
| **M4 — Windows backend** | `WS_EX_NOACTIVATE` popup anchored via `Shell_NotifyIconGetRect`; theme follow; outside-click dismiss; UIA; NVDA + Narrator passes. Same scene drawer, new shim. | Cross-platform adopters (mac + Win). | 1.0-new (groundwork shipped) |
| **M5 — Linux backend** | Native-menu fallback + pointer `ContextMenu`; AT-SPI / Orca pass; `Unsupported::TrayAnchor` honestly returned. | Linux adopters (fallback tray + styled context menus). | 1.0-new (skeleton) |
| **1.0** | All three platforms, all four screen readers verified, muda facade, hybrid menu bar, semver stability commitment. | Everyone; `s/muda/muri/` is a supported migration. | 1.0-new |

Key point for the reviewer: **M1–M2 deliver value to usagio without any of the
facade, Windows, or Linux work.** The facade (M3) and full platform matrix (M4–M5)
are what make 1.0, but early adoption does not wait for them. The order is
deliberately "prove the hardest thing (custom macOS popup + a11y) first, generalize
second."

## 9. Glossary

Precise definitions of the surface vocabulary used throughout the spec. A
**surface** is any distinct on-screen menu presentation muri (or, for passthrough,
the OS) puts up.

- **Tray icon** — the status-bar / notification-area glyph itself (`NSStatusItem`
  button on macOS, `Shell_NotifyIcon` icon on Windows, SNI/AppIndicator item on
  Linux). muri owns installing it (subsuming `tray-icon`).
- **Tray (popup) menu** — the styled menu muri draws *anchored to the tray icon*
  when it is clicked. Custom-drawn on macOS/Windows; **not available** on Linux
  (falls back to a native SNI/dbusmenu menu).
- **Context menu** — a styled menu muri draws at an **explicit screen point** (a
  right-click menu), via `ContextMenu::open_at(point, edge)`. Works anywhere a
  pointer coordinate exists — including Linux. muri's portable styled primitive.
- **Popup** — the generic term for any transient muri-drawn window (the tray menu
  and the context menu are both popups; a flyout is a nested popup). Borderless,
  always-on-top, non-activating where the OS allows.
- **Dropdown** — a popup anchored below (or flipped above) an arbitrary caller
  rectangle rather than the tray icon — e.g. a menu attached to a toolbar button.
  A 1.0-new surface (`Popup::anchored_to(rect, edge)`); see
  [`01-api-contract.md`](01-api-contract.md).
- **Flyout** — a nested submenu panel drawn *beside* its parent row in a second
  OS window (right by default, flipped left on spill). Opened by hover or by
  keyboard Right/Enter. Currently **one level deep** (`flyout.rs`,
  [`40-input-interaction.md`](40-input-interaction.md)).
- **Menu bar** — the OS application menu bar (macOS global top-of-screen menu; a
  Windows / GTK window menu bar) and the macOS system/status menus. **Passthrough
  to native** per decision #2; muri never draws it.

## 10. High-level architecture

```
   Consumer app
        │
        │  (A) muda drop-in                     (B) muri native API
        ▼                                            ▼
 ┌───────────────────────────┐            ┌───────────────────────────────┐
 │  muri::compat::muda facade │            │  Surfaces (public API, doc 01) │
 │  Menu/Submenu/MenuItem/... │──builds──► │  Tray · ContextMenu · Popup    │
 │  global MenuEvent channel  │            │  Menu/Item/Row/Segment/Style   │
 │  Theme::native() default   │            │  Theme/ThemeSource · MenuId    │
 │  routes menu-bar → native  │            └───────────────┬───────────────┘
 └──────────────┬────────────┘                            │
        native   │  custom                                 │  one unified
     passthrough │  surfaces                               │  MenuEvent channel
                 ▼                                          ▼
        ┌────────────────┐               ┌─────────────────────────────────────┐
        │  real native    │              │  Engine core (pure, portable, doc 10)│
        │  menu (NSMenu /  │              │  menu model · layout(Flex/Align) ·   │
        │  HMENU / GTK)    │              │  theme resolve · anchor · flyout ·   │
        │  via wrapped     │              │  keynav · a11y tree                  │
        │  muda            │              └──────────────────┬──────────────────┘
        └────────┬────────┘                                  │
                 │                          ┌────────────────▼────────────────┐
                 │                          │  Shared scene drawer (ONE impl)  │
                 │                          │  softbuffer + tiny-skia +        │
                 │                          │  cosmic-text  → Pixmap           │
                 │                          └────────────────┬─────────────────┘
                 │                                           │  platform shim:
                 │                                           │  anchor(rect/point),
                 │                                           │  dismiss, theme query,
                 │                                           │  a11y adapter
                 ▼                          ┌────────────────┼────────────────┐
          (OS draws it)                     ▼                ▼                ▼
                                       macOS backend    Windows backend   Linux backend
                                       NSStatusItem     Shell_NotifyIcon  SNI + no anchor
                                       + NSPanel        + NOACTIVATE win   → native fallback
                                       + NSAccessibility + UIA             + AT-SPI, pointer
                                                                            ContextMenu only
```

Four layers, top to bottom:

1. **Facade + native surfaces** — two entry points into the same engine. The
   facade (A) exists for migration; the native API (B) is where the flexibility
   unlock lives.
2. **Engine core** — pure, portable, exhaustively unit-testable: the menu data
   model, the `Flex`/`Align` layout, theme resolution, popup/flyout placement
   math, the keyboard-nav state machine, and the accessibility tree. No I/O, no
   platform calls. This is most of the crate and most of the tests.
3. **Shared scene drawer** — one CPU-raster implementation used on every OS.
4. **Platform backends** — thin per-OS shims for anchoring, dismiss, theme query,
   and the AccessKit adapter, plus the wrapped-native-muda passthrough path for
   menu-bar surfaces (decision #2).

The invariant: **everything above the platform backends is portable and tested
without a window server; each backend is only a shim.** That is what keeps three
platforms tractable in one crate.
