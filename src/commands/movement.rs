//! Point motion and scrolling.

use crate::buffer::Buffer;
use crate::editor::{Editor, PromptKind};

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

pub fn forward_char(ed: &mut Editor, n: Option<u32>) {
    let n = n.unwrap_or(1) as usize;
    let (win, buf) = ed.cur();
    win.point = (win.point + n).min(buf.len_chars());
}

pub fn backward_char(ed: &mut Editor, n: Option<u32>) {
    let n = n.unwrap_or(1) as usize;
    let (win, _) = ed.cur();
    win.point = win.point.saturating_sub(n);
}

fn vertical_move(ed: &mut Editor, delta: isize) {
    // Consecutive line moves keep the column of the first one (goal column).
    let goal = {
        let (win, buf) = ed.cur();
        let col = win.point - buf.line_start(win.point);
        *ed.goal_col.get_or_insert(col)
    };
    let (win, buf) = ed.cur();
    let line = buf.char_to_line(win.point) as isize + delta;
    let line = line.clamp(0, buf.len_lines() as isize - 1) as usize;
    let start = buf.line_to_char(line);
    let end = buf.line_end(start);
    win.point = (start + goal).min(end);
}

pub fn next_line(ed: &mut Editor, n: Option<u32>) {
    vertical_move(ed, n.unwrap_or(1) as isize);
}

pub fn previous_line(ed: &mut Editor, n: Option<u32>) {
    vertical_move(ed, -(n.unwrap_or(1) as isize));
}

/// Char index after skipping to the end of the next word.
pub fn word_end_after(buf: &Buffer, mut pos: usize) -> usize {
    let len = buf.len_chars();
    while pos < len && !is_word(buf.rope.char(pos)) {
        pos += 1;
    }
    while pos < len && is_word(buf.rope.char(pos)) {
        pos += 1;
    }
    pos
}

/// Char index of the start of the previous word.
pub fn word_start_before(buf: &Buffer, mut pos: usize) -> usize {
    while pos > 0 && !is_word(buf.rope.char(pos - 1)) {
        pos -= 1;
    }
    while pos > 0 && is_word(buf.rope.char(pos - 1)) {
        pos -= 1;
    }
    pos
}

pub fn forward_word(ed: &mut Editor, n: Option<u32>) {
    for _ in 0..n.unwrap_or(1) {
        let (win, buf) = ed.cur();
        win.point = word_end_after(buf, win.point);
    }
}

pub fn backward_word(ed: &mut Editor, n: Option<u32>) {
    for _ in 0..n.unwrap_or(1) {
        let (win, buf) = ed.cur();
        win.point = word_start_before(buf, win.point);
    }
}

pub fn beginning_of_line(ed: &mut Editor, _n: Option<u32>) {
    let (win, buf) = ed.cur();
    win.point = buf.line_start(win.point);
}

pub fn end_of_line(ed: &mut Editor, _n: Option<u32>) {
    let (win, buf) = ed.cur();
    win.point = buf.line_end(win.point);
}

pub fn beginning_of_buffer(ed: &mut Editor, _n: Option<u32>) {
    let (win, _) = ed.cur();
    win.point = 0;
}

pub fn end_of_buffer(ed: &mut Editor, _n: Option<u32>) {
    let (win, buf) = ed.cur();
    win.point = buf.len_chars();
}

fn page(ed: &Editor) -> usize {
    // Keep two lines of context, like Emacs' next-screen-context-lines.
    ed.selected_text_height().saturating_sub(2).max(1)
}

pub fn scroll_up_command(ed: &mut Editor, _n: Option<u32>) {
    let page = page(ed);
    let height = ed.selected_text_height();
    let (win, buf) = ed.cur();
    let max_top = buf.len_lines().saturating_sub(1);
    if win.top_line >= max_top {
        ed.message("End of buffer");
        return;
    }
    win.top_line = (win.top_line + page).min(max_top);
    let point_line = buf.char_to_line(win.point);
    if point_line < win.top_line {
        win.point = buf.line_to_char(win.top_line);
    }
    let _ = height;
}

pub fn scroll_down_command(ed: &mut Editor, _n: Option<u32>) {
    let page = page(ed);
    let height = ed.selected_text_height();
    let (win, buf) = ed.cur();
    if win.top_line == 0 {
        ed.message("Beginning of buffer");
        return;
    }
    win.top_line = win.top_line.saturating_sub(page);
    let point_line = buf.char_to_line(win.point);
    if point_line >= win.top_line + height {
        let last = win.top_line + height - 1;
        win.point = buf.line_to_char(last.min(buf.len_lines().saturating_sub(1)));
    }
}

pub fn recenter(ed: &mut Editor, _n: Option<u32>) {
    let height = ed.selected_text_height();
    let (win, buf) = ed.cur();
    let point_line = buf.char_to_line(win.point);
    win.top_line = point_line.saturating_sub(height / 2);
}

pub fn goto_line_cmd(ed: &mut Editor, n: Option<u32>) {
    if let Some(n) = n {
        goto_line(ed, n as usize);
        return;
    }
    ed.prompt(PromptKind::GotoLine, "Goto line: ");
}

pub fn goto_line(ed: &mut Editor, line: usize) {
    let (win, buf) = ed.cur();
    let line = line.max(1).min(buf.len_lines());
    win.point = buf.line_to_char(line - 1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    #[test]
    fn word_boundaries() {
        let b = Buffer::new(0, "t", "foo bar-baz  qux");
        assert_eq!(word_end_after(&b, 0), 3);
        assert_eq!(word_end_after(&b, 3), 7);
        assert_eq!(word_end_after(&b, 7), 11);
        assert_eq!(word_end_after(&b, 11), 16);
        assert_eq!(word_start_before(&b, 16), 13);
        assert_eq!(word_start_before(&b, 13), 8);
        assert_eq!(word_start_before(&b, 3), 0);
    }

    #[test]
    fn word_boundaries_unicode() {
        let b = Buffer::new(0, "t", "héllo wörld");
        assert_eq!(word_end_after(&b, 0), 5);
        assert_eq!(word_end_after(&b, 5), 11);
    }
}
