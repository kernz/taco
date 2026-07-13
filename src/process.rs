//! Background processes: the asynchronous "Native C exception" that M-x
//! compile and friends are built on. Rust owns spawn/read/kill mechanics
//! only — what a process's output *means* is Scheme's business, delivered
//! through the on-output/on-exit callbacks passed to (start-process ...).
//!
//! Each pipe gets a reader thread that only ever touches the pipe and an
//! mpsc Sender: editor state (and the SteelVal callbacks, which are not
//! Send) never leaves the main thread. The Child also stays on the main
//! thread so (process-kill id) can signal it. The main loop polls
//! `scheme::pump_processes` between input events; exits are detected by
//! both pipes reaching EOF and confirmed with try_wait — never a blocking
//! wait.

use crate::buffer::BufferId;
use crate::scheme::with_editor;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use steel::rvals::SteelVal;
use steel::steel_vm::engine::Engine;
use steel::steel_vm::register_fn::RegisterFn;

pub struct ProcessHandle {
    /// Emacs' process name — carried for a future process listing; Rust
    /// itself never branches on it.
    #[allow(dead_code)]
    pub name: String,
    pub buffer: BufferId,
    pub child: Child,
    pub rx: Receiver<String>,
    pub on_output: Option<SteelVal>,
    pub on_exit: Option<SteelVal>,
    /// Both pipe readers hung up — the child exited (or closed its stdio);
    /// reap with try_wait on the next pump.
    pub streams_closed: bool,
    /// Set by (process-kill id): output still in flight is dropped instead
    /// of appended, so a killed compile can't scribble over a buffer that
    /// a restarted one has already regenerated. The exit callback still
    /// runs — whether "killed" is worth announcing is Scheme's call.
    pub discard_output: bool,
}

/// Spawn `sh -c command` in `cwd` with merged-at-chunk-granularity
/// stdout+stderr flowing into one channel (Emacs merges them through a
/// pty; two pipes racing into one channel is the same modulo interleaving
/// at read boundaries).
pub fn spawn(
    name: &str,
    cwd: &str,
    command: &str,
    buffer: BufferId,
    on_output: Option<SteelVal>,
    on_exit: Option<SteelVal>,
) -> std::io::Result<ProcessHandle> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let (tx, rx) = std::sync::mpsc::channel();
    spawn_reader(stdout, tx.clone());
    spawn_reader(stderr, tx); // both senders dropped => rx disconnects
    Ok(ProcessHandle {
        name: name.to_string(),
        buffer,
        child,
        rx,
        on_output,
        on_exit,
        streams_closed: false,
        discard_output: false,
    })
}

/// Read the pipe to EOF in 4K chunks, sending valid UTF-8 slices. A code
/// point can straddle a read boundary, so trailing incomplete bytes are
/// carried into the next read instead of being mangled by a lossy
/// conversion; genuinely invalid bytes (≥ 4 pending can't be a prefix)
/// are flushed lossily rather than held forever.
fn spawn_reader(mut pipe: impl Read + Send + 'static, tx: Sender<String>) {
    std::thread::spawn(move || {
        let mut carry: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    carry.extend_from_slice(&buf[..n]);
                    let valid = match std::str::from_utf8(&carry) {
                        Ok(_) => carry.len(),
                        Err(e) => e.valid_up_to(),
                    };
                    let flush = if valid > 0 {
                        valid
                    } else if carry.len() >= 4 {
                        carry.len() // not a UTF-8 prefix: give up losslessness
                    } else {
                        continue;
                    };
                    let rest = carry.split_off(flush);
                    let s = String::from_utf8_lossy(&carry).into_owned();
                    carry = rest;
                    if tx.send(s).is_err() {
                        return;
                    }
                }
            }
        }
        if !carry.is_empty() {
            let _ = tx.send(String::from_utf8_lossy(&carry).into_owned());
        }
    });
}

fn callback(v: SteelVal) -> Option<SteelVal> {
    match v {
        SteelVal::BoolV(false) => None,
        f => Some(f),
    }
}

pub fn register(engine: &mut Engine) {
    // (start-process name buffer-name cwd command on-output on-exit) -> id
    // or -1 on spawn failure. The buffer is created (not shown) if
    // missing; "" for cwd means the editor's default directory; either
    // callback may be #f. on-output: (lambda (text start end) ...) with
    // char offsets of the appended region; on-exit: (lambda (code) ...),
    // -1 for a signal-killed child.
    engine.register_fn(
        "start-process",
        |name: String,
         buffer_name: String,
         cwd: String,
         command: String,
         on_output: SteelVal,
         on_exit: SteelVal|
         -> isize {
            with_editor(|ed| {
                let buffer = ed
                    .buffer_by_name(&buffer_name)
                    .unwrap_or_else(|| ed.create_buffer(buffer_name.as_str(), ""));
                let cwd = if cwd.is_empty() {
                    ed.default_dir().display().to_string()
                } else {
                    cwd
                };
                match spawn(&name, &cwd, &command, buffer, callback(on_output), callback(on_exit)) {
                    Ok(handle) => {
                        let id = ed.alloc_process_id();
                        ed.processes.insert(id, handle);
                        id as isize
                    }
                    Err(e) => {
                        ed.message(format!("start-process: {e}"));
                        -1
                    }
                }
            })
        },
    );
    engine.register_fn("process-live?", |id: isize| {
        with_editor(|ed| ed.processes.contains_key(&(id.max(0) as u64)))
    });
    // SIGKILL the process; the pump reaps it (and runs its on-exit with
    // code -1) once the pipes drain.
    engine.register_fn("process-kill", |id: isize| -> String {
        with_editor(|ed| {
            match ed.processes.get_mut(&(id.max(0) as u64)) {
                Some(handle) => {
                    handle.discard_output = true;
                    match handle.child.kill() {
                        Ok(()) => String::new(),
                        Err(e) => e.to_string(),
                    }
                }
                None => format!("process-kill: no process {id}"),
            }
        })
    });
}
