#[cfg(not(target_family = "wasm"))]
mod support;

#[cfg(not(target_family = "wasm"))]
async fn run() -> support::Result<()> {
    use nanocode::code_mode::{
        attachment::AttachmentTarget,
        remote::{SocketPolicy, SocketTransport},
        runtime::ToolRuntime,
    };
    use nanocode::tool_contract::ToolContext;

    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "embedded".into());
    if mode == "--help" {
        println!(
            "cargo run --locked -p nanocode --example code_mode -- [embedded|connect URL|accept ADDRESS] [JAVASCRIPT...]\nEach JavaScript argument runs as a separate cell in one session. No inference is used.\nRemote mode requires NANOCODE_RUNNER_TOKEN, NANOCODE_RUNNER_MAX_FRAME_BYTES and NANOCODE_RUNNER_TIMEOUT_MS.\nRun the runner example separately; registered Rust tools are callable through the same connection."
        );
        return Ok(());
    }
    let mut tools = support::tools()?;
    match mode.as_str() {
        "embedded" => {}
        "connect" | "accept" => {
            let endpoint = args.next().ok_or("remote mode requires an endpoint")?;
            let token = std::env::var("NANOCODE_RUNNER_TOKEN")?;
            let policy = SocketPolicy {
                max_frame_bytes: std::env::var("NANOCODE_RUNNER_MAX_FRAME_BYTES")?.parse()?,
                request_timeout: std::time::Duration::from_millis(
                    std::env::var("NANOCODE_RUNNER_TIMEOUT_MS")?.parse()?,
                ),
            };
            let transport = if mode == "connect" {
                SocketTransport::connect(AttachmentTarget::new(endpoint, token)?, policy).await?
            } else {
                let listener = nanocode_executor::TcpListener::bind(endpoint).await?;
                eprintln!("listening {}", listener.local_addr()?);
                SocketTransport::accept(&listener, &token, policy).await?
            };
            tools = tools.code_mode_host(transport.into_host());
        }
        _ => return Err("expected embedded, connect or accept; use --help".into()),
    }
    let runtime = ToolRuntime::new_with_tools(std::env::current_dir()?, Vec::new(), &tools);
    let mut sources: Vec<String> = args.collect();
    if sources.is_empty() {
        sources = vec![
            "store('answer', await tools.checked_sum({a:19,b:23})); text(load('answer'));".into(),
            "text({remembered:load('answer'),quotient:await tools.quotient({a:84,b:2})});".into(),
        ];
    }
    let outcome: support::Result<()> = async {
        for (index, source) in sources.iter().enumerate() {
            let call = index.to_string();
            let result = runtime
                .execute_code(
                    source,
                    ToolContext::new("sdk-example", "code-mode-example", &call, &[], usize::MAX),
                )
                .await?;
            println!(
                "{}",
                serde_json::json!({"cell":index,"success":result.success,"output":result.output,"nested_calls":result.nested_calls.len()})
            );
            if !result.success {
                return Err("Code Mode cell failed".into());
            }
        }
        Ok(())
    }
    .await;
    runtime.control().cancel().await;
    outcome
}

#[cfg(not(target_family = "wasm"))]
fn main() {
    support::main(run());
}

#[cfg(target_family = "wasm")]
fn main() {}
