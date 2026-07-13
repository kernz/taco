//! Mouse events. Left click selects the window under the pointer and moves
//! its point to the clicked glyph; dragging extends a region from the
//! press point (mark at the anchor, point follows the pointer); the wheel
//! scrolls the window under the pointer by 3 lines without moving point —
//! unless the scroll would push point off screen, in which case point is
//! clamped to the nearest visible line (which is also what keeps
//! `ensure_point_visible` from snapping the viewport straight back on the
//! next frame).
//!
//! `handle` reports whether editor state changed, so the main loop only
//! redraws for events that did something — high-frequency drag/move events
//! that hit nothing must not trigger cursor hide/show cycles (flicker).

use crate::editor::{Editor, InputMode};
use crate::render::{char_at_display_col, gutter_width};
use crate::window::{Rect, WindowId};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

const WHEEL_LINES: usize = 3;

pub fn handle(ed: &mut Editor, ev: MouseEvent) -> bool {
    // A prompt owns the input focus; the mouse is ignored while one is up.
    if !matches!(ed.input, InputMode::Normal) {
        return false;
    }
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => click(ed, ev.column, ev.row),
        MouseEventKind::Drag(MouseButton::Left) => drag(ed, ev.column, ev.row),
        MouseEventKind::Up(MouseButton::Left) => {
            // A click that never dragged leaves no region; a finished drag
            // keeps its region active (already on screen — nothing to draw).
            ed.mouse_drag = None;
            false
        }
        MouseEventKind::ScrollUp => {
            scroll(ed, ev.column, ev.row, -(WHEEL_LINES as isize));
            true
        }
        MouseEventKind::ScrollDown => {
            scroll(ed, ev.column, ev.row, WHEEL_LINES as isize);
            true
        }
        _ => false,
    }
}

/// The window whose rectangle (mode line included) contains the cell.
fn window_at(ed: &Editor, x: u16, y: u16) -> Option<(WindowId, Rect)> {
    ed.windows
        .layout(ed.window_area())
        .into_iter()
        .find(|(_, r)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
}

/// Map a cell inside window `wid`'s text area to a char index in its
/// buffer. None for an empty buffer.
fn point_in(ed: &Editor, wid: WindowId, rect: &Rect, x: u16, y: u16) -> Option<usize> {
    let win = ed.windows.find(wid).expect("hit window exists");
    let buf = ed.buffers.get(&win.buffer).expect("live buffer");
    if buf.len_lines() == 0 {
        return None;
    }
    let rel_row = (y - rect.y) as usize;
    let line = (win.top_line + rel_row).min(buf.len_lines().saturating_sub(1));
    let gutter = gutter_width(ed, buf, rect);
    let dcol = ((x - rect.x) as usize).saturating_sub(gutter);
    Some(char_at_display_col(buf, line, dcol))
}

pub fn click(ed: &mut Editor, x: u16, y: u16) -> bool {
    let Some((wid, rect)) = window_at(ed, x, y) else {
        return false;
    };
    ed.windows.selected = wid;
    ed.mouse_drag = None;
    // A plain click always deactivates any existing region (Emacs mouse-1).
    {
        let buf_id = ed.windows.find(wid).expect("clicked window exists").buffer;
        let buf = ed.buffers.get_mut(&buf_id).expect("live buffer");
        buf.mark = None;
        buf.mark_active = false;
    }
    let rel_row = (y - rect.y) as usize;
    if rel_row >= rect.text_height() {
        return true; // mode line: selecting the window is all a click does
    }
    let Some(point) = point_in(ed, wid, &rect, x, y) else {
        return true;
    };
    ed.windows.find_mut(wid).expect("clicked window exists").point = point;
    // Anchor for a possible drag-selection starting at this press.
    ed.mouse_drag = Some((wid, point));
    true
}

/// Extend the drag-selection: mark stays at the press anchor, point follows
/// the pointer. Drags outside the anchor window (or onto its mode line)
/// change nothing and must not force a redraw.
fn drag(ed: &mut Editor, x: u16, y: u16) -> bool {
    let Some((wid, anchor)) = ed.mouse_drag else {
        return false;
    };
    let Some((hit_wid, rect)) = window_at(ed, x, y) else {
        return false;
    };
    if hit_wid != wid || (y - rect.y) as usize >= rect.text_height() {
        return false;
    }
    let Some(point) = point_in(ed, wid, &rect, x, y) else {
        return false;
    };
    let buf_id = ed.windows.find(wid).expect("drag window exists").buffer;
    let buf = ed.buffers.get_mut(&buf_id).expect("live buffer");
    let activated = !buf.mark_active || buf.mark != Some(anchor);
    buf.mark = Some(anchor);
    buf.mark_active = true;
    let win = ed.windows.find_mut(wid).expect("drag window exists");
    let moved = win.point != point;
    win.point = point;
    moved || activated
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

    #[test]
    fn drag_selects_region_from_press_anchor() {
        let mut ed = editor_with("hello\nworld\nlast");
        ed.term_size = (80, 24);
        click(&mut ed, 2, 0); // press at line 0 char 2
        assert_eq!(ed.mouse_drag, Some((ed.windows.selected, 2)));
        assert!(!ed.cur_buffer().mark_active);
        assert!(drag(&mut ed, 3, 1)); // drag to line 1 char 3
        let buf = ed.cur_buffer();
        assert_eq!(buf.mark, Some(2));
        assert!(buf.mark_active);
        assert_eq!(ed.windows.selected_ref().point, 9);
        // Releasing keeps the region but needs no redraw.
        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 3,
            row: 1,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        assert!(!handle(&mut ed, up));
        assert!(ed.cur_buffer().mark_active);
        assert_eq!(ed.mouse_drag, None);
    }

    #[test]
    fn click_deactivates_existing_region() {
        let mut ed = editor_with("hello\nworld");
        ed.term_size = (80, 24);
        {
            let (win, buf) = ed.cur();
            win.point = 4;
            buf.mark = Some(0);
            buf.mark_active = true;
        }
        click(&mut ed, 1, 1);
        let buf = ed.cur_buffer();
        assert!(!buf.mark_active);
        assert_eq!(buf.mark, None);
        assert_eq!(ed.windows.selected_ref().point, 7);
    }

    #[test]
    fn ignored_events_do_not_request_redraw() {
        let mut ed = editor_with("hello");
        ed.term_size = (80, 24);
        let moved = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 1,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        assert!(!handle(&mut ed, moved));
        // A drag with no preceding press changes nothing.
        assert!(!drag(&mut ed, 1, 0));
        // A drag onto the mode line changes nothing.
        click(&mut ed, 1, 0);
        assert!(!drag(&mut ed, 1, 23));
    }
}
