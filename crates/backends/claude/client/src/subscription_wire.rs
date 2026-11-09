use crate::{ClaudeError, ContentBlock, Message, MessagesRequest, Role, WireMessage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const CLAUDE_CODE_VERSION: &str = "2.1.304";
pub const CLAUDE_CODE_SDK_VERSION: &str = "0.112.1";
pub const IDENTITY: &str = "You are a Claude agent, built on Anthropic's Claude Agent SDK.";
const LEGACY_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const BILLING: &str = "x-anthropic-billing-header:";
const PLACEHOLDER: &[u8] = b"cch=00000";
const MARKER: &[u8] = b"\"system\":[{\"type\":\"text\",\"text\":\"x-anthropic-billing-header:";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionIdentity {
    pub install_id: Option<String>,
    pub account_uuid: Option<String>,
    pub user_id: Option<String>,
    pub platform: Option<String>,
    pub arch: Option<String>,
    pub version: Option<String>,
}
impl SubscriptionIdentity {
    pub fn validate(&self) -> Result<(), ClaudeError> {
        for value in [
            &self.install_id,
            &self.account_uuid,
            &self.user_id,
            &self.platform,
            &self.arch,
            &self.version,
        ]
        .into_iter()
        .flatten()
        {
            if value.is_empty() || value.len() > 8192 || value.chars().any(char::is_control) {
                return Err(ClaudeError::Protocol(
                    "invalid subscription wire identity".into(),
                ));
            }
        }
        Ok(())
    }
    pub fn version(&self) -> &str {
        self.version.as_deref().unwrap_or(CLAUDE_CODE_VERSION)
    }
    pub fn os(&self) -> String {
        let platform = self
            .platform
            .as_deref()
            .unwrap_or(if cfg!(target_os = "macos") {
                "darwin"
            } else if cfg!(target_os = "windows") {
                "win32"
            } else {
                "linux"
            });
        match platform.to_lowercase().as_str() {
            "darwin" => "MacOS".into(),
            "windows" | "win32" => "Windows".into(),
            "linux" => "Linux".into(),
            "freebsd" => "FreeBSD".into(),
            other => format!("Other::{other}"),
        }
    }
    pub fn arch(&self) -> String {
        let arch = self
            .arch
            .as_deref()
            .unwrap_or(if cfg!(target_arch = "aarch64") {
                "arm64"
            } else if cfg!(target_arch = "x86") {
                "ia32"
            } else {
                "x64"
            });
        match arch.to_lowercase().as_str() {
            "amd64" | "x64" => "x64".into(),
            "arm64" | "aarch64" => "arm64".into(),
            "386" | "x86" | "ia32" => "x86".into(),
            other => format!("other::{other}"),
        }
    }
    pub fn metadata(&self, session: &str) -> Result<Value, ClaudeError> {
        if let Some(user) = &self.user_id {
            let json_id = user.starts_with('{')
                && serde_json::from_str::<Value>(user).ok().is_some_and(|v| {
                    v.get("session_id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty())
                });
            if json_id || crate::subscription_identity::is_legacy_user_id(user) {
                return Ok(json!({"user_id":user}));
            }
        }
        let install = self.install_id.as_deref().unwrap_or(session);
        let input = self.account_uuid.as_ref().map_or_else(
            || format!("omp-claude-device-id-v1:{install}"),
            |account| format!("omp-claude-device-id-v2\0{install}\0{account}"),
        );
        #[derive(Serialize)]
        struct User<'a> {
            device_id: String,
            session_id: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            account_uuid: Option<&'a str>,
        }
        let user = User {
            device_id: hex_digest(input.as_bytes()),
            session_id: session,
            account_uuid: self.account_uuid.as_deref(),
        };
        Ok(json!({"user_id":serde_json::to_string(&user)?}))
    }
}
pub(crate) fn hex_digest(input: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(input);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push(char::from(HEX[usize::from(byte >> 4)]));
        hex.push(char::from(HEX[usize::from(byte & 0xf)]));
    }
    hex
}
pub fn prefix(name: &str) -> String {
    name.into()
}
pub const fn strip(_: &mut String) {}
fn fingerprint<M: WireMessage>(messages: &[M], version: &str) -> String {
    let first = messages
        .iter()
        .map(WireMessage::message)
        .find(|m| m.role == Role::User)
        .and_then(|m| {
            m.content.iter().find_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
        })
        .unwrap_or("");
    let units: Vec<u16> = first.encode_utf16().collect();
    let selected = [4, 7, 20].map(|i| units.get(i).copied().unwrap_or(u16::from(b'0')));
    let seed = format!(
        "59cf53e54c78{}{version}",
        String::from_utf16_lossy(&selected)
    );
    hex_digest(seed.as_bytes())[..3].into()
}
pub fn prepare(request: &mut MessagesRequest, identity: &SubscriptionIdentity) {
    let messages = std::mem::take(&mut request.messages);
    prepare_with(request, &messages, identity);
    request.messages = messages;
}

fn prepare_with<M: WireMessage>(
    request: &mut MessagesRequest,
    messages: &[M],
    identity: &SubscriptionIdentity,
) {
    let mut existing = match request.system.take().map(std::sync::Arc::unwrap_or_clone) {
        Some(Value::String(s)) => vec![json!({"type":"text","text":s})],
        Some(Value::Array(v)) => v,
        Some(v) => vec![v],
        None => vec![],
    };
    let generated = existing
        .first()
        .and_then(|v| v.get("text"))
        .and_then(Value::as_str)
        .is_some_and(|s| s.starts_with(BILLING))
        && matches!(
            existing
                .get(1)
                .and_then(|v| v.get("text"))
                .and_then(Value::as_str),
            Some(IDENTITY | LEGACY_IDENTITY)
        );
    if generated {
        existing.drain(..2);
    } else if existing.iter().any(|v| {
        v.get("text")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with(BILLING))
    }) {
        request.system = Some(std::sync::Arc::new(Value::Array(existing)));
        return;
    }

    if matches!(
        existing
            .first()
            .and_then(|v| v.get("text"))
            .and_then(Value::as_str),
        Some(IDENTITY | LEGACY_IDENTITY)
    ) {
        existing.remove(0);
    }
    existing.retain(|v| v.get("text").and_then(Value::as_str) != Some(""));
    let billing = format!(
        "{BILLING} cc_version={}.{}; cc_entrypoint=cli; cch=00000;",
        identity.version(),
        fingerprint(messages, identity.version())
    );

    let control = request.cache_control.as_ref().map_or_else(
        || {
            if existing.iter().any(|v| {
                v.get("cache_control")
                    .and_then(|c| c.get("ttl"))
                    .and_then(Value::as_str)
                    == Some("1h")
            }) {
                json!({"type":"ephemeral","ttl":"1h"})
            } else {
                json!({"type":"ephemeral"})
            }
        },
        |c| serde_json::to_value(c).expect("cache policy serializes"),
    );
    let mut blocks = vec![
        json!({"type":"text","text":billing}),
        json!({"type":"text","text":IDENTITY,"cache_control":control}),
    ];
    blocks.extend(existing);
    request.system = Some(std::sync::Arc::new(Value::Array(blocks)));
}

fn stringify(value: &Value, out: &mut Vec<u8>) -> Result<(), ClaudeError> {
    match value {
        Value::Array(a) => {
            out.push(b'[');
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                stringify(v, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => stringify_object(map, out, |_, value, out| stringify(value, out))?,
        _ => serde_json::to_writer(&mut *out, value)?,
    }
    Ok(())
}

fn stringify_object(
    map: &serde_json::Map<String, Value>,
    out: &mut Vec<u8>,
    mut field: impl FnMut(&str, &Value, &mut Vec<u8>) -> Result<(), ClaudeError>,
) -> Result<(), ClaudeError> {
    out.push(b'{');
    let mut first = true;
    for key in ["type", "text"]
        .into_iter()
        .filter(|k| map.contains_key(*k))
        .chain(
            map.keys()
                .map(String::as_str)
                .filter(|k| !matches!(*k, "type" | "text")),
        )
    {
        if !first {
            out.push(b',');
        }
        first = false;
        serde_json::to_writer(&mut *out, key)?;
        out.push(b':');
        field(key, &map[key], out)?;
    }
    out.push(b'}');
    Ok(())
}

pub(crate) fn encode_message(message: &Message) -> Result<String, ClaudeError> {
    let mut bytes = Vec::new();
    stringify(&serde_json::to_value(message)?, &mut bytes)?;
    String::from_utf8(bytes)
        .map_err(|_| ClaudeError::Protocol("invalid subscription wire bytes".into()))
}

pub fn body(
    request: &MessagesRequest,
    streaming: bool,
    identity: &SubscriptionIdentity,
    session: &str,
    explicit_cache_tail: bool,
) -> Result<String, ClaudeError> {
    wire(
        request,
        &request.messages,
        streaming,
        identity,
        session,
        explicit_cache_tail,
    )
    .map(|(body, _)| body)
}

pub(crate) fn wire<M: WireMessage>(
    head: &MessagesRequest,
    messages: &[M],
    streaming: bool,
    identity: &SubscriptionIdentity,
    session: &str,
    explicit_cache_tail: bool,
) -> Result<(String, std::ops::Range<usize>), ClaudeError> {
    identity.validate()?;
    let mut prepared = head.head();
    prepare_with(&mut prepared, messages, identity);
    crate::validate_cache_layout(
        &prepared.tools,
        prepared.system.as_deref(),
        messages.iter().map(WireMessage::message),
        prepared.cache_control.as_ref(),
    )?;
    let mut patched = None;
    if explicit_cache_tail && let Some(control) = prepared.cache_control.take() {
        let marker = serde_json::to_value(&control)?;
        match crate::cache_tail(messages) {
            Some((index, block)) => {
                let mut message = messages[index].message().clone();
                message.content[block]
                    .extra_mut()
                    .entry("cache_control".to_owned())
                    .or_insert(marker);
                patched = Some((index, message));
            }
            None => crate::mark_system_tail(
                prepared.system.as_mut().map(std::sync::Arc::make_mut),
                marker,
            ),
        }
    }
    let encoded = messages
        .iter()
        .enumerate()
        .map(|(index, message)| match &patched {
            Some((at, patched)) if *at == index => {
                encode_message(patched).map(std::borrow::Cow::Owned)
            }
            _ => message.encoded(true, &encode_message),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut value = serde_json::to_value(&prepared)?;
    value["stream"] = json!(streaming);
    value["metadata"] = identity.metadata(session)?;
    if value.get("tools").is_none() {
        value["tools"] = json!([]);
    }
    let Value::Object(map) = &value else {
        return Err(ClaudeError::Protocol(
            "subscription wire request is not an object".into(),
        ));
    };
    let mut head = Vec::new();
    let mut at = None;
    stringify_object(map, &mut head, |key, value, out| {
        if key == "messages" {
            at = Some(out.len());
            Ok(())
        } else {
            stringify(value, out)
        }
    })?;
    let at = at
        .ok_or_else(|| ClaudeError::Protocol("subscription wire request has no messages".into()))?;
    let mut bytes = Vec::with_capacity(
        head.len()
            + 2
            + encoded
                .iter()
                .map(|message| message.len() + 1)
                .sum::<usize>(),
    );
    bytes.extend_from_slice(&head[..at]);
    bytes.push(b'[');
    let messages_at = bytes.len();
    for (index, message) in encoded.iter().enumerate() {
        if index > 0 {
            bytes.push(b',');
        }
        bytes.extend_from_slice(message.as_bytes());
    }
    let messages_at = messages_at..bytes.len();
    bytes.push(b']');
    bytes.extend_from_slice(&head[at..]);
    if let Some(marker) = memchr::memmem::find(&bytes, MARKER) {
        let start = marker + MARKER.len();
        let window = &bytes[start..bytes.len().min(start + 150 + PLACEHOLDER.len())];
        if let Some(offset) = memchr::memmem::find(window, PLACEHOLDER) {
            let checksum = format!(
                "{:05x}",
                xxhash_rust::xxh64::xxh64(&bytes, 0x4d65_9218_e32a_3268) & 0xfffff
            );
            bytes[start + offset + 4..start + offset + 9].copy_from_slice(checksum.as_bytes());
        }
    }
    String::from_utf8(bytes)
        .map(|body| (body, messages_at))
        .map_err(|_| ClaudeError::Protocol("invalid subscription wire bytes".into()))
}
