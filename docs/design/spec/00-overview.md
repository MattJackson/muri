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

## 2. The vision (north star) and the design pivot

> "Anyone on `muda` can move to muri and have their product look the **same** with
> minimal code changes, but suddenly gain massive flexibility to change the look
> however they want — vs 'this is what it is'."

**The design pivot (owner's direction, and the frame for this whole spec):** design
the **RIGHT API for muri first** — shaped for muri's actual use, a *live* app (like
usagio) that rewrites its menu ~every 0.75s — **and if it maps cleanly onto muda,
great.** muri's **own native API is the primary, first-class surface** ([`01`](01-api-contract.md)):
a fluent, owned `Menu::new().add(…)` builder with per-item styling and a native,
typed event model (a `.on(callback)` per item and/or a muri event stream keyed by
id), driven by a retained `TrayHandle` you keep and call to update the menu at
runtime. This is what the feature layer is built around; it is not a byproduct of
mimicking muda.

**muda compatibility is a first-class adoption on-ramp built *on top of* the native
API — not a constraint that shapes it.** Because the native item model maps **1:1
onto muda's item kinds** (§`01`), the compatibility layer (`muri::compat::muda`,
[`02`](02-muda-compat.md)) is a mechanical wrapper that lets an existing muda app
migrate by changing imports (`use muri::compat::muda as muda;`) and **compile, run,
and look native by default** (`Theme::native()` with real vibrancy). Crucially the
compat layer **preserves muda's passive integration model** — the host keeps
owning its own event loop and reads the global `MenuEvent` channel, exactly like
muda — so a drop-in does **not** force the app onto muri's `Tray::run` / `TrayHandle`
model. Two doors into one engine:

- the **native API** — the feature layer: the `add(…)` builder, per-item styling,
  the retained handle, and typed `.on()` callbacks (see [`01`](01-api-contract.md));
- the **muda-compat layer** — the simple, faithful drop-in that rides on the same
  engine and progressively unlocks the native features (see [`02`](02-muda-compat.md)
  and the worked migration in [`60`](60-migration-guide.md)).

"Drop in simply, then progressively add features" is the adoption story;
"the right native API for a live menu app" is the design story. Both are load-bearing.

## 3. Goals and non-goals

### Goals (1.0)

- **A native-first, feature-rich API** (locked decision #1, the pivot): a fluent
  owned `Menu::new().add(…)` builder with per-item styling and a native typed event
  model, driven by a retained `TrayHandle` for live runtime updates — shaped for a
  live menu app, not for mimicking muda. See [`01`](01-api-contract.md).
- **One consistent, fully custom-drawn look** on macOS, Windows, and Linux for the
  surfaces muri owns (tray, context menu, dropdown/popup, flyout submenus).
- **A first-class muda drop-in on-ramp** (locked decision #1): `muri::compat::muda`
  + `Theme::native()` + the global `MenuEvent` channel, so an unchanged muda app
  compiles, runs, and looks native — built *on* the native API, riding on it.
- **Hybrid menu bar** (locked decision #2): custom surfaces are muri; the OS
  application menu bar / system menu passes through to native.
- **Full presentation control**: per-item and per-segment alignment and multi-column
  rows; literal or semantic colors; fonts (family / size / weight); leading &
  trailing icons/logos (PNG or SVG bytes); enabled / checked state; separators;
  section headers; **N-level nested submenus** as flyout panels (locked decision #8);
  a full `Theme` override surface; and an **extension seam** (`MenuNode` trait) so
  new content kinds are additive over 1.x (§`00.11`, [`01`](01-api-contract.md)).
- **Native events, re-implemented** (locked decision #5): muri owns native
  menu-bar handling and event emission; no dependency on the muda crate for native
  events.
- **Follows OS dark/light + accent by default**, HiDPI-crisp, opens with no
  perceptible delay (CPU raster, no GPU warm-up), and **`Theme::native()` renders
  with real translucent vibrancy** (locked decision #6) — macOS `NSVisualEffectView`,
  Windows acrylic — not an opaque approximation.
- **Accessibility as a first-class 1.0 gate, on by default** (locked decisions #4 &
  #7): a published parallel a11y tree bridged through AccessKit to NSAccessibility /
  UIA / AT-SPI, plus muri-owned keyboard navigation, verified against **all** target
  screen readers, with the `a11y` feature (and `accesskit`) enabled by default.

### Non-goals (1.0 and beyond)

- **Not a general GUI framework.** No arbitrary widget trees, forms, text inputs,
  tabs, or scroll-views. muri draws *menus* — lists of rows, possibly nested.
- **Not a replacement for application windows / dialogs.** Text entry (e.g.
  paste-an-API-key) belongs in a normal window the host owns; a non-activating
  popup deliberately cannot host a first-responder text field.
- **Not a fork of muda.** muda is a data-model sync over native menu objects;
  there is no drawable layer to fork. Custom drawing means bypassing native menus
  entirely — which is exactly what muri does. The **application menu bar** still
  passes through to a *native* OS menu (AppKit/Win32/GTK) per decision #2, but muri
  drives that itself and does **not** depend on the muda crate for native events
  (decision #5); the compat layer maps muda's *API surface* onto muri, it does not
  re-host muda's engine.
- **Not a global-hotkey provider.** Accelerator *text* is displayed and in-menu
  mnemonics are handled, but system-wide hotkeys are out of scope (that is the
  `global-hotkey` crate's job). See [`40-input-interaction.md`](40-input-interaction.md).
- **Not a tray-anchored styled popup on Linux.** Architecturally impossible on
  SNI/AppIndicator + Wayland; muri says so honestly (decision #3, §6 below).

## 4. Locked decisions and their rationale (the single source of truth)

These are **fixed for 1.0**. Every other document designs *to* them and does not
relitigate them; where a section spec had a provisional/⚠-owner-confirm flag for
one of these, that flag is removed. Decisions #1–#4 keep their reference numbers
from the original spec (with #1 reframed by the pivot); #5–#8 are the pivot's newly
locked decisions.

| # | Locked decision |
|---|---|
| 1 | **Native-first API; muda-compat is a first-class drop-in built on it** (the pivot). |
| 2 | **Hybrid menu bar** — custom surfaces muri, app menu bar passthrough to native. |
| 3 | **All three platforms** ship in 1.0 (macOS, Windows, Linux). |
| 4 | **All screen readers verified** in 1.0 (VoiceOver, NVDA, Narrator, Orca). |
| 5 | **Native events, re-implemented** — no dependency on the muda crate for native events. |
| 6 | **`Theme::native()` requires real vibrancy** (macOS `NSVisualEffectView`, Windows acrylic). |
| 7 | **`a11y` on by default** — `accesskit` is a default dependency. |
| 8 | **N-level nested submenus** in 1.0 (flyout is a stack, not one level). |

### Decision 1 — Native-first API; muda-compat is a first-class drop-in built on it

**What:** muri's **own native API is the primary, first-class design surface**
([`01`](01-api-contract.md)) — a fluent owned `Menu::new().add(impl Into<Item>)`
builder with per-item styling (`.color()`/`.align()`/`.icon()`/`.on()`/…), a native
typed event model (per-item `.on(callback)` and/or a muri event stream keyed by id),
N-level submenus, and a retained `TrayHandle` for live runtime updates. muda
compatibility is **not** a hard requirement that shapes that API; it is a
**first-class, faithful drop-in on-ramp** (`muri::compat::muda`, [`02`](02-muda-compat.md))
built *on top of* the native engine. Because the native item model maps **1:1 onto
muda's item kinds**, the compat layer is a mechanical wrapper: an unchanged muda app
changes its imports and **compiles, runs, and looks native** (`Theme::native()`
with vibrancy). The compat layer **preserves muda's passive model** — the host keeps
its own event loop and the global `MenuEvent::receiver()` channel; it is not forced
onto `Tray::run` / `TrayHandle`.

**Rationale:** two goals, one engine. (a) *Design* — muri exists for a live app that
rewrites its menu ~0.75s and wants rich per-item styling; that app deserves the
right API, not muda's shape bent onto a renderer. (b) *Adoption* — the only realistic
path for muda users is a simple, faithful drop-in *first*, then progressive feature
adoption. Both are served by making the native API primary and the muda mapping ride
on top, rather than treating muda-parity as the design constraint. The pivot resolves
the old tension where `s/muda/muri/` would silently change the app's event-loop model:
the drop-in stays passive (muda-shaped), and only a consumer who *opts into* the
native API adopts the handle+callback model.

**Tension it resolves:** muda's event model is a **global crossbeam channel**
(`MenuEvent::receiver()`); muri's native model is **typed per-item `.on()` callbacks
plus a muri event stream**. 1.0 does **not** force the native API into muda's global
channel: the native surface fires typed callbacks (and a native stream), and the
**compat layer** exposes the global `MenuEvent` channel by mapping muri's native
events onto it (decision #5, [`03`](03-threading-events-versioning.md) §3). Native
consumers never touch the global channel; drop-in consumers get it for free.

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

### Decision 5 — Native events, re-implemented (no muda-crate dependency)

**What:** muri owns its **native** menu-bar handling and event emission end-to-end;
1.0 does **not** depend on the `muda` crate for native events. The native surface
emits typed per-item `.on()` callbacks and a muri event stream keyed by `MenuId`.
The optional compat shim **maps muri's native events onto muda's global-channel
shape** (`MenuEvent::receiver()`), so a drop-in app's channel loop still works.

**Rationale:** the earlier design imagined *wrapping* muda's native menus for the
passthrough menu bar and *bridging* muda's channel into muri's — two channels joined
by a forwarder thread ([`03`](03-threading-events-versioning.md) called this "the
subtlest correctness risk"). Owning emission natively removes that seam entirely for
muri's own surfaces: there is one native event source, and the compat channel is a
thin projection of it. (The **native application menu bar** is still OS-owned and
passes through per decision #2; that is an AppKit/Win32/GTK menu, not a dependency on
the muda crate's event machinery.)

### Decision 6 — `Theme::native()` requires real vibrancy

**What:** on macOS, `Theme::native()` **must** render with true translucent vibrancy
for 1.0 — the raster layer is hosted over an `NSVisualEffectView` so the panel is a
real blurred material, not an opaque fill. The Windows equivalent is **acrylic**; on
Linux the styled surface is opaque (compositor-owned blur is not client-controllable)
and the native-menu fallback is drawn by the host (n/a).

**Rationale:** "looks native" is a load-bearing promise of the drop-in on-ramp
(decision #1). A solid-color menu next to a real macOS menu reads as *not native*;
vibrancy is the difference. This was previously booked as a permanent divergence
(old D1) and a possible future enhancement; the pivot promotes it to a requirement.
It forces a **transparency/compositing rework**: the shipped drawer blits opaque
(un-premultiplied over the theme background); 1.0 adds an alpha-preserving present
path so the effect view shows through, and the macOS backend hosts the surface over
the effect view. See [`10`](10-rendering-layout.md) §11 and [`20`](20-platform-macos.md) §2.

### Decision 7 — `a11y` on by default

**What:** the `a11y` cargo feature is **on by default** in 1.0; `accesskit` (+
`accesskit_winit` + platform adapters) is a default dependency. The flag survives so
a size-constrained embedder can drop it, but the default flips to include it.

**Rationale:** decision #4 makes accessibility non-optional, and a custom-drawn menu
that is default-inaccessible would make the plain `s/muda/muri/` drop-in *silently
lose* the accessibility muda gave for free — a regression that violates the spirit of
decision #1. The parallel a11y tree is the only thing that gives a `tiny-skia` pixmap
correct semantics, so it ships in the default build. See
[`30-accessibility.md`](30-accessibility.md) §0 and
[`03`](03-threading-events-versioning.md) §4.

### Decision 8 — N-level nested submenus

**What:** 1.0 supports **arbitrarily deep** nested submenus, not a single flyout
level. A `Submenu` is itself `.add()`-able into a `Submenu`, so nesting is inherent
in the builder; at runtime the flyout is a **stack** of popup windows (one OS window
per open level), the keyboard-nav focus is a stack, and dismiss/hover generalize
across the stack.

**Rationale:** real menus nest, and the muda drop-in must not silently flatten a
muda menu that nests >1 deep (the earlier plan's facade-flattening was a fidelity
hole). Making nesting inherent in the `add(…)` builder means the native API and the
compat mapping both get depth for free. The real cost is **accessibility**: N levels
multiply the cross-OS-window screen-reader traversal problem (decision #4, §7 below),
which [`30`](30-accessibility.md) owns. See [`40`](40-input-interaction.md) §5
(flyout-stack state machine) and the platform docs (each manages a stack of popup
windows).

## 5. Target platforms and rendering stack

| OS | Tray icon | Styled anchored popup | Context menu (`open_at`) | Menu bar | Screen reader |
|---|---|---|---|---|---|
| macOS | yes (`NSStatusItem`) | yes (button rect) | yes | native passthrough | VoiceOver |
| Windows | yes (`Shell_NotifyIcon`) | yes (`Shell_NotifyIconGetRect`) | yes | native passthrough | NVDA + Narrator |
| Linux | yes (SNI/AppIndicator) | **no** (`Unsupported::TrayAnchor`) | yes (pointer) | native passthrough | AT-SPI / Orca |

Rendering stack (all permissive-licensed, no GPU dependency) — **historical, being
superseded; see §5.1 and [ADR-0001](../adr/0001-own-the-menu-use-engines-not-frameworks.md)**:

- **`winit` 0.30** — windowing / event loop.
- **`softbuffer` 0.4** — a CPU framebuffer for the window.
- **`tiny-skia` 0.11** — 2D raster (rounded rects, hairlines, glyph/icon blit).
- **`cosmic-text` 0.12** — system-font lookup, shaping, layout.
- **`accesskit` 0.17** (+ `accesskit_winit` 0.23, platform adapters) — the
  screen-reader bridge, behind the `a11y` feature, which is **on by default**
  (decision #7).

On macOS the styled surface is hosted over an `NSVisualEffectView` for real
translucent vibrancy (decision #6); the Windows equivalent is acrylic.

Why CPU raster and not a GPU toolkit: a popup must appear the instant the icon is
clicked and cannot absorb a GPU device/surface warm-up (~0.5s cold). CPU raster
has no such cost, produces a tiny binary, and gives full pixel control. Native
per-OS drawing was rejected because it cannot deliver one consistent custom look
and is ~3× the code. Full rationale in the roadmap; the *choice* is locked.

### 5.1 Rendering stack: UNLOCKED (dependency diet — see ADR-0001)

**Update (2026-09-09).** The concrete crate stack in §5 above (`winit` + `softbuffer`
+ `tiny-skia` + `cosmic-text`, and `accesskit_winit`) is **no longer locked**. It is
retained above as historical context and is **superseded** by the *dependency-diet*
direction recorded in
[**ADR-0001 — Own the menu, use engines not frameworks**](../adr/0001-own-the-menu-use-engines-not-frameworks.md).
What is *unchanged and still locked*: CPU raster (no GPU warm-up), **one shared scene
drawer** used identically on every OS, and all eight §4 decisions (vibrancy,
a11y-on-by-default, N-level submenus, all three platforms, all four screen readers).
The diet changes **which crates** produce those pixels, not the architecture above the
platform shims.

**Principle:** *own the menu system and the thin platform glue; **use** — do not
reinvent — the deep engines.* Reinventing a text shaper (`rustybuzz`, a HarfBuzz port),
a glyph rasterizer (`swash`), or the a11y platform bridges (`accesskit`) would be
multi-year, buggier, slower work. The weight problem is the general-purpose
**frameworks** wrapped around those engines, where muri uses only a sliver.

**Target end-state (by 1.0):** depend on a handful of **engine** crates + raw OS FFI,
owning all menu logic + rendering glue + windowing. Concretely:

- **Text:** replace `cosmic-text` (43 transitive crates; it sets the MSRV floor at
  **1.85** via a non-optional `unicode-segmentation 1.13.3`) with `rustybuzz` +
  `swash` + `fontdb` used directly, plus a **muri-owned shaping-glue + font-fallback
  layer (~300 lines)** that preserves cross-script fallback + color-emoji — **zero
  menu-relevant feature loss**. Result: **MSRV 1.85 → ~1.73**, ≈**14 fewer crates**.
  The `resolve_ui_family` bold-fallback invariant ([`10` §7.1](10-rendering-layout.md))
  ports cleanly (its only `cosmic-text` touchpoint is `fs.db().face().families`).
- **Raster:** replace `tiny-skia` with a **small in-house blitter** (fill rect,
  anti-aliased rounded-rect, alpha blit — the only primitives muri uses) and rework
  icon PNG handling to shed the ~9-crate png/zlib subtree; guard with **golden-image
  tests**.
- **Windowing:** go **native per-OS** and remove `winit` (~18 exclusive crates, and it
  drags a **duplicate `objc2` 0.5** stack) — folded **into** the already-mandatory
  non-activating-`NSPanel` + vibrancy rewrite on macOS, so windowing is rewritten
  **once, not twice**. Drop `softbuffer` + `raw-window-handle` (present via
  CALayer / CoreGraphics). Rebuild the a11y bridge on `accesskit_macos` /
  `accesskit_windows` / `accesskit_unix` directly (**drop `accesskit_winit`**). Windows
  uses a native Win32 message pump; Linux uses `xdg_popup` with the documented Wayland
  tray-anchor carve-out (§6).
- **Keep (do not reinvent):** engines `swash`, `rustybuzz`, `fontdb`, `accesskit`; raw
  OS FFI `objc2` / `objc2-foundation` / `objc2-app-kit`, `windows-sys`.

**Sequencing.** This is a **multi-release journey**, not one commit. The diet lands
across the milestone ladder (§8): **M1** does the macOS native-windowing + text/raster
swap; **M4/M5** carry it to Windows/Linux. Every step ships **usable + green**, and
**macOS stays green throughout**. **1.0 = diet complete + all three platforms + all
four screen readers verified.** The **Tier-1 data model + Tier-2 surface API**
([`03` §5](03-threading-events-versioning.md)) stay stable through the diet, so for
usagio (the early embedder / real-world tester of the macOS native surface) the diet
is **internal-only churn**. Full context, measured facts, and consequences are in
[ADR-0001](../adr/0001-own-the-menu-use-engines-not-frameworks.md).

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
| **M1 — macOS native API solid** | macOS `NSStatusItem` tray + styled popup + **N-level** flyout stack + mouse & keyboard nav + dark/light follow; the **native `add(…)` builder**, per-item styling + `.on()`, the retained **`TrayHandle`** for live updates, the native event stream, and **`Theme::native()` vibrancy** (`NSVisualEffectView`). muri's **native** API; no compat layer. | usagio behind its `custom-popup` flag, macOS only. | Partly shipped (builder/handle/vibrancy = 1.0-new; VoiceOver pass pending) |
| **M2 — macOS a11y + context menu** | VoiceOver pass on macOS; `ContextMenu::open_at` implemented; the flyout-stack a11y model resolved (per-level adapters). | usagio ships the styled macOS popup as default; keeps native muda one release as a kill-switch. | 1.0-new |
| **M3 — muda-compat drop-in + fidelity** | `muri::compat::muda` (passive-model drop-in: host keeps its loop + global `MenuEvent` channel + `init_for_*`) mapped 1:1 onto the native builder, so an unchanged muda app compiles, runs, and looks native (`Theme::native()` vibrancy). | New muda-based adopters on macOS via `s/muda/muri/`. | 1.0-new |
| **M4 — Windows backend** | `WS_EX_NOACTIVATE` popup anchored via `Shell_NotifyIconGetRect`; theme follow + **acrylic** vibrancy; outside-click dismiss; N-level flyout stack; UIA; NVDA + Narrator passes. Same scene drawer, new shim. | Cross-platform adopters (mac + Win). | 1.0-new (groundwork shipped) |
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
- **Flyout** — a nested submenu panel drawn *beside* its parent row in a separate
  OS window (right by default, flipped left on spill). Opened by hover or by
  keyboard Right/Enter. **N levels deep** (decision #8): each open level is its own
  OS window, forming a **flyout stack** (`flyout.rs`,
  [`40-input-interaction.md`](40-input-interaction.md) §5).
- **Menu bar** — the OS application menu bar (macOS global top-of-screen menu; a
  Windows / GTK window menu bar) and the macOS system/status menus. **Passthrough
  to native** per decision #2; muri never draws it.

## 10. High-level architecture

```
   Consumer app
        │
        │  (A) muri NATIVE API (primary)        (B) muda-compat drop-in (rides on A)
        ▼                                            ▼
 ┌───────────────────────────────┐        ┌───────────────────────────┐
 │  Surfaces (public API, doc 01) │        │  muri::compat::muda        │
 │  Tray + TrayHandle · Context   │◄─maps──│  Menu/Submenu/MenuItem/... │
 │  Menu/Popup · Menu.add(Item)   │  1:1   │  global MenuEvent channel  │
 │  per-item style · .on() + stream│        │  Theme::native() default   │
 │  MenuNode extension trait      │        │  init_for_* → native bar   │
 └───────────────┬───────────────┘        │  (host keeps its own loop) │
                 │  native typed events    └─────────────┬─────────────┘
                 │  (callbacks + stream);          native │  custom
                 │  compat projects them          passthru│  surfaces
                 │  onto the global channel               │
                 ▼                          ┌─────────────┼───────────────────────┐
 ┌─────────────────────────────────────┐   │             ▼                        │
 │  Engine core (pure, portable, doc 10)│   │   ┌────────────────┐                 │
 │  menu model · layout(Flex/Align) ·   │◄──┘   │  OS app menu    │  (menu bar only,│
 │  theme resolve · anchor · flyout     │       │  bar (NSMenu /   │   decision #2 — │
 │  STACK · keynav · a11y tree          │       │  HMENU / GTK)    │   OS-owned, not │
 └──────────────────┬──────────────────┘       └────────┬────────┘   the muda crate)│
                    │                          └─────────┼───────────────────────┘
     ┌──────────────▼──────────────────┐                 ▼
     │  Shared scene drawer (ONE impl)  │           (OS draws it)
     │  softbuffer + tiny-skia +        │
     │  cosmic-text → Pixmap (alpha-    │
     │  preserving; hosted over an      │
     │  NSVisualEffectView on macOS)    │
     └────────────────┬─────────────────┘
                      │  platform shim: anchor(rect/point), dismiss,
                      │  theme query, vibrancy backdrop, a11y adapter
      ┌───────────────┼───────────────┐
      ▼               ▼               ▼
 macOS backend    Windows backend   Linux backend
 NSStatusItem     Shell_NotifyIcon  SNI + no anchor
 + NSPanel        + NOACTIVATE win   → native fallback
 + NSVisualEffect + acrylic          + AT-SPI, pointer
 + NSAccessibility + UIA             ContextMenu only
```

Four layers, top to bottom:

1. **Native surfaces (primary) + muda-compat door.** Two entry points into the same
   engine. The native API (A) is primary — the builder, per-item styling, the
   retained handle, and typed events are the flexibility unlock. The muda-compat door
   (B) is a first-class drop-in that maps 1:1 onto (A) and preserves muda's passive
   loop + global channel; it *rides on* the native API, never reshapes it. muri emits
   native events itself (decision #5) and only *projects* them onto the global channel
   for the compat door; the OS application menu bar is passthrough (decision #2) and
   is a native AppKit/Win32/GTK menu, not a dependency on the muda crate.
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

## 11. Extensibility & roadmap — `add(…)` is a permanently stable door

The native builder's `Menu::add(impl Into<Item>)` is designed to be the **one stable
door** that never has to change as muri grows new content kinds. Two mechanisms make
that true and are specified in [`01`](01-api-contract.md):

- **`Item` is `#[non_exhaustive]`.** 1.0's built-in nodes — **Text, Image,
  Separator, Submenu, Check, Predefined** — are the kinds the renderer handles
  first-class. Promoting a future kind (e.g. `Video`) to a blessed built-in variant
  later is **not** a breaking change.
- **The `MenuNode` extension trait ships in 1.0** (even though rich nodes do not).
  It exposes the row contract — measure (constraints → layout), draw (into the
  `tiny-skia` scene), hit-test, and accessibility (role/name/state → the `AxNode`
  model in [`30`](30-accessibility.md)). `Item::Custom(Box<dyn MenuNode>)` lets an
  arbitrary third-party or future row plug into the **same `add(…)`** and the **same
  render / keynav / a11y pipeline**. Per-row dyn dispatch is fine — menus have few
  rows.

**1.0 scope** is "the builder + the core nodes + the extension trait." **1.x
roadmap** (additive, no breaking change, built on the 1.0 `MenuNode` trait): rich
content nodes such as **Video** and other embedded media. The muda-compat layer only
ever maps muda's kinds onto the **built-in** nodes; it never touches the extension
seam.
