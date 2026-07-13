//! C-h k / C-h f.

use crate::editor::{Editor, InputMode, PromptKind};

pub fn describe_key(ed: &mut Editor, _n: Option<u32>) {
    ed.input = InputMode::DescribeKey {
        seq: Vec::new(),
        prompt: "Describe key: ".to_string(),
        on_done: None,
    };
    ed.message("Describe key: ");
}

pub fn describe_function(ed: &mut Editor, _n: Option<u32>) {
    ed.prompt(PromptKind::DescribeFunction, "Describe function: ");
}

pub fn execute_extended_command(ed: &mut Editor, _n: Option<u32>) {
    ed.prompt(PromptKind::ExecuteCommand, "M-x ");
}
