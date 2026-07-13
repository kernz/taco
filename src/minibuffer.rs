//! The minibuffer: line-editing prompts in the echo area. Keys route
//! through the minibuffer keymap first (single chords — how plugins bind
//! C-n/C-p/RET/TAB via (minibuffer-set-key ...)), then fall back to the
//! built-in line editing below. Completion UIs live entirely in Scheme,
//! driven by the minibuffer hooks (see main.rs) and the candidate display
//! primitive (minibuffer-show-candidates lst idx).

use crate::commands::{files, movement};
use crate::dispatch::{execute_command, Lookup};
use crate::editor::{Editor, InputMode, PostAction, PromptKind, YesNoAction};
use crate::keys::{Chord, Key};
use crate::{rect, search};
use steel::rvals::SteelVal;

/// Completion domain of a prompt, exposed to Scheme as
/// (minibuffer-completion-kind). None means the prompt is not completable.
pub fn completion_kind(kind: &PromptKind) -> Option<&str> {
    match kind {
        PromptKind::ExecuteCommand | PromptKind::DescribeFunction => Some("command"),
        PromptKind::SwitchBuffer => Some("buffer"),
        PromptKind::FindFile | PromptKind::SaveFileAs => Some("file"),
        PromptKind::Generic { completion, .. } => completion.as_deref(),
        _ => None,
    }
}

pub fn handle_key(ed: &mut Editor, chord: Chord) -> PostAction {
    let InputMode::Prompt(p) = &mut ed.input else {
        return PostAction::None;
    };

    // Yes/no confirmations act on a single keypress.
    if let PromptKind::YesNo(action) = &p.kind {
        let action = action.clone();
        match chord.self_insert_char() {
            Some('y') => {
                ed.input = InputMode::Normal;
                return perform_yes(ed, action);
            }
            Some('n') => {
                ed.input = InputMode::Normal;
                ed.message("Cancelled");
            }
            _ => ed.message("Please answer y or n"),
        }
        return PostAction::None;
    }

    // Plugin bindings take precedence over the built-in editing (this is
    // how a completion UI claims C-n/C-p/RET/TAB while a prompt is active).
    if let Lookup::Cmd(name) = ed.minibuffer_map.lookup(&[chord]) {
        return execute_command(ed, &name);
    }

    let InputMode::Prompt(p) = &mut ed.input else {
        return PostAction::None;
    };
    let ctrl_char = |c: char| chord == Chord { ctrl: true, meta: false, key: Key::Char(c) };
    match chord {
        Chord { ctrl: false, meta: false, key: Key::Enter } => return submit(ed),
        Chord { ctrl: false, meta: false, key: Key::Backspace } => {
            if p.cursor > 0 {
                p.cursor -= 1;
                remove_char(&mut p.input, p.cursor);
            }
        }
        _ if ctrl_char('b') => p.cursor = p.cursor.saturating_sub(1),
        _ if ctrl_char('f') => p.cursor = (p.cursor + 1).min(p.input.chars().count()),
        _ if ctrl_char('a') => p.cursor = 0,
        _ if ctrl_char('e') => p.cursor = p.input.chars().count(),
        _ if ctrl_char('d') => remove_char(&mut p.input, p.cursor),
        _ if ctrl_char('k') => {
            let cut = byte_of(&p.input, p.cursor);
            let killed = p.input.split_off(cut);
            if !killed.is_empty() {
                ed.kill_ring.push(killed);
            }
        }
        _ => {
            if let Some(c) = chord.self_insert_char() {
                let at = byte_of(&p.input, p.cursor);
                p.input.insert(at, c);
                p.cursor += 1;
            }
        }
    }
    PostAction::None
}

/// Submit the prompt with its current input, as RET does. Also reachable
/// from Scheme through (exit-minibuffer). YesNo prompts only answer to
/// their single keypress and cannot be submitted this way.
pub fn submit(ed: &mut Editor) -> PostAction {
    match &ed.input {
        InputMode::Prompt(p) if !matches!(p.kind, PromptKind::YesNo(_)) => {}
        _ => return PostAction::None,
    }
    let InputMode::Prompt(p) = std::mem::replace(&mut ed.input, InputMode::Normal) else {
        unreachable!("checked above");
    };
    complete(ed, p.kind, p.input)
}

/// Byte offset of char index `at` (clamped to the end).
fn byte_of(s: &str, at: usize) -> usize {
    s.char_indices().nth(at).map(|(i, _)| i).unwrap_or(s.len())
}

fn remove_char(s: &mut String, at: usize) {
    let i = byte_of(s, at);
    if i < s.len() {
        s.remove(i);
    }
}

fn perform_yes(ed: &mut Editor, action: YesNoAction) -> PostAction {
    match action {
        YesNoAction::QuitModified => {
            ed.quit = true;
            PostAction::None
        }
        YesNoAction::KillModifiedBuffer(id) => {
            ed.remove_buffer(id);
            PostAction::None
        }
        YesNoAction::Generic(f) => PostAction::RunScheme(f, Vec::new()),
    }
}

fn complete(ed: &mut Editor, kind: PromptKind, input: String) -> PostAction {
    match kind {
        PromptKind::ExecuteCommand => {
            let name = input.trim();
            if name.is_empty() {
                return PostAction::None;
            }
            if ed.registry.contains_key(name) {
                return execute_command(ed, name);
            }
            ed.message(format!("No such command: {name}"));
        }
        PromptKind::FindFile => {
            let path = ed.resolve_path(&input);
            files::find_file_path(ed, &path);
        }
        PromptKind::SaveFileAs => {
            let path = ed.resolve_path(&input);
            let buf = ed.cur_buffer_mut();
            buf.path = Some(path.clone());
            buf.name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            files::save_buffer(ed, None);
        }
        PromptKind::SwitchBuffer => files::switch_to_buffer_name(ed, &input),
        PromptKind::GotoLine => match input.trim().parse::<usize>() {
            Ok(n) => movement::goto_line(ed, n),
            Err(_) => ed.message(format!("Not a line number: {input}")),
        },
        PromptKind::QueryReplaceFrom => {
            if input.is_empty() {
                ed.message("Empty regexp");
                return PostAction::None;
            }
            let prompt = format!("Query replace regexp {input} with: ");
            ed.prompt(PromptKind::QueryReplaceTo { from: input }, prompt);
        }
        PromptKind::QueryReplaceTo { from } => {
            search::start_query_replace(ed, from, input);
        }
        PromptKind::RectInsert => rect::apply_string_rectangle(ed, &input),
        PromptKind::DescribeFunction => {
            let name = input.trim();
            match ed.registry.get(name) {
                Some(cmd) => {
                    let doc = cmd.doc.clone();
                    ed.message(format!("{name}: {doc}"));
                }
                None => ed.message(format!("No such function: {name}")),
            }
        }
        PromptKind::YesNo(_) => unreachable!("handled per-key"),
        PromptKind::Generic { on_submit, .. } => {
            return PostAction::RunScheme(on_submit, vec![SteelVal::StringV(input.into())]);
        }
    }
    PostAction::None
}
