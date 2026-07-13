//! File and buffer lifecycle commands.

use crate::buffer::Buffer;
use crate::editor::{Editor, PostAction, PromptKind, YesNoAction};
use std::path::Path;
use steel::rvals::SteelVal;

pub fn find_file(ed: &mut Editor, _n: Option<u32>) {
    let dir = ed.default_dir();
    let mut prefill = dir.display().to_string();
    if !prefill.ends_with('/') {
        prefill.push('/');
    }
    ed.prompt_prefilled(PromptKind::FindFile, "Find file: ", prefill);
}

/// Visit `path` directly (used by prompt completion and the Scheme API).
/// Directories hand off to whatever mode registered itself as the
/// directory opener (dired.scm, normally) — queued like any other
/// engine-reentry, since this can run with an Editor borrow held.
pub fn find_file_path(ed: &mut Editor, path: &Path) {
    if path.is_dir() {
        if let Some(f) = ed.directory_opener.clone() {
            let arg = SteelVal::StringV(path.display().to_string().into());
            ed.deferred.push(PostAction::RunScheme(f, vec![arg]));
        } else {
            ed.message("No directory opener registered (dired.scm not loaded?)");
        }
        return;
    }
    if let Some(id) = ed.buffer_by_path(path) {
        ed.show_buffer(id);
        ed.file_visited = true;
        return;
    }
    match ed.add_buffer(|id| Buffer::from_file(id, path)) {
        Ok(id) => {
            ed.show_buffer(id);
            ed.file_visited = true;
            if !path.exists() {
                ed.message("(New file)");
            }
        }
        Err(e) => ed.message(format!("{e:#}")),
    }
}

pub fn save_buffer(ed: &mut Editor, _n: Option<u32>) {
    let buf = ed.cur_buffer();
    if buf.path.is_none() {
        let dir = ed.default_dir();
        let mut prefill = dir.display().to_string();
        if !prefill.ends_with('/') {
            prefill.push('/');
        }
        ed.prompt_prefilled(PromptKind::SaveFileAs, "File to save in: ", prefill);
        return;
    }
    if !buf.modified {
        ed.message("(No changes need to be saved)");
        return;
    }
    let buf = ed.cur_buffer_mut();
    match buf.save() {
        Ok(()) => {
            let msg = format!(
                "Wrote {}",
                buf.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
            );
            ed.message(msg);
        }
        Err(e) => ed.message(format!("{e:#}")),
    }
}

pub fn switch_to_buffer(ed: &mut Editor, _n: Option<u32>) {
    let default = ed
        .recency
        .iter()
        .skip(1)
        .find_map(|id| ed.buffers.get(id))
        .map(|b| b.name.clone());
    let prompt = match &default {
        Some(name) => format!("Switch to buffer (default {name}): "),
        None => "Switch to buffer: ".to_string(),
    };
    ed.prompt(PromptKind::SwitchBuffer, prompt);
}

/// Prompt completion for C-x b: empty input takes the default (most recent
/// other buffer); unknown names create a fresh buffer, like Emacs.
pub fn switch_to_buffer_name(ed: &mut Editor, name: &str) {
    let name = name.trim();
    let target = if name.is_empty() {
        ed.recency.get(1).copied()
    } else {
        ed.buffer_by_name(name)
    };
    match target {
        Some(id) => ed.show_buffer(id),
        None if name.is_empty() => ed.message("No other buffer"),
        None => {
            let id = ed.create_buffer(name, "");
            ed.show_buffer(id);
        }
    }
}

pub fn kill_buffer(ed: &mut Editor, _n: Option<u32>) {
    let buf = ed.cur_buffer();
    let id = buf.id;
    if buf.modified && buf.path.is_some() {
        ed.prompt(
            PromptKind::YesNo(YesNoAction::KillModifiedBuffer(id)),
            format!("Buffer {} modified; kill anyway? (y or n) ", buf.name),
        );
        return;
    }
    ed.remove_buffer(id);
}

pub fn save_buffers_kill_terminal(ed: &mut Editor, _n: Option<u32>) {
    let dirty = ed
        .buffers
        .values()
        .any(|b| b.modified && b.path.is_some());
    if dirty {
        ed.prompt(
            PromptKind::YesNo(YesNoAction::QuitModified),
            "Modified buffers exist; exit anyway? (y or n) ",
        );
        return;
    }
    ed.quit = true;
}
