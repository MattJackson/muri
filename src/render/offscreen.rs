//! Display-free, one-call offscreen rendering of a built [`Menu`] to pixels
//! (issue #59).
//!
//! [`render_menu_to_png`] / [`render_menu_to_rgba`] rasterize a menu popup to a
//! bitmap **without** creating a tray, a window, or requiring any display, TCC
//! prompt, or Accessibility permission — the exact same layout + paint pass a
//! live popup runs ([`render_menu`](super::paint::render_menu) over a
//! [`RasterDrawer`]), driven straight into an in-memory
//! [`Framebuffer`](super::Framebuffer) and read back out. That makes them
//! suitable for generating screenshots and cross-OS golden images on a headless
//! CI runner.
//!
//! These entry points are always available: they need no platform/tray feature
//! and carry no `#[cfg(target_os)]` — they render the same way on every OS.
//!
//! ## Theme resolution is handled for you
//!
//! The caller passes only `&Menu`, `&MenuOptions`, and a device `scale`; the
//! concrete [`Theme`](crate::Theme) is resolved internally from
//! [`MenuOptions::theme`](crate::MenuOptions) exactly as a real popup resolves
//! it, and the matching font tier is selected the same way
//! ([`RasterDrawer::for_menu_options`]): a forced OS look pins the *target* OS's
//! UI font via the forced-theme / bundled-font resolution path, so the offscreen
//! pixels match what the on-screen popup would draw.
//!
//! ## Cross-OS looks from a single host
//!
//! Set [`MenuOptions::theme`](crate::MenuOptions) to a forced source
//! ([`ThemeSource::MacOs`](crate::ThemeSource::MacOs) /
//! [`Windows`](crate::ThemeSource::Windows) /
//! [`Gnome`](crate::ThemeSource::Gnome)) to render any OS's OEM menu look from
//! any host — the basis for a single-runner cross-OS golden suite. With the
//! `bundled-fonts` feature on, a forced look also renders in a vendored OSS
//! substitute face when the real target font is absent, so the output is
//! byte-identical across macOS/Windows/Linux runners.
//!
//! ## Appearance and hovered rows
//!
//! The render is deterministic and headless, so it does **not** query the live
//! system appearance or accent: a `System(..)`/forced source with
//! [`ThemeMode::Auto`](crate::ThemeMode) resolves to the *light* look. Pass an
//! explicit dark mode (e.g. `ThemeSource::MacOs(ThemeMode::Dark)`) for the dark
//! variant. No row is highlighted (no hovered state); to render a specific
//! hovered row, drive [`render_menu`](super::paint::render_menu) over a
//! [`RasterDrawer`] directly with a `highlight` index.

use crate::menu::Menu;
use crate::theme::{MenuOptions, OsFamily, Theme};

use super::paint::render_menu;
use super::RasterDrawer;

/// The host OS family, mapped from [`std::env::consts::OS`] (a compile-time
/// target constant, *not* a `#[cfg(target_os)]` branch — this module is
/// OS-agnostic by contract). Used only to resolve a `System(..)` theme source
/// against the host look; forced sources ignore it entirely.
fn host_os_family() -> OsFamily {
    match std::env::consts::OS {
        "macos" | "ios" => OsFamily::MacOs,
        "windows" => OsFamily::Windows,
        // Every other target (the Linux/BSD desktops muri's X11 backend serves)
        // uses the GNOME look as the generic non-mac/non-Windows default.
        _ => OsFamily::Gnome,
    }
}

/// Resolve the concrete [`Theme`] from `options` the same way a live popup does,
/// but display-free: forced sources (`MacOs`/`Windows`/`Gnome`) yield the target
/// OS look on any host; `System`/`Preset`/`Custom` resolve against the host
/// family. No live OS accent/palette/menu-font is injected (that path needs a
/// display/TCC), and the appearance defaults to light, so the output is
/// deterministic on a headless runner.
fn resolve_theme(options: &MenuOptions) -> Theme {
    options.theme.resolve(host_os_family(), false)
}

/// Render `menu` into a fresh [`RasterDrawer`] using the theme and font tier
/// resolved from `options` — the shared core of both public entry points.
fn render_to_drawer(menu: &Menu, options: &MenuOptions, scale: f32) -> RasterDrawer {
    let theme = resolve_theme(options);
    // `for_menu_options` picks the same drawer a live popup would: a forced OS
    // theme gets the target OS's UI font (`with_forced_theme` / bundled-font
    // tier); System/Preset/Custom get the host-native font.
    let mut drawer = RasterDrawer::for_menu_options(scale, options);
    let _ = render_menu(&mut drawer, menu, &theme, options, None);
    drawer
}

/// Render a built [`Menu`] to PNG bytes, display-free — no tray, no window, no
/// display/TCC/Accessibility permission required.
///
/// The popup is laid out and painted exactly as a live one would be
/// ([`render_menu`](super::paint::render_menu) over a [`RasterDrawer`]), then
/// encoded to a straight-alpha RGBA8 PNG. `scale` is the device scale factor
/// (device pixels per logical pixel, e.g. `2.0` for a Retina-density capture).
///
/// The [`Theme`](crate::Theme) is resolved internally from
/// [`options.theme`](crate::MenuOptions) — the caller never builds a `Theme`.
/// Force a cross-OS look with
/// [`ThemeSource::MacOs`](crate::ThemeSource::MacOs) /
/// [`Windows`](crate::ThemeSource::Windows) /
/// [`Gnome`](crate::ThemeSource::Gnome) to render any OS's OEM menu from any
/// host (ideal for CI screenshots and cross-OS golden tests).
///
/// The render is deterministic and headless: it does not query the live system
/// appearance or accent, so a `System(..)`/forced source with
/// [`ThemeMode::Auto`](crate::ThemeMode) resolves to the *light* look (pass an
/// explicit dark mode for the dark variant), and no row is highlighted.
///
/// # Examples
///
/// ```
/// use muri::{Menu, MenuOptions, Row};
///
/// let menu = Menu::new().row(Row::new("quit").label("Quit"));
/// let png = muri::render_menu_to_png(&menu, &MenuOptions::default(), 2.0);
/// assert!(!png.is_empty());
/// ```
pub fn render_menu_to_png(menu: &Menu, options: &MenuOptions, scale: f32) -> Vec<u8> {
    render_to_drawer(menu, options, scale).encode_png()
}

/// Render a built [`Menu`] to straight-alpha RGBA8 pixels, returning
/// `(rgba, width, height)` in device pixels — the same display-free pass as
/// [`render_menu_to_png`] without the PNG encode, for callers that want the raw
/// buffer (diffing, custom encoding, feeding another rasterizer).
///
/// `rgba` is `width * height * 4` bytes, row-major, un-premultiplied
/// (`R, G, B, A`). `scale` is the device scale factor. The
/// [`Theme`](crate::Theme) is resolved internally from
/// [`options.theme`](crate::MenuOptions); force a cross-OS look with
/// [`ThemeSource::MacOs`](crate::ThemeSource::MacOs) /
/// [`Windows`](crate::ThemeSource::Windows) /
/// [`Gnome`](crate::ThemeSource::Gnome). Like [`render_menu_to_png`], the render
/// is deterministic and headless (light look for an `Auto` appearance, no
/// highlighted row).
///
/// # Examples
///
/// ```
/// use muri::{Menu, MenuOptions, Row};
///
/// let menu = Menu::new().row(Row::new("quit").label("Quit"));
/// let (rgba, w, h) = muri::render_menu_to_rgba(&menu, &MenuOptions::default(), 2.0);
/// assert_eq!(rgba.len(), (w * h * 4) as usize);
/// ```
pub fn render_menu_to_rgba(menu: &Menu, options: &MenuOptions, scale: f32) -> (Vec<u8>, u32, u32) {
    let drawer = render_to_drawer(menu, options, scale);
    let (w, h) = drawer.device_size();
    (drawer.framebuffer().to_straight_rgba(), w, h)
}
