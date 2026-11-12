use std::{
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::process::CommandExt,
    },
    process::{Command, ExitCode, Stdio},
};

pub const ARGUMENT: &str = "read-outputs";

pub const TERMINATED: u8 = 128 + libc::SIGTERM as u8;

fn reads(command: &mut Command) -> io::Result<bool> {
    let outputs = super::output::open();
    if outputs.is_empty() {
        return Ok(false);
    }
    command.arg(ARGUMENT);
    for output in &outputs {
        rustix::io::fcntl_setfd(output, rustix::io::FdFlags::empty())?;
        command.arg(output.as_raw_fd().to_string());
    }
    Ok(true)
}

pub fn hand_over(mut command: Command) -> io::Result<u8> {
    if !reads(&mut command)? {
        return Ok(TERMINATED);
    }
    Err(command.exec())
}

pub fn leave_reader(mut command: Command) -> io::Result<bool> {
    if !reads(&mut command)? {
        return Ok(false);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    Ok(true)
}

pub fn read_outputs_if_requested() -> Option<ExitCode> {
    let mut arguments = std::env::args().skip(1);
    (arguments.next()? == ARGUMENT).then(|| read(arguments))
}

#[allow(unsafe_code)]
fn read(descriptors: impl Iterator<Item = String>) -> ExitCode {
    std::thread::spawn(|| {
        while !matches!(
            rustix::process::wait(rustix::process::WaitOptions::empty()),
            Err(rustix::io::Errno::CHILD)
        ) {}
    });
    let readers: Vec<_> = descriptors
        .filter_map(|descriptor| descriptor.parse::<RawFd>().ok())
        .map(|descriptor| unsafe { OwnedFd::from_raw_fd(descriptor) })
        .map(|output| {
            std::thread::spawn(move || {
                if let Ok(flags) = rustix::fs::fcntl_getfl(&output) {
                    let _ = rustix::fs::fcntl_setfl(&output, flags - rustix::fs::OFlags::NONBLOCK);
                }
                let _ = io::copy(&mut std::fs::File::from(output), &mut io::sink());
            })
        })
        .collect();
    for reader in readers {
        let _ = reader.join();
    }
    ExitCode::from(TERMINATED)
}
