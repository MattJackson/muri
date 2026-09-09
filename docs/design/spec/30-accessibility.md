# 30 — Accessibility: the all-screen-reader gate

Status: **foundational spec** for the single riskiest 1.0 claim. Locked decision #4
([`00-overview.md` §4](00-overview.md)) requires **every** target screen reader —
VoiceOver (macOS), NVDA **and** Narrator (Windows), AT-SPI / Orca (Linux) — to be
**live-verified**, not merely "AccessKit is wired." This document specifies the
accessibility model, the per-platform AccessKit bridge, the unresolved
cross-OS-window flyout problem, focus/announcement behavior, OS-setting honoring,
and the human-in-the-loop verification plan that *is* the 1.0 gate.

Cross-references: the menu/`Row` model and `accessible_name()`
([`01-api-contract.md`](01-api-contract.md)); the keyboard-nav state machine and
mnemonics/accelerators ([`40-input-interaction.md`](40-input-interaction.md)); the
per-platform windowing and focus reality
([`20-platform-macos.md`](20-platform-macos.md),
[`21-platform-windows.md`](21-platform-windows.md),
[`22-platform-linux.md`](22-platform-linux.md)); the verification protocol and CI
matrix ([`50-testing-verification.md`](50-testing-verification.md)); threading and
the feature-flag matrix ([`03-threading-events-versioning.md`](03-threading-events-versioning.md)).

> **Why this is the hardest 1.0 claim.** Native menus (`NSMenu` / `HMENU` / GTK)
> give accessibility *for free* because the OS both draws them and exposes them.
> muri throws that away: a `tiny-skia` pixmap is an **opaque rectangle** to an
> assistive technology — no `NSView` subtree, no UIA provider, no AT-SPI object to
> enumerate. Everything an AT reads, muri must *manufacture* and keep truthful. A
> plausible-looking tree is necessary but **nowhere near sufficient**: the tree can
> be perfect and a real screen reader can still announce nothing, because
> AccessKit's per-platform fidelity varies and — the crux of this document —
> **focus does not obviously cross an OS-window boundary**. Read this whole
> document adversarially. Every "should work" below is a claim that a human must
> falsify or confirm on a real device.

---

## 0. Locked decision — `a11y` on by default (#7)

**Locked** ([`00-overview.md` §4](00-overview.md) decision #7,
[`03-threading-events-versioning.md` §4](03-threading-events-versioning.md)): the
`a11y` cargo feature is **on by default in 1.0**. `accesskit` + `accesskit_winit` +
the platform adapters are **default dependencies** in every default build. The
parallel a11y tree is not an add-on — it is the *only* thing that gives a
custom-drawn menu correct semantics, and decision #4 (all screen readers verified)
makes it non-optional. The flag survives so a size-constrained embedder can
explicitly drop it, but the default **includes** it.

This whole document — the verification gate (§7), the CI matrix
([`50`](50-testing-verification.md)), and the "menu semantics are correct out of the
box" promise (§6) — is written to default-on and depends on it. **The rationale for
locking it on** (not merely defaulting): with a11y off, a plain muda drop-in
(`use muri::compat::muda as muda;`) would *silently lose* the accessibility muda's
native menus gave for free — a regression that would break the "unchanged muda app
just works" bar the compat on-ramp promises ([`02` §1](02-muda-compat.md)). Locking
a11y on is what makes that bar honest. Nothing in this document is conditioned on
this any longer; it is decided.

---

## 1. The accessibility model — the menu tree *is* the a11y tree

muri publishes a **parallel accessibility tree** that mirrors the declarative
`Menu` structure and is kept in sync with what is drawn and focused. It is a pure,
backend-neutral value type in `src/a11y.rs` (**shipped**, densely unit-tested), fed
to the platform AT through AccessKit by the live backend.

### 1.1 Types (shipped)

```rust
pub struct AxId(pub u64);                 // depth-first from 0; maps 1:1 to accesskit::NodeId(u64)
pub enum AxRole { Menu, MenuItem, MenuItemCheckbox, GroupLabel, Separator }
pub struct AxNode {
    pub id: AxId, pub role: AxRole, pub name: String,
    pub enabled: bool, pub checked: Option<bool>,
    pub has_popup: bool, pub expanded: Option<bool>,
    pub pos_in_set: Option<usize>, pub set_size: Option<usize>,
    pub item_index: Option<usize>,        // back-reference into Menu::items for action routing
    pub children: Vec<AxNode>,
}
pub struct AxTree { pub root: AxNode }     // root is always AxRole::Menu
```

### 1.2 Mapping from `Menu` to roles/states (`build_tree`, shipped)

| muri construct | `AxRole` | AccessKit `Role` | Focusable | Notes |
|---|---|---|---|---|
| the popup / flyout container | `Menu` | `Role::Menu` | no | root; empty name |
| `Item::Row` (no checked state) | `MenuItem` | `Role::MenuItem` | if `enabled && id != none()` | name = `Row::accessible_name()` |
| `Item::Row` with `checked: Some(_)` | `MenuItemCheckbox` | `Role::MenuItemCheckBox` | as above | `checked` → `Toggled::True/False` |
| `Item::SectionHeader` | `GroupLabel` | `Role::Label` | **no** | dimmed/inert heading |
| `Item::Separator` | `Separator` | `Role::Splitter` | **no** | structural only |
| `Item::Submenu { label, .. }` | `MenuItem`/`MenuItemCheckbox` | as row | yes | `has_popup = true`, `expanded = Some(false)`; children built recursively |

**Name.** `AxNode.name` comes from `Row::accessible_name()` — the segment texts
joined by spaces ([`01` §2](01-api-contract.md)). This is deliberately the *same*
string type-ahead ([`40`](40-input-interaction.md)) matches on, so what a sighted
user types and what a screen reader speaks are guaranteed identical.

**States.** `enabled` from `Row.enabled`; `checked` from `Row.checked` (drives the
checkbox role *and* the `Toggled` state — a checked row is both); `has_popup`/
`expanded` only for `Item::Submenu`.

**Set-position (`pos_in_set` / `set_size`).** The single subtlety worth guarding:
positions count **only focusable siblings**, so a screen reader announces "3 of 4",
not "6 of 7", skipping the header, the separator, and the disabled row. This is
computed against `Menu::interactive_count()` — the *same* source of truth
`keynav` uses to decide what arrow keys land on — so "N of M" always agrees with
where the highlight actually stops. The shipped test
`set_position_counts_only_focusable_siblings` locks this.

### 1.3 Navigation helpers (shipped)

- `focused_id(&AxTree, &MenuFocus) -> Option<AxId>` — maps a keynav selection
  ([`40`](40-input-interaction.md)) onto the node the backend should tell the AT to
  focus: the selected flyout child, else the open flyout's parent, else the
  selected top-level row.
- `locate(&AxTree, AxId) -> Option<(usize, Option<usize>)>` — the **inverse**:
  maps an AT-initiated action (whose target is an `AxId`) back to a position in the
  live `Menu` to act on. The shipped signature resolves top-level + one flyout level
  deep; **decision #8 (N-level, [`40` §5](40-input-interaction.md)) requires
  generalizing it** to a path down the flyout stack (e.g. `Vec<usize>` or
  `(top, Vec<usize>)`), so an AT action targeting a deeply-nested row resolves to the
  right level. This is part of the same fan-out-to-N-adapters refactor as §3.2
  Option B.
- `set_expanded(&mut AxTree, parent, expanded)` — flips a submenu node's `expanded`
  in place when the backend opens/closes its flyout, so the state (and the
  resulting AT notification) stays truthful.
- `announcement(&AxNode) -> String` — a deterministic, **test-only** rendering of
  what a node "says" (`"Settings, submenu, collapsed, menu item, 3 of 4"`). The
  live AT composes its *own* phrasing from the structured fields; `announcement`
  exists so tests can assert the composed meaning without a screen reader.

### 1.4 `accessibility_label` for icon-only rows (1.0-new, adopt)

`Row::accessible_name()` derives the name from segment **text**. A row that is only
an icon (`leading`/`trailing` with no segments) produces an **empty name** — silent
to a screen reader. [`01` §2](01-api-contract.md) flagged this to us; **this spec
adopts** an optional override:

```rust
// 1.0-new, on Row
pub accessibility_label: Option<String>,          // overrides accessible_name() for the AT
pub fn accessibility_label(self, s: impl Into<String>) -> Self;
```

`AxNode.name` becomes `row.accessibility_label.clone().unwrap_or_else(|| row.accessible_name())`.
This is additive and Tier-1-safe. **Verification obligation:** any row whose visible
content is icon-only *must* set it, or the tree carries an empty-name node; §7's
review pass explicitly hunts for empty `MenuItem` names as a defect.

---

## 2. Per-platform AccessKit bridge

The pure tree becomes platform accessibility through AccessKit. `src/a11y.rs`'s
`accesskit` submodule (`#[cfg(feature = "a11y")]`) converts an `AxTree` +
focused `AxId` into an `accesskit::TreeUpdate` (`tree_update`), one AccessKit node
per muri node, `NodeId(id.0)`, `focus` set to the focused node (root when nothing
is selected). Mapping is shipped and tested (`accesskit_update_carries_every_node_and_the_focus`).

The backend then hands that `TreeUpdate` to a per-window platform **adapter**:

| Platform | AccessKit adapter | OS API it drives | muri status |
|---|---|---|---|
| macOS | `accesskit_winit::Adapter` (→ `accesskit_macos`) | NSAccessibility (`AXMenu`/`AXMenuItem`) | **shipped, device-unverified** |
| Windows | `accesskit_winit` (→ `accesskit_windows`) | UIA (`Menu`/`MenuItem` control patterns) | 1.0-new (groundwork) |
| Linux | `accesskit_winit` (→ `accesskit_unix`) | AT-SPI (`menu`/`menu item`) | 1.0-new (skeleton) |

Dependency triple pinning (`winit` 0.30 / `accesskit` 0.17 / `accesskit_winit`
0.23, bump together) is in [`03` §4](03-threading-events-versioning.md).

### 2.1 Adapter attach lifecycle (macOS, shipped — the reference)

From `src/tray/macos.rs`:

1. **Create hidden, attach, then show.** AccessKit must attach *before* the window
   is first shown. `open_popup` builds the window with `.with_visible(false)` under
   `a11y`, constructs `Adapter::with_event_loop_proxy(&window, proxy)`, then calls
   `window.set_visible(true)`. Getting this order wrong means the AT never sees the
   window. **This is a live-loop ordering invariant, not covered by any unit test**
   — §7 must confirm it on device.
2. **Feed every window event to the adapter first.** In `window_event`, for the
   popup window, `adapter.process_event(&win, &event)` runs *before* muri handles
   the event, so focus/bounds/HiDPI changes reach NSAccessibility.
3. **Adapter events arrive as `UserEvent::Accessibility`** through the same
   `EventLoopProxy` the tray click uses, applied on the UI thread
   ([`03` §2](03-threading-events-versioning.md)):
   - `InitialTreeRequested` → push the current tree (`sync_a11y`);
   - `ActionRequested(req)` → apply it to the live popup (§4.2);
   - `AccessibilityDeactivated` → no-op.
4. **Lazy activation.** `sync_a11y` calls `adapter.update_if_active(|| …)`, so the
   tree is only *built and pushed when an AT is actually listening* — no cost when
   no screen reader is running. The tree is rebuilt from the live `Menu` + current
   `MenuFocus` on every focus / hover / flyout / content change.
5. **Teardown.** `close_popup` drops the adapter (`self.adapter = None`) with the
   window.

**Windows / Linux must adopt the identical shape** — one `UserEvent`-equivalent
queue, adapter attached before show, events fed adapter-first — per the unified
threading model in [`03` §2](03-threading-events-versioning.md). Divergence here is
the most likely source of a per-platform a11y bug.

### 2.2 Action-request handling — AT actions onto the live menu (shipped, macOS)

`on_a11y_action` mirrors the mouse/keyboard paths so a VoiceOver/NVDA action moves
the *real* highlight, not a phantom AT cursor:

- Resolve `req.target` via `a11y::locate` to `(top, child)`.
- `Action::Focus` → set `hovered = top`; if a `child` is named and the flyout for
  `top` isn't open, open it, then highlight the child; repaint; `sync_a11y`.
- `Action::Click` → a submenu parent (`child.is_none() && submenu_child(top)`)
  **expands its flyout**; a leaf row **dispatches** its `MenuId` and closes the
  stack. Inert ids never fire. Dispatch goes through the same `Tray::dispatch` path
  as mouse/keyboard, so the `on_click` closure **and** the global `MenuEvent`
  channel both fire ([`01` §6](01-api-contract.md),
  [`03` §3](03-threading-events-versioning.md)).

This is the one place the a11y layer *mutates live UI*, and it is the second thing
§7 must verify per AT (does VO's "press" actually activate?).

---

## 3. THE hard problem — a flyout is a separate OS window (now a STACK of them)

This is the load-bearing unsolved risk of the whole 1.0 accessibility claim, and it
is called out in [`00` §7](00-overview.md) and in the `src/tray/macos.rs` `App`
doc comment. **Decision #8 (N-level nested submenus, [`00` §4](00-overview.md),
[`40` §5](40-input-interaction.md)) makes it strictly harder:** a menu is no longer
one parent + at most one flyout, but a parent plus a **stack of up to N flyout OS
windows**, each opened from a row in the level above it. Every claim below that was
already suspect for one flyout is **multiplied by depth** — the screen reader must
follow focus cleanly across an OS-window boundary at *every* level, in and back out.

### 3.1 What is true today (and why it is suspect)

- A flyout submenu is drawn in a **second `winit`/OS window** (`struct Flyout`),
  positioned beside the parent by `place_flyout`. Under decision #8 the shipped
  single flyout generalizes to a **`Vec<Flyout>` — a stack of windows**, level *k+1*
  placed relative to level *k* ([`40` §5](40-input-interaction.md)).
- **Only the parent popup window has an AccessKit adapter today.** The flyout
  window(s) have **none**. Flyout items exist purely as **nested child nodes inside
  the parent window's tree** (`build_tree` recurses into every submenu; `sync_a11y`
  calls `set_expanded` and pushes the whole nested tree to the *parent's* adapter).
- To keep the menu from dismissing, muri deliberately keeps each flyout's
  `WindowId` in the `focused` set and only closes when *no* muri window is focused —
  the set already generalizes to N windows.

The unverified assumption baked into this: **that a screen reader will announce a
node living in window A's accessibility tree while OS keyboard/AT focus is on window
B (the flyout).** That is very likely **false**, because on every target platform
the accessibility topology is *per-OS-window*:

- **macOS:** each window has its own `AXUIElement` root. VoiceOver's cursor tracks
  the key window. If the flyout window becomes key, VO looks at the flyout window's
  (empty, unadapted) tree and finds **nothing** — the parent's nested nodes are in
  a different window's tree and unreachable.
- **Windows/UIA:** each `HWND` is its own fragment root. A popup menu is
  conventionally its *own* HWND with its own provider, related to the opener via
  `ControllerFor`. Nesting the flyout's items under the parent HWND's provider
  while a *different* HWND is on screen contradicts the UIA tree UIA actually walks.
- **Linux/AT-SPI:** each toplevel is its own accessible object; Orca follows the
  focused toplevel.

So the shipped model (Option A below) produces a *correct-looking tree* that a real
screen reader may be **unable to reach** the moment the flyout takes focus. That is
exactly the "plausible tree that doesn't actually work" failure this spec exists to
prevent.

### 3.2 The options

**Option A — nested-in-parent tree, no flyout adapter (shipped).**
One adapter on the parent; flyout items are nested nodes; the flyout window is
accessibility-invisible.
- *Pro:* zero refactor; the tree is already built correctly; one focus target.
- *Con:* only works **if the flyout window never takes OS/AT focus** — i.e. the
  parent popup stays key and the flyout is purely visual. That requires a truly
  non-activating child surface on all three platforms and contradicts the current
  code, which tracks `Focused(true)` for the flyout. **Highest risk of announcing
  nothing.** Reject as the default.

**Option B — per-flyout-window adapter + explicit relation (CHOSEN for 1.0).**
**Every** flyout `winit` window in the stack gets its **own**
`accesskit_winit::Adapter`, rooted at its own `AxRole::Menu`. Each level's submenu
item keeps `has_popup`/`expanded` and declares an explicit AccessKit relationship to
the *next* level's window root (the child menu as the item's popup / `controls`; the
child root points back via a `member_of`/labelled-by relation as the platform
prefers). Focus is driven into **whichever adapter owns the currently focused
window**: pushing level *k+1* sends a `TreeUpdate` with `focus` = first child to
level *k+1*'s adapter and a `focus` = parent (expanded) update to level *k*'s
adapter; popping reverses it.
- *Pro:* matches the per-OS-window accessibility topology every platform enforces;
  is the model UIA popup menus already assume; lets the SR follow focus across the
  boundary the way it does for native submenus — and it is the *only* option that
  scales to the N-level stack decision #8 requires.
- *Con:* a real refactor, made larger by depth — `sync_a11y` must fan out to **N
  adapters** (one per open level); `locate`/`focused_id` must be relative to a
  window/level; and the cross-window *relation* is the exact thing AccessKit
  fidelity differs on per platform, so it is **precisely what §7 must verify
  hardest**, now at every level of the stack. Cross-window focus hand-off (does the
  SR cursor actually jump into level *k+1* and back out to level *k* on Left/Esc,
  repeatedly?) is the make-or-break test, and each added level is another chance for
  it to fail.

**Option C — single-window composite render, no separate flyout OS window
(documented fallback).**
Draw parent + flyout into **one** larger transparent window (bounding box of both
panels, transparent gap), so there is exactly one OS window, one adapter, and no
cross-window focus problem at all.
- *Pro:* dissolves the entire hard problem; the shipped nested tree (Option A's
  tree) becomes *correct* because it is genuinely one window.
- *Con (materially reduced by decision #6):* it requires per-pixel window
  transparency the shipped code does **not** have — `redraw`/`redraw_flyout` blit
  over an **opaque** theme background (softbuffer is opaque), so a transparent gap
  between panels is not expressible today. **But that transparency/compositing
  rework is now being built anyway** for `Theme::native()` vibrancy (decision #6,
  [`10` §11](10-rendering-layout.md), [`20` §2](20-platform-macos.md)): once the
  raster layer composites with real alpha over a native effect view, a transparent
  inter-panel gap is expressible on the same machinery. So Option C is a **more
  viable per-platform fallback than before** — its chief cost is largely absorbed.
  The residual cost: the single window must grow to bound an **N-level** stack
  (decision #8), and it revisits shadow/clipping and `place_flyout` relative to one
  window's coordinate space.

### 3.3 The pick and the fallback

**1.0 chooses Option B** (per-flyout-window adapter with an explicit parent↔child
relation) as the model that respects the platform accessibility topology and gives
the best chance of a screen reader actually crossing the boundary. **Option C is the
documented escape hatch:** if, during §7 verification, Option B's cross-window focus
hand-off proves unreliable on *any* single platform (a genuine possibility given
AccessKit's uneven per-platform relation support), 1.0 falls back to Option C on
that platform — accepting the transparency rework — rather than shipping a menu that
reads as empty. **Option A is rejected** as a shipped default precisely because its
tree looks right while likely announcing nothing.

**Hard verification requirements for the chosen model (§7 gate items) — must hold
at every level of the N-deep stack, not just the first:**
1. Open a flyout by keyboard (Right/Enter) — the SR cursor **moves into** the
   flyout and announces the first child with correct "N of M". Opening a *nested*
   flyout (Right again on a deeper submenu) moves the cursor one level deeper again.
2. Left/Esc — the SR cursor **returns to** the submenu item one level up, announced
   `expanded → collapsed`; repeated Left/Esc walks the stack back out level by
   level.
3. The submenu item at each level announces `has_popup` ("submenu") **and**
   `expanded` state at the moment focus is on it.
4. Mouse-hover opening a flyout drives the SR focus identically (hover and keyboard
   share one `MenuFocus`; `sync_a11y` is called from both cursor handlers), at every
   level.
5. No orphaned/duplicate announcements (a level's nested copy must not *also* be
   reachable under Option B — building each level under its own adapter means an
   ancestor adapter's tree must **not** additionally nest those same nodes, or the
   SR sees the items twice). **This dedup is a concrete Option-B implementation
   trap, and it recurs at every level of the stack.**

Until items 1–3 pass — **at multiple depths** — on **all four** screen readers, the
multi-window flyout stack is **not** verified and 1.0 is not shippable (decision
#4). Deeper nesting is more surface for the boundary crossing to fail, so §7 must
test at least a two-level-deep flyout, not only one.

---

## 4. Focus & announcements

### 4.1 One focus model, three drivers

Keyboard (`keynav`), mouse hover, and AT actions all mutate the **same**
`MenuFocus` ([`40`](40-input-interaction.md)); the a11y layer never has an
independent cursor.

- **Keyboard:** `on_key` runs the pure `handle_key`, updates `hovered`/flyout
  state, then calls `sync_a11y` — so the AT focus follows arrow keys in lockstep.
- **Mouse hover:** `on_parent_cursor`/`on_flyout_cursor` update the hovered row and
  call `sync_a11y` even when the flyout state itself is unchanged, so the announced
  focus tracks the highlighted row under the pointer.
- **AT action:** §2.2 — the AT drives the highlight *back* into the same state.

`sync_a11y` rebuilds the tree from the live `Menu` + `MenuFocus`, sets the open
flyout's `expanded`, computes `focused_id`, and pushes the `TreeUpdate`
(`update_if_active`, so free when no AT listens). Because focus is a *field of every
update* (not a separate call), the announced node can never drift from the drawn
highlight — they are computed from one state in one function.

**Adversarial notes on focus:**
- **The borderless popup must actually receive key events.** `on_key` is only
  reached if the OS delivers `KeyboardInput` to a borderless, non-activating winit
  window. macOS `WindowLevel::AlwaysOnTop` today is **not** a true non-activating
  `NSPanel` ([`20`](20-platform-macos.md)); whether it can host first-responder key
  input while the app is `Accessory` is device-unverified. If keys don't arrive,
  keyboard a11y silently does nothing regardless of a perfect tree.
- **First-focus announcement.** When the popup opens, does the SR announce the menu
  *and* land on a first item? `sync_a11y` pushes focus = root when nothing is
  selected. 1.0 should decide whether opening *pre-selects* the first focusable row
  for AT users (native menus do). Flagged; recommend pre-selecting the first
  focusable item on open under a11y so VO/NVDA announce a real item, not an empty
  container.
- **Live content churn.** usagio rebuilds the menu on a ~0.75s tick and swaps via
  `TrayHandle::set_menu` ([`03` §1](03-threading-events-versioning.md)). Every swap
  rebuilds `AxTree` with **fresh depth-first ids** — node identity is *not* stable
  across rebuilds. A screen reader mid-utterance during a swap may be talking about
  a node id that no longer means the same row. §7 must test "menu updates while VO
  is reading it" explicitly; if it's disruptive, 1.0 may need to suppress rebuilds
  while an AT action/utterance is in flight, or keep ids stable across rebuilds
  (keyed by `MenuId` rather than DFS order). **Open risk.**

### 4.2 Announcement content

The live AT composes phrasing from the structured fields (name, role,
checked/`Toggled`, `has_popup`/`expanded`, `pos_in_set`/`set_size`). muri's
`announcement()` is only the test oracle. What the AT actually voices differs per
platform — VoiceOver, NVDA, and Narrator each phrase "checked", "submenu",
"3 of 4" differently, and that is **expected**; §7 verifies *meaning conveyed*, not
exact wording.

### 4.3 OS accessibility settings — what 1.0 honors

| OS setting | 1.0 stance |
|---|---|
| **Reduced motion** | **Honored trivially / by construction.** muri has **no** open/close or flyout animation — popups appear instantly (that instant-open is a core design goal, [`00` §5](00-overview.md)). There is nothing to reduce. 1.0 promises: muri never animates a menu, so reduced-motion is always satisfied. If any animation is ever added, it must gate on the OS reduce-motion flag. |
| **Increase / high contrast** | **Partially, via `FollowSystem`.** Semantic colors resolve to live OS colors ([`01` §4](01-api-contract.md)), so a high-contrast *theme* is reflected when the OS palette changes. But muri does **not** yet query the explicit "increase contrast" accessibility flag (macOS `NSWorkspace.accessibilityDisplayShouldIncreaseContrast`, Windows high-contrast themes, GTK) to thicken separators/borders. **1.0-new:** query it and, when set, resolve to higher-contrast semantic values and draw a visible row-highlight border rather than a subtle fill. A custom `Theme` (`ThemeSource::Custom`) is the consumer's responsibility. |
| **Larger text / Dynamic Type** | **Not honored by default today — a 1.0 gap to close.** muri uses fixed menu font sizes (`Font::default()` = System 13.0). A user who sets a larger system text size gets a fixed-size menu. **1.0-new:** `MenuOptions` gains an opt-in to scale `row_font`/`header_font` by the OS text-size multiplier under `FollowSystem`; the two-pass layout already reflows to font metrics ([`10`](10-rendering-layout.md)), so honoring it is a font-size input, not a layout change. Must be opt-in so a consumer's fixed design isn't broken silently. |

These three are **AT-adjacent** but distinct from screen-reader support; §7's gate
is specifically the screen readers, but a reviewer should spot-check contrast and
text-size behavior on at least one platform.

---

## 5. Accelerators & mnemonics exposure to AT

Accelerator *text* and in-menu mnemonics are specified in
[`40` §accelerators](40-input-interaction.md) (display + handling; global hotkeys
are an explicit non-goal, [`00` §3](00-overview.md)). For accessibility:

- **Accelerators must be exposed as structured shortcut metadata, not just drawn
  text.** A right-aligned "⌘Q" segment is invisible-as-a-shortcut to an AT if it is
  only a glyph in the name. 1.0 sets the AccessKit node's keyboard-shortcut property
  (`Node::set_keyboard_shortcut`) so VO/NVDA/Narrator announce "Command-Q" as the
  item's shortcut, and does **not** fold the accelerator into `AxNode.name` (or it
  would be spoken twice). This means the accelerator is a *first-class field* the
  menu model must carry to the a11y layer — flagged for [`40`](40-input-interaction.md)
  to add an accelerator field the a11y bridge reads.
- **Mnemonics** (the underlined access character) map to type-ahead
  ([`40`](40-input-interaction.md)); since type-ahead and `accessible_name` share
  the same string, the AT already announces the letter it responds to. No extra
  exposure needed for 1.0, but if underline styling is added it should not pollute
  the accessible name.

---

## 6. What "correct out of the box" means (the default-build promise)

With `a11y` locked default-on (§0), a compat-migrated muda app
(`use muri::compat::muda as muda;` / `s/muda/muri/`) gets — **unconditionally, by
default, with no extra code and no feature flag to remember**:

- the popup exposed as a menu container, every row as a (checkable) menu item with a
  correct name/enabled/checked/set-position;
- keyboard, mouse, and AT-action focus in lockstep;
- submenu items announced with `has_popup`/`expanded`;
- the same activation firing both `on_click` and the global `MenuEvent` channel.

The **one** thing it does *not* get for free is a name for an **icon-only** row
(§1.4) — the consumer must set `accessibility_label`. Everything else is derived.

---

## 7. The all-screen-reader verification plan (the 1.0 GATE)

AccessKit fidelity is per-platform *and* per-AT; a compiled bridge is necessary but
not sufficient (decision #4). Screen readers **cannot be fully automated**, so the
gate is a **repeatable human pass** per AT, scripted below. Cross-ref
[`50-testing-verification.md`](50-testing-verification.md) for the CI matrix that
gates the *automatable* layers (pure `a11y` unit tests already dense in
`src/a11y.rs`; PNG snapshot tests; `--features a11y` build/clippy on each platform)
beneath this manual gate.

### 7.1 What "verified" means (per screen reader)

An AT is **verified** for 1.0 when a tester, following that AT's script below on a
real device with a canonical fixture menu (a header, a checked row, a plain row, a
separator, a disabled row, a one-level submenu, an icon-only row with
`accessibility_label`, and a leaf `Quit`), confirms **all** of:

1. **Enumerate.** Opening the popup announces a *menu*, and the AT can reach **every
   focusable row** by that AT's native navigation — and **cannot** land on the
   header, separator, or disabled row.
2. **Announce.** Each row speaks its **name**, its **role** (menu item / checkable /
   submenu), its **checked** state, and its **"N of M"** counting focusable siblings
   only. The icon-only row speaks its `accessibility_label`, not silence.
3. **Cross the flyout boundary (§3.3 items 1–3).** Opening a submenu moves the AT
   cursor **into** the flyout; closing returns it to the parent, announced
   collapsed. No items announced twice; no dead/empty window.
4. **Act.** Activating a leaf via the AT fires exactly one `MenuEvent` with the right
   id and dismisses; activating a submenu parent opens it.
5. **Track live focus.** Arrow-key and mouse-hover movement is followed by the AT
   cursor in lockstep.
6. **Survive a content swap.** A `set_menu` swap mid-navigation does not crash, wedge
   the AT, or announce a stale row as if current (§4.1).

Anything less than all six on an AT means that AT is **not verified** and 1.0 does
not ship (locked decision #4 is hard).

### 7.2 Per-AT manual scripts

**VoiceOver (macOS).**
- Launch fixture, `⌘`-click the tray icon to open (or trigger open).
- **Rotor / VO-cursor:** `VO`+arrow through items; confirm names, roles,
  "N of M"; confirm header/separator/disabled are skipped or announced non-actionable.
- **Rotor menu list:** open the rotor; confirm items enumerate.
- On the submenu item: confirm "submenu, collapsed"; `VO`+`→` / open → confirm the
  VO cursor **enters** the flyout and lands on the first child (the §3 crux).
- `VO`+`←` / Esc → confirm return to parent, "collapsed".
- `VO`+`Space` on a leaf → confirm one activation + dismiss.
- Toggle a checkable row → confirm "checked/unchecked".

**NVDA (Windows), browse + focus modes.**
- Open popup; in focus mode arrow through; confirm UIA `Menu`/`MenuItem` roles,
  states, "N of M", shortcut (⌘/Ctrl) announced from the shortcut field (§5), **not**
  duplicated in the name.
- Submenu: Right/open → confirm NVDA follows into the flyout HWND/fragment; Left/Esc
  → back. (This exercises the UIA `ControllerFor`/popup relation of Option B.)
- Enter on a leaf → one event + dismiss.

**Narrator (Windows), Scan mode.**
- Repeat the NVDA item enumeration in Scan mode; Narrator's UIA consumption differs
  from NVDA's, so it is a **separate** gate (decision #4 lists both). Confirm scan
  reaches every focusable row and the flyout children.

**Orca (Linux), flat review + focus tracking.**
- Only on the **styled `ContextMenu`** path (pointer-anchored) or the
  **native-menu fallback** ([`22`](22-platform-linux.md)) — there is *no* styled
  tray popup on Linux (decision #3). For the native fallback, AT-SPI is free via
  GTK/dbusmenu and the gate is "the native menu is accessible," which it is by
  construction. For the styled `ContextMenu`, run Orca flat-review over the AT-SPI
  tree from `accesskit_unix`; confirm enumeration, roles, states, and — if a flyout
  is used in a context menu — the §3 boundary crossing.

### 7.3 Making it repeatable

Full automation is impossible, but repeatability is not:
- **A fixed fixture menu** (above) and a **written checklist** (the six criteria)
  per AT, versioned in-repo under `docs/design/` or `tests/manual/`, so any tester
  runs the identical pass.
- **Structured-tree assertions in CI** as the automatable floor: `announcement()`
  and `tree_update()` unit tests already assert the *content* of what each node
  would say and that every node + focus is carried; extend these to the fixture so a
  tree regression is caught by `cargo test` before a human ever runs a screen reader.
  (This is why `announcement` is deterministic and pure — see
  [`50`](50-testing-verification.md).)
- **A recorded pass per release:** each 1.0 (and each subsequent breaking) release
  attaches a short capture / signed checklist per AT, so "verified" is auditable,
  not folklore.

---

## 8. Adversarial risk register (read before signing off)

Every item here is a place a11y can look correct and **fail with a real screen
reader**:

1. **Cross-OS-window flyout focus (§3)** — the #1 risk, **now multiplied by
   decision #8's N-level stack**: a perfect nested tree the SR cannot reach once a
   flyout takes focus, at *every* level. Mitigation: Option B (N adapters) + the
   §3.3 gate tested at multiple depths; Option C fallback per platform (its cost now
   largely absorbed by the decision-#6 vibrancy transparency rework).
2. **Borderless popup not receiving key input (§4.1)** — a non-activating surface
   that can't host first responder makes keyboard a11y a no-op. Tied to the
   `NSPanel` question in [`20`](20-platform-macos.md); unresolved on device.
3. **Empty-name nodes** — icon-only rows without `accessibility_label` (§1.4) read
   as silent items. §7.1 criterion 2 hunts for these.
4. **Node-id instability across `set_menu` rebuilds (§4.1)** — DFS ids change every
   swap; an AT mid-utterance can be desynchronized. Consider `MenuId`-keyed stable
   ids.
5. **Adapter attach-order regressions (§2.1)** — create-hidden-then-show and
   feed-events-first are live-loop invariants no unit test covers; a Windows/Linux
   backend that gets the order wrong silently exposes nothing.
6. **Accelerator double-speech (§5)** — folding "⌘Q" into the name *and* the
   shortcut field makes the AT say it twice; keep them separate.
7. **Per-AT fidelity divergence** — Narrator ≠ NVDA even on the same UIA tree; both
   are independent gates. AccessKit's macOS/UIA/AT-SPI backends do not expose the
   same relation set, so the §3 cross-window relation may work on one platform and
   not another — hence the per-platform Option-C fallback.
8. **Default-on is locked (§0)** — a11y ships on by default (decision #7); this is
   no longer an open question. The residual risk is only that a size-constrained
   embedder who *explicitly* drops the feature loses the semantics — a documented,
   opt-out consequence, not a silent default.
