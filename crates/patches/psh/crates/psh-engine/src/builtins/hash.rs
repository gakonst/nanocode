//! The command hash table and `hash [-lr] [-p path] [-dt] [name ...]`.
//!
//! Commands found through `PATH` are remembered with how often they ran; a remembered
//! path is used as it is, without searching again. The table belongs to the value of
//! `PATH` it was filled under: when `PATH` changes, it starts empty (bash empties it on
//! every assignment to `PATH`).

use std::rc::Rc;

use crate::host::FileKind;
use crate::state::{R, Vp};

use super::{out, s, shell_quote};

#[derive(Clone, Default)]
pub(crate) struct HashTable {
    /// The `PATH` the entries were found with.
    path: Vec<u8>,
    /// Name, path, hits; in insertion order.
    entries: Vec<(Vec<u8>, Vec<u8>, u32)>,
}

impl HashTable {
    fn sync(&mut self, path: &[u8]) {
        if self.path != path {
            self.path = path.to_vec();
            self.entries.clear();
        }
    }

    fn get(&self, name: &[u8]) -> Option<&Vec<u8>> {
        self.entries.iter().find(|e| e.0 == name).map(|e| &e.1)
    }

    fn insert(&mut self, name: &[u8], path: Vec<u8>, hits: u32) {
        match self.entries.iter_mut().find(|e| e.0 == name) {
            Some(e) => {
                e.1 = path;
                e.2 = hits;
            }
            None => self.entries.push((name.to_vec(), path, hits)),
        }
    }
}

fn current_path(vp: &Vp) -> Vec<u8> {
    vp.var("PATH")
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| crate::exec::DEFAULT_PATH.to_vec())
}

/// The table, emptied first if `PATH` changed since it was filled.
fn table(vp: &mut Vp) -> &mut HashTable {
    let path = current_path(vp);
    let t = Rc::make_mut(&mut vp.hash);
    t.sync(&path);
    t
}

/// The remembered path of a command, if any (`type` says "hashed").
pub(crate) fn hashed(vp: &Vp, name: &[u8]) -> Option<Vec<u8>> {
    if vp.hash.path != current_path(vp) {
        return None;
    }
    vp.hash.get(name).cloned()
}

/// Searches `PATH` (or `path`) for a command name without a slash, ignoring the table:
/// the first executable, else the first other file (which then fails to run with
/// "Permission denied", as in bash). A relative `PATH` entry gives a relative path
/// (`dir/name`), as bash shows it.
pub(crate) async fn search(vp: &Vp, name: &[u8], path: Option<&[u8]>) -> Option<Vec<u8>> {
    let path = path.map_or_else(|| current_path(vp), <[u8]>::to_vec);
    let mut fallback = None;
    for dir in vp.host().path_entries(&path) {
        let dir = &dir[..];
        let relative = !dir.is_empty() && vp.abs_path(dir) != dir;
        if let Some(found) = vp.host().resolve(name, dir, &vp.cwd).await {
            if !relative {
                return Some(found);
            }
            // The host may have added an extension (PATHEXT): keep its file name.
            let base = found.rsplit(|&b| b == b'/').next().unwrap_or(name);
            let mut shown = dir.to_vec();
            shown.push(b'/');
            shown.extend_from_slice(base);
            return Some(shown);
        }
        if fallback.is_none() {
            let mut cand = if dir.is_empty() {
                b".".to_vec()
            } else {
                dir.to_vec()
            };
            cand.push(b'/');
            cand.extend_from_slice(name);
            let abs = vp.abs_path(&cand);
            if matches!(vp.host().stat(&abs, true).await, Ok(m) if m.kind != FileKind::Dir) {
                fallback = Some(if relative { cand } else { abs });
            }
        }
    }
    fallback
}

/// Finds the program for a command name without a slash, as running it does: the
/// table, else a search of `PATH` (or `path`, for `command -p`), which is remembered.
/// `run` counts a hit.
pub(crate) async fn find(
    vp: &mut Vp,
    name: &[u8],
    path: Option<&[u8]>,
    run: bool,
) -> Option<Vec<u8>> {
    let hit = u32::from(run);
    if path.is_none()
        && let Some(e) = table(vp).entries.iter_mut().find(|e| e.0 == name)
    {
        e.2 += hit;
        return Some(e.1.clone());
    }
    let found = search(vp, name, path).await?;
    table(vp).insert(name, found.clone(), hit);
    Some(found)
}

const USAGE: &[u8] = b"hash: usage: hash [-lr] [-p pathname] [-dt] [name ...]\n";

pub(super) async fn hash(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let (mut list, mut reset, mut delete, mut show) = (false, false, false, false);
    let mut set: Option<Vec<u8>> = None;
    let mut i = 1;
    'opts: while let Some(a) = args.get(i) {
        if a == b"--" {
            i += 1;
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        for (j, &c) in a.iter().enumerate().skip(1) {
            match c {
                b'l' => list = true,
                b'r' => reset = true,
                b'd' => delete = true,
                b't' => show = true,
                b'p' => {
                    let value = if j + 1 < a.len() {
                        a[j + 1..].to_vec()
                    } else {
                        i += 1;
                        match args.get(i) {
                            Some(v) => v.clone(),
                            None => {
                                vp.error("hash: -p: option requires an argument").await;
                                vp.error_raw(USAGE).await;
                                return Ok(2);
                            }
                        }
                    };
                    set = Some(value);
                    i += 1;
                    continue 'opts;
                }
                _ => {
                    vp.error(&format!("hash: -{}: invalid option", c as char))
                        .await;
                    vp.error_raw(USAGE).await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let names = &args[i..];
    if reset {
        table(vp).entries.clear();
    }
    if (show || delete) && names.is_empty() {
        let o = if show { 't' } else { 'd' };
        vp.error(&format!("hash: -{o}: option requires an argument"))
            .await;
        return Ok(1);
    }
    if names.is_empty() {
        if reset || set.is_some() {
            return Ok(0);
        }
        let t = table(vp).clone();
        if t.entries.is_empty() {
            return out(vp, "hash", b"hash: hash table empty\n").await;
        }
        let mut buf = Vec::new();
        if list {
            for (n, p, _) in &t.entries {
                buf.extend_from_slice(b"builtin hash -p ");
                buf.extend_from_slice(&shell_quote(p, vp.utf8()));
                buf.push(b' ');
                buf.extend_from_slice(&shell_quote(n, vp.utf8()));
                buf.push(b'\n');
            }
        } else {
            buf.extend_from_slice(b"hits\tcommand\n");
            for (_, p, hits) in &t.entries {
                buf.extend_from_slice(format!("{hits:4}\t").as_bytes());
                buf.extend_from_slice(p);
                buf.push(b'\n');
            }
        }
        return out(vp, "hash", &buf).await;
    }
    let mut status = 0;
    if let Some(p) = set {
        let is_dir = matches!(
            vp.host().stat(&vp.abs_path(&p), true).await,
            Ok(m) if m.kind == FileKind::Dir
        );
        for n in names {
            if is_dir {
                vp.error(&format!("hash: {}: Is a directory", s(&p))).await;
                status = 1;
            } else {
                table(vp).insert(n, p.clone(), 0);
            }
        }
        return Ok(status);
    }
    if delete {
        for n in names {
            let t = table(vp);
            match t.entries.iter().position(|e| e.0 == *n) {
                Some(k) => {
                    t.entries.remove(k);
                }
                None => {
                    vp.error(&format!("hash: {}: not found", s(n))).await;
                    status = 1;
                }
            }
        }
        return Ok(status);
    }
    if show {
        let mut buf = Vec::new();
        for n in names {
            match table(vp).get(n).cloned() {
                Some(p) => {
                    if list {
                        buf.extend_from_slice(b"builtin hash -p ");
                        buf.extend_from_slice(&p);
                        buf.push(b' ');
                        buf.extend_from_slice(n);
                    } else {
                        if names.len() > 1 {
                            buf.extend_from_slice(n);
                            buf.push(b'\t');
                        }
                        buf.extend_from_slice(&p);
                    }
                    buf.push(b'\n');
                }
                None => {
                    if !buf.is_empty() {
                        super::write_fd(vp, 1, "hash", &std::mem::take(&mut buf)).await?;
                    }
                    vp.error(&format!("hash: {}: not found", s(n))).await;
                    status = 1;
                }
            }
        }
        if !buf.is_empty() && !super::write_fd(vp, 1, "hash", &buf).await? {
            status = 1;
        }
        return Ok(status);
    }
    for n in names {
        // Paths, builtins and functions are not remembered.
        if n.contains(&b'/')
            || super::is_builtin(vp, n)
            || std::str::from_utf8(n).is_ok_and(|f| vp.funcs.contains_key(f))
        {
            continue;
        }
        match search(vp, n, None).await {
            Some(p) => table(vp).insert(n, p, 0),
            None => {
                vp.error(&format!("hash: {}: not found", s(n))).await;
                status = 1;
            }
        }
    }
    Ok(status)
}
