//! Class operations (0040, docs/compiler.md 7.9): class and method definition, home
//! objects, `super`, derived constructors and private names.
//!
//! Private names are private symbols (D1400): symbol cells flagged `SYM_PRIVATE`, used
//! as ordinary property keys of the object's own table. [[OwnPropertyKeys]] skips them,
//! so reflection, copying, JSON and freezing never see them. The private ops below
//! read and write the table directly: never the prototype chain, and never a proxy
//! trap (a private name on a proxy is the proxy object's own).

use super::{Exec, Got, Step};
use crate::bytecode::Op;
use crate::bytecode::template::func_flags;
use crate::heap::Lookup;
use crate::heap::cells::FuncTemplate;
use crate::heap::cells::flags::{SYM_PRIVATE_METHOD, SYM_PRIVATE_READONLY};
use crate::heap::{Atom, ObjectClass, PropFlags, Property, PropertyDescriptor, atoms};
use crate::interp::conv::{self, Fault, type_error};
use crate::interp::frame;
use crate::interp::frame::Continuation;
use crate::interp::realm::{ErrorKind, Intrinsic, intrinsic_obj};
use crate::value::{Obj, Value};

const PROXY_CREATE_DATA_PROPERTY_OR_THROW: u16 =
    super::intrinsics::id("ProxyCreateDataPropertyOrThrow");

fn reference_error<T>(msg: &str) -> Result<T, Fault> {
    Err(Fault::error(ErrorKind::ReferenceError, msg.into()))
}

impl Exec<'_> {
    /// Executes a class op (the ops between `PrivateSymbol` and `DefineClassField`
    /// and the constructor checks).
    pub(super) fn class_op(&mut self, op: Op, next: u32) -> Step {
        match op {
            Op::PrivateSymbol => {
                let atom = self.imm_atom();
                let flags =
                    self.byte(self.pc + 5) as u16 & (SYM_PRIVATE_METHOD | SYM_PRIVATE_READONLY);
                let desc = self.heap.atom_string(atom)?;
                let sym = self.heap.new_private_symbol(desc, flags)?;
                self.push(Value::symbol(sym));
            }
            Op::GetPrivateField => {
                let (o, key) = (self.top(1), self.private_key(self.top(0)));
                let obj = self.private_target(o)?;
                match self.private_own(obj, key)? {
                    Property::Data { value, .. } => {
                        self.sp -= 2;
                        self.push(value);
                    }
                    Property::Accessor { getter, .. } if getter.is_undefined() => {
                        return self.private_read_only(key);
                    }
                    Property::Accessor { getter, .. } => {
                        self.sp -= 2;
                        self.pc = next;
                        return self.call_accessor(getter, o, None);
                    }
                }
            }
            Op::PutPrivateField => {
                let (o, key, v) = (self.top(2), self.private_key(self.top(1)), self.top(0));
                let obj = self.private_target(o)?;
                let sym = self.heap.atom_symbol(key).expect("private name");
                if self.heap.symbol_private_flags(sym) & SYM_PRIVATE_READONLY != 0 {
                    return self.private_read_only(key);
                }
                match self.private_own(obj, key)? {
                    Property::Data { flags, .. } if flags.writable() => {
                        self.heap.set_own_data_value(obj, key, v);
                        self.sp -= 3;
                    }
                    Property::Accessor { setter, .. } if !setter.is_undefined() => {
                        self.sp -= 3;
                        self.pc = next;
                        return self.call_accessor(setter, o, Some(v));
                    }
                    _ => return self.private_read_only(key),
                }
            }
            Op::DefinePrivateField => {
                let kind = self.byte(self.pc + 1);
                let (o, key, v) = (self.top(2), self.private_key(self.top(1)), self.top(0));
                let obj = o.as_object().expect("private field target");
                self.private_add_check(obj)?;
                if self.heap.has_own_property(obj, key) {
                    return type_error(&if kind == 0 {
                        crate::text::cat!(
                            "private class field '",
                            conv::describe_key(self.heap, key),
                            "' already exists"
                        )
                    } else {
                        "private method is already present".into()
                    });
                }
                // Fields are writable; methods are not (an assignment is a TypeError).
                let flags = if kind == 0 {
                    PropFlags::WRITABLE
                } else {
                    PropFlags::NONE
                };
                self.heap
                    .add_private_property(obj, key, &Property::data(v, flags))?;
                self.sp -= 2;
            }
            Op::DefinePrivateAccessor => {
                let (o, key) = (self.top(3), self.private_key(self.top(2)));
                let (getter, setter) = (self.top(1), self.top(0));
                let obj = o.as_object().expect("private accessor target");
                self.private_add_check(obj)?;
                if self.heap.has_own_property(obj, key) {
                    return type_error("private method is already present");
                }
                let p = Property::Accessor {
                    getter,
                    setter,
                    flags: PropFlags::NONE,
                };
                self.heap.add_private_property(obj, key, &p)?;
                self.sp -= 3;
            }
            Op::PrivateIn => {
                let (key, o) = (self.private_key(self.top(1)), self.top(0));
                let Some(obj) = o.as_object() else {
                    return type_error("invalid 'in' operand");
                };
                let r = self.heap.has_own_property(obj, key);
                self.sp -= 2;
                self.push(Value::bool(r));
            }
            Op::GetSuper => {
                let o = self.top(0).as_object().expect("home object or function");
                let p = self
                    .heap
                    .get_prototype_of(o)
                    .map_or(Value::NULL, Value::object);
                self.set(self.sp - 1, p);
            }
            Op::GetSuperField | Op::GetSuperElem => {
                let elem = op == Op::GetSuperElem;
                let n = 2 + elem as u32;
                let (this, base) = (self.top(n - 1), self.top(n - 2));
                let key = if elem {
                    self.property_key(base, self.top(0))?
                } else {
                    self.imm_atom()
                };
                if base.is_null() {
                    return self.nullish_base(base, key, "read");
                }
                match self.lookup(base, key)? {
                    Got::Value(v) => {
                        self.sp -= n;
                        self.push(v);
                    }
                    Got::Getter(getter) => {
                        self.sp -= n;
                        self.pc = next;
                        return self.call_accessor(getter, this, None);
                    }
                    Got::Proxy(p) => {
                        self.sp -= n;
                        self.pc = next;
                        return self.call_proxy_get(p, key, this, Continuation::PushResult);
                    }
                }
            }
            Op::PutSuperField | Op::PutSuperElem => {
                let elem = op == Op::PutSuperElem;
                let n = 3 + elem as u32;
                let (this, base, v) = (self.top(n - 1), self.top(n - 2), self.top(0));
                let key = if elem {
                    self.property_key(base, self.top(1))?
                } else {
                    self.imm_atom()
                };
                self.pc = next;
                return self.set_super(this, base, key, v, n);
            }
            Op::SetHomeObject => {
                let (home, f) = (self.top(1), self.top(0));
                self.set_home(f, home);
            }
            Op::DefineClass => {
                let heritage = self.byte(self.pc + 1) & 1 != 0;
                let (parent, proto_parent, ctor) = (self.top(2), self.top(1), self.top(0));
                let f = ctor.as_object().expect("class constructor");
                let proto_parent = if heritage {
                    proto_parent.as_object()
                } else {
                    Some(intrinsic_obj(self.heap, Intrinsic::ObjectProto))
                };
                if heritage && let Some(p) = parent.as_object() {
                    self.heap.set_prototype_of(f, Some(p));
                }
                let proto = self.heap.new_object(proto_parent)?;
                let d = PropertyDescriptor::data(Value::object(proto), PropFlags::NONE);
                self.heap.define_own_property(f, atoms::PROTOTYPE, &d)?;
                crate::interp::realm::define_hidden(self.heap, proto, atoms::CONSTRUCTOR, ctor)?;
                self.set_home(ctor, Value::object(proto));
                self.sp -= 3;
                self.push(ctor);
                self.push(Value::object(proto));
            }
            Op::DefineClassMethod => {
                let key = self.imm_atom();
                let kind = self.byte(self.pc + 5);
                let (o, f) = (self.top(1), self.top(0));
                self.define_class_member(o, key, f, kind)?;
                self.sp -= 1;
            }
            Op::DefineClassMethodElem => {
                let kind = self.byte(self.pc + 1);
                let (o, k, f) = (self.top(2), self.top(1), self.top(0));
                let key = self.property_key(o, k)?;
                self.define_class_member(o, key, f, kind)?;
                self.sp -= 2;
            }
            Op::DefineClassField => {
                let (o, k, v) = (self.top(2), self.top(1), self.top(0));
                let key = self.property_key(o, k)?;
                let obj = o.as_object().expect("class field target");
                if self.heap.is_proxy(obj) {
                    // The proxy's defineProperty trap.
                    self.sp -= 2;
                    self.pc = next;
                    let k = self.heap.atom_to_value(key)?;
                    return self.call_prelude(
                        PROXY_CREATE_DATA_PROPERTY_OR_THROW,
                        &[o, k, v],
                        Continuation::DiscardResult,
                    );
                }
                // CreateDataPropertyOrThrow.
                if !self.heap.create_data_property(obj, key, v)? {
                    return type_error(if self.heap.has_own_property(obj, key) {
                        "could not define property"
                    } else {
                        "object is not extensible"
                    });
                }
                self.sp -= 2;
            }
            Op::SuperCall | Op::SuperCallSpread => {
                let nt = self.pop();
                let argc = if op == Op::SuperCall {
                    self.imm_u16(1)
                } else {
                    let args = self.top(0);
                    self.spread_args(args)?
                };
                // qjs's messages: the super constructor is the class's [[Prototype]].
                let f = self.top(argc);
                match f.as_object() {
                    None => return type_error("not a function"),
                    Some(fo) if !self.is_constructor(fo) => {
                        return type_error(&self.not_a_constructor_message(f));
                    }
                    Some(_) => {}
                }
                return self.construct_with(argc, next, Some(nt));
            }
            Op::CheckCtor => {
                if self.header_int(frame::FLAGS) as i32 & frame::FLAG_CONSTRUCT == 0 {
                    return type_error("class constructors must be invoked with 'new'");
                }
            }
            Op::CheckThis => {
                if self.top(0).is_undefined() {
                    return reference_error("this is not initialized");
                }
            }
            Op::CheckThisUninit => {
                if !self.pop().is_undefined() {
                    return reference_error("'this' can be initialized only once");
                }
            }
            Op::CheckCtorReturn => {
                let v = self.top(0);
                if !v.is_undefined() && v.as_object().is_none() {
                    return type_error(
                        "derived class constructor must return an object or undefined",
                    );
                }
            }
            Op::ThrowError => {
                let msg = self.heap.atom_name(self.imm_atom());
                let kind = match self.byte(self.pc + 5) {
                    1 => ErrorKind::ReferenceError,
                    _ => ErrorKind::TypeError,
                };
                return Err(Fault::error(kind, msg));
            }
            _ => unreachable!("not a class op: {}", op.name()),
        }
        self.pc = next;
        Ok(None)
    }

    /// The key of a private name value (a private symbol, made by `private_symbol`).
    fn private_key(&self, sym: Value) -> Atom {
        self.heap
            .symbol_atom(sym.as_symbol().expect("private name"))
    }

    /// The object of a private access: TypeError for primitives (qjs's message).
    fn private_target(&self, o: Value) -> Result<Obj, Fault> {
        match o.as_object() {
            Some(obj) => Ok(obj),
            None => type_error("not an object"),
        }
    }

    /// The object's own private element `key`: a TypeError if it has none (the brand
    /// check, for methods and accessors).
    fn private_own(&self, obj: Obj, key: Atom) -> Result<Property, Fault> {
        if let Some(p) = self.heap.get_own_property(obj, key) {
            return Ok(p);
        }
        let sym = self.heap.atom_symbol(key).expect("private name");
        if self.heap.symbol_private_flags(sym) & SYM_PRIVATE_METHOD != 0 {
            return type_error("invalid brand on object");
        }
        type_error(&crate::text::cat!(
            "private class field '",
            conv::describe_key(self.heap, key),
            "' does not exist"
        ))
    }

    /// PrivateFieldAdd and PrivateMethodOrAccessorAdd: a non-extensible object takes
    /// no new private elements (nonextensible-applies-to-private; qjs still adds them).
    /// A proxy keeps them on the proxy object itself, which the heap marks
    /// non-extensible (its [[IsExtensible]] would be the trap: not consulted).
    fn private_add_check(&self, obj: Obj) -> Result<(), Fault> {
        if !self.heap.is_proxy(obj) && !self.heap.is_extensible(obj) {
            return type_error("object is not extensible");
        }
        Ok(())
    }

    fn private_read_only<T>(&self, key: Atom) -> Result<T, Fault> {
        type_error(&crate::text::cat!(
            "'",
            conv::describe_key(self.heap, key),
            "' is read-only"
        ))
    }

    /// The home object slot of closure `f`, if its template has one (`func_flags::HOME`):
    /// after the captures.
    pub(super) fn home_slot(&self, f: Obj) -> Option<u32> {
        if self.heap.object_class(f) != ObjectClass::Function {
            return None;
        }
        let t = self.template_of(f);
        let a = self.heap.arena();
        if u32::from(a.get::<u16>(t, FuncTemplate::FLAGS)) & func_flags::HOME == 0 {
            return None;
        }
        Some(a.get(t, FuncTemplate::CAPTURES) as u32)
    }

    /// Sets the home object of `f` (if it has a home slot).
    pub(super) fn set_home(&mut self, f: Value, home: Value) {
        if let Some(fo) = f.as_object()
            && let Some(slot) = self.home_slot(fo)
        {
            self.heap.set_object_slot(fo, slot, home);
        }
    }

    /// A class method (`kind` 0), getter (1) or setter (2): DefinePropertyOrThrow,
    /// non-enumerable; sets the home object.
    fn define_class_member(
        &mut self,
        o: Value,
        key: Atom,
        f: Value,
        kind: u8,
    ) -> Result<(), Fault> {
        let obj = o.as_object().expect("class or prototype");
        let d = match kind {
            0 => PropertyDescriptor::data(f, PropFlags::WRITABLE | PropFlags::CONFIGURABLE),
            1 => PropertyDescriptor {
                get: Some(f),
                enumerable: Some(false),
                configurable: Some(true),
                ..Default::default()
            },
            _ => PropertyDescriptor {
                set: Some(f),
                enumerable: Some(false),
                configurable: Some(true),
                ..Default::default()
            },
        };
        if !self.heap.define_own_property(obj, key, &d)? {
            return type_error("could not define property");
        }
        self.set_home(f, o);
        Ok(())
    }

    /// `super[key] = v`: base.[[Set]](key, v, this) (OrdinarySet with a receiver that is
    /// not the base). The top `n` operands are consumed.
    fn set_super(&mut self, this: Value, base: Value, key: Atom, v: Value, n: u32) -> Step {
        let Some(b) = base.as_object() else {
            return type_error("not an object");
        };
        let read_only = |heap: &crate::heap::Heap| {
            type_error(&crate::text::cat!(
                "'",
                conv::describe_key(heap, key),
                "' is read-only"
            ))
        };
        let found = match self.heap.lookup_property(b, key) {
            Lookup::Found(h, p) => Some((h, p)),
            Lookup::Absent => None,
            Lookup::Proxy(p) => {
                self.sp -= n;
                let k = self.heap.atom_to_value(key)?;
                let args = [Value::object(p), k, v, this];
                return self.call_prelude(
                    super::proxy::PROXY_SET_OR_THROW,
                    &args,
                    Continuation::DiscardResult,
                );
            }
        };
        match found {
            Some((_, Property::Accessor { setter, .. })) => {
                if setter.is_undefined() {
                    // qjs's message (without the key).
                    return type_error("no setter for property");
                }
                self.sp -= n;
                return self.call_accessor(setter, this, Some(v));
            }
            // The override fix (D2304): see `ordinary_set`.
            Some((holder, Property::Data { flags, .. }))
                if !flags.writable()
                    && !(self.heap.frozen(holder) && this.as_object() != Some(holder)) =>
            {
                return read_only(self.heap);
            }
            _ => {}
        }
        let Some(r) = this.as_object() else {
            return type_error("not an object");
        };
        if self.heap.is_proxy(r) {
            // Its getOwnPropertyDescriptor and defineProperty traps. (A false result is
            // not a TypeError here yet.)
            self.sp -= n;
            let k = self.heap.atom_to_value(key)?;
            return self.call_prelude(
                super::proxy::PROXY_SET_ON_RECEIVER,
                &[this, k, v],
                Continuation::DiscardResult,
            );
        }
        match self.heap.get_own_property(r, key) {
            Some(Property::Accessor { .. }) => return read_only(self.heap),
            Some(Property::Data { flags, .. }) => {
                if !flags.writable() {
                    return read_only(self.heap);
                }
                let d = PropertyDescriptor {
                    value: Some(v),
                    ..Default::default()
                };
                if !self.heap.define_own_property(r, key, &d)? {
                    return read_only(self.heap);
                }
            }
            None => {
                if !self.heap.create_data_property(r, key, v)? {
                    return type_error("object is not extensible");
                }
            }
        }
        self.sp -= n;
        Ok(None)
    }
}
