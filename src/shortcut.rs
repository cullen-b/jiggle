//! The optional system-wide shortcut, stored as readable text like "ctrl+opt+space".

use std::cell::Cell;

// CGEventFlags bits for the modifiers we care about.
pub const SHIFT: u64 = 0x0002_0000;
pub const CTRL: u64 = 0x0004_0000;
pub const OPT: u64 = 0x0008_0000;
pub const CMD: u64 = 0x0010_0000;
const MODS: u64 = SHIFT | CTRL | OPT | CMD;

/// Virtual key codes (ANSI layout positions) and the names used in the setting.
const KEYS: &[(&str, u16)] = &[
    ("a", 0), ("s", 1), ("d", 2), ("f", 3), ("h", 4), ("g", 5), ("z", 6), ("x", 7), ("c", 8), ("v", 9),
    ("b", 11), ("q", 12), ("w", 13), ("e", 14), ("r", 15), ("y", 16), ("t", 17), ("1", 18), ("2", 19),
    ("3", 20), ("4", 21), ("6", 22), ("5", 23), ("equal", 24), ("9", 25), ("7", 26), ("minus", 27), ("8", 28),
    ("0", 29), ("rightbracket", 30), ("o", 31), ("u", 32), ("leftbracket", 33), ("i", 34), ("p", 35),
    ("return", 36), ("l", 37), ("j", 38), ("quote", 39), ("k", 40), ("semicolon", 41), ("backslash", 42),
    ("comma", 43), ("slash", 44), ("n", 45), ("m", 46), ("period", 47), ("tab", 48), ("space", 49),
    ("grave", 50), ("delete", 51), ("f1", 122), ("f2", 120), ("f3", 99), ("f4", 118), ("f5", 96), ("f6", 97),
    ("f7", 98), ("f8", 100), ("f9", 101), ("f10", 109), ("f11", 103), ("f12", 111), ("f13", 105),
    ("f14", 107), ("f15", 113), ("f16", 106), ("f17", 64), ("f18", 79), ("f19", 80), ("f20", 90),
    ("left", 123), ("right", 124), ("down", 125), ("up", 126),
];
const ESCAPE: u16 = 53;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub key: u16,
    pub mods: u64,
}

fn key_name(key: u16) -> Option<&'static str> {
    KEYS.iter().find(|(_, k)| *k == key).map(|(n, _)| *n)
}

fn is_function_key(key: u16) -> bool {
    key_name(key).is_some_and(|n| n.len() > 1 && n.starts_with('f') && n[1..].chars().all(|c| c.is_ascii_digit()))
}

impl Shortcut {
    /// Parses "cmd+shift+j"; modifier names may also be command/control/option/alt or ⌘⌃⌥⇧.
    pub fn parse(text: &str) -> Option<Self> {
        let mut mods = 0;
        let mut key = None;
        for part in text.split('+').map(|p| p.trim().to_ascii_lowercase()) {
            match part.as_str() {
                "cmd" | "command" | "⌘" => mods |= CMD,
                "ctrl" | "control" | "⌃" => mods |= CTRL,
                "opt" | "option" | "alt" | "⌥" => mods |= OPT,
                "shift" | "⇧" => mods |= SHIFT,
                "enter" => key = Some(36),
                "backspace" => key = Some(51),
                "backtick" => key = Some(50),
                name => key = Some(KEYS.iter().find(|(n, _)| *n == name)?.1),
            }
        }
        Self::new(key?, mods)
    }

    /// A usable shortcut needs ⌘, ⌃ or ⌥ (or an F-key) so ordinary typing can't trigger it.
    pub fn new(key: u16, flags: u64) -> Option<Self> {
        let mods = flags & MODS;
        let ok = key_name(key).is_some() && (mods & (CMD | CTRL | OPT) != 0 || is_function_key(key));
        ok.then_some(Self { key, mods })
    }

    /// The text stored in the setting, e.g. "ctrl+opt+space".
    pub fn to_setting(self) -> String {
        let mut parts = Vec::new();
        for (bit, name) in [(CTRL, "ctrl"), (OPT, "opt"), (SHIFT, "shift"), (CMD, "cmd")] {
            if self.mods & bit != 0 {
                parts.push(name);
            }
        }
        parts.push(key_name(self.key).unwrap_or("?"));
        parts.join("+")
    }

    /// How menus show it, e.g. "⌃⌥Space".
    pub fn display(self) -> String {
        let mut s = String::new();
        for (bit, sym) in [(CTRL, "⌃"), (OPT, "⌥"), (SHIFT, "⇧"), (CMD, "⌘")] {
            if self.mods & bit != 0 {
                s.push_str(sym);
            }
        }
        let name = key_name(self.key).unwrap_or("?");
        let pretty = match name {
            "space" => "Space".to_string(),
            "return" => "↩".to_string(),
            "tab" => "⇥".to_string(),
            "delete" => "⌫".to_string(),
            "left" => "←".to_string(),
            "right" => "→".to_string(),
            "up" => "↑".to_string(),
            "down" => "↓".to_string(),
            n if n.len() == 1 => n.to_ascii_uppercase(),
            n if is_function_key(self.key) => n.to_ascii_uppercase(),
            n => n.to_string(),
        };
        s + &pretty
    }

    pub fn matches(self, key: u16, flags: u64) -> bool {
        self.key == key && self.mods == flags & MODS
    }
}

/// Cheap first check for every key press: could this be a shortcut at all?
pub fn could_be_shortcut(key: u16, flags: u64) -> bool {
    flags & (CMD | CTRL | OPT) != 0 || is_function_key(key)
}

thread_local! {
    static RECORDING: Cell<bool> = const { Cell::new(false) };
}

pub fn is_recording() -> bool {
    RECORDING.with(Cell::get)
}

pub fn set_recording(on: bool) {
    RECORDING.with(|r| r.set(on));
}

/// What a key press does while recording.
pub enum Recorded {
    Cancelled,
    Got(Shortcut),
    NeedsModifier,
}

pub fn record(key: u16, flags: u64) -> Recorded {
    if key == ESCAPE && flags & MODS == 0 {
        return Recorded::Cancelled;
    }
    match Shortcut::new(key, flags) {
        Some(s) => Recorded::Got(s),
        None => Recorded::NeedsModifier,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_round_trips() {
        let s = Shortcut::parse("Ctrl+Option+Space").unwrap();
        assert_eq!(s, Shortcut { key: 49, mods: CTRL | OPT });
        assert_eq!(s.to_setting(), "ctrl+opt+space");
        assert_eq!(s.display(), "⌃⌥Space");
        assert_eq!(Shortcut::parse("⌘+⇧+j").unwrap().display(), "⇧⌘J");
        assert_eq!(Shortcut::parse("f13").unwrap().to_setting(), "f13");
    }

    #[test]
    fn rejects_shortcuts_that_would_eat_typing() {
        assert_eq!(Shortcut::parse("j"), None);
        assert_eq!(Shortcut::parse("shift+j"), None);
        assert_eq!(Shortcut::parse("ctrl+nosuchkey"), None);
        assert_eq!(Shortcut::parse(""), None);
    }

    #[test]
    fn matching_ignores_caps_lock_and_fn() {
        let s = Shortcut::parse("cmd+shift+space").unwrap();
        let caps_lock = 0x0001_0000;
        let fn_key = 0x0080_0000;
        assert!(s.matches(49, CMD | SHIFT | caps_lock | fn_key));
        assert!(!s.matches(49, CMD));
        assert!(!s.matches(49, CMD | SHIFT | OPT));
    }

    #[test]
    fn recording() {
        assert!(matches!(record(ESCAPE, 0), Recorded::Cancelled));
        assert!(matches!(record(38, 0), Recorded::NeedsModifier));
        assert!(matches!(record(38, CMD | OPT), Recorded::Got(Shortcut { key: 38, .. })));
    }
}
