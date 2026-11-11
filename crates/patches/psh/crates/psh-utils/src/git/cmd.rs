//! The commands: which there are, and the small ones (rev-parse, cat-file, branch, tag,
//! remote, config and other plumbing).

use std::io::Write;

use super::repo::{Entry, RefValue, Repo};
use super::rev;
use super::store::{Kind, Oid, join};
use super::wild::wildmatch;
use super::write::Edit;
use super::{Error, Git, Io, Res, option, writes};

pub fn dispatch(git: &Git<'_>, io: &mut Io<'_>, command: &str, args: &[String]) -> Res<i32> {
    match command {
        "rev-parse" => rev_parse(git, io, args),
        "cat-file" => cat_file(git, io, args),
        "merge-base" => merge_base(git, io, args),
        "symbolic-ref" => symbolic_ref(git, io, args),
        "show-ref" => show_ref(git, io, args),
        "ls-tree" => ls_tree(git, io, args),
        "branch" => branch(git, io, args),
        "tag" => tag(git, io, args),
        "remote" => remote(git, io, args),
        "config" => config(git, io, args),
        "status" => super::status::status(git, io, args),
        "log" => super::log::log(git, io, args),
        "show" => super::log::show(git, io, args),
        "diff" => super::log::diff(git, io, args),
        "rev-list" => super::log::rev_list(git, io, args),
        "ls-files" => super::log::ls_files(git, io, args),
        "blame" | "annotate" => super::blame::blame(git, io, args),
        "grep" => super::extra::grep(git, io, args),
        "describe" => super::extra::describe(git, io, args),
        "stash" => super::stash::stash(git, io, args),
        "add" | "stage" => super::stage::add(git, io, args),
        "rm" => super::stage::rm(git, io, args),
        "mv" => super::stage::mv(git, io, args),
        "reset" => super::stage::reset(git, io, args),
        "restore" => super::stage::restore(git, io, args),
        "commit" => super::commit::commit(git, io, args),
        "checkout" => super::checkout::checkout(git, io, args),
        "switch" => super::checkout::switch(git, io, args),
        "clean" => super::stage::clean(git, io, args),
        "merge" => super::checkout::merge(git, io, args),
        "worktree" => super::extra::worktree(git, io, args),
        _ => Err(Error::Lacks(
            format!("'{command}' is not one of its commands"),
            "git beyond its local commands",
        )),
    }
}

fn exists(git: &Git<'_>, name: &str) -> Res<bool> {
    let path = if name.starts_with('/') {
        name.as_bytes().to_vec()
    } else {
        join(&git.cwd, name.as_bytes())
    };
    Ok(git.repo.fs.stat_opt(&path, false)?.is_some())
}

/// A reference's name as git abbreviates it.
pub fn shorten(name: &str) -> &str {
    for prefix in ["refs/heads/", "refs/tags/", "refs/remotes/"] {
        if let Some(short) = name.strip_prefix(prefix) {
            return short;
        }
    }
    name
}

/// The full reference name a revision argument stands for, if it is one.
fn symbolic_name(repo: &Repo<'_>, spec: &str) -> Res<Option<String>> {
    if let Some(open) = spec.rfind("@{")
        && spec.ends_with('}')
        && matches!(
            spec[open + 2..spec.len() - 1].to_ascii_lowercase().as_str(),
            "u" | "upstream" | "push"
        )
    {
        let name = &spec[..open];
        let branch = if name.is_empty() || name == "HEAD" || name == "@" {
            repo.head_ref()?
        } else {
            rev::find_ref(repo, name)?.map(|(full, _)| full)
        };
        return Ok(branch.and_then(|b| rev::upstream(repo, &b)));
    }
    if spec == "HEAD" || spec == "@" {
        return Ok(Some(repo.head_ref()?.unwrap_or_else(|| "HEAD".to_string())));
    }
    Ok(rev::find_ref(repo, spec)?.map(|(full, _)| full))
}

fn relative(path: &[u8], base: &[u8]) -> Vec<u8> {
    let a: Vec<&[u8]> = path
        .split(|&b| b == b'/')
        .filter(|p| !p.is_empty())
        .collect();
    let b: Vec<&[u8]> = base
        .split(|&b| b == b'/')
        .filter(|p| !p.is_empty())
        .collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<&[u8]> = vec![b".."; b.len() - common];
    parts.extend(&a[common..]);
    if parts.is_empty() {
        return b".".to_vec();
    }
    parts.join(&b'/')
}

fn rev_parse(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut verify = false;
    let mut quiet = false;
    let mut short: Option<usize> = None;
    let mut abbrev_ref = false;
    let mut full_name = false;
    let mut files = false;
    let mut revs = 0;
    let bool_line = |b: bool| if b { "true" } else { "false" };
    let linked = git.repo.git_dir != git.repo.common_dir;
    let in_git_dir = git.cwd.starts_with(&git.repo.git_dir);
    for arg in args {
        let arg = arg.as_str();
        if files {
            writeln!(io.out, "{arg}")?;
            continue;
        }
        match arg {
            "--" => {
                files = true;
                if !verify {
                    writeln!(io.out, "--")?;
                }
            }
            "--verify" => verify = true,
            "-q" | "--quiet" => quiet = true,
            "--short" => short = Some(git.repo.abbrev_len()?),
            "--abbrev-ref" => abbrev_ref = true,
            "--symbolic-full-name" => full_name = true,
            "--show-toplevel" => {
                let tree = git.repo.fs.physical(git.work_tree()?);
                io.out.write_all(&tree)?;
                io.out.write_all(b"\n")?;
            }
            "--show-prefix" => {
                io.out.write_all(&git.prefix)?;
                io.out.write_all(b"\n")?;
            }
            "--show-cdup" => {
                let depth = git.prefix.iter().filter(|&&b| b == b'/').count();
                writeln!(io.out, "{}", "../".repeat(depth))?;
            }
            "--git-dir" | "--absolute-git-dir" => {
                let at_top = git.prefix.is_empty() && !in_git_dir;
                let line =
                    if arg == "--git-dir" && !linked && at_top && git.repo.work_tree.is_some() {
                        b".git".to_vec()
                    } else if in_git_dir && arg == "--git-dir" && git.cwd == git.repo.git_dir {
                        b".".to_vec()
                    } else {
                        git.repo.fs.physical(&git.repo.git_dir)
                    };
                io.out.write_all(&line)?;
                io.out.write_all(b"\n")?;
            }
            "--git-common-dir" => {
                let line = if linked {
                    git.repo.fs.physical(&git.repo.common_dir)
                } else {
                    relative(&git.repo.common_dir, &git.cwd)
                };
                io.out.write_all(&line)?;
                io.out.write_all(b"\n")?;
            }
            "--is-inside-work-tree" => {
                let inside = git.repo.work_tree.is_some() && !in_git_dir;
                writeln!(io.out, "{}", bool_line(inside))?;
            }
            "--is-inside-git-dir" => writeln!(io.out, "{}", bool_line(in_git_dir))?,
            "--is-bare-repository" => {
                writeln!(io.out, "{}", bool_line(git.repo.work_tree.is_none()))?;
            }
            "--is-shallow-repository" => {
                let shallow = git
                    .repo
                    .fs
                    .stat_opt(&join(&git.repo.common_dir, b"shallow"), true)?
                    .is_some();
                writeln!(io.out, "{}", bool_line(shallow))?;
            }
            "--all" | "--branches" | "--tags" | "--remotes" => {
                let prefix = match arg {
                    "--all" => "refs/",
                    "--branches" => "refs/heads/",
                    "--tags" => "refs/tags/",
                    _ => "refs/remotes/",
                };
                for (_, oid) in git.repo.refs(prefix)? {
                    writeln!(io.out, "{}", oid.hex())?;
                }
            }
            _ if arg.starts_with("--short=") => {
                short = Some(
                    arg[8..]
                        .parse::<usize>()
                        .map_err(|_| option("rev-parse", arg))?
                        .clamp(4, 40),
                );
            }
            _ if arg.starts_with("--abbrev-ref=") => abbrev_ref = true,
            _ if arg.starts_with('-') => return Err(option("rev-parse", arg)),
            _ => {
                revs += 1;
                if (abbrev_ref || full_name) && rev::resolve(&git.repo, arg, &git.prefix)?.is_some()
                {
                    if let Some(name) = symbolic_name(&git.repo, arg)? {
                        let name = if abbrev_ref { shorten(&name) } else { &name };
                        writeln!(io.out, "{name}")?;
                    }
                    continue;
                }
                let (spec, negate) = match arg.strip_prefix('^') {
                    Some(rest) => (rest, "^"),
                    None => (arg, ""),
                };
                let range = if verify {
                    None
                } else {
                    spec.find("...")
                        .map(|at| (at, 3))
                        .or_else(|| spec.find("..").map(|at| (at, 2)))
                };
                let print = |io: &mut Io<'_>, oid: &Oid, mark: &str| -> Res<()> {
                    let name = match short {
                        Some(len) => git.repo.short_at(oid, len)?,
                        None => oid.hex(),
                    };
                    writeln!(io.out, "{mark}{name}")?;
                    Ok(())
                };
                if let Some((at, dots)) = range {
                    let left = if at == 0 { "HEAD" } else { &spec[..at] };
                    let right = if at + dots == spec.len() {
                        "HEAD"
                    } else {
                        &spec[at + dots..]
                    };
                    let a = rev::resolve(&git.repo, left, &git.prefix)?;
                    let b = rev::resolve(&git.repo, right, &git.prefix)?;
                    if let (Some(a), Some(b)) = (a, b) {
                        print(io, &b, "")?;
                        if dots == 3 {
                            print(io, &a, "")?;
                            let (a, b) =
                                (rev::commit_of(&git.repo, a)?, rev::commit_of(&git.repo, b)?);
                            for base in rev::merge_bases(&git.repo, a, &[b])? {
                                print(io, &base, "^")?;
                            }
                        } else {
                            print(io, &a, "^")?;
                        }
                        continue;
                    }
                }
                match rev::resolve(&git.repo, spec, &git.prefix)? {
                    Some(oid) => print(io, &oid, negate)?,
                    None if verify => {
                        if quiet {
                            return Err(Error::Exit(1));
                        }
                        return Err(Error::Fatal("Needed a single revision".to_string()));
                    }
                    None => {
                        writeln!(io.out, "{arg}")?;
                        if !exists(git, arg)? {
                            return Err(rev::unknown(arg));
                        }
                        files = true;
                    }
                }
            }
        }
    }
    if verify && revs != 1 {
        if quiet {
            return Err(Error::Exit(1));
        }
        return Err(Error::Fatal("Needed a single revision".to_string()));
    }
    Ok(0)
}

pub fn tree_line(out: &mut dyn Write, entry: &Entry, path: &[u8]) -> Res<()> {
    let kind = match entry.mode {
        0o40000 => "tree",
        0o160000 => "commit",
        _ => "blob",
    };
    write!(out, "{:06o} {kind} {}\t", entry.mode, entry.oid.hex())?;
    out.write_all(&quote_path(path))?;
    out.write_all(b"\n")?;
    Ok(())
}

/// A path as git prints it in listings (core.quotePath): in double quotes with C
/// escapes when it has control characters, quotes, backslashes or bytes above ASCII.
pub fn quote_path(path: &[u8]) -> Vec<u8> {
    if !path
        .iter()
        .any(|&b| b < 0x20 || b == b'"' || b == b'\\' || b >= 0x7f)
    {
        return path.to_vec();
    }
    let mut out = vec![b'"'];
    for &b in path {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            7 => out.extend_from_slice(b"\\a"),
            8 => out.extend_from_slice(b"\\b"),
            9 => out.extend_from_slice(b"\\t"),
            10 => out.extend_from_slice(b"\\n"),
            11 => out.extend_from_slice(b"\\v"),
            12 => out.extend_from_slice(b"\\f"),
            13 => out.extend_from_slice(b"\\r"),
            b if !(0x20..0x7f).contains(&b) => {
                out.extend_from_slice(format!("\\{b:03o}").as_bytes())
            }
            b => out.push(b),
        }
    }
    out.push(b'"');
    out
}

fn cat_file(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut mode = None;
    let mut rest = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-t" | "-s" | "-p" | "-e" => mode = Some(arg.as_str()),
            a if a.starts_with('-') => return Err(option("cat-file", a)),
            a => rest.push(a),
        }
    }
    let (mode, spec) = match (mode, rest.as_slice()) {
        (Some(mode), [spec]) => (mode, *spec),
        (None, [kind @ ("blob" | "tree" | "commit" | "tag"), spec]) => (*kind, *spec),
        _ => {
            return Err(Error::Usage(
                "usage: git cat-file (-t | -s | -e | -p | <type>) <object>".to_string(),
            ));
        }
    };
    let Some(oid) = rev::resolve(&git.repo, spec, &git.prefix)? else {
        if mode == "-e" {
            return Ok(1);
        }
        return Err(Error::Fatal(format!("Not a valid object name {spec}")));
    };
    if mode == "-e" {
        return Ok(i32::from(!git.repo.odb.has(&oid)?));
    }
    let (kind, data) = git.repo.object(&oid)?;
    match mode {
        "-t" => writeln!(io.out, "{}", kind.name())?,
        "-s" => writeln!(io.out, "{}", data.len())?,
        "-p" if kind == Kind::Tree => {
            for entry in git.repo.tree(&oid)?.iter() {
                tree_line(io.out, entry, &entry.name)?;
            }
        }
        "-p" => io.out.write_all(&data)?,
        want => {
            let peeled = match want {
                "tree" if kind == Kind::Commit => git.repo.commit(&oid)?.tree,
                _ => oid,
            };
            let (kind, data) = git.repo.object(&peeled)?;
            if kind.name() != want {
                return Err(Error::Fatal(format!("git cat-file {spec}: bad file")));
            }
            io.out.write_all(&data)?;
        }
    }
    Ok(0)
}

fn merge_base(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut all = false;
    let mut is_ancestor = false;
    let mut revs = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-a" | "--all" => all = true,
            "--is-ancestor" => is_ancestor = true,
            a if a.starts_with('-') => return Err(option("merge-base", a)),
            a => {
                let oid = rev::resolve(&git.repo, a, &git.prefix)?
                    .ok_or_else(|| Error::Fatal(format!("Not a valid object name {a}")))?;
                revs.push(rev::commit_of(&git.repo, oid)?);
            }
        }
    }
    if revs.len() < 2 {
        return Err(Error::Usage(
            "usage: git merge-base [-a | --all] <commit> <commit>...".to_string(),
        ));
    }
    if is_ancestor {
        return Ok(i32::from(!rev::is_ancestor(&git.repo, revs[0], revs[1])?));
    }
    let bases = rev::merge_bases(&git.repo, revs[0], &revs[1..])?;
    if bases.is_empty() {
        return Ok(1);
    }
    for base in bases.iter().take(if all { bases.len() } else { 1 }) {
        writeln!(io.out, "{}", base.hex())?;
    }
    Ok(0)
}

fn symbolic_ref(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut short = false;
    let mut quiet = false;
    let mut names = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--short" => short = true,
            "-q" | "--quiet" => quiet = true,
            a if a.starts_with('-') => return Err(writes("symbolic-ref")),
            a => names.push(a),
        }
    }
    let [name] = names.as_slice() else {
        return Err(writes("symbolic-ref"));
    };
    match rev::symbolic(&git.repo, name)? {
        Some(target) => {
            writeln!(io.out, "{}", if short { shorten(&target) } else { &target })?;
            Ok(0)
        }
        None if quiet => Ok(1),
        None => Err(Error::Fatal(format!("ref {name} is not a symbolic ref"))),
    }
}

fn show_ref(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut heads = false;
    let mut tags = false;
    let mut head = false;
    let mut verify = false;
    let mut quiet = false;
    let mut hash: Option<usize> = None;
    let mut patterns = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--heads" | "--branches" => heads = true,
            "--tags" => tags = true,
            "--head" => head = true,
            "--verify" => verify = true,
            "-q" | "--quiet" => quiet = true,
            "-s" | "--hash" => hash = Some(40),
            "--" => {}
            a if a.starts_with("--hash=") => hash = a[7..].parse().ok(),
            a if a.starts_with('-') => return Err(option("show-ref", a)),
            a => patterns.push(a),
        }
    }
    let line = |io: &mut Io<'_>, name: &str, oid: &Oid| -> Res<()> {
        if quiet {
            return Ok(());
        }
        let mut hex = oid.hex();
        match hash {
            Some(len) => {
                hex.truncate(len.clamp(4, 40));
                writeln!(io.out, "{hex}")?;
            }
            None => writeln!(io.out, "{hex} {name}")?,
        }
        Ok(())
    };
    if verify {
        for name in &patterns {
            let found = if name.starts_with("refs/") || *name == "HEAD" {
                git.repo.resolve_ref(name)?
            } else {
                None
            };
            match found {
                Some(oid) => line(io, name, &oid)?,
                None if quiet => return Ok(1),
                None => return Err(Error::Fatal(format!("'{name}' - not a valid ref"))),
            }
        }
        return Ok(0);
    }
    let mut found = 0;
    if head && let Some(oid) = git.repo.head()? {
        line(io, "HEAD", &oid)?;
        found += 1;
    }
    for (name, oid) in git.repo.refs("refs/")? {
        if (heads || tags)
            && !(heads && name.starts_with("refs/heads/"))
            && !(tags && name.starts_with("refs/tags/"))
        {
            continue;
        }
        if !patterns.is_empty()
            && !patterns
                .iter()
                .any(|p| name == *p || name.ends_with(&format!("/{p}")))
        {
            continue;
        }
        line(io, &name, &oid)?;
        found += 1;
    }
    Ok(i32::from(found == 0))
}

fn ls_tree(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut recurse = false;
    let mut trees = false;
    let mut only_trees = false;
    let mut names = false;
    let mut long = false;
    let mut full = false;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-r" => recurse = true,
            "-t" => trees = true,
            "-d" => only_trees = true,
            "-l" | "--long" => long = true,
            "--name-only" | "--name-status" => names = true,
            "--full-name" | "--full-tree" => full = true,
            "--" => {
                rest.extend(args[i + 1..].iter().map(String::as_str));
                break;
            }
            a if a.starts_with('-') => return Err(option("ls-tree", a)),
            a => rest.push(a),
        }
        i += 1;
    }
    let Some((spec, paths)) = rest.split_first() else {
        return Err(Error::Usage(
            "usage: git ls-tree [<options>] <tree-ish> [<path>...]".to_string(),
        ));
    };
    let oid = rev::resolve(&git.repo, spec, &git.prefix)?
        .ok_or_else(|| Error::Fatal(format!("Not a valid object name {spec}")))?;
    let (peeled, kind) = git.repo.peel(&oid)?;
    let root = match kind {
        Kind::Commit => git.repo.commit(&peeled)?.tree,
        Kind::Tree => peeled,
        _ => return Err(Error::Fatal("not a tree object".to_string())),
    };
    let prefix = if full { Vec::new() } else { git.prefix.clone() };
    let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
    let spec = super::wild::Pathspec::new(&prefix, &paths)
        .map_err(|p| Error::Fatal(format!("{p}: is outside repository")))?;
    let wanted: Vec<Vec<u8>> = match spec.literal() {
        Some(paths) if !paths.is_empty() => paths,
        Some(_) => vec![prefix.strip_suffix(b"/").unwrap_or(&prefix).to_vec()],
        None => return Err(option("ls-tree", "a pattern")),
    };
    let descend_arg: Vec<bool> = if paths.is_empty() {
        vec![true]
    } else {
        paths.iter().map(|p| p.ends_with('/')).collect()
    };
    struct Listing<'g, 'a, 'o> {
        git: &'g Git<'a>,
        io: &'g mut Io<'o>,
        recurse: bool,
        trees: bool,
        only_trees: bool,
        names: bool,
        long: bool,
        strip: usize,
    }
    impl Listing<'_, '_, '_> {
        fn line(&mut self, entry: &Entry, path: &[u8]) -> Res<()> {
            let shown = &path[self.strip.min(path.len())..];
            if self.names {
                self.io.out.write_all(&quote_path(shown))?;
                self.io.out.write_all(b"\n")?;
                return Ok(());
            }
            if self.long {
                let kind = match entry.mode {
                    0o40000 => "tree",
                    0o160000 => "commit",
                    _ => "blob",
                };
                let size = if kind == "blob" {
                    format!("{:>7}", self.git.repo.blob(&entry.oid)?.len())
                } else {
                    format!("{:>7}", "-")
                };
                write!(
                    self.io.out,
                    "{:06o} {kind} {} {size}\t",
                    entry.mode,
                    entry.oid.hex()
                )?;
                self.io.out.write_all(&quote_path(shown))?;
                self.io.out.write_all(b"\n")?;
                return Ok(());
            }
            tree_line(self.io.out, entry, shown)
        }

        fn walk(&mut self, tree: &Oid, base: &[u8]) -> Res<()> {
            for entry in self.git.repo.tree(tree)?.iter() {
                let path = if base.is_empty() {
                    entry.name.clone()
                } else {
                    join(base, &entry.name)
                };
                if entry.is_tree() {
                    if !self.recurse || self.trees || self.only_trees {
                        self.line(entry, &path)?;
                    }
                    if self.recurse && !self.only_trees {
                        self.walk(&entry.oid, &path)?;
                    }
                } else if !self.only_trees {
                    self.line(entry, &path)?;
                }
            }
            Ok(())
        }
    }
    let mut listing = Listing {
        git,
        io,
        recurse,
        trees,
        only_trees,
        names,
        long,
        strip: prefix.len(),
    };
    for (path, descend) in wanted.iter().zip(descend_arg) {
        let Some(entry) = listing.git.repo.tree_entry(&root, path)? else {
            continue;
        };
        if entry.is_tree() && (descend || path.is_empty() || recurse) {
            if recurse && trees && !path.is_empty() && !descend {
                listing.line(&entry, path)?;
            }
            listing.walk(&entry.oid, path)?;
        } else {
            listing.line(&entry, path)?;
        }
    }
    Ok(0)
}

/// The branch each worktree other than this one has checked out.
fn other_worktree_branches(repo: &Repo<'_>) -> Res<Vec<String>> {
    let mut branches = Vec::new();
    let mut heads = Vec::new();
    if repo.git_dir != repo.common_dir {
        heads.push(join(&repo.common_dir, b"HEAD"));
    }
    let dir = join(&repo.common_dir, b"worktrees");
    for name in repo.fs.list(&dir)? {
        let own = join(&dir, &name);
        if own != repo.git_dir {
            heads.push(join(&own, b"HEAD"));
        }
    }
    for head in heads {
        if let Some(text) = repo.fs.read_opt(&head)?
            && let Some(target) = text.strip_prefix(b"ref: ")
        {
            branches.push(String::from_utf8_lossy(target).trim().to_string());
        }
    }
    Ok(branches)
}

/// What git branch shows for a detached HEAD.
pub fn detached(repo: &Repo<'_>, head: &Oid) -> Res<String> {
    for dir in ["rebase-merge", "rebase-apply"] {
        if repo
            .fs
            .stat_opt(&join(&repo.git_dir, dir.as_bytes()), true)?
            .is_some()
        {
            return Err(Error::Lacks(
                "a rebase is in progress".to_string(),
                "git that describes one",
            ));
        }
    }
    let log = repo.fs.read_opt(&join(&repo.git_dir, b"logs/HEAD"))?;
    let text = String::from_utf8_lossy(log.as_deref().unwrap_or_default()).into_owned();
    for line in text.lines().rev() {
        let Some((ids, msg)) = line.split_once('\t') else {
            continue;
        };
        let Some(moved) = msg.strip_prefix("checkout: moving from ") else {
            continue;
        };
        let Some((_, to)) = moved.rsplit_once(" to ") else {
            continue;
        };
        let new = ids.get(41..81).and_then(|h| Oid::from_hex(h.as_bytes()));
        let at = if new == Some(*head) { "at" } else { "from" };
        // A tag or a remote branch is named; anything else is the commit it was.
        let named = match rev::find_ref(repo, to)? {
            Some((full, _))
                if full.starts_with("refs/tags/") || full.starts_with("refs/remotes/") =>
            {
                Some(shorten(&full).to_string())
            }
            _ => None,
        };
        let shown = match (named, new) {
            (Some(name), _) => name,
            (None, Some(oid)) => repo.short(&oid)?,
            (None, None) => to.to_string(),
        };
        return Ok(format!("(HEAD detached {at} {shown})"));
    }
    Ok("(no branch)".to_string())
}

fn subject(repo: &Repo<'_>, oid: &Oid) -> Res<String> {
    let (peeled, kind) = repo.peel(oid)?;
    if kind != Kind::Commit {
        return Ok(String::new());
    }
    let commit = repo.commit(&peeled)?;
    Ok(String::from_utf8_lossy(&super::pretty::subject(commit.message())).into_owned())
}

fn branch(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut all = false;
    let mut remotes = false;
    let mut verbose = 0;
    let mut list = false;
    let mut show_current = false;
    let mut patterns = Vec::new();
    let mut contains: Option<Oid> = None;
    let mut merged: Option<(Oid, bool)> = None;
    let mut format: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let commit_arg = |i: &mut usize| -> Res<Oid> {
            let spec = match args.get(*i + 1) {
                Some(next) if !next.starts_with('-') => {
                    *i += 1;
                    next.as_str()
                }
                _ => "HEAD",
            };
            let oid = rev::resolve(&git.repo, spec, &git.prefix)?
                .ok_or_else(|| Error::Fatal(format!("malformed object name {spec}")))?;
            rev::commit_of(&git.repo, oid)
        };
        match arg {
            "-a" | "--all" => all = true,
            "-r" | "--remotes" => remotes = true,
            "-v" | "--verbose" => verbose += 1,
            "-vv" => verbose += 2,
            "-l" | "--list" => list = true,
            "--show-current" => show_current = true,
            "--no-color" | "--no-column" => {}
            "--contains" => contains = Some(commit_arg(&mut i)?),
            "--merged" => merged = Some((commit_arg(&mut i)?, true)),
            "--no-merged" => merged = Some((commit_arg(&mut i)?, false)),
            "--format" => {
                i += 1;
                format = args.get(i).cloned();
            }
            _ if arg.starts_with("--format=") => format = Some(arg[9..].to_string()),
            _ if arg.starts_with("--sort") || arg.starts_with("--color") => {
                return Err(option("branch", arg));
            }
            _ if arg.starts_with('-') => return super::commit::branch(git, io, args),
            _ => patterns.push(arg.to_string()),
        }
        i += 1;
    }
    let head_ref = git.repo.head_ref()?;
    if show_current {
        if let Some(name) = &head_ref {
            writeln!(io.out, "{}", shorten(name))?;
        }
        return Ok(0);
    }
    let listing = list
        || all
        || remotes
        || verbose > 0
        || contains.is_some()
        || merged.is_some()
        || format.is_some();
    if !patterns.is_empty() && !listing {
        return super::commit::branch(git, io, args);
    }
    let head = git.repo.head()?;
    struct Row {
        mark: char,
        name: String,
        oid: Option<Oid>,
        full: String,
        target: Option<String>,
    }
    let mut rows: Vec<Row> = Vec::new();
    let others = other_worktree_branches(&git.repo)?;
    if !remotes {
        if head_ref.is_none()
            && let Some(head) = &head
        {
            rows.push(Row {
                mark: '*',
                name: detached(&git.repo, head)?,
                oid: Some(*head),
                full: "HEAD".to_string(),
                target: None,
            });
        }
        for (name, oid) in git.repo.refs("refs/heads/")? {
            let mark = if head_ref.as_deref() == Some(name.as_str()) {
                '*'
            } else if others.contains(&name) {
                '+'
            } else {
                ' '
            };
            rows.push(Row {
                mark,
                name: shorten(&name).to_string(),
                oid: Some(oid),
                full: name,
                target: None,
            });
        }
    }
    if all || remotes {
        for (name, oid) in git.repo.refs("refs/remotes/")? {
            let short = shorten(&name);
            let target = match git.repo.read_ref(&name)? {
                Some(RefValue::Symbolic(target)) => Some(shorten(&target).to_string()),
                _ => None,
            };
            rows.push(Row {
                mark: ' ',
                name: if all {
                    format!("remotes/{short}")
                } else {
                    short.to_string()
                },
                oid: Some(oid),
                full: name,
                target,
            });
        }
    }
    let mut kept = Vec::new();
    for row in rows {
        let short = shorten(&row.full);
        if !patterns.is_empty()
            && !patterns
                .iter()
                .any(|p| wildmatch(p.as_bytes(), short.as_bytes(), false))
        {
            continue;
        }
        if let (Some(want), Some(oid)) = (contains, row.oid)
            && !rev::is_ancestor(&git.repo, want, oid)?
        {
            continue;
        }
        if let (Some((tip, want)), Some(oid)) = (merged, row.oid)
            && rev::is_ancestor(&git.repo, oid, tip)? != want
        {
            continue;
        }
        kept.push(row);
    }
    if let Some(format) = format {
        for row in &kept {
            let line = ref_format(
                &git.repo,
                &format,
                &row.full,
                row.oid.as_ref(),
                row.mark == '*',
            )?;
            writeln!(io.out, "{line}")?;
        }
        return Ok(0);
    }
    let width = kept
        .iter()
        .map(|r| r.name.chars().count())
        .max()
        .unwrap_or(0);
    for row in &kept {
        if let Some(target) = &row.target {
            writeln!(io.out, "{} {} -> {target}", row.mark, row.name)?;
            continue;
        }
        if verbose == 0 {
            writeln!(io.out, "{} {}", row.mark, row.name)?;
            continue;
        }
        let Some(oid) = &row.oid else { continue };
        let pad = " ".repeat(width - row.name.chars().count());
        let mut track = String::new();
        if row.full.starts_with("refs/heads/")
            && let Some(up) = rev::upstream(&git.repo, &row.full)
        {
            match git.repo.resolve_ref(&up)? {
                Some(theirs) => {
                    let (ahead, behind) = rev::ahead_behind(&git.repo, *oid, theirs)?;
                    let state = match (ahead, behind) {
                        (0, 0) => String::new(),
                        (a, 0) => format!("ahead {a}"),
                        (0, b) => format!("behind {b}"),
                        (a, b) => format!("ahead {a}, behind {b}"),
                    };
                    if verbose > 1 {
                        let sep = if state.is_empty() { "" } else { ": " };
                        track = format!("[{}{sep}{state}] ", shorten(&up));
                    } else if !state.is_empty() {
                        track = format!("[{state}] ");
                    }
                }
                None if verbose > 1 => track = format!("[{}: gone] ", shorten(&up)),
                None => track = "[gone] ".to_string(),
            }
        }
        writeln!(
            io.out,
            "{} {}{pad} {} {track}{}",
            row.mark,
            row.name,
            git.repo.short(oid)?,
            subject(&git.repo, oid)?
        )?;
    }
    Ok(0)
}

/// A for-each-ref format with the atoms agents use.
fn ref_format(
    repo: &Repo<'_>,
    format: &str,
    full: &str,
    oid: Option<&Oid>,
    is_head: bool,
) -> Res<String> {
    let mut out = String::new();
    let mut rest = format;
    while let Some(at) = rest.find("%(") {
        out.push_str(&rest[..at]);
        let Some(close) = rest[at..].find(')') else {
            break;
        };
        let atom = &rest[at + 2..at + close];
        rest = &rest[at + close + 1..];
        match atom {
            "refname" => out.push_str(full),
            "refname:short" => out.push_str(shorten(full)),
            "objectname" => out.push_str(&oid.map(Oid::hex).unwrap_or_default()),
            "objectname:short" => {
                if let Some(oid) = oid {
                    out.push_str(&repo.short(oid)?);
                }
            }
            "HEAD" => out.push(if is_head { '*' } else { ' ' }),
            "upstream" | "upstream:short" => {
                if let Some(up) = rev::upstream(repo, full) {
                    out.push_str(if atom == "upstream" {
                        &up
                    } else {
                        shorten(&up)
                    });
                }
            }
            "subject" | "contents:subject" => {
                if let Some(oid) = oid {
                    out.push_str(&subject(repo, oid)?);
                }
            }
            _ => return Err(option("--format", &format!("%({atom})"))),
        }
    }
    out.push_str(rest);
    Ok(out)
}

fn tag(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut patterns = Vec::new();
    let mut list = args.is_empty();
    let mut contains: Option<Oid> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-l" | "--list" => list = true,
            "--contains" => {
                i += 1;
                let spec = args.get(i).map_or("HEAD", String::as_str);
                let oid = rev::resolve(&git.repo, spec, &git.prefix)?
                    .ok_or_else(|| Error::Fatal(format!("malformed object name {spec}")))?;
                contains = Some(rev::commit_of(&git.repo, oid)?);
                list = true;
            }
            a if a.starts_with("--sort") || a.starts_with("-n") || a.starts_with("--format") => {
                return Err(option("tag", a));
            }
            a if a.starts_with('-') => return super::commit::tag(git, io, args),
            a => patterns.push(a.to_string()),
        }
        i += 1;
    }
    if !list {
        return super::commit::tag(git, io, args);
    }
    for (name, oid) in git.repo.refs("refs/tags/")? {
        let short = shorten(&name);
        if !patterns.is_empty()
            && !patterns
                .iter()
                .any(|p| wildmatch(p.as_bytes(), short.as_bytes(), false))
        {
            continue;
        }
        if let Some(want) = contains {
            let (peeled, kind) = git.repo.peel(&oid)?;
            if kind != Kind::Commit || !rev::is_ancestor(&git.repo, want, peeled)? {
                continue;
            }
        }
        writeln!(io.out, "{short}")?;
    }
    Ok(0)
}

fn remote_names(repo: &Repo<'_>) -> Vec<String> {
    let mut names: Vec<String> = repo
        .config
        .entries
        .iter()
        .filter_map(|(key, _)| {
            let rest = key.strip_prefix("remote.")?;
            let (name, _) = rest.rsplit_once('.')?;
            Some(name.to_string())
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

fn remote(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut verbose = false;
    let mut rest = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-v" | "--verbose" => verbose = true,
            a => rest.push(a),
        }
    }
    match rest.as_slice() {
        [] | ["show"] => {
            for name in remote_names(&git.repo) {
                if !verbose {
                    writeln!(io.out, "{name}")?;
                    continue;
                }
                let url = git
                    .repo
                    .config
                    .get(&format!("remote.{name}.url"))
                    .unwrap_or(&name);
                let push = git
                    .repo
                    .config
                    .get(&format!("remote.{name}.pushurl"))
                    .unwrap_or(url);
                writeln!(io.out, "{name}\t{url} (fetch)")?;
                writeln!(io.out, "{name}\t{push} (push)")?;
            }
            Ok(0)
        }
        ["get-url", more @ ..] => {
            let push = more.contains(&"--push");
            let Some(name) = more.iter().find(|a| !a.starts_with('-')) else {
                return Err(Error::Usage(
                    "usage: git remote get-url [--push] [--all] <name>".to_string(),
                ));
            };
            let url = git.repo.config.get(&format!("remote.{name}.url"));
            let pushurl = git.repo.config.get(&format!("remote.{name}.pushurl"));
            match (if push { pushurl.or(url) } else { url }, url) {
                (Some(url), _) => {
                    writeln!(io.out, "{url}")?;
                    Ok(0)
                }
                _ => {
                    io.warn(&format!("error: No such remote '{name}'\n"));
                    Ok(2)
                }
            }
        }
        ["show", ..] | ["update", ..] | ["prune", ..] => Err(Error::Lacks(
            format!(
                "'remote {}' talks to another repository: network access is not available in this shell",
                rest[0]
            ),
            "git that reaches other repositories",
        )),
        ["add", name, url] => {
            if remote_names(&git.repo).iter().any(|n| n == name) {
                io.warn(&format!("error: remote {name} already exists.\n"));
                return Ok(3);
            }
            if !super::write::valid_ref(&format!("refs/remotes/{name}")) {
                return Err(Error::Fatal(format!("'{name}' is not a valid remote name")));
            }
            let fetch = format!("+refs/heads/*:refs/remotes/{name}/*");
            remote_config(
                git,
                &[
                    (format!("remote.{name}.url"), Edit::Set(url)),
                    (format!("remote.{name}.fetch"), Edit::Add(&fetch)),
                ],
            )?;
            Ok(0)
        }
        ["set-url", name, url] => {
            if !remote_names(&git.repo).iter().any(|n| n == name) {
                io.warn(&format!("error: No such remote '{name}'\n"));
                return Ok(2);
            }
            remote_config(git, &[(format!("remote.{name}.url"), Edit::Set(url))])?;
            Ok(0)
        }
        ["remove" | "rm", name] => {
            if !remote_names(&git.repo).iter().any(|n| n == name) {
                io.warn(&format!("error: No such remote: '{name}'\n"));
                return Ok(2);
            }
            let mut edits = vec![(format!("remote.{name}"), Edit::RemoveSection)];
            // The branches that followed it follow nothing.
            for (key, value) in &git.repo.config.entries {
                if let Some(branch) = key
                    .strip_prefix("branch.")
                    .and_then(|k| k.strip_suffix(".remote"))
                    && value == name
                {
                    edits.push((format!("branch.{branch}.remote"), Edit::Unset));
                    edits.push((format!("branch.{branch}.merge"), Edit::Unset));
                }
            }
            remote_config(git, &edits)?;
            let prefix = format!("refs/remotes/{name}/");
            let refs = git.repo.refs(&prefix)?;
            let names: Vec<&str> = refs.iter().map(|(full, _)| full.as_str()).collect();
            for full in &names {
                git.remove_loose(full);
            }
            git.remove_loose(&format!("{prefix}HEAD"));
            git.delete_packed(&names)?;
            Ok(0)
        }
        _ => Err(writes("remote")),
    }
}

/// Edits of the repository's own configuration file, one after the other.
fn remote_config(git: &Git<'_>, edits: &[(String, Edit<'_>)]) -> Res<()> {
    let path = join(&git.repo.common_dir, b"config");
    let text = git.repo.fs.read_opt(&path)?.unwrap_or_default();
    let mut text = String::from_utf8_lossy(&text).into_owned();
    for (key, edit) in edits {
        let edit = match edit {
            Edit::Set(value) => Edit::Set(value),
            Edit::Add(value) => Edit::Add(value),
            Edit::Unset => Edit::Unset,
            Edit::RemoveSection => Edit::RemoveSection,
            Edit::RenameSection(to) => Edit::RenameSection(to),
        };
        if let Some(next) = super::write::edit_config(&text, key, edit) {
            text = next;
        }
    }
    git.write(path, text.into_bytes());
    Ok(())
}

fn config(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let env = |name: &[u8]| -> Option<Vec<u8>> {
        git.env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    let path = join(&git.repo.common_dir, b"config");
    let mut push = |op: super::write::Op| git.op(op);
    super::setup::config(
        git.repo.fs,
        &env,
        Some((&path, &git.repo.config)),
        args,
        io,
        &mut push,
    )
}
