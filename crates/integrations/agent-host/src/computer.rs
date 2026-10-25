use std::sync::Arc;

#[cfg(not(target_family = "wasm"))]
pub(super) struct Cua {
    tools: tokio::sync::OnceCell<Option<Computer>>,
    unavailable: std::sync::OnceLock<String>,
}

#[cfg(not(target_family = "wasm"))]
pub(super) type Cell = Option<Arc<Cua>>;

#[cfg(not(target_family = "wasm"))]
pub(super) type Computer = crate::ComputerTools;

#[cfg(not(target_family = "wasm"))]
pub(super) fn cell(explicit: Option<Computer>) -> Arc<Cua> {
    Arc::new(Cua {
        tools: match explicit {
            Some(computer) => tokio::sync::OnceCell::new_with(Some(Some(computer))),
            None => tokio::sync::OnceCell::new(),
        },
        unavailable: std::sync::OnceLock::new(),
    })
}

#[cfg(not(target_family = "wasm"))]
pub(super) async fn connect(cell: Cell) -> Option<Computer> {
    use tracing::Instrument as _;

    let cua = cell?;
    cua.tools
        .get_or_init(|| async {
            let connected = match crate::ComputerConfig::detect() {
                Ok(config) => {
                    if let Some(missing) = config.unavailable() {
                        let _ = cua.unavailable.set(missing);
                    }
                    crate::ComputerTools::connect(config)
                        .instrument(tracing::info_span!("agent.host.computer.connect"))
                        .await
                }
                Err(error) => Err(error),
            };
            connected
                .map_err(|error| {
                    let _ = cua.unavailable.set(error.to_string());
                })
                .ok()
        })
        .await
        .clone()
}

#[cfg(not(target_family = "wasm"))]
pub(super) fn unavailable(cell: &Cell) -> Option<String> {
    cell.as_ref()?.unavailable.get().cloned()
}

#[cfg(not(target_family = "wasm"))]
pub(super) fn shared(computer: Option<&Computer>) -> Vec<Arc<dyn nanocode_tool_contracts::Tool>> {
    computer
        .map(|computer| {
            computer
                .tools()
                .map(|tool| Arc::new(tool) as Arc<dyn nanocode_tool_contracts::Tool>)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(target_family = "wasm")]
pub(super) type Cell = Option<Arc<Computer>>;

#[cfg(target_family = "wasm")]
pub(super) enum Computer {}

#[cfg(target_family = "wasm")]
pub(super) async fn connect(cell: Cell) -> Option<Computer> {
    cell.map(|computer| match *computer {})
}

#[cfg(target_family = "wasm")]
pub(super) fn unavailable(cell: &Cell) -> Option<String> {
    cell.as_ref().map(|computer| match **computer {})
}

#[cfg(target_family = "wasm")]
pub(super) fn shared(computer: Option<&Computer>) -> Vec<Arc<dyn nanocode_tool_contracts::Tool>> {
    computer.map_or_else(Vec::new, |computer| match *computer {})
}
