//! Accelerators (spec `02` §5), mirroring muda's `accelerator` module.
//!
//! In a native menu bar an accelerator registers and fires natively; in a muri
//! custom surface it is **displayed only** and handled while the menu is open
//! (divergence D5). The facade parses a useful subset of muda's `Code` /
//! `Modifiers` — enough for the common `"CmdOrCtrl+S"`-style strings and the
//! `Accelerator::new(Some(Modifiers::SUPER), Code::KeyS)` form — not muda's full
//! `KeyboardEvent.code` table.

use std::str::FromStr;

use super::{Error, Result};

/// A set of modifier keys, mirroring muda's `Modifiers` (a bitflag set).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers(u32);

impl Modifiers {
    /// The Shift modifier.
    pub const SHIFT: Self = Self(1 << 0);
    /// The Control modifier.
    pub const CONTROL: Self = Self(1 << 1);
    /// The Alt / Option modifier.
    pub const ALT: Self = Self(1 << 2);
    /// The Super / Command / Windows modifier.
    pub const SUPER: Self = Self(1 << 3);
    /// Alias of [`Modifiers::SUPER`] (muda's `META`).
    pub const META: Self = Self(1 << 3);

    /// Whether this set contains every modifier in `other`.
    pub fn contains(&self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no modifiers are set.
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Modifiers(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

macro_rules! define_codes {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// A keyboard key in an [`Accelerator`], mirroring muda's `Code` names.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[non_exhaustive]
        pub enum Code {
            $(
                #[doc = concat!("The `", $name, "` key.")]
                $variant,
            )*
        }

        impl Code {
            /// The `KeyboardEvent.code`-style name for this key.
            pub fn name(&self) -> &'static str {
                match self {
                    $( Code::$variant => $name, )*
                }
            }
        }

        impl FromStr for Code {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self> {
                $( if s.eq_ignore_ascii_case($name) { return Ok(Code::$variant); } )*
                friendly_code(s)
            }
        }
    };
}

define_codes! {
    KeyA => "KeyA", KeyB => "KeyB", KeyC => "KeyC", KeyD => "KeyD", KeyE => "KeyE",
    KeyF => "KeyF", KeyG => "KeyG", KeyH => "KeyH", KeyI => "KeyI", KeyJ => "KeyJ",
    KeyK => "KeyK", KeyL => "KeyL", KeyM => "KeyM", KeyN => "KeyN", KeyO => "KeyO",
    KeyP => "KeyP", KeyQ => "KeyQ", KeyR => "KeyR", KeyS => "KeyS", KeyT => "KeyT",
    KeyU => "KeyU", KeyV => "KeyV", KeyW => "KeyW", KeyX => "KeyX", KeyY => "KeyY",
    KeyZ => "KeyZ",
    Digit0 => "Digit0", Digit1 => "Digit1", Digit2 => "Digit2", Digit3 => "Digit3",
    Digit4 => "Digit4", Digit5 => "Digit5", Digit6 => "Digit6", Digit7 => "Digit7",
    Digit8 => "Digit8", Digit9 => "Digit9",
    F1 => "F1", F2 => "F2", F3 => "F3", F4 => "F4", F5 => "F5", F6 => "F6",
    F7 => "F7", F8 => "F8", F9 => "F9", F10 => "F10", F11 => "F11", F12 => "F12",
    Enter => "Enter", Escape => "Escape", Space => "Space", Tab => "Tab",
    Backspace => "Backspace", Delete => "Delete",
    ArrowUp => "ArrowUp", ArrowDown => "ArrowDown", ArrowLeft => "ArrowLeft",
    ArrowRight => "ArrowRight",
    Home => "Home", End => "End", PageUp => "PageUp", PageDown => "PageDown",
    Minus => "Minus", Equal => "Equal", Comma => "Comma", Period => "Period",
    Slash => "Slash", Semicolon => "Semicolon", Quote => "Quote",
    Backquote => "Backquote", BracketLeft => "BracketLeft",
    BracketRight => "BracketRight", Backslash => "Backslash",
}

/// Map muda's friendly single-token forms (`"S"`, `"1"`, `"F5"`) onto a [`Code`].
fn friendly_code(s: &str) -> Result<Code> {
    let up = s.to_ascii_uppercase();
    if up.chars().count() == 1 {
        let c = up.chars().next().unwrap();
        if c.is_ascii_alphabetic() {
            return Code::from_str(&format!("Key{c}"));
        }
        if c.is_ascii_digit() {
            return Code::from_str(&format!("Digit{c}"));
        }
    }
    Err(Error::AcceleratorParse(format!("unknown key code: {s}")))
}

/// A keyboard accelerator: a modifier set plus a key (muda's `Accelerator`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accelerator {
    /// The modifier keys.
    pub mods: Modifiers,
    /// The key.
    pub key: Code,
}

impl Accelerator {
    /// A new accelerator (mirrors `Accelerator::new(Some(mods), key)`).
    pub fn new(mods: Option<Modifiers>, key: Code) -> Self {
        Accelerator {
            mods: mods.unwrap_or_default(),
            key,
        }
    }
}

impl FromStr for Accelerator {
    type Err = Error;

    /// Parse a `"Mod+Mod+Key"` string, e.g. `"CmdOrCtrl+S"` or `"Alt+Shift+F5"`.
    fn from_str(s: &str) -> Result<Self> {
        let mut mods = Modifiers::default();
        let mut key: Option<Code> = None;
        for raw in s.split('+') {
            let part = raw.trim();
            if part.is_empty() {
                continue;
            }
            match part.to_ascii_lowercase().as_str() {
                "shift" => mods |= Modifiers::SHIFT,
                "ctrl" | "control" => mods |= Modifiers::CONTROL,
                "alt" | "option" => mods |= Modifiers::ALT,
                "super" | "cmd" | "command" | "meta" | "win" | "cmdorctrl" | "commandorcontrol" => {
                    mods |= Modifiers::SUPER
                }
                _ => key = Some(Code::from_str(part)?),
            }
        }
        match key {
            Some(key) => Ok(Accelerator::new(Some(mods), key)),
            None => Err(Error::AcceleratorParse(format!(
                "no key in accelerator: {s}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_super_and_letter() {
        let acc = Accelerator::new(Some(Modifiers::SUPER), Code::KeyS);
        assert!(acc.mods.contains(Modifiers::SUPER));
        assert_eq!(acc.key, Code::KeyS);
    }

    #[test]
    fn parse_cmd_or_ctrl_s() {
        let acc: Accelerator = "CmdOrCtrl+S".parse().unwrap();
        assert!(acc.mods.contains(Modifiers::SUPER));
        assert_eq!(acc.key, Code::KeyS);
    }

    #[test]
    fn parse_multi_modifier_function_key() {
        let acc: Accelerator = "Alt+Shift+F5".parse().unwrap();
        assert!(acc.mods.contains(Modifiers::ALT));
        assert!(acc.mods.contains(Modifiers::SHIFT));
        assert_eq!(acc.key, Code::F5);
    }

    #[test]
    fn parse_code_name_form() {
        let acc: Accelerator = "Ctrl+KeyA".parse().unwrap();
        assert!(acc.mods.contains(Modifiers::CONTROL));
        assert_eq!(acc.key, Code::KeyA);
    }

    #[test]
    fn parse_missing_key_errors() {
        assert!("Ctrl+Alt".parse::<Accelerator>().is_err());
    }
}
