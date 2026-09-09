# 20 — Platform backend: macOS (shipped reference)

Status: **section spec.** Grounded in the shipped `src/tray/macos.rs`,
`src/anchor.rs`, and `src/render/`. macOS is the **reference backend** — the one
platform with a live tray + popup + flyout loop today (milestone M1, doc 00 §8).
This document specifies what ships, what is groundwork, what is 1.0-new, and every
place AppKit fights a borderless `winit` surface.

Cross-references: surfaces & the `TrayHandle` gap
[`01-api-contract.md`](01-api-contract.md); the `Accessory`-vs-menu-bar facade
contract [`02-muda-compat.md`](02-muda-compat.md) §2; the `UserEvent` threading
model [`03-threading-events-versioning.md`](03-threading-events-versioning.md) §2;
the shared scene drawer [`10-rendering-layout.md`](10-rendering-layout.md);
VoiceOver + the multi-window flyout a11y problem
[`30-accessibility.md`](30-accessibility.md); keyboard/mouse nav
[`40-input-interaction.md`](40-input-interaction.md).

**Feasibility read: GREEN/AMBER.** The tray, popup, flyout, mouse/keyboard nav, and
theme-follow are shipped and tested. The AMBER is concentrated in one combined
**window-surgery workstream** the newly-locked decisions add: (a) the borderless
`winit` window must become a true non-activating `NSPanel` (§2) that (b) hosts an
`NSVisualEffectView` backdrop for **required vibrancy** (§2, decision #6) under a
per-pixel-transparent raster surface, plus (c) an **N-level flyout stack** of such
panels (§2, decision #8). The remaining tensions are unchanged: the
non-activating-panel ↔ keyboard-first-responder bind (§2) and the
`Accessory`-vs-`Regular` activation-policy conflict when a tray coexists with a
native menu bar (§5). The pure engine, anchoring math, and a11y tree are done; the
risk is AppKit fighting a hand-rolled foreign window.

---

## 1. Tray icon — `NSStatusItem` (shipped)

### Creation

`MacosAnchor::install` (`src/tray/macos.rs`) builds the status item through
`objc2` / `objc2-app-kit`:

```
NSStatusBar::systemStatusBar()
    .statusItemWithLength(NSVariableStatusItemLength)   // width follows content
```

`NSVariableStatusItemLength` lets the item size to its image/title. The item's
`button(mtm)` (`NSStatusBarButton`, itself an `NSButton`) is **both the click
target and the anchor rect source** — muri never creates its own status-bar view.

### Click routing

macOS status items report clicks through target/action, not an event stream. muri
defines a tiny Obj-C subclass with `objc2::define_class!`:

```
#[name = "MuriTrayTarget"]  #[thread_kind = MainThreadOnly]
struct TrayTarget;   // -trayClicked: forwards to the winit loop
```

`install` wires it up with `button.setTarget(Some(&target))` +
`button.setAction(Some(sel!(trayClicked:)))`. On click, `trayClicked:` reads a
`static TRAY_PROXY: OnceLock<EventLoopProxy<UserEvent>>` and posts
`UserEvent::ToggleTray` into the winit loop (§4). The `Retained<TrayTarget>` is
held in `MacosAnchor._target` so AppKit's non-retaining `target` weak reference
does not dangle — dropping it would silently break clicks.

**Left vs right click — a 1.0 gap.** Target/action fires on the primary click
only. To distinguish left/right/double, 1.0 must set
`button.sendActionOn(NSEventMask::LeftMouseDown | .RightMouseDown)` and inspect
`NSApp.currentEvent` inside `trayClicked:` (its `type` / `modifierFlags`), then
post a richer `UserEvent` carrying the button. The shipped code treats every click
as a toggle. `tray-icon` parity (`TrayIconEvent::Click`/`DoubleClick`/`Enter`/
`Move`/`Leave`, doc 02 §8) requires this refinement; it is **1.0-new**.

### Icon format + decoding

`MacosAnchor::set_icon` feeds the raw bytes to AppKit rather than muri's raster
pipeline:

```
let data = NSData::with_bytes(bytes);
NSImage::initWithData(alloc, &data).filter(|i| i.isValid())
    → image.setSize(NSSize::new(18.0, 18.0)); button.setImage(...)
```

So `Icon::Png` **and** `Icon::Svg` both go through `NSImage::initWithData` (AppKit
decodes PNG natively; it also accepts PDF/TIFF, and modern macOS decodes SVG data
here). If `initWithData` returns `nil`/invalid, muri falls back to
`button.setTitle("●")`. `Icon::Checkmark` / `Icon::Symbol` currently fall through
to the "●" title. **1.0 refinement:** map `Icon::Symbol("gearshape")` to
`NSImage::imageWithSystemSymbolName` for true SF Symbols menu-bar glyphs, and set
`image.setTemplate(true)` so the glyph auto-inverts for dark/light menu bars —
without `setTemplate`, a colored PNG will not tint correctly against a dark menu
bar. This is the one place the tray icon is *not* drawn by muri's raster core, and
that is correct: the OS owns the menu-bar strip.

### Teardown

The shipped `MacosAnchor` has **no explicit removal**; dropping the
`Retained<NSStatusItem>` releases it, and AppKit removes the item when its
retain count hits zero. **1.0 must add an explicit
`NSStatusBar::systemStatusBar().removeStatusItem(&item)`** in a `Drop` impl
(mirroring Windows' `NIM_DELETE`, doc 21 §1) — relying on ARC release alone leaves
a phantom icon if any stray `Retained` clone survives. Flagged.

## 2. Popup / anchored window (shipped)

The styled popup is a borderless `winit` window with a `softbuffer` surface; muri
paints the `tiny-skia` pixmap into it. `App::open_popup` (`src/tray/macos.rs`):

```
Window::default_attributes()
    .with_decorations(false)          // no title bar / traffic lights
    .with_resizable(false)
    .with_transparent(true)           // rounded-corner panel shows through
    .with_window_level(WindowLevel::AlwaysOnTop)
    .with_inner_size(laid.size)       // measured offscreen first
    .with_position(origin)            // from place_popup
```

### Anchoring (via shared `anchor.rs`)

1. **Measure offscreen.** A throwaway `RasterDrawer::new(2.0)` runs `render_menu`
   to get `laid.size` *before* the window exists, so the window is created at its
   final size (no visible resize flash).
2. **Get the icon rect.** `MacosAnchor::anchor_rect` reads
   `button.window().frame()` — screen coordinates, **bottom-left origin** (AppKit
   convention). It flips Y into muri's top-left logical space using the primary
   screen height: `top = screen_h - (frame.origin.y + frame.size.height)`.
3. **Place.** `place_popup(anchor, laid.size, work_area, Edge::Bottom, 2.0)`
   (`anchor.rs`) opens below the menu-bar icon, flips above on spill, clamps into
   the work area. `work_area` is `NSScreen::visibleFrame` (excludes the menu bar
   and Dock).

**Y-flip fragility (adversarial).** `anchor_rect` flips against
`NSScreen::screens().firstObject()` — the *primary* screen. A status item that the
system relocates onto a **secondary** display's menu bar (multi-monitor, or the
notch overflow) is flipped against the wrong height and the popup lands on the
wrong monitor. 1.0 must flip against `button.window().screen()` (the screen the
button actually lives on) and pass *that* screen's `visibleFrame` as the work
area, not always the primary. This is a concrete multi-monitor failure mode.

### Focus behavior — the core AppKit tension

The shipped popup uses `WindowLevel::AlwaysOnTop`, which maps to an ordinary
`NSWindow` raised to a high window level. **It is not a non-activating panel.** A
plain `NSWindow` becoming key *deactivates the previously focused app*, which for a
menu popup is wrong on two counts:

- it steals focus from the user's foreground app (the menu bar convention is that a
  status menu does **not** deactivate the app behind it); and
- `PredefinedMenuItem` edit actions (copy/paste, doc 02 §4.6) target the *first
  responder*, which the popup just stole.

**The fix (1.0-new, the roadmap's flagged item):** back the popup with an
`NSPanel` configured `NSWindowStyleMask::NonactivatingPanel`, plus
`panel.setFloatingPanel(true)` and `panel.setBecomesKeyOnlyIfNeeded(true)`. A
non-activating panel receives mouse and (when it does become key) keyboard events
**without deactivating the owning app**. winit 0.30 does not expose
`NSPanel`/style-mask creation, so 1.0 has two routes:

- **Route A (preferred):** create the `NSPanel` directly via `objc2-app-kit`, drive
  its content with `softbuffer` through its `raw-window-handle`, and integrate it
  with the winit loop as a foreign window. Keeps muri off a winit fork.
- **Route B:** post-process the winit-created `NSWindow` — reparent/reclass is not
  supported, so at minimum set `window.setStyleMask` additions and
  `setLevel(NSStatusWindowLevel)` via `objc2` on the raw handle. This does **not**
  give true non-activating behavior (an `NSWindow` cannot become a
  `NonactivatingPanel` after creation), so Route B only gets the window *level*
  right, not the activation semantics.

**Keyboard-input caveat (documented in the shipped code).** `App::on_key` notes
that driving keynav "requires the borderless popup to receive key events; on macOS
that is device-verified interactively." A non-key window does **not** receive
`keyDown:`. A `NonactivatingPanel` will only receive keys once it is made key
(`becomesKeyOnlyIfNeeded`), which happens on click. So: mouse nav works on a
non-key panel; **keyboard nav requires the panel to become key**, and making it key
must not deactivate the host app — which is exactly what `NonactivatingPanel`
guarantees and a plain raised `NSWindow` does not. This is the single load-bearing
reason to do the `NSPanel` work for 1.0. Cross-ref
[`40-input-interaction.md`](40-input-interaction.md).

### Rounded corners, transparency, and vibrancy (1.0-new, REQUIRED — decision #6)

`with_transparent(true)` + the drawer clearing to transparent (`begin_frame`) lets
the rounded-rect panel corners show through. But the shipped compositing is opaque:
`softbuffer` is opaque per-pixel, so `App::redraw` **un-premultiplies the tiny-skia
pixmap over the theme background** into `0RGB` words — the *window* is transparent
at the corners but the panel **body is opaque theme color**. That opaque body is
exactly what locked **decision #6 forbids for 1.0**: `Theme::native()` must render
with **real translucent vibrancy** — the blurred menu material — not an opaque
approximation.

**What 1.0 must build (macOS).** The popup hosts the `softbuffer`/pixmap layer
**over an `NSVisualEffectView`** backdrop:

- The window's content view is an `NSVisualEffectView` (material `.menu` /
  `.popover`, `blendingMode = .behindWindow`, `state = .active`) that produces the
  real behind-window blur the OS composites; the raster layer is composited **above**
  it.
- The winit/`softbuffer` surface must be **per-pixel transparent** so the effect
  view shows through: `App::redraw` stops un-premultiplying over an opaque theme
  color and instead writes **premultiplied alpha** where the panel background is
  drawn at **reduced alpha (or skipped entirely)** so the vibrancy shows, while text
  and icons stay fully opaque.
- The **rounded-corner mask** must clip the `NSVisualEffectView` (e.g. a mask
  layer / `maskImage` matching `theme.corner_radius`), so the blur has muri's
  rounded panel shape, not a square.

CPU raster cannot *itself* blur — the blur is the OS backdrop the raster is
composited over. That is the point: muri draws the styled content; AppKit provides
the vibrancy behind it.

**This is one combined macOS window-surgery workstream with the `NSPanel` work
(§2 focus behavior).** Both require creating/manipulating the popup window through
`objc2` rather than via plain winit attributes — the non-activating `NSPanel`
(Route A) and the `NSVisualEffectView` backdrop are configured on the *same*
hand-rolled window, driven with `softbuffer` through its `raw-window-handle` and
integrated with the winit loop as a foreign window. Do them together.

**Reconciliation with doc 30 Option C.** The a11y spec keeps a "single transparent
window compositing parent + flyout" fallback (Option C) whose only blocker was
"muri has no per-pixel window transparency today." That transparency/compositing
rework is now being built **anyway** for vibrancy — so Option C's cost drops
sharply and it becomes a viable per-platform escape hatch if the Option-B
cross-window a11y focus hand-off fails (doc 30 §3).

### Dismiss (shipped)

Outside-click / focus-loss dismiss uses `WindowEvent::Focused(false)` across a
`focused: HashSet<WindowId>`: the whole stack closes **only when no muri window
holds focus**, so opening a flyout (which briefly moves focus off the parent) does
not self-dismiss. Each new flyout window is pre-inserted into `focused` on creation
to cover the gap before it gains focus. This multi-window focus-set is the shipped
answer to "why doesn't opening a submenu close the menu." It works on macOS because
AppKit delivers `resignKey`/`becomeKey` reliably per window; contrast Windows, where
the equivalent is unreliable (doc 21 §2).

**N-level flyout stack (1.0-new — decision #8).** The shipped code opens **one**
flyout window (`Option<Flyout>`). Locked decision #8 requires **N-level nested
submenus**, so 1.0 replaces this with a **stack** — `Vec<Flyout>`, one borderless
`NSPanel` per open level, each placed by `place_flyout` relative to the *previous*
level's rect (doc 10 §12). The `focused: HashSet<WindowId>` dismiss model
**generalizes to N windows unchanged**: it already keys on "does *any* muri window
hold focus," so a 3-deep stack dismisses exactly when the whole set empties;
`Left`/`Esc` pops the deepest level (doc 40 §5). The real cost of N levels is not
the geometry or the focus set — it is the **cross-OS-window screen-reader traversal
multiplied across every stack depth** (doc 30 §3, §6).

## 3. `ContextMenu::open_at(point, edge)` (skeleton → M2)

`ContextMenu::open_at` is `todo!()` today (doc 01 §5.2). On macOS it is
straightforward relative to the tray: it reuses the *entire* popup machinery
(`open_popup`'s window creation, `place_popup`, dismiss, keynav, a11y), differing
only in the **anchor rectangle**: instead of `MacosAnchor::anchor_rect` it uses a
zero-size `LogicalRect` at `point`. The facade's `show_context_menu_for_nsview`
(doc 02 §6) maps muda's `Position` (or `NSApp.currentEvent.locationInWindow`
converted to screen coords when `None`) to that `LogicalPoint`. The `nsview`
handle supplies the screen/work-area context.

**Refactor requirement (1.0-new).** `open_popup` is currently a method on the
tray's `App`. `ContextMenu` and `Popup` (doc 01 §5.3) need the same loop without
the tray icon. 1.0 factors a shared `PopupSession` owning window/surface/laid/
flyout/focused so `Tray`, `ContextMenu`, and `Popup` are thin adapters over one
event handler, not three copies of `App`. This is called out in doc 01 §5.3 and is
the natural home for the `PredefinedMenuItem` edit-action emulation (doc 02 §4.6).

**Dropdown / `Popup::anchored_to(rect, edge)`** is the same session anchored to an
arbitrary caller rect instead of a point — trivially supported once `PopupSession`
exists.

## 4. Event loop integration (winit) and threading

- **One `EventLoop<UserEvent>`** built in `run_tray`; its proxy is stashed in
  `TRAY_PROXY` so the Obj-C click target can post into it. This is the same bridge
  AccessKit uses (`accesskit_winit::Adapter::with_event_loop_proxy`, §6).
- **`ApplicationHandler<UserEvent>` = `App`.** `user_event` handles
  `ToggleTray` / `Accessibility`; `window_event` handles focus, cursor, mouse,
  keyboard, redraw for both the popup and flyout windows (distinguished by
  `WindowId`).
- **Main-thread constraint.** `run_tray` acquires `MainThreadMarker::new()` and
  returns `Error::Platform` if not on the main thread — winit's macOS event loop is
  main-thread-only, and `NSStatusItem` / `NSApplication` are main-thread-only. This
  is exactly muda's own "menus on the UI thread" constraint
  ([`03-threading-events-versioning.md`](03-threading-events-versioning.md) §1).
- **`TrayHandle` (1.0-new, load-bearing).** `run` consumes `self`, so the consumer
  cannot call `set_menu` afterward (doc 01 §5.1, doc 03 §2). The macOS backend is
  the model for the fix: add a `UserEvent::Command(TrayCommand)` variant posted via
  a cloned `EventLoopProxy`; the loop applies `set_menu`/`set_icon`/`open`/`close`
  on the UI thread (rebuild `LaidMenu`, re-render). `open`/`close` (doc 01 §5.1) are
  `todo!()` and land the same way — `open` synthesizes the `ToggleTray`-open path
  programmatically.

## 5. App-menu-bar native passthrough + the activation-policy conflict

Per decision #2 (doc 00 §4), muri **never draws the macOS menu bar** — the window
server owns it at the top of the screen. The compat layer's `init_for_nsapp`
(doc 02 §6) builds a **native `NSMenu` itself** via objc2 (decision #5 — not the muda
crate) and installs it with `NSApplication.mainMenu = …`; that native menu owns
rendering, the standard app/Edit/Window/Help menus, Services, and accelerator
registration for free, and its target/action feeds the same muri `dispatch` /
global-channel path as custom surfaces (doc 03 §3).

**The conflict the foundation flagged (must-resolve).** `run_tray` today
unconditionally calls:

```
app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
```

`Accessory` = no Dock icon, **no app menu bar** — correct for a pure status-bar
app. But `init_for_nsapp` installs a menu bar, which requires
`NSApplicationActivationPolicy::Regular` (a Dock presence; `Accessory` /
`Prohibited` apps do not show a menu bar). An app that wants **both** a muri tray
and a native menu bar cannot be `Accessory`.

**Contract (doc 02 §2, divergence D9), specified here:** the activation policy is
decided from the **combination of surfaces installed**, before any surface is
shown. Under the pivot this splits cleanly along the two doors: **muri's native
API decides its own policy** — a tray-only native app is `Accessory` — and the
**muda-compat adapter** (which preserves muda's passive model and attaches to the
host's *own* event loop, not `Tray::run`) sets `Regular` when the host installs a
native menu bar via `init_for_nsapp`. Concretely:

| Surfaces installed | Activation policy |
|---|---|
| Tray only (no `init_for_nsapp`) | `Accessory` (shipped default) |
| `init_for_nsapp` present (± tray) | `Regular` |
| Neither (context menu on an existing app) | leave the app's existing policy untouched |

Concretely: `Tray::run` must **stop hard-coding `Accessory`** and instead take the
policy from the surface-combination decision (a `MenuOptions`/builder field on the
native path). A tray-only app built through muri's *native* API keeps `Accessory`;
the muda-compat adapter sets `NSApp`'s policy to `Regular` (before the host's loop
starts) the moment `init_for_nsapp` requests a menu bar. Getting this wrong is
visible: a `Regular` tray-only app sprouts an unwanted Dock icon; an `Accessory`
app with `init_for_nsapp` silently shows **no menu bar at all**.

## 6. Accessibility (macOS) — pointer to doc 30

AccessKit is wired on macOS today: `App` holds an
`Option<accesskit_winit::Adapter>` created **before the window is first shown**
(the popup is created `with_visible(false)` under `a11y`, the adapter attached,
then revealed). `sync_a11y` rebuilds the tree from the menu + focus on every
change; adapter events return via `UserEvent::Accessibility`; `on_a11y_action`
applies VoiceOver Focus/Click onto the live popup. **The unresolved hard problem —
each flyout is a separate OS window whose items live as nested nodes in the parent
window's tree and is not independently adapted — is owned by**
[`30-accessibility.md`](30-accessibility.md). With N-level flyouts (decision #8) that
problem is **multiplied across a stack of flyout windows**: doc 30's chosen model
(Option B, a per-flyout-window adapter) must fan out to one adapter per stack level
with chained parent↔child relations, and VoiceOver must cross the OS-window boundary
at *every* depth. Do not duplicate the analysis here; the VoiceOver device pass is
the M2 gate.

## 7. Status vs 1.0 gap (macOS)

| Capability | Status |
|---|---|
| `NSStatusItem` tray install + PNG/SVG icon via `NSImage` | **Shipped** |
| Borderless popup, offscreen-measured, `place_popup`-anchored | **Shipped** |
| Flyout (one level today, second window), hover-stack, focus-set dismiss | **Shipped** |
| Mouse + keyboard nav, `keynav` integration | **Shipped** (keyboard device-verified) |
| Dark/light follow (`effectiveAppearance`) | **Shipped** |
| AccessKit adapter + VoiceOver action routing | **Shipped** (device pass = M2 gate) |
| Explicit status-item removal on drop | **1.0-new** (§1) |
| Left/right/double-click discrimination (`tray-icon` parity) | **1.0-new** (§1) |
| Non-activating `NSPanel` (no focus-steal; reliable keys) | **1.0-new** (§2) |
| Vibrancy: `NSVisualEffectView` backdrop + transparent raster compositing | **1.0-new** (§2, decision #6) |
| N-level flyout **stack** (`Vec<Flyout>`, `place_flyout` per level) | **1.0-new** (§2, decision #8) |
| Flip against the button's *actual* screen (multi-monitor) | **1.0-new** (§2) |
| SF Symbol / template-image tray glyph | **1.0-new** (§1) |
| `ContextMenu::open_at`, `Popup::anchored_to`, `PopupSession` | **1.0-new** (§3) |
| `TrayHandle` command channel; `open`/`close` | **1.0-new** (§4) |
| Activation-policy from surface combination | **1.0-new** (§5) |

### Concrete risks / unknowns

1. **Non-activating panel via `objc2` outside winit.** winit 0.30 cannot create an
   `NSPanel`; Route A (§2) drives a hand-rolled `NSPanel` through
   `raw-window-handle` + `softbuffer`, integrated with the winit loop. Unverified
   that winit's macOS loop cooperates with a foreign window it did not create — the
   chief macOS engineering risk for 1.0.
2. **Keyboard on a non-key panel.** Keynav needs `keyDown:`, which needs key
   status, which must not deactivate the host — only `NonactivatingPanel`
   reconciles all three. Until the panel work lands, keyboard nav on a truly
   background app is unverified.
3. **Multi-monitor / notch overflow** Y-flip and work-area selection (§2) — a real
   mis-placement bug on secondary-display menu bars.
4. **Activation-policy timing** (§5): must be set before the first surface shows,
   and the muda-compat adapter cannot always know the full surface set up front —
   decide at first `run`/`init_for_nsapp`, error if a conflicting later call
   disagrees.
5. **Vibrancy compositing via `NSVisualEffectView` + a transparent raster surface**
   (§2, decision #6): a concrete *new* macOS engineering risk on par with the
   `NSPanel` work — and it is the **same combined window-surgery workstream** (both
   configure the hand-rolled foreign window through `objc2`). Unknowns: whether the
   winit loop cooperates with a foreign `NSPanel` hosting an `NSVisualEffectView`
   behind a `softbuffer` layer; getting premultiplied-alpha compositing and the
   rounded-corner backdrop mask right so the blur shows without haloing text; and
   the perf cost of a translucent surface repainting on the ~0.75s live-update tick.
6. **N-level flyout a11y across a window stack** (§2, §6, decision #8): the
   cross-OS-window screen-reader traversal (doc 30) must hold at every stack depth —
   owned by doc 30 but surfaced by macOS's per-window `NSPanel` topology.
