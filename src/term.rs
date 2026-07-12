//! Terminal ownership: raw mode + alternate screen, with kitty keyboard
//! enhancement when available (gives unambiguous C-/, C-SPC and friends).
//! Restoration happens on Drop and in a panic hook, so a crash never leaves
//! the user's terminal raw.

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute};
use std::io::stdout;

pub struct TermGuard {
    kitty: bool,
}

impl TermGuard {
    pub fn new() -> std::io::Result<Self> {
        enable_raw_mode()?;
        let kitty = terminal::supports_keyboard_enhancement().unwrap_or(false);
        let mut out = stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        if kitty {
            execute!(
                out,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                )
            )?;
        }
        Ok(TermGuard { kitty })
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        restore(self.kitty);
    }
}

pub fn restore(kitty: bool) {
    let mut out = stdout();
    if kitty {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen, cursor::Show);
    let _ = disable_raw_mode();
}

/// Install a panic hook that restores the terminal before the default hook
/// prints the panic message.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore(true);
        default(info);
    }));
}
