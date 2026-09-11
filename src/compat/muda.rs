//! The `muri::compat::muda` facade — muda's public API surface mapped onto
//! muri's native model (spec `02`).
//!
//! Type names, module layout, and builder shapes mirror muda so an unchanged app
//! compiles after only redirecting its imports (spec `60` §3). Each item type
//! translates to a muri [`Item`](crate::menu::Item)/[`Row`](crate::menu::Row);
//! `init_for_*` tags the menu **native-passthrough** while
//! `show_context_menu_for_*` / a tray builder route it to a muri **custom**
//! surface (the routing table in spec `02` §2).
//!
//! ### Divergences from a byte-for-byte muda drop-in (documented, spec `02` §9)
//!
//! - The platform surface methods (`init_for_nsapp`, `init_for_hwnd`,
//!   `show_context_menu_for_*`, …) are exposed on **every** target rather than
//!   `#[cfg(target_os)]`-gated as muda gates them. muri routes all `target_os`
//!   branching through its `Platform` seam (ADR-0002 / the `strict_cfg` test), so
//!   the facade cannot scatter target gates; the extra methods are harmless.
//! - `Position` is a small facade type, not the real `dpi::Position`; only the
//!   `None` (current-cursor) form is exercised by the worked migration (spec
//!   `02` §6). This is best-effort until muri pins muda's `dpi` version.
//! - `IconMenuItem` built from raw RGBA renders its leading icon in the muri
//!   custom surface: the RGBA is encoded to PNG
//!   ([`render::encode_rgba_png`](crate::render)) and carried as an
//!   [`Icon::Png`](crate::menu::Icon::Png) — the same bridge the tray icon uses
//!   (issue #9). A PNG-backed icon ([`Icon::from_png`]) keeps its encoded bytes
//!   and uses the native `Icon::Png` path directly (no decode/re-encode; #24). A
//!   `NativeIcon` maps to
//!   [`Icon::Symbol`](crate::menu::Icon::Symbol).
//! - `Accelerator` is displayed only (spec `02` §5, divergence D5); the facade
//!   parses a useful subset of muda's `Code`/`Modifiers`.

// The ONLY `unsafe` in this facade is the `unsafe fn` *signature* on the Windows
// (`init_for_hwnd`, `show_context_menu_for_hwnd`) and macOS
// (`show_context_menu_for_nsview`) surface methods, preserved verbatim so muda
// callers that wrap them in `unsafe { … }` compile unchanged (spec `02` §6). No
// `unsafe` *operations* are performed anywhere in this module — it is entirely
// safe code. The crate-wide `#![deny(unsafe_code)]` stays intact; this localized
// allow only permits the parity markers.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::menu::{
    Align, Flex, Item as MuriItem, Menu as MuriMenu, Row as MuriRow, Segment as MuriSegment,
};

pub use crate::event::MenuEventReceiver;
pub use crate::menu::{Icon as MuriIcon, MenuEvent, MenuId};
// Re-exported so callers can style rows without reaching outside the compat
// module: a value color (#19), per-span value runs (#25), and their weight.
pub use crate::menu::StyleRun;
pub use crate::style::{Color, Weight};

pub mod about_metadata;
pub mod accelerator;

use about_metadata::AboutMetadata;
use accelerator::Accelerator;

// =============================================================================
// Errors (spec 02 §7) — a muda-compatible superset so a migrated `match` compiles
// =============================================================================

/// The facade error type: a superset of muda's variants a consumer might
/// `match` on, plus muri's own. For custom surfaces the facade only ever
/// produces [`Error::BadIcon`], [`Error::Unsupported`], and [`Error::Platform`];
/// the muda-specific variants are present so a migrated `match` type-checks even
/// though muri never returns them from a custom surface (spec `02` §7).
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The item is not a child of the menu it was removed from.
    NotAChildOfThisMenu,
    /// A menu that can only be initialized once was initialized twice.
    AlreadyInitialized,
    /// An accelerator string could not be parsed.
    AcceleratorParse(String),
    /// The supplied icon bytes/dimensions were invalid.
    BadIcon(String),
    /// A capability unavailable on the current platform.
    Unsupported(crate::Unsupported),
    /// A platform API call failed (tray install, main-thread requirement, thread
    /// spawn, or a residual per-OS API error — the message names which). The
    /// tray-install handshake surfaces a Windows/Linux install failure here via
    /// [`TrayIconBuilder::build_result`](super::tray_icon::TrayIconBuilder::build_result).
    Platform(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotAChildOfThisMenu => f.write_str("item is not a child of this menu"),
            Error::AlreadyInitialized => f.write_str("menu has already been initialized"),
            Error::AcceleratorParse(s) => write!(f, "failed to parse accelerator: {s}"),
            Error::BadIcon(s) => write!(f, "bad icon: {s}"),
            Error::Unsupported(u) => write!(f, "unsupported on this platform: {u}"),
            Error::Platform(s) => write!(f, "platform error: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// Map muri's core [`crate::Error`] onto the facade error. The concrete failure
/// site (tray install, main thread, thread spawn) is preserved in the
/// [`Error::Platform`] message; [`Error::Unsupported`] / [`Error::BadIcon`] map
/// across unchanged.
impl From<crate::Error> for Error {
    fn from(e: crate::Error) -> Self {
        match e {
            crate::Error::Unsupported(u) => Error::Unsupported(u),
            crate::Error::BadIcon(s) => Error::BadIcon(s),
            crate::Error::Platform(s) => Error::Platform(s),
        }
    }
}

/// The facade result alias, mirroring `muda::Result`.
pub type Result<T> = std::result::Result<T, Error>;

/// An invalid-icon error, mirroring `muda`/`tray-icon`'s `BadIcon`.
#[derive(Debug)]
pub struct BadIcon(pub String);

impl std::fmt::Display for BadIcon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bad icon: {}", self.0)
    }
}

impl std::error::Error for BadIcon {}

// =============================================================================
// MenuId auto-generation (spec 02 §3) — muri's own opaque id sequence
// =============================================================================

/// Generate the next auto id: a process-global atomic counter starting at 1,
/// stringified (`"1"`, `"2"`, `"3"`, …).
///
/// **Documented divergence — ids are NOT bit-identical to muda's.** This is
/// muri's *own* opaque sequence, not a reproduction of muda's numeric values.
/// Real muda 0.19.3 starts its Windows counter at 1000 and, on macOS/GTK,
/// consumes the counter *twice* per `Menu`/`Submenu` (once for the id, once for
/// an internal handle tag), so muda's auto-id stream skips numbers; muri's clean
/// `1, 2, 3, …` deliberately diverges rather than mimic that fragile internal
/// double-counter. Apps must treat auto-generated ids as opaque and compare
/// against [`IsMenuItem::id`] / the item's own `id()` — never against hardcoded
/// muda numbers. The only contract the facade guarantees is that auto ids are
/// **unique and strictly increasing** within a process.
fn next_auto_id() -> MenuId {
    static COUNTER: AtomicU32 = AtomicU32::new(1);
    MenuId(COUNTER.fetch_add(1, Ordering::Relaxed).to_string())
}

fn resolve_id(id: Option<MenuId>) -> MenuId {
    id.unwrap_or_else(next_auto_id)
}

// =============================================================================
// Icons (spec 02 §4.5, §4.7)
// =============================================================================

/// A menu/tray icon built from **raw RGBA** ([`from_rgba`](Icon::from_rgba)) or
/// **encoded PNG** ([`from_png`](Icon::from_png)) bytes, mirroring muda/`tray-icon`'s
/// `Icon`. Raw RGBA is encoded to PNG
/// ([`render::encode_rgba_png`](crate::render)) and carried as an
/// [`Icon::Png`](crate::menu::Icon::Png) (issue #9); PNG bytes keep their encoded
/// representation and use the native `Icon::Png` path directly, with no
/// decode/re-encode cycle (#24). Both feed the tray icon and menu-item leading
/// icons.
#[derive(Clone, Debug)]
pub struct Icon {
    /// Straight-alpha RGBA pixels, row-major, 4 bytes per pixel. Empty (and
    /// meaningless) for a PNG-backed icon ([`Icon::from_png`]) — use
    /// [`Icon::data`] to introspect which representation this icon actually
    /// holds rather than reading this field directly (issue #53).
    ///
    /// `pub(crate)` rather than fully `pub`: the sibling `tray_icon` compat
    /// module still reads it directly (and the field-level shape is not part
    /// of the facade's *external* contract), but an external caller must go
    /// through [`Icon::data`], which is honest about the PNG case.
    pub(crate) rgba: Vec<u8>,
    /// Pixel width. `0` (and meaningless) for a PNG-backed icon; see the note
    /// on [`rgba`](Icon::rgba) above.
    pub(crate) width: u32,
    /// Pixel height. `0` (and meaningless) for a PNG-backed icon; see the note
    /// on [`rgba`](Icon::rgba) above.
    pub(crate) height: u32,
    /// Encoded PNG bytes, when this icon was built with [`Icon::from_png`].
    ///
    /// This is kept private so callers continue to use the compatibility
    /// constructors rather than depending on the storage representation.
    png: Option<Arc<[u8]>>,
}

/// The icon's actual underlying representation (issue #53), returned by
/// [`Icon::data`]. `Icon`'s `rgba`/`width`/`height` fields read as
/// empty/zero for a PNG-backed icon ([`Icon::from_png`]) — this is the honest
/// accessor a caller should use to tell the two representations apart rather
/// than trusting those fields unconditionally.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum IconData<'a> {
    /// Straight-alpha RGBA pixels, row-major, 4 bytes per pixel.
    Rgba {
        /// Pixel width.
        width: u32,
        /// Pixel height.
        height: u32,
        /// The raw pixel bytes (`width * height * 4` long).
        pixels: &'a [u8],
    },
    /// Encoded PNG bytes, as given to [`Icon::from_png`].
    Png(&'a [u8]),
}

impl Icon {
    /// Build an icon from raw RGBA bytes, erroring if the length does not match
    /// `width * height * 4` (mirrors muda's `Icon::from_rgba`).
    pub fn from_rgba(rgba: Vec<u8>, width: u32, height: u32) -> std::result::Result<Self, BadIcon> {
        // Reject zero-area icons up front: `0x0` would otherwise validate (its
        // expected length is 0), then be silently dropped downstream because the
        // encoder (`render::encode_rgba_png`) rejects a zero dimension — a
        // degenerate icon that reports success at every step yet never draws.
        if width == 0 || height == 0 {
            return Err(BadIcon(format!(
                "icon dimensions must be non-zero, got {width}x{height}"
            )));
        }
        // Checked arithmetic: `width`/`height` may come from untrusted image
        // metadata, and `w * h * 4` overflows `usize` for pathological dimensions
        // (e.g. u32::MAX x u32::MAX) — which panics under the default debug
        // overflow checks *before* the length guard runs, and silently wraps to a
        // wrong `expected` in release. Overflow is itself a rejected icon.
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4));
        match expected {
            Some(expected) if rgba.len() == expected => Ok(Icon {
                rgba,
                width,
                height,
                png: None,
            }),
            Some(expected) => Err(BadIcon(format!(
                "expected {expected} bytes for {width}x{height} RGBA, got {}",
                rgba.len()
            ))),
            None => Err(BadIcon(format!(
                "icon dimensions {width}x{height} overflow the addressable buffer size"
            ))),
        }
    }

    /// Build an icon from encoded PNG bytes (issue #24).
    ///
    /// The bytes are retained **without decoding** and used for muri's native
    /// [`Icon::Png`](crate::menu::Icon::Png) path — no decode-into-RGBA then
    /// re-encode cycle. This is the muda/`tray-icon` `Icon::from_path`-style
    /// convenience for callers that already hold PNG asset bytes.
    pub fn from_png(bytes: &[u8]) -> std::result::Result<Self, BadIcon> {
        if bytes.is_empty() {
            return Err(BadIcon("PNG icon bytes are empty".to_owned()));
        }
        Ok(Icon {
            rgba: Vec::new(),
            width: 0,
            height: 0,
            png: Some(Arc::from(bytes)),
        })
    }

    /// The icon's actual underlying representation (issue #53): `Png` for an
    /// icon built with [`Icon::from_png`], `Rgba` for one built with
    /// [`Icon::from_rgba`]. The honest introspection accessor — the `rgba`/
    /// `width`/`height` fields read as empty/zero for a PNG-backed icon, so a
    /// caller that wants to know which representation it is holding (rather
    /// than assuming raw RGBA) should match on this instead.
    pub fn data(&self) -> IconData<'_> {
        match &self.png {
            Some(png) => IconData::Png(png),
            None => IconData::Rgba {
                width: self.width,
                height: self.height,
                pixels: &self.rgba,
            },
        }
    }
}

/// An OS stock icon usable on an [`IconMenuItem`], mirroring muda's `NativeIcon`.
/// In a muri custom surface each maps to [`Icon::Symbol`](crate::menu::Icon::Symbol)
/// (divergence D6: symbol drawing is 1.0-new, spec `02` §4.7). Only a
/// representative subset of muda's stock icons is modeled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NativeIcon {
    /// A user/person icon.
    User,
    /// A caution / warning icon.
    Caution,
    /// A general information icon.
    Info,
    /// A computer icon.
    Computer,
    /// A folder icon.
    Folder,
    /// A generic status-available icon.
    StatusAvailable,
    /// A generic status-unavailable icon.
    StatusUnavailable,
}

impl NativeIcon {
    /// The symbol name this stock icon maps to in a muri custom surface.
    fn symbol_name(&self) -> &'static str {
        match self {
            NativeIcon::User => "person",
            NativeIcon::Caution => "exclamationmark.triangle",
            NativeIcon::Info => "info.circle",
            NativeIcon::Computer => "desktopcomputer",
            NativeIcon::Folder => "folder",
            NativeIcon::StatusAvailable => "circle.fill",
            NativeIcon::StatusUnavailable => "circle",
        }
    }
}

// =============================================================================
// The item trait + owned-kind enum (spec 02 §4.8)
// =============================================================================

/// muda's object-safe item supertype: implemented by every facade item type so
/// [`Menu::append`]`(&dyn IsMenuItem)` type-checks unchanged.
pub trait IsMenuItem {
    /// This item's owned kind (used by [`Menu::items`]).
    fn kind(&self) -> MenuItemKind;
    /// This item's id.
    fn id(&self) -> MenuId;
}

/// The owned enum returned by [`Menu::items`] / [`Submenu::items`], mirroring
/// muda's `MenuItemKind`.
#[derive(Clone)]
#[non_exhaustive]
pub enum MenuItemKind {
    /// A plain text item.
    MenuItem(MenuItem),
    /// A nested submenu.
    Submenu(Submenu),
    /// A predefined OS-action / separator item.
    Predefined(PredefinedMenuItem),
    /// A checkable item.
    Check(CheckMenuItem),
    /// An item with a leading icon.
    Icon(IconMenuItem),
}

impl MenuItemKind {
    /// This item's id.
    pub fn id(&self) -> MenuId {
        match self {
            MenuItemKind::MenuItem(i) => i.id(),
            MenuItemKind::Submenu(i) => i.id(),
            MenuItemKind::Predefined(i) => i.id(),
            MenuItemKind::Check(i) => i.id(),
            MenuItemKind::Icon(i) => i.id(),
        }
    }

    /// Translate this item to a muri [`Item`](crate::menu::Item).
    fn to_muri(&self) -> MuriItem {
        match self {
            MenuItemKind::MenuItem(i) => {
                let s = i.inner.borrow();
                MuriItem::Row(apply_styled_label(
                    MuriRow::new(s.id.clone()).enabled(s.enabled),
                    &s.text,
                    &s.style,
                ))
            }
            MenuItemKind::Check(i) => {
                let s = i.inner.borrow();
                // The checkbox state drives the leading checkmark (#12); the muri
                // style adds bold + value coloring on top (#28) without forcing a
                // second checkmark.
                let mut style = s.style.clone();
                style.checkmark = style.checkmark || s.checked;
                MuriItem::Row(apply_styled_label(
                    MuriRow::new(s.id.clone()).enabled(s.enabled),
                    &s.text,
                    &style,
                ))
            }
            MenuItemKind::Icon(i) => {
                let s = i.inner.borrow();
                let mut row = apply_styled_label(
                    MuriRow::new(s.id.clone()).enabled(s.enabled),
                    &s.text,
                    &s.style,
                );
                match &s.icon {
                    // A PNG-backed icon uses its encoded bytes directly (#24); a
                    // raw-RGBA icon is encoded to PNG and rendered as the row's
                    // leading image — the same bridge the tray icon uses
                    // (`render::encode_rgba_png` → `Icon::Png`), so a provider logo
                    // on a header row draws (issue #9).
                    Some(IconSource::Rgba(icon)) => {
                        if let Some(png) = &icon.png {
                            row = row.leading(MuriIcon::Png(png.clone()));
                        } else if let Some(png) =
                            // Cached encode: `to_muri` re-runs on every `set_menu`,
                            // so encoding an unchanged logo each tick would waste
                            // CPU and defeat the render decode cache. The cache
                            // returns a stable Arc for identical RGBA.
                            super::encode_rgba_cached(
                                &icon.rgba,
                                icon.width,
                                icon.height,
                            )
                        {
                            row = row.leading(MuriIcon::Png(png));
                        }
                    }
                    // A stock NativeIcon maps to a named symbol.
                    Some(IconSource::Native(native)) => {
                        row = row.leading(MuriIcon::Symbol(native.symbol_name()));
                    }
                    None => {}
                }
                MuriItem::Row(row)
            }
            MenuItemKind::Submenu(i) => {
                let s = i.inner.borrow();
                let label = apply_styled_label(
                    MuriRow::new(s.id.clone()).enabled(s.enabled),
                    &s.text,
                    &s.style,
                );
                MuriItem::Submenu {
                    label,
                    menu: kinds_to_muri_menu(&s.items),
                }
            }
            MenuItemKind::Predefined(i) => {
                let s = i.inner.borrow();
                match &s.predefined {
                    // A separator is exact (spec 02 §4.6).
                    Predefined::Separator => MuriItem::Separator,
                    // Every other predefined item renders as a VISIBLE, DISABLED
                    // row in a custom surface — never silently dropped (the
                    // honesty rule, spec 02 §4.6).
                    _ => MuriItem::Row(
                        MuriRow::new(s.id.clone())
                            .label(s.text.clone())
                            .enabled(false),
                    ),
                }
            }
        }
    }
}

fn kinds_to_muri_menu(items: &[MenuItemKind]) -> MuriMenu {
    let mut menu = MuriMenu::new();
    for kind in items {
        menu.items.push(kind.to_muri());
    }
    menu
}

/// Apply a muda item's label text to a muri row, honoring a TAB tab-stop so the
/// native `label\tvalue` two-column layout is preserved (#12): the text before
/// the first TAB grows to fill the row and the trailing text is flush-right,
/// matching muda's NSMenu tab-stop column. Text without a TAB is a single label.
/// The muri-compat row styling extensions, applied by [`apply_styled_label`].
/// All fields are additive muri-only extensions (not muda's API); defaults are
/// inert so an unstyled row renders exactly as before.
#[derive(Clone, Default)]
struct RowStyle {
    /// A leading checkmark (`set_active`, #18; or a `CheckMenuItem`'s checkbox, #28).
    checkmark: bool,
    /// Bold text (`set_active` bundles it with the checkmark; `set_bold` sets it
    /// alone, #26).
    bold: bool,
    /// #19: a single color for the trailing `\t` value segment.
    value_color: Option<Color>,
    /// #25: per-span colors/weights on the value segment (UTF-16 ranges into the
    /// post-`\t` text). Takes precedence over `value_color` when non-empty.
    value_runs: Vec<StyleRun>,
    /// #27: a leading image icon (used by `Submenu`, whose muda type has no icon
    /// slot); encoded/routed like an `IconMenuItem`.
    icon: Option<Icon>,
}

/// [`apply_label`] plus [`RowStyle`]: checkmark + bold ([`active`](RowStyle::active)),
/// bold-only ([`bold`](RowStyle::bold), #26), a single ([`value_color`](RowStyle::value_color),
/// #19) or per-span ([`value_runs`](RowStyle::value_runs), #25) value color, and an
/// optional leading [`icon`](RowStyle::icon) (#27). Bold is a whole-segment
/// [`StyleRun`] weight, preserving the OS point size.
fn apply_styled_label(row: MuriRow, text: &str, style: &RowStyle) -> MuriRow {
    let checkmark = style.checkmark;
    let bold = style.bold;
    let mut row = row;
    // Leading slot: an explicit image icon wins; else the active checkmark.
    if let Some(icon) = &style.icon {
        if let Some(png) = &icon.png {
            row = row.leading(MuriIcon::Png(png.clone()));
        } else if let Some(png) = super::encode_rgba_cached(&icon.rgba, icon.width, icon.height) {
            row = row.leading(MuriIcon::Png(png));
        }
    } else if checkmark {
        row = row.leading(MuriIcon::Checkmark);
    }
    if checkmark {
        row = row.checked(true);
    }
    // Bold a segment across its whole text without changing size/family.
    let bold_seg = |seg: MuriSegment, s: &str, color: Color| {
        let len = s.encode_utf16().count();
        seg.runs(vec![StyleRun::new(0, len, color).weight(Weight::Bold)])
    };
    // Apply value styling to the trailing segment: per-span runs (#25) win; else
    // a single color; bold is layered onto whichever applies.
    let style_value = |seg: MuriSegment, s: &str| -> MuriSegment {
        if !style.value_runs.is_empty() {
            let mut runs = style.value_runs.clone();
            if bold {
                for r in &mut runs {
                    if r.weight.is_none() {
                        r.weight = Some(Weight::Bold);
                    }
                }
            }
            seg.runs(runs)
        } else if bold {
            bold_seg(seg, s, style.value_color.unwrap_or(Color::Label))
        } else if let Some(c) = style.value_color {
            seg.color(c)
        } else {
            seg
        }
    };
    match text.split_once('\t') {
        Some((lead, tail)) => {
            let lead = lead.trim_end();
            let tail = tail.trim_start();
            let mut lead_seg = MuriSegment::new(lead).flex(Flex::Grow);
            if bold {
                lead_seg = bold_seg(lead_seg, lead, Color::Label);
            }
            let tail_seg = style_value(MuriSegment::new(tail).align(Align::Right), tail);
            row.segments(vec![lead_seg, tail_seg])
        }
        None => {
            let mut seg = MuriSegment::new(text.to_owned());
            if bold {
                seg = bold_seg(seg, text, Color::Label);
            }
            row.segments(vec![seg])
        }
    }
}

/// Generates the muri-compat row-styling setters for an item type whose interior
/// `inner: Rc<RefCell<_>>` state carries a `style: RowStyle` field (#18/#19/#25/#26).
macro_rules! row_style_setters {
    () => {
        /// **muri extension (#18):** mark this row *active* — bold text with a
        /// leading checkmark (the 0.5.x active-account look). Not muda's API.
        pub fn set_active(&self, active: bool) {
            let mut st = self.inner.borrow_mut();
            st.style.checkmark = active;
            st.style.bold = active;
        }
        /// **muri extension (#26):** render this row **bold** without a checkmark
        /// (e.g. a bold header row). Not muda's API.
        pub fn set_bold(&self, bold: bool) {
            self.inner.borrow_mut().style.bold = bold;
        }
        /// **muri extension (#19):** color the trailing `\t` value segment (e.g.
        /// [`Color::SystemRed`]); pass `None` to clear. Not muda's API.
        pub fn set_value_color(&self, color: Option<Color>) {
            self.inner.borrow_mut().style.value_color = color;
        }
        /// **muri extension (#25):** per-span colors/weights on the trailing `\t`
        /// value ([`StyleRun`] ranges are UTF-16 offsets into the value text);
        /// takes precedence over [`set_value_color`](Self::set_value_color).
        pub fn set_value_runs(&self, runs: Vec<StyleRun>) {
            self.inner.borrow_mut().style.value_runs = runs;
        }
    };
}

// =============================================================================
// MuriRowExt (issue #53) — the muri-only row-styling extensions, grouped
// =============================================================================

/// **muri extensions**, grouped behind one trait for discoverability (issue
/// #53): `set_active`/`set_bold`/`set_value_color`/`set_value_runs` (generated
/// inline on each item type by `row_style_setters!`) plus [`Submenu`]'s
/// `set_icon` (which has no equivalent on the other item types — they either
/// have their own icon API, e.g. [`IconMenuItem::set_icon`], or none). None of
/// this is muda's API; it exists so a consumer can `use
/// muri::compat::muda::MuriRowExt` and find every muri-only row knob in one
/// place, and so generic code can be written against `&dyn MuriRowExt` /
/// `impl MuriRowExt` bounds.
///
/// The inherent methods of the same name (from `row_style_setters!`) remain
/// available on each concrete type and are what method-call syntax resolves to
/// (inherent methods take priority over trait methods at the same name); this
/// trait's methods delegate to them, so behavior is identical either way.
///
/// [`set_icon`](MuriRowExt::set_icon) has a no-op default: only [`Submenu`]
/// overrides it. The other item types don't grow a `MuriRowExt::set_icon`
/// override because they already have their own, differently-shaped icon
/// setter (e.g. [`IconMenuItem::set_icon`]) that this trait does not shadow.
pub trait MuriRowExt {
    /// Mark this row *active*: bold text with a leading checkmark (#18).
    fn set_active(&self, active: bool);
    /// Render this row bold, without a checkmark (#26).
    fn set_bold(&self, bold: bool);
    /// Color the trailing `\t` value segment; `None` clears it (#19).
    fn set_value_color(&self, color: Option<Color>);
    /// Per-span colors/weights on the trailing `\t` value (#25).
    fn set_value_runs(&self, runs: Vec<StyleRun>);
    /// Give this row a leading image icon. Only [`Submenu`] overrides this
    /// (#27); every other implementer keeps the default no-op.
    fn set_icon(&self, _icon: Option<Icon>) {}
}

/// Implement [`MuriRowExt`] for a type carrying the `row_style_setters!`
/// inherent methods, by delegating to them.
macro_rules! impl_muri_row_ext {
    ($ty:ty) => {
        impl MuriRowExt for $ty {
            fn set_active(&self, active: bool) {
                <$ty>::set_active(self, active);
            }
            fn set_bold(&self, bold: bool) {
                <$ty>::set_bold(self, bold);
            }
            fn set_value_color(&self, color: Option<Color>) {
                <$ty>::set_value_color(self, color);
            }
            fn set_value_runs(&self, runs: Vec<StyleRun>) {
                <$ty>::set_value_runs(self, runs);
            }
        }
    };
}

// =============================================================================
// MenuItem (spec 02 §4.3)
// =============================================================================

struct MenuItemState {
    id: MenuId,
    text: String,
    enabled: bool,
    #[allow(dead_code)] // displayed by the backend (D5); stored for parity.
    accelerator: Option<Accelerator>,
    /// muri styling extensions (#18/#19/#25/#26/#27); see [`RowStyle`].
    style: RowStyle,
}

/// A plain text menu item (muda's `MenuItem`). Cloning shares the same item (like
/// muda, which wraps interior state), so post-construction `set_*` mutations are
/// observed through every clone.
#[derive(Clone)]
pub struct MenuItem {
    inner: Rc<RefCell<MenuItemState>>,
}

impl MenuItem {
    /// A new item with an auto-generated id (spec `02` §3).
    pub fn new(text: impl AsRef<str>, enabled: bool, accelerator: Option<Accelerator>) -> Self {
        Self::build(None, text, enabled, accelerator)
    }

    /// A new item with an explicit id.
    pub fn with_id(
        id: impl Into<MenuId>,
        text: impl AsRef<str>,
        enabled: bool,
        accelerator: Option<Accelerator>,
    ) -> Self {
        Self::build(Some(id.into()), text, enabled, accelerator)
    }

    fn build(
        id: Option<MenuId>,
        text: impl AsRef<str>,
        enabled: bool,
        accelerator: Option<Accelerator>,
    ) -> Self {
        MenuItem {
            inner: Rc::new(RefCell::new(MenuItemState {
                id: resolve_id(id),
                text: text.as_ref().to_owned(),
                enabled,
                accelerator,
                style: RowStyle::default(),
            })),
        }
    }

    row_style_setters!();

    /// This item's id.
    pub fn id(&self) -> MenuId {
        self.inner.borrow().id.clone()
    }

    /// This item's current text.
    pub fn text(&self) -> String {
        self.inner.borrow().text.clone()
    }

    /// Replace the item's text (takes `&self`, as muda does).
    pub fn set_text(&self, text: impl AsRef<str>) {
        self.inner.borrow_mut().text = text.as_ref().to_owned();
    }

    /// Whether the item is enabled.
    pub fn is_enabled(&self) -> bool {
        self.inner.borrow().enabled
    }

    /// Enable or disable the item.
    pub fn set_enabled(&self, enabled: bool) {
        self.inner.borrow_mut().enabled = enabled;
    }

    /// Replace the item's accelerator (displayed only in a custom surface, D5).
    pub fn set_accelerator(&self, accelerator: Option<Accelerator>) {
        self.inner.borrow_mut().accelerator = accelerator;
    }
}

impl IsMenuItem for MenuItem {
    fn kind(&self) -> MenuItemKind {
        MenuItemKind::MenuItem(self.clone())
    }
    fn id(&self) -> MenuId {
        MenuItem::id(self)
    }
}

impl_muri_row_ext!(MenuItem);

// =============================================================================
// CheckMenuItem (spec 02 §4.4)
// =============================================================================

struct CheckMenuItemState {
    id: MenuId,
    text: String,
    enabled: bool,
    checked: bool,
    #[allow(dead_code)]
    accelerator: Option<Accelerator>,
    /// muri styling extensions (#28: the same set_bold/value coloring the other
    /// item types have); the leading checkmark itself comes from `checked`.
    style: RowStyle,
}

/// A checkable menu item (muda's `CheckMenuItem`). Maps exactly to a muri
/// [`Row`](crate::menu::Row) with a check column (spec `02` §4.4).
#[derive(Clone)]
pub struct CheckMenuItem {
    inner: Rc<RefCell<CheckMenuItemState>>,
}

impl CheckMenuItem {
    /// A new checkable item with an auto-generated id.
    pub fn new(
        text: impl AsRef<str>,
        enabled: bool,
        checked: bool,
        accelerator: Option<Accelerator>,
    ) -> Self {
        Self::build(None, text, enabled, checked, accelerator)
    }

    /// A new checkable item with an explicit id.
    pub fn with_id(
        id: impl Into<MenuId>,
        text: impl AsRef<str>,
        enabled: bool,
        checked: bool,
        accelerator: Option<Accelerator>,
    ) -> Self {
        Self::build(Some(id.into()), text, enabled, checked, accelerator)
    }

    fn build(
        id: Option<MenuId>,
        text: impl AsRef<str>,
        enabled: bool,
        checked: bool,
        accelerator: Option<Accelerator>,
    ) -> Self {
        CheckMenuItem {
            inner: Rc::new(RefCell::new(CheckMenuItemState {
                id: resolve_id(id),
                text: text.as_ref().to_owned(),
                enabled,
                checked,
                accelerator,
                style: RowStyle::default(),
            })),
        }
    }

    row_style_setters!();

    /// This item's id.
    pub fn id(&self) -> MenuId {
        self.inner.borrow().id.clone()
    }

    /// This item's current text.
    pub fn text(&self) -> String {
        self.inner.borrow().text.clone()
    }

    /// Replace the item's text.
    pub fn set_text(&self, text: impl AsRef<str>) {
        self.inner.borrow_mut().text = text.as_ref().to_owned();
    }

    /// Whether the item is enabled.
    pub fn is_enabled(&self) -> bool {
        self.inner.borrow().enabled
    }

    /// Enable or disable the item.
    pub fn set_enabled(&self, enabled: bool) {
        self.inner.borrow_mut().enabled = enabled;
    }

    /// Whether the item is checked.
    pub fn is_checked(&self) -> bool {
        self.inner.borrow().checked
    }

    /// Set the checked state.
    pub fn set_checked(&self, checked: bool) {
        self.inner.borrow_mut().checked = checked;
    }

    /// Replace the item's accelerator (displayed only in a custom surface, D5).
    pub fn set_accelerator(&self, accelerator: Option<Accelerator>) {
        self.inner.borrow_mut().accelerator = accelerator;
    }
}

impl IsMenuItem for CheckMenuItem {
    fn kind(&self) -> MenuItemKind {
        MenuItemKind::Check(self.clone())
    }
    fn id(&self) -> MenuId {
        CheckMenuItem::id(self)
    }
}

impl_muri_row_ext!(CheckMenuItem);

// =============================================================================
// IconMenuItem (spec 02 §4.5)
// =============================================================================

enum IconSource {
    /// An image icon — raw RGBA ([`Icon::from_rgba`], encoded to PNG) or already
    /// PNG-encoded ([`Icon::from_png`]) — rendered as the row's leading image in
    /// the custom surface (issue #9 / #24).
    Rgba(Icon),
    /// A stock icon mapped to a named muri [`Symbol`](MuriIcon::Symbol).
    Native(NativeIcon),
}

struct IconMenuItemState {
    id: MenuId,
    text: String,
    enabled: bool,
    icon: Option<IconSource>,
    #[allow(dead_code)]
    accelerator: Option<Accelerator>,
    /// muri styling extensions (#18/#19/#25/#26/#27); see [`RowStyle`].
    style: RowStyle,
}

/// A menu item with a leading icon (muda's `IconMenuItem`). Maps to a muri
/// [`Row`](crate::menu::Row) with a leading icon (spec `02` §4.5); a raw-RGBA
/// icon is encoded to PNG and rendered as the row's leading image (see the
/// module-level `IconMenuItem` note).
#[derive(Clone)]
pub struct IconMenuItem {
    inner: Rc<RefCell<IconMenuItemState>>,
}

impl IconMenuItem {
    /// A new icon item with an auto-generated id.
    pub fn new(
        text: impl AsRef<str>,
        enabled: bool,
        icon: Option<Icon>,
        accelerator: Option<Accelerator>,
    ) -> Self {
        Self::build(None, text, enabled, icon.map(IconSource::Rgba), accelerator)
    }

    /// A new icon item with an explicit id.
    pub fn with_id(
        id: impl Into<MenuId>,
        text: impl AsRef<str>,
        enabled: bool,
        icon: Option<Icon>,
        accelerator: Option<Accelerator>,
    ) -> Self {
        Self::build(
            Some(id.into()),
            text,
            enabled,
            icon.map(IconSource::Rgba),
            accelerator,
        )
    }

    /// A new icon item backed by an OS stock [`NativeIcon`].
    pub fn with_native_icon(
        text: impl AsRef<str>,
        enabled: bool,
        native_icon: Option<NativeIcon>,
        accelerator: Option<Accelerator>,
    ) -> Self {
        Self::build(
            None,
            text,
            enabled,
            native_icon.map(IconSource::Native),
            accelerator,
        )
    }

    fn build(
        id: Option<MenuId>,
        text: impl AsRef<str>,
        enabled: bool,
        icon: Option<IconSource>,
        accelerator: Option<Accelerator>,
    ) -> Self {
        IconMenuItem {
            inner: Rc::new(RefCell::new(IconMenuItemState {
                id: resolve_id(id),
                text: text.as_ref().to_owned(),
                enabled,
                icon,
                accelerator,
                style: RowStyle::default(),
            })),
        }
    }

    row_style_setters!();

    /// This item's id.
    pub fn id(&self) -> MenuId {
        self.inner.borrow().id.clone()
    }

    /// This item's current text.
    pub fn text(&self) -> String {
        self.inner.borrow().text.clone()
    }

    /// Replace the item's text.
    pub fn set_text(&self, text: impl AsRef<str>) {
        self.inner.borrow_mut().text = text.as_ref().to_owned();
    }

    /// Whether the item is enabled.
    pub fn is_enabled(&self) -> bool {
        self.inner.borrow().enabled
    }

    /// Enable or disable the item.
    pub fn set_enabled(&self, enabled: bool) {
        self.inner.borrow_mut().enabled = enabled;
    }

    /// Replace the leading icon.
    pub fn set_icon(&self, icon: Option<Icon>) {
        self.inner.borrow_mut().icon = icon.map(IconSource::Rgba);
    }

    /// Replace the leading icon with an OS stock [`NativeIcon`].
    pub fn set_native_icon(&self, native_icon: Option<NativeIcon>) {
        self.inner.borrow_mut().icon = native_icon.map(IconSource::Native);
    }

    /// Replace the item's accelerator (displayed only in a custom surface, D5).
    pub fn set_accelerator(&self, accelerator: Option<Accelerator>) {
        self.inner.borrow_mut().accelerator = accelerator;
    }
}

impl IsMenuItem for IconMenuItem {
    fn kind(&self) -> MenuItemKind {
        MenuItemKind::Icon(self.clone())
    }
    fn id(&self) -> MenuId {
        IconMenuItem::id(self)
    }
}

impl_muri_row_ext!(IconMenuItem);

// =============================================================================
// PredefinedMenuItem (spec 02 §4.6)
// =============================================================================

/// The kind of a [`PredefinedMenuItem`].
#[derive(Clone)]
enum Predefined {
    Separator,
    Copy,
    Cut,
    Paste,
    SelectAll,
    Undo,
    Redo,
    Minimize,
    Maximize,
    Fullscreen,
    CloseWindow,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    About(#[allow(dead_code)] Option<AboutMetadata>),
    Services,
    BringAllToFront,
}

impl Predefined {
    fn default_text(&self) -> &'static str {
        match self {
            Predefined::Separator => "",
            Predefined::Copy => "Copy",
            Predefined::Cut => "Cut",
            Predefined::Paste => "Paste",
            Predefined::SelectAll => "Select All",
            Predefined::Undo => "Undo",
            Predefined::Redo => "Redo",
            Predefined::Minimize => "Minimize",
            Predefined::Maximize => "Zoom",
            Predefined::Fullscreen => "Enter Full Screen",
            Predefined::CloseWindow => "Close Window",
            Predefined::Hide => "Hide",
            Predefined::HideOthers => "Hide Others",
            Predefined::ShowAll => "Show All",
            Predefined::Quit => "Quit",
            Predefined::About(_) => "About",
            Predefined::Services => "Services",
            Predefined::BringAllToFront => "Bring All to Front",
        }
    }
}

struct PredefinedState {
    id: MenuId,
    text: String,
    predefined: Predefined,
}

/// A predefined OS-action / separator item (muda's `PredefinedMenuItem`). In a
/// native menu bar these behave natively; in a muri custom surface `separator`
/// is exact and every other action renders as a **visible, disabled** row per
/// the honesty rule (spec `02` §4.6, divergence D4).
#[derive(Clone)]
pub struct PredefinedMenuItem {
    inner: Rc<RefCell<PredefinedState>>,
}

impl PredefinedMenuItem {
    fn build(predefined: Predefined, text: Option<&str>) -> Self {
        let text = text
            .map(str::to_owned)
            .unwrap_or_else(|| predefined.default_text().to_owned());
        PredefinedMenuItem {
            inner: Rc::new(RefCell::new(PredefinedState {
                id: next_auto_id(),
                text,
                predefined,
            })),
        }
    }

    /// A separator (maps exactly to [`Item::Separator`](crate::menu::Item::Separator)).
    pub fn separator() -> Self {
        Self::build(Predefined::Separator, None)
    }

    /// The Copy edit action.
    pub fn copy(text: Option<&str>) -> Self {
        Self::build(Predefined::Copy, text)
    }
    /// The Cut edit action.
    pub fn cut(text: Option<&str>) -> Self {
        Self::build(Predefined::Cut, text)
    }
    /// The Paste edit action.
    pub fn paste(text: Option<&str>) -> Self {
        Self::build(Predefined::Paste, text)
    }
    /// The Select All edit action.
    pub fn select_all(text: Option<&str>) -> Self {
        Self::build(Predefined::SelectAll, text)
    }
    /// The Undo action.
    pub fn undo(text: Option<&str>) -> Self {
        Self::build(Predefined::Undo, text)
    }
    /// The Redo action.
    pub fn redo(text: Option<&str>) -> Self {
        Self::build(Predefined::Redo, text)
    }
    /// The Minimize window action.
    pub fn minimize(text: Option<&str>) -> Self {
        Self::build(Predefined::Minimize, text)
    }
    /// The Maximize/Zoom window action.
    pub fn maximize(text: Option<&str>) -> Self {
        Self::build(Predefined::Maximize, text)
    }
    /// The toggle-fullscreen window action.
    pub fn fullscreen(text: Option<&str>) -> Self {
        Self::build(Predefined::Fullscreen, text)
    }
    /// The Close Window action.
    pub fn close_window(text: Option<&str>) -> Self {
        Self::build(Predefined::CloseWindow, text)
    }
    /// The Hide (app) action.
    pub fn hide(text: Option<&str>) -> Self {
        Self::build(Predefined::Hide, text)
    }
    /// The Hide Others action.
    pub fn hide_others(text: Option<&str>) -> Self {
        Self::build(Predefined::HideOthers, text)
    }
    /// The Show All action.
    pub fn show_all(text: Option<&str>) -> Self {
        Self::build(Predefined::ShowAll, text)
    }
    /// The Quit action.
    pub fn quit(text: Option<&str>) -> Self {
        Self::build(Predefined::Quit, text)
    }
    /// The standard About panel; the metadata is used in a native menu bar
    /// (menu-bar-only off macOS in a custom surface — spec `02` §4.6).
    pub fn about(text: Option<&str>, metadata: Option<AboutMetadata>) -> Self {
        Self::build(Predefined::About(metadata), text)
    }
    /// The macOS Services submenu (menu-bar-only; a disabled placeholder in a
    /// custom surface — spec `02` §4.6).
    pub fn services(text: Option<&str>) -> Self {
        Self::build(Predefined::Services, text)
    }
    /// The Bring All to Front action.
    pub fn bring_all_to_front(text: Option<&str>) -> Self {
        Self::build(Predefined::BringAllToFront, text)
    }

    /// This item's id.
    pub fn id(&self) -> MenuId {
        self.inner.borrow().id.clone()
    }

    /// This item's current text.
    pub fn text(&self) -> String {
        self.inner.borrow().text.clone()
    }

    /// Whether this is a separator.
    #[allow(dead_code)] // used by the round-trip test.
    fn is_separator(&self) -> bool {
        matches!(self.inner.borrow().predefined, Predefined::Separator)
    }
}

impl IsMenuItem for PredefinedMenuItem {
    fn kind(&self) -> MenuItemKind {
        MenuItemKind::Predefined(self.clone())
    }
    fn id(&self) -> MenuId {
        PredefinedMenuItem::id(self)
    }
}

// =============================================================================
// Surface routing (spec 02 §2)
// =============================================================================

/// How a facade [`Menu`] was routed, decided by which surface method the
/// consumer called (spec `02` §2). Exposed to the facade tests only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceMode {
    /// No surface method called yet.
    Undetermined,
    /// `init_for_*` was called → native menu-bar passthrough.
    MenuBar,
    /// `show_context_menu_for_*` / a tray builder consumed it → muri custom.
    Custom,
}

/// A facade `Position`, standing in for muda's re-exported `dpi::Position`. Only
/// the `None` (current-cursor) form is honored by the current best-effort
/// context-menu surface; a supplied point is accepted for signature parity but
/// not yet applied (the popup opens at the pointer). Wiring an explicit position
/// through to a muri [`LogicalPoint`](crate::geometry::LogicalPoint) is a
/// follow-up.
#[derive(Clone, Copy, Debug)]
pub struct Position {
    /// Logical x coordinate.
    pub x: f64,
    /// Logical y coordinate.
    pub y: f64,
}

// =============================================================================
// Menu (spec 02 §4.1)
// =============================================================================

struct MenuState {
    id: MenuId,
    items: Vec<MenuItemKind>,
    mode: SurfaceMode,
}

/// muda's root menu container (spec `02` §4.1). Wraps a muri menu tree plus a
/// mode tag decided by which surface method is called. Cloning shares the same
/// menu (like muda).
#[derive(Clone)]
pub struct Menu {
    inner: Rc<RefCell<MenuState>>,
}

impl Default for Menu {
    fn default() -> Self {
        Menu::new()
    }
}

impl Menu {
    /// An empty menu with an auto-generated id.
    pub fn new() -> Self {
        Menu {
            inner: Rc::new(RefCell::new(MenuState {
                id: next_auto_id(),
                items: Vec::new(),
                mode: SurfaceMode::Undetermined,
            })),
        }
    }

    /// An empty menu with an explicit id.
    pub fn with_id(id: impl Into<MenuId>) -> Self {
        Menu {
            inner: Rc::new(RefCell::new(MenuState {
                id: id.into(),
                items: Vec::new(),
                mode: SurfaceMode::Undetermined,
            })),
        }
    }

    /// This menu's id.
    pub fn id(&self) -> MenuId {
        self.inner.borrow().id.clone()
    }

    /// Append an item.
    pub fn append(&self, item: &dyn IsMenuItem) -> Result<()> {
        self.inner.borrow_mut().items.push(item.kind());
        Ok(())
    }

    /// Append several items.
    pub fn append_items(&self, items: &[&dyn IsMenuItem]) -> Result<()> {
        for item in items {
            self.append(*item)?;
        }
        Ok(())
    }

    /// Prepend an item.
    pub fn prepend(&self, item: &dyn IsMenuItem) -> Result<()> {
        self.inner.borrow_mut().items.insert(0, item.kind());
        Ok(())
    }

    /// Insert an item at `position` (clamped to the current length).
    pub fn insert(&self, item: &dyn IsMenuItem, position: usize) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        let position = position.min(state.items.len());
        state.items.insert(position, item.kind());
        Ok(())
    }

    /// Remove the item with the same id, erroring if it is not present (mirrors
    /// muda's `NotAChildOfThisMenu`).
    pub fn remove(&self, item: &dyn IsMenuItem) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        let target = item.id();
        if let Some(pos) = state.items.iter().position(|k| k.id() == target) {
            state.items.remove(pos);
            Ok(())
        } else {
            Err(Error::NotAChildOfThisMenu)
        }
    }

    /// The current items as owned [`MenuItemKind`]s (round-trips the tree).
    pub fn items(&self) -> Vec<MenuItemKind> {
        self.inner.borrow().items.clone()
    }

    /// The muri menu tree this facade menu maps onto.
    pub(crate) fn to_muri_menu(&self) -> MuriMenu {
        kinds_to_muri_menu(&self.inner.borrow().items)
    }

    /// **muri escape hatch (issue #45):** the muri [`Menu`](crate::menu::Menu)
    /// tree this facade menu maps onto, exposed publicly so a caller can drop
    /// down to muri's own API (e.g. to build a [`crate::ContextMenu`] or
    /// [`crate::Popup`] directly with muri-only options this facade doesn't
    /// otherwise expose) without abandoning the muda-shaped item-building API.
    /// Not muda's API. A snapshot: later mutation of `self` through
    /// [`Menu::append`] etc. is not reflected in a tree already returned by an
    /// earlier call.
    pub fn as_native(&self) -> MuriMenu {
        self.to_muri_menu()
    }

    /// This menu's routed surface mode (facade-test accessor).
    #[allow(dead_code)] // read by the facade routing tests.
    pub(crate) fn mode(&self) -> SurfaceMode {
        self.inner.borrow().mode
    }

    fn set_mode(&self, mode: SurfaceMode) {
        self.inner.borrow_mut().mode = mode;
    }

    /// Tag this menu native-passthrough and (in a full backend) materialize the
    /// native OS menu. Shared by every `init_for_*` method.
    fn route_menu_bar(&self) -> Result<()> {
        self.set_mode(SurfaceMode::MenuBar);
        // A full backend builds an NSMenu/HMENU/GTK menu here (decision #5) and
        // wires its target/action into the same muri `dispatch`. The tree is
        // materialized eagerly so translation is exercised.
        let _ = self.to_muri_menu();
        Ok(())
    }

    /// Tag this menu custom and build the muri [`ContextMenu`](crate::ContextMenu)
    /// it maps onto. Shared by every `show_context_menu_for_*` method and by the
    /// tray builder.
    pub(crate) fn build_custom_surface(&self) -> crate::ContextMenu {
        self.set_mode(SurfaceMode::Custom);
        crate::ContextMenu::new(self.to_muri_menu())
    }

    /// The custom surface with a caller-supplied [`MenuOptions`] applied (#45).
    /// Factored out of [`open_custom_with_options`](Self::open_custom_with_options)
    /// so a test can observe that the options actually reach the surface (via
    /// [`crate::ContextMenu::menu_options`]) rather than only after the popup
    /// opens and consumes it.
    pub(crate) fn custom_surface_with_options(
        &self,
        options: crate::MenuOptions,
    ) -> crate::ContextMenu {
        self.build_custom_surface().options(options)
    }
}

/// muda's `ContextMenu` helper trait: `init_for_*` (native menu bar) and
/// `show_context_menu_for_*` (transient custom menu). Implemented by [`Menu`] and
/// [`Submenu`]. See the module-level note on why these are not `#[cfg]`-gated.
pub trait ContextMenu {
    /// Install as the macOS application menu bar (native passthrough).
    ///
    /// **No OS menu bar is installed** — muri has no menu-bar surface; the menu
    /// tree is validated and tagged only (documented divergence D3/D4, #29). muri
    /// also deliberately does **not** change `NSApplicationActivationPolicy` to
    /// `Regular` the way muda does (divergence D9, #32): a tray/menu-bar-extra app
    /// is typically an `Accessory` agent, and forcing `Regular` would pop a Dock
    /// icon. Neither the tray nor the popup depends on it.
    fn init_for_nsapp(&self) -> Result<()>;

    /// Install as a Windows window menu bar (native passthrough).
    ///
    /// # Safety
    /// `hwnd` must be a valid window handle. (Signature parity with muda; the
    /// facade performs no unsafe operations.)
    unsafe fn init_for_hwnd(&self, hwnd: isize) -> Result<()>;

    /// Install into a GTK window's menu bar (native passthrough). The facade
    /// takes no `gtk::Window` handle (no GTK dependency in M3); it routes through
    /// muri and is best-effort (divergence D3/D4).
    fn init_for_gtk_window(&self) -> Result<()>;

    /// Show as a transient context menu at `position` (or, when `None`, the live mouse cursor — falling back to the screen origin only where the platform cannot report the pointer, e.g. Wayland) on macOS.
    ///
    /// # Safety
    /// `nsview` must be a valid `NSView` pointer. (Signature parity with muda.)
    unsafe fn show_context_menu_for_nsview(
        &self,
        nsview: *mut c_void,
        position: Option<Position>,
    ) -> Result<()>;

    /// Show as a transient context menu at `position` (or, when `None`, the live mouse cursor — falling back to the screen origin only where the platform cannot report the pointer, e.g. Wayland) on Windows.
    ///
    /// # Safety
    /// `hwnd` must be a valid window handle. (Signature parity with muda.)
    unsafe fn show_context_menu_for_hwnd(
        &self,
        hwnd: isize,
        position: Option<Position>,
    ) -> Result<()>;

    /// Show as a transient context menu at `position` (or, when `None`, the live mouse cursor — falling back to the screen origin only where the platform cannot report the pointer, e.g. Wayland) on Linux.
    /// muri takes an **opaque** `*mut c_void` GTK window handle (muda takes an
    /// `&impl IsA<gtk::Widget>`) so the facade needs no GTK dependency yet keeps
    /// muda's call arity — a migrating GTK consumer's positional argument still
    /// compiles (#31).
    fn show_context_menu_for_gtk_window(
        &self,
        gtk_window: *mut c_void,
        position: Option<Position>,
    ) -> Result<()>;
}

/// Build the muri context surface and **open it** at `position` (#30). The popup
/// is driven by [`ContextMenu::open_at`](crate::ContextMenu::open_at), which
/// routes through the real platform popup loop; row clicks reach muda's global
/// [`MenuEvent`] channel via the backend's dispatch. `position` is muda's screen
/// point; `None` (the "current cursor" form muda supports) resolves to the live
/// mouse cursor via [`Platform::cursor_position`](crate::Platform::cursor_position),
/// falling back to the screen origin only where the platform can't report the
/// pointer (a Wayland session).
fn open_custom(menu: &Menu, position: Option<Position>) -> Result<()> {
    let surface = menu.build_custom_surface();
    let point = cursor_or_origin(position);
    surface
        .open_at(point, crate::Edge::Bottom)
        .map_err(Error::from)
}

/// The open point for a compat context menu: an explicit muda `position`, else the
/// live cursor ([`Platform::cursor_position`](crate::Platform::cursor_position)),
/// else the screen origin (Wayland / no pointer). Matches muda's `None` = "at the
/// cursor" behavior (#1).
fn cursor_or_origin(position: Option<Position>) -> crate::LogicalPoint {
    use crate::platform::Platform as _;
    position
        .map(|p| crate::LogicalPoint::new(p.x as f32, p.y as f32))
        .or_else(|| crate::platform::current().cursor_position())
        .unwrap_or_default()
}

impl Menu {
    /// **muri escape hatch (issue #45):** like [`show_context_menu_for_nsview`]
    /// / the other `ContextMenu::show_context_menu_for_*` methods, but threads
    /// an explicit [`MenuOptions`](crate::MenuOptions) (theme, width bounds,
    /// gutter policy) through to the opened popup instead of leaving it on
    /// [`crate::MenuOptions`]`::default`. Not muda's API — this is how a consumer opens
    /// a muri-styled custom menu with muri-only tuning (e.g. an in-menu
    /// "Preview theme" switcher that reopens the same menu under a different
    /// [`ThemeSource`](crate::ThemeSource)) through the facade rather than
    /// dropping to [`Menu::as_native`] and building a
    /// [`crate::ContextMenu`] by hand.
    ///
    /// [`show_context_menu_for_nsview`]: ContextMenu::show_context_menu_for_nsview
    pub fn open_custom_with_options(
        &self,
        position: Option<Position>,
        options: crate::MenuOptions,
    ) -> Result<()> {
        let surface = self.custom_surface_with_options(options);
        let point = cursor_or_origin(position);
        surface
            .open_at(point, crate::Edge::Bottom)
            .map_err(Error::from)
    }
}

impl ContextMenu for Menu {
    fn init_for_nsapp(&self) -> Result<()> {
        self.route_menu_bar()
    }
    unsafe fn init_for_hwnd(&self, _hwnd: isize) -> Result<()> {
        self.route_menu_bar()
    }
    fn init_for_gtk_window(&self) -> Result<()> {
        self.route_menu_bar()
    }
    unsafe fn show_context_menu_for_nsview(
        &self,
        _nsview: *mut c_void,
        position: Option<Position>,
    ) -> Result<()> {
        open_custom(self, position)
    }
    unsafe fn show_context_menu_for_hwnd(
        &self,
        _hwnd: isize,
        position: Option<Position>,
    ) -> Result<()> {
        open_custom(self, position)
    }
    fn show_context_menu_for_gtk_window(
        &self,
        _gtk_window: *mut c_void,
        position: Option<Position>,
    ) -> Result<()> {
        open_custom(self, position)
    }
}

// =============================================================================
// Submenu (spec 02 §4.2)
// =============================================================================

struct SubmenuState {
    id: MenuId,
    text: String,
    enabled: bool,
    items: Vec<MenuItemKind>,
    /// muri styling extensions (#18/#19/#25/#26/#27); see [`RowStyle`].
    style: RowStyle,
}

/// A nested submenu (muda's `Submenu`). Maps to
/// [`Item::Submenu`](crate::menu::Item::Submenu), nested to any depth (divergence
/// D8 resolved — spec `02` §4.2). Cloning shares the same submenu.
#[derive(Clone)]
pub struct Submenu {
    inner: Rc<RefCell<SubmenuState>>,
}

impl Submenu {
    /// A new submenu with an auto-generated id.
    pub fn new(text: impl AsRef<str>, enabled: bool) -> Self {
        Self::build(None, text, enabled)
    }

    /// A new submenu with an explicit id.
    pub fn with_id(id: impl Into<MenuId>, text: impl AsRef<str>, enabled: bool) -> Self {
        Self::build(Some(id.into()), text, enabled)
    }

    fn build(id: Option<MenuId>, text: impl AsRef<str>, enabled: bool) -> Self {
        Submenu {
            inner: Rc::new(RefCell::new(SubmenuState {
                id: resolve_id(id),
                text: text.as_ref().to_owned(),
                enabled,
                items: Vec::new(),
                style: RowStyle::default(),
            })),
        }
    }

    row_style_setters!();

    /// **muri extension (#27):** give this submenu row a leading image icon
    /// (muda's `Submenu` has no icon slot); the icon draws in the row's leading
    /// slot exactly like an [`IconMenuItem`]. Pass `None` to clear. Not muda's API.
    pub fn set_icon(&self, icon: Option<Icon>) {
        self.inner.borrow_mut().style.icon = icon;
    }

    /// This submenu's id.
    pub fn id(&self) -> MenuId {
        self.inner.borrow().id.clone()
    }

    /// This submenu's current text.
    pub fn text(&self) -> String {
        self.inner.borrow().text.clone()
    }

    /// Replace the submenu's text.
    pub fn set_text(&self, text: impl AsRef<str>) {
        self.inner.borrow_mut().text = text.as_ref().to_owned();
    }

    /// Whether the submenu is enabled.
    pub fn is_enabled(&self) -> bool {
        self.inner.borrow().enabled
    }

    /// Enable or disable the submenu.
    pub fn set_enabled(&self, enabled: bool) {
        self.inner.borrow_mut().enabled = enabled;
    }

    /// Append an item.
    pub fn append(&self, item: &dyn IsMenuItem) -> Result<()> {
        self.inner.borrow_mut().items.push(item.kind());
        Ok(())
    }

    /// Append several items.
    pub fn append_items(&self, items: &[&dyn IsMenuItem]) -> Result<()> {
        for item in items {
            self.append(*item)?;
        }
        Ok(())
    }

    /// Prepend an item.
    pub fn prepend(&self, item: &dyn IsMenuItem) -> Result<()> {
        self.inner.borrow_mut().items.insert(0, item.kind());
        Ok(())
    }

    /// Insert an item at `position` (clamped to the current length).
    pub fn insert(&self, item: &dyn IsMenuItem, position: usize) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        let position = position.min(state.items.len());
        state.items.insert(position, item.kind());
        Ok(())
    }

    /// Remove the item with the same id.
    pub fn remove(&self, item: &dyn IsMenuItem) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        let target = item.id();
        if let Some(pos) = state.items.iter().position(|k| k.id() == target) {
            state.items.remove(pos);
            Ok(())
        } else {
            Err(Error::NotAChildOfThisMenu)
        }
    }

    /// The current items as owned [`MenuItemKind`]s.
    pub fn items(&self) -> Vec<MenuItemKind> {
        self.inner.borrow().items.clone()
    }

    fn to_menu(&self) -> Menu {
        let state = self.inner.borrow();
        Menu {
            inner: Rc::new(RefCell::new(MenuState {
                id: state.id.clone(),
                items: state.items.clone(),
                mode: SurfaceMode::Undetermined,
            })),
        }
    }
}

impl IsMenuItem for Submenu {
    fn kind(&self) -> MenuItemKind {
        MenuItemKind::Submenu(self.clone())
    }
    fn id(&self) -> MenuId {
        Submenu::id(self)
    }
}

// Hand-written rather than `impl_muri_row_ext!`: `Submenu` is the one item
// type that also overrides `set_icon` (#27) — its muri-only leading-icon
// setter, since it has no icon slot in muda's own API (unlike `IconMenuItem`,
// which already has its own, differently-shaped `set_icon`).
impl MuriRowExt for Submenu {
    fn set_active(&self, active: bool) {
        Submenu::set_active(self, active);
    }
    fn set_bold(&self, bold: bool) {
        Submenu::set_bold(self, bold);
    }
    fn set_value_color(&self, color: Option<Color>) {
        Submenu::set_value_color(self, color);
    }
    fn set_value_runs(&self, runs: Vec<StyleRun>) {
        Submenu::set_value_runs(self, runs);
    }
    fn set_icon(&self, icon: Option<Icon>) {
        Submenu::set_icon(self, icon);
    }
}

impl ContextMenu for Submenu {
    fn init_for_nsapp(&self) -> Result<()> {
        self.to_menu().route_menu_bar()
    }
    unsafe fn init_for_hwnd(&self, _hwnd: isize) -> Result<()> {
        self.to_menu().route_menu_bar()
    }
    fn init_for_gtk_window(&self) -> Result<()> {
        self.to_menu().route_menu_bar()
    }
    unsafe fn show_context_menu_for_nsview(
        &self,
        _nsview: *mut c_void,
        position: Option<Position>,
    ) -> Result<()> {
        open_custom(&self.to_menu(), position)
    }
    unsafe fn show_context_menu_for_hwnd(
        &self,
        _hwnd: isize,
        position: Option<Position>,
    ) -> Result<()> {
        open_custom(&self.to_menu(), position)
    }
    fn show_context_menu_for_gtk_window(
        &self,
        _gtk_window: *mut c_void,
        position: Option<Position>,
    ) -> Result<()> {
        open_custom(&self.to_menu(), position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::Item;

    #[test]
    fn menu_id_auto_generation_is_monotonic_and_unique() {
        // The counter is process-global and shared with every other test running
        // in parallel, so a `nb == na + 1` assertion is flaky (another test can
        // advance the counter in between). Assert the invariant the facade
        // actually guarantees — auto ids are strictly increasing and unique — not
        // muda's exact numeric sequence (which the facade deliberately does not
        // reproduce; see `next_auto_id`).
        let a = MenuItem::new("A", true, None);
        let b = MenuItem::new("B", true, None);
        let na: u32 = a.id().0.parse().expect("auto id is an integer");
        let nb: u32 = b.id().0.parse().expect("auto id is an integer");
        assert!(
            nb > na,
            "auto ids are strictly increasing and unique (got {na} then {nb})"
        );
    }

    #[test]
    fn explicit_id_is_used_verbatim() {
        let item = MenuItem::with_id("open", "Open", true, None);
        assert_eq!(item.id(), MenuId::from("open"));
    }

    #[test]
    fn from_rgba_rejects_overflowing_dimensions_without_panicking() {
        // width*height*4 overflows usize; before the checked-arithmetic fix this
        // panicked under debug overflow checks (the default test profile) before
        // the length guard ran. It must return a BadIcon error instead.
        let err = Icon::from_rgba(Vec::new(), u32::MAX, u32::MAX);
        assert!(err.is_err(), "overflowing dimensions must error, not panic");

        // A normal well-formed icon still succeeds, and a plain length mismatch
        // still errors (the guard the function exists for is intact).
        assert!(Icon::from_rgba(vec![0; 4], 1, 1).is_ok());
        assert!(Icon::from_rgba(vec![0; 3], 1, 1).is_err());

        // A zero dimension is rejected up front rather than accepted (its length
        // guard passes: 0 bytes) and then silently dropped by the encoder, which
        // rejects a zero dimension — a "success" that never draws.
        assert!(Icon::from_rgba(Vec::new(), 0, 0).is_err());
        assert!(Icon::from_rgba(Vec::new(), 0, 8).is_err());
        assert!(Icon::from_rgba(Vec::new(), 8, 0).is_err());
    }

    /// Issue #15 diagnosis: prove the muda→muri conversion already produces the
    /// correct structure — a disabled `IconMenuItem` becomes a `Row` with a
    /// *leading* icon, and a `Submenu` with a `label\tvalue` text becomes a
    /// label row whose segments are `[Grow, Right-aligned]`. muri's painter
    /// (`render::paint`) draws both correctly (see its `issue15_*` test); the
    /// on-device GNOME discrepancy is in the SNI/AppIndicator *native* menu
    /// path, which GNOME Shell renders (lib.rs OS matrix), not muri's painter.
    #[test]
    fn icon_header_and_tab_submenu_convert_to_leading_icon_and_flex_segments() {
        use crate::menu::{Align, Flex, Icon as MuriIcon};

        let menu = Menu::new();
        let icon = Icon::from_rgba(vec![0u8; 4], 1, 1).expect("valid RGBA");
        // A disabled section-header IconMenuItem carrying a provider logo.
        let header = IconMenuItem::with_id("hdr:claude", "Claude", false, Some(icon), None);
        // An account Submenu whose label is `email\tvalue`.
        let acct = Submenu::with_id("acct:me", "me@example.com\t47% / 52%", true);
        menu.append(&header).unwrap();
        menu.append(&acct).unwrap();

        let muri = menu.to_muri_menu();

        // The header row carries a LEADING png icon (never trailing).
        let Item::Row(row) = &muri.items[0] else {
            panic!(
                "IconMenuItem must convert to Item::Row, got {:?}",
                muri.items[0]
            );
        };
        assert!(
            matches!(row.leading, Some(MuriIcon::Png(_))),
            "provider logo must be the row's leading icon"
        );
        assert!(
            row.trailing.is_none(),
            "the logo must not be a trailing icon"
        );

        // The submenu label splits at the TAB into a growing lead + right value.
        let Item::Submenu { label, .. } = &muri.items[1] else {
            panic!("Submenu must convert to Item::Submenu");
        };
        assert_eq!(
            label.segments.len(),
            2,
            "label\\tvalue splits into two segments"
        );
        assert_eq!(label.segments[0].flex, Flex::Grow, "lead segment grows");
        assert_eq!(
            label.segments[1].align,
            Align::Right,
            "value segment is right-aligned"
        );
    }

    /// #18 + #19: an active submenu row converts to a bold, checkmarked muri row
    /// whose trailing `\t` value carries the severity color.
    #[test]
    fn active_submenu_is_bold_checked_and_value_colored() {
        use crate::menu::{Icon as MuriIcon, Item};
        use crate::style::{Color, Weight};

        let menu = Menu::new();
        let acct = Submenu::with_id("acct:me", "me@example.com\t47% / 52%", true);
        acct.set_active(true);
        acct.set_value_color(Some(Color::SystemRed));
        menu.append(&acct).unwrap();

        let muri = menu.to_muri_menu();
        let Item::Submenu { label, .. } = &muri.items[0] else {
            panic!("expected a submenu");
        };

        // Active → checked + leading checkmark.
        assert_eq!(label.checked, Some(true));
        assert!(matches!(label.leading, Some(MuriIcon::Checkmark)));

        // Bold lead, and a bold + red trailing value.
        assert_eq!(label.segments.len(), 2);
        assert!(
            label.segments[0]
                .runs
                .iter()
                .any(|r| r.weight == Some(Weight::Bold)),
            "active lead segment is bold"
        );
        assert!(
            label.segments[1]
                .runs
                .iter()
                .any(|r| r.color == Color::SystemRed && r.weight == Some(Weight::Bold)),
            "active value segment is bold + severity-colored"
        );
    }

    /// #19: a value color without `active` colors the trailing segment (no bold).
    #[test]
    fn value_color_without_active_colors_trailing_segment() {
        use crate::menu::Item;
        use crate::style::Color;

        let menu = Menu::new();
        let item = MenuItem::with_id("x", "Label\t99%", true, None);
        item.set_value_color(Some(Color::SystemOrange));
        menu.append(&item).unwrap();

        let muri = menu.to_muri_menu();
        let Item::Row(row) = &muri.items[0] else {
            panic!("expected a row");
        };
        assert_eq!(row.segments.len(), 2);
        assert_eq!(row.segments[1].color, Some(Color::SystemOrange));
        assert!(
            row.segments[1].runs.is_empty(),
            "no bold runs when not active"
        );
    }

    /// #24: a PNG-backed compat icon passes its encoded bytes straight through to
    /// muri's native `Icon::Png` — no decode/re-encode round-trip.
    #[test]
    fn png_icon_translates_to_native_png_without_reencoding() {
        use crate::menu::{Icon as MuriIcon, Item};
        let png = crate::render::encode_rgba_png(&[10, 20, 30, 255], 1, 1).unwrap();
        let icon = Icon::from_png(&png).expect("PNG icon constructor succeeds");
        let item = IconMenuItem::with_id("png", "PNG", true, Some(icon), None);
        let menu = Menu::new();
        menu.append(&item).unwrap();

        let muri = menu.to_muri_menu();
        let Item::Row(row) = &muri.items[0] else {
            panic!("expected an icon row");
        };
        match &row.leading {
            Some(MuriIcon::Png(bytes)) => assert_eq!(bytes.as_ref(), png.as_slice()),
            other => panic!("expected a native PNG icon, got {other:?}"),
        }
    }

    /// #25/#26/#27/#28: the styling extensions across item types.
    #[test]
    fn styling_extensions_bold_icon_value_runs_and_checkmenuitem() {
        use crate::menu::{Icon as MuriIcon, Item};
        use crate::style::{Color, Weight};

        // #26: bold without a checkmark.
        let hdr = MenuItem::with_id("h", "Header", false, None);
        hdr.set_bold(true);
        // #27: a leading icon on a Submenu.
        let png = crate::render::encode_rgba_png(&[1, 2, 3, 255], 1, 1).unwrap();
        let sub = Submenu::with_id("s", "Account", true);
        sub.set_icon(Some(Icon::from_png(&png).unwrap()));
        // #25: per-span value colors.
        let item = MenuItem::with_id("v", "Lbl\t47% / 52%", true, None);
        item.set_value_runs(vec![
            StyleRun::new(0, 3, Color::SystemGreen),
            StyleRun::new(6, 3, Color::SystemRed),
        ]);
        // #28: CheckMenuItem gains bold + value color; checkbox drives the check.
        let chk = CheckMenuItem::with_id("c", "Opt\t9%", true, true, None);
        chk.set_bold(true);
        chk.set_value_color(Some(Color::SystemOrange));

        let menu = Menu::new();
        menu.append(&hdr).unwrap();
        menu.append(&sub).unwrap();
        menu.append(&item).unwrap();
        menu.append(&chk).unwrap();
        let m = menu.to_muri_menu();

        // #26: bold header, no checkmark.
        let Item::Row(h) = &m.items[0] else { panic!() };
        assert!(h.leading.is_none(), "bold-only row has no checkmark");
        assert!(h.segments[0]
            .runs
            .iter()
            .any(|r| r.weight == Some(Weight::Bold)));

        // #27: submenu has a leading PNG icon.
        let Item::Submenu { label, .. } = &m.items[1] else {
            panic!()
        };
        assert!(matches!(label.leading, Some(MuriIcon::Png(_))));

        // #25: value segment carries the two colored runs.
        let Item::Row(v) = &m.items[2] else { panic!() };
        let runs = &v.segments[1].runs;
        assert!(runs.iter().any(|r| r.color == Color::SystemGreen));
        assert!(runs.iter().any(|r| r.color == Color::SystemRed));

        // #28: CheckMenuItem — checked → checkmark; bold + value color applied
        // (bold makes the value color a bold run rather than a plain color).
        let Item::Row(c) = &m.items[3] else { panic!() };
        assert_eq!(c.checked, Some(true));
        assert!(matches!(c.leading, Some(MuriIcon::Checkmark)));
        assert!(c.segments[1]
            .runs
            .iter()
            .any(|r| r.color == Color::SystemOrange && r.weight == Some(Weight::Bold)));
    }

    #[test]
    fn init_for_nsapp_tags_menu_bar_passthrough() {
        let menu = Menu::new();
        menu.append(&MenuItem::with_id("save", "Save", true, None))
            .unwrap();
        assert_eq!(menu.mode(), SurfaceMode::Undetermined);
        menu.init_for_nsapp().unwrap();
        assert_eq!(menu.mode(), SurfaceMode::MenuBar);
    }

    #[test]
    fn build_custom_surface_tags_custom() {
        let menu = Menu::new();
        menu.append(&MenuItem::with_id("open", "Open", true, None))
            .unwrap();
        let _surface = menu.build_custom_surface();
        assert_eq!(menu.mode(), SurfaceMode::Custom);
    }

    #[test]
    fn item_type_translation_onto_muri_tree() {
        let menu = Menu::new();
        menu.append(&MenuItem::with_id("open", "Open", true, None))
            .unwrap();
        menu.append(&CheckMenuItem::with_id("chk", "Notify", true, true, None))
            .unwrap();
        menu.append(&IconMenuItem::with_native_icon(
            "User",
            true,
            Some(NativeIcon::User),
            None,
        ))
        .unwrap();
        menu.append(&PredefinedMenuItem::separator()).unwrap();
        menu.append(&PredefinedMenuItem::quit(Some("Quit MyApp")))
            .unwrap();
        let sub = Submenu::with_id("acct", "Account", true);
        sub.append(&MenuItem::with_id("switch", "Switch", true, None))
            .unwrap();
        menu.append(&sub).unwrap();

        let muri = menu.to_muri_menu();
        assert_eq!(muri.items.len(), 6);

        // MenuItem → Row
        match &muri.items[0] {
            Item::Row(r) => {
                assert_eq!(r.id, MenuId::from("open"));
                assert!(r.enabled);
                assert_eq!(r.checked, None);
            }
            _ => panic!("expected a Row"),
        }
        // CheckMenuItem → Row.checked
        match &muri.items[1] {
            Item::Row(r) => assert_eq!(r.checked, Some(true)),
            _ => panic!("expected a checked Row"),
        }
        // IconMenuItem (NativeIcon) → Row.leading = Symbol
        match &muri.items[2] {
            Item::Row(r) => assert!(matches!(r.leading, Some(MuriIcon::Symbol(_)))),
            _ => panic!("expected an icon Row"),
        }
        // separator → Item::Separator
        assert!(matches!(muri.items[3], Item::Separator));
        // other predefined → visible DISABLED row (honesty rule)
        match &muri.items[4] {
            Item::Row(r) => {
                assert!(!r.enabled);
                assert_eq!(r.accessible_name(), "Quit MyApp");
            }
            _ => panic!("expected a disabled predefined Row"),
        }
        // Submenu → Item::Submenu with its nested child
        match &muri.items[5] {
            Item::Submenu { label, menu } => {
                assert_eq!(label.id, MenuId::from("acct"));
                assert_eq!(menu.items.len(), 1);
            }
            _ => panic!("expected a Submenu"),
        }
    }

    #[test]
    fn tab_label_becomes_two_columns_and_checked_shows_a_checkmark() {
        // #12: a `label\tvalue` tab-stop splits into a Grow left segment + a
        // right-aligned trailing column (muda's NSMenu tab stop), and a checked
        // CheckMenuItem gets the leading native checkmark.
        let menu = Menu::new();
        menu.append(&MenuItem::with_id("a", "Account\t47% / 89%", true, None))
            .unwrap();
        menu.append(&CheckMenuItem::with_id("b", "Active", true, true, None))
            .unwrap();
        let muri = menu.to_muri_menu();

        match &muri.items[0] {
            Item::Row(r) => {
                assert_eq!(r.segments.len(), 2, "tab splits into two segments");
                assert_eq!(r.segments[0].text, "Account");
                assert!(matches!(r.segments[0].flex, Flex::Grow));
                assert_eq!(r.segments[1].text, "47% / 89%");
                assert!(matches!(r.segments[1].align, Align::Right));
            }
            _ => panic!("expected a two-column Row"),
        }
        match &muri.items[1] {
            Item::Row(r) => {
                assert_eq!(r.checked, Some(true));
                assert!(
                    matches!(r.leading, Some(MuriIcon::Checkmark)),
                    "a checked item shows the leading checkmark"
                );
            }
            _ => panic!("expected a checked Row"),
        }
    }

    #[test]
    fn icon_menu_item_raw_rgba_renders_as_a_leading_png() {
        // A raw-RGBA IconMenuItem (the only compat Icon constructor) must reach
        // the custom surface as a leading Icon::Png, not be dropped (issue #9 —
        // the menu-item counterpart of the 0.9.2 tray D6 fix).
        let rgba = vec![9, 8, 7, 255];
        let icon = Icon::from_rgba(rgba.clone(), 1, 1).expect("valid RGBA");
        let menu = Menu::new();
        menu.append(&IconMenuItem::new("Claude", true, Some(icon), None))
            .unwrap();

        let muri = menu.to_muri_menu();
        match &muri.items[0] {
            Item::Row(r) => match &r.leading {
                Some(MuriIcon::Png(bytes)) => {
                    let (decoded, w, h) =
                        crate::render::decode_png(bytes).expect("leading icon PNG decodes");
                    assert_eq!((w, h), (1, 1));
                    assert_eq!(
                        decoded, rgba,
                        "the RGBA round-trips onto the row's leading icon"
                    );
                }
                other => panic!("expected a leading PNG icon, got {other:?}"),
            },
            _ => panic!("expected an icon Row"),
        }
    }

    #[test]
    fn items_round_trips_to_kinds() {
        let menu = Menu::new();
        menu.append(&MenuItem::with_id("a", "A", true, None))
            .unwrap();
        menu.append(&PredefinedMenuItem::separator()).unwrap();
        let kinds = menu.items();
        assert_eq!(kinds.len(), 2);
        assert!(matches!(kinds[0], MenuItemKind::MenuItem(_)));
        assert!(matches!(kinds[1], MenuItemKind::Predefined(ref p) if p.is_separator()));
    }

    #[test]
    fn interior_mutability_setters_take_shared_ref() {
        let item = MenuItem::with_id("x", "Before", true, None);
        let clone = item.clone();
        item.set_text("After");
        item.set_enabled(false);
        // The clone observes the mutation (muda's shared-item semantics).
        assert_eq!(clone.text(), "After");
        assert!(!clone.is_enabled());
    }

    #[test]
    fn remove_absent_item_errors() {
        let menu = Menu::new();
        let a = MenuItem::with_id("a", "A", true, None);
        let b = MenuItem::with_id("b", "B", true, None);
        menu.append(&a).unwrap();
        assert!(matches!(menu.remove(&b), Err(Error::NotAChildOfThisMenu)));
        assert!(menu.remove(&a).is_ok());
    }

    /// #53 (item 2): `Icon::data()` must report `Png` for a `from_png` icon and
    /// `Rgba` for a `from_rgba` icon — the honest accessor, since the raw
    /// `rgba`/`width`/`height` fields read as empty/zero for the PNG case.
    #[test]
    fn icon_data_reports_png_for_from_png_and_rgba_for_from_rgba() {
        let png_bytes = crate::render::encode_rgba_png(&[1, 2, 3, 255], 1, 1).unwrap();
        let png_icon = Icon::from_png(&png_bytes).expect("valid PNG icon");
        match png_icon.data() {
            IconData::Png(bytes) => assert_eq!(bytes, png_bytes.as_slice()),
            other => panic!("expected IconData::Png, got {other:?}"),
        }

        let rgba = vec![9, 8, 7, 255];
        let rgba_icon = Icon::from_rgba(rgba.clone(), 1, 1).expect("valid RGBA icon");
        match rgba_icon.data() {
            IconData::Rgba {
                width,
                height,
                pixels,
            } => {
                assert_eq!((width, height), (1, 1));
                assert_eq!(pixels, rgba.as_slice());
            }
            other => panic!("expected IconData::Rgba, got {other:?}"),
        }
    }

    /// #53 (item 3): `MuriRowExt` is callable on every item type (dispatched
    /// through a trait bound, not just inherent method-call syntax), and
    /// `Submenu`'s `set_icon` override actually reaches the row's leading icon.
    #[test]
    fn muri_row_ext_is_callable_on_every_item_type() {
        fn mark_active<T: MuriRowExt>(row: &T) {
            row.set_active(true);
            row.set_bold(true);
            row.set_value_color(Some(Color::SystemRed));
            row.set_value_runs(vec![StyleRun::new(0, 1, Color::SystemGreen)]);
        }

        let item = MenuItem::with_id("i", "Lbl\tv", true, None);
        let chk = CheckMenuItem::with_id("c", "Lbl\tv", true, false, None);
        let icon_item = IconMenuItem::with_id("ic", "Lbl\tv", true, None, None);
        let sub = Submenu::with_id("s", "Lbl\tv", true);

        mark_active(&item);
        mark_active(&chk);
        mark_active(&icon_item);
        mark_active(&sub);

        // The trait's default no-op `set_icon` is a no-op for a non-`Submenu`
        // type; `Submenu`'s override actually sets the leading icon (#27).
        let png = crate::render::encode_rgba_png(&[4, 5, 6, 255], 1, 1).unwrap();
        MuriRowExt::set_icon(&sub, Some(Icon::from_png(&png).unwrap()));

        let menu = Menu::new();
        menu.append(&sub).unwrap();
        let muri = menu.to_muri_menu();
        let Item::Submenu { label, .. } = &muri.items[0] else {
            panic!("expected a submenu");
        };
        assert!(
            matches!(label.leading, Some(MuriIcon::Png(_))),
            "MuriRowExt::set_icon on Submenu reaches the row's leading icon"
        );
        // The bold+active styling from `mark_active` landed too.
        assert_eq!(label.checked, Some(true));
    }

    /// #45: `Menu::as_native` exposes the same muri tree `to_muri_menu` builds,
    /// so an external caller can drop to muri's own API without losing the
    /// menu it built through the facade.
    #[test]
    fn as_native_exposes_the_same_muri_tree() {
        let menu = Menu::new();
        menu.append(&MenuItem::with_id("open", "Open", true, None))
            .unwrap();
        let native = menu.as_native();
        assert_eq!(native.items.len(), 1);
        match &native.items[0] {
            Item::Row(r) => assert_eq!(r.id, MenuId::from("open")),
            other => panic!("expected a Row, got {other:?}"),
        }
    }

    /// #45: `open_custom_with_options` must thread the given `MenuOptions`
    /// through to the opened surface. Observe the surface's `menu_options()`
    /// directly (via the same `custom_surface_with_options` seam `build()` uses)
    /// — this fails if the options are dropped, unlike asserting only
    /// `mode() == Custom`, which `build_custom_surface` sets regardless of options.
    #[test]
    fn open_custom_with_options_threads_the_options_into_the_surface() {
        let menu = Menu::new();
        menu.append(&MenuItem::with_id("open", "Open", true, None))
            .unwrap();
        let options = crate::MenuOptions::default().min_width(123.0);
        let surface = menu.custom_surface_with_options(options.clone());
        assert_eq!(
            surface.menu_options().min_width,
            Some(123.0),
            "with_options width must reach the custom surface"
        );
        assert_eq!(menu.mode(), SurfaceMode::Custom);

        // And the public entry point still executes the options-aware path (it
        // returns a platform Error headlessly rather than panicking).
        let _ = menu.open_custom_with_options(Some(Position { x: 10.0, y: 20.0 }), options);
    }
}
