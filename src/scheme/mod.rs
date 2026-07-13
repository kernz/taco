//! The interpreter layer. Scheme never touches editor memory or the
//! terminal: every capability is a Rust function registered here, each
//! taking a short-lived borrow of the thread-local Editor.
//!
//! Invariant: no registered function holds the Editor borrow across a call
//! back into the engine, and the dispatcher never invokes the engine while
//! holding a borrow (see `PostAction`).

mod fs;

use crate::commands::{self, files, movement};
use crate::editor::{Command, CommandFn, Editor, FaceColor, InputMode, PostAction, PromptKind, YesNoAction};
use crate::keys::parse_seq;
use crate::minibuffer;
use crate::treesit;
use std::cell::RefCell;
use steel::rvals::SteelVal;
use steel::steel_vm::engine::Engine;
use steel::steel_vm::register_fn::RegisterFn;

thread_local! {
    static EDITOR: RefCell<Editor> = RefCell::new(new_editor());
}

fn new_editor() -> Editor {
    let mut ed = Editor::new();
    commands::install(&mut ed);
    ed
}

/// Every access to editor state goes through here; the borrow lives only for
/// the closure.
pub fn with_editor<R>(f: impl FnOnce(&mut Editor) -> R) -> R {
    EDITOR.with(|e| f(&mut e.borrow_mut()))
}

/// Commands whose Scheme form takes arguments and is registered explicitly
/// below (their keybound form prompts interactively instead).
const EXPLICIT: &[&str] = &["find-file", "switch-to-buffer", "goto-line"];

pub fn build_engine() -> Engine {
    let mut engine = Engine::new();

    // 1. Every native command doubles as a zero-argument Scheme function, so
    //    config code can call (forward-char), (kill-line), (other-window)...
    for spec in commands::COMMANDS {
        if EXPLICIT.contains(&spec.name) {
            continue;
        }
        let f = spec.f;
        engine.register_fn(spec.name, move || {
            with_editor(|ed| {
                ed.cur_buffer_mut().undo.push_boundary();
                f(ed, None);
            })
        });
    }

    // 2. Functions that carry arguments — the rest of the contract.
    engine.register_fn("insert-text", |s: String| {
        with_editor(|ed| {
            if ed.cur_buffer().read_only {
                ed.message("Buffer is read-only");
                return;
            }
            let (win, buf) = ed.cur();
            buf.insert(win.point, &s);
            win.point += s.chars().count();
        })
    });
    // Emacs' (delete-region start end): unlike set-buffer-string! (which
    // regenerates non-user-authored content like a dired listing, with no
    // undo entries and `modified` left false — see Buffer::set_text), this
    // goes through Buffer::remove, so it's a normal, undoable, modifying
    // edit. Any mode doing surgical text edits (an indent command, a
    // comment toggle, ...) should use goto-char + delete-region! +
    // insert-text, never a whole-buffer set-buffer-string! rewrite.
    engine.register_fn("delete-region!", |start: isize, end: isize| {
        with_editor(|ed| {
            if ed.cur_buffer().read_only {
                ed.message("Buffer is read-only");
                return;
            }
            let (win, buf) = ed.cur();
            let len = buf.len_chars();
            let a = (start.max(0) as usize).min(len);
            let b = (end.max(0) as usize).min(len);
            let (a, b) = (a.min(b), a.max(b));
            buf.remove(a, b);
            if win.point > a {
                win.point = if win.point >= b { win.point - (b - a) } else { a };
            }
        })
    });
    engine.register_fn("point", || {
        with_editor(|ed| ed.windows.selected_ref().point as isize)
    });
    engine.register_fn("goto-char", |n: isize| {
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            win.point = (n.max(0) as usize).min(buf.len_chars());
        })
    });
    engine.register_fn("buffer-string", || {
        with_editor(|ed| ed.cur_buffer().to_string_lossless())
    });
    engine.register_fn("line-number", || {
        with_editor(|ed| {
            let point = ed.windows.selected_ref().point;
            ed.cur_buffer().char_to_line(point) as isize + 1
        })
    });
    // Narrow getters mirroring `point`/`line-number`: mark position/state
    // (set by set-mark-command) and the characters immediately after/before
    // point, so a mode (e.g. electric-pair-style bracket skipping) doesn't
    // have to copy the whole buffer through `buffer-string` to peek one char.
    engine.register_fn("mark", || {
        with_editor(|ed| ed.cur_buffer().mark.map(|m| m as isize).unwrap_or(-1))
    });
    engine.register_fn("mark-active?", || {
        with_editor(|ed| ed.cur_buffer().mark_active)
    });
    // The writing half of the mark API (Emacs' push-mark/deactivate-mark):
    // what lets a Scheme command like mark-defun establish a region.
    engine.register_fn("set-mark", |pos: isize| {
        with_editor(|ed| {
            let len = ed.cur_buffer().len_chars();
            let buf = ed.cur_buffer_mut();
            buf.mark = Some((pos.max(0) as usize).min(len));
            buf.mark_active = true;
        })
    });
    engine.register_fn("deactivate-mark", || {
        with_editor(|ed| ed.cur_buffer_mut().mark_active = false)
    });
    engine.register_fn("char-after", || {
        with_editor(|ed| {
            let point = ed.windows.selected_ref().point;
            let buf = ed.cur_buffer();
            if point < buf.len_chars() {
                buf.rope.char(point).to_string()
            } else {
                String::new()
            }
        })
    });
    engine.register_fn("char-before", || {
        with_editor(|ed| {
            let point = ed.windows.selected_ref().point;
            let buf = ed.cur_buffer();
            if point > 0 && point <= buf.len_chars() {
                buf.rope.char(point - 1).to_string()
            } else {
                String::new()
            }
        })
    });
    engine.register_fn("current-buffer", || {
        with_editor(|ed| ed.cur_buffer().name.clone())
    });
    engine.register_fn("buffer-modified?", || {
        with_editor(|ed| ed.cur_buffer().modified)
    });
    engine.register_fn("buffer-list", || {
        with_editor(|ed| {
            ed.buffers.values().map(|b| b.name.clone()).collect::<Vec<_>>()
        })
    });
    engine.register_fn("find-file", |path: String| {
        with_editor(|ed| {
            let p = ed.resolve_path(&path);
            files::find_file_path(ed, &p);
        })
    });
    engine.register_fn("switch-to-buffer", |name: String| {
        with_editor(|ed| files::switch_to_buffer_name(ed, &name))
    });
    engine.register_fn("goto-line", |n: isize| {
        with_editor(|ed| movement::goto_line(ed, n.max(1) as usize))
    });
    engine.register_fn("message", |s: String| {
        with_editor(|ed| ed.message(s))
    });

    // The minibuffer contract (Emacs names where they exist). Completion
    // UIs are plugins: they observe the prompt through the hooks fired by
    // the main loop, rebind keys with (minibuffer-set-key ...) and drive
    // the native candidate list with (minibuffer-show-candidates ...).
    engine.register_fn("minibufferp", || {
        with_editor(|ed| matches!(ed.input, InputMode::Prompt(_)))
    });
    engine.register_fn("minibuffer-prompt", || {
        with_editor(|ed| match &ed.input {
            InputMode::Prompt(p) => p.prompt.clone(),
            _ => String::new(),
        })
    });
    engine.register_fn("minibuffer-contents", || {
        with_editor(|ed| match &ed.input {
            InputMode::Prompt(p) => p.input.clone(),
            _ => String::new(),
        })
    });
    engine.register_fn("set-minibuffer-contents", |s: String| {
        with_editor(|ed| {
            if let InputMode::Prompt(p) = &mut ed.input {
                p.cursor = s.chars().count();
                p.input = s;
            }
        })
    });
    engine.register_fn("delete-minibuffer-contents", || {
        with_editor(|ed| {
            if let InputMode::Prompt(p) = &mut ed.input {
                p.input.clear();
                p.cursor = 0;
            }
        })
    });
    engine.register_fn("minibuffer-completion-kind", || {
        with_editor(|ed| match &ed.input {
            InputMode::Prompt(p) => {
                minibuffer::completion_kind(&p.kind).unwrap_or("").to_string()
            }
            _ => String::new(),
        })
    });
    // Submit the prompt as RET would. Runs inside the engine, so a Scheme
    // command produced by the submission is queued on ed.deferred for the
    // main loop (the engine cannot re-enter itself).
    engine.register_fn("exit-minibuffer", || {
        with_editor(|ed| {
            let action = minibuffer::submit(ed);
            if !matches!(action, PostAction::None) {
                ed.deferred.push(action);
            }
        })
    });
    engine.register_fn("minibuffer-show-candidates", |lst: Vec<String>, idx: isize| {
        with_editor(|ed| {
            if let InputMode::Prompt(p) = &mut ed.input {
                p.selected = (idx.max(0) as usize).min(lst.len().saturating_sub(1));
                p.completions = lst;
            }
        })
    });
    engine.register_fn("default-directory", || {
        with_editor(|ed| ed.default_dir().display().to_string())
    });
    // Text-area width of the selected window in columns (Emacs'
    // window-width, minus the line-number gutter when it's showing) — how
    // man.scm sizes MANWIDTH, and generally the only sane wrap target a
    // Scheme formatter has.
    engine.register_fn("window-width", || {
        with_editor(|ed| {
            let rect = ed.window_rect(ed.windows.selected);
            let gutter = crate::render::gutter_width(ed, ed.cur_buffer(), &rect);
            (rect.w as usize).saturating_sub(gutter) as isize
        })
    });

    // Help introspection: the registry doc string of a command, every key
    // sequence bound to it (global map plus all named buffer-local maps),
    // and the current buffer's buffer-local variable names. help.scm
    // builds all of C-h on these.
    engine.register_fn("command-doc", |name: String| {
        with_editor(|ed| {
            ed.registry.get(&name).map(|c| c.doc.clone()).unwrap_or_default()
        })
    });
    // -> list of (seq map-name) pairs; map-name "" for the global map.
    engine.register_fn("command-bindings", |name: String| -> Vec<SteelVal> {
        with_editor(|ed| {
            let pair = |seq: String, map: &str| {
                SteelVal::ListV(
                    vec![
                        SteelVal::StringV(seq.into()),
                        SteelVal::StringV(map.to_string().into()),
                    ]
                    .into_iter()
                    .collect(),
                )
            };
            let mut out: Vec<SteelVal> = ed
                .global_map
                .bindings_of(&name)
                .into_iter()
                .map(|s| pair(s, ""))
                .collect();
            let mut maps: Vec<_> = ed.keymaps.iter().collect();
            maps.sort_by(|a, b| a.0.cmp(b.0));
            for (map_name, map) in maps {
                for seq in map.bindings_of(&name) {
                    out.push(pair(seq, map_name));
                }
            }
            out
        })
    });
    engine.register_fn("buffer-local-keys", || {
        with_editor(|ed| {
            let mut keys: Vec<String> = ed.cur_buffer().locals.keys().cloned().collect();
            keys.sort();
            keys
        })
    });
    // Read one full key sequence interactively (the C-h c/k mechanic,
    // reusing the DescribeKey input mode): `f` gets the formatted sequence
    // and the name of the command it resolves to ("" when undefined).
    engine.register_fn("read-key-sequence", |prompt: String, f: SteelVal| {
        with_editor(|ed| {
            ed.message(prompt.clone());
            ed.input = InputMode::DescribeKey {
                seq: Vec::new(),
                prompt,
                on_done: Some(f),
            };
        })
    });

    // Candidate sources for Scheme-side completion plugins.
    engine.register_fn("command-names", || {
        with_editor(|ed| {
            let mut names: Vec<String> = ed.registry.keys().cloned().collect();
            names.sort();
            names
        })
    });
    engine.register_fn("buffer-names", || {
        with_editor(|ed| {
            let mut names: Vec<String> =
                ed.buffers.values().map(|b| b.name.clone()).collect();
            names.sort();
            names
        })
    });
    engine.register_fn("directory-files", |dir: String| {
        let path = with_editor(|ed| ed.resolve_path(&dir));
        let mut names: Vec<String> = match std::fs::read_dir(&path) {
            Ok(rd) => rd
                .flatten()
                .map(|e| {
                    let mut n = e.file_name().to_string_lossy().into_owned();
                    if e.path().is_dir() {
                        n.push('/');
                    }
                    n
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    });

    // Editor options (Emacs customize-style). Booleans accept #t/#f or ints.
    engine.register_fn("set-option", |name: String, value: SteelVal| {
        with_editor(|ed| set_option(ed, &name, &value))
    });
    engine.register_fn("get-option", |name: String| {
        with_editor(|ed| match name.as_str() {
            "display-line-numbers" => ed.show_line_numbers,
            _ => false,
        })
    });

    // Appearance: background color of the mode line and of highlighted
    // spans (region, isearch/query-replace match), and the text color of
    // the gutter's line numbers — "line-number" for ordinary lines,
    // "line-number-current-line" for the line holding point (Emacs' own
    // face names). Unset faces keep the original look (reverse-video for
    // mode-line/highlight, dim/reverse for the line numbers).
    engine.register_fn("set-face-color", |face: String, color: String| {
        with_editor(|ed| set_face_color(ed, &face, &color))
    });

    // 3. Keymap and command definition — how Scheme extends the editor.
    // Every mode (dired included) is just a name in `ed.keymaps`; Rust
    // knows nothing beyond "global" as a distinguished always-on map.
    engine.register_fn("global-set-key", |seq: String, cmd: String| {
        with_editor(|ed| bind_in(ed, None, &seq, &cmd))
    });
    engine.register_fn("define-key", |map: String, seq: String, cmd: String| {
        with_editor(|ed| bind_in(ed, Some(map), &seq, &cmd))
    });
    engine.register_fn("minibuffer-set-key", |seq: String, cmd: String| {
        with_editor(|ed| {
            let Some(chords) = parse_seq(&seq) else {
                ed.message(format!("minibuffer-set-key: cannot parse {seq:?}"));
                return;
            };
            if chords.len() > 1 {
                ed.message(format!(
                    "minibuffer-set-key: only single keys are supported, got {seq:?}"
                ));
                return;
            }
            ed.minibuffer_map.bind(&chords, &cmd);
        })
    });
    engine.register_fn(
        "define-command",
        |name: String, doc: String, f: SteelVal| {
            with_editor(|ed| {
                ed.registry
                    .insert(name, Command { doc, f: CommandFn::Scheme(f) });
            })
        },
    );

    // 4. Buffer-local mode state — what makes a "mode" possible without any
    // Rust-side knowledge of what modes exist (see buffer.rs doc comment).
    engine.register_fn("set-buffer-mode-name", |name: String| {
        with_editor(|ed| ed.cur_buffer_mut().mode_name = name)
    });
    engine.register_fn("use-local-map", |name: String| {
        with_editor(|ed| ed.cur_buffer_mut().local_map = Some(name))
    });
    engine.register_fn("buffer-local-set!", |key: String, value: SteelVal| {
        with_editor(|ed| {
            ed.cur_buffer_mut().locals.insert(key, value);
        })
    });
    engine.register_fn("buffer-local-get", |key: String| {
        with_editor(|ed| {
            ed.cur_buffer()
                .locals
                .get(&key)
                .cloned()
                .unwrap_or(SteelVal::BoolV(false))
        })
    });
    engine.register_fn("buffer-local-get-in", |name: String, key: String| {
        with_editor(|ed| {
            ed.buffers
                .values()
                .find(|b| b.name == name)
                .and_then(|b| b.locals.get(&key).cloned())
                .unwrap_or(SteelVal::BoolV(false))
        })
    });

    // 5. Generic prompt/confirm continuations — the one escape hatch every
    // mode reads interactive input through, instead of a dedicated Rust
    // PromptKind variant per feature.
    engine.register_fn(
        "read-string",
        |prompt: String, initial: String, completion: String, on_submit: SteelVal| {
            with_editor(|ed| {
                let completion = if completion.is_empty() { None } else { Some(completion) };
                ed.prompt_prefilled(PromptKind::Generic { on_submit, completion }, prompt, initial);
            })
        },
    );
    engine.register_fn("y-or-n-p", |prompt: String, on_yes: SteelVal| {
        with_editor(|ed| ed.prompt(PromptKind::YesNo(YesNoAction::Generic(on_yes)), prompt));
    });

    // 6. Tree-sitter — real dynamic grammar installation (git clone,
    // compile with the system C compiler, load at runtime), mirroring
    // Emacs' treesit-install-language-grammar. Mechanism only: which file
    // extension uses which language is entirely Scheme's call
    // (tree-sit-enable-for-extension, bootstrap.scm) — Rust has no idea
    // "rust" or "python" exist, only that some name maps to a compiled
    // grammar. Returns "" on success, an error message otherwise.
    engine.register_fn(
        "tree-sit-install-language-grammar",
        |name: String, url: String| match treesit::install_language_grammar(&name, &url) {
            Ok((lang, fresh)) => {
                with_editor(|ed| {
                    // Announce only an actual first-time install; reloading
                    // the cached grammar on every later launch is routine.
                    if fresh {
                        ed.message(format!("Installed tree-sitter grammar {name:?}"));
                    }
                    ed.treesit_languages.insert(name, std::rc::Rc::new(lang));
                });
                String::new()
            }
            Err(e) => {
                with_editor(|ed| ed.message(format!("tree-sit-install-language-grammar: {e}")));
                e
            }
        },
    );
    engine.register_fn("tree-sit-enable", |name: String| {
        with_editor(|ed| {
            let Some(lang) = ed.treesit_languages.get(&name).cloned() else {
                let msg = format!(
                    "tree-sit-enable: {name:?} is not installed (call tree-sit-install-language-grammar first)"
                );
                ed.message(msg.clone());
                return msg;
            };
            let already = ed.cur_buffer().syntax.as_ref().map(|s| s.language.as_str())
                == Some(name.as_str());
            if already {
                return String::new();
            }
            match treesit::SyntaxState::new(&name, &lang) {
                Some(state) => {
                    ed.cur_buffer_mut().syntax = Some(state);
                    String::new()
                }
                None => {
                    let msg = format!("tree-sit-enable: could not build a highlighter for {name:?}");
                    ed.message(msg.clone());
                    msg
                }
            }
        })
    });

    // Node-level query against the current buffer's tree-sitter grammar:
    // the char range of the innermost (or outermost, for lisps where every
    // nested form shares one node kind) node at point whose kind is in
    // `kinds` — how Scheme implements mark-defun without Rust knowing what
    // a "defun" is per language. (-1 -1) when there is no tree-sitter
    // state or no matching node.
    engine.register_fn(
        "tree-sit-node-range-at-point",
        |kinds: Vec<String>, outermost: bool| -> Vec<isize> {
            with_editor(|ed| {
                let point = ed.windows.selected_ref().point;
                let buf = ed.cur_buffer();
                buf.syntax
                    .as_ref()
                    .and_then(|s| s.node_range_at(&buf.rope, point, &kinds, outermost))
                    .map(|(s, e)| vec![s as isize, e as isize])
                    .unwrap_or_else(|| vec![-1, -1])
            })
        },
    );

    fs::register(&mut engine);
    crate::process::register(&mut engine);

    engine
}

/// Drain output/exit events from background processes: append each
/// process's pending chunks to its buffer, queue its Scheme callbacks on
/// ed.deferred, then run them with no Editor borrow held (the audited
/// drain — see main.rs). Returns true when anything happened, so the main
/// loop knows to redraw; a quiet poll tick redraws nothing.
pub fn pump_processes(engine: &mut Engine) -> bool {
    use std::sync::mpsc::TryRecvError;
    let happened = with_editor(|ed| {
        let ids: Vec<u64> = ed.processes.keys().copied().collect();
        let mut happened = false;
        for id in ids {
            let handle = ed.processes.get_mut(&id).expect("live process");
            let discard = handle.discard_output;
            let mut chunk = String::new();
            loop {
                match handle.rx.try_recv() {
                    Ok(s) => chunk.push_str(&s),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        handle.streams_closed = true;
                        break;
                    }
                }
            }
            if discard {
                chunk.clear(); // killed process: late output is dropped
            }
            if !chunk.is_empty() {
                happened = true;
                let (buffer, on_output) = {
                    let h = ed.processes.get(&id).expect("live process");
                    (h.buffer, h.on_output.clone())
                };
                let (start, end) = ed.append_to_buffer(buffer, &chunk);
                if let Some(f) = on_output {
                    ed.deferred.push(PostAction::RunScheme(
                        f,
                        vec![
                            SteelVal::StringV(chunk.into()),
                            SteelVal::IntV(start as isize),
                            SteelVal::IntV(end as isize),
                        ],
                    ));
                }
            }
            // Reap only after both pipes hung up, and only with try_wait —
            // a blocking wait here would stall the whole editor.
            let exit_code = {
                let h = ed.processes.get_mut(&id).expect("live process");
                if h.streams_closed {
                    match h.child.try_wait() {
                        Ok(Some(status)) => Some(status.code().unwrap_or(-1)),
                        Ok(None) => None, // pipes closed but still running
                        Err(_) => Some(-1),
                    }
                } else {
                    None
                }
            };
            if let Some(code) = exit_code {
                happened = true;
                let h = ed.processes.remove(&id).expect("live process");
                if let Some(f) = h.on_exit {
                    ed.deferred
                        .push(PostAction::RunScheme(f, vec![SteelVal::IntV(code as isize)]));
                }
            }
        }
        happened
    });
    if happened {
        drain_deferred(engine);
    }
    happened
}

fn set_option(ed: &mut Editor, name: &str, value: &SteelVal) {
    let as_bool = match value {
        SteelVal::BoolV(b) => Some(*b),
        SteelVal::IntV(i) => Some(*i != 0),
        _ => None,
    };
    match (name, as_bool) {
        ("display-line-numbers", Some(b)) => ed.show_line_numbers = b,
        ("display-line-numbers", None) => {
            ed.message("set-option: display-line-numbers takes a boolean")
        }
        _ => ed.message(format!("set-option: unknown option {name:?}")),
    }
}

fn set_face_color(ed: &mut Editor, face: &str, color: &str) {
    let Some(c) = FaceColor::parse(color) else {
        ed.message(format!("set-face-color: unknown color {color:?}"));
        return;
    };
    match face {
        "mode-line" => ed.faces.mode_line = Some(c),
        "highlight" => ed.faces.highlight = Some(c),
        "line-number" => ed.faces.line_number = Some(c),
        "line-number-current-line" => ed.faces.line_number_current = Some(c),
        // Anything else is treated as a tree-sitter capture name
        // ("keyword", "string", "comment", ...) — open-ended, so no
        // per-name Rust code is needed as new grammars get installed.
        _ => {
            ed.faces.syntax.insert(face.to_string(), c);
        }
    }
}

/// `map = None` targets the global map; `Some(name)` creates/targets a
/// named buffer-local map in `ed.keymaps` (dired-mode-map, ...).
fn bind_in(ed: &mut Editor, map: Option<String>, seq: &str, cmd: &str) {
    let Some(chords) = parse_seq(seq) else {
        ed.message(format!("set-key: cannot parse key sequence {seq:?}"));
        return;
    };
    if chords.is_empty() {
        ed.message(format!("set-key: empty key sequence {seq:?}"));
        return;
    }
    let target = match map {
        None => &mut ed.global_map,
        Some(name) => ed.keymaps.entry(name).or_default(),
    };
    target.bind(&chords, cmd);
}

/// The default keybindings, expressed through the same public contract that
/// user config uses (Emacs' C-core / Lisp-layer split).
pub fn load_bootstrap(engine: &mut Engine) {
    run(engine, include_str!("bootstrap.scm"), "bootstrap.scm");
}

/// dired, entirely in Steel (see src/scheme/dired.scm) — spec-mandated core
/// behavior, so it ships in the binary like bootstrap.scm rather than living
/// in examples/ as an opt-in plugin.
/// compile.scm loads before dired.scm: dired's A command reuses the
/// results-* machinery (results-parse-current-buffer!, compilation-mode-map).
pub fn load_compile(engine: &mut Engine) {
    run(engine, include_str!("compile.scm"), "compile.scm");
}

/// help.scm loads after compile.scm: its *Help* buffer q binding reuses
/// compile.scm's quit-window command.
pub fn load_help(engine: &mut Engine) {
    run(engine, include_str!("help.scm"), "help.scm");
}

/// man.scm loads after compile.scm for the same reason help.scm does: its
/// *Man* buffer q binding reuses compile.scm's quit-window command.
pub fn load_man(engine: &mut Engine) {
    run(engine, include_str!("man.scm"), "man.scm");
}

pub fn load_dired(engine: &mut Engine) {
    run(engine, include_str!("dired.scm"), "dired.scm");
}

/// rust-mode, entirely in Steel (see src/scheme/rust-mode.scm) — built in
/// like dired.scm rather than living in examples/, since it should be on by
/// default for .rs files rather than something the user has to opt into.
pub fn load_python_mode(engine: &mut Engine) {
    run(engine, include_str!("python-mode.scm"), "python-mode.scm");
}

pub fn load_c_mode(engine: &mut Engine) {
    run(engine, include_str!("c-mode.scm"), "c-mode.scm");
}

pub fn load_scheme_mode(engine: &mut Engine) {
    run(engine, include_str!("scheme-mode.scm"), "scheme-mode.scm");
}

pub fn load_rust_mode(engine: &mut Engine) {
    run(engine, include_str!("rust-mode.scm"), "rust-mode.scm");
}

/// ~/.config/taco/init.scm, if present.
pub fn load_init(engine: &mut Engine) {
    let Some(path) = dirs::config_dir().map(|d| d.join("taco/init.scm")) else {
        return;
    };
    if let Ok(src) = std::fs::read_to_string(&path) {
        run(engine, &src, &path.display().to_string());
    }
}

fn run(engine: &mut Engine, src: &str, what: &str) {
    if let Err(e) = engine.compile_and_run_raw_program(src.to_string()) {
        with_editor(|ed| ed.message(format!("Error in {what}: {e}")));
    }
}

/// Invoke a Scheme command closure. Called with NO Editor borrow held.
pub fn call_scheme_command(engine: &mut Engine, f: SteelVal, args: Vec<SteelVal>) {
    if let Err(e) = engine.call_function_with_args(f, args) {
        with_editor(|ed| ed.message(format!("Scheme error: {e}")));
    }
}

/// Handle one normalized chord exactly as the main loop does: dispatch it,
/// run the resulting Scheme action, drain deferred work, then fire the
/// minibuffer lifecycle hooks for the prompt transition this event caused
/// (hook-triggered transitions do not re-fire). Never called with an Editor
/// borrow held.
pub fn process_chord(engine: &mut Engine, chord: crate::keys::Chord) {
    let prompt_active =
        || with_editor(|ed| matches!(ed.input, InputMode::Prompt(_)));
    let was_prompt = prompt_active();
    let action = with_editor(|ed| crate::dispatch::handle_chord(ed, chord));
    match action {
        PostAction::None => {}
        PostAction::RunScheme(f, args) => call_scheme_command(engine, f, args),
    }
    drain_deferred(engine);
    match (was_prompt, prompt_active()) {
        (false, true) => call_run_hooks(engine, "minibuffer-setup-hook"),
        (true, true) => call_run_hooks(engine, "post-command-hook"),
        (true, false) => call_run_hooks(engine, "minibuffer-exit-hook"),
        (false, false) => {}
    }
    drain_deferred(engine);
    fire_find_file_hook(engine);
}

/// Fire "find-file-hook" once for every real file `find_file_path` just
/// visited this event (directories go through `directory_opener` instead —
/// see `Editor.file_visited`'s doc comment). This is the generic
/// extension-dispatch point: `tree-sit-enable-for-extension` (bootstrap.scm)
/// hangs off it, but so can any future mode.
pub fn fire_find_file_hook(engine: &mut Engine) {
    if with_editor(|ed| std::mem::take(&mut ed.file_visited)) {
        call_run_hooks(engine, "find-file-hook");
        drain_deferred(engine);
    }
}

/// Fire a named hook: calls the Scheme function (run-hooks name), defined
/// in bootstrap.scm. Called with NO Editor borrow held. Silently a no-op if
/// run-hooks does not exist (engine built without bootstrap).
pub fn call_run_hooks(engine: &mut Engine, hook: &str) {
    let Ok(f) = engine.extract_value("run-hooks") else {
        return;
    };
    let args = vec![SteelVal::StringV(hook.to_string().into())];
    if let Err(e) = engine.call_function_with_args(f, args) {
        with_editor(|ed| ed.message(format!("Hook error ({hook}): {e}")));
    }
}

/// Run everything native fns queued on ed.deferred while the engine was
/// active (see Editor::deferred). Draining may queue more work — a Scheme
/// command that exits another prompt — so loop, with a cap to stay safe
/// against a pathological plugin cycle.
pub fn drain_deferred(engine: &mut Engine) {
    for _ in 0..100 {
        let next = with_editor(|ed| {
            if ed.deferred.is_empty() {
                None
            } else {
                Some(ed.deferred.remove(0))
            }
        });
        match next {
            None => return,
            Some(PostAction::None) => {}
            Some(PostAction::RunScheme(f, args)) => call_scheme_command(engine, f, args),
        }
    }
    with_editor(|ed| ed.message("Deferred action limit reached (plugin loop?)"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{self, Lookup};
    use crate::editor::PostAction;
    use crate::keys::parse_seq;

    /// A bootstrap error is only reported on the echo line; fail loudly
    /// here instead so a bad bootstrap.scm cannot slip through.
    #[test]
    fn bootstrap_loads_cleanly() {
        let mut engine = build_engine();
        engine
            .compile_and_run_raw_program(include_str!("bootstrap.scm").to_string())
            .unwrap();
    }

    /// Same guard for compile.scm (in its real load position, right after
    /// bootstrap).
    #[test]
    fn compile_loads_cleanly() {
        let mut engine = build_engine();
        engine
            .compile_and_run_raw_program(include_str!("bootstrap.scm").to_string())
            .unwrap();
        engine
            .compile_and_run_raw_program(include_str!("compile.scm").to_string())
            .unwrap();
    }

    /// Same guard for help.scm (real load position: after compile.scm,
    /// whose quit-window it reuses).
    #[test]
    fn help_loads_cleanly() {
        let mut engine = build_engine();
        for src in [
            include_str!("bootstrap.scm"),
            include_str!("compile.scm"),
            include_str!("help.scm"),
        ] {
            engine.compile_and_run_raw_program(src.to_string()).unwrap();
        }
    }

    /// The C-h help system, driven through the real key path: C-h c
    /// echoes the command name, C-h k / C-h x / C-h a / C-h v fill a
    /// *Help* buffer, undefined keys say so, C-g cancels.
    #[test]
    fn help_end_to_end() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);
        load_help(&mut engine);

        let feed = |engine: &mut Engine, keys: &str| {
            for chord in parse_seq(keys).unwrap() {
                process_chord(engine, chord);
            }
        };
        let type_text = |engine: &mut Engine, text: &str| {
            for c in text.chars() {
                process_chord(engine, crate::keys::Chord {
                    ctrl: false,
                    meta: false,
                    key: crate::keys::Key::Char(c),
                });
            }
        };
        let help_text = || {
            with_editor(|ed| {
                let id = ed.buffer_by_name("*Help*").expect("*Help* buffer exists");
                ed.buffers[&id].to_string_lossless()
            })
        };

        // C-h c C-p: brief echo, exactly the tutorial's wording.
        feed(&mut engine, "C-h c C-p");
        with_editor(|ed| {
            assert_eq!(
                ed.echo.as_deref(),
                Some("C-p runs the command previous-line")
            );
        });

        // C-h c on an unbound key.
        feed(&mut engine, "C-h c C-M-y");
        with_editor(|ed| {
            assert_eq!(ed.echo.as_deref(), Some("C-M-y is undefined"));
        });

        // C-g cancels a pending C-h capture.
        feed(&mut engine, "C-h c C-g");
        with_editor(|ed| {
            assert!(matches!(ed.input, InputMode::Normal));
            assert_eq!(ed.echo.as_deref(), Some("Quit"));
        });

        // C-h k C-n: full documentation in *Help*; we stay in our window.
        let before = with_editor(|ed| ed.cur_buffer().name.clone());
        feed(&mut engine, "C-h k C-n");
        let text = help_text();
        assert!(text.contains("C-n runs the command next-line"), "{text}");
        assert!(text.contains("It is bound to: C-n"), "{text}");
        assert!(text.contains("Documentation:"), "{text}");
        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().name, before, "help stole the window");
            assert!(ed.buffers.values().find(|b| b.name == "*Help*").unwrap().read_only);
        });

        // C-h x save-buffer RET: describe a command by name.
        feed(&mut engine, "C-h x");
        with_editor(|ed| assert!(matches!(ed.input, InputMode::Prompt(_))));
        type_text(&mut engine, "save-buffer");
        feed(&mut engine, "RET");
        let text = help_text();
        assert!(text.contains("save-buffer is an interactive command"), "{text}");
        assert!(text.contains("C-x C-s"), "binding column missing: {text}");

        // C-h a file RET: apropos lists matching commands with bindings.
        feed(&mut engine, "C-h a");
        type_text(&mut engine, "file");
        feed(&mut engine, "RET");
        let text = help_text();
        assert!(text.contains("find-file"), "{text}");
        assert!(text.contains("C-x C-f"), "{text}");

        // C-h v on a buffer-local variable.
        engine
            .compile_and_run_raw_program(
                r#"(buffer-local-set! "comment-start" "// ")"#.to_string(),
            )
            .unwrap();
        feed(&mut engine, "C-h v");
        type_text(&mut engine, "comment-start");
        feed(&mut engine, "RET");
        let text = help_text();
        assert!(text.contains("comment-start is a buffer-local variable"), "{text}");
        assert!(text.contains("\"// \""), "{text}");

        // C-h ?: the overview.
        feed(&mut engine, "C-h ?");
        let text = help_text();
        assert!(text.contains("You have typed C-h, the help character"), "{text}");
    }

    /// Runtime evaluation, through the real key path: M-: echoes the value
    /// and mutates the global environment, C-x C-e picks the right sexp
    /// (nesting, strings with parens, quote prefixes), eval-buffer runs a
    /// whole buffer, load-file loads from disk — and a built-in
    /// Scheme-level command can be redefined live, the Emacs property the
    /// whole feature exists for.
    #[test]
    fn eval_commands_end_to_end() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        let feed = |engine: &mut Engine, keys: &str| {
            for chord in parse_seq(keys).unwrap() {
                process_chord(engine, chord);
            }
        };
        let type_text = |engine: &mut Engine, text: &str| {
            for c in text.chars() {
                process_chord(engine, crate::keys::Chord {
                    ctrl: false,
                    meta: false,
                    key: crate::keys::Key::Char(c),
                });
            }
        };
        let check = |engine: &mut Engine, src: &str| {
            let result = engine.compile_and_run_raw_program(src.to_string()).unwrap();
            assert_eq!(format!("{:?}", result.last().unwrap()), "#true", "failed: {src}");
        };

        // M-: (+ 1 2) echoes 3.
        feed(&mut engine, "M-:");
        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else { panic!("M-: no prompt") };
            assert_eq!(p.prompt, "Eval: ");
        });
        type_text(&mut engine, "(+ 1 2)");
        feed(&mut engine, "RET");
        with_editor(|ed| assert_eq!(ed.echo.as_deref(), Some("3")));

        // M-: (define ...) mutates the live global environment.
        feed(&mut engine, "M-:");
        type_text(&mut engine, "(define taco-eval-var 31)");
        feed(&mut engine, "RET");
        check(&mut engine, "(equal? taco-eval-var 31)");

        // C-x C-e on the sexp before point: nested position, a string
        // containing parens, and a quote prefix.
        let set_buffer = |text: &str| {
            with_editor(|ed| {
                let (win, buf) = ed.cur();
                buf.set_text(text);
                win.point = buf.len_chars();
            })
        };
        set_buffer("(list (* 6 7))");
        with_editor(|ed| ed.windows.selected_mut().point = 13); // after inner )
        feed(&mut engine, "C-x C-e");
        with_editor(|ed| assert_eq!(ed.echo.as_deref(), Some("42")));

        set_buffer("(string-append \"a)b\" \"c\")  ");
        feed(&mut engine, "C-x C-e");
        with_editor(|ed| assert_eq!(ed.echo.as_deref(), Some("a)bc")));

        set_buffer("'(1 2)");
        feed(&mut engine, "C-x C-e");
        with_editor(|ed| assert_eq!(ed.echo.as_deref(), Some("(1 2)")));

        set_buffer("no sexp here (");
        feed(&mut engine, "C-x C-e");
        with_editor(|ed| {
            // "(" opened and never closed: the atom before it is not what
            // ends at point, so nothing qualifies.
            assert_eq!(ed.echo.as_deref(), Some("No s-expression before point"));
        });

        // eval-buffer: several top-level forms, later ones see earlier ones.
        set_buffer("(define eb-a 5)\n(define eb-b (+ eb-a 2))\n");
        engine.compile_and_run_raw_program("(eval-buffer)".to_string()).unwrap();
        check(&mut engine, "(equal? eb-b 7)");

        // load-file through its prompt.
        let path = std::env::temp_dir().join(format!("taco-load-{}.scm", std::process::id()));
        std::fs::write(&path, "(define lf-var 99)").unwrap();
        feed(&mut engine, "M-x");
        type_text(&mut engine, "load-file");
        feed(&mut engine, "RET");
        type_text(&mut engine, &path.display().to_string());
        feed(&mut engine, "RET");
        check(&mut engine, "(equal? lf-var 99)");
        with_editor(|ed| {
            assert!(ed.echo.as_deref().unwrap_or("").starts_with("Loaded"), "{:?}", ed.echo);
        });
        std::fs::remove_file(&path).ok();

        // The point of it all: redefine a built-in Scheme command live and
        // the very next keystroke runs the new definition.
        feed(&mut engine, "M-:");
        type_text(
            &mut engine,
            r#"(define-command "eval-buffer" "hijacked" (lambda () (message "hijacked!")))"#,
        );
        feed(&mut engine, "RET");
        feed(&mut engine, "M-x");
        type_text(&mut engine, "eval-buffer");
        feed(&mut engine, "RET");
        with_editor(|ed| assert_eq!(ed.echo.as_deref(), Some("hijacked!")));
    }

    /// Same guard for man.scm (real load position: after compile.scm and
    /// help.scm, whose quit-window it reuses).
    #[test]
    fn man_loads_cleanly() {
        let mut engine = build_engine();
        for src in [
            include_str!("bootstrap.scm"),
            include_str!("compile.scm"),
            include_str!("help.scm"),
            include_str!("man.scm"),
        ] {
            engine.compile_and_run_raw_program(src.to_string()).unwrap();
        }
    }

    /// The pure halves of man.scm: nroff backspace-overstrike parsing
    /// (bold/underline spans over cleaned text, runs merged), topic
    /// translation, and the word-plus-"(3)" default entry at point.
    #[test]
    fn man_fontify_and_topic_parsing() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);
        load_help(&mut engine);
        load_man(&mut engine);
        let check = |engine: &mut Engine, src: &str| {
            let result = engine.compile_and_run_raw_program(src.to_string()).unwrap();
            assert_eq!(
                format!("{:?}", result.last().unwrap()),
                "#true",
                "failed: {src}"
            );
        };

        // "N BS N a BS a ..." = bold NAME; "_ BS c" = underlined c. The
        // backspaces are built from (integer->char 8), same as man.scm.
        check(
            &mut engine,
            r#"(let* ((BS man-backspace)
                     (raw (string-append
                           "N" BS "N" "a" BS "a" " ok "
                           "_" BS "F" "_" BS "I" "LE")))
                 (equal? (man-fontify-parse raw)
                         (cons "Na ok FILE"
                               '((0 2 "Man-overstrike")
                                 (6 8 "Man-underline")))))"#,
        );
        // A char overstruck by an underscore keeps the letter; a trailing
        // backspace just drops the char before it.
        check(
            &mut engine,
            r#"(equal? (man-fontify-parse
                        (string-append "x" man-backspace "_" "y" man-backspace))
                       (cons "x" '((0 1 "Man-underline"))))"#,
        );
        // No overstrikes: text passes through untouched.
        check(
            &mut engine,
            r#"(equal? (man-fontify-parse "plain text\n") (cons "plain text\n" '()))"#,
        );

        // Emacs' reference syntax: "ls(2)" -> "2 ls"; plain topics pass through.
        check(&mut engine, r#"(equal? (man-translate-topic "ls(2)") "2 ls")"#);
        check(&mut engine, r#"(equal? (man-translate-topic "2 ls") "2 ls")"#);

        // The apropos->candidates awk pipeline: comma-separated aliases
        // each become "name(sec)"; unparsable lines contribute nothing.
        // Driven with fake apropos output substituted for `man -k .`.
        check(
            &mut engine,
            r#"(equal? (string-lines
                        (car (run-shell-command
                              (string-append
                               "printf 'grep, egrep (1)      - print lines\n"
                               "IO::Socket::IP (3pm) - IP socket\n"
                               "mandb: nothing appropriate\n' | "
                               (substring man-apropos-command
                                          (string-length "man -k . 2>/dev/null | ")
                                          (string-length man-apropos-command))))))
                       '("grep(1)" "egrep(1)" "IO::Socket::IP(3pm)"))"#,
        );

        // The native completion matcher: prefix hits first, then substring
        // hits, source order kept; empty input passes everything through.
        check(
            &mut engine,
            r#"(equal? (filter-matching '("mandb(8)" "man(1)" "woman(3)" "ls(1)") "man")
                       '("mandb(8)" "man(1)" "woman(3)"))"#,
        );
        check(
            &mut engine,
            r#"(equal? (filter-matching '("a" "b") "") '("a" "b"))"#,
        );
        check(
            &mut engine,
            r#"(equal? (filter-suffix '("a(1)" "b(3)" "c(1)") "(1)") '("a(1)" "c(1)"))"#,
        );

        // Emacs' "SEC NAME" prompt form completes names within the section
        // (driven against the real man database, like man_end_to_end).
        check(
            &mut engine,
            r#"(equal? (car (man-completion-candidates "3 malloc")) "malloc(3)")"#,
        );
        check(
            &mut engine,
            r#"(equal? (filter-suffix (man-completion-candidates "3 mal") "(3)")
                       (man-completion-candidates "3 mal"))"#,
        );

        // Default entry: the word at point, keeping a "(3)" suffix.
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("see printf(3) and ls for details");
            win.point = 6; // inside "printf"
        });
        check(&mut engine, r#"(equal? (man-default-entry) "printf(3)")"#);
        with_editor(|ed| ed.windows.selected_mut().point = 19); // inside "ls"
        check(&mut engine, r#"(equal? (man-default-entry) "ls")"#);
        // The space after "(3)": not touching a word on either side.
        with_editor(|ed| ed.windows.selected_mut().point = 13);
        check(&mut engine, r#"(equal? (man-default-entry) "")"#);
    }

    /// The real thing: (man-getpage "ls") runs the system man in the
    /// background; when the process exits, the *Man ls* buffer holds the
    /// cleaned page (no backspaces), read-only in Man mode with bold/
    /// underline face spans, shown in the other window without stealing
    /// focus. Needs man + its pages installed, like the dired tests need a
    /// real filesystem.
    #[test]
    fn man_end_to_end() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);
        load_help(&mut engine);
        load_man(&mut engine);

        engine
            .compile_and_run_raw_program(r#"(man-getpage "ls")"#.to_string())
            .unwrap();
        assert!(
            with_editor(|ed| !ed.processes.is_empty()),
            "man did not start (is man installed?)"
        );
        pump_until_done(&mut engine);

        let before = with_editor(|ed| ed.cur_buffer().name.clone());
        with_editor(|ed| {
            let id = ed.buffer_by_name("*Man ls*").expect("*Man ls* buffer exists");
            let buf = &ed.buffers[&id];
            let text = buf.to_string_lossless();
            assert!(
                text.contains("ls - list directory contents"),
                "page text: {:.200}",
                text
            );
            assert!(!text.contains('\u{8}'), "backspaces survived fontifying");
            assert!(buf.read_only);
            assert_eq!(buf.mode_name, "Man");
            assert_eq!(buf.local_map.as_deref(), Some("Man-mode-map"));
            assert!(
                !buf.face_spans.is_empty(),
                "no overstrike spans found (man produced unformatted output?)"
            );
            assert_eq!(before, ed.cur_buffer().name, "man stole the selected window");
        });

        // A repeat request reuses the finished buffer instead of respawning.
        engine
            .compile_and_run_raw_program(r#"(man-getpage "ls")"#.to_string())
            .unwrap();
        assert!(
            with_editor(|ed| ed.processes.is_empty()),
            "second man-getpage respawned the process"
        );

        // A bogus topic fails without creating a ready page.
        engine
            .compile_and_run_raw_program(
                r#"(man-getpage "definitely-no-such-page-xyz")"#.to_string(),
            )
            .unwrap();
        pump_until_done(&mut engine);
        with_editor(|ed| {
            let echo = ed.echo.clone().unwrap_or_default();
            assert!(
                echo.contains("definitely-no-such-page-xyz") || echo.contains("man"),
                "no failure message, echo: {echo:?}"
            );
        });
        let result = engine
            .compile_and_run_raw_program(
                r#"(equal? (buffer-local-get-in "*Man definitely-no-such-page-xyz*" "man-ready") #t)"#
                    .to_string(),
            )
            .unwrap();
        assert_eq!(format!("{:?}", result.last().unwrap()), "#false");
    }

    /// M-x man completing through the vertico plugin, driven over the real
    /// key path: the prompt opens with kind "man" and the full apropos
    /// candidate list; typing narrows it to matching "name(section)"
    /// entries. Needs a populated man database, like man_end_to_end.
    #[test]
    fn man_completion_through_vertico() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);
        load_help(&mut engine);
        load_man(&mut engine);
        engine
            .compile_and_run_raw_program(
                include_str!("../../examples/vertico.scm").to_string(),
            )
            .unwrap();

        let feed = |engine: &mut Engine, keys: &str| {
            for chord in parse_seq(keys).unwrap() {
                process_chord(engine, chord);
            }
        };
        let type_text = |engine: &mut Engine, text: &str| {
            for c in text.chars() {
                process_chord(engine, crate::keys::Chord {
                    ctrl: false,
                    meta: false,
                    key: crate::keys::Key::Char(c),
                });
            }
        };

        // M-x man RET: the M-x prompt hands over to the man prompt, whose
        // setup fires vertico with the whole topic list.
        feed(&mut engine, "M-x");
        type_text(&mut engine, "man");
        feed(&mut engine, "RET");
        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else {
                panic!("M-x man did not open a prompt")
            };
            assert!(p.prompt.starts_with("Manual entry"), "prompt: {:?}", p.prompt);
            assert!(
                p.completions.len() > 100,
                "expected the full apropos list, got {} candidates",
                p.completions.len()
            );
        });

        // The "SEC NAME" form: candidates come from that section only.
        type_text(&mut engine, "3 mall");
        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else { panic!() };
            assert!(!p.completions.is_empty(), "no candidates for '3 mall'");
            assert_eq!(p.completions[0], "malloc(3)", "candidates: {:?}",
                &p.completions[..p.completions.len().min(5)]);
            assert!(
                p.completions.iter().all(|c| c.ends_with("(3)")),
                "candidates outside section 3: {:?}",
                &p.completions[..p.completions.len().min(5)]
            );
        });
        for _ in 0..6 {
            feed(&mut engine, "backspace");
        }

        // Typing narrows to matching "name(section)" candidates.
        type_text(&mut engine, "printf");
        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else { panic!() };
            assert!(!p.completions.is_empty(), "no candidates for printf");
            assert!(
                p.completions.iter().all(|c| c.contains("printf")),
                "unrelated candidates: {:?}",
                &p.completions[..p.completions.len().min(5)]
            );
            assert!(
                p.completions[0] == "printf(1)" || p.completions[0] == "printf(3)",
                "shortest prefix match should rank first: {:?}",
                &p.completions[..p.completions.len().min(5)]
            );
        });
        feed(&mut engine, "C-g");
        with_editor(|ed| assert!(matches!(ed.input, InputMode::Normal)));
    }

    /// Same guard for dired.scm: a syntax error there would otherwise only
    /// surface as an echo-line message the first time a directory is opened.
    /// Stacked in the real load order (bootstrap -> compile -> dired), since
    /// dired's A command calls into compile.scm's results machinery.
    #[test]
    fn dired_loads_cleanly() {
        let mut engine = build_engine();
        engine
            .compile_and_run_raw_program(include_str!("bootstrap.scm").to_string())
            .unwrap();
        engine
            .compile_and_run_raw_program(include_str!("compile.scm").to_string())
            .unwrap();
        engine
            .compile_and_run_raw_program(include_str!("dired.scm").to_string())
            .unwrap();
    }

    /// Same guard for the language modes, stacked in the real load order
    /// (bootstrap -> compile -> dired -> rust -> python -> c -> scheme) —
    /// Steel resolves free identifiers per file, so one typo'd primitive
    /// name anywhere fails the whole file.
    #[test]
    fn language_modes_load_cleanly() {
        let mut engine = build_engine();
        for (src, name) in [
            (include_str!("bootstrap.scm"), "bootstrap.scm"),
            (include_str!("compile.scm"), "compile.scm"),
            (include_str!("help.scm"), "help.scm"),
            (include_str!("man.scm"), "man.scm"),
            (include_str!("dired.scm"), "dired.scm"),
            (include_str!("rust-mode.scm"), "rust-mode.scm"),
            (include_str!("python-mode.scm"), "python-mode.scm"),
            (include_str!("c-mode.scm"), "c-mode.scm"),
            (include_str!("scheme-mode.scm"), "scheme-mode.scm"),
        ] {
            engine
                .compile_and_run_raw_program(src.to_string())
                .unwrap_or_else(|e| panic!("{name} failed to load: {e}"));
        }
    }

    /// The pure-Scheme indent heuristics of the new language modes,
    /// exercised directly — never through (python-mode) etc., which would
    /// install tree-sitter grammars over the network.
    #[test]
    fn language_mode_indent_heuristics() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_python_mode(&mut engine);
        load_c_mode(&mut engine);
        load_scheme_mode(&mut engine);

        let indent_case = |engine: &mut Engine, text: &str, cmd: &str, expect: &str| {
            with_editor(|ed| {
                let (win, buf) = ed.cur();
                buf.set_text(text);
                win.point = buf.len_chars();
            });
            engine.compile_and_run_raw_program(format!("({cmd})")).unwrap();
            with_editor(|ed| {
                assert_eq!(
                    ed.cur_buffer().to_string_lossless(),
                    expect,
                    "{cmd} on {text:?}"
                );
            });
        };

        // Python: a level in after ":", a level out on a dedent keyword.
        indent_case(&mut engine, "def f():\nreturn 1", "python-indent-line",
                    "def f():\n    return 1");
        indent_case(&mut engine, "if x:\n    y()\n    else:", "python-indent-line",
                    "if x:\n    y()\nelse:");
        indent_case(&mut engine, "try:\nexcept:", "python-indent-line",
                    "try:\nexcept:");

        // C: brace depth, closer dedents.
        indent_case(&mut engine, "int f() {\nint x;", "c-indent-line",
                    "int f() {\n    int x;");
        indent_case(&mut engine, "int f() {\n    int x;\n    }", "c-indent-line",
                    "int f() {\n    int x;\n}");

        // Scheme: innermost open paren column + 2; closed forms don't count.
        indent_case(&mut engine, "(define (f x)\n(+ x 1))", "scheme-indent-line",
                    "(define (f x)\n  (+ x 1))");
        indent_case(&mut engine, "(let ((a 1))\na)", "scheme-indent-line",
                    "(let ((a 1))\n  a)");
        indent_case(&mut engine, "(f 1)\nx", "scheme-indent-line",
                    "(f 1)\nx");

        // The generic electric helpers (used by all three modes).
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("");
            win.point = 0;
        });
        engine
            .compile_and_run_raw_program(
                "(scheme-electric-open-paren) (scheme-electric-close-paren)".to_string(),
            )
            .unwrap();
        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().to_string_lossless(), "()");
            assert_eq!(ed.windows.selected_ref().point, 2, "closer skipped, not doubled");
        });
    }

    /// Same guard for rust-mode.scm.
    #[test]
    fn rust_mode_loads_cleanly() {
        let mut engine = build_engine();
        engine
            .compile_and_run_raw_program(include_str!("bootstrap.scm").to_string())
            .unwrap();
        engine
            .compile_and_run_raw_program(include_str!("rust-mode.scm").to_string())
            .unwrap();
    }

    /// Exercises rust-mode's pure-Scheme helpers directly — rust-indent-line,
    /// the electric pair commands, and comment-dwim — without going through
    /// (rust-mode)/find-file-hook, which would try to install the
    /// tree-sitter grammar over the network.
    #[test]
    fn rust_mode_indent_and_electric_pairs() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        engine
            .compile_and_run_raw_program(include_str!("rust-mode.scm").to_string())
            .unwrap();

        let run = |engine: &mut Engine, src: &str| {
            engine.compile_and_run_raw_program(src.to_string()).unwrap();
        };

        // Indent: point on the second (unindented) line of a just-opened
        // block should indent one level in.
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("fn main() {\nlet x = 1;");
            win.point = buf.len_chars();
        });
        run(&mut engine, "(rust-indent-line)");
        with_editor(|ed| {
            assert_eq!(
                ed.cur_buffer().to_string_lossless(),
                "fn main() {\n    let x = 1;"
            );
        });

        // A line starting with a closer dedents back out of the block, even
        // if it started out mis-indented.
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("fn main() {\n    let x = 1;\n    }");
            win.point = buf.len_chars();
        });
        run(&mut engine, "(rust-indent-line)");
        with_editor(|ed| {
            assert_eq!(
                ed.cur_buffer().to_string_lossless(),
                "fn main() {\n    let x = 1;\n}"
            );
        });

        // Electric pairs: "(" inserts "()" with point left in between;
        // typing ")" right after skips over it instead of duplicating it.
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("");
            win.point = 0;
        });
        run(&mut engine, "(rust-electric-open-paren)");
        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().to_string_lossless(), "()");
            assert_eq!(ed.windows.selected_ref().point, 1);
        });
        run(&mut engine, "(rust-electric-close-paren)");
        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().to_string_lossless(), "()");
            assert_eq!(
                ed.windows.selected_ref().point,
                2,
                "typing the closer should skip over the auto-inserted one, not duplicate it"
            );
        });

        // comment-dwim with no region: appends a trailing comment using the
        // buffer-local comment-start rust-mode set up.
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("let y = 2;");
            win.point = 0;
        });
        run(&mut engine, "(buffer-local-set! \"comment-start\" \"// \")");
        run(&mut engine, "(comment-dwim)");
        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().to_string_lossless(), "let y = 2; // ");
        });
    }

    /// RET in rust-mode: the new line lands already indented for its block,
    /// pressing RET between an empty {} opens the block (closer on its own
    /// line, point one level in), and typing "}" on a fresh line snaps it
    /// back to the block's indent.
    #[test]
    fn rust_mode_newline_and_indent() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        engine
            .compile_and_run_raw_program(include_str!("rust-mode.scm").to_string())
            .unwrap();
        let run = |engine: &mut Engine, src: &str| {
            engine.compile_and_run_raw_program(src.to_string()).unwrap();
        };

        // RET at the end of a line ending in "{": body line already one
        // indent-width in.
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("fn main() {");
            win.point = buf.len_chars();
        });
        run(&mut engine, "(rust-newline-and-indent)");
        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().to_string_lossless(), "fn main() {\n    ");
            assert_eq!(ed.windows.selected_ref().point, 16);
        });

        // RET between the empty pair `{|}`: closer moves to its own line at
        // the parent indent, point on a fresh body line one level in.
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("fn main() {}");
            win.point = 11;
        });
        run(&mut engine, "(rust-newline-and-indent)");
        with_editor(|ed| {
            assert_eq!(
                ed.cur_buffer().to_string_lossless(),
                "fn main() {\n    \n}"
            );
            assert_eq!(
                ed.windows.selected_ref().point,
                16,
                "point should sit at the end of the indented body line"
            );
        });

        // Typing "}" as the first non-space char of a line dedents it to
        // the block's indent, even though RET indented it as a body line.
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("fn main() {\n    let x = 1;\n    ");
            win.point = buf.len_chars();
        });
        run(&mut engine, "(rust-electric-close-brace)");
        with_editor(|ed| {
            assert_eq!(
                ed.cur_buffer().to_string_lossless(),
                "fn main() {\n    let x = 1;\n}"
            );
            assert_eq!(ed.windows.selected_ref().point, 28);
        });

        // The step follows the global indent-width.
        run(&mut engine, "(set! indent-width 2)");
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("fn main() {");
            win.point = buf.len_chars();
        });
        run(&mut engine, "(rust-newline-and-indent)");
        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().to_string_lossless(), "fn main() {\n  ");
        });
        run(&mut engine, "(set! indent-width 4)");
    }

    /// rust-indent-line and comment-dwim must edit through delete-region!/
    /// insert-text, not set-buffer-string! (which is for regenerating
    /// non-user content like a dired listing: no undo entry, and it resets
    /// `modified` to false) — otherwise C-/ silently does nothing after
    /// either command runs.
    #[test]
    fn indent_and_comment_dwim_are_undoable() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        engine
            .compile_and_run_raw_program(include_str!("rust-mode.scm").to_string())
            .unwrap();
        let run = |engine: &mut Engine, src: &str| {
            engine.compile_and_run_raw_program(src.to_string()).unwrap();
        };

        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("fn main() {\nlet x = 1;");
            win.point = buf.len_chars();
        });
        run(&mut engine, "(rust-indent-line)");
        with_editor(|ed| {
            assert_eq!(
                ed.cur_buffer().to_string_lossless(),
                "fn main() {\n    let x = 1;"
            );
            assert!(ed.cur_buffer().modified, "indenting is a real, user-visible edit");
            assert!(
                ed.cur_buffer_mut().undo_group().is_some(),
                "rust-indent-line must be undoable"
            );
            assert_eq!(ed.cur_buffer().to_string_lossless(), "fn main() {\nlet x = 1;");
        });

        run(&mut engine, "(buffer-local-set! \"comment-start\" \"// \")");
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("let y = 2;");
            win.point = 0;
        });
        run(&mut engine, "(comment-dwim)");
        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().to_string_lossless(), "let y = 2; // ");
            assert!(ed.cur_buffer().modified);
            assert!(
                ed.cur_buffer_mut().undo_group().is_some(),
                "comment-dwim (no region) must be undoable"
            );
            assert_eq!(ed.cur_buffer().to_string_lossless(), "let y = 2;");
        });

        // Region case: mark set across both lines, both get commented, and
        // undo restores both as one group.
        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.set_text("let a = 1;\nlet b = 2;");
            win.point = buf.len_chars();
            buf.mark = Some(0);
            buf.mark_active = true;
        });
        run(&mut engine, "(comment-dwim)");
        with_editor(|ed| {
            assert_eq!(
                ed.cur_buffer().to_string_lossless(),
                "// let a = 1;\n// let b = 2;"
            );
            assert!(
                ed.cur_buffer_mut().undo_group().is_some(),
                "comment-dwim (region) must be undoable"
            );
            assert_eq!(
                ed.cur_buffer().to_string_lossless(),
                "let a = 1;\nlet b = 2;"
            );
        });
    }

    /// End-to-end contract test, headless: bootstrap builds the keymap
    /// through the public API, Scheme edits the buffer, defines a command,
    /// binds it, and the dispatcher runs it.
    #[test]
    fn steel_contract_end_to_end() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);
        load_dired(&mut engine);

        // Bootstrap populated the global map, dired.scm its own named map,
        // through the exact same (global-set-key ...)/(define-key ...) API
        // user config uses.
        with_editor(|ed| {
            assert_eq!(
                ed.global_map.lookup(&parse_seq("C-x C-f").unwrap()),
                Lookup::Cmd("find-file".into())
            );
            assert_eq!(
                ed.keymaps
                    .get("dired-mode-map")
                    .unwrap()
                    .lookup(&parse_seq("% m").unwrap()),
                Lookup::Cmd("dired-mark-regexp".into())
            );
        });

        engine
            .compile_and_run_raw_program(
                r#"
                (insert-text "hello world")
                (beginning-of-line)
                (forward-word)
                (define-command "my-shout" "Upcase the word after point"
                  (lambda () (upcase-word)))
                (global-set-key "C-c s" "my-shout")
                "#
                .to_string(),
            )
            .unwrap();

        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().to_string_lossless(), "hello world");
            assert_eq!(ed.windows.selected_ref().point, 5);
            assert!(ed.registry.contains_key("my-shout"));
        });

        // Drive the Scheme-defined command through the key dispatcher.
        let mut post = PostAction::None;
        for chord in parse_seq("C-c s").unwrap() {
            post = with_editor(|ed| dispatch::handle_chord(ed, chord));
        }
        match post {
            PostAction::RunScheme(f, args) => call_scheme_command(&mut engine, f, args),
            _ => panic!("C-c s did not resolve to the Scheme command"),
        }

        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().to_string_lossless(), "hello WORLD");
        });
    }

    /// Drives the Steel port of dired end-to-end through the real engine:
    /// open a directory, confirm the listing and read-only state, then
    /// bulk-rename a file through wgrep (edit the listing as text, commit)
    /// and confirm the rename actually happened on disk. This is what used
    /// to be covered by src/dired/{mod,wgrep}.rs's own #[cfg(test)]s before
    /// dired moved to Scheme.
    #[test]
    fn dired_end_to_end() {
        let dir = std::env::temp_dir().join(format!("taco-dired-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("alpha.txt"), "a").unwrap();
        std::fs::write(dir.join("beta.txt"), "b").unwrap();

        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);
        load_dired(&mut engine);

        engine
            .compile_and_run_raw_program(format!(r#"(dired "{}")"#, dir.display()))
            .unwrap();
        with_editor(|ed| {
            let text = ed.cur_buffer().to_string_lossless();
            assert!(text.contains("alpha.txt"), "listing: {text}");
            assert!(text.contains("beta.txt"), "listing: {text}");
            assert!(ed.cur_buffer().read_only);
        });

        // wgrep: rename alpha.txt -> gamma.txt by editing the listing text.
        engine
            .compile_and_run_raw_program("(wgrep-mode)".to_string())
            .unwrap();
        with_editor(|ed| {
            assert!(!ed.cur_buffer().read_only, "wgrep-mode makes the buffer writable");
            let text = ed.cur_buffer().to_string_lossless().replace("alpha.txt", "gamma.txt");
            ed.cur_buffer_mut().set_text(&text);
        });
        engine
            .compile_and_run_raw_program("(wgrep-commit)".to_string())
            .unwrap();

        assert!(dir.join("gamma.txt").exists());
        assert!(!dir.join("alpha.txt").exists());
        assert!(dir.join("beta.txt").exists());
        with_editor(|ed| {
            assert!(ed.cur_buffer().read_only, "wgrep-commit restores read-only");
            let text = ed.cur_buffer().to_string_lossless();
            assert!(text.contains("gamma.txt"), "listing after commit: {text}");
        });

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// dired's A (dired-do-find-regexp): matches from the marked files
    /// land in a grep-style buffer wired to compile.scm's results
    /// machinery, and next-error jumps to the real file and line.
    #[test]
    fn dired_find_regexp_end_to_end() {
        let dir = std::env::temp_dir().join(format!("taco-dired-A-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("one.txt"), "hello\nneedle here\nbye\n").unwrap();
        std::fs::write(dir.join("two.txt"), "needle again\nnothing\n").unwrap();
        std::fs::write(dir.join("three.txt"), "no matches\n").unwrap();

        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);
        load_dired(&mut engine);

        engine
            .compile_and_run_raw_program(format!(r#"(dired "{}")"#, dir.display()))
            .unwrap();
        // Mark one.txt and two.txt, then search them.
        engine
            .compile_and_run_raw_program(
                r#"(dired-goto-entry "one.txt") (dired-mark)
                   (dired-goto-entry "two.txt") (dired-mark)
                   (dired-find-regexp-run "needle")"#
                    .to_string(),
            )
            .unwrap();
        with_editor(|ed| {
            let buf = ed.cur_buffer();
            assert_eq!(buf.name, "*Find Regexp*");
            assert!(buf.read_only);
            assert_eq!(buf.mode_name, "Grep");
            assert_eq!(buf.local_map.as_deref(), Some("compilation-mode-map"));
            let text = buf.to_string_lossless();
            assert!(text.contains("one.txt:2:needle here"), "results: {text}");
            assert!(text.contains("two.txt:1:needle again"), "results: {text}");
            assert!(!text.contains("three.txt"), "unmarked file searched: {text}");
            assert_eq!(ed.echo.as_deref(), Some("2 matches"));
        });
        // next-error visits the first match.
        engine.compile_and_run_raw_program("(next-error)".to_string()).unwrap();
        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().name, "one.txt");
            let point = ed.windows.selected_ref().point;
            assert_eq!(ed.cur_buffer().char_to_line(point), 1); // line 2
        });

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The vertico plugin, headless, through the exact main-loop code path:
    /// M-x pops the full candidate list, typing narrows it live, C-n/C-p
    /// cycle with wrap-around, RET submits the selected candidate.
    #[test]
    fn vertico_plugin_end_to_end() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);
        load_help(&mut engine);
        load_man(&mut engine); // vertico's man source is man.scm's
        engine
            .compile_and_run_raw_program(
                include_str!("../../examples/vertico.scm").to_string(),
            )
            .unwrap();

        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.insert(0, "hello");
            win.point = 0;
        });
        let feed = |engine: &mut Engine, keys: &str| {
            for chord in parse_seq(keys).unwrap() {
                process_chord(engine, chord);
            }
        };

        // M-x: the setup hook renders every command as a candidate.
        feed(&mut engine, "M-x");
        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else {
                panic!("M-x did not open a prompt")
            };
            assert!(p.completions.len() > 20, "expected all commands listed");
        });

        // Typing narrows on every keystroke (no TAB involved).
        feed(&mut engine, "f o r w a r d -");
        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else { panic!() };
            assert_eq!(p.completions, vec!["forward-char", "forward-word"]);
            assert_eq!(p.selected, 0);
        });

        // C-n / C-p cycle and wrap.
        feed(&mut engine, "C-n");
        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else { panic!() };
            assert_eq!(p.selected, 1);
        });
        feed(&mut engine, "C-n");
        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else { panic!() };
            assert_eq!(p.selected, 0, "C-n wraps to the first candidate");
        });
        feed(&mut engine, "C-p");
        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else { panic!() };
            assert_eq!(p.selected, 1, "C-p wraps to the last candidate");
        });

        // RET submits the highlighted candidate: forward-word moves point.
        feed(&mut engine, "RET");
        with_editor(|ed| {
            assert!(matches!(ed.input, InputMode::Normal));
            assert_eq!(ed.windows.selected_ref().point, 5, "forward-word ran");
        });
    }

    /// File completion through the vertico plugin lists directories ahead
    /// of files, regardless of alphabetical order.
    #[test]
    fn vertico_file_completion_dirs_first() {
        let dir = std::env::temp_dir().join(format!("taco-vertico-e2e-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("beta")).unwrap();
        std::fs::write(dir.join("alpha.txt"), "a").unwrap();

        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);
        load_help(&mut engine);
        load_man(&mut engine); // vertico's man source is man.scm's
        engine
            .compile_and_run_raw_program(
                include_str!("../../examples/vertico.scm").to_string(),
            )
            .unwrap();

        for chord in parse_seq("C-x C-f").unwrap() {
            process_chord(&mut engine, chord);
        }
        engine
            .compile_and_run_raw_program(format!(
                r#"(set-minibuffer-contents "{}/") (vertico--refresh)"#,
                dir.display()
            ))
            .unwrap();

        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else {
                panic!("C-x C-f did not open a prompt")
            };
            assert_eq!(p.completions, vec!["beta/", "alpha.txt"]);
        });

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Stock minibuffer editing (no plugin): full cursor movement and
    /// in-place edits, and no candidate list ever appears.
    #[test]
    fn minibuffer_line_editing() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        let feed = |engine: &mut Engine, keys: &str| {
            for chord in parse_seq(keys).unwrap() {
                process_chord(engine, chord);
            }
        };

        feed(&mut engine, "M-x");
        feed(&mut engine, "y a n k s");
        // C-b back over "s", C-d deletes it; C-a then C-f C-f, C-k kills
        // the tail, leaving "ya".
        feed(&mut engine, "C-b C-d C-a C-f C-f C-k");
        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else { panic!() };
            assert_eq!(p.input, "ya");
            assert_eq!(p.cursor, 2);
            assert!(p.completions.is_empty(), "no completion UI without a plugin");
        });
        // C-e/C-a position; insertion lands at the cursor.
        feed(&mut engine, "C-a n o C-e");
        with_editor(|ed| {
            let InputMode::Prompt(p) = &ed.input else { panic!() };
            assert_eq!(p.input, "noya");
            assert_eq!(p.cursor, 4);
        });
        // The C-k'd tail went to the kill ring (C-d deletes silently).
        with_editor(|ed| assert_eq!(ed.kill_ring.yank(), Some("nk")));
        feed(&mut engine, "C-g");
        with_editor(|ed| assert!(matches!(ed.input, InputMode::Normal)));
    }

    #[test]
    fn dired_command_smoke() {
        let dir = std::env::temp_dir().join(format!("taco-dired-smoke-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "aaa").unwrap();
        std::fs::write(dir.join("b.txt"), "bbb").unwrap();
        std::fs::write(dir.join(".hidden"), "h").unwrap();

        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);
        load_dired(&mut engine);
        let run = |engine: &mut Engine, src: &str| {
            engine.compile_and_run_raw_program(src.to_string()).unwrap();
        };

        run(&mut engine, &format!(r#"(dired "{}")"#, dir.display()));
        with_editor(|ed| {
            let t = ed.cur_buffer().to_string_lossless();
            assert!(!t.contains(".hidden"), "hidden file shown by default: {t}");
        });

        run(&mut engine, "(dired-toggle-hidden)");
        with_editor(|ed| {
            let t = ed.cur_buffer().to_string_lossless();
            assert!(t.contains(".hidden"), "toggle-hidden didn't show dotfile: {t}");
        });
        run(&mut engine, "(dired-toggle-hidden)");

        // Mark a.txt, then mark-regexp on b, then unmark-all.
        run(&mut engine, "(dired-goto-entry \"a.txt\")");
        run(&mut engine, "(dired-mark)");
        with_editor(|ed| {
            let t = ed.cur_buffer().to_string_lossless();
            assert!(t.contains("*"), "mark not shown: {t}");
        });
        run(&mut engine, "(dired-mark-regexp \"b\\\\.txt\")");
        run(&mut engine, "(dired-unmark-all)");
        with_editor(|ed| {
            let t = ed.cur_buffer().to_string_lossless();
            assert!(!t.contains('*'), "unmark-all left a mark: {t}");
        });

        // mkdir
        run(&mut engine, r#"(dired-mkdir "newdir")"#);
        assert!(dir.join("newdir").is_dir());
        with_editor(|ed| {
            let t = ed.cur_buffer().to_string_lossless();
            assert!(t.contains("newdir"), "listing missing newdir: {t}");
        });

        // rename a.txt -> a2.txt: find its line, then rename.
        let goto_named = |engine: &mut Engine, name: &str| {
            engine
                .compile_and_run_raw_program(format!("(dired-goto-entry \"{name}\")"))
                .unwrap();
        };
        goto_named(&mut engine, "a.txt");
        run(&mut engine, r#"(dired-rename-to! (dired-entry-path (dired-entry-at-point)) "a2.txt")"#);
        assert!(dir.join("a2.txt").exists());
        assert!(!dir.join("a.txt").exists());

        // copy b.txt -> b2.txt
        goto_named(&mut engine, "b.txt");
        run(
            &mut engine,
            r#"(dired-copy-to! (dired-entry-path (dired-entry-at-point)) #f "b2.txt")"#,
        );
        assert!(dir.join("b2.txt").exists());
        assert!(dir.join("b.txt").exists(), "copy should not remove the source");

        // diff a2.txt against b2.txt (different content -> non-empty diff, shown in *diff*).
        goto_named(&mut engine, "a2.txt");
        run(
            &mut engine,
            &format!(r#"(dired-diff-against! (dired-entry-path (dired-entry-at-point)) "{}")"#,
                dir.join("b2.txt").display()),
        );
        with_editor(|ed| {
            let id = ed.buffer_by_name("*diff*").expect("*diff* buffer created");
            let t = ed.buffers.get(&id).unwrap().to_string_lossless();
            assert!(t.contains("@@") || t.contains("no differences"), "diff output: {t}");
        });

        // Switch back to the dired window/buffer for further ops.
        run(&mut engine, &format!(r#"(open-dired "{}")"#, dir.display()));

        // compress newdir -> newdir.tar.gz
        goto_named(&mut engine, "newdir");
        run(&mut engine, "(dired-compress)");
        assert!(dir.join("newdir.tar.gz").exists(), "compress didn't produce a tarball");

        // shell command on the file at point (b2.txt): `wc -c`
        goto_named(&mut engine, "b2.txt");
        run(&mut engine, r#"(dired-run-shell "wc -c")"#);
        with_editor(|ed| {
            let id = ed.buffer_by_name("*Shell Command Output*").expect("shell output buffer");
            let t = ed.buffers.get(&id).unwrap().to_string_lossless();
            assert!(t.trim().len() > 0, "shell output empty: {t:?}");
        });

        // Back to dired, delete a2.txt via the generic y-or-n-p continuation.
        run(&mut engine, &format!(r#"(open-dired "{}")"#, dir.display()));
        goto_named(&mut engine, "a2.txt");
        run(&mut engine, "(dired-do-delete)");
        // y-or-n-p opened a prompt; answer 'y' through the real key path.
        for chord in crate::keys::parse_seq("y").unwrap() {
            process_chord(&mut engine, chord);
        }
        assert!(!dir.join("a2.txt").exists(), "delete via y-or-n-p didn't remove the file");

        // up-directory from sub/, then dired-jump back into it, then kill-all.
        run(&mut engine, &format!(r#"(open-dired "{}")"#, dir.join("sub").display()));
        run(&mut engine, "(dired-up-directory)");
        with_editor(|ed| {
            assert_eq!(
                ed.cur_buffer().locals.get("dired-directory").map(|v| match v {
                    steel::rvals::SteelVal::StringV(s) => s.to_string(),
                    _ => String::new(),
                }),
                Some(dir.display().to_string())
            );
        });

        run(&mut engine, "(dired-kill-all)");
        with_editor(|ed| {
            let any_dired = ed.buffers.values().any(|b| b.locals.contains_key("dired-directory"));
            assert!(!any_dired, "kill-all left a dired buffer behind");
        });

        std::fs::remove_dir_all(&dir).ok();

        // Fresh setup for RET-on-directory and dired-jump.
        let dir2 = std::env::temp_dir().join(format!("taco-dired-smoke2-{}", std::process::id()));
        std::fs::create_dir_all(dir2.join("child")).unwrap();
        std::fs::write(dir2.join("child").join("f.txt"), "x").unwrap();

        let mut engine2 = build_engine();
        load_bootstrap(&mut engine2);
        load_compile(&mut engine2);
        load_dired(&mut engine2);
        let run2 = |engine: &mut Engine, src: &str| {
            engine.compile_and_run_raw_program(src.to_string()).unwrap();
        };

        run2(&mut engine2, &format!(r#"(dired "{}")"#, dir2.display()));
        run2(&mut engine2, r#"(dired-goto-entry "child")"#);
        run2(&mut engine2, "(dired-find-file)");
        with_editor(|ed| {
            let t = ed.cur_buffer().to_string_lossless();
            assert!(t.contains("f.txt"), "RET on directory didn't open it: {t}");
        });

        // Visit f.txt for real, then dired-jump should land back in its dir
        // with point on f.txt.
        run2(&mut engine2, &format!(r#"(find-file "{}")"#, dir2.join("child").join("f.txt").display()));
        run2(&mut engine2, "(dired-jump)");
        with_editor(|ed| {
            let t = ed.cur_buffer().to_string_lossless();
            assert!(t.contains("f.txt"), "dired-jump landed in the wrong listing: {t}");
        });
        run2(&mut engine2, "(dired-find-file)"); // point should be on f.txt's line
        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().name, "f.txt", "dired-jump didn't place point on f.txt");
        });

        std::fs::remove_dir_all(&dir2).ok();
    }

    /// The real thing, end-to-end: clone tree-sitter-rust from GitHub,
    /// compile it with the system C compiler, dynamically load it, enable
    /// it on a buffer with real Rust source, and confirm actual tree-sitter
    /// highlight spans come back (not just "the file loaded without
    /// crashing"). Needs network + a C compiler; if either is missing this
    /// is the one test in the suite that can't run offline.
    #[test]
    fn tree_sitter_install_and_highlight_rust() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);

        let result = engine
            .compile_and_run_raw_program(
                r#"(equal? (tree-sit-install-language-grammar "rust" "https://github.com/tree-sitter/tree-sitter-rust") "")"#
                    .to_string(),
            )
            .unwrap();
        assert_eq!(
            result.first().map(|v| format!("{v:?}")),
            Some("#true".to_string()),
            "install-language-grammar did not report success"
        );

        with_editor(|ed| {
            let (win, buf) = ed.cur();
            buf.insert(0, "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n");
            win.point = 0;
        });
        engine
            .compile_and_run_raw_program(r#"(tree-sit-enable "rust")"#.to_string())
            .unwrap();

        // Force the lazy rehighlight (normally driven by render::draw).
        with_editor(|ed| {
            let buf = ed.cur_buffer_mut();
            if let Some(syntax) = buf.syntax.as_mut() {
                syntax.ensure_current(&buf.rope);
            }
        });

        with_editor(|ed| {
            let buf = ed.cur_buffer();
            let syntax = buf.syntax.as_ref().expect("tree-sit-enable attached syntax state");
            assert_eq!(syntax.language, "rust");
            assert!(!syntax.spans.is_empty(), "no highlight spans produced");
            let text = buf.to_string_lossless();
            let has_keyword_fn = syntax.spans.iter().any(|(s, e, name)| {
                *name == "keyword" && text.chars().skip(*s).take(e - s).collect::<String>() == "fn"
            });
            assert!(has_keyword_fn, "expected 'fn' tagged as keyword, spans: {:?}", syntax.spans);
        });

        // Node query (mark-defun's backend): point inside the body of
        // `add` selects the whole function_item.
        with_editor(|ed| {
            let point = ed.cur_buffer().to_string_lossless().find("a + b").unwrap();
            ed.windows.selected_mut().point = point;
        });
        let result = engine
            .compile_and_run_raw_program(
                r#"(equal? (tree-sit-node-range-at-point '("function_item") #f) '(0 43))"#
                    .to_string(),
            )
            .unwrap();
        assert_eq!(
            format!("{:?}", result.last().unwrap()),
            "#true",
            "expected the full fn add(...) range (0 43)"
        );

        // The generic mark-defun command wires it to the region.
        engine
            .compile_and_run_raw_program(
                r#"(buffer-local-set! "defun-node-kinds" '("function_item")) (mark-defun)"#
                    .to_string(),
            )
            .unwrap();
        with_editor(|ed| {
            assert_eq!(ed.windows.selected_ref().point, 0);
            assert_eq!(ed.cur_buffer().mark, Some(43));
            assert!(ed.cur_buffer().mark_active);
        });
    }

    /// regexp-match returns capture groups; regexp-match-positions returns
    /// char (not byte) offsets, which multibyte text would expose.
    #[test]
    fn regexp_capture_primitives() {
        let mut engine = build_engine();
        let check = |engine: &mut Engine, src: &str| {
            let result = engine.compile_and_run_raw_program(src.to_string()).unwrap();
            assert_eq!(
                format!("{:?}", result.last().unwrap()),
                "#true",
                "failed: {src}"
            );
        };
        check(
            &mut engine,
            r#"(equal? (regexp-match "([a-z./]+):([0-9]+)" "src/main.rs:42: error")
                       '("src/main.rs:42" "src/main.rs" "42"))"#,
        );
        check(&mut engine, r#"(equal? (regexp-match "z" "no match at all") #f)"#);
        // "é" is 1 char / 2 bytes: char-correct offsets put the match at 2..4.
        check(
            &mut engine,
            r#"(equal? (regexp-match-positions "[0-9]+" "é 42" 0) '((2 4)))"#,
        );
        // Invalid pattern is #f, not an error (same contract as regexp-match?).
        check(&mut engine, r#"(equal? (regexp-match "(" "x") #f)"#);
    }

    /// set-mark/deactivate-mark: the Scheme-visible region contract used
    /// by mark-defun.
    #[test]
    fn set_mark_from_scheme() {
        let mut engine = build_engine();
        with_editor(|ed| {
            ed.cur_buffer_mut().set_text("hello world");
        });
        engine
            .compile_and_run_raw_program("(goto-char 0) (set-mark 5)".to_string())
            .unwrap();
        with_editor(|ed| {
            let buf = ed.cur_buffer();
            assert_eq!(buf.mark, Some(5));
            assert!(buf.mark_active);
            assert_eq!(buf.region(0), Some((0, 5)));
        });
        engine.compile_and_run_raw_program("(deactivate-mark)".to_string()).unwrap();
        with_editor(|ed| assert!(!ed.cur_buffer().mark_active));
    }

    /// Face spans placed from Scheme reach the renderer's per-line ranges,
    /// beat tree-sitter spans (they come first), and vanish on set_text.
    #[test]
    fn face_spans_render_and_clear() {
        let mut engine = build_engine();
        engine
            .compile_and_run_raw_program(
                r#"(set-face-color "compilation-error" "red")
                   (switch-to-buffer "*results*")
                   (set-buffer-string! "foo.c:3:1: error\nok line")
                   (buffer-add-face-span! "*results*" 0 7 "compilation-error")"#
                    .to_string(),
            )
            .unwrap();
        with_editor(|ed| {
            let buf = ed.cur_buffer();
            assert_eq!(buf.face_spans, vec![(0, 7, "compilation-error".to_string())]);
            let ranges = crate::render::syntax_ranges(buf, &ed.faces, 0);
            assert_eq!(ranges.len(), 1);
            assert_eq!((ranges[0].0, ranges[0].1), (0, 7));
            // Line 1 is untouched.
            assert!(crate::render::syntax_ranges(buf, &ed.faces, 1).is_empty());
        });
        // Regenerating the content invalidates the (static) spans.
        engine
            .compile_and_run_raw_program(r#"(set-buffer-string! "regenerated")"#.to_string())
            .unwrap();
        with_editor(|ed| assert!(ed.cur_buffer().face_spans.is_empty()));
    }

    /// Pump the main loop's process half until the process table empties
    /// (or a timeout trips) — the headless stand-in for main.rs's poll
    /// loop.
    fn pump_until_done(engine: &mut Engine) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while with_editor(|ed| !ed.processes.is_empty()) {
            pump_processes(engine);
            assert!(std::time::Instant::now() < deadline, "process never finished");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        pump_processes(engine); // flush any callbacks queued on the last tick
    }

    /// The full async-process contract: streamed chunks land in the buffer
    /// in order, the filter callback sees each appended range, the exit
    /// callback gets the status code, and process-kill terminates a
    /// long-running child (with exit code -1).
    #[test]
    fn process_streaming_end_to_end() {
        let mut engine = build_engine();
        engine
            .compile_and_run_raw_program(
                r#"
                (define *chunks* '())
                (define *exit-code* 'unset)
                (define id
                  (start-process "test" "*out*" ""
                                 "printf 'a\\n'; sleep 0.05; printf 'b\\n'"
                                 (lambda (text start end)
                                   (set! *chunks* (cons (list text start end) *chunks*)))
                                 (lambda (code) (set! *exit-code* code))))
                "#
                .to_string(),
            )
            .unwrap();
        assert!(with_editor(|ed| ed.processes.len() == 1));
        pump_until_done(&mut engine);

        // Chunk boundaries depend on pump timing (one tick may batch both
        // printfs), so assert the order-preserving concatenation and the
        // final range end, not the exact split.
        let result = engine
            .compile_and_run_raw_program(
                r#"(list (equal? *exit-code* 0)
                         (equal? (apply string-append
                                        (map car (reverse *chunks*)))
                                 "a\nb\n")
                         (equal? (car (map (lambda (c) (list-ref c 2)) *chunks*)) 4))"#
                    .to_string(),
            )
            .unwrap();
        assert_eq!(
            format!("{:?}", result.last().unwrap()),
            "'(#true #true #true)",
            "exit code / streamed chunk callbacks mismatch"
        );
        with_editor(|ed| {
            let id = ed.buffer_by_name("*out*").expect("buffer auto-created");
            assert_eq!(ed.buffers[&id].to_string_lossless(), "a\nb\n");
        });

        // Kill: a sleeping child dies with code -1 (signal).
        engine
            .compile_and_run_raw_program(
                r#"
                (define *kill-code* 'unset)
                (define kid (start-process "sleeper" "*sleep*" "" "sleep 30"
                                           #f
                                           (lambda (code) (set! *kill-code* code))))
                "#
                .to_string(),
            )
            .unwrap();
        engine
            .compile_and_run_raw_program("(process-kill kid)".to_string())
            .unwrap();
        pump_until_done(&mut engine);
        let result = engine
            .compile_and_run_raw_program("(equal? *kill-code* -1)".to_string())
            .unwrap();
        assert_eq!(format!("{:?}", result.last().unwrap()), "#true");
    }

    /// The whole compile pipeline, headless: compilation-start streams a
    /// fake compiler's output into *compilation*, the filter recognizes
    /// gnu- and rust-style locations and colors them, the sentinel stamps
    /// the outcome, and next-error jumps to the real file:line:col.
    #[test]
    fn compile_end_to_end() {
        let tmp = std::env::temp_dir().join(format!("taco-compile-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("foo.c"), "int main() {\n  int x;\n  return y;\n}\n").unwrap();

        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);

        // Make the temp dir the editor's default directory.
        with_editor(|ed| {
            let id = ed.create_buffer("anchor", "");
            ed.buffers.get_mut(&id).unwrap().path = Some(tmp.join("anchor"));
            ed.show_buffer(id);
        });

        engine
            .compile_and_run_raw_program(
                r#"(compilation-start
                    "printf 'warming up\\nfoo.c:3:10: error: y undeclared\\nfoo.c:2:7: warning: unused variable\\n'")"#
                    .to_string(),
            )
            .unwrap();
        pump_until_done(&mut engine);

        // Two records parsed, in output order; mode line shows the exit.
        let result = engine
            .compile_and_run_raw_program(
                r#"(list
                     (map (lambda (r) (cdr r)) (results-errors "*compilation*"))
                     (buffer-local-get-in "*compilation*" "compilation-command"))"#
                    .to_string(),
            )
            .unwrap();
        let printed = format!("{:?}", result.last().unwrap());
        assert!(
            printed.contains(r#"("foo.c" 3 10 "error")"#)
                && printed.contains(r#"("foo.c" 2 7 "warning")"#),
            "parsed error records wrong: {printed}"
        );
        with_editor(|ed| {
            let id = ed.buffer_by_name("*compilation*").unwrap();
            let buf = &ed.buffers[&id];
            assert_eq!(buf.mode_name, "Compilation:exit [0]");
            assert!(buf.to_string_lossless().contains("Compilation finished at"));
            assert!(buf.read_only);
            // Both matched "file:line:col:" prefixes got colored.
            assert_eq!(buf.face_spans.len(), 2, "spans: {:?}", buf.face_spans);
            let text = buf.to_string_lossless();
            let (s, e, face) = &buf.face_spans[0];
            assert_eq!(&text[*s..*e], "foo.c:3:10:");
            assert_eq!(face, "compilation-error");
        });

        // next-error from anywhere: first error -> foo.c line 3 col 10.
        engine.compile_and_run_raw_program("(next-error)".to_string()).unwrap();
        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().name, "foo.c");
            let point = ed.windows.selected_ref().point;
            assert_eq!(ed.cur_buffer().char_to_line(point), 2); // line 3, 0-based 2
            let col = point - ed.cur_buffer().line_start(point);
            assert_eq!(col, 9); // col 10, 0-based 9
        });
        engine.compile_and_run_raw_program("(next-error)".to_string()).unwrap();
        with_editor(|ed| {
            let point = ed.windows.selected_ref().point;
            assert_eq!(ed.cur_buffer().char_to_line(point), 1); // second record: line 2
        });
        // Past the last record: stays put with a message.
        engine.compile_and_run_raw_program("(next-error)".to_string()).unwrap();
        with_editor(|ed| {
            assert_eq!(ed.echo.as_deref(), Some("No more errors"));
        });

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// rustc's two-line message shape: the "error[E0308]:" header carries
    /// the severity; the "-->" line below it carries the location. The
    /// python traceback rule is also exercised.
    #[test]
    fn compile_parses_rust_and_python_shapes() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
        load_compile(&mut engine);
        engine
            .compile_and_run_raw_program(
                r#"
                (switch-to-buffer "*grep*")
                (set-buffer-string!
                 (string-append
                  "warning: unused variable: `x`\n"
                  " --> src/lib.rs:7:9\n"
                  "error[E0308]: mismatched types\n"
                  "  --> src/main.rs:12:5\n"
                  "Traceback (most recent call last):\n"
                  "  File \"app.py\", line 30, in <module>\n"
                  "thread 'main' panicked at src/bin/x.rs:3:5:\n"))
                (results-parse-current-buffer! "/proj")
                "#
                .to_string(),
            )
            .unwrap();
        let result = engine
            .compile_and_run_raw_program(
                r#"(map (lambda (r) (cdr r)) (results-errors "*grep*"))"#.to_string(),
            )
            .unwrap();
        let printed = format!("{:?}", result.last().unwrap());
        for expected in [
            r#"("src/lib.rs" 7 9 "warning")"#,
            r#"("src/main.rs" 12 5 "error")"#,
            r#"("app.py" 30 #false "error")"#,
            r#"("src/bin/x.rs" 3 5 "error")"#,
        ] {
            assert!(printed.contains(expected), "missing {expected} in {printed}");
        }
    }

    /// append_to_buffer: no undo, `modified` untouched, and only windows
    /// whose point sat at the old end follow the new output.
    #[test]
    fn append_to_buffer_follows_tail_point() {
        with_editor(|ed| {
            let id = ed.create_buffer("*out*", "abc");
            ed.show_buffer(id);
            ed.windows.selected_mut().point = 3; // at old end: follows
            ed.append_to_buffer(id, "def\n");
            assert_eq!(ed.cur_buffer().to_string_lossless(), "abcdef\n");
            assert_eq!(ed.windows.selected_ref().point, 7);
            assert!(!ed.cur_buffer().modified);

            // Point moved away by the user stays put.
            ed.windows.selected_mut().point = 1;
            ed.append_to_buffer(id, "more");
            assert_eq!(ed.windows.selected_ref().point, 1);

            // Nothing to undo: generated output isn't an edit.
            assert!(ed.cur_buffer_mut().undo_group().is_none());
        });
    }
}
