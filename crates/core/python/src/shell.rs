use std::{cell::OnceCell, future::Future, io, pin::Pin, rc::Rc, time::Duration};

use nanocode_shell::{Call, Commands, FileSystem, Reply, join};

use crate::{
    Ended, Entry, Files, Kind, Limit, Limits, Python, Ran, Remove, Run, Stat, Tree, Write,
};

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
                let copied = Rc::new(copy(&files, &call.cwd, shell.file).await?);
                let ran = python.run(run, copied.clone());
                apply(&copied, &files).await?;
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

async fn copy(files: &Rc<dyn FileSystem>, root: &[u8], limit: u64) -> io::Result<Tree> {
    let tree = Tree::new(vec![root.to_vec()]);
    let mut dirs = vec![root.to_vec()];
    while let Some(dir) = dirs.pop() {
        let mode = files.stat(&dir, true).get().await?.mode;
        for name in files.list(&dir).get().await? {
            let path = join(&dir, &name);
            let found = files.stat(&path, true).get().await?;
            match found.kind {
                nanocode_shell::Kind::Dir => dirs.push(path),
                nanocode_shell::Kind::File => {
                    let entry = Entry::File {
                        data: files.read(&path, limit).get().await?.to_vec(),
                        mode: found.mode,
                        mtime_ns: found.mtime_ns,
                    };
                    tree.put(path, entry);
                }
                nanocode_shell::Kind::Symlink => {}
            }
        }
        tree.put(dir, Entry::Dir { mode });
    }
    Ok(tree)
}

async fn apply(tree: &Tree, files: &Rc<dyn FileSystem>) -> io::Result<()> {
    let delta = tree.delta();
    for path in &delta.removed {
        let what = match files.stat(path, false).get().await?.kind {
            nanocode_shell::Kind::Dir => nanocode_shell::Remove::Dir,
            _ => nanocode_shell::Remove::File,
        };
        files.remove(path, what).get().await?;
    }
    for (path, mode) in &delta.made {
        files.mkdir(path, *mode).get().await?;
    }
    for path in &delta.written {
        let data = tree.take(path).unwrap_or_default();
        let write = nanocode_shell::Write::Replace;
        files.write(path, &data, write).get().await?;
    }
    Ok(())
}
