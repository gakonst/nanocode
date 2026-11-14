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
        workspace.clone(),
        Arc::new(nanocode_sync::Atom::new(workspace)),
        None,
        Arc::new(crate::claude_bash::Jobs::default()),
        executor,
        true,
    )
    .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HandFiles {
    #[default]
    Machine,
    Confined,
}

pub fn hand_tools(directory: PathBuf) -> Result<Tools> {
    hand_tools_with(directory, HandFiles::Machine)
}

pub fn hand_tools_with(directory: PathBuf, files: HandFiles) -> Result<Tools> {
    let files = match files {
        HandFiles::Machine => crate::tools::ClaudeWorkspaceFiles::machine(&directory),
        HandFiles::Confined => crate::tools::ClaudeWorkspaceFiles::confined(&directory),
    }
    .map_err(|error| {
        NanocodeError::InvalidRequest(format!(
            "Claude file tools require the hand directory {}: {error}",
            directory.display()
        ))
    })?;
    crate::claude_files::selection(
        &Arc::new(files),
        directory.clone(),
        Arc::new(nanocode_sync::Atom::new(directory)),
        None,
        Arc::new(crate::claude_bash::Jobs::default()),
        Arc::new(nanocode_process::NativeForeground),
        true,
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
    direct: bool,
) -> Result<crate::ClaudeTools> {
    let jobs = Arc::new(crate::claude_bash::Jobs::with_subagents(&additive));
    let mut cwd = None;
    let selected = match selected {
        Some(tools) => tools,
        None => {
            let files = crate::tools::ClaudeWorkspaceFiles::new(&workspace).map_err(|error| {
                NanocodeError::InvalidRequest(format!(
                    "Claude file tools require the runtime workspace {}: {error}",
                    workspace.display()
                ))
            })?;
            let current = Arc::new(nanocode_sync::Atom::new(workspace.clone()));
            cwd = Some(Arc::clone(&current));
            crate::claude_files::selection(
                &Arc::new(files),
                workspace.clone(),
                current,
                Some(handle.session_environment()),
                jobs.clone(),
                executor,
                !direct,
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
    let builder = match direct {
        true => selected
            .into_builder()
            .exposure(nanocode_tool_host::ToolExposure::DirectOnly),
        false => selected.into_builder(),
    };
    let mut selected = providers
        .iter()
        .fold(builder, |builder, provider| {
            builder.shared_provider(provider.clone())
        })
        .build()
        .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))?;
    selected = selected.add_shared_tools(additive)?;
    let delivery = match (handle.root_session_id() != handle.session_id(), headless) {
        (true, _) => crate::claude_bash::Delivery::EndsWithTurn,
        (false, true) => crate::claude_bash::Delivery::Held,
        (false, false) => crate::claude_bash::Delivery::Prompted,
    };
    let bound = jobs.clone();
    let reaped = jobs.clone();
    let held = jobs.clone();
    let tools = crate::claude_tools::install(&selected, workspace, handle)?
        .adapter_shutdown(move || {
            let jobs = jobs.clone();
            async move {
                jobs.shutdown().await;
            }
        })
        .agent_bound(move |agent, steer| bound.bind(agent, steer, delivery));
    let tools = match cwd {
        Some(cwd) => tools.working_directory(cwd),
        None => tools,
    };
    let tools = match delivery {
        crate::claude_bash::Delivery::Held => tools.turn_hold(move || held.hold()),
        _ => tools,
    };
    Ok(if !matches!(delivery, crate::claude_bash::Delivery::Prompted) {
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
