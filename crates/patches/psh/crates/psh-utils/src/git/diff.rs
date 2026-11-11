//! What changed between two trees, the index and the worktree, and how git prints it:
//! patches, --stat, --numstat, --shortstat, --name-only, --name-status and --check,
//! with git's rename detection (diffcore-rename: exact, by base name, then by
//! similarity).

use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

use super::cmd::quote_path;
use super::patch::{Space, TextDiff, check, is_binary};
use super::repo::Repo;
use super::store::{Data, Oid, join};
use super::wild::Pathspec;
use super::{Error, Res, option};

/// One side of a change. data is there when the content is not an object (a worktree
/// file).
#[derive(Clone)]
pub struct File {
    pub mode: u32,
    pub oid: Oid,
    pub data: Option<Data>,
}

impl PartialEq for File {
    fn eq(&self, other: &File) -> bool {
        self.mode == other.mode && self.oid == other.oid
    }
}

pub struct Change {
    pub path: Vec<u8>,
    /// The name it was renamed from.
    pub from: Option<Vec<u8>>,
    pub old: Option<File>,
    pub new: Option<File>,
    /// Similarity of a rename, out of 60000 as git counts it.
    pub score: u32,
}

const MAX_SCORE: u32 = 60000;

impl Change {
    pub fn status(&self) -> char {
        match (&self.old, &self.new) {
            (None, _) => 'A',
            (_, None) => 'D',
            (Some(_), Some(_)) if self.from.is_some() => 'R',
            (Some(old), Some(new)) if old.mode & 0o170000 != new.mode & 0o170000 => 'T',
            _ => 'M',
        }
    }
}

pub fn content(repo: &Repo<'_>, file: &File) -> Res<Data> {
    if let Some(data) = &file.data {
        return Ok(data.clone());
    }
    if file.mode == 0o160000 {
        return Ok(Rc::new(
            format!("Subproject commit {}\n", file.oid.hex()).into_bytes(),
        ));
    }
    repo.blob(&file.oid)
}

/// The files of a tree, by path.
pub fn flatten(
    repo: &Repo<'_>,
    tree: &Oid,
    base: &[u8],
    spec: &Pathspec,
    out: &mut Vec<(Vec<u8>, u32, Oid)>,
) -> Res<()> {
    for entry in repo.tree(tree)?.iter() {
        let path = if base.is_empty() {
            entry.name.clone()
        } else {
            join(base, &entry.name)
        };
        if entry.is_tree() {
            if spec.may_match_under(&path) {
                flatten(repo, &entry.oid, &path, spec, out)?;
            }
        } else if spec.matches(&path) {
            out.push((path, entry.mode, entry.oid));
        }
    }
    Ok(())
}

fn side(
    repo: &Repo<'_>,
    tree: Option<&Oid>,
    base: &[u8],
    spec: &Pathspec,
    added: bool,
    out: &mut Vec<Change>,
) -> Res<()> {
    let Some(tree) = tree else { return Ok(()) };
    let mut files = Vec::new();
    flatten(repo, tree, base, spec, &mut files)?;
    for (path, mode, oid) in files {
        let file = Some(File {
            mode,
            oid,
            data: None,
        });
        out.push(Change {
            path,
            from: None,
            old: if added { None } else { file.clone() },
            new: if added { file } else { None },
            score: 0,
        });
    }
    Ok(())
}

/// The changes from one tree to another (either may be absent).
pub fn trees(
    repo: &Repo<'_>,
    old: Option<&Oid>,
    new: Option<&Oid>,
    base: &[u8],
    spec: &Pathspec,
    out: &mut Vec<Change>,
) -> Res<()> {
    let (Some(old), Some(new)) = (old, new) else {
        side(repo, old, base, spec, false, out)?;
        return side(repo, new, base, spec, true, out);
    };
    if old == new {
        return Ok(());
    }
    let (a, b) = (repo.tree(old)?, repo.tree(new)?);
    let (mut i, mut j) = (0, 0);
    // git's order of a tree: a directory sorts as its name with a slash.
    let key = |e: &super::repo::Entry| {
        let mut k = e.name.clone();
        if e.is_tree() {
            k.push(b'/');
        }
        k
    };
    while i < a.len() || j < b.len() {
        let order = match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) => {
                if x.name == y.name {
                    std::cmp::Ordering::Equal
                } else {
                    key(x).cmp(&key(y))
                }
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Greater,
        };
        let path = |e: &super::repo::Entry| {
            if base.is_empty() {
                e.name.clone()
            } else {
                join(base, &e.name)
            }
        };
        let one = |e: &super::repo::Entry, added: bool, out: &mut Vec<Change>| -> Res<()> {
            let p = path(e);
            if e.is_tree() {
                if spec.may_match_under(&p) {
                    side(repo, Some(&e.oid), &p, spec, added, out)?;
                }
            } else if spec.matches(&p) {
                let file = Some(File {
                    mode: e.mode,
                    oid: e.oid,
                    data: None,
                });
                out.push(Change {
                    path: p,
                    from: None,
                    old: if added { None } else { file.clone() },
                    new: if added { file } else { None },
                    score: 0,
                });
            }
            Ok(())
        };
        match order {
            std::cmp::Ordering::Less => {
                one(&a[i], false, out)?;
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                one(&b[j], true, out)?;
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                let (x, y) = (&a[i], &b[j]);
                i += 1;
                j += 1;
                if x.mode == y.mode && x.oid == y.oid {
                    continue;
                }
                let p = path(x);
                match (x.is_tree(), y.is_tree()) {
                    (true, true) => {
                        if spec.may_match_under(&p) {
                            trees(repo, Some(&x.oid), Some(&y.oid), &p, spec, out)?;
                        }
                    }
                    (false, false) => {
                        if spec.matches(&p) {
                            out.push(Change {
                                path: p,
                                from: None,
                                old: Some(File {
                                    mode: x.mode,
                                    oid: x.oid,
                                    data: None,
                                }),
                                new: Some(File {
                                    mode: y.mode,
                                    oid: y.oid,
                                    data: None,
                                }),
                                score: 0,
                            });
                        }
                    }
                    _ => {
                        one(x, false, out)?;
                        one(y, true, out)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn base_name(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&b| b == b'/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// git's span hashes of a file (diffcore-delta): how many bytes each hash stands for.
fn spans(data: &[u8]) -> HashMap<u32, u32> {
    const HASHBASE: u32 = 107927;
    let text = !is_binary(data);
    let mut counts: HashMap<u32, u32> = HashMap::new();
    let (mut n, mut accum1, mut accum2) = (0u32, 0u32, 0u32);
    for (i, &c) in data.iter().enumerate() {
        if text && c == b'\r' && data.get(i + 1) == Some(&b'\n') {
            continue;
        }
        let old = accum1;
        accum1 = (accum1 << 7) ^ (accum2 >> 25);
        accum2 = (accum2 << 7) ^ (old >> 25);
        accum1 = accum1.wrapping_add(u32::from(c));
        n += 1;
        if n < 64 && c != b'\n' {
            continue;
        }
        let hash = accum1.wrapping_add(accum2.wrapping_mul(0x61)) % HASHBASE;
        *counts.entry(hash).or_default() += n;
        n = 0;
        accum1 = 0;
        accum2 = 0;
    }
    if n > 0 {
        let hash = accum1.wrapping_add(accum2.wrapping_mul(0x61)) % HASHBASE;
        *counts.entry(hash).or_default() += n;
    }
    counts
}

struct Candidate {
    data: Data,
    spans: Option<HashMap<u32, u32>>,
}

struct Renames<'r, 'a> {
    repo: &'r Repo<'a>,
    loaded: HashMap<usize, Candidate>,
}

impl Renames<'_, '_> {
    fn load(&mut self, key: usize, file: &File) -> Res<()> {
        if !self.loaded.contains_key(&key) {
            let data = content(self.repo, file)?;
            self.loaded.insert(key, Candidate { data, spans: None });
        }
        Ok(())
    }

    fn spans(&mut self, key: usize) -> &HashMap<u32, u32> {
        let candidate = self.loaded.get_mut(&key).expect("loaded");
        if candidate.spans.is_none() {
            candidate.spans = Some(spans(&candidate.data));
        }
        candidate.spans.as_ref().expect("counted")
    }

    /// git's estimate_similarity: the share of the larger file that the two have in
    /// common, out of MAX_SCORE.
    fn similarity(&mut self, src: (usize, &File), dst: (usize, &File), minimum: u32) -> Res<u32> {
        if src.1.mode & 0o170000 != 0o100000 || dst.1.mode & 0o170000 != 0o100000 {
            return Ok(0);
        }
        self.load(src.0, src.1)?;
        self.load(dst.0, dst.1)?;
        let (a, b) = (
            self.loaded[&src.0].data.len() as u64,
            self.loaded[&dst.0].data.len() as u64,
        );
        let (max, min) = (a.max(b), a.min(b));
        if max * u64::from(MAX_SCORE - minimum) < (max - min) * u64::from(MAX_SCORE) {
            return Ok(0);
        }
        if a == 0 || b == 0 {
            return Ok(0);
        }
        let theirs = self.spans(dst.0).clone();
        let mut copied = 0u64;
        for (hash, count) in self.spans(src.0) {
            if let Some(other) = theirs.get(hash) {
                copied += u64::from(*count.min(other));
            }
        }
        Ok((copied * u64::from(MAX_SCORE) / max) as u32)
    }
}

/// git's default for diff.renameLimit.
const RENAME_LIMIT: usize = 1000;

/// Pairs deleted files with added ones as git's rename detection does. minimum is the
/// least similarity of a rename, out of 60000.
pub fn detect_renames(repo: &Repo<'_>, changes: Vec<Change>, minimum: u32) -> Res<Vec<Change>> {
    let srcs: Vec<usize> = (0..changes.len())
        .filter(|&i| changes[i].new.is_none())
        .collect();
    let dsts: Vec<usize> = (0..changes.len())
        .filter(|&i| changes[i].old.is_none())
        .collect();
    if srcs.is_empty() || dsts.is_empty() {
        return Ok(changes);
    }
    // For each destination, the source it is a rename of and the score.
    let mut paired: HashMap<usize, (usize, u32)> = HashMap::new();
    let mut used: Vec<bool> = vec![false; changes.len()];
    let old = |i: usize| changes[i].old.as_ref().expect("a deletion");
    let new = |i: usize| changes[i].new.as_ref().expect("an addition");

    // Exact renames: the same object under another name.
    let mut by_oid: HashMap<Oid, Vec<usize>> = HashMap::new();
    for &s in &srcs {
        by_oid.entry(old(s).oid).or_default().push(s);
    }
    for &d in &dsts {
        let Some(same) = by_oid.get(&new(d).oid) else {
            continue;
        };
        let mut best: Option<(usize, u32)> = None;
        for (tried, &s) in same.iter().enumerate() {
            if used[s] || old(s).mode & 0o170000 != new(d).mode & 0o170000 {
                continue;
            }
            let score = 1 + u32::from(base_name(&changes[s].path) == base_name(&changes[d].path));
            if best.is_none_or(|(_, b)| score > b) {
                best = Some((s, score));
                if score == 2 {
                    break;
                }
            }
            if tried + 1 > 4 {
                break;
            }
        }
        if let Some((s, _)) = best {
            used[s] = true;
            paired.insert(d, (s, MAX_SCORE));
        }
    }

    if minimum < MAX_SCORE {
        let mut renames = Renames {
            repo,
            loaded: HashMap::new(),
        };
        // A file that kept its base name, alone on both sides, and is mostly the same.
        let basename_minimum = minimum + (MAX_SCORE - minimum) / 2;
        let unique = |list: &[usize], skip: &dyn Fn(usize) -> bool| {
            let mut names: HashMap<&[u8], Option<usize>> = HashMap::new();
            for &i in list {
                if skip(i) {
                    continue;
                }
                names
                    .entry(base_name(&changes[i].path))
                    .and_modify(|v| *v = None)
                    .or_insert(Some(i));
            }
            names
        };
        let src_names = unique(&srcs, &|s| used[s]);
        let dst_names = unique(&dsts, &|d| paired.contains_key(&d));
        let mut found = Vec::new();
        for &s in &srcs {
            if used[s] {
                continue;
            }
            let name = base_name(&changes[s].path);
            if src_names.get(name) != Some(&Some(s)) {
                continue;
            }
            let Some(Some(d)) = dst_names.get(name).copied() else {
                continue;
            };
            if paired.contains_key(&d) || found.iter().any(|(fd, _, _)| *fd == d) {
                continue;
            }
            let score = renames.similarity((s, old(s)), (d, new(d)), basename_minimum)?;
            if score >= basename_minimum {
                found.push((d, s, score));
            }
        }
        for (d, s, score) in found {
            used[s] = true;
            paired.insert(d, (s, score));
        }

        // The rest: every remaining pair, best first.
        let left_src: Vec<usize> = srcs.iter().copied().filter(|&s| !used[s]).collect();
        let left_dst: Vec<usize> = dsts
            .iter()
            .copied()
            .filter(|d| !paired.contains_key(d))
            .collect();
        if !left_src.is_empty()
            && !left_dst.is_empty()
            && left_src.len() * left_dst.len() <= RENAME_LIMIT * RENAME_LIMIT
        {
            #[derive(Clone, Copy)]
            struct Score {
                dst: Option<usize>,
                src: usize,
                score: u32,
                name: u32,
            }
            // git's score_compare: unused slots last, then by score, then by name.
            let compare = |a: &Score, b: &Score| -> std::cmp::Ordering {
                match (a.dst, b.dst) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (Some(_), None) => std::cmp::Ordering::Less,
                    _ => b.score.cmp(&a.score).then(b.name.cmp(&a.name)),
                }
            };
            let mut matrix: Vec<Score> = Vec::new();
            for &d in &left_dst {
                let mut best = [Score {
                    dst: None,
                    src: 0,
                    score: 0,
                    name: 0,
                }; 4];
                for &s in &left_src {
                    let this = Score {
                        dst: Some(d),
                        src: s,
                        score: renames.similarity((s, old(s)), (d, new(d)), minimum)?,
                        name: u32::from(base_name(&changes[s].path) == base_name(&changes[d].path)),
                    };
                    let mut worst = 0;
                    for k in 1..4 {
                        if compare(&best[k], &best[worst]) == std::cmp::Ordering::Greater {
                            worst = k;
                        }
                    }
                    if compare(&best[worst], &this) == std::cmp::Ordering::Greater {
                        best[worst] = this;
                    }
                }
                matrix.extend_from_slice(&best);
            }
            matrix.sort_by(compare);
            for m in &matrix {
                let Some(d) = m.dst else { break };
                if m.score < minimum {
                    break;
                }
                if paired.contains_key(&d) || used[m.src] {
                    continue;
                }
                used[m.src] = true;
                paired.insert(d, (m.src, m.score));
            }
        }
    }

    let mut sources: HashMap<usize, (Vec<u8>, File)> = HashMap::new();
    for (s, _) in paired.values() {
        sources.insert(*s, (changes[*s].path.clone(), old(*s).clone()));
    }
    let mut out = Vec::with_capacity(changes.len());
    for (i, mut change) in changes.into_iter().enumerate() {
        if used[i] {
            continue;
        }
        if let Some((s, score)) = paired.get(&i) {
            let (from, file) = sources.remove(s).expect("a source");
            change.from = Some(from);
            change.old = Some(file);
            change.score = *score;
        }
        out.push(change);
    }
    Ok(out)
}

/// What a diff command was asked to print, and how.
pub struct DiffOpts {
    pub patch: bool,
    pub stat: bool,
    pub shortstat: bool,
    pub numstat: bool,
    pub name_only: bool,
    pub name_status: bool,
    pub check: bool,
    pub summary: bool,
    pub space: Space,
    pub context: usize,
    /// The least similarity of a rename (out of 60000), or no rename detection.
    pub renames: Option<u32>,
    pub full_index: bool,
    pub abbrev: Option<usize>,
    pub src_prefix: String,
    pub dst_prefix: String,
    pub filter: Option<String>,
    pub quiet: bool,
    pub exit_code: bool,
    pub stat_width: Option<usize>,
    pub stat_name_width: Option<usize>,
    pub stat_graph_width: Option<usize>,
    pub reverse: bool,
    pub columns: Option<usize>,
}

impl DiffOpts {
    pub fn new(repo: &Repo<'_>, columns: Option<usize>) -> Res<DiffOpts> {
        let renames = match repo.config.get("diff.renames") {
            Some(v)
                if matches!(
                    v.to_ascii_lowercase().as_str(),
                    "false" | "no" | "off" | "0"
                ) =>
            {
                None
            }
            Some(v) if matches!(v.to_ascii_lowercase().as_str(), "copies" | "copy") => {
                return Err(option("diff", "diff.renames=copies"));
            }
            _ => Some(MAX_SCORE / 2),
        };
        if let Some(algorithm) = repo.config.get("diff.algorithm")
            && !matches!(algorithm, "myers" | "default")
        {
            return Err(option("diff", &format!("diff.algorithm={algorithm}")));
        }
        let (mut src_prefix, mut dst_prefix) = ("a/".to_string(), "b/".to_string());
        if repo.config.bool("diff.noprefix") == Some(true) {
            src_prefix.clear();
            dst_prefix.clear();
        } else if repo.config.bool("diff.mnemonicprefix") == Some(true) {
            return Err(option("diff", "diff.mnemonicPrefix"));
        }
        let context = match repo.config.get("diff.context") {
            Some(n) => n.parse().map_err(|_| option("diff", "diff.context"))?,
            None => 3,
        };
        Ok(DiffOpts {
            patch: false,
            stat: false,
            shortstat: false,
            numstat: false,
            name_only: false,
            name_status: false,
            check: false,
            summary: false,
            space: Space::Exact,
            context,
            renames,
            full_index: false,
            abbrev: None,
            src_prefix,
            dst_prefix,
            filter: None,
            quiet: false,
            exit_code: false,
            stat_width: None,
            stat_name_width: None,
            stat_graph_width: None,
            reverse: false,
            columns,
        })
    }

    /// Whether any output format was asked for.
    pub fn any(&self) -> bool {
        self.patch
            || self.stat
            || self.shortstat
            || self.numstat
            || self.name_only
            || self.name_status
            || self.check
            || self.summary
    }

    /// Takes args[*i] when it is a diff option (and its value); false when it is not
    /// one.
    pub fn parse(&mut self, command: &str, args: &[String], i: &mut usize) -> Res<bool> {
        let arg = args[*i].as_str();
        let number =
            |text: &str| -> Res<usize> { text.parse::<usize>().map_err(|_| option(command, arg)) };
        let score = |text: &str| -> Res<u32> {
            if text.is_empty() {
                return Ok(MAX_SCORE / 2);
            }
            let (digits, percent) = match text.strip_suffix('%') {
                Some(d) => (d, true),
                None => (text, false),
            };
            let n: u32 = digits.parse().map_err(|_| option(command, arg))?;
            // git reads -M90 as 90% and -M5 as 50%: digits after a decimal point.
            let scale = 10u32.pow(digits.len() as u32);
            Ok(if percent {
                n.min(100) * (MAX_SCORE / 100)
            } else {
                (u64::from(n) * u64::from(MAX_SCORE) / u64::from(scale)) as u32
            })
        };
        match arg {
            "-p" | "-u" | "--patch" => self.patch = true,
            "-s" | "--no-patch" => {
                self.patch = false;
                self.stat = false;
                self.shortstat = false;
                self.numstat = false;
                self.name_only = false;
                self.name_status = false;
                self.quiet = true;
            }
            "--stat" => self.stat = true,
            "--shortstat" => self.shortstat = true,
            "--numstat" => self.numstat = true,
            "--name-only" => self.name_only = true,
            "--name-status" => self.name_status = true,
            "--check" => self.check = true,
            "--summary" => self.summary = true,
            "-w" | "--ignore-all-space" => self.space = Space::All,
            "-b" | "--ignore-space-change" => self.space = Space::Change,
            "--ignore-space-at-eol" => self.space = Space::AtEol,
            "--patch-with-stat" => {
                self.patch = true;
                self.stat = true;
            }
            "--full-index" => self.full_index = true,
            "--no-renames" => self.renames = None,
            "-M" | "--find-renames" => self.renames = Some(MAX_SCORE / 2),
            "--no-prefix" => {
                self.src_prefix.clear();
                self.dst_prefix.clear();
            }
            "--exit-code" => self.exit_code = true,
            "--quiet" => {
                self.quiet = true;
                self.exit_code = true;
            }
            "-R" => self.reverse = true,
            "--no-color"
            | "--no-ext-diff"
            | "--no-textconv"
            | "--color=never"
            | "--no-abbrev"
            | "--default-prefix"
            | "--indent-heuristic"
            | "--diff-algorithm=myers"
            | "--diff-algorithm=default"
            | "--no-color-moved"
            | "--no-relative" => {}
            "-U" | "--unified" => {
                *i += 1;
                let value = args.get(*i).ok_or_else(|| option(command, arg))?;
                self.context = number(value)?;
                self.patch = true;
            }
            "--diff-filter" => {
                *i += 1;
                self.filter = Some(args.get(*i).ok_or_else(|| option(command, arg))?.clone());
            }
            _ => {
                if let Some(n) = arg.strip_prefix("-U") {
                    self.context = number(n)?;
                    self.patch = true;
                } else if let Some(n) = arg.strip_prefix("--unified=") {
                    self.context = number(n)?;
                    self.patch = true;
                } else if let Some(n) = arg.strip_prefix("-M") {
                    self.renames = Some(score(n)?);
                } else if let Some(n) = arg.strip_prefix("--find-renames=") {
                    self.renames = Some(score(n)?);
                } else if let Some(n) = arg.strip_prefix("--abbrev=") {
                    self.abbrev = Some(number(n)?.clamp(4, 40));
                } else if let Some(f) = arg.strip_prefix("--diff-filter=") {
                    self.filter = Some(f.to_string());
                } else if let Some(widths) = arg.strip_prefix("--stat=") {
                    self.stat = true;
                    let mut parts = widths.split(',');
                    if let Some(w) = parts.next() {
                        self.stat_width = Some(number(w)?);
                    }
                    if let Some(w) = parts.next() {
                        self.stat_name_width = Some(number(w)?);
                    }
                    if parts.next().is_some() {
                        return Err(option(command, arg));
                    }
                } else if let Some(w) = arg.strip_prefix("--stat-width=") {
                    self.stat = true;
                    self.stat_width = Some(number(w)?);
                } else if let Some(w) = arg.strip_prefix("--stat-name-width=") {
                    self.stat = true;
                    self.stat_name_width = Some(number(w)?);
                } else if let Some(w) = arg.strip_prefix("--stat-graph-width=") {
                    self.stat = true;
                    self.stat_graph_width = Some(number(w)?);
                } else if let Some(p) = arg.strip_prefix("--src-prefix=") {
                    self.src_prefix = p.to_string();
                } else if let Some(p) = arg.strip_prefix("--dst-prefix=") {
                    self.dst_prefix = p.to_string();
                } else if matches!(
                    arg,
                    "-w" | "-b"
                        | "--ignore-all-space"
                        | "--ignore-space-change"
                        | "--ignore-space-at-eol"
                        | "--ignore-blank-lines"
                        | "--ignore-cr-at-eol"
                        | "--word-diff"
                        | "--color-words"
                        | "--histogram"
                        | "--patience"
                        | "--minimal"
                        | "--raw"
                        | "--summary"
                        | "--dirstat"
                        | "--binary"
                        | "-C"
                        | "--find-copies"
                        | "--find-copies-harder"
                        | "-B"
                        | "--color"
                        | "--color=always"
                        | "-W"
                        | "--function-context"
                        | "--relative"
                        | "--compact-summary"
                        | "-z"
                        | "--text"
                        | "-a"
                        | "--cc"
                        | "-c"
                        | "-m"
                        | "--submodule"
                        | "--color-moved"
                        | "--anchored"
                        | "--pickaxe-all"
                        | "--ext-diff"
                        | "--textconv"
                        | "--ita-invisible-in-index"
                        | "-O"
                ) || arg.starts_with("--word-diff")
                    || arg.starts_with("--color")
                    || arg.starts_with("--diff-algorithm")
                    || arg.starts_with("--relative")
                    || arg.starts_with("--inter-hunk-context")
                    || arg.starts_with("--dirstat")
                    || arg.starts_with("--submodule")
                    || arg.starts_with("--ignore-matching-lines")
                    || arg.starts_with("--output")
                    || arg.starts_with("--line-prefix")
                    || arg.starts_with("--stat-count")
                    || arg.starts_with("-S")
                    || arg.starts_with("-G")
                    || arg.starts_with("-I")
                    || arg.starts_with("-C")
                    || arg.starts_with("-B")
                    || arg.starts_with("-l")
                    || arg.starts_with("-O")
                    || arg.starts_with("--ws-error-highlight")
                {
                    return Err(option(command, arg));
                } else {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

/// git's pprint_rename: the two names of a rename with what they share factored out.
fn rename_name(a: &[u8], b: &[u8]) -> Vec<u8> {
    let (qa, qb) = (quote_path(a), quote_path(b));
    if qa != a || qb != b {
        let mut out = qa;
        out.extend_from_slice(b" => ");
        out.extend_from_slice(&qb);
        return out;
    }
    let mut prefix = 0;
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if x != y {
            break;
        }
        if *x == b'/' {
            prefix = i + 1;
        }
    }
    let mut suffix = 0;
    // From the ends (a name ends in a NUL for this), back to the shared prefix's slash.
    let at = |s: &[u8], i: isize| -> u8 { s.get(i as usize).copied().unwrap_or(0) };
    let floor = prefix as isize - isize::from(prefix > 0);
    let (mut i, mut j) = (a.len() as isize, b.len() as isize);
    while i >= floor && j >= floor && at(a, i) == at(b, j) {
        if at(a, i) == b'/' {
            suffix = a.len() - i as usize;
        }
        i -= 1;
        j -= 1;
    }
    let a_mid = a.len().saturating_sub(prefix + suffix);
    let b_mid = b.len().saturating_sub(prefix + suffix);
    let mut out = Vec::new();
    if prefix + suffix > 0 {
        out.extend_from_slice(&a[..prefix]);
        out.push(b'{');
    }
    out.extend_from_slice(&a[prefix..prefix + a_mid]);
    out.extend_from_slice(b" => ");
    out.extend_from_slice(&b[prefix..prefix + b_mid]);
    if prefix + suffix > 0 {
        out.push(b'}');
        out.extend_from_slice(&a[a.len() - suffix..]);
    }
    out
}

struct Stat {
    name: Vec<u8>,
    added: u64,
    deleted: u64,
    binary: bool,
}

fn decimal_width(n: u64) -> usize {
    n.to_string().len()
}

fn display_width(s: &[u8]) -> usize {
    String::from_utf8_lossy(s).chars().count()
}

fn scale(it: u64, width: u64, max: u64) -> u64 {
    if it == 0 {
        return 0;
    }
    1 + it * (width - 1) / max
}

pub struct Printer<'r, 'a> {
    pub repo: &'r Repo<'a>,
    pub opts: &'r DiffOpts,
}

impl Printer<'_, '_> {
    fn short(&self, oid: &Oid) -> Res<String> {
        if self.opts.full_index {
            return Ok(oid.hex());
        }
        match self.opts.abbrev {
            Some(len) => self.repo.short_at(oid, len),
            None => self.repo.short(oid),
        }
    }

    fn filtered<'c>(&self, changes: &'c [Change]) -> Vec<&'c Change> {
        let Some(filter) = &self.opts.filter else {
            return changes.iter().collect();
        };
        let include: Vec<char> = filter.chars().filter(char::is_ascii_uppercase).collect();
        let exclude: Vec<char> = filter
            .chars()
            .filter(char::is_ascii_lowercase)
            .map(|c| c.to_ascii_uppercase())
            .collect();
        changes
            .iter()
            .filter(|c| {
                let status = c.status();
                (include.is_empty() || include.contains(&status)) && !exclude.contains(&status)
            })
            .collect()
    }

    fn stats(&self, changes: &[&Change]) -> Res<Vec<Stat>> {
        let mut stats = Vec::new();
        for change in changes {
            let name = match &change.from {
                Some(from) => rename_name(from, &change.path),
                None => quote_path(&change.path),
            };
            let old = match &change.old {
                Some(file) => content(self.repo, file)?,
                None => Rc::default(),
            };
            let new = match &change.new {
                Some(file) => content(self.repo, file)?,
                None => Rc::default(),
            };
            let same = match (&change.old, &change.new) {
                (Some(a), Some(b)) => a.oid == b.oid,
                _ => false,
            };
            if is_binary(&old) || is_binary(&new) {
                stats.push(Stat {
                    name,
                    added: if same { 0 } else { new.len() as u64 },
                    deleted: if same { 0 } else { old.len() as u64 },
                    binary: true,
                });
                continue;
            }
            let (added, deleted) = if same {
                (0, 0)
            } else {
                let diff = TextDiff::with(&old, &new, self.opts.space);
                (diff.added() as u64, diff.removed() as u64)
            };
            stats.push(Stat {
                name,
                added,
                deleted,
                binary: false,
            });
        }
        Ok(stats)
    }

    fn summary_line(out: &mut dyn Write, files: usize, added: u64, deleted: u64) -> Res<()> {
        if files == 0 {
            return Ok(());
        }
        write!(
            out,
            " {files} file{} changed",
            if files == 1 { "" } else { "s" }
        )?;
        if added != 0 || deleted == 0 {
            write!(
                out,
                ", {added} insertion{}(+)",
                if added == 1 { "" } else { "s" }
            )?;
        }
        if deleted != 0 || added == 0 {
            write!(
                out,
                ", {deleted} deletion{}(-)",
                if deleted == 1 { "" } else { "s" }
            )?;
        }
        out.write_all(b"\n")?;
        Ok(())
    }

    /// git's show_stats.
    fn stat(&self, out: &mut dyn Write, stats: &[Stat]) -> Res<()> {
        if stats.is_empty() {
            return Ok(());
        }
        let mut max_len = 0;
        let mut max_change = 0u64;
        let mut number_width = 1;
        let mut bin_width = 0;
        for s in stats {
            max_len = max_len.max(display_width(&s.name));
            if s.binary {
                bin_width = bin_width.max(14 + decimal_width(s.added) + decimal_width(s.deleted));
                number_width = 3;
                continue;
            }
            max_change = max_change.max(s.added + s.deleted);
        }
        let mut width = self.opts.stat_width.or(self.opts.columns).unwrap_or(80);
        number_width = number_width.max(decimal_width(max_change));
        if width < 16 + 6 + number_width {
            width = 16 + 6 + number_width;
        }
        let mut graph_width = if max_change as usize + 4 > bin_width {
            max_change as usize
        } else {
            bin_width - 4
        };
        if let Some(limit) = self.opts.stat_graph_width
            && limit < graph_width
        {
            graph_width = limit;
        }
        let mut name_width = match self.opts.stat_name_width {
            Some(limit) if limit > 0 && limit < max_len => limit,
            _ => max_len,
        };
        if name_width + number_width + 6 + graph_width > width {
            if graph_width > width * 3 / 8 - number_width - 6 {
                graph_width = (width * 3 / 8 - number_width - 6).max(6);
            }
            if let Some(limit) = self.opts.stat_graph_width
                && graph_width > limit
            {
                graph_width = limit;
            }
            if name_width > width - number_width - 6 - graph_width {
                name_width = width - number_width - 6 - graph_width;
            } else {
                graph_width = width - number_width - 6 - name_width;
            }
        }
        let (mut total_added, mut total_deleted) = (0, 0);
        for s in stats {
            let mut name = &s.name[..];
            let mut prefix = "";
            let mut len = name_width;
            let name_len = display_width(name);
            if name_width < name_len {
                prefix = "...";
                len -= 3;
                // The tail that fits, from a slash where there is one.
                let text = String::from_utf8_lossy(name).into_owned();
                let skip: usize = text
                    .char_indices()
                    .nth(name_len - len)
                    .map_or(name.len(), |(i, _)| i);
                name = &name[skip..];
                if let Some(slash) = name.iter().position(|&b| b == b'/') {
                    name = &name[slash..];
                }
            }
            let padding = len.saturating_sub(display_width(name));
            out.write_all(b" ")?;
            out.write_all(prefix.as_bytes())?;
            out.write_all(name)?;
            write!(out, "{} | ", " ".repeat(padding))?;
            if s.binary {
                write!(out, "{:>number_width$}", "Bin")?;
                if s.added == 0 && s.deleted == 0 {
                    out.write_all(b"\n")?;
                    continue;
                }
                writeln!(out, " {} -> {} bytes", s.deleted, s.added)?;
                continue;
            }
            let (mut add, mut del) = (s.added, s.deleted);
            total_added += add;
            total_deleted += del;
            if graph_width as u64 <= max_change {
                let mut total = scale(add + del, graph_width as u64, max_change);
                if total < 2 && add != 0 && del != 0 {
                    total = 2;
                }
                if add < del {
                    add = scale(add, graph_width as u64, max_change);
                    del = total - add;
                } else {
                    del = scale(del, graph_width as u64, max_change);
                    add = total - del;
                }
            }
            write!(out, "{:>number_width$}", s.added + s.deleted)?;
            if s.added + s.deleted != 0 {
                out.write_all(b" ")?;
            }
            writeln!(
                out,
                "{}{}",
                "+".repeat(add as usize),
                "-".repeat(del as usize)
            )?;
        }
        Self::summary_line(out, stats.len(), total_added, total_deleted)
    }

    fn header_name(&self, prefix: &str, path: &[u8]) -> Vec<u8> {
        let mut full = prefix.as_bytes().to_vec();
        full.extend_from_slice(path);
        quote_path(&full)
    }

    fn patch(&self, out: &mut dyn Write, change: &Change) -> Res<()> {
        // A change of type is a deletion and an addition.
        if change.status() == 'T' {
            let gone = Change {
                path: change.path.clone(),
                from: None,
                old: change.old.clone(),
                new: None,
                score: 0,
            };
            let came = Change {
                path: change.path.clone(),
                from: None,
                old: None,
                new: change.new.clone(),
                score: 0,
            };
            self.patch(out, &gone)?;
            return self.patch(out, &came);
        }
        let old_path = change.from.as_ref().unwrap_or(&change.path);
        let a = self.header_name(&self.opts.src_prefix, old_path);
        let b = self.header_name(&self.opts.dst_prefix, &change.path);
        out.write_all(b"diff --git ")?;
        out.write_all(&a)?;
        out.write_all(b" ")?;
        out.write_all(&b)?;
        out.write_all(b"\n")?;
        let zero = Oid::ZERO;
        match (&change.old, &change.new) {
            (None, Some(new)) => writeln!(out, "new file mode {:06o}", new.mode)?,
            (Some(old), None) => writeln!(out, "deleted file mode {:06o}", old.mode)?,
            (Some(old), Some(new)) if old.mode != new.mode => {
                writeln!(out, "old mode {:06o}", old.mode)?;
                writeln!(out, "new mode {:06o}", new.mode)?;
            }
            _ => {}
        }
        if let Some(from) = &change.from {
            writeln!(out, "similarity index {}%", change.score * 100 / MAX_SCORE)?;
            out.write_all(b"rename from ")?;
            out.write_all(&quote_path(from))?;
            out.write_all(b"\nrename to ")?;
            out.write_all(&quote_path(&change.path))?;
            out.write_all(b"\n")?;
        }
        let old_oid = change.old.as_ref().map_or(zero, |f| f.oid);
        let new_oid = change.new.as_ref().map_or(zero, |f| f.oid);
        if old_oid == new_oid {
            return Ok(());
        }
        write!(
            out,
            "index {}..{}",
            self.short(&old_oid)?,
            self.short(&new_oid)?
        )?;
        if let (Some(old), Some(new)) = (&change.old, &change.new)
            && old.mode == new.mode
        {
            write!(out, " {:06o}", old.mode)?;
        }
        out.write_all(b"\n")?;
        let old = match &change.old {
            Some(file) => content(self.repo, file)?,
            None => Rc::default(),
        };
        let new = match &change.new {
            Some(file) => content(self.repo, file)?,
            None => Rc::default(),
        };
        let name = |present: bool, name: &[u8]| -> Vec<u8> {
            if present {
                name.to_vec()
            } else {
                b"/dev/null".to_vec()
            }
        };
        let (a, b) = (
            name(change.old.is_some(), &a),
            name(change.new.is_some(), &b),
        );
        if is_binary(&old) || is_binary(&new) {
            out.write_all(b"Binary files ")?;
            out.write_all(&a)?;
            out.write_all(b" and ")?;
            out.write_all(&b)?;
            out.write_all(b" differ\n")?;
            return Ok(());
        }
        let diff = TextDiff::with(&old, &new, self.opts.space);
        if diff.changes.is_empty() {
            return Ok(());
        }
        // A name with a space ends in a tab, so that patch(1) reads it whole.
        let tab = |name: &[u8]| -> &'static [u8] {
            if name.contains(&b' ') && name != b"/dev/null" {
                b"\t"
            } else {
                b""
            }
        };
        out.write_all(b"--- ")?;
        out.write_all(&a)?;
        out.write_all(tab(&a))?;
        out.write_all(b"\n+++ ")?;
        out.write_all(&b)?;
        out.write_all(tab(&b))?;
        out.write_all(b"\n")?;
        diff.write(out, self.opts.context)
    }

    /// Prints the changes in every format asked for. True when there were any.
    /// Whether a change is one when white space is passed over: a file whose text
    /// differs only there is no change at all.
    fn differs(&self, change: &Change) -> Res<bool> {
        let (Some(old), Some(new)) = (&change.old, &change.new) else {
            return Ok(true);
        };
        if old.mode != new.mode || change.from.is_some() {
            return Ok(true);
        }
        let (a, b) = (content(self.repo, old)?, content(self.repo, new)?);
        if is_binary(&a) || is_binary(&b) {
            return Ok(true);
        }
        Ok(!TextDiff::with(&a, &b, self.opts.space).changes.is_empty())
    }

    pub fn print(&self, out: &mut dyn Write, changes: &[Change]) -> Res<bool> {
        let mut changes = self.filtered(changes);
        let mut found = !changes.is_empty();
        if self.opts.quiet && !self.opts.check {
            return Ok(found);
        }
        if self.opts.name_only || self.opts.name_status {
            for change in &changes {
                if self.opts.name_status {
                    match change.status() {
                        'R' => {
                            write!(out, "R{:03}\t", change.score * 100 / MAX_SCORE)?;
                            out.write_all(&quote_path(change.from.as_deref().unwrap_or_default()))?;
                            out.write_all(b"\t")?;
                        }
                        status => write!(out, "{status}\t")?,
                    }
                }
                out.write_all(&quote_path(&change.path))?;
                out.write_all(b"\n")?;
            }
        }
        // git 2.50 names a file that differs only in white space passed over, and
        // leaves it out of the counts and the patch.
        if self.opts.space != Space::Exact {
            let mut kept = Vec::with_capacity(changes.len());
            for change in changes {
                if self.differs(change)? {
                    kept.push(change);
                }
            }
            changes = kept;
        }
        if self.opts.check {
            found = false;
            for change in &changes {
                let Some(new_file) = &change.new else {
                    continue;
                };
                if new_file.mode & 0o170000 != 0o100000 {
                    continue;
                }
                let old = match &change.old {
                    Some(file) => content(self.repo, file)?,
                    None => Rc::default(),
                };
                let new = content(self.repo, new_file)?;
                if is_binary(&new) || is_binary(&old) {
                    continue;
                }
                found |= check(out, &quote_path(&change.path), &old, &new)?;
            }
            return Ok(found);
        }
        let mut separator = false;
        if self.opts.stat || self.opts.shortstat || self.opts.numstat {
            let stats = self.stats(&changes)?;
            if self.opts.numstat {
                for s in &stats {
                    if s.binary {
                        out.write_all(b"-\t-\t")?;
                    } else {
                        write!(out, "{}\t{}\t", s.added, s.deleted)?;
                    }
                    out.write_all(&s.name)?;
                    out.write_all(b"\n")?;
                }
            }
            if self.opts.stat {
                self.stat(out, &stats)?;
            }
            if self.opts.shortstat {
                let added = stats.iter().filter(|s| !s.binary).map(|s| s.added).sum();
                let deleted = stats.iter().filter(|s| !s.binary).map(|s| s.deleted).sum();
                Self::summary_line(out, stats.len(), added, deleted)?;
            }
            separator = !stats.is_empty();
        }
        if self.opts.summary {
            separator |= summary(out, &changes)?;
        }
        if self.opts.patch {
            if separator {
                out.write_all(b"\n")?;
            }
            for change in &changes {
                self.patch(out, change)?;
            }
        }
        Ok(found)
    }
}

/// The lines of --summary: a file made, deleted, renamed or given another mode.
fn summary(out: &mut dyn Write, changes: &[&Change]) -> Res<bool> {
    let mut any = false;
    for change in changes {
        let name = quote_path(&change.path);
        let mut line = Vec::new();
        match (&change.old, &change.new) {
            (None, Some(new)) => {
                line.extend_from_slice(format!(" create mode {:06o} ", new.mode).as_bytes());
                line.extend_from_slice(&name);
            }
            (Some(old), None) => {
                line.extend_from_slice(format!(" delete mode {:06o} ", old.mode).as_bytes());
                line.extend_from_slice(&name);
            }
            (Some(old), Some(new)) => {
                if let Some(from) = &change.from {
                    line.extend_from_slice(b" rename ");
                    line.extend_from_slice(&rename_name(from, &change.path));
                    line.extend_from_slice(
                        format!(" ({}%)", change.score * 100 / MAX_SCORE).as_bytes(),
                    );
                }
                if old.mode != new.mode {
                    if !line.is_empty() {
                        line.push(b'\n');
                    }
                    line.extend_from_slice(
                        format!(" mode change {:06o} => {:06o}", old.mode, new.mode).as_bytes(),
                    );
                    if change.from.is_none() {
                        line.push(b' ');
                        line.extend_from_slice(&name);
                    }
                }
            }
            (None, None) => {}
        }
        if !line.is_empty() {
            any = true;
            out.write_all(&line)?;
            out.write_all(b"\n")?;
        }
    }
    Ok(any)
}

/// What git commit prints of what it committed: the counts, then a line for each
/// file made, deleted, renamed or given another mode.
pub fn commit_summary(repo: &Repo<'_>, out: &mut dyn Write, changes: Vec<Change>) -> Res<()> {
    let changes = detect_renames(repo, changes, MAX_SCORE / 2)?;
    let mut opts = DiffOpts::new(repo, None)?;
    opts.shortstat = true;
    opts.summary = true;
    Printer { repo, opts: &opts }.print(out, &changes)?;
    Ok(())
}

/// A list of changes turned around (git diff -R).
pub fn reversed(changes: Vec<Change>) -> Vec<Change> {
    changes
        .into_iter()
        .map(|c| Change {
            path: c.path,
            from: None,
            old: c.new,
            new: c.old,
            score: 0,
        })
        .collect()
}

pub fn unmerged() -> Error {
    Error::Lacks(
        "the index has unmerged paths".to_string(),
        "git that prints conflicts",
    )
}
