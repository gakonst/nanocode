use nanocode_agent_core::{AgentHandle, NanocodeError, Result};
use nanocode_tool_host::{Tool, Tools};
use nanocode_tools::ToolCollection as _;
use std::{path::PathBuf, sync::Arc};

pub fn install_default_tools(
    workspace: PathBuf,
    handle: &AgentHandle,
    selected: Option<Tools>,
    additive: Vec<Arc<dyn Tool>>,
) -> Result<crate::ClaudeTools> {
    install_default_tools_with_process_routes(workspace, handle, selected, additive, None)
}

pub fn install_default_tools_with_process_routes(
    workspace: PathBuf,
    handle: &AgentHandle,
    selected: Option<Tools>,
    additive: Vec<Arc<dyn Tool>>,
    process_routes: Option<Arc<dyn nanocode_tool_host::attachment::ProcessRouteStore>>,
) -> Result<crate::ClaudeTools> {
    install_default_tools_with_runner_pool(
        workspace,
        handle,
        selected,
        additive,
        process_routes,
        None,
    )
}

pub fn runner_tools(workspace: PathBuf) -> Result<Tools> {
    runner_tools_with_executor(workspace, Arc::new(nanocode_process::NativeForeground))
}

pub fn runner_tools_with_executor(
    workspace: PathBuf,
    executor: Arc<dyn nanocode_process_contracts::ForegroundExecutor>,
) -> Result<Tools> {
    let files = crate::tools::ClaudeWorkspaceFiles::new(&workspace).map_err(|error| {
        NanocodeError::InvalidRequest(format!(
            "Claude file tools require the runner workspace {}: {error}",
            workspace.display()
        ))
    })?;
    crate::claude_files::selection(
        &Arc::new(files),
        workspace,
        None,
        Arc::new(crate::claude_bash::Jobs::default()),
        executor,
    )
    .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))
}

pub fn hand_tools(directory: PathBuf) -> Result<Tools> {
    let files = crate::tools::ClaudeWorkspaceFiles::machine(&directory).map_err(|error| {
        NanocodeError::InvalidRequest(format!(
            "Claude file tools require the hand directory {}: {error}",
            directory.display()
        ))
    })?;
    crate::claude_files::selection(
        &Arc::new(files),
        directory,
        None,
        Arc::new(crate::claude_bash::Jobs::default()),
        Arc::new(nanocode_process::NativeForeground),
    )
    .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))
}

pub fn install_default_tools_with_runner_pool(
    workspace: PathBuf,
    handle: &AgentHandle,
    selected: Option<Tools>,
    additive: Vec<Arc<dyn Tool>>,
    process_routes: Option<Arc<dyn nanocode_tool_host::attachment::ProcessRouteStore>>,
    runner_pool: Option<nanocode_tool_host::attachment::RunnerRouting>,
) -> Result<crate::ClaudeTools> {
    install_default_tools_with_executor(
        workspace,
        handle,
        selected,
        additive,
        process_routes,
        runner_pool,
        Arc::new(nanocode_process::NativeForeground),
    )
}

pub fn install_default_tools_with_executor(
    workspace: PathBuf,
    handle: &AgentHandle,
    selected: Option<Tools>,
    additive: Vec<Arc<dyn Tool>>,
    process_routes: Option<Arc<dyn nanocode_tool_host::attachment::ProcessRouteStore>>,
    runner_pool: Option<nanocode_tool_host::attachment::RunnerRouting>,
    executor: Arc<dyn nanocode_process_contracts::ForegroundExecutor>,
) -> Result<crate::ClaudeTools> {
    install_default_tools_with_providers(
        workspace,
        handle,
        selected,
        additive,
        process_routes,
        runner_pool,
        executor,
        &[],
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn install_default_tools_with_providers(
    workspace: PathBuf,
    handle: &AgentHandle,
    selected: Option<Tools>,
    additive: Vec<Arc<dyn Tool>>,
    process_routes: Option<Arc<dyn nanocode_tool_host::attachment::ProcessRouteStore>>,
    runner_pool: Option<nanocode_tool_host::attachment::RunnerRouting>,
    executor: Arc<dyn nanocode_process_contracts::ForegroundExecutor>,
    providers: &[Arc<dyn nanocode_tool_host::runtime::DynamicToolProvider>],
    headless: bool,
) -> Result<crate::ClaudeTools> {
    let jobs = Arc::new(crate::claude_bash::Jobs::with_subagents(&additive));
    let selected = match selected {
        Some(tools) => tools,
        None => {
            let files = crate::tools::ClaudeWorkspaceFiles::new(&workspace).map_err(|error| {
                NanocodeError::InvalidRequest(format!(
                    "Claude file tools require the runtime workspace {}: {error}",
                    workspace.display()
                ))
            })?;
            crate::claude_files::selection(
                &Arc::new(files),
                workspace.clone(),
                Some(handle.session_environment()),
                jobs.clone(),
                executor,
            )
            .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))?
        }
    };
    let selected = if selected.has_code_mode_host() {
        selected
    } else {
        let selected = match process_routes {
            Some(store) => selected.with_default_process_route_store(store),
            None => selected,
        };
        match runner_pool {
            Some(pool) => selected.with_runner_pool(pool),
            None => selected,
        }
    };
    let mut selected = providers
        .iter()
        .fold(selected.into_builder(), |builder, provider| {
            builder.shared_provider(provider.clone())
        })
        .build()
        .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))?;
    selected = selected.add_shared_tools(additive)?;
    let ends_with_turn = headless || handle.root_session_id() != handle.session_id();
    let bound = jobs.clone();
    let reaped = jobs.clone();
    let tools = crate::claude_tools::install(&selected, workspace, handle)?
        .adapter_shutdown(move || {
            let jobs = jobs.clone();
            async move {
                jobs.shutdown().await;
            }
        })
        .agent_bound(move |agent, steer| bound.bind(agent, steer, ends_with_turn));
    Ok(if ends_with_turn {
        tools.adapter_cleanup(move || {
            let jobs = reaped.clone();
            async move {
                jobs.reap();
            }
        })
    } else {
        tools
    })
}
