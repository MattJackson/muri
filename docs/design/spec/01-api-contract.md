# 01 — API contract: muri's native, styleable public API

Status: **foundational spec.** This document specifies muri's **own** public API —
the one a consumer reaches past the facade to unlock full styling. The muda
compatibility facade is [`02-muda-compat.md`](02-muda-compat.md); threading and
event semantics are [`03-threading-events-versioning.md`](03-threading-events-versioning.md).

Signatures below are grounded in the shipped crate. Where a type is **shipped** it
is quoted as-is; where 1.0 adds or changes something it is marked **1.0-new** or
**1.0-change** with the rationale.

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

## 2. The declarative menu tree

All types in `muri::menu`, re-exported at the crate root. **Shipped** unless noted.

```rust
pub struct Menu { pub items: Vec<Item> }

impl Menu {
    pub fn new() -> Self;
    pub fn item(self, item: Item) -> Self;
    pub fn row(self, row: Row) -> Self;
    pub fn separator(self) -> Self;
    pub fn section_header(self, row: Row) -> Self;
    pub fn submenu(self, label: Row, menu: Menu) -> Self;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn interactive_count(&self) -> usize;   // focusable items at this level
}

pub enum Item {
    Row(Row),
    Separator,
    SectionHeader(Row),                          // styled, non-interactive heading
    Submenu { label: Row, menu: Menu },          // opens a flyout panel beside it
}

impl Item {
    pub fn is_interactive(&self) -> bool;        // rows/submenus with a real id & enabled
}
```

`Menu` is `Default` (empty). `interactive_count` and `is_interactive` are the
single source of truth for "what can be focused/clicked" and are consumed by both
`keynav` and `a11y` — a separator, a section header, a disabled row, and an inert
info row (`MenuId::none()`) are all non-interactive.

### Row

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
- **Honest caveat:** a CPU raster surface cannot reproduce macOS menu **vibrancy /
  translucency** (the blurred backdrop) — that is a compositor effect over a
  transparent `NSVisualEffectView`. `Theme::native()` yields an *opaque* menu of
  the right color, not a vibrant one. This is a documented, permanent divergence
  and is called out in [`02-muda-compat.md`](02-muda-compat.md) and
  [`60-migration-guide.md`](60-migration-guide.md). The alternative (host the
  raster layer over a native vibrancy view on macOS) is a possible future macOS-only
  enhancement, not a 1.0 promise.

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

muri's surfaces are the objects that put a `Menu` on screen. All own an optional
`on_click` closure and expose `accessibility_tree()`.

### 5.1 `Tray` (shipped)

Owns the tray icon + the tray-anchored popup + anchoring. Subsumes `tray-icon`.

```rust
pub struct Tray { /* icon, menu, tooltip, options, on_click */ }
impl Tray {
    pub fn new(icon: Icon) -> Self;
    pub fn menu(self, menu: Menu) -> Self;
    pub fn tooltip(self, text: impl Into<String>) -> Self;
    pub fn options(self, options: MenuOptions) -> Self;
    pub fn theme(self, theme: ThemeSource) -> Self;             // convenience for options.theme
    pub fn on_click(self, handler: impl Fn(&MenuId) + Send + 'static) -> Self;
    pub fn set_menu(&mut self, menu: Menu);                     // swap content at runtime
    pub fn accessibility_tree(&self) -> AxTree;
    pub fn dispatch(&self, id: &MenuId);                        // testable click path
    pub fn open(&self) -> Result<()>;                           // 1.0-new (todo! today)
    pub fn close(&self);                                        // 1.0-new (todo! today)
    pub fn run(self) -> Result<()>;   // install icon + run event loop; consumes self
}
```

Behavior contract:

- **`run` consumes `self` and blocks** running the platform event loop. On macOS
  it must be called on the main thread (`MainThreadMarker`); it sets the app to
  `Accessory` activation policy (no Dock icon) — **note the menu-bar-app conflict**
  flagged in [`20-platform-macos.md`](20-platform-macos.md): an app that *also*
  wants a native menu bar cannot be `Accessory`. The facade must reconcile.
- On **Linux**, `run` returns `Err(Unsupported::TrayAnchor)`.
- **`set_menu(&mut self, menu)`** swaps content cheaply; the next open re-renders.
  This is the runtime-update path (usagio rebuilds on a tick). **Design tension:**
  `run` consumes `self`, so after `run` the consumer no longer holds the `Tray` to
  call `set_menu`. 1.0 must provide a `TrayHandle` (a `Clone + Send` handle
  returned before/by `run`, or via a builder that splits handle from loop) so the
  menu can be updated *while the loop runs*. This is a **1.0-new, load-bearing**
  requirement for any live app and is under-specified in the shipped skeleton;
  see [`03-threading-events-versioning.md`](03-threading-events-versioning.md) §2.
- **`open` / `close`** are `todo!()` today; 1.0 implements programmatic show/hide.

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

## 6. Event delivery

Two mechanisms, both delivering the activated row's `MenuId`.

1. **Per-surface closure (shipped):** `on_click(impl Fn(&MenuId) + Send +
   'static)`. Invoked on the UI thread when a row is activated by mouse, keyboard
   (`NavAction::Activate`), or an AccessKit `Click` action. Inert rows
   (`MenuId::none()`) never fire.
2. **Global channel (1.0-new, muda parity):** a process-global
   `muri::MenuEvent::receiver() -> &'static MenuEventReceiver` delivering
   `MenuEvent { id: MenuId }`. Required by the facade (decision #1). muri's core
   fires *both*: the closure (if set) and the global channel. See
   [`03-threading-events-versioning.md`](03-threading-events-versioning.md) §3 for
   the ordering and threading contract.

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
- **`on_click` is `Box<dyn Fn(&MenuId) + Send + 'static>`** — owned by the surface,
  `Send` so it can be moved to the UI thread, `'static` so it outlives
  construction. It is **not** `Sync` and **not** `FnMut`; a handler needing mutable
  shared state uses interior mutability (`Arc<Mutex<_>>` / atomics), as the shipped
  tests do.
- **Surfaces are not `Send`/`Sync`** once running — they own platform windows and
  must live on the UI thread. Runtime interaction from other threads goes through
  the global `MenuEvent` channel (read side) and the 1.0-new `TrayHandle` (write
  side), never by moving the surface.
- **The a11y tree (`AxTree`) is a snapshot**, rebuilt from the menu on demand;
  it borrows nothing and is safe to build off the UI thread for testing.

## 9. What the public surface is (and is not)

The complete 1.0 public export set: `Tray`, `ContextMenu`, `Popup`, `Menu`,
`Item`, `Row`, `Segment`, `StyleRun`, `Align`, `Flex`, `Color`, `Rgba`, `Font`,
`FontFamily`, `Weight`, `Icon`, `Theme`, `ThemeSource`, `MenuOptions`, `Insets`,
`Edge`, `LogicalPoint`, `LogicalSize`, `LogicalRect`, `MenuId`, `MenuEvent`,
`MenuEventReceiver`, `Error`, `Unsupported`, `Result`, plus the `compat` module
(doc 02) and the lower-level `a11y` / `keynav` / `anchor` / `flyout` / `layout` /
`render` modules (exposed for advanced integration and testing, semver-covered but
lower-churn-risk-documented in doc 03).

Nothing consumer-specific leaks in: no usagio id grammar, no `switch:`/`capture:`
strings, no snapshot types. muri knows only `Menu` and opaque `MenuId`s.
