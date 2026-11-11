//! git status: the short and porcelain formats and the long one.

use std::collections::BTreeMap;
use std::io::Write;

use super::cmd::{quote_path, shorten};
use super::diff::{Change, detect_renames};
use super::rev;
use super::store::join;
use super::wild::Pathspec;
use super::worktree::{Untracked, Work, untracked};
use super::{Error, Git, Io, Res, option};

#[derive(Default)]
struct Row {
    index: char,
    work: char,
    from: Option<Vec<u8>>,
}

/// A path as status shows it from the working directory.
fn shown(path: &[u8], prefix: &[u8], relative: bool, quote_space: bool) -> Vec<u8> {
    let rel = if relative {
        let mut joined = shown_plain(path, prefix);
        if joined.is_empty() {
            joined = b"./".to_vec();
        }
        joined
    } else {
        path.to_vec()
    };
    let quoted = quote_path(&rel);
    if quote_space && quoted == rel && rel.contains(&b' ') {
        let mut out = vec![b'"'];
        out.extend_from_slice(&rel);
        out.push(b'"');
        return out;
    }
    quoted
}

/// A path from the working directory, unquoted.
pub fn shown_plain(path: &[u8], prefix: &[u8]) -> Vec<u8> {
    if prefix.is_empty() {
        return path.to_vec();
    }
    let dirs: Vec<&[u8]> = prefix
        .split(|&b| b == b'/')
        .filter(|p| !p.is_empty())
        .collect();
    let parts: Vec<&[u8]> = path.split(|&b| b == b'/').collect();
    let common = dirs
        .iter()
        .zip(&parts[..parts.len() - 1])
        .take_while(|(a, b)| a == b)
        .count();
    let mut out: Vec<&[u8]> = vec![b".."; dirs.len() - common];
    out.extend(&parts[common..]);
    out.join(&b'/')
}

pub fn in_progress(git: &Git<'_>) -> Res<()> {
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "rebase-merge",
        "rebase-apply",
    ] {
        if git
            .repo
            .fs
            .stat_opt(&join(&git.repo.git_dir, name.as_bytes()), true)?
            .is_some()
        {
            return Err(Error::Lacks(
                "a merge, rebase, cherry-pick, revert or bisect is in progress".to_string(),
                "git that describes one",
            ));
        }
    }
    Ok(())
}

/// How a branch stands to the one it follows (ahead and behind, or None when that one
/// is gone), as status and checkout say it.
fn tracking_text(up: &str, state: Option<(usize, usize)>) -> String {
    let commits = |n: usize| if n == 1 { "commit" } else { "commits" };
    match state {
        None => format!(
            "Your branch is based on '{up}', but the upstream is gone.\n  (use \"git branch --unset-upstream\" to fixup)\n"
        ),
        Some((0, 0)) => format!("Your branch is up to date with '{up}'.\n"),
        Some((ahead, 0)) => format!(
            "Your branch is ahead of '{up}' by {ahead} {}.\n  (use \"git push\" to publish your local commits)\n",
            commits(ahead)
        ),
        Some((0, behind)) => format!(
            "Your branch is behind '{up}' by {behind} {}, and can be fast-forwarded.\n  (use \"git pull\" to update your local branch)\n",
            commits(behind)
        ),
        Some((ahead, behind)) => format!(
            "Your branch and '{up}' have diverged,\nand have {ahead} and {behind} different commits each, respectively.\n  (use \"git pull\" if you want to integrate the remote branch with yours)\n"
        ),
    }
}

/// What checkout says of a branch at ours that follows the reference up.
pub fn tracking(git: &Git<'_>, up: &str, ours: super::store::Oid) -> Res<String> {
    let state = match git.repo.resolve_ref(up)? {
        Some(theirs) => Some(rev::ahead_behind(&git.repo, ours, theirs)?),
        None => None,
    };
    Ok(tracking_text(shorten(up), state))
}

pub fn status(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    report(git, io, args, None)
}

/// git status as it will read once a command's changes are carried out: index is
/// the one the command leaves, after says what its files will hold, and restored are
/// the untracked files it brings back.
pub fn after(
    git: &Git<'_>,
    io: &mut Io<'_>,
    mut work: Work<'_, '_>,
    index: &super::index::Index,
    after: std::collections::HashMap<Vec<u8>, Option<(u32, super::store::Oid)>>,
    restored: Vec<Vec<u8>>,
) -> Res<()> {
    work.index = index.clone();
    work.index.written = i64::MAX;
    work.after = after;
    // A directory nothing tracks is one line, as the walk would show it.
    let mut shown = Vec::new();
    for path in restored {
        let mut line = path.clone();
        for (at, _) in path.iter().enumerate().filter(|(_, b)| **b == b'/') {
            if !work.tracked_under(&path[..at]) {
                line = path[..=at].to_vec();
                break;
            }
        }
        shown.push(line);
    }
    report(git, io, &[], Some((work, shown)))?;
    Ok(())
}

fn report(
    git: &Git<'_>,
    io: &mut Io<'_>,
    args: &[String],
    prepared: Option<(Work<'_, '_>, Vec<Vec<u8>>)>,
) -> Res<i32> {
    let mut short = git.repo.config.bool("status.short") == Some(true);
    let mut branch = git.repo.config.bool("status.branch") == Some(true);
    let mut porcelain = false;
    let mut mode = match git.repo.config.get("status.showuntrackedfiles") {
        Some("no") => Untracked::No,
        Some("all") => Untracked::All,
        _ => Untracked::Normal,
    };
    let mut renames = git.repo.config.get("status.renames").is_none_or(|v| {
        !matches!(
            v.to_ascii_lowercase().as_str(),
            "false" | "no" | "off" | "0"
        )
    });
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-s" | "--short" => short = true,
            "-b" | "--branch" => branch = true,
            "--porcelain" | "--porcelain=v1" | "--porcelain=1" => porcelain = true,
            "--long" => short = false,
            "--no-short" => short = false,
            "--no-branch" => branch = false,
            "-u" | "--untracked-files" | "-uall" | "--untracked-files=all" => mode = Untracked::All,
            "-uno" | "--untracked-files=no" => mode = Untracked::No,
            "-unormal" | "--untracked-files=normal" => mode = Untracked::Normal,
            "--no-renames" => renames = false,
            "--renames" => renames = true,
            "--no-ahead-behind" | "--ahead-behind" | "--no-optional-locks" | "--no-column" => {}
            "--" => {
                paths.extend(args[i + 1..].iter().cloned());
                break;
            }
            _ if arg.starts_with('-') && !arg.starts_with("--") && arg.len() > 2 => {
                // Letters together: -sb, -sbu.
                for c in arg[1..].chars() {
                    match c {
                        's' => short = true,
                        'b' => branch = true,
                        'u' => mode = Untracked::All,
                        _ => return Err(option("status", arg)),
                    }
                }
            }
            _ if arg.starts_with('-') => return Err(option("status", arg)),
            _ => paths.push(arg.to_string()),
        }
        i += 1;
    }
    let root = git.work_tree()?.to_vec();
    in_progress(git)?;
    let spec = Pathspec::new(&git.prefix, &paths)
        .map_err(|p| Error::Fatal(format!("{p}: '{p}' is outside repository")))?;
    let refresh = prepared.is_none() && git.var("GIT_OPTIONAL_LOCKS").as_deref() != Some("0");
    let (mut work, restored) = match prepared {
        Some(prepared) => prepared,
        None => (Work::new(&git.repo, &root)?, Vec::new()),
    };
    work.quick = true;
    let head = git.repo.head()?;
    let head_tree = match &head {
        Some(oid) => Some(git.repo.commit(&rev::commit_of(&git.repo, *oid)?)?.tree),
        None => None,
    };

    let mut rows: BTreeMap<Vec<u8>, Row> = BTreeMap::new();
    let mut unmerged: BTreeMap<Vec<u8>, u8> = BTreeMap::new();
    for entry in &work.index.entries {
        if entry.stage != 0 && spec.matches(&entry.path) {
            *unmerged.entry(entry.path.clone()).or_default() |= 1 << (entry.stage - 1);
        }
    }
    if !unmerged.is_empty() {
        return Err(super::diff::unmerged());
    }
    let mut staged = work.staged(head_tree.as_ref(), &spec)?;
    if renames {
        staged = detect_renames(&git.repo, staged, 30000)?;
    }
    let env_home = |name: &[u8]| -> Option<Vec<u8>> {
        git.env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    // The walk for untracked files first: it passes the tracked ones too.
    let mut others = untracked(&work, mode, &spec, &env_home)?;
    if mode != Untracked::No && !restored.is_empty() {
        others.extend(restored);
        others.sort();
        others.dedup();
    }
    let unstaged = work.unstaged(&spec)?;
    work.answered()?;
    // What was read to be compared is recorded, as git status does where it may: the
    // next status finds those files by their size and time.
    let noted = work.noted.take();
    if refresh
        && !noted.is_empty()
        && git
            .repo
            .fs
            .stat_opt(&join(&git.repo.git_dir, b"index.lock"), false)?
            .is_none()
    {
        let mut index = work.index.clone();
        for (path, meta) in noted {
            if let Ok(at) = index.find(&path) {
                match meta {
                    Some(meta) => index.entries[at].record(&meta, None),
                    None => index.entries[at].size = 0,
                }
            }
        }
        git.set_index(index);
    }
    let staged_any = !staged.is_empty();
    let letter = |c: &Change| c.status();
    for change in &staged {
        let row = rows.entry(change.path.clone()).or_default();
        row.index = letter(change);
        row.from.clone_from(&change.from);
    }
    for change in &unstaged {
        let row = rows.entry(change.path.clone()).or_default();
        row.work = letter(change);
    }
    let head_ref = git.repo.head_ref()?;
    let upstream = head_ref
        .as_deref()
        .and_then(|b| rev::upstream(&git.repo, b));
    let tracking = match (&upstream, &head) {
        (Some(up), Some(ours)) => match git.repo.resolve_ref(up)? {
            Some(theirs) => Some(Some(rev::ahead_behind(&git.repo, *ours, theirs)?)),
            None => Some(None),
        },
        _ => None,
    };
    let relative = !porcelain && git.repo.config.bool("status.relativepaths") != Some(false);
    let prefix = git.prefix.clone();

    if short || porcelain {
        if branch {
            let mut line = "## ".to_string();
            match (&head_ref, &head) {
                (Some(name), None) => {
                    line.push_str(&format!("No commits yet on {}", shorten(name)));
                }
                (Some(name), Some(_)) => {
                    line.push_str(shorten(name));
                    if let Some(up) = &upstream {
                        line.push_str(&format!("...{}", shorten(up)));
                        match tracking {
                            Some(Some((0, 0))) | None => {}
                            Some(Some((ahead, 0))) => line.push_str(&format!(" [ahead {ahead}]")),
                            Some(Some((0, behind))) => {
                                line.push_str(&format!(" [behind {behind}]"))
                            }
                            Some(Some((ahead, behind))) => {
                                line.push_str(&format!(" [ahead {ahead}, behind {behind}]"));
                            }
                            Some(None) => line.push_str(" [gone]"),
                        }
                    }
                }
                (None, _) => line.push_str("HEAD (no branch)"),
            }
            writeln!(io.out, "{line}")?;
        }
        for (path, row) in &rows {
            let x = if row.index == '\0' { ' ' } else { row.index };
            let y = if row.work == '\0' { ' ' } else { row.work };
            write!(io.out, "{x}{y} ")?;
            if let Some(from) = &row.from {
                io.out.write_all(&shown(from, &prefix, relative, true))?;
                io.out.write_all(b" -> ")?;
            }
            io.out.write_all(&shown(path, &prefix, relative, true))?;
            io.out.write_all(b"\n")?;
        }
        for path in &others {
            io.out.write_all(b"?? ")?;
            io.out.write_all(&shown(path, &prefix, relative, true))?;
            io.out.write_all(b"\n")?;
        }
        return Ok(0);
    }

    match (&head_ref, &head) {
        (Some(name), _) => writeln!(io.out, "On branch {}", shorten(name))?,
        (None, Some(oid)) => {
            let text = super::cmd::detached(&git.repo, oid)?;
            match text.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
                Some(inner) if inner.starts_with("HEAD detached") => writeln!(io.out, "{inner}")?,
                _ => writeln!(io.out, "Not currently on any branch.")?,
            }
        }
        (None, None) => writeln!(io.out, "Not currently on any branch.")?,
    }
    if head.is_none() {
        writeln!(io.out, "\nNo commits yet\n")?;
    }
    if let (Some(up), Some(state)) = (&upstream, tracking) {
        io.out
            .write_all(tracking_text(shorten(up), state).as_bytes())?;
        writeln!(io.out)?;
    }
    let label = |status: char| match status {
        'A' => "new file:   ",
        'D' => "deleted:    ",
        'R' => "renamed:    ",
        'T' => "typechange: ",
        _ => "modified:   ",
    };
    if staged_any {
        writeln!(io.out, "Changes to be committed:")?;
        if head.is_some() {
            writeln!(
                io.out,
                "  (use \"git restore --staged <file>...\" to unstage)"
            )?;
        } else {
            writeln!(io.out, "  (use \"git rm --cached <file>...\" to unstage)")?;
        }
        for (path, row) in &rows {
            if row.index == '\0' {
                continue;
            }
            write!(io.out, "\t{}", label(row.index))?;
            if let Some(from) = &row.from {
                io.out.write_all(&shown(from, &prefix, relative, false))?;
                io.out.write_all(b" -> ")?;
            }
            io.out.write_all(&shown(path, &prefix, relative, false))?;
            io.out.write_all(b"\n")?;
        }
        writeln!(io.out)?;
    }
    let dirty = rows.values().any(|r| r.work != '\0');
    if dirty {
        writeln!(io.out, "Changes not staged for commit:")?;
        if rows.values().any(|r| r.work == 'D') {
            writeln!(
                io.out,
                "  (use \"git add/rm <file>...\" to update what will be committed)"
            )?;
        } else {
            writeln!(
                io.out,
                "  (use \"git add <file>...\" to update what will be committed)"
            )?;
        }
        writeln!(
            io.out,
            "  (use \"git restore <file>...\" to discard changes in working directory)"
        )?;
        for (path, row) in &rows {
            if row.work == '\0' {
                continue;
            }
            write!(io.out, "\t{}", label(row.work))?;
            io.out.write_all(&shown(path, &prefix, relative, false))?;
            io.out.write_all(b"\n")?;
        }
        writeln!(io.out)?;
    }
    if !others.is_empty() {
        writeln!(io.out, "Untracked files:")?;
        writeln!(
            io.out,
            "  (use \"git add <file>...\" to include in what will be committed)"
        )?;
        for path in &others {
            io.out.write_all(b"\t")?;
            io.out.write_all(&shown(path, &prefix, relative, false))?;
            io.out.write_all(b"\n")?;
        }
        writeln!(io.out)?;
    } else if mode == Untracked::No && staged_any {
        writeln!(
            io.out,
            "Untracked files not listed (use -u option to show untracked files)"
        )?;
    }
    if !staged_any {
        if dirty {
            writeln!(
                io.out,
                "no changes added to commit (use \"git add\" and/or \"git commit -a\")"
            )?;
        } else if !others.is_empty() {
            writeln!(
                io.out,
                "nothing added to commit but untracked files present (use \"git add\" to track)"
            )?;
        } else if head.is_none() {
            writeln!(
                io.out,
                "nothing to commit (create/copy files and use \"git add\" to track)"
            )?;
        } else if mode == Untracked::No {
            writeln!(io.out, "nothing to commit (use -u to show untracked files)")?;
        } else {
            writeln!(io.out, "nothing to commit, working tree clean")?;
        }
    }
    Ok(0)
}
