//! Suspension maps as bytes (D3041): the compile cache's form of a dump's maps, which a
//! fresh VM imports instead of compiling its scripts again with recording.

use alloc::string::String;
use alloc::vec::Vec;

use super::smap::{
    Decl, DeclKind, FunctionMap, Node, NodeKind, Part, Role, Site, SiteKind, Slot, Special, Step,
    Syntax,
};

/// A value that encodes itself.
pub(crate) trait Code: Sized {
    fn put(&self, out: &mut Vec<u8>);
    fn get(r: &mut Reader<'_>) -> Option<Self>;
}

/// Reads encoded values.
pub(crate) struct Reader<'a> {
    pub b: &'a [u8],
    pub at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
}

impl Code for u32 {
    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_le_bytes());
    }
    fn get(r: &mut Reader<'_>) -> Option<u32> {
        r.take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
}

impl Code for u16 {
    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_le_bytes());
    }
    fn get(r: &mut Reader<'_>) -> Option<u16> {
        r.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
}

impl Code for u8 {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(*self);
    }
    fn get(r: &mut Reader<'_>) -> Option<u8> {
        r.take(1).map(|b| b[0])
    }
}

impl Code for bool {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
    fn get(r: &mut Reader<'_>) -> Option<bool> {
        u8::get(r).map(|b| b != 0)
    }
}

impl Code for String {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        out.extend_from_slice(self.as_bytes());
    }
    fn get(r: &mut Reader<'_>) -> Option<String> {
        let n = u32::get(r)? as usize;
        core::str::from_utf8(r.take(n)?).ok().map(String::from)
    }
}

impl<T: Code> Code for Vec<T> {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        for x in self {
            x.put(out);
        }
    }
    fn get(r: &mut Reader<'_>) -> Option<Vec<T>> {
        let n = u32::get(r)? as usize;
        // (No allocation before the elements are there.)
        let mut v = Vec::with_capacity(n.min(r.b.len() - r.at.min(r.b.len())));
        for _ in 0..n {
            v.push(T::get(r)?);
        }
        Some(v)
    }
}

impl<T: Code> Code for Option<T> {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Some(x) => {
                out.push(1);
                x.put(out);
            }
            None => out.push(0),
        }
    }
    fn get(r: &mut Reader<'_>) -> Option<Option<T>> {
        match u8::get(r)? {
            0 => Some(None),
            1 => T::get(r).map(Some),
            _ => None,
        }
    }
}

impl<A: Code, B: Code> Code for (A, B) {
    fn put(&self, out: &mut Vec<u8>) {
        self.0.put(out);
        self.1.put(out);
    }
    fn get(r: &mut Reader<'_>) -> Option<(A, B)> {
        Some((A::get(r)?, B::get(r)?))
    }
}

/// Fieldless enums by their index in a list of every variant.
macro_rules! unit_enum {
    ($t:ident: $($v:ident),* $(,)?) => {
        impl Code for $t {
            fn put(&self, out: &mut Vec<u8>) {
                const ALL: &[$t] = &[$($t::$v),*];
                out.push(ALL.iter().position(|x| x == self).unwrap_or(0) as u8);
            }
            fn get(r: &mut Reader<'_>) -> Option<$t> {
                const ALL: &[$t] = &[$($t::$v),*];
                ALL.get(u8::get(r)? as usize).copied()
            }
        }
    };
}

/// Structs field by field.
macro_rules! record {
    ($t:ident { $($f:ident),* $(,)? }) => {
        impl Code for $t {
            fn put(&self, out: &mut Vec<u8>) {
                $(self.$f.put(out);)*
            }
            fn get(r: &mut Reader<'_>) -> Option<$t> {
                Some($t { $($f: Code::get(r)?),* })
            }
        }
    };
}

unit_enum!(SiteKind: Await, Yield, AsyncYieldAwait, AsyncYield, AsyncYieldReturn, AsyncReturn,
    ForAwaitNext, ForAwaitClose, AwaitUsing, YieldStar, YieldStarAwait);
unit_enum!(Special: Arguments, NewTarget, Super);
unit_enum!(DeclKind: Var, Let, Const, Using, AwaitUsing, Class, Function, Param, CatchParam,
    FuncName);
unit_enum!(Role: Params, Stmt, FuncDecl, Body, Test, Then, Else, Init, Update, Head, Iterable,
    Label, Block, CatchParam, CatchBody, Finally, Discriminant, Case, Default, Operand, Op, Left,
    Right, Target, Callee, Open, Arg, Spread, Object, Property, Element, Hole, Quasi, Subst, Tag,
    Rest, Key, ComputedKey, Value, Shorthand, Method, Expr);

impl Code for NodeKind {
    fn put(&self, out: &mut Vec<u8>) {
        use NodeKind::*;
        const UNIT: &[NodeKind] = &[
            Body,
            Block,
            Expr,
            If,
            Label,
            While,
            DoWhile,
            For,
            ForIn,
            ForOf,
            ForAwait,
            Try,
            Switch,
            Return,
            Throw,
            Await,
            Yield,
            YieldStar,
            New,
            ImportCall,
            Binary,
            Logical,
            Assign,
            Unary,
            Update,
            Conditional,
            Sequence,
            Template,
            TaggedTemplate,
            Array,
            Object,
            ArrayPattern,
            ObjectPattern,
            Default,
        ];
        match *self {
            Decl(k) => {
                out.push(200);
                k.put(out);
            }
            Call { optional } => {
                out.push(201);
                optional.put(out);
            }
            Member { computed, optional } => {
                out.push(202);
                computed.put(out);
                optional.put(out);
            }
            k => out.push(UNIT.iter().position(|x| *x == k).unwrap_or(0) as u8),
        }
    }
    fn get(r: &mut Reader<'_>) -> Option<NodeKind> {
        use NodeKind::*;
        const UNIT: &[NodeKind] = &[
            Body,
            Block,
            Expr,
            If,
            Label,
            While,
            DoWhile,
            For,
            ForIn,
            ForOf,
            ForAwait,
            Try,
            Switch,
            Return,
            Throw,
            Await,
            Yield,
            YieldStar,
            New,
            ImportCall,
            Binary,
            Logical,
            Assign,
            Unary,
            Update,
            Conditional,
            Sequence,
            Template,
            TaggedTemplate,
            Array,
            Object,
            ArrayPattern,
            ObjectPattern,
            Default,
        ];
        Some(match u8::get(r)? {
            200 => Decl(DeclKind::get(r)?),
            201 => Call {
                optional: bool::get(r)?,
            },
            202 => Member {
                computed: bool::get(r)?,
                optional: bool::get(r)?,
            },
            i => *UNIT.get(i as usize)?,
        })
    }
}

impl Code for Slot {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Slot::Binding { name, scope } => {
                out.push(0);
                name.put(out);
                scope.put(out);
            }
            Slot::Hidden(s) => {
                out.push(1);
                s.put(out);
            }
            Slot::Argument(i) => {
                out.push(2);
                i.put(out);
            }
            Slot::Temp(s) => {
                out.push(3);
                s.put(out);
            }
            Slot::Nothing => out.push(4),
        }
    }
    fn get(r: &mut Reader<'_>) -> Option<Slot> {
        Some(match u8::get(r)? {
            0 => Slot::Binding {
                name: String::get(r)?,
                scope: u32::get(r)?,
            },
            1 => Slot::Hidden(String::get(r)?),
            2 => Slot::Argument(u16::get(r)?),
            3 => Slot::Temp(String::get(r)?),
            4 => Slot::Nothing,
            _ => return None,
        })
    }
}

record!(Step { node, part });
record!(Part { role, start, end });
record!(Decl {
    name,
    kind,
    scope,
    at,
    part
});
record!(Node {
    kind,
    start,
    end,
    parts,
    decls
});
record!(Syntax {
    nodes,
    special,
    erased
});
record!(Site {
    name,
    kind,
    pos,
    resume_pc,
    args,
    locals,
    boxed,
    stack,
    unsupported,
    path,
    gap
});
record!(FunctionMap {
    name,
    start,
    end,
    sites,
    computed_keys,
    syntax
});
