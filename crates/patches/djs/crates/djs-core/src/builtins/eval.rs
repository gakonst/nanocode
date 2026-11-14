//! Dynamic code (0059, D014, D1590): the leaves that compile source text at run time for
//! indirect `eval` and the four dynamic function constructors. They compile only. The
//! builtins (builtins/eval.ts) call the result, so running the code is an ordinary JS
//! call: fuel, suspension and snapshots work as for any other code.

use alloc::vec::Vec;

use crate::compiler::{self, CompileErrorKind, CompileOptions, DynamicKind};
use crate::heap::{Heap, HeapError};
use crate::interp::conv::Fault;
use crate::interp::{Intrinsic, realm};
use crate::value::{Str, Value};

/// The file name of dynamic code in stack traces, as qjs names it.
const FILE: &str = "<input>";

/// The source as UTF-8 for the compiler. Lone surrogates (which UTF-8 cannot hold)
/// become U+FFFD.
fn utf8(heap: &Heap, s: Str) -> Vec<u8> {
    heap.str_to_string(s).into_bytes()
}

/// A SyntaxError in dynamic code: its message and position (line, column) in the
/// source as qjs numbers it, which the caller puts in the stack as qjs's first
/// `<input>:line:col` frame (0106).
pub(crate) struct SyntaxAt {
    pub message: alloc::string::String,
    pub line: u32,
    pub col: u32,
}

/// What compiling dynamic code gives: the closure, or a SyntaxError with its position.
pub(crate) type Compiled = Result<Result<Value, SyntaxAt>, Fault>;

/// `col_shift`: what qjs's source has more at the start of line 1 (the Function
/// constructor's source starts with `(`).
fn compile_error(e: compiler::CompileError, col_shift: u32) -> Compiled {
    match e.kind {
        CompileErrorKind::Syntax => Ok(Err(SyntaxAt {
            message: e.message,
            line: e.line,
            col: if e.line == 1 {
                e.col + col_shift
            } else {
                e.col
            },
        })),
        CompileErrorKind::OutOfMemory => Err(Fault::heap(HeapError::OutOfMemory)),
        CompileErrorKind::HostOutOfMemory => Err(Fault::heap(HeapError::HostOutOfMemory)),
    }
}

/// A closure for a top-level template (as `Vm::instantiate`).
fn instantiate(heap: &mut Heap, template: crate::heap::layout::Ref) -> Result<Value, Fault> {
    let proto = realm::intrinsic_obj(heap, Intrinsic::FunctionProto);
    let f = heap.alloc_closure(Some(proto), template.0, 0)?;
    Ok(Value::object(f))
}

/// PerformEval's parse (19.2.1.1) for indirect eval: the source as eval code
/// (`Goal::Eval`: top-level declarations are locals of the call, strict eval steps
/// 16-17), returned as a closure that the builtin calls with `this` = the global object;
/// the call returns the completion value.
pub(crate) fn compile_eval(heap: &mut Heap, src: Str) -> Compiled {
    let bytes = utf8(heap, src);
    match compiler::compile(heap, &bytes, &CompileOptions::eval(FILE)) {
        Ok(template) => Ok(Ok(instantiate(heap, template)?)),
        Err(e) => compile_error(e, 0),
    }
}

/// CreateDynamicFunction's parse (20.2.1.1.1, steps 6-17): builds the sourceText from the
/// parameter text (already joined with ",") and the body, and compiles it as a Script
/// whose call returns the function.
pub(crate) fn compile_function(
    heap: &mut Heap,
    kind: DynamicKind,
    params: Str,
    body: Str,
) -> Compiled {
    const NAME: &str = " anonymous(";
    const OPEN: &str = "\n) {\n";
    const CLOSE: &str = "\n}";
    let mut src = alloc::string::String::with_capacity(
        kind.prefix().len()
            + NAME.len()
            + heap.str_len(params) as usize
            + OPEN.len()
            + heap.str_len(body) as usize
            + CLOSE.len(),
    );
    src.push_str(kind.prefix());
    src.push_str(NAME);
    heap.push_str_to(params, &mut src);
    let params_close = src.len() as u32 + 1;
    src.push_str(OPEN);
    heap.push_str_to(body, &mut src);
    src.push_str(CLOSE);
    let src = src.into_bytes();
    match compiler::compile_dynamic_function(
        heap,
        &src,
        &CompileOptions::script(FILE),
        kind,
        params_close,
    ) {
        Ok(template) => Ok(Ok(instantiate(heap, template)?)),
        Err(e) => compile_error(e, 1),
    }
}
