use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

use nanocode_http::header::HeaderValue;
use serde_json::{Value, json};
use tracing::Instrument as _;

use crate::{
    ClaudeAccessToken, ClaudeAuthFuture, ClaudeAuthUnavailable, ClaudeClient, ClaudeTokenSource,
    RefreshingClaudeAuth,
};

const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const SECURE_STORAGE_DIR_ENV: &str = "CLAUDE_SECURESTORAGE_CONFIG_DIR";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);
const KEYCHAIN_COMMAND_LIMIT: usize = 4032;
const LOCK_FILE: &str = ".nanocode-oauth-refresh.lock";

#[derive(Clone)]
struct ClaudeCodeLogin {
    http: nanocode_http::Client,
}

fn nfc(path: &std::ffi::OsStr) -> String {
    use unicode_normalization::UnicodeNormalization as _;
    path.to_string_lossy().nfc().collect()
}

fn keychain_service() -> String {
    let scoped = match std::env::var_os(SECURE_STORAGE_DIR_ENV) {
        Some(directory) => Some(directory).filter(|directory| !directory.is_empty()),
        None => std::env::var_os(nanocode_home::CLAUDE_CONFIG_DIR_ENV)
            .filter(|directory| !directory.is_empty()),
    };
    match scoped {
        Some(directory) => {
            let digest = nanocode_digest::Sha256::digest(nfc(&directory).as_bytes());
            let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            format!("{KEYCHAIN_SERVICE}-{}", &hex[..8])
        }
        None => KEYCHAIN_SERVICE.to_owned(),
    }
}

fn keychain_account() -> Option<String> {
    let user = std::env::var("USER").ok().filter(|user| !user.is_empty())?;
    Some(
        if user
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        {
            user
        } else {
            "claude-code-user".to_owned()
        },
    )
}

fn credentials_path(home: &nanocode_home::AgentHome) -> PathBuf {
    let directory = match std::env::var_os(SECURE_STORAGE_DIR_ENV) {
        Some(directory) if !directory.is_empty() => PathBuf::from(nfc(&directory)),
        Some(_) => home.user_home().map_or_else(
            || home.claude_home().to_path_buf(),
            |user| user.join(".claude"),
        ),
        None => home.claude_home().to_path_buf(),
    };
    directory.join(".credentials.json")
}

enum Store {
    Keychain {
        service: String,
        account: Option<String>,
    },
    File(PathBuf),
}

struct Login {
    store: Store,
    document: Value,
    directory: PathBuf,
}

impl Login {
    async fn load() -> Result<Self, ClaudeAuthUnavailable> {
        let home = nanocode_home::AgentHome::from_env()
            .map_err(|error| unavailable(&error.to_string()))?;
        let path = credentials_path(&home);
        let directory = path
            .parent()
            .map_or_else(|| home.claude_home().to_path_buf(), Path::to_path_buf);
        let mut keychain = None;
        if cfg!(target_os = "macos") {
            let service = keychain_service();
            let account = keychain_account();
            let mut command = tokio::process::Command::new("security");
            command.arg("find-generic-password");
            if let Some(account) = &account {
                command.args(["-a", account]);
            }
            let output = command
                .args(["-w", "-s", &service])
                .output()
                .await
                .map_err(|error| unavailable(&format!("keychain lookup failed: {error}")))?;
            if output.status.success() {
                return Self::parse(
                    Store::Keychain { service, account },
                    &output.stdout,
                    directory,
                );
            }
            keychain = Some(format!(
                "macOS keychain item {service:?}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let bytes = tokio::fs::read(&path).await.map_err(|error| {
            let file = format!("{}: {error}", path.display());
            unavailable(&match keychain {
                Some(keychain) => format!(
                    "no Claude Code login found ({keychain}; {file}); run `claude` to log in"
                ),
                None => format!("no Claude Code login found ({file}); run `claude` to log in"),
            })
        })?;
        Self::parse(Store::File(path), &bytes, directory)
    }

    fn parse(
        store: Store,
        bytes: &[u8],
        directory: PathBuf,
    ) -> Result<Self, ClaudeAuthUnavailable> {
        let document: Value = serde_json::from_slice(bytes)
            .map_err(|error| unavailable(&format!("invalid Claude Code login: {error}")))?;
        if !document["claudeAiOauth"].is_object() {
            return Err(unavailable(
                "Claude Code login has no claudeAiOauth credentials",
            ));
        }
        Ok(Self {
            store,
            document,
            directory,
        })
    }

    fn access(&self) -> Result<(&str, u64), ClaudeAuthUnavailable> {
        let oauth = &self.document["claudeAiOauth"];
        let token = oauth["accessToken"]
            .as_str()
            .ok_or_else(|| unavailable("Claude Code login has no access token"))?;
        let expires_at = oauth["expiresAt"]
            .as_u64()
            .ok_or_else(|| unavailable("Claude Code login has no expiry"))?;
        Ok((token, expires_at))
    }

    fn usable(&self, rejected: Option<&HeaderValue>) -> Result<bool, ClaudeAuthUnavailable> {
        let (token, expires_at) = self.access()?;
        let margin = u64::try_from(REFRESH_MARGIN.as_millis()).unwrap_or(u64::MAX);
        let fresh = expires_at > nanocode_platform::unix_millis().saturating_add(margin);
        let rejected = rejected
            .is_some_and(|rejected| rejected.as_bytes() == format!("Bearer {token}").as_bytes());
        Ok(fresh && !rejected)
    }

    fn token(&self) -> Result<ClaudeAccessToken, ClaudeAuthUnavailable> {
        let (token, expires_at) = self.access()?;
        ClaudeAccessToken::new(token, UNIX_EPOCH + Duration::from_millis(expires_at))
    }

    async fn save(&self) -> Result<(), ClaudeAuthUnavailable> {
        let bytes = serde_json::to_vec(&self.document).map_err(|error| {
            unavailable(&format!(
                "cannot encode the refreshed Claude Code login: {error}"
            ))
        })?;
        match &self.store {
            Store::File(path) => write_file(path, &bytes).await,
            Store::Keychain { service, account } => {
                let account = account.as_deref().ok_or_else(|| {
                    unavailable(
                        "cannot save the refreshed Claude Code login to the macOS keychain because USER is unset",
                    )
                })?;
                write_keychain(service, account, &bytes).await?;
                let (token, _) = self.access()?;
                let saved = Self::load().await?;
                if saved.access()?.0 != token {
                    return Err(unavailable(
                        "the macOS keychain did not keep the refreshed Claude Code login",
                    ));
                }
                Ok(())
            }
        }
    }
}

async fn write_file(path: &Path, bytes: &[u8]) -> Result<(), ClaudeAuthUnavailable> {
    let path = path.to_path_buf();
    let bytes = bytes.to_vec();
    nanocode_platform::spawn_blocking(move || {
        use std::io::Write as _;
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        let temporary = directory.join(format!(
            ".credentials.json.{}.tmp",
            nanocode_platform::uuid_v4()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let written = options.open(&temporary).and_then(|mut file| {
            file.write_all(&bytes)?;
            file.sync_all()
        });
        let renamed = written.and_then(|()| std::fs::rename(&temporary, &path));
        if renamed.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        renamed.map_err(|error| {
            unavailable(&format!(
                "cannot save the refreshed Claude Code login to {}: {error}",
                path.display()
            ))
        })
    })
    .await
    .map_err(|error| unavailable(&format!("Claude Code login save task failed: {error}")))?
}

async fn write_keychain(
    service: &str,
    account: &str,
    bytes: &[u8],
) -> Result<(), ClaudeAuthUnavailable> {
    use tokio::io::AsyncWriteExt as _;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let command =
        format!("add-generic-password -U -a \"{account}\" -s \"{service}\" -X \"{hex}\"\n");
    let interactive = command.len() <= KEYCHAIN_COMMAND_LIMIT;
    let mut security = tokio::process::Command::new("security");
    if interactive {
        security.arg("-i").stdin(std::process::Stdio::piped());
    } else {
        tracing::warn!(
            target: "nanocode_claude",
            bytes = bytes.len(),
            "the Claude Code login exceeds the security -i command limit; passing it as arguments"
        );
        security
            .args([
                "add-generic-password",
                "-U",
                "-a",
                account,
                "-s",
                service,
                "-X",
                &hex,
            ])
            .stdin(std::process::Stdio::null());
    }
    let mut child = security
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| unavailable(&format!("keychain update failed to start: {error}")))?;
    if interactive {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| unavailable("keychain update has no stdin"))?;
        stdin
            .write_all(command.as_bytes())
            .await
            .map_err(|error| unavailable(&format!("keychain update failed: {error}")))?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|error| unavailable(&format!("keychain update failed: {error}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() || !stderr.trim().is_empty() {
        return Err(unavailable(&format!(
            "keychain update of {service:?} failed: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

async fn lock(directory: &Path) -> Result<std::fs::File, ClaudeAuthUnavailable> {
    let path = directory.join(LOCK_FILE);
    nanocode_platform::spawn_blocking(move || {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        file.lock()?;
        Ok::<_, std::io::Error>(file)
    })
    .await
    .map_err(|error| unavailable(&format!("Claude Code login lock task failed: {error}")))?
    .map_err(|error| {
        unavailable(&format!(
            "cannot lock {} for the Claude Code login refresh: {error}",
            directory.join(LOCK_FILE).display()
        ))
    })
}

impl ClaudeCodeLogin {
    async fn current(
        &self,
        rejected: Option<&HeaderValue>,
    ) -> Result<ClaudeAccessToken, ClaudeAuthUnavailable> {
        let login = Login::load().await?;
        if login.usable(rejected)? {
            return login.token();
        }
        let refresh = self.clone();
        let directory = login.directory;
        let rejected = rejected.cloned();
        nanocode_platform::spawn_task(
            async move { refresh.refresh(&directory, rejected.as_ref()).await }.in_current_span(),
        )
        .await
        .map_err(|error| unavailable(&format!("Claude Code login refresh task failed: {error}")))?
    }

    async fn refresh(
        &self,
        directory: &Path,
        rejected: Option<&HeaderValue>,
    ) -> Result<ClaudeAccessToken, ClaudeAuthUnavailable> {
        let guard = lock(directory).await?;
        let mut login = Login::load().await?;
        if login.usable(rejected)? {
            drop(guard);
            tracing::info!(target: "nanocode_claude", "reused a Claude Code login refreshed by another process");
            return login.token();
        }
        self.exchange(&mut login).await?;
        login.save().await?;
        drop(guard);
        tracing::info!(target: "nanocode_claude", "refreshed the Claude Code login");
        login.token()
    }

    async fn exchange(&self, login: &mut Login) -> Result<(), ClaudeAuthUnavailable> {
        let oauth = &login.document["claudeAiOauth"];
        let refresh = oauth["refreshToken"]
            .as_str()
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                unavailable("Claude Code login expired and has no refresh token; run `claude` to log in again")
            })?
            .to_owned();
        let client_id = oauth["clientId"].as_str().unwrap_or(CLIENT_ID).to_owned();
        let mut body = json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh,
            "client_id": client_id,
        });
        if let Some(scopes) = oauth["scopes"].as_array() {
            let scopes = scopes
                .iter()
                .map(|scope| {
                    scope
                        .as_str()
                        .ok_or_else(|| unavailable("Claude Code login scopes must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if !scopes.is_empty() {
                body["scope"] = Value::String(scopes.join(" "));
            }
        }
        let response = self
            .http
            .post(TOKEN_URL)
            .header("content-type", "application/json")
            .timeout(REFRESH_TIMEOUT)
            .body(body.to_string())
            .send()
            .await
            .map(nanocode_http::decode_response)
            .map_err(|error| {
                unavailable(&format!(
                    "Claude Code login refresh could not reach {TOKEN_URL}: {}",
                    error.without_url()
                ))
            })?;
        let status = response.status();
        let text = response.text().await.map_err(|error| {
            unavailable(&format!(
                "Claude Code login refresh response failed: {}",
                error.without_url()
            ))
        })?;
        let reply: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            let error = reply["error"].as_str().unwrap_or("unknown_error");
            let description = reply["error_description"]
                .as_str()
                .map_or_else(String::new, |description| format!(": {description}"));
            return Err(unavailable(&format!(
                "Claude Code login refresh was rejected with HTTP {} ({error}{description}); run `claude` to log in again",
                status.as_u16()
            )));
        }
        let access = reply["access_token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .ok_or_else(|| unavailable("Claude Code login refresh response has no access_token"))?;
        let expires_in = reply["expires_in"]
            .as_u64()
            .ok_or_else(|| unavailable("Claude Code login refresh response has no expires_in"))?;
        let now = nanocode_platform::unix_millis();
        let oauth = login.document["claudeAiOauth"]
            .as_object_mut()
            .ok_or_else(|| unavailable("Claude Code login has no claudeAiOauth credentials"))?;
        oauth.insert("accessToken".into(), Value::String(access.to_owned()));
        if let Some(rotated) = reply["refresh_token"]
            .as_str()
            .filter(|token| !token.is_empty())
        {
            oauth.insert("refreshToken".into(), Value::String(rotated.to_owned()));
        }
        oauth.insert(
            "expiresAt".into(),
            json!(now.saturating_add(expires_in.saturating_mul(1000))),
        );
        if let Some(seconds) = reply["refresh_token_expires_in"].as_u64() {
            oauth.insert(
                "refreshTokenExpiresAt".into(),
                json!(now.saturating_add(seconds.saturating_mul(1000))),
            );
        }
        if let Some(scope) = reply["scope"].as_str() {
            oauth.insert(
                "scopes".into(),
                Value::Array(
                    scope
                        .split_whitespace()
                        .map(|scope| Value::String(scope.to_owned()))
                        .collect(),
                ),
            );
        }
        Ok(())
    }
}

impl ClaudeTokenSource for ClaudeCodeLogin {
    fn refresh(&self) -> ClaudeAuthFuture<'_, Result<ClaudeAccessToken, ClaudeAuthUnavailable>> {
        Box::pin(self.current(None))
    }

    fn refresh_rejected<'a>(
        &'a self,
        rejected: &'a HeaderValue,
    ) -> ClaudeAuthFuture<'a, Result<Option<ClaudeAccessToken>, ClaudeAuthUnavailable>> {
        Box::pin(async move { self.current(Some(rejected)).await.map(Some) })
    }
}

fn unavailable(reason: &str) -> ClaudeAuthUnavailable {
    tracing::warn!(target: "nanocode_claude", reason, "Claude Code login unavailable");
    ClaudeAuthUnavailable::new(reason.to_owned())
}

impl ClaudeClient {
    pub fn with_api_key(endpoint: impl Into<String>, api_key: impl Into<String>) -> Self {
        nanocode_tls::install_default_rustls_crypto_provider();
        Self::new(nanocode_http::client(), endpoint, api_key)
    }
}

impl Default for ClaudeClient {
    fn default() -> Self {
        nanocode_tls::install_default_rustls_crypto_provider();
        let http = nanocode_http::client();
        let claude_code = Arc::new(RefreshingClaudeAuth::new(
            Arc::new(ClaudeCodeLogin { http: http.clone() }),
            REFRESH_MARGIN,
        ));
        Self::subscription(http, Arc::new(crate::home::HomeFirst::new(claude_code)))
    }
}
