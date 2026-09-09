# muri 1.0 — Foundational Design Specification

This directory is the **authoritative, spec-for-spec design of muri 1.0**. It
supersedes the narrative roadmap (`docs/design/muri.md` in the usagio /
claude-usage repo) as the *contract* the implementation is measured against. The
roadmap explains *why muri exists and how it grew*; these documents define *what
1.0 must be and must guarantee*.

muri is a cross-platform, fully-styleable tray-icon + popup-menu crate: a
custom-drawn replacement for the `muda` + `tray-icon` pairing. Where those crates
sync a data model onto *native* OS menu objects (`NSMenu` / Win32 `HMENU` /
GTK menus) and inherit the OS look, muri **draws menus itself** on a portable CPU
raster surface (`winit` + `softbuffer` + `tiny-skia` + `cosmic-text`), giving the
consumer total control of alignment, color, font, logo, and layout — identical
across OSes.

## The 1.0 north star (owner's words) and the pivot

> "Anyone on `muda` can move to muri and have their product look the **same** with
> minimal code changes, but suddenly gain massive flexibility to change the look
> however they want — vs 'this is what it is'."

**The design pivot:** design the **RIGHT native API for muri first** — for a *live*
app (usagio) that rewrites its menu ~0.75s — and let muda compatibility ride on top.
muri's own native API is the **primary, first-class** surface (a fluent
`Menu::new().add(…)` builder, per-item styling, a retained `TrayHandle`, native
typed events). muda-compat (`muri::compat::muda`) is a **first-class, faithful
drop-in on-ramp** built on it: `s/muda/muri/` compiles, runs, and looks native
(vibrancy included), preserving muda's passive event-loop model — then the consumer
progressively adopts muri's native features.

## Locked 1.0 decisions

These are **fixed**. The specs design *to* them; they are not relitigated. Full
rationale and the single source of truth are in [`00-overview.md` §4](00-overview.md).

1. **Native-first API; muda-compat is a first-class drop-in built on it** (the pivot).
2. **Hybrid menu bar** — muri custom-draws tray/context/dropdown/popup menus; the
   OS **application menu bar / system menu** passes through to a real native menu
   (which muri installs itself, not via the muda crate).
3. **All three platforms ship in 1.0**: macOS, Windows, Linux (Linux via a
   pointer-anchored context-menu path + native-menu fallback, since tray-anchoring
   is architecturally unsupported there).
4. **All screen readers verified in 1.0**: VoiceOver (macOS), NVDA + Narrator
   (Windows), AT-SPI / Orca (Linux).
5. **Native events, re-implemented** — no dependency on the muda crate for native
   events; the compat layer projects muri's native events onto the global channel.
6. **`Theme::native()` requires real vibrancy** — macOS `NSVisualEffectView`,
   Windows acrylic (not an opaque approximation).
7. **`a11y` on by default** — `accesskit` is a default dependency.
8. **N-level nested submenus** in 1.0 — the flyout is a stack, not one level.

## Build handoff

New to this repo and implementing 1.0? Start at [`HANDOFF.md`](HANDOFF.md) — the
build-team kickoff, the decision log (the "why"), and the current state of the code.

## Incremental adoption (usable before 1.0)

1.0 is the *full* target, but early embedders (usagio first) adopt muri **as soon
as it is usable**, not gated on 1.0. The milestone ladder and the pre-1.0
API-stability contract that lets them build safely against the macOS surface today
are specified in [`00-overview.md` §8](00-overview.md) and
[`03-threading-events-versioning.md` §5](03-threading-events-versioning.md).

## How to read this spec

Each document is self-contained but cross-referenced. If you are implementing or
reviewing one area, read its document plus [`00-overview.md`](00-overview.md) for
the shared vocabulary and architecture.

---

## Section index

### Foundation (this pass)

#### [`00-overview.md`](00-overview.md) — Mission, decisions, architecture
Mission and the vision above; goals / non-goals; the four locked decisions and
their rationale; target platforms and the accessibility bar; the incremental
adoption milestone ladder; a glossary of surface terms (tray, popup, context
menu, dropdown, flyout, menu bar, surface); and the high-level architecture
diagram (engine core → surfaces → platform backends → facade). **Read this
first.**

#### [`01-api-contract.md`](01-api-contract.md) — muri's native API (the north star)
The **primary, first-class** public API: the fluent owned `Menu::new().add(impl
Into<Item>)` builder; the item kinds (`Text` / `Image` / `Separator` / `Submenu` /
`Check` / `Predefined`) with per-item modifiers (`.color`/`.align`/`.icon`/`.on`/…);
the `MenuNode` extension seam for future/third-party rows; native typed events
(per-item `.on()` + a muri stream, *not* muda's global channel); the retained
`TrayHandle` for live runtime updates; N-level submenus; the `Theme` / `Color` /
`Font` surface and the lower-level `Row`/`Segment` flush-right primitive; ownership,
lifetimes, and exact signatures grounded in the shipped code.

#### [`02-muda-compat.md`](02-muda-compat.md) — muda compat: the first-class drop-in on-ramp
How `muri::compat::muda` lets an unchanged muda app change imports and **compile,
run, and look native**, built *on top of* the native API (each muda kind maps 1:1
onto a native constructor). Enumerates muda's public API (`Menu`, `Submenu`,
`MenuItem`, `CheckMenuItem`, `IconMenuItem`, `PredefinedMenuItem`, `Accelerator`,
`MenuId`, `MenuEvent` + global receiver, `ContextMenu`/`IsMenuItem`, `init_for_*`,
`show_context_menu_for_*`) **and the `tray-icon` surface muri subsumes**; specifies
that the layer **preserves muda's passive event-loop model** (does not force
`Tray::run`/`TrayHandle`), the surface routing (custom muri vs native passthrough per
decision #2), and the honest MAP / PARTIAL / CANNOT divergence register.

#### [`03-threading-events-versioning.md`](03-threading-events-versioning.md) — Threading, events, versioning
muda's threading constraints (menus/events on the main/UI thread; the global
event channel) vs muri's threading model; muri's native event emission for **all**
surfaces it draws (custom + the native menu bar it installs) through **one**
`dispatch` (decision #5 — no wrapped-muda channel, no forwarder), projected onto the
global channel for the compat door; the pre-1.0 early-embedder stability contract;
the semver / API-stability
policy for 1.0; the feature-flag matrix; MSRV; dependency stance
(winit / softbuffer / tiny-skia / cosmic-text / accesskit versions); and the error
model.

### Rendering & platform (section agents)

#### [`10-rendering-layout.md`](10-rendering-layout.md) — The shared scene drawer
**Brief:** Specify the `SceneDrawer` trait and the one `RasterDrawer`
implementation; the two-pass `render_menu` layout (intrinsic width pass, draw
pass) and the `Flex::Grow` + `Align::Right` flush-right / no-chevron-column
guarantee that is muri's reason to exist; the `LaidMenu` / `LaidRow` hit-test map;
DPI / device-scale handling (`RasterDrawer::new(scale)`, per-window
`scale_factor`); multi-monitor work-area clamping (defer placement math to
`anchor.rs` / `flyout.rs`, documented in the platform specs); text shaping via
`cosmic-text` including the pinned-UI-family fix (`resolve_ui_family`,
`family_shapes_both_weights` — regular and bold must shape in the *same* face);
IME and RTL/BiDi text handling (currently unaddressed — call out what 1.0 must add
for `StyleRun` UTF-16 offsets to survive bidi reordering); `StyleRun` UTF-16
offset semantics; icon decode/scale (PNG decode-once, SVG rasterize-per-DPI —
note SVG/`Symbol` icons are **not yet drawn**, `draw_image` is nearest-neighbour).
Must reconcile the theme-independent `Theme::light()` used inside `style_pieces`
against the live theme (a known seam).

#### [`20-platform-macos.md`](20-platform-macos.md) — macOS backend (shipped)
**Brief:** Spec the shipped macOS backend as the reference: `NSStatusItem` tray via
`objc2` (`MacosAnchor`, the `MuriTrayTarget` click target, `EventLoopProxy`
bridge); the borderless `winit` + `softbuffer` popup and second flyout window;
tray anchoring through `anchor_rect` (button window frame, Y-flip to top-left
space) → `place_popup`; `ContextMenu::open_at` (still `todo!()`); the app-menu-bar
**native passthrough** seam (decision #2 — `NSApplication` main menu is *not*
muri's; the facade forwards `init_for_nsapp` to native muda; muri sets
`ActivationPolicy::Accessory` for pure tray apps, which conflicts with a
menu-bar app — resolve this); the event loop and `ApplicationHandler`; multi-window
focus-set dismiss (`focused: HashSet<WindowId>`, close-all only when empty); the
non-activating-panel requirement the roadmap flags (winit `WindowLevel::AlwaysOnTop`
today vs a true non-activating `NSPanel`) and whether it steals focus / can host a
first responder. Call out every place AppKit fights a borderless winit surface.

#### [`21-platform-windows.md`](21-platform-windows.md) — Windows backend (groundwork)
**Brief:** Spec the Windows backend from the compile-verified groundwork:
`WindowsAnchor` (message-only window owning a `Shell_NotifyIcon` `NIM_ADD` icon,
`Shell_NotifyIconGetRect` → logical via `GetDpiForWindow`, `NIM_DELETE` on
`Drop`); `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` layered popup; `popup_origin`
(`Edge::Top` for bottom taskbars) via shared `place_popup`; outside-click dismiss
via `WH_MOUSE_LL` + `WM_ACTIVATEAPP` (bare `Focused(false)` unreliable); PNG →
`HICON` decode (still `todo!()`); UIA a11y via `accesskit_windows`; the window
menu-bar passthrough (`init_for_hwnd` → native). Specify the still-`todo!()` popup
event loop and how it shares `App`-like state with macOS.

#### [`22-platform-linux.md`](22-platform-linux.md) — Linux backend (skeleton + carve-out)
**Brief:** Spec the honest Linux carve-out. `LinuxAnchor::anchor_rect` returns
`Unsupported::TrayAnchor` (SNI / AppIndicator host owns the icon; no geometry, no
click coordinate; Wayland forbids client toplevel positioning). Define the two
supported paths: (1) **native-menu fallback** — render the same `Menu` spec
through a native `dbusmenu` / GTK tree (a11y free via AT-SPI); (2)
**pointer-anchored `ContextMenu::open_at`** — the styled surface where a pointer
coordinate exists, via `xdg_positioner` relative to the caller's own surface.
Spec tray install (SNI registration — currently `todo!()`), the app-menu-bar
passthrough (`init_for_gtk_window` → native GTK), AT-SPI via `accesskit_unix`, and
the `ClientPositioning` unsupported case. State plainly what Linux will and will
not promise.

#### [`30-accessibility.md`](30-accessibility.md) — Accessibility (all-SR gate)
**Brief:** Spec the pure `a11y` tree (`AxTree` / `AxNode` / `AxRole`,
`build_tree`, `focused_id`, `locate`, `set_expanded`, `announcement`) and its
mapping from the declarative `Menu` (popup → `Menu`, row → `MenuItem` /
`MenuItemCheckbox`, header → `GroupLabel`, submenu → `has_popup` + `expanded`
child, `pos_in_set` / `set_size` over focusable siblings only). Spec the per-platform
AccessKit bridge (`a11y::accesskit::tree_update` → `TreeUpdate`; `accesskit_macos`
/ `accesskit_windows` / `accesskit_unix`); the shipped macOS adapter attach
(`accesskit_winit::Adapter`, create-hidden-then-show, action-request → live popup).
**The hard unresolved problem to attack:** the **flyout is a separate OS window**
and is *not* independently adapted — its items live as nested nodes in the parent
window's tree, so a screen reader crossing an OS-window boundary into a flyout is
unverified. Define the multi-window-flyout a11y model and the all-SR verification
plan (VoiceOver, NVDA, Narrator, Orca) as a 1.0 gate.

#### [`40-input-interaction.md`](40-input-interaction.md) — Input & interaction
**Brief:** Spec the pure `keynav` state machine (`handle_key`, `MenuFocus` /
`FlyoutFocus`, `NavKey`, `NavAction`): arrows wrap and skip non-focusable items,
Home/End, Right/Enter open a flyout, Left/Esc pop one level (Esc at top =
close-all), Enter/Space activate, type-ahead by first character. Spec the
mouse hover-stack (`flyout::next_flyout`, `HoverTarget`) and how keyboard and
mouse share the *same* flyout-stack focus state. Spec accelerators /
mnemonics: display of accelerator text, in-menu mnemonic handling, and the
explicit **non-goal** of global system hotkeys. Define type-ahead timeout /
multi-char buffering (today it is single-char, first-letter only — a 1.0 gap).
Define dismiss semantics across all surfaces (click-outside, Esc, activate,
focus-loss) and the **N-level flyout stack** (locked decision #8 — Right/Enter
pushes a level, Left/Esc pops one).

#### [`50-testing-verification.md`](50-testing-verification.md) — Testing & verification
**Brief:** Spec the test strategy: pure-logic unit tests (already dense in
`menu` / `theme` / `layout` / `anchor` / `flyout` / `keynav` / `a11y`); headless
PNG snapshot tests (`tests/snapshot.rs`, `RasterDrawer::encode_png`) and how they
gate visual regressions cross-platform without a display server; per-platform
screen-reader verification protocol (the human-in-the-loop VoiceOver / NVDA /
Narrator / Orca passes that are the 1.0 a11y gate, since AccessKit fidelity is
per-platform); and the CI matrix (fmt + clippy `-D warnings` + build/test on
macOS today; add Windows + Linux jobs and `--features a11y` as backends land;
pin toolchain to MSRV 1.86). Define what "verified" means per screen reader.

#### [`60-migration-guide.md`](60-migration-guide.md) — muda → muri migration
**Brief:** A worked, end-to-end migration of a real muda + tray-icon app (usagio
is the reference) using the facade: the `s/muda/muri/` import change; how the
tray, context menu, and (passthrough) app menu bar each route; the
`Theme::native()` default and the first restyle step (swap to
`ThemeSource::Custom` and reach for `Segment` / `Flex` / `Align` / `Color`); every
passthrough caveat (PredefinedMenuItem OS actions, accelerators, Linux tray,
IconMenuItem fidelity) surfaced inline as the reader hits them. Cross-reference
[`02-muda-compat.md`](02-muda-compat.md) for the per-item contract.

---

## Status legend used across the specs

- **Shipped** — implemented and unit/snapshot-tested in the crate today.
- **Groundwork** — compile-verified but not wired to a live loop (e.g. Windows
  anchor).
- **Skeleton** — types exist; behavior is `todo!()` (e.g. Linux install).
- **1.0-new** — specified here, not yet built.

Every claim in these specs about *current* behavior is grounded in the source at
the commit that introduced this directory; where a spec proposes a change from the
shipped choice, it says so explicitly.
