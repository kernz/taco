//! Incremental regexp search (C-s / C-r) and query-replace (M-%).

use crate::commands::check_editable;
use crate::editor::{Editor, InputMode, IsearchState, PromptKind, QueryReplaceState};
use crate::keys::{Chord, Key};
use regex::Regex;

fn char_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

fn byte_to_char(s: &str, bi: usize) -> usize {
    s[..bi].chars().count()
}

/// Find a regex match in `text` (char offsets). Forward: first match starting
/// at or after `from`; backward: last match starting before `from`.
pub fn find_match(
    text: &str,
    re: &Regex,
    from: usize,
    forward: bool,
) -> Option<(usize, usize)> {
    let from_byte = char_to_byte(text, from);
    if forward {
        re.find_at(text, from_byte)
            .map(|m| (byte_to_char(text, m.start()), byte_to_char(text, m.end())))
    } else {
        re.find_iter(text)
            .take_while(|m| m.start() < from_byte)
            .last()
            .map(|m| (byte_to_char(text, m.start()), byte_to_char(text, m.end())))
    }
}

// ---- isearch ---------------------------------------------------------------

pub fn isearch_forward(ed: &mut Editor, _n: Option<u32>) {
    start_isearch(ed, true);
}

pub fn isearch_backward(ed: &mut Editor, _n: Option<u32>) {
    start_isearch(ed, false);
}

fn start_isearch(ed: &mut Editor, forward: bool) {
    let origin = ed.windows.selected_ref().point;
    ed.input = InputMode::ISearch(IsearchState {
        query: String::new(),
        forward,
        origin,
        current: None,
        wrapped: false,
        failed: false,
    });
}

/// Echo-line text for an active isearch.
pub fn isearch_echo(s: &IsearchState) -> String {
    format!(
        "{}{}Regexp I-search{}: {}",
        if s.failed { "Failing " } else { "" },
        if s.wrapped { "Wrapped " } else { "" },
        if s.forward { "" } else { " backward" },
        s.query
    )
}

pub fn isearch_key(ed: &mut Editor, chord: Chord) {
    const C_S: Chord = Chord { ctrl: true, meta: false, key: Key::Char('s') };
    const C_R: Chord = Chord { ctrl: true, meta: false, key: Key::Char('r') };
    const BSP: Chord = Chord { ctrl: false, meta: false, key: Key::Backspace };
    const RET: Chord = Chord { ctrl: false, meta: false, key: Key::Enter };

    match chord {
        RET => {
            // Accept: leave point at the match, remember where we started.
            let InputMode::ISearch(s) = &ed.input else { return };
            let origin = s.origin;
            ed.input = InputMode::Normal;
            let buf = ed.cur_buffer_mut();
            buf.mark = Some(origin);
            ed.message("Mark saved where search started");
        }
        C_S => repeat_isearch(ed, true),
        C_R => repeat_isearch(ed, false),
        BSP => {
            let InputMode::ISearch(s) = &mut ed.input else { return };
            s.query.pop();
            s.wrapped = false;
            research(ed, false);
        }
        _ => {
            if let Some(c) = chord.self_insert_char() {
                let InputMode::ISearch(s) = &mut ed.input else { return };
                s.query.push(c);
                research(ed, false);
            }
        }
    }
}

/// C-s/C-r while searching: reuse the query in the given direction, moving
/// past the current match. A failing search wraps on repeat.
fn repeat_isearch(ed: &mut Editor, forward: bool) {
    let InputMode::ISearch(s) = &mut ed.input else { return };
    s.forward = forward;
    if s.failed {
        s.wrapped = true;
        s.failed = false;
    }
    research(ed, true);
}

/// Re-run the search. `advance` moves past the current match (repeat) rather
/// than extending it in place (typed chars).
fn research(ed: &mut Editor, advance: bool) {
    let text = ed.cur_buffer().to_string_lossless();
    let InputMode::ISearch(s) = &mut ed.input else { return };
    if s.query.is_empty() {
        s.current = None;
        s.failed = false;
        let origin = s.origin;
        let (win, buf) = ed.cur();
        win.point = origin.min(buf.len_chars());
        return;
    }
    let Ok(re) = Regex::new(&s.query) else {
        // Incomplete regex (e.g. an unclosed bracket) — show as failing.
        s.failed = true;
        return;
    };
    let base = match (s.current, advance) {
        // Step past the current match (+1 guards against empty matches).
        (Some((start, end)), true) if s.forward => end.max(start + 1),
        (Some((start, _)), true) => start,
        (Some((start, _)), false) => start,
        (None, _) => s.origin,
    };
    let from = if s.wrapped {
        if s.forward { 0 } else { text.chars().count() }
    } else {
        base
    };
    match find_match(&text, &re, from, s.forward) {
        Some((start, end)) => {
            s.current = Some((start, end));
            s.failed = false;
            let target = if s.forward { end } else { start };
            let (win, buf) = ed.cur();
            win.point = target.min(buf.len_chars());
        }
        None => {
            s.failed = true;
        }
    }
}

// ---- query-replace ----------------------------------------------------------

pub fn query_replace(ed: &mut Editor, _n: Option<u32>) {
    if !check_editable(ed) {
        return;
    }
    ed.prompt(PromptKind::QueryReplaceFrom, "Query replace regexp: ");
}

/// Second prompt completed: start the interactive loop at the first match.
pub fn start_query_replace(ed: &mut Editor, from: String, to: String) {
    let re = match Regex::new(&from) {
        Ok(re) => re,
        Err(e) => {
            ed.message(format!("Invalid regexp: {e}"));
            return;
        }
    };
    let text = ed.cur_buffer().to_string_lossless();
    let point = ed.windows.selected_ref().point;
    match find_match(&text, &re, point, true) {
        Some(m) => {
            ed.input = InputMode::QueryReplace(QueryReplaceState {
                regex: from,
                replacement: to,
                current: Some(m),
                count: 0,
            });
            let (win, buf) = ed.cur();
            win.point = m.0.min(buf.len_chars());
        }
        None => ed.message("Replaced 0 occurrences"),
    }
}

pub fn query_replace_echo(s: &QueryReplaceState) -> String {
    format!(
        "Query replacing regexp {} with {}: (y)es (n)ext (!)all (q)uit",
        s.regex, s.replacement
    )
}

pub fn query_replace_key(ed: &mut Editor, chord: Chord) {
    let key = match chord {
        Chord { ctrl: false, meta: false, key: Key::Char(c) } => c,
        Chord { ctrl: false, meta: false, key: Key::Enter } => 'q',
        _ => return,
    };
    match key {
        'y' => {
            replace_current(ed);
            next_match(ed);
        }
        'n' => next_match(ed),
        '!' => loop {
            let InputMode::QueryReplace(s) = &ed.input else { break };
            if s.current.is_none() {
                break;
            }
            replace_current(ed);
            next_match(ed);
            if !matches!(ed.input, InputMode::QueryReplace(_)) {
                break;
            }
        },
        'q' => finish_query_replace(ed),
        _ => {}
    }
}

fn replace_current(ed: &mut Editor) {
    let text = ed.cur_buffer().to_string_lossless();
    let InputMode::QueryReplace(s) = &mut ed.input else { return };
    let Some((start, end)) = s.current else { return };
    let Ok(re) = Regex::new(&s.regex) else { return };
    let start_byte = char_to_byte(&text, start);
    let Some(caps) = re.captures_at(&text, start_byte) else {
        s.current = None;
        return;
    };
    // Expand $1-style capture references in the replacement.
    let mut expanded = String::new();
    caps.expand(&s.replacement, &mut expanded);
    s.count += 1;
    let new_end = start + expanded.chars().count();
    s.current = Some((start, new_end));
    let (win, buf) = ed.cur();
    buf.remove(start, end);
    buf.insert(start, &expanded);
    win.point = new_end.min(buf.len_chars());
}

fn next_match(ed: &mut Editor) {
    let text = ed.cur_buffer().to_string_lossless();
    let InputMode::QueryReplace(s) = &mut ed.input else { return };
    let Ok(re) = Regex::new(&s.regex) else {
        finish_query_replace(ed);
        return;
    };
    let from = match s.current {
        // Past the current region; +1 guards against empty matches looping.
        Some((start, end)) => end.max(start + 1),
        None => ed.windows.selected_ref().point,
    };
    match find_match(&text, &re, from, true) {
        Some(m) => {
            s.current = Some(m);
            let (win, buf) = ed.cur();
            win.point = m.0.min(buf.len_chars());
        }
        None => finish_query_replace(ed),
    }
}

fn finish_query_replace(ed: &mut Editor) {
    if let InputMode::QueryReplace(s) = &ed.input {
        let n = s.count;
        ed.input = InputMode::Normal;
        ed.message(format!(
            "Replaced {n} occurrence{}",
            if n == 1 { "" } else { "s" }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_forward_and_backward() {
        let text = "foo bar foo baz foo";
        let re = Regex::new("foo").unwrap();
        assert_eq!(find_match(text, &re, 0, true), Some((0, 3)));
        assert_eq!(find_match(text, &re, 1, true), Some((8, 11)));
        assert_eq!(find_match(text, &re, 18, false), Some((16, 19)));
        assert_eq!(find_match(text, &re, 16, false), Some((8, 11)));
        assert_eq!(find_match(text, &re, 3, false), Some((0, 3)));
        assert_eq!(find_match(text, &re, 0, false), None);
    }

    #[test]
    fn unicode_offsets() {
        let text = "héllo wörld wörld";
        let re = Regex::new("wörld").unwrap();
        assert_eq!(find_match(text, &re, 0, true), Some((6, 11)));
        assert_eq!(find_match(text, &re, 7, true), Some((12, 17)));
    }
}
