//! taco — a terminal Emacs. Native core in Rust; configuration and
//! extension in Steel Scheme through a strict API contract.

mod buffer;
mod commands;
mod dired;
mod dispatch;
mod editor;
mod keys;
mod killring;
mod minibuffer;
mod mouse;
mod rect;
mod render;
mod scheme;
mod search;
mod term;
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
    scheme::load_init(&mut engine);

    if let Some(f) = &file_arg {
        scheme::with_editor(|ed| commands::files::find_file_path(ed, Path::new(f)));
    }

    term::install_panic_hook();
    let _guard = term::TermGuard::new()?;
    let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
    scheme::with_editor(|ed| ed.term_size = (w, h));

    let mut out = std::io::stdout();
    let mut esc_meta = false;

    loop {
        scheme::with_editor(|ed| render::draw(ed, &mut out))?;

        match crossterm::event::read()? {
            Event::Key(k) => {
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
            }
            Event::Mouse(m) => scheme::with_editor(|ed| mouse::handle(ed, m)),
            Event::Resize(w, h) => scheme::with_editor(|ed| ed.term_size = (w, h)),
            _ => {}
        }

        if scheme::with_editor(|ed| ed.quit) {
            break;
        }
    }
    Ok(())
}
