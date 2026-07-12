# taco

A terminal-only Emacs clone. Native core in **Rust** (crossterm + ropey), configuration and
extension in **Steel Scheme** (`steel-core` crate) through a strict API contract — the same
C-core / Lisp-layer split as real Emacs. No GUI, no vim/evil bindings, no features beyond
the spec below.

```
cargo run [file]        # build & run (debug startup ~1s = Steel engine init)
cargo test              # 31 unit + integration tests
```

## Status

Everything below is **implemented and verified** (unit tests + driving the real binary in a
PTY harness). Build is warning-free. ~4.7k lines.

## Architecture

Two strict layers:

1. **Native core (Rust)** — owns all state and the terminal. Scheme can never touch memory
   or terminal output directly.
2. **Interpreter layer (Steel)** — `src/scheme/bootstrap.scm` (embedded, defines the whole
   default keymap via the public API) then `~/.config/taco/init.scm` (user config).

### Module map (`src/`)

| File | Role |
|---|---|
| `main.rs` | event loop: engine → bootstrap → init.scm → raw-mode loop |
| `editor.rs` | `Editor` struct: buffers, windows, kill ring, registry, input modes, options |
| `buffer.rs` | ropey `Rope` + path/modified/mark/undo/mode; char indices everywhere |
| `undo.rs` | command-grouped undo log (Boundary / Insert / Delete records) |
| `window.rs` | binary split tree, per-window point + scroll, layout → rects |
| `keys.rs` | `Chord {ctrl, meta, key}`, `"C-x C-f"` string parsing, crossterm normalization |
| `dispatch.rs` | prefix-trie keymap walk, pending echo (`C-x -`), C-g, C-u state machine |
| `commands/` | all native commands: `fn(&mut Editor, Option<u32>)` (mod/movement/editing/ring/windows/files/help) |
| `minibuffer.rs` | prompt state machine (`PromptKind` enum): minibuffer keymap → built-in line editing |
| `mouse.rs` | click → window/point mapping (gutter-aware), wheel → 3-line viewport scroll |
| `search.rs` | incremental regexp isearch + query-replace loop |
| `rect.rs` | rectangle mode |
| `killring.rs` | ring + yank rotation |
| `dired/` | listing buffer + buffer-local keymap; `wgrep.rs` = writable bulk rename |
| `render.rs` | full-frame redraw: windows, mode lines, region/match highlights, gutter, echo line |
| `term.rs` | RAII raw mode + alt screen + kitty keyboard flags; panic-safe restore |
| `scheme/mod.rs` | **the contract**: thread-local Editor + every `register_fn` |

### The three invariants (don't break these)

1. `Editor` lives in a `thread_local RefCell` (`scheme/mod.rs`). Every API fn takes a
   short-lived borrow via `with_editor(|ed| ...)`.
2. **Never call the Steel engine while holding an Editor borrow.** The dispatcher returns
   `PostAction::RunScheme` and the main loop (which owns the engine) performs the call
   afterwards. Native fns that run *inside* the engine (e.g. `exit-minibuffer`) queue any
   follow-up action on `Editor::deferred`; the main loop drains it after every engine call.
3. Dispatch is name-based: keymap → command name (String) → registry entry
   (`Native(fn)` or `Scheme(SteelVal closure)` + docstring). `C-h k` / `C-h f` / M-x read
   from the registry.

## Keybindings (the complete spec — do not add more)

**System**: `C-x C-c` quit · `C-g` cancel · `C-x C-s` save · `C-x b` switch buffer ·
`C-x k` kill buffer · `C-x C-f` open file · `C-/` undo · `M-x` run command

**Minibuffer**: full line editing inside every prompt (`C-b`/`C-f`/`C-a`/`C-e`/
`Backspace`/`C-d`/`C-k`). No built-in completion UI — that is a plugin
(`examples/vertico.scm`): vertical candidates under the prompt (≤6 rows), `C-n`/`C-p`
cycle, `RET` submits the selection, `TAB` inserts it.

**Mouse**: left click selects the window under the pointer and moves point to the clicked
glyph (gutter/split aware); the wheel scrolls the window under the pointer by 3 lines
without moving point unless it would leave the view.

**Movement**: `M-<`/`M->` buffer start/end · `C-v`/`M-v` page · `C-l` recenter ·
`C-a`/`C-e` line start/end · `C-n`/`C-p` lines · `M-g g` goto line · `M-f`/`M-b` words ·
`C-f`/`C-b` chars

**Search/edit**: `C-s`/`C-r` incremental regexp search (repeat for next) · `M-%`
query-replace (y/n/!/q, `$1` capture refs) · `TAB` indent · `C-j` newline+indent ·
`M-\` delete surrounding whitespace · `C-o` open-line (newline after point, cursor stays) ·
`C-d` delete char · `M-backspace` kill word back ·
`C-x SPC` rectangle mode → `C-x r t` insert into rectangle

**Kill ring**: `C-SPC` set mark · `M-w` copy · `C-w` kill · `C-y` yank · `M-y` yank-pop ·
`M-d` kill word · `C-k` kill line (consecutive kills append)

**Format/windows**: `C-t` transpose chars · `M-u`/`M-l` up/downcase word ·
`C-u n char` insert n copies · `C-x o`/`0`/`1`/`2`/`3` windows · `C-h k`/`C-h f` describe

**Dired**: enter via `C-x C-j` (dired-jump: current file's directory, cursor on the file),
`C-c f d` (prompt), `C-c o -` (current dir), `C-c p D` (project root = nearest `.git`).
Listings are `ls -la`-shaped (permissions, links, owner, group, size, mtime) with a `..`
entry at the top — `RET` on it changes to the parent. Inside: `RET` visit · `o` visit
other window · `^` up ·
`m`/`u`/`U` mark/unmark/unmark-all · `% m` mark by regexp · `d`+`x` flag+delete ·
`D` delete · `R` rename · `C` copy · `+` mkdir · `=` diff · `Z` compress (gz / tar.gz) ·
`g` refresh · `)` toggle hidden · `!` shell on marked · `q` kill all dired buffers.
Wgrep: `C-c C-e` editable → `C-c C-c` commit renames / `C-c C-k` abort.

## The Scheme contract

Auto-registered: **every native command** as a zero-arg fn (`(forward-char)`,
`(kill-line)`, `(split-window-below)`, ...). Plus:

- Text/state: `(insert-text s)` `(point)` `(goto-char n)` `(buffer-string)` `(line-number)`
  `(current-buffer)` `(buffer-modified?)` `(buffer-list)` `(find-file path)`
  `(switch-to-buffer name)` `(goto-line n)` `(message s)` `(dired path)` `(dired-directory)`
- Extension: `(define-command name doc lambda)` · `(global-set-key "C-c x" "name")` ·
  `(dired-set-key ...)` `(wgrep-set-key ...)` `(minibuffer-set-key ...)` (single keys)
- Minibuffer (Emacs names where they exist): `(minibufferp)` `(minibuffer-prompt)`
  `(minibuffer-contents)` `(set-minibuffer-contents s)` `(delete-minibuffer-contents)`
  `(minibuffer-completion-kind)` → `"command"|"buffer"|"file"|""` · `(exit-minibuffer)` ·
  `(minibuffer-show-candidates lst idx)` (drives the native ≤6-row vertical list)
- Completion sources: `(command-names)` `(buffer-names)` `(directory-files dir)`
  `(default-directory)`
- Hooks (bootstrap.scm): `(add-hook name f)` `(remove-hook name)` `(run-hooks name)`.
  The core fires `"minibuffer-setup-hook"`, `"post-command-hook"` (per key while a prompt
  is active) and `"minibuffer-exit-hook"`.
- Options: `(set-option "display-line-numbers" #t)` · `(get-option name)`

**Completion UIs live entirely in Scheme.** `examples/vertico.scm` is the reference
plugin: it hangs candidate computation off the hooks above and rebinds `C-n`/`C-p`/`RET`/
`TAB` in the minibuffer keymap. Install it by copying the file's contents into
`~/.config/taco/init.scm`.

### Example `~/.config/taco/init.scm`

```scheme
(set-option "display-line-numbers" #t)

(define-command "toggle-line-numbers" "Toggle the line-number gutter"
  (lambda () (set-option "display-line-numbers"
                         (not (get-option "display-line-numbers")))))
(global-set-key "C-c n" "toggle-line-numbers")

(define-command "duplicate-line" "Duplicate the current line"
  (lambda ()
    (beginning-of-line) (kill-line) (yank) (newline) (yank)))
(global-set-key "C-c d" "duplicate-line")
```

## Testing

- `cargo test` — keymap trie, word motion, kill ring, undo, rectangle, search offsets
  (unicode), wgrep renames on a real tempdir, dired `ls -la`/`..` on a tempdir, mouse
  click/scroll mapping, minibuffer line editing, and headless Steel contract tests —
  including the whole vertico plugin driven through the real main-loop code path
  (`scheme::process_chord`).
- End-to-end: Python PTY harness (drives the real binary, reconstructs the screen).
  Gotchas if rebuilding one: set `TIOCSWINSZ` or the terminal is 0×0; wait ~2s before
  sending keys (engine init + canonical-mode buffering).
- No clippy on this machine (Fedora cargo without rustup).

## Known limitations / next candidates

- Non-kitty terminals: `C-j` is byte-identical to `RET`; kitty protocol terminals are clean
  (flags pushed automatically). Legacy aliases handled: `C-/`→`C-_`(0x1F), `C-SPC`→NUL,
  ESC prefix = Meta.
- Long lines truncate with `$` (no wrap). Tabs display at 8; indent is
  previous-line-relative, else 4 spaces.
- wgrep tracks renames by line position (reordering lines isn't supported).
- Scheme-defined commands aren't yet callable from other Scheme code (only via keys/M-x);
  a `(call-command "name")` bridge would close that.
- Undo groups per command (typing runs coalesce); no redo.
