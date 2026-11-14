use std::{io, process::Stdio, time::Duration};

use futures_util::StreamExt;
use nanocode_mcp::{Inbound, Message, Outgoing, RpcError, jsonrpc::PARSE_ERROR};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStderr, ChildStdout, Command},
};

use nanocode_process::ProcessGroupGuard;

const MAX_MCP_STDIO_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const MCP_STDIO_EXIT_TIMEOUT: Duration = Duration::from_secs(2);
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

pub fn spawn(
    mut command: Command,
    outgoing: Outgoing,
    inbound: Inbound,
) -> io::Result<Option<ChildStderr>> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("spawned MCP server without a process identifier"))?;
    let process_group = ProcessGroupGuard::new(pid);
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("spawned MCP server without stdout"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("spawned MCP server without stdin"))?;
    let stderr = child.stderr.take();

    drop(nanocode_platform::spawn_task(read_messages(stdout, inbound.clone())));
    drop(nanocode_platform::spawn_task(async move {
        let mut outgoing = outgoing;
        while let Some(message) = outgoing.next().await {
            if let Err(error) = write_message(&mut stdin, &message).await {
                nanocode_trace::error!("error writing MCP stdio message: {error}");
                inbound.close();
                break;
            }
        }
        drop(stdin);
        let mut process_group = process_group;
        let terminated = process_group.terminate_and_disarm();
        let waited = match tokio::time::timeout(MCP_STDIO_EXIT_TIMEOUT, child.wait()).await {
            Ok(result) => result.map(|_| ()),
            Err(_) => child.kill().await,
        };
        if let Err(error) = terminated.and(waited) {
            nanocode_trace::debug!("failed to stop MCP stdio server: {error}");
        }
    }));
    Ok(stderr)
}

async fn write_message(
    stdin: &mut tokio::process::ChildStdin,
    message: &Message,
) -> io::Result<()> {
    let mut line = message.to_vec().map_err(io::Error::other)?;
    line.push(b'\n');
    stdin.write_all(&line).await?;
    stdin.flush().await
}

async fn read_messages(stdout: ChildStdout, inbound: Inbound) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        match read_line(&mut reader, &mut line).await {
            Ok(false) => break,
            Ok(true) => {}
            Err(error) => {
                nanocode_trace::error!("error reading MCP stdio message: {error}");
                break;
            }
        }
        let content = line.strip_suffix(b"\n").unwrap_or(&line);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        let content = content.strip_prefix(UTF8_BOM).unwrap_or(content);
        match Message::from_slice(content) {
            Ok(message) => {
                if !inbound.deliver(message) {
                    return;
                }
            }
            Err(error) if ignorable_notification(content) => {
                nanocode_trace::trace!("ignoring non-standard MCP stdio notification: {error}");
            }
            Err(error) => {
                nanocode_trace::debug!("parse error on incoming MCP stdio message: {error}");
                let reply = Message::Error {
                    id: None,
                    error: RpcError::new(PARSE_ERROR, "Parse error"),
                };
                if !inbound.send(reply) {
                    return;
                }
            }
        }
    }
    inbound.close();
}

fn ignorable_notification(content: &[u8]) -> bool {
    serde_json::from_slice::<Value>(content).is_ok_and(|value| {
        value.get("id").is_none() && value.get("method").is_some_and(Value::is_string)
    })
}

async fn read_line(reader: &mut BufReader<ChildStdout>, line: &mut Vec<u8>) -> io::Result<bool> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(!line.is_empty());
        }
        let (taken, complete) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (index + 1, true),
            None => (available.len(), false),
        };
        if line.len().saturating_add(taken) > MAX_MCP_STDIO_MESSAGE_BYTES.saturating_add(1) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "max line length exceeded",
            ));
        }
        line.extend_from_slice(&available[..taken]);
        reader.consume(taken);
        if complete {
            return Ok(true);
        }
    }
}
