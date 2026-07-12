//! wgrep-style bulk rename: make the dired listing writable, edit names as
//! plain text, then commit (rename on disk) or abort.
//!
//! The snapshot maps buffer lines to paths positionally: editing a name in
//! place renames it. Reordering or deleting lines is not tracked.

use crate::buffer::Mode;
use crate::editor::Editor;
use std::path::PathBuf;

pub fn wgrep_mode_cmd(ed: &mut Editor, _n: Option<u32>) {
    let buf = ed.cur_buffer_mut();
    let Mode::Dired(state) = &mut buf.mode else {
        ed.message("Not a dired buffer");
        return;
    };
    if state.wgrep.is_some() {
        ed.message("Already editing");
        return;
    }
    let mut snapshot: Vec<Option<PathBuf>> = vec![None]; // header line
    snapshot.extend(state.entries.iter().map(|e| {
        // The parent tracking entry is not renamable.
        (e.name != "..").then(|| e.path.clone())
    }));
    state.wgrep = Some(snapshot);
    buf.read_only = false;
    ed.message("Editable dired: edit names, then C-c C-c to commit or C-c C-k to abort");
}

pub fn wgrep_commit_cmd(ed: &mut Editor, _n: Option<u32>) {
    let (renames, errors) = {
        let buf = ed.cur_buffer_mut();
        let Mode::Dired(state) = &mut buf.mode else {
            ed.message("Not a dired buffer");
            return;
        };
        let Some(snapshot) = state.wgrep.take() else {
            ed.message("Not in wgrep mode (C-c C-e first)");
            return;
        };
        let dir = state.dir.clone();
        let name_col = state.name_col;
        let mut renames = 0usize;
        let mut errors: Vec<String> = Vec::new();
        for (i, old) in snapshot.iter().enumerate() {
            let Some(old_path) = old else { continue };
            if i >= buf.rope.len_lines() {
                continue;
            }
            let line: String = buf.rope.line(i).to_string();
            let line = line.trim_end_matches('\n');
            let new_name: String = line.chars().skip(name_col).collect();
            let new_name = new_name.trim();
            let old_name = old_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if new_name.is_empty() || new_name == old_name {
                continue;
            }
            let dest = dir.join(new_name);
            match std::fs::rename(old_path, &dest) {
                Ok(()) => renames += 1,
                Err(e) => errors.push(format!("{old_name}: {e}")),
            }
        }
        buf.read_only = true;
        (renames, errors)
    };
    super::revert_cmd(ed, None);
    if errors.is_empty() {
        ed.message(format!(
            "Applied {renames} rename{}",
            if renames == 1 { "" } else { "s" }
        ));
    } else {
        ed.message(format!("{renames} renamed; failed: {}", errors.join("; ")));
    }
}

pub fn wgrep_abort_cmd(ed: &mut Editor, _n: Option<u32>) {
    {
        let buf = ed.cur_buffer_mut();
        let Mode::Dired(state) = &mut buf.mode else {
            ed.message("Not a dired buffer");
            return;
        };
        if state.wgrep.take().is_none() {
            ed.message("Not in wgrep mode");
            return;
        }
        buf.read_only = true;
    }
    super::revert_cmd(ed, None);
    ed.message("Changes aborted");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    #[test]
    fn wgrep_renames_edited_lines() {
        let dir = std::env::temp_dir().join(format!("taco-wgrep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("alpha.txt"), "a").unwrap();
        std::fs::write(dir.join("beta.txt"), "b").unwrap();

        let mut ed = Editor::new();
        crate::dired::open_dired(&mut ed, &dir);
        wgrep_mode_cmd(&mut ed, None);
        assert!(!ed.cur_buffer().read_only);

        // Edit "alpha.txt" to "gamma.txt" in the listing text.
        {
            let buf = ed.cur_buffer_mut();
            let text = buf.to_string_lossless().replace("alpha.txt", "gamma.txt");
            buf.rope = ropey::Rope::from_str(&text);
        }
        wgrep_commit_cmd(&mut ed, None);

        assert!(dir.join("gamma.txt").exists());
        assert!(!dir.join("alpha.txt").exists());
        assert!(dir.join("beta.txt").exists());
        assert!(ed.cur_buffer().read_only);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wgrep_abort_restores_listing() {
        let dir = std::env::temp_dir().join(format!("taco-wabort-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep.txt"), "x").unwrap();

        let mut ed = Editor::new();
        crate::dired::open_dired(&mut ed, &dir);
        wgrep_mode_cmd(&mut ed, None);
        {
            let buf = ed.cur_buffer_mut();
            let text = buf.to_string_lossless().replace("keep.txt", "lost.txt");
            buf.rope = ropey::Rope::from_str(&text);
        }
        wgrep_abort_cmd(&mut ed, None);

        assert!(dir.join("keep.txt").exists());
        assert!(!dir.join("lost.txt").exists());
        let buf = ed.cur_buffer();
        assert!(buf.read_only);
        assert!(buf.to_string_lossless().contains("keep.txt"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
