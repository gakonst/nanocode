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
        let shell = selection::chosen().ok_or("this host has not made psh its shell")?;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let warm = match super::embedded::spawn(
            command,
            workspace,
            &environment,
            super::embedded::Streams::Apart,
        ) {
            Some(spawned) => Some(spawned),
            None => super::warm::spawn(command, workspace, &shell, false, &environment).await,
        };
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
        let outcome = nanocode_platform::timeout(timeout, async {
            match spawned.output {
                process::ProcessOutput::Pipes { stdout, stderr } => {
                    let captures = (
                        Capture::new(stdout, max_stdout),
                        Capture::new(stderr, max_stderr),
                    );
                    collect(&mut spawned.child, captures, output, &secrets).await
                }
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                process::ProcessOutput::Streams { stdout, stderr } => {
                    let captures = (
                        Capture::new(Some(stdout), max_stdout),
                        Capture::new(Some(stderr), max_stderr),
                    );
                    collect(&mut spawned.child, captures, output, &secrets).await
                }
                _ => Err(std::io::Error::other("foreground execution requires pipes")),
            }
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

async fn collect<O: Pipe, E: Pipe>(
    child: &mut process::ProcessChild,
    (mut stdout, mut stderr): (Capture<O>, Capture<E>),
    output: Option<&dyn OutputSink>,
    secrets: &nanocode_process_contracts::Redactor,
) -> std::io::Result<ForegroundOutput> {
    let exit_code = {
        let read = async {
            nanocode_sync::try_join!(stdout.read(output, secrets), stderr.read(output, secrets))
        };
        let mut read = std::pin::pin!(read);
        nanocode_sync::select! {
            status = child.wait() => status?,
            ended = &mut read => {
                ended?;
                child.wait().await?
            }
        }
    };
    stdout.rest(output, secrets)?;
    stderr.rest(output, secrets)?;
    Ok(ForegroundOutput {
        truncated: stdout.truncated || stderr.truncated,
        stdout: stdout.text(secrets),
        stderr: stderr.text(secrets),
        exit_code,
    })
}

trait Pipe: AsyncRead + process::Ready + Unpin + Send + 'static {}

impl<P: AsyncRead + process::Ready + Unpin + Send + 'static> Pipe for P {}

struct Capture<P> {
    pipe: Option<P>,
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl<P: Pipe> Capture<P> {
    const fn new(pipe: Option<P>, limit: usize) -> Self {
        Self {
            pipe,
            bytes: Vec::new(),
            limit,
            truncated: false,
        }
    }

    fn keep(
        &mut self,
        chunk: &[u8],
        output: Option<&dyn OutputSink>,
        secrets: &nanocode_process_contracts::Redactor,
    ) {
        if let Some(output) = output {
            output.write(&crate::redact(chunk, secrets));
        }
        let keep = chunk.len().min(self.limit.saturating_sub(self.bytes.len()));
        self.bytes.extend_from_slice(&chunk[..keep]);
        self.truncated |= keep < chunk.len();
    }

    async fn read(
        &mut self,
        output: Option<&dyn OutputSink>,
        secrets: &nanocode_process_contracts::Redactor,
    ) -> std::io::Result<()> {
        let mut buffer = [0_u8; crate::output::READ_BUFFER_LENGTH];
        while let Some(pipe) = self.pipe.as_mut() {
            match pipe.read(&mut buffer).await? {
                0 => self.pipe = None,
                read => self.keep(&buffer[..read], output, secrets),
            }
        }
        Ok(())
    }

    fn rest(
        &mut self,
        output: Option<&dyn OutputSink>,
        secrets: &nanocode_process_contracts::Redactor,
    ) -> std::io::Result<()> {
        let mut buffer = [0_u8; crate::output::READ_BUFFER_LENGTH];
        while let Some(mut pipe) = self.pipe.take() {
            match pipe.ready(&mut buffer)? {
                None => {
                    let open = pipe.open();
                    drop(nanocode_platform::spawn_task(crate::output::discard(
                        pipe, open,
                    )));
                }
                Some(0) => {}
                Some(read) => {
                    self.keep(&buffer[..read], output, secrets);
                    self.pipe = Some(pipe);
                }
            }
        }
        Ok(())
    }

    fn text(&self, secrets: &nanocode_process_contracts::Redactor) -> String {
        String::from_utf8_lossy(&crate::redact(&self.bytes, secrets)).into_owned()
    }
}
