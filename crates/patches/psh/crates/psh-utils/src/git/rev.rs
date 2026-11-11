//! Names of objects (gitrevisions(7)) and walks over history (git-rev-list(1)).

use std::collections::{BinaryHeap, HashMap};
use std::rc::Rc;

use super::index::Index;
use super::repo::{Commit, RefValue, Repo};
use super::store::{Kind, Oid, hex_val, join};
use super::{Error, Res};

/// The message git has for a name that is neither a revision nor a file.
pub fn unknown(spec: &str) -> Error {
    Error::Fatal(format!(
        "ambiguous argument '{spec}': unknown revision or path not in the working tree.\n\
         Use '--' to separate paths from revisions, like this:\n\
         'git <command> [<revision>...] -- [<file>...]'"
    ))
}

fn pseudo_ref(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b == b'_' || b == b'-')
}

/// The full name of the reference a short name means, with the object it names.
pub fn find_ref(repo: &Repo<'_>, name: &str) -> Res<Option<(String, Oid)>> {
    if name.is_empty() || name.contains("..") || name.ends_with('/') || name.ends_with(".lock") {
        return Ok(None);
    }
    let mut tries = Vec::new();
    if name.starts_with("refs/") || pseudo_ref(name) {
        tries.push(name.to_string());
    }
    for prefix in ["refs/", "refs/tags/", "refs/heads/", "refs/remotes/"] {
        tries.push(format!("{prefix}{name}"));
    }
    tries.push(format!("refs/remotes/{name}/HEAD"));
    for full in tries {
        if let Some(oid) = repo.resolve_ref(&full)? {
            return Ok(Some((full, oid)));
        }
    }
    Ok(None)
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| hex_val(b).is_some())
}

fn short_hex(repo: &Repo<'_>, hex: &str, want_commit: bool) -> Res<Option<Oid>> {
    let found = repo.odb.with_prefix(hex.as_bytes())?;
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found[0])),
        _ => {
            let mut commits = Vec::new();
            for oid in &found {
                if let Some((kind, _)) = repo.odb.read(oid)? {
                    let commitish = kind == Kind::Commit || kind == Kind::Tag;
                    if commitish == want_commit {
                        commits.push(*oid);
                    }
                }
            }
            if commits.len() == 1 {
                return Ok(Some(commits[0]));
            }
            Err(Error::Fatal(format!("short object ID {hex} is ambiguous")))
        }
    }
}

/// The branch a name stands for where gitrevisions wants one: empty and @ are HEAD's.
fn branch_of(repo: &Repo<'_>, name: &str) -> Res<Option<String>> {
    if name.is_empty() || name == "@" || name == "HEAD" {
        return Ok(repo.head_ref()?);
    }
    Ok(find_ref(repo, name)?.map(|(full, _)| full))
}

/// The upstream of a branch (refs/remotes/R/B, or the local branch it merges).
pub fn upstream(repo: &Repo<'_>, branch: &str) -> Option<String> {
    let short = branch.strip_prefix("refs/heads/")?;
    let remote = repo.config.get(&format!("branch.{short}.remote"))?;
    let merge = repo.config.get(&format!("branch.{short}.merge"))?;
    if remote == "." {
        return Some(merge.to_string());
    }
    let name = merge.strip_prefix("refs/heads/")?;
    Some(format!("refs/remotes/{remote}/{name}"))
}

fn reflog(repo: &Repo<'_>, full: &str, n: usize) -> Res<Option<Oid>> {
    let dir = if full.starts_with("refs/") {
        &repo.common_dir
    } else {
        &repo.git_dir
    };
    let Some(text) = repo
        .fs
        .read_opt(&join(&join(dir, b"logs"), full.as_bytes()))?
    else {
        return Ok(None);
    };
    let lines: Vec<&[u8]> = text
        .split(|&b| b == b'\n')
        .filter(|l| l.len() >= 81)
        .collect();
    if n == 0 {
        return Ok(lines.last().and_then(|l| Oid::from_hex(&l[41..81])));
    }
    if n > lines.len() {
        return Ok(None);
    }
    Ok(Oid::from_hex(&lines[lines.len() - n][..40]))
}

fn basic(repo: &Repo<'_>, spec: &str) -> Res<Option<Oid>> {
    if spec == "@" {
        return Ok(repo.head()?);
    }
    if spec.len() == 40
        && let Some(oid) = Oid::from_hex(spec.as_bytes())
    {
        return Ok(Some(oid));
    }
    if let Some((_, oid)) = find_ref(repo, spec)? {
        return Ok(Some(oid));
    }
    if spec.len() >= 4 && spec.len() < 40 && is_hex(spec) {
        return short_hex(repo, spec, true);
    }
    // What git describe prints: NAME-COUNT-gHEX.
    if let Some(at) = spec.rfind("-g") {
        let hex = &spec[at + 2..];
        if hex.len() >= 4 && is_hex(hex) {
            return short_hex(repo, hex, true);
        }
    }
    Ok(None)
}

fn peel_to(repo: &Repo<'_>, oid: Oid, want: &str) -> Res<Oid> {
    let mut oid = oid;
    loop {
        let (kind, data) = repo.object(&oid)?;
        let done = match want {
            "" => kind != Kind::Tag,
            "object" => true,
            name => kind.name() == name,
        };
        if done {
            return Ok(oid);
        }
        oid = match kind {
            Kind::Tag => super::repo::tag_target(&data)
                .ok_or_else(|| Error::Fatal("a tag names no object".to_string()))?,
            Kind::Commit if want == "tree" => repo.commit(&oid)?.tree,
            _ => {
                return Err(Error::Fatal(format!(
                    "{} cannot be peeled to a {want}",
                    oid.hex()
                )));
            }
        };
    }
}

/// The commit a name means (tags followed).
pub fn commit_of(repo: &Repo<'_>, oid: Oid) -> Res<Oid> {
    peel_to(repo, oid, "commit")
}

fn revision(repo: &Repo<'_>, spec: &str) -> Res<Option<Oid>> {
    if let Some(open) = spec.rfind("^{")
        && spec.ends_with('}')
    {
        let want = &spec[open + 2..spec.len() - 1];
        if want.starts_with('/') {
            return Err(Error::Lacks(
                format!("the revision '{spec}' searches commit messages"),
                "git that searches messages for a revision",
            ));
        }
        let Some(base) = revision(repo, &spec[..open])? else {
            return Ok(None);
        };
        return match want {
            "" | "object" | "commit" | "tree" | "blob" | "tag" => {
                Ok(Some(peel_to(repo, base, want)?))
            }
            _ => Ok(None),
        };
    }
    if let Some(open) = spec.rfind("@{")
        && spec.ends_with('}')
    {
        let inside = &spec[open + 2..spec.len() - 1];
        let name = &spec[..open];
        if matches!(
            inside.to_ascii_lowercase().as_str(),
            "u" | "upstream" | "push"
        ) {
            let Some(branch) = branch_of(repo, name)? else {
                return Err(Error::Fatal("HEAD does not point to a branch".to_string()));
            };
            let short = branch.strip_prefix("refs/heads/").unwrap_or(&branch);
            let Some(up) = upstream(repo, &branch) else {
                return Err(Error::Fatal(format!(
                    "no upstream configured for branch '{short}'"
                )));
            };
            return Ok(repo.resolve_ref(&up)?);
        }
        if let Ok(n) = inside.parse::<usize>() {
            let full = if name.is_empty() {
                repo.head_ref()?.unwrap_or_else(|| "HEAD".to_string())
            } else if name == "HEAD" || name == "@" {
                "HEAD".to_string()
            } else {
                match find_ref(repo, name)? {
                    Some((full, _)) => full,
                    None => return Ok(None),
                }
            };
            return match reflog(repo, &full, n)? {
                Some(oid) => Ok(Some(oid)),
                None if n == 0 => Ok(repo.resolve_ref(&full)?),
                None => Err(Error::Fatal(format!(
                    "log for '{}' only has fewer entries than {n}",
                    if name.is_empty() { "HEAD" } else { name }
                ))),
            };
        }
        return Err(Error::Lacks(
            format!("the revision '{spec}' reads the reflog by date or by checkout"),
            "git that reads reflogs that way",
        ));
    }
    let digits = spec.bytes().rev().take_while(u8::is_ascii_digit).count();
    let at = spec.len() - digits;
    if at > 0 {
        let op = spec.as_bytes()[at - 1];
        if op == b'^' || op == b'~' {
            let n: usize = if digits == 0 {
                1
            } else {
                spec[at..].parse().unwrap_or(usize::MAX)
            };
            let Some(base) = revision(repo, &spec[..at - 1])? else {
                return Ok(None);
            };
            let mut oid = commit_of(repo, base)?;
            if op == b'^' {
                if n == 0 {
                    return Ok(Some(oid));
                }
                return Ok(repo.commit(&oid)?.parents.get(n - 1).copied());
            }
            for _ in 0..n {
                match repo.commit(&oid)?.parents.first() {
                    Some(parent) => oid = *parent,
                    None => return Ok(None),
                }
            }
            return Ok(Some(oid));
        }
    }
    basic(repo, spec)
}

/// The object a revision names, or None when it names nothing. prefix is the working
/// directory inside the worktree, with its trailing slash, for REV:./PATH.
pub fn resolve(repo: &Repo<'_>, spec: &str, prefix: &[u8]) -> Res<Option<Oid>> {
    let mut depth = 0;
    let mut colon = None;
    for (i, b) in spec.bytes().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b':' if depth == 0 => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(colon) = colon else {
        return revision(repo, spec);
    };
    let (rev, path) = (&spec[..colon], &spec[colon + 1..]);
    let mut full = Vec::new();
    let path = if let Some(rel) = path.strip_prefix("./") {
        full.extend_from_slice(prefix);
        full.extend_from_slice(rel.as_bytes());
        &full[..]
    } else {
        path.as_bytes()
    };
    if rev.is_empty() {
        if path.starts_with(b"/") {
            return Err(Error::Lacks(
                format!("the revision '{spec}' searches commit messages"),
                "git that searches messages for a revision",
            ));
        }
        let (stage, path) = match path {
            [d @ b'0'..=b'3', b':', rest @ ..] => (d - b'0', rest),
            _ => (0, path),
        };
        let index = Index::load(repo)?;
        return Ok(index
            .entries
            .iter()
            .find(|e| e.path == path && e.stage == stage)
            .map(|e| e.oid));
    }
    let Some(base) = revision(repo, rev)? else {
        return Ok(None);
    };
    let tree = peel_to(repo, base, "tree")?;
    Ok(repo.tree_entry(&tree, path)?.map(|e| e.oid))
}

const SEEN: u8 = 1;
const UNINTERESTING: u8 = 2;
const TREESAME: u8 = 4;
const ADDED: u8 = 8;

/// What a walk shows and where it stops.
#[derive(Default)]
pub struct Walk {
    pub include: Vec<Oid>,
    pub exclude: Vec<Oid>,
    /// Paths (from the repository's root, no trailing slash) a commit must touch.
    pub paths: Vec<Vec<u8>>,
    pub first_parent: bool,
    /// Commits committed before this are left out and not walked past.
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub no_merges: bool,
    pub merges: bool,
}

#[derive(PartialEq, Eq)]
struct Queued {
    time: i64,
    order: std::cmp::Reverse<u64>,
    oid: Oid,
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.time, &self.order).cmp(&(other.time, &other.order))
    }
}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A walk in git's default order: newest committer date first, parents after the
/// commits that named them when dates tie.
pub struct Walker<'r, 'a> {
    repo: &'r Repo<'a>,
    walk: Walk,
    flags: HashMap<Oid, u8>,
    queue: BinaryHeap<Queued>,
    counter: u64,
    /// With exclusions the whole list is decided before the first commit is shown.
    limited: Option<std::vec::IntoIter<Oid>>,
}

impl<'r, 'a> Walker<'r, 'a> {
    pub fn new(repo: &'r Repo<'a>, walk: Walk) -> Res<Walker<'r, 'a>> {
        let mut walker = Walker {
            repo,
            walk,
            flags: HashMap::new(),
            queue: BinaryHeap::new(),
            counter: 0,
            limited: None,
        };
        for oid in walker.walk.exclude.clone() {
            walker.flag(oid, UNINTERESTING);
            walker.push(oid)?;
        }
        for oid in walker.walk.include.clone() {
            walker.push(oid)?;
        }
        if !walker.walk.exclude.is_empty() {
            walker.limit()?;
        }
        Ok(walker)
    }

    fn flag(&mut self, oid: Oid, flag: u8) {
        *self.flags.entry(oid).or_default() |= flag;
    }

    fn has(&self, oid: &Oid, flag: u8) -> bool {
        self.flags.get(oid).is_some_and(|f| f & flag != 0)
    }

    fn push(&mut self, oid: Oid) -> Res<()> {
        if self.has(&oid, SEEN) {
            return Ok(());
        }
        self.flag(oid, SEEN);
        let time = self.repo.commit(&oid)?.time();
        self.counter += 1;
        self.queue.push(Queued {
            time,
            order: std::cmp::Reverse(self.counter),
            oid,
        });
        Ok(())
    }

    fn mark_uninteresting(&mut self, commit: &Commit) -> Res<()> {
        let mut stack: Vec<Oid> = commit.parents.clone();
        while let Some(oid) = stack.pop() {
            if self.has(&oid, UNINTERESTING) {
                continue;
            }
            self.flag(oid, UNINTERESTING);
            // A commit already read has parents that may be queued as interesting.
            if self.has(&oid, SEEN) {
                stack.extend(self.repo.commit(&oid)?.parents.iter().copied());
            }
        }
        Ok(())
    }

    /// The paths' entries in a commit's tree.
    fn at_paths(&self, tree: &Oid) -> Res<Vec<Option<(u32, Oid)>>> {
        let mut found = Vec::with_capacity(self.walk.paths.len());
        for path in &self.walk.paths {
            found.push(self.repo.tree_entry(tree, path)?.map(|e| (e.mode, e.oid)));
        }
        Ok(found)
    }

    /// The parents to walk on to, and whether the commit changes nothing the walk is
    /// limited to (git's history simplification).
    fn simplify(&mut self, commit: &Commit) -> Res<Vec<Oid>> {
        let mut parents = commit.parents.clone();
        if self.walk.first_parent {
            parents.truncate(1);
        }
        if self.walk.paths.is_empty() {
            return Ok(parents);
        }
        let own = self.at_paths(&commit.tree)?;
        if parents.is_empty() {
            if own.iter().all(Option::is_none) {
                self.flag(commit.id, TREESAME);
            }
            return Ok(parents);
        }
        for parent in &parents {
            let theirs = self.at_paths(&self.repo.commit(parent)?.tree)?;
            if theirs == own {
                self.flag(commit.id, TREESAME);
                return Ok(vec![*parent]);
            }
        }
        Ok(parents)
    }

    fn limit(&mut self) -> Res<()> {
        let mut list = Vec::new();
        let mut slop = 5;
        let mut last = i64::MAX;
        while let Some(next) = self.queue.pop() {
            let commit = self.repo.commit(&next.oid)?;
            if self.has(&next.oid, UNINTERESTING) {
                self.mark_uninteresting(&commit)?;
                for parent in &commit.parents {
                    self.push(*parent)?;
                }
                // git's slop: go on while something newer than the last commit kept,
                // or something still interesting, is queued.
                let top = self.queue.peek();
                let Some(top) = top else { break };
                let interesting = self.queue.iter().any(|q| !self.has(&q.oid, UNINTERESTING));
                if last <= top.time || interesting {
                    slop = 5;
                    continue;
                }
                slop -= 1;
                if slop > 0 {
                    continue;
                }
                break;
            }
            if self.walk.since.is_some_and(|since| next.time < since) {
                continue;
            }
            let parents = self.simplify(&commit)?;
            for parent in parents {
                self.push(parent)?;
            }
            last = next.time;
            list.push(next.oid);
        }
        self.queue.clear();
        self.limited = Some(list.into_iter());
        Ok(())
    }

    fn shown(&self, commit: &Commit) -> bool {
        if self.has(&commit.id, UNINTERESTING | TREESAME) {
            return false;
        }
        if self.walk.no_merges && commit.parents.len() > 1 {
            return false;
        }
        if self.walk.merges && commit.parents.len() < 2 {
            return false;
        }
        !self.walk.until.is_some_and(|until| commit.time() > until)
    }

    /// The next commit of the walk.
    pub fn next(&mut self) -> Res<Option<Rc<Commit>>> {
        loop {
            if let Some(list) = &mut self.limited {
                let Some(oid) = list.next() else {
                    return Ok(None);
                };
                let commit = self.repo.commit(&oid)?;
                if self.shown(&commit) {
                    return Ok(Some(commit));
                }
                continue;
            }
            let Some(next) = self.queue.pop() else {
                return Ok(None);
            };
            if self.walk.since.is_some_and(|since| next.time < since) {
                continue;
            }
            let commit = self.repo.commit(&next.oid)?;
            if !self.has(&next.oid, ADDED) {
                self.flag(next.oid, ADDED);
                let parents = self.simplify(&commit)?;
                for parent in parents {
                    self.push(parent)?;
                }
            }
            if self.shown(&commit) {
                return Ok(Some(commit));
            }
        }
    }
}

const PARENT1: u8 = 1;
const PARENT2: u8 = 2;
const STALE: u8 = 4;
const RESULT: u8 = 8;

/// The best common ancestors of one commit and others (git-merge-base(1)), newest
/// first.
pub fn merge_bases(repo: &Repo<'_>, one: Oid, others: &[Oid]) -> Res<Vec<Oid>> {
    if others.contains(&one) {
        return Ok(vec![one]);
    }
    let found = paint(repo, one, others)?;
    if found.len() < 2 {
        return Ok(found);
    }
    // Leave out the ones another reaches.
    let mut keep = Vec::new();
    for (i, candidate) in found.iter().enumerate() {
        let rest: Vec<Oid> = found
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, o)| *o)
            .collect();
        let mut redundant = false;
        for other in &rest {
            if paint(repo, *other, &[*candidate])?.contains(candidate) {
                redundant = true;
                break;
            }
        }
        if !redundant {
            keep.push(*candidate);
        }
    }
    Ok(keep)
}

fn paint(repo: &Repo<'_>, one: Oid, others: &[Oid]) -> Res<Vec<Oid>> {
    let mut flags: HashMap<Oid, u8> = HashMap::new();
    let mut queue = BinaryHeap::new();
    let mut counter = 0u64;
    let mut put = |queue: &mut BinaryHeap<Queued>, oid: Oid| -> Res<()> {
        counter += 1;
        queue.push(Queued {
            time: repo.commit(&oid)?.time(),
            order: std::cmp::Reverse(counter),
            oid,
        });
        Ok(())
    };
    flags.insert(one, PARENT1);
    put(&mut queue, one)?;
    for other in others {
        *flags.entry(*other).or_default() |= PARENT2;
        put(&mut queue, *other)?;
    }
    let mut results: Vec<(i64, Oid)> = Vec::new();
    while queue.iter().any(|q| flags[&q.oid] & STALE == 0) {
        let Some(next) = queue.pop() else { break };
        let mut mine = flags[&next.oid] & (PARENT1 | PARENT2 | STALE);
        if mine == PARENT1 | PARENT2 {
            let entry = flags.entry(next.oid).or_default();
            if *entry & RESULT == 0 {
                *entry |= RESULT;
                results.push((next.time, next.oid));
            }
            mine |= STALE;
        }
        for parent in &repo.commit(&next.oid)?.parents {
            let theirs = flags.entry(*parent).or_default();
            if *theirs & mine == mine {
                continue;
            }
            *theirs |= mine;
            put(&mut queue, *parent)?;
        }
    }
    results.sort_by_key(|r| std::cmp::Reverse(r.0));
    Ok(results
        .into_iter()
        .filter(|(_, oid)| flags[oid] & STALE == 0)
        .map(|(_, oid)| oid)
        .collect())
}

/// Whether ancestor is reachable from commit (or is it).
pub fn is_ancestor(repo: &Repo<'_>, ancestor: Oid, commit: Oid) -> Res<bool> {
    if ancestor == commit {
        return Ok(true);
    }
    Ok(paint(repo, ancestor, &[commit])?.contains(&ancestor))
}

/// How many commits each of two tips has that the other lacks.
pub fn ahead_behind(repo: &Repo<'_>, ours: Oid, theirs: Oid) -> Res<(usize, usize)> {
    let count = |from: Oid, not: Oid| -> Res<usize> {
        let mut walker = Walker::new(
            repo,
            Walk {
                include: vec![from],
                exclude: vec![not],
                ..Walk::default()
            },
        )?;
        let mut n = 0;
        while walker.next()?.is_some() {
            n += 1;
        }
        Ok(n)
    };
    Ok((count(ours, theirs)?, count(theirs, ours)?))
}

/// A symbolic reference's target, when name is one.
pub fn symbolic(repo: &Repo<'_>, name: &str) -> Res<Option<String>> {
    Ok(match repo.read_ref(name)? {
        Some(RefValue::Symbolic(target)) => Some(target),
        _ => None,
    })
}
