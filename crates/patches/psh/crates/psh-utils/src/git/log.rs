//! log, show, diff, rev-list and ls-files: the commands that walk history or compare
//! trees.

use std::io::Write;

use super::cmd::{quote_path, shorten};
use super::diff::{Change, DiffOpts, Printer, detect_renames, reversed, trees};
use super::pretty::{DateMode, Decorations, Format, Pretty, tag_header};
use super::repo::{Commit, Ident, Repo};
use super::rev::{self, Walk, Walker};
use super::store::{Kind, Oid, join};
use super::wild::Pathspec;
use super::worktree::{State, Untracked, Work, untracked};
use super::{Error, Git, Io, Res, option};
use crate::timefmt::days_from_civil;

/// A date as --since and --until take it: seconds since the epoch. offset is the
/// local zone's, for dates that name none.
fn approxidate(text: &str, now: i64, offset: i64) -> Option<i64> {
    let text = text.trim().to_ascii_lowercase();
    if text == "now" {
        return Some(now);
    }
    if text == "yesterday" {
        return Some(now - 86400);
    }
    if let Ok(secs) = text.parse::<i64>()
        && text.len() >= 9
    {
        return Some(secs);
    }
    let words: Vec<&str> = text
        .split([' ', '.', '_'])
        .filter(|w| !w.is_empty())
        .collect();
    if let [count, unit, "ago"] = words.as_slice()
        && let Ok(count) = count.parse::<i64>()
    {
        let unit = unit.trim_end_matches('s');
        let secs = match unit {
            "second" | "sec" => 1,
            "minute" | "min" => 60,
            "hour" => 3600,
            "day" => 86400,
            "week" => 7 * 86400,
            "month" => 30 * 86400,
            "year" => 365 * 86400,
            _ => return None,
        };
        return Some(now - count * secs);
    }
    // YYYY-MM-DD, with a time and a zone when they are there.
    let (date, rest) = match text.find(['t', ' ']) {
        Some(at) => (&text[..at], text[at + 1..].trim()),
        None => (text.as_str(), ""),
    };
    let mut ymd = date.split('-');
    let year: i64 = ymd.next()?.parse().ok()?;
    let month: u32 = ymd.next()?.parse().ok()?;
    let day: u32 = ymd.next()?.parse().ok()?;
    if ymd.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) || year < 1970
    {
        return None;
    }
    let mut secs = days_from_civil(year, month, day) * 86400;
    let mut zone = offset;
    let mut clock = rest;
    if let Some(at) = rest.find(['+', '-', 'z']) {
        let z = &rest[at..];
        clock = rest[..at].trim();
        zone = if z == "z" {
            0
        } else {
            let digits: String = z[1..].chars().filter(char::is_ascii_digit).collect();
            let n: i64 = digits.parse().ok()?;
            let secs = (n / 100) * 3600 + (n % 100) * 60;
            if z.starts_with('-') { -secs } else { secs }
        };
    }
    if clock.is_empty() {
        // A date alone means this time of day on it.
        secs += (now + offset).rem_euclid(86400);
    } else {
        let mut parts = clock.split(':');
        let h: i64 = parts.next()?.parse().ok()?;
        let m: i64 = parts.next().unwrap_or("0").parse().ok()?;
        let s: i64 = parts.next().unwrap_or("0").parse().ok()?;
        secs += h * 3600 + m * 60 + s;
    }
    Some(secs - zone)
}

/// git's complaint about a revision that is also a file, where no -- says which.
fn not_a_file(git: &Git<'_>, arg: &str) -> Res<()> {
    if git.repo.work_tree.is_none() {
        return Ok(());
    }
    if git
        .repo
        .fs
        .stat_opt(&join(&git.cwd, arg.as_bytes()), false)?
        .is_some()
    {
        return Err(Error::Fatal(format!(
            "ambiguous argument '{arg}': both revision and filename
             Use '--' to separate paths from revisions, like this:
             'git <command> [<revision>...] -- [<file>...]'"
        )));
    }
    Ok(())
}

/// The revisions, limits and paths of a log-like command line.
struct Revs {
    walk: Walk,
    max_count: Option<usize>,
    skip: usize,
    reverse: bool,
    authors: Vec<String>,
    greps: Vec<String>,
    ignore_case: bool,
    paths: Vec<String>,
    no_walk: bool,
    /// The objects named, in order, for show.
    named: Vec<(String, Oid)>,
    /// A...B was given: its two sides, for rev-list --left-right.
    symmetric: Option<(Oid, Oid)>,
}

impl Revs {
    fn new() -> Revs {
        Revs {
            walk: Walk::default(),
            max_count: None,
            skip: 0,
            reverse: false,
            authors: Vec::new(),
            greps: Vec::new(),
            ignore_case: false,
            paths: Vec::new(),
            no_walk: false,
            named: Vec::new(),
            symmetric: None,
        }
    }

    /// Takes args[*i] when it is a revision option; false when it is not.
    fn option(
        &mut self,
        git: &Git<'_>,
        command: &str,
        args: &[String],
        i: &mut usize,
    ) -> Res<bool> {
        let arg = args[*i].as_str();
        let number = |text: &str| -> Res<usize> {
            text.parse::<usize>()
                .map_err(|_| Error::Fatal(format!("'{text}': not an integer")))
        };
        let value = |i: &mut usize| -> Res<String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| {
                Error::Fatal(format!(
                    "option '{}' requires a value",
                    arg.trim_start_matches('-')
                ))
            })
        };
        let date = |text: &str| -> Res<i64> {
            approxidate(text, git.now, git.offset)
                .ok_or_else(|| option(command, &format!("the date '{text}'")))
        };
        let tips = |prefix: &str| -> Res<Vec<Oid>> {
            let mut oids = Vec::new();
            for (_, oid) in git.repo.refs(prefix)? {
                let (peeled, kind) = git.repo.peel(&oid)?;
                if kind == Kind::Commit {
                    oids.push(peeled);
                }
            }
            Ok(oids)
        };
        match arg {
            "-n" | "--max-count" => self.max_count = Some(number(&value(i)?)?),
            "--skip" => self.skip = number(&value(i)?)?,
            "--since" | "--after" => self.walk.since = Some(date(&value(i)?)?),
            "--until" | "--before" => self.walk.until = Some(date(&value(i)?)?),
            "--author" => self.authors.push(value(i)?),
            "--grep" => self.greps.push(value(i)?),
            "-i" | "--regexp-ignore-case" => self.ignore_case = true,
            "--first-parent" => self.walk.first_parent = true,
            "--no-merges" => self.walk.no_merges = true,
            "--merges" => self.walk.merges = true,
            "--reverse" => self.reverse = true,
            "--no-walk" => self.no_walk = true,
            "--all" => {
                self.walk.include.extend(tips("refs/")?);
                if let Some(head) = git.repo.head()? {
                    self.walk.include.push(head);
                }
            }
            "--branches" => self.walk.include.extend(tips("refs/heads/")?),
            "--tags" => self.walk.include.extend(tips("refs/tags/")?),
            "--remotes" => self.walk.include.extend(tips("refs/remotes/")?),
            "--date-order"
            | "--full-history"
            | "--no-show-signature"
            | "--no-notes"
            | "--no-graph"
            | "--no-follow"
            | "--end-of-options" => {}
            "--graph"
            | "--follow"
            | "--topo-order"
            | "--author-date-order"
            | "--ancestry-path"
            | "--left-right"
            | "--cherry-pick"
            | "--cherry-mark"
            | "--cherry"
            | "--simplify-merges"
            | "--walk-reflogs"
            | "-g"
            | "--show-signature"
            | "--source"
            | "--boundary"
            | "--not"
            | "--stdin"
            | "--objects"
            | "--parents"
            | "--children"
            | "--left-only"
            | "--right-only"
            | "--simplify-by-decoration"
            | "--show-pulls"
            | "--sparse"
            | "--dense"
            | "--bisect"
            | "--notes"
            | "--invert-grep"
            | "--all-match"
            | "-E"
            | "--extended-regexp"
            | "-F"
            | "--fixed-strings"
            | "-P"
            | "--perl-regexp"
            | "--committer"
            | "--glob"
            | "--exclude"
            | "--timestamp"
            | "--show-linear-break" => {
                return Err(option(command, arg));
            }
            _ => {
                if let Some(n) = arg.strip_prefix("--max-count=") {
                    self.max_count = Some(number(n)?);
                } else if let Some(n) = arg.strip_prefix("-n") {
                    self.max_count = Some(number(n)?);
                } else if arg.len() > 1
                    && arg[1..].bytes().all(|b| b.is_ascii_digit())
                    && arg.starts_with('-')
                {
                    self.max_count = Some(number(&arg[1..])?);
                } else if let Some(n) = arg.strip_prefix("--skip=") {
                    self.skip = number(n)?;
                } else if let Some(d) = arg
                    .strip_prefix("--since=")
                    .or(arg.strip_prefix("--after="))
                {
                    self.walk.since = Some(date(d)?);
                } else if let Some(d) = arg
                    .strip_prefix("--until=")
                    .or(arg.strip_prefix("--before="))
                {
                    self.walk.until = Some(date(d)?);
                } else if let Some(a) = arg.strip_prefix("--author=") {
                    self.authors.push(a.to_string());
                } else if let Some(g) = arg.strip_prefix("--grep=") {
                    self.greps.push(g.to_string());
                } else if arg.starts_with("--committer=")
                    || arg.starts_with("--glob=")
                    || arg.starts_with("--exclude=")
                    || arg.starts_with("--branches=")
                    || arg.starts_with("--tags=")
                    || arg.starts_with("--remotes=")
                    || arg.starts_with("--min-parents")
                    || arg.starts_with("--max-parents")
                    || arg.starts_with("--min-age")
                    || arg.starts_with("--max-age")
                    || arg.starts_with("--notes=")
                    || arg.starts_with("--ancestry-path=")
                    || arg.starts_with("-L")
                    || arg.starts_with("--diff-merges")
                    || arg.starts_with("--no-walk=")
                {
                    return Err(option(command, arg));
                } else {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Takes a positional argument as a revision; false when it names none.
    fn revision(&mut self, git: &Git<'_>, arg: &str) -> Res<bool> {
        let repo = &git.repo;
        let commit = |spec: &str| -> Res<Option<Oid>> {
            let spec = if spec.is_empty() { "HEAD" } else { spec };
            match rev::resolve(repo, spec, &git.prefix)? {
                Some(oid) => Ok(Some(rev::commit_of(repo, oid)?)),
                None => Ok(None),
            }
        };
        if let Some(at) = arg.find("...") {
            let (Some(a), Some(b)) = (commit(&arg[..at])?, commit(&arg[at + 3..])?) else {
                return Ok(false);
            };
            self.walk.include.push(a);
            self.walk.include.push(b);
            self.walk.exclude.extend(rev::merge_bases(repo, a, &[b])?);
            self.symmetric = Some((a, b));
            return Ok(true);
        }
        if let Some(at) = arg.find("..") {
            let (Some(a), Some(b)) = (commit(&arg[..at])?, commit(&arg[at + 2..])?) else {
                return Ok(false);
            };
            self.walk.exclude.push(a);
            self.walk.include.push(b);
            return Ok(true);
        }
        if let Some(rest) = arg.strip_prefix('^') {
            let Some(oid) = commit(rest)? else {
                return Ok(false);
            };
            self.walk.exclude.push(oid);
            return Ok(true);
        }
        let Some(oid) = rev::resolve(repo, arg, &git.prefix)? else {
            return Ok(false);
        };
        self.named.push((arg.to_string(), oid));
        Ok(true)
    }

    /// The rest of a command line: revisions, then paths.
    fn positional(&mut self, git: &Git<'_>, args: &[String]) -> Res<()> {
        let dashes = args.iter().any(|a| a == "--");
        let mut i = 0;
        while i < args.len() {
            let arg = args[i].as_str();
            if arg == "--" {
                self.paths.extend(args[i + 1..].iter().cloned());
                return Ok(());
            }
            if self.revision(git, arg)? {
                if !dashes {
                    not_a_file(git, arg)?;
                }
            } else {
                // The first name that is no revision starts the paths, if it is a file.
                let path = join(&git.cwd, arg.as_bytes());
                let known = git.repo.fs.stat_opt(&path, false)?.is_some();
                if !known {
                    return Err(rev::unknown(arg));
                }
                self.paths.extend(args[i..].iter().cloned());
                return Ok(());
            }
            i += 1;
        }
        Ok(())
    }

    fn pathspec(&self, git: &Git<'_>) -> Res<Pathspec> {
        Pathspec::new(&git.prefix, &self.paths)
            .map_err(|p| Error::Fatal(format!("{p}: '{p}' is outside repository")))
    }

    fn matches(&self, commit: &Commit) -> Res<bool> {
        if !self.authors.is_empty() {
            let who = Ident::parse(&commit.author);
            let mut line = who.name.to_vec();
            line.extend_from_slice(b" <");
            line.extend_from_slice(who.email);
            line.push(b'>');
            let mut any = false;
            for source in &self.authors {
                any |=
                    super::extra::pattern(source, false, false, self.ignore_case)?.is_match(&line);
            }
            if !any {
                return Ok(false);
            }
        }
        if !self.greps.is_empty() {
            let mut any = false;
            for source in &self.greps {
                let pattern = super::extra::pattern(source, false, false, self.ignore_case)?;
                any |= commit
                    .message()
                    .split(|&b| b == b'\n')
                    .any(|line| pattern.is_match(line));
            }
            if !any {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The commits of the walk, as the limits leave them.
    fn commits(&mut self, git: &Git<'_>, each: &mut dyn FnMut(&Commit) -> Res<bool>) -> Res<()> {
        let spec = self.pathspec(git)?;
        let mut walk = std::mem::take(&mut self.walk);
        walk.paths = match spec.literal() {
            Some(paths) => paths,
            None => return Err(option("log", "a path pattern")),
        };
        for (_, oid) in &self.named {
            walk.include.push(rev::commit_of(&git.repo, *oid)?);
        }
        if walk.include.is_empty() && self.named.is_empty() {
            match git.repo.head()? {
                Some(head) => walk.include.push(head),
                None => {
                    let branch = git.repo.head_ref()?.unwrap_or_default();
                    return Err(Error::Fatal(format!(
                        "your current branch '{}' does not have any commits yet",
                        shorten(&branch)
                    )));
                }
            }
        }
        let mut walker = Walker::new(&git.repo, walk)?;
        let mut skip = self.skip;
        let mut left = self.max_count;
        let mut held = Vec::new();
        while left != Some(0) {
            let Some(commit) = walker.next()? else { break };
            if !self.matches(&commit)? {
                continue;
            }
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if let Some(n) = &mut left {
                *n -= 1;
            }
            if self.reverse {
                held.push(commit);
            } else if !each(&commit)? {
                return Ok(());
            }
        }
        for commit in held.iter().rev() {
            if !each(commit)? {
                break;
            }
        }
        Ok(())
    }
}

/// The options of log and show that say how a commit is printed.
struct Shape {
    format: Format,
    date: DateMode,
    abbrev_commit: bool,
    decorate: bool,
    abbrev: Option<usize>,
}

impl Shape {
    fn new(repo: &Repo<'_>) -> Res<Shape> {
        let format = match repo.config.get("format.pretty") {
            Some(format) => Format::parse(format)?,
            None => Format::Medium,
        };
        let date = match repo.config.get("log.date") {
            Some(mode) => DateMode::parse(mode)?,
            None => DateMode::Default,
        };
        Ok(Shape {
            format,
            date,
            abbrev_commit: repo.config.bool("log.abbrevcommit") == Some(true),
            decorate: matches!(
                repo.config.get("log.decorate"),
                Some("short" | "full" | "true")
            ),
            abbrev: None,
        })
    }

    fn option(&mut self, command: &str, args: &[String], i: &mut usize) -> Res<bool> {
        let arg = args[*i].as_str();
        match arg {
            "--oneline" => {
                self.format = Format::Oneline;
                self.abbrev_commit = true;
            }
            "--abbrev-commit" => self.abbrev_commit = true,
            "--no-abbrev-commit" => self.abbrev_commit = false,
            "--decorate" | "--decorate=short" => self.decorate = true,
            "--no-decorate" | "--decorate=no" => self.decorate = false,
            "--decorate=auto" => {}
            "--relative-date" => self.date = DateMode::Relative,
            "--pretty" => self.format = Format::Medium,
            "--format" | "--date" => {
                *i += 1;
                let value = args.get(*i).ok_or_else(|| option(command, arg))?;
                if arg == "--date" {
                    self.date = DateMode::parse(value)?;
                } else {
                    self.format = Format::parse(value)?;
                    if let Format::User(format, _) = &self.format {
                        self.format = Format::User(format.clone(), true);
                    }
                }
            }
            _ => {
                if let Some(format) = arg.strip_prefix("--pretty=") {
                    self.format = Format::parse(format)?;
                } else if let Some(format) = arg.strip_prefix("--format=") {
                    self.format = Format::parse(format)?;
                } else if let Some(mode) = arg.strip_prefix("--date=") {
                    self.date = DateMode::parse(mode)?;
                } else if let Some(n) = arg.strip_prefix("--abbrev=") {
                    self.abbrev = Some(
                        n.parse::<usize>()
                            .map_err(|_| option(command, arg))?
                            .clamp(4, 40),
                    );
                } else if arg.starts_with("--decorate") {
                    return Err(option(command, arg));
                } else {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn pretty<'r, 'a>(&self, git: &'r Git<'a>) -> Res<Pretty<'r, 'a>> {
        let wants = match &self.format {
            Format::User(format, _) => format.contains("%d") || format.contains("%D"),
            _ => self.decorate,
        };
        Ok(Pretty {
            repo: &git.repo,
            format: self.format.clone(),
            date: self.date.clone(),
            abbrev_commit: self.abbrev_commit,
            decorate: if wants {
                Some(Decorations::load(&git.repo)?)
            } else {
                None
            },
            now: git.now,
            abbrev: self.abbrev,
        })
    }

    /// A format that ends each commit with a newline, rather than separating commits.
    fn terminates(&self) -> bool {
        matches!(
            self.format,
            Format::Oneline | Format::Reference | Format::User(_, true)
        )
    }
}

/// The changes a commit made to its first parent (or to nothing, for a root).
fn commit_changes(
    repo: &Repo<'_>,
    commit: &Commit,
    spec: &Pathspec,
    opts: &DiffOpts,
) -> Res<Vec<Change>> {
    let parent = match commit.parents.first() {
        Some(parent) => Some(repo.commit(parent)?.tree),
        None => None,
    };
    let mut changes = Vec::new();
    trees(
        repo,
        parent.as_ref(),
        Some(&commit.tree),
        b"",
        spec,
        &mut changes,
    )?;
    if let Some(minimum) = opts.renames {
        changes = detect_renames(repo, changes, minimum)?;
    }
    Ok(changes)
}

/// Prints one commit of log or show: its text, then what it changed.
#[allow(clippy::too_many_arguments)]
fn print_commit(
    out: &mut dyn Write,
    pretty: &Pretty<'_, '_>,
    shape: &Shape,
    opts: &DiffOpts,
    spec: &Pathspec,
    commit: &Commit,
    first: &mut bool,
    merges: Merges,
) -> Res<()> {
    let repo = pretty.repo;
    let merge = commit.parents.len() > 1;
    // What follows the commit's text, and whether a line separates the two even when
    // nothing follows.
    let mut body = Vec::new();
    let mut separate = false;
    if opts.any() && merge && matches!(merges, Merges::Combined) {
        // git show prints a merge as a combined diff: the paths that differ from
        // every parent. A merge that took each file from one side has none. Its
        // --stat is against the first parent.
        let mut common: Option<Vec<Vec<u8>>> = None;
        for parent in &commit.parents {
            let tree = repo.commit(parent)?.tree;
            let mut changes = Vec::new();
            trees(
                repo,
                Some(&tree),
                Some(&commit.tree),
                b"",
                spec,
                &mut changes,
            )?;
            let paths: Vec<Vec<u8>> = changes.into_iter().map(|c| c.path).collect();
            common = Some(match common {
                Some(seen) => seen.into_iter().filter(|p| paths.contains(p)).collect(),
                None => paths,
            });
        }
        if common.is_some_and(|c| !c.is_empty()) {
            return Err(Error::Lacks(
                "this merge's result differs from every parent".to_string(),
                "git that prints combined diffs",
            ));
        }
        if opts.stat || opts.shortstat || opts.numstat {
            let changes = commit_changes(repo, commit, spec, opts)?;
            let stats = DiffOpts {
                patch: false,
                name_only: false,
                name_status: false,
                src_prefix: opts.src_prefix.clone(),
                dst_prefix: opts.dst_prefix.clone(),
                filter: opts.filter.clone(),
                ..*opts
            };
            Printer { repo, opts: &stats }.print(&mut body, &changes)?;
        }
        separate = true;
    } else if opts.any() && (!merge || matches!(merges, Merges::FirstParent)) {
        let changes = commit_changes(repo, commit, spec, opts)?;
        Printer { repo, opts }.print(&mut body, &changes)?;
        if body.is_empty() && opts.filter.is_some() {
            return Ok(());
        }
    }
    let terminates = shape.terminates();
    let empty = matches!(&shape.format, Format::User(format, _) if format.is_empty());
    if !*first && !terminates {
        out.write_all(b"\n")?;
    }
    *first = false;
    out.write_all(&pretty.commit(commit)?)?;
    if terminates && !empty {
        out.write_all(b"\n")?;
    }
    if body.is_empty() && !separate {
        return Ok(());
    }
    if !matches!(shape.format, Format::Oneline) && !empty {
        if opts.stat && opts.patch && !merge {
            out.write_all(b"---")?;
        }
        out.write_all(b"\n")?;
    }
    out.write_all(&body)?;
    Ok(())
}

/// Whether a commit leaves the paths as one of its parents has them.
fn unchanged(repo: &Repo<'_>, commit: &Commit, spec: &Pathspec) -> Res<bool> {
    if commit.parents.is_empty() {
        let mut changes = Vec::new();
        trees(repo, None, Some(&commit.tree), b"", spec, &mut changes)?;
        return Ok(changes.is_empty());
    }
    for parent in &commit.parents {
        let tree = repo.commit(parent)?.tree;
        let mut changes = Vec::new();
        trees(
            repo,
            Some(&tree),
            Some(&commit.tree),
            b"",
            spec,
            &mut changes,
        )?;
        if changes.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Clone, Copy)]
enum Merges {
    Skip,
    Combined,
    FirstParent,
}

/// Splits a log-like command line among the three sets of options; what is left is
/// revisions and paths.
fn options(
    git: &Git<'_>,
    command: &str,
    args: &[String],
    shape: &mut Shape,
    revs: &mut Revs,
    opts: &mut DiffOpts,
) -> Res<Vec<String>> {
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            rest.extend(args[i..].iter().cloned());
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            rest.push(arg.to_string());
        } else if matches!(arg, "--no-color" | "--color=never" | "--no-pager") {
        } else if !(shape.option(command, args, &mut i)?
            || revs.option(git, command, args, &mut i)?
            || opts.parse(command, args, &mut i)?)
        {
            return Err(option(command, arg));
        }
        i += 1;
    }
    Ok(rest)
}

pub fn log(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut revs = Revs::new();
    let mut shape = Shape::new(&git.repo)?;
    let mut opts = DiffOpts::new(&git.repo, git.columns)?;
    let rest = options(git, "log", args, &mut shape, &mut revs, &mut opts)?;
    revs.positional(git, &rest)?;
    let spec = revs.pathspec(git)?;
    let merges = if revs.walk.first_parent {
        Merges::FirstParent
    } else {
        Merges::Skip
    };
    let pretty = shape.pretty(git)?;
    let mut first = true;
    revs.commits(git, &mut |commit| {
        print_commit(
            io.out, &pretty, &shape, &opts, &spec, commit, &mut first, merges,
        )?;
        Ok(true)
    })?;
    Ok(0)
}

pub fn show(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut revs = Revs::new();
    let mut shape = Shape::new(&git.repo)?;
    let mut opts = DiffOpts::new(&git.repo, git.columns)?;
    let rest = options(git, "show", args, &mut shape, &mut revs, &mut opts)?;
    revs.positional(git, &rest)?;
    if !revs.walk.include.is_empty() || !revs.walk.exclude.is_empty() {
        return Err(option("show", "a range of commits"));
    }
    if !opts.any() && !opts.quiet {
        opts.patch = true;
    }
    let spec = revs.pathspec(git)?;
    let paths_empty = revs.paths.is_empty();
    let pretty = shape.pretty(git)?;
    let mut named = std::mem::take(&mut revs.named);
    if named.is_empty() {
        match git.repo.head()? {
            Some(head) => named.push(("HEAD".to_string(), head)),
            None => {
                let branch = git.repo.head_ref()?.unwrap_or_default();
                return Err(Error::Fatal(format!(
                    "your current branch '{}' does not have any commits yet",
                    shorten(&branch)
                )));
            }
        }
    }
    let mut first = true;
    for (name, oid) in named {
        let mut oid = oid;
        loop {
            let (kind, data) = git.repo.object(&oid)?;
            match kind {
                Kind::Blob => io.out.write_all(&data)?,
                Kind::Tree => {
                    if !first {
                        io.out.write_all(b"\n")?;
                    }
                    writeln!(io.out, "tree {name}\n")?;
                    for entry in git.repo.tree(&oid)?.iter() {
                        io.out.write_all(&entry.name)?;
                        if entry.is_tree() {
                            io.out.write_all(b"/")?;
                        }
                        io.out.write_all(b"\n")?;
                    }
                }
                Kind::Tag => {
                    if !first {
                        io.out.write_all(b"\n")?;
                    }
                    io.out.write_all(&tag_header(&data, &shape.date, git.now))?;
                    first = true;
                    oid = super::repo::tag_target(&data)
                        .ok_or_else(|| Error::Fatal("a tag names no object".to_string()))?;
                    continue;
                }
                Kind::Commit => {
                    let commit = git.repo.commit(&oid)?;
                    // With paths, a commit that changes none of them is not shown.
                    if !paths_empty && unchanged(&git.repo, &commit, &spec)? {
                        break;
                    }
                    print_commit(
                        io.out,
                        &pretty,
                        &shape,
                        &opts,
                        &spec,
                        &commit,
                        &mut first,
                        Merges::Combined,
                    )?;
                    break;
                }
            }
            first = false;
            break;
        }
    }
    Ok(0)
}

pub fn diff(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut opts = DiffOpts::new(&git.repo, git.columns)?;
    let mut cached = false;
    let mut rest = Vec::new();
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            paths.extend(args[i + 1..].iter().cloned());
            break;
        }
        match arg {
            "--cached" | "--staged" => cached = true,
            "--no-index" => return Err(option("diff", arg)),
            "--merge-base" => return Err(option("diff", arg)),
            _ if !arg.starts_with('-') => rest.push(arg.to_string()),
            _ => {
                if !opts.parse("diff", args, &mut i)? {
                    return Err(option("diff", arg));
                }
            }
        }
        i += 1;
    }
    // Revisions first; the first name that is none starts the paths.
    let repo = &git.repo;
    let dashes = args.iter().any(|a| a == "--");
    let mut sides: Vec<Oid> = Vec::new();
    let mut merge_base = false;
    let tree_of = |spec: &str| -> Res<Option<Oid>> {
        let spec = if spec.is_empty() { "HEAD" } else { spec };
        let Some(oid) = rev::resolve(repo, spec, &git.prefix)? else {
            return Ok(None);
        };
        let (peeled, kind) = repo.peel(&oid)?;
        Ok(Some(match kind {
            Kind::Commit => peeled,
            Kind::Tree => peeled,
            _ => return Err(option("diff", "a blob")),
        }))
    };
    let mut at = 0;
    while at < rest.len() {
        let arg = rest[at].as_str();
        let found = if let Some(dots) = arg.find("...") {
            match (tree_of(&arg[..dots])?, tree_of(&arg[dots + 3..])?) {
                (Some(a), Some(b)) => {
                    merge_base = true;
                    sides.extend([a, b]);
                    true
                }
                _ => false,
            }
        } else if let Some(dots) = arg.find("..") {
            match (tree_of(&arg[..dots])?, tree_of(&arg[dots + 2..])?) {
                (Some(a), Some(b)) => {
                    sides.extend([a, b]);
                    true
                }
                _ => false,
            }
        } else {
            match tree_of(arg)? {
                Some(oid) => {
                    sides.push(oid);
                    true
                }
                None => false,
            }
        };
        if found && !dashes {
            not_a_file(git, arg)?;
        }
        if !found {
            if repo
                .fs
                .stat_opt(&join(&git.cwd, arg.as_bytes()), false)?
                .is_none()
            {
                // A deleted file is still a path when the index knows it.
                let spec = Pathspec::new(&git.prefix, &[arg.to_string()]).ok();
                let index = super::index::Index::load(repo)?;
                let tracked =
                    spec.is_some_and(|s| index.entries.iter().any(|e| s.matches(&e.path)));
                if !tracked {
                    return Err(rev::unknown(arg));
                }
            }
            paths.extend(rest[at..].iter().cloned());
            break;
        }
        at += 1;
    }
    let spec = Pathspec::new(&git.prefix, &paths)
        .map_err(|p| Error::Fatal(format!("{p}: '{p}' is outside repository")))?;
    let tree = |oid: &Oid| -> Res<Oid> {
        let (kind, _) = repo.object(oid)?;
        Ok(match kind {
            Kind::Commit => repo.commit(oid)?.tree,
            _ => *oid,
        })
    };
    let mut changes = match (sides.as_slice(), cached) {
        ([], false) => {
            let root = git.work_tree()?.to_vec();
            Work::new(repo, &root)?.unstaged(&spec)?
        }
        ([], true) | ([_], true) => {
            let root = git.work_tree()?.to_vec();
            let head = match sides.first() {
                Some(oid) => Some(tree(oid)?),
                None => match repo.head()? {
                    Some(head) => Some(repo.commit(&rev::commit_of(repo, head)?)?.tree),
                    None => None,
                },
            };
            Work::new(repo, &root)?.staged(head.as_ref(), &spec)?
        }
        ([one], false) => {
            let root = git.work_tree()?.to_vec();
            Work::new(repo, &root)?.against_tree(&tree(one)?, &spec)?
        }
        ([a, b], false) => {
            let mut from = *a;
            if merge_base {
                from = *rev::merge_bases(repo, *a, &[*b])?
                    .first()
                    .ok_or_else(|| Error::Fatal("no merge base".to_string()))?;
            }
            let mut changes = Vec::new();
            trees(
                repo,
                Some(&tree(&from)?),
                Some(&tree(b)?),
                b"",
                &spec,
                &mut changes,
            )?;
            changes
        }
        _ => return Err(option("diff", "more than two revisions")),
    };
    if opts.reverse {
        changes = reversed(changes);
        std::mem::swap(&mut opts.src_prefix, &mut opts.dst_prefix);
    }
    if let Some(minimum) = opts.renames {
        changes = detect_renames(repo, changes, minimum)?;
    }
    if !opts.any() {
        opts.patch = true;
    }
    let found = Printer { repo, opts: &opts }.print(io.out, &changes)?;
    if opts.check {
        return Ok(if found { 2 } else { 0 });
    }
    Ok(i32::from(opts.exit_code && found))
}

pub fn rev_list(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut revs = Revs::new();
    let mut count = false;
    let mut left_right = false;
    let mut abbrev = false;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            rest.extend(args[i..].iter().cloned());
            break;
        }
        match arg {
            "--count" => count = true,
            "--left-right" => left_right = true,
            "--abbrev-commit" => abbrev = true,
            _ if !arg.starts_with('-') => rest.push(arg.to_string()),
            _ => {
                if !revs.option(git, "rev-list", args, &mut i)? {
                    return Err(option("rev-list", arg));
                }
            }
        }
        i += 1;
    }
    revs.positional(git, &rest)?;
    if left_right {
        let (Some((a, b)), true) = (revs.symmetric, count) else {
            return Err(option("rev-list", "--left-right"));
        };
        let (left, right) = rev::ahead_behind(&git.repo, a, b)?;
        writeln!(io.out, "{left}\t{right}")?;
        return Ok(0);
    }
    if revs.named.is_empty() && revs.walk.include.is_empty() {
        return Err(Error::Usage(
            "usage: git rev-list [<options>] <commit>... [--] [<path>...]".to_string(),
        ));
    }
    let mut total = 0usize;
    revs.commits(git, &mut |commit| {
        total += 1;
        if !count {
            if abbrev {
                writeln!(io.out, "{}", git.repo.short(&commit.id)?)?;
            } else {
                writeln!(io.out, "{}", commit.id.hex())?;
            }
        }
        Ok(true)
    })?;
    if count {
        writeln!(io.out, "{total}")?;
    }
    Ok(0)
}

pub fn ls_files(git: &Git<'_>, io: &mut Io<'_>, args: &[String]) -> Res<i32> {
    let mut cached = false;
    let mut modified = false;
    let mut deleted = false;
    let mut others = false;
    let mut stage = false;
    let mut standard = false;
    let mut full_name = false;
    let mut unmatch = false;
    let mut zero = false;
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-c" | "--cached" => cached = true,
            "-m" | "--modified" => modified = true,
            "-d" | "--deleted" => deleted = true,
            "-o" | "--others" => others = true,
            "-s" | "--stage" => stage = true,
            "-z" => zero = true,
            "--exclude-standard" => standard = true,
            "--full-name" => full_name = true,
            "--error-unmatch" => unmatch = true,
            "--" => {
                paths.extend(args[i + 1..].iter().cloned());
                break;
            }
            _ if arg.starts_with('-') => return Err(option("ls-files", arg)),
            _ => paths.push(arg.to_string()),
        }
        i += 1;
    }
    if !(modified || deleted || others) {
        cached = true;
    }
    if others && !standard {
        return Err(option("ls-files", "--others without --exclude-standard"));
    }
    let root = git.work_tree()?.to_vec();
    let own = if paths.is_empty() && !git.prefix.is_empty() {
        vec![".".to_string()]
    } else {
        paths.clone()
    };
    let spec = Pathspec::new(&git.prefix, &own)
        .map_err(|p| Error::Fatal(format!("{p}: '{p}' is outside repository")))?;
    let work = Work::new(&git.repo, &root)?;
    let end: &[u8] = if zero { b"\0" } else { b"\n" };
    let name = |path: &[u8]| -> Vec<u8> {
        let rel = if full_name {
            path.to_vec()
        } else {
            super::status::shown_plain(path, &git.prefix)
        };
        if zero { rel } else { quote_path(&rel) }
    };
    let mut lines: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut matched = false;
    if others {
        let env = |key: &[u8]| -> Option<Vec<u8>> {
            git.env
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        for path in untracked(&work, Untracked::All, &spec, &env)? {
            let line = name(&path);
            lines.push((path, line));
        }
        work.answered()?;
    }
    for entry in &work.index.entries {
        if !spec.matches(&entry.path) {
            continue;
        }
        matched = true;
        let state = if modified || deleted {
            Some(work.state(entry)?)
        } else {
            None
        };
        let prefix = if stage {
            format!("{:06o} {} {}\t", entry.mode, entry.oid.hex(), entry.stage).into_bytes()
        } else {
            Vec::new()
        };
        let mut line = prefix;
        line.extend_from_slice(&name(&entry.path));
        if cached {
            lines.push((entry.path.clone(), line.clone()));
        }
        if deleted
            && matches!(state, Some(State::Gone))
            && git
                .repo
                .fs
                .stat_opt(&join(&root, &entry.path), false)?
                .is_none()
        {
            lines.push((entry.path.clone(), line.clone()));
        }
        if modified && matches!(state, Some(State::Gone | State::Changed(_))) {
            lines.push((entry.path.clone(), line));
        }
    }
    if unmatch && !matched && !paths.is_empty() {
        io.warn(&format!(
            "error: pathspec '{}' did not match any file(s) known to git\nDid you forget to 'git add'?\n",
            paths[0]
        ));
        return Ok(1);
    }
    for (_, line) in lines {
        io.out.write_all(&line)?;
        io.out.write_all(end)?;
    }
    Ok(0)
}
