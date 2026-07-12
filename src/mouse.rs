//! Mouse events. Left click selects the window under the pointer and moves
//! its point to the clicked glyph; the wheel scrolls the window under the
//! pointer by 3 lines without moving point — unless the scroll would push
//! point off screen, in which case point is clamped to the nearest visible
//! line (which is also what keeps `ensure_point_visible` from snapping the
//! viewport straight back on the next frame).

use crate::editor::{Editor, InputMode};
use crate::render::{char_at_display_col, gutter_width};
use crate::window::{Rect, WindowId};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

const WHEEL_LINES: usize = 3;

pub fn handle(ed: &mut Editor, ev: MouseEvent) {
    // A prompt owns the input focus; the mouse is ignored while one is up.
    if !matches!(ed.input, InputMode::Normal) {
        return;
    }
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => click(ed, ev.column, ev.row),
        MouseEventKind::ScrollUp => scroll(ed, ev.column, ev.row, -(WHEEL_LINES as isize)),
        MouseEventKind::ScrollDown => scroll(ed, ev.column, ev.row, WHEEL_LINES as isize),
        _ => {}
    }
}

/// The window whose rectangle (mode line included) contains the cell.
fn window_at(ed: &Editor, x: u16, y: u16) -> Option<(WindowId, Rect)> {
    ed.windows
        .layout(ed.window_area())
        .into_iter()
        .find(|(_, r)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
}

pub fn click(ed: &mut Editor, x: u16, y: u16) {
    let Some((wid, rect)) = window_at(ed, x, y) else {
        return;
    };
    ed.windows.selected = wid;
    let rel_row = (y - rect.y) as usize;
    if rel_row >= rect.text_height() {
        return; // mode line: selecting the window is all a click does
    }
    let point = {
        let win = ed.windows.find(wid).expect("clicked window exists");
        let buf = ed.buffers.get(&win.buffer).expect("live buffer");
        if buf.len_lines() == 0 {
            return;
        }
        let line = (win.top_line + rel_row).min(buf.len_lines().saturating_sub(1));
        let gutter = gutter_width(ed, buf, &rect);
        let dcol = ((x - rect.x) as usize).saturating_sub(gutter);
        char_at_display_col(buf, line, dcol)
    };
    ed.windows.find_mut(wid).expect("clicked window exists").point = point;
}

pub fn scroll(ed: &mut Editor, x: u16, y: u16, delta: isize) {
    let Some((wid, rect)) = window_at(ed, x, y) else {
        return;
    };
    let text_h = rect.text_height().max(1);
    let (new_top, new_point) = {
        let win = ed.windows.find(wid).expect("scrolled window exists");
        let buf = ed.buffers.get(&win.buffer).expect("live buffer");
        let max_top = buf.len_lines().saturating_sub(1);
        let new_top =
            (win.top_line as isize + delta).clamp(0, max_top as isize) as usize;
        // Point follows only when pushed off screen, keeping its column.
        let point = win.point.min(buf.len_chars());
        let line = buf.char_to_line(point);
        let visible = new_top..new_top + text_h;
        let new_point = if visible.contains(&line) {
            point
        } else {
            let target = line.clamp(new_top, (new_top + text_h - 1).min(max_top));
            let col = point - buf.line_start(point);
            let ls = buf.line_to_char(target);
            (ls + col).min(buf.line_end(ls))
        };
        (new_top, new_point)
    };
    let win = ed.windows.find_mut(wid).expect("scrolled window exists");
    win.top_line = new_top;
    win.point = new_point;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_with(text: &str) -> Editor {
        let mut ed = Editor::new();
        let (win, buf) = ed.cur();
        buf.insert(0, text);
        win.point = 0;
        ed
    }

    #[test]
    fn click_maps_cell_to_point() {
        let mut ed = editor_with("hello\nwor\tld\nlast");
        ed.term_size = (80, 24);
        // Row 1 col 2 -> line 1 char 2 ('r').
        click(&mut ed, 2, 1);
        assert_eq!(ed.windows.selected_ref().point, 8);
        // The tab spans columns 3..8 on line 1: clicking inside it lands on
        // the tab char itself.
        click(&mut ed, 5, 1);
        assert_eq!(ed.windows.selected_ref().point, 9);
        // Past the end of a line clamps to its end.
        click(&mut ed, 70, 0);
        assert_eq!(ed.windows.selected_ref().point, 5);
        // Below the last line clamps to the last line.
        click(&mut ed, 0, 20);
        assert_eq!(ed.windows.selected_ref().point, 13);
    }

    #[test]
    fn click_accounts_for_gutter() {
        let mut ed = editor_with("hello\nworld");
        ed.term_size = (80, 24);
        ed.show_line_numbers = true; // gutter is 3 cells wide (" 1 ")
        click(&mut ed, 3, 0);
        assert_eq!(ed.windows.selected_ref().point, 0);
        click(&mut ed, 5, 0);
        assert_eq!(ed.windows.selected_ref().point, 2);
    }

    #[test]
    fn wheel_scrolls_without_moving_point_until_pushed() {
        let text: String = (0..60).map(|i| format!("line {i}\n")).collect();
        let mut ed = editor_with(&text);
        ed.term_size = (80, 24); // 23 rows of windows, 22 text rows
        scroll(&mut ed, 0, 0, 3);
        let win = ed.windows.selected_ref();
        assert_eq!(win.top_line, 3);
        // Point (line 0) was pushed off the top: clamped to first visible.
        let line_of_point = 3; // "line 3" starts the view
        assert_eq!(
            scheme_line(&ed, win.point),
            line_of_point
        );
        // Scrolling back up: point stays put once visible.
        scroll(&mut ed, 0, 0, -3);
        let win = ed.windows.selected_ref();
        assert_eq!(win.top_line, 0);
        assert_eq!(scheme_line(&ed, win.point), 3);
        // Top clamps at 0.
        scroll(&mut ed, 0, 0, -3);
        assert_eq!(ed.windows.selected_ref().top_line, 0);
    }

    fn scheme_line(ed: &Editor, point: usize) -> usize {
        ed.cur_buffer().char_to_line(point)
    }
}
