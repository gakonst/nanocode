use super::{AttachmentError, AttachmentEvent, AttachmentStatus, AttachmentTarget, driver};
use crate::{
    Tools,
    prepared::{PreparedToolRuntime, PreparedTools},
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{mpsc, watch};

pub enum RunnerEndpoint {
    Listen(std::net::SocketAddr),
    Connect(String),
}

pub struct RunnerConfig {
    pub endpoint: RunnerEndpoint,
    pub token: String,
    pub workspace: PathBuf,
    pub name: String,
}

pub fn run_runner(
    config: RunnerConfig,
    tools: Tools,
) -> nanocode_sync::future::BoxFuture<'static, Result<(), AttachmentError>> {
    Box::pin(async move { run_runner_with_tools(config, tools).await })
}

pub fn run_runner_with_tools(
    config: RunnerConfig,
    tools: Tools,
) -> nanocode_sync::future::BoxFuture<'static, Result<(), AttachmentError>> {
    Box::pin(async move {
        super::RemoteRunner::new(config.name.clone())?;
        let workspace = tokio::fs::canonicalize(&config.workspace)
            .await
            .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
        let metadata = super::AttachmentMetadata::machine(super::AttachmentMachine::new(
            config.name.clone(),
            config.name,
            workspace.to_string_lossy(),
            ["process", "files"],
        )?);
        match config.endpoint {
            RunnerEndpoint::Connect(endpoint) => {
                let (attachment, mut events) = tools
                    .attach(AttachmentTarget::new(endpoint, config.token)?)
                    .metadata(metadata)
                    .start()?;
                nanocode_sync::select! {
                    result = attachment.closed() => result,
                    _ = async { while let Some(event) = events.recv().await { log_event(&event); } } => attachment.closed().await,
                    signal = tokio::signal::ctrl_c() => { signal.map_err(|error| AttachmentError::Transport(error.to_string().into()))?; attachment.detach().await },
                }
            }
            RunnerEndpoint::Listen(address) => {
                if !address.ip().is_loopback() {
                    return Err(AttachmentError::Transport("plaintext runner listeners require a loopback address; use a TLS proxy for remote access".into()));
                }
                let target = AttachmentTarget::new(format!("ws://{address}/"), config.token)?;
                let listener = tokio::net::TcpListener::bind(address)
                    .await
                    .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
                let runtime = Arc::new(
                    PreparedToolRuntime::initialize(PreparedTools::prepare(&tools)?).await?,
                );
                let catalog = serde_json::to_value(runtime.catalog()?)
                    .map_err(|error| AttachmentError::Catalog(error.to_string().into()))?;
                let (commands, command_rx) = mpsc::channel(1);
                let (events, mut event_rx) = mpsc::channel(128);
                let (status, _) = watch::channel(AttachmentStatus::Connecting);
                let (closed, closed_rx) = watch::channel(None);
                let mut running = Box::pin(driver::run(
                    driver::Config {
                        endpoint: target.endpoint().clone(),
                        listener: Some(listener),
                        authorization: format!("Bearer {}", target.bearer()).into(),
                        tools: catalog,
                        metadata: Some(metadata),
                    },
                    runtime,
                    command_rx,
                    events,
                    status,
                    closed,
                ));
                nanocode_sync::select! {
                    _ = &mut running => {},
                    _ = async { while let Some(event) = event_rx.recv().await { log_event(&event); } } => {},
                    signal = tokio::signal::ctrl_c() => {
                        signal.map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
                        let _ = commands.send(driver::Command::Detach).await;
                        running.await;
                    }
                }
                closed_rx
                    .borrow()
                    .clone()
                    .unwrap_or(Err(AttachmentError::Closed))
            }
        }
    })
}

fn log_event(event: &AttachmentEvent) {
    match event {
        AttachmentEvent::Connecting => eprintln!("runner: connecting"),
        AttachmentEvent::Retrying { reason, delay_ms } => {
            eprintln!("runner: retrying in {delay_ms}ms ({reason})")
        }
        AttachmentEvent::Attached => eprintln!("runner: connected"),
        AttachmentEvent::CatalogPublished { tool_count } => {
            eprintln!("runner: published {tool_count} tools")
        }
        AttachmentEvent::CallStarted { call_id, name } => {
            eprintln!("runner: executing {name} ({call_id})")
        }
        AttachmentEvent::CallCompleted { call_id, outcome } => {
            eprintln!("runner: completed {call_id} ({outcome:?})")
        }
        AttachmentEvent::Detached { reason } => eprintln!("runner: disconnected ({reason})"),
        AttachmentEvent::Fenced { reason } => eprintln!("runner: rejected ({reason})"),
    }
}
