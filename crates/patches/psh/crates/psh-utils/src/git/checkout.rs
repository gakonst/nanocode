//! What goes to the worktree: checkout and switch between commits (git's two-way
//! merge of the old tree, the new one and the index), files checked out of the index or
//! a commit, and reset --hard.

use std::collections::BTreeSet;
use std::io::Write;

use super::cmd::{quote_path, shorten};
use super::diff::{File, flatten, trees};
use super::index::{Index, IndexEntry};
use super::rev;
use super::stage::{answered, edit, later, prime};
use super::store::{Oid, join};
use super::wild::Pathspec;
use super::worktree::{State, Work};
use super::write::{Edit, Mode, Op, edit_config, valid_ref};
use super::{Error, Git, Io, Res, option};

fn submodule() -> Error {
    Error::Lacks(
        "a submodule would be checked out".to_string(),
        "git that checks out submodules",
    )
}

/// The directories above the paths, deepest first.
fn above(paths: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut dirs = BTreeSet::new();
    for path in paths {
        let mut at = path.as_slice();
        while let Some(slash) = at.iter().rposition(|&b| b == b'/') {
            at = &at[..slash];
            dirs.insert(at.to_vec());
        }
    }
    let mut dirs: Vec<Vec<u8>> = dirs.into_iter().collect();
    dirs.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    dirs
}

/// What a checkout does to the worktree: the files that go, then the ones written.
#[derive(Default)]
pub struct Out {
    pub removed: Vec<Vec<u8>>,
    pub written: Vec<(Vec<u8>, u32, Oid)>,
}

impl Out {
    pub fn plan(self, git: &Git<'_>, root: &[u8]) -> Res<()> {
        for path in &self.removed {
            git.op(Op::Remove(join(root, path)));
        }
        for dir in above(&self.removed) {
            git.op(Op::Rmdir(join(root, &dir)));
        }
        for (path, mode, oid) in self.written {
            let full = join(root, &path);
            // What is there goes first: a link is never written through, and a file's
            // mode is the one it is made with.
            git.op(Op::Remove(full.clone()));
            let data = git.repo.blob(&oid)?.to_vec();
            match mode {
                0o160000 => return Err(submodule()),
                0o120000 => git.op(Op::Link {
                    path: full,
                    target: data,
                }),
                _ => git.op(Op::Write {
                    path: full,
                    data,
                    mode: if mode & 0o100 != 0 {
                        Mode::Exec
                    } else {
                        Mode::Plain
                    },
                }),
            }
        }
        Ok(())
    }

    /// A file is written that the index goes on holding another content for: its
    /// entry is marked as git marks one that changed without its time showing it.
    pub fn dirty(&self, index: &mut Index, path: &[u8]) {
        if let Ok(at) = index.find(path) {
            index.entries[at].size = 0;
        }
    }

    pub fn write(&mut self, index: &mut Index, path: Vec<u8>, mode: u32, oid: Oid) {
        let mut entry = IndexEntry::new(path.clone(), mode, oid);
        entry.fresh = true;
        index.set(entry);
        self.written.push((path, mode, oid));
    }
}

/// The index and the worktree as a tree has them (reset --hard, checkout -f): every
/// tracked file that differs is written, and those the tree does not have go.
pub fn hard(
    git: &Git<'_>,
    work: &Work<'_, '_>,
    mut index: Index,
    tree: Option<&Oid>,
) -> Res<Index> {
    work.smudges()?;
    let mut files = Vec::new();
    if let Some(tree) = tree {
        flatten(&git.repo, tree, b"", &Pathspec::default(), &mut files)?;
    }
    let mut out = Out {
        removed: index
            .entries
            .iter()
            .filter(|e| {
                files
                    .binary_search_by(|f| f.0.as_slice().cmp(&e.path))
                    .is_err()
            })
            .map(|e| e.path.clone())
            .collect(),
        written: Vec::new(),
    };
    for path in &out.removed {
        index.remove(path);
    }
    let mut blocked = false;
    for (path, mode, oid) in files {
        if mode == 0o160000 {
            if index.get(&path).is_none_or(|e| e.oid != oid) {
                return Err(submodule());
            }
            continue;
        }
        if let Some(entry) = index.get(&path)
            && entry.oid == oid
            && entry.mode == mode
        {
            match later(work.state(entry), &mut blocked)? {
                Some(State::Same) | None => continue,
                Some(_) => {}
            }
        }
        out.write(&mut index, path, mode, oid);
    }
    answered(blocked)?;
    out.plan(git, &work.root)?;
    index.tree = match tree {
        Some(tree) => Some(prime(&git.repo, tree, b"")?),
        None => None,
    };
    Ok(index)
}

/// Files of the worktree from the index, or from a tree. With a tree and
/// update_index its entries become the index's. Without overlay, what the pathspec
/// names in the index and the tree does not have is removed. The number of files
/// written.
pub fn paths(
    git: &Git<'_>,
    work: &Work<'_, '_>,
    index: &mut Index,
    tree: Option<&Oid>,
    from_tree: bool,
    spec: &Pathspec,
    update_index: bool,
) -> Res<usize> {
    paths_with(git, work, index, tree, from_tree, spec, update_index, false)
}

#[allow(clippy::too_many_arguments)]
fn paths_with(
    git: &Git<'_>,
    work: &Work<'_, '_>,
    index: &mut Index,
    tree: Option<&Oid>,
    from_tree: bool,
    spec: &Pathspec,
    update_index: bool,
    overlay: bool,
) -> Res<usize> {
    work.smudges()?;
    let mut out = Out::default();
    let mut blocked = false;
    if !from_tree {
        let entries: Vec<IndexEntry> = index
            .entries
            .iter()
            .filter(|e| spec.matches(&e.path) && !e.skip_worktree)
            .cloned()
            .collect();
        for entry in entries {
            if entry.mode == 0o160000 {
                continue;
            }
            // The entry as the index on disk has it says whether the file changed.
            let state = match work.index.get(&entry.path) {
                Some(old) if old.oid == entry.oid && old.mode == entry.mode => {
                    later(work.state(old), &mut blocked)?
                }
                _ => Some(State::Gone),
            };
            if matches!(state, Some(State::Same) | None) {
                continue;
            }
            out.write(index, entry.path, entry.mode, entry.oid);
        }
    } else {
        let mut files = Vec::new();
        if let Some(tree) = tree {
            flatten(&git.repo, tree, b"", spec, &mut files)?;
        }
        if !overlay {
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
                if update_index {
                    index.remove(&path);
                }
                if work.meta(&path)?.is_some() {
                    out.removed.push(path);
                }
            }
        }
        for (path, mode, oid) in files {
            if mode == 0o160000 {
                return Err(submodule());
            }
            let same = match later(work.file_raw(&path, mode), &mut blocked)? {
                Some(Some((file, _))) => file.oid == oid && file.mode == mode,
                Some(None) => false,
                None => continue,
            };
            if same {
                if update_index
                    && index
                        .get(&path)
                        .is_none_or(|e| e.oid != oid || e.mode != mode)
                {
                    let mut entry = IndexEntry::new(path, mode, oid);
                    entry.fresh = true;
                    index.set(entry);
                }
                continue;
            }
            if update_index {
                out.write(index, path, mode, oid);
            } else {
                out.dirty(index, &path);
                out.written.push((path, mode, oid));
            }
        }
    }
    answered(blocked)?;
    let count = out.written.len() + out.removed.len();
    out.plan(git, &work.root)?;
    Ok(count)
}

/// From one commit's tree to another's, as git read-tree -m -u does it: a path that
/// differs between the two is taken from the new one where the index and the worktree
/// have the old one, kept where the index already has the new one, and anything else
/// stops the checkout. The message when it stops.
fn two_way(
    git: &Git<'_>,
    work: &Work<'_, '_>,
    index: &mut Index,
    old: Option<&Oid>,
    new: Option<&Oid>,
    merge: bool,
) -> Res<Option<String>> {
    let mut changes = Vec::new();
    trees(&git.repo, old, new, b"", &Pathspec::default(), &mut changes)?;
    if changes.is_empty() {
        return Ok(None);
    }
    work.smudges()?;
    let mut out = Out::default();
    let (mut local, mut untracked) = (Vec::new(), Vec::new());
    let mut blocked = false;
    let same = |entry: &IndexEntry, file: &File| entry.oid == file.oid && entry.mode == file.mode;
    for change in changes {
        let path = change.path;
        if [&change.old, &change.new]
            .iter()
            .any(|f| f.as_ref().is_some_and(|f| f.mode == 0o160000))
        {
            return Err(submodule());
        }
        let entry = index.get(&path).cloned();
        // The worktree's file is the index's, or is gone.
        let mut clean = |entry: &IndexEntry| -> Res<bool> {
            Ok(!matches!(
                later(work.state(entry), &mut blocked)?,
                Some(State::Changed(_))
            ))
        };
        match (entry, change.old, change.new) {
            (None, None, Some(new)) => {
                // A file of this name that nothing tracks would be lost; a directory
                // is in the way unless it empties.
                match later(work.meta(&path), &mut blocked)? {
                    Some(Some(meta))
                        if meta.kind != psh_engine::host::FileKind::Dir
                            || !index.entries.iter().any(|e| {
                                e.path.starts_with(&path) && e.path.get(path.len()) == Some(&b'/')
                            }) =>
                    {
                        untracked.push(path);
                    }
                    _ => out.write(index, path, new.mode, new.oid),
                }
            }
            (None, Some(_), None) => {}
            (None, Some(_), Some(_)) => local.push(path),
            (Some(entry), None, Some(new)) => {
                if !same(&entry, &new) {
                    local.push(path);
                }
            }
            (Some(entry), Some(old), None) => {
                if same(&entry, &old) && clean(&entry)? {
                    index.remove(&path);
                    out.removed.push(path);
                } else {
                    local.push(path);
                }
            }
            (Some(entry), Some(old), Some(new)) => {
                if same(&entry, &new) {
                    continue;
                }
                if same(&entry, &old) && clean(&entry)? {
                    out.write(index, path, new.mode, new.oid);
                } else {
                    local.push(path);
                }
            }
            (_, None, None) => {}
        }
    }
    answered(blocked)?;
    if !local.is_empty() || !untracked.is_empty() {
        let mut text = String::new();
        let mut list = |head: &str, paths: &[Vec<u8>], tail: &str| {
            if paths.is_empty() {
                return;
            }
            text.push_str(head);
            for path in paths {
                text.push_str(&format!("\t{}\n", String::from_utf8_lossy(path)));
            }
            text.push_str(tail);
        };
        let (by, before) = if merge {
            ("merge", "merge")
        } else {
            ("checkout", "switch branches")
        };
        list(
            &format!(
                "error: Your local changes to the following files would be overwritten by {by}:\n"
            ),
            &local,
            &format!("Please commit your changes or stash them before you {before}.\n"),
        );
        list(
            &format!(
                "error: The following untracked working tree files would be overwritten by {by}:\n"
            ),
            &untracked,
            &format!("Please move or remove them before you {before}.\n"),
        );
        text.push_str("Aborting\n");
        return Ok(Some(text));
    }
    // A file that goes makes room for a directory of its name, and the other way.
    out.removed
        .retain(|gone| !out.written.iter().any(|(path, ..)| path == gone));
    out.plan(git, &work.root)?;
    Ok(None)
}

/// What differs from the commit now checked out, as git checkout lists it: the
/// status letter and the path of each file whose content in the worktree, or whose
/// place in the index, is not the commit's.
fn local_changes(
    io: &mut Io<'_>,
    mut work: Work<'_, '_>,
    index: &Index,
    tree: Option<&Oid>,
) -> Res<()> {
    let Some(tree) = tree else { return Ok(()) };
    // The files the plan writes are the commit's once it is carried out.
    let rewritten: BTreeSet<&[u8]> = index
        .entries
        .iter()
        .filter(|e| e.fresh)
        .map(|e| e.path.as_slice())
        .collect();
    work.index = index.clone();
    work.index.written = i64::MAX;
    work.quick = true;
    for change in work.against_tree(tree, &Pathspec::default())? {
        if rewritten.contains(change.path.as_slice()) {
            continue;
        }
        write!(io.out, "{}\t", change.status())?;
        io.out.write_all(&quote_path(&change.path))?;
        io.out.write_all(b"\n")?;
    }
    Ok(())
}

/// The branch HEAD was on before the last checkout, or the commit it was at.
fn previous(git: &Git<'_>) -> Res<String> {
    let log = git
        .repo
        .fs
        .read_opt(&join(&git.repo.git_dir, b"logs/HEAD"))?;
    let text = String::from_utf8_lossy(log.as_deref().unwrap_or_default()).into_owned();
    for line in text.lines().rev() {
        if let Some((_, msg)) = line.split_once('\t')
            && let Some(moved) = msg.strip_prefix("checkout: moving from ")
            && let Some((from, _)) = moved.rsplit_once(" to ")
        {
            return Ok(from.to_string());
        }
    }
    Err(Error::Fatal("invalid reference: @{-1}".to_string()))
}

const DETACHED_ADVICE: &str = "\nYou are in 'detached HEAD' state. You can look around, make experimental\nchanges and commit them, and you can discard any commits you make in this\nstate without impacting any branches by switching back to a branch.\n\nIf you want to create a new branch to retain commits you create, you may\ndo so (now or later) by using -c with the switch command. Example:\n\n  git switch -c <new-branch-name>\n\nOr undo this operation with:\n\n  git switch -\n\nTurn off this advice by setting config variable advice.detachedHead to false\n\n";

fn head_line(git: &Git<'_>, what: &str, oid: &Oid) -> Res<String> {
    let subject = super::pretty::subject(git.repo.commit(oid)?.message());
    Ok(format!(
        "{what} {} {}\n",
        git.repo.short(oid)?,
        String::from_utf8_lossy(&subject)
    ))
}

/// Where HEAD goes.
enum Target {
    /// A branch that is there (its full name) and its commit.
    Branch(String, Option<Oid>),
    Detach(Oid, String),
    /// A branch to make: its name, where it starts and what that was called, and
    /// whether one of that name may be there already.
    Create {
        name: String,
        start: Option<Oid>,
        from: String,
        reset: bool,
        track: Option<(String, String)>,
    },
}

/// The remote branch a full reference name is, as (remote, branch).
fn remote_branch(git: &Git<'_>, full: &str) -> Option<(String, String)> {
    let rest = full.strip_prefix("refs/remotes/")?;
    // The remote whose name is the longest start of it.
    let mut found: Option<(String, String)> = None;
    for (key, _) in &git.repo.config.entries {
        if let Some(name) = key
            .strip_prefix("remote.")
            .and_then(|k| k.strip_suffix(".url"))
            && let Some(branch) = rest.strip_prefix(name).and_then(|r| r.strip_prefix('/'))
            && found.as_ref().is_none_or(|(n, _)| n.len() < name.len())
        {
            found = Some((name.to_string(), branch.to_string()));
        }
    }
    found.or_else(|| {
        rest.split_once('/')
            .map(|(remote, branch)| (remote.to_string(), branch.to_string()))
    })
}

/// The configuration that makes a branch follow a remote one.
pub fn track(git: &Git<'_>, name: &str, remote: &str, branch: &str) -> Res<()> {
    let path = join(&git.repo.common_dir, b"config");
    let text = git.repo.fs.read_opt(&path)?.unwrap_or_default();
    let text = String::from_utf8_lossy(&text).into_owned();
    let section = format!("branch.{name}");
    let text = edit_config(&text, &format!("{section}.remote"), Edit::Set(remote)).unwrap_or(text);
    let merge = format!("refs/heads/{branch}");
    let text = edit_config(&text, &format!("{section}.merge"), Edit::Set(&merge)).unwrap_or(text);
    git.write(path, text.into_bytes());
    Ok(())
}

fn run(git: &Git<'_>, io: &mut Io<'_>, args: &[String], switch: bool) -> Res<i32> {
    let command = if switch { "switch" } else { "checkout" };
    let (mut quiet, mut force, mut detach) = (false, false, false);
    let mut create: Option<(String, bool)> = None;
    let mut tracking: Option<bool> = None;
    let mut words: Vec<String> = Vec::new();
    let mut files: Option<Vec<String>> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let mut named = |reset: bool| -> Res<()> {
            i += 1;
            let name = args.get(i).ok_or_else(|| {
                Error::Usage(format!("error: switch '{}' requires a value", &arg[1..]))
            })?;
            create = Some((name.clone(), reset));
            Ok(())
        };
        match arg {
            "-q" | "--quiet" | "--no-progress" | "--progress" => quiet |= arg.len() < 9,
            "-f" | "--force" => force = true,
            "--discard-changes" if switch => force = true,
            "-d" | "--detach" => detach = true,
            "-t" | "--track" => tracking = Some(true),
            "--no-track" => tracking = Some(false),
            "--no-guess" | "--guess" => {}
            "-b" if !switch => named(false)?,
            "-B" if !switch => named(true)?,
            "-c" | "--create" if switch => named(false)?,
            "-C" | "--force-create" if switch => named(true)?,
            "--" if !switch => {
                files = Some(args[i + 1..].to_vec());
                break;
            }
            "-" => words.push(arg.to_string()),
            "-p" | "--patch" => {
                return Err(Error::Lacks(
                    format!("'{command}' with '-p' asks what to check out"),
                    "terminal for git to ask on",
                ));
            }
            _ if arg.starts_with('-') => return Err(option(command, arg)),
            _ => words.push(arg.to_string()),
        }
        i += 1;
    }
    let repo = &git.repo;
    let root = git.work_tree()?.to_vec();
    let head = repo.head()?;
    let head_ref = repo.head_ref()?;
    let branch_of = |name: &str| -> Res<Option<(String, Oid)>> {
        let full = format!("refs/heads/{name}");
        Ok(repo.resolve_ref(&full)?.map(|oid| (full, oid)))
    };
    let commit = |spec: &str| -> Res<Option<Oid>> {
        match rev::resolve(repo, spec, &git.prefix)? {
            Some(oid) => Ok(Some(rev::commit_of(repo, oid)?)),
            None => Ok(None),
        }
    };
    // What the words mean: a branch, a commit, or files.
    let mut target = None;
    let mut source: Option<String> = None;
    if let Some((name, reset)) = create {
        if !valid_ref(&format!("refs/heads/{name}")) || name == "HEAD" {
            return Err(Error::Fatal(format!("'{name}' is not a valid branch name")));
        }
        if !reset && branch_of(&name)?.is_some() {
            return Err(Error::Fatal(format!(
                "a branch named '{name}' already exists"
            )));
        }
        let from = match words.as_slice() {
            [] => None,
            [start] => Some(start.clone()),
            _ => {
                return Err(Error::Fatal(format!(
                    "Cannot update paths and switch to branch '{name}' at the same time."
                )));
            }
        };
        let start = match &from {
            Some(spec) => Some(commit(spec)?.ok_or_else(|| {
                Error::Fatal(format!(
                    "'{spec}' is not a commit and a branch '{name}' cannot be created from it"
                ))
            })?),
            None => head,
        };
        let follows = match &from {
            Some(spec) if tracking != Some(false) => match rev::find_ref(repo, spec)? {
                Some((full, _)) if full.starts_with("refs/remotes/") => remote_branch(git, &full),
                Some((full, _)) if tracking == Some(true) && full.starts_with("refs/heads/") => {
                    Some((".".to_string(), shorten(&full).to_string()))
                }
                _ => None,
            },
            _ => None,
        };
        target = Some(Target::Create {
            name,
            start,
            from: from.unwrap_or_else(|| "HEAD".to_string()),
            reset,
            track: follows,
        });
    } else if files.is_none() || !words.is_empty() {
        let first = match words.first().map(String::as_str) {
            Some("-") => Some(previous(git)?),
            other => other.map(str::to_string),
        };
        match first {
            None => {
                target = Some(match (&head_ref, head) {
                    (Some(branch), _) => Target::Branch(branch.clone(), head),
                    (None, Some(head)) => Target::Detach(head, head.hex()),
                    (None, None) => Target::Branch("HEAD".to_string(), None),
                });
            }
            Some(first) => {
                let more = words.len() > 1 || files.as_ref().is_some_and(|f| !f.is_empty());
                if more {
                    if commit(&first)?.is_some()
                        || rev::resolve(repo, &first, &git.prefix)?.is_some()
                    {
                        source = Some(first);
                        let mut rest = words[1..].to_vec();
                        rest.extend(files.take().unwrap_or_default());
                        files = Some(rest);
                    } else if files.is_some() {
                        return Err(Error::Fatal(format!("invalid reference: {first}")));
                    } else {
                        files = Some(words.clone());
                    }
                } else if let (Some((full, oid)), false) = (branch_of(&first)?, detach) {
                    target = Some(Target::Branch(full, Some(oid)));
                } else if let Some(oid) = commit(&first)? {
                    if switch && !detach {
                        return Err(Error::Fatal(format!(
                            "a branch is expected, got commit '{first}'"
                        )));
                    }
                    target = Some(Target::Detach(oid, first));
                } else {
                    // A branch one remote has, and no other: git makes the local one.
                    let remotes: Vec<(String, Oid)> = repo
                        .refs("refs/remotes/")?
                        .into_iter()
                        .filter(|(full, _)| {
                            remote_branch(git, full).is_some_and(|(_, branch)| branch == first)
                        })
                        .collect();
                    match remotes.as_slice() {
                        [(full, oid)] if valid_ref(&first) => {
                            target = Some(Target::Create {
                                name: first.clone(),
                                start: Some(*oid),
                                from: shorten(full).to_string(),
                                reset: false,
                                track: remote_branch(git, full),
                            });
                        }
                        _ if switch => {
                            return Err(Error::Fatal(format!("invalid reference: {first}")));
                        }
                        _ => files = Some(words.clone()),
                    }
                }
            }
        }
    }
    git.lock_index()?;
    let work = Work::new(repo, &root)?;
    let mut index = edit(&work)?;
    let Some(target) = target else {
        // Files, from the index or from a commit.
        let names = super::stage::named(git, &files.unwrap_or_default())?;
        let spec = Pathspec::new(&git.prefix, &names)
            .map_err(|p| Error::Fatal(format!("{p}: '{p}' is outside repository")))?;
        let tree = match &source {
            Some(spec) => {
                let oid = rev::resolve(repo, spec, &git.prefix)?
                    .ok_or_else(|| Error::Fatal(format!("invalid reference: {spec}")))?;
                let (peeled, kind) = repo.peel(&oid)?;
                Some(if kind == super::store::Kind::Tree {
                    peeled
                } else {
                    repo.commit(&rev::commit_of(repo, oid)?)?.tree
                })
            }
            None => None,
        };
        for name in &names {
            let one =
                Pathspec::new(&git.prefix, std::slice::from_ref(name)).map_err(Error::Fatal)?;
            let mut known = index.entries.iter().any(|e| one.matches(&e.path));
            if !known && let Some(tree) = &tree {
                let mut found = Vec::new();
                flatten(repo, tree, b"", &one, &mut found)?;
                known = !found.is_empty();
            }
            if !known {
                return Err(Error::Says(
                    format!("error: pathspec '{name}' did not match any file(s) known to git\n"),
                    1,
                ));
            }
        }
        let count = paths_with(
            git,
            &work,
            &mut index,
            tree.as_ref(),
            tree.is_some(),
            &spec,
            true,
            true,
        )?;
        if !quiet {
            let from = match &source {
                Some(spec) => match rev::resolve(repo, spec, &git.prefix)? {
                    Some(oid) => repo.short(&oid)?,
                    None => spec.clone(),
                },
                None => "the index".to_string(),
            };
            io.warn(&format!(
                "Updated {count} path{} from {from}\n",
                if count == 1 { "" } else { "s" }
            ));
        }
        git.set_index(index);
        return Ok(0);
    };
    git.hooks(&["post-checkout"])?;
    let tree_of = |oid: Option<Oid>| -> Res<Option<Oid>> {
        match oid {
            Some(oid) => Ok(Some(repo.commit(&oid)?.tree)),
            None => Ok(None),
        }
    };
    let old_tree = tree_of(match head {
        Some(head) => Some(rev::commit_of(repo, head)?),
        None => None,
    })?;
    let new = match &target {
        Target::Branch(_, oid) => *oid,
        Target::Detach(oid, _) => Some(*oid),
        Target::Create { start, .. } => *start,
    };
    let new_tree = tree_of(new)?;
    if force {
        index = hard(git, &work, index, new_tree.as_ref())?;
    } else if let Some(refusal) = two_way(
        git,
        &work,
        &mut index,
        old_tree.as_ref(),
        new_tree.as_ref(),
        false,
    )? {
        return Err(Error::Says(refusal, 1));
    }
    let was = match &head_ref {
        Some(branch) => shorten(branch).to_string(),
        None => head.map_or_else(|| "HEAD".to_string(), |h| h.hex()),
    };
    let moved = head != new;
    let mut notes = String::new();
    if head_ref.is_none()
        && let Some(old) = head
        && moved
        && !quiet
    {
        notes.push_str(&head_line(git, "Previous HEAD position was", &old)?);
    }
    let mut upstream = None;
    let mut said = String::new();
    match &target {
        Target::Branch(full, _) => {
            let short = shorten(full).to_string();
            if head_ref.as_deref() == Some(full.as_str()) {
                notes.push_str(&format!("Already on '{short}'\n"));
            } else {
                git.point_head(
                    Some(full),
                    new,
                    &format!("checkout: moving from {was} to {short}"),
                )?;
                notes.push_str(&format!("Switched to branch '{short}'\n"));
            }
            upstream = rev::upstream(repo, full);
        }
        Target::Detach(oid, spec) => {
            if head_ref.is_some() || moved {
                git.point_head(
                    None,
                    Some(*oid),
                    &format!("checkout: moving from {was} to {spec}"),
                )?;
            }
            if head_ref.is_some()
                && !quiet
                && repo.config.bool("advice.detachedhead") != Some(false)
            {
                notes.push_str(&format!("Note: switching to '{spec}'.\n{DETACHED_ADVICE}"));
            }
            if !quiet {
                notes.push_str(&head_line(git, "HEAD is now at", oid)?);
            }
        }
        Target::Create {
            name,
            start,
            from,
            reset,
            track: follows,
        } => {
            let full = format!("refs/heads/{name}");
            let existed = branch_of(name)?.is_some();
            if let Some(start) = start {
                let message = if existed {
                    format!("branch: Reset to {from}")
                } else {
                    format!("branch: Created from {from}")
                };
                git.set_ref(&full, *start, &message)?;
            }
            if let Some((remote, branch)) = follows {
                track(git, name, remote, branch)?;
                let shown = if remote == "." {
                    branch.clone()
                } else {
                    format!("{remote}/{branch}")
                };
                said = format!("branch '{name}' set up to track '{shown}'.\n");
                upstream = Some(if remote == "." {
                    format!("refs/heads/{branch}")
                } else {
                    format!("refs/remotes/{remote}/{branch}")
                });
            }
            git.point_head(
                Some(&full),
                new,
                &format!("checkout: moving from {was} to {name}"),
            )?;
            notes.push_str(&if *reset && existed {
                format!("Switched to and reset branch '{name}'\n")
            } else {
                format!("Switched to a new branch '{name}'\n")
            });
        }
    }
    if !quiet {
        local_changes(io, work, &index, new_tree.as_ref())?;
        io.out.write_all(said.as_bytes())?;
        io.warn(&notes);
        if let (Some(up), Some(ours)) = (upstream, new) {
            io.out
                .write_all(super::status::tracking(git, &up, ours)?.as_bytes())?;
        }
    }
    git.set_index(index);
    Ok(0)
}

pub fn checkout(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    run(git, io, args, false)
}

pub fn switch(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    run(git, io, args, true)
}

/// git merge where it only moves the branch forward: what is merged has HEAD among
/// its ancestors, so the files go from one commit to the other as a checkout takes
/// them.
pub fn merge(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut only, mut quiet, mut stat) = (false, false, true);
    let mut specs = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--ff-only" => only = true,
            "--ff" | "--no-edit" | "--no-verify" => {}
            "-q" | "--quiet" => quiet = true,
            "--stat" => stat = true,
            "-n" | "--no-stat" => stat = false,
            a if a.starts_with('-') => {
                return Err(Error::Lacks(
                    format!("'merge' with '{a}'"),
                    "git that merges histories: it only moves a branch forward",
                ));
            }
            _ => specs.push(arg.clone()),
        }
    }
    let [spec] = specs.as_slice() else {
        return Err(Error::Lacks(
            "'merge' of none or several commits".to_string(),
            "git that merges histories: it only moves a branch forward",
        ));
    };
    let repo = &git.repo;
    let root = git.work_tree()?.to_vec();
    super::status::in_progress(git)?;
    let theirs = match rev::resolve(repo, spec, &git.prefix)? {
        Some(oid) => rev::commit_of(repo, oid)?,
        None => {
            return Err(Error::Says(
                format!("merge: {spec} - not something we can merge\n"),
                1,
            ));
        }
    };
    let Some(head) = repo.head()? else {
        return Err(Error::Lacks(
            "'merge' into a branch with no commits".to_string(),
            "git that merges histories: it only moves a branch forward",
        ));
    };
    let head = rev::commit_of(repo, head)?;
    if rev::is_ancestor(repo, theirs, head)? {
        if !quiet {
            writeln!(io.out, "Already up to date.")?;
        }
        return Ok(0);
    }
    if !rev::is_ancestor(repo, head, theirs)? {
        if only {
            return Err(Error::Says(
                "hint: Diverging branches can't be fast-forwarded, you need to either:\nhint:\nhint: \tgit merge --no-ff\nhint:\nhint: or:\nhint:\nhint: \tgit rebase\nhint:\nhint: Disable this message with \"git config set advice.diverging false\"\nfatal: Not possible to fast-forward, aborting.\n".to_string(),
                128,
            ));
        }
        return Err(Error::Lacks(
            format!("'merge' of '{spec}', whose history has diverged from HEAD's"),
            "git that merges histories: it only moves a branch forward",
        ));
    }
    git.lock_index()?;
    git.hooks(&["post-merge"])?;
    let (ours, new) = (repo.commit(&head)?.tree, repo.commit(&theirs)?.tree);
    let work = Work::new(repo, &root)?;
    let mut index = edit(&work)?;
    if !quiet {
        writeln!(
            io.out,
            "Updating {}..{}",
            repo.short(&head)?,
            repo.short(&theirs)?
        )?;
    }
    if let Some(refusal) = two_way(git, &work, &mut index, Some(&ours), Some(&new), true)? {
        return Err(Error::Says(refusal, 1));
    }
    git.write(
        join(&repo.git_dir, b"ORIG_HEAD"),
        format!("{}\n", head.hex()).into_bytes(),
    );
    git.move_head(theirs, &format!("merge {spec}: Fast-forward"))?;
    git.set_index(index);
    if quiet {
        return Ok(0);
    }
    writeln!(io.out, "Fast-forward")?;
    if stat {
        let mut changes = Vec::new();
        trees(
            repo,
            Some(&ours),
            Some(&new),
            b"",
            &Pathspec::default(),
            &mut changes,
        )?;
        let mut opts = super::diff::DiffOpts::new(repo, git.columns)?;
        if let Some(minimum) = opts.renames {
            changes = super::diff::detect_renames(repo, changes, minimum)?;
        }
        opts.stat = true;
        opts.summary = true;
        super::diff::Printer { repo, opts: &opts }.print(io.out, &changes)?;
    }
    Ok(0)
}

/// Every file under a directory, and the directory, out of the way: the files, then
/// the directories from the deepest up.
fn remove_tree(git: &Git<'_>, top: &[u8]) -> Res<()> {
    let fs = git.repo.fs;
    let mut dirs = Vec::new();
    let mut stack = vec![top.to_vec()];
    while let Some(at) = stack.pop() {
        for name in fs.list(&at)? {
            let child = join(&at, &name);
            match fs.stat_opt(&child, false)? {
                Some(meta) if meta.kind == psh_engine::host::FileKind::Dir => stack.push(child),
                Some(_) => git.op(Op::Remove(child)),
                None => {}
            }
        }
        dirs.push(at);
    }
    dirs.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    for dir in dirs {
        git.op(Op::Rmdir(dir));
    }
    Ok(())
}

/// A path an argument names, from the working directory.
fn named_path(git: &Git<'_>, arg: &str) -> Vec<u8> {
    if arg.starts_with('/') {
        super::repo::normalize(arg.as_bytes())
    } else {
        super::repo::normalize(&join(&git.cwd, arg.as_bytes()))
    }
}

/// The linked worktrees: each one's directory under worktrees/ and where it is.
fn linked(git: &Git<'_>) -> Res<Vec<(Vec<u8>, Vec<u8>)>> {
    let fs = git.repo.fs;
    let dir = join(&git.repo.common_dir, b"worktrees");
    let mut found = Vec::new();
    for name in fs.list(&dir)? {
        let own = join(&dir, &name);
        if let Some(text) = fs.read_opt(&join(&own, b"gitdir"))? {
            let text = String::from_utf8_lossy(&text).trim().to_string();
            let path = text
                .strip_suffix("/.git")
                .unwrap_or(&text)
                .as_bytes()
                .to_vec();
            found.push((own, path));
        }
    }
    Ok(found)
}

/// git worktree add: another working tree of the repository, on a branch of its own
/// or at a commit.
pub fn worktree_add(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut detach, mut quiet) = (false, false);
    let mut create: Option<(String, bool)> = None;
    let mut words: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-d" | "--detach" => detach = true,
            "-q" | "--quiet" => quiet = true,
            "-f" | "--force" | "--checkout" | "--no-track" | "--no-guess-remote" => {}
            "-b" | "-B" => {
                i += 1;
                let name = args.get(i).ok_or_else(|| {
                    Error::Usage(format!("error: switch '{}' requires a value", &arg[1..]))
                })?;
                create = Some((name.clone(), arg == "-B"));
            }
            _ if arg.starts_with('-') => return Err(option("worktree add", arg)),
            _ => words.push(arg),
        }
        i += 1;
    }
    let (place, spec) = match words.as_slice() {
        [place] => (*place, None),
        [place, spec] => (*place, Some(*spec)),
        _ => {
            return Err(Error::Usage(
                "usage: git worktree add [-f] [--detach] [--checkout] [--lock [--reason <string>]]\n                        [--orphan] [(-b | -B) <new-branch>] <path> [<commit-ish>]".to_string(),
            ));
        }
    };
    let repo = &git.repo;
    let fs = repo.fs;
    let full = named_path(git, place);
    if let Some(meta) = fs.stat_opt(&full, false)?
        && (meta.kind != psh_engine::host::FileKind::Dir || !fs.list(&full)?.is_empty())
    {
        return Err(Error::Fatal(format!("'{place}' already exists")));
    }
    let base =
        String::from_utf8_lossy(full.rsplit(|&b| b == b'/').next().unwrap_or(&full)).into_owned();
    let trees_dir = join(&repo.common_dir, b"worktrees");
    let mut name = base.clone();
    let mut n = 1;
    while fs
        .stat_opt(&join(&trees_dir, name.as_bytes()), false)?
        .is_some()
    {
        name = format!("{base}{n}");
        n += 1;
    }
    let commit = |spec: &str| -> Res<Oid> {
        match rev::resolve(repo, spec, &git.prefix)? {
            Some(oid) => rev::commit_of(repo, oid),
            None => Err(Error::Fatal(format!("invalid reference: {spec}"))),
        }
    };
    let existing = |short: &str| -> Res<Option<String>> {
        let full = format!("refs/heads/{short}");
        Ok(repo.resolve_ref(&full)?.map(|_| full))
    };
    // The branch it is on (and what a new one is made from), or none at a commit.
    let (branch, made, at) = match (&create, spec) {
        (Some((new, reset)), spec) => {
            if !valid_ref(&format!("refs/heads/{new}")) {
                return Err(Error::Fatal(format!("'{new}' is not a valid branch name")));
            }
            if existing(new)?.is_some() && !reset {
                return Err(Error::Fatal(format!(
                    "a branch named '{new}' already exists"
                )));
            }
            let from = spec.unwrap_or("HEAD");
            (
                Some(format!("refs/heads/{new}")),
                Some(from.to_string()),
                commit(from)?,
            )
        }
        (None, Some(spec)) => match existing(spec)? {
            Some(full) if !detach => (Some(full), None, commit(spec)?),
            _ => (None, None, commit(spec)?),
        },
        (None, None) if detach => (None, None, commit("HEAD")?),
        (None, None) => match existing(&base)? {
            Some(full) => (Some(full), None, commit(&base)?),
            None => {
                if !valid_ref(&format!("refs/heads/{base}")) {
                    return Err(Error::Fatal(format!("'{base}' is not a valid branch name")));
                }
                (
                    Some(format!("refs/heads/{base}")),
                    Some("HEAD".to_string()),
                    commit("HEAD")?,
                )
            }
        },
    };
    let note = match (&branch, &made) {
        (Some(full), Some(_)) => format!("new branch '{}'", shorten(full)),
        (Some(full), None) => format!("checking out '{}'", shorten(full)),
        (None, _) => format!("detached HEAD {}", repo.short(&at)?),
    };
    if !quiet {
        io.warn(&format!("Preparing worktree ({note})\n"));
    }
    if let (Some(full), None) = (&branch, &made)
        && let Some(tree) = super::commit::used_by(git, full)?
    {
        return Err(Error::Fatal(format!(
            "'{}' is already used by worktree at '{}'",
            shorten(full),
            String::from_utf8_lossy(&tree)
        )));
    }
    git.hooks(&["post-checkout"])?;
    if let Some(root) = &repo.work_tree {
        Work::new(repo, root)?.smudges()?;
    }
    let dir = join(&trees_dir, name.as_bytes());
    if let (Some(full), Some(from)) = (&branch, &made) {
        git.set_ref(full, at, &format!("branch: Created from {from}"))?;
    }
    // Where it is, as the file system names the directory above it.
    let above = full
        .iter()
        .rposition(|&b| b == b'/')
        .map_or(&full[..], |cut| &full[..cut.max(1)]);
    let real = join(&fs.physical(above), base.as_bytes());
    let head = match &branch {
        Some(full) => format!("ref: {full}\n"),
        None => format!("{}\n", at.hex()),
    };
    git.write(join(&dir, b"HEAD"), head.into_bytes());
    git.write(join(&dir, b"commondir"), b"../..\n".to_vec());
    let mut gitdir = join(&real, b".git");
    gitdir.push(b'\n');
    git.write(join(&dir, b"gitdir"), gitdir);
    git.write(
        join(&dir, b"ORIG_HEAD"),
        format!("{}\n", at.hex()).into_bytes(),
    );
    let who = git.ident(super::write::Who::Committer, false)?;
    let mut log = format!("{} {} {who}\n", Oid::ZERO.hex(), at.hex());
    if branch.is_some() {
        log.push_str(&format!("{0} {0} {who}\treset: moving to HEAD\n", at.hex()));
    }
    git.write(join(&dir, b"logs/HEAD"), log.into_bytes());
    let mut pointer = b"gitdir: ".to_vec();
    pointer.extend_from_slice(&join(
        &join(&fs.physical(&repo.common_dir), b"worktrees"),
        name.as_bytes(),
    ));
    pointer.push(b'\n');
    git.write(join(&full, b".git"), pointer);
    let tree = repo.commit(&at)?.tree;
    let mut files = Vec::new();
    flatten(repo, &tree, b"", &Pathspec::default(), &mut files)?;
    let mut index = Index::default();
    let mut out = Out::default();
    for (path, mode, oid) in files {
        if mode == 0o160000 {
            return Err(submodule());
        }
        out.write(&mut index, path, mode, oid);
    }
    index.tree = Some(prime(repo, &tree, b"")?);
    out.plan(git, &full)?;
    git.plan.borrow_mut().index = Some(super::write::IndexOut {
        path: join(&dir, b"index"),
        root: full,
        index,
    });
    if !quiet {
        io.out
            .write_all(head_line(git, "HEAD is now at", &at)?.as_bytes())?;
    }
    Ok(0)
}

/// git worktree remove and prune.
pub fn worktree_remove(git: &Git<'_>, io: &mut Io<'_>, args: &[String], prune: bool) -> Res<i32> {
    let fs = git.repo.fs;
    if prune {
        for (own, path) in linked(git)? {
            if fs.stat_opt(&path, true)?.is_none() {
                remove_tree(git, &own)?;
            }
        }
        return Ok(0);
    }
    let mut force = false;
    let mut places = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-f" | "--force" => force = true,
            a if a.starts_with('-') => return Err(option("worktree remove", a)),
            a => places.push(a),
        }
    }
    let [place] = places.as_slice() else {
        return Err(Error::Usage(
            "usage: git worktree remove [-f] <worktree>".to_string(),
        ));
    };
    let wanted = named_path(git, place);
    let real = fs.physical(&wanted);
    let Some((own, path)) = linked(git)?
        .into_iter()
        .find(|(_, path)| *path == wanted || *path == real)
    else {
        return Err(Error::Fatal(format!("'{place}' is not a working tree")));
    };
    if !force {
        // Its own index against its own files: nothing changed, nothing untracked.
        let env = |name: &[u8]| -> Option<Vec<u8>> {
            git.env
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        let theirs = super::repo::Repo::discover(fs, &path, &env, &[])?;
        let work = Work::new(&theirs, &path)?;
        let all = Pathspec::default();
        let others =
            super::worktree::untracked(&work, super::worktree::Untracked::Normal, &all, &env)?;
        let head_tree = match theirs.head()? {
            Some(head) => Some(theirs.commit(&rev::commit_of(&theirs, head)?)?.tree),
            None => None,
        };
        let dirty = !others.is_empty()
            || !work.unstaged(&all)?.is_empty()
            || !work.staged(head_tree.as_ref(), &all)?.is_empty();
        work.answered()?;
        if dirty {
            return Err(Error::Fatal(format!(
                "'{place}' contains modified or untracked files, use --force to delete it"
            )));
        }
    }
    let _ = io;
    remove_tree(git, &path)?;
    remove_tree(git, &own)?;
    Ok(0)
}
