use std::{
    io,
    os::{
        fd::{AsRawFd, RawFd},
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
        nanocode_sys::unix::set_close_on_exec(output, false)?;
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

fn read(descriptors: impl Iterator<Item = String>) -> ExitCode {
    std::thread::spawn(nanocode_sys::unix::reap_children);
    let readers: Vec<_> = descriptors
        .filter_map(|descriptor| descriptor.parse::<RawFd>().ok())
        .filter_map(|descriptor| nanocode_sys::unix::inherited_descriptor(descriptor).ok())
        .map(|output| {
            std::thread::spawn(move || {
                if let Ok(flags) = nanocode_sys::unix::status_flags(&output) {
                    let _ = nanocode_sys::unix::set_status_flags(&output, flags & !libc::O_NONBLOCK);
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
