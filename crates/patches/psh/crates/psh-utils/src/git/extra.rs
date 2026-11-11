//! worktree list, stash list and show, describe and grep.

use std::collections::{BinaryHeap, HashMap};
use std::io::Write;

use regex::bytes::{Regex, RegexBuilder};

use super::cmd::{quote_path, shorten};
use super::diff::{DiffOpts, Printer, detect_renames, trees};
use super::index::Index;
use super::patch::is_binary;
use super::rev;
use super::status::shown_plain;
use super::store::{Kind, Oid, join};
use super::wild::Pathspec;
use super::worktree::Work;
use super::{Error, Git, Io, Res, option, writes};
use crate::search::posix::for_lines;

pub fn worktree(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let porcelain = match args {
        [list] if list == "list" => false,
        [list, flag] if list == "list" && flag == "--porcelain" => true,
        [list, ..] if list == "list" => return Err(option("worktree list", &args[1])),
        [add, rest @ ..] if add == "add" => return super::checkout::worktree_add(git, io, rest),
        [remove, rest @ ..] if remove == "remove" => {
            return super::checkout::worktree_remove(git, io, rest, false);
        }
        [prune] if prune == "prune" => return super::checkout::worktree_remove(git, io, &[], true),
        _ => return Err(writes("worktree")),
    };
    struct Tree {
        path: Vec<u8>,
        head: Option<Oid>,
        branch: Option<String>,
        bare: bool,
    }
    let repo = &git.repo;
    let fs = repo.fs;
    let read_head = |dir: &[u8]| -> Res<(Option<Oid>, Option<String>)> {
        let Some(text) = fs.read_opt(&join(dir, b"HEAD"))? else {
            return Ok((None, None));
        };
        let text = String::from_utf8_lossy(&text).trim().to_string();
        match text.strip_prefix("ref: ") {
            Some(name) => Ok((repo.resolve_ref(name)?, Some(name.to_string()))),
            None => Ok((Oid::from_hex(text.as_bytes()), None)),
        }
    };
    let mut list = Vec::new();
    let bare = repo.config.bool("core.bare") == Some(true);
    let main_path = if bare {
        repo.common_dir.clone()
    } else {
        match repo.common_dir.strip_suffix(b"/.git") {
            Some(top) => top.to_vec(),
            None => repo.common_dir.clone(),
        }
    };
    let (head, branch) = read_head(&repo.common_dir)?;
    list.push(Tree {
        path: fs.physical(&main_path),
        head,
        branch,
        bare,
    });
    let dir = join(&repo.common_dir, b"worktrees");
    let mut linked = Vec::new();
    for name in fs.list(&dir)? {
        let own = join(&dir, &name);
        let Some(gitfile) = fs.read_opt(&join(&own, b"gitdir"))? else {
            continue;
        };
        let gitfile = String::from_utf8_lossy(&gitfile).trim().to_string();
        let path = gitfile
            .strip_suffix("/.git")
            .unwrap_or(&gitfile)
            .as_bytes()
            .to_vec();
        let (head, branch) = read_head(&own)?;
        linked.push(Tree {
            path,
            head,
            branch,
            bare: false,
        });
    }
    linked.sort_by(|a, b| a.path.cmp(&b.path));
    list.extend(linked);
    if porcelain {
        for tree in &list {
            io.out.write_all(b"worktree ")?;
            io.out.write_all(&tree.path)?;
            io.out.write_all(b"\n")?;
            if tree.bare {
                writeln!(io.out, "bare")?;
            } else {
                if let Some(head) = &tree.head {
                    writeln!(io.out, "HEAD {}", head.hex())?;
                }
                match &tree.branch {
                    Some(branch) => writeln!(io.out, "branch {branch}")?,
                    None => writeln!(io.out, "detached")?,
                }
            }
            writeln!(io.out)?;
        }
        return Ok(0);
    }
    let width = list
        .iter()
        .map(|t| String::from_utf8_lossy(&t.path).chars().count())
        .max()
        .unwrap_or(0);
    let mut names = Vec::new();
    for tree in &list {
        names.push(match &tree.head {
            Some(head) if !tree.bare => repo.short(head)?,
            _ => String::new(),
        });
    }
    let abbrev = names.iter().map(String::len).max().unwrap_or(0);
    for (tree, name) in list.iter().zip(&names) {
        let path = String::from_utf8_lossy(&tree.path).into_owned();
        let pad = " ".repeat(width - path.chars().count());
        write!(io.out, "{path}{pad}  ")?;
        if tree.bare {
            writeln!(io.out, "(bare)")?;
            continue;
        }
        write!(io.out, "{name:<abbrev$} ")?;
        match &tree.branch {
            Some(branch) => writeln!(io.out, "[{}]", shorten(branch))?,
            None => writeln!(io.out, "(detached HEAD)")?,
        }
    }
    Ok(0)
}

pub fn stash(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let repo = &git.repo;
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => {
            let log = repo
                .fs
                .read_opt(&join(&repo.common_dir, b"logs/refs/stash"))?;
            let text = log.unwrap_or_default();
            let lines: Vec<&[u8]> = text
                .split(|&b| b == b'\n')
                .filter(|l| !l.is_empty())
                .collect();
            for (i, line) in lines.iter().rev().enumerate() {
                let message = match line.iter().position(|&b| b == b'\t') {
                    Some(tab) => &line[tab + 1..],
                    None => &b""[..],
                };
                write!(io.out, "stash@{{{i}}}: ")?;
                io.out.write_all(message)?;
                io.out.write_all(b"\n")?;
            }
            Ok(0)
        }
        Some("show") => {
            let mut opts = DiffOpts::new(repo, git.columns)?;
            let mut name = "stash@{0}".to_string();
            let mut i = 1;
            while i < args.len() {
                if !args[i].starts_with('-') {
                    name = match args[i].parse::<usize>() {
                        Ok(n) => format!("stash@{{{n}}}"),
                        Err(_) => args[i].clone(),
                    };
                } else if !opts.parse("stash show", args, &mut i)? {
                    return Err(option("stash show", &args[i]));
                }
                i += 1;
            }
            if repo.resolve_ref("refs/stash")?.is_none() {
                io.warn("No stash entries found.\n");
                return Ok(1);
            }
            let Some(oid) = rev::resolve(repo, &name, &git.prefix)? else {
                return Err(Error::Fatal(format!("{name} is not a valid reference")));
            };
            let commit = repo.commit(&rev::commit_of(repo, oid)?)?;
            let Some(parent) = commit.parents.first() else {
                return Err(Error::Fatal(format!("'{name}' is not a stash-like commit")));
            };
            let base = repo.commit(parent)?.tree;
            let mut changes = Vec::new();
            trees(
                repo,
                Some(&base),
                Some(&commit.tree),
                b"",
                &Pathspec::default(),
                &mut changes,
            )?;
            if let Some(minimum) = opts.renames {
                changes = detect_renames(repo, changes, minimum)?;
            }
            if !opts.any() {
                opts.stat = true;
            }
            Printer { repo, opts: &opts }.print(io.out, &changes)?;
            Ok(0)
        }
        Some("list") => Err(option("stash list", &args[1])),
        _ => Err(writes("stash")),
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct ByDate(i64, std::cmp::Reverse<u64>, Oid);

pub fn describe(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let repo = &git.repo;
    let mut tags = false;
    let mut always = false;
    let mut long = false;
    let mut dirty: Option<String> = None;
    let mut exact = false;
    let mut abbrev: Option<usize> = None;
    let mut spec: Option<&str> = None;
    for arg in args {
        match arg.as_str() {
            "--tags" => tags = true,
            "--always" => always = true,
            "--long" => long = true,
            "--dirty" => dirty = Some("-dirty".to_string()),
            "--exact-match" => exact = true,
            a if a.starts_with("--dirty=") => dirty = Some(a[8..].to_string()),
            a if a.starts_with("--abbrev=") => {
                abbrev = Some(a[9..].parse().map_err(|_| option("describe", a))?);
            }
            a if a.starts_with('-') => return Err(option("describe", a)),
            a => spec = Some(a),
        }
    }
    if dirty.is_some() && spec.is_some() {
        return Err(Error::Fatal(
            "option '--dirty' and commit-ishes cannot be used together".to_string(),
        ));
    }
    let name = spec.unwrap_or("HEAD");
    let Some(oid) = rev::resolve(repo, name, &git.prefix)? else {
        return Err(Error::Fatal(format!("Not a valid object name {name}")));
    };
    let start = rev::commit_of(repo, oid)?;
    let short = |oid: &Oid| -> Res<String> {
        match abbrev {
            Some(0) => Ok(String::new()),
            Some(len) => repo.short_at(oid, len.clamp(4, 40)),
            None => repo.short(oid),
        }
    };
    let suffix = |io: &mut Io<'_>| -> Res<()> {
        if let Some(mark) = &dirty {
            let root = git.work_tree()?.to_vec();
            let work = Work::new(repo, &root)?;
            let tree = repo.commit(&start)?.tree;
            if !work.against_tree(&tree, &Pathspec::default())?.is_empty() {
                write!(io.out, "{mark}")?;
            }
        }
        writeln!(io.out)?;
        Ok(())
    };
    // The names: annotated tags before lightweight ones, a newer tag before an older.
    let mut named: HashMap<Oid, (String, u8, i64)> = HashMap::new();
    for (full, oid) in repo.refs("refs/tags/")? {
        let (kind, data) = repo.object(&oid)?;
        let (target, prio, date) = if kind == Kind::Tag {
            let (peeled, _) = repo.peel(&oid)?;
            let date = data
                .split(|&b| b == b'\n')
                .find_map(|l| l.strip_prefix(b"tagger "))
                .map_or(0, |t| super::repo::Ident::parse(t).time);
            (peeled, 2u8, date)
        } else {
            (oid, 1u8, 0)
        };
        let replace = match named.get(&target) {
            None => true,
            Some((_, old_prio, old_date)) => {
                prio > *old_prio || (prio == 2 && *old_prio == 2 && date > *old_date)
            }
        };
        if replace {
            named.insert(target, (shorten(&full).to_string(), prio, date));
        }
    }
    let usable = |prio: u8| prio == 2 || tags;
    if let Some((tag, prio, _)) = named.get(&start)
        && usable(*prio)
    {
        write!(io.out, "{tag}")?;
        if long {
            write!(io.out, "-0-g{}", short(&start)?)?;
        }
        suffix(io)?;
        return Ok(0);
    }
    if exact {
        return Err(Error::Fatal(format!(
            "no tag exactly matches '{}'",
            start.hex()
        )));
    }
    const SEEN: u32 = 1 << 31;
    struct Candidate {
        name: String,
        depth: usize,
        within: u32,
        order: usize,
    }
    let mut flags: HashMap<Oid, u32> = HashMap::new();
    let mut queue: BinaryHeap<ByDate> = BinaryHeap::new();
    let mut counter = 0u64;
    let push = |queue: &mut BinaryHeap<ByDate>, counter: &mut u64, oid: Oid| -> Res<()> {
        *counter += 1;
        queue.push(ByDate(
            repo.commit(&oid)?.time(),
            std::cmp::Reverse(*counter),
            oid,
        ));
        Ok(())
    };
    flags.insert(start, SEEN);
    push(&mut queue, &mut counter, start)?;
    let mut found: Vec<Candidate> = Vec::new();
    let mut seen = 0usize;
    let mut unannotated = 0;
    let mut annotated = 0;
    let mut gave_up: Option<Oid> = None;
    while let Some(ByDate(_, _, c)) = queue.pop() {
        seen += 1;
        if let Some((tag, prio, _)) = named.get(&c) {
            if !usable(*prio) {
                unannotated += 1;
            } else if found.len() < 10 {
                let within = 1u32 << found.len();
                found.push(Candidate {
                    name: tag.clone(),
                    depth: seen - 1,
                    within,
                    order: found.len(),
                });
                *flags.entry(c).or_default() |= within;
                if *prio == 2 {
                    annotated += 1;
                }
            } else {
                gave_up = Some(c);
                break;
            }
        }
        let mine = flags[&c];
        for candidate in &mut found {
            if mine & candidate.within == 0 {
                candidate.depth += 1;
            }
        }
        if annotated > 0 && queue.is_empty() {
            let best_depth = found.iter().map(|t| t.depth).min().unwrap_or(0);
            let best: u32 = found
                .iter()
                .filter(|t| t.depth == best_depth)
                .fold(0, |acc, t| acc | t.within);
            if mine & best == best {
                break;
            }
        }
        for parent in &repo.commit(&c)?.parents {
            let entry = flags.entry(*parent).or_default();
            let fresh = *entry & SEEN == 0;
            *entry |= mine;
            if fresh {
                push(&mut queue, &mut counter, *parent)?;
            }
        }
    }
    if found.is_empty() {
        if always {
            write!(io.out, "{}", short(&start)?)?;
            suffix(io)?;
            return Ok(0);
        }
        if unannotated > 0 {
            return Err(Error::Fatal(format!(
                "No annotated tags can describe '{}'.\nHowever, there were unannotated tags: try --tags.",
                start.hex()
            )));
        }
        return Err(Error::Fatal(format!(
            "No tags can describe '{}'.\nTry --always, or create some tags.",
            start.hex()
        )));
    }
    found.sort_by(|a, b| a.depth.cmp(&b.depth).then(a.order.cmp(&b.order)));
    if let Some(c) = gave_up {
        push(&mut queue, &mut counter, c)?;
    }
    // The depth of the best candidate, counted to the end.
    let within = found[0].within;
    let mut depth = found[0].depth;
    while let Some(ByDate(_, _, c)) = queue.pop() {
        let mine = flags[&c];
        if mine & within != 0 {
            if queue.iter().all(|q| flags[&q.2] & within != 0) {
                break;
            }
        } else {
            depth += 1;
        }
        for parent in &repo.commit(&c)?.parents {
            let entry = flags.entry(*parent).or_default();
            let fresh = *entry & SEEN == 0;
            *entry |= mine;
            if fresh {
                push(&mut queue, &mut counter, *parent)?;
            }
        }
    }
    write!(io.out, "{}", found[0].name)?;
    if abbrev != Some(0) {
        write!(io.out, "-{depth}-g{}", short(&start)?)?;
    }
    suffix(io)?;
    Ok(0)
}

/// A pattern of grep or log as a regex: POSIX basic or extended, or a fixed string.
pub fn pattern(text: &str, extended: bool, fixed: bool, ignore_case: bool) -> Res<Regex> {
    let source = if fixed {
        regex::escape(text)
    } else {
        for_lines(text.as_bytes(), extended)
            .ok()
            .and_then(|t| t.pattern)
            .ok_or_else(|| option("the pattern", text))?
    };
    RegexBuilder::new(&source)
        .case_insensitive(ignore_case)
        .multi_line(true)
        .build()
        .map_err(|_| option("the pattern", text))
}

fn word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn line_matches(patterns: &[Regex], line: &[u8], words: bool) -> bool {
    patterns.iter().any(|re| {
        if !words {
            return re.is_match(line);
        }
        let mut at = 0;
        while at <= line.len() {
            let Some(m) = re.find_at(line, at) else {
                return false;
            };
            let before = m.start() == 0 || !word(line[m.start() - 1]);
            let after = m.end() == line.len() || !word(line[m.end()]);
            if before && after && m.end() > m.start() {
                return true;
            }
            at = m.start() + 1;
        }
        false
    })
}

pub fn grep(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let repo = &git.repo;
    let mut numbers = repo.config.bool("grep.linenumber") == Some(true);
    let mut names_only = false;
    let mut without = false;
    let mut count = false;
    let mut quiet = false;
    let mut ignore_case = false;
    let mut words = false;
    let mut invert = false;
    let mut extended = false;
    let mut fixed = false;
    let mut no_name = false;
    let mut skip_binary = false;
    let mut text = false;
    let mut cached = false;
    let mut full_name = false;
    let mut sources: Vec<String> = Vec::new();
    let mut rest: Vec<String> = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    let mut dashes = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--" => {
                dashes = true;
                paths.extend(args[i + 1..].iter().cloned());
                break;
            }
            "-n" | "--line-number" => numbers = true,
            "--no-line-number" => numbers = false,
            "-l" | "--files-with-matches" | "--name-only" => names_only = true,
            "-L" | "--files-without-match" => without = true,
            "-c" | "--count" => count = true,
            "-q" | "--quiet" => quiet = true,
            "-i" | "--ignore-case" => ignore_case = true,
            "-w" | "--word-regexp" => words = true,
            "-v" | "--invert-match" => invert = true,
            "-E" | "--extended-regexp" => extended = true,
            "-G" | "--basic-regexp" => extended = false,
            "-F" | "--fixed-strings" => fixed = true,
            "-h" => no_name = true,
            "-H" => no_name = false,
            "-I" => skip_binary = true,
            "-a" | "--text" => text = true,
            "--cached" => cached = true,
            "--full-name" => full_name = true,
            "-r" | "--recursive" | "--no-color" | "--color=never" | "--no-index-ignored" => {}
            "-e" => {
                i += 1;
                sources.push(args.get(i).ok_or_else(|| option("grep", "-e"))?.clone());
            }
            _ if arg.starts_with('-') && arg.len() > 1 => {
                // Letters together: -ni, -nE, -il.
                let letters = &arg[1..];
                if arg.starts_with("--") || !letters.chars().all(|c| "nlLcqiwvEGFhHIar".contains(c))
                {
                    return Err(option("grep", arg));
                }
                for c in letters.chars() {
                    match c {
                        'n' => numbers = true,
                        'l' => names_only = true,
                        'L' => without = true,
                        'c' => count = true,
                        'q' => quiet = true,
                        'i' => ignore_case = true,
                        'w' => words = true,
                        'v' => invert = true,
                        'E' => extended = true,
                        'G' => extended = false,
                        'F' => fixed = true,
                        'h' => no_name = true,
                        'H' => no_name = false,
                        'I' => skip_binary = true,
                        'a' => text = true,
                        _ => {}
                    }
                }
            }
            _ => rest.push(arg.to_string()),
        }
        i += 1;
    }
    let mut rest = rest.into_iter();
    if sources.is_empty() {
        match rest.next() {
            Some(first) => sources.push(first),
            None => return Err(Error::Fatal("no pattern given".to_string())),
        }
    }
    let mut patterns = Vec::new();
    for source in &sources {
        patterns.push(pattern(source, extended, fixed, ignore_case)?);
    }
    // Revisions, then paths.
    let mut revs: Vec<(String, Oid)> = Vec::new();
    let mut in_paths = false;
    for arg in rest {
        if !in_paths && let Some(oid) = rev::resolve(repo, &arg, &git.prefix)? {
            let known = repo
                .fs
                .stat_opt(&join(&git.cwd, arg.as_bytes()), false)?
                .is_some();
            if !dashes && known && repo.work_tree.is_some() {
                return Err(Error::Fatal(format!(
                    "ambiguous argument '{arg}': both revision and filename\n\
                     Use '--' to separate paths from revisions, like this:\n\
                     'git <command> [<revision>...] -- [<file>...]'"
                )));
            }
            revs.push((arg, oid));
            continue;
        }
        in_paths = true;
        paths.push(arg);
    }
    let own = if paths.is_empty() && !git.prefix.is_empty() {
        vec![".".to_string()]
    } else {
        paths
    };
    let spec = Pathspec::new(&git.prefix, &own)
        .map_err(|p| Error::Fatal(format!("{p}: '{p}' is outside repository")))?;
    let mut found = false;
    let mut search = |io: &mut Io<'_>, label: &[u8], path: &[u8], data: &[u8]| -> Res<()> {
        let mut name = label.to_vec();
        let rel = if full_name {
            path.to_vec()
        } else {
            shown_plain(path, &git.prefix)
        };
        name.extend_from_slice(&quote_path(&rel));
        let binary = !text && is_binary(data);
        if binary && skip_binary {
            return Ok(());
        }
        let mut hits = 0usize;
        let mut lines: Vec<(usize, &[u8])> = Vec::new();
        let mut number = 0;
        for line in data.split_inclusive(|&b| b == b'\n') {
            number += 1;
            let line = line.strip_suffix(b"\n").unwrap_or(line);
            if line_matches(&patterns, line, words) != invert {
                hits += 1;
                if names_only || without || quiet || binary {
                    break;
                }
                if !count {
                    lines.push((number, line));
                }
            }
        }
        if hits > 0 {
            found = true;
        }
        if quiet {
            return Ok(());
        }
        if without {
            if hits == 0 {
                io.out.write_all(&name)?;
                io.out.write_all(b"\n")?;
            }
            return Ok(());
        }
        if hits == 0 {
            return Ok(());
        }
        if names_only {
            io.out.write_all(&name)?;
            io.out.write_all(b"\n")?;
        } else if count {
            io.out.write_all(&name)?;
            writeln!(io.out, ":{hits}")?;
        } else if binary {
            io.out.write_all(b"Binary file ")?;
            io.out.write_all(&name)?;
            io.out.write_all(b" matches\n")?;
        } else {
            for (number, line) in lines {
                if !no_name {
                    io.out.write_all(&name)?;
                    io.out.write_all(b":")?;
                }
                if numbers {
                    write!(io.out, "{number}:")?;
                }
                io.out.write_all(line)?;
                io.out.write_all(b"\n")?;
            }
        }
        Ok(())
    };
    if revs.is_empty() {
        let index = Index::load(repo)?;
        let root = if cached {
            Vec::new()
        } else {
            git.work_tree()?.to_vec()
        };
        // A host that answers later is asked for every file before this run ends.
        let mut blocked = None;
        for entry in &index.entries {
            if entry.stage != 0 || entry.mode & 0o170000 != 0o100000 || !spec.matches(&entry.path) {
                continue;
            }
            let data = if cached {
                repo.blob(&entry.oid)?.to_vec()
            } else {
                let full = join(&root, &entry.path);
                let read = match repo.fs.stat_opt(&full, false) {
                    Ok(Some(meta)) if meta.kind == psh_engine::host::FileKind::File => {
                        repo.fs.read(&full)
                    }
                    Ok(_) => continue,
                    Err(e) => Err(e),
                };
                match read {
                    Ok(data) => data,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        blocked = Some(e);
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
            };
            if blocked.is_none() {
                search(io, b"", &entry.path, &data)?;
            }
        }
        if let Some(e) = blocked {
            return Err(e.into());
        }
    } else {
        for (label, oid) in &revs {
            let (peeled, kind) = repo.peel(oid)?;
            let tree = match kind {
                Kind::Commit => repo.commit(&peeled)?.tree,
                Kind::Tree => peeled,
                _ => return Err(option("grep", "a blob")),
            };
            let mut files = Vec::new();
            super::diff::flatten(repo, &tree, b"", &spec, &mut files)?;
            let mut prefix = label.clone().into_bytes();
            prefix.push(b':');
            for (path, mode, oid) in files {
                if mode & 0o170000 != 0o100000 {
                    continue;
                }
                let data = repo.blob(&oid)?;
                search(io, &prefix, &path, &data)?;
            }
        }
    }
    Ok(i32::from(!found))
}
