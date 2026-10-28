use super::{process, selection};
use std::{ffi::OsString, path::Path, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};

pub use nanocode_process_contracts::{ForegroundOutput, OutputSink};

pub fn run_bash<'call>(
    workspace: &'call Path,
    command: &'call str,
    timeout: Duration,
    max_stdout: usize,
    max_stderr: usize,
    session: Option<&'call crate::SessionEnvironment>,
    output: Option<&'call dyn OutputSink>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<ForegroundOutput, String>> + Send + 'call>,
> {
    run_bash_with_environment(
        workspace,
        command,
        timeout,
        max_stdout,
        max_stderr,
        session,
        &[],
        output,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_bash_with_environment<'call>(
    workspace: &'call Path,
    command: &'call str,
    timeout: Duration,
    max_stdout: usize,
    max_stderr: usize,
    session: Option<&'call crate::SessionEnvironment>,
    environment: &'call [(OsString, OsString)],
    output: Option<&'call dyn OutputSink>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<ForegroundOutput, String>> + Send + 'call>,
> {
    Box::pin(async move {
        let overrides: Vec<_> = environment
            .iter()
            .cloned()
            .chain(
                session
                    .map(crate::SessionEnvironment::os_variables)
                    .unwrap_or_default(),
            )
            .collect();
        let (environment, secrets) = process::sanitized_environment(&overrides, &[]);
        let shell = selection::bash_shell().ok_or("Bash executable not found")?;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let warm = super::warm::spawn(command, workspace, &shell, false, &environment).await;
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let warm = None;
        let mut spawned = match warm {
            Some(spawned) => spawned,
            None => process::spawn(command, workspace, &shell, false, false, &environment)
                .map_err(|error| error.to_string())?,
        };
        struct Cleanup(process::SharedProcessGroup);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.terminate_and_disarm();
            }
        }
        let cleanup = Cleanup(spawned.process_group);
        let process::ProcessOutput::Pipes { stdout, stderr } = spawned.output else {
            return Err("foreground execution requires pipes".into());
        };
        let outcome = tokio::time::timeout(timeout, async {
            let (status, stdout, stderr) = tokio::join!(
                spawned.child.wait(),
                capture(stdout, max_stdout, output, &secrets),
                capture(stderr, max_stderr, output, &secrets)
            );
            let (stdout, stdout_cut) = stdout?;
            let (stderr, stderr_cut) = stderr?;
            Ok::<_, std::io::Error>(ForegroundOutput {
                stdout,
                stderr,
                exit_code: status?,
                truncated: stdout_cut || stderr_cut,
            })
        })
        .await;
        drop(cleanup);
        match outcome {
            Ok(result) => result.map_err(|error| error.to_string()),
            Err(_) => {
                let _ = spawned.child.wait().await;
                Err("Bash command timed out".into())
            }
        }
    })
}

async fn capture<R: AsyncRead + Unpin>(
    pipe: Option<R>,
    limit: usize,
    output: Option<&dyn OutputSink>,
    secrets: &nanocode_process_contracts::Redactor,
) -> std::io::Result<(String, bool)> {
    let Some(mut pipe) = pipe else {
        return Ok((String::new(), false));
    };
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = pipe.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        if let Some(output) = output {
            output.write(&crate::redact(&buffer[..read], secrets));
        }
        let keep = read.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok((
        String::from_utf8_lossy(&crate::redact(&bytes, secrets)).into_owned(),
        truncated,
    ))
}
