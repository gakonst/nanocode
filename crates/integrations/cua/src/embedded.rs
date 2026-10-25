use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use cua_driver_core::{
    protocol::Request,
    server::{ToolProvider, handle_request_with_transport_session},
};
use cua_driver_sdk::{CuaDriver, DriverHostOptions};
use nanocode_mcp::RpcError;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::OnceCell,
};

static PROVIDER: OnceCell<Arc<Provider>> = OnceCell::const_new();
static SESSIONS: AtomicU64 = AtomicU64::new(0);

struct Provider {
    driver: Arc<CuaDriver>,
    tools: Value,
}

#[async_trait::async_trait]
impl ToolProvider for Provider {
    fn tools_list(&self) -> Value {
        self.tools.clone()
    }

    fn is_known_tool(&self, name: &str) -> bool {
        cua_driver_core::tool::renamed_tool(name).is_some()
            || self.driver.local_is_known_tool(name).unwrap_or(false)
    }

    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<Value, String> {
        let result = self
            .driver
            .call_tool_from_trusted_adapter(name, arguments)
            .await
            .map_err(|error| error.to_string())?;
        serde_json::from_str(&result.raw_json).map_err(|error| error.to_string())
    }
}

async fn provider() -> Result<Arc<Provider>, String> {
    PROVIDER
        .get_or_try_init(|| async {
            cua_driver_core::authorization::validate_startup_authorization()
                .map_err(|error| error.to_string())?;
            cua_driver_core::policy::validate_configured_policy()
                .map_err(|error| error.to_string())?;
            let driver = CuaDriver::try_create_service_for_host(DriverHostOptions {
                cursor: cursor_overlay::CursorConfig {
                    enabled: false,
                    ..cursor_overlay::CursorConfig::default()
                },
                host_owns_permission_ux: true,
                host_bundle_id: std::env::var(cua_driver_core::HOST_BUNDLE_ID_ENV).ok(),
                claude_code_compatibility: false,
                prepare_desktop_environment: true,
                register_host_tools: None,
                authorization_host: None,
                activity_observer: None,
            })
            .map_err(|error| error.to_string())?;
            let tools = driver
                .list_tools_json()
                .await
                .map_err(|error| error.to_string())?;
            let tools = serde_json::from_str(&tools).map_err(|error| error.to_string())?;
            Ok(Arc::new(Provider { driver, tools }))
        })
        .await
        .cloned()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permissions {
    pub accessibility: bool,
    pub screen_recording: bool,
}

pub fn permissions() -> Permissions {
    let status = cua_driver_sdk::current_mac_os_permission_status();
    Permissions {
        accessibility: status.accessibility,
        screen_recording: status.screen_recording,
    }
}

pub fn request_permissions() -> Permissions {
    let status = cua_driver_sdk::request_mac_os_permissions();
    Permissions {
        accessibility: status.accessibility,
        screen_recording: status.screen_recording,
    }
}

pub struct Server {
    provider: Arc<Provider>,
    session: String,
}

impl Server {
    pub async fn start(label: &str) -> Result<Self, String> {
        Ok(Self {
            provider: provider().await?,
            session: format!(
                "{label}-{}-{}",
                std::process::id(),
                SESSIONS.fetch_add(1, Ordering::Relaxed)
            ),
        })
    }

    pub fn tools(&self) -> &Value {
        &self.provider.tools
    }

    pub async fn request(&self, message: Value) -> Option<Value> {
        let request = match serde_json::from_value::<Request>(message) {
            Ok(request) if request.is_notification() => return None,
            Ok(request) => request,
            Err(error) => {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("Parse error: {error}")},
                }));
            }
        };
        let id = request.id.clone().unwrap_or(Value::Null);
        let response = handle_request_with_transport_session(
            request,
            id.clone(),
            self.provider.as_ref(),
            &self.session,
        )
        .await;
        Some(serde_json::to_value(response).unwrap_or_else(|error| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32603, "message": error.to_string()},
            })
        }))
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, RpcError> {
        let response = self
            .request(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            }))
            .await
            .unwrap_or(Value::Null);
        match response.get("result") {
            Some(result) => Ok(result.clone()),
            None => Err(RpcError::new(
                response["error"]["code"].as_i64().unwrap_or(-32603),
                response["error"]["message"]
                    .as_str()
                    .unwrap_or("cua-driver returned no result"),
            )),
        }
    }
}

pub async fn serve_stdio() -> Result<(), String> {
    let server = Server::start("mcp").await?;
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut writer = tokio::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        if reader
            .read_line(&mut line)
            .await
            .map_err(|error| error.to_string())?
            == 0
        {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(line.trim()) {
            Ok(message) => server.request(message).await,
            Err(error) => Some(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32700, "message": format!("Parse error: {error}")},
            })),
        };
        let Some(response) = response else { continue };
        let mut bytes = serde_json::to_vec(&response).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        writer
            .write_all(&bytes)
            .await
            .map_err(|error| error.to_string())?;
        writer.flush().await.map_err(|error| error.to_string())?;
    }
}
