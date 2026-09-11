//! Display-free offscreen-render tests for the public API (issue #59):
//! [`muri::render_menu_to_png`] / [`muri::render_menu_to_rgba`] rasterize a
//! built menu to pixels with **no tray, window, display, TCC, or Accessibility**
//! — the headless path a CI runner uses for screenshots and cross-OS goldens.
//!
//! Two things are proven here:
//!
//! 1. **Headless output shape** — the public entry points return non-empty,
//!    correctly-sized, decodable pixels with no platform surface at all.
//! 2. **Cross-OS golden capability from one runner** — forcing
//!    [`ThemeSource::MacOs`]/[`Windows`]/[`Gnome`] renders all three OEM looks on
//!    this single host; the three renders are asserted to differ, and (under the
//!    `bundled-fonts` feature, which makes the forced-theme faces host-independent
//!    via the vendored OSS substitutes — see `assets/fonts/`) each is compared to
//!    a committed golden PNG under `tests/snapshots/`.
//!
//! Regenerate the goldens after an intentional visual change with the
//! `bundled-fonts` feature enabled:
//! `MURI_UPDATE_SNAPSHOTS=1 cargo test --features bundled-fonts --test headless`,
//! then re-run without the env var to confirm determinism before committing.

mod support;

use muri::{
    render_menu_to_png, render_menu_to_rgba, Align, Color, Flex, Icon, Menu, MenuOptions, Row,
    Segment, ThemeMode, ThemeSource,
};

/// A representative menu exercising the metric/color tells that distinguish the
/// OS looks: a section header, a flush-right label/value row, a checked
/// leading-icon row (checkmark gutter), a separator, and a secondary-colored
/// trailing value.
fn sample_menu() -> Menu {
    Menu::new()
        .section_header(Row::info().label("Account"))
        .row(Row::new("acct:me").segments(vec![
            Segment::new("me@example.com").flex(Flex::Grow),
            Segment::new("47%")
                .align(Align::Right)
                .color(Color::SystemGreen),
        ]))
        .row(
            Row::new("acct:on")
                .label("Sync enabled")
                .leading(Icon::Checkmark)
                .checked(true),
        )
        .separator()
        .row(Row::new("quit").segments(vec![
            Segment::new("Quit").flex(Flex::Grow),
            Segment::new("v0.10")
                .align(Align::Right)
                .color(Color::SecondaryLabel),
        ]))
}

fn forced(family: ThemeSource) -> MenuOptions {
    MenuOptions::default().theme(family)
}

#[test]
fn render_menu_to_png_is_nonempty_and_decodable() {
    let menu = sample_menu();
    let opts = MenuOptions::default();

    let png = render_menu_to_png(&menu, &opts, 2.0);
    assert!(!png.is_empty(), "PNG output must be non-empty");

    // Decodes to a positive, correctly-sized straight-alpha RGBA buffer.
    let (rgba, w, h) = muri::render::decode_png(&png).expect("PNG decodes");
    assert!(w > 0 && h > 0, "decoded PNG must have positive dimensions");
    assert_eq!(rgba.len(), (w * h * 4) as usize, "RGBA is w*h*4 bytes");
}

#[test]
fn render_menu_to_rgba_matches_png_dimensions() {
    let menu = sample_menu();
    let opts = MenuOptions::default();

    let (rgba, w, h) = render_menu_to_rgba(&menu, &opts, 2.0);
    assert!(w > 0 && h > 0, "RGBA render must have positive dimensions");
    assert_eq!(rgba.len(), (w * h * 4) as usize, "RGBA is w*h*4 bytes");

    // The PNG entry point renders the same menu at the same size.
    let png = render_menu_to_png(&menu, &opts, 2.0);
    let (_p, pw, ph) = muri::render::decode_png(&png).expect("PNG decodes");
    assert_eq!((pw, ph), (w, h), "PNG and RGBA sizes agree");
}

#[test]
fn scale_changes_device_size() {
    let menu = sample_menu();
    let opts = MenuOptions::default();
    let (_a, w1, h1) = render_menu_to_rgba(&menu, &opts, 1.0);
    let (_b, w2, h2) = render_menu_to_rgba(&menu, &opts, 2.0);
    assert!(w2 > w1 && h2 > h1, "2x render must be larger than 1x");
}

/// The whole point of the cross-OS golden capability: forcing each OS produces a
/// visibly *different* OEM look from the same host and menu. Distinct per-OS
/// metrics (row height, padding, corner radius, colors) make the renders differ
/// regardless of which fonts the host has installed.
#[test]
fn forced_os_looks_differ() {
    let menu = sample_menu();
    let mac = render_menu_to_rgba(&menu, &forced(ThemeSource::MacOs(ThemeMode::Light)), 2.0);
    let win = render_menu_to_rgba(&menu, &forced(ThemeSource::Windows(ThemeMode::Light)), 2.0);
    let gnome = render_menu_to_rgba(&menu, &forced(ThemeSource::Gnome(ThemeMode::Light)), 2.0);

    assert_ne!(mac, win, "macOS and Windows forced looks must differ");
    assert_ne!(win, gnome, "Windows and GNOME forced looks must differ");
    assert_ne!(mac, gnome, "macOS and GNOME forced looks must differ");
}

/// Cross-OS goldens rendered from a single runner via the public API. Gated on
/// `bundled-fonts`: only then are the forced-theme faces host-independent (the
/// vendored OSS substitutes), so a committed PNG is byte-stable across
/// macOS/Windows/Linux CI. Without the feature a forced look falls back to the
/// host's sans-serif face, which is not cross-OS reproducible, so the pixel
/// comparison is skipped (the `forced_os_looks_differ` test still covers the
/// metric layer everywhere).
#[cfg(feature = "bundled-fonts")]
mod goldens {
    use super::*;
    use support::assert_golden_png;

    fn assert_forced_golden(source: ThemeSource, name: &str) {
        let png = render_menu_to_png(&sample_menu(), &forced(source), 2.0);
        assert_golden_png(name, &png);
    }

    #[test]
    fn headless_macos_golden() {
        assert_forced_golden(
            ThemeSource::MacOs(ThemeMode::Light),
            "headless-macos-light-2x",
        );
    }

    #[test]
    fn headless_windows_golden() {
        assert_forced_golden(
            ThemeSource::Windows(ThemeMode::Light),
            "headless-windows-light-2x",
        );
    }

    #[test]
    fn headless_gnome_golden() {
        assert_forced_golden(
            ThemeSource::Gnome(ThemeMode::Light),
            "headless-gnome-light-2x",
        );
    }
}
