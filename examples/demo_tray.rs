//! A runnable demo: installs a real macOS tray icon whose click opens muri's
//! custom-drawn, styled popup — provider groups with flush-right colored values
//! (no chevron column), a checkmark on the active account, separators, **flyout
//! submenus** (the account detail panel and Settings open a second panel beside
//! the row), and a greyed version tail on Quit.
//!
//! ```sh
//! cargo run --example demo_tray
//! ```
//!
//! On macOS this shows a status-bar icon; left-click it to open the popup, hover
//! to highlight a row, hover a `›` row to open its flyout to the right, click a
//! leaf row to fire its id (printed to stdout) and close, or click outside to
//! dismiss. Clicking "Quit" exits. On other platforms `Tray::run` is not
//! implemented yet (Windows) or unsupported (Linux).

use muri::{Align, Color, Flex, Font, Icon, Menu, Row, Segment, StyleRun, Tray, Weight};

/// Build a tiny solid-color circular 16×16 PNG so the demo ships no binary asset.
fn swatch_png(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut pm = tiny_skia::Pixmap::new(16, 16).unwrap();
    for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
        let (x, y) = ((i % 16) as i32 - 8, (i / 16) as i32 - 8);
        let a = if x * x + y * y <= 49 { 255u8 } else { 0 };
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

/// The per-account detail panel shown as a flyout beside an account row: reset
/// windows (flush-right values), an "updated" line, then the account actions.
fn account_submenu(slug: &str) -> Menu {
    Menu::new()
        .row(Row::info().segments(vec![
            Segment::new("Session resets in").flex(Flex::Grow),
            Segment::new("3h 12m")
                .align(Align::Right)
                .color(Color::SecondaryLabel),
        ]))
        .row(Row::info().segments(vec![
            Segment::new("Weekly resets in").flex(Flex::Grow),
            Segment::new("2d 4h")
                .align(Align::Right)
                .color(Color::SecondaryLabel),
        ]))
        .row(Row::info().segment(Segment::new("updated 1m ago").color(Color::SecondaryLabel)))
        .separator()
        .row(Row::new(format!("switch:{slug}")).label("Switch to this account"))
        .row(Row::new(format!("launch:{slug}")).label("Launch client"))
        .row(Row::new(format!("remove:{slug}")).label("Remove…"))
}

fn demo_menu() -> Menu {
    let claude_logo = swatch_png(217, 119, 87);
    let codex_logo = swatch_png(80, 80, 90);

    Menu::new()
        .section_header(
            Row::info()
                .leading(Icon::from_png_bytes(claude_logo))
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
            account_submenu("claude:me"),
        )
        .submenu(
            Row::new("acct:claude:work").segments(vec![
                Segment::new("work@example.com").flex(Flex::Grow),
                Segment::new("12% / 30%")
                    .align(Align::Right)
                    .runs(vec![StyleRun::new(0, 3, Color::SystemGreen)]),
            ]),
            account_submenu("claude:work"),
        )
        .separator()
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
        .submenu(
            Row::new("settings").label("Settings"),
            Menu::new()
                .row(Row::new("settings:apikey").label("API key…"))
                .row(Row::new("settings:autoswap").label("Auto-swap accounts"))
                .separator()
                .row(Row::new("settings:notifications").label("Notifications…")),
        )
        .row(Row::new("quit").segments(vec![
            Segment::new("Quit").flex(Flex::Grow),
            Segment::new(concat!("usagio v", env!("CARGO_PKG_VERSION")))
                .align(Align::Right)
                .color(Color::SecondaryLabel),
        ]))
}

fn main() {
    let icon = Icon::from_png_bytes(swatch_png(120, 170, 255));
    let tray = Tray::new(icon)
        .tooltip("muri demo")
        .menu(demo_menu())
        .on_click(|id| {
            println!("clicked: {}", id.as_str());
            if id.as_str() == "quit" {
                std::process::exit(0);
            }
        });

    println!("muri demo_tray: click the status-bar icon to open the styled popup.");
    if let Err(e) = tray.run() {
        eprintln!("tray error: {e}");
    }
}
