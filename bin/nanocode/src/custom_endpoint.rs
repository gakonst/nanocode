use crate::host::ModelClients;

const ANTHROPIC_BASE_URL: &str = "NANOCODE_ANTHROPIC_BASE_URL";
const BASE_URL: &str = "NANOCODE_OPENAI_BASE_URL";
const MODELS_URL: &str = "NANOCODE_MODELS_URL";
const LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub fn apply(clients: ModelClients) -> Result<ModelClients, String> {
    let mut clients = anthropic(clients)?;
    let base = env(BASE_URL)
        .filter(|_| !clients.supplies_openai())
        .map(|url| url.trim_end_matches('/').to_owned());
    let models_url = env(MODELS_URL).or_else(|| base.as_ref().map(|base| format!("{base}/models")));
    if base.is_none() && models_url.is_none() {
        return Ok(clients);
    }
    nanocode::codex::transport::install_default_rustls_crypto_provider();
    let key = env("NANOCODE_OPENAI_API_KEY").or_else(|| env("OPENAI_API_KEY"));
    if let Some(url) = models_url {
        let ids = fetch_models(&url, key.as_deref())?;
        if ids.is_empty() {
            return Err(format!("{url} listed no models"));
        }
        clients = clients.models(
            ids.iter()
                .map(|id| nanocode::Model::Custom(nanocode::CustomModel::new(id))),
        );
    }
    if let Some(base) = base {
        let auth = match key {
            Some(key) => nanocode::codex::OpenAiAuth::api_key(key),
            None => nanocode::codex::OpenAiAuth::gateway_managed(),
        };
        clients = clients.openai(move || {
            nanocode::OpenAi::builder(auth.clone())
                .api_base_url(base.clone())
                .transport(nanocode::codex::transport::ResponsesTransport::Https)
        });
    }
    Ok(clients)
}

fn anthropic(clients: ModelClients) -> Result<ModelClients, String> {
    let Some(base) = env(ANTHROPIC_BASE_URL).filter(|_| !clients.supplies_claude()) else {
        return Ok(clients);
    };
    let key = env("NANOCODE_ANTHROPIC_API_KEY")
        .or_else(|| env("ANTHROPIC_API_KEY"))
        .ok_or_else(|| {
            format!("{ANTHROPIC_BASE_URL} needs NANOCODE_ANTHROPIC_API_KEY or ANTHROPIC_API_KEY")
        })?;
    let url = format!("{}/messages", base.trim_end_matches('/'));
    nanocode::codex::transport::install_default_rustls_crypto_provider();
    let http = nanocode_http::client();
    Ok(clients.claude(move || {
        nanocode::claude::ClaudeClient::new(http.clone(), url.clone(), key.clone())
    }))
}

fn fetch_models(url: &str, key: Option<&str>) -> Result<Vec<String>, String> {
    #[derive(serde::Deserialize)]
    struct List {
        data: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        id: String,
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let mut request = nanocode_http::client().get(url).timeout(LIST_TIMEOUT);
        if let Some(key) = key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .await
            .map(nanocode_http::decode_flate_response)
            .map_err(|error| format!("failed to list models from {url}: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("failed to list models from {url}: HTTP {status}"));
        }
        let list: List = response
            .json()
            .await
            .map_err(|error| format!("{url} did not return an OpenAI model list: {error}"))?;
        let mut ids = Vec::new();
        for entry in list.data {
            let id = entry.id.trim();
            if !id.is_empty() && !ids.iter().any(|known: &String| known == id) {
                ids.push(id.to_owned());
            }
        }
        Ok(ids)
    })
}
