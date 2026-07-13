//! Frame rendering: full redraw per event with the cursor hidden, which is
//! flicker-free at editor scale. Every cell of every row is written, so no
//! Clear is needed.

use crate::buffer::Buffer;
use crate::editor::{Editor, FaceColor, Faces, InputMode, MAX_COMPLETION_ROWS};
use crate::search;
use crate::window::Rect;
use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::{cursor, queue, style::Print};
use std::io::Write;
use unicode_width::UnicodeWidthChar;

const TAB_WIDTH: usize = 8;

fn crossterm_color(c: FaceColor) -> Color {
    match c {
        FaceColor::Black => Color::Black,
        FaceColor::Red => Color::Red,
        FaceColor::Green => Color::Green,
        FaceColor::Yellow => Color::Yellow,
        FaceColor::Blue => Color::Blue,
        FaceColor::Magenta => Color::Magenta,
        FaceColor::Cyan => Color::Cyan,
        FaceColor::White => Color::White,
    }
}

/// Apply a face's background color if set, falling back to reverse-video.
fn set_highlight(out: &mut impl Write, color: Option<FaceColor>) -> std::io::Result<()> {
    match color {
        Some(c) => queue!(
            out,
            SetBackgroundColor(crossterm_color(c)),
            SetForegroundColor(Color::Black)
        ),
        None => queue!(out, SetAttribute(Attribute::Reverse)),
    }
}

/// Undo `set_highlight`.
fn clear_highlight(out: &mut impl Write) -> std::io::Result<()> {
    queue!(out, SetAttribute(Attribute::Reset), ResetColor)
}

/// Color just the gutter digits (never the buffer text, never the gutter
/// background): a plain foreground color when a face is configured, else the
/// original look — dim for ordinary lines, bold for the line holding point.
fn set_line_number_color(
    out: &mut impl Write,
    color: Option<FaceColor>,
    current: bool,
) -> std::io::Result<()> {
    match color {
        Some(c) => queue!(out, SetForegroundColor(crossterm_color(c))),
        None if current => queue!(out, SetAttribute(Attribute::Bold)),
        None => queue!(out, SetAttribute(Attribute::Dim)),
    }
}

/// Highlighted char-column ranges for one buffer line.
type LineRanges = Vec<(usize, usize)>;

pub fn draw(ed: &mut Editor, out: &mut impl Write) -> std::io::Result<()> {
    clamp_windows(ed);
    ensure_point_visible(ed);

    let (tw, th) = ed.term_size;
    let layout = ed.windows.layout(ed.window_area());
    let selected = ed.windows.selected;
    let faces = ed.faces.clone();

    // Lazily rehighlight any visible buffer whose text changed since the
    // last frame (see Buffer::mark_syntax_dirty / SyntaxState) — at most
    // once per buffer per edit, since a redraw follows every keystroke.
    for (wid, _) in &layout {
        let Some(win) = ed.windows.find(*wid) else { continue };
        let Some(buf) = ed.buffers.get_mut(&win.buffer) else { continue };
        if let Some(syntax) = buf.syntax.as_mut() {
            syntax.ensure_current(&buf.rope);
        }
    }

    queue!(out, cursor::Hide)?;

    let mut cursor_pos: Option<(u16, u16)> = None;
    for (wid, rect) in &layout {
        let win = ed.windows.find(*wid).expect("layout window exists");
        let buf = ed.buffers.get(&win.buffer).expect("live buffer");
        let point = win.point.min(buf.len_chars());
        let top = win.top_line.min(buf.len_lines().saturating_sub(1));

        // Optional line-number gutter (Scheme option "display-line-numbers").
        let gutter = gutter_width(ed, buf, rect);
        let point_line = buf.char_to_line(point);
        let text_rect = Rect {
            x: rect.x + gutter as u16,
            w: rect.w.saturating_sub(gutter as u16),
            ..*rect
        };

        for row in 0..rect.text_height() {
            let line_idx = top + row;
            let ranges = if *wid == selected {
                highlight_ranges(ed, buf, point, line_idx)
            } else {
                Vec::new()
            };
            let syntax = syntax_ranges(buf, &faces, line_idx);
            let text = if line_idx < buf.len_lines() {
                let l: String = buf.rope.line(line_idx).to_string();
                l.trim_end_matches('\n').to_string()
            } else {
                String::new()
            };
            if gutter > 0 {
                let num = if line_idx < buf.len_lines() {
                    format!("{:>w$} ", line_idx + 1, w = gutter - 1)
                } else {
                    " ".repeat(gutter)
                };
                // Only the digits are styled — never the buffer text next to
                // them. The line holding point gets its own customizable
                // color (`line-number-current-line`); every other line uses
                // `line-number`. Both fall back to the original dim/reverse
                // look when unset.
                queue!(out, cursor::MoveTo(rect.x, rect.y + row as u16))?;
                if line_idx == point_line && line_idx < buf.len_lines() {
                    set_line_number_color(out, faces.line_number_current, true)?;
                } else {
                    set_line_number_color(out, faces.line_number, false)?;
                }
                queue!(out, Print(num))?;
                clear_highlight(out)?;
            }
            draw_line(out, &text_rect, row as u16, &text, &ranges, &syntax, &faces)?;
        }
        draw_mode_line(out, rect, buf, point, *wid == selected, &faces)?;
        if rect.x > 0 {
            for row in 0..rect.h {
                queue!(
                    out,
                    cursor::MoveTo(rect.x - 1, rect.y + row),
                    Print('│')
                )?;
            }
        }

        if *wid == selected {
            let line = buf.char_to_line(point);
            if line >= top && (line - top) < rect.text_height() {
                let col = display_col(buf, point) + gutter;
                if (col as u16) < rect.w {
                    cursor_pos = Some((rect.x + col as u16, rect.y + (line - top) as u16));
                }
            }
        }
    }

    draw_completions(ed, out, tw, th)?;
    draw_echo_line(ed, out, tw, th, &mut cursor_pos)?;

    if let Some((x, y)) = cursor_pos {
        queue!(out, cursor::MoveTo(x, y), cursor::Show)?;
    }
    out.flush()
}

/// Keep window points/scroll within buffer bounds (buffers shrink under
/// windows that share them).
fn clamp_windows(ed: &mut Editor) {
    let lens: std::collections::HashMap<_, _> = ed
        .buffers
        .iter()
        .map(|(id, b)| (*id, (b.len_chars(), b.len_lines())))
        .collect();
    for w in ed.windows.windows_mut() {
        if let Some((chars, lines)) = lens.get(&w.buffer) {
            w.point = w.point.min(*chars);
            w.top_line = w.top_line.min(lines.saturating_sub(1));
        }
    }
}

/// Scroll the selected window so point is on screen.
fn ensure_point_visible(ed: &mut Editor) {
    let height = ed.selected_text_height();
    let (win, buf) = ed.cur();
    let line = buf.char_to_line(win.point);
    if line < win.top_line {
        win.top_line = line;
    } else if line >= win.top_line + height {
        win.top_line = line + 1 - height;
    }
}

/// Char-column highlight ranges on `line_idx`: the active region (or
/// rectangle), and the current isearch/query-replace match.
fn highlight_ranges(
    ed: &Editor,
    buf: &Buffer,
    point: usize,
    line_idx: usize,
) -> LineRanges {
    let mut ranges = Vec::new();
    if line_idx >= buf.len_lines() {
        return ranges;
    }
    let ls = buf.line_to_char(line_idx);
    let le = buf.line_end(ls);

    let mut push_span = |start: usize, end: usize| {
        if start < end {
            ranges.push((start, end));
        }
    };

    if ed.rect_mode {
        if let Some(mark) = buf.mark {
            let (l1, l2, left, right) =
                crate::rect::bounds(buf, mark.min(buf.len_chars()), point);
            if line_idx >= l1 && line_idx <= l2 {
                let len = le - ls;
                push_span(left.min(len), right.min(len));
            }
        }
    } else if let Some((start, end)) = buf.region(point) {
        let s = start.max(ls);
        let e = end.min(le);
        if s < e {
            push_span(s - ls, e - ls);
        }
    }

    let m = match &ed.input {
        InputMode::ISearch(s) => s.current,
        InputMode::QueryReplace(s) => s.current,
        _ => None,
    };
    if let Some((start, end)) = m {
        let s = start.max(ls);
        let e = end.min(le);
        if s < e && start <= le && end >= ls {
            push_span(s - ls, e - ls);
        }
    }
    ranges
}

/// Char-column, per-token syntax-color ranges on `line_idx`, from the
/// buffer's tree-sitter spans (already recomputed for this frame by
/// `draw`'s ensure_current sweep). Capture names with no configured color
/// ((set-face-color "keyword" ...) etc.) are simply skipped — no color,
/// same as any other unset face.
type SyntaxRanges = Vec<(usize, usize, FaceColor)>;

pub(crate) fn syntax_ranges(buf: &Buffer, faces: &Faces, line_idx: usize) -> SyntaxRanges {
    let mut ranges = Vec::new();
    if line_idx >= buf.len_lines() {
        return ranges;
    }
    let ls = buf.line_to_char(line_idx);
    let le = buf.line_end(ls);
    // Scheme-placed spans (buffer-add-face-span!) go first: draw_line takes
    // the first range covering a char, so they win over tree-sitter tokens.
    for (start, end, name) in &buf.face_spans {
        let s = (*start).max(ls);
        let e = (*end).min(le);
        if s < e {
            if let Some(&color) = faces.syntax.get(name) {
                ranges.push((s - ls, e - ls, color));
            }
        }
    }
    let Some(syntax) = &buf.syntax else { return ranges };
    for (start, end, name) in &syntax.spans {
        let s = (*start).max(ls);
        let e = (*end).min(le);
        if s < e {
            if let Some(&color) = faces.syntax.get(*name) {
                ranges.push((s - ls, e - ls, color));
            }
        }
    }
    ranges
}

/// Per-char style while drawing a line: `Bg` (region/isearch-match, a
/// background flip) always wins over `Fg` (a tree-sitter token's color, a
/// plain foreground change) when both would apply on the same character —
/// the same visual rule as a selected region overriding font-lock in Emacs.
#[derive(PartialEq, Eq, Clone, Copy)]
enum CharStyle {
    Plain,
    Bg,
    Fg,
}

/// Print one buffer line into a window row: tabs expanded, wide chars
/// accounted for, truncation marked with '$', region/search highlights
/// reversed, tree-sitter tokens colored.
fn draw_line(
    out: &mut impl Write,
    rect: &Rect,
    row: u16,
    text: &str,
    ranges: &LineRanges,
    syntax: &SyntaxRanges,
    faces: &Faces,
) -> std::io::Result<()> {
    queue!(out, cursor::MoveTo(rect.x, rect.y + row))?;
    let width = rect.w as usize;
    let mut col = 0usize; // display column
    let mut truncated = false;
    let mut style = CharStyle::Plain;

    for (ci, ch) in text.chars().enumerate() {
        let syntax_color = syntax
            .iter()
            .find(|(s, e, _)| ci >= *s && ci < *e)
            .map(|(_, _, c)| *c);
        let want = if ranges.iter().any(|(s, e)| ci >= *s && ci < *e) {
            CharStyle::Bg
        } else if syntax_color.is_some() {
            CharStyle::Fg
        } else {
            CharStyle::Plain
        };
        if want != style {
            clear_highlight(out)?;
            match want {
                CharStyle::Bg => set_highlight(out, faces.highlight)?,
                CharStyle::Fg => queue!(out, SetForegroundColor(crossterm_color(syntax_color.unwrap())))?,
                CharStyle::Plain => {}
            }
            style = want;
        }
        let w = match ch {
            '\t' => TAB_WIDTH - (col % TAB_WIDTH),
            _ => ch.width().unwrap_or(0),
        };
        if col + w > width.saturating_sub(1) && width > 0 {
            truncated = true;
            break;
        }
        match ch {
            '\t' => queue!(out, Print(" ".repeat(w)))?,
            _ => queue!(out, Print(ch))?,
        }
        col += w;
    }
    if style != CharStyle::Plain {
        clear_highlight(out)?;
    }
    if truncated {
        queue!(out, Print("$"))?;
        col += 1;
    }
    if col < width {
        queue!(out, Print(" ".repeat(width - col)))?;
    }
    Ok(())
}

fn draw_mode_line(
    out: &mut impl Write,
    rect: &Rect,
    buf: &Buffer,
    point: usize,
    selected: bool,
    faces: &Faces,
) -> std::io::Result<()> {
    let line = buf.char_to_line(point) + 1;
    let col = point - buf.line_start(point);
    let flags = if buf.read_only {
        "%%"
    } else if buf.modified {
        "**"
    } else {
        "--"
    };
    let mut s = format!(
        "-{}- {}   L{} C{}  ({}) ",
        flags,
        buf.name,
        line,
        col,
        buf.mode_name
    );
    let width = rect.w as usize;
    while s.chars().count() < width {
        s.push('-');
    }
    let s: String = s.chars().take(width).collect();
    queue!(
        out,
        cursor::MoveTo(rect.x, rect.y + rect.h.saturating_sub(1))
    )?;
    if selected {
        set_highlight(out, faces.mode_line)?;
    } else {
        queue!(out, SetAttribute(Attribute::Dim))?;
    }
    queue!(out, Print(s))?;
    clear_highlight(out)
}

/// The Vertico-style candidate rows, directly above the echo line. The
/// window layout already shrank by the same row count (Editor::window_area),
/// so nothing here overdraws a text window or mode line.
fn draw_completions(
    ed: &Editor,
    out: &mut impl Write,
    tw: u16,
    th: u16,
) -> std::io::Result<()> {
    let InputMode::Prompt(p) = &ed.input else { return Ok(()) };
    let rows = p.completions.len().min(MAX_COMPLETION_ROWS);
    if rows == 0 {
        return Ok(());
    }
    let sel = p.selected.min(p.completions.len() - 1);
    // Keep the selection vertically centered in the candidate window;
    // near either end of the list it walks to the edge instead, since the
    // window cannot scroll past the first or last candidate.
    let start = sel
        .saturating_sub(rows / 2)
        .min(p.completions.len() - rows);
    let width = tw as usize;
    for (i, cand) in p.completions[start..start + rows].iter().enumerate() {
        let y = th.saturating_sub(1 + (rows - i) as u16);
        let mut line: String = cand.chars().take(width).collect();
        let pad = width.saturating_sub(line.chars().map(|c| c.width().unwrap_or(0)).sum());
        line.push_str(&" ".repeat(pad));
        let selected = start + i == sel;
        queue!(
            out,
            cursor::MoveTo(0, y),
            SetAttribute(if selected { Attribute::Reverse } else { Attribute::Reset }),
            Print(line),
            SetAttribute(Attribute::Reset)
        )?;
    }
    Ok(())
}

fn draw_echo_line(
    ed: &Editor,
    out: &mut impl Write,
    tw: u16,
    th: u16,
    cursor_pos: &mut Option<(u16, u16)>,
) -> std::io::Result<()> {
    let (content, cursor_col) = match &ed.input {
        InputMode::Prompt(p) => {
            let text = format!("{}{}", p.prompt, p.input);
            let col = p
                .prompt
                .chars()
                .chain(p.input.chars().take(p.cursor))
                .map(|c| c.width().unwrap_or(0))
                .sum::<usize>();
            (text, Some(col))
        }
        InputMode::ISearch(s) => (search::isearch_echo(s), None),
        InputMode::QueryReplace(s) => (search::query_replace_echo(s), None),
        InputMode::DescribeKey { .. } | InputMode::Normal => {
            (ed.echo.clone().unwrap_or_default(), None)
        }
    };
    let width = tw as usize;
    let mut line: String = content.chars().take(width).collect();
    let pad = width.saturating_sub(line.chars().map(|c| c.width().unwrap_or(0)).sum());
    line.push_str(&" ".repeat(pad));
    queue!(out, cursor::MoveTo(0, th.saturating_sub(1)), Print(line))?;
    if let Some(col) = cursor_col {
        *cursor_pos = Some(((col as u16).min(tw.saturating_sub(1)), th.saturating_sub(1)));
    }
    Ok(())
}

/// Width of the line-number gutter for `buf` in `rect` (0 when the option
/// is off). Shared with the mouse coordinate mapping.
pub fn gutter_width(ed: &Editor, buf: &Buffer, rect: &Rect) -> usize {
    if ed.show_line_numbers {
        (digits(buf.len_lines()).max(2) + 1).min(rect.w as usize / 2)
    } else {
        0
    }
}

fn digits(mut n: usize) -> usize {
    let mut d = 1;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

/// Inverse of `display_col`: the char index on `line` whose glyph covers
/// display column `dcol` (end of line when past the text). Used by the
/// mouse click-to-point mapping.
pub fn char_at_display_col(buf: &Buffer, line: usize, dcol: usize) -> usize {
    let ls = buf.line_to_char(line);
    let le = buf.line_end(ls);
    let mut col = 0usize;
    for i in ls..le {
        let ch = buf.rope.char(i);
        let w = match ch {
            '\t' => TAB_WIDTH - (col % TAB_WIDTH),
            _ => ch.width().unwrap_or(0),
        };
        if col + w > dcol {
            return i;
        }
        col += w;
    }
    le
}

/// Display column of `point` accounting for tabs and wide glyphs.
fn display_col(buf: &Buffer, point: usize) -> usize {
    let ls = buf.line_start(point);
    let mut col = 0;
    for i in ls..point {
        let ch = buf.rope.char(i);
        col += match ch {
            '\t' => TAB_WIDTH - (col % TAB_WIDTH),
            _ => ch.width().unwrap_or(0),
        };
    }
    col
}
