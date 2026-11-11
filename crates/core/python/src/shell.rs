use std::{
    cell::{OnceCell, RefCell},
    collections::{BTreeMap, BTreeSet},
    future::Future,
    io,
    pin::Pin,
    rc::Rc,
    time::Duration,
};

use nanocode_shell::{Call, Commands, FileSystem, Reply};

use crate::{Ended, Files, Kind, Limit, Limits, Python, Ran, Remove, Run, Stat, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub steps: u64,
    pub time: Duration,
    pub memory: Option<u64>,
    pub sleep: Duration,
    pub depth: usize,
}

pub struct Command {
    kept: Rc<Kept>,
    rest: Rc<dyn Commands>,
}

struct Kept {
    budget: Budget,
    python: OnceCell<Python>,
}

const NAMES: [&[u8]; 4] = [
    b"python3",
    b"python",
    b"/usr/bin/python3",
    b"/usr/bin/python",
];

impl Command {
    pub fn new(budget: Budget, rest: Rc<dyn Commands>) -> Self {
        let python = OnceCell::new();
        Self {
            kept: Rc::new(Kept { budget, python }),
            rest,
        }
    }
}

impl Commands for Command {
    fn has(&self, name: &[u8]) -> bool {
        NAMES.contains(&name) || self.rest.has(name)
    }

    fn run(&self, call: Call) -> Pin<Box<dyn Future<Output = i32>>> {
        if !call
            .argv
            .first()
            .is_some_and(|name| NAMES.contains(&&**name))
        {
            return self.rest.run(call);
        }
        let kept = self.kept.clone();
        Box::pin(async move {
            match kept.run(&call).await {
                Ok(status) => status,
                Err(error) => {
                    let _ = call
                        .stderr
                        .write_all(format!("python3: {error}\n").as_bytes())
                        .await;
                    1
                }
            }
        })
    }
}

impl Kept {
    async fn run(&self, call: &Call) -> io::Result<i32> {
        let argv: Result<Vec<String>, _> = call
            .argv
            .iter()
            .skip(1)
            .map(|argument| String::from_utf8(argument.clone()))
            .collect();
        let argv = argv.map_err(|_| io::Error::other("an argument is not UTF-8"))?;
        let mut stdin = Vec::new();
        if call.stdin.read_to_end(&mut stdin).await.is_err() {
            stdin.clear();
        }
        let python = match self.python.get() {
            Some(python) => python,
            None => {
                let python = Python::new()?;
                self.python.get_or_init(|| python)
            }
        };
        let shell = call.shell.limits();
        let run = Run {
            argv,
            stdin,
            cwd: call.cwd.clone(),
            env: call.env.clone(),
            limits: Limits {
                output: shell.output,
                file: shell.file,
                sleep: self.budget.sleep,
                steps: self.budget.steps,
                time: self.budget.time,
                memory: self.budget.memory,
                depth: self.budget.depth,
            },
            stop: call.shell.stop(),
        };
        let files = call.shell.files();
        let ran = match files.stat(&call.cwd, true) {
            Reply::Now(_) => python.run(run, Rc::new(Direct(files))),
            Reply::Later(asked) => {
                asked.await?;
                let copied = Rc::new(Copied::of(&files, &call.cwd, shell.file).await?);
                let ran = python.run(run, copied.clone());
                copied.apply(&files).await?;
                ran
            }
        };
        self.report(call, ran).await
    }

    async fn report(&self, call: &Call, ran: Ran) -> io::Result<i32> {
        call.stdout.write_all(&ran.stdout).await?;
        call.stderr.write_all(&ran.stderr).await?;
        let (status, said) = match ran.ended {
            Ended::Exited(status) => return Ok(status),
            Ended::Cancelled => return Ok(128 + 15),
            Ended::Needs(missing) => {
                call.shell.lacks(b"python3");
                let lines: String = missing
                    .iter()
                    .map(|missing| format!("{missing}\n"))
                    .collect();
                (127, lines)
            }
            Ended::Refused(limit) => {
                let said = match limit {
                    Limit::Output => "it printed more than this shell keeps",
                    Limit::Sleep => "it waited longer than this shell allows",
                    Limit::Steps => "it ran more steps than this shell allows",
                    Limit::Time => "it ran longer than this shell allows",
                    Limit::Memory => "it held more memory than this shell allows",
                };
                (126, format!("python3: {said}\n"))
            }
        };
        call.stderr.write_all(said.as_bytes()).await?;
        Ok(status)
    }
}

fn later() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "the host cannot answer this at once",
    )
}

fn now<T>(reply: Reply<T>) -> io::Result<T> {
    match reply {
        Reply::Now(result) => result,
        Reply::Later(_) => Err(later()),
    }
}

const fn stat(stat: nanocode_shell::Stat) -> Stat {
    Stat {
        kind: match stat.kind {
            nanocode_shell::Kind::File => Kind::File,
            nanocode_shell::Kind::Dir => Kind::Dir,
            nanocode_shell::Kind::Symlink => Kind::Symlink,
        },
        size: stat.size,
        mode: stat.mode,
        mtime_ns: stat.mtime_ns,
    }
}

const fn how(how: Write) -> nanocode_shell::Write {
    match how {
        Write::Replace => nanocode_shell::Write::Replace,
        Write::Append => nanocode_shell::Write::Append,
        Write::New => nanocode_shell::Write::New,
    }
}

const fn what(what: Remove) -> nanocode_shell::Remove {
    match what {
        Remove::File => nanocode_shell::Remove::File,
        Remove::Dir => nanocode_shell::Remove::Dir,
    }
}

struct Direct(Rc<dyn FileSystem>);

impl Files for Direct {
    fn stat(&self, path: &[u8]) -> io::Result<Stat> {
        now(self.0.stat(path, true)).map(stat)
    }

    fn read(&self, path: &[u8], limit: u64) -> io::Result<Vec<u8>> {
        now(self.0.read(path, limit)).map(|data| data.to_vec())
    }

    fn write(&self, path: &[u8], data: &[u8], write: Write) -> io::Result<()> {
        now(self.0.write(path, data, how(write)))
    }

    fn list(&self, path: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        now(self.0.list(path))
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> io::Result<()> {
        now(self.0.mkdir(path, mode))
    }

    fn remove(&self, path: &[u8], remove: Remove) -> io::Result<()> {
        now(self.0.remove(path, what(remove)))
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        now(self.0.rename(from, to))
    }
}

enum Node {
    File {
        data: Vec<u8>,
        mode: u32,
        mtime_ns: i64,
        changed: bool,
    },
    Dir {
        mode: u32,
    },
}

struct Copied {
    root: Vec<u8>,
    first: BTreeSet<Vec<u8>>,
    nodes: RefCell<BTreeMap<Vec<u8>, Node>>,
}

fn parent(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) | None => b"/",
        Some(cut) => &path[..cut],
    }
}

fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut path = dir.to_vec();
    if !path.ends_with(b"/") {
        path.push(b'/');
    }
    path.extend_from_slice(name);
    path
}

impl Copied {
    async fn of(files: &Rc<dyn FileSystem>, root: &[u8], limit: u64) -> io::Result<Self> {
        let mut nodes = BTreeMap::new();
        let mut dirs = vec![root.to_vec()];
        while let Some(dir) = dirs.pop() {
            let mode = files.stat(&dir, true).get().await?.mode;
            for name in files.list(&dir).get().await? {
                let path = join(&dir, &name);
                let found = files.stat(&path, true).get().await?;
                match found.kind {
                    nanocode_shell::Kind::Dir => dirs.push(path),
                    nanocode_shell::Kind::File => {
                        let data = files.read(&path, limit).get().await?.to_vec();
                        let node = Node::File {
                            data,
                            mode: found.mode,
                            mtime_ns: found.mtime_ns,
                            changed: false,
                        };
                        nodes.insert(path, node);
                    }
                    nanocode_shell::Kind::Symlink => {}
                }
            }
            nodes.insert(dir, Node::Dir { mode });
        }
        Ok(Self {
            root: root.to_vec(),
            first: nodes.keys().cloned().collect(),
            nodes: RefCell::new(nodes),
        })
    }

    fn within(&self, path: &[u8]) -> io::Result<()> {
        let inside = path
            .strip_prefix(&*self.root)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/") || self.root == b"/");
        if inside {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "outside the working directory, which is all this shell copied in for Python",
            ))
        }
    }

    async fn apply(&self, files: &Rc<dyn FileSystem>) -> io::Result<()> {
        let nodes = self.nodes.take();
        let gone = self
            .first
            .iter()
            .rev()
            .filter(|path| match nodes.get(*path) {
                None => true,
                Some(Node::Dir { .. }) => false,
                Some(Node::File { .. }) => !self.first.contains(parent(path)),
            });
        for path in gone {
            let found = files.stat(path, false).get().await?;
            let what = match found.kind {
                nanocode_shell::Kind::Dir => nanocode_shell::Remove::Dir,
                _ => nanocode_shell::Remove::File,
            };
            files.remove(path, what).get().await?;
        }
        for (path, node) in nodes.iter() {
            match node {
                Node::Dir { mode } if !self.first.contains(path) => {
                    files.mkdir(path, *mode).get().await?;
                }
                Node::File {
                    data,
                    changed: true,
                    ..
                } => {
                    let write = nanocode_shell::Write::Replace;
                    files.write(path, data, write).get().await?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn missing() -> io::Error {
    io::ErrorKind::NotFound.into()
}

impl Files for Copied {
    fn stat(&self, path: &[u8]) -> io::Result<Stat> {
        self.within(path)?;
        match self.nodes.borrow().get(path) {
            Some(Node::File {
                data,
                mode,
                mtime_ns,
                ..
            }) => Ok(Stat {
                kind: Kind::File,
                size: data.len() as u64,
                mode: *mode,
                mtime_ns: *mtime_ns,
            }),
            Some(Node::Dir { mode }) => Ok(Stat {
                kind: Kind::Dir,
                size: 0,
                mode: *mode,
                mtime_ns: 0,
            }),
            None => Err(missing()),
        }
    }

    fn read(&self, path: &[u8], _limit: u64) -> io::Result<Vec<u8>> {
        self.within(path)?;
        match self.nodes.borrow().get(path) {
            Some(Node::File { data, .. }) => Ok(data.clone()),
            Some(Node::Dir { .. }) => Err(io::ErrorKind::IsADirectory.into()),
            None => Err(missing()),
        }
    }

    fn write(&self, path: &[u8], more: &[u8], write: Write) -> io::Result<()> {
        self.within(path)?;
        let mut nodes = self.nodes.borrow_mut();
        if !matches!(nodes.get(parent(path)), Some(Node::Dir { .. })) {
            return Err(missing());
        }
        match (nodes.get_mut(path), write) {
            (Some(Node::Dir { .. }), _) => Err(io::ErrorKind::IsADirectory.into()),
            (Some(Node::File { .. }), Write::New) => Err(io::ErrorKind::AlreadyExists.into()),
            (Some(Node::File { data, changed, .. }), Write::Append) => {
                data.extend_from_slice(more);
                *changed = true;
                Ok(())
            }
            (Some(Node::File { data, changed, .. }), Write::Replace) => {
                *data = more.to_vec();
                *changed = true;
                Ok(())
            }
            (None, _) => {
                let node = Node::File {
                    data: more.to_vec(),
                    mode: 0o644,
                    mtime_ns: 0,
                    changed: true,
                };
                nodes.insert(path.to_vec(), node);
                Ok(())
            }
        }
    }

    fn list(&self, path: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        self.within(path)?;
        let nodes = self.nodes.borrow();
        match nodes.get(path) {
            Some(Node::Dir { .. }) => Ok(nodes
                .keys()
                .filter(|other| other.len() > path.len() && parent(other) == path)
                .map(|other| other[other.len() - (other.len() - path.len() - 1)..].to_vec())
                .collect()),
            Some(Node::File { .. }) => Err(io::ErrorKind::NotADirectory.into()),
            None => Err(missing()),
        }
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> io::Result<()> {
        self.within(path)?;
        let mut nodes = self.nodes.borrow_mut();
        if nodes.contains_key(path) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        if !matches!(nodes.get(parent(path)), Some(Node::Dir { .. })) {
            return Err(missing());
        }
        nodes.insert(path.to_vec(), Node::Dir { mode });
        Ok(())
    }

    fn remove(&self, path: &[u8], remove: Remove) -> io::Result<()> {
        self.within(path)?;
        let mut nodes = self.nodes.borrow_mut();
        match (nodes.get(path), remove) {
            (None, _) => return Err(missing()),
            (Some(Node::Dir { .. }), Remove::File) => {
                return Err(io::ErrorKind::IsADirectory.into());
            }
            (Some(Node::File { .. }), Remove::Dir) => {
                return Err(io::ErrorKind::NotADirectory.into());
            }
            (Some(Node::Dir { .. }), Remove::Dir) => {
                if nodes
                    .keys()
                    .any(|other| other.len() > path.len() && parent(other) == path)
                {
                    return Err(io::ErrorKind::DirectoryNotEmpty.into());
                }
            }
            (Some(Node::File { .. }), Remove::File) => {}
        }
        nodes.remove(path);
        Ok(())
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        self.within(from)?;
        self.within(to)?;
        let mut nodes = self.nodes.borrow_mut();
        if !nodes.contains_key(from) {
            return Err(missing());
        }
        if !matches!(nodes.get(parent(to)), Some(Node::Dir { .. })) {
            return Err(missing());
        }
        let moved: Vec<Vec<u8>> = nodes
            .keys()
            .filter(|path| {
                path.strip_prefix(from)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
            })
            .cloned()
            .collect();
        for path in moved {
            let Some(mut node) = nodes.remove(&path) else {
                continue;
            };
            if let Node::File { changed, .. } = &mut node {
                *changed = true;
            }
            let mut placed = to.to_vec();
            placed.extend_from_slice(&path[from.len()..]);
            nodes.insert(placed, node);
        }
        Ok(())
    }
}
