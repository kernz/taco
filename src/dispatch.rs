//! Key event dispatcher. Chords accumulate in `editor.pending` and walk a
//! prefix trie (Keymap). Lookup order: active input mode (minibuffer /
//! isearch / query-replace / describe-key) -> buffer-local map (dired,
//! wgrep) -> global map -> self-insert fallback.

use crate::buffer::Mode;
use crate::editor::{CommandFn, Editor, InputMode, PostAction, PrefixArg};
use crate::keys::{format_seq, Chord, Key};
use crate::{commands, minibuffer, search};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Keymap {
    map: HashMap<Chord, Entry>,
}

#[derive(Debug)]
pub enum Entry {
    Cmd(String),
    Prefix(Keymap),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// Sequence matches nothing in this map.
    None,
    /// Sequence is a proper prefix of at least one binding.
    Pending,
    /// Sequence resolves to a command.
    Cmd(String),
}

impl Keymap {
    /// Bind `seq` to command `name`, creating prefix maps as needed.
    /// Overwrites conflicting bindings.
    pub fn bind(&mut self, seq: &[Chord], name: &str) {
        assert!(!seq.is_empty(), "empty key sequence");
        if seq.len() == 1 {
            self.map.insert(seq[0], Entry::Cmd(name.to_string()));
            return;
        }
        let entry = self
            .map
            .entry(seq[0])
            .and_modify(|e| {
                if matches!(e, Entry::Cmd(_)) {
                    *e = Entry::Prefix(Keymap::default());
                }
            })
            .or_insert_with(|| Entry::Prefix(Keymap::default()));
        match entry {
            Entry::Prefix(sub) => sub.bind(&seq[1..], name),
            Entry::Cmd(_) => unreachable!(),
        }
    }

    pub fn lookup(&self, seq: &[Chord]) -> Lookup {
        let Some(first) = seq.first() else {
            return Lookup::Pending;
        };
        match self.map.get(first) {
            None => Lookup::None,
            Some(Entry::Cmd(name)) => {
                if seq.len() == 1 {
                    Lookup::Cmd(name.clone())
                } else {
                    Lookup::None
                }
            }
            Some(Entry::Prefix(sub)) => {
                if seq.len() == 1 {
                    Lookup::Pending
                } else {
                    sub.lookup(&seq[1..])
                }
            }
        }
    }
}

const CTRL_G: Chord = Chord { ctrl: true, meta: false, key: Key::Char('g') };
const CTRL_U: Chord = Chord { ctrl: true, meta: false, key: Key::Char('u') };

/// Handle one normalized chord. Returns the action the caller (which owns
/// the Steel engine) must perform afterwards.
pub fn handle_chord(ed: &mut Editor, chord: Chord) -> PostAction {
    ed.echo = None;

    // C-g cancels everything, everywhere.
    if chord == CTRL_G {
        commands::keyboard_quit(ed, None);
        return PostAction::None;
    }

    // Active input modes swallow keys before any keymap.
    match &ed.input {
        InputMode::Prompt(_) => {
            return minibuffer::handle_key(ed, chord);
        }
        InputMode::ISearch(_) => {
            search::isearch_key(ed, chord);
            return PostAction::None;
        }
        InputMode::QueryReplace(_) => {
            search::query_replace_key(ed, chord);
            return PostAction::None;
        }
        InputMode::DescribeKey(_) => {
            describe_key_chord(ed, chord);
            return PostAction::None;
        }
        InputMode::Normal => {}
    }

    // Universal argument: C-u starts it, digits extend it.
    if ed.pending.is_empty() {
        if chord == CTRL_U {
            ed.prefix = PrefixArg::Universal;
            ed.message("C-u-");
            return PostAction::None;
        }
        if ed.prefix != PrefixArg::None {
            if let Some(c) = chord.self_insert_char() {
                if let Some(d) = c.to_digit(10) {
                    let cur = match ed.prefix {
                        PrefixArg::Num(n) => n,
                        _ => 0,
                    };
                    ed.prefix = PrefixArg::Num(cur.saturating_mul(10).saturating_add(d));
                    ed.message(format!("C-u {}-", match ed.prefix {
                        PrefixArg::Num(n) => n.to_string(),
                        _ => String::new(),
                    }));
                    return PostAction::None;
                }
            }
        }
    }

    ed.pending.push(chord);
    let seq = ed.pending.clone();

    let (local, wgrep_active) = local_map_info(ed);
    let lookup = resolve(ed, &seq, local, wgrep_active);

    match lookup {
        Lookup::Pending => {
            ed.message(format!("{}-", format_seq(&seq)));
            PostAction::None
        }
        Lookup::Cmd(name) => {
            ed.pending.clear();
            execute_command(ed, &name)
        }
        Lookup::None => {
            ed.pending.clear();
            // Self-insert fallback for single unmodified printable chars.
            if seq.len() == 1 {
                if let Some(c) = seq[0].self_insert_char() {
                    let n = ed.prefix_num();
                    ed.prefix = PrefixArg::None;
                    commands::self_insert(ed, c, n);
                    ed.last_command = Some("self-insert".into());
                    return PostAction::None;
                }
                // RET / TAB / backspace fall back to editing defaults so the
                // bootstrap only needs to bind what the spec lists.
                match seq[0] {
                    Chord { ctrl: false, meta: false, key: Key::Enter } => {
                        return execute_command(ed, "newline");
                    }
                    Chord { ctrl: false, meta: false, key: Key::Backspace } => {
                        return execute_command(ed, "delete-backward-char");
                    }
                    _ => {}
                }
            }
            ed.prefix = PrefixArg::None;
            ed.message(format!("{} is undefined", format_seq(&seq)));
            PostAction::None
        }
    }
}

/// Which buffer-local keymap applies to the current buffer.
fn local_map_info(ed: &Editor) -> (LocalMap, bool) {
    match &ed.cur_buffer().mode {
        Mode::Dired(d) if d.wgrep.is_some() => (LocalMap::Wgrep, true),
        Mode::Dired(_) => (LocalMap::Dired, false),
        Mode::Fundamental => (LocalMap::None, false),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LocalMap {
    None,
    Dired,
    Wgrep,
}

fn resolve(ed: &Editor, seq: &[Chord], local: LocalMap, _wgrep: bool) -> Lookup {
    let local_map = match local {
        LocalMap::None => None,
        LocalMap::Dired => Some(&ed.dired_map),
        LocalMap::Wgrep => Some(&ed.wgrep_map),
    };
    if let Some(map) = local_map {
        match map.lookup(seq) {
            Lookup::None => {}
            hit => return hit,
        }
    }
    ed.global_map.lookup(seq)
}

/// Run a command by name. Native commands run immediately; Scheme commands
/// are returned so the caller can invoke the engine with no Editor borrow.
pub fn execute_command(ed: &mut Editor, name: &str) -> PostAction {
    let prefix = ed.prefix_num();
    ed.prefix = PrefixArg::None;
    ed.this_command = Some(name.to_string());

    // Consecutive C-n/C-p share a goal column; anything else clears it.
    if name != "next-line" && name != "previous-line" {
        ed.goal_col = None;
    }

    // Copy the callable out so no registry borrow outlives command execution.
    enum Callable {
        Native(crate::editor::NativeFn),
        Scheme(steel::rvals::SteelVal),
    }
    let callable = match ed.registry.get(name) {
        None => {
            ed.message(format!("Unknown command: {name}"));
            return PostAction::None;
        }
        Some(cmd) => match &cmd.f {
            CommandFn::Native(f) => Callable::Native(*f),
            CommandFn::Scheme(v) => Callable::Scheme(v.clone()),
        },
    };

    ed.cur_buffer_mut().undo.push_boundary();
    let action = match callable {
        Callable::Native(f) => {
            f(ed, prefix);
            PostAction::None
        }
        Callable::Scheme(v) => PostAction::RunScheme(v),
    };
    ed.last_command = ed.this_command.take();
    action
}

/// C-h k: accumulate chords until they resolve (or dead-end) in the maps.
fn describe_key_chord(ed: &mut Editor, chord: Chord) {
    let InputMode::DescribeKey(seq) = &mut ed.input else {
        return;
    };
    seq.push(chord);
    let seq = seq.clone();
    let (local, wgrep) = local_map_info(ed);
    match resolve(ed, &seq, local, wgrep) {
        Lookup::Pending => {
            ed.message(format!("Describe key: {}-", format_seq(&seq)));
        }
        Lookup::Cmd(name) => {
            ed.input = InputMode::Normal;
            let doc = ed
                .registry
                .get(&name)
                .map(|c| c.doc.clone())
                .unwrap_or_default();
            ed.message(format!("{} runs the command {} — {}", format_seq(&seq), name, doc));
        }
        Lookup::None => {
            ed.input = InputMode::Normal;
            ed.message(format!("{} is undefined", format_seq(&seq)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::parse_seq;

    #[test]
    fn trie_walk() {
        let mut km = Keymap::default();
        km.bind(&parse_seq("C-x C-f").unwrap(), "find-file");
        km.bind(&parse_seq("C-x C-s").unwrap(), "save-buffer");
        km.bind(&parse_seq("C-x r t").unwrap(), "string-rectangle");
        km.bind(&parse_seq("C-f").unwrap(), "forward-char");

        assert_eq!(
            km.lookup(&parse_seq("C-x").unwrap()),
            Lookup::Pending
        );
        assert_eq!(
            km.lookup(&parse_seq("C-x C-f").unwrap()),
            Lookup::Cmd("find-file".into())
        );
        assert_eq!(
            km.lookup(&parse_seq("C-x r").unwrap()),
            Lookup::Pending
        );
        assert_eq!(
            km.lookup(&parse_seq("C-x r t").unwrap()),
            Lookup::Cmd("string-rectangle".into())
        );
        assert_eq!(km.lookup(&parse_seq("C-x z").unwrap()), Lookup::None);
        assert_eq!(
            km.lookup(&parse_seq("C-f").unwrap()),
            Lookup::Cmd("forward-char".into())
        );
    }

    #[test]
    fn rebinding_prefix_over_cmd() {
        let mut km = Keymap::default();
        km.bind(&parse_seq("C-c").unwrap(), "old");
        km.bind(&parse_seq("C-c f d").unwrap(), "dired-open-dir");
        assert_eq!(
            km.lookup(&parse_seq("C-c f d").unwrap()),
            Lookup::Cmd("dired-open-dir".into())
        );
    }
}
