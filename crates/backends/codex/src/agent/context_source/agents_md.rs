use std::{path::Path, sync::Arc};

use nanocode_home::{Diagnostic, Level, ProjectHome, combine_instructions};
use nanocode_trace::warn;

pub(crate) fn load_global_instructions(nanocode_home: Option<&Path>) -> Option<Arc<str>> {
    let global = nanocode_home::global_instructions(nanocode_home?);
    report(&global.diagnostics);
    global.combined().map(Arc::from)
}

pub(super) fn load_instructions(
    workspace: &Path,
    global_instructions: Option<&str>,
) -> Option<String> {
    combine_instructions(
        global_instructions,
        load_project_instructions(workspace).as_deref(),
    )
}

fn load_project_instructions(workspace: &Path) -> Option<String> {
    let instructions = ProjectHome::discover(workspace).read_instructions();
    report(&instructions.diagnostics);
    instructions.combined()
}

fn report(diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        if matches!(diagnostic.level, Level::Warning) {
            warn!(
                path = %diagnostic.path.display(),
                message = %diagnostic.message,
                "instruction file skipped"
            );
        }
    }
}
