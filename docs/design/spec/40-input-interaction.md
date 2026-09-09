# 40 — Input & interaction

Status: **section spec.** Grounded in the shipped pure `src/keynav.rs`,
`src/flyout.rs`, and the live `src/tray/macos.rs` event loop. **Shipped** describes
current behavior; **1.0-new**/**1.0-change** marks additions with rationale; **⚠
owner-confirm** flags a provisional decision awaiting the owner.

Cross-references: the data model, `is_interactive`, and `MenuId`
[`01-api-contract.md`](01-api-contract.md); the accelerator/mnemonic facade contract
[`02-muda-compat.md` §5](02-muda-compat.md); the unified event channel and threading
[`03-threading-events-versioning.md`](03-threading-events-versioning.md); rendering,
hit-testing, and the shared item-index space
[`10-rendering-layout.md`](10-rendering-layout.md); the a11y action model
[`30-accessibility.md`](30-accessibility.md); per-OS dismiss/focus specifics
[`20-platform-macos.md`](20-platform-macos.md),
[`21-platform-windows.md`](21-platform-windows.md),
[`22-platform-linux.md`](22-platform-linux.md).

---

## 1. Model: one pure state machine, thin platform translation

All navigation *logic* lives in the pure `keynav` module — no window, no event
loop, no platform call — so it is exhaustively unit-tested off any loop
(`keynav.rs` has 15 tests). A backend's only job is to (a) translate its platform
key event into a `NavKey`, (b) call `handle_key`, and (c) apply the returned
`NavAction` to windows. The **same `MenuFocus` the keyboard drives is the focus the
mouse hover-stack and the a11y layer read** (§4, §8), so keyboard, mouse, and screen
reader never disagree about what is selected.

```rust
pub enum NavKey { Down, Up, Right, Left, Activate, Escape, Home, End, Char(char) }

pub struct FlyoutFocus { pub parent: usize, pub child: Option<usize> }
// 1.0-change (decision #8, N-level submenus): the single `Option<FlyoutFocus>`
// becomes a focus STACK. Each frame is one open flyout level; `parent` indexes
// into the menu one level up, `child` selects within this level.
pub struct MenuFocus   { pub top: Option<usize>, pub flyout: Vec<FlyoutFocus> }

pub enum NavAction { None, Redraw, OpenFlyout(usize), CloseFlyout, Activate(MenuId), CloseAll }
// OpenFlyout(i) opens a flyout for row `i` of the *currently deepest* open level
// (top-level when the stack is empty); it PUSHES a frame. CloseFlyout POPS one.

pub fn handle_key(menu: &Menu, focus: &mut MenuFocus, key: NavKey) -> NavAction;
```

`MenuFocus.top` and each stack frame's `FlyoutFocus.parent`/`child` are indices into
`Menu::items` (and, per level, the corresponding nested submenu's items) — **the same
index space as `LaidRow.index` and `AxNode.item_index`** (doc 10 §5, doc 30). This shared index is the load-bearing
invariant of the whole interaction layer.

## 2. Keyboard navigation (shipped, in `keynav`)

Selection **wraps** and **skips every non-focusable item** — separators, section
headers, disabled rows, and inert (`MenuId::none()`) info rows, i.e. everything
`Item::is_interactive()` marks `false` (doc 01 §2). `focusable_indices` is the
single source of truth.

### 2.1 Top-level transition table (no flyout open)

| Key | Precondition | Effect | `NavAction` |
|---|---|---|---|
| `Down`/`Up` | — | move to next/prev focusable, wrapping; from `None`, `Down`→first, `Up`→last | `Redraw` |
| `Home`/`End` | — | select first / last focusable | `Redraw` |
| `Char(c)` | a later focusable row's name starts with `c` (ASCII-ci) | jump to it (wraps) | `Redraw` / `None` |
| `Right` | selected row is a submenu | open its flyout, focus first child | `OpenFlyout(i)` |
| `Right` | selected row is a leaf | nothing | `None` |
| `Activate` (Enter/Space) | submenu selected | open its flyout | `OpenFlyout(i)` |
| `Activate` | enabled leaf with a real id | fire it, dismiss stack | `Activate(id)` |
| `Activate` | inert/disabled/no selection | nothing | `None` |
| `Left` | — | nothing (already at top) | `None` |
| `Escape` | — | dismiss the whole menu | `CloseAll` |

### 2.2 In-flyout transition table (a flyout stack is open)

Keys act on the **deepest** open level; the top `FlyoutFocus.child` on the stack
moves. Because a submenu can nest arbitrarily (decision #8), `Right`/`Activate` on
a submenu child **descends** (pushes a level) rather than no-opping.

| Key | Precondition | Effect | `NavAction` |
|---|---|---|---|
| `Down`/`Up`/`Home`/`End`/`Char` | — | move/jump within the deepest level (wraps, skips non-focusable) | `Redraw` / `None` |
| `Left` or `Escape` | — | close the deepest flyout, focus returns to its parent row (pop one frame) | `CloseFlyout` |
| `Right` | selected child is a submenu | open its nested flyout, push a level, focus first grandchild | `OpenFlyout(i)` |
| `Right` | selected child is a leaf | nothing | `None` |
| `Activate` (Enter/Space) | selected child is a submenu | open its nested flyout, push a level | `OpenFlyout(i)` |
| `Activate` | child leaf with a real id | fire it, dismiss stack | `Activate(id)` |
| (any) | a parent up the stack is no longer a submenu (menu swapped underneath) | drop the stale sub-stack, focus the nearest live parent | `CloseFlyout` |

`Escape` semantics are a **stack pop**: with one or more flyouts open it closes the
*deepest* one (`CloseFlyout`), unwinding one level per press; at the top it closes
everything (`CloseAll`). The
`escape_pops_one_level_then_closes_all` test locks this.

### 2.3 Platform key translation (shipped, macOS `translate_key`)

`winit` `Key` → `NavKey`: `ArrowDown/Up/Right/Left` → directional; `Enter`/`Space`
→ `Activate`; `Escape` → `Escape`; `Home`/`End` → those; `Key::Character(s)` →
`Char(first char)`; everything else → `None` (ignored). Windows and Linux backends
adopt the same table against their key events (doc 21, doc 22). **1.0 requirement:**
the translation table is per-backend but must be identical in meaning; a shared
`translate_key` helper (parameterized by the backend's key type, or a small
conversion at the winit layer since all three backends use winit) is preferred so
the mapping can't drift.

### 2.4 Type-ahead: shipped behavior and the 1.0 gap

Shipped `type_ahead`: from the item after the current selection (wrapping), the
first focusable row whose accessible name (`Row::accessible_name()`, segment text
joined by spaces — doc 01 §2) **starts with** the typed char, compared with
`to_ascii_lowercase`. Single character, first-letter only.

Gaps 1.0 must close (⚠ owner-confirm on the exact timeout):
- **Multi-char buffering.** Native menus accumulate a buffer within a timeout
  (~0.5–1s of inactivity resets it), so typing "se" lands on "Settings" not the
  next `s`-row. 1.0-new: a `TypeAhead { buffer: String, last: Instant }` owned by
  the *backend* (timing is I/O, not pure), feeding `keynav` a resolved prefix. Keep
  `keynav` pure by adding `NavKey::TypeAheadPrefix(String)` (or passing the buffer
  in), so the timeout lives in the loop and the matching stays unit-tested.
- **Prefix vs first-letter.** With a buffer, match by prefix; with a single repeated
  letter, cycle among rows starting with it (native behavior). Both are expressible
  once the buffer exists.
- **Unicode case folding.** `to_ascii_lowercase` leaves accented/CJK text unfolded,
  so type-ahead is effectively ASCII-only today. 1.0 should fold with Unicode
  lowercasing (`char::to_lowercase`) for the compared prefix.
- **⚠ owner-confirm:** the reset timeout value (propose 1.0s) and whether Space is
  reserved for `Activate` (shipped) or contributes to the type-ahead buffer once a
  buffer is non-empty (native menus treat a leading space as type-ahead). Propose:
  Space activates when the buffer is empty, appends when it is not.

## 3. Accelerators & mnemonics (1.0-new — concrete model)

muri has **no** accelerator code today; this is a 1.0-new subsystem, and the model
is spelled out here because the facade (doc 02 §5) depends on it. Global system
hotkeys remain an explicit **non-goal** (doc 00 §3) — that is the `global-hotkey`
crate's job.

### 3.1 Data model

```rust
// 1.0-new, in muri::menu (mirrors muda::accelerator so the facade shares it)
pub struct Accelerator { pub mods: Modifiers, pub key: Code }
pub struct Modifiers { pub cmd_or_ctrl: bool, pub shift: bool, pub alt: bool, pub meta: bool }
pub enum Code { /* KeyA..KeyZ, Digit0..9, F1..F24, named keys … (muda's `Code`) */ }

impl Accelerator {
    pub fn from_str(s: &str) -> Result<Self, Error>;   // "CmdOrCtrl+Shift+S", muda syntax
    pub fn display(&self) -> String;                    // platform glyphs, §3.3
}
```

Attach point: a **1.0-new `Row.accelerator: Option<Accelerator>`** field (preferred
over a facade sidecar map, so muri's native API also exposes accelerators). Empty
by default; `#[non_exhaustive]` growth on `Row` is already the policy (doc 03 §5).

### 3.2 Display (dogfoods the flush-right feature)

An accelerator renders as a **right-aligned trailing segment** in the row: the
renderer synthesizes `Segment::new(acc.display()).align(Align::Right)` in
`secondary_label` color when a row has an accelerator and no author-supplied
trailing value. This is exactly the `Flex::Grow` label + `Align::Right` value case
the layout engine exists for (doc 10 §4) — accelerator display is free once
flush-right works, and needs no new drawing primitive. When a menu has submenus the
accelerator sits left of the reserved chevron column (doc 10 §4).

### 3.3 Platform display conventions

`display()` is platform-specific formatting of the *same* `Accelerator`:
- **macOS:** glyphs, no separators, in canonical order — ⌃⌥⇧⌘ then the key (e.g.
  `⇧⌘S`). `CmdOrCtrl` → ⌘.
- **Windows/Linux:** words joined by `+`, e.g. `Ctrl+Shift+S`. `CmdOrCtrl` → `Ctrl`.

### 3.4 Dispatch (in-menu only)

| Context | Behavior |
|---|---|
| **Native menu-bar passthrough** (doc 02 §6) | the native menu registers and fires the accelerator app-wide while focused — full fidelity, muri does nothing. |
| **muri custom surface, menu open** | on a `KeyboardInput` with modifiers, muri matches against visible rows' accelerators (this level + open flyout) and, on a match, dispatches that row exactly like `NavAction::Activate` and closes the stack. |
| **muri custom surface, menu closed** | **does not fire** — muri never registers a system/app-wide hotkey (doc 00 §3, doc 02 D5). |

This is the load-bearing divergence the facade documents loudly (doc 02 §5, D5): a
muda accelerator on a *tray/context* item that the app expects to fire from anywhere
will not, once that surface is muri-drawn. The `keynav` machine gains a
`NavKey::Accelerator(Accelerator)` (or the backend resolves the row and issues
`NavAction::Activate(id)` directly) so matching stays pure and testable.

### 3.5 Mnemonics (`&File`)

- **Passthrough:** native (Windows/Linux underline-access).
- **Custom surface:** muri parses a single leading `&` mnemonic in a segment's text,
  strips it from the drawn glyphs, underlines the mnemonic character, and — while
  the menu is open — maps Alt+letter to selecting/activating that row. Parsing is
  pure (a `mnemonic_of(&Row) -> Option<char>` in `keynav`/`menu`); underline
  drawing is a small renderer addition (doc 10). On macOS, which has no menu
  mnemonics convention, mnemonics are **parsed-and-stripped but not underlined or
  bound** by default (the `&` must not leak into the drawn text on any platform).

### 3.6 Adversarial: accelerator conflicts

- **Host-app conflict.** While a muri popup is the focused/key window, its key
  events go to muri, not the host — so a host Cmd+S won't reach the app while the
  menu is open. This is acceptable (the menu is modal-ish) but must be documented.
- **Duplicate accelerators** within one menu: first match in item order wins;
  document it and consider a debug-assert.
- **Modifier normalization** across platforms (`CmdOrCtrl`, `Meta` vs `Super`) must
  match muda's parsing exactly so migrated strings behave identically (doc 02 §5).

## 4. Mouse / hover-stack, and how it stays in sync with the keyboard

Hover-stack logic is pure (`flyout::next_flyout` + `HoverTarget`); geometry is pure
(`flyout::place_flyout`, doc 10 §12). The **key sync fact:** the backend stores the
current selection in a single `hovered: Option<usize>` per window and, when a key
arrives, builds a `MenuFocus` *from* those hovered fields, calls `handle_key`, then
writes the result *back* into the same hovered fields (macOS `App::on_key`). So the
mouse highlight and keyboard selection are literally the same state — moving the
mouse then pressing Down continues from the hovered row, and vice versa.

`HoverTarget` and the transition (`next_flyout(current, target) -> Option<usize>`):

| Pointer over | `HoverTarget` | Resulting open-flyout parent |
|---|---|---|
| a submenu parent row | `ParentRow(i)` | `Some(i)` — open/switch to it |
| a non-submenu row | `OtherRow` | `None` — close any open flyout |
| the open flyout panel, or the gap crossed to reach it | `Flyout` | unchanged (stays open) |
| outside the menu entirely | `Outside` | unchanged (dismissal is a click-outside/Esc concern) |

Backend mapping (macOS `on_parent_cursor`): a hovered submenu row → `ParentRow`, a
hovered non-submenu row → `OtherRow`, no hovered row → `Outside`; then
`next_flyout` decides open/switch/close. `on_flyout_cursor` keeps the flyout open
and highlights the hovered child. Hover-only-changed cursor moves request a redraw
only when the hovered index actually changed (doc 10 §10).

**Adversarial:** the `Flyout`/`Outside` "keep current" rule is what lets the pointer
travel diagonally from a parent row across empty space into the flyout without it
snapping shut (the classic submenu "triangle" problem) — muri approximates it with
"crossing the gap counts as `Flyout`," which the backend must classify correctly
from the two windows' geometry. A true motion-prediction triangle is **out of scope
for 1.0**; document the gap-as-Flyout approximation.

## 5. The flyout stack (N-level submenus — locked decision #8)

**Shipped baseline:** the current code opens exactly **one** flyout level — the
backend holds `Option<Flyout>` (one child window); `keynav`'s in-flyout
`Right`/`Activate`-on-submenu are no-ops; `FlyoutFocus` has no recursion. That is
the *starting point*, not the 1.0 target.

**Locked for 1.0 (decision #8, doc 00):** muri supports **N-level nested submenus**,
so a submenu inside a submenu opens a further flyout, arbitrarily deep. This is not
provisional and is not relitigated here; the muda-compat drop-in maps a deeply
nested muda menu **directly** (no flattening — doc 02), because muri's own model now
nests to the same depth. What 1.0 implements, concretely:

- **State — a focus stack.** `MenuFocus.flyout` becomes `Vec<FlyoutFocus>` (§1): a
  stack of open levels. `keynav` `Right`/`Activate` on a submenu child **pushes** a
  frame (`OpenFlyout` at the deepest level); `Left`/`Escape` **pops** one frame
  (`CloseFlyout`); `Escape` at the top closes everything (`CloseAll`). The
  `escape_pops_one_level_then_closes_all` test generalizes to N pops.
- **Windows — a window stack.** The backend replaces `Option<Flyout>` with a
  `Vec<Flyout>`; each level is a separate OS window placed by `place_flyout`
  **relative to the previous level's panel** (right by default, flipped left on
  spill, clamped — doc 10 §12). Opening a flyout at level *k* drops any levels
  deeper than *k* first (switching parents truncates the stack).
- **Hover stack.** `next_flyout` generalizes to "(which level, which parent)"; the
  diagonal gap-crossing rule (§4) must hold across each pair of adjacent panels in
  the stack, not just parent→first-flyout.
- **Dismiss/focus.** The `focused: HashSet<WindowId>` model already generalizes to N
  windows (§6): the "set empty ⇒ dismiss the whole stack" rule is unchanged, and a
  newly created deeper flyout is pre-marked focused exactly as the first level is.
- **A11y is the real cost, not the geometry.** Each level is its own OS window, so
  N levels **multiply** the unverified cross-OS-window screen-reader traversal
  problem — a screen reader must follow focus descending *and* ascending across N
  window boundaries. This is the hardest part of decision #8 and is owned by
  [`30-accessibility.md`](30-accessibility.md) (Option B fans a per-window adapter
  out down the whole stack; the §3.3 verification generalizes to pushing/popping
  arbitrary depth). The keynav/geometry generalization above is mechanical; the a11y
  boundary-crossing across a *stack* is the 1.0 gate risk to attack.

## 6. Dismiss semantics across surfaces

Dismissal is a per-backend concern (focus is an OS notion), but the *rules* are
uniform across tray / context / dropdown, including the separate-OS-window flyout.

| Trigger | Behavior | Shipped (macOS) | Windows / Linux |
|---|---|---|---|
| **Esc** | pop one level; at top, close all | `CloseFlyout`/`CloseAll` from `keynav` | same (shared `keynav`) |
| **Activation** (click / Enter / a11y Click on a leaf) | dispatch `MenuId`, then close the whole stack | `dispatch` + `close_popup` | same |
| **Click-outside** | close the whole stack | via focus-loss (clicking elsewhere defocuses the popup) | Windows: `WH_MOUSE_LL` low-level mouse hook + `WM_ACTIVATEAPP` (bare `Focused(false)` is unreliable) — doc 21; Linux: pointer grab / `xdg_popup` auto-dismiss — doc 22 |
| **Focus loss** | close all **only when no muri window holds focus** | `focused: HashSet<WindowId>`; remove on `Focused(false)`, `close_popup` when empty | same set model (doc 03 §2) |
| **Tray icon re-click** | toggle closed | `UserEvent::ToggleTray` | same via each backend's toggle |

**The multi-window focus subtlety (shipped, load-bearing).** A flyout is a second OS
window, so opening it momentarily moves focus off the parent. If dismissal keyed on
the parent alone losing focus, opening a submenu would close the menu. muri tracks
**a set** of its window ids and dismisses only when the set is *empty*, and
pre-inserts a newly created flyout's id before it can steal-then-report focus. This
is the single trickiest piece of the live loop; it is why `Focused(false)` handling
checks `self.focused.is_empty()` rather than reacting to any single window.

**Adversarial dismissal hazards to hand to review:**
- **`WindowLevel::AlwaysOnTop` ≠ a non-activating `NSPanel`.** The shipped macOS
  popup is a borderless always-on-top winit window, not a true non-activating panel.
  It can steal app activation, and — the deeper problem — a non-activating panel
  that *can't* become key won't receive `KeyboardInput` at all, yet muri needs key
  events for navigation (§2). The code comment concedes keyboard nav is
  "device-verified interactively." **1.0 must resolve** whether the popup is a real
  non-activating `NSPanel` that can still host a first responder for keys (doc 20),
  or accepts brief activation. This is the biggest open input risk on macOS.
- **Click-outside via focus-loss only** (macOS) misses the case where no other
  focusable window exists (a pure `Accessory` tray app): clicking the desktop may
  not deliver `Focused(false)`. 1.0 should add an explicit global mouse monitor
  (like the Windows `WH_MOUSE_LL` path) rather than relying solely on focus.
- **Right-click / secondary activation, double-click, and drag** are not modeled;
  1.0 defines them as no-ops in a menu (a menu row is a single-click target).

## 7. IME / composed input into type-ahead

**Shipped:** none. `translate_key` reads `Key::Character` from `KeyboardInput`;
`winit` `Ime` events (`Preedit`/`Commit`) are not handled, so composed input (CJK,
dead keys, accented input) never reaches type-ahead.

**1.0 requirements (⚠ owner-confirm on scope):**
- Handle `WindowEvent::Ime(Ime::Commit(s))` by feeding the *committed* string into
  the type-ahead buffer (§2.4) grapheme-by-grapheme; ignore `Preedit` for menu
  navigation (a menu has no text field to show a preedit in). This lets a composed
  character participate in type-ahead once committed.
- **Do not** attempt to host an IME edit session in the popup — muri draws menus,
  not text fields (doc 00 §3). Type-into-a-field belongs in a host window.
- **Adversarial:** on a non-activating popup the OS may not route IME to the window
  at all (same first-responder problem as §6); IME-into-type-ahead is therefore
  gated on the §6 macOS panel decision. Reasonable 1.0 scope: **commit-only,
  best-effort, verified where the OS delivers it; documented as best-effort
  elsewhere.**

## 8. Mapping input onto the a11y action model

Keyboard, mouse, and screen-reader input converge on the same transitions; the a11y
bridge (doc 30) is just a fourth input source into the identical `MenuFocus`. On
macOS `on_a11y_action` handles AccessKit `ActionRequest`s routed through the
`UserEvent` channel (doc 03 §2):

| AccessKit `Action` | Maps to | muri behavior |
|---|---|---|
| `Focus` | move selection | set `hovered`/flyout child from `a11y::locate`; open the flyout if the target is inside a collapsed submenu; `sync_a11y` |
| `Click` on a leaf | `NavAction::Activate` | `dispatch(id)` + `close_popup` |
| `Click` on a submenu parent | `NavAction::OpenFlyout` | open the flyout, `sync_a11y` |

`a11y::locate(tree, AxId) -> (top, Option<child>)` maps the AT's node id back to the
shared item-index space, and `focused_id`/`set_expanded` keep the published tree in
lockstep after every keyboard/mouse/AT transition (`sync_a11y` is called from every
input path). The rule for review: **there is exactly one focus model
(`MenuFocus`), and all four input sources read and write it** — this is what makes
"AT focus moves in lockstep with keyboard nav" (doc 00 §7) true by construction, not
by re-synchronization. The unresolved cross-OS-window flyout traversal is doc 30's
gate (§5, doc 30).

## 9. Summary of 1.0-new / 1.0-change input work

- **1.0-new:** accelerator subsystem (`Accelerator`, `Row.accelerator`, display,
  in-menu dispatch, mnemonics) — §3; multi-char type-ahead buffer with timeout and
  Unicode folding — §2.4; IME commit → type-ahead — §7; a shared cross-backend
  key-translation table — §2.3.
- **1.0-change:** the **flyout focus stack** for N-level submenus (`MenuFocus.flyout`
  → `Vec<FlyoutFocus>`, backend `Vec<Flyout>`, `Right`/`Activate` push, `Left`/`Esc`
  pop) — §5 (locked decision #8); resolve the macOS non-activating-panel vs key-event
  tension — §6; add an explicit global click-outside monitor rather than
  focus-loss-only — §6.
- **⚠ owner-confirm:** type-ahead timeout/Space behavior — §2.4; IME scope — §7;
  per-row vs per-menu chevron gutter interaction with accelerator display — §3.2 /
  doc 10 §4. (N-level flyout depth is **no longer** an open question — locked #8, §5.)
- **Hardest tensions handed to adversarial review:** the non-activating-popup ↔
  keyboard/IME first-responder problem (§6, §7); the multi-window focus-set dismiss
  correctness (§6); the submenu-triangle gap approximation (§4); and accelerator
  divergence for menu-bar items migrated into a custom surface (§3.4, doc 02 D5).
