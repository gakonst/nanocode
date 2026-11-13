use nanocode_code_mode::CodeModeToolObservation;
use nanocode_tools::ToolOutputContent;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceToolRouting {
    ProcessStart,
    ProcessInput,
    Path,
    Patch,
    Shell,
    File,
}

impl WorkspaceToolRouting {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProcessStart => "process_start",
            Self::ProcessInput => "process_input",
            Self::Path => "path",
            Self::Patch => "patch",
            Self::Shell => "shell",
            Self::File => "file",
        }
    }

    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        [
            Self::ProcessStart,
            Self::ProcessInput,
            Self::Path,
            Self::Patch,
            Self::Shell,
            Self::File,
        ]
        .into_iter()
        .find(|routing| routing.as_str() == value)
    }
}

pub trait CodeModeToolPolicy: Send + Sync {
    fn direct_host_tool(&self, _name: &str) -> bool {
        false
    }

    fn workspace_routing(&self, _name: &str) -> Option<WorkspaceToolRouting> {
        None
    }

    fn exclusive(&self, _name: &str) -> bool {
        false
    }

    fn append_observation_content(
        &self,
        _calls: &[CodeModeToolObservation<'_>],
        _content: &mut Vec<ToolOutputContent>,
    ) {
    }

    fn session_notices(&self, _session_id: &str) -> Vec<String> {
        Vec::new()
    }

    fn session_notice<'a>(&'a self, _session_id: &'a str) -> crate::embedded::HostFuture<'a, ()> {
        Box::pin(std::future::pending())
    }
}
