//! Pure keyboard-navigation state machine for the menu + one flyout level.
//!
//! muri owns keyboard navigation directly (independent of any accessibility
//! backend, per the design): arrow keys move the highlight, Right/Left open and
//! close the flyout, Enter/Space activate, Esc pops one level, and a typed
//! character jumps to the next matching row (type-ahead). Keeping the logic here
//! — free of any window, event loop, or platform call — makes it exhaustively
//! unit-testable; the live backend ([`crate::tray`]) only translates its platform
//! key events into [`NavKey`]s and applies the returned [`NavAction`].
//!
//! ## Focus model
//!
//! Navigation state is a [`MenuFocus`]: the selected **top-level** item index and,
//! when a submenu is open, a [`FlyoutFocus`] naming the open parent and the
//! selected **child** item index. This mirrors the live macOS backend's single
//! open-flyout state exactly (the same one the mouse hover-stack drives), so
//! keyboard and mouse selection stay consistent. Descending into a *nested*
//! submenu from within a flyout is intentionally a no-op here: the current macOS
//! backend renders one flyout level, matching mouse behavior.
//!
//! Selection wraps, and skips non-focusable items (separators, section headers,
//! disabled rows, and inert info rows — everything [`Item::is_interactive`] marks
//! `false`).

use crate::menu::{Item, Menu, MenuId};

/// A key press, normalized from the platform key event into the small set that
/// menu navigation cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    /// Move the highlight to the next focusable row (wraps).
    Down,
    /// Move the highlight to the previous focusable row (wraps).
    Up,
    /// Open the flyout of the selected submenu row (no-op elsewhere).
    Right,
    /// Close the open flyout, returning focus to its parent (no-op at top level).
    Left,
    /// Activate the selected row (Enter/Space): fire its click, or open a
    /// submenu.
    Activate,
    /// Pop one level: close the flyout if open, else dismiss the whole menu.
    Escape,
    /// Jump to the first focusable row.
    Home,
    /// Jump to the last focusable row.
    End,
    /// Type-ahead: jump to the next focusable row whose name starts with this
    /// character (case-insensitive).
    Char(char),
}

/// The open-flyout portion of [`MenuFocus`]: which top-level submenu is open and
/// which of its child items is selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlyoutFocus {
    /// Top-level item index (into [`Menu::items`]) of the open submenu parent.
    pub parent: usize,
    /// Selected child item index within the submenu, if any.
    pub child: Option<usize>,
}

/// The current keyboard-navigation selection over the menu and its (optional)
/// open flyout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuFocus {
    /// Selected top-level item index (into [`Menu::items`]), if any.
    pub top: Option<usize>,
    /// The open flyout and its selected child, if a submenu is open.
    pub flyout: Option<FlyoutFocus>,
}

/// What the backend should do after a key press. The pure [`handle_key`] mutates
/// the [`MenuFocus`] in place and returns one of these for the live loop to act on
/// (open/close a flyout window, dispatch a click, dismiss, or just repaint).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavAction {
    /// Nothing changed; no repaint required.
    None,
    /// Selection moved within the current level; repaint the affected window.
    Redraw,
    /// Open (and focus into) the flyout for this top-level parent index.
    OpenFlyout(usize),
    /// Close the open flyout, returning focus to its parent row.
    CloseFlyout,
    /// Activate the row with this id: the backend dispatches it and closes the
    /// whole stack.
    Activate(MenuId),
    /// Dismiss the entire menu stack.
    CloseAll,
}

/// Advance the navigation state for one key press, returning the action the
/// backend should perform. Pure: no I/O, no platform calls.
pub fn handle_key(menu: &Menu, focus: &mut MenuFocus, key: NavKey) -> NavAction {
    match focus.flyout {
        Some(fly) => handle_in_flyout(menu, focus, fly, key),
        None => handle_in_top(menu, focus, key),
    }
}

fn handle_in_top(menu: &Menu, focus: &mut MenuFocus, key: NavKey) -> NavAction {
    match key {
        NavKey::Down => {
            move_selection(menu, &mut focus.top, Dir::Next);
            NavAction::Redraw
        }
        NavKey::Up => {
            move_selection(menu, &mut focus.top, Dir::Prev);
            NavAction::Redraw
        }
        NavKey::Home => {
            focus.top = first_focusable(menu);
            NavAction::Redraw
        }
        NavKey::End => {
            focus.top = last_focusable(menu);
            NavAction::Redraw
        }
        NavKey::Char(c) => match type_ahead(menu, focus.top, c) {
            Some(i) => {
                focus.top = Some(i);
                NavAction::Redraw
            }
            None => NavAction::None,
        },
        NavKey::Right => match focus.top {
            Some(i) if is_submenu_at(menu, i) => open_flyout(menu, focus, i),
            _ => NavAction::None,
        },
        NavKey::Activate => match focus.top {
            Some(i) if is_submenu_at(menu, i) => open_flyout(menu, focus, i),
            Some(i) => match interactive_id_at(menu, i) {
                Some(id) => NavAction::Activate(id),
                None => NavAction::None,
            },
            None => NavAction::None,
        },
        NavKey::Left => NavAction::None,
        NavKey::Escape => NavAction::CloseAll,
    }
}

fn handle_in_flyout(
    menu: &Menu,
    focus: &mut MenuFocus,
    fly: FlyoutFocus,
    key: NavKey,
) -> NavAction {
    let Some(child) = submenu_child(menu, fly.parent) else {
        // The parent is no longer a submenu (menu swapped underneath us): drop
        // the stale flyout and return focus to the top level.
        focus.flyout = None;
        focus.top = Some(fly.parent);
        return NavAction::CloseFlyout;
    };
    let mut sel = fly.child;
    match key {
        NavKey::Down => {
            move_selection(child, &mut sel, Dir::Next);
            commit_child(focus, fly.parent, sel);
            NavAction::Redraw
        }
        NavKey::Up => {
            move_selection(child, &mut sel, Dir::Prev);
            commit_child(focus, fly.parent, sel);
            NavAction::Redraw
        }
        NavKey::Home => {
            commit_child(focus, fly.parent, first_focusable(child));
            NavAction::Redraw
        }
        NavKey::End => {
            commit_child(focus, fly.parent, last_focusable(child));
            NavAction::Redraw
        }
        NavKey::Char(c) => match type_ahead(child, sel, c) {
            Some(i) => {
                commit_child(focus, fly.parent, Some(i));
                NavAction::Redraw
            }
            None => NavAction::None,
        },
        NavKey::Left | NavKey::Escape => {
            focus.flyout = None;
            focus.top = Some(fly.parent);
            NavAction::CloseFlyout
        }
        // Nested descent isn't rendered by the current single-level backend.
        NavKey::Right => NavAction::None,
        NavKey::Activate => match sel {
            Some(ci) if is_submenu_at(child, ci) => NavAction::None,
            Some(ci) => match interactive_id_at(child, ci) {
                Some(id) => NavAction::Activate(id),
                None => NavAction::None,
            },
            None => NavAction::None,
        },
    }
}

/// Set `focus` to a submenu of `parent` with its first focusable child selected.
fn open_flyout(menu: &Menu, focus: &mut MenuFocus, parent: usize) -> NavAction {
    let child = submenu_child(menu, parent).and_then(first_focusable);
    focus.top = Some(parent);
    focus.flyout = Some(FlyoutFocus { parent, child });
    NavAction::OpenFlyout(parent)
}

fn commit_child(focus: &mut MenuFocus, parent: usize, child: Option<usize>) {
    focus.flyout = Some(FlyoutFocus { parent, child });
}

#[derive(Clone, Copy)]
enum Dir {
    Next,
    Prev,
}

/// Move `sel` to the next/previous focusable item in `menu`, wrapping. A `None`
/// or now-unfocusable selection jumps to the first (Next) or last (Prev).
fn move_selection(menu: &Menu, sel: &mut Option<usize>, dir: Dir) {
    let f = focusable_indices(menu);
    if f.is_empty() {
        *sel = None;
        return;
    }
    let new = match sel.and_then(|cur| f.iter().position(|&x| x == cur)) {
        Some(pos) => match dir {
            Dir::Next => f[(pos + 1) % f.len()],
            Dir::Prev => f[(pos + f.len() - 1) % f.len()],
        },
        None => match dir {
            Dir::Next => f[0],
            Dir::Prev => f[f.len() - 1],
        },
    };
    *sel = Some(new);
}

fn first_focusable(menu: &Menu) -> Option<usize> {
    focusable_indices(menu).first().copied()
}

fn last_focusable(menu: &Menu) -> Option<usize> {
    focusable_indices(menu).last().copied()
}

/// Type-ahead: the next focusable item after `current` (wrapping) whose accessible
/// name starts with `c`, case-insensitively.
fn type_ahead(menu: &Menu, current: Option<usize>, c: char) -> Option<usize> {
    let needle = c.to_ascii_lowercase();
    let f = focusable_indices(menu);
    if f.is_empty() {
        return None;
    }
    let start = current
        .and_then(|cur| f.iter().position(|&x| x == cur))
        .map(|p| p + 1)
        .unwrap_or(0);
    for k in 0..f.len() {
        let idx = f[(start + k) % f.len()];
        if item_name(menu, idx)
            .chars()
            .next()
            .map(|ch| ch.to_ascii_lowercase() == needle)
            .unwrap_or(false)
        {
            return Some(idx);
        }
    }
    None
}

fn focusable_indices(menu: &Menu) -> Vec<usize> {
    menu.items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.is_interactive())
        .map(|(i, _)| i)
        .collect()
}

fn submenu_child(menu: &Menu, i: usize) -> Option<&Menu> {
    match menu.items.get(i) {
        Some(Item::Submenu { menu, .. }) => Some(menu),
        _ => None,
    }
}

fn is_submenu_at(menu: &Menu, i: usize) -> bool {
    matches!(menu.items.get(i), Some(Item::Submenu { .. }))
}

fn interactive_id_at(menu: &Menu, i: usize) -> Option<MenuId> {
    match menu.items.get(i) {
        Some(Item::Row(r)) if r.enabled && !r.id.is_none() => Some(r.id.clone()),
        _ => None,
    }
}

fn item_name(menu: &Menu, i: usize) -> String {
    match menu.items.get(i) {
        Some(Item::Row(r)) | Some(Item::SectionHeader(r)) => r.accessible_name(),
        Some(Item::Submenu { label, .. }) => label.accessible_name(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::Row;

    // Header (0), account rows (1,2), separator (3), submenu (4), quit (5).
    // Focusable: 1, 2, 4, 5.
    fn menu() -> Menu {
        Menu::new()
            .section_header(Row::info().label("Claude"))
            .row(Row::new("a").label("Apple"))
            .row(Row::new("b").label("Banana"))
            .separator()
            .submenu(
                Row::new("settings").label("Settings"),
                Menu::new()
                    .row(Row::new("s1").label("One"))
                    .row(Row::new("s2").label("Two")),
            )
            .row(Row::new("quit").label("Quit"))
    }

    fn press(m: &Menu, f: &mut MenuFocus, k: NavKey) -> NavAction {
        handle_key(m, f, k)
    }

    #[test]
    fn down_from_nothing_selects_first_focusable_skipping_header() {
        let m = menu();
        let mut f = MenuFocus::default();
        assert_eq!(press(&m, &mut f, NavKey::Down), NavAction::Redraw);
        assert_eq!(f.top, Some(1)); // skips the section header at 0
    }

    #[test]
    fn up_from_nothing_selects_last_focusable() {
        let m = menu();
        let mut f = MenuFocus::default();
        press(&m, &mut f, NavKey::Up);
        assert_eq!(f.top, Some(5));
    }

    #[test]
    fn down_skips_separator_and_wraps() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(1),
            flyout: None,
        };
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.top, Some(2));
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.top, Some(4)); // skips separator at 3
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.top, Some(5));
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.top, Some(1)); // wraps past the end
    }

    #[test]
    fn up_wraps_to_last() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(1),
            flyout: None,
        };
        press(&m, &mut f, NavKey::Up);
        assert_eq!(f.top, Some(5));
    }

    #[test]
    fn home_and_end_jump_to_bounds() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: None,
        };
        press(&m, &mut f, NavKey::Home);
        assert_eq!(f.top, Some(1));
        press(&m, &mut f, NavKey::End);
        assert_eq!(f.top, Some(5));
    }

    #[test]
    fn right_on_submenu_opens_flyout_and_focuses_first_child() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: None,
        };
        assert_eq!(press(&m, &mut f, NavKey::Right), NavAction::OpenFlyout(4));
        assert_eq!(
            f.flyout,
            Some(FlyoutFocus {
                parent: 4,
                child: Some(0),
            })
        );
    }

    #[test]
    fn right_on_leaf_does_nothing() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(1),
            flyout: None,
        };
        assert_eq!(press(&m, &mut f, NavKey::Right), NavAction::None);
        assert!(f.flyout.is_none());
    }

    #[test]
    fn activate_leaf_returns_its_id() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(5),
            flyout: None,
        };
        assert_eq!(
            press(&m, &mut f, NavKey::Activate),
            NavAction::Activate(MenuId::from("quit"))
        );
    }

    #[test]
    fn activate_submenu_opens_it() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: None,
        };
        assert_eq!(
            press(&m, &mut f, NavKey::Activate),
            NavAction::OpenFlyout(4)
        );
    }

    #[test]
    fn navigation_within_flyout_then_left_closes() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: Some(FlyoutFocus {
                parent: 4,
                child: Some(0),
            }),
        };
        // Down moves within the child menu.
        assert_eq!(press(&m, &mut f, NavKey::Down), NavAction::Redraw);
        assert_eq!(f.flyout.unwrap().child, Some(1));
        // Down wraps back to the first child.
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.flyout.unwrap().child, Some(0));
        // Left closes the flyout, focus back on the parent.
        assert_eq!(press(&m, &mut f, NavKey::Left), NavAction::CloseFlyout);
        assert!(f.flyout.is_none());
        assert_eq!(f.top, Some(4));
    }

    #[test]
    fn activate_in_flyout_returns_child_id() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: Some(FlyoutFocus {
                parent: 4,
                child: Some(1),
            }),
        };
        assert_eq!(
            press(&m, &mut f, NavKey::Activate),
            NavAction::Activate(MenuId::from("s2"))
        );
    }

    #[test]
    fn escape_pops_one_level_then_closes_all() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(4),
            flyout: Some(FlyoutFocus {
                parent: 4,
                child: Some(0),
            }),
        };
        assert_eq!(press(&m, &mut f, NavKey::Escape), NavAction::CloseFlyout);
        assert!(f.flyout.is_none());
        assert_eq!(press(&m, &mut f, NavKey::Escape), NavAction::CloseAll);
    }

    #[test]
    fn type_ahead_jumps_and_wraps() {
        let m = menu();
        let mut f = MenuFocus {
            top: Some(1),
            flyout: None,
        };
        // From "Apple" (1), 'q' jumps to "Quit" (5).
        assert_eq!(press(&m, &mut f, NavKey::Char('q')), NavAction::Redraw);
        assert_eq!(f.top, Some(5));
        // From "Quit", 'b' wraps forward to "Banana" (2).
        press(&m, &mut f, NavKey::Char('b'));
        assert_eq!(f.top, Some(2));
        // A character no row starts with does nothing.
        assert_eq!(press(&m, &mut f, NavKey::Char('z')), NavAction::None);
        assert_eq!(f.top, Some(2));
    }

    #[test]
    fn empty_menu_has_no_selection() {
        let m = Menu::new();
        let mut f = MenuFocus::default();
        press(&m, &mut f, NavKey::Down);
        assert_eq!(f.top, None);
    }
}
