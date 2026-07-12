//! Text buffers: a ropey Rope plus file association, modification state,
//! the mark, the undo log, and the major mode. All positions are char indices.

use crate::dired::DiredState;
use crate::undo::{UndoLog, UndoRecord};
use anyhow::{Context, Result};
use ropey::Rope;
use std::path::{Path, PathBuf};

pub type BufferId = usize;

#[derive(Debug, Default)]
pub enum Mode {
    #[default]
    Fundamental,
    Dired(DiredState),
}

impl Mode {
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Fundamental => "Fundamental",
            Mode::Dired(d) if d.wgrep.is_some() => "Dired:Wgrep",
            Mode::Dired(_) => "Dired",
        }
    }
}

#[derive(Debug)]
pub struct Buffer {
    pub id: BufferId,
    pub name: String,
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub modified: bool,
    pub read_only: bool,
    /// The mark (C-SPC). `mark_active` distinguishes a live region.
    pub mark: Option<usize>,
    pub mark_active: bool,
    pub mode: Mode,
    pub undo: UndoLog,
    /// Point to restore when the buffer is next shown in a window.
    pub last_point: usize,
    /// Suppress undo recording while replaying an undo group.
    replaying: bool,
}

impl Buffer {
    pub fn new(id: BufferId, name: impl Into<String>, text: &str) -> Self {
        Buffer {
            id,
            name: name.into(),
            rope: Rope::from_str(text),
            path: None,
            modified: false,
            read_only: false,
            mark: None,
            mark_active: false,
            mode: Mode::Fundamental,
            undo: UndoLog::default(),
            last_point: 0,
            replaying: false,
        }
    }

    pub fn from_file(id: BufferId, path: &Path) -> Result<Self> {
        let text = if path.exists() {
            std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?
        } else {
            String::new()
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut buf = Buffer::new(id, name, &text);
        buf.path = Some(path.to_path_buf());
        Ok(buf)
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// Insert `text` at char index `at`, recording undo and clamping `at`.
    pub fn insert(&mut self, at: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        let at = at.min(self.len_chars());
        self.rope.insert(at, text);
        self.modified = true;
        self.mark_active = false;
        if !self.replaying {
            self.undo
                .record(UndoRecord::Insert { at, len: text.chars().count() });
        }
    }

    /// Remove chars in `[start, end)`, recording undo. Returns the removed text.
    pub fn remove(&mut self, start: usize, end: usize) -> String {
        let end = end.min(self.len_chars());
        let start = start.min(end);
        if start == end {
            return String::new();
        }
        let text: String = self.rope.slice(start..end).into();
        self.rope.remove(start..end);
        self.modified = true;
        self.mark_active = false;
        if !self.replaying {
            self.undo
                .record(UndoRecord::Delete { at: start, text: text.clone() });
        }
        text
    }

    /// Revert one undo group. Returns the char position to move point to,
    /// or None when there was nothing to undo.
    pub fn undo_group(&mut self) -> Option<usize> {
        let group = self.undo.pop_group();
        if group.is_empty() {
            return None;
        }
        self.replaying = true;
        let mut point = None;
        for rec in group {
            match rec {
                UndoRecord::Insert { at, len } => {
                    let end = (at + len).min(self.rope.len_chars());
                    self.rope.remove(at..end);
                    point = Some(at);
                }
                UndoRecord::Delete { at, text } => {
                    self.rope.insert(at, &text);
                    point = Some(at + text.chars().count());
                }
                UndoRecord::Boundary => {}
            }
        }
        self.replaying = false;
        self.modified = true;
        point
    }

    pub fn save(&mut self) -> Result<()> {
        let path = self
            .path
            .clone()
            .context("buffer has no associated file")?;
        let mut out = String::with_capacity(self.rope.len_bytes());
        for chunk in self.rope.chunks() {
            out.push_str(chunk);
        }
        std::fs::write(&path, out)
            .with_context(|| format!("writing {}", path.display()))?;
        self.modified = false;
        Ok(())
    }

    /// Char index of the first char of the line containing `pos`.
    pub fn line_start(&self, pos: usize) -> usize {
        let line = self.char_to_line(pos);
        self.rope.line_to_char(line)
    }

    /// Char index just before the newline of the line containing `pos`
    /// (or end of buffer on the last line).
    pub fn line_end(&self, pos: usize) -> usize {
        let line = self.char_to_line(pos);
        let start = self.rope.line_to_char(line);
        let mut end = start + self.rope.line(line).len_chars();
        if end > start && self.rope.char(end - 1) == '\n' {
            end -= 1;
        }
        end
    }

    pub fn char_to_line(&self, pos: usize) -> usize {
        self.rope.char_to_line(pos.min(self.len_chars()))
    }

    pub fn line_to_char(&self, line: usize) -> usize {
        self.rope.line_to_char(line.min(self.len_lines().saturating_sub(1)))
    }

    /// Live region between mark and point, ordered.
    pub fn region(&self, point: usize) -> Option<(usize, usize)> {
        if !self.mark_active {
            return None;
        }
        let mark = self.mark?;
        Some((mark.min(point), mark.max(point)))
    }

    pub fn to_string_lossless(&self) -> String {
        self.rope.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_roundtrip() {
        let mut b = Buffer::new(0, "t", "hello world");
        b.undo.push_boundary();
        b.insert(5, ", brave");
        assert_eq!(b.to_string_lossless(), "hello, brave world");
        b.undo.push_boundary();
        b.remove(0, 5);
        assert_eq!(b.to_string_lossless(), ", brave world");
        let p = b.undo_group().unwrap();
        assert_eq!(b.to_string_lossless(), "hello, brave world");
        assert_eq!(p, 5);
        b.undo_group().unwrap();
        assert_eq!(b.to_string_lossless(), "hello world");
        assert!(b.undo_group().is_none());
    }

    #[test]
    fn line_bounds() {
        let b = Buffer::new(0, "t", "one\ntwo\nthree");
        assert_eq!(b.line_start(5), 4);
        assert_eq!(b.line_end(5), 7);
        assert_eq!(b.line_end(12), 13);
    }
}
