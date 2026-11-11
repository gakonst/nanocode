//! `test` and `[`: POSIX's rules by argument count, then a recursive-descent parser for
//! longer expressions with `!`, `-a`, `-o` and parentheses.

use crate::host::{Access, FileKind};
use crate::sched::LocalBoxFuture;
use crate::state::{R, Vp};

pub(super) async fn test(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    run(vp, "test", &args[1..]).await
}

pub(super) async fn bracket(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    if args.last().map(Vec::as_slice) != Some(b"]") || args.len() < 2 {
        vp.error("[: missing `]'").await;
        return Ok(2);
    }
    run(vp, "[", &args[1..args.len() - 1]).await
}

async fn run(vp: &mut Vp, name: &str, args: &[Vec<u8>]) -> R {
    match eval(vp, args).await {
        Ok(b) => Ok(i32::from(!b)),
        Err(msg) => {
            vp.error(&format!("{name}: {msg}")).await;
            Ok(2)
        }
    }
}

type T = Result<bool, String>;

fn s(b: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(b)
}

fn is_unary(op: &[u8]) -> bool {
    op.len() == 2 && op[0] == b'-' && b"abcdefghkprstuwxzGLNOSnovR".contains(&op[1])
}

fn is_binary(op: &[u8]) -> bool {
    matches!(
        op,
        b"=" | b"=="
            | b"!="
            | b"<"
            | b">"
            | b"-eq"
            | b"-ne"
            | b"-lt"
            | b"-le"
            | b"-gt"
            | b"-ge"
            | b"-nt"
            | b"-ot"
            | b"-ef"
    )
}

fn eval<'a>(vp: &'a mut Vp, a: &'a [Vec<u8>]) -> LocalBoxFuture<'a, T> {
    Box::pin(async move {
        match a.len() {
            0 => Ok(false),
            1 => Ok(!a[0].is_empty()),
            2 => {
                if a[0] == b"!" {
                    Ok(a[1].is_empty())
                } else if is_unary(&a[0]) {
                    unary(vp, &a[0], &a[1]).await
                } else {
                    Err(format!("{}: unary operator expected", s(&a[0])))
                }
            }
            3 => {
                if is_binary(&a[1]) {
                    binary(vp, &a[0], &a[1], &a[2]).await
                } else if a[1] == b"-a" {
                    Ok(!a[0].is_empty() && !a[2].is_empty())
                } else if a[1] == b"-o" {
                    Ok(!a[0].is_empty() || !a[2].is_empty())
                } else if a[0] == b"!" {
                    Ok(!eval(vp, &a[1..]).await?)
                } else if a[0] == b"(" && a[2] == b")" {
                    Ok(!a[1].is_empty())
                } else {
                    Err(format!("{}: binary operator expected", s(&a[1])))
                }
            }
            4 if a[0] == b"!" => Ok(!eval(vp, &a[1..]).await?),
            4 if a[0] == b"(" && a[3] == b")" => eval(vp, &a[1..3]).await,
            _ => {
                let mut p = Parser { vp, a, i: 0 };
                let r = p.or().await?;
                if p.i < a.len() {
                    return Err("too many arguments".into());
                }
                Ok(r)
            }
        }
    })
}

struct Parser<'a> {
    vp: &'a mut Vp,
    a: &'a [Vec<u8>],
    i: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a [u8]> {
        self.a.get(self.i).map(Vec::as_slice)
    }

    fn or(&mut self) -> LocalBoxFuture<'_, T> {
        Box::pin(async move {
            let mut v = self.and().await?;
            while self.peek() == Some(b"-o") {
                self.i += 1;
                let r = self.and().await?;
                v = v || r;
            }
            Ok(v)
        })
    }

    async fn and(&mut self) -> T {
        let mut v = self.not().await?;
        while self.peek() == Some(b"-a") {
            self.i += 1;
            let r = self.not().await?;
            v = v && r;
        }
        Ok(v)
    }

    fn not(&mut self) -> LocalBoxFuture<'_, T> {
        Box::pin(async move {
            if self.peek() == Some(b"!") {
                self.i += 1;
                return Ok(!self.not().await?);
            }
            self.primary().await
        })
    }

    async fn primary(&mut self) -> T {
        let Some(t) = self.peek() else {
            return Err("argument expected".into());
        };
        if t == b"(" {
            self.i += 1;
            let v = self.or().await?;
            if self.peek() != Some(b")") {
                return Err("`)' expected".into());
            }
            self.i += 1;
            return Ok(v);
        }
        let a = self.a;
        if let Some(op) = a.get(self.i + 1)
            && is_binary(op)
            && self.i + 2 < a.len()
        {
            let (l, r) = (&a[self.i], &a[self.i + 2]);
            self.i += 3;
            return binary(self.vp, l, op, r).await;
        }
        if is_unary(t) && self.i + 1 < a.len() {
            let arg = &a[self.i + 1];
            self.i += 2;
            return unary(self.vp, t, arg).await;
        }
        self.i += 1;
        Ok(!t.is_empty())
    }
}

/// `-v name[sub]`: but for an associative array the subscript is arithmetic
/// (`a[i+1]`, through a nameref too); then the element is looked up by its number.
fn is_set(vp: &mut Vp, name: &str) -> T {
    if let Some(open) = name.find('[')
        && name.ends_with(']')
    {
        let (base, sub) = (&name[..open], &name[open + 1..name.len() - 1]);
        let numeric = vp.var_ref(base).is_some_and(|v| !v.is_assoc());
        if numeric && !matches!(sub, "@" | "*") && sub.trim().parse::<i64>().is_err() {
            let n = crate::arith::eval(vp, sub.as_bytes())?;
            return Ok(vp.is_set(&format!("{base}[{n}]")));
        }
    }
    Ok(vp.is_set(name))
}

fn int(v: &[u8]) -> Result<i64, String> {
    let t = std::str::from_utf8(v)
        .unwrap_or("")
        .trim_matches(|c| c == ' ' || c == '\t' || c == '\n');
    let digits = t.strip_prefix(['-', '+']).unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{}: integer expected", s(v)));
    }
    t.parse::<i64>()
        .map_err(|_| format!("{}: integer expected", s(v)))
}

/// A unary test (`op` is `-f` and so on), for `test` and `[[ ]]`.
pub(crate) async fn unary(vp: &mut Vp, op: &[u8], arg: &[u8]) -> T {
    let c = op[1];
    match c {
        b'z' => return Ok(arg.is_empty()),
        b'n' => return Ok(!arg.is_empty()),
        b'v' => {
            let Ok(name) = std::str::from_utf8(arg) else {
                return Ok(false);
            };
            // A nameref to an element (`declare -n r='a[1]'`): bash looks for a
            // variable of that literal name, which cannot exist.
            if let Some(v) = vp.vars.get(name)
                && v.nameref
                && v.get().is_some_and(|t| t.contains(&b'['))
            {
                return Ok(false);
            }
            return is_set(vp, name);
        }
        b'o' => {
            return Ok(crate::options::by_name(arg).is_some_and(|o| vp.opts.get(o)));
        }
        b'R' => {
            return Ok(std::str::from_utf8(arg)
                .ok()
                .and_then(|n| vp.vars.get(n))
                .is_some_and(|v| v.nameref && v.get().is_some()));
        }
        b't' => {
            let n = int(arg)?;
            return Ok(n >= 0
                && vp
                    .fd(n as u32)
                    .is_some_and(|f| matches!(&*f, crate::fd::FdObj::File(h) if h.is_terminal())));
        }
        _ => {}
    }
    if arg.is_empty() {
        return Ok(false);
    }
    let path = vp.abs_path(arg);
    let host = vp.host();
    let follow = !matches!(c, b'h' | b'L');
    let Ok(m) = host.stat(&path, follow).await else {
        return Ok(false);
    };
    Ok(match c {
        b'a' | b'e' => true,
        b'f' => m.kind == FileKind::File,
        b'd' => m.kind == FileKind::Dir,
        b'h' | b'L' => m.kind == FileKind::Symlink,
        b'p' => m.kind == FileKind::Fifo,
        b'S' => m.kind == FileKind::Socket,
        b'b' => m.kind == FileKind::BlockDevice,
        b'c' => m.kind == FileKind::CharDevice,
        b's' => m.size > 0,
        b'g' => m.mode & 0o2000 != 0,
        b'u' => m.mode & 0o4000 != 0,
        b'k' => m.mode & 0o1000 != 0,
        b'r' => host.access(&path, Access::Read).await.unwrap_or(false),
        b'w' => host.access(&path, Access::Write).await.unwrap_or(false),
        b'x' => host.access(&path, Access::Execute).await.unwrap_or(false),
        b'O' => m.uid == host.info().euid,
        // The effective group: the host reports the real one first.
        b'G' => host.info().groups.first() == Some(&m.gid),
        // Modified since last read: the host reports no access times.
        b'N' => false,
        _ => false,
    })
}

/// A binary test, for `test` and (but for the arithmetic and pattern operators,
/// which differ) `[[ ]]`.
pub(crate) async fn binary(vp: &Vp, l: &[u8], op: &[u8], r: &[u8]) -> T {
    Ok(match op {
        b"=" | b"==" => l == r,
        b"!=" => l != r,
        b"<" => l < r,
        b">" => l > r,
        b"-eq" => int(l)? == int(r)?,
        b"-ne" => int(l)? != int(r)?,
        b"-lt" => int(l)? < int(r)?,
        b"-le" => int(l)? <= int(r)?,
        b"-gt" => int(l)? > int(r)?,
        b"-ge" => int(l)? >= int(r)?,
        b"-nt" | b"-ot" | b"-ef" => {
            let host = vp.host();
            let a = host.stat(&vp.abs_path(l), true).await.ok();
            let b = host.stat(&vp.abs_path(r), true).await.ok();
            match op {
                b"-nt" => match (a, b) {
                    (Some(a), Some(b)) => a.mtime_ns > b.mtime_ns,
                    (Some(_), None) => true,
                    _ => false,
                },
                b"-ot" => match (a, b) {
                    (Some(a), Some(b)) => a.mtime_ns < b.mtime_ns,
                    (None, Some(_)) => true,
                    _ => false,
                },
                _ => match (a, b) {
                    (Some(a), Some(b)) => a.dev == b.dev && a.ino == b.ino,
                    _ => false,
                },
            }
        }
        _ => return Err(format!("{}: binary operator expected", s(op))),
    })
}
