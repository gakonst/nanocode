use std::{path::PathBuf, sync::Arc};

use nanocode_process_contracts::{FileHost, ShellExecutor};
use nanocode_tool_host::{Tools, ToolsBuildError};

use super::{
    apply_patch::HostApplyPatchHandler,
    code_mode_policy::standard_tool_policy,
    plan::UpdatePlanTool,
    shell::{ExecCommandHandler, WriteStdinHandler},
};

pub fn host_tools(
    workspace: PathBuf,
    files: Arc<dyn FileHost>,
    shell: Arc<dyn ShellExecutor>,
) -> Result<Tools, ToolsBuildError> {
    Tools::builder()
        .without_defaults()
        .tool(HostApplyPatchHandler::new(workspace.clone(), files))
        .tool(ExecCommandHandler::new(workspace, Arc::clone(&shell)))
        .tool(WriteStdinHandler::new(shell))
        .tool(UpdatePlanTool::new())
        .build()
        .map(|tools| tools.with_code_mode_policy(standard_tool_policy()))
}

pub fn host_file_tools(
    workspace: PathBuf,
    files: Arc<dyn FileHost>,
) -> Result<Tools, ToolsBuildError> {
    Tools::builder()
        .without_defaults()
        .tool(HostApplyPatchHandler::new(workspace, files))
        .build()
        .map(|tools| tools.with_code_mode_policy(standard_tool_policy()))
}
