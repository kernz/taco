//! Text-changing commands (other than the kill ring).

use super::check_editable;
use crate::editor::Editor;

pub fn newline(ed: &mut Editor, n: Option<u32>) {
    super::self_insert(ed, '\n', n);
}

/// C-o: insert a newline after point, leaving point (and the screen
/// position of the cursor) where it was.
pub fn open_line(ed: &mut Editor, n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let n = n.unwrap_or(1).max(1) as usize;
    let (win, buf) = ed.cur();
    buf.insert(win.point, &"\n".repeat(n));
}

pub fn delete_char(ed: &mut Editor, n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let n = n.unwrap_or(1) as usize;
    let (win, buf) = ed.cur();
    if win.point >= buf.len_chars() {
        ed.message("End of buffer");
        return;
    }
    let end = (win.point + n).min(buf.len_chars());
    buf.remove(win.point, end);
}

pub fn delete_backward_char(ed: &mut Editor, n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let n = n.unwrap_or(1) as usize;
    let (win, buf) = ed.cur();
    if win.point == 0 {
        ed.message("Beginning of buffer");
        return;
    }
    let start = win.point.saturating_sub(n);
    buf.remove(start, win.point);
    win.point = start;
}

/// Leading-whitespace length (in chars) of the line starting at `line_start`.
fn indent_of(buf: &crate::buffer::Buffer, line_start: usize) -> usize {
    let end = buf.line_end(line_start);
    (line_start..end)
        .take_while(|&i| matches!(buf.rope.char(i), ' ' | '\t'))
        .count()
}

/// Tab: indent-relative — match the previous non-blank line's indentation;
/// if already there (or no previous line), step in by four spaces.
pub fn indent_line(ed: &mut Editor, _n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let (win, buf) = ed.cur();
    let start = buf.line_start(win.point);
    let cur_indent = indent_of(buf, start);

    let mut target = cur_indent + 4;
    let mut line = buf.char_to_line(win.point);
    while line > 0 {
        line -= 1;
        let ls = buf.line_to_char(line);
        if buf.line_end(ls) > ls {
            let prev = indent_of(buf, ls);
            if prev > cur_indent {
                target = prev;
            }
            break;
        }
    }

    let offset_in_line = win.point - start;
    buf.remove(start, start + cur_indent);
    let spaces = " ".repeat(target);
    buf.insert(start, &spaces);
    // Keep point after the indentation.
    win.point = start + target + offset_in_line.saturating_sub(cur_indent);
}

pub fn newline_and_indent(ed: &mut Editor, _n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    {
        let (win, buf) = ed.cur();
        buf.insert(win.point, "\n");
        win.point += 1;
    }
    indent_line(ed, None);
}

pub fn delete_horizontal_space(ed: &mut Editor, _n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let (win, buf) = ed.cur();
    let mut start = win.point;
    while start > 0 && matches!(buf.rope.char(start - 1), ' ' | '\t') {
        start -= 1;
    }
    let mut end = win.point;
    while end < buf.len_chars() && matches!(buf.rope.char(end), ' ' | '\t') {
        end += 1;
    }
    buf.remove(start, end);
    win.point = start;
}

pub fn transpose_chars(ed: &mut Editor, _n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let (win, buf) = ed.cur();
    if win.point == 0 || buf.len_chars() < 2 {
        ed.message("Cannot transpose here");
        return;
    }
    // At end of line/buffer, swap the two preceding chars; otherwise drag the
    // char before point over the one after it, advancing point.
    let at_eol = win.point >= buf.len_chars() || buf.rope.char(win.point) == '\n';
    let (a, b) = if at_eol {
        if win.point < 2 {
            ed.message("Cannot transpose here");
            return;
        }
        (win.point - 2, win.point - 1)
    } else {
        (win.point - 1, win.point)
    };
    let ca = buf.rope.char(a);
    let cb = buf.rope.char(b);
    buf.remove(a, b + 1);
    buf.insert(a, &format!("{cb}{ca}"));
    if !at_eol {
        win.point = b + 1;
    }
}

fn case_word(ed: &mut Editor, upper: bool) {
    if !check_editable(ed) {
        return;
    }
    let (win, buf) = ed.cur();
    let end = super::movement::word_end_after(buf, win.point);
    if end == win.point {
        return;
    }
    let text: String = buf.rope.slice(win.point..end).into();
    let replaced = if upper {
        text.to_uppercase()
    } else {
        text.to_lowercase()
    };
    let start = win.point;
    buf.remove(start, end);
    buf.insert(start, &replaced);
    win.point = start + replaced.chars().count();
}

pub fn upcase_word(ed: &mut Editor, _n: Option<u32>) {
    case_word(ed, true);
}

pub fn downcase_word(ed: &mut Editor, _n: Option<u32>) {
    case_word(ed, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_line_keeps_point() {
        let mut ed = Editor::new();
        {
            let (win, buf) = ed.cur();
            buf.insert(0, "ab");
            win.point = 1;
        }
        open_line(&mut ed, None);
        assert_eq!(ed.cur_buffer().to_string_lossless(), "a\nb");
        assert_eq!(ed.windows.selected_ref().point, 1);
        open_line(&mut ed, Some(2));
        assert_eq!(ed.cur_buffer().to_string_lossless(), "a\n\n\nb");
        assert_eq!(ed.windows.selected_ref().point, 1);
    }
}

pub fn undo_cmd(ed: &mut Editor, n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    for _ in 0..n.unwrap_or(1) {
        let (win, buf) = ed.cur();
        match buf.undo_group() {
            Some(point) => {
                win.point = point.min(buf.len_chars());
                ed.message("Undo!");
            }
            None => {
                ed.message("No further undo information");
                return;
            }
        }
    }
}
