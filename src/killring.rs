//! The kill ring: killed text accumulates here; C-y yanks the front,
//! M-y rotates. Consecutive kills append into one entry.

const MAX_ENTRIES: usize = 60;

#[derive(Debug, Default)]
pub struct KillRing {
    entries: Vec<String>,
    /// Rotation offset for yank-pop: 0 = most recent kill.
    yank_index: usize,
}

impl KillRing {
    pub fn push(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.entries.push(text);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.yank_index = 0;
    }

    /// Append to the latest entry (consecutive forward kills).
    pub fn append(&mut self, text: &str) {
        match self.entries.last_mut() {
            Some(last) => last.push_str(text),
            None => self.push(text.to_string()),
        }
        self.yank_index = 0;
    }

    /// Prepend to the latest entry (consecutive backward kills).
    pub fn prepend(&mut self, text: &str) {
        match self.entries.last_mut() {
            Some(last) => *last = format!("{text}{last}"),
            None => self.push(text.to_string()),
        }
        self.yank_index = 0;
    }

    /// Current yank text (front of ring adjusted by rotation).
    pub fn yank(&self) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        let idx = self.entries.len() - 1 - (self.yank_index % self.entries.len());
        self.entries.get(idx).map(String::as_str)
    }

    /// Rotate to the previous kill and return it (M-y).
    pub fn yank_pop(&mut self) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        self.yank_index = (self.yank_index + 1) % self.entries.len();
        self.yank()
    }

    pub fn reset_rotation(&mut self) {
        self.yank_index = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yank_and_rotate() {
        let mut kr = KillRing::default();
        kr.push("one".into());
        kr.push("two".into());
        kr.push("three".into());
        assert_eq!(kr.yank(), Some("three"));
        assert_eq!(kr.yank_pop(), Some("two"));
        assert_eq!(kr.yank_pop(), Some("one"));
        assert_eq!(kr.yank_pop(), Some("three"));
        kr.push("four".into());
        assert_eq!(kr.yank(), Some("four"));
    }

    #[test]
    fn append_prepend() {
        let mut kr = KillRing::default();
        kr.push("bar".into());
        kr.append("baz");
        kr.prepend("foo");
        assert_eq!(kr.yank(), Some("foobarbaz"));
    }
}
