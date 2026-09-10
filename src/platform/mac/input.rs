//! Keyboard translation: an `NSEvent` key-down into a muri [`NavKey`].
//!
//! Ported from the winit `translate_key` mapping. AppKit reports keys by
//! hardware `keyCode` (layout-independent for the navigation keys) plus the
//! typed `characters`, so named navigation keys are matched by keycode and
//! everything else falls back to the first typed character.

use objc2_app_kit::NSEvent;

use crate::keynav::NavKey;

// Virtual keycodes (`Events.h` / `HIToolbox`), stable across keyboard layouts.
const KEY_RETURN: u16 = 36;
const KEY_KEYPAD_ENTER: u16 = 76;
const KEY_SPACE: u16 = 49;
const KEY_ESCAPE: u16 = 53;
const KEY_LEFT: u16 = 123;
const KEY_RIGHT: u16 = 124;
const KEY_DOWN: u16 = 125;
const KEY_UP: u16 = 126;
const KEY_HOME: u16 = 115;
const KEY_END: u16 = 119;

/// Translate a key-down [`NSEvent`] into a muri [`NavKey`], or `None` for keys
/// the menu ignores. Navigation keys match on layout-independent keycodes; a
/// character key uses the typed character (modifiers ignored).
pub(super) fn translate_ns_key(event: &NSEvent) -> Option<NavKey> {
    match event.keyCode() {
        KEY_DOWN => Some(NavKey::Down),
        KEY_UP => Some(NavKey::Up),
        KEY_RIGHT => Some(NavKey::Right),
        KEY_LEFT => Some(NavKey::Left),
        KEY_RETURN | KEY_KEYPAD_ENTER | KEY_SPACE => Some(NavKey::Activate),
        KEY_ESCAPE => Some(NavKey::Escape),
        KEY_HOME => Some(NavKey::Home),
        KEY_END => Some(NavKey::End),
        _ => {
            let chars = event.charactersIgnoringModifiers()?;
            chars
                .to_string()
                .chars()
                .next()
                .filter(|c| !c.is_control())
                .map(NavKey::Char)
        }
    }
}
