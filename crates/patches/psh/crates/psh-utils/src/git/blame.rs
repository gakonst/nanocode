//! git blame: which commit each line of a file is from. A line is passed from a commit
//! to the parent whose version of the file still has it, by the same comparison git
//! diff makes, following the file across renames.

use std::collections::{BinaryHeap, HashMap};
use std::io::Write;
use std::rc::Rc;

use super::diff::{Change, File, detect_renames, trees};
use super::patch::TextDiff;
use super::pretty::{DateMode, date};
use super::repo::{Ident, Repo};
use super::rev;
use super::store::{Data, Oid, join};
use super::wild::Pathspec;
use super::worktree::blob_oid;
use super::{Error, Git, Io, Res, option};

/// A version of the file: in a commit (none: the worktree) under a path.
struct Origin {
    commit: Option<Oid>,
    path: Vec<u8>,
    blob: Oid,
    data: Data,
}

/// Lines of the result (from lno) that are lines of a version (from s_lno).
#[derive(Clone)]
struct Entry {
    lno: usize,
    count: usize,
    s_lno: usize,
    origin: usize,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Queued(i64, std::cmp::Reverse<u64>, usize);

struct Blame<'r, 'a> {
    repo: &'r Repo<'a>,
    origins: Vec<Origin>,
    by_name: HashMap<(Option<Oid>, Vec<u8>), usize>,
    entries: Vec<Entry>,
}

impl Blame<'_, '_> {
    fn origin(&mut self, commit: Oid, path: &[u8], blob: Oid) -> Res<usize> {
        if let Some(at) = self.by_name.get(&(Some(commit), path.to_vec())) {
            return Ok(*at);
        }
        let data = self.repo.blob(&blob)?;
        self.origins.push(Origin {
            commit: Some(commit),
            path: path.to_vec(),
            blob,
            data,
        });
        let at = self.origins.len() - 1;
        self.by_name.insert((Some(commit), path.to_vec()), at);
        Ok(at)
    }

    /// The version of the file a parent has: under the same name, or under the name
    /// the commit renamed it from.
    fn in_parent(&mut self, parent: Oid, child_tree: &Oid, path: &[u8]) -> Res<Option<usize>> {
        let tree = self.repo.commit(&parent)?.tree;
        if let Some(entry) = self.repo.tree_entry(&tree, path)?
            && !entry.is_tree()
        {
            return Ok(Some(self.origin(parent, path, entry.oid)?));
        }
        let mut changes = Vec::new();
        trees(
            self.repo,
            Some(&tree),
            Some(child_tree),
            b"",
            &Pathspec::default(),
            &mut changes,
        )?;
        let changes: Vec<Change> = changes
            .into_iter()
            .filter(|c| c.new.is_none() || (c.old.is_none() && c.path == path))
            .collect();
        for change in detect_renames(self.repo, changes, 30000)? {
            if change.path == path
                && let (Some(from), Some(File { oid, .. })) = (&change.from, &change.old)
            {
                return Ok(Some(self.origin(parent, from, *oid)?));
            }
        }
        Ok(None)
    }

    /// Passes the lines of a version that a parent's version also has to the parent.
    fn pass(&mut self, from: usize, to: usize) {
        let diff = TextDiff::new(&self.origins[to].data, &self.origins[from].data);
        // The stretches of the child's lines that the parent has, and by how much
        // their numbers differ there.
        let mut same: Vec<(usize, usize, isize)> = Vec::new();
        let mut at = 0;
        let mut offset = 0isize;
        for (old, new) in &diff.changes {
            if new.start > at {
                same.push((at, new.start, offset));
            }
            at = new.end;
            offset = old.end as isize - new.end as isize;
        }
        if diff.new.len() > at {
            same.push((at, diff.new.len(), offset));
        }
        let mut next = Vec::with_capacity(self.entries.len());
        for entry in std::mem::take(&mut self.entries) {
            if entry.origin != from {
                next.push(entry);
                continue;
            }
            let (start, end) = (entry.s_lno, entry.s_lno + entry.count);
            let mut pos = start;
            for &(a, b, offset) in &same {
                let (lo, hi) = (a.max(start), b.min(end));
                if lo >= hi {
                    continue;
                }
                if lo > pos {
                    next.push(Entry {
                        lno: entry.lno + (pos - start),
                        count: lo - pos,
                        s_lno: pos,
                        origin: from,
                    });
                }
                next.push(Entry {
                    lno: entry.lno + (lo - start),
                    count: hi - lo,
                    s_lno: (lo as isize + offset) as usize,
                    origin: to,
                });
                pos = hi;
            }
            if pos < end {
                next.push(Entry {
                    lno: entry.lno + (pos - start),
                    count: end - pos,
                    s_lno: pos,
                    origin: from,
                });
            }
        }
        self.entries = next;
    }

    fn run(&mut self, head: Option<Oid>) -> Res<()> {
        let mut queue: BinaryHeap<Queued> = BinaryHeap::new();
        let mut counter = 0u64;
        let mut queued = vec![true];
        queue.push(Queued(i64::MAX, std::cmp::Reverse(0), 0));
        while let Some(Queued(_, _, at)) = queue.pop() {
            queued[at] = false;
            if !self.entries.iter().any(|e| e.origin == at) {
                continue;
            }
            let (parents, tree) = match self.origins[at].commit {
                Some(commit) => {
                    let commit = self.repo.commit(&commit)?;
                    (commit.parents.clone(), commit.tree)
                }
                None => {
                    let head = head.expect("a worktree version has HEAD under it");
                    (vec![head], self.repo.commit(&head)?.tree)
                }
            };
            let path = self.origins[at].path.clone();
            let mut found = Vec::new();
            for parent in parents {
                let origin = match self.origins[at].commit {
                    Some(_) => self.in_parent(parent, &tree, &path)?,
                    None => match self.repo.tree_entry(&tree, &path)? {
                        Some(entry) => Some(self.origin(parent, &path, entry.oid)?),
                        None => None,
                    },
                };
                if let Some(origin) = origin
                    && !found.contains(&origin)
                {
                    found.push(origin);
                }
            }
            let mut touched = Vec::new();
            if let Some(&same) = found
                .iter()
                .find(|&&p| self.origins[p].blob == self.origins[at].blob)
            {
                for entry in &mut self.entries {
                    if entry.origin == at {
                        entry.origin = same;
                    }
                }
                touched.push(same);
            } else {
                for parent in found {
                    if !self.entries.iter().any(|e| e.origin == at) {
                        break;
                    }
                    self.pass(at, parent);
                    touched.push(parent);
                }
            }
            for parent in touched {
                if queued.len() < self.origins.len() {
                    queued.resize(self.origins.len(), false);
                }
                if !queued[parent] && self.entries.iter().any(|e| e.origin == parent) {
                    queued[parent] = true;
                    counter += 1;
                    let time = match self.origins[parent].commit {
                        Some(commit) => self.repo.commit(&commit)?.time(),
                        None => i64::MAX,
                    };
                    queue.push(Queued(time, std::cmp::Reverse(counter), parent));
                }
            }
        }
        Ok(())
    }
}

fn line_range(spec: &str, lines: usize, path: &str) -> Res<(usize, usize)> {
    let bad = || option("blame", &format!("-L {spec}"));
    let (from, to) = spec.split_once(',').unwrap_or((spec, ""));
    let start = if from.is_empty() {
        1
    } else {
        from.parse::<usize>().map_err(|_| bad())?
    };
    let end = if to.is_empty() {
        lines
    } else if let Some(count) = to.strip_prefix('+') {
        start + count.parse::<usize>().map_err(|_| bad())?.saturating_sub(1)
    } else {
        to.parse::<usize>().map_err(|_| bad())?
    };
    if start == 0 || start > lines.max(1) || lines == 0 {
        return Err(Error::Fatal(format!(
            "file {path} has only {lines} line{}",
            if lines == 1 { "" } else { "s" }
        )));
    }
    Ok((start - 1, end.min(lines).max(start)))
}

pub fn blame(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let repo = &git.repo;
    let mut ranges: Vec<String> = Vec::new();
    let mut suppress = false;
    let mut email = false;
    let mut long = false;
    let mut root = repo.config.bool("blame.showroot") == Some(true);
    let mut names: Vec<String> = Vec::new();
    let mut file: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--" => {
                file = args.get(i + 1).cloned();
                if args.len() > i + 2 {
                    return Err(option("blame", "more than one file"));
                }
                break;
            }
            "-s" => suppress = true,
            "-e" | "--show-email" => email = true,
            "-l" => long = true,
            "--root" => root = true,
            "-L" => {
                i += 1;
                ranges.push(args.get(i).ok_or_else(|| option("blame", "-L"))?.clone());
            }
            _ if arg.starts_with("-L") => ranges.push(arg[2..].to_string()),
            _ if arg.starts_with('-') => return Err(option("blame", arg)),
            _ => names.push(arg.to_string()),
        }
        i += 1;
    }
    if ranges.len() > 1
        || ranges
            .iter()
            .any(|r| r.starts_with(':') || r.starts_with('/'))
    {
        return Err(option("blame", "-L"));
    }
    let (spec, file) = match (file, names.as_slice()) {
        (Some(file), []) => (None, file),
        (Some(file), [spec]) => (Some(spec.clone()), file),
        (None, [file]) => (None, file.clone()),
        (None, [a, b]) => {
            if rev::resolve(repo, a, &git.prefix)?.is_some() {
                (Some(a.clone()), b.clone())
            } else {
                (Some(b.clone()), a.clone())
            }
        }
        _ => {
            return Err(Error::Usage(
                "usage: git blame [<options>] [<rev-opts>] [<rev>] [--] <file>".to_string(),
            ));
        }
    };
    let paths = Pathspec::new(&git.prefix, std::slice::from_ref(&file))
        .ok()
        .and_then(|s| s.literal())
        .ok_or_else(|| Error::Fatal(format!("{file}: '{file}' is outside repository")))?;
    let path = paths[0].clone();
    let shown = String::from_utf8_lossy(&path).into_owned();
    let start = match &spec {
        Some(spec) => {
            let oid = rev::resolve(repo, spec, &git.prefix)?
                .ok_or_else(|| Error::Fatal(format!("bad revision '{spec}'")))?;
            rev::commit_of(repo, oid)?
        }
        None => repo
            .head()?
            .ok_or_else(|| Error::Fatal(format!("no such path '{shown}' in HEAD")))?,
    };
    let tree = repo.commit(&start)?.tree;
    let entry = repo
        .tree_entry(&tree, &path)?
        .filter(|e| !e.is_tree())
        .ok_or_else(|| {
            let name = spec.as_deref().unwrap_or("HEAD");
            Error::Fatal(format!("no such path {shown} in {name}"))
        });
    let mut blame = Blame {
        repo,
        origins: Vec::new(),
        by_name: HashMap::new(),
        entries: Vec::new(),
    };
    // Without a revision the file is the worktree's, which may be ahead of HEAD.
    let mut worktree = None;
    if spec.is_none()
        && let Some(top) = &repo.work_tree
    {
        let full = join(top, &path);
        match repo.fs.read_opt(&full)? {
            Some(data) => {
                let oid = blob_oid(&data);
                if entry.as_ref().map_or(true, |e| e.oid != oid) {
                    if entry.is_err() {
                        return Err(Error::Fatal(format!("no such path '{shown}' in HEAD")));
                    }
                    worktree = Some((oid, Rc::new(data)));
                }
            }
            None => {
                return Err(Error::Fatal(format!(
                    "Cannot lstat '{shown}': No such file or directory"
                )));
            }
        }
    }
    match worktree {
        Some((blob, data)) => blame.origins.push(Origin {
            commit: None,
            path: path.clone(),
            blob,
            data,
        }),
        None => {
            let entry = entry?;
            blame.origin(start, &path, entry.oid)?;
        }
    }
    let lines = TextDiff::new(b"", &blame.origins[0].data).new.len();
    let (first, last) = match ranges.first() {
        Some(range) => line_range(range, lines, &shown)?,
        None => (0, lines),
    };
    if last > first {
        blame.entries.push(Entry {
            lno: first,
            count: last - first,
            s_lno: first,
            origin: 0,
        });
    }
    blame.run(Some(start))?;
    blame.entries.sort_by_key(|e| e.lno);

    // What each version's commit prints as.
    struct Shown {
        hex: String,
        boundary: bool,
        who: Vec<u8>,
        time: String,
    }
    let mut shown_by: HashMap<usize, Shown> = HashMap::new();
    let mut abbrev = repo.abbrev_len()?;
    for entry in &blame.entries {
        if shown_by.contains_key(&entry.origin) {
            continue;
        }
        let origin = &blame.origins[entry.origin];
        let line = match origin.commit {
            Some(oid) => {
                let commit = repo.commit(&oid)?;
                let who = Ident::parse(&commit.author);
                let name = if email {
                    let mut text = b"<".to_vec();
                    text.extend_from_slice(who.email);
                    text.push(b'>');
                    text
                } else {
                    who.name.to_vec()
                };
                abbrev = abbrev.max(repo.short(&oid)?.len());
                Shown {
                    hex: oid.hex(),
                    boundary: commit.parents.is_empty() && !root,
                    who: name,
                    time: date(&DateMode::Iso, who.time, who.zone, git.now),
                }
            }
            None => {
                let minutes = git.offset / 60;
                let zone = (minutes / 60 * 100 + minutes % 60) as i32;
                Shown {
                    hex: Oid::ZERO.hex(),
                    boundary: false,
                    who: if email {
                        b"<not.committed.yet>".to_vec()
                    } else {
                        b"Not Committed Yet".to_vec()
                    },
                    time: date(&DateMode::Iso, git.now, zone, git.now),
                }
            }
        };
        shown_by.insert(entry.origin, line);
    }
    let length = if long { 40 } else { abbrev + 1 };
    let width = |text: &[u8]| String::from_utf8_lossy(text).chars().count();
    let longest_author = shown_by.values().map(|s| width(&s.who)).max().unwrap_or(0);
    let show_name = blame
        .entries
        .iter()
        .any(|e| blame.origins[e.origin].path != path);
    let longest_file = blame
        .entries
        .iter()
        .map(|e| width(&blame.origins[e.origin].path))
        .max()
        .unwrap_or(0);
    let digits = last.max(1).to_string().len();
    let final_lines = TextDiff::new(b"", &blame.origins[0].data).new;
    for entry in &blame.entries {
        let origin = &blame.origins[entry.origin];
        let line = &shown_by[&entry.origin];
        for k in 0..entry.count {
            if line.boundary {
                write!(io.out, "^{}", &line.hex[..length - 1])?;
            } else {
                write!(io.out, "{}", &line.hex[..length])?;
            }
            if show_name {
                io.out.write_all(b" ")?;
                io.out.write_all(&origin.path)?;
                write!(io.out, "{}", " ".repeat(longest_file - width(&origin.path)))?;
            }
            if !suppress {
                io.out.write_all(b" (")?;
                io.out.write_all(&line.who)?;
                write!(
                    io.out,
                    "{} {}",
                    " ".repeat(longest_author - width(&line.who)),
                    line.time
                )?;
            }
            write!(io.out, " {:>digits$}) ", entry.lno + k + 1)?;
            let text = final_lines[entry.lno + k];
            io.out.write_all(text)?;
            if text.last() != Some(&b'\n') {
                io.out.write_all(b"\n")?;
            }
        }
    }
    Ok(0)
}
