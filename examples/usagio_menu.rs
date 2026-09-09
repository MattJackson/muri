//! Rebuilds usagio's real tray menu through the muri API, proving the API can
//! express every feature usagio's hand-rolled `RowStyle` needs: provider group
//! headers with logos, accounts with a flush-right colored "S% / W%" value and
//! NO reserved chevron column, a checkmark + bold on the active account,
//! per-account detail submenus, and a greyed version tail on Quit.
//!
//! This constructs the menu and prints a tree; it does not open a UI (the
//! rendering backend is not implemented yet). Run with:
//!
//! ```sh
//! cargo run --example usagio_menu
//! ```

use muri::{Align, Color, Flex, Font, Icon, Item, Menu, Row, Segment, StyleRun, Weight};

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
        menu = menu.row(Row::new("noop").label("\u{2713} Active").enabled(false));
    } else {
        menu = menu.row(
            Row::new(format!("switch:{}:{}", acct.provider, acct.key))
                .label("Switch to this account"),
        );
    }
    if acct.supports_launch {
        menu = menu
            .row(Row::new(format!("launch:{}:{}", acct.provider, acct.key)).label("Launch client"));
    }
    if acct.supports_remove {
        menu = menu.row(
            Row::new(format!("remove:{}:{}", acct.provider, acct.key)).label("Remove\u{2026}"),
        );
    }
    menu
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

    // Bottom actions.
    menu = menu
        .submenu(
            Row::new("capture").label("Capture current login"),
            Menu::new(),
        )
        .submenu(Row::new("settings").label("Settings"), Menu::new());

    // "Quit ........ usagio vX" — label Grows, version tail is greyed + flush-right.
    menu = menu.row(Row::new("quit").segments(vec![
        Segment::new("Quit").flex(Flex::Grow),
        Segment::new(format!("usagio v{}", env!("CARGO_PKG_VERSION")))
            .align(Align::Right)
            .color(Color::SecondaryLabel),
    ]));

    menu
}

fn print_menu(menu: &Menu, depth: usize) {
    let pad = "  ".repeat(depth);
    for item in &menu.items {
        match item {
            Item::Separator => println!("{pad}----"),
            Item::SectionHeader(row) => println!("{pad}# {}", row_text(row)),
            Item::Row(row) => println!("{pad}- {} [{}]", row_text(row), row.id.as_str()),
            Item::Submenu { label, menu } => {
                println!("{pad}> {} [{}]", row_text(label), label.id.as_str());
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
}
