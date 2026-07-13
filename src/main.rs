//! taco — a terminal Emacs. Native core in Rust; configuration and
//! extension in Steel Scheme through a strict API contract.

mod buffer;
mod commands;
mod dispatch;
mod editor;
mod keys;
mod killring;
mod minibuffer;
mod mouse;
mod process;
mod rect;
mod render;
mod scheme;
mod search;
mod term;
mod treesit;
mod undo;
mod window;

use crossterm::event::{Event, KeyCode, KeyEventKind};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let file_arg = std::env::args().nth(1);

    // Engine first: registers the API contract, then the bootstrap keymap and
    // the user's init.scm run through it.
    let mut engine = scheme::build_engine();
    scheme::load_bootstrap(&mut engine);
    scheme::load_compile(&mut engine);
    scheme::load_help(&mut engine);
    scheme::load_man(&mut engine);
    scheme::load_dired(&mut engine);
    scheme::load_rust_mode(&mut engine);
    scheme::load_python_mode(&mut engine);
    scheme::load_c_mode(&mut engine);
    scheme::load_scheme_mode(&mut engine);
    scheme::load_init(&mut engine);

    if let Some(f) = &file_arg {
        scheme::with_editor(|ed| commands::files::find_file_path(ed, Path::new(f)));
        // A directory argument queues (open-dired ...) on ed.deferred (see
        // find_file_path) rather than running it inline — drain it now, or
        // the buffer stays an empty placeholder until the first keypress
        // and that keypress gets dispatched against the wrong buffer. A
        // real file sets ed.file_visited instead, so opening `taco foo.rs`
        // gets tree-sit-enable-for-extension's find-file-hook immediately
        // too, same as process_chord does per keystroke.
        scheme::drain_deferred(&mut engine);
        scheme::fire_find_file_hook(&mut engine);
    }

    term::install_panic_hook();
    let _guard = term::TermGuard::new()?;
    let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
    scheme::with_editor(|ed| ed.term_size = (w, h));

    let mut out = std::io::stdout();
    let mut esc_meta = false;

    // Redraw only when an event actually changed editor state: ignored
    // events (mouse motion, key releases) otherwise cause visible cursor
    // hide/show flicker at drag frequency.
    let mut dirty = true;
    loop {
        if dirty {
            scheme::with_editor(|ed| render::draw(ed, &mut out))?;
            dirty = false;
        }

        // With background processes alive, poll so their output pumps at
        // ~20Hz even while the keyboard is idle; otherwise block as before.
        let has_procs = scheme::with_editor(|ed| !ed.processes.is_empty());
        let ev = if has_procs {
            if crossterm::event::poll(std::time::Duration::from_millis(50))? {
                Some(crossterm::event::read()?)
            } else {
                None
            }
        } else {
            Some(crossterm::event::read()?)
        };

        match ev {
            None => {}
            Some(Event::Key(k)) => {
                if k.kind == KeyEventKind::Release {
                    continue;
                }
                // Bare ESC acts as a Meta prefix for the next key (legacy
                // terminals without a real Alt).
                if k.code == KeyCode::Esc {
                    esc_meta = true;
                    continue;
                }
                let Some(chord) = keys::normalize(&k, esc_meta) else {
                    esc_meta = false;
                    continue;
                };
                esc_meta = false;
                scheme::process_chord(&mut engine, chord);
                dirty = true;
            }
            Some(Event::Mouse(m)) => {
                dirty |= scheme::with_editor(|ed| mouse::handle(ed, m));
            }
            Some(Event::Resize(w, h)) => {
                scheme::with_editor(|ed| ed.term_size = (w, h));
                dirty = true;
            }
            Some(_) => {}
        }

        dirty |= scheme::pump_processes(&mut engine);

        if scheme::with_editor(|ed| ed.quit) {
            break;
        }
    }

    // Don't leave compile children running past the editor.
    scheme::with_editor(|ed| {
        for handle in ed.processes.values_mut() {
            let _ = handle.child.kill();
            let _ = handle.child.wait();
        }
        ed.processes.clear();
    });
    Ok(())
}
