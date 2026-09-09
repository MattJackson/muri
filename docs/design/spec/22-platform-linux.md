# 22 — Platform backend: Linux (skeleton + honest carve-out)

Status: **section spec.** Grounded in `src/tray/linux.rs` (`LinuxAnchor`, a
deliberate stub), the shared `src/anchor.rs`, and the carve-out already stated in
[`00-overview.md` §6](00-overview.md). Linux is the platform where the design
**fundamentally fights the OS**, and this document says so plainly, then specifies
the two paths muri *does* promise.

Cross-references: the locked carve-out [`00-overview.md`](00-overview.md) §6 &
decision #3; the `Unsupported` error model
[`03-threading-events-versioning.md`](03-threading-events-versioning.md) §7 & the
Linux dependency stance §4; the `tray-icon`/`init_for_gtk_window` facade contract
[`02-muda-compat.md`](02-muda-compat.md) §6, §8, D7; `ContextMenu::open_at`
[`01-api-contract.md`](01-api-contract.md) §5.2; AT-SPI / Orca
[`30-accessibility.md`](30-accessibility.md).

**Feasibility read: RED for the tray-anchored styled popup (architecturally
impossible — this is a permanent, honest non-goal), GREEN/AMBER for the two paths
muri actually ships:** the native-menu fallback (green — it *is* native) and the
pointer-anchored styled `ContextMenu` (amber — feasible but Wayland positioning and
layer-shell portability are the real unknowns). The whole Linux story is degrade
honestly, never silently no-op.

---

## 1. Why the tray-anchored styled popup is impossible (state it plainly)

The modern Linux tray is **StatusNotifierItem (SNI) / AppIndicator over D-Bus**
(`org.kde.StatusNotifierItem`, `org.freedesktop.StatusNotifierWatcher`). The
critical facts, each independently fatal to a tray-anchored custom popup:

1. **The host owns and draws the icon in its own process.** A GNOME Shell
   extension, KDE plasmoid, or XEmbed shim renders the icon. The application only
   *exports* an icon (name or pixmap) and a menu description over D-Bus.
2. **The app is never told the icon's on-screen rectangle.** There is no geometry
   in the SNI protocol. `tray-icon` documents `TrayIcon::rect()` as **Unsupported
   on Linux** for exactly this reason (doc 02 §8). Without a rect, `place_popup`
   (anchor.rs) has nothing to anchor to.
3. **The app never receives the click coordinate.** Activation arrives as a
   context-free D-Bus signal (`Activate`/`SecondaryActivate` with a *requested*
   x/y that hosts routinely send as `0,0`), not a real pointer position. So even a
   cursor-anchored fallback has no trustworthy point on tray click.
4. **Wayland forbids a client from positioning its own toplevel.** There is no
   `set_outer_position` in the core protocol; winit's `set_outer_position` is a
   **no-op on Wayland**. A client cannot place a window at absolute screen
   coordinates even if it *had* them.

Therefore `LinuxAnchor::anchor_rect` returns `Err(Unsupported::TrayAnchor)` and
`supports_tray_anchor()` returns `false` (both shipped in `src/tray/linux.rs`), and
`Tray::run` surfaces `Unsupported::TrayAnchor` on Linux (doc 01 §5.1, doc 03 §7).
muri does **not** pretend otherwise. This is not a muri limitation to be
engineered away — it is a property of SNI + Wayland that binds every toolkit
equally. Saying so is more valuable than a silent no-op.

## 2. The two supported Linux paths

### Path 1 — Native-menu fallback (recommended default)

Render the **same `muri::Menu` spec** through a **native menu tree** the SNI host
draws:

- Register the SNI item and export a `com.canonical.dbusmenu` menu built from the
  `Menu`. The host (GNOME/KDE) draws it in its own process on tray activation.
- **Loses the custom look** (the host's theme draws it), but works **everywhere a
  Linux tray works**, on X11 and Wayland alike, and is **accessible via AT-SPI for
  free** because it is a real native menu.

**Mapping `Menu` → dbusmenu:** `Item::Row` → a menu item (label from concatenated
segment text, `enabled`, `checked` → a checkmark item, `MenuId` carried as the
item's action id); `Item::Separator` → a separator; `Item::SectionHeader` → a
disabled label item; `Item::Submenu` → a nested dbusmenu submenu. muri ships **N-level nested submenus**
in 1.0 (decision #8), and on this path that is **free**: dbusmenu nests arbitrarily
deep and the host draws the whole tree, so the native fallback carries the full
nesting muri's own renderer builds elsewhere with no extra work. Styling
(`Segment`/`Flex`/`Align`/`Color`/`Font`) is **dropped** — dbusmenu carries label +
icon + state only. This is the honest cost of the fallback, documented in the
migration guide.

**Icons:** dbusmenu takes icon *names* (freedesktop theme) or PNG *pixmap* bytes.
`Icon::Png` → pixmap; `Icon::Symbol`/`NativeIcon` → a themed icon name where one
maps, else omitted.

**Install status:** `LinuxAnchor::install` is `todo!()` today (the source says
"SNI/AppIndicator registration + dbusmenu fallback"). 1.0 builds it. Dependency
choice (a pure-Rust `ksni`, or GTK/`libdbusmenu` via muda's own Linux backend) is
**TBD** and tracked in doc 03 §4 — `ksni` avoids a GTK runtime dependency for the
tray but does not give `init_for_gtk_window` passthrough (§4 below), so 1.0 likely
needs both: `ksni`-style SNI for the tray + GTK for the menu-bar passthrough. Bad
tray registration → `Error::Platform`.

### Path 2 — Pointer-anchored `ContextMenu::open_at(point, edge)` (the styled path)

Where a **real pointer coordinate exists** — a right-click inside the app's *own*
window — muri *can* draw its styled surface. This is muri's portable primitive
(doc 01 §5.2) and the only place the custom look reaches Linux.

The mechanism is **relative** positioning, which Wayland *does* allow:

- The styled popup is a child/grab surface positioned via **`xdg_positioner`**
  relative to the **caller's own surface** (the window the right-click happened in),
  not absolute screen coordinates. `xdg_positioner` (anchor rect + gravity +
  constraint-adjustment flags) is exactly `place_popup`'s job expressed in
  Wayland's vocabulary, and the compositor does the flip/clamp the same way
  `anchor.rs` does for macOS/Windows.
- This requires the caller's surface handle. `ContextMenu::open_at(&self, …)` must
  therefore integrate with the **caller's** event loop / `raw-window-handle` on
  Wayland — it cannot self-position a detached toplevel (that is
  `Unsupported::ClientPositioning`, doc 03 §7). **Open question (doc 01 §5.2):**
  whether `ContextMenu` runs its own nested loop or borrows the caller's winit loop
  + surface. On Wayland it **must** borrow (needs the parent surface for the
  positioner); on X11 it can self-position. 1.0 resolves this by taking a
  `raw-window-handle` parameter on the Wayland path (the facade's
  `show_context_menu_for_gtk_window` supplies the GTK window's handle, doc 02 §6).
- **X11:** an **override-redirect** window (`_NET_WM_WINDOW_TYPE_POPUP_MENU`, the
  X11 equivalent of "no WM decoration, no focus steal") positioned at the absolute
  pointer coordinate — X11 *does* allow client positioning, so the macOS/Windows
  `place_popup` path works directly there.

**N-level nested submenus on the styled path (decision #8).** muri ships N-level
submenus, so a styled `ContextMenu` flyout is a **stack** of surfaces, not one. On
**Wayland** each nested level is a child `xdg_popup` **parented to the previous
level's popup surface** (an `xdg_popup` may itself be the parent of a deeper
`xdg_popup`), positioned with its own `xdg_positioner` relative to that parent.
This means the Wayland parent-surface + input-serial + grab constraint (§3)
**compounds per level**: the whole nested chain is only mappable during the
triggering input event, and the grab is held by the topmost popup and propagated
down the chain. On **X11**, each level is another override-redirect window placed
by `place_flyout` relative to the previous level's rect — the portable
`place_popup`/`place_flyout` math (anchor.rs/flyout.rs) applies directly. The
native fallback (Path 1) carries N levels for free via dbusmenu (§2).

## 3. Wayland positioning + layer-shell reality (the real unknowns)

The pointer path above is feasible, but Linux windowing is a compositor lottery.
The concrete tensions 1.0 must confront:

- **`xdg_popup` requires a parent and a grab.** A true `xdg_popup` (the correct
  surface type for a menu) can only be mapped relative to a parent `xdg_surface`
  and, for outside-click dismiss, needs a `xdg_popup` grab (dismisses on click
  outside, delivered as `popup_done`). That grab requires a recent input event
  serial from the parent — so the styled context menu is only mappable **during**
  the handling of the triggering click, from the app's own surface. A menu opened
  "later" or from no parent surface cannot be an `xdg_popup`.
- **Layer-shell (`wlr-layer-shell`) is not universal.** `zwlr_layer_shell_v1`
  (used by wlroots compositors — sway, Hyprland, KDE) would let a popup place
  itself at a layer without a parent, but it is **not implemented by GNOME's
  Mutter** and is not a stable protocol. muri must **not** hard-depend on
  layer-shell; it is at best an optional enhancement on wlroots compositors and a
  **portability risk** to call out. The portable answer is `xdg_popup` +
  `xdg_positioner` from the caller's surface (§2), accepting its "must open from a
  parent surface during an input event" constraint.
- **Focus / no-activate:** Wayland has no "always-on-top no-activate" flag a client
  controls; the compositor owns stacking and focus. An `xdg_popup` with a grab is
  the *only* portable way to get menu-like focus + outside-dismiss behavior, and it
  is the compositor — not muri — that enforces it. This is why the fallback (Path 1)
  is the *recommended* default: it sidesteps the entire Wayland surface-type maze by
  letting the host draw the menu.

## 4. App-menu-bar native passthrough — `init_for_gtk_window`

Per decision #2, muri does not draw the GTK window menu bar. The compat layer's
`init_for_gtk_window(window, container)` (doc 02 §6) builds a GTK menu tree
**itself** via gtk (decision #5 — not the muda crate) and packs it into the window's
menu-bar box (`container`); GTK owns rendering, mnemonics, accelerators, and AT-SPI
a11y for free, and it feeds the same muri dispatch/global-channel path (doc 03 §3).
This path pulls in **GTK** as a Linux dependency (doc 03 §4) — the same GTK muda
itself uses — which is the second reason 1.0 likely carries both a GTK-based
passthrough/fallback and (optionally) a lighter SNI crate for the tray.
No activation-policy conflict exists on Linux (unlike macOS §5 there); a tray and a
GTK menu bar coexist.

## 5. Accessibility (Linux) — pointer to doc 30

- **Path 1 (native fallback):** AT-SPI comes free — the host/GTK menu is a real
  accessible object tree; Orca reads it with no muri involvement.
- **Path 2 (styled `ContextMenu`):** the custom raster surface is opaque to AT-SPI
  unless bridged. With the `a11y` feature **on by default** (decision #7, doc 03
  §4), the styled path always carries the AccessKit bridge — a Linux styled menu is
  never silently inaccessible. 1.0 bridges muri's a11y tree via **`accesskit_unix`**
  (AT-SPI, doc 03 §4), verified against **Orca** (decision #4). The separate-OS-window
  flyout problem (doc 00 §7) applies here too — and with N-level submenus (decision
  #8) it is a **stack** of `xdg_popup` accessibles, multiplying the boundary
  crossings Orca must follow — and is owned by
  [`30-accessibility.md`](30-accessibility.md). Note: on Wayland an `xdg_popup` may
  not map to an independent AT-SPI accessible cleanly — another reason the flyout
  a11y model is a 1.0 gate, and another point in favor of the native fallback where
  accessibility must be guaranteed.

## 6. What Linux will and will not promise (the honest contract)

**muri promises on Linux:**

- A working tray icon via SNI/AppIndicator with a **native menu** rendered from the
  same `Menu` spec (Path 1), on X11 and Wayland, accessible via AT-SPI.
- A **styled `ContextMenu::open_at`** wherever a real pointer coordinate and the
  caller's surface exist — right-click menus inside the app's own window (Path 2),
  via `xdg_positioner`/`xdg_popup` (Wayland) or override-redirect (X11).
- Native menu-bar passthrough via `init_for_gtk_window`.
- Honest, *typed* failure: `Tray::run` → `Unsupported::TrayAnchor`;
  self-positioning without a parent surface → `Unsupported::ClientPositioning`.
- `tray-icon` parity carried forward faithfully: `TrayIcon::rect()` → `Unsupported`,
  and **`TrayIconEvent` is not emitted on Linux** (the host never delivers the
  click/coordinate) — the facade does not fabricate events (doc 02 §8).

**muri does NOT promise on Linux:**

- A **tray-anchored styled popup** (§1) — architecturally impossible; permanent
  non-goal (doc 00 §3, divergence D7).
- Custom **styling in the native-menu fallback** (§2 Path 1) — dbusmenu carries
  label/icon/state only; `Segment`/`Flex`/`Align`/`Color`/`Font` are dropped.
- **Layer-shell** anchoring (§3) — not portable across GNOME; not a dependency.
- A styled popup opened outside an input-event/parent-surface context on Wayland.
- **Vibrancy / translucent blur on the styled surface.** `Theme::native()` requires
  real vibrancy on macOS (`NSVisualEffectView`) and Windows (acrylic) per decision
  #6, but on Linux the compositor owns compositing and blur is **not a
  client-controllable effect** in any portable protocol — there is no cross-desktop
  equivalent a client can request. So the styled `ContextMenu` (Path 2) is an
  opaque `Theme::native()` panel on Linux, and the native fallback (Path 1) is drawn
  by the host and is n/a for muri styling. This is a documented Linux stance, not a
  divergence to engineer away.

## 7. Status vs 1.0 gap (Linux)

| Capability | Status |
|---|---|
| `LinuxAnchor::anchor_rect` → `Unsupported::TrayAnchor` | **Shipped** (correct by design) |
| `supports_tray_anchor()` → `false` | **Shipped** |
| SNI/AppIndicator registration + install | **Skeleton** (`install` is `todo!()`) |
| `Menu` → `com.canonical.dbusmenu` native fallback | **1.0-new** (Path 1) |
| Styled `ContextMenu::open_at` via `xdg_positioner`/`xdg_popup` (Wayland) | **1.0-new** (Path 2) |
| Styled `ContextMenu::open_at` via override-redirect (X11) | **1.0-new** (Path 2) |
| N-level flyout **stack** (nested `xdg_popup` chain / stacked override-redirect; free via dbusmenu on the fallback) | **1.0-new** (§2, §3, decision #8) |
| `raw-window-handle` parenting for the Wayland popup | **1.0-new** (open question, doc 01 §5.2) |
| `init_for_gtk_window` native passthrough | **1.0-new** (facade, needs GTK dep) |
| AT-SPI free on fallback; `accesskit_unix` on styled path; Orca pass | **1.0-new** (gate) |
| Dependency choice (`ksni` vs GTK/`libdbusmenu`) | **TBD** (doc 03 §4) |

### Concrete risks / unknowns

1. **Wayland surface-type maze** (§3): `xdg_popup` needs a parent surface + input
   serial + grab, so the styled menu is only mappable during the triggering click
   from the app's own surface. Everything else is `Unsupported::ClientPositioning`.
   The hardest Linux design constraint — and **N-level submenus** (decision #8)
   compound it, since each nested level is a child `xdg_popup` parented to the
   previous popup, so the whole chain lives or dies with the one triggering grab.
2. **Layer-shell non-portability** (§3): `wlr-layer-shell` is absent on GNOME/Mutter
   and unstable; muri must not depend on it. Any "detached" styled popup on GNOME
   Wayland is simply not available — degrade to Path 1.
3. **Dependency weight** (§2, §4): the tray fallback wants a lightweight SNI crate,
   but `init_for_gtk_window` passthrough wants GTK; 1.0 likely carries both, a real
   binary-size and build-complexity cost to weigh.
4. **AT-SPI over an `xdg_popup`** (§5): whether the styled Wayland popup exposes a
   clean AT-SPI accessible, and whether Orca crosses the flyout OS-window boundary
   (doc 30), are unverified — a gate item and a strong argument for defaulting to
   the native fallback where a11y must be guaranteed.
5. **Host diversity:** SNI behavior differs across GNOME (extension required), KDE,
   and XEmbed shims; the requested activation coordinate is unreliable (often
   `0,0`). Tray *activation* works; anything geometry-dependent does not.
