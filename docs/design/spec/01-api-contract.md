# 01 — API contract: muri's native, styleable public API

Status: **foundational spec — the north star.** This document specifies muri's
**own** public API. Per the pivot (locked decision #1, [`00`](00-overview.md) §2/§4),
the native API is the **primary, first-class** surface — designed for a live menu
app, not derived from muda. The muda-compat drop-in is a first-class on-ramp that
maps **1:1 onto this API** ([`02-muda-compat.md`](02-muda-compat.md)); threading and
event semantics are [`03-threading-events-versioning.md`](03-threading-events-versioning.md).

Signatures below are grounded in the shipped crate. The 1.0 native API adds a
fluent, **owned `Menu::new().add(…)` builder** with typed item kinds, per-item
styling, native typed events, N-level submenus, and an extension seam; where this
differs from the shipped `Row`-centric model it is marked **1.0-change** and the
migration is noted. Where a type is **shipped** it is quoted as-is; **1.0-new** marks
additions.

---

## 1. Design principles

1. **The data model is the product.** `Menu` / `Item` / `Row` / `Segment` is a
   pure, backend-agnostic, `Clone`+`Debug` tree with no I/O. A consumer builds it,
   hands it to a surface, and muri draws it. The same tree drives rendering, the
   hit-test map, keyboard nav, and the accessibility tree — so it must be
   expressive enough to be *all* of those at once.
2. **Semantic-by-default, literal-when-you-want.** Colors and fonts default to
   *semantic* roles that resolve against the active `Theme` (and to live OS colors
   under `FollowSystem`), so a menu looks native without the consumer choosing a
   single hex value. Literal `Color::Rgba` / named fonts are always available.
3. **Builders are consuming (`self`-taking) and cheap.** Every builder method
   takes `self` and returns `Self`, so construction is a fluent expression. The
   tree is plain data (`pub` fields) so a consumer can also mutate it directly.
4. **Opaque `MenuId` strings.** muri hands a row's id back verbatim on activation
   and never interprets it. The id *grammar* is entirely the consumer's business.
5. **The flexibility unlock is progressive.** Facade → `Theme::native()` (looks
   native) → `ThemeSource::Custom(Theme)` (restyle globally) → per-`Segment`
   `Flex`/`Align`/`Color`/`Font` (restyle per row). No cliff between "muda-like"
   and "fully custom."

## 2. The declarative menu tree and the `add(…)` builder

All types in `muri::menu`, re-exported at the crate root. The **1.0 primary surface**
is a fluent, owned `Menu::new().add(…)` builder; the north-star shape (owner's
sketch):

```rust
Menu::new()
    .add(text("Text row"))
    .add(submenu("Sub Menu")
        .add(text("SubMenu Text 1"))
        .add(separator())
        .add(image("logo.png"))
        .add(text("Quit").color(Color::Red).align(Align::Right).on(|| quit())));
```

### 2.1 `Menu` and the one stable door (`add`)

```rust
pub struct Menu { /* items + a MenuId→handler registry (see §6) */ }

impl Menu {
    pub fn new() -> Self;
    pub fn add(self, item: impl Into<Item>) -> Self;   // 1.0-new — THE stable door
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn interactive_count(&self) -> usize;          // focusable items at this level
    pub fn items(&self) -> &[Item];                    // pure data view (layout/a11y/keynav)
}
```

`add(impl Into<Item>)` accepts **any** item kind — `Text`, `Image`, `Separator`,
`Submenu`, `Check`, `Predefined`, or a `Custom` node — via `Into<Item>`. It is
deliberately the *only* growth point: new content kinds (1.x) plug into the same
`add` (§9, [`00` §11](00-overview.md)). N-level nesting is **inherent**: a `Submenu`
is itself `.add()`-able into a `Submenu`.

**1.0-change (migration from the shipped `Row` model).** The shipped crate models
`Menu { pub items: Vec<Item> }` with `Item::{Row, Separator, SectionHeader, Submenu}`
and `Menu::{item,row,separator,section_header,submenu}` builders. 1.0 keeps that
`Vec<Item>` **data representation** (so the pure layout/keynav/a11y core is
unchanged) but reshapes the *public builder* to the typed-item `add(…)` form above.
The shipped `row`/`item`/`submenu` methods and the `Row`/`Segment` primitives are
**retained as the lower-level API** (§2.4); the typed builders (`Text`, `Submenu`,
…) are ergonomic constructors over them. Migration is mechanical and additive.

### 2.2 `Item` — the open kind set

```rust
#[non_exhaustive]                       // 1.0-change — future kinds are additive
pub enum Item {
    Text(Text),                         // a label row (the common case)
    Image(Image),                       // an image/logo row
    Separator,                          // a divider
    SectionHeader(Text),                // styled, non-interactive heading
    Submenu(Submenu),                   // opens a flyout panel beside it (N-level)
    Check(Check),                       // a checkable row
    Predefined(Predefined),             // a muda-style OS-action item (§ compat)
    Custom(Box<dyn MenuNode>),          // 1.0-new — third-party / future rows (§9)
}

impl Item {
    pub fn is_interactive(&self) -> bool;   // enabled rows/submenus/checks with a real id
}
```

`Item` is `#[non_exhaustive]` so promoting a future built-in (e.g. `Video`) is not a
breaking change ([`00` §11](00-overview.md)). `interactive_count` / `is_interactive`
are the single source of truth for "what can be focused/clicked," consumed by both
`keynav` and `a11y` — a separator, a section header, a disabled row, and an inert
info row (`MenuId::none()`) are all non-interactive.

### 2.3 Typed item builders + free-function constructors (1.0-new)

Each kind has a fluent builder with per-item modifiers, and a lowercase free-function
constructor as an alternative to `::new` (idiomatic-Rust ergonomics):

```rust
pub fn text(s: impl Into<String>) -> Text;         // == Text::new(s)
pub fn image(src: impl Into<Icon>) -> Image;       // == Image::new(src)
pub fn submenu(label: impl Into<String>) -> Submenu;
pub fn separator() -> Item;                          // == Item::Separator
pub fn check(label: impl Into<String>) -> Check;

pub struct Text { /* segments, id, enabled, on-handler … */ }
impl Text {
    pub fn new(s: impl Into<String>) -> Self;
    pub fn id(self, id: impl Into<MenuId>) -> Self;   // else an internal id is assigned
    pub fn color(self, c: Color) -> Self;             // per-item color (headline advantage)
    pub fn align(self, a: Align) -> Self;             // Left | Center | Right
    pub fn bold(self) -> Self;                        // == .weight(Weight::Bold)
    pub fn font(self, f: Font) -> Self;
    pub fn icon(self, icon: impl Into<Icon>) -> Self; // leading icon/logo
    pub fn trailing(self, icon: impl Into<Icon>) -> Self;
    pub fn enabled(self, yes: bool) -> Self;
    pub fn accelerator(self, a: Accelerator) -> Self; // displayed + in-menu (doc 40 §3)
    pub fn segment(self, s: Segment) -> Self;         // advanced multi-column (§2.4)
    pub fn on(self, f: impl FnMut() + Send + 'static) -> Self;  // native click (§6)
}
// From<Text>/From<Image>/From<Submenu>/From<Check>/From<Predefined> for Item;
// From<&str>/From<String> for Text (so .add("Quit") works).

pub struct Submenu { /* label: Text, menu: Menu */ }
impl Submenu {
    pub fn new(label: impl Into<String>) -> Self;
    pub fn add(self, item: impl Into<Item>) -> Self;  // nest (N-level, inherent)
    pub fn icon(self, icon: impl Into<Icon>) -> Self;
    pub fn enabled(self, yes: bool) -> Self;
    pub fn id(self, id: impl Into<MenuId>) -> Self;
}

pub struct Check { /* like Text + checked: bool */ }
impl Check {
    pub fn new(label: impl Into<String>) -> Self;
    pub fn checked(self, yes: bool) -> Self;
    // + the same color/align/enabled/on/… modifiers as Text
}

pub struct Image { /* icon + optional id/on */ }
impl Image { pub fn new(src: impl Into<Icon>) -> Self; pub fn on(self, f: impl FnMut()+Send+'static) -> Self; /* … */ }
```

Per-item modifiers (`.color`/`.align`/`.icon`/`.bold`/`.enabled`/`.checked`/
`.accelerator`/`.on`) apply uniformly to the built-in kinds and, where meaningful, to
`Custom` nodes (§9). **Per-item styling is muri's headline advantage over muda** — a
native menu cannot color one value red or right-align it — so it is first-class on
the common `Text`/`Check` row, not buried in a `Segment` API.

### 2.3.1 Each muda item kind maps 1:1 onto a native constructor

Because the native kinds were designed to *cover* muda's, the compat wrapper is
mechanical — it never needs anything but these built-in constructors:

| muda | muri native constructor |
|---|---|
| `MenuItem::new(text, enabled, accel)` | `text(text).enabled(..).accelerator(..)` |
| `CheckMenuItem::new(text, .., checked, ..)` | `check(text).checked(..)` |
| `IconMenuItem::new(text, icon, ..)` | `text(text).icon(..)` (or `image(..)`) |
| `Submenu::new(text, enabled)` + children | `submenu(text).enabled(..).add(..)…` |
| `PredefinedMenuItem::separator()` | `separator()` |
| `PredefinedMenuItem::quit()/copy()/about()/…` | `Predefined::quit()/copy()/about()/…` |
| `MenuId` / `with_id` | `.id(..)` (same `MenuId(String)`) |
| accelerator | `.accelerator(Accelerator)` |

The compat layer ([`02`](02-muda-compat.md)) is thus a thin translation onto these
constructors; it never touches the `MenuNode` extension seam (§6.5).

### 2.4 The lower-level `Row` / `Segment` primitive (retained)

`Text` is sugar over a single-segment row; the multi-column, flush-right primitive
(`Row` + `Segment`, §2.5 below and §`10`) is retained for rows that need several
independently-aligned columns (a `Flex::Grow` label + an `Align::Right` value). A
`Text` with more than one `.segment(…)` produces exactly such a `Row`. The `Row`
data type (below) is what the pure layout/hit-test/a11y core consumes; the typed
builders are constructors over it.

### 2.5 `Row` (the lower-level row data type, retained)

```rust
pub struct Row {
    pub id: MenuId,                  // MenuId::none() ⇒ inert (info rows, headers)
    pub segments: Vec<Segment>,      // laid out left→right across columns
    pub leading: Option<Icon>,       // logo / avatar / checkmark column
    pub trailing: Option<Icon>,
    pub enabled: bool,               // default true
    pub checked: Option<bool>,       // Some ⇒ check column; None ⇒ no column
    pub background: Option<Color>,
    pub min_height: Option<f32>,
}

impl Row {
    pub fn new(id: impl Into<MenuId>) -> Self;   // enabled, no segments
    pub fn info() -> Self;                        // id = none(); for info lines/headers
    pub fn label(self, text: impl Into<String>) -> Self;   // append a plain left segment
    pub fn segment(self, segment: Segment) -> Self;
    pub fn segments(self, segments: Vec<Segment>) -> Self;
    pub fn leading(self, icon: Icon) -> Self;
    pub fn trailing(self, icon: Icon) -> Self;
    pub fn enabled(self, enabled: bool) -> Self;
    pub fn checked(self, checked: bool) -> Self;
    pub fn background(self, color: Color) -> Self;
    pub fn min_height(self, height: f32) -> Self;
    pub fn accessible_name(&self) -> String;      // segment texts joined by spaces
}
```

`accessible_name()` is the a11y and type-ahead name; it is **not** an override
point in 1.0 (the name is derived from segment text). **1.0-new opportunity:** an
optional `accessibility_label: Option<String>` for rows whose visible text is not
their best spoken name (e.g. a row that is only an icon). Flagged for
[`30-accessibility.md`](30-accessibility.md) to decide; deferring it is acceptable
for 1.0 as long as icon-only rows are rare.

### Segment, StyleRun, and the flush-right guarantee

```rust
pub struct Segment {
    pub text: String,
    pub runs: Vec<StyleRun>,   // per-substring color/weight spans; empty ⇒ whole-segment
    pub align: Align,          // Left (default) | Center | Right
    pub flex: Flex,            // Fixed (default) | Grow
    pub font: Option<Font>,    // else the row/theme font
    pub color: Option<Color>,  // else Color::Label
}
impl Segment {
    pub fn new(text: impl Into<String>) -> Self;
    pub fn align(self, align: Align) -> Self;
    pub fn flex(self, flex: Flex) -> Self;
    pub fn runs(self, runs: Vec<StyleRun>) -> Self;
    pub fn font(self, font: Font) -> Self;
    pub fn color(self, color: Color) -> Self;
}

pub struct StyleRun { pub start: usize, pub len: usize, pub color: Color, pub weight: Option<Weight> }
// start/len are UTF-16 code units — matches NSRange and usagio's existing spans.
impl StyleRun {
    pub fn new(start: usize, len: usize, color: Color) -> Self;
    pub fn weight(self, weight: Weight) -> Self;
}

pub enum Align { Left, Center, Right }   // default Left
pub enum Flex  { Fixed, Grow }           // default Fixed
```

**The core guarantee (muri's whole reason to exist):** a `Flex::Grow` label
followed by an `Align::Right` value produces a **truly flush-right value with no
reserved chevron/submenu column**. The layout engine (`layout::resolve_segments`,
[`10-rendering-layout.md`](10-rendering-layout.md)) gives all leftover row width to
the `Grow` segment and right-aligns the value against the band edge. Native menus
cannot do this because they always reserve a submenu column; muri can because it
owns the pixels. This is contractually part of the API: a consumer relying on it
is relying on a guarantee, not an accident.

**UTF-16 offset caveat (1.0 gate for doc 10):** `StyleRun` offsets are UTF-16 code
units. `render::paint::style_pieces` currently walks `char`s accumulating
`len_utf16()` to slice pieces — correct for LTR. Under RTL/BiDi reordering these
offsets must map to the *logical* string, not the visual order; 1.0 must confirm
`cosmic-text` shaping preserves this or document the limitation. See
[`10-rendering-layout.md`](10-rendering-layout.md).

## 3. Style primitives

All in `muri::style`.

```rust
pub enum Color {                          // literal OR semantic (theme/OS-resolved)
    Rgba(u8, u8, u8, u8),
    Label, SecondaryLabel, Accent, Separator,
    SystemRed, SystemOrange, SystemGreen, SystemYellow,
}
impl Color { pub fn rgb(r,g,b) -> Self; pub fn is_literal(&self) -> bool; }

pub struct Rgba { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }   // fully resolved
// consts BLACK/WHITE/TRANSPARENT; new(r,g,b,a); opaque(r,g,b)

pub enum FontFamily { System, SystemMono, Named(String) }        // default System
pub enum Weight { Regular, Medium, Semibold, Bold }              // ot_weight()->u16
pub struct Font { pub family: FontFamily, pub size: f32, pub weight: Weight }
impl Font { pub fn system(size,weight); pub fn mono(size,weight); pub fn with_weight(self,w); }
// Font::default() = System 13.0 Regular

pub enum Icon {
    Png(Arc<[u8]>),     // decoded at draw (tiny-skia); shipped-drawn
    Svg(Arc<[u8]>),     // rasterize per-DPI — 1.0-new (not yet drawn)
    Checkmark,          // themed check glyph in the leading column; shipped-drawn
    Symbol(&'static str),  // SF Symbol on macOS, bundled fallback elsewhere — 1.0-new
}
impl Icon { pub fn from_png_bytes(bytes: impl Into<Arc<[u8]>>); pub fn from_svg_bytes(...); }
```

**Fidelity status the API must not overstate:** today the drawer renders
`Icon::Png` and `Icon::Checkmark` (and `checked == Some(true)` without a leading
icon draws a checkmark). `Icon::Svg` and `Icon::Symbol` are *accepted by the type*
but **not yet drawn** (`render::paint::draw_row_content` skips them). 1.0 must
either draw them or the facade/docs must be explicit. `Icon` uses `Arc<[u8]>` so
icons are cheap to clone across the ~0.75s menu rebuilds a consumer like usagio
does.

## 4. Theming

All in `muri::theme`.

```rust
pub enum ThemeSource { FollowSystem, Light, Dark, Custom(Theme) }   // default FollowSystem
impl ThemeSource {
    pub fn wants_dark(&self, system_dark: impl Fn() -> bool) -> bool;
    pub fn resolve_theme(&self, system_is_dark: bool) -> Theme;
}

pub struct Theme {
    pub background, label, secondary_label, accent, separator, row_highlight: Color,
    pub row_font, header_font: Font,
    pub row_height, corner_radius, column_gap: f32,
    pub padding: Insets,
}
impl Theme {
    pub fn light() -> Self;
    pub fn dark() -> Self;
    pub fn native() -> Self;                     // 1.0-new — see below
    pub fn resolve(&self, color: Color) -> Rgba; // semantic → concrete
}

pub struct MenuOptions { pub min_width: Option<f32>, pub max_width: Option<f32>, pub theme: ThemeSource }
```

### `Theme::native()` — the muda-parity default (1.0-new)

Locked decision #1 requires a `Theme::native()` that makes a facade-built menu
look native. Specification:

- `Theme::native()` returns a theme whose color fields are the **semantic roles
  themselves** (`background = Color::Label`? no — `label = Color::Label`,
  `accent = Color::Accent`, `separator = Color::Separator`, `row_highlight =
  Color::Accent`, `background` = a semantic `Color::MenuBackground`), so that at
  draw time every color resolves to the **live OS color** (NSColor / UISettings)
  rather than a baked palette. Fonts are `FontFamily::System` at the platform
  default menu size. Spacing/radius match the platform menu metrics.
- **1.0-new semantic colors required:** the shipped `Color` enum lacks a
  menu-background role and control colors. 1.0 adds at minimum
  `Color::MenuBackground` (resolves to `NSColor.controlBackgroundColor` /
  `menu` background per OS) so `Theme::native()` can defer the background to the OS
  too. Without it, `Theme::light()/dark()` bake `rgb(246,246,246)` / `rgb(40,40,40)`
  which is *close* but not the exact OS menu material (no vibrancy/translucency).
- **Vibrancy is required (locked decision #6).** `Theme::native()` renders with
  **real translucent vibrancy**, not an opaque approximation: on macOS the raster
  layer is hosted over an `NSVisualEffectView` so the panel is a genuine blurred
  material; the Windows equivalent is **acrylic**. This is not a future
  enhancement — it is a 1.0 promise, because "looks native" is load-bearing for the
  muda drop-in. It forces a transparency/compositing rework (the drawer must present
  alpha-preserving output; the panel background draws at reduced alpha or is skipped
  so the effect view shows through) specified in
  [`10-rendering-layout.md`](10-rendering-layout.md) §11 and
  [`20-platform-macos.md`](20-platform-macos.md) §2. On Linux the styled surface is
  opaque (compositor-owned blur is not client-controllable) and the native-menu
  fallback is host-drawn — the one place vibrancy does not apply.

`ThemeSource::FollowSystem` (the default `MenuOptions.theme`) already resolves to
`Theme::light()` / `Theme::dark()` by querying `system_is_dark`. **1.0-change:**
`FollowSystem` should resolve to `Theme::native()`'s light/dark variants so the
default look is OS colors, and the facade sets `MenuOptions { theme:
ThemeSource::FollowSystem, .. }` implicitly. The existing pure `resolve_theme` test
seam (a fixed bool) is preserved.

**Custom-theme dark-mode gap (shipped bug to fix in 1.0):**
`ThemeSource::Custom(theme)` ignores `system_is_dark` — a custom theme is a single
fixed look. That is fine for a consumer that wants one look, but a consumer that
wants "my colors, but follow dark/light" has no path. 1.0 should add either
`ThemeSource::CustomFollowSystem { light: Theme, dark: Theme }` or a
`Theme::variant(dark: bool)` hook. Flagged; not blocking M1–M2.

## 5. Surfaces

muri's surfaces are the objects that put a `Menu` on screen. All carry the `Menu`'s
per-item `.on()` handlers plus an optional surface-level `.on_event(&MenuId)` sink
(§6), and expose `accessibility_tree()`.

### 5.1 `Tray` + `TrayHandle` (the live-app model — primary)

Owns the tray icon + the tray-anchored popup + anchoring. Subsumes `tray-icon`. The
**retained handle is the primary runtime model** (locked decision #1): muri is built
for a live app that rewrites its menu ~0.75s, so updating the menu *while the loop
runs* is a first-class path, not an afterthought.

```rust
pub struct Tray { /* icon, menu, tooltip, options */ }
impl Tray {
    pub fn new(icon: impl Into<Icon>) -> Self;
    pub fn menu(self, menu: Menu) -> Self;
    pub fn tooltip(self, text: impl Into<String>) -> Self;
    pub fn options(self, options: MenuOptions) -> Self;
    pub fn theme(self, theme: ThemeSource) -> Self;             // convenience for options.theme
    pub fn accessibility_tree(&self) -> AxTree;
    pub fn dispatch(&self, id: &MenuId);                        // testable click path
    pub fn handle(&self) -> TrayHandle;   // 1.0-new — Clone+Send; valid to clone before run()
    pub fn run(self) -> Result<()>;       // install icon + run event loop; consumes self
}

// 1.0-new — the retained, thread-safe handle for live updates (doc 03 §2)
pub struct TrayHandle { /* posts commands into the running loop via EventLoopProxy */ }
impl TrayHandle {                          // Clone + Send
    pub fn set_menu(&self, menu: Menu);    // swap content; next paint re-renders
    pub fn set_icon(&self, icon: impl Into<Icon>);
    pub fn set_tooltip(&self, text: impl Into<String>);
    pub fn set_visible(&self, visible: bool);
    pub fn open(&self);
    pub fn close(&self);
}
```

Behavior contract:

- **The retained-handle pattern (the resolution of the old `run(self)` tension).**
  Obtain a `TrayHandle` from `tray.handle()` **before** `run` takes over, keep it,
  and call `handle.set_menu(new_menu)` / `set_icon` / … from any thread as the app's
  state changes. The handle posts commands into the UI loop (via the `winit`
  `EventLoopProxy` the macOS backend already uses), which applies them on the UI
  thread (rebuild `LaidMenu`, re-render). `TrayHandle` is `Clone + Send`. This is
  **load-bearing for any live app** and an M1 deliverable, not 1.0 polish
  ([`03`](03-threading-events-versioning.md) §2). It also cleanly replaces the muda
  drop-in tension: a native consumer holds a handle; a *compat* consumer keeps its
  own passive loop and never sees `run`/`TrayHandle` ([`02`](02-muda-compat.md)).
- **`run` consumes `self` and blocks** running the platform event loop. On macOS it
  must be called on the main thread (`MainThreadMarker`); a **tray-only** native app
  sets `Accessory` activation policy (no Dock icon). The menu-bar-app conflict — an
  app that *also* installs a native menu bar cannot be `Accessory` — is decided by
  the surface combination (the compat layer sets `Regular` when `init_for_nsapp` is
  used); see [`20-platform-macos.md`](20-platform-macos.md) §5.
- On **Linux**, `run` returns `Err(Unsupported::TrayAnchor)` (decision #3).
- **`open` / `close`** are `todo!()` today; 1.0 implements programmatic show/hide via
  `TrayHandle`.

### 5.2 `ContextMenu` (skeleton)

A styled menu at an explicit screen point. muri's **portable** primitive — works
on Linux via `xdg_positioner`.

```rust
pub struct ContextMenu { /* menu, options, on_click */ }
impl ContextMenu {
    pub fn new(menu: Menu) -> Self;
    pub fn options(self, options: MenuOptions) -> Self;
    pub fn on_click(self, handler: impl Fn(&MenuId) + Send + 'static) -> Self;
    pub fn accessibility_tree(&self) -> AxTree;
    pub fn dispatch(&self, id: &MenuId);
    pub fn open_at(&self, point: LogicalPoint, edge: Edge) -> Result<()>;   // todo! today
}
```

`open_at` is `todo!()` today; it is the M2 deliverable and the Linux styled path.
**Open question:** `open_at(&self, ...)` takes `&self` and must run/participate in
an event loop — on Linux/Wayland it needs the caller's own surface handle for
`xdg_positioner`. 1.0 must decide whether `ContextMenu` runs its own nested loop
or integrates with the caller's `winit` loop (see the `raw-window-handle` question
in [`22-platform-linux.md`](22-platform-linux.md)).

### 5.3 `Popup` / `Dropdown` (1.0-new)

The generic dropdown surface: a popup anchored to an **arbitrary caller
rectangle** (e.g. a toolbar button), not the tray icon. Reuses the exact
`place_popup` math the tray uses.

```rust
// 1.0-new
pub struct Popup { /* menu, options, on_click */ }
impl Popup {
    pub fn new(menu: Menu) -> Self;
    pub fn options(self, options: MenuOptions) -> Self;
    pub fn on_click(self, handler: impl Fn(&MenuId) + Send + 'static) -> Self;
    pub fn anchored_to(&self, anchor: LogicalRect, edge: Edge) -> Result<()>;
}
```

`Tray`, `ContextMenu`, and `Popup` differ only in *how the anchor rectangle is
obtained* (tray icon rect / a point / a caller rect); all three funnel into the
same `place_popup` + scene drawer + dismiss logic. 1.0 should factor a shared
`PopupSession` so the three surfaces are thin adapters, not three copies of the
macOS `App` event handler.

## 6. Event delivery — native first (decision #5)

muri emits its **own** native events (decision #5 — no dependency on the muda crate
for events). The native surface has two mechanisms; the muda-style global channel is
a **compat-only projection** of them, not the native model.

1. **Per-item `.on(FnMut)` callback (1.0-new, native primary).** `text("Quit").on(||
   quit())` attaches a typed handler to *that item*. It runs on the UI thread when
   the item is activated by mouse, keyboard (`NavAction::Activate`), or an AccessKit
   `Click`. Zero-arg — the item's identity is implicit. Inert items never fire.
2. **muri event stream keyed by id (1.0-new, native).** A surface-level stream /
   handler delivering the activated `MenuId`, for consumers who prefer a single
   sink over per-item closures: a surface `.on_event(impl FnMut(&MenuId))` (and a
   pollable `muri::events()` stream). This is muri's *own* stream — not muda's global
   channel.
3. **Global `MenuEvent` channel (compat projection):** the process-global
   `muri::MenuEvent::receiver() -> &'static MenuEventReceiver` delivering
   `MenuEvent { id }` exists so the **muda-compat** door works unchanged; muri
   *projects* native activations onto it. A native consumer need never touch it.
   See [`03`](03-threading-events-versioning.md) §3 for ordering/threading.

**The purity tension and its resolution (1.0-change).** Attaching an `FnMut` to an
item would make the menu tree non-`Clone`/non-`Debug`, breaking the pure-core
testing invariant that layout / keynav / a11y / snapshot tests depend on
([`50`](50-testing-verification.md), [`03` §1](03-threading-events-versioning.md)).
So `.on(…)` does **not** store the closure in the data tree: the builder assigns the
item a `MenuId` (an internal one if the consumer gave none) and registers the handler
in the `Menu`/surface's **handler registry keyed by `MenuId`**. The `items(): &[Item]`
data view stays pure, `Clone + Debug`, and drives all the pure engine functions;
handlers are consulted only at dispatch. This keeps per-item `.on()` ergonomic
*and* the core testable.

`MenuEvent` and `MenuId`:

`MenuEvent` and `MenuId`:

```rust
pub struct MenuEvent { pub id: MenuId }
pub struct MenuId(pub String);
impl MenuId {
    pub fn none() -> Self;            // inert sentinel (empty string)
    pub fn as_str(&self) -> &str;
    pub fn is_none(&self) -> bool;
}
// From<&str>, From<String>
```

`MenuId` is deliberately `pub struct MenuId(pub String)` — **structurally
identical to muda's `MenuId`** — which is what lets the facade share ids without
conversion (decision #1). The empty-string `none()` sentinel is a muri addition
for inert rows and has no muda equivalent (muda auto-assigns ids); the facade
handles that mapping ([`02-muda-compat.md`](02-muda-compat.md) §3).

## 6.5 The `MenuNode` extension seam (1.0-new — ships in 1.0)

`add(…)` is the one stable door, and `Item::Custom(Box<dyn MenuNode>)` is how a
row that is *not* a built-in kind plugs into the **same** builder and the **same**
render / keynav / a11y pipeline. The trait ships in 1.0 even though the rich nodes
built on it (Video, etc.) are 1.x roadmap ([`00` §11](00-overview.md)):

```rust
// 1.0-new
pub trait MenuNode: Debug + Send + 'static {
    fn measure(&self, ctx: &MeasureCtx, constraints: LogicalSize) -> LogicalSize; // intrinsic layout
    fn draw(&self, scene: &mut dyn SceneDrawer, layout: LogicalRect, ctx: &DrawCtx); // into the tiny-skia scene
    fn hit_test(&self, layout: LogicalRect, point: LogicalPoint) -> bool;
    fn accessibility(&self) -> AxNodeSpec;   // role / name / state → the AxNode model (doc 30)
    fn is_interactive(&self) -> bool { false }
    fn id(&self) -> Option<&MenuId> { None }
}
```

- **Same pipeline.** `render_menu` measures/draws a `Custom` node through the same
  `SceneDrawer` ([`10`](10-rendering-layout.md) §2); `keynav` treats it as focusable
  iff `is_interactive()`; `a11y::build_tree` folds its `accessibility()` into the
  tree ([`30`](30-accessibility.md)). Per-item modifiers (`.on()`, `.enabled`) apply
  to a custom node where meaningful (a custom node with an id + `is_interactive` can
  carry an `.on()` handler).
- **Dyn dispatch is fine** — menus have few rows.
- **The compat layer never touches this seam.** muda's kinds map only onto the
  *built-in* nodes ([`02`](02-muda-compat.md)); `Custom` is a native-API-only
  extension.

## 7. Geometry and errors

```rust
pub struct LogicalPoint { pub x: f32, pub y: f32 }
pub struct LogicalSize  { pub width: f32, pub height: f32 }
pub struct LogicalRect  { pub origin: LogicalPoint, pub size: LogicalSize }
    // max_x/max_y/contains
pub enum Edge { Bottom, Top, Left, Right }   // grow direction; default Bottom
pub struct Insets { pub top,right,bottom,left: f32 }   // uniform/symmetric/horizontal/vertical

pub enum Unsupported { TrayAnchor, ClientPositioning }
pub enum Error { Unsupported(Unsupported), BadIcon(String), Platform(String) }
pub type Result<T> = std::result::Result<T, Error>;
```

All muri coordinates are **logical** (DPI-independent); the drawer owns
device-scale conversion. `LogicalRect` origin is top-left (the macOS backend
flips `NSStatusItem`'s bottom-left frame into this space). Errors are a small
closed set; the facade maps muda's richer `Error` onto these
([`02-muda-compat.md`](02-muda-compat.md) §7).

## 8. Ownership and lifetimes

- **The menu tree is owned data** (`Clone + Debug`, `'static`). No borrows leak
  into it; `Icon` holds `Arc<[u8]>`. A consumer can build a `Menu`, clone it, hand
  it to a surface, and keep or drop its copy freely.
- **Handlers live in a `MenuId`-keyed registry, not in the data tree.** Per-item
  `.on(FnMut + Send + 'static)` handlers (and a surface `.on_event`) are owned by the
  surface's handler registry keyed by `MenuId` (§6), so the `Item` data tree stays
  pure `Clone + Debug` for the engine and tests. Handlers are `Send` (movable to the
  UI thread) and `'static`; a handler needing shared state uses interior mutability
  (`Arc<Mutex<_>>` / atomics), as the shipped tests do.
- **Surfaces are not `Send`/`Sync`** once running — they own platform windows and
  must live on the UI thread. Runtime interaction from other threads goes through the
  **`TrayHandle`** (write side — live menu/icon updates) and muri's native event
  stream / the compat global channel (read side), never by moving the surface.
- **The a11y tree (`AxTree`) is a snapshot**, rebuilt from the menu on demand;
  it borrows nothing and is safe to build off the UI thread for testing.

## 9. What the public surface is (and is not)

The complete 1.0 public export set: the surfaces `Tray`, `TrayHandle`,
`ContextMenu`, `Popup`; the builder `Menu` + `add`; the item kinds and their
builders `Item`, `Text`, `Image`, `Submenu`, `Check`, `Predefined`, and the
`MenuNode` trait; the free-function constructors `text`, `image`, `submenu`,
`separator`, `check`; the lower-level `Row`, `Segment`, `StyleRun`; the style
primitives `Align`, `Flex`, `Color`, `Rgba`, `Font`, `FontFamily`, `Weight`, `Icon`;
the accelerator model `Accelerator`, `Modifiers`, `Code` (1.0-new, [`40`](40-input-interaction.md));
theming `Theme`, `ThemeSource`, `MenuOptions`, `Insets`; geometry `Edge`,
`LogicalPoint`, `LogicalSize`, `LogicalRect`; events `MenuId`, `MenuEvent`,
`MenuEventReceiver`; errors `Error`, `Unsupported`, `Result`; plus the `compat`
module (doc 02) and the lower-level `a11y` / `keynav` / `anchor` / `flyout` /
`layout` / `render` modules (exposed for advanced integration and testing,
semver-covered but lower-churn-risk-documented in doc 03).

Nothing consumer-specific leaks in: no usagio id grammar, no `switch:`/`capture:`
strings, no snapshot types. muri knows only `Menu` and opaque `MenuId`s.
