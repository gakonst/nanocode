//! git stash: what the worktree and the index hold put away as commits (push), and
//! brought back where that takes no merging of a file's contents (apply, pop), and
//! the list of them kept in the log of refs/stash.

use std::collections::HashMap;
use std::io::Write;

use super::checkout::{Out, hard};
use super::cmd::shorten;
use super::commit::{commit_object, write_tree};
use super::diff::{flatten, trees};
use super::index::{Index, IndexEntry};
use super::rev;
use super::stage::{answered, edit, later, update_tracked};
use super::store::{Kind, Oid, join};
use super::wild::Pathspec;
use super::worktree::{State, Untracked, Work, untracked};
use super::write::{Op, Who};
use super::{Error, Git, Io, Res, option};

struct Entry {
    old: Oid,
    new: Oid,
    /// The rest of the line: who, when and the message.
    rest: Vec<u8>,
}

fn log_path(git: &Git<'_>) -> Vec<u8> {
    join(&git.repo.common_dir, b"logs/refs/stash")
}

/// The stash entries, oldest first.
fn entries(git: &Git<'_>) -> Res<Vec<Entry>> {
    let text = git.repo.fs.read_opt(&log_path(git))?.unwrap_or_default();
    let mut found = Vec::new();
    for line in text.split(|&b| b == b'\n') {
        if let (Some(old), Some(new)) = (
            line.get(..40).and_then(Oid::from_hex),
            line.get(41..81).and_then(Oid::from_hex),
        ) {
            found.push(Entry {
                old,
                new,
                rest: line[81..].to_vec(),
            });
        }
    }
    Ok(found)
}

/// Which entry an argument names: stash@{N}, N, or the newest.
fn which(git: &Git<'_>, args: &[String], command: &str) -> Res<(Vec<Entry>, usize, bool)> {
    let mut quiet = false;
    let mut n = 0;
    for arg in args {
        match arg.as_str() {
            "-q" | "--quiet" => quiet = true,
            "--index" => return Err(option(command, arg)),
            a if a.starts_with('-') => return Err(option(command, a)),
            a => {
                let inner = a
                    .strip_prefix("stash@{")
                    .or_else(|| a.strip_prefix("refs/stash@{"))
                    .and_then(|r| r.strip_suffix('}'))
                    .unwrap_or(a);
                n = inner.parse().map_err(|_| {
                    Error::Says(format!("error: {a} is not a valid reference\n"), 1)
                })?;
            }
        }
    }
    let found = entries(git)?;
    if found.is_empty() {
        return Err(Error::Says("No stash entries found.\n".to_string(), 1));
    }
    if n >= found.len() {
        return Err(Error::Says(
            format!("error: stash@{{{n}}} is not a valid reference\n"),
            1,
        ));
    }
    Ok((found, n, quiet))
}

/// Takes the entry n (0 is the newest) out of the list.
fn drop_entry(git: &Git<'_>, mut list: Vec<Entry>, n: usize) -> Res<Oid> {
    let at = list.len() - 1 - n;
    let gone = list.remove(at);
    if let Some(next) = list.get_mut(at) {
        next.old = gone.old;
    }
    match list.last() {
        None => git.delete_ref("refs/stash")?,
        Some(top) => {
            let mut text = Vec::new();
            for entry in &list {
                text.extend_from_slice(
                    format!("{} {}", entry.old.hex(), entry.new.hex()).as_bytes(),
                );
                text.extend_from_slice(&entry.rest);
                text.push(b'\n');
            }
            git.write(log_path(git), text);
            git.write(
                join(&git.repo.common_dir, b"refs/stash"),
                format!("{}\n", top.new.hex()).into_bytes(),
            );
        }
    }
    Ok(gone.new)
}

fn env_of<'g>(git: &'g Git<'_>) -> impl Fn(&[u8]) -> Option<Vec<u8>> + 'g {
    |name: &[u8]| {
        git.env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }
}

fn push(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut quiet, mut others) = (false, false);
    let mut message: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-q" | "--quiet" => quiet = true,
            "-u" | "--include-untracked" => others = true,
            "--no-include-untracked" => others = false,
            "-m" | "--message" => {
                i += 1;
                message = args.get(i).cloned();
            }
            "-um" | "-qm" => {
                others |= arg == "-um";
                quiet |= arg == "-qm";
                i += 1;
                message = args.get(i).cloned();
            }
            "--" if i + 1 == args.len() => {}
            "-p" | "--patch" => {
                return Err(Error::Lacks(
                    "'stash' with '-p' asks what to stash".to_string(),
                    "terminal for git to ask on",
                ));
            }
            _ if arg.starts_with("--message=") => message = Some(arg[10..].to_string()),
            _ if arg.starts_with('-') => return Err(option("stash push", arg)),
            _ => {
                return Err(Error::Lacks(
                    "'stash push' of the paths it names".to_string(),
                    "git that stashes a part of the worktree",
                ));
            }
        }
        i += 1;
    }
    let repo = &git.repo;
    let root = git.work_tree()?.to_vec();
    super::status::in_progress(git)?;
    git.lock_index()?;
    let Some(head) = repo.head()? else {
        return Err(Error::Says(
            "You do not have the initial commit yet\n".to_string(),
            1,
        ));
    };
    let head = repo.commit(&rev::commit_of(repo, head)?)?;
    let work = Work::new(repo, &root)?;
    let index = edit(&work)?;
    let all = Pathspec::default();
    let env = env_of(git);
    let extra = if others {
        untracked(&work, Untracked::All, &all, &env)?
    } else {
        Vec::new()
    };
    work.answered()?;
    let staged: Vec<&IndexEntry> = index.entries.iter().filter(|e| !e.intent_to_add).collect();
    let index_tree = write_tree(repo, &staged, 0, b"", index.tree.as_ref())?.oid;
    let mut with_worktree = index.clone();
    update_tracked(
        git,
        &work,
        &mut with_worktree,
        &all,
        &mut |_, _| Ok(()),
        false,
    )?;
    let tracked: Vec<&IndexEntry> = with_worktree
        .entries
        .iter()
        .filter(|e| !e.intent_to_add)
        .collect();
    let work_tree = write_tree(repo, &tracked, 0, b"", with_worktree.tree.as_ref())?.oid;
    let extra: Vec<Vec<u8>> = extra.into_iter().filter(|p| !p.ends_with(b"/")).collect();
    if index_tree == head.tree && work_tree == head.tree && extra.is_empty() {
        if !quiet {
            writeln!(io.out, "No local changes to save")?;
        }
        return Ok(0);
    }
    let branch = match repo.head_ref()? {
        Some(name) => shorten(&name).to_string(),
        None => "(no branch)".to_string(),
    };
    let subject = super::pretty::subject(head.message());
    let at = format!(
        "{branch}: {} {}",
        repo.short(&head.id)?,
        String::from_utf8_lossy(&subject)
    );
    let author = git.ident(Who::Author, true)?;
    let committer = git.ident(Who::Committer, true)?;
    let made = |tree: &Oid, parents: &[Oid], message: &str| {
        commit_object(repo, tree, parents, &author, &committer, message)
    };
    let mut parents = vec![
        head.id,
        made(&index_tree, &[head.id], &format!("index on {at}\n"))?,
    ];
    if !extra.is_empty() {
        let mut held = Index::default();
        let mut blocked = false;
        for path in &extra {
            if let Some(Some((file, _))) = later(work.file(path, 0), &mut blocked)? {
                let data = file.data.as_ref().map(|d| d.to_vec()).unwrap_or_default();
                let oid = repo.odb.write(Kind::Blob, data)?;
                held.set(IndexEntry::new(path.clone(), file.mode, oid));
            }
        }
        answered(blocked)?;
        let held: Vec<&IndexEntry> = held.entries.iter().collect();
        let tree = write_tree(repo, &held, 0, b"", None)?.oid;
        parents.push(made(&tree, &[], &format!("untracked files on {at}\n"))?);
    }
    let title = match &message {
        Some(message) => format!("On {branch}: {message}"),
        None => format!("WIP on {at}"),
    };
    let stashed = made(&work_tree, &parents, &format!("{title}\n"))?;
    let old = repo.resolve_ref("refs/stash")?;
    git.write(
        join(&repo.common_dir, b"refs/stash"),
        format!("{}\n", stashed.hex()).into_bytes(),
    );
    git.op(Op::Append {
        path: log_path(git),
        data: format!(
            "{} {} {}\t{title}\n",
            old.unwrap_or(Oid::ZERO).hex(),
            stashed.hex(),
            committer
        )
        .into_bytes(),
    });
    let index = hard(git, &work, index, Some(&head.tree))?;
    let gone = Out {
        removed: extra,
        written: Vec::new(),
    };
    gone.plan(git, &root)?;
    git.set_index(index);
    if !quiet {
        writeln!(io.out, "Saved working directory and index state {title}")?;
    }
    Ok(0)
}

fn apply(git: &Git<'_>, io: &mut Io<'_>, args: &[String], pop: bool) -> Res<i32> {
    let (list, n, quiet) = which(git, args, if pop { "stash pop" } else { "stash apply" })?;
    let repo = &git.repo;
    let root = git.work_tree()?.to_vec();
    git.lock_index()?;
    let stashed = repo.commit(&list[list.len() - 1 - n].new)?;
    let Some(base) = stashed.parents.first() else {
        return Err(Error::Fatal(format!(
            "'{}' is not a stash-like commit",
            stashed.id.hex()
        )));
    };
    let base_tree = repo.commit(base)?.tree;
    let work = Work::new(repo, &root)?;
    let mut index = edit(&work)?;
    let all = Pathspec::default();
    let mut changes = Vec::new();
    trees(
        repo,
        Some(&base_tree),
        Some(&stashed.tree),
        b"",
        &all,
        &mut changes,
    )?;
    let mut out = Out::default();
    let mut after: HashMap<Vec<u8>, Option<(u32, Oid)>> = HashMap::new();
    let (mut local, mut in_the_way) = (Vec::new(), Vec::new());
    let mut blocked = false;
    for change in changes {
        let path = change.path;
        let ours = index.get(&path).map(|e| (e.mode, e.oid));
        let was = change.old.as_ref().map(|f| (f.mode, f.oid));
        let theirs = change.new.as_ref().map(|f| (f.mode, f.oid));
        if ours == theirs {
            continue;
        }
        if ours != was {
            return Err(Error::Lacks(
                format!(
                    "the stash and what is checked out both changed '{}'",
                    String::from_utf8_lossy(&path)
                ),
                "git that merges the contents of files",
            ));
        }
        match index.get(&path) {
            Some(entry) => {
                if let Some(State::Changed(_)) = later(work.state(entry), &mut blocked)? {
                    local.push(path.clone());
                }
            }
            None => {
                if later(work.meta(&path), &mut blocked)?.flatten().is_some() {
                    in_the_way.push(path.clone());
                }
            }
        }
        match theirs {
            Some((mode, oid)) => {
                after.insert(path.clone(), Some((mode, oid)));
                if was.is_none() {
                    out.write(&mut index, path, mode, oid);
                } else {
                    out.dirty(&mut index, &path);
                    out.written.push((path, mode, oid));
                }
            }
            None => {
                after.insert(path.clone(), None);
                out.removed.push(path);
            }
        }
    }
    let mut restored = Vec::new();
    if let Some(extra) = stashed.parents.get(2) {
        let mut files = Vec::new();
        flatten(repo, &repo.commit(extra)?.tree, b"", &all, &mut files)?;
        for (path, mode, oid) in files {
            if later(work.meta(&path), &mut blocked)?.flatten().is_some() {
                return Err(Error::Says(
                    format!(
                        "{} already exists, no checkout\nerror: could not restore untracked files from stash\n",
                        String::from_utf8_lossy(&path)
                    ),
                    1,
                ));
            }
            restored.push(path.clone());
            out.written.push((path, mode, oid));
        }
    }
    answered(blocked)?;
    if !local.is_empty() || !in_the_way.is_empty() {
        let mut text = String::new();
        let mut say = |head: &str, paths: &[Vec<u8>], tail: &str| {
            if paths.is_empty() {
                return;
            }
            text.push_str(head);
            for path in paths {
                text.push_str(&format!("\t{}\n", String::from_utf8_lossy(path)));
            }
            text.push_str(tail);
        };
        say(
            "error: Your local changes to the following files would be overwritten by merge:\n",
            &local,
            "Please commit your changes or stash them before you merge.\n",
        );
        say(
            "error: The following untracked working tree files would be overwritten by merge:\n",
            &in_the_way,
            "Please move or remove them before you merge.\n",
        );
        text.push_str("Aborting\n");
        if pop {
            text.push_str("The stash entry is kept in case you need it again.\n");
        }
        return Err(Error::Says(text, 1));
    }
    out.plan(git, &root)?;
    if !quiet {
        super::status::after(git, io, work, &index, after, restored)?;
    }
    if pop {
        let gone = drop_entry(git, list, n)?;
        if !quiet {
            writeln!(io.out, "Dropped refs/stash@{{{n}}} ({})", gone.hex())?;
        }
    }
    git.set_index(index);
    Ok(0)
}

pub fn stash(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    match args.first().map(String::as_str) {
        None => push(git, io, &[]),
        Some("push") => push(git, io, &args[1..]),
        Some("save") => {
            let (flags, words): (Vec<String>, Vec<String>) =
                args[1..].iter().cloned().partition(|a| a.starts_with('-'));
            let mut args = flags;
            if !words.is_empty() {
                args.push("-m".to_string());
                args.push(words.join(" "));
            }
            push(git, io, &args)
        }
        Some("pop") => apply(git, io, &args[1..], true),
        Some("apply") => apply(git, io, &args[1..], false),
        Some("drop") => {
            let (list, n, quiet) = which(git, &args[1..], "stash drop")?;
            let gone = drop_entry(git, list, n)?;
            if !quiet {
                writeln!(io.out, "Dropped refs/stash@{{{n}}} ({})", gone.hex())?;
            }
            Ok(0)
        }
        Some("clear") => {
            if git.repo.resolve_ref("refs/stash")?.is_some() {
                git.delete_ref("refs/stash")?;
            }
            Ok(0)
        }
        Some("list" | "show") => super::extra::stash(git, io, args),
        Some(first) if first.starts_with('-') => push(git, io, args),
        Some(other) => Err(Error::Lacks(
            format!("'stash {other}'"),
            "git with that stash command",
        )),
    }
}
