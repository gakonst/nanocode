//! What makes history and names it: commit, and the branches and tags git branch and
//! git tag make, move and delete.

use std::io::Write;

use super::cmd::shorten;
use super::diff::{Change, trees};
use super::index::{CacheTree, IndexEntry};
use super::repo::{Ident, Repo};
use super::rev;
use super::stage::{edit, update_tracked};
use super::store::{Kind, Oid, join};
use super::wild::Pathspec;
use super::worktree::Work;
use super::write::{Edit, Op, Who, edit_config, parse_date, valid_ref};
use super::{Error, Git, Io, Res, option};

/// The tree objects of the index's entries (their paths share the first depth bytes),
/// as the record the index keeps of them. A directory the index last wrote as a tree,
/// and has not changed since, is not written again.
pub fn write_tree(
    repo: &Repo<'_>,
    entries: &[&IndexEntry],
    depth: usize,
    name: &[u8],
    cache: Option<&CacheTree>,
) -> Res<CacheTree> {
    if let Some(cache) = cache
        && cache.count >= 0
        && cache.count as usize == entries.len()
    {
        return Ok(cache.clone());
    }
    let mut data = Vec::new();
    let mut children = Vec::new();
    let mut j = 0;
    while j < entries.len() {
        let first = entries[j];
        let rest = &first.path[depth..];
        match rest.iter().position(|&b| b == b'/') {
            Some(slash) => {
                let prefix = &first.path[..depth + slash + 1];
                let len = entries[j..]
                    .iter()
                    .take_while(|e| e.path.starts_with(prefix))
                    .count();
                let known = cache.and_then(|c| c.children.iter().find(|c| c.name == rest[..slash]));
                let child = write_tree(
                    repo,
                    &entries[j..j + len],
                    depth + slash + 1,
                    &rest[..slash],
                    known,
                )?;
                data.extend_from_slice(b"40000 ");
                data.extend_from_slice(&rest[..slash]);
                data.push(0);
                data.extend_from_slice(&child.oid.0);
                children.push(child);
                j += len;
            }
            None => {
                data.extend_from_slice(format!("{:o} ", first.mode).as_bytes());
                data.extend_from_slice(rest);
                data.push(0);
                data.extend_from_slice(&first.oid.0);
                j += 1;
            }
        }
    }
    let oid = repo.odb.write(Kind::Tree, data)?;
    Ok(CacheTree {
        name: name.to_vec(),
        count: entries.len() as i64,
        oid,
        children,
    })
}

/// git's cleanup of a message that was not edited (--cleanup=whitespace): no blank
/// lines at its start or end, none in a row, no blanks at the ends of lines.
pub fn cleanup(message: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in message.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push('\n');
            blank = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A commit object.
pub fn commit_object(
    repo: &Repo<'_>,
    tree: &Oid,
    parents: &[Oid],
    author: &str,
    committer: &str,
    message: &str,
) -> Res<Oid> {
    let mut data = format!("tree {}\n", tree.hex());
    for parent in parents {
        data.push_str(&format!("parent {}\n", parent.hex()));
    }
    data.push_str(&format!(
        "author {author}\ncommitter {committer}\n\n{message}"
    ));
    Ok(repo.odb.write(Kind::Commit, data.into_bytes())?)
}

pub fn commit(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut all, mut amend, mut allow_empty, mut allow_empty_message) =
        (false, false, false, false);
    let (mut quiet, mut no_verify, mut signoff, mut no_edit, mut reset_author) =
        (false, false, false, false, false);
    let mut messages: Vec<String> = Vec::new();
    let mut author: Option<String> = None;
    let mut date: Option<String> = None;
    let mut file: Option<String> = None;
    let mut reuse: Option<String> = None;
    let mut fixup: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let value = |i: &mut usize| -> Res<String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| {
                Error::Usage(format!(
                    "error: switch '{}' requires a value",
                    arg.trim_start_matches('-')
                ))
            })
        };
        match arg {
            "-a" | "--all" => all = true,
            "--amend" => amend = true,
            "--allow-empty" => allow_empty = true,
            "--allow-empty-message" => allow_empty_message = true,
            "-q" | "--quiet" => quiet = true,
            "-n" | "--no-verify" => no_verify = true,
            "-s" | "--signoff" => signoff = true,
            "--no-edit" => no_edit = true,
            "--reset-author" => reset_author = true,
            "-v" | "--verbose" | "--no-gpg-sign" | "--no-status" | "--status"
            | "--no-post-rewrite" => {}
            "-m" | "--message" => messages.push(value(&mut i)?),
            "-F" | "--file" => file = Some(value(&mut i)?),
            "-C" | "--reuse-message" => reuse = Some(value(&mut i)?),
            "--author" => author = Some(value(&mut i)?),
            "--date" => date = Some(value(&mut i)?),
            "--fixup" => fixup = Some(value(&mut i)?),
            "--" => {
                if i + 1 < args.len() {
                    return Err(Error::Lacks(
                        "'commit' of the paths it names".to_string(),
                        "git that commits a part of the worktree past the index",
                    ));
                }
            }
            _ if arg.starts_with("--message=") => messages.push(arg[10..].to_string()),
            _ if arg.starts_with("--author=") => author = Some(arg[9..].to_string()),
            _ if arg.starts_with("--date=") => date = Some(arg[7..].to_string()),
            _ if arg.starts_with("--file=") => file = Some(arg[7..].to_string()),
            _ if arg.starts_with("--fixup=") => fixup = Some(arg[8..].to_string()),
            _ if arg.starts_with("--reuse-message=") => reuse = Some(arg[16..].to_string()),
            _ if arg.starts_with("--cleanup=")
                && matches!(&arg[10..], "whitespace" | "default") => {}
            _ if arg.starts_with("-S") || arg.starts_with("--gpg-sign") => {
                return Err(Error::Lacks(
                    "'commit' that is signed".to_string(),
                    "keys to sign with",
                ));
            }
            _ if arg.starts_with("--") => return Err(option("commit", arg)),
            _ if arg.starts_with('-') && arg.len() > 2 => {
                // Letters together: -am MESSAGE, -qam, -mMESSAGE.
                let letters = &arg[1..];
                for (at, c) in letters.char_indices() {
                    match c {
                        'a' => all = true,
                        'q' => quiet = true,
                        'n' => no_verify = true,
                        's' => signoff = true,
                        'v' => {}
                        'm' => {
                            let rest = &letters[at + 1..];
                            if rest.is_empty() {
                                messages.push(value(&mut i)?);
                            } else {
                                messages.push(rest.to_string());
                            }
                            break;
                        }
                        _ => return Err(option("commit", arg)),
                    }
                }
            }
            _ if arg.starts_with('-') => return Err(option("commit", arg)),
            _ => {
                return Err(Error::Lacks(
                    "'commit' of the paths it names".to_string(),
                    "git that commits a part of the worktree past the index",
                ));
            }
        }
        i += 1;
    }
    let repo = &git.repo;
    let root = git.work_tree()?.to_vec();
    super::status::in_progress(git)?;
    if repo.config.bool("commit.gpgsign") == Some(true) {
        return Err(Error::Lacks(
            "this repository signs its commits".to_string(),
            "keys to sign with",
        ));
    }
    git.lock_index()?;
    let mut hooks = vec!["prepare-commit-msg", "post-commit"];
    if !no_verify {
        hooks.extend(["pre-commit", "commit-msg"]);
    }
    git.hooks(&hooks)?;
    let work = Work::new(repo, &root)?;
    let mut index = edit(&work)?;
    if all {
        update_tracked(
            git,
            &work,
            &mut index,
            &Pathspec::default(),
            &mut |_, _| Ok(()),
            false,
        )?;
    }
    let head = match repo.head()? {
        Some(head) => Some(repo.commit(&rev::commit_of(repo, head)?)?),
        None => None,
    };
    if amend && head.is_none() {
        return Err(Error::Fatal("You have nothing to amend.".to_string()));
    }
    let parents: Vec<Oid> = match (&head, amend) {
        (Some(head), true) => head.parents.clone(),
        (Some(head), false) => vec![head.id],
        (None, _) => Vec::new(),
    };
    let entries: Vec<&IndexEntry> = index.entries.iter().filter(|e| !e.intent_to_add).collect();
    let written = write_tree(repo, &entries, 0, b"", index.tree.as_ref())?;
    let tree = written.oid;
    let parent_tree = match parents.first() {
        Some(parent) => Some(repo.commit(parent)?.tree),
        None => None,
    };
    if !allow_empty {
        let unchanged = match parent_tree {
            Some(parent) => parent == tree,
            None => entries.is_empty() && !amend,
        };
        if unchanged && amend {
            return Err(Error::Says(
                "You asked to amend the most recent commit, but doing so would make\nit empty. You can repeat your command with --allow-empty, or you can\nremove the commit entirely with \"git reset HEAD^\".\n".to_string(),
                1,
            ));
        }
        if unchanged {
            super::status::status(git, io, &[])?;
            return Ok(1);
        }
    }
    // The message: what was given, or the one of the commit amended or named.
    let reused = match &reuse {
        Some(spec) => {
            let oid = rev::resolve(repo, spec, &git.prefix)?
                .ok_or_else(|| Error::Fatal(format!("could not lookup commit '{spec}'")))?;
            Some(repo.commit(&rev::commit_of(repo, oid)?)?)
        }
        None => None,
    };
    let mut message = if !messages.is_empty() {
        messages.join("\n\n")
    } else if let Some(file) = &file {
        if file == "-" {
            return Err(Error::Lacks(
                "'commit -F -' reads its message from standard input".to_string(),
                "git that reads a message there",
            ));
        }
        let path = if file.starts_with('/') {
            file.as_bytes().to_vec()
        } else {
            join(&git.cwd, file.as_bytes())
        };
        let text = repo.fs.read_opt(&path)?.ok_or_else(|| {
            Error::Fatal(format!(
                "could not read log file '{file}': No such file or directory"
            ))
        })?;
        String::from_utf8_lossy(&text).into_owned()
    } else if let Some(spec) = &fixup {
        let oid = rev::resolve(repo, spec, &git.prefix)?
            .ok_or_else(|| Error::Fatal(format!("could not lookup commit '{spec}'")))?;
        let target = repo.commit(&rev::commit_of(repo, oid)?)?;
        format!(
            "fixup! {}",
            String::from_utf8_lossy(&super::pretty::subject(target.message()))
        )
    } else if let Some(commit) = &reused {
        String::from_utf8_lossy(commit.message()).into_owned()
    } else if let (true, true, Some(head)) = (amend, no_edit, &head) {
        String::from_utf8_lossy(head.message()).into_owned()
    } else {
        return Err(Error::Lacks(
            "'commit' without a message opens an editor".to_string(),
            "editor: give the message with -m",
        ));
    };
    if signoff {
        let line = format!("Signed-off-by: {}", git.person(Who::Committer, true)?);
        let text = message.trim_end().to_string();
        if !text.lines().any(|l| l == line) {
            let trailers = text.rsplit("\n\n").next().is_some_and(|last| {
                last.lines().all(|l| l.contains(": ")) && text.contains("\n\n")
            });
            message = format!("{text}{}{line}\n", if trailers { "\n" } else { "\n\n" });
        }
    }
    let message = cleanup(&message);
    if message.is_empty() && !allow_empty_message {
        return Err(Error::Says(
            "Aborting commit due to empty commit message.\n".to_string(),
            1,
        ));
    }
    // Who wrote it: the one of the commit amended or reused, unless told otherwise.
    let kept = match (&reused, amend, &head) {
        (Some(commit), _, _) => Some(commit.author.clone()),
        (None, true, Some(head)) => Some(head.author.clone()),
        _ => None,
    };
    let mut author_line = match kept {
        Some(line) if !reset_author => String::from_utf8_lossy(&line).into_owned(),
        _ => git.ident(Who::Author, true)?,
    };
    if let Some(who) = &author {
        let Some((name, rest)) = who.split_once('<') else {
            return Err(Error::Fatal(format!(
                "--author '{who}' is not 'Name <email>' and matches no existing author"
            )));
        };
        let email = rest.split('>').next().unwrap_or_default();
        let when: Vec<&str> = author_line.rsplitn(3, ' ').collect();
        author_line = format!("{} <{}> {} {}", name.trim(), email.trim(), when[1], when[0]);
    }
    if let Some(date) = &date {
        let who = author_line
            .rsplitn(3, ' ')
            .nth(2)
            .unwrap_or_default()
            .to_string();
        author_line = format!("{who} {}", parse_date(git, date)?);
    }
    let committer_line = git.ident(Who::Committer, true)?;
    let oid = commit_object(
        repo,
        &tree,
        &parents,
        &author_line,
        &committer_line,
        &message,
    )?;
    let first = message.lines().next().unwrap_or_default();
    let kind = match (amend, &head) {
        (true, _) => "commit (amend)",
        (false, None) => "commit (initial)",
        (false, Some(_)) => "commit",
    };
    git.move_head(oid, &format!("{kind}: {first}"))?;
    if index.entries.iter().all(|e| !e.intent_to_add) {
        index.tree = Some(written);
    }
    git.set_index(index);
    if quiet {
        return Ok(0);
    }
    let branch = match repo.head_ref()? {
        Some(name) => shorten(&name).to_string(),
        None => "detached HEAD".to_string(),
    };
    write!(
        io.out,
        "[{branch}{} {}] ",
        if parents.is_empty() {
            " (root-commit)"
        } else {
            ""
        },
        repo.short(&oid)?
    )?;
    io.out
        .write_all(&super::pretty::subject(message.as_bytes()))?;
    io.out.write_all(b"\n")?;
    let (a, c) = (
        Ident::parse(author_line.as_bytes()),
        Ident::parse(committer_line.as_bytes()),
    );
    if (a.name, a.email) != (c.name, c.email) {
        io.out.write_all(b" Author: ")?;
        io.out.write_all(a.name)?;
        io.out.write_all(b" <")?;
        io.out.write_all(a.email)?;
        io.out.write_all(b">\n")?;
    }
    if amend || reuse.is_some() || date.is_some() {
        writeln!(
            io.out,
            " Date: {}",
            super::pretty::date_default(a.time, a.zone)
        )?;
    }
    let mut changes: Vec<Change> = Vec::new();
    trees(
        repo,
        parent_tree.as_ref(),
        Some(&tree),
        b"",
        &Pathspec::default(),
        &mut changes,
    )?;
    super::diff::commit_summary(repo, io.out, changes)?;
    Ok(0)
}

fn message_of(git: &Git<'_>, messages: &[String], file: Option<&String>) -> Res<Option<String>> {
    if !messages.is_empty() {
        return Ok(Some(messages.join("\n\n")));
    }
    let Some(file) = file else { return Ok(None) };
    let path = if file.starts_with('/') {
        file.as_bytes().to_vec()
    } else {
        join(&git.cwd, file.as_bytes())
    };
    let text = git.repo.fs.read_opt(&path)?.ok_or_else(|| {
        Error::Fatal(format!(
            "could not open or read '{file}': No such file or directory"
        ))
    })?;
    Ok(Some(String::from_utf8_lossy(&text).into_owned()))
}

/// git tag that makes, replaces or deletes tags.
pub fn tag(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut annotate, mut force, mut delete) = (false, false, false);
    let mut messages = Vec::new();
    let mut file = None;
    let mut names = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-a" | "--annotate" => annotate = true,
            "-f" | "--force" => force = true,
            "-d" | "--delete" => delete = true,
            "-m" | "--message" | "-F" | "--file" => {
                i += 1;
                let value = args.get(i).cloned().ok_or_else(|| {
                    Error::Usage(format!("error: switch '{}' requires a value", &arg[1..]))
                })?;
                if arg == "-m" || arg == "--message" {
                    messages.push(value);
                } else {
                    file = Some(value);
                }
            }
            "-am" | "-fa" | "-af" => {
                annotate = true;
                force |= arg.contains('f');
                if arg.ends_with('m') {
                    i += 1;
                    messages.extend(args.get(i).cloned());
                }
            }
            _ if arg.starts_with("--message=") => messages.push(arg[10..].to_string()),
            _ if arg.starts_with("-m") && arg.len() > 2 => messages.push(arg[2..].to_string()),
            "-s" | "--sign" | "-u" | "--local-user" | "-v" | "--verify" => {
                return Err(Error::Lacks(
                    format!("'tag' with '{arg}'"),
                    "keys to sign or verify with",
                ));
            }
            _ if arg.starts_with('-') => return Err(option("tag", arg)),
            _ => names.push(arg.to_string()),
        }
        i += 1;
    }
    let repo = &git.repo;
    if delete {
        let mut status = 0;
        for name in &names {
            let full = format!("refs/tags/{name}");
            match repo.resolve_ref(&full)? {
                Some(oid) => {
                    git.delete_ref(&full)?;
                    writeln!(io.out, "Deleted tag '{name}' (was {})", repo.short(&oid)?)?;
                }
                None => {
                    io.warn(&format!("error: tag '{name}' not found.\n"));
                    status = 1;
                }
            }
        }
        return Ok(status);
    }
    let (name, spec) = match names.as_slice() {
        [name] => (name, "HEAD"),
        [name, spec] => (name, spec.as_str()),
        _ => return Err(Error::Fatal("too many arguments".to_string())),
    };
    let full = format!("refs/tags/{name}");
    if !valid_ref(&full) {
        return Err(Error::Fatal(format!("'{name}' is not a valid tag name.")));
    }
    let old = repo.resolve_ref(&full)?;
    if old.is_some() && !force {
        return Err(Error::Fatal(format!("tag '{name}' already exists")));
    }
    let target = rev::resolve(repo, spec, &git.prefix)?
        .ok_or_else(|| Error::Fatal(format!("Failed to resolve '{spec}' as a valid ref.")))?;
    let message = message_of(git, &messages, file.as_ref())?;
    let oid = match message {
        Some(message) => {
            let (kind, _) = repo.object(&target)?;
            let data = format!(
                "object {}\ntype {}\ntag {name}\ntagger {}\n\n{}",
                target.hex(),
                kind.name(),
                git.ident(Who::Committer, true)?,
                cleanup(&message)
            );
            repo.odb.write(Kind::Tag, data.into_bytes())?
        }
        None if annotate => {
            return Err(Error::Lacks(
                "'tag -a' without a message opens an editor".to_string(),
                "editor: give the message with -m",
            ));
        }
        None => target,
    };
    git.write(
        join(&repo.common_dir, full.as_bytes()),
        format!("{}\n", oid.hex()).into_bytes(),
    );
    if let Some(old) = old
        && old != oid
    {
        writeln!(io.out, "Updated tag '{name}' (was {})", repo.short(&old)?)?;
    }
    Ok(0)
}

/// The worktree a branch is checked out in, when one has it.
pub fn used_by(git: &Git<'_>, full: &str) -> Res<Option<Vec<u8>>> {
    let repo = &git.repo;
    let fs = repo.fs;
    let on = |head: &[u8]| -> Res<bool> {
        Ok(fs.read_opt(head)?.is_some_and(|text| {
            text.strip_prefix(b"ref: ")
                .is_some_and(|t| String::from_utf8_lossy(t).trim() == full)
        }))
    };
    if on(&join(&repo.common_dir, b"HEAD"))? {
        let main = repo
            .common_dir
            .strip_suffix(b"/.git")
            .unwrap_or(&repo.common_dir)
            .to_vec();
        return Ok(Some(fs.physical(&main)));
    }
    let dir = join(&repo.common_dir, b"worktrees");
    for name in fs.list(&dir)? {
        let own = join(&dir, &name);
        if on(&join(&own, b"HEAD"))?
            && let Some(text) = fs.read_opt(&join(&own, b"gitdir"))?
        {
            let path = String::from_utf8_lossy(&text).trim().to_string();
            let path = path.strip_suffix("/.git").unwrap_or(&path);
            return Ok(Some(path.as_bytes().to_vec()));
        }
    }
    Ok(None)
}

fn config_edit(git: &Git<'_>, key: &str, edit: Edit<'_>) -> Res<bool> {
    let path = join(&git.repo.common_dir, b"config");
    let text = git.repo.fs.read_opt(&path)?.unwrap_or_default();
    let text = String::from_utf8_lossy(&text).into_owned();
    match edit_config(&text, key, edit) {
        Some(text) => {
            git.write(path, text.into_bytes());
            Ok(true)
        }
        None => Ok(false),
    }
}

/// git branch that makes, moves or deletes branches, or says what one follows.
pub fn branch(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let (mut delete, mut force, mut rename, mut quiet, mut unset) =
        (false, false, false, false, false);
    let mut upstream: Option<String> = None;
    let mut tracking: Option<bool> = None;
    let mut names = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-d" | "--delete" => delete = true,
            "-D" => (delete, force) = (true, true),
            "-m" | "--move" => rename = true,
            "-M" => (rename, force) = (true, true),
            "-f" | "--force" => force = true,
            "-q" | "--quiet" => quiet = true,
            "-t" | "--track" => tracking = Some(true),
            "--no-track" => tracking = Some(false),
            "--unset-upstream" => unset = true,
            "-u" | "--set-upstream-to" => {
                i += 1;
                upstream = args.get(i).cloned();
            }
            _ if arg.starts_with("--set-upstream-to=") => upstream = Some(arg[18..].to_string()),
            _ if arg.starts_with("-u") && arg.len() > 2 => upstream = Some(arg[2..].to_string()),
            _ if arg.starts_with('-') => return Err(option("branch", arg)),
            _ => names.push(arg.to_string()),
        }
        i += 1;
    }
    let repo = &git.repo;
    let head_ref = repo.head_ref()?;
    let head = repo.head()?;
    let full_of = |name: &str| format!("refs/heads/{name}");
    let valid = |name: &str| -> Res<()> {
        if !valid_ref(&full_of(name)) || name == "HEAD" {
            return Err(Error::Fatal(format!("'{name}' is not a valid branch name")));
        }
        Ok(())
    };
    if delete {
        let mut status = 0;
        for name in &names {
            let full = full_of(name);
            let Some(oid) = repo.resolve_ref(&full)? else {
                io.warn(&format!("error: branch '{name}' not found\n"));
                status = 1;
                continue;
            };
            if let Some(tree) = used_by(git, &full)? {
                io.warn(&format!(
                    "error: cannot delete branch '{name}' used by worktree at '{}'\n",
                    String::from_utf8_lossy(&tree)
                ));
                status = 1;
                continue;
            }
            if !force {
                let against = match rev::upstream(repo, &full) {
                    Some(up) => repo.resolve_ref(&up)?.or(head),
                    None => head,
                };
                let merged = match against {
                    Some(tip) => rev::is_ancestor(repo, oid, rev::commit_of(repo, tip)?)?,
                    None => false,
                };
                if !merged {
                    io.warn(&format!(
                        "error: the branch '{name}' is not fully merged\nhint: If you are sure you want to delete it, run 'git branch -D {name}'\nhint: Disable this message with \"git config set advice.forceDeleteBranch false\"\n"
                    ));
                    status = 1;
                    continue;
                }
            }
            git.delete_ref(&full)?;
            config_edit(git, &format!("branch.{name}"), Edit::RemoveSection)?;
            if !quiet {
                writeln!(io.out, "Deleted branch {name} (was {}).", repo.short(&oid)?)?;
            }
        }
        return Ok(status);
    }
    if rename {
        let (old, new) = match names.as_slice() {
            [new] => match &head_ref {
                Some(current) => (shorten(current).to_string(), new.clone()),
                None => {
                    return Err(Error::Fatal(
                        "cannot rename the current branch while not on any".to_string(),
                    ));
                }
            },
            [old, new] => (old.clone(), new.clone()),
            _ => {
                return Err(Error::Fatal(
                    "too many arguments for a rename operation".to_string(),
                ));
            }
        };
        valid(&new)?;
        let (from, to) = (full_of(&old), full_of(&new));
        let unborn = head_ref.as_deref() == Some(from.as_str()) && head.is_none();
        let oid = repo.resolve_ref(&from)?;
        if oid.is_none() && !unborn {
            return Err(Error::Fatal(format!("no branch named '{old}'")));
        }
        if from != to && repo.resolve_ref(&to)?.is_some() && !force {
            return Err(Error::Fatal(format!(
                "a branch named '{new}' already exists"
            )));
        }
        let message = format!("Branch: renamed {from} to {to}");
        if let Some(oid) = oid {
            let logs = |name: &str| join(&join(&repo.common_dir, b"logs"), name.as_bytes());
            let had_log = repo.fs.stat_opt(&logs(&from), true)?.is_some();
            git.remove_loose(&to);
            if had_log {
                git.op(Op::Rename {
                    from: logs(&from),
                    to: logs(&to),
                });
            }
            git.op(Op::Remove(join(&repo.common_dir, from.as_bytes())));
            git.delete_packed(&[&from, &to])?;
            git.write(
                join(&repo.common_dir, to.as_bytes()),
                format!("{}\n", oid.hex()).into_bytes(),
            );
            git.log_ref(&to, Some(oid), Some(oid), &message)?;
        }
        if head_ref.as_deref() == Some(from.as_str()) {
            git.write(
                join(&repo.git_dir, b"HEAD"),
                format!("ref: {to}\n").into_bytes(),
            );
            if let Some(oid) = oid {
                git.log_ref("HEAD", Some(oid), None, &message)?;
                git.log_ref("HEAD", None, Some(oid), &message)?;
            }
        }
        config_edit(
            git,
            &format!("branch.{old}"),
            Edit::RenameSection(&format!("branch.{new}")),
        )?;
        return Ok(0);
    }
    if unset {
        let name = match names.first() {
            Some(name) => name.clone(),
            None => head_ref
                .as_deref()
                .map(shorten)
                .unwrap_or_default()
                .to_string(),
        };
        if rev::upstream(repo, &full_of(&name)).is_none() {
            return Err(Error::Fatal(format!(
                "branch '{name}' has no upstream information"
            )));
        }
        let path = join(&repo.common_dir, b"config");
        let text = repo.fs.read_opt(&path)?.unwrap_or_default();
        let mut text = String::from_utf8_lossy(&text).into_owned();
        for key in ["remote", "merge"] {
            if let Some(next) = edit_config(&text, &format!("branch.{name}.{key}"), Edit::Unset) {
                text = next;
            }
        }
        git.write(path, text.into_bytes());
        return Ok(0);
    }
    // The remote branch, or with --track the local one, that a new branch follows.
    let follows = |spec: &str, local: bool| -> Res<Option<(String, String)>> {
        Ok(match rev::find_ref(repo, spec)? {
            Some((full, _)) if full.starts_with("refs/remotes/") => full["refs/remotes/".len()..]
                .split_once('/')
                .map(|(remote, branch)| (remote.to_string(), branch.to_string())),
            Some((full, _)) if local && full.starts_with("refs/heads/") => {
                Some((".".to_string(), shorten(&full).to_string()))
            }
            _ => None,
        })
    };
    let set_up = |io: &mut Io<'_>, name: &str, remote: &str, branch: &str| -> Res<()> {
        super::checkout::track(git, name, remote, branch)?;
        let shown = if remote == "." {
            branch.to_string()
        } else {
            format!("{remote}/{branch}")
        };
        if !quiet {
            writeln!(io.out, "branch '{name}' set up to track '{shown}'.")?;
        }
        Ok(())
    };
    if let Some(up) = upstream {
        let name = match names.first() {
            Some(name) => name.clone(),
            None => match &head_ref {
                Some(current) => shorten(current).to_string(),
                None => {
                    return Err(Error::Fatal(
                        "could not set upstream of HEAD to it when it does not point to any branch"
                            .to_string(),
                    ));
                }
            },
        };
        if repo.resolve_ref(&full_of(&name))?.is_none() {
            return Err(Error::Fatal(format!("branch '{name}' does not exist")));
        }
        let Some((remote, branch)) = follows(&up, true)? else {
            return Err(Error::Fatal(format!(
                "the requested upstream branch '{up}' does not exist"
            )));
        };
        set_up(io, &name, &remote, &branch)?;
        return Ok(0);
    }
    let (name, start) = match names.as_slice() {
        [name] => (name, None),
        [name, start] => (name, Some(start.as_str())),
        _ => return Err(Error::Fatal("too many arguments".to_string())),
    };
    valid(name)?;
    let full = full_of(name);
    let existed = repo.resolve_ref(&full)?.is_some();
    if existed && !force {
        return Err(Error::Fatal(format!(
            "a branch named '{name}' already exists"
        )));
    }
    if existed && let Some(tree) = used_by(git, &full)? {
        return Err(Error::Fatal(format!(
            "cannot force update the branch '{name}' used by worktree at '{}'",
            String::from_utf8_lossy(&tree)
        )));
    }
    // Without a start git names the branch HEAD is on.
    let current = head_ref.as_deref().map(shorten);
    let spec = start.or(current).unwrap_or("HEAD");
    let oid = match rev::resolve(repo, spec, &git.prefix)? {
        Some(oid) => rev::commit_of(repo, oid)?,
        None => {
            return Err(Error::Fatal(format!("not a valid object name: '{spec}'")));
        }
    };
    let message = if existed {
        format!("branch: Reset to {spec}")
    } else {
        format!("branch: Created from {spec}")
    };
    git.set_ref(&full, oid, &message)?;
    if tracking != Some(false)
        && let Some((remote, branch)) = follows(spec, tracking == Some(true))?
    {
        set_up(io, name, &remote, &branch)?;
    }
    Ok(0)
}
