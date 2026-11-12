// The machine follows GoAWK's interpreter (interp/vm.go, interp/interp.go,
// interp/io.go, interp/functions.go), Copyright (c) 2022 Ben Hoyt, MIT License (see
// NOTICE), restructured: one async loop with explicit call frames and a key-snapshot
// stack for `for (k in a)`, so no awk construct recurses on the Rust stack.

//! The awk machine: runs a compiled [`Program`] over the input.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::rc::Rc;

use psh_engine::host::{OpenOptions, error_text};
use psh_engine::util::{ShellIo, ShellStdio, UtilCtx};
use psh_regex::{ExecOptions, Regex};

use super::array::{ArrRc, Array};
use super::ast::GetlineSrc;
use super::compile::{Arr, CodeId, Op, Program, Redir, SplitSep, Target, Var};
use super::io::{OutKind, Output, RecordReader, Rs, Src, find_nonempty};
use super::lexer::Builtin;
use super::printf::{self, Env};
use super::resolve::Special;
use super::value::{S, Value, int_str, is_integral, parse_prefix, special_str};

/// How the record loop or a block ends early.
pub enum Flow {
    Next,
    Nextfile,
    Exit,
    /// A fatal error: the message (status 2).
    Fatal(String),
    /// One gawk reports without the source line and input position.
    FatalPlain(String),
}

type R<T> = Result<T, Flow>;

enum Step {
    Done,
    Check,
    Flush,
    Wait,
}

fn fatal<T>(msg: impl Into<String>) -> R<T> {
    Err(Flow::Fatal(msg.into()))
}

/// How FS splits a record.
pub enum Fs {
    /// " ": runs of blanks and newlines, leading and trailing ones ignored.
    Space,
    /// One character, literally.
    Char(Vec<u8>),
    /// "": every character a field (gawk).
    Empty,
    Regex(Rc<Regex>),
}

struct Frame {
    code: CodeId,
    ip: usize,
    /// Where its scalar locals start on the stack.
    base: usize,
    arrays: Vec<ArrRc>,
    /// The for-in snapshots below this frame's.
    for_in_base: usize,
}

struct ForIn {
    keys: Vec<S>,
    next: usize,
}

/// Instructions between safepoints.
const CHECK_EVERY: u32 = 4096;
/// The limit of nested user function calls.
const MAX_CALL_DEPTH: usize = 10_000;
/// The largest field index or NF.
const MAX_FIELD: f64 = 10_000_000.0;

pub struct Interp<'c, 'v> {
    pub ctx: &'c mut UtilCtx<'v>,
    prog: Rc<Program>,
    pub utf8: bool,
    stack: Vec<Value>,
    pub globals: Vec<Value>,
    pub arrays: Vec<ArrRc>,
    frames: Vec<Frame>,
    for_in: Vec<ForIn>,
    steps: u32,

    // The record: its bytes, and the text of them a value shares, made when the
    // program first reads $0 (or a field that is all of it) as a value.
    rec: Vec<u8>,
    line: Option<S>,
    /// The record read last, before it becomes $0 or a getline variable's value.
    input: Vec<u8>,
    /// What `print` wrote last: its buffer serves the next `print`.
    printed: Vec<u8>,
    /// The emptied argument list of the last builtin call, for the next.
    call_args: Vec<Value>,
    line_is_str: bool,
    fields: Vec<Field>,
    fields_ok: bool,
    nf: usize,
    /// FS as it was when the record was read (POSIX).
    record_fs: Rc<Fs>,

    // Special variables.
    pub nr: f64,
    pub fnr: f64,
    fs: S,
    fs_split: Rc<Fs>,
    ofs: S,
    ors: S,
    rs: S,
    rs_mode: Rs,
    rt: S,
    subsep: S,
    convfmt: S,
    ofmt: S,
    filename: Value,
    rstart: f64,
    rlength: f64,
    pub argc: f64,
    /// What the program assigned to NR, FNR, RSTART, RLENGTH, ARGC or NF, until awk
    /// changes the variable itself: `NR = "x"; print NR` prints `x`.
    assigned: [Option<Value>; 6],

    // Input and output.
    main: Option<RecordReader>,
    /// The next ARGV index to look at.
    argv_index: usize,
    had_files: bool,
    main_done: bool,
    files_in: HashMap<Vec<u8>, RecordReader>,
    cmds_in: HashMap<Vec<u8>, RecordReader>,
    outputs: HashMap<Vec<u8>, Output>,

    pub exit_status: i32,
    /// The line of the statement running, and where the program's lines come from
    /// (first line, `cmd. line` or a file name), for fatal errors.
    src_line: u32,
    pub places: Vec<(u32, String)>,
    regex_cache: HashMap<Vec<u8>, Rc<Regex>>,
    fs_cache: Option<(S, Rc<Fs>)>,
    range_active: Vec<bool>,
    rand_state: u64,
    seed: f64,
}

fn s(b: &[u8]) -> S {
    Rc::from(b)
}

/// A field of the record: where it is in the line until the program reads it, so a
/// record costs a value only for the fields that are used.
#[derive(Clone)]
enum Field {
    At(usize, usize),
    Is(Value),
}

impl<'c, 'v> Interp<'c, 'v> {
    pub fn new(ctx: &'c mut UtilCtx<'v>, prog: Rc<Program>) -> Self {
        let utf8 = ctx.utf8();
        let globals = vec![Value::Uninit; prog.num_scalars];
        let arrays = (0..prog.num_arrays)
            .map(|_| Rc::new(RefCell::new(Array::default())))
            .collect();
        let n_actions = prog.actions.len();
        Interp {
            ctx,
            prog,
            utf8,
            stack: Vec::with_capacity(64),
            globals,
            arrays,
            frames: Vec::new(),
            for_in: Vec::new(),
            steps: 0,
            rec: Vec::new(),
            line: None,
            input: Vec::new(),
            printed: Vec::new(),
            call_args: Vec::new(),
            line_is_str: false,
            fields: Vec::new(),
            fields_ok: true,
            nf: 0,
            record_fs: Rc::new(Fs::Space),
            nr: 0.0,
            fnr: 0.0,
            fs: s(b" "),
            fs_split: Rc::new(Fs::Space),
            ofs: s(b" "),
            ors: s(b"\n"),
            rs: s(b"\n"),
            rs_mode: Rs::Byte(b'\n'),
            rt: s(b""),
            subsep: s(b"\x1c"),
            convfmt: s(b"%.6g"),
            ofmt: s(b"%.6g"),
            filename: Value::Uninit,
            rstart: 0.0,
            rlength: 0.0,
            argc: 0.0,
            assigned: Default::default(),
            main: None,
            argv_index: 1,
            had_files: false,
            main_done: false,
            files_in: HashMap::new(),
            cmds_in: HashMap::new(),
            outputs: HashMap::new(),
            exit_status: 0,
            src_line: 0,
            places: Vec::new(),
            regex_cache: HashMap::new(),
            fs_cache: None,
            range_active: vec![false; n_actions],
            rand_state: seed_state(0.0),
            seed: 0.0,
        }
    }

    // ---- Conversions.

    pub fn num_to_str(&self, n: f64, fmt: &S) -> S {
        if is_integral(n) {
            // What an i64 holds is written into a buffer on the stack: one allocation,
            // for the text itself.
            if n.abs() < 1e18 {
                let (mut digits, mut at) = ([0u8; 20], 20);
                let whole = n as i64;
                let mut left = whole.unsigned_abs();
                loop {
                    at -= 1;
                    digits[at] = b'0' + (left % 10) as u8;
                    left /= 10;
                    if left == 0 {
                        break;
                    }
                }
                if whole < 0 {
                    at -= 1;
                    digits[at] = b'-';
                }
                return s(&digits[at..]);
            }
            if n.is_finite() {
                return s(int_str(n).as_bytes());
            }
            return s(special_str(n).as_bytes());
        }
        if &**fmt == b"%.6g" {
            let mut out = Vec::new();
            printf::format_float(&mut out, &printf::Spec::default(), b'g', n);
            return Rc::from(out);
        }
        // `%s` in OFMT converts with CONVFMT; in CONVFMT itself it would recurse, and
        // gives "" as in gawk.
        let in_convfmt = Rc::ptr_eq(fmt, &self.convfmt) || **fmt == *self.convfmt;
        let to_str = |v: &Value| match v {
            Value::Num(x) if !in_convfmt => self.num_to_str(*x, &self.convfmt),
            Value::Num(_) => s(b""),
            _ => self.plain_str(v),
        };
        let env = Env {
            utf8: self.utf8,
            to_str: &to_str,
        };
        match printf::sprintf(&env, fmt, &[Value::Num(n)]) {
            Ok(v) => Rc::from(v),
            Err(_) => s(b""),
        }
    }

    /// A value as a string for CONVFMT-free uses inside formatting (`%s` of a number in
    /// CONVFMT itself would recurse): numbers with %.6g.
    fn plain_str(&self, v: &Value) -> S {
        match v {
            Value::Uninit => s(b""),
            Value::Num(n) => {
                if is_integral(*n) && n.is_finite() {
                    s(int_str(*n).as_bytes())
                } else {
                    let mut out = Vec::new();
                    printf::format_float(&mut out, &printf::Spec::default(), b'g', *n);
                    Rc::from(out)
                }
            }
            Value::Str(x) | Value::StrNum(x) => x.clone(),
        }
    }

    pub fn to_str(&self, v: &Value) -> S {
        match v {
            Value::Uninit => s(b""),
            Value::Num(n) => self.num_to_str(*n, &self.convfmt),
            Value::Str(x) | Value::StrNum(x) => x.clone(),
        }
    }

    /// For print: numbers with OFMT.
    fn to_output_str(&self, v: &Value) -> S {
        match v {
            Value::Num(n) => self.num_to_str(*n, &self.ofmt),
            _ => self.to_str(v),
        }
    }

    pub fn sprintf(&self, fmt: &[u8], args: &[Value]) -> R<Vec<u8>> {
        let to_str = |v: &Value| self.to_str(v);
        let env = Env {
            utf8: self.utf8,
            to_str: &to_str,
        };
        printf::sprintf(&env, fmt, args).or_else(fatal)
    }

    fn compare(&self, l: &Value, r: &Value) -> Option<std::cmp::Ordering> {
        match (l.cmp_num(), r.cmp_num()) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            _ => Some(self.to_str(l).as_ref().cmp(self.to_str(r).as_ref())),
        }
    }

    // ---- Regexes.

    pub fn regex(&mut self, src: &[u8]) -> R<Rc<Regex>> {
        if let Some(r) = self.regex_cache.get(src) {
            return Ok(r.clone());
        }
        let re = match super::regex::compile(src, self.utf8) {
            Ok(r) => Rc::new(r),
            Err(e) => return fatal(e),
        };
        if self.regex_cache.len() > 500 {
            self.regex_cache.clear();
        }
        self.regex_cache.insert(src.to_vec(), re.clone());
        Ok(re)
    }

    fn regex_of(&mut self, v: &Value) -> R<Rc<Regex>> {
        let src = self.to_str(v);
        self.regex(&src)
    }

    // ---- Fields and the record.

    /// A new record from input ($0 a strnum).
    pub fn set_line(&mut self, line: &[u8]) {
        self.rec.clear();
        self.rec.extend_from_slice(line);
        self.new_record();
    }

    /// The record read last becomes $0: the two buffers change places.
    fn take_input(&mut self) {
        std::mem::swap(&mut self.rec, &mut self.input);
        self.new_record();
    }

    fn new_record(&mut self) {
        self.line = None;
        self.line_is_str = false;
        self.assigned[5] = None;
        self.fields_ok = false;
        if !Rc::ptr_eq(&self.record_fs, &self.fs_split) {
            self.record_fs = self.fs_split.clone();
        }
    }

    /// $0 as the text a value shares.
    fn line(&mut self) -> S {
        match &self.line {
            Some(line) => line.clone(),
            None => {
                let line = s(&self.rec);
                self.line = Some(line.clone());
                line
            }
        }
    }

    fn ensure_fields(&mut self) {
        if self.fields_ok {
            return;
        }
        self.fields_ok = true;
        let paragraph = matches!(self.rs_mode, Rs::Paragraph);
        let mut fields = std::mem::take(&mut self.fields);
        fields.clear();
        let (base, len) = (self.rec.as_ptr() as usize, self.rec.len());
        split_into(&self.rec, &self.record_fs, paragraph, self.utf8, |f| {
            // A field is a part of the line: only its place is kept.
            fields.push(match (f.as_ptr() as usize).checked_sub(base) {
                Some(at) if at + f.len() <= len => Field::At(at, at + f.len()),
                _ => Field::Is(Value::strnum(f)),
            })
        });
        self.fields = fields;
        self.nf = self.fields.len();
    }

    /// The value of a field that is still a place in the line (the line itself when
    /// the field is all of it).
    fn field_value(&mut self, from: usize, to: usize) -> Value {
        if from == 0 && to == self.rec.len() {
            Value::StrNum(self.line())
        } else {
            Value::strnum(&self.rec[from..to])
        }
    }

    /// A field as `print` writes it, appended to `out` from where it lies in the
    /// record. A field past NF is empty.
    fn field_text(&mut self, i: usize, out: &mut Vec<u8>) {
        if i == 0 {
            return out.extend_from_slice(&self.rec);
        }
        self.ensure_fields();
        match self.fields.get(i - 1) {
            None => {}
            Some(Field::Is(v)) => out.extend_from_slice(&self.to_output_str(v)),
            Some(&Field::At(from, to)) => out.extend_from_slice(&self.rec[from..to]),
        }
    }

    /// The characters in a field, counted where it lies in the record.
    fn field_length(&mut self, i: usize) -> usize {
        if i == 0 {
            return self.char_pos(&self.rec, self.rec.len());
        }
        self.ensure_fields();
        match self.fields.get(i - 1) {
            None => 0,
            Some(Field::Is(v)) => {
                let text = self.to_str(v);
                self.char_pos(&text, text.len())
            }
            Some(&Field::At(from, to)) => self.char_pos(&self.rec[from..to], to - from),
        }
    }

    /// The number a field is in arithmetic, read where it lies in the record: no
    /// value is made for it. A field past NF is 0.
    fn field_num(&mut self, i: usize) -> f64 {
        if i == 0 {
            return parse_prefix(&self.rec);
        }
        self.ensure_fields();
        match self.fields.get(i - 1) {
            None => 0.0,
            Some(Field::Is(v)) => v.num(),
            Some(&Field::At(from, to)) => parse_prefix(&self.rec[from..to]),
        }
    }

    /// Every field as a value: before the line they are places in is replaced.
    fn settle_fields(&mut self) {
        for k in 0..self.fields.len() {
            if let Field::At(from, to) = self.fields[k] {
                self.fields[k] = Field::Is(self.field_value(from, to));
            }
        }
    }

    fn get_field(&mut self, i: f64) -> R<Value> {
        if i < 0.0 {
            return fatal(format!("attempt to access field {}", i as i64));
        }
        let i = i as usize;
        if i == 0 {
            let line = self.line();
            return Ok(if self.line_is_str {
                Value::Str(line)
            } else {
                Value::StrNum(line)
            });
        }
        self.ensure_fields();
        // A field past NF is "" (a string: `$9 == 0` is false, as in gawk).
        Ok(match self.fields.get(i - 1) {
            None => Value::Str(s(b"")),
            Some(Field::Is(v)) => v.clone(),
            Some(&Field::At(from, to)) => {
                let v = self.field_value(from, to);
                self.fields[i - 1] = Field::Is(v.clone());
                v
            }
        })
    }

    fn set_field(&mut self, i: f64, v: Value) -> R<()> {
        if i < 0.0 {
            return fatal(format!("attempt to access field {}", i as i64));
        }
        if i > MAX_FIELD {
            return fatal(format!("field index too large: {}", i as i64));
        }
        let i = i as usize;
        if i == 0 {
            let line = self.to_str(&v);
            self.set_line(&line);
            // An assigned record is a string ($0 = "0" is true).
            self.line_is_str = true;
            return Ok(());
        }
        self.ensure_fields();
        self.settle_fields();
        if self.fields.len() < i {
            self.fields.resize(i, Field::Is(Value::Uninit));
        }
        // A number stays a number; the record gets its string.
        self.fields[i - 1] = Field::Is(match v {
            Value::Uninit => Value::Str(s(b"")),
            v => v,
        });
        self.nf = self.fields.len();
        self.rebuild();
        Ok(())
    }

    fn rebuild(&mut self) {
        let mut line = Vec::new();
        for (k, f) in self.fields.iter().enumerate() {
            if k > 0 {
                line.extend_from_slice(&self.ofs);
            }
            match f {
                Field::At(from, to) => line.extend_from_slice(&self.rec[*from..*to]),
                Field::Is(v) => line.extend_from_slice(&self.to_str(v)),
            }
        }
        self.rec = line;
        self.line = None;
        self.line_is_str = true;
        self.assigned[5] = None;
    }

    fn set_nf(&mut self, n: f64) -> R<()> {
        if n < 0.0 {
            return fatal(format!("NF set to negative value {}", n as i64));
        }
        if n > MAX_FIELD {
            return fatal("NF set too large");
        }
        self.ensure_fields();
        self.settle_fields();
        let n = n as usize;
        self.fields.resize(n, Field::Is(Value::Uninit));
        self.nf = n;
        self.rebuild();
        Ok(())
    }

    // ---- Special variables.

    fn get_special(&mut self, sp: Special) -> Value {
        use Special::*;
        if let Some(i) = assigned_slot(sp)
            && let Some(v) = &self.assigned[i]
            && (sp != Nf || self.fields_ok)
        {
            return v.clone();
        }
        match sp {
            Nf => {
                self.ensure_fields();
                Value::Num(self.nf as f64)
            }
            Nr => Value::Num(self.nr),
            Fnr => Value::Num(self.fnr),
            Rstart => Value::Num(self.rstart),
            Rlength => Value::Num(self.rlength),
            Argc => Value::Num(self.argc),
            Convfmt => Value::Str(self.convfmt.clone()),
            Filename => self.filename.clone(),
            Fs => Value::Str(self.fs.clone()),
            Ofmt => Value::Str(self.ofmt.clone()),
            Ofs => Value::Str(self.ofs.clone()),
            Ors => Value::Str(self.ors.clone()),
            Rs => Value::Str(self.rs.clone()),
            Rt => Value::Str(self.rt.clone()),
            Subsep => Value::Str(self.subsep.clone()),
        }
    }

    pub fn set_special(&mut self, sp: Special, v: Value) -> R<()> {
        use Special::*;
        let keep = v.clone();
        match sp {
            Nf => self.set_nf(v.num())?,
            Nr => self.nr = v.num().trunc(),
            Fnr => self.fnr = v.num().trunc(),
            Rstart => self.rstart = v.num(),
            Rlength => self.rlength = v.num(),
            Argc => self.argc = v.num(),
            Convfmt => self.convfmt = self.to_str(&v),
            Filename => self.filename = v,
            Fs => {
                let fs = self.to_str(&v);
                self.fs_split = self.make_fs(&fs)?;
                self.fs = fs;
            }
            Ofmt => self.ofmt = self.to_str(&v),
            Ofs => self.ofs = self.to_str(&v),
            Ors => self.ors = self.to_str(&v),
            Rs => {
                let rs = self.to_str(&v);
                self.rs_mode = self.make_rs(&rs)?;
                self.rs = rs;
            }
            Rt => self.rt = self.to_str(&v),
            Subsep => self.subsep = self.to_str(&v),
        }
        if let Some(i) = assigned_slot(sp) {
            self.assigned[i] = Some(keep);
        }
        Ok(())
    }

    /// The splitter for a field separator (FS or split's third argument).
    pub fn make_fs(&mut self, fs: &S) -> R<Rc<Fs>> {
        if let Some((k, v)) = &self.fs_cache
            && k == fs
        {
            return Ok(v.clone());
        }
        let chars = if self.utf8 {
            crate::textio::char_lengths(fs).count()
        } else {
            fs.len()
        };
        let split = if &**fs == b" " {
            Fs::Space
        } else if fs.is_empty() {
            Fs::Empty
        } else if chars == 1 && &**fs != b"\\" {
            Fs::Char(fs.to_vec())
        } else {
            Fs::Regex(self.regex(fs)?)
        };
        let split = Rc::new(split);
        self.fs_cache = Some((fs.clone(), split.clone()));
        Ok(split)
    }

    fn make_rs(&mut self, rs: &[u8]) -> R<Rs> {
        let chars = if self.utf8 {
            crate::textio::char_lengths(rs).count()
        } else {
            rs.len()
        };
        Ok(if rs.is_empty() {
            Rs::Paragraph
        } else if rs.len() == 1 {
            Rs::Byte(rs[0])
        } else if chars == 1 {
            Rs::Seq(rs.to_vec())
        } else {
            Rs::Regex(self.regex(rs)?)
        })
    }

    /// `-v name=value` and `name=value` operands.
    pub fn assign_var(&mut self, name: &[u8], value: &[u8]) -> R<()> {
        let v = Value::strnum(value);
        if let Some(sp) = super::resolve::special(name) {
            return self.set_special(sp, v);
        }
        if let Some(&i) = self.prog.scalar_names.get(name) {
            self.globals[i as usize] = v;
            return Ok(());
        }
        if self.prog.array_names.contains_key(name) {
            return fatal(format!(
                "cannot use array `{}' as a scalar",
                String::from_utf8_lossy(name)
            ));
        }
        // Not used by the program: nothing to set.
        Ok(())
    }

    pub fn global_array(&self, name: &[u8]) -> Option<ArrRc> {
        self.prog
            .array_names
            .get(name)
            .map(|&i| self.arrays[i as usize].clone())
    }

    // ---- The stack.

    fn pop(&mut self) -> Value {
        self.stack.pop().expect("awk stack underflow")
    }

    fn push(&mut self, v: Value) {
        self.stack.push(v);
    }

    fn top(&mut self) -> &mut Value {
        self.stack.last_mut().expect("awk stack underflow")
    }

    fn array(&self, a: Arr) -> ArrRc {
        match a {
            Arr::Global(i) => self.arrays[i as usize].clone(),
            Arr::Local(i) => self.frames.last().unwrap().arrays[i as usize].clone(),
        }
    }

    fn load(&mut self, v: Var) -> Value {
        match v {
            Var::Global(i) => self.globals[i as usize].clone(),
            Var::Local(i) => self.stack[self.frames.last().unwrap().base + i as usize].clone(),
            Var::Special(sp) => self.get_special(sp),
        }
    }

    fn store(&mut self, v: Var, value: Value) -> R<()> {
        match v {
            Var::Global(i) => self.globals[i as usize] = value,
            Var::Local(i) => {
                let base = self.frames.last().unwrap().base;
                self.stack[base + i as usize] = value;
            }
            Var::Special(sp) => self.set_special(sp, value)?,
        }
        Ok(())
    }

    /// An array subscript: numbers that are integers as integers, else CONVFMT.
    fn key(&self, v: &Value) -> S {
        self.to_str(v)
    }

    // ---- Running code.

    /// Runs a block (BEGIN, END, a pattern, an action). A pattern leaves its value on
    /// the stack.
    pub async fn exec(&mut self, id: CodeId) -> R<()> {
        let depth = self.frames.len();
        let stack_base = self.stack.len();
        self.frames.push(Frame {
            code: id,
            ip: 0,
            base: stack_base,
            arrays: Vec::new(),
            for_in_base: self.for_in.len(),
        });
        let prog = self.prog.clone();
        let r = loop {
            let next = match self.step(&prog, depth) {
                Ok(Step::Done) => break Ok(()),
                Ok(next) => next,
                Err(f) => break Err(f),
            };
            let waited = match next {
                Step::Check => {
                    self.ctx.check().await;
                    Ok(())
                }
                Step::Flush => self.flush_printed().await,
                _ => self.wait(&prog).await,
            };
            if let Err(f) = waited {
                break Err(f);
            }
        };
        if r.is_err() {
            // Unwind what the error left: frames, locals, loop snapshots.
            self.frames.truncate(depth);
            self.stack.truncate(stack_base);
            self.for_in.clear();
        }
        r
    }

    async fn wait(&mut self, prog: &Program) -> R<()> {
        let f = self.frames.last().unwrap();
        match &prog.code(f.code)[f.ip - 1] {
            Op::Call(b, n) => self.call_io(*b, *n as usize).await,
            Op::PrintFields(fields, redir) => {
                let out = self.print_fields(fields);
                let written = self.write_out(*redir, &out).await;
                self.printed = out;
                written
            }
            Op::Print(n, redir) => {
                let out = self.print_values(*n as usize);
                let written = self.write_out(*redir, &out).await;
                self.printed = out;
                written
            }
            Op::Printf(n, redir) => {
                let out = self.printf_values(*n as usize)?;
                self.write_out(*redir, &out).await
            }
            Op::Getline(src, target) => self.getline(*src, *target).await,
            _ => unreachable!(),
        }
    }

    fn print_fields(&mut self, fields: &[u32]) -> Vec<u8> {
        let mut out = std::mem::take(&mut self.printed);
        out.clear();
        for (k, i) in fields.iter().enumerate() {
            if k > 0 {
                out.extend_from_slice(&self.ofs);
            }
            self.field_text(*i as usize, &mut out);
        }
        out.extend_from_slice(&self.ors);
        out
    }

    fn print_values(&mut self, n: usize) -> Vec<u8> {
        let at = self.stack.len() - n;
        let mut out = std::mem::take(&mut self.printed);
        out.clear();
        if n == 0 {
            out.extend_from_slice(&self.rec);
        } else {
            for (k, v) in self.stack[at..].iter().enumerate() {
                if k > 0 {
                    out.extend_from_slice(&self.ofs);
                }
                out.extend_from_slice(&self.to_output_str(v));
            }
        }
        self.stack.truncate(at);
        out.extend_from_slice(&self.ors);
        out
    }

    fn printf_values(&mut self, n: usize) -> R<Vec<u8>> {
        let at = self.stack.len() - n;
        let args: Vec<Value> = self.stack.drain(at..).collect();
        let fmt = self.to_str(&args[0]);
        self.sprintf(&fmt, &args[1..])
    }

    fn print_now(&mut self, out: Vec<u8>) -> bool {
        let now = self.ctx.write_now(&out);
        self.printed = out;
        now
    }

    async fn flush_printed(&mut self) -> R<()> {
        let out = std::mem::take(&mut self.printed);
        let written = self.write_stdout(&out).await;
        self.printed = out;
        written
    }

    fn step(&mut self, prog: &Program, depth: usize) -> R<Step> {
        let f = self.frames.last().unwrap();
        let (mut code, mut ip) = (prog.code(f.code), f.ip);
        loop {
            if ip >= code.len() {
                // The end of a block, or a function's end without return.
                if self.frames.len() == depth + 1 {
                    self.frames.pop();
                    return Ok(Step::Done);
                }
                self.return_from(Value::Uninit);
                let f = self.frames.last().unwrap();
                code = prog.code(f.code);
                ip = f.ip;
                continue;
            }
            self.steps += 1;
            if self.steps >= CHECK_EVERY {
                self.steps = 0;
                self.frames.last_mut().unwrap().ip = ip;
                return Ok(Step::Check);
            }
            let op = &code[ip];
            ip += 1;
            match op {
                Op::Num(n) => self.push(Value::Num(*n)),
                Op::Str(x) => self.push(Value::Str(x.clone())),
                Op::Dup => {
                    let v = self.top().clone();
                    self.push(v);
                }
                Op::Drop => {
                    self.pop();
                }
                Op::Swap => {
                    let n = self.stack.len();
                    self.stack.swap(n - 1, n - 2);
                }
                Op::Rote => {
                    let n = self.stack.len();
                    self.stack[n - 3..].rotate_left(1);
                }
                Op::Field => {
                    let i = self.pop().num();
                    let v = self.get_field(i)?;
                    self.push(v);
                }
                Op::FieldInt(i) => {
                    let v = self.get_field(*i as f64)?;
                    self.push(v);
                }
                Op::FieldNum(i) => {
                    let n = self.field_num(*i as usize);
                    self.push(Value::Num(n));
                }
                Op::FieldLength(i) => {
                    let n = self.field_length(*i as usize);
                    self.push(Value::Num(n as f64));
                }
                Op::Load(v) => {
                    let x = self.load(*v);
                    self.push(x);
                }
                Op::ArrayGet(a) => {
                    let k = self.pop();
                    let k = self.key(&k);
                    let arr = self.array(*a);
                    let v = arr.borrow_mut().get_or_create(k).clone();
                    self.push(v);
                }
                Op::In(a) => {
                    let k = self.pop();
                    let k = self.key(&k);
                    let has = self.array(*a).borrow().contains(&k);
                    self.push(Value::bool(has));
                }
                Op::Store(v) => {
                    let x = self.pop();
                    self.store(*v, x)?;
                }
                Op::StoreField => {
                    let i = self.pop().num();
                    let v = self.pop();
                    self.set_field(i, v)?;
                }
                Op::StoreFieldSub => {
                    let i = self.pop().num();
                    let v = self.pop();
                    if self.top().num() > 0.0 {
                        self.set_field(i, v)?;
                    }
                }
                Op::StoreArray(a) => {
                    let k = self.pop();
                    let k = self.key(&k);
                    let v = self.pop();
                    self.array(*a).borrow_mut().set(k, v);
                }
                Op::Delete(a) => {
                    let k = self.pop();
                    let k = self.key(&k);
                    self.array(*a).borrow_mut().delete(&k);
                }
                Op::DeleteAll(a) => self.array(*a).borrow_mut().clear(),
                Op::MatchRecord(i) => {
                    let re = prog.regexes[*i as usize].clone();
                    let m = re.is_match(&self.rec);
                    self.push(Value::bool(m));
                }
                Op::MatchConst(i, neg) => {
                    let v = self.pop();
                    let text = self.to_str(&v);
                    let m = prog.regexes[*i as usize].is_match(&text);
                    self.push(Value::bool(m != *neg));
                }
                Op::IndexMulti(n) => {
                    let n = *n as usize;
                    let at = self.stack.len() - n;
                    let mut key = Vec::new();
                    for (k, v) in self.stack[at..].iter().enumerate() {
                        if k > 0 {
                            key.extend_from_slice(&self.subsep);
                        }
                        key.extend_from_slice(&self.to_str(v));
                    }
                    self.stack.truncate(at);
                    self.push(Value::Str(Rc::from(key)));
                }
                Op::ConcatMulti(n) => {
                    let n = *n as usize;
                    let at = self.stack.len() - n;
                    let mut out = Vec::new();
                    for v in &self.stack[at..] {
                        out.extend_from_slice(&self.to_str(v));
                    }
                    self.stack.truncate(at);
                    self.push(Value::Str(Rc::from(out)));
                }
                Op::Concat => {
                    let r = self.pop();
                    let l = self.pop();
                    let mut out = self.to_str(&l).to_vec();
                    out.extend_from_slice(&self.to_str(&r));
                    self.push(Value::Str(Rc::from(out)));
                }
                Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Pow => {
                    let r = self.pop().num();
                    let l = self.pop().num();
                    let v = match op {
                        Op::Add => l + r,
                        Op::Sub => l - r,
                        Op::Mul => l * r,
                        Op::Div => {
                            if r == 0.0 {
                                return fatal("division by zero attempted");
                            }
                            l / r
                        }
                        Op::Mod => {
                            if r == 0.0 {
                                return fatal("division by zero attempted in `%'");
                            }
                            remainder(l, r)
                        }
                        _ => pow(l, r),
                    };
                    self.push(Value::Num(v));
                }
                Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                    let r = self.pop();
                    let l = self.pop();
                    let o = self.compare(&l, &r);
                    use std::cmp::Ordering::*;
                    let b = match op {
                        Op::Eq => o == Some(Equal),
                        Op::Ne => o != Some(Equal),
                        Op::Lt => o == Some(Less),
                        Op::Le => matches!(o, Some(Less | Equal)),
                        Op::Gt => o == Some(Greater),
                        _ => matches!(o, Some(Greater | Equal)),
                    };
                    self.push(Value::bool(b));
                }
                Op::Match | Op::NotMatch => {
                    let r = self.pop();
                    let l = self.pop();
                    let re = self.regex_of(&r)?;
                    let m = re.is_match(&self.to_str(&l));
                    self.push(Value::bool(m == matches!(op, Op::Match)));
                }
                Op::Not => {
                    let b = !self.pop().truth();
                    self.push(Value::bool(b));
                }
                Op::Neg => {
                    let n = -self.pop().num();
                    self.push(Value::Num(n));
                }
                Op::Plus => {
                    let n = self.pop().num();
                    self.push(Value::Num(n));
                }
                Op::Boolean => {
                    let b = self.pop().truth();
                    self.push(Value::bool(b));
                }
                Op::Jump(o) => ip = (ip as i64 + *o as i64) as usize,
                Op::JumpFalse(o) => {
                    if !self.pop().truth() {
                        ip = (ip as i64 + *o as i64) as usize;
                    }
                }
                Op::JumpTrue(o) => {
                    if self.pop().truth() {
                        ip = (ip as i64 + *o as i64) as usize;
                    }
                }
                Op::Next => return Err(Flow::Next),
                Op::Nextfile => return Err(Flow::Nextfile),
                Op::Exit => return Err(Flow::Exit),
                Op::ExitStatus => {
                    let n = self.pop().num();
                    self.exit_status = (n as i64 & 0xff) as i32;
                    return Err(Flow::Exit);
                }
                Op::ForInStart(a) => {
                    let keys = self.array(*a).borrow().keys();
                    self.for_in.push(ForIn { keys, next: 0 });
                }
                Op::ForInNext(var, end) => {
                    let f = self.for_in.last_mut().unwrap();
                    if f.next < f.keys.len() {
                        let k = f.keys[f.next].clone();
                        f.next += 1;
                        self.store(*var, Value::StrNum(k))?;
                    } else {
                        self.for_in.pop();
                        ip = (ip as i64 + *end as i64) as usize;
                    }
                }
                Op::ForInEnd => {
                    self.for_in.pop();
                }
                Op::LengthArray(a) => {
                    let n = self.array(*a).borrow().len();
                    self.push(Value::Num(n as f64));
                }
                Op::Split(a, sep) => {
                    let fs = match sep {
                        SplitSep::Fs => self.fs_split.clone(),
                        SplitSep::Dynamic => {
                            let v = self.pop();
                            let v = self.to_str(&v);
                            self.make_fs(&v)?
                        }
                        SplitSep::Regex(i) => Rc::new(Fs::Regex(prog.regexes[*i as usize].clone())),
                    };
                    let v = self.pop();
                    let text = self.to_str(&v);
                    let arr = self.array(*a);
                    let mut arr = arr.borrow_mut();
                    arr.clear();
                    let mut n = 0;
                    // Newlines separate the fields of a paragraph record, not split()'s.
                    split_into(&text, &fs, false, self.utf8, |f| {
                        n += 1;
                        arr.set(s(n.to_string().as_bytes()), Value::strnum(f));
                    });
                    drop(arr);
                    self.push(Value::Num(n as f64));
                }
                Op::MatchArray(a) => {
                    let re = self.pop();
                    let v = self.pop();
                    let re = self.regex_of(&re)?;
                    let text = self.to_str(&v);
                    let arr = self.array(*a);
                    arr.borrow_mut().clear();
                    let start = match re.exec(&text, ExecOptions::default()) {
                        Some(caps) => {
                            let (s0, e0) = caps[0].unwrap();
                            self.assigned[2] = None;
                            self.assigned[3] = None;
                            self.rstart = self.char_pos(&text, s0) as f64 + 1.0;
                            self.rlength = self.char_pos(&text[s0..], e0 - s0) as f64;
                            let mut arr = arr.borrow_mut();
                            for (k, c) in caps.iter().enumerate() {
                                if let Some((cs, ce)) = c {
                                    let ks = k.to_string();
                                    arr.set(s(ks.as_bytes()), Value::strnum(&text[*cs..*ce]));
                                    let mut start_key = ks.as_bytes().to_vec();
                                    start_key.extend_from_slice(&self.subsep);
                                    let mut len_key = start_key.clone();
                                    start_key.extend_from_slice(b"start");
                                    len_key.extend_from_slice(b"length");
                                    let st = self.char_pos(&text, *cs) + 1;
                                    let ln = self.char_pos(&text[*cs..], ce - cs);
                                    arr.set(Rc::from(start_key), Value::Num(st as f64));
                                    arr.set(Rc::from(len_key), Value::Num(ln as f64));
                                }
                            }
                            self.rstart
                        }
                        None => {
                            self.assigned[2] = None;
                            self.assigned[3] = None;
                            self.rstart = 0.0;
                            self.rlength = -1.0;
                            0.0
                        }
                    };
                    self.push(Value::Num(start));
                }
                Op::Sprintf(n) => {
                    let n = *n as usize;
                    let at = self.stack.len() - n;
                    let args: Vec<Value> = self.stack.drain(at..).collect();
                    let fmt = self.to_str(&args[0]);
                    let out = self.sprintf(&fmt, &args[1..])?;
                    self.push(Value::Str(Rc::from(out)));
                }
                Op::CallUser { func, arrays } => {
                    if self.frames.len() > MAX_CALL_DEPTH {
                        return fatal("function call nesting too deep");
                    }
                    let f = &prog.funcs[*func as usize];
                    let mut arrs: Vec<ArrRc> = arrays.iter().map(|a| self.array(*a)).collect();
                    while arrs.len() < f.num_arrays {
                        arrs.push(Rc::new(RefCell::new(Array::default())));
                    }
                    self.frames.last_mut().unwrap().ip = ip;
                    let base = self.stack.len() - f.num_scalars;
                    self.frames.push(Frame {
                        code: CodeId::Func(*func as usize),
                        ip: 0,
                        base,
                        arrays: arrs,
                        for_in_base: self.for_in.len(),
                    });
                    code = &f.body;
                    ip = 0;
                }
                Op::Return | Op::ReturnNull => {
                    let v = if matches!(op, Op::Return) {
                        self.pop()
                    } else {
                        Value::Uninit
                    };
                    // A return value that is a number stays one; strings likewise.
                    self.return_from(v);
                    let f = self.frames.last().unwrap();
                    code = prog.code(f.code);
                    ip = f.ip;
                }
                Op::Nulls(n) => {
                    for _ in 0..*n {
                        self.push(Value::Uninit);
                    }
                }
                Op::Line(n) => self.src_line = *n,
                Op::Call(b, n) if !matches!(b, Builtin::System | Builtin::Close | Builtin::Fflush) => {
                    self.call_builtin(*b, *n as usize)?
                }
                Op::PrintFields(fields, Redir::None) => {
                    let out = self.print_fields(fields);
                    if !self.print_now(out) {
                        self.frames.last_mut().unwrap().ip = ip;
                        return Ok(Step::Flush);
                    }
                }
                Op::Print(n, Redir::None) => {
                    let out = self.print_values(*n as usize);
                    if !self.print_now(out) {
                        self.frames.last_mut().unwrap().ip = ip;
                        return Ok(Step::Flush);
                    }
                }
                Op::Printf(n, Redir::None) => {
                    let out = self.printf_values(*n as usize)?;
                    if !self.print_now(out) {
                        self.frames.last_mut().unwrap().ip = ip;
                        return Ok(Step::Flush);
                    }
                }
                Op::Call(..) | Op::PrintFields(..) | Op::Print(..) | Op::Printf(..) | Op::Getline(..) => {
                    self.frames.last_mut().unwrap().ip = ip;
                    return Ok(Step::Wait);
                }
            }
        }
    }

    /// Leaves a function: drops its locals and snapshots, pushes its value.
    fn return_from(&mut self, v: Value) {
        let f = self.frames.pop().unwrap();
        self.stack.truncate(f.base);
        self.for_in.truncate(f.for_in_base);
        self.push(v);
    }

    /// The character count of `text[..byte]`.
    pub fn char_pos(&self, text: &[u8], byte: usize) -> usize {
        if self.utf8 && !text[..byte].is_ascii() {
            crate::textio::char_lengths(&text[..byte]).count()
        } else {
            byte
        }
    }

    // ---- Output.

    async fn write_out(&mut self, redir: Redir, data: &[u8]) -> R<()> {
        let mode = match redir {
            Redir::None => {
                return self.write_stdout(data).await;
            }
            m => m,
        };
        let dest = self.pop();
        let name = self.to_str(&dest);
        if name.is_empty() {
            let op = match mode {
                Redir::Write => ">",
                Redir::Append => ">>",
                _ => "|",
            };
            return fatal(format!(
                "expression for `{op}' redirection has null string value"
            ));
        }
        if !matches!(mode, Redir::Pipe) {
            match &*name {
                b"/dev/stdout" | b"-" => return self.write_stdout(data).await,
                b"/dev/stderr" => {
                    self.ctx.error_raw(data).await;
                    return Ok(());
                }
                _ => {}
            }
        }
        if !self.outputs.contains_key(&*name) {
            let out = match mode {
                Redir::Pipe => {
                    // Output so far comes first (gawk flushes before it starts a pipe).
                    let stdio = ShellStdio {
                        stdin: ShellIo::Pipe,
                        stdout: ShellIo::Inherit,
                    };
                    let child = self.ctx.spawn_shell(&name, stdio).await;
                    Output {
                        kind: OutKind::Cmd(child),
                        buf: Vec::new(),
                    }
                }
                _ => {
                    let opts = if matches!(mode, Redir::Append) {
                        OpenOptions::append()
                    } else {
                        OpenOptions::write()
                    };
                    match self.ctx.open(&name, opts).await {
                        Ok(f) => Output {
                            kind: OutKind::File(f),
                            buf: Vec::new(),
                        },
                        Err(e) => {
                            return fatal(format!(
                                "can't redirect to `{}' ({})",
                                String::from_utf8_lossy(&name),
                                error_text(&e)
                            ));
                        }
                    }
                }
            };
            self.outputs.insert(name.to_vec(), out);
        }
        let out = self.outputs.get_mut(&*name).unwrap();
        if let Err(e) = out.write(data).await {
            if e.kind() == io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return fatal(format!(
                "write failure on `{}' ({})",
                String::from_utf8_lossy(&name),
                error_text(&e)
            ));
        }
        Ok(())
    }

    async fn write_stdout(&mut self, data: &[u8]) -> R<()> {
        match self.ctx.write(data).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                // As gawk: a closed pipe ends awk quietly (SIGPIPE).
                self.exit_status = 141;
                Err(Flow::Fatal(String::new()))
            }
            Err(e) => fatal(format!("write failure ({})", error_text(&e))),
        }
    }

    /// Flushes stdout and every output stream (before `system`, at `fflush()`).
    pub async fn flush_all(&mut self) -> bool {
        let mut ok = self.ctx.flush().await.is_ok();
        for out in self.outputs.values_mut() {
            ok &= out.flush().await.is_ok();
        }
        ok
    }

    async fn close(&mut self, name: &[u8]) -> i32 {
        if let Some(out) = self.outputs.remove(name) {
            return out.close().await;
        }
        if let Some(mut r) = self.cmds_in.remove(name)
            && let Src::Cmd(c) = &mut r.src
        {
            c.stdout = None;
            return c.wait().await;
        }
        if self.files_in.remove(name).is_some() {
            return 0;
        }
        -1
    }

    /// Closes every stream at the end, commands waited for.
    pub async fn close_all(&mut self) {
        let outs: Vec<_> = self.outputs.drain().collect();
        for (_, out) in outs {
            out.close().await;
        }
        let cmds: Vec<_> = self.cmds_in.drain().collect();
        for (_, mut r) in cmds {
            if let Src::Cmd(c) = &mut r.src {
                c.stdout = None;
                c.wait().await;
            }
        }
        self.files_in.clear();
    }

    // ---- Input.

    /// [`Self::next_main_record`] when the record is in what the reader holds.
    fn buffered_main_record(&mut self) -> bool {
        let Some(reader) = self.main.as_mut() else {
            return false;
        };
        let Some(rt) = reader.buffered(&self.rs_mode, &mut self.input) else {
            return false;
        };
        self.rt = rt;
        self.nr += 1.0;
        self.assigned[0] = None;
        self.fnr += 1.0;
        self.assigned[1] = None;
        true
    }

    /// The next record of the main input (ARGV's files, else stdin) into `input`: NR
    /// and FNR are counted. False at the end of the input.
    pub async fn next_main_record(&mut self) -> R<bool> {
        loop {
            if self.main.is_none() {
                if self.main_done {
                    return Ok(false);
                }
                if !self.open_next_file().await? {
                    self.main_done = true;
                    return Ok(false);
                }
            }
            let reader = self.main.as_mut().unwrap();
            match reader.read(self.ctx, &self.rs_mode, &mut self.input).await {
                Ok(Some(rt)) => {
                    self.rt = rt;
                    self.nr += 1.0;
                    self.assigned[0] = None;
                    self.fnr += 1.0;
                    self.assigned[1] = None;
                    return Ok(true);
                }
                Ok(None) => self.main = None,
                Err(e) => {
                    let name = self.to_str(&self.filename.clone());
                    self.main = None;
                    return fatal(format!(
                        "error reading input file `{}': {}",
                        String::from_utf8_lossy(&name),
                        error_text(&e)
                    ));
                }
            }
        }
    }

    /// Opens the next input from ARGV (assignments done on the way); false when there
    /// are none left.
    async fn open_next_file(&mut self) -> R<bool> {
        let argv = self.global_array(b"ARGV");
        loop {
            if (self.argv_index as f64) >= self.argc {
                if self.had_files {
                    return Ok(false);
                }
                self.had_files = true;
                self.main = Some(RecordReader::new(Src::Stdin));
                // gawk names stdin "-" (it is "" in BEGIN).
                self.filename = Value::StrNum(s(b"-"));
                self.fnr = 0.0;
                self.assigned[1] = None;
                return Ok(true);
            }
            let k = self.argv_index.to_string();
            self.argv_index += 1;
            let arg = match &argv {
                Some(a) => a.borrow().get(k.as_bytes()).cloned(),
                None => None,
            };
            let Some(arg) = arg else { continue };
            let arg = self.to_str(&arg);
            if arg.is_empty() {
                continue;
            }
            if let Some((name, value)) = split_assignment(&arg) {
                let value = super::lexer::unescape(value);
                self.assign_var(name, &value)?;
                continue;
            }
            self.had_files = true;
            self.fnr = 0.0;
            self.assigned[1] = None;
            self.filename = Value::StrNum(arg.clone());
            if &*arg == b"-" || &*arg == b"/dev/stdin" {
                self.main = Some(RecordReader::new(Src::Stdin));
                return Ok(true);
            }
            match self.ctx.open(&arg, OpenOptions::read()).await {
                Ok(f) => {
                    self.main = Some(RecordReader::new(Src::File(f)));
                    return Ok(true);
                }
                Err(e) => {
                    // gawk: a warning, the status 2 at the end, and on to the next.
                    let msg = format!(
                        "warning: command line argument `{}' is a directory: skipped",
                        String::from_utf8_lossy(&arg)
                    );
                    let is_dir = matches!(
                        self.ctx.stat(&arg, true).await,
                        Ok(m) if m.kind == psh_engine::host::FileKind::Dir
                    );
                    if is_dir {
                        self.ctx.error(&msg).await;
                        continue;
                    }
                    return Err(Flow::FatalPlain(format!(
                        "cannot open file `{}' for reading: {}",
                        String::from_utf8_lossy(&arg),
                        error_text(&e)
                    )));
                }
            }
        }
    }

    /// Skips the rest of the current file (`nextfile`).
    pub fn skip_file(&mut self) {
        self.main = None;
    }

    async fn getline(&mut self, src: GetlineSrc, target: Target) -> R<()> {
        // The target's index is on top, then the file or command.
        let index = match target {
            Target::Field | Target::Array(_) => Some(self.pop()),
            _ => None,
        };
        let got = match src {
            GetlineSrc::Main => {
                // A prompt written before is visible first.
                let _ = self.ctx.flush().await;
                match self.next_main_record().await {
                    Ok(got) => got,
                    Err(Flow::Fatal(m) | Flow::FatalPlain(m)) => {
                        self.ctx.error(&m).await;
                        self.push(Value::Num(-1.0));
                        return Ok(());
                    }
                    Err(f) => return Err(f),
                }
            }
            GetlineSrc::File | GetlineSrc::Cmd => {
                let v = self.pop();
                let name = self.to_str(&v);
                let rs = self.rs_mode.clone();
                let cmd = matches!(src, GetlineSrc::Cmd);
                let exists = if cmd {
                    self.cmds_in.contains_key(&*name)
                } else {
                    self.files_in.contains_key(&*name)
                };
                if !exists {
                    let reader = if cmd {
                        let _ = self.flush_all().await;
                        let stdio = ShellStdio {
                            stdin: ShellIo::Inherit,
                            stdout: ShellIo::Pipe,
                        };
                        RecordReader::new(Src::Cmd(self.ctx.spawn_shell(&name, stdio).await))
                    } else if &*name == b"-" || &*name == b"/dev/stdin" {
                        RecordReader::new(Src::Stdin)
                    } else {
                        match self.ctx.open(&name, OpenOptions::read()).await {
                            Ok(f) => RecordReader::new(Src::File(f)),
                            Err(_) => {
                                self.push(Value::Num(-1.0));
                                return Ok(());
                            }
                        }
                    };
                    if cmd {
                        self.cmds_in.insert(name.to_vec(), reader);
                    } else {
                        self.files_in.insert(name.to_vec(), reader);
                    }
                }
                let reader = if cmd {
                    self.cmds_in.get_mut(&*name).unwrap()
                } else {
                    self.files_in.get_mut(&*name).unwrap()
                };
                match reader.read(self.ctx, &rs, &mut self.input).await {
                    Ok(Some(rt)) => {
                        self.rt = rt;
                        if cmd {
                            self.nr += 1.0;
                            self.assigned[0] = None;
                        }
                        true
                    }
                    Ok(None) => false,
                    Err(_) => {
                        self.push(Value::Num(-1.0));
                        return Ok(());
                    }
                }
            }
        };
        if !got {
            self.push(Value::Num(0.0));
            return Ok(());
        }
        match target {
            Target::Record => {
                self.take_input();
                if matches!(src, GetlineSrc::File) {
                    // NR and FNR are left alone for a file.
                }
            }
            Target::Var(v) => {
                let value = Value::strnum(&self.input);
                self.store(v, value)?
            }
            Target::Field => {
                let i = index.unwrap().num();
                let value = Value::strnum(&self.input);
                self.set_field(i, value)?;
            }
            Target::Array(a) => {
                let k = self.key(&index.unwrap());
                let value = Value::strnum(&self.input);
                self.array(a).borrow_mut().set(k, value);
            }
        }
        self.push(Value::Num(1.0));
        Ok(())
    }

    // ---- The program.

    /// BEGIN, the records, END; the exit status.
    pub async fn run_program(&mut self) -> i32 {
        let prog = self.prog.clone();
        let mut exiting = false;
        match self.exec(CodeId::Begin).await {
            Ok(()) => {}
            Err(Flow::Exit) => exiting = true,
            Err(Flow::Fatal(m)) => return self.die(&m, true).await,
            Err(Flow::FatalPlain(m)) => return self.die(&m, false).await,
            Err(_) => {}
        }
        if !exiting && (!prog.actions.is_empty() || prog.has_end) {
            match self.records().await {
                Ok(()) => {}
                Err(Flow::Exit) => {}
                Err(Flow::Fatal(m)) => return self.die(&m, true).await,
                Err(Flow::FatalPlain(m)) => return self.die(&m, false).await,
                Err(_) => {}
            }
        }
        // END runs after an exit elsewhere too (not after one in END).
        if prog.has_end {
            match self.exec(CodeId::End).await {
                Ok(()) | Err(Flow::Exit) => {}
                Err(Flow::Fatal(m)) => return self.die(&m, true).await,
                Err(Flow::FatalPlain(m)) => return self.die(&m, false).await,
                Err(_) => {}
            }
        }
        self.close_all().await;
        if self.ctx.flush().await.is_err() && self.exit_status == 0 {
            return 2;
        }
        self.exit_status
    }

    async fn die(&mut self, msg: &str, context: bool) -> i32 {
        self.close_all().await;
        if msg.is_empty() {
            return self.exit_status;
        }
        let _ = self.ctx.flush().await;
        let mut where_ = String::new();
        if context && self.src_line > 0 {
            let line = self.src_line;
            where_ = match self.places.iter().rev().find(|(first, _)| *first <= line) {
                Some((first, name)) => format!("{name}:{}: ", line - first + 1),
                None => format!("cmd. line:{line}: "),
            };
        }
        if context && (self.nr > 0.0 || !matches!(self.filename, Value::Uninit)) {
            let f = self.to_str(&self.filename.clone());
            let f = if f.is_empty() { s(b"-") } else { f };
            where_ += &format!(
                "(FILENAME={} FNR={}) ",
                String::from_utf8_lossy(&f),
                self.fnr
            );
        }
        self.ctx.error(&format!("{where_}fatal: {msg}")).await;
        2
    }

    async fn records(&mut self) -> R<()> {
        let prog = self.prog.clone();
        'records: loop {
            if !self.buffered_main_record() && !self.next_main_record().await? {
                return Ok(());
            }
            self.take_input();
            for (i, action) in prog.actions.iter().enumerate() {
                let matched = match action.pattern.len() {
                    0 => true,
                    1 => {
                        self.exec(CodeId::Pattern(i, 0)).await?;
                        self.pop().truth()
                    }
                    _ => {
                        if !self.range_active[i] {
                            self.exec(CodeId::Pattern(i, 0)).await?;
                            self.range_active[i] = self.pop().truth();
                        }
                        let m = self.range_active[i];
                        if m {
                            self.exec(CodeId::Pattern(i, 1)).await?;
                            self.range_active[i] = !self.pop().truth();
                        }
                        m
                    }
                };
                if !matched {
                    continue;
                }
                if action.body.is_none() {
                    let out = self.print_values(0);
                    if !self.print_now(out) {
                        self.flush_printed().await?;
                    }
                    continue;
                }
                match self.exec(CodeId::Action(i)).await {
                    Ok(()) => {}
                    Err(Flow::Next) => continue 'records,
                    Err(Flow::Nextfile) => {
                        self.skip_file();
                        continue 'records;
                    }
                    Err(f) => return Err(f),
                }
            }
        }
    }

    // ---- Builtins.

    fn call_builtin(&mut self, b: Builtin, n: usize) -> R<()> {
        use Builtin as B;
        let at = self.stack.len() - n;
        // The arguments move into the list the last call left: no list per call.
        let mut args = std::mem::take(&mut self.call_args);
        args.clear();
        args.extend(self.stack.drain(at..));
        let v = match b {
            B::Length => {
                let t = self.to_str(&args[0]);
                Value::Num(self.char_pos(&t, t.len()) as f64)
            }
            B::Substr => {
                let t = self.to_str(&args[0]);
                let len = self.char_pos(&t, t.len()) as f64;
                // Truncated, as gawk does with the values agents use; start < 1 is 1.
                let start = args[1].num();
                let start = if start.is_nan() { 1.0 } else { start.trunc() };
                let count = match args.get(2) {
                    Some(c) => {
                        let c = c.num();
                        if c.is_nan() { 0.0 } else { c.trunc() }
                    }
                    None => f64::INFINITY,
                };
                let first = start.max(1.0);
                let end = if count == f64::INFINITY {
                    len + 1.0
                } else {
                    (first + count).min(len + 1.0)
                };
                let end = if start < 1.0 && count != f64::INFINITY {
                    end.min(len + 1.0)
                } else {
                    end
                };
                if end <= first || first > len {
                    Value::Str(s(b""))
                } else {
                    let (a, z) = (first as usize - 1, end as usize - 1);
                    let (ba, bz) = self.byte_range(&t, a, z);
                    Value::Str(s(&t[ba..bz]))
                }
            }
            B::Index => {
                let hay = self.to_str(&args[0]);
                let needle = self.to_str(&args[1]);
                let pos = if needle.is_empty() {
                    1
                } else {
                    hay.windows(needle.len())
                        .position(|w| w == &*needle)
                        .map_or(0, |p| self.char_pos(&hay, p) + 1)
                };
                Value::Num(pos as f64)
            }
            B::Tolower | B::Toupper => {
                let t = self.to_str(&args[0]);
                let upper = b == B::Toupper;
                let out = if self.utf8 && !t.is_ascii() {
                    match std::str::from_utf8(&t) {
                        Ok(st) if upper => st.to_uppercase().into_bytes(),
                        Ok(st) => st.to_lowercase().into_bytes(),
                        Err(_) if upper => t.to_ascii_uppercase(),
                        Err(_) => t.to_ascii_lowercase(),
                    }
                } else if upper {
                    t.to_ascii_uppercase()
                } else {
                    t.to_ascii_lowercase()
                };
                Value::Str(Rc::from(out))
            }
            B::Int => Value::Num(args[0].num().trunc()),
            B::Sqrt => Value::Num(args[0].num().sqrt()),
            B::Exp => Value::Num(args[0].num().exp()),
            B::Log => Value::Num(args[0].num().ln()),
            B::Sin => Value::Num(args[0].num().sin()),
            B::Cos => Value::Num(args[0].num().cos()),
            B::Atan2 => Value::Num(args[0].num().atan2(args[1].num())),
            B::Rand => Value::Num(self.rand()),
            B::Srand => {
                let prev = self.seed;
                self.seed = match args.first() {
                    Some(v) => v.num(),
                    None => self.ctx.now().as_secs() as f64,
                };
                self.rand_state = seed_state(self.seed);
                Value::Num(prev)
            }
            B::Match => {
                let text = self.to_str(&args[0]);
                let re = self.regex_of(&args[1])?;
                match re.find(&text, ExecOptions::default()) {
                    Some((st, en)) => {
                        self.assigned[2] = None;
                        self.assigned[3] = None;
                        self.rstart = self.char_pos(&text, st) as f64 + 1.0;
                        self.rlength = self.char_pos(&text[st..], en - st) as f64;
                    }
                    None => {
                        self.assigned[2] = None;
                        self.assigned[3] = None;
                        self.rstart = 0.0;
                        self.rlength = -1.0;
                    }
                }
                Value::Num(self.rstart)
            }
            B::Sub | B::Gsub => {
                let re = self.regex_of(&args[0])?;
                let repl = self.to_str(&args[1]);
                let text = self.to_str(&args[2]);
                let (out, count) = substitute(&re, &repl, &text, b == B::Gsub, self.utf8);
                self.push(Value::Num(count as f64));
                match out {
                    Some(o) => Value::Str(Rc::from(o)),
                    None => args[2].clone(),
                }
            }
            B::Gensub => {
                let re = self.regex_of(&args[0])?;
                let repl = self.to_str(&args[1]);
                let how = self.to_str(&args[2]);
                let text = self.to_str(&args[3]);
                let which = if how.first().is_some_and(|c| *c == b'g' || *c == b'G') {
                    0
                } else {
                    let n = args[2].num();
                    if n < 1.0 { 1 } else { n as usize }
                };
                Value::Str(Rc::from(gensub(&re, &repl, &text, which, self.utf8)))
            }
            B::Systime => Value::Num(self.ctx.now().as_secs() as f64),
            B::Strftime => {
                let fmt = match args.first() {
                    Some(f) => self.to_str(f),
                    None => s(b"%a %b %e %H:%M:%S %Z %Y"),
                };
                let t = match args.get(1) {
                    Some(t) => t.num().trunc() as i64,
                    None => self.ctx.now().as_secs() as i64,
                };
                let utc = args.get(2).is_some_and(|u| u.truth());
                Value::Str(Rc::from(super::builtins::strftime(self.ctx, &fmt, t, utc)))
            }
            B::Mktime => {
                let spec = self.to_str(&args[0]);
                let utc = args.get(1).is_some_and(|u| u.truth());
                Value::Num(super::builtins::mktime(self.ctx, &spec, utc))
            }
            B::Split | B::Sprintf => unreachable!("compiled to their own instructions"),
            B::System | B::Close | B::Fflush => unreachable!("run by call_io"),
        };
        args.clear();
        self.call_args = args;
        self.push(v);
        Ok(())
    }

    async fn call_io(&mut self, b: Builtin, n: usize) -> R<()> {
        use Builtin as B;
        let at = self.stack.len() - n;
        // The arguments move into the list the last call left: no list per call.
        let mut args = std::mem::take(&mut self.call_args);
        args.clear();
        args.extend(self.stack.drain(at..));
        let v = match b {
            B::System => {
                let cmd = self.to_str(&args[0]);
                self.flush_all().await;
                let stdio = ShellStdio {
                    stdin: ShellIo::Inherit,
                    stdout: ShellIo::Inherit,
                };
                let mut child = self.ctx.spawn_shell(&cmd, stdio).await;
                let st = child.wait().await;
                Value::Num(st as f64)
            }
            B::Close => {
                let name = self.to_str(&args[0]);
                Value::Num(self.close(&name).await as f64)
            }
            B::Fflush => {
                let ok = match args.first() {
                    None => self.flush_all().await,
                    Some(v) => {
                        let name = self.to_str(v);
                        if name.is_empty() {
                            self.flush_all().await
                        } else if &*name == b"/dev/stdout" {
                            self.ctx.flush().await.is_ok()
                        } else if let Some(o) = self.outputs.get_mut(&*name) {
                            o.flush().await.is_ok()
                        } else {
                            false
                        }
                    }
                };
                Value::Num(if ok { 0.0 } else { -1.0 })
            }
            _ => unreachable!("run by call_builtin"),
        };
        args.clear();
        self.call_args = args;
        self.push(v);
        Ok(())
    }

    /// Byte offsets of the characters `a..z`.
    fn byte_range(&self, t: &[u8], a: usize, z: usize) -> (usize, usize) {
        if !self.utf8 {
            return (a.min(t.len()), z.min(t.len()));
        }
        let mut pos = 0;
        let (mut ba, mut bz) = (t.len(), t.len());
        for (k, len) in crate::textio::char_lengths(t).enumerate() {
            if k == a {
                ba = pos;
            }
            if k == z {
                bz = pos;
                break;
            }
            pos += len;
        }
        (ba, bz)
    }

    fn rand(&mut self) -> f64 {
        // splitmix64: deterministic from the seed, as awk's rand() is.
        self.rand_state = self.rand_state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.rand_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// `l % r` (C's fmod). Whole numbers a double holds exactly take the integer
/// remainder: the same value, the sign of `l` kept on a zero, without the library's
/// bit-by-bit division.
fn remainder(l: f64, r: f64) -> f64 {
    let exact = (1u64 << f64::MANTISSA_DIGITS) as f64;
    if l.abs() < exact && r.abs() < exact && l == l.trunc() && r == r.trunc() {
        let m = ((l as i64) % (r as i64)) as f64;
        return if l.is_sign_negative() { -m.abs() } else { m };
    }
    l % r
}

fn pow(l: f64, r: f64) -> f64 {
    // Integer exponents by multiplication, as awk's (exact for small integers).
    if r == r.trunc() && r.abs() < 1024.0 {
        let mut n = r.abs() as u32;
        let mut base = l;
        let mut acc = 1.0;
        while n > 0 {
            if n & 1 == 1 {
                acc *= base;
            }
            base *= base;
            n >>= 1;
        }
        return if r < 0.0 { 1.0 / acc } else { acc };
    }
    l.powf(r)
}

/// `name=value` with a valid awk name, as an operand or `-v`.
pub fn split_assignment(arg: &[u8]) -> Option<(&[u8], &[u8])> {
    let eq = arg.iter().position(|&b| b == b'=')?;
    let name = &arg[..eq];
    let first = *name.first()?;
    if !(first == b'_' || first.is_ascii_alphabetic()) {
        return None;
    }
    if !name.iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric()) {
        return None;
    }
    Some((name, &arg[eq + 1..]))
}

/// Splits `text` into fields as FS (or split's separator) says.
pub fn split_into(text: &[u8], fs: &Fs, paragraph: bool, utf8: bool, mut f: impl FnMut(&[u8])) {
    match fs {
        Fs::Space => {
            for w in text
                .split(|&b| b == b' ' || b == b'\t' || b == b'\n')
                .filter(|w| !w.is_empty())
            {
                f(w);
            }
        }
        _ if text.is_empty() => {}
        Fs::Char(c) => {
            let mut emit = |part: &[u8]| {
                if paragraph {
                    for p in part.split(|&b| b == b'\n') {
                        f(p);
                    }
                } else {
                    f(part);
                }
            };
            if c.len() == 1 {
                for part in text.split(|&b| b == c[0]) {
                    emit(part);
                }
            } else {
                let mut start = 0;
                let mut i = 0;
                while i + c.len() <= text.len() {
                    if &text[i..i + c.len()] == c.as_slice() {
                        emit(&text[start..i]);
                        i += c.len();
                        start = i;
                    } else {
                        i += 1;
                    }
                }
                emit(&text[start..]);
            }
        }
        Fs::Empty => {
            if utf8 {
                let mut pos = 0;
                for len in crate::textio::char_lengths(text) {
                    f(&text[pos..pos + len]);
                    pos += len;
                }
            } else {
                for k in 0..text.len() {
                    f(&text[k..k + 1]);
                }
            }
        }
        Fs::Regex(re) => {
            let mut emit = |part: &[u8]| {
                if paragraph {
                    for p in part.split(|&b| b == b'\n') {
                        f(p);
                    }
                } else {
                    f(part);
                }
            };
            let mut start = 0;
            let mut from = 0;
            while from <= text.len() {
                let Some((ms, me)) = find_nonempty(re, text, from) else {
                    break;
                };
                emit(&text[start..ms]);
                start = me;
                from = start;
            }
            emit(&text[start..]);
        }
    }
}

/// sub/gsub: the new text (None if nothing matched) and the number of replacements.
/// In the replacement `&` is the match, `\&` a literal `&`, `\\&` a backslash and the
/// match (POSIX, as gawk).
pub fn substitute(
    re: &Regex,
    repl: &[u8],
    text: &[u8],
    global: bool,
    utf8: bool,
) -> (Option<Vec<u8>>, usize) {
    let mut out = Vec::new();
    let mut count = 0;
    let mut pos = 0;
    // Whether the last match ended here (an empty match right after it is skipped).
    let mut last_end: Option<usize> = None;
    while pos <= text.len() {
        let Some((ms, me)) = re.find_at(text, pos, ExecOptions::default()) else {
            break;
        };
        if ms == me && last_end == Some(ms) {
            // An empty match where the previous one ended: step over a character.
            if ms >= text.len() {
                break;
            }
            let step = char_step(text, ms, utf8);
            out.extend_from_slice(&text[pos..ms + step]);
            pos = ms + step;
            last_end = None;
            continue;
        }
        out.extend_from_slice(&text[pos..ms]);
        expand_repl(&mut out, repl, &text[ms..me]);
        count += 1;
        last_end = Some(me);
        if ms == me {
            // Empty match: copy one character and go on.
            if me >= text.len() {
                pos = text.len() + 1;
                break;
            }
            let step = char_step(text, me, utf8);
            out.extend_from_slice(&text[me..me + step]);
            pos = me + step;
        } else {
            pos = me;
        }
        if !global {
            break;
        }
    }
    if count == 0 {
        return (None, 0);
    }
    if pos < text.len() {
        out.extend_from_slice(&text[pos..]);
    }
    (Some(out), count)
}

fn char_step(text: &[u8], at: usize, utf8: bool) -> usize {
    if utf8 {
        crate::textio::char_lengths(&text[at..]).next().unwrap_or(1)
    } else {
        1
    }
}

fn expand_repl(out: &mut Vec<u8>, repl: &[u8], matched: &[u8]) {
    // gawk's rules: before `&`, a run of k backslashes gives k/2 of them and then a
    // literal `&` (k odd) or the match (k even); elsewhere backslashes stay as they are.
    let mut i = 0;
    while i < repl.len() {
        match repl[i] {
            b'\\' => {
                let k = repl[i..].iter().take_while(|&&b| b == b'\\').count();
                if repl.get(i + k) == Some(&b'&') {
                    out.extend(std::iter::repeat_n(b'\\', k / 2));
                    if k % 2 == 1 {
                        out.push(b'&');
                    } else {
                        out.extend_from_slice(matched);
                    }
                    i += k + 1;
                } else {
                    out.extend(std::iter::repeat_n(b'\\', k));
                    i += k;
                }
            }
            b'&' => {
                out.extend_from_slice(matched);
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
}

/// gawk's gensub: `\0`/`&` the match, `\N` group N; `which` 0 = all, else the Nth.
fn gensub(re: &Regex, repl: &[u8], text: &[u8], which: usize, utf8: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut n = 0;
    let mut last_end: Option<usize> = None;
    while pos <= text.len() {
        let Some(caps) = re.exec_at(text, pos, ExecOptions::default()) else {
            break;
        };
        let (ms, me) = caps[0].unwrap();
        if ms == me && last_end == Some(ms) {
            if ms >= text.len() {
                break;
            }
            let step = char_step(text, ms, utf8);
            out.extend_from_slice(&text[pos..ms + step]);
            pos = ms + step;
            last_end = None;
            continue;
        }
        n += 1;
        out.extend_from_slice(&text[pos..ms]);
        if which == 0 || n == which {
            let mut i = 0;
            while i < repl.len() {
                match repl[i] {
                    b'\\' if i + 1 < repl.len() && repl[i + 1].is_ascii_digit() => {
                        let g = (repl[i + 1] - b'0') as usize;
                        if let Some(Some((cs, ce))) = caps.get(g) {
                            out.extend_from_slice(&text[*cs..*ce]);
                        }
                        i += 2;
                    }
                    b'\\' if repl.get(i + 1) == Some(&b'&') => {
                        out.push(b'&');
                        i += 2;
                    }
                    b'\\' if repl.get(i + 1) == Some(&b'\\') => {
                        out.push(b'\\');
                        i += 2;
                    }
                    b'&' => {
                        out.extend_from_slice(&text[ms..me]);
                        i += 1;
                    }
                    c => {
                        out.push(c);
                        i += 1;
                    }
                }
            }
        } else {
            out.extend_from_slice(&text[ms..me]);
        }
        last_end = Some(me);
        if ms == me {
            if me >= text.len() {
                pos = text.len() + 1;
                break;
            }
            let step = char_step(text, me, utf8);
            out.extend_from_slice(&text[me..me + step]);
            pos = me + step;
        } else {
            pos = me;
        }
        if which != 0 && n >= which {
            break;
        }
    }
    if pos < text.len() {
        out.extend_from_slice(&text[pos..]);
    }
    out
}

/// The slot in `Interp::assigned` of a special variable that keeps an assigned value.
fn assigned_slot(sp: Special) -> Option<usize> {
    Some(match sp {
        Special::Nr => 0,
        Special::Fnr => 1,
        Special::Rstart => 2,
        Special::Rlength => 3,
        Special::Argc => 4,
        Special::Nf => 5,
        _ => return None,
    })
}

/// The generator's state for a seed: the seed's integer, 0 the same as 1 (as glibc's
/// srandom, which gawk uses: `srand(1)` and the initial seed 0 give one sequence).
fn seed_state(seed: f64) -> u64 {
    let n = seed as i64 as u64;
    let n = if n == 0 { 1 } else { n };
    n ^ 0x853c_49e6_748f_ea9b
}
