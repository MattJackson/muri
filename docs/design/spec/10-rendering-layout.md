# 10 — Rendering & layout: the shared scene drawer

Status: **section spec.** Grounded in the shipped `src/render/mod.rs`,
`src/render/paint.rs`, `src/layout.rs`, `src/style.rs`, `src/theme.rs`,
`src/anchor.rs`, `src/flyout.rs`, and `src/tray/macos.rs`. Where a type or path is
**shipped** it is described as-is; where 1.0 must change or add something it is
marked **1.0-new** / **1.0-change** with the reason, and a **⚠ owner-confirm** note
flags a provisional decision the owner is still being asked to confirm.

Cross-references: the data model and the flush-right contract
[`01-api-contract.md`](01-api-contract.md); theming and `Theme::native()`
[`01-api-contract.md` §4](01-api-contract.md); the muda facade fidelity register
[`02-muda-compat.md` §9](02-muda-compat.md); input, hit-testing consumers, and the
hover stack [`40-input-interaction.md`](40-input-interaction.md); the a11y tree
that the same `Menu` also drives [`30-accessibility.md`](30-accessibility.md);
platform anchoring/dismiss shims [`20-platform-macos.md`](20-platform-macos.md),
[`21-platform-windows.md`](21-platform-windows.md),
[`22-platform-linux.md`](22-platform-linux.md).

---

## 1. The rendering model in one paragraph

muri holds no retained render graph. The **`Menu` tree is the scene** (doc 01 §2):
every frame, [`render_menu`](../../../src/render/paint.rs) walks the declarative
tree, measures text through the drawer, resolves the `Flex`/`Align` layout, and
emits primitive draw calls (`fill_round_rect`, `draw_separator`, `draw_text`,
`draw_image`) into a `tiny-skia` `Pixmap`; the backend blits that pixmap to the
window's `softbuffer` surface. There is exactly **one** `SceneDrawer`
implementation, `RasterDrawer`, used identically on every OS; all per-OS code lives
in the anchor/dismiss shims (doc 20–22), never in drawing. This is the invariant
that keeps three platforms tractable: **everything in this document is portable and
unit/snapshot-testable without a window server.**

## 2. The `SceneDrawer` boundary (shipped)

`render_menu` is generic over `SceneDrawer`, so the layout/paint pass is written
and tested against the trait, independent of any window (`RasterDrawer` is the only
impl; test doubles are trivial). The full trait:

```rust
pub trait SceneDrawer {
    fn begin_frame(&mut self, size: LogicalSize);                 // clear to transparent
    fn fill_round_rect(&mut self, rect: LogicalRect, corner_radius: f32, color: Rgba);
    fn draw_separator(&mut self, rect: LogicalRect, color: Rgba); // 1px hairline
    fn measure_text(&self, text: &str, font: &Font) -> f32;       // intrinsic width, logical px
    fn line_height(&self, font: &Font) -> f32;
    fn draw_text(&mut self, run: &TextRun<'_>);
    fn draw_image(&mut self, rgba: &[u8], src_w: u32, src_h: u32, dest: LogicalRect);
}

pub struct TextRun<'a> {
    pub text: &'a str, pub origin: LogicalPoint,
    pub font: &'a Font, pub color: Rgba, pub weight: Weight,
}
```

**All coordinates in the trait are logical pixels; the implementation owns
device-scale conversion** (§6). The method set is deliberately tiny — a menu is
rounded rects, hairlines, text runs, and bitmaps — which is what makes a second
drawer (e.g. a future GPU or PDF target) a bounded task and keeps the paint code
honest about what a menu actually needs.

**Contract note (1.0):** `measure_text`/`line_height` take `&self` while the draw
methods take `&mut self`. `RasterDrawer` satisfies the `&self` measurement methods
through `RefCell<FontSystem>` interior mutability (§7). 1.0 keeps this split so the
two-pass layout can measure before it begins a frame.

## 3. `Menu` → drawn rows: the two-pass `render_menu` (shipped)

`render_menu(drawer, menu, theme, opts, highlight) -> LaidMenu`. The pass is:

**Column decisions (once).** A **leading column** (`LEADING_COLUMN = 20.0`) is
reserved iff *any* item has a leading icon or a `checked` state (`needs_leading`).
A **trailing column** (`TRAILING_COLUMN = 14.0`) is reserved iff *any* item is an
`Item::Submenu` (`is_submenu`). Both are all-or-nothing for the whole menu so every
row's text band starts and ends at the same x — column alignment across rows.

**Pass 1 — intrinsic width & heights.**
- For each item's row, `row_intrinsic` = Σ segment `measure_text` widths +
  `theme.column_gap` between segments. Section headers measure in `header_font`,
  others in `row_font`.
- `desired = pad.left + leading_w + max_content + trailing_w + pad.right`, clamped
  to `[opts.min_width ?? 200, opts.max_width ?? 380]` → the popup **width**.
- The **content band** is `band_x = pad.left + leading_w`,
  `band_w = width - pad.right - trailing_w - band_x` (≥ 1). This is the width
  handed to `layout::resolve_segments` as the available band width.
- Row heights: `Separator` → `SEPARATOR_HEIGHT = 11.0`; header/row/submenu →
  `theme.row_height` (default 22.0), a row may raise it via `min_height`. Summed +
  `pad.top + pad.bottom` → the popup **height**.

**Pass 2 — draw.**
1. `begin_frame(size)` (clears to transparent), then `fill_round_rect` of the whole
   panel with `theme.corner_radius` in the resolved background color. **Under
   `Theme::native()` (vibrancy, decision #6 — §11): the panel background fill is
   drawn at *reduced alpha* or skipped entirely** so the native
   `NSVisualEffectView` / acrylic backdrop shows through the raster; only the
   opaque foreground (text, icons, highlight, separators) is painted over the
   translucent material. A `Custom`/`Light`/`Dark` theme still fills an opaque
   panel body as before.
2. Per item: separator → hairline at row mid-height; section header → segment band
   in `secondary_label`; row/submenu → optional highlight fill, then
   `draw_row_content`.
3. The highlight (when `highlight == Some(i)` and the row is interactive) is a
   `fill_round_rect` (radius 5.0) in `theme.accent`, inset from the panel edges;
   highlighted text is forced to `Rgba::WHITE`.

`draw_row_content` draws, in order: the leading icon/checkmark column
(`draw_image` for PNG, a centered `\u{2713}` glyph for `Icon::Checkmark` or
`checked == Some(true)`), the segment band (§4), and — for a submenu — a trailing
`\u{203A}` (›) chevron centered in the trailing column.

The result:

```rust
pub struct LaidRow  { pub index: usize, pub rect: LogicalRect, pub id: MenuId, pub interactive: bool }
pub struct LaidMenu { pub size: LogicalSize, pub rows: Vec<LaidRow> }
impl LaidMenu {
    pub fn hit(&self, point: LogicalPoint) -> Option<usize>;     // first interactive row containing point
    pub fn id_at(&self, point: LogicalPoint) -> Option<MenuId>;
}
```

`LaidMenu` is both the presented pixmap's companion **hit-test map** (§5) and the
size the window is created at. **One `LaidMenu` per surface window**, and with
N-level nested submenus (decision #8) there is a *stack* of them: the parent popup
has its own, and **each open flyout level has its own independent `LaidMenu`** in
its own window (doc 20 `Flyout.laid`, now a `Vec<Flyout>`). The stack grows and
shrinks as the user descends/ascends submenus; every level is measured and laid
out by the same `render_menu` pass.

## 4. The flush-alignment / no-chevron guarantee (muri's reason to exist)

The layout math is `layout::resolve_segments(metrics, band_w)` (pure, in
`src/layout.rs`): every segment gets ≥ its intrinsic width; leftover
`band_w - Σintrinsic` (clamped ≥ 0) is split evenly among `Flex::Grow` segments;
within its resolved box a segment's text is placed by `Align` (`Right` ⇒
`text_x = box.x + slack`). A `Flex::Grow` label followed by an `Align::Right`
`Flex::Fixed` value therefore yields **a value flush against the right edge of the
content band**.

**Where the "no reserved chevron column" guarantee is real, precisely.** The
trailing column is reserved **only when the menu actually contains a submenu**
(`menu.items.iter().any(is_submenu)`). A menu with no submenus has
`trailing_w = 0`, so `band_w` runs to `width - pad.right` and a right-aligned value
sits flush at the panel's inner edge — the thing native menus cannot do because
`NSMenu`/`HMENU`/GTK always reserve a submenu-arrow gutter. **This is a contractual
guarantee** (doc 01 §2), and it is asserted by `layout.rs`'s
`grow_label_then_right_value_is_flush_right` test and the `render` snapshot tests.

**Honest caveat (must be documented, not silently true):** when a menu *does*
contain a submenu, muri reserves `TRAILING_COLUMN = 14.0` for **every** row so
columns line up, and draws the chevron only on submenu rows. So "no chevron column"
holds for submenu-free menus (the common tray/context case) and degrades to "a
14px trailing gutter shared by all rows" once any submenu exists. 1.0 documents
this exactly rather than overclaiming. **1.0-new option (⚠ owner-confirm):** reserve
the trailing column *per row* (only submenu rows) instead of per menu, so a mixed
menu keeps non-submenu values flush; this complicates cross-row x-alignment and is
a look decision for the owner.

## 5. Hit-testing (shipped)

`LaidMenu::hit` / `id_at` do a linear scan of `rows` (menus are small; linear is
correct and cheap) returning the first **interactive** row whose `rect` contains
the window-relative logical point. The backend converts a physical cursor position
to logical via the window's `scale_factor` (`App::to_logical`) before querying.

Invariants the rest of the system leans on:
- `rows` holds one entry per top-level `Item::Row`/`Item::Submenu` only (separators
  and section headers are not in the map — they are never hit-testable).
- `LaidRow.index` indexes back into `Menu::items`, so a hit maps directly to the
  same index `keynav` (`MenuFocus.top`) and `a11y` (`AxNode.item_index`) use —
  **one index space shared by rendering, input, and accessibility.** This is why
  the mouse `hovered: Option<usize>` and the keyboard `focus.top` are the *same*
  field on the backend (doc 40 §4).
- Each flyout level's rows are hit-tested against *that level's* own `LaidMenu`,
  in that window's coordinate space (doc 20 `on_flyout_cursor`). With N-level
  nesting (decision #8) the cursor is tested against whichever level's window it is
  over; the shared item-index space holds within each level's `Menu`.

## 6. HiDPI, device scale, and fractional scaling

`RasterDrawer::new(scale)` fixes a device-scale factor (device px per logical px).
`begin_frame` allocates the backing pixmap at
`round(logical.width * scale) × round(logical.height * scale)`; every primitive
multiplies logical coordinates by `scale` at draw time. `measure_text`/`line_height`
report **logical** widths (they shape at `size * 1.0`), while `draw_text` shapes at
`size * scale` device pixels so glyphs are crisp at native resolution. Font metrics
are `Metrics::new(px, px * 1.3)` (line height = 1.3 × size).

**Backend wiring (shipped, macOS):** the popup is created with a probe drawer at
`2.0` to size the window, then the real drawer is rebuilt at the live
`window.scale_factor()` before first paint; `softbuffer` is resized to the drawer's
`device_size()` each frame.

**Fractional scaling (1.0 concerns to nail):**
- `scale` is `f32`, so 1.25 / 1.5 / 1.75 (common on Windows, and on some Linux) are
  supported by construction; the `round()` in `begin_frame` means the device pixmap
  may be ±1px off `logical * scale`. The window's logical inner size and the
  pixmap's device size must agree after rounding or the blit smears — **1.0 must fix
  the window logical size to the same rounding `begin_frame` uses** (feed
  `LaidMenu.size` through, not a separately rounded value).
- `draw_separator` snaps y to a device-pixel grid (`(y*s).round()`) and forces
  ≥ 1px height so hairlines stay visible at fractional scale; `fill_round_rect`
  clamps the corner radius to `min(w/2, h/2)`. These are the two places sub-pixel
  geometry would otherwise vanish.
- **`ScaleFactorChanged` is not handled (shipped gap).** The drawer's `scale` is
  fixed when the popup opens; if the window moves to a monitor with a different DPI
  mid-life the pixmap is not reallocated. A transient menu rarely moves between
  monitors while open, so this is low-severity, but **1.0 must handle
  `WindowEvent::ScaleFactorChanged` by rebuilding the drawer and re-rendering**, at
  least defensively.

## 7. Text shaping via cosmic-text

`RasterDrawer` owns a `FontSystem` (system font lookup + shaping) and a
`SwashCache` (rasterized-glyph cache), both in `RefCell` for the `&self`
measurement methods. Each `measure_text`/`draw_text` builds a fresh `cosmic-text`
`Buffer`, sets the text with `Shaping::Advanced`, and shapes it.

### 7.1 The pinned UI family (shipped fix — keep it)

The generic `Family::SansSerif` is deliberately avoided. `resolve_ui_family`
picks the first candidate from `[".SF NS", "SF Pro Text", "SF Pro", "Segoe UI",
"Helvetica Neue", "Arial", "DejaVu Sans", "Liberation Sans"]` for which
`family_shapes_both_weights` confirms that **both regular (400) and bold (700)
shape entirely within that one family** — because on stock macOS `.SF NS` shapes
regular but has no static bold face `fontdb` will match, so a bold request silently
falls back to **Menlo** (monospace). Pinning one concrete family for `System` keeps
a bold section header and a regular row visually one typeface. This is the
"different face for bold" glitch the Phase 1 report flagged; **the resolver is a
correctness invariant, not an optimization — do not regress it.** `FontFamily::Named`
that doesn't resolve falls through cosmic-text's own substitution silently; 1.0
should surface a debug warning when a named family isn't found.

### 7.2 The `StyleRun` UTF-16 / bidi problem (1.0 gate)

`Segment.runs: Vec<StyleRun>` carries per-substring color/weight spans with
**UTF-16** `start`/`len` (chosen to match `NSRange` and usagio's existing spans,
doc 01 §2). `style_pieces` splits a segment into consecutive `(text, color,
weight)` pieces by walking `char`s and accumulating `len_utf16()`, then
`draw_row_content` shapes and draws each piece left-to-right, advancing
`px += measure_text(piece)`.

This is **correct for LTR text** and is what ships. It has three problems 1.0 must
resolve or explicitly scope:

1. **Piece-splitting before shaping breaks complex scripts.** Splitting a segment
   into pieces on the *logical* string and shaping each piece independently breaks
   cross-boundary shaping: Arabic cursive joining, ligatures, and combining marks
   that straddle a style-run boundary will shape wrong. Even without style runs,
   the whole-segment path shapes correctly; the damage is specifically at run
   boundaries.
2. **`px += width` positioning assumes visual order == logical order.** For RTL or
   bidi text the shaped glyphs are visually reordered, but muri lays pieces
   left-to-right by measured advance, so an RTL segment with style runs renders in
   the wrong visual order.
3. **The whole layout band is LTR-only.** `resolve_segments` places segment boxes
   left→right; `Align::Right` means "right edge of the box," not "trailing edge in
   the paragraph direction." There is no paragraph-direction concept.

**1.0 decision (⚠ owner-confirm — recommended scope):** for 1.0, muri **officially
supports LTR scripts (incl. per-run color/weight) and correct rendering of
individual complex-script *glyphs within a single unstyled segment*** (cosmic-text
handles that), and **explicitly scopes out bidi/RTL reordering and styled runs over
RTL/complex text** as a documented limitation (mirrors doc 01 §2's UTF-16 caveat
and doc 02 D-register). The correct fix — shape the *entire* segment once, obtain
per-glyph cluster→byte mapping and visual positions from cosmic-text, then apply
`StyleRun` colors by mapping UTF-16 offsets → byte offsets → glyph clusters and
painting glyphs at their shaped positions (never re-measuring pieces) — is a larger
change; it is the right 1.0 target if the owner needs RTL, and this section is where
its cost is booked. Menu labels for a tray/context menu are overwhelmingly short
LTR strings, so scoping is defensible; the owner confirms whether RTL is a 1.0
requirement.

**UTF-16 offset survival, stated:** as long as pieces are cut on the logical string
(as today), UTF-16 offsets map to the logical string and survive; it is *visual
reordering* that breaks, not the offset arithmetic itself.

### 7.3 The `Theme::light()` seam inside `style_pieces` (shipped bug — fix in 1.0)

`style_pieces` resolves each `StyleRun.color` against a hard-coded `Theme::light()`,
with the comment "literal/system colors resolve theme-independently." That holds for
`Color::Rgba(..)` and the fixed `System*` colors, but a `StyleRun` using
`Color::Label` / `SecondaryLabel` / `Accent` / `Separator` resolves against the
**light** palette even when the menu is drawn dark — so a themed run color is wrong
in dark mode. **1.0-change:** thread the live `&Theme` (already in scope in
`draw_row_content`) into `style_pieces` and resolve against it. Low-risk, and it
closes the "known seam" the README calls out. Snapshot a dark-mode styled-run menu
to lock it.

## 8. Icons: decode, scale, and the not-yet-drawn kinds

`Icon` (doc 01 §3) is `Png(Arc<[u8]>)`, `Svg(Arc<[u8]>)`, `Checkmark`,
`Symbol(&'static str)`. Current drawer behavior:
- **`Png`** — decoded per draw by `decode_png` (tiny-skia `decode_png`,
  un-premultiplied to straight-alpha RGBA), then `draw_image` blits it into the
  16×16 (`ICON_SIZE`) leading cell with **nearest-neighbour** sampling.
- **`Checkmark`** (and `checked == Some(true)` with no leading icon) — a centered
  `\u{2713}` glyph in `theme.accent` (white when highlighted).
- **`Svg` and `Symbol` are accepted by the type but not drawn** (`draw_row_content`
  falls through). The facade maps muda `NativeIcon` → `Symbol` (doc 02 §4.7), so
  until this lands a `NativeIcon` shows no glyph in a custom surface (doc 02 D6).

**1.0 requirements:**
- Draw `Svg` (rasterize per-DPI via a resvg/usvg-class path into the icon cell) and
  `Symbol` (SF Symbol on macOS via AppKit → RGBA, bundled fallback glyphs
  elsewhere). Until then, the facade/docs state the gap (doc 02 D6).
- **Replace nearest-neighbour `draw_image` with at least bilinear sampling** —
  nearest-neighbour on a downscaled logo is visibly rough, and icons are decoded at
  arbitrary source sizes into a 16px cell. This is a quality gate, not correctness.
- **Cache decode.** `decode_png` runs **every frame for every visible PNG** (§9).
  1.0 caches the decoded straight-alpha RGBA keyed by `Arc` pointer identity so a
  logo decodes once, not once per repaint. `Icon` already holds `Arc<[u8]>`
  precisely so this cache is cheap and clone-safe across the ~0.75s menu rebuilds a
  consumer like usagio performs.

## 9. Performance invariants

muri's north-star is "opens with no perceptible delay" (doc 00 §5, CPU raster, no
GPU warm-up). Rendering must not erode that. State the following as **invariants**
(some are shipped-good, some are 1.0 fixes):

**Shipped-good.** `FontSystem` and `SwashCache` are built once per `RasterDrawer`
and reused, so glyph rasterization is cached across frames and across glyphs
(SwashCache is keyed by glyph+size+subpixel). Menus are small (a handful of rows),
so per-frame `Vec` allocations for `rows`/`plan`/`metrics`/`boxes`/`pieces` are
negligible in absolute terms.

**1.0 fixes / must-hold invariants.**
1. **Text is shaped far more than necessary.** A single row's text is currently
   shaped in pass 1 (`row_intrinsic`), again in pass 2 (`SegmentMetrics` build),
   and **again per style-piece** (`measure_text` inside the piece loop), plus once
   more to draw. Each shape builds a brand-new `Buffer`. For a static menu redrawn
   every frame this is wasteful. **Invariant for 1.0:** shape each segment **once
   per frame**, cache its measured width and its shaped run, and reuse it for
   layout, piece-splitting, and drawing. Target: O(visible glyphs) shaping per
   frame, not O(glyphs × passes).
2. **Per-frame PNG decode must be eliminated** (§8) — decode-once cache keyed by
   `Arc` identity.
3. **No unbounded repaint loop.** The shipped macOS `redraw` calls
   `window.request_redraw()` at the *end of every paint*, i.e. it repaints
   continuously at the display refresh rate whether or not anything changed (§10).
   **Invariant for 1.0:** a repaint is issued **only in response to a state change**
   (hover moved, selection moved, flyout opened/closed, menu content swapped, theme
   or scale changed), never unconditionally. See §10.
4. **`begin_frame` reallocates the pixmap every frame.** For a fixed-size popup this
   is a fresh heap allocation + clear each paint. 1.0 should reuse the pixmap when
   the device size is unchanged and only reallocate on resize. Minor, but it
   compounds with (3) under a continuous repaint loop.

## 10. Redraw / damage strategy

**Shipped model:** whole-frame repaint. There is no damage-rect tracking; each
`redraw`/`redraw_flyout` re-runs the full two-pass `render_menu`, re-blits the whole
`softbuffer` buffer, and (on macOS) requests another redraw. Repaints are triggered
by: `WindowEvent::RedrawRequested`, cursor moves that change the hovered row
(`on_parent_cursor`/`on_flyout_cursor` call `request_redraw` only when `hovered`
changed — good), keyboard nav (`on_key` requests redraw of affected windows),
flyout open/close, and a11y focus changes.

**1.0 requirements:**
- **Drop the unconditional trailing `request_redraw`** in `redraw` (§9.3). Menus are
  static between interactions; a menu that isn't animating should paint on open and
  then only on a state change. This is the single biggest idle-CPU win and removes a
  busy-loop that would otherwise pin a core while a menu is merely visible.
- **Whole-frame repaint is acceptable for 1.0** given menu sizes — muri does *not*
  need damage rectangles to hit its latency target. Keep the model simple; the win
  is in *when* to repaint (event-driven), not *how much* (partial). Document
  whole-frame repaint as the intended 1.0 strategy so a reviewer doesn't mistake the
  absence of damage tracking for an omission.
- **One repaint per logical change, coalesced.** Multiple state changes in one event
  (e.g. keyboard nav that both moves the top selection and updates the flyout child)
  should request at most one redraw per affected window, as `on_key` already does.

## 11. Theme resolution & `Theme::native()` color sourcing

Theme resolution is pure (`src/theme.rs`): `ThemeSource::{FollowSystem, Light, Dark,
Custom}` → `Theme` via `resolve_theme(system_is_dark)`; `Theme::resolve(Color)`
maps a semantic `Color` to a concrete `Rgba`. The pure layer resolves against a
fixed bool so it is unit-testable; the backend injects the **live** OS appearance.

**Follow-system (shipped, macOS):** `App::theme()` calls
`options.theme.wants_dark(system_is_dark)`, where `system_is_dark` reads
`NSApplication.effectiveAppearance().name()` and checks for "dark". That picks
`Theme::light()`/`dark()` per the live menu-bar appearance. Good.

**The accent gap (shipped bug — fix in 1.0).** Both default themes set
`accent = Color::Accent`, and `Theme::resolve(Color::Accent)` returns a **hard-coded
fallback blue** (`ACCENT_FALLBACK = rgb(0,122,255)`) because the semantic value
can't resolve to itself. So the shipped renderer does **not** actually follow the OS
accent — "follows OS accent by default" (doc 00 §5) is currently aspirational.
**1.0-change:** the backend must query the live OS accent and inject it as a
*literal* `Color::Rgba` into the resolved `Theme` before drawing (so
`resolve(Color::Accent)` returns the real accent, not the fallback). Same pattern
for the other live roles.

**`Theme::native()` (1.0-new, doc 01 §4) — name the OS color sources per platform.**
`Theme::native()` yields a theme whose fields are the *semantic roles themselves* so
they resolve to live OS colors, injected by each backend:

| muri role | macOS (`NSColor`) | Windows | Linux (native fallback) |
|---|---|---|---|
| `background` (`MenuBackground`, 1.0-new) | `NSColor.controlBackgroundColor` / menu material | `GetSysColor(COLOR_MENU)` / `UISettings` `Background` | n/a — native menu draws itself |
| `label` | `labelColor` | `GetSysColor(COLOR_MENUTEXT)` / `Foreground` | n/a |
| `secondary_label` | `secondaryLabelColor` | dimmed system text | n/a |
| `accent` | `controlAccentColor` | `UISettings.GetColorValue(UIColorType.Accent)` | n/a |
| `separator` | `separatorColor` | `COLOR_3DSHADOW`-class | n/a |
| `row_highlight` | `selectedContentBackgroundColor` / accent | `COLOR_HIGHLIGHT` / accent | n/a |

Linux styled surfaces (pointer `ContextMenu`) resolve the same macOS-parity palette
muri ships; the Linux *tray* is native passthrough and muri draws nothing (doc 22,
doc 00 §6). **1.0-new** requires adding `Color::MenuBackground` to the (now
`#[non_exhaustive]`, doc 03 §5) `Color` enum so `Theme::native()` can defer the
background to the OS rather than baking `rgb(246,246,246)`/`rgb(40,40,40)`.

**Vibrancy is REQUIRED for `Theme::native()` in 1.0 (locked decision #6).** This is
no longer optional: `Theme::native()` must produce a **real translucent, blurred
menu material**, not an opaque panel of the right color. A CPU raster surface cannot
*itself* blur what is behind it, so the mechanism is compositing the raster **over a
native effect view**:

- **macOS:** the popup window hosts the `softbuffer`/pixmap layer *over* an
  `NSVisualEffectView` backdrop — a transparent winit surface composited above a
  native effect view (`.menu`/`.popover` material, `behindWindow` blending). The
  panel `fill_round_rect` background is drawn at **reduced alpha or skipped
  entirely** (§3 pass 2) so the effect view shows through; only the opaque
  foreground (text, icons, highlight, separators) is painted. The rounded-corner
  mask must clip the effect view to the panel shape. This touches **window
  creation (doc 20 §2)**, not the portable drawer's primitive set — the
  `SceneDrawer` trait is unchanged; what changes is how its output is composited
  and what the backend puts *behind* it.
- **Windows:** the equivalent is an **acrylic backdrop** (`DwmSetWindowAttribute` /
  `SystemBackdropType` acrylic, or the layered-window composite) behind the same
  reduced-alpha raster (doc 21 §2).
- **Linux:** the styled pointer-`ContextMenu` surface has **no portable vibrancy**
  — blur is compositor-owned and unavailable through a client protocol on GNOME
  Wayland, so the Linux styled surface degrades to an opaque `Theme::native()`
  panel; the native-menu fallback is **n/a** (the host draws the menu). Documented
  as the one platform where `Theme::native()` is opaque (doc 22).

**Compositing rework this requires (1.0-new, load-bearing).** The shipped backend
blits **opaque**: `App::redraw` un-premultiplies the `tiny-skia` pixmap *over the
theme background* into `0RGB` words (doc 20 §2), so the surface is transparent only
at the rounded corners and the panel body is opaque. Vibrancy needs a
**transparent / alpha-preserving composite path**: present the pixmap's straight
alpha so the effect view behind it is visible wherever the panel fill was reduced
or skipped. This is newly-required 1.0 work on every platform's present path (macOS
`softbuffer` over the effect view; Windows `UpdateLayeredWindow` per-pixel alpha).
It also makes doc 30's **Option C** (single-window composite of parent + flyout
stack, which needs the same per-pixel transparency) materially cheaper — the same
alpha-preserving present path serves both.

**Divergence-register note:** D1 (doc 02 §9, doc 01 §4) **changes meaning** — it is
no longer "no vibrancy, permanent." macOS and Windows custom surfaces now *are*
vibrant/translucent under `Theme::native()`; D1 narrows to "the Linux styled
`ContextMenu` surface is opaque (compositor-owned blur is unavailable), and exact
material match is not guaranteed pixel-for-pixel."

## 12. Multi-monitor & the work-area / anchor interaction

Placement arithmetic is pure and shared (`anchor::place_popup`,
`flyout::place_flyout`): given an anchor rect, a popup size, the target monitor's
**work area**, an `Edge`, and a gap, they return a top-left origin that flips on
spill and clamps fully on-screen (exhaustively unit-tested in `anchor.rs` /
`flyout.rs`). muri coordinates are logical, top-left origin (doc 01 §7). The
per-OS job is only to supply the right anchor rect and the right work area.

**N-level flyout stack placement (decision #8).** With N-level nested submenus,
`place_flyout` is applied **once per level**: level *k*'s panel is placed relative
to the highlighted submenu row in level *k−1*'s panel (its anchor rect), not always
relative to the root popup. Each level opens to the right of its parent row and
**flips left on spill**, so a deep stack that runs out of room to the right cascades
back leftward; every level independently clamps into the *same* work area. The
placement math is unchanged per level — the 1.0 work is driving it over a
`Vec<Flyout>` stack (doc 20 §2, doc 40 §5) instead of a single `Option<Flyout>`, and
the gap-crossing hover rule (doc 40 §4) must hold across N adjacent panels. Vertical
clamp still applies per level.

**Shipped macOS reality and its multi-monitor bug (call out for doc 20).** Both
`MacosAnchor::anchor_rect` (Y-flip using screen height) and `App::work_area` read
`NSScreen::screens().firstObject()` — **the primary screen** — not the screen the
menu-bar/status-item actually lives on. On a multi-monitor setup where the status
item is on a secondary display, the Y-flip and the work-area clamp use the wrong
monitor's geometry, so the popup can be mispositioned or clamped to the wrong
bounds. The *math* is correct; it is fed the wrong monitor. **1.0-change:** resolve
the anchor's screen (the `NSScreen` whose frame contains the status-button window)
and use *that* screen's frame for the Y-flip and its `visibleFrame` for the work
area. Per-monitor DPI likewise: the work area and anchor must come from the
anchor's monitor, and the drawer scale must match that monitor's `scale_factor`
(§6). Windows (`Shell_NotifyIconGetRect` + `MonitorFromPoint`/work area) and Linux
(pointer-relative, doc 22) have the analogous rule: **always use the anchor's own
monitor, never the primary.**

## 13. What this document guarantees to the rest of the spec

- One portable `SceneDrawer`/`RasterDrawer`, tested headless (doc 50 snapshots via
  `RasterDrawer::encode_png`).
- A single item-index space shared by `LaidMenu` (render/hit-test), `MenuFocus`
  (input, doc 40), and `AxNode.item_index` (a11y, doc 30).
- The flush-right / no-reserved-chevron guarantee for submenu-free menus, with the
  submenu-menu gutter caveat documented (§4).
- Logical-coordinate drawing with implementation-owned device scaling, and the
  fractional-scale / `ScaleFactorChanged` / multi-monitor fixes enumerated as 1.0
  work (§6, §12).
- **Vibrancy / translucent `Theme::native()` (decision #6):** the reduced-alpha /
  skipped panel fill and the alpha-preserving composite path over a native effect
  view (macOS `NSVisualEffectView`, Windows acrylic; Linux styled surface opaque),
  a 1.0 requirement on the present path but not on the `SceneDrawer` primitives
  (§3, §11, doc 20 §2).
- **N-level flyout stack (decision #8):** per-level `LaidMenu`s and per-level
  `place_flyout` over a `Vec<Flyout>`, cascading left on spill (§3, §5, §12).
- The three open renderer risks handed to adversarial review: **bidi/RTL + styled
  runs (§7.2), the `Theme::light()` style-run seam (§7.3, a fix), and the accent /
  live-OS-color injection gap (§11, a fix)** — plus the shipped continuous-repaint
  loop (§9.3/§10) that must not survive to 1.0.
