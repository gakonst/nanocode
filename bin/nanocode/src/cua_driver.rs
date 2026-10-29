use std::process::ExitCode;

#[cfg(target_os = "macos")]
#[tokio::main]
async fn main() -> ExitCode {
    let report = |permissions: nanocode_cua::embedded::Permissions| {
        let state = |granted| if granted { "granted" } else { "not granted" };
        println!(
            "accessibility: {}\nscreen recording: {}",
            state(permissions.accessibility),
            state(permissions.screen_recording)
        );
        ExitCode::SUCCESS
    };
    match std::env::args().nth(1).as_deref() {
        Some("mcp") => match nanocode_cua::embedded::serve_stdio().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("nanocode-cua-driver: {error}");
                ExitCode::FAILURE
            }
        },
        Some("permissions") => report(nanocode_cua::embedded::permissions()),
        Some("request-permissions") => report(nanocode_cua::embedded::request_permissions()),
        _ => {
            eprintln!("usage: nanocode-cua-driver mcp | permissions | request-permissions");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    eprintln!("nanocode-cua-driver: the built-in cua-driver runs only on macOS");
    ExitCode::FAILURE
}
