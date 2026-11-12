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
    let (program, mut arguments) = started()?;
    arguments.push(ARGUMENT.to_owned());
    Ok(nanocode_home::shell::Shell::psh(program, arguments))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn started() -> std::io::Result<(std::path::PathBuf, Vec<String>)> {
    Ok((std::env::current_exe()?, Vec::new()))
}

#[cfg(target_os = "linux")]
fn started() -> std::io::Result<(std::path::PathBuf, Vec<String>)> {
    use std::os::unix::ffi::OsStrExt;

    let executable = std::env::current_exe()?;
    let line = std::fs::read("/proc/self/cmdline")?;
    let given: Vec<&[u8]> = line
        .strip_suffix(&[0])
        .unwrap_or(&line)
        .split(|byte| *byte == 0)
        .collect();
    let loader = given.len().saturating_sub(std::env::args_os().count());
    if loader == 0 {
        return Ok((executable, Vec::new()));
    }
    let program = std::fs::canonicalize(std::ffi::OsStr::from_bytes(given[loader]))?;
    given[1..loader]
        .iter()
        .map(|word| word.to_vec())
        .chain([program.into_os_string().into_encoded_bytes()])
        .map(String::from_utf8)
        .collect::<Result<_, _>>()
        .map(|arguments| (executable, arguments))
        .map_err(std::io::Error::other)
}
