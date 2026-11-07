mod admission;
mod execution;
mod registry;
mod schema;

pub use crate::selection::{
    DynamicToolProvider, ProviderGate, ToolExposure, ToolSource, Tools, ToolsBuildError,
    ToolsBuilder,
};
pub use execution::{ToolRuntime, ToolRuntimeControl};
#[doc(hidden)]
pub use registry::RoutingCatalog;
pub(crate) use registry::ToolRegistry;
pub use schema::schema_for;

use std::{
    any::Any,
    collections::{HashMap, HashSet},
    panic::AssertUnwindSafe,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::FutureExt;
use nanocode_tools::{Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput};
use serde_json::value::to_raw_value;
use serde_json::{Value, json};
use tracing::{Instrument, info, info_span};

use crate::code_mode::{self, CodeModeExecution, CodeModeObserver};
pub use crate::embedded::OwnedToolContext;

fn host_owned_name(name: &str) -> bool {
    matches!(name, "exec" | "wait")
}
