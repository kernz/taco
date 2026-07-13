//! The Editor: single owner of all native state. Scheme never sees this
//! struct — it only reaches it through the API functions in `scheme::api`,
//! each of which takes a short-lived borrow via the thread-local.

use crate::buffer::{Buffer, BufferId};
use crate::dispatch::Keymap;
use crate::keys::Chord;
use crate::killring::KillRing;
use crate::treesit::TreesitLanguage;
use crate::window::{Rect, WindowId, WindowTree};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use steel::rvals::SteelVal;

pub type NativeFn = fn(&mut Editor, Option<u32>);

/// Most candidate rows the minibuffer list may occupy (Vertico-style).
pub const MAX_COMPLETION_ROWS: usize = 10;

pub enum CommandFn {
    Native(NativeFn),
    Scheme(SteelVal),
}

pub struct Command {
    pub doc: String,
    pub f: CommandFn,
}

/// What the key loop must do after dispatch returned: Scheme closures are
/// invoked outside any Editor borrow (see scheme::api for the invariant).
pub enum PostAction {
    None,
    RunScheme(SteelVal, Vec<SteelVal>),
}

#[derive(Debug, Clone)]
pub enum YesNoAction {
    QuitModified,
    KillModifiedBuffer(BufferId),
    /// (y-or-n-p prompt on-yes): on-yes is called with no arguments iff the
    /// user answers y. This is the one generic escape hatch every mode
    /// (dired included) confirms destructive actions through — no
    /// mode-specific Rust variant needed.
    Generic(SteelVal),
}

#[derive(Debug, Clone)]
pub enum PromptKind {
    FindFile,
    SaveFileAs,
    SwitchBuffer,
    /// M-x: run any command in the registry by name.
    ExecuteCommand,
    GotoLine,
    QueryReplaceFrom,
    QueryReplaceTo { from: String },
    RectInsert,
    DescribeFunction,
    YesNo(YesNoAction),
    /// (read-string prompt initial completion on-submit): on-submit is
    /// called with the entered string. `completion` is
    /// "file"/"buffer"/"command"/None. The one generic prompt every mode
    /// reads input through.
    Generic { on_submit: SteelVal, completion: Option<String> },
}

#[derive(Debug)]
pub struct Prompt {
    pub kind: PromptKind,
    pub prompt: String,
    pub input: String,
    /// Editing position within `input`, in chars.
    pub cursor: usize,
    /// Candidate list shown vertically above the echo line, set from Scheme
    /// via (minibuffer-show-candidates lst idx). Empty = no list.
    pub completions: Vec<String>,
    pub selected: usize,
}

#[derive(Debug)]
pub struct IsearchState {
    pub query: String,
    pub forward: bool,
    /// Point when the search began (C-g returns here).
    pub origin: usize,
    pub current: Option<(usize, usize)>,
    pub wrapped: bool,
    pub failed: bool,
}

#[derive(Debug)]
pub struct QueryReplaceState {
    pub regex: String,
    pub replacement: String,
    pub current: Option<(usize, usize)>,
    pub count: usize,
}

pub enum InputMode {
    Normal,
    Prompt(Prompt),
    ISearch(IsearchState),
    QueryReplace(QueryReplaceState),
    /// C-h c/k — or any (read-key-sequence prompt f) — collecting a key
    /// sequence. `prompt` is echoed while collecting. When `on_done` is
    /// set, it is a Scheme continuation receiving the formatted sequence
    /// and the resolved command name ("" if undefined) via ed.deferred;
    /// when None, the native brief echo answers directly.
    DescribeKey {
        seq: Vec<Chord>,
        prompt: String,
        on_done: Option<SteelVal>,
    },
}

/// C-u state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixArg {
    None,
    /// C-u pressed, no digits yet (bare C-u = 4, per Emacs).
    Universal,
    Num(u32),
}

/// A basic ANSI color name, settable from Scheme via (set-face-color ...).
/// Kept independent of the terminal crate so editor.rs has no crossterm
/// dependency; render.rs maps this to crossterm::style::Color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
}

impl FaceColor {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "black" => Some(Self::Black),
            "red" => Some(Self::Red),
            "green" => Some(Self::Green),
            "yellow" => Some(Self::Yellow),
            "blue" => Some(Self::Blue),
            "magenta" => Some(Self::Magenta),
            "cyan" => Some(Self::Cyan),
            "white" => Some(Self::White),
            _ => None,
        }
    }
}

/// Scheme-settable background colors for the two highlighted UI elements:
/// the selected window's mode line, and highlighted text spans (region,
/// isearch/query-replace match). `None` keeps the original reverse-video
/// look. `line_number`/`line_number_current` are plain text colors for the
/// gutter digits only (Emacs' `line-number`/`line-number-current-line`
/// faces) — never applied to buffer text, and independent of `highlight`.
/// `syntax` is the open-ended set of tree-sitter capture-name colors (any
/// `(set-face-color name color)` whose name isn't one of the above).
#[derive(Debug, Default, Clone)]
pub struct Faces {
    pub mode_line: Option<FaceColor>,
    pub highlight: Option<FaceColor>,
    pub line_number: Option<FaceColor>,
    pub line_number_current: Option<FaceColor>,
    pub syntax: HashMap<String, FaceColor>,
}

pub struct Editor {
    pub buffers: HashMap<BufferId, Buffer>,
    next_buffer_id: BufferId,
    /// Most-recently-used buffer ids, front = current.
    pub recency: Vec<BufferId>,
    pub windows: WindowTree,
    pub kill_ring: KillRing,
    pub registry: HashMap<String, Command>,
    pub global_map: Keymap,
    /// Named buffer-local keymaps (dired-mode-map, ...), populated purely
    /// from Scheme via (define-key map-name seq cmd) — Rust knows nothing
    /// about which modes exist. Selected per-buffer via `Buffer.local_map`.
    pub keymaps: HashMap<String, Keymap>,
    /// Consulted (single chords only) before the built-in prompt editing
    /// while the minibuffer is active — how plugins bind C-n/RET/TAB there.
    pub minibuffer_map: Keymap,
    pub pending: Vec<Chord>,
    /// Actions produced by native fns called from inside the engine (e.g.
    /// (exit-minibuffer) submitting a Scheme command): the engine cannot be
    /// re-entered there, so the main loop drains this queue afterwards.
    pub deferred: Vec<PostAction>,
    pub prefix: PrefixArg,
    pub input: InputMode,
    pub echo: Option<String>,
    pub quit: bool,
    pub last_command: Option<String>,
    pub this_command: Option<String>,
    /// Goal column for consecutive C-n/C-p.
    pub goal_col: Option<usize>,
    /// Rectangle-mark-mode is active (anchor = buffer mark).
    pub rect_mode: bool,
    /// Scheme-settable option: draw a line-number gutter.
    pub show_line_numbers: bool,
    /// Scheme-settable face colors (set-face-color).
    pub faces: Faces,
    /// Grammars installed via (tree-sit-install-language-grammar name url),
    /// keyed by name. `Rc` since (tree-sit-enable name) hands each buffer
    /// its own `SyntaxState` built from the same query strings.
    pub treesit_languages: HashMap<String, Rc<TreesitLanguage>>,
    /// Set by `find_file_path` when it visits a real file (not a
    /// directory); consumed once by `process_chord`/main.rs to fire
    /// "find-file-hook" outside any Editor borrow.
    pub file_visited: bool,
    pub term_size: (u16, u16),
    /// Region inserted by the last yank, for M-y replacement.
    pub last_yank: Option<(usize, usize)>,
    /// Live left-button drag: window it started in and the anchor point
    /// there. A drag extends a region from the anchor; a click without
    /// movement leaves no region.
    pub mouse_drag: Option<(WindowId, usize)>,
    /// Live background processes (start-process), keyed by their Scheme-
    /// visible id. Drained by scheme::pump_processes from the main loop.
    pub processes: HashMap<u64, crate::process::ProcessHandle>,
    next_process_id: u64,
    /// The Scheme function to call (with a path string) when Rust needs to
    /// visit a directory (e.g. `find-file` on one) — set once by dired.scm
    /// via (register-directory-opener open-dired). Rust has no idea what
    /// "dired" is; this is the one hook that lets file-opening code hand
    /// off to whatever mode wants to own directories.
    pub directory_opener: Option<SteelVal>,
}

impl Editor {
    pub fn new() -> Self {
        let mut buffers = HashMap::new();
        let scratch = Buffer::new(0, "*scratch*", "");
        buffers.insert(0, scratch);
        Editor {
            buffers,
            next_buffer_id: 1,
            recency: vec![0],
            windows: WindowTree::new(0),
            kill_ring: KillRing::default(),
            registry: HashMap::new(),
            global_map: Keymap::default(),
            keymaps: HashMap::new(),
            minibuffer_map: Keymap::default(),
            pending: Vec::new(),
            deferred: Vec::new(),
            prefix: PrefixArg::None,
            input: InputMode::Normal,
            echo: None,
            quit: false,
            last_command: None,
            this_command: None,
            goal_col: None,
            rect_mode: false,
            show_line_numbers: false,
            faces: Faces::default(),
            treesit_languages: HashMap::new(),
            file_visited: false,
            term_size: (80, 24),
            last_yank: None,
            mouse_drag: None,
            processes: HashMap::new(),
            next_process_id: 1,
            directory_opener: None,
        }
    }

    // ---- buffer/window access -------------------------------------------

    /// Selected window and its buffer, borrowed together. Point is clamped,
    /// since another window may have shrunk the shared buffer.
    pub fn cur(&mut self) -> (&mut crate::window::Window, &mut Buffer) {
        let win = self.windows.selected_mut();
        let buf = self
            .buffers
            .get_mut(&win.buffer)
            .expect("window points at live buffer");
        win.point = win.point.min(buf.len_chars());
        (win, buf)
    }

    pub fn cur_buffer(&self) -> &Buffer {
        let win = self.windows.selected_ref();
        self.buffers.get(&win.buffer).expect("live buffer")
    }

    pub fn cur_buffer_mut(&mut self) -> &mut Buffer {
        let id = self.windows.selected_ref().buffer;
        self.buffers.get_mut(&id).expect("live buffer")
    }

    pub fn create_buffer(&mut self, name: impl Into<String>, text: &str) -> BufferId {
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        self.buffers.insert(id, Buffer::new(id, name, text));
        self.recency.push(id);
        id
    }

    pub fn add_buffer(&mut self, mut make: impl FnMut(BufferId) -> anyhow::Result<Buffer>) -> anyhow::Result<BufferId> {
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        let buf = make(id)?;
        self.buffers.insert(id, buf);
        self.recency.push(id);
        Ok(id)
    }

    pub fn buffer_by_name(&self, name: &str) -> Option<BufferId> {
        self.buffers
            .values()
            .find(|b| b.name == name)
            .map(|b| b.id)
    }

    pub fn alloc_process_id(&mut self) -> u64 {
        let id = self.next_process_id;
        self.next_process_id += 1;
        id
    }

    /// Append generated text to buffer `id` (see Buffer::append_generated)
    /// and make windows showing it follow the output: a window whose point
    /// sat at the old end of buffer keeps point at the end and scrolls so
    /// the tail stays visible; a window whose point was moved away by the
    /// user is left alone (Emacs' window-point-insertion-type behavior).
    /// Non-selected windows need the explicit scroll: render's
    /// ensure_point_visible only tracks the selected window. Returns the
    /// appended char range.
    pub fn append_to_buffer(&mut self, id: BufferId, text: &str) -> (usize, usize) {
        let (old_len, new_len, last_line) = {
            let Some(buf) = self.buffers.get_mut(&id) else { return (0, 0) };
            let (old, new) = buf.append_generated(text);
            (old, new, buf.char_to_line(new))
        };
        if old_len == new_len {
            return (old_len, new_len);
        }
        for (wid, rect) in self.windows.layout(self.window_area()) {
            let Some(win) = self.windows.find_mut(wid) else { continue };
            if win.buffer != id || win.point != old_len {
                continue;
            }
            win.point = new_len;
            let height = rect.text_height().max(1);
            if last_line >= win.top_line + height {
                win.top_line = last_line + 1 - height;
            }
        }
        (old_len, new_len)
    }

    pub fn buffer_by_path(&self, path: &std::path::Path) -> Option<BufferId> {
        self.buffers
            .values()
            .find(|b| b.path.as_deref() == Some(path))
            .map(|b| b.id)
    }

    /// Show `id` in the selected window and mark it most recent. The point
    /// in the outgoing buffer is remembered and restored on return.
    pub fn show_buffer(&mut self, id: BufferId) {
        let win = self.windows.selected_mut();
        let old = win.buffer;
        let old_point = win.point;
        if let Some(b) = self.buffers.get_mut(&old) {
            b.last_point = old_point;
        }
        win.buffer = id;
        win.point = self
            .buffers
            .get(&id)
            .map(|b| b.last_point.min(b.len_chars()))
            .unwrap_or(0);
        win.top_line = 0;
        self.touch_buffer(id);
    }

    pub fn touch_buffer(&mut self, id: BufferId) {
        self.recency.retain(|&b| b != id);
        self.recency.insert(0, id);
    }

    /// Remove a buffer; windows showing it fall back to another buffer
    /// (creating *scratch* if it was the last one).
    pub fn remove_buffer(&mut self, id: BufferId) {
        self.buffers.remove(&id);
        self.recency.retain(|&b| b != id);
        if self.buffers.is_empty() {
            let sid = self.next_buffer_id;
            self.next_buffer_id += 1;
            self.buffers.insert(sid, Buffer::new(sid, "*scratch*", ""));
            self.recency.push(sid);
        }
        let fallback = self.recency[0];
        for w in self.windows.windows_mut() {
            if w.buffer == id {
                w.buffer = fallback;
                w.point = 0;
                w.top_line = 0;
            }
        }
    }

    // ---- layout helpers ---------------------------------------------------

    /// Rows the minibuffer candidate list occupies above the echo line.
    pub fn completion_rows(&self) -> u16 {
        match &self.input {
            InputMode::Prompt(p) => p.completions.len().min(MAX_COMPLETION_ROWS) as u16,
            _ => 0,
        }
    }

    /// Terminal area available to windows (echo line and any minibuffer
    /// candidate rows excluded — the Vertico-style list grows upward).
    pub fn window_area(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            w: self.term_size.0,
            h: self
                .term_size
                .1
                .saturating_sub(1 + self.completion_rows()),
        }
    }

    pub fn window_rect(&self, id: WindowId) -> Rect {
        self.windows
            .layout(self.window_area())
            .into_iter()
            .find(|(wid, _)| *wid == id)
            .map(|(_, r)| r)
            .unwrap_or(Rect { x: 0, y: 0, w: 80, h: 24 })
    }

    pub fn selected_text_height(&self) -> usize {
        self.window_rect(self.windows.selected).text_height().max(1)
    }

    // ---- misc --------------------------------------------------------------

    pub fn message(&mut self, msg: impl Into<String>) {
        self.echo = Some(msg.into());
    }

    pub fn prompt(&mut self, kind: PromptKind, prompt: impl Into<String>) {
        self.prompt_prefilled(kind, prompt, String::new());
    }

    pub fn prompt_prefilled(
        &mut self,
        kind: PromptKind,
        prompt: impl Into<String>,
        input: impl Into<String>,
    ) {
        let input = input.into();
        self.input = InputMode::Prompt(Prompt {
            kind,
            prompt: prompt.into(),
            cursor: input.chars().count(),
            input,
            completions: Vec::new(),
            selected: 0,
        });
    }

    pub fn prefix_num(&self) -> Option<u32> {
        match self.prefix {
            PrefixArg::None => None,
            PrefixArg::Universal => Some(4),
            PrefixArg::Num(n) => Some(n),
        }
    }

    /// Directory to base relative prompts on: the current buffer's file or
    /// dired directory, else the process cwd.
    pub fn default_dir(&self) -> PathBuf {
        let buf = self.cur_buffer();
        if let Some(SteelVal::StringV(s)) = buf.locals.get("dired-directory") {
            return PathBuf::from(s.as_str());
        }
        if let Some(p) = &buf.path {
            if let Some(parent) = p.parent() {
                return parent.to_path_buf();
            }
        }
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    pub fn resolve_path(&self, input: &str) -> PathBuf {
        let p = PathBuf::from(shellexpand_home(input));
        if p.is_absolute() {
            p
        } else {
            self.default_dir().join(p)
        }
    }
}

fn shellexpand_home(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    s.to_string()
}
