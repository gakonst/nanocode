//! File modes as `chmod` and `mkdir -m` take them: octal (`755`) or symbolic
//! (`u+x,go-w`, `a=rX`, `g=u`), and as `ls -l` shows them (`-rwxr-xr-x`). Written for
//! psh after the POSIX `chmod` description and GNU's manual.

use psh_engine::host::FileKind;

const SUID: u32 = 0o4000;
const SGID: u32 = 0o2000;
const STICKY: u32 = 0o1000;

/// A parsed mode: applied to a file's old mode with [`Mode::apply`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The octal number and how many digits it had (GNU keeps a directory's set-id
    /// bits for fewer than five digits).
    Octal(u32, usize),
    Symbolic(Vec<Clause>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clause {
    /// The `ugo` bits the clause affects, as a mask of `0o7777`; `None`: not given (all,
    /// less the umask).
    who: Option<u32>,
    actions: Vec<(u8, Perm)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Perm {
    /// `rwxXst` letters.
    Letters(Vec<u8>),
    /// `u`, `g` or `o`: the bits that class has now.
    Copy(u8),
    /// `[+-=]0755`, GNU.
    Octal(u32),
}

/// Parses a mode; `None` when it is invalid.
pub fn parse(spec: &[u8]) -> Option<Mode> {
    if !spec.is_empty() && spec.iter().all(|b| (b'0'..=b'7').contains(b)) {
        let n = u32::from_str_radix(std::str::from_utf8(spec).ok()?, 8).ok()?;
        return (n <= 0o7777).then_some(Mode::Octal(n, spec.len()));
    }
    let mut clauses = Vec::new();
    for part in spec.split(|&b| b == b',') {
        let mut i = 0;
        let mut who = None::<u32>;
        while let Some(&c) = part.get(i) {
            let bits = match c {
                b'u' => 0o4700,
                b'g' => 0o2070,
                b'o' => 0o1007,
                b'a' => 0o7777,
                _ => break,
            };
            who = Some(who.unwrap_or(0) | bits);
            i += 1;
        }
        let mut actions = Vec::new();
        if i >= part.len() {
            return None;
        }
        while i < part.len() {
            let op = part[i];
            if !matches!(op, b'+' | b'-' | b'=') {
                return None;
            }
            i += 1;
            let start = i;
            let perm = match part.get(i) {
                Some(&c @ (b'u' | b'g' | b'o')) => {
                    i += 1;
                    Perm::Copy(c)
                }
                Some(c) if c.is_ascii_digit() => {
                    while part.get(i).is_some_and(u8::is_ascii_digit) {
                        i += 1;
                    }
                    let digits = std::str::from_utf8(&part[start..i]).ok()?;
                    Perm::Octal(
                        u32::from_str_radix(digits, 8)
                            .ok()
                            .filter(|&n| n <= 0o7777)?,
                    )
                }
                _ => {
                    while part.get(i).is_some_and(|c| b"rwxXst".contains(c)) {
                        i += 1;
                    }
                    Perm::Letters(part[start..i].to_vec())
                }
            };
            actions.push((op, perm));
        }
        clauses.push(Clause { who, actions });
    }
    Some(Mode::Symbolic(clauses))
}

impl Mode {
    /// The new mode of a file with mode `old` (`0o7777` bits) and kind `kind`.
    pub fn apply(&self, old: u32, is_dir: bool, umask: u32) -> u32 {
        match self {
            Mode::Octal(n, digits) => {
                if is_dir && *digits < 5 {
                    *n | (old & (SUID | SGID))
                } else {
                    *n
                }
            }
            Mode::Symbolic(clauses) => {
                let mut mode = old & 0o7777;
                for c in clauses {
                    let (who, mask) = match c.who {
                        Some(w) => (w, 0o7777),
                        None => (0o7777, !umask & 0o7777),
                    };
                    for (op, perm) in &c.actions {
                        let bits = match perm {
                            Perm::Octal(n) => *n,
                            Perm::Copy(from) => {
                                let class = match from {
                                    b'u' => (mode >> 6) & 7,
                                    b'g' => (mode >> 3) & 7,
                                    _ => mode & 7,
                                };
                                class * 0o111
                            }
                            Perm::Letters(l) => {
                                let mut b = 0;
                                for &ch in l {
                                    b |= match ch {
                                        b'r' => 0o444,
                                        b'w' => 0o222,
                                        b'x' => 0o111,
                                        b'X' if is_dir || mode & 0o111 != 0 => 0o111,
                                        b's' => SUID | SGID,
                                        b't' => STICKY,
                                        _ => 0,
                                    };
                                }
                                b
                            }
                        };
                        let bits = bits & who & mask;
                        match op {
                            b'+' => mode |= bits,
                            b'-' => mode &= !bits,
                            _ => {
                                // `=` clears what the clause names (a directory keeps
                                // its set-id bits unless they are named), then sets.
                                let mut clear = who & mask & 0o777;
                                if !is_dir {
                                    clear |= who & mask & (SUID | SGID);
                                }
                                if who & mask & STICKY != 0 && !is_dir {
                                    clear |= STICKY;
                                }
                                mode = (mode & !clear) | bits;
                            }
                        }
                    }
                }
                mode
            }
        }
    }
}

/// `ls -l`'s ten characters: the kind and the permission bits.
pub fn mode_string(kind: FileKind, mode: u32) -> String {
    let mut s = String::with_capacity(10);
    s.push(match kind {
        FileKind::Dir => 'd',
        FileKind::Symlink => 'l',
        FileKind::Fifo => 'p',
        FileKind::Socket => 's',
        FileKind::BlockDevice => 'b',
        FileKind::CharDevice => 'c',
        FileKind::File | FileKind::Other => '-',
    });
    let bit = |m: u32, c: char| if mode & m != 0 { c } else { '-' };
    let special = |x: u32, sp: u32, set: char, unset: char| match (mode & x != 0, mode & sp != 0) {
        (true, true) => set,
        (false, true) => unset,
        (true, false) => 'x',
        (false, false) => '-',
    };
    s.push(bit(0o400, 'r'));
    s.push(bit(0o200, 'w'));
    s.push(special(0o100, SUID, 's', 'S'));
    s.push(bit(0o040, 'r'));
    s.push(bit(0o020, 'w'));
    s.push(special(0o010, SGID, 's', 'S'));
    s.push(bit(0o004, 'r'));
    s.push(bit(0o002, 'w'));
    s.push(special(0o001, STICKY, 't', 'T'));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap(spec: &str, old: u32, dir: bool) -> u32 {
        parse(spec.as_bytes()).unwrap().apply(old, dir, 0o022)
    }

    #[test]
    fn modes() {
        assert_eq!(ap("755", 0o644, false), 0o755);
        assert_eq!(ap("+x", 0o644, false), 0o755);
        assert_eq!(ap("u+x", 0o644, false), 0o744);
        assert_eq!(ap("a+w", 0o644, false), 0o666);
        // No `who`: the umask's bits stay.
        assert_eq!(ap("+w", 0o644, false), 0o644);
        assert_eq!(ap("go-r", 0o644, false), 0o600);
        assert_eq!(ap("u=rw,go=", 0o777, false), 0o600);
        assert_eq!(ap("a=rX", 0o700, true), 0o555);
        assert_eq!(ap("a=rX", 0o600, false), 0o444);
        assert_eq!(ap("g=u", 0o740, false), 0o770);
        assert_eq!(ap("u+s", 0o755, false), 0o4755);
        assert_eq!(ap("+t", 0o755, true), 0o1755);
        assert_eq!(ap("755", 0o2700, true), 0o2755);
        assert_eq!(ap("00755", 0o2700, true), 0o755);
        assert_eq!(ap("-x", 0o755, false), 0o644);
        assert!(parse(b"u+z").is_none() && parse(b"9").is_none() && parse(b"").is_none());
        assert!(parse(b"u").is_none());
        assert_eq!(mode_string(FileKind::File, 0o4755), "-rwsr-xr-x");
        assert_eq!(mode_string(FileKind::Dir, 0o1777), "drwxrwxrwt");
        assert_eq!(mode_string(FileKind::File, 0o2644), "-rw-r-Sr--");
    }
}
