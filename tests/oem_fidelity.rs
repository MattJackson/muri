//! OEM-fidelity golden regressions for the 0.10.8 macOS fixes, driven entirely
//! through the shipped display-free public API ([`muri::render_menu_to_png`] /
//! [`muri::render_menu_to_rgba`], issue #59) so the device-inspected fixes
//! become automated pixel regressions:
//!
//! * **#56 — bold system face.** A row whose segment carries `Weight::Bold`
//!   must actually paint a heavier face than the same text at `Weight::Regular`.
//!   Before #56 a bold request could silently resolve to the light face; here a
//!   feature-independent *ink* comparison (bold must lay down strictly more ink
//!   than regular) and a committed golden pin that bold is applied, not
//!   downgraded.
//!
//! * **#57 — macOS metrics + SF tracking.** A representative menu forced to the
//!   macOS look pins the tightened NSMenu row height / insets and the baked-in
//!   SF letter-spacing. A follow-up that loosened the metrics or dropped the
//!   tracking would move these pixels and fail the golden.
//!
//! Pixel goldens are gated on the `bundled-fonts` feature (like the goldens in
//! `tests/headless.rs`): only then do the forced-theme faces resolve to the
//! vendored OFL substitutes (Inter / Selawik / Cantarell), making the PNGs
//! byte-stable across macOS/Windows/Linux runners. The non-golden assertions
//! (ink heavier, bold changes pixels, per-OS looks distinct) run on every
//! feature set — they need no reproducible font, only a real bold face.
//!
//! Regenerate the goldens after an intentional visual change with the
//! `bundled-fonts` feature enabled:
//! `MURI_UPDATE_SNAPSHOTS=1 cargo test --features bundled-fonts --test oem_fidelity`,
//! then re-run without the env var to confirm determinism before committing.

mod support;

use muri::{
    render_menu_to_rgba, Align, Color, Flex, Font, Icon, Menu, MenuOptions, Row, Segment,
    ThemeMode, ThemeSource, Weight,
};

/// A `MenuOptions` forced to a target-OS look, so the render is host-independent
/// (the whole point of the cross-OS preview / forced themes).
fn forced(source: ThemeSource) -> MenuOptions {
    MenuOptions::default().theme(source)
}

/// A single-row menu whose only styled span is one explicit-weight segment. The
/// text and layout are identical for every weight, so a bold vs regular render
/// differs *only* by the applied face — the exact thing #56 has to get right.
fn weighted_menu(weight: Weight) -> Menu {
    Menu::new().row(Row::new("row").segments(vec![
        Segment::new("Heavier ink means bold applied").font(Font::system(13.0, weight)),
    ]))
}

/// A representative multi-row menu with one weight-controlled row, used to prove
/// that flipping a single row to bold changes the composited pixels.
fn mixed_menu(bold_first: bool) -> Menu {
    let weight = if bold_first {
        Weight::Bold
    } else {
        Weight::Regular
    };
    Menu::new()
        .row(Row::new("who").segments(vec![
            Segment::new("Signed in").font(Font::system(13.0, weight)),
        ]))
        .row(Row::new("mail").segments(vec![Segment::new("me@example.com")]))
        .separator()
        .row(Row::new("quit").label("Quit"))
}

/// The #57 metrics/tracking fixture: a section header, a flush-right
/// tracking-sensitive label/value row (long enough that SF letter-spacing
/// visibly shifts glyph positions), a checked leading-icon row (checkmark
/// gutter), a separator, and a secondary-colored trailing value.
fn metrics_menu() -> Menu {
    Menu::new()
        .section_header(Row::info().label("Metrics & Tracking"))
        .row(Row::new("acct").segments(vec![
            Segment::new("tracking-sensitive-label@example.com").flex(Flex::Grow),
            Segment::new("47%")
                .align(Align::Right)
                .color(Color::SystemGreen),
        ]))
        .row(
            Row::new("on")
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

/// Total ink laid down by a straight-alpha RGBA render: the sum of per-pixel
/// darkness (`255 - luma`) over an opaque light background. A heavier face
/// covers more pixels with darker glyph strokes, so a bold render's ink is
/// strictly greater than the same text at regular weight — the direct,
/// font-independent signal that bold was applied and not downgraded (#56).
fn ink(rgba: &[u8]) -> u64 {
    rgba.chunks_exact(4)
        .map(|px| {
            let luma = 0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32;
            (255.0 - luma) as u64
        })
        .sum()
}

/// #56: a bold segment must paint a strictly heavier face than the identical
/// text at regular weight. Runs on every feature set — it needs a real bold
/// face, not a reproducible one, so it is the guard that catches a silent
/// weight downgrade even without `bundled-fonts`.
#[test]
fn bold_row_inks_heavier_than_regular() {
    let opts = forced(ThemeSource::MacOs(ThemeMode::Light));
    let (bold, _, _) = render_menu_to_rgba(&weighted_menu(Weight::Bold), &opts, 2.0);
    let (reg, _, _) = render_menu_to_rgba(&weighted_menu(Weight::Regular), &opts, 2.0);

    assert_ne!(
        bold, reg,
        "#56: a bold segment must produce different pixels than the same text at regular weight"
    );

    let (bold_ink, reg_ink) = (ink(&bold), ink(&reg));
    assert!(
        bold_ink > reg_ink,
        "#56: bold must lay down more ink than regular (bold {bold_ink} vs regular {reg_ink}); \
         equal or lighter ink means the bold face was silently downgraded"
    );
}

/// #56: flipping a single row of a representative menu to bold must change the
/// composited pixels vs the all-regular menu. Feature-independent.
#[test]
fn bold_menu_differs_from_all_regular() {
    let opts = forced(ThemeSource::MacOs(ThemeMode::Light));
    let (with_bold, _, _) = render_menu_to_rgba(&mixed_menu(true), &opts, 2.0);
    let (all_regular, _, _) = render_menu_to_rgba(&mixed_menu(false), &opts, 2.0);
    assert_ne!(
        with_bold, all_regular,
        "#56: a bold row must change the menu's pixels relative to the all-regular menu"
    );
}

/// Cross-OS distinctness in **dark** mode, expanding on the light-mode
/// `forced_os_looks_differ` in `tests/headless.rs`: the three forced OEM looks
/// must be mutually distinct from a single host. Feature-independent — the
/// distinct per-OS metrics/colors carry the difference regardless of host fonts.
#[test]
fn forced_os_looks_differ_dark() {
    let menu = metrics_menu();
    let mac = render_menu_to_rgba(&menu, &forced(ThemeSource::MacOs(ThemeMode::Dark)), 2.0);
    let win = render_menu_to_rgba(&menu, &forced(ThemeSource::Windows(ThemeMode::Dark)), 2.0);
    let gnome = render_menu_to_rgba(&menu, &forced(ThemeSource::Gnome(ThemeMode::Dark)), 2.0);

    assert_ne!(mac, win, "macOS and Windows dark forced looks must differ");
    assert_ne!(
        win, gnome,
        "Windows and GNOME dark forced looks must differ"
    );
    assert_ne!(mac, gnome, "macOS and GNOME dark forced looks must differ");
}

/// Pixel goldens, host-reproducible only under `bundled-fonts` (the vendored
/// OFL substitutes). See the module docs.
#[cfg(feature = "bundled-fonts")]
mod goldens {
    use super::*;
    use muri::render_menu_to_png;
    use support::assert_golden_png;

    /// #56: committed golden for the regular render of [`weighted_menu`]; its
    /// bold sibling below must differ from it.
    #[test]
    fn oem_bold_regular_golden() {
        let png = render_menu_to_png(
            &weighted_menu(Weight::Regular),
            &forced(ThemeSource::MacOs(ThemeMode::Light)),
            2.0,
        );
        assert_golden_png("oem-bold-regular-macos-2x", &png);
    }

    /// #56: committed golden for the bold render. A regression that downgraded
    /// the bold face would repaint the lighter face and fail this golden.
    #[test]
    fn oem_bold_bold_golden() {
        let png = render_menu_to_png(
            &weighted_menu(Weight::Bold),
            &forced(ThemeSource::MacOs(ThemeMode::Light)),
            2.0,
        );
        assert_golden_png("oem-bold-bold-macos-2x", &png);
    }

    /// #57: committed golden pinning the tightened macOS row height / insets and
    /// the baked-in SF tracking. Loosening the metrics or dropping the tracking
    /// moves these pixels.
    #[test]
    fn oem_macos_metrics_golden() {
        let png = render_menu_to_png(
            &metrics_menu(),
            &forced(ThemeSource::MacOs(ThemeMode::Light)),
            2.0,
        );
        assert_golden_png("oem-macos-metrics-2x", &png);
    }

    fn assert_forced_golden(source: ThemeSource, name: &str) {
        let png = render_menu_to_png(&metrics_menu(), &forced(source), 2.0);
        assert_golden_png(name, &png);
    }

    /// Dark-mode cross-OS goldens (light is already pinned in
    /// `tests/headless.rs`): the target-OS look reproduced host-independently.
    #[test]
    fn oem_macos_dark_golden() {
        assert_forced_golden(ThemeSource::MacOs(ThemeMode::Dark), "oem-macos-dark-2x");
    }

    #[test]
    fn oem_windows_dark_golden() {
        assert_forced_golden(ThemeSource::Windows(ThemeMode::Dark), "oem-windows-dark-2x");
    }

    #[test]
    fn oem_gnome_dark_golden() {
        assert_forced_golden(ThemeSource::Gnome(ThemeMode::Dark), "oem-gnome-dark-2x");
    }
}
