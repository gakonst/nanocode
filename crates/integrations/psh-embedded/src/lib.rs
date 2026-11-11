#![cfg(unix)]

use std::{
    ffi::OsString,
    io,
    os::unix::ffi::OsStringExt,
    sync::Arc,
};

use nanocode_process::{Contained, Embedded, Running};

pub struct Psh;

pub fn embed() -> Result<(), Arc<dyn Embedded>> {
    nanocode_process::embed(Arc::new(Psh))
}

fn bytes(environment: &[(OsString, OsString)]) -> Vec<(Vec<u8>, Vec<u8>)> {
    environment
        .iter()
        .map(|(name, value)| (name.clone().into_vec(), value.clone().into_vec()))
        .collect()
}

impl Embedded for Psh {
    fn contains(&self, script: &str, environment: &[(OsString, OsString)]) -> bool {
        psh::embed::contained(script, &bytes(environment))
    }

    fn start(&self, run: Contained) -> io::Result<Arc<dyn Running>> {
        let ended = run.ended;
        let running = psh::embed::start(
            psh::embed::Command {
                script: run.script.into_bytes(),
                cwd: run.workdir.into_os_string().into_vec(),
                env: bytes(&run.environment),
                stdout: run.stdout,
                stderr: run.stderr,
            },
            move |status| {
                ended(status.map_err(|_| io::Error::other("the shell panicked in this process")));
            },
        )?;
        Ok(Arc::new(Shell(running)))
    }
}

struct Shell(psh::embed::Running);

impl Running for Shell {
    fn interrupt(&self) {
        self.0.interrupt();
    }

    fn stop(&self) {
        self.0.stop();
    }
}
