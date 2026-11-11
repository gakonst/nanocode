//! What goes through the index: add, rm, mv, reset and restore --staged.

use std::io::Write;

use super::cmd::quote_path;
use super::diff::flatten;
use super::index::{CacheTree, Index, IndexEntry};
use super::repo::Repo;
use super::rev;
use super::status::shown_plain;
use super::store::{Kind, Oid, join};
use super::wild::Pathspec;
use super::worktree::{State, Untracked, Work, untracked};
use super::write::Op;
use super::{Error, Git, Io, Res, option};

/// An answer, or None with the run marked when the host has yet to give it: the
/// command goes on, to ask for the rest in the same run.
pub fn later<T>(answer: Res<T>, blocked: &mut bool) -> Res<Option<T>> {
    match answer {
        Ok(value) => Ok(Some(value)),
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
            *blocked = true;
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// The error of a run that was not answered in full.
pub fn answered(blocked: bool) -> Res<()> {
    if blocked {
        return Err(Error::Io(std::io::ErrorKind::WouldBlock.into()));
    }
    Ok(())
}

/// The paths a command names, as its pathspec takes them: one given from the root of
/// the file system is one of the worktree's.
pub fn named(git: &Git<'_>, paths: &[String]) -> Res<Vec<String>> {
    let root = git.work_tree()?;
    let real = git.repo.fs.physical(root);
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        if !path.starts_with('/') {
            out.push(path.clone());
            continue;
        }
        let full = super::repo::normalize(path.as_bytes());
        let inside = [root, real.as_slice()].into_iter().find_map(|top| {
            let rest = full.strip_prefix(top)?;
            (rest.is_empty() || rest[0] == b'/').then(|| rest.strip_prefix(b"/").unwrap_or(rest))
        });
        match inside {
            Some(rest) => out.push(format!(":(top){}", String::from_utf8_lossy(rest))),
            None => {
                return Err(Error::Fatal(format!(
                    "{path}: '{path}' is outside repository at '{}'",
                    String::from_utf8_lossy(&real)
                )));
            }
        }
    }
    Ok(out)
}

/// The index to change. An entry whose file may have changed without its time
/// showing it (written in the second the index was) is compared now, and marked when
/// it did change, as git marks it: the index written next is younger than the file.
pub fn edit(work: &Work<'_, '_>) -> Res<Index> {
    let mut index = work.index.clone();
    let written = index.written.div_euclid(1_000_000_000);
    let mut blocked = false;
    for entry in &mut index.entries {
        if entry.stage != 0 {
            return Err(super::diff::unmerged());
        }
        if i64::from(entry.mtime.0) < written || entry.mode == 0o160000 {
            continue;
        }
        if let Some(State::Changed(_)) = later(work.state(entry), &mut blocked)? {
            entry.size = 0;
        }
    }
    answered(blocked)?;
    Ok(index)
}

/// The trees of a commit as the index's record of what it was written as.
pub fn prime(repo: &Repo<'_>, tree: &Oid, name: &[u8]) -> Res<CacheTree> {
    let mut count = 0;
    let mut children = Vec::new();
    for entry in repo.tree(tree)?.iter() {
        if entry.is_tree() {
            let child = prime(repo, &entry.oid, &entry.name)?;
            count += child.count;
            children.push(child);
        } else {
            count += 1;
        }
    }
    Ok(CacheTree {
        name: name.to_vec(),
        count,
        oid: *tree,
        children,
    })
}

fn env_of<'g>(git: &'g Git<'_>) -> impl Fn(&[u8]) -> Option<Vec<u8>> + 'g {
    |name: &[u8]| {
        git.env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }
}

/// Stages a file of the worktree as it is. False when it is not a file.
fn stage_file(git: &Git<'_>, work: &Work<'_, '_>, index: &mut Index, path: &[u8]) -> Res<bool> {
    let old_mode = index.get(path).map_or(0, |e| e.mode);
    let Some((file, meta)) = work.file(path, old_mode)? else {
        return Ok(false);
    };
    let data = file.data.as_ref().map(|d| d.to_vec()).unwrap_or_default();
    let oid = git.repo.odb.write(Kind::Blob, data)?;
    let mut entry = IndexEntry::new(path.to_vec(), file.mode, oid);
    entry.record(&meta, None);
    index.set(entry);
    Ok(true)
}

/// Stages what changed among the tracked files a pathspec names: the worktree's
/// content, or the removal of a file that is gone. Each path staged is passed to
/// said.
pub fn update_tracked(
    git: &Git<'_>,
    work: &Work<'_, '_>,
    index: &mut Index,
    spec: &Pathspec,
    said: &mut dyn FnMut(&str, &[u8]) -> Res<()>,
    dry: bool,
) -> Res<()> {
    let mut blocked = false;
    for entry in &work.index.entries {
        if entry.skip_worktree || !spec.matches(&entry.path) || entry.mode == 0o160000 {
            continue;
        }
        let Some(state) = later(work.state(entry), &mut blocked)? else {
            continue;
        };
        match state {
            State::Same => {}
            State::Gone => {
                said("remove", &entry.path)?;
                if !dry {
                    index.remove(&entry.path);
                }
            }
            State::Changed(_) => {
                said("add", &entry.path)?;
                if !dry {
                    later(stage_file(git, work, index, &entry.path), &mut blocked)?;
                }
            }
        }
    }
    answered(blocked)
}

pub fn add(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut all, mut update, mut dry, mut verbose, mut force) =
        (false, false, false, false, false);
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-A" | "--all" | "--no-ignore-removal" => all = true,
            "-u" | "--update" => update = true,
            "-n" | "--dry-run" => dry = true,
            "-v" | "--verbose" => verbose = true,
            "-f" | "--force" => force = true,
            "--" => {
                paths.extend(args[i + 1..].iter().cloned());
                break;
            }
            "-p" | "--patch" | "-i" | "--interactive" | "-e" | "--edit" => {
                return Err(Error::Lacks(
                    format!("'add' with '{arg}' asks what to stage"),
                    "terminal for git to ask on",
                ));
            }
            _ if arg.starts_with('-') => return Err(option("add", arg)),
            _ => paths.push(arg.to_string()),
        }
        i += 1;
    }
    let root = git.work_tree()?.to_vec();
    if paths.is_empty() && !all && !update {
        io.warn(
            "Nothing specified, nothing added.\nhint: Maybe you wanted to say 'git add .'?\nhint: Disable this message with \"git config set advice.addEmptyPathspec false\"\n",
        );
        return Ok(0);
    }
    git.lock_index()?;
    let outside = |p: String| Error::Fatal(format!("{p}: '{p}' is outside repository"));
    let specs = named(git, &paths)?;
    let spec = if paths.is_empty() {
        Pathspec::default()
    } else {
        Pathspec::new(&git.prefix, &specs).map_err(outside)?
    };
    let work = Work::new(&git.repo, &root)?;
    let mut index = edit(&work)?;
    let env = env_of(git);
    // The walk for untracked files first: it passes the tracked ones too.
    let others = if update {
        Vec::new()
    } else {
        untracked(&work, Untracked::All, &spec, &env)?
    };
    work.answered()?;
    // Every path named must name something, before anything is staged.
    let mut ignored = Vec::new();
    for (path, one) in paths.iter().zip(&specs) {
        let one = Pathspec::new(&git.prefix, std::slice::from_ref(one)).map_err(outside)?;
        if work.index.entries.iter().any(|e| one.matches(&e.path))
            || others
                .iter()
                .any(|o| one.matches(o.strip_suffix(b"/").unwrap_or(o)))
        {
            continue;
        }
        let full = if path.starts_with('/') {
            path.as_bytes().to_vec()
        } else {
            join(&git.cwd, path.as_bytes())
        };
        match git.repo.fs.stat_opt(&full, false)? {
            Some(meta) if meta.kind == psh_engine::host::FileKind::Dir => {}
            Some(_) if update => {
                return Err(Error::Says(
                    format!("error: pathspec '{path}' did not match any file(s) known to git\n"),
                    128,
                ));
            }
            Some(_) if force => {
                let rel = super::repo::normalize(&full);
                let rel = rel
                    .strip_prefix(root.as_slice())
                    .map(|p| p.strip_prefix(b"/").unwrap_or(p).to_vec())
                    .ok_or_else(|| outside(path.clone()))?;
                ignored.push((rel, true));
            }
            Some(_) => ignored.push((path.as_bytes().to_vec(), false)),
            None => {
                return Err(Error::Fatal(format!(
                    "pathspec '{path}' did not match any files"
                )));
            }
        }
    }
    let mut status = 0;
    if ignored.iter().any(|(_, forced)| !forced) {
        let mut text =
            "The following paths are ignored by one of your .gitignore files:\n".to_string();
        for (path, forced) in &ignored {
            if !forced {
                text.push_str(&String::from_utf8_lossy(path));
                text.push('\n');
            }
        }
        text.push_str(
            "hint: Use -f if you really want to add them.\nhint: Disable this message with \"git config set advice.addIgnoredFile false\"\n",
        );
        io.warn(&text);
        status = 1;
    }
    let mut lines = Vec::new();
    let mut said = |what: &str, path: &[u8]| -> Res<()> {
        if verbose || dry {
            lines.extend_from_slice(format!("{what} '").as_bytes());
            lines.extend_from_slice(path);
            lines.extend_from_slice(b"'\n");
        }
        Ok(())
    };
    update_tracked(git, &work, &mut index, &spec, &mut said, dry)?;
    let mut blocked = false;
    let forced = ignored.into_iter().filter(|(_, f)| *f).map(|(p, _)| p);
    for path in others.into_iter().chain(forced) {
        if path.ends_with(b"/") {
            return Err(Error::Lacks(
                format!(
                    "'{}' is a repository of its own",
                    String::from_utf8_lossy(&path)
                ),
                "git that adds embedded repositories and submodules",
            ));
        }
        said("add", &path)?;
        if !dry {
            later(stage_file(git, &work, &mut index, &path), &mut blocked)?;
        }
    }
    answered(blocked)?;
    io.out.write_all(&lines)?;
    if !dry {
        git.set_index(index);
    }
    Ok(status)
}

/// What is under a tree for a pathspec, into the index: the entries of the paths it
/// names become the tree's, and those the tree does not have go.
fn from_tree(
    git: &Git<'_>,
    work: &Work<'_, '_>,
    index: &mut Index,
    tree: Option<&Oid>,
    spec: &Pathspec,
) -> Res<usize> {
    let mut files = Vec::new();
    if let Some(tree) = tree {
        flatten(&git.repo, tree, b"", spec, &mut files)?;
    }
    let mut touched = 0;
    let gone: Vec<Vec<u8>> = index
        .entries
        .iter()
        .filter(|e| spec.matches(&e.path))
        .filter(|e| {
            files
                .binary_search_by(|f| f.0.as_slice().cmp(&e.path))
                .is_err()
        })
        .map(|e| e.path.clone())
        .collect();
    for path in gone {
        index.remove(&path);
        touched += 1;
    }
    let mut blocked = false;
    for (path, mode, oid) in files {
        touched += 1;
        if index
            .get(&path)
            .is_some_and(|e| e.oid == oid && e.mode == mode)
        {
            continue;
        }
        let mut entry = IndexEntry::new(path, mode, oid);
        // The file is recorded where it holds what the tree does, so that it reads as
        // unchanged without being compared again.
        if mode != 0o160000
            && let Some(Some((file, meta))) = later(work.file(&entry.path, mode), &mut blocked)?
            && file.oid == oid
            && file.mode == mode
        {
            entry.record(&meta, None);
        }
        index.set(entry);
    }
    answered(blocked)?;
    Ok(touched)
}

/// The lines git prints after a reset that left the worktree as it was.
fn unstaged_after(
    git: &Git<'_>,
    io: &mut Io<'_>,
    mut work: Work<'_, '_>,
    index: &Index,
) -> Res<()> {
    work.index = index.clone();
    work.index.written = i64::MAX;
    work.quick = true;
    let changes = work.unstaged(&Pathspec::default())?;
    if changes.is_empty() {
        return Ok(());
    }
    writeln!(io.out, "Unstaged changes after reset:")?;
    for change in changes {
        write!(io.out, "{}\t", change.status())?;
        io.out
            .write_all(&quote_path(&shown_plain(&change.path, &git.prefix)))?;
        io.out.write_all(b"\n")?;
    }
    Ok(())
}

fn tree_of(git: &Git<'_>, spec: &str) -> Res<(Oid, Oid)> {
    let oid = rev::resolve(&git.repo, spec, &git.prefix)?.ok_or_else(|| rev::unknown(spec))?;
    let commit = rev::commit_of(&git.repo, oid)?;
    Ok((commit, git.repo.commit(&commit)?.tree))
}

pub fn reset(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut soft, mut hard, mut quiet) = (false, false, false);
    let mut rest: Vec<String> = Vec::new();
    let mut dashes = None;
    for (i, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "--soft" => soft = true,
            "--hard" => hard = true,
            "--mixed" => {}
            "-q" | "--quiet" => quiet = true,
            "--no-refresh" | "--refresh" => {}
            "--" => {
                dashes = Some(rest.len());
                rest.extend(args[i + 1..].iter().cloned());
                break;
            }
            "-p" | "--patch" => {
                return Err(Error::Lacks(
                    "'reset' with '-p' asks what to reset".to_string(),
                    "terminal for git to ask on",
                ));
            }
            a if a.starts_with('-') => return Err(option("reset", a)),
            _ => rest.push(arg.clone()),
        }
    }
    // The first word is a revision when it names one; what follows are paths.
    let head = git.repo.head()?;
    let (spec, paths) = match (rest.first(), dashes) {
        (Some(first), Some(n)) if n >= 1 => (Some(first.clone()), rest[1..].to_vec()),
        (_, Some(_)) => (None, rest.clone()),
        (Some(first), None) if rev::resolve(&git.repo, first, &git.prefix)?.is_some() => {
            (Some(first.clone()), rest[1..].to_vec())
        }
        (_, None) => (None, rest.clone()),
    };
    let target = match &spec {
        Some(spec) => Some(tree_of(git, spec)?),
        None => match head {
            Some(head) => Some((
                head,
                git.repo.commit(&rev::commit_of(&git.repo, head)?)?.tree,
            )),
            None => None,
        },
    };
    let tree = target.map(|(_, tree)| tree);
    let root = git.work_tree()?.to_vec();
    git.lock_index()?;
    if !paths.is_empty() {
        let paths = named(git, &paths)?;
        if soft || hard {
            return Err(Error::Fatal(format!(
                "Cannot do {} reset with paths.",
                if soft { "soft" } else { "hard" }
            )));
        }
        let spec = Pathspec::new(&git.prefix, &paths)
            .map_err(|p| Error::Fatal(format!("{p}: '{p}' is outside repository")))?;
        let work = Work::new(&git.repo, &root)?;
        let mut index = edit(&work)?;
        from_tree(git, &work, &mut index, tree.as_ref(), &spec)?;
        if !quiet {
            unstaged_after(git, io, work, &index)?;
        }
        git.set_index(index);
        return Ok(0);
    }
    let said = spec.as_deref().unwrap_or("HEAD");
    if hard {
        let work = Work::new(&git.repo, &root)?;
        let index = edit(&work)?;
        let index = super::checkout::hard(git, &work, index, tree.as_ref())?;
        git.set_index(index);
    } else if !soft {
        let work = Work::new(&git.repo, &root)?;
        let mut index = edit(&work)?;
        from_tree(git, &work, &mut index, tree.as_ref(), &Pathspec::default())?;
        index.tree = match &tree {
            Some(tree) => Some(prime(&git.repo, tree, b"")?),
            None => None,
        };
        if !quiet {
            unstaged_after(git, io, work, &index)?;
        }
        git.set_index(index);
    }
    if let Some((commit, _)) = target {
        if let Some(head) = head {
            git.write(
                join(&git.repo.git_dir, b"ORIG_HEAD"),
                format!("{}\n", head.hex()).into_bytes(),
            );
        }
        git.move_head(commit, &format!("reset: moving to {said}"))?;
        if hard && !quiet {
            let subject = super::pretty::subject(git.repo.commit(&commit)?.message());
            write!(io.out, "HEAD is now at {} ", git.repo.short(&commit)?)?;
            io.out.write_all(&subject)?;
            io.out.write_all(b"\n")?;
        }
    }
    Ok(0)
}

/// git restore: the index from a commit (--staged), the worktree from the index or a
/// commit, or both.
pub fn restore(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut staged, mut worktree) = (false, false);
    let mut source = None;
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-S" | "--staged" => staged = true,
            "-W" | "--worktree" => worktree = true,
            "-SW" | "-WS" => (staged, worktree) = (true, true),
            "-q" | "--quiet" | "--no-progress" | "--progress" => {}
            "-s" | "--source" => {
                i += 1;
                source = args.get(i).cloned();
            }
            "--" => {
                paths.extend(args[i + 1..].iter().cloned());
                break;
            }
            "-p" | "--patch" => {
                return Err(Error::Lacks(
                    "'restore' with '-p' asks what to restore".to_string(),
                    "terminal for git to ask on",
                ));
            }
            _ if arg.starts_with("--source=") => source = Some(arg[9..].to_string()),
            _ if arg.starts_with("-s") && arg.len() > 2 => source = Some(arg[2..].to_string()),
            _ if arg.starts_with('-') => return Err(option("restore", arg)),
            _ => paths.push(arg.to_string()),
        }
        i += 1;
    }
    if paths.is_empty() {
        return Err(Error::Fatal(
            "you must specify path(s) to restore".to_string(),
        ));
    }
    if !staged {
        worktree = true;
    }
    let root = git.work_tree()?.to_vec();
    let paths = named(git, &paths)?;
    git.lock_index()?;
    let spec = Pathspec::new(&git.prefix, &paths)
        .map_err(|p| Error::Fatal(format!("{p}: '{p}' is outside repository")))?;
    let tree = match (&source, staged) {
        (Some(spec), _) => Some(tree_of(git, spec)?.1),
        (None, true) => match git.repo.head()? {
            Some(head) => Some(git.repo.commit(&rev::commit_of(&git.repo, head)?)?.tree),
            None => None,
        },
        (None, false) => None,
    };
    let work = Work::new(&git.repo, &root)?;
    let mut index = edit(&work)?;
    let from_source = source.is_some() || staged;
    // What the paths name in the index or in the source, else git says so.
    for path in &paths {
        let one = Pathspec::new(&git.prefix, std::slice::from_ref(path)).map_err(Error::Fatal)?;
        let mut known = index.entries.iter().any(|e| one.matches(&e.path));
        if !known && let Some(tree) = &tree {
            let mut files = Vec::new();
            flatten(&git.repo, tree, b"", &one, &mut files)?;
            known = !files.is_empty();
        }
        if !known {
            return Err(Error::Says(
                format!("error: pathspec '{path}' did not match any file(s) known to git\n"),
                1,
            ));
        }
    }
    if staged {
        from_tree(git, &work, &mut index, tree.as_ref(), &spec)?;
    }
    if worktree {
        super::checkout::paths(
            git,
            &work,
            &mut index,
            if from_source { tree.as_ref() } else { None },
            from_source,
            &spec,
            staged,
        )?;
    }
    let _ = io;
    git.set_index(index);
    Ok(0)
}

/// The lines git rm prints when it will not remove.
fn refuse(kind: &str, paths: &[Vec<u8>], hint: &str) -> String {
    let (files, have) = if paths.len() == 1 {
        ("file", "has")
    } else {
        ("files", "have")
    };
    let mut text = format!("error: the following {files} {have} {kind}:\n");
    for path in paths {
        text.push_str(&format!("    {}\n", String::from_utf8_lossy(path)));
    }
    text.push_str(hint);
    text
}

pub fn rm(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut cached, mut recursive, mut force, mut quiet, mut dry, mut unmatched) =
        (false, false, false, false, false, false);
    let mut paths = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "--cached" => cached = true,
            "-r" => recursive = true,
            "-f" | "--force" => force = true,
            "-rf" | "-fr" => (recursive, force) = (true, true),
            "-q" | "--quiet" => quiet = true,
            "-n" | "--dry-run" => dry = true,
            "--ignore-unmatch" => unmatched = true,
            "--" => {
                paths.extend(args[i + 1..].iter().cloned());
                break;
            }
            a if a.starts_with('-') => return Err(option("rm", a)),
            _ => paths.push(arg.clone()),
        }
    }
    if paths.is_empty() {
        return Err(Error::Fatal(
            "No pathspec was given. Which files should I remove?".to_string(),
        ));
    }
    let root = git.work_tree()?.to_vec();
    let paths = named(git, &paths)?;
    git.lock_index()?;
    let work = Work::new(&git.repo, &root)?;
    let mut index = edit(&work)?;
    let mut doomed: Vec<Vec<u8>> = Vec::new();
    for path in &paths {
        let one = Pathspec::new(&git.prefix, std::slice::from_ref(path))
            .map_err(|p| Error::Fatal(format!("{p}: '{p}' is outside repository")))?;
        let matched: Vec<&IndexEntry> = index
            .entries
            .iter()
            .filter(|e| one.matches(&e.path))
            .collect();
        if matched.is_empty() {
            if unmatched {
                continue;
            }
            return Err(Error::Fatal(format!(
                "pathspec '{path}' did not match any files"
            )));
        }
        let literal = one.literal().and_then(|l| l.into_iter().next());
        let whole = literal
            .as_ref()
            .is_some_and(|l| matched.iter().any(|e| e.path == *l));
        if !whole && !recursive && literal.is_some() {
            return Err(Error::Fatal(format!(
                "not removing '{path}' recursively without -r"
            )));
        }
        doomed.extend(matched.iter().map(|e| e.path.clone()));
    }
    doomed.sort();
    doomed.dedup();
    if !force {
        let head_tree = match git.repo.head()? {
            Some(head) => Some(git.repo.commit(&rev::commit_of(&git.repo, head)?)?.tree),
            None => None,
        };
        let (mut both, mut index_only, mut local_only) = (Vec::new(), Vec::new(), Vec::new());
        let mut blocked = false;
        for path in &doomed {
            let Some(entry) = index.get(path) else {
                continue;
            };
            let in_head = match &head_tree {
                Some(tree) => git.repo.tree_entry(tree, path)?,
                None => None,
            };
            let staged = !in_head.is_some_and(|e| e.oid == entry.oid && e.mode == entry.mode);
            let local = matches!(
                later(work.state(entry), &mut blocked)?,
                Some(State::Changed(_))
            );
            let shown = shown_plain(path, &git.prefix);
            match (staged, local) {
                (true, true) => both.push(shown),
                (true, false) if !cached => index_only.push(shown),
                (false, true) if !cached => local_only.push(shown),
                _ => {}
            }
        }
        answered(blocked)?;
        let mut text = String::new();
        if !both.is_empty() {
            text.push_str(&refuse(
                "staged content different from both the\nfile and the HEAD",
                &both,
                "(use -f to force removal)\n",
            ));
        }
        let keep = "(use --cached to keep the file, or -f to force removal)\n";
        if !index_only.is_empty() {
            text.push_str(&refuse("changes staged in the index", &index_only, keep));
        }
        if !local_only.is_empty() {
            text.push_str(&refuse("local modifications", &local_only, keep));
        }
        if !text.is_empty() {
            return Err(Error::Says(text, 1));
        }
    }
    let mut dirs: Vec<Vec<u8>> = Vec::new();
    for path in &doomed {
        if !quiet {
            io.out.write_all(b"rm '")?;
            io.out.write_all(&shown_plain(path, &git.prefix))?;
            io.out.write_all(b"'\n")?;
        }
        if dry {
            continue;
        }
        index.remove(path);
        if !cached {
            git.op(Op::Remove(join(&root, path)));
            let mut at = path.as_slice();
            while let Some(slash) = at.iter().rposition(|&b| b == b'/') {
                at = &at[..slash];
                dirs.push(at.to_vec());
            }
        }
    }
    // Deepest first, so that a directory emptied of directories goes too.
    dirs.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    dirs.dedup();
    for dir in dirs {
        git.op(Op::Rmdir(join(&root, &dir)));
    }
    if !dry {
        git.set_index(index);
    }
    Ok(0)
}

pub fn mv(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut force, mut verbose, mut dry, mut skip) = (false, false, false, false);
    let mut names = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "-f" | "--force" => force = true,
            "-v" | "--verbose" => verbose = true,
            "-n" | "--dry-run" => dry = true,
            "-k" => skip = true,
            "--" => {
                names.extend(args[i + 1..].iter().cloned());
                break;
            }
            a if a.starts_with('-') => return Err(option("mv", a)),
            _ => names.push(arg.clone()),
        }
    }
    if names.len() < 2 {
        return Err(Error::Usage(
            "usage: git mv [-v] [-f] [-n] [-k] <source> <destination>\n   or: git mv [-v] [-f] [-n] [-k] <source>... <destination-directory>".to_string(),
        ));
    }
    let root = git.work_tree()?.to_vec();
    git.lock_index()?;
    let work = Work::new(&git.repo, &root)?;
    let mut index = edit(&work)?;
    let fs = git.repo.fs;
    // A name as given, as a path of the worktree.
    let inside = |name: &str| -> Res<Vec<u8>> {
        let full = if name.starts_with('/') {
            name.as_bytes().to_vec()
        } else {
            join(&git.cwd, name.as_bytes())
        };
        let full = super::repo::normalize(&full);
        match full.strip_prefix(root.as_slice()) {
            Some(rest) if rest.is_empty() || rest[0] == b'/' => {
                Ok(rest.strip_prefix(b"/").unwrap_or(rest).to_vec())
            }
            _ => Err(Error::Fatal(format!(
                "{name}: '{name}' is outside repository"
            ))),
        }
    };
    let last = names.pop().unwrap_or_default();
    let target = inside(&last)?;
    let is_dir = |path: &[u8]| -> Res<bool> {
        Ok(fs
            .stat_opt(&join(&root, path), true)?
            .is_some_and(|m| m.kind == psh_engine::host::FileKind::Dir))
    };
    let into_dir = target.is_empty() || is_dir(&target)?;
    if names.len() > 1 && !into_dir {
        return Err(Error::Fatal(format!(
            "destination '{last}' is not a directory"
        )));
    }
    for name in &names {
        let src = inside(name)?;
        let dst = if into_dir {
            let base = src.rsplit(|&b| b == b'/').next().unwrap_or(&src);
            if target.is_empty() {
                base.to_vec()
            } else {
                join(&target, base)
            }
        } else {
            target.clone()
        };
        let shown = |path: &[u8]| String::from_utf8_lossy(path).into_owned();
        let bad = |what: &str| -> Res<i32> {
            if skip {
                return Ok(0);
            }
            Err(Error::Fatal(format!(
                "{what}, source={}, destination={}",
                shown(&src),
                shown(&dst)
            )))
        };
        if fs.stat_opt(&join(&root, &src), false)?.is_none() {
            bad("bad source")?;
            continue;
        }
        let dir = is_dir(&src)?;
        let mut prefix = src.clone();
        prefix.push(b'/');
        let moved: Vec<IndexEntry> = index
            .entries
            .iter()
            .filter(|e| {
                if dir {
                    e.path.starts_with(&prefix)
                } else {
                    e.path == src
                }
            })
            .cloned()
            .collect();
        if moved.is_empty() {
            bad(if dir {
                "source directory is empty"
            } else {
                "not under version control"
            })?;
            continue;
        }
        if fs.stat_opt(&join(&root, &dst), false)?.is_some() && !(force && !dir) {
            bad("destination exists")?;
            continue;
        }
        if verbose || dry {
            writeln!(io.out, "Renaming {} to {}", shown(&src), shown(&dst))?;
        }
        if dry {
            continue;
        }
        for mut entry in moved {
            index.remove(&entry.path);
            let rest = entry.path[src.len()..].to_vec();
            entry.path = dst.clone();
            entry.path.extend_from_slice(&rest);
            index.set(entry);
        }
        git.op(Op::Rename {
            from: join(&root, &src),
            to: join(&root, &dst),
        });
    }
    if !dry {
        git.set_index(index);
    }
    Ok(0)
}

/// git clean: the untracked files go, and with -d the directories nothing tracks.
pub fn clean(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut force, mut dry, mut dirs, mut quiet) = (false, false, false, false);
    let mut paths = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "--force" => force = true,
            "--dry-run" => dry = true,
            "--quiet" => quiet = true,
            "--" => {
                paths.extend(args[i + 1..].iter().cloned());
                break;
            }
            "-i" | "--interactive" => {
                return Err(Error::Lacks(
                    "'clean -i' asks what to remove".to_string(),
                    "terminal for git to ask on",
                ));
            }
            a if a.starts_with('-') && !a.starts_with("--") => {
                for c in a[1..].chars() {
                    match c {
                        'f' => force = true,
                        'n' => dry = true,
                        'd' => dirs = true,
                        'q' => quiet = true,
                        _ => return Err(option("clean", a)),
                    }
                }
            }
            a if a.starts_with('-') => return Err(option("clean", a)),
            _ => paths.push(arg.clone()),
        }
    }
    let root = git.work_tree()?.to_vec();
    if !force && !dry && git.repo.config.bool("clean.requireforce") != Some(false) {
        return Err(Error::Fatal(
            "clean.requireForce is true and -f not given: refusing to clean".to_string(),
        ));
    }
    let spec = Pathspec::new(&git.prefix, &named(git, &paths)?)
        .map_err(|p| Error::Fatal(format!("{p}: '{p}' is outside repository")))?;
    let work = Work::new(&git.repo, &root)?;
    let env = env_of(git);
    let others = untracked(&work, Untracked::Normal, &spec, &env)?;
    work.answered()?;
    let fs = git.repo.fs;
    let mut emptied: Vec<Vec<u8>> = Vec::new();
    for path in others {
        let shown = quote_path(&shown_plain(&path, &git.prefix));
        let shown = String::from_utf8_lossy(&shown).into_owned();
        if let Some(dir) = path.strip_suffix(b"/") {
            if !dirs {
                continue;
            }
            if fs
                .stat_opt(&join(&join(&root, dir), b".git"), false)?
                .is_some()
            {
                if !quiet {
                    writeln!(io.out, "Skipping repository {shown}")?;
                }
                continue;
            }
            // Every file under it, then the directories from the deepest up.
            let mut stack = vec![dir.to_vec()];
            while let Some(at) = stack.pop() {
                let full = join(&root, &at);
                for name in fs.list(&full)? {
                    let child = join(&at, &name);
                    match fs.stat_opt(&join(&root, &child), false)? {
                        Some(meta) if meta.kind == psh_engine::host::FileKind::Dir => {
                            stack.push(child);
                        }
                        Some(_) if !dry => git.op(Op::Remove(join(&root, &child))),
                        _ => {}
                    }
                }
                emptied.push(at);
            }
        } else if !dry {
            git.op(Op::Remove(join(&root, &path)));
        }
        if !quiet {
            writeln!(
                io.out,
                "{} {shown}",
                if dry { "Would remove" } else { "Removing" }
            )?;
        }
    }
    if !dry {
        emptied.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        for dir in emptied {
            git.op(Op::Rmdir(join(&root, &dir)));
        }
    }
    Ok(0)
}
