//! The interpreter layer. Scheme never touches editor memory or the
//! terminal: every capability is a Rust function registered here, each
//! taking a short-lived borrow of the thread-local Editor.
//!
//! Invariant: no registered function holds the Editor borrow across a call
//! back into the engine, and the dispatcher never invokes the engine while
//! holding a borrow (see `PostAction`).

use crate::buffer::Mode;
use crate::commands::{self, files, movement};
use crate::editor::{Command, CommandFn, Editor, FaceColor, InputMode, PostAction};
use crate::keys::parse_seq;
use crate::minibuffer;
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
    engine.register_fn("dired", |path: String| {
        with_editor(|ed| {
            let p = ed.resolve_path(&path);
            crate::dired::open_dired(ed, &p);
        })
    });
    engine.register_fn("dired-directory", || {
        with_editor(|ed| match &ed.cur_buffer().mode {
            Mode::Dired(d) => d.dir.display().to_string(),
            _ => String::new(),
        })
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
    // spans (region, isearch/query-replace match, current line number in
    // the gutter). Unset faces keep the default reverse-video look.
    engine.register_fn("set-face-color", |face: String, color: String| {
        with_editor(|ed| set_face_color(ed, &face, &color))
    });

    // 3. Keymap and command definition — how Scheme extends the editor.
    engine.register_fn("global-set-key", |seq: String, cmd: String| {
        with_editor(|ed| bind_in(ed, MapKind::Global, &seq, &cmd))
    });
    engine.register_fn("dired-set-key", |seq: String, cmd: String| {
        with_editor(|ed| bind_in(ed, MapKind::Dired, &seq, &cmd))
    });
    engine.register_fn("wgrep-set-key", |seq: String, cmd: String| {
        with_editor(|ed| bind_in(ed, MapKind::Wgrep, &seq, &cmd))
    });
    engine.register_fn("minibuffer-set-key", |seq: String, cmd: String| {
        with_editor(|ed| bind_in(ed, MapKind::Minibuffer, &seq, &cmd))
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

    engine
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
        _ => ed.message(format!("set-face-color: unknown face {face:?}")),
    }
}

enum MapKind {
    Global,
    Dired,
    Wgrep,
    Minibuffer,
}

fn bind_in(ed: &mut Editor, map: MapKind, seq: &str, cmd: &str) {
    let Some(chords) = parse_seq(seq) else {
        ed.message(format!("global-set-key: cannot parse key sequence {seq:?}"));
        return;
    };
    if chords.is_empty() {
        ed.message(format!("global-set-key: empty key sequence {seq:?}"));
        return;
    }
    // The minibuffer map is consulted one chord at a time (minibuffer.rs),
    // so only single-chord bindings can ever fire there.
    if matches!(map, MapKind::Minibuffer) && chords.len() > 1 {
        ed.message(format!(
            "minibuffer-set-key: only single keys are supported, got {seq:?}"
        ));
        return;
    }
    let target = match map {
        MapKind::Global => &mut ed.global_map,
        MapKind::Dired => &mut ed.dired_map,
        MapKind::Wgrep => &mut ed.wgrep_map,
        MapKind::Minibuffer => &mut ed.minibuffer_map,
    };
    target.bind(&chords, cmd);
}

/// The default keybindings, expressed through the same public contract that
/// user config uses (Emacs' C-core / Lisp-layer split).
pub fn load_bootstrap(engine: &mut Engine) {
    run(engine, include_str!("bootstrap.scm"), "bootstrap.scm");
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
pub fn call_scheme_command(engine: &mut Engine, f: SteelVal) {
    if let Err(e) = engine.call_function_with_args(f, Vec::new()) {
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
        PostAction::RunScheme(f) => call_scheme_command(engine, f),
    }
    drain_deferred(engine);
    match (was_prompt, prompt_active()) {
        (false, true) => call_run_hooks(engine, "minibuffer-setup-hook"),
        (true, true) => call_run_hooks(engine, "post-command-hook"),
        (true, false) => call_run_hooks(engine, "minibuffer-exit-hook"),
        (false, false) => {}
    }
    drain_deferred(engine);
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
            Some(PostAction::RunScheme(f)) => call_scheme_command(engine, f),
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

    /// End-to-end contract test, headless: bootstrap builds the keymap
    /// through the public API, Scheme edits the buffer, defines a command,
    /// binds it, and the dispatcher runs it.
    #[test]
    fn steel_contract_end_to_end() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);

        // Bootstrap populated the keymaps through (global-set-key ...).
        with_editor(|ed| {
            assert_eq!(
                ed.global_map.lookup(&parse_seq("C-x C-f").unwrap()),
                Lookup::Cmd("find-file".into())
            );
            assert_eq!(
                ed.dired_map.lookup(&parse_seq("% m").unwrap()),
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
            PostAction::RunScheme(f) => call_scheme_command(&mut engine, f),
            _ => panic!("C-c s did not resolve to the Scheme command"),
        }

        with_editor(|ed| {
            assert_eq!(ed.cur_buffer().to_string_lossless(), "hello WORLD");
        });
    }

    /// The vertico plugin, headless, through the exact main-loop code path:
    /// M-x pops the full candidate list, typing narrows it live, C-n/C-p
    /// cycle with wrap-around, RET submits the selected candidate.
    #[test]
    fn vertico_plugin_end_to_end() {
        let mut engine = build_engine();
        load_bootstrap(&mut engine);
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
}
