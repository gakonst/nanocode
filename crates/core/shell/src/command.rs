use crate::{
    answers,
    file::{Capture, Source},
    fs::{FileSystem, Remove, Reply, Stat, Write, normalize},
    host::{Host, Vfs},
    shell::{Limit, Limits},
};
use bytes::Bytes;
use psh_engine::{ControlHandle, HostFile, ShellConfig, util::UtilPolicy};
use std::{
    any::Any,
    future::{Future, poll_fn},
    io,
    pin::Pin,
    rc::Rc,
    sync::Arc,
    task::{Context, Poll, Waker},
};

pub trait Commands {
    fn has(&self, name: &[u8]) -> bool;
    fn run(&self, call: Call) -> Pin<Box<dyn Future<Output = i32>>>;
}

pub struct NoCommands;

impl Commands for NoCommands {
    fn has(&self, _name: &[u8]) -> bool {
        false
    }

    fn run(&self, _call: Call) -> Pin<Box<dyn Future<Output = i32>>> {
        Box::pin(std::future::ready(127))
    }
}

pub struct Call {
    pub argv: Vec<Vec<u8>>,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub cwd: Vec<u8>,
    pub stdin: Stream,
    pub stdout: Stream,
    pub stderr: Stream,
    pub shell: Session,
}

pub struct Child {
    pub argv: Vec<Vec<u8>>,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub cwd: Vec<u8>,
    pub stdin: Bytes,
    pub merged: bool,
}

#[derive(Debug)]
pub struct Exited {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

const NOT_FOUND: i32 = 127;
const PROGRAMS: [&[u8]; 2] = [b"/usr/bin/", b"/bin/"];

fn program(path: &[u8]) -> &[u8] {
    PROGRAMS
        .iter()
        .find_map(|directory| path.strip_prefix(*directory))
        .filter(|name| !name.contains(&b'/'))
        .unwrap_or(path)
}

fn quoted(argv: &[Vec<u8>]) -> Vec<u8> {
    let mut source = Vec::new();
    for word in argv {
        if !source.is_empty() {
            source.push(b' ');
        }
        source.push(b'\'');
        for &byte in word {
            if byte == b'\'' {
                source.extend_from_slice(b"'\\''");
            } else {
                source.push(byte);
            }
        }
        source.push(b'\'');
    }
    source
}

#[derive(Clone)]
pub struct Session(pub(crate) Rc<Vfs>);

impl Session {
    pub fn files(&self) -> Rc<dyn FileSystem> {
        Rc::new(Tracked(self.0.clone()))
    }

    pub fn limits(&self) -> Limits {
        self.0.limits
    }

    pub fn fuel(&self) -> u64 {
        self.0.left.get()
    }

    pub fn spend(&self, fuel: u64) {
        let left = self.0.left.get().saturating_sub(fuel);
        self.0.children.set(self.0.children.get() + fuel);
        self.0.left.set(left);
        if left == 0 {
            self.0.refuse(Limit::Fuel);
        }
    }

    pub fn lacks(&self, name: &[u8]) {
        self.0.lacks(name);
    }

    pub fn stop(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let control = self.0.control.get().cloned();
        Arc::new(move || control.as_ref().is_some_and(ControlHandle::cancelled))
    }

    pub fn run(&self, child: Child) -> io::Result<Exited> {
        let vfs = &self.0;
        let mut argv = child.argv;
        let name = program(argv.first().ok_or(io::ErrorKind::InvalidInput)?).to_vec();
        argv[0].clone_from(&name);
        let source = quoted(&argv);
        if let Some(answers) = &vfs.answers
            && let Err(what) = answers::admit(&source, answers)
        {
            return Err(vfs.unanswer(what.as_bytes()));
        }
        let stdin = Rc::new(Source::new(vfs.clone()));
        stdin.set(Some(child.stdin));
        let stdout = Rc::new(Capture::new(vfs.limits.output));
        let stderr = if child.merged {
            stdout.clone()
        } else {
            Rc::new(Capture::new(vfs.limits.output))
        };
        let mut config = ShellConfig::new(Rc::new(Host(vfs.clone())));
        config.env = child.env;
        config.cwd = child.cwd;
        config.stdin = Some(stdin as Rc<dyn HostFile>);
        config.stdout = Some(stdout.clone() as Rc<dyn HostFile>);
        config.stderr = Some(stderr.clone() as Rc<dyn HostFile>);
        config.max_depth = vfs.limits.depth;
        config.utilities = UtilPolicy {
            entries: vfs.utilities,
            sandbox: true,
            off_names: vfs.without.clone(),
            unconfigured: vfs.answers.is_some(),
            ..UtilPolicy::default()
        };
        let mut engine = psh_engine::Shell::new(config);
        engine.start(psh_engine::Script {
            source,
            arg0: name.clone(),
            args: Vec::new(),
            dash_c: true,
            options: Vec::new(),
            stdin: false,
        });
        let known = vfs.missing.borrow().len();
        let mut charged = 0;
        let status = loop {
            let left = vfs.left.get();
            let slice =
                u32::try_from(left).map_or(vfs.slice.get(), |left| left.min(vfs.slice.get()));
            if left == 0 {
                vfs.refuse(Limit::Fuel);
            }
            if slice == 0 || vfs.control.get().is_some_and(ControlHandle::cancelled) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let step = engine.step(slice);
            let spent = engine.spent();
            vfs.children.set(vfs.children.get() + spent - charged);
            vfs.left.set(left.saturating_sub(spent - charged));
            charged = spent;
            match step {
                psh_engine::Step::Done(status) => break status,
                psh_engine::Step::Yield => {}
                psh_engine::Step::Blocked | psh_engine::Step::Idle => {
                    vfs.lacks(&name);
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "this shell cannot wait for a command that Python starts",
                    ));
                }
            }
        };
        if status == NOT_FOUND && vfs.missing.borrow()[known..].contains(&name) {
            return Err(io::ErrorKind::NotFound.into());
        }
        let stdout = stdout.take().0;
        let stderr = stderr.take().0;
        Ok(Exited {
            status,
            stdout,
            stderr,
        })
    }
}

struct Tracked(Rc<Vfs>);

impl FileSystem for Tracked {
    fn stat(&self, path: &[u8], follow: bool) -> Reply<Stat> {
        self.0.stat(&normalize(path), follow)
    }

    fn read(&self, path: &[u8], _limit: u64) -> Reply<Bytes> {
        self.0.read(&normalize(path))
    }

    fn write(&self, path: &[u8], data: &[u8], how: Write) -> Reply<()> {
        self.0.write(&normalize(path), data, how)
    }

    fn list(&self, path: &[u8]) -> Reply<Vec<Vec<u8>>> {
        self.0.list(&normalize(path))
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> Reply<()> {
        self.0.mkdir(normalize(path), mode)
    }

    fn remove(&self, path: &[u8], what: Remove) -> Reply<()> {
        self.0.remove(normalize(path), what)
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> Reply<()> {
        self.0.rename(normalize(from), normalize(to))
    }

    fn symlink(&self, target: &[u8], link: &[u8]) -> Reply<()> {
        self.0.symlink(target, normalize(link))
    }

    fn read_link(&self, path: &[u8]) -> Reply<Vec<u8>> {
        self.0.read_link(&normalize(path))
    }

    fn set_mode(&self, path: &[u8], mode: u32) -> Reply<()> {
        self.0.set_mode(&normalize(path), mode)
    }

    fn set_mtime(&self, path: &[u8], mtime_ns: i64) -> Reply<()> {
        self.0.set_mtime(&normalize(path), mtime_ns)
    }
}

#[derive(Clone)]
pub struct Stream(pub(crate) Rc<dyn HostFile>);

impl Stream {
    pub fn captured(&self) -> bool {
        let file: &dyn Any = self.0.as_ref();
        file.is::<Capture>()
    }

    pub fn unanswered(&self) -> bool {
        let file: &dyn Any = self.0.as_ref();
        file.downcast_ref::<Source>()
            .is_some_and(Source::unanswered)
    }

    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        poll_fn(|cx| self.0.poll_read(cx, buf)).await
    }

    pub fn read_ready(&self, buf: &mut [u8]) -> Option<io::Result<usize>> {
        match self
            .0
            .poll_read(&mut Context::from_waker(Waker::noop()), buf)
        {
            Poll::Ready(read) => Some(read),
            Poll::Pending => None,
        }
    }

    pub async fn read_to_end(&self, out: &mut Vec<u8>) -> io::Result<()> {
        let mut chunk = vec![0; crate::file::PIPE_BYTES];
        loop {
            let read = self.read(&mut chunk).await?;
            if read == 0 {
                return Ok(());
            }
            out.extend_from_slice(&chunk[..read]);
        }
    }

    pub async fn write_all(&self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let wrote = poll_fn(|cx| self.0.poll_write(cx, data)).await?;
            if wrote == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            data = &data[wrote..];
        }
        Ok(())
    }
}
