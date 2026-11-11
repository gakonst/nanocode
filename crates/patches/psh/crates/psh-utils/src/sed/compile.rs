//! The sed script compiler: addresses, commands and their arguments into a flat program
//! (D1140). Written for psh after the GNU sed manual and POSIX, with GNU's extensions
//! and messages; the cases follow probes of GNU sed 4.9 in the recording container.
//! Scripts are bytes; regexes go to psh-regex after sed's own escapes are resolved.

use std::rc::Rc;

use psh_regex::{Options, Regex};

/// Where a piece of the script came from, for messages.
#[derive(Clone, Debug)]
pub enum Origin {
    /// `-e` (or the script operand): its number, from 1.
    Expr(usize),
    /// `-f FILE`.
    File(Vec<u8>),
}

/// The script pieces in order; they are compiled as one text joined by newlines, so
/// that `-e 'a\' -e text` and a `{` in one piece closed in another work as in GNU.
#[derive(Default)]
pub struct Source {
    pub pieces: Vec<(Vec<u8>, Origin)>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Flags {
    pub extended: bool,
    pub utf8: bool,
    pub posix: bool,
    pub sandbox: bool,
}

#[derive(Debug)]
pub enum Addr {
    Line(u64),
    Last,
    /// `None`: the empty regex, the last one used.
    Re(Option<Rc<Regex>>),
    /// `first~step`
    Step(u64, u64),
}

#[derive(Debug)]
pub enum Addr2 {
    Line(u64),
    Last,
    Re(Option<Rc<Regex>>),
    /// `addr1,+N`
    Rel(u64),
    /// `addr1,~N`
    Mult(u64),
}

#[derive(Debug)]
pub enum Repl {
    Lit(Vec<u8>),
    /// `&` (0) or `\N`.
    Group(usize),
    Case(CaseOp),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseOp {
    Upper,
    Lower,
    UpperOne,
    LowerOne,
    End,
}

#[derive(Debug)]
pub struct Subst {
    pub re: Option<Rc<Regex>>,
    pub repl: Vec<Repl>,
    pub global: bool,
    /// Replace from the Nth match on (1: the first).
    pub nth: usize,
    pub print: bool,
    pub write: Option<usize>,
    /// `e`: run the result as a shell command; its output is the pattern space.
    pub eval: bool,
}

#[derive(Debug)]
pub enum Kind {
    /// `{`: the index of its `}`.
    Open(usize),
    Close,
    Eq,
    Append(Vec<u8>),
    Insert(Vec<u8>),
    Change(Vec<u8>),
    /// `b`, `t`, `T`: the target command (the program's length: the end).
    Branch(usize),
    BranchIf(usize),
    BranchUnless(usize),
    Delete,
    DeleteFirst,
    Get,
    GetAppend,
    Hold,
    HoldAppend,
    List(Option<usize>),
    Next,
    NextAppend,
    Print,
    PrintFirst,
    Quit(i32),
    QuitSilent(i32),
    Read(Vec<u8>),
    /// `R`: an index into `Program::rfiles`.
    ReadLine(usize),
    Subst(Box<Subst>),
    /// `w`, `W`: an index into `Program::wfiles`.
    Write(usize),
    WriteFirst(usize),
    Exchange,
    Translate(Vec<(Vec<u8>, Vec<u8>)>),
    /// `e [command]`: the command's output is printed; without one, the pattern space is
    /// the command and its output replaces it.
    Exec(Option<Vec<u8>>),
    Zap,
    FileName,
    Nop,
}

#[derive(Debug)]
pub struct Cmd {
    pub a1: Option<Addr>,
    /// `0,/re/`: the range is active from before the first line.
    pub a1_zero: bool,
    pub a2: Option<Addr2>,
    pub neg: bool,
    pub kind: Kind,
}

#[derive(Debug, Default)]
pub struct Program {
    pub cmds: Vec<Cmd>,
    /// `#n` on the first line.
    pub quiet: bool,
    pub wfiles: Vec<Vec<u8>>,
    pub rfiles: Vec<Vec<u8>>,
}

/// A compile error: GNU's message, or a feature psh leaves to the real sed.
#[derive(Debug)]
pub enum Error {
    /// `sed: -e expression #1, char 3: msg`.
    At(String),
    /// `sed: msg` (labels).
    Plain(String),
}

pub fn compile(src: &Source, flags: Flags) -> Result<Program, Error> {
    let mut text = Vec::new();
    let mut starts = Vec::new();
    for (i, (piece, _)) in src.pieces.iter().enumerate() {
        if i > 0 {
            text.push(b'\n');
        }
        starts.push(text.len());
        text.extend_from_slice(piece);
    }
    let mut p = P {
        s: &text,
        i: 0,
        flags,
        prog: Program::default(),
        blocks: Vec::new(),
        labels: Vec::new(),
        jumps: Vec::new(),
        have_re: false,
    };
    let r = p.program();
    let locate = |pos: usize| -> String {
        // The piece the position is in: positions count consumed bytes, so the end of
        // a piece still belongs to it.
        let k = starts.iter().rposition(|&s| s < pos).unwrap_or(0);
        let k = if pos == 0 { 0 } else { k };
        let off = pos - starts[k];
        match &src.pieces[k].1 {
            Origin::Expr(n) => format!("-e expression #{n}, char {off}"),
            Origin::File(name) => {
                let piece = &src.pieces[k].0;
                let line = piece[..off.saturating_sub(1).min(piece.len())]
                    .iter()
                    .filter(|&&b| b == b'\n')
                    .count()
                    + 1;
                format!("file {} line {line}", String::from_utf8_lossy(name))
            }
        }
    };
    match r {
        Ok(()) => {}
        Err(E::At(pos, msg)) => return Err(Error::At(format!("{}: {msg}", locate(pos)))),
        Err(E::AtEnd(msg)) => {
            // GNU reports these at char 0 of the last piece.
            let at = match src.pieces.last().map(|p| &p.1) {
                Some(Origin::File(name)) => {
                    format!("file {} line 0", String::from_utf8_lossy(name))
                }
                Some(Origin::Expr(n)) => format!("-e expression #{n}, char 0"),
                None => "-e expression #1, char 0".into(),
            };
            return Err(Error::At(format!("{at}: {msg}")));
        }
    }
    if !p.blocks.is_empty() {
        let at = match src.pieces.last().map(|p| &p.1) {
            Some(Origin::File(name)) => format!("file {} line 0", String::from_utf8_lossy(name)),
            Some(Origin::Expr(n)) => format!("-e expression #{n}, char 0"),
            None => "-e expression #1, char 0".into(),
        };
        return Err(Error::At(format!("{at}: unmatched `{{'")));
    }
    let end = p.prog.cmds.len();
    for (cmd, label) in std::mem::take(&mut p.jumps) {
        let target = if label.is_empty() {
            end
        } else {
            match p.labels.iter().find(|(l, _)| *l == label) {
                Some(&(_, t)) => t,
                None => {
                    return Err(Error::Plain(format!(
                        "can't find label for jump to `{}'",
                        String::from_utf8_lossy(&label)
                    )));
                }
            }
        };
        match &mut p.prog.cmds[cmd].kind {
            Kind::Branch(t) | Kind::BranchIf(t) | Kind::BranchUnless(t) => *t = target,
            _ => unreachable!(),
        }
    }
    Ok(p.prog)
}

enum E {
    At(usize, String),
    AtEnd(String),
}

type R<T> = Result<T, E>;

fn at<T>(pos: usize, msg: impl Into<String>) -> R<T> {
    Err(E::At(pos, msg.into()))
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
    flags: Flags,
    prog: Program,
    /// Open `{` commands.
    blocks: Vec<usize>,
    labels: Vec<(Vec<u8>, usize)>,
    jumps: Vec<(usize, Vec<u8>)>,
    /// A regex was compiled before: an empty one may refer to it.
    have_re: bool,
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\x0b' | b'\x0c')
}

impl P<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn skip_spaces(&mut self) {
        while self.peek().is_some_and(is_space) {
            self.i += 1;
        }
    }

    fn program(&mut self) -> R<()> {
        loop {
            while let Some(c) = self.peek() {
                if is_space(c) || c == b'\n' || c == b';' {
                    self.i += 1;
                } else {
                    break;
                }
            }
            let Some(c) = self.peek() else {
                return Ok(());
            };
            if c == b'#' {
                // `#n` alone on the first line: -n.
                if self.i == 0
                    && self.s.get(1) == Some(&b'n')
                    && matches!(self.s.get(2), None | Some(b'\n'))
                {
                    self.prog.quiet = true;
                }
                self.skip_line();
                continue;
            }
            self.command()?;
        }
    }

    fn skip_line(&mut self) {
        while let Some(c) = self.peek() {
            self.i += 1;
            if c == b'\n' {
                break;
            }
        }
    }

    fn command(&mut self) -> R<()> {
        let (a1, a1_zero, a2) = self.addresses()?;
        let n_addr = usize::from(a1.is_some() || a1_zero) + usize::from(a2.is_some());
        self.skip_spaces();
        let mut neg = false;
        if self.peek() == Some(b'!') {
            self.i += 1;
            neg = true;
            self.skip_spaces();
            if self.peek() == Some(b'!') {
                return at(self.i + 1, "multiple `!'s");
            }
        }
        let Some(c) = self.peek() else {
            return at(self.i, "missing command");
        };
        if c == b'\n' || c == b';' {
            return at(self.i, "missing command");
        }
        self.i += 1;
        // `0,/re/` is the only use of line 0.
        if a1_zero && !matches!(a2, Some(Addr2::Re(_))) {
            return at(self.i, "invalid usage of line address 0");
        }
        let idx = self.prog.cmds.len();
        let kind = match c {
            b'{' => {
                self.blocks.push(idx);
                Kind::Open(0)
            }
            b'}' => {
                if n_addr > 0 {
                    return at(self.i, "`}' doesn't want any addresses");
                }
                let Some(open) = self.blocks.pop() else {
                    return at(self.i, "unexpected `}'");
                };
                self.prog.cmds[open].kind = Kind::Open(idx);
                self.end_of_command()?;
                Kind::Close
            }
            b'#' => {
                if n_addr > 0 {
                    return at(self.i, "comments don't accept any addresses");
                }
                self.skip_line();
                return Ok(());
            }
            b':' => {
                if n_addr > 0 {
                    return at(self.i, ": doesn't want any addresses");
                }
                let label = self.label(false);
                if label.is_empty() {
                    return at(self.i, "\":\" lacks a label");
                }
                self.labels.push((label, idx));
                Kind::Nop
            }
            b'=' | b'd' | b'D' | b'g' | b'G' | b'h' | b'H' | b'n' | b'N' | b'p' | b'P' | b'x'
            | b'z' | b'F' => {
                self.end_of_command()?;
                match c {
                    b'=' => Kind::Eq,
                    b'd' => Kind::Delete,
                    b'D' => Kind::DeleteFirst,
                    b'g' => Kind::Get,
                    b'G' => Kind::GetAppend,
                    b'h' => Kind::Hold,
                    b'H' => Kind::HoldAppend,
                    b'n' => Kind::Next,
                    b'N' => Kind::NextAppend,
                    b'p' => Kind::Print,
                    b'P' => Kind::PrintFirst,
                    b'x' => Kind::Exchange,
                    b'z' => Kind::Zap,
                    _ => Kind::FileName,
                }
            }
            b'l' | b'L' | b'q' | b'Q' => {
                if matches!(c, b'q' | b'Q') && n_addr > 1 {
                    return at(self.i, "command only uses one address");
                }
                self.skip_spaces();
                let n = self.number();
                self.end_of_command()?;
                match c {
                    b'q' => Kind::Quit(n.unwrap_or(0) as i32),
                    b'Q' => Kind::QuitSilent(n.unwrap_or(0) as i32),
                    _ => Kind::List(n.map(|n| n as usize)),
                }
            }
            b'a' | b'i' | b'c' => {
                let text = self.text()?;
                match c {
                    b'a' => Kind::Append(text),
                    b'i' => Kind::Insert(text),
                    _ => Kind::Change(text),
                }
            }
            b'b' | b't' | b'T' => {
                let label = self.label(true);
                self.jumps.push((idx, label));
                match c {
                    b'b' => Kind::Branch(0),
                    b't' => Kind::BranchIf(0),
                    _ => Kind::BranchUnless(0),
                }
            }
            b'r' | b'R' | b'w' | b'W' => {
                if self.flags.sandbox {
                    return at(self.i, "e/r/w commands disabled in sandbox mode");
                }
                let name = self.filename()?;
                match c {
                    b'r' => Kind::Read(name),
                    b'R' => Kind::ReadLine(intern(&mut self.prog.rfiles, name)),
                    b'w' => Kind::Write(intern(&mut self.prog.wfiles, name)),
                    _ => Kind::WriteFirst(intern(&mut self.prog.wfiles, name)),
                }
            }
            b's' => Kind::Subst(Box::new(self.subst()?)),
            b'y' => Kind::Translate(self.translate()?),
            b'e' => {
                if self.flags.sandbox {
                    return at(self.i, "e/r/w commands disabled in sandbox mode");
                }
                // The rest of the line is the command.
                self.skip_spaces();
                let start = self.i;
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.i += 1;
                }
                let cmd = self.s[start..self.i].to_vec();
                if self.peek() == Some(b'\n') {
                    self.i += 1;
                }
                Kind::Exec((!cmd.is_empty()).then_some(cmd))
            }
            b'v' => {
                // `v [version]`: psh is new enough.
                while self
                    .peek()
                    .is_some_and(|c| !matches!(c, b'\n' | b';' | b'}'))
                {
                    self.i += 1;
                }
                self.end_of_command()?;
                Kind::Nop
            }
            _ => {
                let shown = String::from_utf8_lossy(&[c]).into_owned();
                return at(self.i, format!("unknown command: `{shown}'"));
            }
        };
        self.prog.cmds.push(Cmd {
            a1,
            a1_zero,
            a2,
            neg,
            kind,
        });
        Ok(())
    }

    /// After a command: blanks, then `;`, a newline, the end, or `}`/`#` (left for the
    /// next command).
    fn end_of_command(&mut self) -> R<()> {
        self.skip_spaces();
        match self.peek() {
            None | Some(b'}' | b'#') => Ok(()),
            Some(b'\n' | b';') => {
                self.i += 1;
                Ok(())
            }
            Some(_) => at(self.i + 1, "extra characters after command"),
        }
    }

    fn number(&mut self) -> Option<u64> {
        let start = self.i;
        let mut n: u64 = 0;
        while let Some(c @ b'0'..=b'9') = self.peek() {
            n = n.saturating_mul(10).saturating_add(u64::from(c - b'0'));
            self.i += 1;
        }
        (self.i > start).then_some(n)
    }

    #[allow(clippy::type_complexity)]
    fn addresses(&mut self) -> R<(Option<Addr>, bool, Option<Addr2>)> {
        let Some(a1) = self.address()? else {
            return Ok((None, false, None));
        };
        let (a1, zero) = match a1 {
            Addr::Line(0) => (None, true),
            a => (Some(a), false),
        };
        self.skip_spaces();
        if self.peek() != Some(b',') {
            if zero {
                // `0p`: reported once the command is read.
                self.skip_spaces();
                if self.peek().is_some() {
                    self.i += 1;
                }
                return at(self.i, "invalid usage of line address 0");
            }
            return Ok((a1, false, None));
        }
        self.i += 1;
        self.skip_spaces();
        let a2 = match self.peek() {
            Some(b'+') => {
                self.i += 1;
                Addr2::Rel(self.number().unwrap_or(0))
            }
            Some(b'~') => {
                self.i += 1;
                Addr2::Mult(self.number().unwrap_or(0))
            }
            _ => match self.address()? {
                Some(Addr::Line(n)) => Addr2::Line(n),
                Some(Addr::Last) => Addr2::Last,
                Some(Addr::Re(r)) => Addr2::Re(r),
                Some(Addr::Step(n, _)) => Addr2::Line(n),
                None => return at(self.i, "unexpected `,'"),
            },
        };
        Ok((a1, zero, Some(a2)))
    }

    fn address(&mut self) -> R<Option<Addr>> {
        match self.peek() {
            Some(b'0'..=b'9') => {
                let n = self.number().unwrap_or(0);
                if self.peek() == Some(b'~') {
                    self.i += 1;
                    let step = self.number().unwrap_or(0);
                    return Ok(Some(Addr::Step(n, step)));
                }
                Ok(Some(Addr::Line(n)))
            }
            Some(b'$') => {
                self.i += 1;
                Ok(Some(Addr::Last))
            }
            Some(b'/') => {
                self.i += 1;
                self.address_regex(b'/').map(Some)
            }
            Some(b'\\') => {
                self.i += 1;
                match self.peek() {
                    Some(d) if d != b'\n' && d != b'\\' => {
                        self.i += 1;
                        self.address_regex(d).map(Some)
                    }
                    _ => at(self.i, "unexpected `,'"),
                }
            }
            _ => Ok(None),
        }
    }

    fn address_regex(&mut self, delim: u8) -> R<Addr> {
        let Some(raw) = self.delimited(delim, true) else {
            return at(self.i, "unterminated address regex");
        };
        let (mut icase, mut multi) = (false, false);
        loop {
            match self.peek() {
                Some(b'I') => icase = true,
                Some(b'M') => multi = true,
                _ => break,
            }
            self.i += 1;
        }
        Ok(Addr::Re(self.regex(&raw, icase, multi)?))
    }

    /// Reads up to an unescaped `delim` (consumed): `\delim` becomes `delim`, other
    /// escapes stay for later. In a regex, a bracket expression may contain `delim`.
    /// `None` at a newline or the end.
    fn delimited(&mut self, delim: u8, regex: bool) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            if c == b'\n' && delim != b'\n' {
                return None;
            }
            self.i += 1;
            if c == delim {
                return Some(out);
            }
            if c == b'\\' {
                let d = self.peek()?;
                self.i += 1;
                if d == delim {
                    out.push(d);
                } else if d == b'\n' {
                    // An escaped newline is a newline.
                    out.push(b'\n');
                } else {
                    out.push(b'\\');
                    out.push(d);
                }
                continue;
            }
            if regex && c == b'[' {
                out.push(c);
                self.bracket(&mut out)?;
                continue;
            }
            out.push(c);
        }
    }

    /// The rest of a bracket expression after `[`, copied as is.
    fn bracket(&mut self, out: &mut Vec<u8>) -> Option<()> {
        if self.peek() == Some(b'^') {
            out.push(b'^');
            self.i += 1;
        }
        if self.peek() == Some(b']') {
            out.push(b']');
            self.i += 1;
        }
        loop {
            let c = self.peek()?;
            if c == b'\n' {
                return None;
            }
            self.i += 1;
            out.push(c);
            match c {
                b']' => return Some(()),
                b'[' if matches!(self.peek(), Some(b':' | b'.' | b'=')) => {
                    let kind = self.peek()?;
                    out.push(kind);
                    self.i += 1;
                    // Up to the matching `:]`, `.]` or `=]`.
                    loop {
                        let d = self.peek()?;
                        if d == b'\n' {
                            return None;
                        }
                        self.i += 1;
                        out.push(d);
                        if d == kind && self.peek() == Some(b']') {
                            out.push(b']');
                            self.i += 1;
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Compiles a regex with sed's escapes resolved; `None` for the empty regex.
    fn regex(&mut self, raw: &[u8], icase: bool, multi: bool) -> R<Option<Rc<Regex>>> {
        if raw.is_empty() {
            if !self.have_re {
                return Err(E::AtEnd("no previous regular expression".into()));
            }
            return Ok(None);
        }
        self.have_re = true;
        let (pat, raw_bytes) = sed_escapes(raw);
        // `\xce` is a byte, which GNU matches even inside a multibyte char (`s/\xce//`
        // on `μ`): a regex with such bytes that are not whole UTF-8 chars works on
        // bytes.
        let utf8 = self.flags.utf8 && !(raw_bytes && std::str::from_utf8(&pat).is_err());
        let opts = Options {
            extended: self.flags.extended,
            icase,
            newline: multi,
            utf8,
        };
        match Regex::new(&pat, opts) {
            Ok(r) => Ok(Some(Rc::new(r))),
            Err(e) => at(self.i, e.message()),
        }
    }

    fn subst(&mut self) -> R<Subst> {
        let Some(delim) = self.peek() else {
            return at(self.i, "unterminated `s' command");
        };
        if delim == b'\\' {
            return at(self.i, "unterminated `s' command");
        }
        self.i += 1;
        if delim == b'\n' {
            // A newline cannot be in a regex, so not delimit one either.
            while self.peek().is_some_and(|c| c != b'\n') {
                self.i += 1;
            }
            return at(self.i, "unterminated `s' command");
        }
        let Some(raw_re) = self.delimited(delim, true) else {
            return at(self.i, "unterminated `s' command");
        };
        let Some(raw_repl) = self.delimited_repl(delim) else {
            return at(self.i, "unterminated `s' command");
        };
        let mut s = Subst {
            re: None,
            repl: Vec::new(),
            global: false,
            nth: 0,
            print: false,
            write: None,
            eval: false,
        };
        let (mut icase, mut multi) = (false, false);
        while let Some(c) = self.peek() {
            match c {
                b'g' => {
                    self.i += 1;
                    if s.global {
                        return at(self.i, "multiple `g' options to `s' command");
                    }
                    s.global = true;
                }
                b'p' => {
                    self.i += 1;
                    if s.print {
                        return at(self.i, "multiple `p' options to `s' command");
                    }
                    s.print = true;
                }
                b'i' | b'I' => {
                    self.i += 1;
                    icase = true;
                }
                b'm' | b'M' => {
                    self.i += 1;
                    multi = true;
                }
                b'e' => {
                    self.i += 1;
                    if self.flags.sandbox {
                        return at(self.i, "e/r/w commands disabled in sandbox mode");
                    }
                    s.eval = true;
                }
                b'0'..=b'9' => {
                    if s.nth != 0 {
                        self.number();
                        return at(self.i, "multiple number options to `s' command");
                    }
                    let n = self.number().unwrap_or(0);
                    if n == 0 {
                        return at(self.i, "number option to `s' command may not be zero");
                    }
                    s.nth = usize::try_from(n).unwrap_or(usize::MAX);
                }
                b'w' => {
                    self.i += 1;
                    if self.flags.sandbox {
                        return at(self.i, "e/r/w commands disabled in sandbox mode");
                    }
                    let name = self.filename()?;
                    s.write = Some(intern(&mut self.prog.wfiles, name));
                    break;
                }
                b' ' | b'\t' => self.i += 1,
                b';' | b'\n' => {
                    self.i += 1;
                    break;
                }
                b'}' | b'#' => break,
                _ => return at(self.i + 1, "unknown option to `s'"),
            }
        }
        if s.nth == 0 {
            s.nth = 1;
        }
        s.re = self.regex(&raw_re, icase, multi)?;
        let groups = s.re.as_ref().map(|r| r.groups());
        s.repl = replacement(&raw_repl);
        for part in &s.repl {
            if let (Repl::Group(n), Some(g)) = (part, groups)
                && *n > g
            {
                return at(
                    self.i,
                    format!("invalid reference \\{n} on `s' command's RHS"),
                );
            }
        }
        Ok(s)
    }

    /// The replacement up to `delim`, its escapes kept (`\delim` is `delim`).
    fn delimited_repl(&mut self, delim: u8) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            if c == b'\n' {
                return None;
            }
            self.i += 1;
            if c == delim {
                return Some(out);
            }
            if c == b'\\' {
                let d = self.peek()?;
                self.i += 1;
                if d == delim && !matches!(d, b'&' | b'0'..=b'9' | b'n') {
                    out.push(d);
                } else {
                    out.push(b'\\');
                    out.push(d);
                }
                continue;
            }
            out.push(c);
        }
    }

    fn translate(&mut self) -> R<Vec<(Vec<u8>, Vec<u8>)>> {
        let Some(delim) = self.peek() else {
            return at(self.i, "unterminated `y' command");
        };
        if delim == b'\n' || delim == b'\\' {
            return at(self.i, "unterminated `y' command");
        }
        self.i += 1;
        let mut parts = Vec::new();
        for _ in 0..2 {
            let mut out = Vec::new();
            loop {
                let Some(c) = self.peek() else {
                    return at(self.i, "unterminated `y' command");
                };
                if c == b'\n' {
                    return at(self.i, "unterminated `y' command");
                }
                self.i += 1;
                if c == delim {
                    break;
                }
                if c == b'\\' {
                    let Some(d) = self.peek() else {
                        return at(self.i, "unterminated `y' command");
                    };
                    if let Some((b, len)) = numeric_escape(&self.s[self.i..]) {
                        out.push(b);
                        self.i += len;
                        continue;
                    }
                    self.i += 1;
                    out.push(match d {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        b'a' => 0x07,
                        b'f' => 0x0c,
                        b'v' => 0x0b,
                        d => d,
                    });
                    continue;
                }
                out.push(c);
            }
            parts.push(out);
        }
        let chars = |b: &[u8]| -> Vec<Vec<u8>> { split_chars(b, self.flags.utf8) };
        let (from, to) = (chars(&parts[0]), chars(&parts[1]));
        if from.len() != to.len() {
            return at(self.i, "strings for `y' command are different lengths");
        }
        self.end_of_command()?;
        Ok(from.into_iter().zip(to).collect())
    }

    /// The text of `a`, `i`, `c`: GNU's one-line form (`a text`, `a\text`) and the
    /// POSIX form (`a\` newline text), with `\` continuing lines.
    fn text(&mut self) -> R<Vec<u8>> {
        self.skip_spaces();
        if matches!(self.peek(), None | Some(b'\n')) {
            return at(self.i, "expected \\ after `a', `c' or `i'");
        }
        if self.flags.posix && self.peek() != Some(b'\\') {
            return at(self.i, "expected \\ after `a', `c' or `i'");
        }
        if self.peek() == Some(b'\\') {
            self.i += 1;
            match self.peek() {
                // `a\` at the very end: no text.
                None => return Ok(Vec::new()),
                Some(b'\n') => self.i += 1,
                _ => {}
            }
        }
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'\n' => break,
                b'\\' => {
                    let Some(d) = self.peek() else { break };
                    self.i += 1;
                    out.push(match d {
                        b't' => b'\t',
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b'a' => 0x07,
                        b'f' => 0x0c,
                        b'v' => 0x0b,
                        d => d,
                    });
                }
                c => out.push(c),
            }
        }
        out.push(b'\n');
        Ok(out)
    }

    /// A label: up to a newline or `;` (and, for jumps, `}`), blanks trimmed.
    fn label(&mut self, jump: bool) -> Vec<u8> {
        self.skip_spaces();
        let start = self.i;
        while let Some(c) = self.peek() {
            if c == b'\n' || c == b';' || (jump && c == b'}') {
                break;
            }
            self.i += 1;
        }
        let label = self.s[start..self.i].trim_ascii_end().to_vec();
        if self.peek().is_some_and(|c| c == b'\n' || c == b';') {
            self.i += 1;
        }
        label
    }

    /// A file name: the rest of the line.
    fn filename(&mut self) -> R<Vec<u8>> {
        self.skip_spaces();
        let start = self.i;
        while self.peek().is_some_and(|c| c != b'\n') {
            self.i += 1;
        }
        let name = self.s[start..self.i].to_vec();
        if self.peek() == Some(b'\n') {
            self.i += 1;
        }
        if name.is_empty() {
            return at(self.i, "missing filename in r/R/w/W commands");
        }
        Ok(name)
    }
}

fn intern(list: &mut Vec<Vec<u8>>, name: Vec<u8>) -> usize {
    match list.iter().position(|n| *n == name) {
        Some(i) => i,
        None => {
            list.push(name);
            list.len() - 1
        }
    }
}

/// Splits text into chars (UTF-8 sequences, or bytes).
pub fn split_chars(b: &[u8], utf8: bool) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let n = if utf8 { char_len(&b[i..]) } else { 1 };
        out.push(b[i..i + n].to_vec());
        i += n;
    }
    out
}

/// The length of the UTF-8 char at the start (1 for a byte that does not start one).
pub fn char_len(b: &[u8]) -> usize {
    let n = match b.first() {
        Some(0xc2..=0xdf) => 2,
        Some(0xe0..=0xef) => 3,
        Some(0xf0..=0xf4) => 4,
        _ => return 1,
    };
    if b.len() >= n && std::str::from_utf8(&b[..n]).is_ok() {
        n
    } else {
        1
    }
}

/// `\dNNN`, `\oNNN`, `\xHH`, `\cX` at `b[i..]` (after the backslash): the byte and the
/// length used.
fn numeric_escape(b: &[u8]) -> Option<(u8, usize)> {
    let (radix, max) = match b.first()? {
        b'd' => (10, 3),
        b'o' => (8, 3),
        b'x' => (16, 2),
        b'c' => {
            let c = *b.get(1)?;
            return Some((c.to_ascii_uppercase() ^ 0x40, 2));
        }
        _ => return None,
    };
    let digits: Vec<u8> = b[1..]
        .iter()
        .take(max)
        .take_while(|c| (**c as char).is_digit(radix))
        .copied()
        .collect();
    if digits.is_empty() {
        return None;
    }
    let v = u32::from_str_radix(std::str::from_utf8(&digits).ok()?, radix).ok()?;
    Some((v as u8, 1 + digits.len()))
}

/// Resolves sed's escapes in a regex (`\n`, `\t`, `\xHH`, `\dNNN`, `\oNNN`, `\cX`, ...),
/// also inside bracket expressions; the regex escapes stay for psh-regex. Also says
/// whether a numeric escape made a byte outside ASCII.
pub fn sed_escapes(raw: &[u8]) -> (Vec<u8>, bool) {
    let mut high = false;
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    let mut in_bracket = false;
    let mut bracket_start = 0;
    while i < raw.len() {
        let c = raw[i];
        if in_bracket {
            if c == b']' && i > bracket_start {
                in_bracket = false;
                out.push(c);
                i += 1;
                continue;
            }
            if c == b'[' && matches!(raw.get(i + 1), Some(b':' | b'.' | b'=')) {
                let kind = raw[i + 1];
                let end = (i + 2..raw.len().saturating_sub(1))
                    .find(|&j| raw[j] == kind && raw[j + 1] == b']')
                    .map(|j| j + 2)
                    .unwrap_or(raw.len());
                out.extend_from_slice(&raw[i..end]);
                i = end;
                continue;
            }
        } else if c == b'[' {
            in_bracket = true;
            out.push(c);
            i += 1;
            if raw.get(i) == Some(&b'^') {
                out.push(b'^');
                i += 1;
            }
            bracket_start = i;
            if raw.get(i) == Some(&b']') {
                out.push(b']');
                i += 1;
            }
            continue;
        }
        if c == b'\\' && i + 1 < raw.len() {
            let d = raw[i + 1];
            let simple = match d {
                b'n' => Some(b'\n'),
                b't' => Some(b'\t'),
                b'r' => Some(b'\r'),
                b'f' => Some(0x0c),
                b'v' => Some(0x0b),
                b'a' => Some(0x07),
                _ => None,
            };
            if let Some(b) = simple {
                out.push(b);
                i += 2;
                continue;
            }
            if let Some((b, len)) = numeric_escape(&raw[i + 1..]) {
                high |= b >= 0x80;
                out.push(b);
                i += 1 + len;
                continue;
            }
            if in_bracket {
                // A backslash is literal in a bracket expression.
                out.push(c);
                i += 1;
                continue;
            }
            out.push(c);
            out.push(d);
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    (out, high)
}

/// The parts of a replacement.
fn replacement(raw: &[u8]) -> Vec<Repl> {
    let mut parts = Vec::new();
    let mut lit = Vec::new();
    let flush = |lit: &mut Vec<u8>, parts: &mut Vec<Repl>| {
        if !lit.is_empty() {
            parts.push(Repl::Lit(std::mem::take(lit)));
        }
    };
    let mut i = 0;
    while i < raw.len() {
        let c = raw[i];
        if c == b'&' {
            flush(&mut lit, &mut parts);
            parts.push(Repl::Group(0));
            i += 1;
            continue;
        }
        if c != b'\\' || i + 1 >= raw.len() {
            lit.push(c);
            i += 1;
            continue;
        }
        let d = raw[i + 1];
        i += 2;
        match d {
            b'0'..=b'9' => {
                flush(&mut lit, &mut parts);
                parts.push(Repl::Group(usize::from(d - b'0')));
            }
            b'L' | b'U' | b'l' | b'u' | b'E' => {
                flush(&mut lit, &mut parts);
                parts.push(Repl::Case(match d {
                    b'L' => CaseOp::Lower,
                    b'U' => CaseOp::Upper,
                    b'l' => CaseOp::LowerOne,
                    b'u' => CaseOp::UpperOne,
                    _ => CaseOp::End,
                }));
            }
            b'n' => lit.push(b'\n'),
            b't' => lit.push(b'\t'),
            b'r' => lit.push(b'\r'),
            b'f' => lit.push(0x0c),
            b'v' => lit.push(0x0b),
            b'a' => lit.push(0x07),
            _ => {
                if let Some((b, len)) = numeric_escape(&raw[i - 1..]) {
                    lit.push(b);
                    i += len - 1;
                } else {
                    lit.push(d);
                }
            }
        }
    }
    flush(&mut lit, &mut parts);
    parts
}
