#[cfg(unix)]
pub const ARGUMENT: &str = "psh-shell";

#[cfg(unix)]
#[must_use]
pub fn serve_if_requested(
    psh: impl FnOnce(Vec<std::ffi::OsString>) -> std::process::ExitCode,
) -> Option<std::process::ExitCode> {
    let mut arguments = std::env::args_os().skip(1);
    (arguments.next()? == ARGUMENT).then(|| psh(arguments.collect()))
}

#[cfg(unix)]
pub fn shell() -> std::io::Result<nanocode_home::shell::Shell> {
    Ok(nanocode_home::shell::Shell::psh(
        std::env::current_exe()?,
        vec![ARGUMENT.to_owned()],
    ))
}
