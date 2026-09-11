//! Headless render snapshot: paints a representative menu straight to an
//! in-memory [`Framebuffer`] (no window, no tray) and saves it to
//! `target/muri-phase1-render.png`. Proves the shared scene drawer renders the
//! styled menu — flush-right colored values with no chevron column, section
//! headers, separators, a checkmark, and a provider logo — identically to what
//! the live macOS popup blits to its surface.

use std::path::PathBuf;

use muri::render::paint::render_menu;
use muri::render::{Framebuffer, RasterDrawer};
use muri::{
    place_flyout, Align, Color, Flex, FlyoutSide, Font, Icon, LogicalPoint, LogicalRect,
    LogicalSize, Menu, MenuOptions, Rgba, Row, Segment, StyleRun, Theme, Weight,
};

/// A tiny solid-color 16×16 PNG so the `draw_image` (leading logo) path is
/// exercised by the snapshot without shipping a binary asset.
fn swatch_png(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut fb = Framebuffer::new(16, 16);
    let px = fb.pixels_mut();
    for i in 0..256usize {
        let x = (i % 16) as i32;
        let y = (i / 16) as i32;
        // A filled rounded-ish blob: circle mask.
        let dx = x - 8;
        let dy = y - 8;
        let a: u16 = if dx * dx + dy * dy <= 49 { 255 } else { 0 };
        let o = i * 4;
        // Premultiplied RGBA (what `Framebuffer` stores).
        px[o] = (r as u16 * a / 255) as u8;
        px[o + 1] = (g as u16 * a / 255) as u8;
        px[o + 2] = (b as u16 * a / 255) as u8;
        px[o + 3] = a as u8;
    }
    fb.encode_png()
}

fn demo_menu() -> Menu {
    let claude_logo = swatch_png(217, 119, 87); // Claude terracotta
    let codex_logo = swatch_png(80, 80, 90); // Codex slate

    Menu::new()
        // Provider group: Claude
        .section_header(
            Row::default()
                .leading(Icon::from_png(claude_logo))
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
            Row::default()
                .leading(Icon::from_png(codex_logo))
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
            .framebuffer()
            .pixels()
            .chunks_exact(4)
            .filter(|p| p[3] > 0)
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

/// The per-account detail submenu shown in the flyout: reset-window info rows
/// (flush-right values, no chevron column), a secondary "updated" line, a
/// separator, then the switch/launch/remove actions.
fn account_submenu() -> Menu {
    Menu::new()
        .row(Row::default().segments(vec![
            Segment::new("Session resets in").flex(Flex::Grow),
            Segment::new("3h 12m")
                .align(Align::Right)
                .color(Color::SecondaryLabel),
        ]))
        .row(Row::default().segments(vec![
            Segment::new("Weekly resets in").flex(Flex::Grow),
            Segment::new("2d 4h")
                .align(Align::Right)
                .color(Color::SecondaryLabel),
        ]))
        .row(Row::default().segment(Segment::new("updated 1m ago").color(Color::SecondaryLabel)))
        .separator()
        .row(Row::new("switch:claude:me").label("Switch to this account"))
        .row(Row::new("launch:claude:me").label("Launch client"))
        .row(Row::new("remove:claude:me").label("Remove…"))
}

/// A parent menu whose active account is an `Item::Submenu` opening
/// [`account_submenu`]. Item index 1 is the account submenu (the flyout parent).
fn flyout_parent_menu(child: Menu) -> Menu {
    let claude_logo = swatch_png(217, 119, 87);
    Menu::new()
        .section_header(
            Row::default()
                .leading(Icon::from_png(claude_logo))
                .segment(Segment::new("Claude").font(Font::system(13.0, Weight::Bold))),
        )
        .submenu(
            Row::new("acct:claude:me")
                .leading(Icon::Checkmark)
                .checked(true)
                .segments(vec![
                    Segment::new("me@example.com")
                        .flex(Flex::Grow)
                        .font(Font::system(13.0, Weight::Bold)),
                    Segment::new("47% / 89%")
                        .align(Align::Right)
                        .runs(vec![StyleRun::new(6, 3, Color::SystemRed)]),
                ]),
            child,
        )
        .separator()
        .submenu(
            Row::new("settings").label("Settings"),
            Menu::new().row(Row::new("apikey").label("API key…")),
        )
        .row(Row::new("quit").segments(vec![
            Segment::new("Quit").flex(Flex::Grow),
            Segment::new("usagio v0.5.4")
                .align(Align::Right)
                .color(Color::SecondaryLabel),
        ]))
}

/// Phase 2 proof: render a parent panel **with an open flyout** and composite the
/// two panels onto one canvas (the flyout to the right of its hovered parent
/// row), exactly as the live macOS backend positions its second popup window via
/// [`place_flyout`]. Saved to `target/muri-phase2-flyout.png`.
#[test]
fn snapshot_flyout_parent_plus_child_is_saved() {
    let out_dir = target_dir();
    let scale = 2.0_f32;
    let theme = Theme::dark();
    let opts = MenuOptions::default();

    let child = account_submenu();
    let parent = flyout_parent_menu(child.clone());
    let submenu_index = 1usize; // the account Item::Submenu

    // Parent panel, with the account row highlighted to read as "open".
    let mut pd = RasterDrawer::new(scale);
    let plaid = render_menu(&mut pd, &parent, &theme, &opts, Some(submenu_index));

    // Child flyout panel.
    let mut cd = RasterDrawer::new(scale);
    let claid = render_menu(&mut cd, &child, &theme, &opts, None);

    // Place the flyout beside the hovered submenu row (huge work area → opens
    // right, flush against the parent's edge, top-aligned to the row).
    let row = plaid
        .rows
        .iter()
        .find(|r| r.index == submenu_index)
        .expect("submenu row is laid out")
        .rect;
    let parent_rect = LogicalRect::new(LogicalPoint::new(0.0, 0.0), plaid.size);
    let work = LogicalRect::new(
        LogicalPoint::new(0.0, 0.0),
        LogicalSize::new(4000.0, 4000.0),
    );
    let placement = place_flyout(parent_rect, row, claid.size, work);
    assert_eq!(placement.side, FlyoutSide::Right);
    assert!(placement.origin.x >= parent_rect.max_x() - 0.5);

    // Composite both device pixmaps onto one canvas over a desktop backdrop.
    let (pw, ph) = pd.device_size();
    let (cw, ch) = cd.device_size();
    let ox = (placement.origin.x * scale).round() as i32;
    let oy = (placement.origin.y * scale).round() as i32;
    let margin = (10.0 * scale) as u32;
    let canvas_w = ((ox as u32 + cw).max(pw)) + margin * 2;
    let canvas_h = ((oy as u32 + ch).max(ph)) + margin * 2;
    let mut canvas = Framebuffer::new(canvas_w, canvas_h);
    canvas.fill(Rgba::opaque(28, 28, 30)); // desktop backdrop

    canvas.draw(pd.framebuffer(), margin as i32, margin as i32);
    canvas.draw(cd.framebuffer(), margin as i32 + ox, margin as i32 + oy);

    let png = canvas.encode_png();
    let path = out_dir.join("muri-phase2-flyout.png");
    std::fs::write(&path, &png).unwrap();
    eprintln!("wrote {} ({} bytes)", path.display(), png.len());

    // The flyout exposes the switch/launch/remove actions as interactive rows.
    assert!(claid.rows.iter().filter(|r| r.interactive).count() >= 3);
}
