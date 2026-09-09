//! Headless render snapshot: paints a representative menu straight to an
//! in-memory `tiny-skia` pixmap (no window, no tray) and saves it to
//! `target/muri-phase1-render.png`. Proves the shared scene drawer renders the
//! styled menu — flush-right colored values with no chevron column, section
//! headers, separators, a checkmark, and a provider logo — identically to what
//! the live macOS popup blits to its softbuffer surface.

use std::path::PathBuf;

use muri::render::paint::render_menu;
use muri::render::RasterDrawer;
use muri::{
    Align, Color, Flex, Font, Icon, Menu, MenuOptions, Row, Segment, StyleRun, Theme, Weight,
};

/// A tiny solid-color 16×16 PNG so the `draw_image` (leading logo) path is
/// exercised by the snapshot without shipping a binary asset.
fn swatch_png(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut pm = tiny_skia::Pixmap::new(16, 16).unwrap();
    for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
        let x = (i % 16) as i32;
        let y = (i / 16) as i32;
        // A filled rounded-ish blob: circle mask.
        let dx = x - 8;
        let dy = y - 8;
        let a = if dx * dx + dy * dy <= 49 { 255 } else { 0 };
        *px = tiny_skia::PremultipliedColorU8::from_rgba(
            (r as u16 * a as u16 / 255) as u8,
            (g as u16 * a as u16 / 255) as u8,
            (b as u16 * a as u16 / 255) as u8,
            a,
        )
        .unwrap();
    }
    pm.encode_png().unwrap()
}

fn demo_menu() -> Menu {
    let claude_logo = swatch_png(217, 119, 87); // Claude terracotta
    let codex_logo = swatch_png(80, 80, 90); // Codex slate

    Menu::new()
        // Provider group: Claude
        .section_header(
            Row::info()
                .leading(Icon::from_png_bytes(claude_logo))
                .segment(Segment::new("Claude").font(Font::system(13.0, Weight::Bold))),
        )
        .row(
            Row::new("switch:claude:me")
                .leading(Icon::Checkmark)
                .checked(true)
                .segments(vec![
                    Segment::new("me@example.com")
                        .flex(Flex::Grow)
                        .font(Font::system(13.0, Weight::Bold)),
                    // "47% / 89%" with the 89% span in system red.
                    Segment::new("47% / 89%")
                        .align(Align::Right)
                        .runs(vec![StyleRun::new(6, 3, Color::SystemRed)]),
                ]),
        )
        .row(Row::new("switch:claude:work").segments(vec![
            Segment::new("work@example.com").flex(Flex::Grow),
            Segment::new("12% / 30%")
                .align(Align::Right)
                .runs(vec![StyleRun::new(0, 3, Color::SystemGreen)]),
        ]))
        .separator()
        // Provider group: Codex
        .section_header(
            Row::info()
                .leading(Icon::from_png_bytes(codex_logo))
                .segment(Segment::new("Codex").font(Font::system(13.0, Weight::Bold))),
        )
        .row(Row::new("switch:codex:me").segments(vec![
            Segment::new("me@example.com").flex(Flex::Grow),
            Segment::new("3h 12m")
                .align(Align::Right)
                .runs(vec![StyleRun::new(0, 6, Color::SystemOrange)]),
        ]))
        .separator()
        .submenu(Row::new("settings").label("Settings"), Menu::new())
        .row(Row::new("quit").segments(vec![
            Segment::new("Quit").flex(Flex::Grow),
            Segment::new("usagio v0.5.4")
                .align(Align::Right)
                .color(Color::SecondaryLabel),
        ]))
}

fn target_dir() -> PathBuf {
    // CARGO_TARGET_TMPDIR is under target/; walk up to target/.
    let mut p = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    while p.file_name().map(|n| n != "target").unwrap_or(false) {
        if !p.pop() {
            break;
        }
    }
    p
}

#[test]
fn snapshot_dark_and_light_are_nonblank_and_saved() {
    let menu = demo_menu();
    let out_dir = target_dir();

    for (name, theme, highlight) in [
        ("muri-phase1-render.png", Theme::dark(), Some(1usize)),
        ("muri-phase1-render-light.png", Theme::light(), None),
    ] {
        let mut drawer = RasterDrawer::new(2.0);
        let laid = render_menu(
            &mut drawer,
            &menu,
            &theme,
            &MenuOptions::default(),
            highlight,
        );

        // The popup is a sensible size.
        assert!(laid.size.width >= 200.0, "width {}", laid.size.width);
        assert!(laid.size.height > 150.0, "height {}", laid.size.height);

        // Device pixmap tracks logical size * scale.
        let (dw, dh) = drawer.device_size();
        assert_eq!(dw, (laid.size.width * 2.0).round() as u32);
        assert_eq!(dh, (laid.size.height * 2.0).round() as u32);

        // Non-blank: a meaningful fraction of pixels are painted (panel fill).
        let painted = drawer
            .pixmap()
            .pixels()
            .iter()
            .filter(|p| p.alpha() > 0)
            .count();
        let total = (dw * dh) as usize;
        assert!(
            painted > total / 2,
            "expected the panel to fill most of the frame, got {painted}/{total}"
        );

        let png = drawer.encode_png();
        let path = out_dir.join(name);
        std::fs::write(&path, &png).unwrap();
        eprintln!("wrote {} ({} bytes)", path.display(), png.len());
    }

    // Interactive rows: 2 Claude accounts + 1 Codex account + Settings + Quit = 5.
    let mut drawer = RasterDrawer::new(1.0);
    let laid = render_menu(
        &mut drawer,
        &menu,
        &Theme::dark(),
        &MenuOptions::default(),
        None,
    );
    assert_eq!(laid.rows.iter().filter(|r| r.interactive).count(), 5);
}
