//! Native command implementations. Every entry in COMMANDS is registered
//! both in the Editor's command registry (for keybindings via
//! `global-set-key`) and as a callable Steel function.

pub mod editing;
pub mod files;
pub mod help;
pub mod movement;
pub mod ring;
pub mod windows;

use crate::editor::{Editor, InputMode, NativeFn, PrefixArg};
use crate::{dired, rect, search};

pub struct Spec {
    pub name: &'static str,
    pub doc: &'static str,
    pub f: NativeFn,
}

macro_rules! spec {
    ($name:literal, $f:path, $doc:literal) => {
        Spec { name: $name, doc: $doc, f: $f }
    };
}

pub const COMMANDS: &[Spec] = &[
    // Basics & system
    spec!("save-buffers-kill-terminal", files::save_buffers_kill_terminal, "Quit the editor, confirming if modified buffers exist."),
    spec!("keyboard-quit", keyboard_quit, "Cancel the in-progress key sequence, prompt, search or region."),
    spec!("save-buffer", files::save_buffer, "Save the current buffer to its file (prompts for a name if it has none)."),
    spec!("switch-to-buffer", files::switch_to_buffer, "Prompt for a buffer name and switch to it (creates it if new)."),
    spec!("kill-buffer", files::kill_buffer, "Kill the current buffer."),
    spec!("find-file", files::find_file, "Prompt for a file and visit it (directories open in dired)."),
    spec!("undo", editing::undo_cmd, "Undo the last change in the current buffer."),
    // Movement
    spec!("forward-char", movement::forward_char, "Move point forward one character."),
    spec!("backward-char", movement::backward_char, "Move point backward one character."),
    spec!("next-line", movement::next_line, "Move point to the next line, keeping the goal column."),
    spec!("previous-line", movement::previous_line, "Move point to the previous line, keeping the goal column."),
    spec!("forward-word", movement::forward_word, "Move point forward one word."),
    spec!("backward-word", movement::backward_word, "Move point backward one word."),
    spec!("beginning-of-line", movement::beginning_of_line, "Move point to the beginning of the current line."),
    spec!("end-of-line", movement::end_of_line, "Move point to the end of the current line."),
    spec!("beginning-of-buffer", movement::beginning_of_buffer, "Move point to the beginning of the buffer."),
    spec!("end-of-buffer", movement::end_of_buffer, "Move point to the end of the buffer."),
    spec!("scroll-up-command", movement::scroll_up_command, "Scroll one screen forward."),
    spec!("scroll-down-command", movement::scroll_down_command, "Scroll one screen backward."),
    spec!("recenter", movement::recenter, "Center the screen on the line containing point."),
    spec!("goto-line", movement::goto_line_cmd, "Prompt for a line number and move point there."),
    // Searching & editing
    spec!("isearch-forward", search::isearch_forward, "Incremental regexp search forward (C-s again for next match)."),
    spec!("isearch-backward", search::isearch_backward, "Incremental regexp search backward."),
    spec!("query-replace", search::query_replace, "Interactively replace regexp matches (y/n/!/q)."),
    spec!("indent-line", editing::indent_line, "Indent the current line (relative to the previous line)."),
    spec!("newline", editing::newline, "Insert a newline at point."),
    spec!("open-line", editing::open_line, "Insert a newline after point without moving the cursor."),
    spec!("newline-and-indent", editing::newline_and_indent, "Insert a newline, then indent the new line."),
    spec!("delete-horizontal-space", editing::delete_horizontal_space, "Delete all spaces and tabs around point."),
    spec!("delete-char", editing::delete_char, "Delete the character after point."),
    spec!("delete-backward-char", editing::delete_backward_char, "Delete the character before point."),
    spec!("backward-kill-word", ring::backward_kill_word, "Kill the word before point."),
    spec!("rectangle-mark-mode", rect::rectangle_mark_mode, "Enter rectangle mode; movement extends the rectangle."),
    spec!("string-rectangle", rect::string_rectangle_cmd, "Prompt for text and insert it into each line of the rectangle."),
    // Kill ring
    spec!("set-mark-command", ring::set_mark_command, "Set the mark at point to start a region."),
    spec!("kill-ring-save", ring::kill_ring_save, "Copy the region to the kill ring."),
    spec!("kill-region", ring::kill_region, "Kill (cut) the region."),
    spec!("yank", ring::yank, "Yank (paste) the most recent kill."),
    spec!("yank-pop", ring::yank_pop, "Replace the just-yanked text with the previous kill."),
    spec!("kill-word", ring::kill_word, "Kill the word starting at point."),
    spec!("kill-line", ring::kill_line, "Kill from point to the end of the line."),
    // Formatting & windows
    spec!("transpose-chars", editing::transpose_chars, "Swap the two characters around point."),
    spec!("upcase-word", editing::upcase_word, "Uppercase from point to the end of the word."),
    spec!("downcase-word", editing::downcase_word, "Lowercase from point to the end of the word."),
    spec!("other-window", windows::other_window, "Select the next window."),
    spec!("delete-other-windows", windows::delete_other_windows, "Delete all windows except the selected one."),
    spec!("split-window-below", windows::split_window_below, "Split the selected window above and below."),
    spec!("split-window-right", windows::split_window_right, "Split the selected window side by side."),
    spec!("delete-window", windows::delete_window, "Delete the selected window."),
    spec!("execute-extended-command", help::execute_extended_command, "Prompt for a command name and run it."),
    spec!("describe-key", help::describe_key, "Read a key sequence and describe the command it runs."),
    spec!("describe-function", help::describe_function, "Prompt for a command name and show its documentation."),
    // Dired
    spec!("dired-open-dir", dired::open_dir_cmd, "Prompt for a directory and open it in dired."),
    spec!("dired-current", dired::current_cmd, "Open the current buffer's directory in dired."),
    spec!("dired-jump", dired::jump_cmd, "Open dired at the current file's directory, cursor on that file."),
    spec!("dired-project-root", dired::project_root_cmd, "Open the project root (nearest ancestor with .git) in dired."),
    spec!("dired-find-file", dired::find_file_cmd, "Visit the file or directory at point."),
    spec!("dired-find-file-other-window", dired::find_file_other_window_cmd, "Visit the file or directory at point in another window."),
    spec!("dired-up-directory", dired::up_directory_cmd, "Open the parent directory."),
    spec!("dired-mark", dired::mark_cmd, "Mark the file at point."),
    spec!("dired-mark-regexp", dired::mark_regexp_cmd, "Prompt for a regexp and mark all matching files."),
    spec!("dired-shell-command", dired::shell_command_cmd, "Run a shell command on the marked files (or the file at point)."),
    spec!("dired-flag-deletion", dired::flag_deletion_cmd, "Flag the file at point for deletion."),
    spec!("dired-do-flagged-delete", dired::do_flagged_delete_cmd, "Delete the files flagged with D."),
    spec!("dired-unmark", dired::unmark_cmd, "Unmark the file at point."),
    spec!("dired-unmark-all", dired::unmark_all_cmd, "Unmark all files."),
    spec!("dired-do-delete", dired::do_delete_cmd, "Delete the file at point."),
    spec!("dired-do-rename", dired::do_rename_cmd, "Rename the file at point."),
    spec!("dired-do-copy", dired::do_copy_cmd, "Copy the file at point."),
    spec!("dired-create-directory", dired::create_directory_cmd, "Prompt for a name and create a directory."),
    spec!("dired-diff", dired::diff_cmd, "Diff the file at point against another file."),
    spec!("dired-compress", dired::compress_cmd, "Compress the file (gz) or directory (tar.gz) at point."),
    spec!("dired-revert", dired::revert_cmd, "Refresh the dired listing."),
    spec!("dired-toggle-hidden", dired::toggle_hidden_cmd, "Toggle showing hidden files."),
    spec!("dired-kill-all", dired::kill_all_cmd, "Kill all dired buffers."),
    spec!("wgrep-mode", dired::wgrep_mode_cmd, "Make the dired buffer writable to edit file names as plain text."),
    spec!("wgrep-commit", dired::wgrep_commit_cmd, "Apply the edited file names (renames files on disk)."),
    spec!("wgrep-abort", dired::wgrep_abort_cmd, "Abort wgrep editing and restore the listing."),
];

/// Install every native command into the editor's registry.
pub fn install(ed: &mut Editor) {
    for spec in COMMANDS {
        ed.registry.insert(
            spec.name.to_string(),
            crate::editor::Command {
                doc: spec.doc.to_string(),
                f: crate::editor::CommandFn::Native(spec.f),
            },
        );
    }
}

/// C-g: cancel whatever is in progress.
pub fn keyboard_quit(ed: &mut Editor, _n: Option<u32>) {
    ed.pending.clear();
    ed.prefix = PrefixArg::None;
    // Abort an in-progress isearch back to its origin.
    let isearch_origin = match &ed.input {
        InputMode::ISearch(s) => Some(s.origin),
        _ => None,
    };
    if let Some(origin) = isearch_origin {
        let (win, buf) = ed.cur();
        win.point = origin.min(buf.len_chars());
    }
    ed.input = InputMode::Normal;
    ed.rect_mode = false;
    let buf = ed.cur_buffer_mut();
    buf.mark_active = false;
    ed.message("Quit");
}

/// Insert `c` (`n` times) at point — the fallback for unbound printable keys
/// and the implementation of `C-u n char`.
pub fn self_insert(ed: &mut Editor, c: char, n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let n = n.unwrap_or(1).max(1) as usize;
    let text: String = std::iter::repeat(c).take(n).collect();
    let (win, buf) = ed.cur();
    buf.insert(win.point, &text);
    win.point += n;
}

/// Guard for editing commands: false (with a message) on read-only buffers.
pub fn check_editable(ed: &mut Editor) -> bool {
    if ed.cur_buffer().read_only {
        ed.message("Buffer is read-only");
        false
    } else {
        true
    }
}
