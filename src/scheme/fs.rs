//! Filesystem, process, and text "Native C exceptions": narrow, policy-free
//! primitives exposed to Scheme. Every mode that needs OS interop (dired
//! included) is built on these — nothing here knows what a "mode" is.

use crate::scheme::with_editor;
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use steel::rvals::SteelVal;
use steel::steel_vm::engine::Engine;
use steel::steel_vm::register_fn::RegisterFn;

pub fn register(engine: &mut Engine) {
    engine.register_fn("directory-entries", directory_entries);
    engine.register_fn("delete-file", |path: String| result_string(std::fs::remove_file(path)));
    engine.register_fn("delete-directory-recursive", |path: String| {
        result_string(std::fs::remove_dir_all(path))
    });
    engine.register_fn("rename-file", |from: String, to: String| {
        result_string(std::fs::rename(from, to))
    });
    engine.register_fn("copy-file", |from: String, to: String| {
        result_string(std::fs::copy(from, to).map(|_| ()))
    });
    engine.register_fn("copy-directory-recursive", |from: String, to: String| {
        result_string(copy_dir_recursive(Path::new(&from), Path::new(&to)))
    });
    engine.register_fn("make-directory", |path: String| {
        result_string(std::fs::create_dir_all(path))
    });
    engine.register_fn("read-file-to-string", |path: String| {
        std::fs::read_to_string(path).unwrap_or_default()
    });
    engine.register_fn("file-exists?", |path: String| Path::new(&path).exists());
    engine.register_fn("parent-directory", |path: String| {
        Path::new(&path)
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    });
    engine.register_fn("canonicalize-path", |path: String| {
        std::fs::canonicalize(&path)
            .map(|p| p.display().to_string())
            .unwrap_or(path)
    });
    engine.register_fn("resolve-path", |path: String| {
        with_editor(|ed| ed.resolve_path(&path).display().to_string())
    });
    engine.register_fn("buffer-file-name", || {
        with_editor(|ed| {
            ed.cur_buffer()
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        })
    });
    engine.register_fn("file-name-only", |path: String| {
        Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    engine.register_fn("file-name-extension", |path: String| {
        Path::new(&path)
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    engine.register_fn(
        "diff-unified",
        |label_a: String, text_a: String, label_b: String, text_b: String| {
            similar::TextDiff::from_lines(&text_a, &text_b)
                .unified_diff()
                .header(&label_a, &label_b)
                .to_string()
        },
    );
    engine.register_fn("gzip-file", |path: String| {
        gzip_file(Path::new(&path))
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    });
    engine.register_fn("tar-gzip-directory", |path: String| {
        tar_gzip_dir(Path::new(&path))
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    });
    engine.register_fn("run-shell-command", |cmd: String| -> Vec<String> {
        match std::process::Command::new("sh").arg("-c").arg(&cmd).output() {
            Ok(out) => vec![
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ],
            Err(e) => vec![String::new(), e.to_string()],
        }
    });
    // Newline split of an arbitrary string (buffer-lines' free-standing
    // sibling). Native because Scheme-side splitting walks the string
    // char-by-char through the interpreter — hopeless on e.g. the 300KB
    // of `man -k .` output man.scm feeds through this.
    engine.register_fn("string-lines", |s: String| -> Vec<String> {
        s.lines().map(|l| l.to_string()).collect()
    });
    // Completion-style candidate matching: prefix matches first, then
    // plain substring matches, each side keeping the source order (the
    // ordering of vertico's default completion styles). Native because a
    // completion UI runs this on every keystroke, over candidate lists
    // that can be thousands long (M-x man's apropos list) — far too hot
    // for the interpreter.
    engine.register_fn(
        "filter-matching",
        |candidates: Vec<String>, input: String| -> Vec<String> {
            if input.is_empty() {
                return candidates;
            }
            let mut prefix = Vec::new();
            let mut substring = Vec::new();
            for c in candidates {
                if c.starts_with(&input) {
                    prefix.push(c);
                } else if c.contains(&input) {
                    substring.push(c);
                }
            }
            prefix.extend(substring);
            prefix
        },
    );
    // filter-matching's companion for tagged candidate lists ("name(3)"):
    // keep the candidates ending in `suffix`, source order kept. Native
    // for the same per-keystroke-over-thousands reason.
    engine.register_fn(
        "filter-suffix",
        |candidates: Vec<String>, suffix: String| -> Vec<String> {
            candidates.into_iter().filter(|c| c.ends_with(&suffix)).collect()
        },
    );
    engine.register_fn("regexp-match?", |pattern: String, text: String| {
        regex::Regex::new(&pattern)
            .map(|re| re.is_match(&text))
            .unwrap_or(false)
    });
    // Capturing variants: #f when the pattern is invalid or doesn't match.
    // (regexp-match pat text) -> (full g1 g2 ...), "" for groups that
    // didn't participate.
    engine.register_fn("regexp-match", |pattern: String, text: String| -> SteelVal {
        let Ok(re) = regex::Regex::new(&pattern) else {
            return SteelVal::BoolV(false);
        };
        match re.captures(&text) {
            Some(caps) => SteelVal::ListV(
                (0..caps.len())
                    .map(|i| {
                        let s = caps.get(i).map(|m| m.as_str()).unwrap_or("");
                        SteelVal::StringV(s.into())
                    })
                    .collect(),
            ),
            None => SteelVal::BoolV(false),
        }
    });
    // (regexp-match-positions pat text start) -> ((s0 e0) (s1 e1) ...) in
    // char offsets (all taco positions are chars, but the regex crate
    // reports bytes), #f per group when it didn't participate. `start` is
    // a char offset to begin searching from.
    engine.register_fn(
        "regexp-match-positions",
        |pattern: String, text: String, start: isize| -> SteelVal {
            let Ok(re) = regex::Regex::new(&pattern) else {
                return SteelVal::BoolV(false);
            };
            let byte_start = char_to_byte(&text, start.max(0) as usize);
            match re.captures_at(&text, byte_start) {
                Some(caps) => SteelVal::ListV(
                    (0..caps.len())
                        .map(|i| match caps.get(i) {
                            Some(m) => SteelVal::ListV(
                                vec![
                                    SteelVal::IntV(byte_to_char(&text, m.start()) as isize),
                                    SteelVal::IntV(byte_to_char(&text, m.end()) as isize),
                                ]
                                .into_iter()
                                .collect(),
                            ),
                            None => SteelVal::BoolV(false),
                        })
                        .collect(),
                ),
                None => SteelVal::BoolV(false),
            }
        },
    );
    engine.register_fn("set-buffer-string!", |text: String| {
        with_editor(|ed| ed.cur_buffer_mut().set_text(&text))
    });
    engine.register_fn("set-buffer-read-only!", |ro: bool| {
        with_editor(|ed| ed.cur_buffer_mut().read_only = ro)
    });
    engine.register_fn("buffer-lines", || {
        with_editor(|ed| {
            ed.cur_buffer()
                .rope
                .lines()
                .map(|l| l.to_string().trim_end_matches('\n').to_string())
                .collect::<Vec<_>>()
        })
    });
    // Split only if there's a single window, then select the other one —
    // the "pop to another window" half of Emacs' display-buffer. Scheme
    // follows up with (switch-to-buffer name) to actually put a buffer
    // there, so this stays a pure window-management primitive.
    engine.register_fn("other-window-or-split", || {
        with_editor(|ed| {
            if ed.windows.leaves().len() == 1 {
                ed.windows.split(crate::window::SplitDir::Below);
            }
            ed.windows.select_next();
        })
    });
    engine.register_fn("register-directory-opener", |f: SteelVal| {
        with_editor(|ed| ed.directory_opener = Some(f))
    });

    // Buffer-targeted (by name) variants of current-buffer operations —
    // needed by asynchronous callbacks (a process filter/sentinel), which
    // run with whatever buffer happens to be current at that moment.
    engine.register_fn(
        "buffer-add-face-span!",
        |name: String, start: isize, end: isize, face: String| {
            with_editor(|ed| {
                let Some(buf) = ed.buffers.values_mut().find(|b| b.name == name) else {
                    ed.message(format!("buffer-add-face-span!: no buffer {name:?}"));
                    return;
                };
                let len = buf.len_chars();
                let a = (start.max(0) as usize).min(len);
                let b = (end.max(0) as usize).min(len);
                if a < b {
                    buf.face_spans.push((a, b, face));
                }
            })
        },
    );
    engine.register_fn("buffer-clear-face-spans!", |name: String| {
        with_editor(|ed| {
            if let Some(buf) = ed.buffers.values_mut().find(|b| b.name == name) {
                buf.face_spans.clear();
            }
        })
    });
    engine.register_fn("buffer-local-set-in!", |name: String, key: String, value: SteelVal| {
        with_editor(|ed| {
            let Some(buf) = ed.buffers.values_mut().find(|b| b.name == name) else {
                ed.message(format!("buffer-local-set-in!: no buffer {name:?}"));
                return;
            };
            buf.locals.insert(key, value);
        })
    });
    engine.register_fn("set-buffer-mode-name-in!", |name: String, mode: String| {
        with_editor(|ed| {
            if let Some(buf) = ed.buffers.values_mut().find(|b| b.name == name) {
                buf.mode_name = mode;
            }
        })
    });
    engine.register_fn("buffer-append-in!", |name: String, text: String| {
        with_editor(|ed| {
            let Some(id) = ed.buffer_by_name(&name) else {
                ed.message(format!("buffer-append-in!: no buffer {name:?}"));
                return;
            };
            ed.append_to_buffer(id, &text);
        })
    });
    // "Sun Jul 13 12:34:56 2026" (UTC — taco has no timezone machinery;
    // same convention as dired's mtime column).
    engine.register_fn("current-time-string", || {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        time_string(secs)
    });
}

/// Byte offset of char `n` in `s` (s.len() when past the end).
fn char_to_byte(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(b, _)| b).unwrap_or(s.len())
}

/// Char offset of byte `b` in `s` (assumed to lie on a char boundary).
fn byte_to_char(s: &str, b: usize) -> usize {
    s[..b].chars().count()
}

/// Emacs current-time-string format: "Sun Jul 13 12:34:56 2026".
fn time_string(secs: i64) -> String {
    // 1970-01-01 was a Thursday.
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let days = secs.div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let rem = secs.rem_euclid(86400);
    format!(
        "{} {} {:>2} {:02}:{:02}:{:02} {}",
        DAYS[days.rem_euclid(7) as usize],
        MONTHS[(m - 1) as usize],
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        y
    )
}

fn result_string(r: std::io::Result<()>) -> String {
    match r {
        Ok(()) => String::new(),
        Err(e) => e.to_string(),
    }
}

fn copy_dir_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for item in std::fs::read_dir(from)? {
            let item = item?;
            copy_dir_recursive(&item.path(), &to.join(item.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

fn gzip_file(path: &Path) -> anyhow::Result<PathBuf> {
    let out_path = PathBuf::from(format!("{}.gz", path.display()));
    let input = std::fs::read(path)?;
    let out = std::fs::File::create(&out_path)?;
    let mut enc = flate2::write::GzEncoder::new(out, flate2::Compression::default());
    enc.write_all(&input)?;
    enc.finish()?;
    Ok(out_path)
}

fn tar_gzip_dir(path: &Path) -> anyhow::Result<PathBuf> {
    let out_path = PathBuf::from(format!("{}.tar.gz", path.display()));
    let out = std::fs::File::create(&out_path)?;
    let enc = flate2::write::GzEncoder::new(out, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);
    let base = path.file_name().unwrap_or_default();
    tar.append_dir_all(base, path)?;
    tar.into_inner()?.finish()?;
    Ok(out_path)
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

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Jan 12 03:45" (UTC, fixed 12 columns).
fn mtime_string(secs: i64) -> String {
    let (_, m, d) = civil_from_days(secs.div_euclid(86400));
    let rem = secs.rem_euclid(86400);
    format!(
        "{} {:>2} {:02}:{:02}",
        MONTHS[(m - 1) as usize],
        d,
        rem / 3600,
        (rem % 3600) / 60
    )
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

/// One entry as a Scheme list: (path name is-dir? mode-string nlink owner
/// group size mtime-string). No sorting, no formatting into columns, no
/// mark — that's Scheme's job (dired.scm).
fn entry_record(name: String, path: PathBuf) -> SteelVal {
    let md = std::fs::symlink_metadata(&path).ok();
    let is_dir = path.is_dir();
    let (mode, nlink, uid, gid, size, mtime) = md
        .map(|m| (m.mode(), m.nlink(), m.uid(), m.gid(), m.size(), m.mtime()))
        .unwrap_or((0, 0, 0, 0, 0, 0));
    // Resolved lazily per call — directory listings aren't hot enough to
    // warrant caching these across calls.
    let users = id_names("/etc/passwd");
    let groups = id_names("/etc/group");
    let owner = users.get(&uid).cloned().unwrap_or_else(|| uid.to_string());
    let group = groups.get(&gid).cloned().unwrap_or_else(|| gid.to_string());
    SteelVal::ListV(
        vec![
            SteelVal::StringV(path.display().to_string().into()),
            SteelVal::StringV(name.into()),
            SteelVal::BoolV(is_dir),
            SteelVal::StringV(mode_string(mode, is_dir).into()),
            SteelVal::IntV(nlink as isize),
            SteelVal::StringV(owner.into()),
            SteelVal::StringV(group.into()),
            SteelVal::IntV(size as isize),
            SteelVal::StringV(mtime_string(mtime).into()),
        ]
        .into_iter()
        .collect(),
    )
}

/// Unsorted entries for `dir` (dotfiles excluded unless `show_hidden`), plus
/// a synthetic ".." entry pointing at the parent when one exists. Sorting,
/// column layout, and marks are Scheme's job.
fn directory_entries(dir: String, show_hidden: bool) -> Vec<SteelVal> {
    let dir = Path::new(&dir);
    let mut out = Vec::new();
    if let Some(parent) = dir.parent() {
        out.push(entry_record("..".into(), parent.to_path_buf()));
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for item in rd.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        out.push(entry_record(name, item.path()));
    }
    out
}
