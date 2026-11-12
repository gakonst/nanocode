//! Running a compiled sed program over its input (D1140): the cycle, addresses and
//! ranges, the commands, and GNU's output rules (a last line without a newline is
//! printed without one, and the newline is supplied before any later output).

use std::io;
use std::rc::Rc;

use psh_engine::host::{OpenOptions, error_text};
use psh_engine::util::{ShellIo, ShellStdio, UtilCtx, UtilFile};
use psh_regex::{ExecOptions, Regex};

use super::compile::{Addr, Addr2, CaseOp, Kind, Program, Repl, Subst, char_len};
use crate::common::Input;

pub struct Settings {
    pub quiet: bool,
    /// `-z`: lines end in NUL.
    pub zero: bool,
    /// `-l N`: the wrap width of `l`.
    pub line_wrap: usize,
    pub utf8: bool,
}

/// Where the main output goes: stdout, or the new contents of a file edited in place.
pub enum Sink {
    Stdout,
    Buffer(Vec<u8>),
}

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
pub enum End {
    /// The input is used up.
    Input,
    /// `q` or `Q` with this status.
    Quit(i32),
    /// A fatal error (status 4) or a closed stdout (the status to exit with).
    Fail(i32),
}

/// The lines of one or more input files, read as needed.
pub struct Reader {
    names: Vec<Vec<u8>>,
    next: usize,
    /// The name of the file being read, shared by its lines.
    name: Rc<[u8]>,
    cur: Option<Input>,
    buf: Vec<u8>,
    start: usize,
    eof: bool,
    delim: u8,
    /// The status for unreadable files (2).
    pub status: i32,
    /// A read error that ends sed (status 4).
    pub fatal: bool,
}

pub struct Line {
    pub text: Vec<u8>,
    /// It ended in the delimiter (or more input follows it).
    pub nl: bool,
    pub name: Rc<[u8]>,
}

impl Reader {
    pub fn new(names: Vec<Vec<u8>>, delim: u8) -> Reader {
        Reader {
            names,
            next: 0,
            name: Rc::from(&b"-"[..]),
            cur: None,
            buf: Vec::new(),
            start: 0,
            eof: true,
            delim,
            status: 0,
            fatal: false,
        }
    }

    fn cur_name(&self) -> Vec<u8> {
        self.names
            .get(self.next.wrapping_sub(1))
            .cloned()
            .unwrap_or_else(|| b"-".to_vec())
    }

    /// Makes sure there is unread data, opening the next files as needed.
    async fn fill(&mut self, ctx: &mut UtilCtx<'_>) -> bool {
        loop {
            if self.start < self.buf.len() {
                return true;
            }
            if !self.eof {
                self.buf.clear();
                self.start = 0;
                let mut chunk = vec![0u8; 64 * 1024];
                let r = match &self.cur {
                    Some(input) => input.read(ctx, &mut chunk).await,
                    None => Ok(0),
                };
                match r {
                    Ok(0) => self.eof = true,
                    Ok(n) => {
                        self.buf.extend_from_slice(&chunk[..n]);
                        continue;
                    }
                    Err(e) => {
                        let name = String::from_utf8_lossy(&self.cur_name()).into_owned();
                        ctx.error(&format!("read error on {name}: {}", error_text(&e)))
                            .await;
                        self.fatal = true;
                        self.eof = true;
                        return false;
                    }
                }
                continue;
            }
            // The next file.
            if self.fatal || self.next >= self.names.len() {
                return false;
            }
            let name = self.names[self.next].clone();
            self.next += 1;
            self.name = Rc::from(name.as_slice());
            self.cur = None;
            match Input::open(ctx, &name).await {
                Ok(input) => {
                    self.cur = Some(input);
                    self.eof = false;
                }
                Err(e) if e.kind() == io::ErrorKind::IsADirectory => {
                    let shown = String::from_utf8_lossy(&name).into_owned();
                    ctx.error(&format!("read error on {shown}: Is a directory"))
                        .await;
                    self.fatal = true;
                    return false;
                }
                Err(e) => {
                    let shown = String::from_utf8_lossy(&name).into_owned();
                    ctx.error(&format!("can't read {shown}: {}", error_text(&e)))
                        .await;
                    self.status = 2;
                }
            }
        }
    }

    /// The next line, in `text` (a buffer to reuse; cleared first).
    pub async fn next_line(&mut self, ctx: &mut UtilCtx<'_>, mut text: Vec<u8>) -> Option<Line> {
        if !self.fill(ctx).await {
            return None;
        }
        let name = self.name.clone();
        text.clear();
        loop {
            let rest = &self.buf[self.start..];
            if let Some(i) = memchr::memchr(self.delim, rest) {
                text.extend_from_slice(&rest[..i]);
                self.start += i + 1;
                return Some(Line {
                    text,
                    nl: true,
                    name,
                });
            }
            text.extend_from_slice(rest);
            self.start = self.buf.len();
            if self.eof {
                break;
            }
            // More of this file.
            if !self.fill_same(ctx).await {
                break;
            }
        }
        // The file ended without a delimiter: GNU supplies it when more input follows.
        let nl = self.fill(ctx).await;
        Some(Line { text, nl, name })
    }

    fn buffered_line(&mut self) -> Option<&[u8]> {
        let start = self.start;
        let i = memchr::memchr(self.delim, &self.buf[start..])?;
        self.start += i + 1;
        Some(&self.buf[start..start + i])
    }

    /// Reads more of the current file only.
    async fn fill_same(&mut self, ctx: &mut UtilCtx<'_>) -> bool {
        if self.eof {
            return false;
        }
        let mut chunk = vec![0u8; 64 * 1024];
        let r = match &self.cur {
            Some(input) => input.read(ctx, &mut chunk).await,
            None => Ok(0),
        };
        match r {
            Ok(0) => {
                self.eof = true;
                false
            }
            Ok(n) => {
                self.buf.clear();
                self.start = 0;
                self.buf.extend_from_slice(&chunk[..n]);
                true
            }
            Err(e) => {
                let name = String::from_utf8_lossy(&self.cur_name()).into_owned();
                ctx.error(&format!("read error on {name}: {}", error_text(&e)))
                    .await;
                self.fatal = true;
                self.eof = true;
                false
            }
        }
    }

    /// Whether no line follows.
    pub async fn at_end(&mut self, ctx: &mut UtilCtx<'_>) -> bool {
        !self.fill(ctx).await
    }
}

/// An output: the main one or a `w` file.
#[derive(Default)]
struct Out {
    buf: Vec<u8>,
    /// The last line written lacked its newline: supply it before more output.
    missing: bool,
}

enum Append {
    Text(Vec<u8>),
    File(Vec<u8>),
    Line(Vec<u8>),
}

#[derive(Clone, Copy, Default)]
struct RangeState {
    active: bool,
    end: u64,
    /// The range ended on this line (for `c`).
    closed_now: bool,
}

/// The persistent part of a run: hold space, `w`/`R` files, the last regex.
pub struct State {
    hold: Vec<u8>,
    hold_nl: bool,
    last_re: Option<Rc<Regex>>,
    wfiles: Vec<Option<(Option<UtilFile>, Out)>>,
    rfiles: Vec<Option<RFile>>,
}

struct RFile {
    file: Option<Input>,
    buf: Vec<u8>,
    done: bool,
}

impl State {
    /// Opens (truncating) the `w` files, as GNU does before reading any input.
    pub async fn new(ctx: &mut UtilCtx<'_>, prog: &Program) -> Result<State, i32> {
        let mut wfiles = Vec::new();
        for name in &prog.wfiles {
            if name == b"/dev/stdout" || name == b"/dev/stderr" {
                wfiles.push(Some((None, Out::default())));
                continue;
            }
            match ctx.open(name, OpenOptions::write()).await {
                Ok(f) => wfiles.push(Some((Some(f), Out::default()))),
                Err(e) => {
                    let shown = String::from_utf8_lossy(name).into_owned();
                    ctx.error(&format!("couldn't open file {shown}: {}", error_text(&e)))
                        .await;
                    return Err(4);
                }
            }
        }
        Ok(State {
            hold: Vec::new(),
            hold_nl: true,
            last_re: None,
            wfiles,
            rfiles: (0..prog.rfiles.len()).map(|_| None).collect(),
        })
    }

    /// Writes what the `w` files have buffered once it is much (stderr: always).
    async fn drain(&mut self, ctx: &mut UtilCtx<'_>, prog: &Program) -> Result<(), i32> {
        let much = self.wfiles.iter().any(|w| {
            w.as_ref().is_some_and(|(f, o)| {
                o.buf.len() >= 64 * 1024 || (f.is_none() && !o.buf.is_empty())
            })
        });
        if !much {
            return Ok(());
        }
        self.flush(ctx, prog).await
    }

    /// Writes what the `w` files have buffered.
    pub async fn flush(&mut self, ctx: &mut UtilCtx<'_>, prog: &Program) -> Result<(), i32> {
        for (i, w) in self.wfiles.iter_mut().enumerate() {
            let Some((file, out)) = w else { continue };
            if out.buf.is_empty() {
                continue;
            }
            let data = std::mem::take(&mut out.buf);
            let r = match file {
                Some(f) => f.write_all(&data).await,
                None if prog.wfiles[i] == b"/dev/stderr" => {
                    ctx.error_raw(&data).await;
                    Ok(())
                }
                None => ctx.write(&data).await,
            };
            if let Err(e) = r {
                let shown = String::from_utf8_lossy(&prog.wfiles[i]).into_owned();
                if file.is_none() {
                    return Err(crate::common::write_failed(ctx, &e).await);
                }
                ctx.error(&format!("couldn't write to {shown}: {}", error_text(&e)))
                    .await;
                return Err(4);
            }
        }
        Ok(())
    }

    /// `-s`/`-i`: GNU starts each input file with an empty hold space and rewinds the
    /// `R` files.
    pub fn rewind(&mut self) {
        self.hold.clear();
        self.hold_nl = true;
        for r in &mut self.rfiles {
            *r = None;
        }
    }
}

/// One run over a stream of input (all files, or one with `-s`/`-i`).
pub struct Run<'p> {
    prog: &'p Program,
    set: &'p Settings,
    sink: Sink,
    out: Out,
    ps: Vec<u8>,
    ps_nl: bool,
    line_no: u64,
    line_name: Rc<[u8]>,
    /// The last pattern space's buffer, for the next line.
    spare: Vec<u8>,
    /// The last substitution's buffer, for the next one.
    scratch: Vec<u8>,
    tflag: bool,
    appends: Vec<Append>,
    ranges: Vec<RangeState>,
}

/// What a command did to the cycle.
enum Flow {
    Next,
    Jump(usize),
    /// End the cycle: autoprint or not; then read the next line or restart (`D`).
    EndCycle {
        print: bool,
        restart: bool,
    },
    /// End the cycle (with autoprint) and stop.
    Quit(i32),
    /// Stop now (`Q`, or no input for `n`/`N` after the cycle's end).
    QuitNow(i32),
}

impl<'p> Run<'p> {
    pub fn new(prog: &'p Program, set: &'p Settings, sink: Sink) -> Run<'p> {
        Run {
            prog,
            set,
            sink,
            out: Out::default(),
            ps: Vec::new(),
            ps_nl: true,
            line_no: 0,
            line_name: Rc::from(&b""[..]),
            spare: Vec::new(),
            scratch: Vec::new(),
            tflag: false,
            appends: Vec::new(),
            ranges: vec![RangeState::default(); prog.cmds.len()],
        }
    }

    /// Replaces where the main output goes, returning the old sink.
    pub fn set_sink(&mut self, sink: Sink) -> Sink {
        std::mem::replace(&mut self.sink, sink)
    }

    fn delim(&self) -> u8 {
        if self.set.zero { 0 } else { b'\n' }
    }

    /// Runs the program over `reader`. Line numbers continue from `first_line`.
    pub async fn run(&mut self, ctx: &mut UtilCtx<'_>, st: &mut State, reader: &mut Reader) -> End {
        let mut restart = false;
        let mut steps: u32 = 0;
        let plain = self.plain_substs();
        loop {
            if !restart && plain {
                if let Err(s) = self.batch(st, reader) {
                    return End::Fail(s);
                }
                if let Err(s) = self.drain(ctx).await {
                    return End::Fail(s);
                }
                ctx.check().await;
            }
            if !restart {
                let spare = std::mem::take(&mut self.spare);
                let Some(line) = reader.next_line(ctx, spare).await else {
                    break;
                };
                self.load(line);
            }
            let mut pc = 0;
            let flow = loop {
                if pc >= self.prog.cmds.len() {
                    break Flow::EndCycle {
                        print: true,
                        restart: false,
                    };
                }
                steps = steps.wrapping_add(1);
                if steps.is_multiple_of(256) {
                    ctx.check().await;
                }
                let cmd = &self.prog.cmds[pc];
                if !self.selected(ctx, st, reader, pc).await {
                    pc = match cmd.kind {
                        Kind::Open(close) => close + 1,
                        _ => pc + 1,
                    };
                    continue;
                }
                match self.exec(ctx, st, reader, pc).await {
                    Ok(Flow::Next) => pc += 1,
                    Ok(Flow::Jump(t)) => pc = t,
                    Ok(f) => break f,
                    Err(status) => return End::Fail(status),
                }
            };
            let r = match flow {
                Flow::EndCycle { print, restart: r } => {
                    restart = r;
                    // `D` keeps the queued text for the cycle it restarts.
                    self.end_cycle(ctx, st, print && !r, !r).await
                }
                Flow::Quit(code) => {
                    if let Err(s) = self.end_cycle(ctx, st, true, true).await {
                        return End::Fail(s);
                    }
                    return End::Quit(code);
                }
                Flow::QuitNow(code) => {
                    self.appends.clear();
                    return End::Quit(code);
                }
                Flow::Next | Flow::Jump(_) => unreachable!(),
            };
            if let Err(s) = r {
                return End::Fail(s);
            }
            if reader.fatal {
                return End::Fail(4);
            }
        }
        if reader.fatal {
            return End::Fail(4);
        }
        End::Input
    }

    fn plain_substs(&self) -> bool {
        self.prog.cmds.iter().all(|c| {
            c.a1.is_none()
                && c.a2.is_none()
                && !c.a1_zero
                && !c.neg
                && matches!(&c.kind, Kind::Subst(s) if s.re.is_some() && s.write.is_none() && !s.eval)
        })
    }

    fn batch(&mut self, st: &mut State, reader: &mut Reader) -> Result<(), i32> {
        let prog = self.prog;
        let delim = self.delim();
        if let [cmd] = prog.cmds.as_slice()
            && let Kind::Subst(s) = &cmd.kind
            && let Some(re) = &s.re
            && !s.print
            && !self.set.quiet
            && !self.out.missing
        {
            st.last_re = Some(re.clone());
            let finder = re.literal().map(memchr::memmem::Finder::new);
            while self.out.buf.len() < 64 * 1024
                && let Some(line) = reader.buffered_line()
            {
                self.line_no += 1;
                if !replace_into(
                    re,
                    finder.as_ref(),
                    s,
                    line,
                    self.set.utf8,
                    &mut self.out.buf,
                ) {
                    self.out.buf.extend_from_slice(line);
                }
                self.out.buf.push(delim);
            }
            return Ok(());
        }
        while self.out.buf.len() < 64 * 1024
            && let Some(line) = reader.buffered_line()
        {
            self.ps.clear();
            self.ps.extend_from_slice(line);
            self.ps_nl = true;
            self.line_no += 1;
            self.tflag = false;
            if !Rc::ptr_eq(&self.line_name, &reader.name) {
                self.line_name = reader.name.clone();
            }
            for cmd in &prog.cmds {
                if let Kind::Subst(s) = &cmd.kind
                    && self.subst(st, s)?
                {
                    self.tflag = true;
                    if s.print {
                        let ps = std::mem::take(&mut self.ps);
                        self.emit_line(&ps, true);
                        self.ps = ps;
                    }
                }
            }
            if !self.set.quiet {
                let ps = std::mem::take(&mut self.ps);
                self.emit_line(&ps, true);
                self.ps = ps;
            }
        }
        Ok(())
    }

    fn load(&mut self, line: Line) {
        self.spare = std::mem::replace(&mut self.ps, line.text);
        self.ps_nl = line.nl;
        self.line_name = line.name;
        self.line_no += 1;
        self.tflag = false;
    }

    async fn end_cycle(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        st: &mut State,
        print: bool,
        appends: bool,
    ) -> Result<(), i32> {
        if print && !self.set.quiet {
            let ps = std::mem::take(&mut self.ps);
            let nl = self.ps_nl;
            self.emit_line(&ps, nl);
            self.ps = ps;
        }
        if appends {
            self.flush_appends(ctx).await;
        }
        self.drain(ctx).await?;
        st.drain(ctx, self.prog).await
    }

    /// Writes the main output that has piled up.
    async fn drain(&mut self, ctx: &mut UtilCtx<'_>) -> Result<(), i32> {
        if self.out.buf.len() < 64 * 1024 && !matches!(self.sink, Sink::Buffer(_)) {
            return Ok(());
        }
        self.flush(ctx).await
    }

    pub async fn flush(&mut self, ctx: &mut UtilCtx<'_>) -> Result<(), i32> {
        let data = std::mem::take(&mut self.out.buf);
        match &mut self.sink {
            Sink::Buffer(b) => b.extend_from_slice(&data),
            Sink::Stdout => {
                if let Err(e) = ctx.write(&data).await {
                    return Err(crate::common::write_failed(ctx, &e).await);
                }
            }
        }
        Ok(())
    }

    /// A line of the pattern space: with its newline unless the input lacked it.
    fn emit_line(&mut self, text: &[u8], nl: bool) {
        let d = self.delim();
        emit(&mut self.out, text, nl, d);
    }

    /// Text that ends in its own newline (`i`, `=`, `l`, `F`).
    fn emit_text(&mut self, text: &[u8]) {
        let d = self.delim();
        if self.out.missing {
            self.out.buf.push(d);
            self.out.missing = false;
        }
        self.out.buf.extend_from_slice(text);
    }

    async fn flush_appends(&mut self, ctx: &mut UtilCtx<'_>) {
        for a in std::mem::take(&mut self.appends) {
            match a {
                Append::Text(t) => self.emit_text(&t),
                Append::Line(t) => self.emit_text(&t),
                Append::File(name) => {
                    let mut data = Vec::new();
                    let ok = if name == b"/dev/stdin" {
                        ctx.read_to_end(&mut data).await.is_ok()
                    } else {
                        match ctx.open(&name, OpenOptions::read()).await {
                            Ok(f) => f.read_to_end(&mut data).await.is_ok(),
                            Err(_) => false,
                        }
                    };
                    if ok && !data.is_empty() {
                        self.emit_text(&data);
                    }
                }
            }
        }
    }

    async fn is_last(&mut self, ctx: &mut UtilCtx<'_>, reader: &mut Reader) -> bool {
        reader.at_end(ctx).await
    }

    async fn re_match(&mut self, st: &mut State, re: &Option<Rc<Regex>>) -> Result<bool, i32> {
        let re = match re {
            Some(r) => {
                st.last_re = Some(r.clone());
                r.clone()
            }
            None => match &st.last_re {
                Some(r) => r.clone(),
                None => return Err(1),
            },
        };
        Ok(re.find(&self.ps, ExecOptions::default()).is_some())
    }

    async fn match_a1(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        st: &mut State,
        reader: &mut Reader,
        a: &Addr,
    ) -> bool {
        match a {
            Addr::Line(n) => self.line_no == *n,
            Addr::Last => self.is_last(ctx, reader).await,
            Addr::Re(re) => self.re_match(st, re).await.unwrap_or(false),
            Addr::Step(first, step) => {
                if *step == 0 {
                    self.line_no == *first
                } else {
                    self.line_no >= *first && (self.line_no - first).is_multiple_of(*step)
                }
            }
        }
    }

    async fn selected(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        st: &mut State,
        reader: &mut Reader,
        idx: usize,
    ) -> bool {
        let cmd = &self.prog.cmds[idx];
        let m = match (&cmd.a1, &cmd.a2) {
            (None, None) if !cmd.a1_zero => true,
            (Some(a1), None) => self.match_a1(ctx, st, reader, a1).await,
            (_, Some(a2)) => self.range(ctx, st, reader, idx, a2).await,
            (None, None) => false,
        };
        m != cmd.neg
    }

    async fn range(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        st: &mut State,
        reader: &mut Reader,
        idx: usize,
        a2: &Addr2,
    ) -> bool {
        let cmd = &self.prog.cmds[idx];
        let mut r = self.ranges[idx];
        r.closed_now = false;
        let n = self.line_no;
        let hit = if r.active {
            match a2 {
                Addr2::Line(_) | Addr2::Rel(_) | Addr2::Mult(_) => {
                    if n > r.end {
                        // Lines were consumed past the end (`N`): the range is over.
                        r.active = false;
                        false
                    } else {
                        if n == r.end {
                            r.active = false;
                            r.closed_now = true;
                        }
                        true
                    }
                }
                Addr2::Last => {
                    if self.is_last(ctx, reader).await {
                        r.active = false;
                        r.closed_now = true;
                    }
                    true
                }
                Addr2::Re(re) => {
                    if self.re_match(st, re).await.unwrap_or(false) {
                        r.active = false;
                        r.closed_now = true;
                    }
                    true
                }
            }
        } else {
            let start = if cmd.a1_zero {
                // `0,/re/`: active from before line 1, so line 1 may end it.
                if n == 1 {
                    r.active = true;
                    if let Addr2::Re(re) = a2
                        && self.re_match(st, re).await.unwrap_or(false)
                    {
                        r.active = false;
                        r.closed_now = true;
                    }
                    self.ranges[idx] = r;
                    return true;
                }
                false
            } else {
                match &cmd.a1 {
                    Some(a1) => self.match_a1(ctx, st, reader, a1).await,
                    None => false,
                }
            };
            if start {
                r.active = true;
                match a2 {
                    Addr2::Line(e) => {
                        r.end = *e;
                        if *e <= n {
                            r.active = false;
                        }
                    }
                    Addr2::Rel(k) => {
                        r.end = n + k;
                        if *k == 0 {
                            r.active = false;
                        }
                    }
                    Addr2::Mult(k) => {
                        if *k == 0 || n.is_multiple_of(*k) {
                            r.active = false;
                        } else {
                            r.end = n.div_ceil(*k) * k;
                        }
                    }
                    Addr2::Last => {
                        if self.is_last(ctx, reader).await {
                            r.active = false;
                        }
                    }
                    Addr2::Re(_) => {}
                }
                if !r.active {
                    r.closed_now = true;
                }
            }
            start
        };
        self.ranges[idx] = r;
        hit
    }

    /// Resets what does not carry over from one input file to the next (`-s`, `-i`).
    pub fn new_file(&mut self) {
        self.line_no = 0;
        for r in &mut self.ranges {
            *r = RangeState::default();
        }
    }

    async fn exec(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        st: &mut State,
        reader: &mut Reader,
        pc: usize,
    ) -> Result<Flow, i32> {
        let prog = self.prog;
        let cmd = &prog.cmds[pc];
        let nl = b'\n';
        Ok(match &cmd.kind {
            Kind::Open(_) | Kind::Close | Kind::Nop => Flow::Next,
            Kind::Eq => {
                let t = format!("{}\n", self.line_no);
                self.emit_text(t.as_bytes());
                Flow::Next
            }
            Kind::Append(t) => {
                self.appends.push(Append::Text(t.clone()));
                Flow::Next
            }
            Kind::Insert(t) => {
                self.emit_text(t);
                Flow::Next
            }
            Kind::Change(t) => {
                // With a range, the text replaces the whole range: printed at its end.
                if cmd.a2.is_none() || cmd.neg || self.ranges[pc].closed_now {
                    self.emit_text(t);
                }
                Flow::EndCycle {
                    print: false,
                    restart: false,
                }
            }
            Kind::Branch(t) => Flow::Jump(*t),
            Kind::BranchIf(t) => {
                if self.tflag {
                    self.tflag = false;
                    Flow::Jump(*t)
                } else {
                    Flow::Next
                }
            }
            Kind::BranchUnless(t) => {
                if self.tflag {
                    self.tflag = false;
                    Flow::Next
                } else {
                    Flow::Jump(*t)
                }
            }
            Kind::Delete => Flow::EndCycle {
                print: false,
                restart: false,
            },
            Kind::DeleteFirst => match self.ps.iter().position(|&b| b == nl) {
                None => Flow::EndCycle {
                    print: false,
                    restart: false,
                },
                Some(i) => {
                    self.ps.drain(..=i);
                    Flow::EndCycle {
                        print: false,
                        restart: true,
                    }
                }
            },
            Kind::Get => {
                self.ps = st.hold.clone();
                self.ps_nl = st.hold_nl;
                Flow::Next
            }
            Kind::GetAppend => {
                self.ps.push(nl);
                self.ps.extend_from_slice(&st.hold);
                self.ps_nl = st.hold_nl;
                Flow::Next
            }
            Kind::Hold => {
                st.hold = self.ps.clone();
                st.hold_nl = self.ps_nl;
                Flow::Next
            }
            Kind::HoldAppend => {
                st.hold.push(nl);
                st.hold.extend_from_slice(&self.ps);
                st.hold_nl = self.ps_nl;
                Flow::Next
            }
            Kind::Exchange => {
                std::mem::swap(&mut self.ps, &mut st.hold);
                std::mem::swap(&mut self.ps_nl, &mut st.hold_nl);
                Flow::Next
            }
            Kind::Zap => {
                self.ps.clear();
                Flow::Next
            }
            Kind::List(n) => {
                let width = n.unwrap_or(self.set.line_wrap);
                let t = list(&self.ps, width);
                self.emit_text(&t);
                Flow::Next
            }
            Kind::Next => {
                if self.is_last(ctx, reader).await {
                    // GNU: no next line ends sed, printing as at the end of a cycle.
                    return Ok(Flow::Quit(0));
                }
                if !self.set.quiet {
                    let ps = std::mem::take(&mut self.ps);
                    self.emit_line(&ps, self.ps_nl);
                }
                self.flush_appends(ctx).await;
                self.drain(ctx).await?;
                match reader.next_line(ctx, Vec::new()).await {
                    Some(line) => {
                        self.load(line);
                        Flow::Next
                    }
                    None => Flow::QuitNow(0),
                }
            }
            Kind::NextAppend => {
                if self.is_last(ctx, reader).await {
                    return Ok(Flow::Quit(0));
                }
                self.flush_appends(ctx).await;
                self.drain(ctx).await?;
                match reader.next_line(ctx, Vec::new()).await {
                    Some(line) => {
                        let mut ps = std::mem::take(&mut self.ps);
                        ps.push(nl);
                        ps.extend_from_slice(&line.text);
                        self.load(line);
                        self.ps = ps;
                        Flow::Next
                    }
                    None => Flow::Quit(0),
                }
            }
            Kind::Print => {
                let ps = std::mem::take(&mut self.ps);
                self.emit_line(&ps, self.ps_nl);
                self.ps = ps;
                Flow::Next
            }
            Kind::PrintFirst => {
                match self.ps.iter().position(|&b| b == nl) {
                    Some(i) => {
                        let first = self.ps[..i].to_vec();
                        self.emit_line(&first, true);
                    }
                    None => {
                        let ps = std::mem::take(&mut self.ps);
                        self.emit_line(&ps, self.ps_nl);
                        self.ps = ps;
                    }
                }
                Flow::Next
            }
            Kind::Quit(code) => Flow::Quit(*code),
            Kind::QuitSilent(code) => Flow::QuitNow(*code),
            Kind::Read(name) => {
                self.appends.push(Append::File(name.clone()));
                Flow::Next
            }
            Kind::ReadLine(i) => {
                if let Some(line) = read_rline(ctx, st, &prog.rfiles[*i], *i).await {
                    self.appends.push(Append::Line(line));
                }
                Flow::Next
            }
            Kind::Write(i) => {
                let ps = std::mem::take(&mut self.ps);
                self.write_file(st, *i, &ps, self.ps_nl);
                self.ps = ps;
                Flow::Next
            }
            Kind::WriteFirst(i) => {
                let first = match self.ps.iter().position(|&b| b == nl) {
                    Some(k) => self.ps[..k].to_vec(),
                    None => self.ps.clone(),
                };
                self.write_file(st, *i, &first, true);
                Flow::Next
            }
            Kind::Subst(s) => {
                if self.subst(st, s)? {
                    self.tflag = true;
                    if s.eval {
                        let ps = std::mem::take(&mut self.ps);
                        self.ps = shell_output(ctx, &ps, true).await;
                    }
                    if s.print {
                        let ps = std::mem::take(&mut self.ps);
                        self.emit_line(&ps, self.ps_nl);
                        self.ps = ps;
                    }
                    if let Some(i) = s.write {
                        let ps = std::mem::take(&mut self.ps);
                        self.write_file(st, i, &ps, self.ps_nl);
                        self.ps = ps;
                    }
                }
                Flow::Next
            }
            Kind::Exec(Some(cmd)) => {
                let out = shell_output(ctx, cmd, false).await;
                if !out.is_empty() {
                    self.emit_text(&out);
                }
                Flow::Next
            }
            Kind::Exec(None) => {
                let ps = std::mem::take(&mut self.ps);
                self.ps = shell_output(ctx, &ps, true).await;
                Flow::Next
            }
            Kind::Translate(map) => {
                self.ps = translate(&self.ps, map, self.set.utf8);
                Flow::Next
            }
            Kind::FileName => {
                let mut t = self.line_name.to_vec();
                t.push(b'\n');
                self.emit_text(&t);
                Flow::Next
            }
        })
    }

    /// `w FILE`: `/dev/stdout` is the main output unless that is a file edited in place.
    fn write_file(&mut self, st: &mut State, i: usize, text: &[u8], nl: bool) {
        let d = self.delim();
        if self.prog.wfiles[i] == b"/dev/stdout" && matches!(self.sink, Sink::Stdout) {
            emit(&mut self.out, text, nl, d);
        } else if let Some((_, out)) = &mut st.wfiles[i] {
            emit(out, text, nl, d);
        }
    }

    fn subst(&mut self, st: &mut State, s: &Subst) -> Result<bool, i32> {
        let re = match &s.re {
            Some(r) => {
                st.last_re = Some(r.clone());
                r.clone()
            }
            None => match &st.last_re {
                Some(r) => r.clone(),
                None => return Err(1),
            },
        };
        let mut out = std::mem::take(&mut self.scratch);
        out.clear();
        if replace_into(&re, None, s, &self.ps, self.set.utf8, &mut out) {
            self.scratch = std::mem::replace(&mut self.ps, out);
            Ok(true)
        } else {
            self.scratch = out;
            Ok(false)
        }
    }
}

/// `e`: runs `script` in a fresh shell (`sh -c`, empty stdin) and returns what it
/// printed, without its last newline when it replaces the pattern space.
async fn shell_output(ctx: &mut UtilCtx<'_>, script: &[u8], chomp: bool) -> Vec<u8> {
    let stdio = ShellStdio {
        stdin: ShellIo::Pipe,
        stdout: ShellIo::Pipe,
    };
    let mut child = ctx.spawn_shell(script, stdio).await;
    child.stdin = None;
    let mut out = Vec::new();
    if let Some(f) = child.stdout.take() {
        let _ = f.read_to_end(&mut out).await;
    }
    child.wait().await;
    if chomp && out.last() == Some(&b'\n') {
        out.pop();
    }
    out
}

fn replace_into(
    re: &Regex,
    finder: Option<&memchr::memmem::Finder>,
    s: &Subst,
    ps: &[u8],
    utf8: bool,
    out: &mut Vec<u8>,
) -> bool {
    if let Some(finder) = finder {
        let lit = finder.needle();
        let mut pos = 0;
        let mut count = 0;
        let mut did = false;
        for s0 in finder.find_iter(ps) {
            count += 1;
            if count >= s.nth {
                let e0 = s0 + lit.len();
                out.extend_from_slice(&ps[pos..s0]);
                expand(out, &s.repl, ps, &[Some((s0, e0))], utf8);
                pos = e0;
                did = true;
                if !s.global {
                    break;
                }
            }
        }
        if did {
            out.extend_from_slice(&ps[pos..]);
        }
        return did;
    }
    let mut pos = 0;
    let mut from = 0;
    let mut count = 0;
    let mut prev_end: Option<usize> = None;
    let mut did = false;
    // Only a replacement that names a subexpression needs their positions.
    let grouped = s.repl.iter().any(|r| matches!(r, Repl::Group(n) if *n > 0));
    let mut whole = [None];
    while from <= ps.len() {
        let held;
        let caps: &[Option<(usize, usize)>] = if grouped {
            let Some(found) = re.exec_at(ps, from, ExecOptions::default()) else {
                break;
            };
            held = found;
            &held
        } else {
            let Some(found) = re.find_at(ps, from, ExecOptions::default()) else {
                break;
            };
            whole[0] = Some(found);
            &whole
        };
        let (s0, e0) = caps[0].unwrap_or((from, from));
        let step = |at: usize| if utf8 { char_len(&ps[at..]) } else { 1 };
        if s0 == e0 && prev_end == Some(s0) {
            // An empty match right after a match does not count.
            if s0 >= ps.len() {
                break;
            }
            from = s0 + step(s0);
            continue;
        }
        count += 1;
        if count >= s.nth {
            out.extend_from_slice(&ps[pos..s0]);
            expand(out, &s.repl, ps, caps, utf8);
            pos = e0;
            did = true;
            if !s.global {
                break;
            }
        }
        prev_end = Some(e0);
        from = if s0 == e0 {
            if e0 >= ps.len() {
                break;
            }
            e0 + step(e0)
        } else {
            e0
        };
    }
    if did {
        out.extend_from_slice(&ps[pos..]);
    }
    did
}

fn emit(out: &mut Out, text: &[u8], nl: bool, delim: u8) {
    if out.missing {
        out.buf.push(delim);
        out.missing = false;
    }
    out.buf.extend_from_slice(text);
    if nl {
        out.buf.push(delim);
    } else {
        out.missing = true;
    }
}

/// The next line of an `R` file, with its newline.
async fn read_rline(
    ctx: &mut UtilCtx<'_>,
    st: &mut State,
    name: &[u8],
    i: usize,
) -> Option<Vec<u8>> {
    if st.rfiles[i].is_none() {
        let file = if name == b"/dev/stdin" {
            Some(Input::Stdin)
        } else {
            ctx.open(name, OpenOptions::read())
                .await
                .ok()
                .map(Input::File)
        };
        st.rfiles[i] = Some(RFile {
            done: file.is_none(),
            file,
            buf: Vec::new(),
        });
    }
    let r = st.rfiles[i].as_mut()?;
    loop {
        if let Some(k) = r.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = r.buf.drain(..=k).collect();
            return Some(line);
        }
        if r.done {
            if r.buf.is_empty() {
                return None;
            }
            return Some(std::mem::take(&mut r.buf));
        }
        let mut chunk = vec![0u8; 16 * 1024];
        let n = match &r.file {
            Some(f) => f.read(ctx, &mut chunk).await.unwrap_or(0),
            None => 0,
        };
        if n == 0 {
            r.done = true;
        } else {
            r.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

/// Appends the replacement for one match, applying `\U \L \u \l \E`.
fn expand(
    out: &mut Vec<u8>,
    repl: &[Repl],
    text: &[u8],
    caps: &[Option<(usize, usize)>],
    utf8: bool,
) {
    let mut mode: Option<CaseOp> = None;
    let mut once: Option<CaseOp> = None;
    let put = |out: &mut Vec<u8>, s: &[u8], mode: Option<CaseOp>, once: &mut Option<CaseOp>| {
        if s.is_empty() {
            return;
        }
        let mut rest = s;
        if let Some(op) = once.take() {
            let n = if utf8 { char_len(rest) } else { 1 };
            let (first, tail) = rest.split_at(n);
            out.extend(case(first, op == CaseOp::UpperOne, utf8));
            rest = tail;
        }
        match mode {
            Some(CaseOp::Upper) => out.extend(case(rest, true, utf8)),
            Some(CaseOp::Lower) => out.extend(case(rest, false, utf8)),
            _ => out.extend_from_slice(rest),
        }
    };
    for part in repl {
        match part {
            Repl::Lit(s) => put(out, s, mode, &mut once),
            Repl::Group(n) => {
                if let Some(Some((a, b))) = caps.get(*n) {
                    put(out, &text[*a..*b], mode, &mut once);
                }
            }
            Repl::Case(op) => match op {
                CaseOp::Upper | CaseOp::Lower => {
                    mode = Some(*op);
                    once = None;
                }
                CaseOp::End => {
                    mode = None;
                    once = None;
                }
                CaseOp::UpperOne | CaseOp::LowerOne => once = Some(*op),
            },
        }
    }
}

fn case(s: &[u8], upper: bool, utf8: bool) -> Vec<u8> {
    if utf8 && let Ok(t) = std::str::from_utf8(s) {
        let t = if upper {
            t.to_uppercase()
        } else {
            t.to_lowercase()
        };
        return t.into_bytes();
    }
    if upper {
        s.to_ascii_uppercase()
    } else {
        s.to_ascii_lowercase()
    }
}

fn translate(ps: &[u8], map: &[(Vec<u8>, Vec<u8>)], utf8: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(ps.len());
    let mut i = 0;
    while i < ps.len() {
        let n = if utf8 { char_len(&ps[i..]) } else { 1 };
        let c = &ps[i..i + n];
        match map.iter().find(|(f, _)| f.as_slice() == c) {
            Some((_, t)) => out.extend_from_slice(t),
            None => out.extend_from_slice(c),
        }
        i += n;
    }
    out
}

/// `l`: the pattern space unambiguously, wrapped at `width` (0: never).
fn list(ps: &[u8], width: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let max = width.saturating_sub(1);
    let mut col = 0;
    for &b in ps {
        let tok: Vec<u8> = match b {
            b'\\' => b"\\\\".to_vec(),
            0x07 => b"\\a".to_vec(),
            0x08 => b"\\b".to_vec(),
            0x0c => b"\\f".to_vec(),
            b'\n' => b"\\n".to_vec(),
            b'\r' => b"\\r".to_vec(),
            b'\t' => b"\\t".to_vec(),
            0x0b => b"\\v".to_vec(),
            0x20..=0x7e => vec![b],
            _ => format!("\\{b:03o}").into_bytes(),
        };
        if width > 0 && col + tok.len() > max {
            out.extend_from_slice(b"\\\n");
            col = 0;
        }
        col += tok.len();
        out.extend_from_slice(&tok);
    }
    out.extend_from_slice(b"$\n");
    out
}
