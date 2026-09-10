//! Keyboard translation: a Win32 virtual-key code into a muri [`NavKey`].
//!
//! The low-level keyboard hook (`WH_KEYBOARD_LL`, see [`super`]) hands us a
//! layout-independent [`VIRTUAL_KEY`] on every key-down while a popup is open.
//! Named navigation keys map by their `VK_*` code; a plain alphanumeric key maps
//! to its lowercase character for type-ahead. Everything else is ignored.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_UP,
};

use crate::keynav::NavKey;

/// Translate a virtual-key code into a muri [`NavKey`], or `None` for keys the
/// menu ignores.
///
/// Navigation keys match on their layout-independent `VK_*` code; an ASCII
/// letter (`VK_A`..=`VK_Z`, i.e. `0x41`..=`0x5A`) or digit (`0x30`..=`0x39`)
/// becomes a lowercase [`NavKey::Char`] for type-ahead.
///
// DEVICE-VERIFY(0.9.0): the `VK`→char mapping is the US-layout identity
// (`VK_A == b'A'`); a real keyboard-layout-aware translation (`ToUnicodeEx`)
// needs a Windows box + a non-US layout to verify.
pub(super) fn translate_vk(vk: VIRTUAL_KEY) -> Option<NavKey> {
    match vk {
        VK_DOWN => Some(NavKey::Down),
        VK_UP => Some(NavKey::Up),
        VK_RIGHT => Some(NavKey::Right),
        VK_LEFT => Some(NavKey::Left),
        VK_RETURN | VK_SPACE => Some(NavKey::Activate),
        VK_ESCAPE => Some(NavKey::Escape),
        VK_HOME => Some(NavKey::Home),
        VK_END => Some(NavKey::End),
        0x30..=0x39 => Some(NavKey::Char((b'0' + (vk as u8 - 0x30)) as char)),
        0x41..=0x5A => Some(NavKey::Char((b'a' + (vk as u8 - 0x41)) as char)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_navigation_keys_map_by_code() {
        assert_eq!(translate_vk(VK_DOWN), Some(NavKey::Down));
        assert_eq!(translate_vk(VK_ESCAPE), Some(NavKey::Escape));
        assert_eq!(translate_vk(VK_RETURN), Some(NavKey::Activate));
        assert_eq!(translate_vk(VK_SPACE), Some(NavKey::Activate));
    }

    #[test]
    fn alphanumeric_keys_map_to_lowercase_chars() {
        assert_eq!(translate_vk(0x41), Some(NavKey::Char('a'))); // VK_A
        assert_eq!(translate_vk(0x5A), Some(NavKey::Char('z'))); // VK_Z
        assert_eq!(translate_vk(0x30), Some(NavKey::Char('0'))); // VK_0
    }

    #[test]
    fn unmapped_keys_are_ignored() {
        assert_eq!(translate_vk(0x10), None); // VK_SHIFT
    }
}
