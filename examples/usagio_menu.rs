//! Rebuilds usagio's real tray menu through the muri API, proving the API can
//! express every feature usagio's hand-rolled `RowStyle` needs: provider group
//! headers with logos, accounts with a flush-right colored "S% / W%" value and
//! NO reserved chevron column, a checkmark + bold on the active account,
//! per-account detail submenus, populated Capture/Settings submenus with nested
//! flyouts and checkable rows, leading/trailing icons (PNG, SVG, checkmark, and
//! a named symbol), and a greyed version tail on Quit.
//!
//! It also shows the pure, GUI-free parts of the API a consumer can rely on
//! today: resolving semantic colors against a custom [`muri::Theme`], resolving
//! the flush-right layout with [`muri::layout`], and building a
//! [`muri::ContextMenu`] for the pointer-anchored path.
//!
//! This constructs the menu and prints a tree; it does not open a UI (the
//! rendering backend is not implemented yet). Run with:
//!
//! ```sh
//! cargo run --example usagio_menu
//! ```

use muri::layout::{resolve_segments, SegmentMetrics};
use muri::{
    Align, Color, ContextMenu, Flex, Font, Icon, Item, Menu, Row, Segment, StyleRun, Theme, Weight,
};

/// A stand-in for usagio's `Snapshot` so this example is self-contained.
struct Account {
    provider: &'static str,
    key: &'static str,
    display: &'static str,
    /// e.g. "47% / 89%" or a locked countdown "3h 12m"
    trailing: &'static str,
    /// (utf16_start, utf16_len, color) severity spans within `trailing`
    severity: Vec<(usize, usize, Color)>,
    active: bool,
    supports_launch: bool,
    supports_remove: bool,
}

struct Group {
    display_name: &'static str,
    // In the real crate this is the 16px provider logo PNG bytes.
    icon_png: &'static [u8],
    accounts: Vec<Account>,
}

fn severity_runs(spans: &[(usize, usize, Color)]) -> Vec<StyleRun> {
    spans
        .iter()
        .map(|&(start, len, color)| StyleRun::new(start, len, color))
        .collect()
}

/// The per-account detail submenu: reset-window info rows, an "updated …" line,
/// then Switch / Launch / Remove actions.
fn account_submenu(acct: &Account) -> Menu {
    let mut menu = Menu::new()
        // Info rows read as normal text but are non-interactive (id = none).
        .row(Row::info().label("Session resets in 2h 41m"))
        .row(Row::info().label("Weekly resets in 3d 4h"))
        .row(Row::info().segment(Segment::new("updated 12s ago").color(Color::SecondaryLabel)))
        .separator();

    if acct.active {
        menu = menu.row(
            Row::new("noop")
                .leading(Icon::Checkmark)
                .segment(Segment::new("Active").color(Color::SystemGreen))
                .enabled(false),
        );
    } else {
        menu = menu.row(
            Row::new(format!("switch:{}:{}", acct.provider, acct.key))
                .label("Switch to this account"),
        );
    }
    if acct.supports_launch {
        menu = menu.row(
            Row::new(format!("launch:{}:{}", acct.provider, acct.key))
                .label("Launch client")
                // A named symbol trails the action (SF Symbol on macOS).
                .trailing(Icon::Symbol("arrow.up.forward.app")),
        );
    }
    if acct.supports_remove {
        menu = menu.row(
            Row::new(format!("remove:{}:{}", acct.provider, acct.key)).label("Remove\u{2026}"),
        );
    }
    menu
}

/// The Capture submenu: one entry per provider to capture the current login.
fn capture_submenu(groups: &[Group]) -> Menu {
    let mut menu = Menu::new();
    for group in groups {
        menu = menu.row(
            Row::new(format!("capture:{}", group.display_name.to_lowercase()))
                .leading(Icon::from_png_bytes(group.icon_png))
                .label(format!("Capture {} login", group.display_name)),
        );
    }
    menu
}

/// The Settings submenu: a checkable toggle, an auto-swap threshold flyout, and
/// a backup flyout — exercising nested submenus, `checked`, and an SVG icon.
fn settings_submenu() -> Menu {
    let autoswap = Menu::new()
        .row(Row::new("autoswap:off").checked(false).label("Off"))
        .row(Row::new("autoswap:80").checked(false).label("At 80%"))
        .row(Row::new("autoswap:90").checked(true).label("At 90%"))
        .separator()
        .row(Row::new("autoswap:now").label("Swap now"));

    let backup = Menu::new()
        .row(Row::new("backup:save").label("Save backup\u{2026}"))
        .row(Row::new("backup:restore").label("Restore backup\u{2026}"));

    Menu::new()
        .row(
            Row::new("notifications:limits")
                .checked(true)
                .label("Notify near limits"),
        )
        .submenu(Row::new("autoswap").label("Auto-swap threshold"), autoswap)
        .submenu(
            Row::new("backup")
                // An SVG leading icon, rasterized per-DPI at draw time.
                .leading(Icon::from_svg_bytes(
                    br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#.as_slice(),
                ))
                .label("Backup"),
            backup,
        )
        .separator()
        .row(Row::new("refresh:now").label("Refresh now"))
}

fn build(groups: &[Group]) -> Menu {
    let mut menu = Menu::new();

    for group in groups {
        // Bold, logo'd, non-interactive group header (Claude, then Codex).
        menu = menu.section_header(
            Row::info()
                .leading(Icon::from_png_bytes(group.icon_png))
                .segment(Segment::new(group.display_name).font(Font::system(13.0, Weight::Bold))),
        );

        for acct in &group.accounts {
            // The label Grows to eat the leftover width, so the value segment
            // sits flush at the true right edge — no reserved chevron column.
            let label = Segment::new(acct.display)
                .flex(Flex::Grow)
                .font(if acct.active {
                    Font::system(13.0, Weight::Bold)
                } else {
                    Font::system(13.0, Weight::Regular)
                });

            let value = Segment::new(acct.trailing)
                .align(Align::Right)
                .runs(severity_runs(&acct.severity));

            let mut label_row = Row::new(format!("switch:{}:{}", acct.provider, acct.key))
                .segments(vec![label, value])
                .checked(acct.active);
            if acct.active {
                label_row = label_row.leading(Icon::Checkmark);
            }

            menu = menu.submenu(label_row, account_submenu(acct));
        }

        menu = menu.separator();
    }

    // Bottom actions — now with populated submenus.
    menu = menu
        .submenu(
            Row::new("capture").label("Capture current login"),
            capture_submenu(groups),
        )
        .submenu(Row::new("settings").label("Settings"), settings_submenu());

    // "Quit ........ usagio vX" — label Grows, version tail is greyed + flush-right.
    menu = menu.row(Row::new("quit").segments(vec![
        Segment::new("Quit").flex(Flex::Grow),
        Segment::new(format!("usagio v{}", env!("CARGO_PKG_VERSION")))
            .align(Align::Right)
            .color(Color::SecondaryLabel),
    ]));

    menu
}

fn icon_tag(icon: &Option<Icon>) -> &'static str {
    match icon {
        None => "",
        Some(Icon::Checkmark) => " [check]",
        Some(Icon::Png(_)) => " [png]",
        Some(Icon::Svg(_)) => " [svg]",
        Some(Icon::Symbol(_)) => " [symbol]",
    }
}

fn print_row(row: &Row, prefix: &str, pad: &str) {
    let check = match row.checked {
        Some(true) => "[x] ",
        Some(false) => "[ ] ",
        None => "",
    };
    let dim = if row.enabled { "" } else { " (disabled)" };
    println!(
        "{pad}{prefix}{}{check}{}{}{}  [{}]",
        icon_tag(&row.leading),
        row_text(row),
        icon_tag(&row.trailing),
        dim,
        row.id.as_str(),
    );
}

fn print_menu(menu: &Menu, depth: usize) {
    let pad = "  ".repeat(depth);
    for item in &menu.items {
        match item {
            Item::Separator => println!("{pad}----"),
            Item::SectionHeader(row) => {
                println!("{pad}#{} {}", icon_tag(&row.leading), row_text(row))
            }
            Item::Row(row) => print_row(row, "- ", &pad),
            Item::Submenu { label, menu } => {
                print_row(label, "> ", &pad);
                print_menu(menu, depth + 1);
            }
        }
    }
}

fn row_text(row: &Row) -> String {
    row.segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("  ")
}

fn demo_theme_resolution() {
    // A consumer can resolve semantic colors against any theme with no GUI.
    let dark = Theme::dark();
    let label = dark.resolve(Color::Label);
    let red = dark.resolve(Color::SystemRed);
    println!(
        "theme resolution (dark): Label -> rgba({},{},{},{}), SystemRed -> rgba({},{},{},{})",
        label.r, label.g, label.b, label.a, red.r, red.g, red.b, red.a,
    );
}

fn demo_flush_right_layout() {
    // The "Quit ...... usagio v1" row: a Grow label + a Fixed, right-aligned
    // version tail. Given measured intrinsic widths, the layout engine flushes
    // the tail to the right edge with no reserved column.
    let content_width = 220.0;
    let segs = [
        SegmentMetrics::new(30.0, Flex::Grow, Align::Left), // "Quit"
        SegmentMetrics::new(60.0, Flex::Fixed, Align::Right), // "usagio v1"
    ];
    let boxes = resolve_segments(&segs, content_width);
    println!(
        "flush-right layout: content {content_width}px -> tail text starts at x={} (right edge {})",
        boxes[1].text_x,
        content_width - 60.0,
    );
}

fn demo_context_menu() {
    // The pointer-anchored primitive that also works on Linux.
    let menu = Menu::new()
        .row(Row::new("copy").label("Copy"))
        .row(Row::new("paste").label("Paste"))
        .separator()
        .row(Row::new("select-all").label("Select All"));
    let cm = ContextMenu::new(menu).on_click(|id| println!("context click: {}", id.as_str()));
    // Exercise the dispatch path without a GUI.
    cm.dispatch(&"copy".into());
    println!("context menu has {} items", cm.menu().len());
}

fn main() {
    let groups = vec![
        Group {
            display_name: "Claude",
            icon_png: b"<claude.png bytes>",
            accounts: vec![
                Account {
                    provider: "claude",
                    key: "me@example.com",
                    display: "me@example.com",
                    trailing: "47% / 89%",
                    // color the "89%" span red (utf16 offset 6, len 3)
                    severity: vec![(6, 3, Color::SystemRed)],
                    active: true,
                    supports_launch: true,
                    supports_remove: true,
                },
                Account {
                    provider: "claude",
                    key: "work@example.com",
                    display: "work@example.com",
                    trailing: "12% / 30%",
                    severity: vec![],
                    active: false,
                    supports_launch: true,
                    supports_remove: true,
                },
            ],
        },
        Group {
            display_name: "Codex",
            icon_png: b"<codex.png bytes>",
            accounts: vec![Account {
                provider: "codex",
                key: "me@example.com",
                display: "me@example.com",
                trailing: "3h 12m",
                severity: vec![(0, 6, Color::SystemOrange)],
                active: false,
                supports_launch: false,
                supports_remove: true,
            }],
        },
    ];

    let menu = build(&groups);
    println!("usagio menu, as built through the muri API:\n");
    print_menu(&menu, 0);

    println!("\n--- pure API demos (no GUI needed) ---");
    demo_theme_resolution();
    demo_flush_right_layout();
    demo_context_menu();
}
