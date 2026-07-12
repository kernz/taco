//! C-h k / C-h f.

use crate::editor::{Editor, InputMode, PromptKind};

pub fn describe_key(ed: &mut Editor, _n: Option<u32>) {
    ed.input = InputMode::DescribeKey(Vec::new());
    ed.message("Describe key: ");
}

pub fn describe_function(ed: &mut Editor, _n: Option<u32>) {
    ed.prompt(PromptKind::DescribeFunction, "Describe function: ");
}

pub fn execute_extended_command(ed: &mut Editor, _n: Option<u32>) {
    ed.prompt(PromptKind::ExecuteCommand, "M-x ");
}
