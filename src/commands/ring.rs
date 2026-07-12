//! Region, kill-ring and yank commands.

use super::check_editable;
use crate::editor::Editor;

const KILL_COMMANDS: &[&str] = &[
    "kill-line",
    "kill-word",
    "backward-kill-word",
    "kill-region",
];

fn last_was_kill(ed: &Editor) -> bool {
    ed.last_command
        .as_deref()
        .is_some_and(|c| KILL_COMMANDS.contains(&c))
}

fn last_was_yank(ed: &Editor) -> bool {
    matches!(ed.last_command.as_deref(), Some("yank") | Some("yank-pop"))
}

pub fn set_mark_command(ed: &mut Editor, _n: Option<u32>) {
    let (win, buf) = ed.cur();
    buf.mark = Some(win.point);
    buf.mark_active = true;
    ed.rect_mode = false;
    ed.message("Mark set");
}

pub fn kill_ring_save(ed: &mut Editor, _n: Option<u32>) {
    let (win, buf) = ed.cur();
    let Some((start, end)) = buf.region(win.point) else {
        ed.message("The mark is not set now, so there is no region");
        return;
    };
    let text: String = buf.rope.slice(start..end).into();
    buf.mark_active = false;
    ed.kill_ring.push(text);
    ed.message("Region copied");
}

pub fn kill_region(ed: &mut Editor, _n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let append = last_was_kill(ed);
    let (win, buf) = ed.cur();
    let Some((start, end)) = buf.region(win.point) else {
        ed.message("The mark is not set now, so there is no region");
        return;
    };
    let text = buf.remove(start, end);
    win.point = start;
    if append {
        ed.kill_ring.append(&text);
    } else {
        ed.kill_ring.push(text);
    }
}

pub fn yank(ed: &mut Editor, _n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    ed.kill_ring.reset_rotation();
    let Some(text) = ed.kill_ring.yank().map(String::from) else {
        ed.message("Kill ring is empty");
        return;
    };
    let (win, buf) = ed.cur();
    let start = win.point;
    buf.insert(start, &text);
    win.point = start + text.chars().count();
    ed.last_yank = Some((start, win.point));
}

pub fn yank_pop(ed: &mut Editor, _n: Option<u32>) {
    if !last_was_yank(ed) || ed.last_yank.is_none() {
        ed.message("Previous command was not a yank");
        return;
    }
    if !check_editable(ed) {
        return;
    }
    let Some(text) = ed.kill_ring.yank_pop().map(String::from) else {
        ed.message("Kill ring is empty");
        return;
    };
    let (start, end) = ed.last_yank.unwrap();
    let (win, buf) = ed.cur();
    buf.remove(start, end.min(buf.len_chars()));
    buf.insert(start, &text);
    win.point = start + text.chars().count();
    ed.last_yank = Some((start, win.point));
}

pub fn kill_word(ed: &mut Editor, n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let append = last_was_kill(ed);
    for i in 0..n.unwrap_or(1) {
        let (win, buf) = ed.cur();
        let end = super::movement::word_end_after(buf, win.point);
        if end == win.point {
            break;
        }
        let text = buf.remove(win.point, end);
        if append || i > 0 {
            ed.kill_ring.append(&text);
        } else {
            ed.kill_ring.push(text);
        }
    }
}

pub fn backward_kill_word(ed: &mut Editor, n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let prepend = last_was_kill(ed);
    for i in 0..n.unwrap_or(1) {
        let (win, buf) = ed.cur();
        let start = super::movement::word_start_before(buf, win.point);
        if start == win.point {
            break;
        }
        let text = buf.remove(start, win.point);
        win.point = start;
        if prepend || i > 0 {
            ed.kill_ring.prepend(&text);
        } else {
            ed.kill_ring.push(text);
        }
    }
}

pub fn kill_line(ed: &mut Editor, _n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    let append = last_was_kill(ed);
    let (win, buf) = ed.cur();
    if win.point >= buf.len_chars() {
        ed.message("End of buffer");
        return;
    }
    let eol = buf.line_end(win.point);
    // At end of line, kill the newline instead (joining lines).
    let end = if win.point == eol { eol + 1 } else { eol };
    let text = buf.remove(win.point, end.min(buf.len_chars()));
    if append {
        ed.kill_ring.append(&text);
    } else {
        ed.kill_ring.push(text);
    }
}
