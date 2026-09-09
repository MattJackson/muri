# 02 — The muda compatibility facade (load-bearing)

Status: **foundational spec — the critical one.** This document is the contract for
locked decision #1 (muda drop-in) and #2 (hybrid menu bar). It enumerates muda's
**entire** public API (and the `tray-icon` surface muri subsumes) and specifies,
per item: how `muri::compat::muda` mirrors it, which surface it maps to (custom
muri vs native passthrough), and the exact fidelity/behavior contract.

muda API details below reflect muda's published `docs.rs` surface (Windows /
macOS / Linux-gtk). tray-icon likewise. Where muda's behavior is OS-specific it is
noted per platform.

Cross-references: native API [`01-api-contract.md`](01-api-contract.md); threading
& the global event channel [`03-threading-events-versioning.md`](03-threading-events-versioning.md);
the worked migration [`60-migration-guide.md`](60-migration-guide.md).

---

## 1. The migration guarantee, stated precisely

**Guarantee (`s/muda/muri/`):** an app whose only menu dependencies are `muda` and
`tray-icon`, that uses them within the documented supported set (§10), can:

1. change `use muda::…` → `use muri::compat::muda::…` and `use tray_icon::…` →
   `use muri::compat::tray_icon::…` (or enable a `muda-compat` feature that
   re-exports under the original paths);
2. keep its `MenuEvent::receiver()` / `TrayIconEvent::receiver()` loop unchanged;
3. compile without further edits;
4. and **look native by default** (via `Theme::native()`), because:
   - **menu-bar surfaces** (`init_for_nsapp` / `init_for_hwnd` /
     `init_for_gtk_window`) pass through to the *real native menu* — byte-for-byte
     native, because it *is* native (decision #2);
   - **custom surfaces** (tray menu, `show_context_menu_for_*`) are drawn by muri
     with `Theme::native()`, which resolves to live OS colors and the system font
     — *visually near-native*, with the documented divergences in §9.

**What the guarantee does NOT promise:**

- Pixel-identical custom-surface rendering vs the OS menu (no vibrancy/translucency;
  muri's own metrics; see §9).
- That every `PredefinedMenuItem` OS action behaves natively *inside a custom
  surface* (§4.6 — the hardest caveat).
- Global accelerator activation from a custom tray/context menu (§5).
- A styled tray-anchored popup on Linux (decision #3).

The guarantee is deliberately scoped so it is *true*, not aspirational. §10
enumerates the supported set and every divergence.

## 2. Facade shape and surface routing

```rust
// muri::compat::muda  — mirrors muda's module layout and type names
pub mod muda {
    pub struct Menu { /* wraps either a native muda::Menu (menu-bar) or a muri::Menu */ }
    pub struct Submenu { .. }
    pub struct MenuItem { .. }
    pub struct CheckMenuItem { .. }
    pub struct IconMenuItem { .. }
    pub struct PredefinedMenuItem { .. }
    pub struct MenuId(pub String);          // = muri::MenuId
    pub struct MenuEvent { pub id: MenuId }
    pub type MenuEventReceiver = ...;        // global channel (doc 03 §3)
    pub trait ContextMenu { .. }             // muda's helper trait
    pub trait IsMenuItem { .. }
    pub enum MenuItemKind { .. }
    pub enum NativeIcon { .. }
    pub struct Icon { .. }                   // = tray-icon/muda Icon (from_rgba etc.)
    pub mod accelerator { pub struct Accelerator; pub struct Modifiers; pub enum Code; .. }
    pub mod about_metadata { pub struct AboutMetadata; pub struct AboutMetadataBuilder; }
    pub enum Error { .. }
    // free/trait fns: init_for_*, show_context_menu_for_* (on the ContextMenu trait)
}
pub mod tray_icon { pub struct TrayIcon; pub struct TrayIconBuilder; /* … */ }
```

### The routing decision (decision #2), made concrete

The facade's `Menu` is a **mode-tagged wrapper**. Its mode is decided by *which
method the consumer calls on it*, because that is the only signal of intent muda's
API gives:

| Consumer calls | Intent | Facade routes to | Rendered by |
|---|---|---|---|
| `Menu::init_for_nsapp()` | macOS app menu bar | wrapped **native `muda::Menu`** | OS (NSMenu) |
| `Menu::init_for_hwnd(hwnd)` | Windows window menu bar | wrapped **native muda** | OS (HMENU) |
| `Menu::init_for_gtk_window(w, b)` | Linux window menu bar | wrapped **native muda** | OS (GTK) |
| `menu.show_context_menu_for_nsview(view, pos)` | transient context menu | **muri `ContextMenu`** | muri (custom) |
| `menu.show_context_menu_for_hwnd(hwnd, pos)` | transient context menu | **muri `ContextMenu`** | muri (custom) |
| `menu.show_context_menu_for_gtk_window(w, pos)` | transient context menu | **muri `ContextMenu`** | muri (custom) |
| `tray_icon::TrayIconBuilder…with_menu(menu)` | tray menu | **muri `Tray`** | muri (custom) |

**The seam and its hazard:** the *same* `muda::Menu` value can, in principle, be
used both as a menu bar (`init_for_nsapp`) and shown as a context menu
(`show_context_menu_*`) — muda allows this. The facade cannot know at construction
time which it will be. Therefore the facade `Menu` **builds a muri `Menu` tree
eagerly (always)** and **lazily builds a native `muda::Menu` only if an `init_for_*`
method is called.** A menu used both ways is drawn natively in the bar and by muri
in the context menu — visually different in the two places. This is a documented
divergence (§9, item D3). In practice apps do not reuse one `Menu` as both bar and
context menu, so the hazard is rare; the facade prioritizes never *silently
breaking* the common case.

**macOS `Accessory` vs menu-bar conflict (must-resolve):** muri's `Tray::run` sets
`NSApplicationActivationPolicy::Accessory` (no Dock icon, no app menu) for a
pure status-bar app. But `init_for_nsapp` (native menu bar) requires a
`Regular` app with a Dock presence. An app that uses **both** a muri tray and a
native menu bar cannot be `Accessory`. **Contract:** the facade sets the activation
policy from the *combination* of surfaces the consumer installs — `Regular` if any
`init_for_nsapp` is called, `Accessory` only for a tray-only app. This must be
decided before any surface is shown. See [`20-platform-macos.md`](20-platform-macos.md).

## 3. `MenuId` and identity

- muda: `pub struct MenuId(pub String)`, `From<…>`, auto-generated (an
  incrementing counter) when a builder is not given an explicit id; a builder
  can also be given an explicit id via `with_id`.
- muri: `pub struct MenuId(pub String)` — **structurally identical**, so the facade
  shares ids with **zero conversion**.

**Contract:**
- When the consumer supplies an explicit id, the facade uses it verbatim as the
  muri `Row.id`.
- When muda would auto-generate an id, the facade generates the *same* id muda
  would (same counter semantics) so `MenuEvent.id` comparisons in the consumer's
  handler still match. This requires the facade to replicate muda's id-allocation
  (a process-global atomic counter, stringified) exactly. **1.0 test gate:** a
  fixture app that never sets ids must observe identical ids under muda and under
  the facade.
- muri's `MenuId::none()` (empty string) is a *muri-only* inert sentinel with no
  muda equivalent. The facade never produces it for a muda `MenuItem` (every muda
  item is addressable). It is used only for facade-synthesized inert rows (a
  `PredefinedMenuItem::separator` becomes `Item::Separator`, not an inert row).

## 4. Menu items — per-type contract

muda's item types and the trait `IsMenuItem` (the object-safe supertype;
`MenuItemKind` is the owned enum returned by `Menu::items()`).

### 4.1 `Menu` (root)

- muda: the root container. On Windows/Linux it is a **menu bar**; on macOS it is
  the **global app menu** (via `init_for_nsapp`). Methods: `new`,
  `append`/`prepend`/`insert`(`&dyn IsMenuItem`), `remove`, `items() ->
  Vec<MenuItemKind>`, `init_for_*`, `show_context_menu_for_*` (via `ContextMenu`
  trait), `set_as_windows_menu_for_nsapp` etc.
- muri facade: `compat::muda::Menu` wraps a `muri::Menu` builder plus the mode tag
  (§2). `append`/`prepend`/`insert`/`remove` mutate the underlying `Vec<Item>`.
  `items()` reconstructs `Vec<MenuItemKind>` from the tree.
- **Fidelity:** menu-bar mode = native (perfect). Context/tray mode = muri custom
  (Theme::native, §9 divergences).

### 4.2 `Submenu`

- muda: a nested menu; `IsMenuItem`; has text, enabled, an id, children.
- muri: `Item::Submenu { label: Row, menu: Menu }`. The submenu's text →
  `label.label(text)`; enabled → `label.enabled`; children → the nested `Menu`.
- **Fidelity note:** muda submenus in a *native menu bar* nest as native
  submenus. In a muri custom surface they open as **flyout panels** — one level
  deep today ([`40-input-interaction.md`](40-input-interaction.md)). A muda menu
  with **submenus nested >1 deep shown as a context menu** exceeds the current
  flyout depth; 1.0 must either lift the single-level limit or the facade flattens
  / documents it. **This is a 1.0 gate item**, flagged for doc 40.

### 4.3 `MenuItem`

- muda: text + enabled + optional accelerator + id; `IsMenuItem`; builder
  `MenuItemBuilder`. `set_text`, `set_enabled`, `set_accelerator`.
- muri: `Item::Row(Row::new(id).label(text).enabled(..))`. Accelerator → §5.
- `set_text`/`set_enabled` post-construction: the facade holds the `Row` and
  mutates it, then triggers a re-render of the owning surface on next open (or
  immediately if open, via the 1.0-new `TrayHandle`).

### 4.4 `CheckMenuItem`

- muda: like `MenuItem` plus a boolean checked state; `is_checked`/`set_checked`.
- muri: `Row::new(id).label(text).checked(bool)` → renders a check column;
  a11y role becomes `MenuItemCheckbox` automatically (`a11y::build_tree`).
- **Fidelity:** exact. muri already models `checked: Option<bool>` and draws a
  checkmark (via `Icon::Checkmark` or `checked == Some(true)`).

### 4.5 `IconMenuItem`

- muda: `MenuItem` + a leading `Icon` (or `NativeIcon`); `set_icon`.
- muri: `Row::new(id).label(text).leading(Icon::Png(..))`.
- **Fidelity caveat:** muda `Icon` is created from **RGBA bytes** (`Icon::from_rgba(rgba,
  w, h)`), muri `Icon` is `Png`/`Svg` **encoded** bytes (+ `Checkmark`/`Symbol`).
  The facade adds `Icon::from_rgba(rgba, w, h)` mirroring muda, storing the raw
  RGBA and feeding it to the drawer's `draw_image` directly (which already takes
  straight-alpha RGBA). `NativeIcon` (an OS stock icon) → §4.7.

### 4.6 `PredefinedMenuItem` — the hardest caveat

muda's `PredefinedMenuItem` constructors carry **OS-native behavior** the *native
menu system* performs. In a native menu bar (passthrough) they work perfectly. In
a **custom muri surface** there is no native menu system to perform them, so muri
must emulate — and some cannot be faithfully emulated. Full enumeration:

| Constructor | Native behavior | In native bar (passthrough) | In muri custom surface |
|---|---|---|---|
| `separator()` | a divider | native | **exact** — `Item::Separator` |
| `copy()` / `cut()` / `paste()` / `select_all()` / `undo()` / `redo()` | edit action to the focused first responder | native | **emulated best-effort** — synthesize the platform edit command (macOS: send `copy:`/`paste:` to `NSApp.sendAction` to the first responder; Windows: post `WM_COPY`/`WM_PASTE`; Linux: XTEST/AT-SPI). Works if a first responder exists; a non-activating popup means muri must target the *previously* focused window. **Caveat: fidelity depends on the host having a focusable text target.** |
| `undo()` / `redo()` | undo manager | native | best-effort as above; often a no-op without a responder. |
| `minimize()` / `maximize()` / `fullscreen()` / `close_window()` | window management on the key window | native | **emulated** via the platform window API on the app's key window (macOS `NSWindow` miniaturize/zoom/toggleFullScreen/performClose; Windows `ShowWindow`; Linux via the WM). Requires the app to have a key window; a tray-only app may have none. |
| `hide()` / `hide_others()` / `show_all()` | app visibility (macOS) | native | macOS: call the corresponding `NSApplication` selector. Windows/Linux: muda itself no-ops several of these; the facade matches muda's platform coverage. |
| `quit()` | terminate the app | native | **emulated** — macOS `NSApp.terminate`, else `std::process::exit(0)` (or a facade-configurable quit hook). **Caveat:** a graceful-shutdown app should prefer the global `MenuEvent` on a known id over relying on the emulated terminate. |
| `about(AboutMetadata)` | show the standard About panel | native | **emulated** — macOS `NSApp.orderFrontStandardAboutPanel` with the metadata; Windows/Linux muda shows its own dialog, which muri **cannot draw** (not a menu) → the facade shows the native standard panel where one exists, else it is a **documented no-op / consumer-supplied** callback. This is the least faithful item in a custom surface. |
| `services()` | the macOS Services submenu | native | **cannot be emulated in a custom surface** — the Services menu is populated by the OS and is only meaningful in the native menu bar. The facade renders it as a **disabled placeholder row** in a custom surface and documents that Services requires the native menu bar. |
| `bring_all_to_front()` | macOS window action | native | emulated via `NSApplication`. |

**Contract summary for `PredefinedMenuItem`:**
- In **menu-bar passthrough**: full native fidelity (it *is* native).
- In a **custom surface**: `separator` is exact; edit/window/app actions are
  **best-effort emulated** and documented as depending on a suitable target
  window/responder; `services` and (off-macOS) `about` are **not faithfully
  emulable** and are documented as menu-bar-only.
- The facade never *silently* drops one: an inemulable predefined item renders as a
  visible, disabled row so the menu shape is preserved and the limitation is
  obvious. This is the single most important honesty rule in the facade.

### 4.7 `NativeIcon`

- muda: an enum of OS stock icons (`NativeIcon::User`, `Caution`, etc.) usable on
  `IconMenuItem`.
- muri facade: maps to `Icon::Symbol(&'static str)` (SF Symbol on macOS, bundled
  fallback elsewhere). **Caveat:** `Icon::Symbol` is **1.0-new and not yet drawn**
  (doc 01 §3); until it is, a `NativeIcon` renders with no leading glyph in a
  custom surface. 1.0 must draw `Symbol` or the facade falls back to text.

### 4.8 `IsMenuItem` / `MenuItemKind`

- `IsMenuItem` (muda's object-safe item trait): the facade re-exports a trait of
  the same name implemented by all facade item types, so
  `Menu::append(&dyn IsMenuItem)` type-checks unchanged.
- `MenuItemKind` (owned enum returned by `items()`): the facade reconstructs it
  from the muri `Item` tree so `menu.items()` round-trips.

## 5. Accelerators and mnemonics

- muda `accelerator` module: `Accelerator { mods: Modifiers, key: Code }`,
  `Accelerator::from_str` (e.g. `"CmdOrCtrl+S"`), attached to items; the **native
  menu system registers and fires them globally** while the app is focused.
- muri facade: stores the accelerator on the `Row` (a **1.0-new** `Row` field,
  `accelerator: Option<Accelerator>`, or a facade-side sidecar map keyed by
  `MenuId`) and:
  - in **menu-bar passthrough**: the native menu registers it — full fidelity.
  - in a **custom surface**: muri **displays** the accelerator text right-aligned
    in the row (a natural use of `Segment` + `Align::Right` — dogfoods the
    flush-right feature) and handles the keystroke **only while the menu is open**
    (mnemonic-style). It does **not** register a system-wide/app-wide hotkey that
    fires when the menu is closed.
- **Divergence (explicit):** a muda accelerator on a *context/tray* item that the
  user expects to fire from anywhere in the app will **not** fire when the muri
  menu is closed. Apps needing global hotkeys use the `global-hotkey` crate; this
  is a stated non-goal (doc 00 §3). The facade documents this loudly, since it is
  a behavior change for menu-bar items *migrated into* a custom surface.
- **Mnemonics** (`&File` underline-access on Windows/Linux): in passthrough,
  native. In a custom surface, muri parses the `&` mnemonic, strips it from the
  drawn text, underlines the mnemonic character, and wires Alt+letter while open
  ([`40-input-interaction.md`](40-input-interaction.md)).

## 6. `ContextMenu` trait, `init_for_*`, `show_context_menu_for_*`

muda exposes these as the `ContextMenu` helper trait (implemented by `Menu` and
`Submenu`):

- `init_for_hwnd(&self, hwnd)` **[Windows, unsafe]** — install as a window menu bar.
- `init_for_nsapp(&self)` **[macOS]** — install as the global app menu.
- `init_for_gtk_window(&self, window, container)` **[Linux]** — install into a GTK
  window's menu-bar box.
- `show_context_menu_for_hwnd(&self, hwnd, position: Option<Position>)`
  **[Windows, unsafe]**.
- `show_context_menu_for_nsview(&self, view, position: Option<Position>)`
  **[macOS, unsafe]**.
- `show_context_menu_for_gtk_window(&self, window, position: Option<Position>)`
  **[Linux]**.
- plus `hide_for_nsview` / `set_as_*` variants.

**Facade contract:**
- **`init_for_*` → native passthrough (decision #2).** The facade lazily
  constructs a real `muda::Menu` from the tree and calls muda's real `init_for_*`.
  The wrapped native menu owns menu-bar rendering, accelerators, and a11y for
  free. muri draws nothing here.
- **`show_context_menu_for_*` → muri custom.** The facade builds a
  `muri::ContextMenu` from the tree and calls `open_at(point, edge)`, mapping the
  `Position` (or the current cursor when `None`) to a `LogicalPoint` and choosing
  `Edge::Bottom`. The `hwnd`/`nsview`/`gtk_window` handle is used for
  monitor/work-area context and (Wayland) `xdg_positioner` parenting.
- **`unsafe` surface preserved:** the facade's `init_for_hwnd` /
  `show_context_menu_for_hwnd` / `show_context_menu_for_nsview` stay `unsafe fn`
  with the same signatures so callers compile unchanged.
- **Position type:** muda uses `dpi::Position` (from the `dpi` crate it re-exports).
  The facade accepts the same `dpi::Position` and converts to muri
  `LogicalPoint`. 1.0 depends on the same `dpi` version muda uses (doc 03 §4).

## 7. Errors

- muda `Error` (thiserror): `NotAChildOfThisMenu`, `AlreadyInitialized`, an accel
  parse error, platform errors, etc.
- muri `Error`: `Unsupported(Unsupported)`, `BadIcon(String)`, `Platform(String)`.
- **Facade contract:** the facade re-exports an `Error` enum that is a *superset*
  compatible with muda's variants the consumer might match on. For passthrough
  surfaces it forwards muda's real error. For custom surfaces it maps: bad icon
  bytes → `BadIcon`; a Linux tray-anchor attempt → `Unsupported(TrayAnchor)`;
  platform-call failures → `Platform`. If the consumer matches on a muda-specific
  variant that muri cannot produce, the facade keeps the variant present (so the
  `match` compiles) but simply never returns it from custom surfaces.

## 8. `tray-icon` crate subsumption

muri's `Tray` subsumes `tray-icon`. The facade mirrors:

- `TrayIconBuilder::new().with_icon(icon).with_tooltip(t).with_menu(menu).build()`
  → `muri::Tray::new(icon).tooltip(t).menu(menu)` + `run`/handle.
- `TrayIcon::set_icon` / `set_tooltip` / `set_title` / `set_visible` /
  `set_menu` → `TrayHandle` methods (doc 01 §5.1, doc 03 §2).
- `TrayIcon::rect()` → `Tray` anchor rect; **Linux: `Unsupported`** (matches
  tray-icon's own "unsupported on Linux").
- `TrayIconEvent` (`Click`/`DoubleClick`/`Enter`/`Move`/`Leave` with
  `MouseButton`/`MouseButtonState`/position/rect) + `TrayIconEvent::receiver()`
  global channel → the facade emits the same events. **Linux caveat carried
  forward verbatim:** `TrayIconEvent` is **not emitted on Linux** (the SNI host
  never delivers the click/coordinate) — the facade matches this exactly rather
  than fabricating events.
- `TrayIconId` → mirrored.

**Fidelity:** on macOS/Windows, muri owns the icon and can emit click/rect events.
The important divergence from raw tray-icon: raw tray-icon's click *opens the
OS's native menu*; muri's click opens the **custom-drawn** popup. That is the
entire point — but it means a migrating app that relied on the OS menu look now
gets muri's `Theme::native()` look (§9).

## 9. Divergence register (every documented difference)

Referenced by id from doc 60. These are the complete, enumerated ways a migrated
app can differ; the guarantee in §1 is "identical *except* these."

- **D1 — No vibrancy/translucency.** Custom surfaces are opaque `Theme::native()`
  color; macOS menu vibrancy is not reproduced (doc 01 §4). Permanent.
- **D2 — Metrics differ subtly.** Row height, padding, corner radius, and font
  metrics are muri's, tuned to look native but not guaranteed pixel-identical to
  the OS menu.
- **D3 — A `Menu` used as both bar and context menu** renders native in the bar,
  custom in the context menu (§2 seam). Rare.
- **D4 — `PredefinedMenuItem` OS actions** in custom surfaces are emulated
  best-effort; `services` and off-macOS `about` are menu-bar-only (§4.6).
- **D5 — Accelerators in custom surfaces** are displayed and handled only while
  the menu is open; no global/app-wide firing when closed (§5).
- **D6 — `NativeIcon` / `Icon::Symbol`** not yet drawn in custom surfaces until
  1.0 ships `Symbol` rendering (§4.7).
- **D7 — Linux tray** has no styled anchored popup; falls back to native menu or
  pointer context menu (decision #3).
- **D8 — Submenu depth** >1 in a custom surface exceeds the shipped single-level
  flyout until 1.0 lifts it (§4.2).
- **D9 — macOS activation policy** may shift from `Accessory` to `Regular` when a
  native menu bar is installed alongside a tray (§2).

## 10. The supported migration set (what §1's guarantee covers)

The guarantee holds for apps that:

- use `muda::Menu` + item types + `MenuEvent::receiver()` and/or `tray-icon`;
- install the app menu bar via `init_for_*` **and/or** use a tray menu **and/or**
  `show_context_menu_for_*`;
- use `PredefinedMenuItem` items understanding the §4.6 custom-surface caveats;
- do not rely on menu-bar accelerators firing from within a *custom* surface (D5),
  on custom-surface vibrancy (D1), or on a styled tray popup on Linux (D7).

For apps entirely on the native menu bar, migration is transparent (everything is
passthrough). The interesting migrations — and muri's actual value — are apps with
**tray/context menus**, which move from OS-drawn to muri-drawn and gain the full
styling API the moment they want it. That is the vision (doc 00 §2) made concrete.
