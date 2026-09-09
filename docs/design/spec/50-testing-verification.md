# 50 — Testing & verification

Status: **foundational spec.** Defines how muri 1.0 is *proven* correct: the pure
unit suite, the headless render-snapshot suite, the muda-fidelity suite, the
all-screen-reader manual verification **gate**, the CI matrix, and the
performance-regression guards.

Cross-references: the engine internals under test —
[`10-rendering-layout.md`](10-rendering-layout.md) (scene drawer, layout, glyph
cache, perf guards), [`40-input-interaction.md`](40-input-interaction.md)
(keynav), [`30-accessibility.md`](30-accessibility.md) (the a11y tree and the SR
verification protocol this document gates on); the facade whose fidelity is
asserted here [`02-muda-compat.md`](02-muda-compat.md); threading / event ordering
[`03-threading-events-versioning.md`](03-threading-events-versioning.md); the
platform backends [`20`](20-platform-macos.md)/[`21`](21-platform-windows.md)/[`22`](22-platform-linux.md).

---

## 1. The testing philosophy: push everything into the pure core

The single architectural decision that makes muri testable is the one stated in
[`00-overview.md` §10](00-overview.md): **everything above the platform backends
is pure, portable, and runs without a window server.** The menu model, `Flex`/
`Align` layout, theme resolution, anchor and flyout placement, the keynav state
machine, and the a11y-tree build are all total functions over owned data with no
I/O. That is not an accident of the current code — it is the property the test
strategy depends on, and 1.0 must preserve it. Concretely:

- **Pure engine functions never fail** (they carry no error type;
  [`03` §7](03-threading-events-versioning.md)), so their tests are input →
  asserted-output with no fixtures, no mocks, no async, no display.
- **The one impure edge** — the CPU raster drawer (`RasterDrawer`,
  [`10`](10-rendering-layout.md)) — is still headless: it paints into an owned
  `tiny_skia::Pixmap` in memory, so even "rendering" tests need no GPU, no
  compositor, and no `winit` window.
- **Only the live surface** (`Tray::run`, `ContextMenu::open_at`, the AccessKit
  platform adapter attach) needs a real event loop and a display. That layer is
  the thin per-OS shim, and it is the *only* thing that cannot be fully covered by
  a deterministic automated test — which is exactly why the screen-reader gate
  (§5) is a human protocol, not a CI job.

The test pyramid, widest at the bottom:

```
                 ┌─────────────────────────────────────┐
   manual gate   │  all-SR verification (§5) — human,   │   release-gating,
   (per release) │  per platform, per screen reader     │   not in CI
                 ├─────────────────────────────────────┤
   thin, few     │  live-surface smoke (§6) — xvfb /     │   build+launch, no assert
                 │  runner, "does it open without panic" │   on pixels/AT
                 ├─────────────────────────────────────┤
   moderate      │  snapshot render (§3) + muda-fidelity │   headless, deterministic
                 │  (§4) — golden PNG + behavior parity  │
                 ├─────────────────────────────────────┤
   broad, fast   │  pure unit (§2): layout / theme /     │   the bulk of the suite;
                 │  anchor / flyout / keynav / a11y /    │   no display, no I/O
                 │  menu / style / facade routing        │
                 └─────────────────────────────────────┘
```

## 2. Pure-logic unit tests

These already form the densest part of the suite and are **Tier-1/Tier-4 stable**
([`03` §5](03-threading-events-versioning.md)). 1.0 does not rewrite them; it
extends them as the 1.0-new logic (type-ahead buffering, nested flyout, custom
dark-mode themes) lands. Coverage requirements, per module:

### 2.1 keynav state machine (`src/keynav.rs`)

`handle_key(menu, &mut MenuFocus, NavKey) -> NavAction` is a pure transition
function and must be exhaustively covered. The shipped suite already asserts:
down-from-nothing skips a section header to the first focusable row; down/up wrap
and skip separators and disabled/inert rows; Home/End jump to bounds; Right/Enter
on a submenu opens its flyout and focuses the first child; Right on a leaf is a
no-op; Activate on a leaf returns its `MenuId`; navigation within a flyout then
Left/Esc closes it and restores parent focus; Escape pops one level then
`CloseAll`; type-ahead jumps and wraps by first character and no-ops on a miss;
an empty menu yields no selection.

**1.0-new coverage the spec requires** (each is a decision flagged in
[`40`](40-input-interaction.md)):

- **Type-ahead buffering:** today it is single-char first-letter only. When 1.0
  adds a multi-char buffer + timeout, add tests for: buffered prefix match
  (`"set"` → "Settings"), the timeout resetting the buffer, and repeated same-key
  cycling among same-initial rows. The timeout is injected as a parameter (a
  `Duration` + a "now" value), never read from a real clock, so the test stays
  pure.
- **Nested flyout descent (N-level, locked decision #8):** the `keynav` flyout
  focus is a **stack** ([`40` §5](40-input-interaction.md)); tests must cover
  `Right`/`Activate`-on-submenu **descending** (pushing a level), `Left` **ascending**
  (popping one), and `Escape` **unwinding N levels** then `CloseAll`. Also assert a
  swapped-menu-underneath unwinds a stale sub-stack. The stack depth is a pure
  input, so these stay unit-testable off any loop.
- **Menu swapped underneath an open flyout:** the shipped code already handles a
  stale parent (`submenu_child` returns `None` → drop the flyout, `CloseFlyout`).
  Keep this test; it protects the `TrayHandle::set_menu`-while-open path
  ([`03` §2](03-threading-events-versioning.md)).

### 2.2 anchor / flip math (`src/anchor.rs`)

`place_popup(anchor, popup, work_area, edge, gap)` is covered for: Bottom opens
below and left-aligned; Bottom flips above on bottom spill; x clamps for a
right-edge icon; Top opens above; Right/Left flip on spill; an oversized popup
pins to the work-area origin. **1.0 must add** multi-monitor cases (a work area
whose origin is non-zero — a second display to the right or above the primary) so
the clamp is proven against a `work_area` that does not start at `(0,0)`. This is
the seam where a real bug hides: the shipped tests all use a `(0,0)`-origin work
area.

### 2.3 flyout placement + hover stack (`src/flyout.rs`)

`place_flyout` is covered for right-flush row-aligned placement, flip-left on
right spill, flip-left clamped into a narrow work area, and vertical clamp to top
and bottom. `next_flyout` (the hover-stack transition) is covered for open/switch
on a parent row, close on a non-parent row, keep-open over the flyout or the gap,
and keep-current when drifting outside. **1.0 must add** a hover-stack test that
proves keyboard focus (`MenuFocus`) and mouse hover drive the *same* single
open-flyout state — cross-module, asserting `handle_key(Right)` and
`next_flyout(ParentRow)` converge on the same `parent` index — since
[`40`](40-input-interaction.md) makes "one flyout state, two drivers" a contract.

### 2.4 a11y tree construction (`src/a11y.rs`)

`build_tree` is covered for: root is a `Menu` with one node per item; roles/states
map (`GroupLabel` header, `MenuItemCheckbox` for a checked row, `Separator`,
disabled row, `has_popup` + `expanded=Some(false)` submenu with child nodes);
`pos_in_set`/`set_size` count **only focusable siblings** (header, separator, and
disabled row excluded); ids unique and findable; `focused_id` tracks top-level and
flyout-child selection and falls back to the parent when a flyout has no child;
`locate` maps ids back to `(top, Option<child>)`; `set_expanded` flips state;
`announcement` renders name/state/role/position deterministically; and (under
`a11y`) the AccessKit `tree_update` carries every node plus the focus id.

**1.0-new coverage:** if [`01` §2](01-api-contract.md)'s optional
`accessibility_label` override lands, a test that an icon-only row announces its
label, not its (empty) segment text. And the **multi-window flyout a11y model**
([`30`](30-accessibility.md)) — whatever it resolves to (nested nodes in the
parent tree vs an independently-adapted flyout window) — must have a pure test on
the *tree shape* it produces, since that shape is the only part of the
cross-OS-window problem that is testable without a live screen reader.

### 2.5 layout, theme, menu, style

- **`layout::resolve_segments`** — the flush-right guarantee ([`01` §2](01-api-contract.md),
  muri's reason to exist). Tests: a `Flex::Grow` label + `Align::Right` value put
  the value's right edge at the content-box edge with **no reserved chevron
  column**; multiple `Grow` segments split leftover width; all-`Fixed` segments
  do not stretch; a segment wider than the content box does not produce negative
  positions. These are golden arithmetic and must be exact (`assert_eq!` on `f32`
  box positions, as the `usagio_menu` example already demonstrates).
- **`theme`** — `Theme::resolve(Color) -> Rgba` for every semantic role in light
  and dark; `ThemeSource::resolve_theme(system_is_dark)` picks light/dark via the
  injected bool (never a live OS query); and the **1.0-new** `Theme::native()`
  color fields resolve to the live-OS semantic roles ([`01` §4](01-api-contract.md)).
  The known **custom-theme dark-mode gap** ([`01` §4](01-api-contract.md)) gets a
  test the moment `CustomFollowSystem`/`Theme::variant` lands.
- **`menu`/`style`** — `interactive_count` / `Item::is_interactive` are the single
  source of truth for keynav and a11y, so they get direct tests: a separator,
  section header, disabled row, and inert `MenuId::none()` row are all
  non-interactive; a plain enabled row and a submenu are interactive.
  `accessible_name()` joins segment text. `Color::is_literal`, `Weight::ot_weight`,
  `Font::default` == System/13.0/Regular.

### 2.6 facade routing (1.0-new, `src/compat/`)

The facade's routing decision ([`02` §2](02-muda-compat.md)) is itself pure and
must be unit-tested **without** ever showing a surface:

- calling `init_for_nsapp`/`init_for_hwnd`/`init_for_gtk_window` on the compat
  `Menu` tags it **native-passthrough** and lazily materializes a native OS menu
  (`NSMenu`/`HMENU`/GTK, built by muri itself — decision #5); calling
  `show_context_menu_for_*` or handing it to a `TrayIconBuilder` routes to a muri
  **custom** surface.
- a facade `Menu` translates each muda item type to the right muri `Item`
  (`Submenu` → `Item::Submenu`, `CheckMenuItem` → `Row.checked`, `IconMenuItem` →
  `Row.leading`, `PredefinedMenuItem::separator` → `Item::Separator`, other
  `PredefinedMenuItem`s → a visible **disabled** row per the honesty rule in
  [`02` §4.6](02-muda-compat.md)).
- `menu.items()` round-trips back to `Vec<MenuItemKind>` from the muri tree.

These are the cheapest place to catch a facade regression and run on every
platform (the tree translation is `cfg`-independent).

## 3. Snapshot rendering tests

### 3.1 What exists today (honest baseline)

`tests/snapshot.rs` paints a representative menu straight to an in-memory
`tiny_skia::Pixmap` via `render_menu` + `RasterDrawer`, then `encode_png`s it to
`target/`. It renders dark and light variants and a composited parent+flyout
panel (using `place_flyout` exactly as the live macOS backend positions its second
window). Its current assertions are **structural, not pixel-exact**:

- the popup is a sensible size (`width >= 200`, `height > 150`);
- the device pixmap tracks `logical_size * scale`;
- **non-blank**: more than half the pixels are painted (the panel fill);
- the interactive-row count is exact (5 in the demo menu; ≥3 in the flyout);
- the flyout opens `FlyoutSide::Right`, flush against the parent's edge.

This is a **smoke + structure** test. It proves the drawer runs headless and the
scene has the right shape, but it does **not** catch a visual regression (a wrong
color, a shifted glyph, a broken corner radius) because nothing compares against a
known-good image. 1.0 must close that gap.

### 3.2 The 1.0 golden-image snapshot suite

1.0 upgrades `tests/snapshot.rs` to true golden snapshots: render → encode PNG →
**compare against a committed reference image**, failing on any pixel difference
beyond a tiny tolerance. Design:

- **Golden images live in the repo** (`tests/snapshots/<name>.png`), one per
  (menu-fixture × theme × state × scale) cell (§3.4). A diff harness (a small
  helper, or the `image` crate for decode + a per-channel max-delta compare, or an
  established crate such as `insta`'s binary-snapshot mode) asserts equality.
- **Tolerance is near-zero but non-zero.** Anti-aliased glyph edges and the
  rounded-rect corners can differ by ±1 in the least-significant bits across
  `tiny-skia`/`swash` patch versions even with identical inputs. The compare uses
  a **max per-channel delta of ~2/255 over ≤0.1% of pixels**; a larger or wider
  difference fails. The threshold is a named constant with a comment, not a magic
  number, so a reviewer can see the honesty budget.
- **Updating goldens is explicit.** An env-gated update mode
  (`MURI_UPDATE_SNAPSHOTS=1`) rewrites the references; a normal `cargo test` never
  writes them. A golden change is reviewed as a diff of the PNG in the PR.

### 3.3 Determinism — the hard part

A golden snapshot is only useful if the *same inputs* produce the *same pixels* on
every developer machine and every CI runner. muri's renderer has three sources of
non-determinism, and 1.0 must pin all three:

1. **Font selection (the big one).** `RasterDrawer::new` calls `resolve_ui_family`
   ([`10`](10-rendering-layout.md), `src/render/mod.rs`), which walks a candidate
   list (`.SF NS`, `SF Pro Text`, `Segoe UI`, `Helvetica Neue`, `DejaVu Sans`,
   `Liberation Sans`, …) and picks **whatever the OS has installed** that shapes
   both regular and bold in one family. That is correct for *looking native live*,
   but it means the snapshot font is San Francisco on a macOS runner, Segoe UI on
   Windows, and DejaVu/Liberation on Ubuntu — three different sets of glyph
   outlines and metrics, so **one golden can never match on all three.** The fix
   for the snapshot suite: build the `RasterDrawer` with a **bundled, repo-vendored
   font** (a permissively-licensed UI face, e.g. a variable or regular+bold pair
   of DejaVu Sans or an SIL-licensed font committed under `tests/fonts/`), loaded
   into a **fresh `FontSystem` with system fonts disabled**, so glyph outlines are
   byte-identical everywhere. This requires a **1.0-new seam**: a
   `RasterDrawer::with_fonts(scale, FontSource)` / `RasterDrawer::new_headless`
   constructor (or a `FontSystem` injection point) that the snapshot test uses and
   the live backend does not. The live backend keeps `resolve_ui_family` for the
   native look; the snapshot suite pins the vendored font. This split must be
   documented as intentional: **snapshots verify layout/drawing correctness with a
   fixed font; they deliberately do not verify the live system-font look** (that is
   what the manual passes and the demo examples cover).
2. **DPI / device scale.** `RasterDrawer::new(scale)` is explicit and already
   deterministic; the suite pins scales it renders at (§3.4). No reliance on a
   runner's real display DPI (there is none — it is offscreen).
3. **Glyph rasterizer / library versions.** `swash` (via `cosmic-text`) and
   `tiny-skia` are the rasterizers; a patch bump can shift sub-pixel coverage.
   `Cargo.lock` is **committed** (the repo already commits it) and CI pins the
   MSRV toolchain (1.86), so the rasterizer is pinned. When a dep bump does move
   pixels within tolerance, the golden is regenerated in that PR; a move beyond
   tolerance is a real review signal, not noise.

### 3.4 Coverage matrix (per-theme / per-state)

The snapshot cells 1.0 must cover, each a committed golden:

| Fixture | Themes | States | Scales |
|---|---|---|---|
| The `usagio`-style provider menu (groups, flush-right colored values, checkmark, logos, version tail) | `light`, `dark`, `native()` (with the vendored font it resolves to fixed colors) | no highlight; row highlighted | 1.0, 2.0 |
| Parent + open flyout composite | `dark` | submenu-parent highlighted, flyout row-aligned | 2.0 |
| A menu exercising every `Item` kind | `light` | default | 2.0 |
| Facade `PredefinedMenuItem`s in a custom surface (separator, and an emulated item as a **disabled** row) | `native()` | default | 2.0 |
| RTL / bidi sample (once [`10`](10-rendering-layout.md) resolves `StyleRun` UTF-16 offsets under reordering) | `light` | default | 2.0 |

The flush-right guarantee ([`01` §2](01-api-contract.md)) is the one that most
needs a golden: a regression there (a reappearing reserved chevron column) is
muri's whole reason to exist breaking, and only a pixel/layout snapshot catches
it. The `layout::resolve_segments` unit test (§2.5) guards the arithmetic; the
snapshot guards that the drawer honours it.

**Vibrancy is out of snapshot scope (verified manually).** The golden snapshots
render the raster layer to a `Pixmap`; the `Theme::native()` vibrancy backdrop
(macOS `NSVisualEffectView` / Windows acrylic, decision #6) is a *compositor* effect
behind a transparent surface and cannot be captured headlessly. Snapshots therefore
render the alpha-preserving raster **over a fixed test background**, verifying the
drawer's own output (reduced-alpha panel fill, corner clip); the *live* vibrancy is
verified by the manual passes and the runnable `examples/` (§6.4), the same way the
live system-font look is.

### 3.5 Running in CI without a display

The snapshot suite is **headless by construction** — it paints to a `Pixmap`, no
`winit` window, no `softbuffer` surface, no compositor. It therefore runs on a
bare CI runner on **all three OSes with no X server, no xvfb, no virtual
framebuffer** (§6 distinguishes this from the live-surface smoke tests, which do
need a display on Linux). Because the font is vendored (§3.3), the *same* goldens
pass on macOS, Windows, and Linux runners — which doubles as a portability proof
of the shared scene drawer.

## 4. muda-fidelity tests

The facade's promise ([`02` §1](02-muda-compat.md)) is behavioral, so its tests
assert **the same observable behavior muda produces**, not just that the facade
compiles. These are the 1.0 "test gates" already called out across docs 02/03,
gathered here:

- **`MenuId` allocation parity** ([`02` §3](02-muda-compat.md)): a fixture app
  that never sets explicit ids must observe **identical** auto-generated ids under
  real muda and under the facade. The facade replicates muda's process-global
  atomic counter semantics. Test: build the same sequence of items through both,
  assert id strings match element-for-element. (Where muda is not a dev-dependency
  we can compile against, the test pins the *documented* counter algorithm and a
  captured reference list from a real muda run.)
- **`MenuEvent` emission + ordering** ([`03` §3](03-threading-events-versioning.md)):
  activating a row fires the `on_click` closure **first, synchronously on the UI
  thread**, then sends `MenuEvent { id }` on the process-global channel; inert
  `MenuId::none()` rows fire neither. Test with an in-process channel and a shared
  counter (the pattern the shipped `lib.rs` dispatch tests use), asserting both
  sinks see the id and the closure ran before the channel recv.
- **Unified emission, one source** ([`03` §3](03-threading-events-versioning.md),
  decision #5): a fixture with **one native menu-bar item and one muri tray item**
  must deliver **both** activations, **in order**, on the single
  `muri::MenuEvent::receiver()`. Because muri now emits events for *all* surfaces it
  draws (custom surfaces and the native menu bar it installs) through one `dispatch`
  — no wrapped-muda channel, no forwarder thread — the test asserts ordering under
  rapid interleaving and that there is exactly one emission per activation (no
  drop, no double-emit). The old two-channel-bridge hazard is gone by construction.
- **`PredefinedMenuItem` honesty** ([`02` §4.6](02-muda-compat.md)): every
  inemulable predefined item renders as a **visible, disabled row** in a custom
  surface — never silently dropped. Test: build a menu with `services()` and
  `about()` (off-macOS) in a custom surface, assert the tree has a disabled row
  per item with the expected label, and the a11y tree marks it `!enabled`.
- **Error superset** ([`02` §7](02-muda-compat.md)): the facade's `Error` exposes
  the muda variants a consumer might `match` on, so a migrated `match` compiles
  even for variants muri never returns from a custom surface. Test: a `match`
  covering muda-style arms type-checks (a compile test / trybuild-style check).
- **`tray-icon` subsumption** ([`02` §8](02-muda-compat.md)): `TrayIconEvent` is
  emitted on macOS/Windows and **not on Linux** (matching tray-icon's own
  contract) — the facade must not fabricate Linux click events. Test the
  Linux-`cfg` path returns no event / `Unsupported` where tray-icon does.

## 5. The all-screen-reader verification plan — a 1.0 GATE

Locked decision #4 ([`00` §4](00-overview.md)) makes "AccessKit is wired" *not*
sufficient: 1.0 requires a **human pass on every target screen reader**, because
AccessKit's fidelity varies per platform and per AT and a `tiny-skia` pixmap is an
opaque rectangle to an AT. This section defines what "verified" means, the
repeatable scripts, the evidence, and how it gates a release. The a11y *model*
being verified — roles, names, set-position, the multi-window-flyout problem — is
specified in [`30-accessibility.md`](30-accessibility.md); this section is the
verification **protocol** that gates on it.

### 5.1 Why it cannot be a CI job

Screen readers are proprietary, stateful, audio-first programs that run against a
live desktop session and a live focus tree. There is no headless VoiceOver, and
NVDA/Narrator/Orca automation is brittle and does not certify what a *user* hears.
So the gate is a scripted **human-in-the-loop** procedure, run per release
candidate, with recorded evidence. CI's role is only to prove the AccessKit
`TreeUpdate` is *built* correctly (§2.4) and the adapter *compiles and attaches*
(§6); the spoken result is verified by a person.

### 5.2 The repeatable script (identical across all four AT)

The same menu fixture and the same ordered steps are run under each AT, so results
are comparable. Fixture: the provider menu with a checked active account, a
disabled row, a section header, a separator, and a submenu (the `a11y.rs` test
menu, shown live). Steps and the **expected announced result** for each:

1. **Open the popup** (tray click / `open`). AT announces entering a **menu** with
   its item count.
2. **Arrow down through every row.** Each focusable row is announced with: its
   **name** (concatenated segment text), its **role** (menu item / checkable menu
   item), its **checked** state where present, its **position** ("3 of 7" over
   *focusable* siblings only — the header, separator, and disabled row are **not**
   counted), and "dimmed" for the disabled row. The header is announced as a
   heading/label and is skipped by focus.
3. **Land on the submenu row.** AT announces "submenu / has popup, collapsed".
4. **Open the flyout** (Right/Enter). AT announces "expanded" and moves focus into
   the flyout, announcing its first child — **across the OS-window boundary** (the
   flyout is a separate window; this is the known-hard case,
   [`30`](30-accessibility.md) / [`00` §7](00-overview.md)). Verifier confirms
   focus actually followed into the flyout and the child is spoken.
5. **Close the flyout** (Left/Esc). Focus returns to and re-announces the parent
   row.
6. **Activate a leaf** (Enter). The row fires (observable app effect) and the menu
   dismisses; AT announces the dismissal / focus return to the host.
7. **AT-initiated action.** Use the AT's own "activate"/"press" command (VoiceOver
   `VO-Space`, NVDA/Narrator Enter via the AT, Orca flat-review activate) on a row
   and confirm it drives the **live** popup (moves highlight / opens flyout /
   dispatches) — proving the action-request round-trip ([`30`](30-accessibility.md)),
   not just read-only exposure.

### 5.3 Per-AT specifics

| AT | Platform | Bridge | Notable focus |
|---|---|---|---|
| **VoiceOver** | macOS | `accesskit_macos` → NSAccessibility (shipped adapter attach: create-hidden-then-show, action-request → live popup, [`20`](20-platform-macos.md)) | The non-activating panel must still host AT focus; the flyout-window boundary (step 4). |
| **NVDA** | Windows | `accesskit_windows` → UIA | Browse vs focus mode; `WS_EX_NOACTIVATE` popup must still be reachable; verify UIA `Menu`/`MenuItem` control types. |
| **Narrator** | Windows | same UIA tree | Narrator interprets UIA differently from NVDA — both are required by decision #4, so both are separate cells even though they share the bridge. |
| **Orca** | Linux | `accesskit_unix` → AT-SPI | On the **native-menu fallback** path AT-SPI is free (the OS draws the menu); on the **pointer-anchored `ContextMenu`** styled path the muri tree must be exposed via AT-SPI. Both paths are verified ([`22`](22-platform-linux.md)). |

### 5.4 What counts as evidence

For each (AT × platform × release-candidate) cell, the verifier records:

- a **screen recording with the AT audio** (or the AT's speech-log/braille-log
  text export) covering all seven steps;
- a **filled checklist** mapping each step to pass/fail and the *actual* spoken
  string vs the expected one from §5.2;
- the muri **commit hash**, AT version, and OS version.

Evidence is stored with the release (a `docs/verification/<version>/` folder or the
release notes). A step is **pass** only if the actual announcement conveys name +
role + state + position equivalently to §5.2 (exact phrasing is the AT's, not
muri's — muri controls the structured fields, per `announcement()` in `a11y.rs`).

### 5.5 How it gates the release

- **1.0 cannot ship** until every cell in the §5.3 table is **pass** on a single
  release candidate build. A fail is a release blocker, not a known issue.
- The **hardest single item** is step 4 (focus crossing into the flyout window).
  If [`30`](30-accessibility.md) cannot make a given AT follow that boundary, the
  fallback is the model change 30 specifies (e.g. drawing the flyout within the
  parent window, or a single-window menu with an inline expanded region) — but the
  **gate does not lower**: "the flyout is unreachable under Orca" is a blocker, not
  a footnote.
- Per the milestone ladder ([`00` §8](00-overview.md)), the gate is applied
  incrementally: **VoiceOver** gates M2 (macOS), **NVDA + Narrator** gate M4
  (Windows), **Orca** gates M5 (Linux); 1.0 requires all four green together on
  the final RC.
- **Regression policy:** any change touching `a11y.rs`, the AccessKit adapter, or
  a platform backend's focus/window handling **re-runs the affected AT cells**
  before the next release. The pure a11y-tree tests (§2.4) run in CI on every
  commit as the early-warning layer.

## 6. CI matrix

### 6.1 Today

`.github/workflows/ci.yml` has one job: `fmt + clippy + build` on `macos-latest`,
using `dtolnay/rust-toolchain@stable`, running `cargo fmt --all --check`,
`cargo clippy --all-targets --all-features -- -D warnings`,
`cargo build --all-targets`, and `cargo build --examples`. It does **not** run
tests, does not pin the MSRV toolchain, and does not cover Windows or Linux.

### 6.2 The 1.0 matrix

| Job (runner) | fmt | clippy `-D warnings` | build (all-targets, examples) | test (unit + snapshot + fidelity) | doc | display needed |
|---|---|---|---|---|---|---|
| **macOS** (`macos-latest`) | ✓ | ✓ (`--all-features`, incl. `a11y`) | ✓ | ✓ | ✓ | no (snapshot/unit are headless) |
| **Windows** (`windows-latest`) | — | ✓ | ✓ | ✓ | — | no |
| **Linux** (`ubuntu-latest`) | ✓ (canonical fmt) | ✓ | ✓ | ✓ | ✓ | only for live-surface smoke (§6.4) |

Notes:

- **Toolchain pin:** every job pins **Rust 1.86** (the `Cargo.toml` MSRV), *not*
  `stable`. The repo's own precommit note (and the global instructions) warn that
  a newer default toolchain's clippy silently accepts lints 1.86 rejects — CI must
  catch that drift, so it runs on the pinned MSRV. A second, non-gating job may run
  `stable`/`beta` as an early warning of future breakage.
- **`--all-features`** ensures the `a11y` and `muda-compat` features compile and
  their tests run; note [`03` §4](03-threading-events-versioning.md) flips `a11y`
  **on by default** at 1.0, so the default-feature build also exercises AccessKit.
- **`cargo test`** is the change from today: the unit suite (§2), the golden
  snapshot suite (§3, headless), and the muda-fidelity suite (§4) all run on all
  three OSes. The snapshot goldens are identical across OSes because the font is
  vendored (§3.3) — a job failing only on one OS is a real portability bug.
- **`cargo doc`** with `-D warnings` (the crate is `#![deny(missing_docs)]`) guards
  the public API docs on the doc-capable runners.
- Platform backends are `cfg`-selected, not feature-gated
  ([`03` §4](03-threading-events-versioning.md)), so each runner naturally compiles
  and tests only its own backend shim; the shared core is tested on all three.

### 6.3 Headless display strategy — the honest version

The snapshot and unit suites need **no display on any OS** (§3.5) — this is the
payoff of the pure-core architecture. The only tests that need a display are the
**live-surface smoke tests** (§6.4), and only on Linux:

- **macOS/Windows:** a live `winit`/`softbuffer` window can be created in a CI
  runner's session; smoke tests can run directly (though they are still limited —
  see §6.4).
- **Linux:** a real popup needs an X/Wayland server. CI runs those under **`xvfb`**
  (`xvfb-run cargo test --features …`) to provide a virtual framebuffer. GTK deps
  for the `init_for_gtk_window` passthrough and the SNI/dbusmenu fallback
  ([`22`](22-platform-linux.md)) must be `apt`-installed in the job.

### 6.4 What CI **cannot** verify (stated plainly)

- **No screen-reader assertion.** CI proves the AccessKit `TreeUpdate` is built
  (§2.4) and, on macOS/Linux-under-xvfb, that the adapter **attaches without
  panicking** — it does **not** prove VoiceOver/NVDA/Narrator/Orca *announce*
  anything. That is §5's manual gate. The CI a11y smoke test is a liveness check,
  not a verification.
- **No pixel verification of the live system-font look.** Goldens use the vendored
  font (§3.3); the native-font appearance is verified by the manual passes and the
  runnable `examples/` (`demo_tray`, `usagio_menu`), which a human runs on a real
  desktop.
- **Live-surface smoke** is intentionally shallow: "does `Tray::run` /
  `ContextMenu::open_at` open and dismiss without panicking under xvfb", with a
  timeout that closes the loop. It asserts nothing about pixels or AT — those are
  §3 and §5. Its value is catching a backend that panics on window creation.

## 7. Performance / regression guards

The 1.0 performance contract ([`00` §5](00-overview.md): "opens with no
perceptible delay", CPU raster, no GPU warm-up) is specified in
[`10-rendering-layout.md`](10-rendering-layout.md); this section states how it is
**guarded** so a refactor cannot silently regress it. Where 10 defines the budgets,
50 defines the tests:

- **Glyph-cache effectiveness.** The drawer holds a `SwashCache`
  (`src/render/mod.rs`); re-rendering the *same* menu (the ~0.75s usagio rebuild
  tick) must not re-rasterize every glyph. A guard test renders a menu twice
  against an instrumented drawer and asserts the second pass produces **zero new
  swash rasterizations** (a counter on cache misses), proving the cache key is
  stable across identical rebuilds.
- **Frame allocation guard.** A steady-state re-render should not grow unbounded or
  allocate a fresh `FontSystem`/cache per frame. A guard asserts that a re-render
  reuses the drawer's `RefCell<FontSystem>`/`RefCell<SwashCache>` (they are owned,
  not recreated) and that `begin_frame` is the only large allocation (one `Pixmap`
  sized to the frame). If 10 specifies a hard per-frame allocation budget, this
  becomes a counted-allocation test (a custom global allocator wrapper in the test
  harness).
- **Open latency (indicative, non-gating).** A `cargo bench` (or a timed test with
  a generous ceiling) measures `render_menu` wall-time for a representative menu at
  scale 2.0 and flags a gross regression (e.g. >2× the recorded baseline). Kept
  non-gating in CI (runner variance) but tracked, since "instant popup" is a
  headline promise.
- **Layout stability.** `resolve_segments` is pure and already unit-tested (§2.5);
  its golden arithmetic tests *are* the regression guard for the flush-right
  layout — the feature muri exists to provide — so they are treated as
  release-gating, not optional.

These guards are cross-referenced from [`10`](10-rendering-layout.md) where the
budgets they enforce are defined; if a budget is not yet specced there, the guard
ships as a *ratchet* (assert "no worse than the recorded baseline") rather than an
absolute number, so it still catches regressions without inventing a threshold.
