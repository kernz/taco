//! Key chords: the unit of the dispatcher. A chord is one keypress with
//! Ctrl/Meta modifiers; a binding is a sequence of chords ("C-x C-f").

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub meta: bool,
    pub key: Key,
}

impl Chord {
    #[cfg(test)]
    pub fn plain(key: Key) -> Self {
        Chord { ctrl: false, meta: false, key }
    }

    /// A chord with no modifiers that would self-insert in a text buffer.
    pub fn self_insert_char(&self) -> Option<char> {
        if self.ctrl || self.meta {
            return None;
        }
        match self.key {
            Key::Char(c) => Some(c),
            Key::Space => Some(' '),
            _ => None,
        }
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            write!(f, "C-")?;
        }
        if self.meta {
            write!(f, "M-")?;
        }
        match self.key {
            Key::Char(c) => write!(f, "{c}"),
            Key::Enter => write!(f, "RET"),
            Key::Tab => write!(f, "TAB"),
            Key::Backspace => write!(f, "backspace"),
            Key::Space => write!(f, "SPC"),
        }
    }
}

pub fn format_seq(seq: &[Chord]) -> String {
    seq.iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse a binding string like "C-x C-f", "M-<", "C-c p D", "% m", "C-x SPC".
pub fn parse_seq(s: &str) -> Option<Vec<Chord>> {
    s.split_whitespace().map(parse_chord).collect()
}

fn parse_chord(tok: &str) -> Option<Chord> {
    let mut ctrl = false;
    let mut meta = false;
    let mut rest = tok;
    loop {
        if let Some(r) = rest.strip_prefix("C-") {
            // Bare "C-" followed by nothing is invalid; "C--" means Ctrl+'-'.
            if !r.is_empty() && !ctrl {
                ctrl = true;
                rest = r;
                continue;
            }
        }
        if let Some(r) = rest.strip_prefix("M-") {
            if !r.is_empty() && !meta {
                meta = true;
                rest = r;
                continue;
            }
        }
        break;
    }
    let key = match rest {
        "RET" => Key::Enter,
        "TAB" | "Tab" => Key::Tab,
        "SPC" | "spacebar" | "space" => Key::Space,
        "backspace" | "DEL" => Key::Backspace,
        _ => {
            let mut chars = rest.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Key::Char(c)
        }
    };
    Some(Chord { ctrl, meta, key })
}

/// Normalize a crossterm key event into a Chord, folding legacy terminal
/// aliases into canonical chords. `esc_meta` is true when the previous event
/// was a bare ESC (terminals without Alt support send ESC as a Meta prefix).
///
/// Legacy quirks handled:
///   C-/  arrives as 0x1F => Ctrl+'_' (or Ctrl+'7' in some terminals)
///   C-SPC arrives as NUL => Ctrl+'@' / Ctrl+' ' / Ctrl+'2'
pub fn normalize(ev: &KeyEvent, esc_meta: bool) -> Option<Chord> {
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let meta = ev.modifiers.contains(KeyModifiers::ALT) || esc_meta;
    let key = match ev.code {
        KeyCode::Char(c) => {
            if ctrl {
                match c {
                    // C-/ legacy aliases
                    '_' | '7' => Key::Char('/'),
                    // C-SPC legacy aliases
                    '@' | '2' | ' ' => Key::Space,
                    _ => Key::Char(c.to_ascii_lowercase()),
                }
            } else if c == ' ' {
                Key::Space
            } else {
                Key::Char(c)
            }
        }
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Null => {
            // Legacy NUL byte is C-SPC.
            return Some(Chord { ctrl: true, meta, key: Key::Space });
        }
        _ => return None,
    };
    Some(Chord { ctrl, meta, key })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chorded_sequences() {
        assert_eq!(
            parse_seq("C-x C-f").unwrap(),
            vec![
                Chord { ctrl: true, meta: false, key: Key::Char('x') },
                Chord { ctrl: true, meta: false, key: Key::Char('f') },
            ]
        );
        assert_eq!(
            parse_seq("M-<").unwrap(),
            vec![Chord { ctrl: false, meta: true, key: Key::Char('<') }]
        );
        assert_eq!(
            parse_seq("% m").unwrap(),
            vec![Chord::plain(Key::Char('%')), Chord::plain(Key::Char('m'))]
        );
        assert_eq!(
            parse_seq("C-x SPC").unwrap(),
            vec![
                Chord { ctrl: true, meta: false, key: Key::Char('x') },
                Chord::plain(Key::Space),
            ]
        );
        assert_eq!(
            parse_seq("M-backspace").unwrap(),
            vec![Chord { ctrl: false, meta: true, key: Key::Backspace }]
        );
        assert_eq!(
            parse_seq("C-M-x").unwrap(),
            vec![Chord { ctrl: true, meta: true, key: Key::Char('x') }]
        );
    }

    #[test]
    fn roundtrips_display() {
        for s in ["C-x C-f", "M-<", "C-c p D", "% m", "C-x SPC", "M-backspace"] {
            let seq = parse_seq(s).unwrap();
            assert_eq!(format_seq(&seq), s.replace("spacebar", "SPC"));
        }
    }

    #[test]
    fn normalizes_legacy_aliases() {
        let ev = KeyEvent::new(KeyCode::Char('_'), KeyModifiers::CONTROL);
        assert_eq!(
            normalize(&ev, false).unwrap(),
            Chord { ctrl: true, meta: false, key: Key::Char('/') }
        );
        let ev = KeyEvent::new(KeyCode::Char('@'), KeyModifiers::CONTROL);
        assert_eq!(
            normalize(&ev, false).unwrap(),
            Chord { ctrl: true, meta: false, key: Key::Space }
        );
        let ev = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE);
        assert_eq!(
            normalize(&ev, true).unwrap(),
            Chord { ctrl: false, meta: true, key: Key::Char('f') }
        );
    }
}
