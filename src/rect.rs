//! Rectangle mode: C-x SPC anchors a rectangle at point; movement extends
//! it; C-x r t replaces each line's slice of the rectangle with a string.

use crate::buffer::Buffer;
use crate::commands::check_editable;
use crate::editor::{Editor, PromptKind};

pub fn rectangle_mark_mode(ed: &mut Editor, _n: Option<u32>) {
    let (win, buf) = ed.cur();
    buf.mark = Some(win.point);
    buf.mark_active = true;
    ed.rect_mode = true;
    ed.message("Rectangle mark mode (C-x r t inserts text, C-g cancels)");
}

pub fn string_rectangle_cmd(ed: &mut Editor, _n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let buf = ed.cur_buffer();
    if !(ed.rect_mode && buf.mark_active && buf.mark.is_some()) {
        ed.message("No rectangle selected (use C-x SPC first)");
        return;
    }
    ed.prompt(PromptKind::RectInsert, "String rectangle: ");
}

/// The rectangle between two char positions: (first_line, last_line,
/// left_col, right_col), columns in chars.
pub fn bounds(buf: &Buffer, a: usize, b: usize) -> (usize, usize, usize, usize) {
    let (start, end) = (a.min(b), a.max(b));
    let l1 = buf.char_to_line(start);
    let l2 = buf.char_to_line(end);
    let c1 = start - buf.line_start(start);
    let c2 = end - buf.line_start(end);
    (l1, l2, c1.min(c2), c1.max(c2))
}

/// Replace the rectangle's slice on every line with `text`.
pub fn apply_string_rectangle(ed: &mut Editor, text: &str) {
    let (win, buf) = ed.cur();
    let Some(mark) = buf.mark else {
        return;
    };
    let (l1, l2, left, right) = bounds(buf, mark.min(buf.len_chars()), win.point);
    // Bottom-up so earlier char indices stay valid.
    for line in (l1..=l2).rev() {
        let ls = buf.line_to_char(line);
        let le = buf.line_end(ls);
        let len = le - ls;
        let a = ls + left.min(len);
        let b = ls + right.min(len);
        buf.remove(a, b);
        // Pad short lines out to the left column before inserting.
        let pad = left.saturating_sub(len);
        let insertion = format!("{}{}", " ".repeat(pad), text);
        buf.insert(a, &insertion);
    }
    buf.mark_active = false;
    ed.rect_mode = false;
    let (win, buf) = ed.cur();
    let ls = buf.line_to_char(l1);
    win.point = (ls + left + text.chars().count()).min(buf.len_chars());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    #[test]
    fn string_rectangle_replaces_columns() {
        let mut ed = Editor::new();
        let id = ed.create_buffer("t", "aaaa\nbbbb\ncccc\n");
        ed.show_buffer(id);
        // Anchor at line 0 col 1, point at line 2 col 3.
        {
            let (win, buf) = ed.cur();
            buf.mark = Some(1);
            buf.mark_active = true;
            win.point = 13; // line 2 (chars 10..14), col 3
        }
        ed.rect_mode = true;
        apply_string_rectangle(&mut ed, "XY");
        assert_eq!(ed.cur_buffer().to_string_lossless(), "aXYa\nbXYb\ncXYc\n");
    }
}
