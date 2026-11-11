//! What git changes: objects, the index, references and their logs, configuration and
//! the files of the worktree.
//!
//! A command's body only reads. What it would change it leaves as a Plan, and the
//! utility's front end carries the plan out through the shell's files once the body
//! has ended: a host that answers later is awaited there, the body may be run again
//! for such a host without having changed anything, and every file changed is one the
//! shell's change report names.

use std::io;

use psh_engine::host::OpenOptions;
use psh_engine::util::UtilCtx;

use super::index::Index;
use super::store::{Oid, join};
use super::{Error, Git, Res};

pub enum Mode {
    Plain,
    Exec,
    /// A loose object: read-only, and left alone when it is already there.
    Object,
}

pub enum Op {
    /// A file with these bytes, in place of what was there, its directories made.
    Write {
        path: Vec<u8>,
        data: Vec<u8>,
        mode: Mode,
    },
    Append {
        path: Vec<u8>,
        data: Vec<u8>,
    },
    Link {
        path: Vec<u8>,
        target: Vec<u8>,
    },
    Mkdir(Vec<u8>),
    /// A file or a symlink; one that is not there is no error.
    Remove(Vec<u8>),
    /// A directory, when it is empty.
    Rmdir(Vec<u8>),
    Rename {
        from: Vec<u8>,
        to: Vec<u8>,
    },
}

pub struct IndexOut {
    pub path: Vec<u8>,
    /// The worktree its fresh entries are files of.
    pub root: Vec<u8>,
    pub index: Index,
}

#[derive(Default)]
pub struct Plan {
    pub ops: Vec<Op>,
    pub index: Option<IndexOut>,
}

fn parent(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&b| b == b'/') {
        Some(0) | None => b"/",
        Some(cut) => &path[..cut],
    }
}

async fn make_dirs(ctx: &UtilCtx<'_>, dir: &[u8]) -> io::Result<()> {
    let mode = 0o777 & !ctx.umask();
    let mut missing = Vec::new();
    let mut at = dir;
    while at != b"/" {
        match ctx.stat(at, true).await {
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::NotFound => missing.push(at),
            Err(e) => return Err(e),
        }
        at = parent(at);
    }
    for dir in missing.into_iter().rev() {
        match ctx.mkdir(dir, mode).await {
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

async fn put(ctx: &UtilCtx<'_>, path: &[u8], data: &[u8], opts: OpenOptions) -> io::Result<()> {
    let file = match ctx.open(path, opts).await {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            make_dirs(ctx, parent(path)).await?;
            ctx.open(path, opts).await?
        }
        other => other?,
    };
    file.write_all(data).await
}

async fn remove(ctx: &UtilCtx<'_>, path: &[u8]) -> io::Result<()> {
    match ctx.remove_file(path).await {
        Err(e)
            if !matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Err(e)
        }
        _ => Ok(()),
    }
}

/// An error with the path it is about, as git names it.
fn at(path: &[u8], what: &str, e: io::Error) -> io::Error {
    io::Error::new(
        e.kind(),
        format!(
            "unable to {what} '{}': {}",
            String::from_utf8_lossy(path),
            psh_engine::host::error_text(&e)
        ),
    )
}

/// Carries a plan out. The first failure ends it: what was done before stays done, as
/// with git when a disk fills.
pub async fn apply(ctx: &UtilCtx<'_>, plan: Plan) -> io::Result<()> {
    for op in plan.ops {
        let (path, what) = match &op {
            Op::Write { path, .. } | Op::Append { path, .. } => (path.clone(), "write"),
            Op::Link { path, .. } => (path.clone(), "link"),
            Op::Mkdir(path) => (path.clone(), "create"),
            Op::Remove(path) | Op::Rmdir(path) => (path.clone(), "unlink"),
            Op::Rename { to, .. } => (to.clone(), "rename to"),
        };
        carry(ctx, op).await.map_err(|e| at(&path, what, e))?;
    }
    if let Some(mut out) = plan.index {
        for entry in &mut out.index.entries {
            if !entry.fresh {
                continue;
            }
            let path = join(&out.root, &entry.path);
            let (meta, extra) = ctx
                .stat_extra(&path, false)
                .await
                .map_err(|e| at(&path, "stat", e))?;
            entry.record(&meta, Some(extra.ctime_ns));
        }
        let data = out.index.serialize();
        let mut lock = out.path.clone();
        lock.extend_from_slice(b".lock");
        put(ctx, &lock, &data, OpenOptions::write())
            .await
            .map_err(|e| at(&lock, "write", e))?;
        ctx.rename(&lock, &out.path)
            .await
            .map_err(|e| at(&out.path, "rename to", e))?;
    }
    Ok(())
}

async fn carry(ctx: &UtilCtx<'_>, op: Op) -> io::Result<()> {
    let umask = ctx.umask();
    {
        match op {
            Op::Write { path, data, mode } => match mode {
                Mode::Object => {
                    let opts = OpenOptions {
                        write: true,
                        create: true,
                        create_new: true,
                        mode: Some(0o444),
                        ..OpenOptions::default()
                    };
                    match put(ctx, &path, &data, opts).await {
                        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
                        _ => {}
                    }
                }
                Mode::Plain | Mode::Exec => {
                    let bits = if matches!(mode, Mode::Exec) {
                        0o777
                    } else {
                        0o666
                    };
                    let opts = OpenOptions {
                        mode: Some(bits & !umask),
                        ..OpenOptions::write()
                    };
                    put(ctx, &path, &data, opts).await?;
                }
            },
            Op::Append { path, data } => put(ctx, &path, &data, OpenOptions::append()).await?,
            Op::Link { path, target } => match ctx.symlink(&target, &path).await {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    make_dirs(ctx, parent(&path)).await?;
                    ctx.symlink(&target, &path).await?;
                }
                other => other?,
            },
            Op::Mkdir(path) => make_dirs(ctx, &path).await?,
            Op::Remove(path) => remove(ctx, &path).await?,
            Op::Rmdir(path) => {
                let _ = ctx.remove_dir(&path).await;
            }
            Op::Rename { from, to } => match ctx.rename(&from, &to).await {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    make_dirs(ctx, parent(&to)).await?;
                    ctx.rename(&from, &to).await?;
                }
                other => other?,
            },
        }
    }
    Ok(())
}

/// Whose name a commit, a tag or a reflog line carries.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Who {
    Author,
    Committer,
}

fn zone_text(offset: i64) -> String {
    let sign = if offset < 0 { '-' } else { '+' };
    let minutes = offset.abs() / 60;
    format!("{sign}{:02}{:02}", minutes / 60, minutes % 60)
}

/// A date as GIT_AUTHOR_DATE, GIT_COMMITTER_DATE and --date give it: seconds and a
/// zone as git stores them, or a date git's approxidate reads.
pub fn parse_date(git: &Git<'_>, text: &str) -> Res<String> {
    let text = text.trim();
    let mut words = text.split_whitespace();
    let (first, second, third) = (words.next(), words.next(), words.next());
    let zone = |z: &str| {
        z.len() == 5 && z.starts_with(['+', '-']) && z[1..].bytes().all(|b| b.is_ascii_digit())
    };
    if let (Some(secs), None) = (first, third) {
        let secs = secs.strip_prefix('@').unwrap_or(secs);
        if !secs.is_empty() && secs.bytes().all(|b| b.is_ascii_digit()) {
            match second {
                Some(z) if zone(z) => return Ok(format!("{secs} {z}")),
                None if first.is_some_and(|f| f.starts_with('@')) || secs.len() > 8 => {
                    return Ok(format!("{secs} +0000"));
                }
                _ => {}
            }
        }
    }
    // The zone the date ends in, when it names one: +hhmm, +hh:mm or Z.
    let tail = |n: usize| text.len().checked_sub(n).and_then(|at| text.get(at..));
    let stated = match (tail(5), tail(6)) {
        (Some(z), _) if zone(z) => Some(z.to_string()),
        (_, Some(z)) if z.as_bytes()[3] == b':' && zone(&z.replace(':', "")) => {
            Some(z.replace(':', ""))
        }
        _ if text.ends_with('Z') && text.contains('T') => Some("+0000".to_string()),
        _ => None,
    };
    match super::log::approxidate(text, git.now, git.offset) {
        Some(secs) => Ok(format!(
            "{secs} {}",
            stated.unwrap_or_else(|| zone_text(git.offset))
        )),
        None => Err(Error::Fatal(format!("invalid date format: {text}"))),
    }
}

impl Git<'_> {
    pub fn var(&self, name: &str) -> Option<String> {
        self.env
            .iter()
            .find(|(k, _)| k == name.as_bytes())
            .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
            .filter(|v| !v.is_empty())
    }

    pub fn op(&self, op: Op) {
        self.plan.borrow_mut().ops.push(op);
    }

    pub fn write(&self, path: Vec<u8>, data: Vec<u8>) {
        self.op(Op::Write {
            path,
            data,
            mode: Mode::Plain,
        });
    }

    /// The index this command leaves.
    pub fn set_index(&self, index: Index) {
        self.plan.borrow_mut().index = Some(IndexOut {
            path: join(&self.repo.git_dir, b"index"),
            root: self.repo.work_tree.clone().unwrap_or_default(),
            index,
        });
    }

    /// Another git holds the index, or the last one left its lock behind.
    pub fn lock_index(&self) -> Res<()> {
        let lock = join(&self.repo.git_dir, b"index.lock");
        if self.repo.fs.stat_opt(&lock, false)?.is_some() {
            return Err(Error::Fatal(format!(
                "Unable to create '{}': File exists.\n\nAnother git process seems to be running in this repository, e.g.\nan editor opened by 'git commit'. Please make sure all processes\nare terminated then try again. If it still fails, a git process\nmay have crashed in this repository earlier:\nremove the file manually to continue.",
                String::from_utf8_lossy(&lock)
            )));
        }
        Ok(())
    }

    /// The name and address of the author or committer, without the date. strict: a
    /// commit or a tag is made, and git refuses to guess who makes it.
    pub fn person(&self, who: Who, strict: bool) -> Res<String> {
        let (name_var, email_var, role) = match who {
            Who::Author => ("GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "Author"),
            Who::Committer => ("GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL", "Committer"),
        };
        let own = |role: &str, key: &str| {
            self.repo
                .config
                .get(&format!("{role}.{key}"))
                .or_else(|| self.repo.config.get(&format!("user.{key}")))
                .map(str::to_string)
                .filter(|v| !v.is_empty())
        };
        let lower = role.to_ascii_lowercase();
        let name = self.var(name_var).or_else(|| own(&lower, "name"));
        let email = self
            .var(email_var)
            .or_else(|| own(&lower, "email"))
            .or_else(|| self.var("EMAIL"));
        let unknown = |what: &str| {
            Error::Fatal(format!(
                "no {what} was given and auto-detection is disabled\n\n*** Please tell me who you are.\n\nRun\n\n  git config --global user.email \"you@example.com\"\n  git config --global user.name \"Your Name\"\n\nto set your account's default identity.\nOmit --global to set the identity only in this repository.\n"
            ))
        };
        let user = || {
            self.var("USER")
                .or_else(|| self.var("LOGNAME"))
                .unwrap_or_else(|| "unknown".to_string())
        };
        let name = match name {
            Some(name) => name,
            None if strict => return Err(unknown("name")),
            None => user(),
        };
        let email = match email {
            Some(email) => email,
            None if strict => return Err(unknown("email")),
            None => format!("{}@localhost", user()),
        };
        let clean = |s: &str| {
            s.trim_matches(|c: char| c.is_ascii_whitespace() || ".,:;<>\"\\'".contains(c))
                .chars()
                .filter(|c| !matches!(c, '<' | '>' | '\n'))
                .collect::<String>()
        };
        Ok(format!("{} <{}>", clean(&name), clean(&email)))
    }

    /// An author or committer line: who and when.
    pub fn ident(&self, who: Who, strict: bool) -> Res<String> {
        let date = match who {
            Who::Author => self.var("GIT_AUTHOR_DATE"),
            Who::Committer => self.var("GIT_COMMITTER_DATE"),
        };
        let date = match date {
            Some(date) => parse_date(self, &date)?,
            None => format!("{} {}", self.now, zone_text(self.offset)),
        };
        Ok(format!("{} {date}", self.person(who, strict)?))
    }

    fn ref_file(&self, name: &str, logs: bool) -> Vec<u8> {
        let shared = (name == "refs" || name.starts_with("refs/"))
            && !name.starts_with("refs/bisect/")
            && !name.starts_with("refs/worktree/")
            && !name.starts_with("refs/rewritten/");
        let dir = if shared {
            &self.repo.common_dir
        } else {
            &self.repo.git_dir
        };
        let dir = if logs {
            join(dir, b"logs")
        } else {
            dir.clone()
        };
        join(&dir, name.as_bytes())
    }

    /// A line for a reference's log, where git keeps one: for HEAD, branches, remote
    /// branches and notes in a repository with a worktree (core.logAllRefUpdates), and
    /// for any reference that has a log already.
    pub fn log_ref(
        &self,
        name: &str,
        old: Option<Oid>,
        new: Option<Oid>,
        message: &str,
    ) -> Res<()> {
        let path = self.ref_file(name, true);
        let all = self
            .repo
            .config
            .get("core.logallrefupdates")
            .map(str::to_ascii_lowercase);
        let wanted = match all.as_deref() {
            Some("always") => true,
            Some("false" | "no" | "off" | "0") => false,
            Some(_) => true,
            None => self.repo.work_tree.is_some(),
        };
        let kept = name == "HEAD"
            || name.starts_with("refs/heads/")
            || name.starts_with("refs/remotes/")
            || name.starts_with("refs/notes/")
            || all.as_deref() == Some("always");
        if !(wanted && kept) && self.repo.fs.stat_opt(&path, true)?.is_none() {
            return Ok(());
        }
        let line = format!(
            "{} {} {}\t{}\n",
            old.unwrap_or(Oid::ZERO).hex(),
            new.unwrap_or(Oid::ZERO).hex(),
            self.ident(Who::Committer, false)?,
            message.replace('\n', " ")
        );
        self.op(Op::Append {
            path,
            data: line.into_bytes(),
        });
        Ok(())
    }

    /// Points a reference at an object and logs it.
    pub fn set_ref(&self, name: &str, new: Oid, message: &str) -> Res<()> {
        let old = self.repo.resolve_ref(name)?;
        self.write(
            self.ref_file(name, false),
            format!("{}\n", new.hex()).into_bytes(),
        );
        self.log_ref(name, old, Some(new), message)
    }

    /// Moves what HEAD names: the branch it is on and both logs, or HEAD itself when
    /// it is detached.
    pub fn move_head(&self, new: Oid, message: &str) -> Res<()> {
        let old = self.repo.head()?;
        match self.repo.head_ref()? {
            // A branch that stays where it is has nothing to log; HEAD's log still
            // says what was done.
            Some(_) if old == Some(new) => {}
            Some(branch) => {
                self.write(
                    self.ref_file(&branch, false),
                    format!("{}\n", new.hex()).into_bytes(),
                );
                self.log_ref(&branch, old, Some(new), message)?;
            }
            None => self.write(
                self.ref_file("HEAD", false),
                format!("{}\n", new.hex()).into_bytes(),
            ),
        }
        self.log_ref("HEAD", old, Some(new), message)
    }

    /// Puts HEAD on a branch, or on a commit.
    pub fn point_head(&self, branch: Option<&str>, new: Option<Oid>, message: &str) -> Res<()> {
        let old = self.repo.head()?;
        let text = match (branch, new) {
            (Some(branch), _) => format!("ref: {branch}\n"),
            (None, Some(oid)) => format!("{}\n", oid.hex()),
            (None, None) => return Ok(()),
        };
        self.write(self.ref_file("HEAD", false), text.into_bytes());
        if old.is_none() && new.is_none() {
            return Ok(());
        }
        self.log_ref("HEAD", old, new, message)
    }

    /// Removes a reference: its file, its line in packed-refs, its log.
    pub fn delete_ref(&self, name: &str) -> Res<()> {
        self.remove_loose(name);
        self.delete_packed(&[name])
    }

    /// Removes a reference's own file and its log, and the directories they empty.
    pub fn remove_loose(&self, name: &str) {
        self.op(Op::Remove(self.ref_file(name, false)));
        self.op(Op::Remove(self.ref_file(name, true)));
        let mut dir = name;
        while let Some((up, _)) = dir.rsplit_once('/') {
            if up == "refs" || up == "refs/heads" || up == "refs/tags" {
                break;
            }
            self.op(Op::Rmdir(self.ref_file(up, false)));
            self.op(Op::Rmdir(self.ref_file(up, true)));
            dir = up;
        }
    }

    /// Takes references out of packed-refs.
    pub fn delete_packed(&self, names: &[&str]) -> Res<()> {
        let packed = join(&self.repo.common_dir, b"packed-refs");
        if let Some(text) = self.repo.fs.read_opt(&packed)? {
            let mut kept = Vec::with_capacity(text.len());
            let mut dropped = false;
            let mut found = false;
            for line in text.split_inclusive(|&b| b == b'\n') {
                let body = line.strip_suffix(b"\n").unwrap_or(line);
                if body.first() == Some(&b'^') {
                    if !dropped {
                        kept.extend_from_slice(line);
                    }
                    continue;
                }
                dropped = body.len() > 41 && names.iter().any(|n| &body[41..] == n.as_bytes());
                found |= dropped;
                if !dropped {
                    kept.extend_from_slice(line);
                }
            }
            if found {
                self.write(packed, kept);
            }
        }
        Ok(())
    }

    /// A hook git would run here is one this shell cannot run: the command says so
    /// instead of going on as if there were none.
    pub fn hooks(&self, names: &[&str]) -> Res<()> {
        let dir = match self.repo.config.get("core.hookspath") {
            Some(path) if path.starts_with('/') => path.as_bytes().to_vec(),
            Some(path) => join(
                self.repo.work_tree.as_deref().unwrap_or(&self.repo.git_dir),
                path.as_bytes(),
            ),
            None => join(&self.repo.common_dir, b"hooks"),
        };
        for name in names {
            if let Some(meta) = self.repo.fs.stat_opt(&join(&dir, name.as_bytes()), true)?
                && meta.kind == psh_engine::host::FileKind::File
                && meta.mode & 0o111 != 0
            {
                return Err(Error::Lacks(
                    format!("this repository has a {name} hook"),
                    "way to run git hooks",
                ));
            }
        }
        Ok(())
    }
}

/// git's check-ref-format for one reference name.
pub fn valid_ref(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.starts_with('-')
        && name != "@"
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("@{")
        && !name
            .bytes()
            .any(|b| b < 0x20 || b == 0x7f || b" ~^:?*[\\".contains(&b))
        && name
            .split('/')
            .all(|part| !part.starts_with('.') && !part.ends_with(".lock"))
}

fn quote_value(value: &str) -> String {
    let quote = value.starts_with(' ') || value.ends_with(' ') || value.contains(['#', ';']);
    let mut out = String::new();
    if quote {
        out.push('"');
    }
    for c in value.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    if quote {
        out.push('"');
    }
    out
}

/// A section header's name as the key names it: section, or section.subsection.
fn header_key(line: &str) -> Option<String> {
    let inner = line.trim().strip_prefix('[')?;
    let inner = &inner[..inner.find(']')?];
    Some(match inner.split_once(char::is_whitespace) {
        Some((name, sub)) => format!(
            "{}.{}",
            name.to_ascii_lowercase(),
            sub.trim()
                .trim_matches('"')
                .replace("\\\"", "\"")
                .replace("\\\\", "\\")
        ),
        None => match inner.split_once('.') {
            Some((name, sub)) => {
                format!("{}.{}", name.to_ascii_lowercase(), sub.to_ascii_lowercase())
            }
            None => inner.to_ascii_lowercase(),
        },
    })
}

/// What an edit of a configuration file does to a key.
pub enum Edit<'a> {
    /// The key has this one value.
    Set(&'a str),
    /// One more value.
    Add(&'a str),
    Unset,
    /// The whole section goes.
    RemoveSection,
    /// The section gets another name.
    RenameSection(&'a str),
}

fn header(section: &str) -> String {
    match section.split_once('.') {
        Some((name, sub)) => format!(
            "[{name} \"{}\"]\n",
            sub.replace('\\', "\\\\").replace('"', "\\\"")
        ),
        None => format!("[{section}]\n"),
    }
}

/// A configuration file's text after an edit, as git config writes it; None when
/// there was nothing to unset, remove or rename. key is section.name or
/// section.subsection.name, as the reader names it; for the section edits it is the
/// section alone.
pub fn edit_config(text: &str, key: &str, edit: Edit<'_>) -> Option<String> {
    let section_edit = matches!(edit, Edit::RemoveSection | Edit::RenameSection(_));
    let (section, name) = if section_edit {
        (key, "")
    } else {
        key.rsplit_once('.')?
    };
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    if let Some(last) = lines.last_mut()
        && !last.ends_with('\n')
    {
        last.push('\n');
    }
    let mut current: Option<String> = None;
    let mut hits = Vec::new();
    let mut last_of_section = None;
    let mut headers = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            current = header_key(trimmed);
            if current.as_deref() == Some(section) {
                headers.push(i);
                last_of_section = Some(i);
            }
            continue;
        }
        if current.as_deref() != Some(section) {
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with(['#', ';']) {
            continue;
        }
        last_of_section = Some(i);
        let found = trimmed
            .split(['=', ' ', '\t'])
            .next()
            .is_some_and(|n| n.eq_ignore_ascii_case(name));
        if found {
            hits.push(i);
        }
    }
    let shown = |value: &str| format!("\t{name} = {}\n", quote_value(value));
    match edit {
        Edit::RemoveSection | Edit::RenameSection(_) => {
            if headers.is_empty() {
                return None;
            }
            let mut out = Vec::new();
            let mut inside = false;
            for (i, line) in lines.into_iter().enumerate() {
                if line.trim().starts_with('[') {
                    inside = headers.contains(&i);
                    if inside {
                        if let Edit::RenameSection(to) = edit {
                            out.push(header(to));
                        }
                        continue;
                    }
                }
                if !inside || matches!(edit, Edit::RenameSection(_)) {
                    out.push(line);
                }
            }
            Some(out.concat())
        }
        Edit::Unset => {
            let at = *hits.last()?;
            lines.remove(at);
            // A section left with nothing but its header goes with its last key.
            if let Some(&head) = headers.iter().rev().find(|&&h| h < at) {
                let rest_empty = lines[head + 1..]
                    .iter()
                    .take_while(|l| !l.trim().starts_with('['))
                    .all(|l| l.trim().is_empty());
                if rest_empty {
                    let end = lines[head + 1..]
                        .iter()
                        .position(|l| l.trim().starts_with('['))
                        .map_or(lines.len(), |n| head + 1 + n);
                    lines.drain(head..end);
                }
            }
            Some(lines.concat())
        }
        Edit::Set(value) if !hits.is_empty() => {
            let at = hits[hits.len() - 1];
            lines[at] = shown(value);
            Some(lines.concat())
        }
        Edit::Set(value) | Edit::Add(value) => {
            match last_of_section {
                Some(at) => lines.insert(at + 1, shown(value)),
                None => {
                    lines.push(header(section));
                    lines.push(shown(value));
                }
            }
            Some(lines.concat())
        }
    }
}
