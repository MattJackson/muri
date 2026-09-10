//! The 1.0 golden-image snapshot suite (`docs/design/spec/50-testing-verification.md`
//! §3.2–§3.5): render a fixture menu headlessly through
//! [`muri::render::RasterDrawer::new_headless`] (system fonts disabled, a
//! repo-vendored DejaVu Sans regular+bold pair loaded instead — see
//! `tests/fonts/`) and compare the resulting `Pixmap` against a committed
//! reference PNG under `tests/snapshots/`, with a small tolerance (§3.2, see
//! `tests/support::assert_golden`).
//!
//! Run `MURI_UPDATE_SNAPSHOTS=1 cargo test --test golden` once to (re)write the
//! references after an intentional visual change, then re-run without the env
//! var to confirm the suite is deterministic before committing the PNGs.
//!
//! These goldens pin the **vendored-font raster path**, not the live
//! system-font look (§3.3) — the text/raster engine is expected to be
//! re-baselined in a later wave; that is expected and does not indicate a
//! regression in the harness itself.

mod support;

use muri::render::paint::render_menu;
use muri::render::{Framebuffer, RasterDrawer};
use muri::{
    place_flyout, Align, Color, Flex, FlyoutSide, Icon, LogicalPoint, LogicalRect, LogicalSize,
    Menu, MenuOptions, Rgba, Row, Segment, Theme,
};

use support::assert_golden;

/// The flush-right guarantee ([`01-api-contract.md`] §2, muri's reason to
/// exist): a `Flex::Grow` label butts up against a right-aligned value with
/// **no reserved chevron column** (no submenu row, no leading icon column in
/// this fixture).
fn flush_right_menu() -> Menu {
    Menu::new()
        .row(Row::new("acct:me").segments(vec![
            Segment::new("a-fairly-long-account-label@example.com").flex(Flex::Grow),
            Segment::new("42%").align(Align::Right),
        ]))
        .row(Row::new("acct:work").segments(vec![
            Segment::new("work").flex(Flex::Grow),
            Segment::new("100%")
                .align(Align::Right)
                .color(Color::SystemGreen),
        ]))
        .separator()
        .row(Row::new("quit").segments(vec![
            Segment::new("Quit").flex(Flex::Grow),
            Segment::new("v1.0")
                .align(Align::Right)
                .color(Color::SecondaryLabel),
        ]))
}

/// Every other coverage-matrix `Item` kind in one menu: a section header, a
/// checked leading-icon row, a separator, and a visible **disabled** row (the
/// `PredefinedMenuItem` honesty rule, §2.6/§4, renders inemulable items as a
/// disabled row rather than dropping them).
fn item_kinds_menu() -> Menu {
    Menu::new()
        .section_header(Row::info().label("Accounts"))
        .row(
            Row::new("acct:me")
                .leading(Icon::Checkmark)
                .checked(true)
                .label("me@example.com"),
        )
        .separator()
        .row(
            Row::new("acct:disabled")
                .enabled(false)
                .label("Unavailable (signed out)"),
        )
}

/// The parent panel of the flyout composite: a submenu row whose flyout is
/// [`account_submenu`].
fn flyout_parent_menu() -> Menu {
    Menu::new()
        .section_header(Row::info().label("Claude"))
        .submenu(
            Row::new("acct:claude:me").label("me@example.com"),
            account_submenu(),
        )
        .row(Row::new("quit").segments(vec![
            Segment::new("Quit").flex(Flex::Grow),
            Segment::new("v1.0")
                .align(Align::Right)
                .color(Color::SecondaryLabel),
        ]))
}

/// The child flyout panel: a couple of info rows plus an action row.
fn account_submenu() -> Menu {
    Menu::new()
        .row(Row::info().segments(vec![
            Segment::new("Session resets in").flex(Flex::Grow),
            Segment::new("3h 12m")
                .align(Align::Right)
                .color(Color::SecondaryLabel),
        ]))
        .separator()
        .row(Row::new("switch:claude:me").label("Switch to this account"))
}

/// Cell 1 of the coverage matrix: the flush-right menu in light + dark, at
/// scale 1.0 and 2.0 (four goldens total).
#[test]
fn golden_flush_right() {
    let menu = flush_right_menu();
    for (theme_name, theme) in [("light", Theme::light()), ("dark", Theme::dark())] {
        for scale in [1.0_f32, 2.0_f32] {
            let mut drawer = RasterDrawer::new_headless(scale);
            render_menu(&mut drawer, &menu, &theme, &MenuOptions::default(), Some(0));
            let name = format!(
                "flush_right-{theme_name}-{}x",
                if scale == 1.0 { "1" } else { "2" }
            );
            assert_golden(&name, drawer.framebuffer());
        }
    }
}

/// Cell 3 of the coverage matrix: every `Item` kind, light theme, scale 2.0.
#[test]
fn golden_item_kinds() {
    let menu = item_kinds_menu();
    let mut drawer = RasterDrawer::new_headless(2.0);
    render_menu(
        &mut drawer,
        &menu,
        &Theme::light(),
        &MenuOptions::default(),
        None,
    );
    assert_golden("item_kinds-light-2x", drawer.framebuffer());
}

/// Cell 2 of the coverage matrix: a parent panel with its flyout open,
/// composited onto one canvas exactly as the live macOS backend positions its
/// second popup window via [`place_flyout`]. Dark theme, scale 2.0.
#[test]
fn golden_flyout_composite() {
    let scale = 2.0_f32;
    let theme = Theme::dark();
    let opts = MenuOptions::default();

    let child = account_submenu();
    let parent = flyout_parent_menu();
    let submenu_index = 1usize; // the account Item::Submenu (after the section header)

    let mut pd = RasterDrawer::new_headless(scale);
    let plaid = render_menu(&mut pd, &parent, &theme, &opts, Some(submenu_index));

    let mut cd = RasterDrawer::new_headless(scale);
    let claid = render_menu(&mut cd, &child, &theme, &opts, None);

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

    let (pw, ph) = pd.device_size();
    let (cw, ch) = cd.device_size();
    let ox = (placement.origin.x * scale).round() as i32;
    let oy = (placement.origin.y * scale).round() as i32;
    let margin = (10.0 * scale) as u32;
    let canvas_w = ((ox as u32 + cw).max(pw)) + margin * 2;
    let canvas_h = ((oy as u32 + ch).max(ph)) + margin * 2;
    let mut canvas = Framebuffer::new(canvas_w, canvas_h);
    canvas.fill(Rgba::opaque(28, 28, 30));

    canvas.draw(pd.framebuffer(), margin as i32, margin as i32);
    canvas.draw(cd.framebuffer(), margin as i32 + ox, margin as i32 + oy);

    assert_golden("flyout_composite-dark-2x", &canvas);
}
