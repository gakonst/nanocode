//! `lazy` (D2990): the code of a lazy template. The frame was pushed from the lazy
//! template (no locals, no operand stack); the function is compiled now (once per
//! template: its `IC` field keeps the compiled template), the closure is switched to
//! the compiled template, and the frame continues there at pc 0.
use super::*;
use crate::compiler::{CompileErrorKind, compile_lazy};
use crate::heap::cells::{FuncInfo, Object};

impl Exec<'_> {
    pub(super) fn lazy(&mut self) -> Step {
        let t = self.template;
        let mut c: Ref = self.heap.arena().get(t, FuncTemplate::IC);
        if c.get().is_none() {
            c = compile_lazy(self.heap, Ref(t)).map_err(|e| match e.kind {
                CompileErrorKind::OutOfMemory => Fault::heap(HeapError::OutOfMemory),
                CompileErrorKind::HostOutOfMemory => Fault::heap(HeapError::HostOutOfMemory),
                CompileErrorKind::Syntax => Fault::error(ErrorKind::InternalError, e.text()),
            })?;
            let a = self.heap.arena_mut();
            a.set(t, FuncTemplate::IC, c);
            // Fuel in proportion to the text compiled.
            let info: Ref = a.get(t, FuncTemplate::INFO);
            if let Some(i) = info.get() {
                let len = a.get::<u32>(i, FuncInfo::SRC_END) - a.get::<u32>(i, FuncInfo::SRC_START);
                self.fuel -= i64::from(len / 32);
            }
        }
        if let Some(f) = self.get(self.bp).as_object() {
            self.heap.arena_mut().set(f.offset(), Object::ELEMENTS, c);
        }
        let a = self.heap.arena();
        let locals = u32::from(a.get::<u16>(c.0, FuncTemplate::LOCALS));
        let stack = u32::from(a.get::<u16>(c.0, FuncTemplate::STACK));
        self.ensure(self.lp + locals + stack)?;
        for i in 0..locals {
            self.set(self.lp + i, Value::UNDEFINED);
        }
        self.sp = self.lp + locals;
        self.enter_template(c.0);
        self.pc = 0;
        Ok(None)
    }
}
