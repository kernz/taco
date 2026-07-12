//! Dired: a directory listing rendered into a read-only buffer, with a
//! buffer-local keymap. Lines map 1:1 to entries; line 0 is the header.

pub mod wgrep;

use crate::buffer::{BufferId, Mode};
use crate::editor::{Editor, PromptKind, YesNoAction};
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// ' ' none, '*' marked, 'D' flagged for deletion.
    pub mark: char,
    // ls -la metadata (zeroed when the entry cannot be stat'ed).
    pub mode: u32,
    pub nlink: u64,
    pub owner: String,
    pub group: String,
    pub size: u64,
    /// Modification time, seconds since the epoch.
    pub mtime: i64,
}

#[derive(Debug)]
pub struct DiredState {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub show_hidden: bool,
    /// Column where the file name starts on every listing line; depends on
    /// the metadata column widths, so it is recomputed with the text.
    pub name_col: usize,
    /// wgrep snapshot: for each buffer line, the path it named when editing
    /// began (None for the header line).
    pub wgrep: Option<Vec<Option<PathBuf>>>,
}

/// uid/gid -> name tables parsed from /etc/passwd / /etc/group ("name:x:id:").
/// Missing or unparsable files just leave numeric ids in the listing.
fn id_names(path: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    if let Ok(s) = std::fs::read_to_string(path) {
        for line in s.lines() {
            let mut it = line.split(':');
            let (Some(name), _, Some(id)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            if let Ok(id) = id.parse() {
                map.entry(id).or_insert_with(|| name.to_string());
            }
        }
    }
    map
}

fn make_entry(
    name: String,
    path: PathBuf,
    users: &HashMap<u32, String>,
    groups: &HashMap<u32, String>,
) -> Entry {
    // The entry itself, symlinks not followed (as ls -la lists them); is_dir
    // still follows so navigating into directory symlinks keeps working.
    let md = std::fs::symlink_metadata(&path).ok();
    let (mode, nlink, uid, gid, size, mtime) = md
        .map(|m| (m.mode(), m.nlink(), m.uid(), m.gid(), m.size(), m.mtime()))
        .unwrap_or((0, 0, 0, 0, 0, 0));
    Entry {
        is_dir: path.is_dir(),
        name,
        path,
        mark: ' ',
        mode,
        nlink,
        owner: users.get(&uid).cloned().unwrap_or_else(|| uid.to_string()),
        group: groups.get(&gid).cloned().unwrap_or_else(|| gid.to_string()),
        size,
        mtime,
    }
}

fn list_dir(dir: &Path, show_hidden: bool) -> Result<Vec<Entry>> {
    let users = id_names("/etc/passwd");
    let groups = id_names("/etc/group");
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        entries.push(make_entry(name, item.path(), &users, &groups));
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    // The parent tracking entry heads the listing (always shown, like ls -la);
    // RET on it goes through the ordinary directory change path.
    if let Some(parent) = dir.parent() {
        entries.insert(0, make_entry("..".into(), parent.to_path_buf(), &users, &groups));
    }
    Ok(entries)
}

/// "drwxr-xr-x" from a unix st_mode.
fn mode_string(mode: u32, is_dir: bool) -> String {
    let type_ch = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o140000 => 's',
        0o060000 => 'b',
        0o020000 => 'c',
        0o010000 => 'p',
        0 if is_dir => 'd',
        _ => '-',
    };
    let mut s = String::with_capacity(10);
    s.push(type_ch);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// "Jan 12 03:45" (UTC, fixed 12 columns).
fn mtime_string(secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, m, d) = civil_from_days(secs.div_euclid(86400));
    let rem = secs.rem_euclid(86400);
    format!("{} {:>2} {:02}:{:02}", MONTHS[(m - 1) as usize], d, rem / 3600, (rem % 3600) / 60)
}

/// Days since 1970-01-01 -> (year, month, day). Howard Hinnant's
/// civil_from_days algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The buffer text for a listing, ls -la style, plus the column where file
/// names start (metadata columns are width-aligned per listing).
fn listing_text(dir: &Path, entries: &[Entry]) -> (String, usize) {
    let width = |f: fn(&Entry) -> usize| entries.iter().map(f).max().unwrap_or(1).max(1);
    let lw = width(|e| e.nlink.to_string().len());
    let ow = width(|e| e.owner.len());
    let gw = width(|e| e.group.len());
    let sw = width(|e| e.size.to_string().len());
    let name_col = 2 + 10 + 1 + lw + 1 + ow + 1 + gw + 1 + sw + 1 + 12 + 1;
    let mut text = format!("  {}:", dir.display());
    for e in entries {
        text.push_str(&format!(
            "\n{} {} {:>lw$} {:<ow$} {:<gw$} {:>sw$} {} {}",
            e.mark,
            mode_string(e.mode, e.is_dir),
            e.nlink,
            e.owner,
            e.group,
            e.size,
            mtime_string(e.mtime),
            e.name,
        ));
    }
    (text, name_col)
}

/// Rebuild the buffer text from the entry list, keeping point on its line.
fn regenerate(ed: &mut Editor) {
    let (win, buf) = ed.cur();
    let Mode::Dired(state) = &mut buf.mode else { return };
    let (text, name_col) = listing_text(&state.dir, &state.entries);
    state.name_col = name_col;
    let line = buf.char_to_line(win.point.min(buf.len_chars()));
    buf.rope = ropey::Rope::from_str(&text);
    buf.modified = false;
    buf.read_only = true;
    let line = line.min(buf.len_lines().saturating_sub(1));
    win.point = buf.line_to_char(line);
}

/// Re-read the directory, preserving marks by file name.
fn refresh(ed: &mut Editor) {
    let buf = ed.cur_buffer_mut();
    let Mode::Dired(state) = &mut buf.mode else { return };
    match list_dir(&state.dir, state.show_hidden) {
        Ok(mut fresh) => {
            for e in &mut fresh {
                if let Some(old) = state.entries.iter().find(|o| o.name == e.name) {
                    e.mark = old.mark;
                }
            }
            state.entries = fresh;
            regenerate(ed);
        }
        Err(e) => ed.message(format!("{e:#}")),
    }
}

/// Open (or reuse) a dired buffer for `dir` in the selected window.
pub fn open_dired(ed: &mut Editor, dir: &Path) {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let existing = ed.buffers.values().find_map(|b| match &b.mode {
        Mode::Dired(d) if d.dir == dir => Some(b.id),
        _ => None,
    });
    if let Some(id) = existing {
        ed.show_buffer(id);
        refresh(ed);
        return;
    }
    let entries = match list_dir(&dir, false) {
        Ok(e) => e,
        Err(e) => {
            ed.message(format!("{e:#}"));
            return;
        }
    };
    let (text, name_col) = listing_text(&dir, &entries);
    let name = dir.display().to_string();
    let id = ed.create_buffer(name, &text);
    {
        let buf = ed.buffers.get_mut(&id).unwrap();
        buf.mode = Mode::Dired(DiredState {
            dir,
            entries,
            show_hidden: false,
            name_col,
            wgrep: None,
        });
        buf.read_only = true;
    }
    ed.show_buffer(id);
}

/// Entry index under point (line - 1), if point is on an entry line.
fn entry_index(ed: &Editor) -> Option<usize> {
    let win = ed.windows.selected_ref();
    let buf = ed.cur_buffer();
    let Mode::Dired(state) = &buf.mode else { return None };
    let line = buf.char_to_line(win.point.min(buf.len_chars()));
    (line >= 1 && line <= state.entries.len()).then(|| line - 1)
}

fn entry_at_point(ed: &Editor) -> Option<Entry> {
    let idx = entry_index(ed)?;
    let Mode::Dired(state) = &ed.cur_buffer().mode else { return None };
    state.entries.get(idx).cloned()
}

fn dired_state(ed: &Editor) -> Option<&DiredState> {
    match &ed.cur_buffer().mode {
        Mode::Dired(d) => Some(d),
        _ => None,
    }
}

fn require_dired(ed: &mut Editor) -> bool {
    if dired_state(ed).is_some() {
        true
    } else {
        ed.message("Not a dired buffer");
        false
    }
}

/// Marking, deleting, renaming etc. never apply to the parent entry.
fn refuse_dotdot(ed: &mut Editor, entry: &Entry) -> bool {
    if entry.name == ".." {
        ed.message("Cannot operate on `..'");
        return true;
    }
    false
}

fn set_mark_at_point(ed: &mut Editor, mark: char) {
    let Some(idx) = entry_index(ed) else {
        ed.message("No file on this line");
        return;
    };
    if let Some(entry) = entry_at_point(ed) {
        if refuse_dotdot(ed, &entry) {
            return;
        }
    }
    {
        let buf = ed.cur_buffer_mut();
        let Mode::Dired(state) = &mut buf.mode else { return };
        state.entries[idx].mark = mark;
    }
    regenerate(ed);
    crate::commands::movement::next_line(ed, None);
}

fn show_in_other_window(ed: &mut Editor, id: BufferId) {
    if ed.windows.leaves().len() == 1 {
        ed.windows.split(crate::window::SplitDir::Below);
    }
    ed.windows.select_next();
    ed.show_buffer(id);
}

// ---- entry points (global bindings) -----------------------------------------

pub fn open_dir_cmd(ed: &mut Editor, _n: Option<u32>) {
    let mut prefill = ed.default_dir().display().to_string();
    if !prefill.ends_with('/') {
        prefill.push('/');
    }
    ed.prompt_prefilled(PromptKind::DiredOpenDir, "Dired (directory): ", prefill);
}

pub fn current_cmd(ed: &mut Editor, _n: Option<u32>) {
    let dir = ed.default_dir();
    open_dired(ed, &dir);
}

/// C-x C-j: dired on the current buffer's directory, cursor on its file.
pub fn jump_cmd(ed: &mut Editor, _n: Option<u32>) {
    let from = ed.cur_buffer().path.clone();
    let dir = ed.default_dir();
    open_dired(ed, &dir);
    let Some(name) = from.and_then(|p| {
        p.file_name().map(|n| n.to_string_lossy().into_owned())
    }) else {
        return;
    };
    goto_entry(ed, &name);
}

/// Move point to the name of entry `name` in the current dired buffer.
fn goto_entry(ed: &mut Editor, name: &str) {
    let target = {
        let buf = ed.cur_buffer();
        let Mode::Dired(state) = &buf.mode else { return };
        state
            .entries
            .iter()
            .position(|e| e.name == name)
            .map(|idx| buf.line_to_char(idx + 1) + state.name_col)
    };
    if let Some(point) = target {
        let (win, buf) = ed.cur();
        win.point = point.min(buf.len_chars());
    }
}

pub fn project_root_cmd(ed: &mut Editor, _n: Option<u32>) {
    let mut dir = ed.default_dir();
    loop {
        if dir.join(".git").exists() {
            open_dired(ed, &dir.clone());
            return;
        }
        if !dir.pop() {
            break;
        }
    }
    ed.message("No project root found (no .git in any ancestor)");
}

// ---- navigation & marking ----------------------------------------------------

pub fn find_file_cmd(ed: &mut Editor, _n: Option<u32>) {
    let Some(entry) = entry_at_point(ed) else {
        ed.message("No file on this line");
        return;
    };
    if entry.is_dir {
        open_dired(ed, &entry.path);
    } else {
        crate::commands::files::find_file_path(ed, &entry.path);
    }
}

pub fn find_file_other_window_cmd(ed: &mut Editor, _n: Option<u32>) {
    let Some(entry) = entry_at_point(ed) else {
        ed.message("No file on this line");
        return;
    };
    if ed.windows.leaves().len() == 1 {
        ed.windows.split(crate::window::SplitDir::Below);
    }
    ed.windows.select_next();
    if entry.is_dir {
        open_dired(ed, &entry.path);
    } else {
        crate::commands::files::find_file_path(ed, &entry.path);
    }
}

pub fn up_directory_cmd(ed: &mut Editor, _n: Option<u32>) {
    let Some(state) = dired_state(ed) else {
        ed.message("Not a dired buffer");
        return;
    };
    match state.dir.parent() {
        Some(parent) => open_dired(ed, &parent.to_path_buf()),
        None => ed.message("At filesystem root"),
    }
}

pub fn mark_cmd(ed: &mut Editor, _n: Option<u32>) {
    set_mark_at_point(ed, '*');
}

pub fn unmark_cmd(ed: &mut Editor, _n: Option<u32>) {
    set_mark_at_point(ed, ' ');
}

pub fn flag_deletion_cmd(ed: &mut Editor, _n: Option<u32>) {
    set_mark_at_point(ed, 'D');
}

pub fn unmark_all_cmd(ed: &mut Editor, _n: Option<u32>) {
    if !require_dired(ed) {
        return;
    }
    {
        let buf = ed.cur_buffer_mut();
        let Mode::Dired(state) = &mut buf.mode else { return };
        for e in &mut state.entries {
            e.mark = ' ';
        }
    }
    regenerate(ed);
    ed.message("All marks removed");
}

pub fn mark_regexp_cmd(ed: &mut Editor, _n: Option<u32>) {
    if !require_dired(ed) {
        return;
    }
    ed.prompt(PromptKind::DiredMarkRegex, "Mark files (regexp): ");
}

pub fn mark_regexp(ed: &mut Editor, pattern: &str) {
    let re = match Regex::new(pattern) {
        Ok(re) => re,
        Err(e) => {
            ed.message(format!("Invalid regexp: {e}"));
            return;
        }
    };
    let mut count = 0;
    {
        let buf = ed.cur_buffer_mut();
        let Mode::Dired(state) = &mut buf.mode else { return };
        for e in &mut state.entries {
            if e.name != ".." && re.is_match(&e.name) {
                e.mark = '*';
                count += 1;
            }
        }
    }
    regenerate(ed);
    ed.message(format!("{count} files marked"));
}

// ---- file IO ------------------------------------------------------------------

pub fn do_delete_cmd(ed: &mut Editor, _n: Option<u32>) {
    let Some(entry) = entry_at_point(ed) else {
        ed.message("No file on this line");
        return;
    };
    if refuse_dotdot(ed, &entry) {
        return;
    }
    ed.prompt(
        PromptKind::YesNo(YesNoAction::DiredDelete(vec![entry.path])),
        format!("Delete {}? (y or n) ", entry.name),
    );
}

pub fn do_flagged_delete_cmd(ed: &mut Editor, _n: Option<u32>) {
    let Some(state) = dired_state(ed) else {
        ed.message("Not a dired buffer");
        return;
    };
    let flagged: Vec<PathBuf> = state
        .entries
        .iter()
        .filter(|e| e.mark == 'D')
        .map(|e| e.path.clone())
        .collect();
    if flagged.is_empty() {
        ed.message("(No deletions requested)");
        return;
    }
    let n = flagged.len();
    ed.prompt(
        PromptKind::YesNo(YesNoAction::DiredDelete(flagged)),
        format!("Delete {n} file{}? (y or n) ", if n == 1 { "" } else { "s" }),
    );
}

pub fn delete_paths(ed: &mut Editor, paths: &[PathBuf]) {
    let mut errors = Vec::new();
    for path in paths {
        let res = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        if let Err(e) = res {
            errors.push(format!("{}: {e}", path.display()));
        }
    }
    refresh(ed);
    if errors.is_empty() {
        ed.message(format!("Deleted {} file(s)", paths.len()));
    } else {
        ed.message(errors.join("; "));
    }
}

pub fn do_rename_cmd(ed: &mut Editor, _n: Option<u32>) {
    let Some(entry) = entry_at_point(ed) else {
        ed.message("No file on this line");
        return;
    };
    if refuse_dotdot(ed, &entry) {
        return;
    }
    ed.prompt_prefilled(
        PromptKind::DiredRename { from: entry.path },
        format!("Rename {} to: ", entry.name),
        entry.name.clone(),
    );
}

pub fn rename_to(ed: &mut Editor, from: &Path, to: &str) {
    let dest = ed.resolve_path(to);
    match std::fs::rename(from, &dest) {
        Ok(()) => {
            refresh(ed);
            ed.message(format!("Renamed to {}", dest.display()));
        }
        Err(e) => ed.message(format!("Rename failed: {e}")),
    }
}

pub fn do_copy_cmd(ed: &mut Editor, _n: Option<u32>) {
    let Some(entry) = entry_at_point(ed) else {
        ed.message("No file on this line");
        return;
    };
    if refuse_dotdot(ed, &entry) {
        return;
    }
    ed.prompt(
        PromptKind::DiredCopy { from: entry.path },
        format!("Copy {} to: ", entry.name),
    );
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for item in std::fs::read_dir(from)? {
            let item = item?;
            copy_recursive(&item.path(), &to.join(item.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

pub fn copy_to(ed: &mut Editor, from: &Path, to: &str) {
    let dest = ed.resolve_path(to);
    match copy_recursive(from, &dest) {
        Ok(()) => {
            refresh(ed);
            ed.message(format!("Copied to {}", dest.display()));
        }
        Err(e) => ed.message(format!("Copy failed: {e}")),
    }
}

pub fn create_directory_cmd(ed: &mut Editor, _n: Option<u32>) {
    ed.prompt(PromptKind::DiredMkdir, "Create directory: ");
}

pub fn mkdir(ed: &mut Editor, name: &str) {
    let dest = ed.resolve_path(name);
    match std::fs::create_dir_all(&dest) {
        Ok(()) => {
            refresh(ed);
            ed.message(format!("Created {}", dest.display()));
        }
        Err(e) => ed.message(format!("mkdir failed: {e}")),
    }
}

pub fn diff_cmd(ed: &mut Editor, _n: Option<u32>) {
    let Some(entry) = entry_at_point(ed) else {
        ed.message("No file on this line");
        return;
    };
    ed.prompt(
        PromptKind::DiredDiff { from: entry.path },
        format!("Diff {} against: ", entry.name),
    );
}

pub fn diff_against(ed: &mut Editor, from: &Path, other: &str) {
    let other = ed.resolve_path(other);
    let (a, b) = match (
        std::fs::read_to_string(&other),
        std::fs::read_to_string(from),
    ) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            ed.message(format!("Cannot diff: {e}"));
            return;
        }
    };
    let diff = similar::TextDiff::from_lines(&a, &b)
        .unified_diff()
        .header(&other.display().to_string(), &from.display().to_string())
        .to_string();
    let text = if diff.is_empty() { "(no differences)".to_string() } else { diff };
    let id = match ed.buffer_by_name("*diff*") {
        Some(id) => {
            let buf = ed.buffers.get_mut(&id).unwrap();
            buf.rope = ropey::Rope::from_str(&text);
            buf.modified = false;
            id
        }
        None => ed.create_buffer("*diff*", &text),
    };
    ed.buffers.get_mut(&id).unwrap().read_only = true;
    show_in_other_window(ed, id);
}

pub fn compress_cmd(ed: &mut Editor, _n: Option<u32>) {
    let Some(entry) = entry_at_point(ed) else {
        ed.message("No file on this line");
        return;
    };
    if refuse_dotdot(ed, &entry) {
        return;
    }
    let result = if entry.is_dir {
        compress_dir(&entry.path)
    } else {
        compress_file(&entry.path)
    };
    match result {
        Ok(out) => {
            refresh(ed);
            ed.message(format!("Compressed to {}", out.display()));
        }
        Err(e) => ed.message(format!("Compress failed: {e:#}")),
    }
}

fn compress_file(path: &Path) -> Result<PathBuf> {
    use std::io::Write;
    let out_path = PathBuf::from(format!("{}.gz", path.display()));
    let input = std::fs::read(path)?;
    let out = std::fs::File::create(&out_path)?;
    let mut enc = flate2::write::GzEncoder::new(out, flate2::Compression::default());
    enc.write_all(&input)?;
    enc.finish()?;
    Ok(out_path)
}

fn compress_dir(path: &Path) -> Result<PathBuf> {
    let out_path = PathBuf::from(format!("{}.tar.gz", path.display()));
    let out = std::fs::File::create(&out_path)?;
    let enc = flate2::write::GzEncoder::new(out, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);
    let base = path.file_name().unwrap_or_default();
    tar.append_dir_all(base, path)?;
    tar.into_inner()?.finish()?;
    Ok(out_path)
}

pub fn revert_cmd(ed: &mut Editor, _n: Option<u32>) {
    if !require_dired(ed) {
        return;
    }
    refresh(ed);
    ed.message("Directory re-read");
}

pub fn toggle_hidden_cmd(ed: &mut Editor, _n: Option<u32>) {
    if !require_dired(ed) {
        return;
    }
    let showing = {
        let buf = ed.cur_buffer_mut();
        let Mode::Dired(state) = &mut buf.mode else { return };
        state.show_hidden = !state.show_hidden;
        state.show_hidden
    };
    refresh(ed);
    ed.message(if showing { "Showing hidden files" } else { "Hiding hidden files" });
}

pub fn shell_command_cmd(ed: &mut Editor, _n: Option<u32>) {
    if !require_dired(ed) {
        return;
    }
    ed.prompt(PromptKind::DiredShell, "! on marked files (shell command): ");
}

pub fn run_shell(ed: &mut Editor, cmd: &str) {
    let Some(state) = dired_state(ed) else { return };
    let mut targets: Vec<String> = state
        .entries
        .iter()
        .filter(|e| e.mark == '*')
        .map(|e| e.path.display().to_string())
        .collect();
    if targets.is_empty() {
        if let Some(e) = entry_at_point(ed) {
            targets.push(e.path.display().to_string());
        }
    }
    if targets.is_empty() {
        ed.message("No files to operate on");
        return;
    }
    let quoted: Vec<String> = targets.iter().map(|t| format!("'{}'", t.replace('\'', "'\\''"))).collect();
    let full = format!("{cmd} {}", quoted.join(" "));
    let output = std::process::Command::new("sh").arg("-c").arg(&full).output();
    match output {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.is_empty() {
                text.push_str("\n--- stderr ---\n");
                text.push_str(&err);
            }
            if text.trim().is_empty() {
                text = format!("({} exited: {})", full, out.status);
            }
            let id = match ed.buffer_by_name("*Shell Command Output*") {
                Some(id) => {
                    let buf = ed.buffers.get_mut(&id).unwrap();
                    buf.rope = ropey::Rope::from_str(&text);
                    buf.modified = false;
                    id
                }
                None => ed.create_buffer("*Shell Command Output*", &text),
            };
            ed.buffers.get_mut(&id).unwrap().read_only = true;
            show_in_other_window(ed, id);
        }
        Err(e) => ed.message(format!("Shell command failed: {e}")),
    }
}

pub fn kill_all_cmd(ed: &mut Editor, _n: Option<u32>) {
    let ids: Vec<BufferId> = ed
        .buffers
        .values()
        .filter(|b| matches!(b.mode, Mode::Dired(_)))
        .map(|b| b.id)
        .collect();
    let n = ids.len();
    for id in ids {
        ed.remove_buffer(id);
    }
    ed.message(format!("Killed {n} dired buffer{}", if n == 1 { "" } else { "s" }));
}

// wgrep entry points live in the wgrep module but are re-exported for the
// command table.
pub use wgrep::{wgrep_abort_cmd, wgrep_commit_cmd, wgrep_mode_cmd};

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("taco-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("file.txt"), "hello").unwrap();
        dir
    }

    #[test]
    fn listing_is_ls_la_shaped() {
        let dir = tempdir("ls");
        let mut ed = Editor::new();
        open_dired(&mut ed, &dir);

        let (state_name_col, entries) = {
            let Mode::Dired(s) = &ed.cur_buffer().mode else { panic!() };
            (s.name_col, s.entries.clone())
        };
        // ".." heads the list; directories before files after it.
        assert_eq!(entries[0].name, "..");
        assert_eq!(entries[1].name, "sub");
        assert_eq!(entries[2].name, "file.txt");
        assert_eq!(entries[2].size, 5);

        let text = ed.cur_buffer().to_string_lossless();
        let lines: Vec<&str> = text.lines().collect();
        // Header, then one line per entry; metadata columns then the name
        // exactly at name_col.
        assert_eq!(lines.len(), 1 + entries.len());
        let sub_line = lines[2];
        assert!(sub_line[2..].starts_with('d'), "mode string: {sub_line}");
        assert!(sub_line.contains("rwx") || sub_line.contains("r-x"));
        let name_at: String = sub_line.chars().skip(state_name_col).collect();
        assert_eq!(name_at, "sub");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dotdot_visits_parent_and_refuses_operations() {
        let dir = tempdir("dotdot");
        let mut ed = Editor::new();
        open_dired(&mut ed, &dir.join("sub"));

        // Point on the ".." line (line 1).
        let start = ed.cur_buffer().line_to_char(1);
        ed.windows.selected_mut().point = start;

        // Marking and deleting the parent entry are refused.
        mark_cmd(&mut ed, None);
        {
            let Mode::Dired(s) = &ed.cur_buffer().mode else { panic!() };
            assert_eq!(s.entries[0].mark, ' ');
        }
        do_delete_cmd(&mut ed, None);
        assert!(matches!(ed.input, crate::editor::InputMode::Normal));

        // RET on ".." lands in the parent directory listing.
        find_file_cmd(&mut ed, None);
        let Mode::Dired(s) = &ed.cur_buffer().mode else {
            panic!("not a dired buffer after ..")
        };
        assert_eq!(s.dir, dir.canonicalize().unwrap());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn jump_places_point_on_current_file() {
        let dir = tempdir("jump");
        let mut ed = Editor::new();
        crate::commands::files::find_file_path(&mut ed, &dir.join("file.txt"));
        jump_cmd(&mut ed, None);

        let buf = ed.cur_buffer();
        let Mode::Dired(s) = &buf.mode else { panic!("dired-jump did not open dired") };
        let line = buf.char_to_line(ed.windows.selected_ref().point);
        assert_eq!(s.entries[line - 1].name, "file.txt");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mtime_formatting() {
        // 2026-07-12 09:30:00 UTC
        assert_eq!(mtime_string(1783848600), "Jul 12 09:30");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19723), (2024, 1, 1));
    }

    #[test]
    fn mode_string_shapes() {
        assert_eq!(mode_string(0o100644, false), "-rw-r--r--");
        assert_eq!(mode_string(0o040755, true), "drwxr-xr-x");
        assert_eq!(mode_string(0o120777, false), "lrwxrwxrwx");
    }
}
