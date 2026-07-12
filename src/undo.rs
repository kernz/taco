//! Command-grouped undo log. Each editing command is preceded by a Boundary;
//! `undo` reverts records until it crosses one.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoRecord {
    Boundary,
    /// Text was inserted at `at` with char length `len`.
    Insert { at: usize, len: usize },
    /// `text` was deleted starting at char index `at`.
    Delete { at: usize, text: String },
}

#[derive(Debug, Default)]
pub struct UndoLog {
    records: Vec<UndoRecord>,
}

impl UndoLog {
    pub fn push_boundary(&mut self) {
        if !matches!(self.records.last(), Some(UndoRecord::Boundary) | None) {
            self.records.push(UndoRecord::Boundary);
        }
    }

    pub fn record(&mut self, rec: UndoRecord) {
        self.records.push(rec);
    }

    /// Pop one undo group (records back to the previous boundary).
    pub fn pop_group(&mut self) -> Vec<UndoRecord> {
        // Skip trailing boundaries.
        while matches!(self.records.last(), Some(UndoRecord::Boundary)) {
            self.records.pop();
        }
        let mut group = Vec::new();
        while let Some(rec) = self.records.last() {
            if matches!(rec, UndoRecord::Boundary) {
                break;
            }
            group.push(self.records.pop().unwrap());
        }
        group
    }
}
