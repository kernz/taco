//! Real dynamic tree-sitter grammar installation: clone a grammar's git
//! repo, compile it with the system C compiler, and load it at runtime —
//! the "Native C exception" for syntax highlighting, mirroring Emacs'
//! `treesit-install-language-grammar`. Mechanism only: which languages
//! exist and which file extensions use them is Scheme's job
//! (`tree-sit-enable-for-extension` in bootstrap.scm).
//!
//! Compiling and dynamically loading the grammar is delegated entirely to
//! `tree-sitter-loader` (the same crate the `tree-sitter` CLI itself uses)
//! — it shells out to the system C compiler and caches the compiled
//! library in the OS cache dir, skipping recompilation when unchanged.

use ropey::Rope;
use std::path::{Path, PathBuf};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Capture names recognized in highlight queries, matched by longest
/// dot-prefix (see `HighlightConfiguration::configure`). Index `i` in a
/// `HighlightEvent::HighlightStart` refers back into this list, which is
/// how `SyntaxState::ensure_current` resolves a span's face name for
/// `(set-face-color name color)`.
pub const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "character",
    "comment",
    "comment.documentation",
    "constant",
    "constant.builtin",
    "constructor",
    "delimiter",
    "embedded",
    "escape",
    "function",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// A grammar installed via `(tree-sit-install-language-grammar name url)`.
pub struct TreesitLanguage {
    pub language: tree_sitter::Language,
    pub highlights_query: String,
    pub injections_query: String,
}

/// Where cloned grammar repos live. The *compiled* library is cached
/// separately by tree-sitter-loader itself (in the OS cache dir); this is
/// just the git checkout, which we also need around to read `queries/*.scm`
/// from.
fn grammars_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("taco/grammars")
}

/// Clone `url` (skipped if already cloned — matches Emacs' idempotent
/// reinstall), compile it, and load it. Ok carries the language plus
/// whether this was a *fresh* install (the repo had to be cloned) — a
/// cached grammar loading on a later launch isn't worth announcing on the
/// echo line. Err carries a message suitable for `(message ...)`.
pub fn install_language_grammar(name: &str, url: &str) -> Result<(TreesitLanguage, bool), String> {
    let dest = grammars_dir().join(name);
    let fresh = !dest.join(".git").exists();
    if fresh {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        let status = std::process::Command::new("git")
            .args(["clone", "--depth", "1", url, &dest.display().to_string()])
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => return Err(format!("git clone exited with {s}")),
            Err(e) => return Err(format!("could not run git: {e} (is git installed?)")),
        }
    }
    load_installed(&dest).map(|lang| (lang, fresh))
}

fn load_installed(dest: &Path) -> Result<TreesitLanguage, String> {
    let src = dest.join("src");
    let loader = tree_sitter_loader::Loader::new().map_err(|e| e.to_string())?;
    let config = tree_sitter_loader::CompileConfig::new(&src, None, None);
    let language = loader
        .load_language_at_path(config)
        .map_err(|e| format!("compiling grammar: {e}"))?;
    let highlights_query = std::fs::read_to_string(dest.join("queries/highlights.scm"))
        .map_err(|e| format!("reading queries/highlights.scm: {e}"))?;
    let injections_query =
        std::fs::read_to_string(dest.join("queries/injections.scm")).unwrap_or_default();
    Ok(TreesitLanguage { language, highlights_query, injections_query })
}

/// Per-buffer highlight state. No incremental parsing: `Highlighter`
/// (unlike a bare `Parser`) always reparses its input from scratch, so
/// there is nothing to gain from keeping an old `Tree` around — this just
/// tracks whether a reparse is owed since the last edit.
pub struct SyntaxState {
    pub language: String,
    config: HighlightConfiguration,
    /// (char_start, char_end, highlight name) — recomputed lazily.
    pub spans: Vec<(usize, usize, &'static str)>,
    dirty: bool,
}

impl std::fmt::Debug for SyntaxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxState").field("language", &self.language).finish()
    }
}

impl SyntaxState {
    pub fn new(name: &str, lang: &TreesitLanguage) -> Option<Self> {
        let mut config = HighlightConfiguration::new(
            lang.language.clone(),
            name,
            &lang.highlights_query,
            &lang.injections_query,
            "",
        )
        .ok()?;
        config.configure(HIGHLIGHT_NAMES);
        Some(SyntaxState { language: name.to_string(), config, spans: Vec::new(), dirty: true })
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Char range of the node at `char_pos` whose kind is in `kinds`: the
    /// innermost such node, or the outermost enclosing one when
    /// `outermost` (a top-level form in a lisp, where every nested list
    /// has the same kind). Parses on demand — queries like mark-defun are
    /// far too rare to justify keeping an incremental tree (see the
    /// struct doc above).
    pub fn node_range_at(
        &self,
        rope: &Rope,
        char_pos: usize,
        kinds: &[String],
        outermost: bool,
    ) -> Option<(usize, usize)> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&self.config.language).ok()?;
        let text = rope.to_string();
        let tree = parser.parse(&text, None)?;
        let byte = rope.char_to_byte(char_pos.min(rope.len_chars()));
        let mut node = tree.root_node().named_descendant_for_byte_range(byte, byte)?;
        let mut found = None;
        loop {
            if kinds.iter().any(|k| k == node.kind()) {
                found = Some((
                    rope.byte_to_char(node.start_byte()),
                    rope.byte_to_char(node.end_byte()),
                ));
                if !outermost {
                    break;
                }
            }
            match node.parent() {
                Some(parent) => node = parent,
                None => break,
            }
        }
        found
    }

    /// Recompute `spans` from `rope` if anything changed since the last
    /// call. Called once per visible buffer per render frame.
    pub fn ensure_current(&mut self, rope: &Rope) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.spans.clear();
        let text = rope.to_string();
        let mut highlighter = Highlighter::new();
        let Ok(events) = highlighter.highlight(&self.config, text.as_bytes(), None, |_| None)
        else {
            return;
        };
        let mut stack: Vec<usize> = Vec::new();
        for event in events {
            let Ok(event) = event else { break };
            match event {
                HighlightEvent::HighlightStart(h) => stack.push(h.0),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    if let Some(&i) = stack.last() {
                        let cs = rope.byte_to_char(start);
                        let ce = rope.byte_to_char(end);
                        self.spans.push((cs, ce, HIGHLIGHT_NAMES[i]));
                    }
                }
            }
        }
    }
}
