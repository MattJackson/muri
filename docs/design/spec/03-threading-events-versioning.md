# 03 — Threading, events, versioning, dependencies

Status: **foundational spec.** Covers muda's threading constraints vs muri's model;
the unified event channel across native-passthrough and custom surfaces; the
pre-1.0 early-embedder stability contract; the 1.0 semver policy; the feature-flag
matrix; MSRV; the dependency stance; and the error model.

Cross-references: surfaces & event delivery [`01-api-contract.md`](01-api-contract.md);
facade & passthrough [`02-muda-compat.md`](02-muda-compat.md).

---

## 1. Threading constraints

### muda's constraints (that muri must honor for compat)

- muda menus must be **created and mutated on the main / UI thread** (they wrap
  native objects: `NSMenu`, `HMENU`, GTK widgets).
- `MenuEvent::receiver()` is a **process-global** `crossbeam-channel` receiver;
  events are pushed from the UI thread and can be *received* from any thread.
- `init_for_*` / `show_context_menu_for_*` are UI-thread calls.

### muri's model

- The **engine core** (menu model, layout, theme resolve, anchor/flyout math,
  keynav, a11y-tree build) is **pure and thread-agnostic** — no I/O, no platform
  calls. It can run on any thread; the dense unit-test suite exercises it off any
  loop. This is the crate's portability guarantee.
- **Surfaces** (`Tray`, `ContextMenu`, `Popup`) own platform windows and **must
  live on the UI thread**. On macOS `Tray::run` acquires a `MainThreadMarker` and
  returns `Error::Platform` if not on the main thread. `run` drives a `winit`
  event loop, which is main-thread-only on macOS by winit contract.
- Surfaces are therefore **not `Send`/`Sync`** while running. Cross-thread
  interaction is by (a) the global `MenuEvent` channel (read side, any thread) and
  (b) the `TrayHandle` (write side) below.

### The `TrayHandle` requirement (1.0-new, load-bearing)

`Tray::run(self)` **consumes** the tray and blocks on the event loop, so the
consumer cannot subsequently call `set_menu`. Any live app needs to update the
menu while the loop runs (usagio rebuilds on a ~0.75s tick). 1.0 must provide a
`Send` handle obtained *before* the loop takes over:

```rust
// 1.0-new
pub struct TrayHandle { /* sends commands into the running loop */ }
impl TrayHandle {
    pub fn set_menu(&self, menu: Menu);
    pub fn set_icon(&self, icon: Icon);
    pub fn set_tooltip(&self, text: impl Into<String>);
    pub fn set_visible(&self, visible: bool);
    pub fn open(&self);
    pub fn close(&self);
}
// obtained via a split builder, e.g.:
impl Tray { pub fn handle(&self) -> TrayHandle; }   // valid to clone before run()
```

`TrayHandle` is `Clone + Send`. Its methods post commands to the UI loop via the
`winit` `EventLoopProxy` (the same mechanism the macOS backend already uses for
`UserEvent::ToggleTray` and AccessKit events). The loop applies them on the UI
thread (rebuild `LaidMenu`, re-render). This closes the "consumed by `run`" gap
and mirrors `tray-icon`'s `set_menu`/`set_icon` post-construction API. **Without
`TrayHandle`, muri is not usable by a live app** — so it is an M1/M2 deliverable,
not deferred to 1.0 polish.

## 2. The macOS event-loop reality (grounded)

The shipped macOS backend (`tray::macos`) is the reference threading model:

- One `EventLoop<UserEvent>`; `UserEvent::ToggleTray` posted from the Obj-C tray
  click target via a `static TRAY_PROXY: OnceLock<EventLoopProxy<UserEvent>>`.
- The popup and each flyout are separate `winit` windows; a
  `focused: HashSet<WindowId>` tracks focus across them so the stack dismisses only
  when *no* muri window holds focus (opening a flyout does not dismiss the menu).
- Under `a11y`, AccessKit adapter events arrive as `UserEvent::Accessibility`
  through the same proxy and are applied on the UI thread.

**Implication for 1.0:** the `TrayHandle` command channel is *another*
`UserEvent` variant (`UserEvent::Command(TrayCommand)`). The unified model is:
**everything that mutates a live surface enters through one `UserEvent` queue on
the UI thread.** Windows and Linux backends must adopt the same shape (a
`UserEvent`-equivalent posted to their loop).

## 3. Events — one unified channel

### The requirement

Decision #1 requires muda's **global `MenuEvent` channel**. muri's shipped model
is a **per-surface closure**. 1.0 must present both, and — critically — fold
**native-passthrough** menu-bar events and **custom-surface** events into **one**
channel so a migrated app's single `MenuEvent::receiver()` loop sees *all*
activations regardless of which surface produced them.

### Specification

```rust
// 1.0-new, at muri crate root and mirrored in compat::muda
pub struct MenuEvent { pub id: MenuId }
pub type MenuEventReceiver = crossbeam_channel::Receiver<MenuEvent>;
impl MenuEvent {
    pub fn receiver() -> &'static MenuEventReceiver;      // process-global, lazy
    pub fn set_event_handler(handler: Option<Box<dyn Fn(MenuEvent) + Send + Sync>>);
}
```

**Dispatch contract when a row is activated (mouse / keyboard / AccessKit
`Click`):**
1. If the surface has an `on_click` closure, it is invoked **synchronously on the
   UI thread** with `&MenuId`.
2. The `MenuEvent { id }` is **also** sent to the global channel (and to
   `set_event_handler` if installed), matching muda.
3. Inert ids (`MenuId::none()`) fire neither.

Both fire, so a consumer can use *either* style; the facade relies on (2). Ordering
is: closure first (UI thread, synchronous), then channel send. `set_event_handler`
mirrors muda's escape hatch for apps that forward events into their own loop
(e.g. tao/tauri) rather than polling the receiver.

### Folding in native-passthrough events (the seam)

Menu-bar surfaces are wrapped native `muda::Menu`s (doc 02 §2). muda's *own*
`MenuEvent::receiver()` fires when a native menu-bar item is clicked. To present
**one** channel, the facade must **bridge** the wrapped muda's channel into
muri's:

- Option A (**chosen**): at facade init, spawn a lightweight forwarder that
  `recv()`s on the wrapped `muda::MenuEvent::receiver()` and re-sends each event on
  muri's global channel. Since ids are shared verbatim (doc 02 §3), the forwarded
  `MenuEvent` is identical.
- Consequence / hazard: **two channels exist under the hood** (native muda's and
  muri's); the forwarder is the only place they join. It must not drop or reorder
  events. **1.0 test gate:** a fixture with one native menu-bar item and one muri
  tray item must deliver both activations, in order, on `muri::MenuEvent::receiver()`.
- Alternative rejected: re-implement muda's channel — impossible without forking
  muda's native emission. Bridging is the only faithful path.

**Adversarial note for reviewers:** this bridge is the subtlest correctness risk
in the whole compat story. A native menu-bar click and a custom-surface click take
*completely different code paths* (one through AppKit/Win32/GTK + wrapped muda, one
through muri's `dispatch`) yet must be indistinguishable at the receiver. Attack:
event ordering under rapid interleaving; the forwarder thread's shutdown on app
exit; double-emission if a consumer accidentally polls *both* muda's and muri's
receivers.

## 4. Dependencies, feature flags, MSRV

### Dependency stance (grounded in `Cargo.toml`)

Portable core (always):
- `tiny-skia = "0.11"` — CPU 2D raster.
- `cosmic-text = "0.12"` — font lookup / shaping / layout.

Optional a11y (`a11y` feature):
- `accesskit = "0.17"` — portable a11y data model.
- `accesskit_winit = "0.23"` (macOS target today) — bridges the tree to the winit
  window / NSAccessibility. 1.0 adds `accesskit_windows` (UIA) and
  `accesskit_unix` (AT-SPI) — either directly or via `accesskit_winit`'s platform
  coverage.

macOS target:
- `winit = "0.30"`, `softbuffer = "0.4"`, `raw-window-handle = "0.6"`,
  `objc2 = "0.6"`, `objc2-foundation = "0.3"`, `objc2-app-kit = "0.3"`.

Windows target:
- `windows-sys = "0.59"` (Foundation, Gdi, Shell, WindowsAndMessaging, HiDpi,
  LibraryLoader features).

1.0-new for the facade:
- `crossbeam-channel` (the global `MenuEvent` channel — same crate muda uses, for
  behavioral parity).
- `dpi` (the position type muda re-exports; pin to muda's version — doc 02 §6).
- On Linux: a `dbusmenu` / SNI stack (e.g. `ksni` or muda-via-gtk) for the
  native-menu fallback, plus GTK for `init_for_gtk_window` passthrough. **Version
  stance TBD in [`22-platform-linux.md`](22-platform-linux.md).**

**Version-pinning discipline:** `winit`, `accesskit`, and `accesskit_winit` are a
**coupled triple** — they must be co-compatible (0.30 / 0.17 / 0.23 today). Any
bump moves all three together and is a semver-relevant event for muri because
`raw-window-handle` types can appear in the public surface. Documented as a "bump
together" rule.

### Feature-flag matrix

| Feature | Default | Pulls in | Purpose |
|---|---|---|---|
| *(none / core)* | — | tiny-skia, cosmic-text | pure engine + `RasterDrawer`; compiles everywhere, `forbid(unsafe_code)` off-macOS/Windows |
| `a11y` | off | accesskit, accesskit_winit (+ platform adapters) | screen-reader bridge; **1.0-change: on by default** given decision #4 makes a11y non-optional. Kept as a flag so a size-constrained consumer can drop it, but the default flips to include it. |
| `muda-compat` | off | crossbeam-channel, dpi, wrapped muda + gtk (Linux) | the `compat::muda` / `compat::tray_icon` facade + global `MenuEvent` channel |
| `serde` (optional) | off | serde | derive on the data model for consumers that persist menus — 1.0-optional, low priority |

Platform backends are selected by `cfg(target_os)`, **not** features (per the
shipped design — one crate, `cfg`-gated shims). The macOS/Windows deps are
`[target.'cfg(...)'.dependencies]`, so a Linux build never compiles them.

`#![forbid(unsafe_code)]` is active on every target **except** macOS and Windows
(which genuinely need `unsafe` for objc2 / windows-sys). The pure core is
unsafe-free.

### MSRV

- **MSRV = 1.86** (`rust-version = "1.86"` in `Cargo.toml`). CI pins this exact
  toolchain (do not rely on the newer Mac default — clippy drift, per the repo's
  precommit note). 1.0 keeps MSRV conservative; a bump is a minor-version,
  changelog-documented event.

## 5. Pre-1.0 early-embedder stability contract

muri is adopted **before** 1.0 (doc 00 §8). Early embedders (usagio, behind its
`custom-popup` feature flag) need to know **what they can build against now**
without churn breaking them, and **what may still move.** This contract governs
the 0.x series.

### Stability tiers (0.x)

- **Tier 1 — Stable-in-practice (safe to depend on now).** The **declarative data
  model**: `Menu`, `Item`, `Row`, `Segment`, `StyleRun`, `Align`, `Flex`, `Color`,
  `Rgba`, `Font`, `FontFamily`, `Weight`, `Icon`, `MenuId`, `Theme`,
  `ThemeSource`, `MenuOptions`, `Insets`, `Edge`, and the logical-geometry types.
  These are pure, heavily tested, and shaped by the usagio requirements already.
  **Change policy pre-1.0:** additive only (new fields default-constructed, new
  enum variants marked `#[non_exhaustive]` where growth is expected). The
  `Color` and `Icon` enums *will* gain variants (`Color::MenuBackground`,
  `Icon::Symbol` drawing) — they should be `#[non_exhaustive]` now so that is not
  a break.
- **Tier 2 — Stabilizing (depend on, expect minor signature churn).** The
  **surface builders**: `Tray`, `ContextMenu`, and the 1.0-new `Popup` /
  `TrayHandle`. The *shape* (builder-style, `on_click`, `menu`, `options`) is
  stable; specific method signatures may refine — notably the `run` vs
  `handle`/`TrayHandle` split (§2) and `ContextMenu::open_at`'s Wayland surface
  argument. An embedder should isolate these behind its own thin adapter (usagio's
  `custom-popup` seam already does this).
- **Tier 3 — Churning (do not hard-depend pre-1.0).** The **facade**
  (`compat::muda`, `compat::tray_icon`), the **global `MenuEvent` channel**
  semantics, `PredefinedMenuItem` emulation, accelerator handling, and the Linux
  paths. These are actively being designed here and land at M3+. An early
  macOS-only embedder should use muri's **native** API (Tier 1/2), not the facade.
- **Tier 4 — Internal-but-public (advanced use only).** `render`, `layout`,
  `anchor`, `flyout`, `keynav`, `a11y` modules are `pub` for testing/advanced
  integration. Semver-covered but most likely to move; depend only if you must,
  and pin exactly.

### The concrete contract for usagio (and peers) now

1. Build against muri's **native** API (`Tray` / `Menu` / `Row` / `Segment` /
   `Theme`) on **macOS only**, behind your own feature flag (`custom-popup`).
2. **Pin an exact version** (`muri = "=0.x.y"`) or a git rev during 0.x — do not
   float, because Tier 2/3 churn is expected and semver-minor pre-1.0 may carry
   breaking changes (0.x semver: minor = may break).
3. Keep native muda as a **kill-switch** one release past cutover (the roadmap's
   Phase 2 milestone), so a muri regression is a flag flip, not an outage.
4. Isolate Tier 2 surface calls behind one adapter module so a `run`/`TrayHandle`
   signature refinement is a one-file change.
5. **Do not** depend on the facade or the global channel until M3; use `on_click`.
6. Report the menu you build; muri hands `MenuId` strings back verbatim, so your
   id grammar is unaffected by any muri change (Tier 1 guarantee).

muri, in return, commits during 0.x to: additive-only changes to Tier 1; a
CHANGELOG entry for any Tier 2 signature change with a migration note; and not
removing a Tier 1 type without a deprecation release.

## 6. 1.0 semver / API-stability policy

At 1.0:

- **Tier 1 + Tier 2 become semver-stable.** Breaking changes to them require 2.0.
  New fields on structs are added only via `#[non_exhaustive]`-guarded growth or
  builder methods; enums that may grow are `#[non_exhaustive]`.
- **The facade (`compat::muda`) tracks muda's API contract**, not muda's version
  number — muri does not promise to mirror a *future* muda API, only the
  documented surface enumerated in doc 02. If muda adds items post-1.0, muri adds
  them in a minor release.
- **The dependency triple** (winit/accesskit/accesskit_winit) and `raw-window-handle`
  appearing in the public surface means a major bump of those *can* force a muri
  major bump; muri 1.0 documents which dependency types are re-exported (aim: as
  few as possible — ideally only `dpi::Position` via the facade and
  `raw-window-handle` for `ContextMenu`/`Popup` integration).
- **MSRV bumps are minor**, changelog-documented.
- **Platform behavior differences** (the doc 02 §9 divergence register, the Linux
  carve-out) are part of the documented contract, not bugs; changing them (e.g.
  adding Linux layer-shell anchoring) is additive.

## 7. Error model (unified)

```rust
pub enum Unsupported { TrayAnchor, ClientPositioning }
pub enum Error { Unsupported(Unsupported), BadIcon(String), Platform(String) }
pub type Result<T> = std::result::Result<T, Error>;
```

- `Unsupported(TrayAnchor)` — Linux tray-anchored popup (decision #3); returned by
  `Tray::run` / `LinuxAnchor::anchor_rect`.
- `Unsupported(ClientPositioning)` — Wayland client toplevel positioning (reserved;
  for a `ContextMenu` path that cannot self-position without a parent surface).
- `BadIcon` — undecodable icon bytes (facade `Icon::from_rgba` with bad
  dimensions maps here, mirroring muda's `BadIcon`).
- `Platform` — a platform API call failed (window creation, `Shell_NotifyIcon`,
  status-item install, event-loop build).
- **`#[non_exhaustive]`** on both `Error` and `Unsupported` so 1.0 can add variants
  (e.g. a future `Unsupported::VibrancyUnavailable`) without a major bump.
- The facade re-exports a muda-compatible `Error` superset (doc 02 §7); matching on
  a muda variant compiles, even if muri never returns it from a custom surface.

All fallible surface entry points (`Tray::run`/`open`, `ContextMenu::open_at`,
`Popup::anchored_to`, facade `init_for_*` / `show_context_menu_for_*`) return
`Result`. Pure engine functions never fail (they are total over their inputs),
which is why they carry no error type — a deliberate design property that keeps the
tested core simple.
