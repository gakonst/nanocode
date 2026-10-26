use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use nanocode_mcp::Tool as McpTool;
use nanocode_sync::Atom;
use nanocode_tools::ToolDefinition;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};
use tokio::sync::watch;

use super::{
    McpServerError,
    client::Client,
    config::{McpServer, McpToolExposure},
    search::{SearchEngine, tokenize},
};

const DEFAULT_SEARCH_LIMIT: usize = 8;
const MAX_MODEL_TOOL_NAME_BYTES: usize = 128;
const NAME_HASH_BYTES: usize = 12;
const NAME_HASH_SUFFIX_BYTES: usize = NAME_HASH_BYTES + 1;

pub(crate) struct ToolEntry {
    pub canonical_name: String,
    pub server_name: String,
    pub remote_name: String,
    namespace: String,
    namespace_description: String,
    pub definition: ToolDefinition,
    pub supports_parallel_tool_calls: bool,
    pub read_only: bool,
    pub tool_exposure: McpToolExposure,
    pub search_text: String,
    pub client: Client,
    pub timeout: Duration,
}

#[derive(Clone)]
struct Catalog {
    entries: BTreeMap<String, Arc<ToolEntry>>,
    active: BTreeSet<String>,
    clients: BTreeMap<String, Client>,
    failures: BTreeMap<String, String>,
    refused: BTreeSet<String>,
    search_index: Option<Arc<SearchIndex>>,
    pending_servers: BTreeSet<String>,
    generations: BTreeMap<String, u64>,
}

pub(crate) struct ConnectedCatalog {
    pub client: Client,
    pub entries: Vec<ToolEntry>,
}

struct SearchIndex {
    entries: Vec<Arc<ToolEntry>>,
    engine: SearchEngine,
}

pub(crate) struct ProviderState {
    catalog: Atom<Catalog>,
    remaining: watch::Sender<usize>,
    discovery_timeout: Duration,
}

#[derive(Serialize)]
pub(crate) struct SearchResponse {
    tools: Vec<SearchTool>,
    pending_servers: usize,
    failed_servers: BTreeMap<String, String>,
    #[serde(skip)]
    loadable_tools: Vec<LoadableNamespace>,
}

impl SearchResponse {
    pub(crate) const fn tool_count(&self) -> usize {
        self.tools.len()
    }

    pub(crate) const fn pending_server_count(&self) -> usize {
        self.pending_servers
    }

    pub(crate) fn loadable_tools(&self) -> Result<Value, serde_json::Error> {
        serde_json::to_value(&self.loadable_tools)
    }
}

#[derive(Serialize)]
struct SearchTool {
    name: String,
    server: String,
    tool: String,
    description: String,
    supports_parallel_tool_calls: bool,
    input_schema: Value,
}

#[derive(Serialize)]
struct LoadableNamespace {
    r#type: &'static str,
    name: String,
    description: String,
    tools: Vec<LoadableFunction>,
}

#[derive(Serialize)]
struct LoadableFunction {
    r#type: &'static str,
    name: String,
    description: String,
    strict: bool,
    defer_loading: bool,
    parameters: Value,
}

impl ProviderState {
    pub(crate) fn new(
        server_names: impl IntoIterator<Item = String>,
        discovery_timeout: Duration,
    ) -> Self {
        let pending_servers = server_names.into_iter().collect::<BTreeSet<_>>();
        let generations = pending_servers
            .iter()
            .cloned()
            .map(|name| (name, 0))
            .collect();
        let (remaining, _) = watch::channel(pending_servers.len());
        Self {
            catalog: Atom::new(Catalog {
                entries: BTreeMap::new(),
                active: BTreeSet::new(),
                clients: BTreeMap::new(),
                failures: BTreeMap::new(),
                refused: BTreeSet::new(),
                search_index: None,
                pending_servers,
                generations,
            }),
            remaining,
            discovery_timeout,
        }
    }

    pub(crate) fn complete_server(
        &self,
        server_name: &str,
        generation: u64,
        result: Result<ConnectedCatalog, McpServerError>,
    ) {
        let result = result.map(|ConnectedCatalog { client, entries }| {
            (
                client,
                entries.into_iter().map(Arc::new).collect::<Vec<_>>(),
            )
        });
        let Some(prewarmed) = self.catalog.try_update(|catalog| {
            if catalog.generations.get(server_name).copied() != Some(generation) {
                return None;
            }
            Some(Self::complete(catalog, server_name, &result))
        }) else {
            return;
        };
        if let Some(tool_count) = prewarmed {
            tracing::info!(
                target: "nanocode_code_mode",
                tool_count,
                "prewarmed MCP tool search index"
            );
        }
        self.publish_remaining();
    }

    fn publish_remaining(&self) {
        self.remaining.send_modify(|remaining| {
            *remaining = self.catalog.read(|catalog| catalog.pending_servers.len());
        });
    }

    fn complete(
        catalog: &mut Catalog,
        server_name: &str,
        result: &Result<(Client, Vec<Arc<ToolEntry>>), McpServerError>,
    ) -> Option<usize> {
        match result {
            Ok((client, entries)) => {
                let removed = catalog
                    .entries
                    .iter()
                    .filter_map(|(name, entry)| {
                        (entry.server_name == server_name).then_some(name.clone())
                    })
                    .collect::<Vec<_>>();
                for name in removed {
                    catalog.entries.remove(&name);
                }
                for entry in entries {
                    if catalog.entries.contains_key(&entry.canonical_name) {
                        catalog.failures.insert(
                            server_name.to_owned(),
                            format!(
                                "MCP tool name collision after normalization: `{}`",
                                entry.canonical_name
                            ),
                        );
                        continue;
                    }
                    catalog
                        .entries
                        .insert(entry.canonical_name.clone(), Arc::clone(entry));
                }
                catalog
                    .clients
                    .insert(server_name.to_owned(), Arc::clone(client));
                let available = catalog.entries.keys().cloned().collect::<BTreeSet<_>>();
                catalog.active.retain(|name| available.contains(name));
            }
            Err(error) => {
                catalog
                    .failures
                    .insert(server_name.to_owned(), error.message.clone());
                if error.refused {
                    catalog.refused.insert(server_name.to_owned());
                }
            }
        }
        catalog.pending_servers.remove(server_name);
        if catalog.pending_servers.is_empty() {
            catalog.search_index = Some(Arc::new(SearchIndex::new(
                catalog
                    .entries
                    .values()
                    .filter(|entry| entry.tool_exposure.is_deferred())
                    .cloned(),
            )));
            Some(catalog.entries.len())
        } else {
            catalog.search_index = None;
            None
        }
    }

    pub(crate) fn begin_server(&self, server_name: &str) -> u64 {
        let generation = self.catalog.update(|catalog| {
            let generation = *catalog
                .generations
                .entry(server_name.to_owned())
                .and_modify(|generation| *generation = generation.saturating_add(1))
                .or_insert(0);
            catalog.failures.remove(server_name);
            catalog.refused.remove(server_name);
            catalog.pending_servers.insert(server_name.to_owned());
            generation
        });
        self.publish_remaining();
        generation
    }

    pub(crate) async fn search(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<SearchResponse, String> {
        let query = query.trim();
        if query.is_empty() {
            return Err("query must not be empty".to_owned());
        }
        let limit = limit.unwrap_or(DEFAULT_SEARCH_LIMIT);
        if limit == 0 {
            return Err("limit must be greater than zero".to_owned());
        }
        self.wait_for_startup().await;
        let catalog = self.catalog.get();
        let index = match &catalog.search_index {
            Some(index) => Arc::clone(index),
            None => {
                tracing::info!(
                    target: "nanocode_code_mode",
                    tool_count = catalog.entries.len(),
                    "building MCP tool search index"
                );
                let index = Arc::new(SearchIndex::new(
                    catalog
                        .entries
                        .values()
                        .filter(|entry| entry.tool_exposure.is_deferred())
                        .cloned(),
                ));
                self.catalog.try_update(|current| {
                    (current.search_index.is_none()
                        && same_entries(&current.entries, &catalog.entries))
                    .then(|| current.search_index = Some(Arc::clone(&index)))
                });
                index
            }
        };
        let pending_servers = catalog.pending_servers.len();
        let failed_servers = catalog.failures.clone();
        drop(catalog);

        let selected = index.search(query, limit);
        let tools = selected.iter().map(|entry| entry.summary()).collect();
        let loadable_tools = coalesce_loadable_tools(&selected);
        let active_count = self.catalog.update(|catalog| {
            for entry in &selected {
                catalog.active.insert(entry.canonical_name.clone());
            }
            catalog.active.len()
        });
        tracing::debug!(
            target: "nanocode_code_mode",
            result_count = selected.len(),
            active_count,
            "searched MCP tool catalog"
        );
        Ok(SearchResponse {
            tools,
            pending_servers,
            failed_servers,
            loadable_tools,
        })
    }

    pub(crate) fn native_entries(&self) -> Vec<Arc<ToolEntry>> {
        self.catalog.read(|catalog| {
            catalog
                .entries
                .values()
                .filter(|entry| entry.tool_exposure.is_deferred())
                .cloned()
                .collect()
        })
    }

    pub(crate) fn native_refresh_generation(&self, server: &str, client: &Client) -> Option<u64> {
        self.catalog.try_update(|catalog| {
            if catalog.pending_servers.contains(server)
                || !catalog
                    .clients
                    .get(server)
                    .is_some_and(|current| Arc::ptr_eq(current, client))
            {
                return None;
            }
            let generation = catalog.generations.get_mut(server)?;
            *generation = generation.saturating_add(1);
            Some(*generation)
        })
    }

    pub(crate) fn native_clients(&self) -> BTreeMap<String, Client> {
        self.catalog.read(|catalog| catalog.clients.clone())
    }

    pub(crate) fn native_status(&self) -> Value {
        let catalog = self.catalog.get();
        serde_json::json!({
            "pending_servers": catalog.pending_servers,
            "failed_servers": catalog.failures,
            "ready_servers": catalog.clients.keys().collect::<Vec<_>>(),
            "complete": catalog.pending_servers.is_empty(),
        })
    }

    pub(crate) fn available_definitions(&self) -> Vec<ToolDefinition> {
        let catalog = self.catalog.get();
        catalog
            .entries
            .values()
            .filter(|entry| entry.tool_exposure.is_available_in_code_mode())
            .map(|entry| entry.definition.clone())
            .collect()
    }

    pub(crate) fn entry(&self, name: &str) -> Option<Arc<ToolEntry>> {
        self.catalog
            .read(|catalog| catalog.entries.get(name).cloned())
    }

    pub(crate) async fn ready_entry(&self, name: &str) -> Option<Arc<ToolEntry>> {
        self.wait_for_startup().await;
        self.entry(name)
    }

    pub(crate) async fn server_entries(
        &self,
    ) -> BTreeMap<String, Result<Vec<Arc<ToolEntry>>, McpServerError>> {
        self.wait_for_startup().await;
        let catalog = self.catalog.get();
        let mut servers = BTreeMap::new();
        for server in catalog.clients.keys() {
            servers.insert(server.clone(), Ok(Vec::new()));
        }
        for server in &catalog.pending_servers {
            servers.insert(
                server.clone(),
                Err("MCP discovery timed out".to_owned().into()),
            );
        }
        for (server, error) in &catalog.failures {
            let error = McpServerError {
                message: error.clone(),
                refused: catalog.refused.contains(server),
            };
            servers.insert(server.clone(), Err(error));
        }
        for entry in catalog.entries.values() {
            if entry.tool_exposure.is_callable()
                && let Some(Ok(entries)) = servers.get_mut(&entry.server_name)
            {
                entries.push(Arc::clone(entry));
            }
        }
        servers
    }

    pub(crate) async fn prepared_entries(&self) -> Result<Vec<Arc<ToolEntry>>, String> {
        self.wait_for_startup().await;
        let catalog = self.catalog.get();
        if !catalog.pending_servers.is_empty() {
            return Err(format!(
                "MCP discovery timed out with pending servers: {}",
                catalog
                    .pending_servers
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !catalog.failures.is_empty() {
            return Err(format!(
                "MCP discovery failed: {}",
                catalog
                    .failures
                    .iter()
                    .map(|(server, error)| format!("{server}: {error}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if let Some(entry) = catalog.entries.values().find(|entry| {
            matches!(
                entry.tool_exposure,
                McpToolExposure::DeferredOnly | McpToolExposure::CodeModeOnly
            )
        }) {
            return Err(format!(
                "MCP tool `{}` uses {:?}, which attachment cannot preserve; use DeferredAndCodeMode or Hidden",
                entry.canonical_name, entry.tool_exposure
            ));
        }
        Ok(catalog
            .entries
            .values()
            .filter(|entry| entry.tool_exposure == McpToolExposure::DeferredAndCodeMode)
            .cloned()
            .collect())
    }

    pub(crate) async fn wait_for_startup(&self) {
        if self.catalog.read(|catalog| catalog.search_index.is_some()) {
            return;
        }
        let mut remaining = self.remaining.subscribe();
        if *remaining.borrow() == 0 {
            return;
        }
        let wait = async {
            while *remaining.borrow_and_update() > 0 {
                if remaining.changed().await.is_err() {
                    break;
                }
            }
        };
        drop(nanocode_platform::timeout(self.discovery_timeout, wait).await);
    }
}

fn same_entries(
    left: &BTreeMap<String, Arc<ToolEntry>>,
    right: &BTreeMap<String, Arc<ToolEntry>>,
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|((left_name, left), (right_name, right))| {
                left_name == right_name && Arc::ptr_eq(left, right)
            })
}

impl SearchIndex {
    fn new(entries: impl IntoIterator<Item = Arc<ToolEntry>>) -> Self {
        let entries = entries.into_iter().collect::<Vec<_>>();
        let engine = SearchEngine::new(entries.iter().map(|entry| tokenize(&entry.search_text)));
        Self { entries, engine }
    }

    fn search(&self, query: &str, limit: usize) -> Vec<Arc<ToolEntry>> {
        self.engine
            .search(&tokenize(query), limit)
            .into_iter()
            .map(|(index, _)| Arc::clone(&self.entries[index]))
            .collect()
    }
}

impl ToolEntry {
    pub(crate) fn attached_provider(&self) -> &str {
        &self.namespace
    }

    pub(crate) fn new_many(
        server_name: &str,
        namespace: &str,
        tools: Vec<McpTool>,
        client: Client,
        config: &McpServer,
    ) -> Vec<Self> {
        let mut tools_by_name = BTreeMap::new();
        for tool in tools {
            tools_by_name.entry(tool.name.clone()).or_insert(tool);
        }
        let mut names = normalized_tool_names(namespace, tools_by_name.keys().map(String::as_str));
        tools_by_name
            .into_iter()
            .filter_map(|(remote_name, tool)| {
                let model_name = names.remove(&remote_name)?;
                Some(Self::new(
                    server_name,
                    namespace,
                    model_name,
                    remote_name,
                    &tool,
                    Arc::clone(&client),
                    config,
                ))
            })
            .collect()
    }

    fn new(
        server_name: &str,
        namespace: &str,
        model_name: String,
        remote_name: String,
        tool: &McpTool,
        client: Client,
        config: &McpServer,
    ) -> Self {
        let canonical_name = format!("{namespace}{model_name}");
        let namespace_description = config
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Tools in the {namespace} namespace."));
        let description = tool.description.as_deref().unwrap_or_default().to_owned();
        let supports_parallel_tool_calls = tool_supports_parallel_calls(
            tool,
            config.supports_parallel_tool_calls,
            config.parallel_tools.contains(tool.name.as_str()),
        );
        let mut input_schema = tool.input_schema.clone();
        if input_schema.get("properties").is_none_or(Value::is_null) {
            input_schema.insert("properties".to_owned(), Value::Object(Map::new()));
        }
        let definition = ToolDefinition::function(
            canonical_name.clone(),
            description.clone(),
            Value::Object(input_schema.clone()),
        )
        .with_output_schema(json!({
            "type": "object",
            "properties": {
                "content": { "type": "array", "items": { "type": "object" } },
                "structuredContent": tool.output_schema
                    .as_ref()
                    .map_or_else(|| json!({}), |schema| Value::Object(schema.clone())),
                "isError": { "type": "boolean" },
                "_meta": { "type": "object" }
            },
            "required": ["content"],
            "additionalProperties": false
        }));
        let mut properties = input_schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        properties.sort();
        let search_text = [
            canonical_name.as_str(),
            server_name,
            remote_name.as_str(),
            tool.title.as_deref().unwrap_or_default(),
            description.as_str(),
            &properties.join(" "),
        ]
        .join(" ");
        Self {
            canonical_name,
            server_name: server_name.to_owned(),
            remote_name,
            namespace: namespace.to_owned(),
            namespace_description,
            definition,
            supports_parallel_tool_calls,
            read_only: tool.is_read_only(),
            tool_exposure: config.tool_exposure,
            search_text,
            client,
            timeout: config.tool_timeout,
        }
    }

    fn summary(&self) -> SearchTool {
        SearchTool {
            name: self.canonical_name.clone(),
            server: self.server_name.clone(),
            tool: self.remote_name.clone(),
            description: self.definition.description().to_owned(),
            supports_parallel_tool_calls: self.supports_parallel_tool_calls,
            input_schema: self
                .definition
                .parameters()
                .map_or(Value::Null, |schema| schema.as_value().clone()),
        }
    }

    fn loadable_function(&self) -> LoadableFunction {
        LoadableFunction {
            r#type: "function",
            name: self
                .canonical_name
                .strip_prefix(&self.namespace)
                .unwrap_or(&self.canonical_name)
                .to_owned(),
            description: self.definition.description().to_owned(),
            strict: false,
            defer_loading: true,
            parameters: self
                .definition
                .parameters()
                .map_or_else(|| json!({}), |schema| schema.as_value().clone()),
        }
    }
}

fn tool_supports_parallel_calls(tool: &McpTool, server_opt_in: bool, tool_policy: bool) -> bool {
    server_opt_in || tool_policy || tool.is_read_only()
}

fn normalize_name(name: &str) -> String {
    let normalized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if normalized.is_empty() {
        "_".to_owned()
    } else {
        normalized
    }
}

pub(crate) fn normalized_server_names<'a>(
    server_names: impl IntoIterator<Item = &'a str>,
) -> BTreeMap<String, String> {
    let mut candidates = server_names
        .into_iter()
        .map(|raw| (raw.to_owned(), format!("mcp__{}", normalize_name(raw))))
        .collect::<Vec<_>>();
    let mut counts = BTreeMap::<String, usize>::new();
    for (_, base) in &candidates {
        *counts.entry(base.clone()).or_default() += 1;
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));

    let maximum_body_bytes = MAX_MODEL_TOOL_NAME_BYTES - NAME_HASH_SUFFIX_BYTES - 2;
    let mut used = BTreeSet::new();
    candidates
        .into_iter()
        .map(|(raw, base)| {
            let collides = counts.get(&base).copied().unwrap_or_default() > 1;
            let body = unique_bounded_name(&base, &raw, maximum_body_bytes, collides, &mut used);
            (raw, format!("{body}__"))
        })
        .collect()
}

fn normalized_tool_names<'a>(
    namespace: &str,
    raw_names: impl IntoIterator<Item = &'a str>,
) -> BTreeMap<String, String> {
    let mut candidates = raw_names
        .into_iter()
        .map(|raw| (raw.to_owned(), normalize_name(raw)))
        .collect::<Vec<_>>();
    let mut counts = BTreeMap::<String, usize>::new();
    for (_, base) in &candidates {
        *counts.entry(base.clone()).or_default() += 1;
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));

    let maximum_bytes = MAX_MODEL_TOOL_NAME_BYTES.saturating_sub(namespace.len());
    let mut used = BTreeSet::new();
    candidates
        .into_iter()
        .map(|(raw, base)| {
            let collides = counts.get(&base).copied().unwrap_or_default() > 1;
            let model = unique_bounded_name(&base, &raw, maximum_bytes, collides, &mut used);
            (raw, model)
        })
        .collect()
}

fn unique_bounded_name(
    base: &str,
    raw_identity: &str,
    maximum_bytes: usize,
    require_hash: bool,
    used: &mut BTreeSet<String>,
) -> String {
    if !require_hash && base.len() <= maximum_bytes && used.insert(base.to_owned()) {
        return base.to_owned();
    }

    let mut attempt = 0_u32;
    loop {
        let identity = if attempt == 0 {
            raw_identity.to_owned()
        } else {
            format!("{raw_identity}\0{attempt}")
        };
        let suffix = name_hash_suffix(&identity);
        let prefix_bytes = maximum_bytes.saturating_sub(suffix.len());
        let prefix = &base[..base.len().min(prefix_bytes)];
        let candidate = format!("{prefix}{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        attempt = attempt.saturating_add(1);
    }
}

fn name_hash_suffix(identity: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(identity.as_bytes());
    let hash = hasher.finalize();
    let mut suffix = String::with_capacity(NAME_HASH_SUFFIX_BYTES);
    suffix.push('_');
    for byte in &hash[..NAME_HASH_BYTES / 2] {
        use std::fmt::Write as _;
        let _ = write!(suffix, "{byte:02x}");
    }
    suffix
}

fn coalesce_loadable_tools(selected: &[Arc<ToolEntry>]) -> Vec<LoadableNamespace> {
    let mut namespaces: Vec<LoadableNamespace> = Vec::new();
    for entry in selected {
        if let Some(namespace) = namespaces
            .iter_mut()
            .find(|namespace| namespace.name == entry.namespace)
        {
            namespace.tools.push(entry.loadable_function());
        } else {
            namespaces.push(LoadableNamespace {
                r#type: "namespace",
                name: entry.namespace.clone(),
                description: entry.namespace_description.clone(),
                tools: vec![entry.loadable_function()],
            });
        }
    }
    namespaces
}
