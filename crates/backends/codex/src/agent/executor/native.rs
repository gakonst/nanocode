use std::sync::Arc;

use crate::client::__private::ModelConfig;

use crate::{NanocodeError, Result};

pub(crate) type ServiceFactory =
    Arc<dyn Fn(Arc<ModelConfig>) -> super::ResponsesClientBox + Send + Sync>;

pub(crate) trait AgentSend: Send {}

impl<T: Send> AgentSend for T {}

pub(crate) trait AgentFactory: Send + Sync {}

impl<T: Send + Sync> AgentFactory for T {}

pub(crate) fn spawn_driver(
    driver: crate::agent::execution::ExecutionFuture<'static, ()>,
) -> Result<()> {
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|_| NanocodeError::TokioRuntimeUnavailable)?;
    drop(nanocode_platform::spawn_on(&runtime, driver));
    Ok(())
}
