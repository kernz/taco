//! Window management commands.

use crate::editor::Editor;
use crate::window::SplitDir;

pub fn split_window_below(ed: &mut Editor, _n: Option<u32>) {
    ed.windows.split(SplitDir::Below);
}

pub fn split_window_right(ed: &mut Editor, _n: Option<u32>) {
    ed.windows.split(SplitDir::Right);
}

pub fn other_window(ed: &mut Editor, n: Option<u32>) {
    for _ in 0..n.unwrap_or(1) {
        ed.windows.select_next();
    }
    let id = ed.windows.selected_ref().buffer;
    ed.touch_buffer(id);
}

pub fn delete_window(ed: &mut Editor, _n: Option<u32>) {
    let sel = ed.windows.selected;
    if !ed.windows.delete(sel) {
        ed.message("Attempt to delete sole ordinary window");
    }
}

pub fn delete_other_windows(ed: &mut Editor, _n: Option<u32>) {
    ed.windows.delete_others();
}
