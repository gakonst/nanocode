use std::{cell::RefCell, io, rc::Rc, time::Duration};

use nanocode_platform::{Instant, UNIX_EPOCH};

use crate::{Changes, Files, Limit, Limits, Run};

pub(crate) use _host::module_def;

pub struct State {
    files: Rc<dyn Files>,
    stdin: Vec<u8>,
    read: usize,
    cwd: Vec<u8>,
    env: Vec<(Vec<u8>, Vec<u8>)>,
    limits: Limits,
    slept: Duration,
    started: Instant,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub changes: Changes,
    pub refused: Option<Limit>,
}

impl State {
    pub fn new(files: Rc<dyn Files>, run: Run) -> Self {
        Self {
            files,
            stdin: run.stdin,
            read: 0,
            cwd: run.cwd,
            env: run.env,
            limits: run.limits,
            slept: Duration::ZERO,
            started: Instant::now(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            changes: Changes::default(),
            refused: None,
        }
    }
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub struct Entered;

impl Entered {
    pub fn leave(self) -> State {
        STATE
            .with_borrow_mut(Option::take)
            .expect("a run leaves the state it entered")
    }
}

impl Drop for Entered {
    fn drop(&mut self) {
        STATE.with_borrow_mut(|state| *state = None);
    }
}

pub fn enter(state: State) -> Entered {
    STATE.with_borrow_mut(|slot| *slot = Some(state));
    Entered
}

fn with<T>(run: impl FnOnce(&mut State) -> io::Result<T>) -> io::Result<T> {
    STATE.with_borrow_mut(|state| match state {
        Some(state) => run(state),
        None => Err(io::Error::other("no Python run is active")),
    })
}

pub fn input() -> Vec<u8> {
    with(|state| {
        let rest = state.stdin[state.read..].to_vec();
        state.read = state.stdin.len();
        Ok(rest)
    })
    .unwrap_or_default()
}

pub fn source(path: &str) -> io::Result<Vec<u8>> {
    with(|state| {
        let path = absolute(&state.cwd, path.as_bytes());
        state.files.read(&path, state.limits.file)
    })
}

pub fn say(text: &str) {
    let _ = with(|state| {
        state.stdout.extend_from_slice(text.as_bytes());
        Ok(())
    });
}

pub fn print(text: &str) {
    let _ = with(|state| {
        state.stderr.extend_from_slice(text.as_bytes());
        Ok(())
    });
}

pub fn text(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "No such file or directory".to_owned(),
        io::ErrorKind::PermissionDenied => "Permission denied".to_owned(),
        io::ErrorKind::AlreadyExists => "File exists".to_owned(),
        io::ErrorKind::NotADirectory => "Not a directory".to_owned(),
        io::ErrorKind::IsADirectory => "Is a directory".to_owned(),
        io::ErrorKind::DirectoryNotEmpty => "Directory not empty".to_owned(),
        io::ErrorKind::FileTooLarge => "File too large".to_owned(),
        io::ErrorKind::ReadOnlyFilesystem => "Read-only file system".to_owned(),
        _ => error.to_string(),
    }
}

fn absolute(cwd: &[u8], path: &[u8]) -> Vec<u8> {
    let mut joined = Vec::new();
    if !path.starts_with(b"/") {
        joined.extend_from_slice(cwd);
        joined.push(b'/');
    }
    joined.extend_from_slice(path);
    let mut out = Vec::new();
    for part in joined.split(|&byte| byte == b'/') {
        match part {
            b"" | b"." => {}
            b".." => {
                let cut = out.iter().rposition(|&byte| byte == b'/').unwrap_or(0);
                out.truncate(cut);
            }
            _ => {
                out.push(b'/');
                out.extend_from_slice(part);
            }
        }
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

#[cfg(not(target_family = "wasm"))]
fn wait(duration: Duration) -> io::Result<()> {
    std::thread::sleep(duration);
    Ok(())
}

#[cfg(target_family = "wasm")]
fn wait(_: Duration) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

#[rustpython_vm::pymodule]
mod _host {
    use super::{Limit, State, UNIX_EPOCH, text, wait, with};
    use crate::{Kind, Remove, Write};
    use rustpython_vm::{
        PyObjectRef, PyResult, VirtualMachine,
        builtins::{PyBaseExceptionRef, PyBytesRef},
        function::ArgBytesLike,
    };
    use std::{io, time::Duration};

    fn failure(vm: &VirtualMachine, error: &io::Error, path: Option<&[u8]>) -> PyBaseExceptionRef {
        let exceptions = &vm.ctx.exceptions;
        let (kind, name, code) = match error.kind() {
            io::ErrorKind::NotFound => (exceptions.file_not_found_error, "ENOENT", 2),
            io::ErrorKind::PermissionDenied => (exceptions.permission_error, "EACCES", 13),
            io::ErrorKind::AlreadyExists => (exceptions.file_exists_error, "EEXIST", 17),
            io::ErrorKind::NotADirectory => (exceptions.not_a_directory_error, "ENOTDIR", 20),
            io::ErrorKind::IsADirectory => (exceptions.is_a_directory_error, "EISDIR", 21),
            io::ErrorKind::DirectoryNotEmpty => (exceptions.os_error, "ENOTEMPTY", 39),
            io::ErrorKind::FileTooLarge => (exceptions.os_error, "EFBIG", 27),
            io::ErrorKind::ReadOnlyFilesystem => (exceptions.os_error, "EROFS", 30),
            io::ErrorKind::InvalidInput => (exceptions.os_error, "EINVAL", 22),
            io::ErrorKind::Unsupported => (exceptions.os_error, "ENOTSUP", 95),
            _ => (exceptions.os_error, "EIO", 5),
        };
        let code = vm
            .import("errno", 0)
            .and_then(|errno| errno.get_attr(name, vm))
            .unwrap_or_else(|_| vm.ctx.new_int(code).into());
        let mut args: Vec<PyObjectRef> = vec![code, vm.ctx.new_str(text(error)).into()];
        if let Some(path) = path {
            let path = String::from_utf8_lossy(path).into_owned();
            args.push(vm.ctx.new_str(path).into());
        }
        vm.new_exception(kind.to_owned(), args)
    }

    fn state<T>(vm: &VirtualMachine, run: impl FnOnce(&mut State) -> io::Result<T>) -> PyResult<T> {
        with(run).map_err(|error| failure(vm, &error, None))
    }

    fn files<T>(
        vm: &VirtualMachine,
        path: &[u8],
        run: impl FnOnce(&mut State) -> io::Result<T>,
    ) -> PyResult<T> {
        with(run).map_err(|error| failure(vm, &error, Some(path)))
    }

    #[pyfunction]
    fn env(vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        let env = state(vm, |state| Ok(state.env.clone()))?;
        let pairs = env
            .into_iter()
            .map(|(name, value)| {
                let pair = vec![
                    vm.ctx.new_bytes(name).into(),
                    vm.ctx.new_bytes(value).into(),
                ];
                vm.ctx.new_tuple(pair).into()
            })
            .collect();
        Ok(vm.ctx.new_list(pairs).into())
    }

    #[pyfunction]
    fn cwd(vm: &VirtualMachine) -> PyResult<PyBytesRef> {
        state(vm, |state| Ok(state.cwd.clone())).map(|cwd| vm.ctx.new_bytes(cwd))
    }

    #[pyfunction]
    fn file_limit(vm: &VirtualMachine) -> PyResult<u64> {
        state(vm, |state| Ok(state.limits.file))
    }

    #[pyfunction]
    fn stat(path: ArgBytesLike, vm: &VirtualMachine) -> PyResult<(u8, u64, u32, i64)> {
        let path = path.borrow_buf();
        files(vm, &path, |state| state.files.stat(&path)).map(|stat| {
            let kind = match stat.kind {
                Kind::File => 0,
                Kind::Dir => 1,
                Kind::Symlink => 2,
            };
            (kind, stat.size, stat.mode, stat.mtime_ns)
        })
    }

    #[pyfunction]
    fn read(path: ArgBytesLike, vm: &VirtualMachine) -> PyResult<PyBytesRef> {
        let path = path.borrow_buf();
        files(vm, &path, |state| {
            let data = state.files.read(&path, state.limits.file)?;
            if data.len() as u64 > state.limits.file {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            Ok(data)
        })
        .map(|data| vm.ctx.new_bytes(data))
    }

    #[pyfunction]
    fn write(path: ArgBytesLike, data: ArgBytesLike, how: u8, vm: &VirtualMachine) -> PyResult<()> {
        let path = path.borrow_buf();
        let data = data.borrow_buf();
        files(vm, &path, |state| {
            if data.len() as u64 > state.limits.file {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            let how = match how {
                0 => Write::Replace,
                1 => Write::Append,
                _ => Write::New,
            };
            state.files.write(&path, &data, how)?;
            state.changes.removed.remove(&*path);
            state.changes.written.insert(path.to_vec());
            Ok(())
        })
    }

    #[pyfunction]
    fn list(path: ArgBytesLike, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        let path = path.borrow_buf();
        let names = files(vm, &path, |state| state.files.list(&path))?;
        let names = names
            .into_iter()
            .map(|name| vm.ctx.new_bytes(name).into())
            .collect();
        Ok(vm.ctx.new_list(names).into())
    }

    #[pyfunction]
    fn mkdir(path: ArgBytesLike, mode: u32, vm: &VirtualMachine) -> PyResult<()> {
        let path = path.borrow_buf();
        files(vm, &path, |state| {
            state.files.mkdir(&path, mode)?;
            state.changes.removed.remove(&*path);
            state.changes.made.insert(path.to_vec());
            Ok(())
        })
    }

    #[pyfunction]
    fn remove(path: ArgBytesLike, dir: bool, vm: &VirtualMachine) -> PyResult<()> {
        let path = path.borrow_buf();
        files(vm, &path, |state| {
            let what = if dir { Remove::Dir } else { Remove::File };
            state.files.remove(&path, what)?;
            state.changes.written.remove(&*path);
            state.changes.made.remove(&*path);
            state.changes.removed.insert(path.to_vec());
            Ok(())
        })
    }

    #[pyfunction]
    fn rename(from: ArgBytesLike, to: ArgBytesLike, vm: &VirtualMachine) -> PyResult<()> {
        let from = from.borrow_buf();
        let to = to.borrow_buf();
        files(vm, &from, |state| {
            state.files.rename(&from, &to)?;
            state.changes.moved.push((from.to_vec(), to.to_vec()));
            Ok(())
        })
    }

    #[pyfunction]
    fn input(size: isize, vm: &VirtualMachine) -> PyResult<PyBytesRef> {
        state(vm, |state| {
            let rest = state.stdin.len() - state.read;
            let take = usize::try_from(size).map_or(rest, |size| size.min(rest));
            let data = state.stdin[state.read..state.read + take].to_vec();
            state.read += take;
            Ok(data)
        })
        .map(|data| vm.ctx.new_bytes(data))
    }

    #[pyfunction]
    fn output(fd: u8, data: ArgBytesLike, vm: &VirtualMachine) -> PyResult<usize> {
        let data = data.borrow_buf();
        state(vm, |state| {
            let used = (state.stdout.len() + state.stderr.len() + data.len()) as u64;
            if used > state.limits.output {
                state.refused.get_or_insert(Limit::Output);
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            let stream = if fd == 2 {
                &mut state.stderr
            } else {
                &mut state.stdout
            };
            stream.extend_from_slice(&data);
            Ok(data.len())
        })
    }

    #[pyfunction]
    fn random(size: usize, vm: &VirtualMachine) -> PyResult<PyBytesRef> {
        let mut bytes = vec![0; size];
        nanocode_platform::fill_random(&mut bytes).map_err(|error| failure(vm, &error, None))?;
        Ok(vm.ctx.new_bytes(bytes))
    }

    #[pyfunction]
    fn now() -> f64 {
        nanocode_platform::system_time()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64())
    }

    #[pyfunction]
    fn monotonic(vm: &VirtualMachine) -> PyResult<f64> {
        state(vm, |state| Ok(state.started.elapsed().as_secs_f64()))
    }

    #[pyfunction]
    fn sleep(seconds: f64, vm: &VirtualMachine) -> PyResult<()> {
        let duration = Duration::try_from_secs_f64(seconds)
            .map_err(|_| vm.new_value_error("sleep length must be non-negative"))?;
        state(vm, |state| {
            let slept = state.slept.saturating_add(duration);
            if slept > state.limits.sleep {
                state.refused.get_or_insert(Limit::Sleep);
                return Err(io::ErrorKind::TimedOut.into());
            }
            wait(duration).inspect_err(|_| {
                state.refused.get_or_insert(Limit::Sleep);
            })?;
            state.slept = slept;
            Ok(())
        })
    }
}
