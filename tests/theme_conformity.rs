//! Cross-OS theme **conformity** goldens (issue #55).
//!
//! A forced theme ([`ThemeSource::MacOs`]/[`Windows`]/[`Gnome`], surfaced here
//! through the [`Theme::for_family`] presets they resolve to) must produce the
//! **same target-OS look on any host** — the whole point of forced themes and of
//! the single-machine cross-OS preview (#45/#54). These goldens pin that: the
//! per-OS **metrics** (row height, padding, column gap, corner radius, separator
//! color, checkmark gutter, semantic colors) are captured host-independently.
//!
//! Host independence comes from [`RasterDrawer::new_headless`], which disables
//! system-font discovery and loads the repo-vendored DejaVu pair (see
//! `tests/fonts/`) — so the shaped text is byte-identical on macOS, Windows, and
//! Linux CI. The proprietary target UI font (Segoe UI / SF Pro) is deliberately
//! *not* exercised here: #54 documents that true font parity needs the real face
//! installed, so a deterministic CI golden pins the metric layer, which is the
//! part that is genuinely host-independent. A regression that made
//! `ThemeSource::Windows` render with the host OS's metrics (not Windows') would
//! change these images and fail CI on every runner.
//!
//! Regenerate after an intentional theme-metric change:
//! `MURI_UPDATE_SNAPSHOTS=1 cargo test --test theme_conformity`, then re-run
//! without the env var to confirm determinism before committing the PNGs.

mod support;

use muri::render::paint::render_menu;
use muri::render::RasterDrawer;
use muri::{Align, Color, Flex, Icon, Menu, MenuOptions, OsFamily, Row, Segment, Theme};

use support::assert_golden;

/// One fixture that surfaces the metric tells that distinguish the three OS
/// themes: a flush-right label/value row, a checked leading-icon row (exercises
/// the checkmark gutter), a section header, a separator, and a secondary-colored
/// trailing value.
fn conformity_menu() -> Menu {
    Menu::new()
        .section_header(Row::label_only("Account"))
        .row(Row::new("acct:me").segments(vec![
            Segment::new("me@example.com").flex(Flex::Grow),
            Segment::new("47%").align(Align::Right).color(Color::SystemGreen),
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
            Segment::new("v0.10").align(Align::Right).color(Color::SecondaryLabel),
        ]))
}

fn assert_family(family: OsFamily, dark: bool, name: &str) {
    let theme = Theme::for_family(family, dark);
    let mut drawer = RasterDrawer::new_headless(2.0);
    render_menu(
        &mut drawer,
        &conformity_menu(),
        &theme,
        &MenuOptions::default(),
        None,
    );
    assert_golden(name, drawer.framebuffer());
}

#[test]
fn conformity_macos_light() {
    assert_family(OsFamily::MacOs, false, "conformity-macos-light-2x");
}

#[test]
fn conformity_macos_dark() {
    assert_family(OsFamily::MacOs, true, "conformity-macos-dark-2x");
}

#[test]
fn conformity_windows_light() {
    assert_family(OsFamily::Windows, false, "conformity-windows-light-2x");
}

#[test]
fn conformity_windows_dark() {
    assert_family(OsFamily::Windows, true, "conformity-windows-dark-2x");
}

#[test]
fn conformity_gnome_light() {
    assert_family(OsFamily::Gnome, false, "conformity-gnome-light-2x");
}

#[test]
fn conformity_gnome_dark() {
    assert_family(OsFamily::Gnome, true, "conformity-gnome-dark-2x");
}
