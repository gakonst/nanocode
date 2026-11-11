//! Interpreter tests on hand-assembled bytecode (docs/compiler.md 7). End-to-end tests
//! through the compiler come with 0014.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};

use super::*;
use crate::bytecode::template::{TemplateParts, alloc_template};
use crate::bytecode::{CodeBuf, Handler, Label};
use crate::heap::{Atom, PropFlags, PropertyDescriptor};
use crate::interp::run::STACK_TRACE_LIMIT;

const LOG: u32 = 1;

/// A function under construction: code plus the parts of its template.
#[derive(Default)]
struct F {
    b: CodeBuf,
    consts: Vec<Value>,
    children: Vec<Ref>,
    captures: Vec<u32>,
    handlers: Vec<(Label, Label, Label, u32)>,
    names: Vec<Atom>,
    params: u16,
    locals: u16,
    flags: u32,
}

impl F {
    fn new() -> F {
        F::default()
    }

    fn build(self, vm: &mut Vm) -> Ref {
        let code = self.b.finish().unwrap();
        let handlers: Vec<Handler> = self
            .handlers
            .iter()
            .map(|&(s, e, t, depth)| Handler {
                start: code.label_pos(s).unwrap(),
                end: code.label_pos(e).unwrap(),
                target: code.label_pos(t).unwrap(),
                depth,
            })
            .collect();
        let parts = TemplateParts {
            flags: self.flags,
            params: self.params,
            locals: self.locals,
            captures: &self.captures,
            code: &code.code,
            consts: &self.consts,
            children: &self.children,
            handlers: &handlers,
            names: &self.names,
            ..Default::default()
        };
        alloc_template(vm.heap_mut(), &parts).unwrap()
    }

    fn string(&mut self, vm: &mut Vm, s: &str) -> u32 {
        let v = Value::string(vm.heap_mut().alloc_str(s).unwrap());
        self.consts.push(v);
        self.consts.len() as u32 - 1
    }
}

fn vm_with(opts: VmOptions) -> Vm {
    let mut vm = Vm::new(opts).unwrap();
    let console = vm.new_object().unwrap();
    let log = vm.new_host_function(LOG, "log").unwrap();
    vm.define(console, "log", log).unwrap();
    vm.define_global("console", Value::object(console)).unwrap();
    vm
}

fn vm() -> Vm {
    vm_with(VmOptions::default())
}

/// The outcome of a run: the completion (as displayed, or `throw ...`) and console output.
#[derive(Debug, PartialEq)]
struct Outcome {
    result: String,
    log: Vec<String>,
}

/// Stops at the requested (small) fuel before a suspending run switches to slices of
/// `SUSPEND_SLICE`, as in the e2e test: every image round trip copies the whole heap, so
/// fuel 1 throughout made a few tests take seconds each (0053).
const SUSPEND_FINE_STOPS: u32 = 1000;
const SUSPEND_SLICE: u64 = 97;

/// Runs until the call finishes, serving `console.log` and rebuilding the VM from its
/// image whenever it runs out of fuel when `suspend` is set.
fn finish(mut vm: Vm, fuel: u64, suspend: bool) -> (Vm, Outcome) {
    let mut log = Vec::new();
    let mut stops = 0;
    loop {
        stops += 1;
        let fuel = if suspend && stops > SUSPEND_FINE_STOPS {
            fuel.max(SUSPEND_SLICE)
        } else {
            fuel
        };
        match vm.run(fuel) {
            Status::Done(v) => {
                let result = vm.display(v);
                return (vm, Outcome { result, log });
            }
            Status::Threw(v) => {
                let result = format!("throw {}", vm.display(v));
                return (vm, Outcome { result, log });
            }
            Status::OutOfFuel => {
                if suspend {
                    let opts = vm.options();
                    vm = Vm::from_bytes(vm.image().to_vec(), opts).unwrap();
                }
            }
            Status::HostCall { call_id, fn_id } => {
                assert_eq!(fn_id, LOG);
                let info = vm.host_call(call_id).unwrap();
                let line: Vec<String> = info.args.iter().map(|&a| vm.display(a)).collect();
                log.push(line.join(" "));
                if suspend {
                    let opts = vm.options();
                    vm = Vm::from_bytes(vm.image().to_vec(), opts).unwrap();
                }
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
            Status::Idle => panic!("idle"),
            Status::HostOutOfMemory => {
                let result = String::from("out of host memory");
                return (vm, Outcome { result, log });
            }
        }
    }
}

/// Builds `main` with `f`, calls it, and runs it to the end in several modes, checking
/// they all agree: plenty of fuel, one unit of fuel at a time (for the first 1000 stops,
/// see `finish`) with suspension through the image, and GC stress.
fn run_main(build: impl Fn(&mut Vm) -> Ref) -> Outcome {
    let mut outcomes = Vec::new();
    for (fuel, suspend, gc_stress) in [(1 << 40, false, false), (1, true, false), (7, false, true)]
    {
        let mut vm = vm_with(VmOptions {
            exec: ExecOptions {
                gc_stress,
                ..Default::default()
            },
            ..Default::default()
        });
        let t = build(&mut vm);
        let main = vm.instantiate(t).unwrap();
        vm.call(main, Value::UNDEFINED, &[]).unwrap();
        let (vm, out) = finish(vm, fuel, suspend);
        vm.heap().verify().unwrap();
        outcomes.push(out);
    }
    assert_eq!(outcomes[0], outcomes[1], "suspension changed the outcome");
    assert_eq!(outcomes[0], outcomes[2], "gc stress changed the outcome");
    outcomes.remove(0)
}

fn result_of(build: impl Fn(&mut Vm) -> Ref) -> String {
    run_main(build).result
}

#[test]
fn arithmetic() {
    let r = result_of(|vm| {
        let mut f = F::new();
        f.b.push_i32(2);
        f.b.push_i32(3);
        f.b.add();
        f.b.push_i8(4);
        f.b.mul();
        f.b.return_();
        f.build(vm)
    });
    assert_eq!(r, "20");
    let r = result_of(|vm| {
        let mut f = F::new();
        f.b.push_i32(i32::MAX);
        f.b.push_i8(1);
        f.b.add();
        f.b.push_i8(3);
        f.b.div();
        f.b.return_();
        f.build(vm)
    });
    assert_eq!(r, "715827882.6666666");
}

/// One binary operator on two constants.
fn binop(a: Value, b: Value, op: fn(&mut CodeBuf)) -> String {
    let mut vm = vm();
    let mut f = F::new();
    f.consts = vec![a, b];
    f.b.push_const(0);
    f.b.push_const(1);
    op(&mut f.b);
    f.b.return_();
    let t = f.build(&mut vm);
    let main = vm.instantiate(t).unwrap();
    vm.call(main, Value::UNDEFINED, &[]).unwrap();
    finish(vm, 1 << 30, false).1.result
}

#[test]
fn operators() {
    let n = Value::number;
    let i = Value::int;
    type Case = (Value, Value, fn(&mut CodeBuf), &'static str);
    let cases: Vec<Case> = vec![
        (n(0.1), n(0.2), CodeBuf::add, "0.30000000000000004"),
        (i(7), i(-2), CodeBuf::mod_, "1"),
        (i(-7), i(2), CodeBuf::mod_, "-1"),
        (n(5.5), i(2), CodeBuf::mod_, "1.5"),
        (i(2), i(10), CodeBuf::pow, "1024"),
        (i(2), n(0.5), CodeBuf::pow, "1.4142135623730951"),
        (i(1), n(f64::INFINITY), CodeBuf::pow, "NaN"),
        (i(1), i(0), CodeBuf::div, "Infinity"),
        (i(-1), i(28), CodeBuf::shl, "-268435456"),
        (i(-16), i(2), CodeBuf::sar, "-4"),
        (i(-16), i(28), CodeBuf::shr, "15"),
        (i(-1), i(0), CodeBuf::shr, "4294967295"),
        (i(6), i(3), CodeBuf::band, "2"),
        (i(6), i(3), CodeBuf::bor, "7"),
        (i(6), i(3), CodeBuf::bxor, "5"),
        (i(1), i(2), CodeBuf::lt, "true"),
        (i(2), i(2), CodeBuf::le, "true"),
        (i(1), i(2), CodeBuf::gt, "false"),
        (n(f64::NAN), i(2), CodeBuf::ge, "false"),
        (n(f64::NAN), i(2), CodeBuf::lt, "false"),
        (i(1), n(1.0), CodeBuf::strict_eq, "true"),
        (n(f64::NAN), n(f64::NAN), CodeBuf::strict_ne, "true"),
        (Value::NULL, Value::UNDEFINED, CodeBuf::eq, "true"),
        (Value::NULL, Value::UNDEFINED, CodeBuf::strict_eq, "false"),
        (Value::TRUE, i(1), CodeBuf::eq, "true"),
        (Value::TRUE, i(2), CodeBuf::ne, "true"),
    ];
    for (a, b, op, want) in cases {
        assert_eq!(binop(a, b, op), want, "{a:?} {b:?}");
    }
}

#[test]
fn strings() {
    let r = run_main(|vm| {
        let mut f = F::new();
        let a = f.string(vm, "ab");
        let n = f.string(vm, "42");
        let long = f.string(vm, &"x".repeat(40));
        f.b.push_const(a);
        f.b.push_i8(1);
        f.b.add(); // "ab1"
        f.b.push_const(long);
        f.b.add(); // a rope
        f.b.get_field(crate::heap::atoms::LENGTH, 0);
        f.b.push_const(n);
        f.b.add(); // 43 + "42" -> "4342"
        f.b.push_const(n);
        f.b.push_i8(42);
        f.b.eq(); // "42" == 42
        f.b.add(); // "4342true"
        f.b.push_const(a);
        f.b.push_const(n);
        f.b.lt(); // "ab" < "42": false
        f.b.add();
        f.b.null();
        f.b.typeof_();
        f.b.add();
        f.b.return_();
        f.build(vm)
    });
    assert_eq!(r.result, "4342truefalseobject");
}

#[test]
fn loops_and_fuel() {
    // sum = 0; for (i = 0; i < 100; i++) sum += i; return sum
    let build = |vm: &mut Vm| {
        let mut f = F::new();
        f.locals = 2;
        let (top, end) = (f.b.new_label(), f.b.new_label());
        f.b.push_i8(0);
        f.b.put_loc(0);
        f.b.push_i8(0);
        f.b.put_loc(1);
        f.b.place_label(top);
        f.b.get_loc(1);
        f.b.push_i8(100);
        f.b.lt();
        f.b.jump_if_false(end);
        f.b.get_loc(0);
        f.b.get_loc(1);
        f.b.add();
        f.b.put_loc(0);
        f.b.get_loc(1);
        f.b.inc();
        f.b.put_loc(1);
        f.b.jump(top);
        f.b.place_label(end);
        f.b.get_loc(0);
        f.b.return_();
        f.build(vm)
    };
    assert_eq!(result_of(build), "4950");
    // Out of fuel: resumes exactly where it stopped.
    let mut vm = vm();
    let t = build(&mut vm);
    let main = vm.instantiate(t).unwrap();
    vm.call(main, Value::UNDEFINED, &[]).unwrap();
    let mut slices = 0;
    loop {
        match vm.run(50) {
            Status::OutOfFuel => slices += 1,
            Status::Done(v) => {
                assert_eq!(v, Value::int(4950));
                break;
            }
            s => panic!("{s:?}"),
        }
    }
    assert!(slices > 20, "{slices}");
}

#[test]
fn console_log_is_a_host_call() {
    let out = run_main(|vm| {
        let mut f = F::new();
        let console = vm.heap_mut().atom_str("console").unwrap();
        let log = vm.heap_mut().atom_str("log").unwrap();
        let hi = f.string(vm, "hi");
        f.b.get_global(console, 0);
        f.b.get_method(log, 0);
        f.b.push_const(hi);
        f.b.push_i8(42);
        f.b.null();
        f.b.call(3);
        f.b.drop();
        f.b.get_global(console, 0);
        f.b.get_method(log, 0);
        f.b.object();
        f.b.push_i8(1);
        f.b.define_field(crate::heap::atoms::VALUE, 0);
        f.b.push_i8(1);
        f.b.push_i8(2);
        f.b.array_from(2);
        f.b.call(2);
        f.b.return_();
        f.build(vm)
    });
    assert_eq!(out.log, ["hi 42 null", "{\"value\":1} [1,2]"]);
    assert_eq!(out.result, "undefined");
}

#[test]
fn host_call_results_and_errors() {
    let mut vm = vm();
    let host = vm.new_host_function(7, "h").unwrap();
    vm.define_global("h", host).unwrap();
    let h = vm.heap_mut().atom_str("h").unwrap();
    // try { return h(1) + 1 } catch (e) { return e }
    let mut f = F::new();
    let (s, e, t) = (f.b.new_label(), f.b.new_label(), f.b.new_label());
    f.b.place_label(s);
    f.b.get_global(h, 0);
    f.b.undefined();
    f.b.push_i8(1);
    f.b.call(1);
    f.b.push_i8(1);
    f.b.add();
    f.b.return_();
    f.b.place_label(e);
    f.b.place_label(t);
    f.b.return_();
    f.handlers.push((s, e, t, 0));
    let tpl = f.build(&mut vm);
    let main = vm.instantiate(tpl).unwrap();
    for (result, want) in [
        (Ok(Value::int(41)), Value::int(42)),
        (Err(Value::int(-1)), Value::int(-1)),
    ] {
        vm.call(main, Value::UNDEFINED, &[]).unwrap();
        let Status::HostCall { call_id, fn_id } = vm.run(1000) else {
            panic!()
        };
        assert_eq!(fn_id, 7);
        assert_eq!(vm.host_call(call_id).unwrap().args, vec![Value::int(1)]);
        // Running again while parked reports the same call.
        assert_eq!(vm.run(1000), Status::HostCall { call_id, fn_id });
        assert_eq!(vm.call(main, Value::UNDEFINED, &[]), Err(VmError::Busy));
        assert_eq!(
            vm.resolve_host_call(call_id + 1, Ok(Value::NULL)),
            Err(VmError::UnknownHostCall)
        );
        vm.resolve_host_call(call_id, result).unwrap();
        assert_eq!(vm.run(1000), Status::Done(want));
    }
    // Calling a host function directly from the embedder.
    vm.call(host, Value::NULL, &[Value::TRUE]).unwrap();
    let Status::HostCall { call_id, .. } = vm.run(10) else {
        panic!()
    };
    assert_eq!(vm.host_call(call_id).unwrap().this, Value::NULL);
    vm.resolve_host_call(call_id, Ok(Value::int(5))).unwrap();
    assert_eq!(vm.run(10), Status::Done(Value::int(5)));
}

fn error_of(build: impl Fn(&mut Vm, &mut F)) -> String {
    result_of(|vm| {
        let mut f = F::new();
        build(vm, &mut f);
        f.b.return_undef();
        f.build(vm)
    })
}

#[test]
fn runtime_errors() {
    let r = error_of(|vm, f| {
        let a = vm.heap_mut().atom_str("nope").unwrap();
        f.b.get_global(a, 0);
    });
    assert_eq!(r, "throw ReferenceError: 'nope' is not defined");
    let r = error_of(|vm, f| {
        let a = vm.heap_mut().atom_str("nope").unwrap();
        f.b.push_i8(1);
        f.b.put_global(a, 0);
    });
    assert_eq!(r, "throw ReferenceError: 'nope' is not defined");
    let r = error_of(|_, f| {
        f.b.undefined();
        f.b.get_field(crate::heap::atoms::NAME, 0);
    });
    assert_eq!(
        r,
        "throw TypeError: cannot read property 'name' of undefined"
    );
    let r = error_of(|_, f| {
        f.b.null();
        f.b.push_i8(0);
        f.b.get_elem();
    });
    assert_eq!(r, "throw TypeError: cannot read property '0' of null");
    let r = error_of(|_, f| {
        f.b.undefined();
        f.b.push_i8(1);
        f.b.put_field(crate::heap::atoms::NAME, 0);
    });
    assert_eq!(
        r,
        "throw TypeError: cannot set property 'name' of undefined"
    );
    let r = error_of(|_, f| {
        f.b.push_i8(1);
        f.b.undefined();
        f.b.call(0);
    });
    assert_eq!(r, "throw TypeError: not a function");
    let r = error_of(|vm, f| {
        let c = vm.heap_mut().atom_str("c").unwrap();
        f.b.throw_const_assign(c);
    });
    assert_eq!(r, "throw TypeError: 'c' is read-only");
    let r = error_of(|_, f| {
        f.b.push_i8(1);
        f.b.push_i8(2);
        f.b.in_();
    });
    assert_eq!(r, "throw TypeError: invalid 'in' operand");
    let r = error_of(|_, f| {
        f.b.push_i8(1);
        f.b.push_i8(2);
        f.b.instanceof();
    });
    assert_eq!(r, "throw TypeError: invalid 'instanceof' right operand");
    let r = error_of(|vm, f| {
        f.locals = 1;
        f.names = vec![vm.heap_mut().atom_str("x").unwrap()];
        f.b.set_loc_uninit(0);
        f.b.get_loc_chk(0);
    });
    assert_eq!(r, "throw ReferenceError: x is not initialized");
    let r = error_of(|_, f| {
        f.b.object();
        f.b.push_i8(1);
        f.b.add();
    });
    // %ToPrimitive now finds Object.prototype.toString (0031): no error, and error_of
    // reports the (discarded) completion.
    assert_eq!(r, "undefined");
    let r = error_of(|_, f| {
        f.b.push_i8(1);
        f.b.throw();
    });
    assert_eq!(r, "throw 1");
}

#[test]
fn objects_and_globals() {
    let r = result_of(|vm| {
        let mut f = F::new();
        let x = vm.heap_mut().atom_str("x").unwrap();
        let g = vm.heap_mut().atom_str("counter").unwrap();
        // globalThis.counter = 1; counter = counter + 1; o = {}; o.x = counter; o[5] = 3;
        // return [o.x, o[5], typeof o, "x" in o, counter]
        f.locals = 1;
        f.b.get_global(crate::heap::atoms::GLOBAL_THIS, 0);
        f.b.push_i8(1);
        f.b.put_field(g, 0);
        f.b.get_global(g, 0);
        f.b.push_i8(1);
        f.b.add();
        f.b.put_global(g, 0);
        f.b.object();
        f.b.put_loc(0);
        f.b.get_loc(0);
        f.b.get_global(g, 0);
        f.b.put_field(x, 0);
        f.b.get_loc(0);
        f.b.push_i8(5);
        f.b.push_i8(3);
        f.b.put_elem();
        f.b.get_loc(0);
        f.b.get_field(x, 0);
        f.b.get_loc(0);
        f.b.push_i8(5);
        f.b.get_elem();
        f.b.get_loc(0);
        f.b.typeof_();
        f.b.push_atom(x);
        f.b.get_loc(0);
        f.b.in_();
        f.b.get_global_typeof(g, 0);
        f.b.array_from(5);
        f.b.return_();
        f.build(vm)
    });
    assert_eq!(r, "[2,3,\"object\",true,2]");
}

/// `function mul(a, b) { return a * b }`
fn mul_template(vm: &mut Vm) -> Ref {
    let mut f = F::new();
    f.params = 2;
    f.b.get_arg(0);
    f.b.get_arg(1);
    f.b.mul();
    f.b.return_();
    f.build(vm)
}

#[test]
fn js_calls_and_argument_padding() {
    let r = result_of(|vm| {
        let child = mul_template(vm);
        let mut f = F::new();
        f.children = vec![child];
        f.locals = 1;
        f.b.fclosure(0);
        f.b.put_loc(0);
        f.b.get_loc(0);
        f.b.undefined();
        f.b.push_i8(6);
        f.b.push_i8(7);
        f.b.call(2); // 42
        f.b.get_loc(0);
        f.b.undefined();
        f.b.push_i8(6);
        f.b.call(1); // NaN
        f.b.get_loc(0);
        f.b.undefined();
        f.b.push_i8(2);
        f.b.push_i8(3);
        f.b.push_i8(4);
        f.b.call(3); // 6, extra argument ignored
        f.b.array_from(3);
        f.b.return_();
        f.build(vm)
    });
    assert_eq!(r, "[42,null,6]");
}

#[test]
fn closures_share_boxes() {
    // function counter() { let n = 10; return () => ++n }
    let mut vm = vm();
    let mut inner = F::new();
    inner.captures = vec![0];
    inner.b.get_cap(0);
    inner.b.inc();
    inner.b.set_cap(0);
    inner.b.return_();
    let inner = inner.build(&mut vm);
    let mut outer = F::new();
    outer.locals = 1;
    outer.children = vec![inner];
    outer.b.new_box(0);
    outer.b.push_i8(10);
    outer.b.put_box(0);
    outer.b.fclosure(0);
    outer.b.return_();
    let outer = outer.build(&mut vm);
    let counter = vm.instantiate(outer).unwrap();
    vm.call(counter, Value::UNDEFINED, &[]).unwrap();
    let Status::Done(f) = vm.run(100) else {
        panic!()
    };
    let hf = vm.heap_mut().new_handle(f).unwrap();
    for want in [11, 12, 13] {
        vm.collect().unwrap();
        let f = vm.heap().get_handle(hf);
        vm.call(f, Value::UNDEFINED, &[]).unwrap();
        assert_eq!(vm.run(100), Status::Done(Value::int(want)));
    }
    vm.heap().verify().unwrap();
}

#[test]
fn accessors_run_through_continuations() {
    // o has a getter (a JS closure returning this.v * 2) and a setter (stores v + 1).
    let r = run_main(|vm| {
        let v = vm.heap_mut().atom_str("v").unwrap();
        let p = vm.heap_mut().atom_str("p").unwrap();
        let mut getter = F::new();
        getter.b.get_global(crate::heap::atoms::GLOBAL_THIS, 0);
        getter.b.get_field(v, 0);
        getter.b.push_i8(2);
        getter.b.mul();
        getter.b.return_();
        let getter = getter.build(vm);
        let mut setter = F::new();
        setter.params = 1;
        setter.b.get_global(crate::heap::atoms::GLOBAL_THIS, 0);
        setter.b.get_arg(0);
        setter.b.push_i8(1);
        setter.b.add();
        setter.b.put_field(v, 0);
        setter.b.push_i8(99);
        setter.b.return_(); // the setter's result is discarded
        let setter = setter.build(vm);
        let gf = vm.instantiate(getter).unwrap();
        let sf = vm.instantiate(setter).unwrap();
        let g = vm.global();
        vm.heap_mut()
            .create_data_property(g, v, Value::int(5))
            .unwrap();
        vm.heap_mut()
            .define_own_property(g, p, &PropertyDescriptor::accessor(gf, sf, PropFlags::ALL))
            .unwrap();
        // return [p, (p = 20, p), v]
        let mut f = F::new();
        f.b.get_global(p, 0);
        f.b.get_global(crate::heap::atoms::GLOBAL_THIS, 0);
        f.b.push_i8(20);
        f.b.put_field(p, 0);
        f.b.get_global(p, 0);
        f.b.get_global(v, 0);
        f.b.array_from(3);
        f.b.return_();
        f.build(vm)
    });
    assert_eq!(r.result, "[10,42,21]");
}

#[test]
fn exceptions_unwind_frames() {
    // function thrower() { null.x }   main: try { thrower() } catch (e) { return "caught " + e.message }
    let r = result_of(|vm| {
        let mut t = F::new();
        t.b.null();
        t.b.get_field(crate::heap::atoms::NAME, 0);
        t.b.return_();
        let t = t.build(vm);
        let mut f = F::new();
        f.children = vec![t];
        let caught = f.string(vm, "caught ");
        let (s, e, h) = (f.b.new_label(), f.b.new_label(), f.b.new_label());
        f.b.place_label(s);
        f.b.push_i8(1); // an operand below the call, dropped by the unwind
        f.b.fclosure(0);
        f.b.undefined();
        f.b.call(0);
        f.b.return_();
        f.b.place_label(e);
        f.b.place_label(h);
        f.b.get_field(crate::heap::atoms::MESSAGE, 0);
        f.b.push_const(caught);
        f.b.swap();
        f.b.add();
        f.b.return_();
        f.handlers.push((s, e, h, 0));
        f.build(vm)
    });
    assert_eq!(r, "caught cannot read property 'name' of null");
}

#[test]
fn stack_overflow() {
    let mut vm = vm_with(VmOptions {
        exec: ExecOptions {
            max_stack: 10_000,
            ..Default::default()
        },
        ..Default::default()
    });
    // function f() { return f() }
    let name = vm.heap_mut().atom_str("f").unwrap();
    let mut f = F::new();
    f.b.get_global(name, 0);
    f.b.undefined();
    f.b.call(0);
    f.b.return_();
    let t = f.build(&mut vm);
    let func = vm.instantiate(t).unwrap();
    vm.define_global("f", func).unwrap();
    vm.call(func, Value::UNDEFINED, &[]).unwrap();
    let (vm, out) = finish(vm, 1 << 30, false);
    assert_eq!(
        out.result,
        "throw RangeError: Maximum call stack size exceeded"
    );
    // The VM is usable again.
    let mut vm = vm;
    let t = mul_template(&mut vm);
    let m = vm.instantiate(t).unwrap();
    vm.call(m, Value::UNDEFINED, &[Value::int(3), Value::int(4)])
        .unwrap();
    assert_eq!(vm.run(100), Status::Done(Value::int(12)));
}

#[test]
fn garbage_is_collected_while_running() {
    // A loop that allocates a lot of garbage strings; the heap must stay small.
    let mut vm = vm();
    let mut f = F::new();
    f.locals = 1;
    let s = f.string(&mut vm, &"y".repeat(50));
    let (top, end) = (f.b.new_label(), f.b.new_label());
    f.b.push_i32(0);
    f.b.put_loc(0);
    f.b.place_label(top);
    f.b.get_loc(0);
    f.b.push_i32(20_000);
    f.b.lt();
    f.b.jump_if_false(end);
    f.b.push_const(s);
    f.b.get_loc(0);
    f.b.add();
    f.b.drop();
    f.b.get_loc(0);
    f.b.inc();
    f.b.put_loc(0);
    f.b.jump(top);
    f.b.place_label(end);
    f.b.get_loc(0);
    f.b.return_();
    let t = f.build(&mut vm);
    let main = vm.instantiate(t).unwrap();
    vm.call(main, Value::UNDEFINED, &[]).unwrap();
    assert_eq!(vm.run(1 << 30), Status::Done(Value::int(20_000)));
    // 20k ropes of ~100 bytes each would be 2 MB without collections.
    assert!(
        vm.heap().arena().len() < 1 << 20,
        "{}",
        vm.heap().arena().len()
    );
}

#[test]
fn image_while_parked() {
    // Parked on a host call: snapshot, restore elsewhere, resolve there.
    let mut vm = vm();
    let mut f = F::new();
    let console = vm.heap_mut().atom_str("console").unwrap();
    let log = vm.heap_mut().atom_str("log").unwrap();
    f.b.get_global(console, 0);
    f.b.get_method(log, 0);
    f.b.push_i8(1);
    f.b.call(1);
    f.b.push_i8(1);
    f.b.add();
    f.b.return_();
    let t = f.build(&mut vm);
    let main = vm.instantiate(t).unwrap();
    vm.call(main, Value::UNDEFINED, &[]).unwrap();
    let Status::HostCall { call_id, .. } = vm.run(100) else {
        panic!()
    };
    let mut moved = Vm::from_bytes(vm.bytes().to_vec(), VmOptions::default()).unwrap();
    drop(vm);
    assert_eq!(moved.state(), RunState::HostCall);
    moved
        .resolve_host_call(call_id, Ok(Value::int(41)))
        .unwrap();
    assert_eq!(moved.run(100), Status::Done(Value::int(42)));
}

#[test]
fn abort_returns_to_idle() {
    // An endless loop, aborted after running out of fuel.
    let mut vm = vm();
    let mut f = F::new();
    let top = f.b.new_label();
    f.b.place_label(top);
    f.b.push_i8(1);
    f.b.drop();
    f.b.jump(top);
    let t = f.build(&mut vm);
    let spin = vm.instantiate(t).unwrap();
    let hs = vm.heap_mut().new_handle(spin).unwrap();
    vm.call(spin, Value::UNDEFINED, &[]).unwrap();
    assert_eq!(vm.run(100), Status::OutOfFuel);
    assert_eq!(vm.call(spin, Value::UNDEFINED, &[]), Err(VmError::Busy));
    vm.abort();
    assert_eq!(vm.state(), RunState::Idle);
    assert_eq!(vm.run(100), Status::Idle);
    // Usable again.
    let t = mul_template(&mut vm);
    let m = vm.instantiate(t).unwrap();
    vm.call(m, Value::UNDEFINED, &[Value::int(6), Value::int(7)])
        .unwrap();
    assert_eq!(vm.run(100), Status::Done(Value::int(42)));

    // Aborting while parked on a host call drops the call.
    let host = vm.new_host_function(9, "h").unwrap();
    vm.call(host, Value::UNDEFINED, &[]).unwrap();
    let Status::HostCall { call_id, .. } = vm.run(10) else {
        panic!()
    };
    vm.abort();
    assert!(vm.host_call(call_id).is_none());
    assert_eq!(
        vm.resolve_host_call(call_id, Ok(Value::NULL)),
        Err(VmError::UnknownHostCall)
    );
    vm.collect().unwrap();
    vm.heap().verify().unwrap();
    // The aborted closure is still usable through its handle.
    let spin = vm.heap().get_handle(hs);
    vm.call(spin, Value::UNDEFINED, &[]).unwrap();
    assert_eq!(vm.run(10), Status::OutOfFuel);
}

/// Runs a Script through the compiler, collecting console.log output.
fn run_js(src: &str) -> Outcome {
    let mut vm = vm();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    finish(vm, 1 << 40, false).1
}

/// 0054: a frozen realm whose prototype `name`s are accessors (pi's lockdown) still shows
/// the error's name in host output, from the nearest native error prototype; own names
/// (set through the accessor's setter) win.
#[test]
fn error_display_with_accessor_names() {
    let out = run_js(
        "for (const C of [Error, TypeError]) {\n\
           const v = C.prototype.name;\n\
           Object.defineProperty(C.prototype, 'name', { get() { return v; }, set(x) {\n\
             Object.defineProperty(this, 'name', { value: x, writable: true, configurable: true }); } });\n\
         }\n\
         class MyError extends Error { constructor(m) { super(m); this.name = 'MyError'; } }\n\
         console.log(new TypeError('a'), new MyError('b'), new Error('c'));\n\
         null.x;\n",
    );
    assert_eq!(out.log[0], "TypeError: a MyError: b Error: c");
    assert_eq!(
        out.result,
        "throw TypeError: cannot read property 'x' of null"
    );
}

/// 0059: CreateDynamicFunction parses the parameters and the body as separate goals, so
/// neither can close the other's brackets (qjs only evaluates the joined source); eval
/// and the constructors compile at the global scope; eval returns the completion value
/// (0058).
#[test]
fn dynamic_code() {
    let out = run_js(
        "const r = [];\n\
         const t = (f) => { try { r.push(String(f())); } catch (e) { r.push(e.name + ': ' + e.message); } };\n\
         globalThis.ran = false;\n\
         t(() => Function('){}; globalThis.ran = true; (function(', '')); \n\
         t(() => Function('/*', '*/){'));\n\
         t(() => Function('', '}{'));\n\
         t(() => Function('', '}); globalThis.ran = true; (function(){'));\n\
         t(() => Function('a = 1, ...b', 'return a + b.length')(undefined, 2, 3));\n\
         t(() => Function('-->', ''));\n\
         t(() => (0, eval)('globalThis.e1 = typeof r'));\n\
         t(() => e1);\n\
         t(() => ran);\n\
         console.log(r.join('|'));\n",
    );
    assert_eq!(
        out.log[0],
        "SyntaxError: expecting ')'|\
         SyntaxError: parameters end before the parameter list|\
         SyntaxError: expecting ')'|\
         SyntaxError: unexpected token after the function body|\
         3|\
         SyntaxError: missing formal parameter|\
         object|object|false"
    );
}

/// D015: functions from the Function constructors without "use strict" bind `this`
/// sloppily (undefined/null: the global object; primitives: wrappers), nothing else.
#[test]
fn sloppy_this_for_dynamic_functions() {
    let out = run_js(
        "const r = [];\n\
         r.push(Function('return this')() === globalThis);\n\
         r.push(Function('return this').call(null) === globalThis);\n\
         r.push(Function('return this').call(5) instanceof Number);\n\
         r.push(Function('\"use strict\"; return this')() === undefined);\n\
         const o = {}; r.push(Function('return this').call(o) === o);\n\
         r.push((function () { return this; })() === undefined);\n\
         try { Function('undeclared = 1')(); r.push('implicit global'); } catch (e) { r.push(e.name); }\n\
         console.log(r.join());\n",
    );
    assert_eq!(out.log[0], "true,true,true,true,true,true,ReferenceError");
}

#[test]
fn error_stack_traces() {
    // Positions checked against qjs.
    let out = run_js(
        "function inner(x) { return x.y.z; }\n\
         const arrow = () => { const r = inner({}); return r; };\n\
         function outer() { const r = arrow(); return r; }\n\
         try { outer(); } catch (e) { console.log(e.stack); }\n\
         try { null.x } catch (e) { console.log(e.stack); }\n\
         const anon = function () { return nope; };\n\
         try { anon(); } catch (e) { console.log(e.stack); }\n\
         function deep(n) { if (n === 0) return null.x; const r = deep(n - 1); return r; }\n\
         try { deep(20); } catch (e) { console.log(e.stack); }\n",
    );
    assert_eq!(
        out.log[0],
        "    at inner (t.js:1:31)\n    at arrow (t.js:2:38)\n    at outer (t.js:3:35)\n    at <eval> (t.js:4:12)\n"
    );
    assert_eq!(out.log[1], "    at <eval> (t.js:5:11)\n");
    assert_eq!(
        out.log[2],
        "    at anon (t.js:6:35)\n    at <eval> (t.js:7:11)\n"
    );
    let deep = &out.log[3];
    assert_eq!(deep.lines().count(), STACK_TRACE_LIMIT as usize, "{deep}");
}

#[test]
fn iterator_helpers_follow_the_spec_where_qjs_differs() {
    // qjs closes the iterator before reduce's "empty iterator" TypeError and closes an
    // exhausted inner iterator of flatMap; the spec (and node) do neither (0051).
    let out = run_js(
        "const log = [];\n\
         function it(n) { let i = 0; return Iterator.from({ next() { log.push('next'); return i < n ? { value: i++, done: false } : { done: true }; }, return() { log.push('return'); return {}; } }); }\n\
         try { it(0).reduce((a, b) => a); } catch (e) { log.push(e.message); }\n\
         console.log(log.join());\n\
         log.length = 0;\n\
         console.log([1].values().flatMap(() => it(1)).toArray().join(), log.join());\n",
    );
    assert_eq!(out.log, ["next,empty iterator", "0 next,next"]);
}

/// Compiles and runs one more Script in `vm` (globals persist between scripts).
fn run_script(vm: Vm, src: &str) -> (Vm, Outcome) {
    finish_script(vm, src, 1 << 40, false)
}

/// `run_script` with a fuel slice, restoring the image at every stop with `suspend`.
fn finish_script(mut vm: Vm, src: &str, fuel: u64, suspend: bool) -> (Vm, Outcome) {
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    finish(vm, fuel, suspend)
}

#[test]
fn weak_refs_and_finalization_after_collect() {
    let vm = vm();
    let (mut vm, out) = run_script(
        vm,
        "globalThis.kept = {};\n\
         globalThis.w1 = new WeakRef(kept);\n\
         globalThis.w2 = new WeakRef({ dies: true });\n\
         globalThis.r = new FinalizationRegistry((h) => console.log('cleanup', h));\n\
         globalThis.token = {};\n\
         r.register({}, 'a');\n\
         r.register({}, 'b', token);\n\
         r.register({}, 'c', {});\n\
         r.register(kept, 'kept');\n\
         r.unregister(token);\n\
         // KeepDuringJob: a collection inside the job leaves w2's target alone.\n\
         console.log(typeof w2.deref());\n",
    );
    assert_eq!(out.log, ["object"]);
    vm.collect().unwrap();
    vm.heap().verify().unwrap();
    // The queued cleanup jobs run after the next call.
    let (mut vm, out) = run_script(vm, "console.log(w1.deref() === kept, w2.deref());\n");
    assert_eq!(out.log, ["true undefined", "cleanup a", "cleanup c"]);
    // Collecting again changes nothing, and the layout is a function of the graph.
    vm.collect().unwrap();
    vm.collect().unwrap();
    let image = vm.bytes().to_vec();
    vm.collect().unwrap();
    assert!(image == vm.bytes(), "a collection changed the image");
    let (_, out) = run_script(vm, "console.log('done');\n");
    assert_eq!(out.log, ["done"]);
}

/// WeakRefs and a registry with `n` registered objects.
fn weak_script(n: u32) -> String {
    format!(
        "globalThis.kept = {{}};\n\
         globalThis.w1 = new WeakRef(kept);\n\
         globalThis.w2 = new WeakRef({{ dies: true }});\n\
         globalThis.r = new FinalizationRegistry((h) => console.log('cleanup', h));\n\
         r.register({{}}, 'a');\n\
         r.register(kept, 'kept');\n\
         for (let i = 0; i < {n}; i++) r.register({{ i }}, i);\n"
    )
}

#[test]
fn strong_weak_refs_never_lose_their_targets() {
    // GC stress collects at every allocation: fewer registrations there.
    strong_weak_refs(300);
}

/// [`strong_weak_refs_never_lose_their_targets`] with the full registry under GC stress
/// (about 6 s): `cargo test -p djs-core --lib -- --ignored strong_weak_refs_full`.
#[test]
#[ignore]
fn strong_weak_refs_full() {
    strong_weak_refs(3000);
}

fn strong_weak_refs(stress_registrations: u32) {
    // D1680: in strong mode no collection is observable: deref keeps finding targets
    // nobody else references, and no cleanup callback runs, after explicit collections
    // and under GC stress.
    for gc_stress in [false, true] {
        let n = if gc_stress {
            stress_registrations
        } else {
            3000
        };
        let vm = vm_with(VmOptions {
            weak_refs: WeakRefs::Strong,
            exec: ExecOptions {
                gc_stress,
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(vm.heap().weak_refs_strong());
        let (mut vm, _) = run_script(vm, &weak_script(n));
        vm.collect().unwrap();
        vm.collect().unwrap();
        vm.heap().verify().unwrap();
        let (mut vm, out) = run_script(vm, "console.log(w1.deref() === kept, w2.deref().dies);\n");
        assert_eq!(out.log, ["true true"], "gc_stress {gc_stress}");
        // The mode is in the image.
        let opts = vm.options();
        vm = Vm::from_bytes(vm.image().to_vec(), opts).unwrap();
        assert!(vm.heap().weak_refs_strong());
        let (mut vm, out) = run_script(
            vm,
            "globalThis.w3 = new WeakRef({ late: 1 }); console.log('ok');\n",
        );
        assert_eq!(out.log, ["ok"]);
        vm.collect().unwrap();
        let (_, out) = run_script(vm, "console.log(w3.deref().late, w2.deref().dies);\n");
        assert_eq!(out.log, ["1 true"]);
    }
    // The same script with collected weak refs: the targets die and callbacks run.
    let (mut vm, first) = run_script(vm(), &weak_script(3000));
    vm.collect().unwrap();
    let (_, out) = run_script(vm, "console.log(w1.deref() === kept, w2.deref());\n");
    assert_eq!(out.log[0], "true undefined");
    // Callbacks run after the collection that found the targets dead: during the first
    // script or after the explicit one, depending on when the heap grows enough.
    assert!(first.log.len() + out.log.len() > 1, "cleanup ran");
}

#[test]
fn absent_weak_refs() {
    let vm = vm_with(VmOptions {
        weak_refs: WeakRefs::Absent,
        ..Default::default()
    });
    let (vm, out) = run_script(
        vm,
        "console.log(typeof WeakRef, typeof FinalizationRegistry, typeof WeakMap);\n",
    );
    assert_eq!(out.log, ["undefined undefined function"]);
    // From a base image too (after_copy), as embedders create VMs.
    let base = Vm::new(VmOptions::default()).unwrap().bytes().to_vec();
    let mut b = Vm::from_base(
        &base,
        VmOptions {
            weak_refs: WeakRefs::Absent,
            ..Default::default()
        },
    )
    .unwrap();
    let w = b.heap_mut().atom_str("WeakRef").unwrap();
    assert!(b.heap().find_property(b.global(), w).is_none());
    drop(vm);
}

#[test]
fn finalization_cleanup_can_wait_for_the_host() {
    let vm = vm_with(VmOptions {
        exec: ExecOptions {
            finalization_after_gc: false,
            ..Default::default()
        },
        ..Default::default()
    });
    let (mut vm, _) = run_script(
        vm,
        "globalThis.r = new FinalizationRegistry((h) => { console.log('cleanup', h); throw new Error('ignored'); });\n\
         r.register({}, 1);\n\
         r.register({}, 2);\n",
    );
    vm.collect().unwrap();
    let (mut vm, out) = run_script(vm, "console.log('no cleanup yet');\n");
    assert_eq!(out.log, ["no cleanup yet"]);
    assert_eq!(vm.schedule_finalization_cleanup().unwrap(), 1);
    assert_eq!(vm.schedule_finalization_cleanup().unwrap(), 0);
    // The first callback throws: the job ends, the error is dropped (D1513), and the
    // other cell waits for the next scheduling.
    let (mut vm, out) = run_script(vm, "console.log('after');\n");
    assert_eq!(out.log, ["after", "cleanup 1"]);
    assert_eq!(vm.schedule_finalization_cleanup().unwrap(), 1);
    let (_, out) = run_script(vm, "console.log('end');\n");
    assert_eq!(out.log, ["end", "cleanup 2"]);
}

/// `Vm::new` with and without the prelude. Run with
/// `cargo test -p djs-core --release vm_new_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn vm_new_cost() {
    extern crate std;
    use std::time::Instant;
    for prelude in [false, true] {
        let opts = VmOptions {
            prelude,
            ..Default::default()
        };
        let n = 200;
        let t = Instant::now();
        let mut size = 0;
        for _ in 0..n {
            let vm = Vm::new(opts).unwrap();
            size = vm.bytes().len();
        }
        let per = t.elapsed().as_secs_f64() / n as f64;
        std::println!(
            "Vm::new prelude={prelude}: {:.1} µs, heap {size} bytes",
            per * 1e6
        );
    }
    // What a base snapshot costs: the image after a collect, restored with full
    // verification, and a plain copy.
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    vm.collect().unwrap();
    let image = vm.bytes().to_vec();
    let n = 2000;
    let t = Instant::now();
    for _ in 0..n {
        let vm = Vm::from_bytes(image.clone(), VmOptions::default()).unwrap();
        std::hint::black_box(vm.bytes().len());
    }
    let per = t.elapsed().as_secs_f64() / n as f64;
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(image.clone());
    }
    let copy = t.elapsed().as_secs_f64() / n as f64;
    std::println!(
        "image {} bytes: from_bytes {:.1} µs, copy {:.1} µs",
        image.len(),
        per * 1e6,
        copy * 1e6
    );
}

#[test]
fn method_call_through_a_getter() {
    // o.m is an accessor whose getter returns a function; `o.m()` calls that function
    // with this = o (get_method through the Replace continuation).
    let mut vm = vm();
    let src = "function getter() { return function () { return this.n + 1; }; }\n\
               function make() { return { n: 41 }; }";
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("g.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert!(matches!(vm.run(1 << 30), Status::Done(_)));
    let g = vm.global();
    let k = vm.heap_mut().atom_str("getter").unwrap();
    let getter = vm.heap().get_own_property(g, k).unwrap().value().unwrap();
    let o = vm.new_object().unwrap();
    let n = vm.heap_mut().atom_str("n").unwrap();
    vm.heap_mut()
        .create_data_property(o, n, Value::int(41))
        .unwrap();
    let m = vm.heap_mut().atom_str("m").unwrap();
    vm.heap_mut()
        .define_own_property(
            o,
            m,
            &PropertyDescriptor::accessor(getter, Value::UNDEFINED, PropFlags::ALL),
        )
        .unwrap();
    vm.define_global("obj", Value::object(o)).unwrap();
    let out = {
        let f = vm
            .compile(
                b"console.log(obj.m(), obj['m'](), typeof obj.m)",
                &crate::compiler::CompileOptions::script("g2.js"),
            )
            .unwrap();
        let this = Value::object(vm.global());
        vm.call(f, this, &[]).unwrap();
        finish(vm, 1 << 30, true).1
    };
    assert_eq!(out.log, ["42 42 function"]);
}

#[test]
fn to_primitive_through_a_host_valueof() {
    // valueOf is a host function: the slow path parks on it and the resolved value
    // replaces the operand.
    let mut vm = vm();
    let host = vm.new_host_function(5, "valueOf").unwrap();
    let o = vm.new_object().unwrap();
    vm.define(o, "valueOf", host).unwrap();
    vm.define_global("o", Value::object(o)).unwrap();
    let f = vm
        .compile(
            b"console.log(o + 1, o * 3)",
            &crate::compiler::CompileOptions::script("h.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let mut log = Vec::new();
    loop {
        match vm.run(1 << 30) {
            Status::HostCall { call_id, fn_id: 5 } => {
                // `o` re-read from the global: the round trips below may move it, so a
                // raw value from before goes stale (embedders hold handles instead).
                let key = vm.heap_mut().atom_str("o").unwrap();
                let g = vm.global();
                let o_now = match vm.heap().get_own_property(g, key) {
                    Some(crate::heap::Property::Data { value, .. }) => value,
                    _ => panic!("global o"),
                };
                assert_eq!(vm.host_call(call_id).unwrap().this, o_now);
                // Through an image round trip, for good measure.
                let opts = vm.options();
                vm = Vm::from_bytes(vm.image().to_vec(), opts).unwrap();
                vm.resolve_host_call(call_id, Ok(Value::int(10))).unwrap();
            }
            Status::HostCall { call_id, .. } => {
                let info = vm.host_call(call_id).unwrap();
                let parts: Vec<String> = info.args.iter().map(|&a| vm.display(a)).collect();
                log.push(parts.join(" "));
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
            Status::Done(_) => break,
            s => panic!("{s:?}"),
        }
    }
    assert_eq!(log, ["11 30"]);
}

/// Runs `src` as a script and returns what it evaluates to (`return` value of a body).
fn eval(vm: &mut Vm, src: &str) -> String {
    let opts = crate::compiler::CompileOptions::async_body("t.js", &[]);
    let f = vm.compile(src.as_bytes(), &opts).unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    match vm.run(1 << 30) {
        // The body's promise (an async function, D008).
        Status::Done(v) => match vm.promise_state(v) {
            Some(Some(Ok(r))) => vm.display(r),
            Some(Some(Err(e))) => panic!("rejected: {}", vm.display(e)),
            Some(None) => panic!("pending"),
            None => vm.display(v),
        },
        Status::Threw(e) => panic!("threw: {}", vm.display(e)),
        s => panic!("{s:?}"),
    }
}

/// A base image as djs-prelude makes it: a fresh VM with the prelude, collected.
fn base_image() -> Vec<u8> {
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    vm.collect().unwrap();
    vm.bytes().to_vec()
}

#[test]
fn from_base_gives_independent_vms() {
    let base = base_image();
    let mut a = Vm::from_base(&base, VmOptions::default()).unwrap();
    let mut b = Vm::from_base(&base, VmOptions::default()).unwrap();
    assert_eq!(
        eval(&mut a, "globalThis.x = 1; return [1, 2, 3].length + 1"),
        "4"
    );
    // The second VM does not see the first one's global.
    assert_eq!(eval(&mut b, "return typeof globalThis.x"), "undefined");
    // A VM from the base behaves like one that compiled its prelude.
    let mut c = Vm::new(VmOptions::default()).unwrap();
    let probe = "return typeof JSON + typeof Math + typeof Array + typeof String + typeof Object \
                 + typeof [].push + typeof ''.slice + [1, 2, 3].length";
    assert_eq!(eval(&mut b, probe), eval(&mut c, probe));
    // Host functions are bound after the copy, per VM.
    let h = a.new_host_function(7, "f").unwrap();
    a.define_global("f", h).unwrap();
    assert_eq!(eval(&mut b, "return typeof f"), "undefined");
}

#[test]
fn images_carry_their_dependency_descriptor() {
    // D2265: compat_tests checks the components; here the load paths refuse.
    let vm = Vm::new(VmOptions::default()).unwrap();
    let at = crate::heap::cells::Roots::OPS.offset as usize;
    assert_eq!(
        vm.bytes()[at..at + 4],
        crate::bytecode::OPS_HASH.to_le_bytes()
    );
    let mut other = vm.bytes().to_vec();
    other[at] ^= 1;
    let expected = crate::bytecode::OPS_HASH;
    let found = expected ^ 1;
    let dep = ImageDep::Ops;
    for r in [
        Vm::from_bytes(other.clone(), VmOptions::default()).err(),
        Vm::from_base(&other, VmOptions::default()).err(),
    ] {
        assert_eq!(
            r,
            Some(LoadError::Incompatible {
                dep,
                found,
                expected
            })
        );
    }
    // Not an image at all.
    assert_eq!(
        Vm::from_base(b"not an image", VmOptions::default()).err(),
        Some(LoadError::BadHeader)
    );
    // A bare heap (no VM) has the layout hash but no descriptor otherwise.
    let bare = crate::heap::Heap::new().unwrap();
    assert!(matches!(
        Vm::from_bytes(bare.bytes().to_vec(), VmOptions::default()),
        Err(LoadError::Incompatible { found: 0, .. })
    ));
}

#[test]
fn from_base_needs_an_idle_vm_and_room() {
    // A VM suspended mid-call is a valid image for from_bytes, but not a base.
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let opts = crate::compiler::CompileOptions::async_body("t.js", &[]);
    let f = vm.compile(b"for (;;) {}", &opts).unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert!(matches!(vm.run(1000), Status::OutOfFuel));
    let busy = vm.bytes().to_vec();
    assert!(Vm::from_bytes(busy.clone(), VmOptions::default()).is_ok());
    assert_eq!(
        Vm::from_base(&busy, VmOptions::default()).err(),
        Some(LoadError::BadHeader)
    );
    let small = VmOptions {
        memory_limit: 4096,
        ..VmOptions::default()
    };
    assert_eq!(
        Vm::from_base(&base_image(), small).err(),
        Some(LoadError::TooLarge)
    );
}

#[test]
fn math_random_is_seeded_per_vm_and_survives_snapshots() {
    let base = base_image();
    let seeded = |seed| VmOptions {
        random_seed: seed,
        ..Default::default()
    };
    let draw = "return [Math.random(), Math.random(), Math.random()].join()";
    let mut a = Vm::from_base(&base, seeded(42)).unwrap();
    let mut b = Vm::from_base(&base, seeded(42)).unwrap();
    let mut c = Vm::from_base(&base, seeded(43)).unwrap();
    let first = eval(&mut a, draw);
    assert_eq!(first, eval(&mut b, draw));
    assert_ne!(first, eval(&mut c, draw));
    // Vm::new applies the seed too.
    let mut d = Vm::new(seeded(42)).unwrap();
    assert_eq!(first, eval(&mut d, draw));
    // A snapshot continues the sequence instead of restarting it.
    let mut e = Vm::from_bytes(a.bytes().to_vec(), seeded(42)).unwrap();
    assert_eq!(eval(&mut a, draw), eval(&mut e, draw));
    let r = eval(&mut e, "const r = Math.random(); return r >= 0 && r < 1");
    assert_eq!(r, "true");
}

/// `acc = ""; for (k in <global name>) { acc += k + ","; [delete obj.<del> on the first
/// step] } return acc`, hand-assembled (the compiler side is 0029).
fn for_in_keys(vm: &mut Vm, target: &str, delete_on_first: Option<&str>) -> Value {
    let t = vm.heap_mut().atom_str(target).unwrap();
    let mut f = F::new();
    f.locals = 2;
    let comma = f.string(vm, ",");
    let empty = f.string(vm, "");
    let (top, end, skip) = (f.b.new_label(), f.b.new_label(), f.b.new_label());
    f.b.push_const(empty);
    f.b.put_loc(0);
    f.b.push_i8(0);
    f.b.put_loc(1);
    f.b.get_global(t, 0);
    f.b.for_in_start();
    f.b.place_label(top);
    f.b.for_in_next();
    f.b.jump_if_true(end);
    f.b.get_loc(0);
    f.b.swap();
    f.b.add();
    f.b.push_const(comma);
    f.b.add();
    f.b.put_loc(0);
    if let Some(d) = delete_on_first {
        let d = vm.heap_mut().atom_str(d).unwrap();
        f.b.get_loc(1);
        f.b.jump_if_true(skip);
        f.b.get_global(t, 0);
        f.b.push_atom(d);
        f.b.delete();
        f.b.drop();
        f.b.push_true();
        f.b.put_loc(1);
        f.b.place_label(skip);
    }
    f.b.jump(top);
    f.b.place_label(end);
    f.b.drop();
    f.b.drop();
    f.b.get_loc(0);
    f.b.return_();
    let tpl = f.build(vm);
    let main = vm.instantiate(tpl).unwrap();
    vm.call(main, Value::UNDEFINED, &[]).unwrap();
    let Status::Done(v) = vm.run(1 << 30) else {
        panic!()
    };
    v
}

#[test]
fn for_in_order_shadowing_and_deletion() {
    let mut vm = vm();
    let src = "var proto = { inherited: 1, shadowed: 2, gone: 3 };\n\
               var obj = { __proto__: proto, b: 1, 2: 0, a: 1, 1: 0 };";
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("fi.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert!(matches!(vm.run(1 << 30), Status::Done(_)));
    // A non-enumerable own `shadowed` hides the prototype's, and a symbol key is skipped.
    let g = vm.global();
    let obj_key = vm.heap_mut().atom_str("obj").unwrap();
    let obj = vm
        .heap()
        .get_own_property(g, obj_key)
        .unwrap()
        .value()
        .unwrap()
        .as_object()
        .unwrap();
    let sh = vm.heap_mut().atom_str("shadowed").unwrap();
    vm.heap_mut()
        .define_own_property(
            obj,
            sh,
            &PropertyDescriptor::data(Value::NULL, PropFlags::WRITABLE),
        )
        .unwrap();
    let sym = vm.heap_mut().new_symbol(Value::UNDEFINED).unwrap();
    let sk = vm.heap().symbol_atom(sym);
    vm.heap_mut()
        .create_data_property(obj, sk, Value::NULL)
        .unwrap();
    let v = for_in_keys(&mut vm, "obj", None);
    assert_eq!(vm.display(v), "1,2,b,a,inherited,gone,");
    // A key deleted during the loop is not visited.
    let v = for_in_keys(&mut vm, "obj", Some("a"));
    assert_eq!(vm.display(v), "1,2,b,inherited,gone,");
    // Strings give their indexes; null and numbers give nothing.
    let xyz = Value::string(vm.heap_mut().alloc_str("xyz").unwrap());
    vm.define_global("s", xyz).unwrap();
    vm.define_global("n", Value::NULL).unwrap();
    let v = for_in_keys(&mut vm, "s", None);
    assert_eq!(vm.display(v), "0,1,2,");
    let v = for_in_keys(&mut vm, "n", None);
    assert_eq!(vm.display(v), "");
}

#[test]
fn vm_call_never_collects() {
    // With GC stress, frame entry is a safepoint, but not for the entry call: the
    // embedder's raw values must stay valid until `run`.
    let mut vm = vm_with(VmOptions {
        exec: ExecOptions {
            gc_stress: true,
            ..Default::default()
        },
        ..Default::default()
    });
    let f = vm
        .compile(
            // (A call: the frame entry is the run's first safepoint.)
            b"return (() => 1 + 1)()",
            &crate::compiler::CompileOptions::async_body("c.js", &[]),
        )
        .unwrap();
    for _ in 0..100 {
        vm.heap_mut().alloc_str("garbage before the call").unwrap();
    }
    let len = vm.heap().arena().len();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert_eq!(vm.heap().arena().len(), len, "Vm::call must not collect");
    assert_eq!(
        vm.heap().cell_kind(f.as_object().unwrap().offset()),
        crate::heap::CellKind::Object
    );
    // The body is async (D008): its result is a fulfilled promise.
    let Status::Done(p) = vm.run(1 << 20) else {
        panic!("not done")
    };
    assert_eq!(vm.promise_state(p), Some(Some(Ok(Value::int(2)))));
    assert!(
        vm.heap().arena().len() < len,
        "run collects at its first safepoint"
    );
}

#[test]
fn date_uses_the_host_clock_and_offset() {
    let base = base_image();
    let mut vm = Vm::from_base(&base, VmOptions::default()).unwrap();
    // No clock set: the epoch (the VM never reads a system clock).
    assert_eq!(eval(&mut vm, "return Date.now()"), "0");
    vm.set_now(1_646_177_400_000.0);
    assert_eq!(
        eval(&mut vm, "return new Date().toISOString()"),
        "2022-03-01T23:30:00.000Z"
    );
    // UTC+2, checked against qjs with TZ=Etc/GMT-2.
    vm.set_time_zone_offset(120);
    let src = "const d = new Date(Date.UTC(2022, 2, 1, 23, 30));
        return [d.toString(), d.getDate(), d.getHours(), d.getTimezoneOffset(), d.toLocaleString(),
          new Date(2022, 0, 1).getTime(), Date.parse('2022-03-01T10:00'), Date.parse('2022-03-01'),
          new Date(2022, 2, 1).toISOString()].join('|')";
    assert_eq!(
        eval(&mut vm, src),
        "Wed Mar 02 2022 01:30:00 GMT+0200|2|1|-120|03/02/2022, 01:30:00 AM|1640988000000|\
         1646121600000|1646092800000|2022-02-28T22:00:00.000Z"
    );
}

const TOOL: u32 = 20;

/// Runs a Script with `console.log` and an async host function `tool(x)`, which the
/// "embedder" completes with `x.toUpperCase()` (or rejects for "fail"), resolving the
/// pending calls one at a time in reverse order whenever the VM is idle. With `suspend`,
/// every stop continues in a VM restored from the image; `gc_stress` collects at every
/// safepoint.
fn run_tools(src: &str, fuel: u64, suspend: bool, gc_stress: bool) -> Outcome {
    let mut vm = vm_with(VmOptions {
        exec: ExecOptions {
            gc_stress,
            ..Default::default()
        },
        ..Default::default()
    });
    let tool = vm.new_async_host_function(TOOL, "tool").unwrap();
    vm.define_global("tool", tool).unwrap();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let mut log = Vec::new();
    let mut pending: Vec<u32> = Vec::new();
    let reload = |vm: Vm| {
        let opts = vm.options();
        Vm::from_bytes(vm.bytes().to_vec(), opts).unwrap()
    };
    loop {
        let status = vm.run(fuel);
        for id in vm.new_host_calls() {
            let info = vm.host_call(id).unwrap();
            assert!(info.is_async);
            assert_eq!(info.fn_id, TOOL);
            pending.push(id);
        }
        match status {
            Status::Done(_) | Status::Threw(_) => {
                assert!(pending.is_empty());
                let result = match status {
                    Status::Done(v) => vm.display(v),
                    Status::Threw(e) => format!("throw {}", vm.display(e)),
                    _ => unreachable!(),
                };
                for r in vm.take_unhandled_rejections() {
                    log.push(format!("unhandled {}", vm.display(r)));
                }
                vm.heap().verify().unwrap();
                return Outcome { result, log };
            }
            Status::OutOfFuel => {}
            Status::HostOutOfMemory => panic!("out of host memory"),
            Status::HostCall { call_id, fn_id } => {
                assert_eq!(fn_id, LOG);
                let info = vm.host_call(call_id).unwrap();
                assert!(!info.is_async);
                let line: Vec<String> = info.args.iter().map(|&a| vm.display(a)).collect();
                log.push(line.join(" "));
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
            Status::Idle => {
                assert_eq!(vm.pending_host_calls(), pending);
                let id = pending.pop().expect("idle without pending calls");
                let arg = vm.host_call(id).unwrap().args[0];
                let s = vm.display(arg);
                let result = if s == "fail" {
                    Err(vm.new_error(ErrorKind::TypeError, "tool failed").unwrap())
                } else {
                    let v = vm.heap_mut().alloc_str(&s.to_uppercase()).unwrap();
                    Ok(Value::string(v))
                };
                log.push(format!("resolve {s}"));
                vm.resolve_host_call(id, result).unwrap();
            }
        }
        if suspend {
            vm = reload(vm);
        }
    }
}

fn tools_outcome(src: &str) -> Outcome {
    let plain = run_tools(src, 1 << 40, false, false);
    assert_eq!(
        plain,
        run_tools(src, 1, true, false),
        "suspension changed it"
    );
    assert_eq!(
        plain,
        run_tools(src, 13, true, true),
        "gc stress changed it"
    );
    plain
}

#[test]
fn async_host_calls_run_concurrently() {
    let out = tools_outcome(
        r#"
        const a = tool("a"), b = tool("b"), c = tool("c");
        console.log(Object.prototype.toString.call(a), a === b);
        Promise.all([a, b, c]).then(v => console.log("all", v.join()));
        b.then(v => console.log("b", v));
        Promise.resolve().then(() => console.log("microtask first"));
        console.log("sync end");
        "#,
    );
    assert_eq!(out.result, "undefined");
    assert_eq!(
        out.log,
        [
            "[object Promise] false",
            "sync end",
            "microtask first",
            "resolve c",
            "resolve b",
            "b B",
            "resolve a",
            "all A,B,C",
        ]
    );
}

#[test]
fn async_host_call_errors_and_thenables() {
    let out = tools_outcome(
        r#"
        tool("fail").catch(e => console.log("caught", e.name, e.message));
        tool("x").then(v => tool(v + "y")).then(v => console.log("chained", v));
        tool("fail");
        "#,
    );
    assert_eq!(
        out.log,
        [
            "resolve fail",
            "resolve x",
            "resolve Xy",
            "chained XY",
            "resolve fail",
            "caught TypeError tool failed",
            "unhandled TypeError: tool failed",
        ]
    );
}

#[test]
fn async_host_function_as_the_entry_callee() {
    let mut vm = vm();
    let tool = vm.new_async_host_function(TOOL, "tool").unwrap();
    vm.call(tool, Value::UNDEFINED, &[Value::int(1)]).unwrap();
    assert_eq!(vm.run(100), Status::Idle);
    let [id] = vm.new_host_calls()[..] else {
        panic!()
    };
    assert!(vm.new_host_calls().is_empty(), "reported once");
    vm.resolve_host_call(id, Ok(Value::int(5))).unwrap();
    let Status::Done(p) = vm.run(1000) else {
        panic!()
    };
    assert_eq!(vm.promise_state(p), Some(Some(Ok(Value::int(5)))));
    assert_eq!(vm.promise_state(Value::int(5)), None);
}

#[test]
fn image_with_pending_host_calls_and_jobs() {
    // Two async calls pending; one resolved but its job not run yet: the image carries
    // the pending call, the queued job and the reactions.
    let mut vm = vm();
    let tool = vm.new_async_host_function(TOOL, "tool").unwrap();
    vm.define_global("tool", tool).unwrap();
    let src = r#"
        const log = [];
        tool(1).then(v => log.push("one " + v));
        tool(2).then(v => { log.push("two " + v); console.log(log.join()); });
    "#;
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert_eq!(vm.run(1 << 20), Status::Idle);
    let ids = vm.new_host_calls();
    assert_eq!(ids.len(), 2);
    vm.resolve_host_call(ids[0], Ok(Value::int(10))).unwrap();
    vm.collect().unwrap();
    let mut moved = Vm::from_bytes(vm.bytes().to_vec(), VmOptions::default()).unwrap();
    drop(vm);
    assert_eq!(moved.pending_host_calls(), [ids[1]]);
    assert!(moved.new_host_calls().is_empty(), "already reported");
    let info = moved.host_call(ids[1]).unwrap();
    assert_eq!(info.args, [Value::int(2)]);
    assert_eq!(moved.run(1 << 20), Status::Idle);
    moved.resolve_host_call(ids[1], Ok(Value::int(20))).unwrap();
    let (moved, out) = finish(moved, 1 << 20, true);
    assert_eq!(out.log, ["one 10,two 20"]);
    assert!(moved.pending_host_calls().is_empty());
    moved.heap().verify().unwrap();
}

// ---- Suspendable frames (D1375), on hand-assembled bytecode -----------------------------

/// `function* g(a) { const x = yield a; const y = yield x + 1; return y * 2 }`
fn generator_template(vm: &mut Vm) -> Ref {
    let mut f = F::new();
    f.params = 1;
    f.locals = 1;
    f.flags = crate::bytecode::template::func_flags::KIND_NORMAL
        | crate::bytecode::template::func_flags::GENERATOR;
    let ret = f.b.new_label();
    f.b.generator_start(0);
    f.b.get_arg(0);
    f.b.yield_(0);
    f.b.jump_if_true(ret);
    f.b.push_i8(1);
    f.b.add();
    f.b.yield_(0);
    f.b.jump_if_true(ret);
    f.b.push_i8(2);
    f.b.mul();
    f.b.return_();
    f.b.place_label(ret);
    f.b.return_();
    f.build(vm)
}

/// `async function f(x) { return await x + 1 }` (`with_catch`: `async function h(p) {
/// try { return await p } catch (e) { return "caught " + e } }`).
fn async_template(vm: &mut Vm, with_catch: bool) -> Ref {
    let mut f = F::new();
    f.params = 1;
    f.locals = 1;
    f.flags = crate::bytecode::template::func_flags::KIND_NORMAL
        | crate::bytecode::template::func_flags::ASYNC;
    let caught = f.string(vm, "caught ");
    let (s, e, h) = (f.b.new_label(), f.b.new_label(), f.b.new_label());
    f.b.async_start(0);
    f.b.place_label(s);
    f.b.get_arg(0);
    f.b.await_(0);
    if !with_catch {
        f.b.push_i8(1);
        f.b.add();
    }
    f.b.async_return(0);
    f.b.place_label(e);
    f.b.place_label(h);
    if with_catch {
        f.b.push_const(caught);
        f.b.swap();
        f.b.add();
        f.b.async_return(0);
    } else {
        f.b.async_reject(0);
    }
    f.handlers.push((s, e, h, 0));
    f.build(vm)
}

/// Runs `src` (a Script) with the hand-assembled `g`, `f` and `h` as globals, in the
/// three modes of `run_main`.
fn run_with_suspendables(src: &str) -> Outcome {
    let mut outcomes = Vec::new();
    for (fuel, suspend, gc_stress) in [(1 << 40, false, false), (1, true, false), (7, false, true)]
    {
        let mut vm = vm_with(VmOptions {
            exec: ExecOptions {
                gc_stress,
                ..Default::default()
            },
            ..Default::default()
        });
        for (name, t) in [
            ("g", generator_template(&mut vm)),
            ("f", async_template(&mut vm, false)),
            ("h", async_template(&mut vm, true)),
            ("ag", async_generator_template(&mut vm)),
            ("gs", yield_star_template(&mut vm)),
            ("collect", for_await_template(&mut vm)),
            ("kinds", kinds_template(&mut vm)),
        ] {
            let v = vm.instantiate(t).unwrap();
            vm.define_global(name, v).unwrap();
        }
        let main = vm
            .compile(
                src.as_bytes(),
                &crate::compiler::CompileOptions::script("t.js"),
            )
            .unwrap();
        let this = Value::object(vm.global());
        vm.call(main, this, &[]).unwrap();
        let (vm, out) = finish(vm, fuel, suspend);
        vm.heap().verify().unwrap();
        outcomes.push(out);
    }
    assert_eq!(outcomes[0], outcomes[1], "suspension changed the outcome");
    assert_eq!(outcomes[0], outcomes[2], "gc stress changed the outcome");
    outcomes.remove(0)
}

#[test]
fn generators_suspend_and_resume_frames() {
    let out = run_with_suspendables(
        r#"
        const r = [];
        const it = g(5);
        r.push(Object.prototype.toString.call(it));
        let s = it.next("ignored"); r.push(s.value + ":" + s.done);
        s = it.next(10); r.push(s.value + ":" + s.done);
        s = it.next(7); r.push(s.value + ":" + s.done);
        s = it.next(); r.push(s.value + ":" + s.done);
        const it2 = g(1); it2.next(); s = it2.return(42); r.push(s.value + ":" + s.done);
        const it3 = g(1); it3.next(); try { it3.throw(new Error("boom")) } catch (e) { r.push("threw " + e.message) }; s = it3.next(); r.push(s.done);
        const it4 = g(1); try { it4.throw(1) } catch (e) { r.push("start throw " + e) }; r.push(it4.next().done);
        for (const v of g(3)) r.push("of " + v);
        console.log(r.join(", "));
        "#,
    );
    // Checked against qjs with the source functions.
    assert_eq!(
        out.log,
        [
            "[object Generator], 5:false, 11:false, 14:true, undefined:true, 42:true, threw boom, \
          true, start throw 1, true, of 3, of NaN"
        ]
    );
}

#[test]
fn async_functions_await_through_jobs() {
    let out = run_with_suspendables(
        r#"
        const log = [];
        f(41).then(v => log.push("f41 " + v));
        f(Promise.resolve(1)).then(v => log.push("fp " + v));
        f(Promise.reject(new Error("x"))).catch(e => log.push("frej " + e.message));
        h(Promise.reject("bad")).then(v => log.push("h " + v));
        h(5).then(v => log.push("h5 " + v));
        h({ then(r) { log.push("then called"); r("thenable") } }).then(v => log.push("h " + v));
        Promise.resolve().then(() => log.push("t1")).then(() => log.push("t2")).then(() => log.push("t3"));
        log.push("sync " + Object.prototype.toString.call(f(0)));
        Promise.resolve().then(()=>0).then(()=>0).then(()=>0).then(()=>0).then(() => console.log(log.join(", ")));
        "#,
    );
    // The order qjs gives for the source functions.
    assert_eq!(
        out.log,
        [
            "sync [object Promise], then called, t1, f41 42, fp 2, frej x, h caught bad, h5 5, t2, \
          h thenable, t3"
        ]
    );
}

#[test]
fn async_entry_awaits_async_host_calls() {
    // async () => { return (await tool("a")) + "!" + (await tool("b")) }, as the entry
    // call (a codemode body), through images and GC stress.
    let build = |vm: &mut Vm| {
        let tool = vm.heap_mut().atom_str("tool").unwrap();
        let mut f = F::new();
        f.locals = 1;
        f.flags = crate::bytecode::template::func_flags::KIND_ASYNC_BODY
            | crate::bytecode::template::func_flags::ASYNC;
        let a = f.string(vm, "a");
        let b = f.string(vm, "b");
        let bang = f.string(vm, "!");
        let (s, e, h) = (f.b.new_label(), f.b.new_label(), f.b.new_label());
        f.b.async_start(0);
        f.b.place_label(s);
        for (k, arg) in [a, b].into_iter().enumerate() {
            f.b.get_global(tool, 0);
            f.b.undefined();
            f.b.push_const(arg);
            f.b.call(1);
            f.b.await_(0);
            if k == 0 {
                f.b.push_const(bang);
                f.b.add();
            } else {
                f.b.add();
            }
        }
        f.b.async_return(0);
        f.b.place_label(e);
        f.b.place_label(h);
        f.b.async_reject(0);
        f.handlers.push((s, e, h, 0));
        f.build(vm)
    };
    for (fuel, suspend, gc_stress) in [(1 << 40, false, false), (1, true, false), (5, true, true)] {
        let mut vm = vm_with(VmOptions {
            exec: ExecOptions {
                gc_stress,
                ..Default::default()
            },
            ..Default::default()
        });
        let tool = vm.new_async_host_function(TOOL, "tool").unwrap();
        vm.define_global("tool", tool).unwrap();
        let t = build(&mut vm);
        let main = vm.instantiate(t).unwrap();
        vm.call(main, Value::UNDEFINED, &[]).unwrap();
        let result = loop {
            match vm.run(fuel) {
                Status::Done(p) => break p,
                Status::OutOfFuel => {}
                Status::Idle => {
                    let [id] = vm.new_host_calls()[..] else {
                        panic!("one call at a time")
                    };
                    let arg = vm.host_call(id).unwrap().args[0];
                    let s = vm.display(arg).to_uppercase();
                    let v = Value::string(vm.heap_mut().alloc_str(&s).unwrap());
                    vm.resolve_host_call(id, Ok(v)).unwrap();
                }
                s => panic!("{s:?}"),
            }
            if suspend {
                let o = vm.options();
                vm = Vm::from_bytes(vm.image().to_vec(), o).unwrap();
            }
        };
        let Some(Some(Ok(v))) = vm.promise_state(result) else {
            panic!("not fulfilled")
        };
        assert_eq!(vm.display(v), "A!B");
        vm.heap().verify().unwrap();
    }
}

// ---- Async generators, yield_star, for await, function kinds (0042) ----------------------

use crate::bytecode::template::func_flags;

/// `async function* ag(a) { const x = yield a; yield x + 1; return "end" }`, as 0041
/// compiles async generators (docs/compiler.md 7.7): the operand of a yield and of a
/// return is awaited, and a `return(v)` resumption awaits `v` and returns it.
fn async_generator_template(vm: &mut Vm) -> Ref {
    let mut f = F::new();
    f.params = 1;
    f.locals = 1;
    f.flags = func_flags::KIND_NORMAL | func_flags::ASYNC | func_flags::GENERATOR;
    let end = f.string(vm, "end");
    let ret = f.b.new_label();
    f.b.generator_start(0);
    f.b.get_arg(0);
    for k in 0..2 {
        if k == 1 {
            f.b.push_i8(1);
            f.b.add();
        }
        f.b.await_(0);
        f.b.yield_(0);
        f.b.jump_if_true(ret);
    }
    f.b.drop();
    f.b.push_const(end);
    f.b.await_(0);
    f.b.return_();
    f.b.place_label(ret);
    f.b.await_(0);
    f.b.return_();
    f.build(vm)
}

/// A sync generator that passes the result objects of the iterator in its argument on
/// with `yield_star` (one step of what `yield*` compiles to): `function* gs(it) { let r;
/// while (!(r = it.next()).done) yield_star r; return r.value }` (ignoring what it
/// receives).
fn yield_star_template(vm: &mut Vm) -> Ref {
    let mut f = F::new();
    f.params = 1;
    f.locals = 1;
    f.flags = func_flags::KIND_NORMAL | func_flags::GENERATOR;
    let next = vm.heap_mut().atom_str("next").unwrap();
    let (top, done) = (f.b.new_label(), f.b.new_label());
    f.b.generator_start(0);
    f.b.place_label(top);
    f.b.get_arg(0);
    f.b.get_method(next, 0);
    f.b.call(0);
    f.b.dup();
    f.b.get_field(crate::heap::atoms::DONE, 0);
    f.b.jump_if_true(done);
    f.b.yield_star(0);
    f.b.drop();
    f.b.drop();
    f.b.jump(top);
    f.b.place_label(done);
    f.b.get_field(crate::heap::atoms::VALUE, 0);
    f.b.return_();
    f.build(vm)
}

/// `async function collect(it) { const out = []; for await (const v of it) out.push(v);
/// return out }`, with `break` after `limit` values when given (closing the iterator).
fn for_await_template(vm: &mut Vm) -> Ref {
    use crate::bytecode::intrinsics::intrinsic_id;
    let mut f = F::new();
    f.params = 2;
    f.locals = 4; // 0 state, 1 iterator, 2 next, 3 out
    f.flags = func_flags::KIND_NORMAL | func_flags::ASYNC;
    let next = vm.heap_mut().atom_str("next").unwrap();
    let (s, e, h) = (f.b.new_label(), f.b.new_label(), f.b.new_label());
    let (top, done, brk, no_return) = (
        f.b.new_label(),
        f.b.new_label(),
        f.b.new_label(),
        f.b.new_label(),
    );
    f.b.async_start(0);
    f.b.place_label(s);
    f.b.array_from(0);
    f.b.put_loc(3);
    f.b.get_arg(0);
    f.b.call_intrinsic(intrinsic_id("GetAsyncIterator").unwrap(), 1);
    f.b.put_loc(1);
    f.b.get_loc(1);
    f.b.get_field(next, 0);
    f.b.put_loc(2);
    f.b.place_label(top);
    // limit reached? (arguments.length checks are the compiler's business; here
    // `limit === out.length` with limit undefined never holds)
    f.b.get_arg(1);
    f.b.get_loc(3);
    f.b.get_field(crate::heap::atoms::LENGTH, 0);
    f.b.strict_eq();
    f.b.jump_if_true(brk);
    f.b.get_loc(2);
    f.b.get_loc(1);
    f.b.call(0);
    f.b.await_(0);
    f.b.iter_check_object();
    f.b.dup();
    f.b.get_field(crate::heap::atoms::DONE, 0);
    f.b.jump_if_true(done);
    f.b.get_field(crate::heap::atoms::VALUE, 0);
    f.b.get_loc(3);
    f.b.swap();
    f.b.array_push();
    f.b.drop();
    f.b.jump(top);
    // break: AsyncIteratorClose with a normal completion.
    f.b.place_label(brk);
    f.b.get_loc(1);
    f.b.call_intrinsic(intrinsic_id("AsyncIteratorReturnMethod").unwrap(), 1);
    f.b.dup();
    f.b.is_undefined();
    f.b.jump_if_true(no_return);
    f.b.get_loc(1);
    f.b.call(0);
    f.b.await_(0);
    f.b.iter_check_object();
    f.b.place_label(no_return);
    f.b.drop();
    f.b.get_loc(3);
    f.b.async_return(0);
    f.b.place_label(done);
    f.b.drop();
    f.b.get_loc(3);
    f.b.async_return(0);
    f.b.place_label(e);
    f.b.place_label(h);
    f.b.async_reject(0);
    f.handlers.push((s, e, h, 0));
    f.build(vm)
}

/// `function kinds() { return [function(){}, async function(){}, function*(){},
/// async function*(){}, {*m(){}}.m-like method] }`: closures of each kind, made by
/// `fclosure`.
fn kinds_template(vm: &mut Vm) -> Ref {
    let mut children = Vec::new();
    for flags in [
        func_flags::KIND_NORMAL,
        func_flags::KIND_NORMAL | func_flags::ASYNC,
        func_flags::KIND_NORMAL | func_flags::GENERATOR,
        func_flags::KIND_NORMAL | func_flags::ASYNC | func_flags::GENERATOR,
        func_flags::KIND_METHOD | func_flags::GENERATOR,
        func_flags::KIND_ARROW | func_flags::ASYNC,
    ] {
        let mut c = F::new();
        c.flags = flags;
        c.b.return_undef();
        children.push(c.build(vm));
    }
    let mut f = F::new();
    let n = children.len();
    f.children = children;
    for i in 0..n {
        f.b.fclosure(i as u32);
    }
    f.b.array_from(n as u16);
    f.b.return_();
    f.build(vm)
}

#[test]
fn async_generators_queue_requests() {
    let out = run_with_suspendables(
        r#"
        const log = [];
        const show = (tag) => (r) => log.push(tag + ":" + r.value + ":" + r.done);
        const it = ag(Promise.resolve(5));
        it.next("ignored").then(show("1"));
        it.next(10).then(show("2"));
        it.next("x").then(show("3"));
        it.next().then(show("4"));
        const it2 = ag(1);
        it2.next();
        it2.return(Promise.resolve(42)).then(show("ret"));
        const it3 = ag(1);
        it3.return(7).then(show("startret"));
        it3.next().then(show("after3"));
        const it4 = ag(1);
        it4.next();
        it4.throw(new Error("boom")).catch((e) => log.push("throw:" + e.message));
        it4.next().then(show("after4"));
        const it5 = ag(1);
        it5.throw(new Error("early")).catch((e) => log.push("startthrow:" + e.message));
        Object.getPrototypeOf(it).next.call({}).catch((e) => log.push(e.name + ": " + e.message));
        log.push(Object.prototype.toString.call(it) + " " + (it[Symbol.asyncIterator]() === it));
        let p = Promise.resolve();
        for (let i = 0; i < 12; i++) p = p.then(() => 0);
        p.then(() => console.log(log.join(", ")));
        "#,
    );
    // The order qjs gives for the source function.
    // The order node gives for the source function (`const x = yield a; yield x + 1;
    // return "end"`). qjs agrees except that it never settles "after3" (a next() queued
    // behind a return() on a generator that has not started).
    assert_eq!(
        out.log,
        [
            "[object AsyncGenerator] true, startthrow:early, TypeError: not an AsyncGenerator \
          object, 1:5:false, startret:7:true, after3:undefined:true, throw:boom, \
          after4:undefined:true, 2:11:false, ret:42:true, 3:end:true, 4:undefined:true"
        ]
    );
}

#[test]
fn yield_star_passes_inner_results_through() {
    let out = run_with_suspendables(
        r#"
        const inner = { i: 0, next() { this.i++; return this.i < 3 ? { value: this.i, done: false, extra: "x" } : { value: "r", done: true }; } };
        const it = gs(inner);
        const a = it.next();
        const b = it.next();
        const c = it.next();
        const show = (r) => Object.keys(r).map((k) => k + "=" + r[k]).join(" ");
        console.log([a, b, c].map(show).join(" | "), a === b);
        "#,
    );
    assert_eq!(out.result, "undefined");
    assert_eq!(
        out.log,
        ["value=1 done=false extra=x | value=2 done=false extra=x | value=r done=true false"]
    );
}

#[test]
fn for_await_over_sync_async_and_generator_iterables() {
    let out = run_with_suspendables(
        r#"
        const log = [];
        collect([1, Promise.resolve(2), 3]).then((v) => log.push("sync " + v.join()));
        collect(ag(Promise.resolve(1))).then((v) => log.push("asyncgen " + v.join()));
        let closed = false;
        const custom = { [Symbol.asyncIterator]() { let i = 0; return {
          next() { i++; return Promise.resolve({ value: i * 10, done: i > 3 }); },
          return() { closed = true; return { done: true }; } }; } };
        collect(custom, 2).then((v) => log.push("custom " + v.join() + " closed " + closed));
        collect([Promise.reject(new Error("bad")), 2]).catch((e) => log.push("rejected " + e.message));
        collect(1).catch((e) => log.push(e.name + ": " + e.message));
        let p = Promise.resolve();
        for (let i = 0; i < 20; i++) p = p.then(() => 0);
        p.then(() => console.log(log.join(", ")));
        "#,
    );
    // The order qjs and node give for the source function.
    assert_eq!(
        out.log,
        [
            "TypeError: not a function, rejected bad, custom 10,20 closed true, asyncgen 1,NaN, \
          sync 1,2,3"
        ]
    );
}

#[test]
fn function_kinds_have_their_prototypes() {
    let out = run_with_suspendables(
        r#"
        const [plain, af, gf, agf, gm, arrow] = kinds();
        const P = Object.getPrototypeOf;
        const GF = P(gf), AF = P(af), AGF = P(agf);
        const r = [
          P(plain) === Function.prototype, AF !== Function.prototype, P(AF) === Function.prototype,
          P(gm) === GF, P(arrow) === AF,
          GF.prototype === P(gf.prototype), P(gm.prototype) === GF.prototype,
          AGF.prototype === P(agf.prototype), P(AGF.prototype) === P(P(AGF.prototype)) ? "x" : "ok",
          "prototype" in af, "prototype" in arrow,
          GF[Symbol.toStringTag], AF[Symbol.toStringTag], AGF[Symbol.toStringTag], AGF.prototype[Symbol.toStringTag],
          GF.constructor.name, AF.constructor.name, AGF.constructor.name, GF.constructor.length,
          P(GF.constructor) === Function, GF.constructor.prototype === GF,
          Object.getOwnPropertyNames(GF).sort().join("/"), Object.getOwnPropertyNames(AF).join("/"),
          Object.entries(Object.getOwnPropertyDescriptor(GF, "prototype")).filter((e) => e[0] !== "value").join("/"),
        ];
        r.push([...new GF.constructor("yield 1")()].join());
        console.log(r.join(", "));
        "#,
    );
    assert_eq!(out.result, "undefined");
    assert_eq!(
        out.log,
        [
            "true, true, true, true, true, true, true, true, ok, false, false, GeneratorFunction, \
          AsyncFunction, AsyncGeneratorFunction, AsyncGenerator, GeneratorFunction, \
          AsyncFunction, AsyncGeneratorFunction, 1, true, true, constructor/prototype, \
          constructor, writable,false/enumerable,false/configurable,true, 1"
        ]
    );
}

#[test]
fn collect_drops_popped_stack_values() {
    // A finished call leaves its frame's values above `sp`; `Vm::collect` must not
    // keep them alive (0043: the base image kept dead templates that way).
    let mut vm = vm();
    let src = "function f() { const big = []; for (let i = 0; i < 20000; i++) big.push(i); return big.length } f();";
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let used = |vm: &Vm| vm.heap().cells().map(|c| c.size as usize).sum::<usize>();
    let h = vm.heap_mut().new_handle(f).unwrap();
    vm.collect().unwrap();
    let before = used(&vm);
    let f = vm.heap().get_handle(h);
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert!(matches!(vm.run(1 << 30), Status::Done(_)));
    vm.collect().unwrap();
    // The 20000-element array (160 KB of elements) is gone.
    assert!(
        used(&vm) < before + 20_000,
        "the array survived: {} -> {}",
        before,
        used(&vm)
    );
}

/// Runs `src` as a Script with a memory limit: the outcome (`None` if the VM cannot
/// even be created). With `suspend`, the VM is rebuilt from its image after every
/// `fuel` slice.
fn run_limited(src: &str, limit: usize, fuel: u64, suspend: bool) -> Option<Outcome> {
    let mut vm = Vm::new(VmOptions {
        memory_limit: limit,
        ..Default::default()
    })
    .ok()?;
    let console = vm.new_object().ok()?;
    let log = vm.new_host_function(LOG, "log").ok()?;
    vm.define(console, "log", log).ok()?;
    vm.define_global("console", Value::object(console)).ok()?;
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .ok()?;
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).ok()?;
    let (vm, out) = finish(vm, fuel, suspend);
    vm.heap().verify().unwrap();
    Some(out)
}

/// Scripts that run out of memory and handle it, each logging one line with "out of
/// memory" when the handler ran: in a caller, in the same frame after dropping the data,
/// in a promise reaction, and as pi's prelude reports a failed body (D1600).
const OOM_CASES: [(&str, &str); 5] = [
    (
        "caller",
        "function fill() { const a = []; for (;;) a.push({ a: 'x'.repeat(1000) + a.length }); }\n\
         try { fill(); } catch (e) { console.log(String(e)); }",
    ),
    (
        "same frame",
        "const a = []; try { for (;;) a.push({ a: 'x'.repeat(1000) + a.length }); }\n\
         catch (e) { a.length = 0; console.log(String(e)); }",
    ),
    (
        "data still live",
        "const a = []; try { for (;;) a.push('x'.repeat(100) + a.length); }\n\
         catch (e) { console.log(String(e), a.length > 0); }",
    ),
    (
        "reaction",
        "async function fill() { const a = []; for (;;) a.push({ a: 'x'.repeat(1000) + a.length }); }\n\
         fill().catch((e) => console.log(String(e)));",
    ),
    (
        "pi prelude",
        "(async () => { const a = []; for (;;) a.push({ a: 'x'.repeat(1000) + a.length }); })().then(\n\
           (v) => console.log(JSON.stringify({ ok: true, v })),\n\
           (e) => console.log(JSON.stringify({ ok: false, error: JSON.stringify({ name: e.name, message: e.message, stack: e.stack }) })));",
    ),
];

/// A fixed sample of `all` for the default test run: the first and the last element plus
/// `n` more picked by a fixed pseudo-random sequence (the same every run). The exhaustive
/// sweeps are `#[ignore]`d tests, run before merging changes to what they cover.
fn sample<T: Copy>(all: &[T], n: usize) -> Vec<T> {
    let mut picked = alloc::vec![false; all.len()];
    picked[0] = true;
    picked[all.len() - 1] = true;
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        picked[(x % all.len() as u64) as usize] = true;
    }
    all.iter()
        .zip(picked)
        .filter(|p| p.1)
        .map(|p| *p.0)
        .collect()
}

/// Out-of-memory handling at the limits from 1 MiB to 12 MiB in 352 KiB steps.
fn out_of_memory_at(limits: &[usize]) {
    for &kb in limits {
        for (name, src) in OOM_CASES {
            let Some(o) = run_limited(src, kb << 10, 1 << 40, false) else {
                continue;
            };
            assert!(
                // (The result is the Script's completion value: a promise for some.)
                !o.result.starts_with("throw")
                    && o.log.len() == 1
                    && o.log[0].contains("out of memory"),
                "{kb} KiB, {name}: {} {:?}",
                o.result,
                o.log
            );
        }
    }
}

fn oom_limits() -> Vec<usize> {
    (1024..12 * 1024).step_by(352).collect()
}

#[test]
fn out_of_memory_is_a_catchable_error_at_sampled_limits() {
    out_of_memory_at(&sample(&oom_limits(), 3));
}

/// The full sweep (about 20 s): `cargo test -p djs-core --lib -- --ignored
/// out_of_memory_is_a_catchable_error_at_every_limit`.
#[test]
#[ignore]
fn out_of_memory_is_a_catchable_error_at_every_limit() {
    out_of_memory_at(&oom_limits());
}

#[test]
fn a_script_continues_after_out_of_memory() {
    // Catch, drop the data, and go on allocating (less): repeatedly.
    let src = "let caught = 0;\n\
               for (let round = 0; round < 3; round++) {\n\
                 let a = [];\n\
                 try { for (;;) a.push('y'.repeat(500) + a.length); } catch (e) { caught++; }\n\
                 a = null;\n\
                 const b = []; for (let i = 0; i < 1000; i++) b.push({ i });\n\
               }\n\
               console.log(caught);";
    for mb in [2, 5, 9] {
        let o = run_limited(src, mb << 20, 1 << 40, false).unwrap();
        assert_eq!(
            (o.result.as_str(), o.log),
            ("undefined", alloc::vec![String::from("3")]),
            "{mb} MiB"
        );
    }
}

#[test]
fn the_out_of_memory_point_is_deterministic() {
    // How far the loop gets depends only on the script and the limit: not on the fuel
    // slices, and not on snapshots taken in between (the GC threshold is in the image,
    // D1601).
    let src = "const a = []; let n = 0;\n\
               try { for (;;) { a.push({ s: 'z'.repeat(300) + n }); n++; } }\n\
               catch (e) { a.length = 0; console.log(n, String(e)); }";
    for mb in [2, 6] {
        let plain = run_limited(src, mb << 20, 1 << 40, false).unwrap();
        let sliced = run_limited(src, mb << 20, 7919, false).unwrap();
        let restored = run_limited(src, mb << 20, 50_000, true).unwrap();
        assert_eq!(plain, sliced, "{mb} MiB");
        assert_eq!(plain, restored, "{mb} MiB");
        assert!(plain.log[0].ends_with("out of memory"), "{:?}", plain.log);
    }
}

#[test]
fn run_metered_and_backtrace() {
    // 0064: the REPL's fuel accounting and "where did it stop".
    let mut vm = Vm::new(VmOptions::default()).unwrap();
    let opts = crate::compiler::CompileOptions::script("t.js");
    let f = vm
        .compile(
            b"let n = 0;\nfunction spin() {\n  for (;;) { n++; }\n}\nspin();",
            &opts,
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let (st, used) = vm.run_metered(1000);
    assert_eq!(st, Status::OutOfFuel);
    assert!(used >= 1000, "{used}");
    let bt = vm.backtrace();
    assert!(bt.starts_with("    at spin (t.js:3:"), "{bt}");
    assert!(bt.contains("at <eval> (t.js:5:"), "{bt}");
    vm.abort();
    assert_eq!(vm.backtrace(), "");
    let f = vm.compile(b"1 + 1;", &opts).unwrap();
    vm.call(f, this, &[]).unwrap();
    let (st, used) = vm.run_metered(1_000_000);
    assert!(matches!(st, Status::Done(_)));
    assert!(used > 0 && used < 1000, "{used}");
}

/// Empties every inline cache of the heap (a "cold" VM in the same state), returning how
/// many functions had one.
fn clear_inline_caches(vm: &mut Vm) -> usize {
    use crate::heap::CellKind;
    use crate::heap::cells::FuncTemplate;
    let templates: Vec<u32> = vm
        .heap()
        .cells()
        .filter(|c| c.kind == CellKind::FuncTemplate)
        .map(|c| c.offset)
        .collect();
    let mut warm = 0;
    for t in templates {
        let a = vm.heap_mut().arena_mut();
        if a.get::<Ref>(t, FuncTemplate::IC).get().is_some() {
            warm += 1;
            a.set(t, FuncTemplate::IC, Ref::NULL);
        }
    }
    warm
}

/// Global accesses whose caches were filled before the globals changed under them.
const GLOBAL_CHANGES: &str = "\
    globalThis.g = 'obj';\n\
    Object.defineProperty(globalThis, 'ro', { value: 'ro', writable: false, configurable: true });\n\
    function readG() { return typeof g === 'undefined' ? 'missing' : g; }\n\
    function writeG(v) { g = v; return g; }\n\
    function writeRo(v) { try { ro = v; return ro; } catch (e) { return String(e); } }\n\
    function readX() { try { return x; } catch (e) { return String(e); } }\n\
    for (let i = 0; i < 3; i++) { readG(); writeG(i); writeRo(i); readX(); }\n\
    globalThis.x = 'global x';\n\
    for (let i = 0; i < 3; i++) readX();\n";

const GLOBAL_CHANGES_2: &str = "\
    console.log(readG(), writeG('w'), writeRo(1), readX());\n\
    Object.defineProperty(globalThis, 'ro', { value: 'ro', writable: true, configurable: true });\n\
    console.log(writeRo(2), writeRo(3));\n\
    delete globalThis.g;\n\
    console.log(readG());\n\
    Object.defineProperty(globalThis, 'g', { get() { return 'getter'; }, configurable: true });\n\
    console.log(readG());\n";

#[test]
fn global_caches_follow_every_change() {
    let (vm, _) = run_script(vm(), GLOBAL_CHANGES);
    let (vm, out) = run_script(vm, GLOBAL_CHANGES_2);
    assert_eq!(
        out.log,
        [
            "2 w TypeError: 'ro' is read-only global x",
            "2 3",
            "missing",
            "getter"
        ]
    );
    // A lexical binding of a later Script shadows the global object property a cache
    // points at.
    let (_, out) = run_script(vm, "let x = 'lexical x';\nconsole.log(readX());\n");
    assert_eq!(out.log, ["lexical x"]);
}

#[test]
fn warm_and_cold_inline_caches_behave_the_same() {
    // Warm the caches, take an image, and run the same script in a VM restored from it,
    // once with the caches as they are and once with all of them emptied (D1620).
    let fields = "\
        function getA(o) { return o.a; }\n\
        function setA(o, v) { o.a = v; return o.a; }\n\
        function callM(o) { return o.m(); }\n\
        globalThis.obj = { x: 1, a: 2, m() { return 'm'; } };\n\
        globalThis.Proto = { a: 'proto a', m() { return 'proto m'; } };\n\
        globalThis.child = Object.create(Object.create(Proto));\n\
        for (let i = 0; i < 3; i++) { getA(obj); setA(obj, i); callM(obj); getA(child); callM(child); 'x'.toUpperCase(); }\n";
    let fields_2 = "\
        console.log(getA(obj), setA(obj, 'w'), callM(obj));\n\
        delete obj.x;\n\
        console.log(getA(obj), setA(obj, 'v'), callM(obj));\n\
        Object.defineProperty(obj, 'a', { value: 'ro', writable: false });\n\
        try { setA(obj, 1); } catch (e) { console.log(String(e)); }\n\
        console.log(getA({ a: 'other' }), getA([]));\n\
        console.log(getA(child), callM(child));\n\
        Object.getPrototypeOf(child).a = 'shadow';\n\
        console.log(getA(child), callM(child));\n";
    let (vm, _) = run_script(vm(), &(String::from(GLOBAL_CHANGES) + fields));
    let image = vm.bytes().to_vec();
    let warm = Vm::from_bytes(image.clone(), VmOptions::default()).unwrap();
    let mut cold = Vm::from_bytes(image, VmOptions::default()).unwrap();
    assert!(
        clear_inline_caches(&mut cold) > 0,
        "no warm caches in the image"
    );
    let (_, warm_out) = run_script(warm, &(String::from(GLOBAL_CHANGES_2) + fields_2));
    let (_, cold_out) = run_script(cold, &(String::from(GLOBAL_CHANGES_2) + fields_2));
    assert_eq!(warm_out, cold_out);
    assert_eq!(warm_out.log.len(), 10, "{:?}", warm_out.log);
}

#[test]
fn warm_and_cold_addition_caches_behave_the_same() {
    // 0083: addition caches (constructors, literals) and receiver-shape prototype reads,
    // warmed, imaged, then run against a prototype chain that changes under them.
    let warmup = "\
        function P(i) { this.x = i; this.y = i; }\n\
        globalThis.mk = (i) => new P(i);\n\
        globalThis.lit = (i) => ({ a: i, b: i });\n\
        globalThis.addZ = (o, v) => { o.z = v; return o; };\n\
        globalThis.Base = { m() { return 'base'; } };\n\
        globalThis.callM = (o) => o.m();\n\
        for (let i = 0; i < 3; i++) { mk(i); lit(i); addZ({ w: i }, i); callM(Object.create(Base)); }\n";
    let after = "\
        console.log(JSON.stringify(mk(1)), JSON.stringify(lit(2)), JSON.stringify(addZ({ w: 0 }, 3)));\n\
        let seen = [];\n\
        Object.defineProperty(P.prototype, 'y', { set(v) { seen.push(v); }, configurable: true });\n\
        Object.defineProperty(Object.prototype, 'b', { set(v) { seen.push('b' + v); }, configurable: true });\n\
        Object.defineProperty(Object.prototype, 'z', { value: 'ro', writable: false, configurable: true });\n\
        let r; try { addZ({ w: 0 }, 4); r = 'added'; } catch (e) { r = String(e); }\n\
        console.log(JSON.stringify(mk(5)), JSON.stringify(lit(6)), r, seen.join());\n\
        const shadow = Object.create(Base); shadow.m = () => 'own';\n\
        console.log(callM(Object.create(Base)), callM(shadow));\n\
        const ne = Object.preventExtensions({ w: 1 });\n\
        delete Object.prototype.z;\n\
        try { addZ(ne, 1); } catch (e) { console.log(String(e)); }\n";
    let (vm, _) = run_script(vm(), warmup);
    let image = vm.bytes().to_vec();
    let warm = Vm::from_bytes(image.clone(), VmOptions::default()).unwrap();
    let mut cold = Vm::from_bytes(image, VmOptions::default()).unwrap();
    assert!(clear_inline_caches(&mut cold) > 0, "no warm caches");
    let (_, warm_out) = run_script(warm, after);
    let (_, cold_out) = run_script(cold, after);
    assert_eq!(warm_out, cold_out);
    assert_eq!(
        warm_out.log,
        [
            "{\"x\":1,\"y\":1} {\"a\":2,\"b\":2} {\"w\":0,\"z\":3}",
            "{\"x\":5} {\"a\":6,\"b\":6} TypeError: 'z' is read-only 5",
            "base own",
            "TypeError: object is not extensible"
        ]
    );
}

#[test]
fn privileged_templates_have_no_debug_fields() {
    // D1980, D2444: the prelude's templates have no debug info (a FuncInfo only for
    // child templates); user code keeps it (line numbers, source text, local names).
    use crate::bytecode::template::debug_info;
    use crate::heap::CellKind;
    use crate::heap::cells::FuncTemplate;
    let (vm, out) = run_script(
        vm(),
        "function f(a) { return a; }\nconsole.log(String(f), String(Array.prototype.map));\n",
    );
    assert_eq!(
        out.log,
        ["function f(a) { return a; } function map() {\n    [native code]\n}"]
    );
    let h = vm.heap();
    let (mut slim, mut full) = (0, 0);
    for c in h.cells().filter(|c| c.kind == CellKind::FuncTemplate) {
        let privileged = u32::from(h.arena().get::<u16>(c.offset, FuncTemplate::FLAGS))
            & crate::bytecode::template::func_flags::PRIVILEGED
            != 0;
        let info = h
            .arena()
            .get::<crate::heap::Ref>(c.offset, FuncTemplate::INFO);
        if privileged {
            slim += 1;
            assert!(
                info.is_null() || crate::bytecode::template::child_count(h, c.offset) > 0,
                "template at {}",
                c.offset
            );
            assert!(debug_info(h, c.offset).script.is_null());
        } else {
            full += 1;
            assert!(!debug_info(h, c.offset).script.is_null());
        }
    }
    assert!(slim > 500 && full >= 2, "{slim} {full}");
    assert!(crate::builtins::fast::FAST.len() < u8::MAX as usize);
    assert!(crate::builtins::r#gen::natives::NATIVES.len() < u8::MAX as usize);
}

/// 0101 follow-up (docs/old-generation.md): what a collection of a VM made from the base
/// costs, against a linear scan of the base region for references out of it (the work a
/// sticky base region would do instead of tracing it). `cargo test --release -p djs-core
/// --lib base_region_scan_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn base_region_scan_cost() {
    extern crate std;
    use crate::heap::walk::{self, Slot};
    use std::time::Instant;
    let mut base = vm();
    base.seal().unwrap();
    let base_len = base.heap().bytes().len() as u32;
    let (vm, _) = run_script(
        base,
        "globalThis.keep = []; for (let i = 0; i < 2000; i++) { const t = { x: i, y: [i] }; if (i % 10 == 0) keep.push(t); }",
    );
    let image = vm.heap().bytes().to_vec();
    let base_len = base_len.min(image.len() as u32);
    let n = 50;
    let mut copies: Vec<Heap> = (0..n)
        .map(|_| Heap::from_bytes_trusted(image.clone(), 1 << 30).unwrap())
        .collect();
    let t = Instant::now();
    for h in &mut copies {
        h.collect().unwrap();
    }
    let mc = t.elapsed().as_secs_f64() / n as f64;
    let mut copies: Vec<Heap> = (0..n)
        .map(|_| Heap::from_bytes_trusted(image.clone(), 1 << 30).unwrap())
        .collect();
    let t = Instant::now();
    for h in &mut copies {
        h.collect_canonical().unwrap();
    }
    let cheney = t.elapsed().as_secs_f64() / n as f64;
    // A minor collection (0120): the base is the old region.
    let mut copies: Vec<Heap> = (0..n)
        .map(|_| Heap::from_bytes_trusted(image.clone(), 1 << 30).unwrap())
        .collect();
    assert_eq!(copies[0].old_end(), base_len);
    let t = Instant::now();
    for h in &mut copies {
        h.collect_minor().unwrap();
    }
    let minor = t.elapsed().as_secs_f64() / n as f64;
    let h = Heap::from_bytes_trusted(image.clone(), 1 << 30).unwrap();
    let t = Instant::now();
    let mut out = 0u32;
    for _ in 0..n {
        let a = h.arena();
        let mut c = 0;
        while c < base_len {
            let kind = h.cell_kind(c);
            let size = h.cell_size(c);
            walk::for_each_slot(kind, c, size, |slot| {
                let t = match slot {
                    Slot::Value(s) => a.read::<Value>(s).ref_offset(),
                    Slot::Ref(s) => a.read::<crate::heap::Ref>(s).get(),
                };
                if t.is_some_and(|t| t >= base_len) {
                    out += 1;
                }
            });
            c += size;
        }
    }
    let scan = t.elapsed().as_secs_f64() / n as f64;
    std::println!(
        "base {base_len} B, arena {} B: cheney {:.3} ms, mark-compact {:.3} ms, minor {:.3} ms, linear scan of the base {:.3} ms ({:.1}x less than mark-compact), {} refs out per scan",
        image.len(),
        cheney * 1e3,
        mc * 1e3,
        minor * 1e3,
        scan * 1e3,
        mc / scan,
        out / n
    );
}

// ---- The host inbox (D1670) --------------------------------------------------------------

/// When the "embedder" of [`run_inbox`] delivers async results.
#[derive(Clone, Copy, Debug)]
enum Delivery {
    /// At the first stop after the call (any status: out of fuel, a sync host call).
    Eager,
    /// One result per IDLE, oldest first.
    OneAtIdle,
    /// Every pending result at IDLE, in one batch.
    BatchAtIdle,
}

/// Runs a Script with `console.log` and `tool(x)`, an async host function the embedder
/// completes with `x` (or rejects with `x` when it is a string starting with "fail"),
/// delivered per `delivery`, with the clock `1000 * call_id` when `clock` is set. The
/// unhandled rejections are taken at every IDLE and at the end, and listed after the
/// console output; `suspend` continues in a VM restored from the image at every stop.
fn run_inbox(src: &str, fuel: u64, suspend: bool, delivery: Delivery, clock: bool) -> Outcome {
    let mut vm = vm();
    let tool = vm.new_async_host_function(TOOL, "tool").unwrap();
    vm.define_global("tool", tool).unwrap();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let mut log = Vec::new();
    let mut unhandled = Vec::new();
    let mut pending: Vec<u32> = Vec::new();
    let take_rejections = |vm: &mut Vm, log: &mut Vec<String>| {
        for r in vm.take_unhandled_rejections() {
            log.push(format!("unhandled {}", vm.display(r)));
        }
    };
    loop {
        let status = vm.run(fuel);
        pending.extend(vm.new_host_calls());
        let deliver = |vm: &mut Vm, id: u32| {
            let arg = vm.host_call(id).unwrap().args[0];
            let fail = vm.display(arg).starts_with("fail");
            let result = if fail { Err(arg) } else { Ok(arg) };
            if clock {
                vm.resolve_host_call_at(id, result, 1000.0 * id as f64)
                    .unwrap();
            } else {
                vm.resolve_host_call(id, result).unwrap();
            }
        };
        let idle = status == Status::Idle;
        match delivery {
            Delivery::Eager => pending.drain(..).for_each(|id| deliver(&mut vm, id)),
            Delivery::OneAtIdle if idle => deliver(&mut vm, pending.remove(0)),
            Delivery::BatchAtIdle if idle => pending.drain(..).for_each(|id| deliver(&mut vm, id)),
            _ => {}
        }
        match status {
            Status::Done(v) | Status::Threw(v) => {
                assert!(pending.is_empty());
                let result = match status {
                    Status::Done(_) => vm.display(v),
                    _ => format!("throw {}", vm.display(v)),
                };
                take_rejections(&mut vm, &mut unhandled);
                vm.heap().verify().unwrap();
                log.extend(unhandled);
                return Outcome { result, log };
            }
            Status::Idle => take_rejections(&mut vm, &mut unhandled),
            Status::OutOfFuel => {}
            Status::HostOutOfMemory => panic!("out of host memory"),
            Status::HostCall { call_id, fn_id } => {
                assert_eq!(fn_id, LOG);
                let info = vm.host_call(call_id).unwrap();
                let line: Vec<String> = info.args.iter().map(|&a| vm.display(a)).collect();
                log.push(line.join(" "));
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
        }
        if suspend {
            let opts = vm.options();
            vm = Vm::from_bytes(vm.image().to_vec(), opts).unwrap();
        }
    }
}

/// The outcome of `src` must not depend on when results are delivered, the fuel slice or
/// restores; returns it.
fn inbox_outcome(src: &str, clock: bool) -> Outcome {
    let reference = run_inbox(src, u64::MAX, false, Delivery::OneAtIdle, clock);
    for delivery in [Delivery::Eager, Delivery::OneAtIdle, Delivery::BatchAtIdle] {
        for fuel in [u64::MAX, 1000, 37] {
            for suspend in [false, true] {
                if suspend && fuel == u64::MAX {
                    continue;
                }
                let got = run_inbox(src, fuel, suspend, delivery, clock);
                assert_eq!(
                    got, reference,
                    "{delivery:?}, fuel {fuel}, suspend {suspend}"
                );
            }
        }
    }
    reference
}

#[test]
fn host_results_apply_at_microtask_checkpoints() {
    // 0055's async_interleave.js: resolved while the loop runs, the result used to land
    // between b1 and b2.
    let out = inbox_outcome(
        r#"
        tool(1).then(v => console.log("a", v));
        let x = 0;
        for (let i = 0; i < 2000; i++) x += i;
        Promise.resolve().then(() => { console.log("b1"); Promise.resolve().then(() => console.log("b2")); });
        "#,
        false,
    );
    assert_eq!(out.log, ["b1", "b2", "a 1"]);
}

#[test]
fn host_results_are_macrotasks_one_at_a_time() {
    // Two results delivered together run as two macrotasks: the first one's reactions
    // (and their microtasks) all run before the second result is applied.
    let out = inbox_outcome(
        r#"
        const a = tool("a"), b = tool("b");
        a.then(v => { console.log("a", v); Promise.resolve().then(() => console.log("a micro")); });
        b.then(v => console.log("b", v));
        "#,
        false,
    );
    assert_eq!(out.log, ["a a", "a micro", "b b"]);
}

#[test]
fn rejections_are_judged_after_each_host_result() {
    // A rejection still unhandled when its macrotask ends is reported, even if a later
    // macrotask handles it, whether the host takes the reports in between or not.
    let out = inbox_outcome(
        r#"
        let late;
        tool("fail early").catch(() => {}).then(() => { late = Promise.reject(new Error("late")); });
        tool("x").then(() => { late.catch(() => console.log("handled late")); });
        Promise.reject(new Error("sync")).catch(() => console.log("handled in time"));
        "#,
        false,
    );
    assert_eq!(
        out.log,
        ["handled in time", "handled late", "unhandled Error: late"]
    );
}

#[test]
fn host_results_carry_their_clock() {
    let out = inbox_outcome(
        r#"
        const log = [];
        tool(1).then(() => log.push(Date.now()));
        tool(2).then(() => { log.push(Date.now()); console.log(log.join()); });
        "#,
        true,
    );
    // Call ids start at 1, but console.log's sync calls take ids too; the clock is
    // 1000 * the async call's id.
    assert_eq!(out.log.len(), 1);
    let times: Vec<f64> = out.log[0].split(',').map(|t| t.parse().unwrap()).collect();
    assert!(times[0] < times[1] && times[0] >= 1000.0, "{times:?}");
}

#[test]
fn delivered_results_survive_images() {
    // Results delivered but not applied yet are in the image.
    let mut vm = vm();
    let tool = vm.new_async_host_function(TOOL, "tool").unwrap();
    vm.define_global("tool", tool).unwrap();
    let src = r#"
        const log = [];
        tool(1).then(v => log.push("one " + v));
        tool(2).then(v => { log.push("two " + v); console.log(log.join()); });
    "#;
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert_eq!(vm.run(1 << 20), Status::Idle);
    let ids = vm.new_host_calls();
    vm.resolve_host_call(ids[0], Ok(Value::int(10))).unwrap();
    vm.resolve_host_call_at(ids[1], Ok(Value::int(20)), 5.0)
        .unwrap();
    assert!(vm.pending_host_calls().is_empty());
    vm.collect().unwrap();
    let moved = Vm::from_bytes(vm.bytes().to_vec(), VmOptions::default()).unwrap();
    drop(vm);
    let (_, out) = finish(moved, 1 << 20, false);
    assert_eq!(out.log, ["one 10,two 20"]);
}

#[test]
fn abort_drops_delivered_results() {
    let mut vm = vm();
    let tool = vm.new_async_host_function(TOOL, "tool").unwrap();
    vm.call(tool, Value::UNDEFINED, &[Value::int(1)]).unwrap();
    assert_eq!(vm.run(100), Status::Idle);
    let [id] = vm.new_host_calls()[..] else {
        panic!()
    };
    vm.resolve_host_call(id, Ok(Value::int(5))).unwrap();
    vm.abort();
    assert_eq!(vm.run(100), Status::Idle);
}

/// D1750: the build hash covers the enabled features (and djs-libm's sources, D3174, which
/// build.rs hashes with djs-core's).
#[test]
fn build_inputs_list_features() {
    extern crate std;
    let mut want: Vec<String> = Vec::new();
    if cfg!(feature = "embed") {
        want.push("feature embed".into());
    }
    if cfg!(feature = "full") {
        want.push("feature full".into());
    }
    if cfg!(feature = "regexp-unicode-props") {
        want.push("feature regexp-unicode-props".into());
    }
    if cfg!(feature = "unicode-normalization") {
        want.push("feature unicode-normalization".into());
    }
    assert_eq!(
        BUILD_INPUTS,
        want.iter().map(String::as_str).collect::<Vec<_>>()
    );
}

#[test]
fn collections_empty_prototype_caches_and_stay_deterministic() {
    // Prototype slots hold offsets: a collection empties them (D1621), so collecting a
    // collected heap still reproduces it byte for byte (D1090), and lookups stay right.
    let src = "globalThis.P = { m() { return 'm'; } };\n\
               globalThis.o = Object.create(P);\n\
               globalThis.f = (x) => x.m();\n\
               for (let i = 0; i < 3; i++) f(o);\n";
    let (mut vm, _) = run_script(vm(), src);
    vm.collect().unwrap();
    let a = vm.bytes().to_vec();
    vm.collect().unwrap();
    assert!(a == vm.bytes(), "a collection changed a collected image");
    let (_, out) = run_script(vm, "console.log(f(o), f(Object.create(o)));\n");
    assert_eq!(out.log, ["m m"]);
}

#[test]
fn a_template_above_its_moving_cache_still_has_it_emptied() {
    // The compactor updates the cells and slides them in one walk (0224), so a template
    // cannot empty its cache's offset slots (D1621) on the way: the cache cell may have
    // moved already. They are emptied when the marker traces the template. Built by hand:
    // f's template is copied above its cache, with garbage below the cache.
    use crate::heap::CellKind;
    use crate::heap::cells::{FuncTemplate, IC_OFFSET_SLOT, IC_WORDS, Object, U32Array};
    use crate::heap::layout::Ref;
    let src = "globalThis.P = { m() { return 'm'; } };\n\
               globalThis.o = Object.create(P);\n\
               globalThis.junk = 'x'.repeat(5000) + 1;\n\
               globalThis.f = (x) => x.m();\n\
               for (let i = 0; i < 3; i++) f(o);\n\
               junk = null;\n";
    let (mut vm, _) = run_script(vm(), src);
    let key = vm.heap_mut().atom_str("f").unwrap();
    let template_of = |vm: &Vm| {
        let h = vm.heap();
        let f = h.get_own_property(vm.global(), key).unwrap().value();
        let f = f.and_then(|v| v.as_object()).unwrap().offset();
        (f, h.arena().get::<Ref>(f, Object::ELEMENTS).get().unwrap())
    };
    let offset_slots = |vm: &Vm, t: u32| {
        let a = vm.heap().arena();
        let ic = a.get::<Ref>(t, FuncTemplate::IC).get().unwrap();
        let n = u32::from(a.get(t, FuncTemplate::IC_COUNT));
        (0..n)
            .filter(|&i| a.read::<u32>(U32Array::DATA.at(ic, i * IC_WORDS)) & IC_OFFSET_SLOT != 0)
            .count()
    };
    let (f, t) = template_of(&vm);
    let ic = vm
        .heap()
        .arena()
        .get::<Ref>(t, FuncTemplate::IC)
        .get()
        .unwrap();
    assert!(ic > t, "the cache is allocated at the first call");
    assert!(offset_slots(&vm, t) > 0, "x.m() caches a prototype slot");
    // The copy above the cache; the original lets go of the cache.
    let h = vm.heap_mut();
    let size = h.cell_size(t);
    let bytes = h.arena().slice(t, size).to_vec();
    let copy = h
        .alloc_cell_init(CellKind::FuncTemplate, size, 8, &bytes[8..])
        .unwrap();
    h.arena_mut().set(t, FuncTemplate::IC, Ref::NULL);
    h.arena_mut().set(f, Object::ELEMENTS, Ref(copy));
    assert!(ic < copy);
    vm.collect().unwrap();
    let (_, t) = template_of(&vm);
    let ic_now = vm
        .heap()
        .arena()
        .get::<Ref>(t, FuncTemplate::IC)
        .get()
        .unwrap();
    assert!(ic_now < ic, "the cache moved down over the garbage");
    assert_eq!(offset_slots(&vm, t), 0);
    vm.heap().verify().unwrap();
    let (_, out) = run_script(vm, "console.log(f(o), f(Object.create(o)));\n");
    assert_eq!(out.log, ["m m"]);
}

#[test]
fn host_memory_failure_is_not_a_js_error() {
    // The arena cannot grow (the host allocator refuses below the cap): the run ends with
    // HostOutOfMemory, the script's catch never runs, and the VM is usable (D1690).
    let src = "let a = [];\n\
               try { for (let i = 0; ; i++) a.push('x'.repeat(1000) + i); }\n\
               catch (e) { console.log('caught', String(e)); }\n";
    let mut vm = vm();
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    crate::heap::arena::FAIL_NEXT_RESERVE.with(|f| f.set(true));
    let st = loop {
        match vm.run(1 << 40) {
            Status::HostCall { call_id, .. } => {
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap()
            }
            st => break st,
        }
    };
    assert_eq!(st, Status::HostOutOfMemory);
    vm.heap().verify().unwrap();
    let (_, out) = run_script(vm, "a = null; console.log('again', [1, 2].length);\n");
    assert_eq!(out.log, ["again 2"]);
}

#[cfg(feature = "embed")]
#[test]
fn fatal_out_of_memory_runs_no_handler() {
    // With `fatal_out_of_memory`, reaching the cap ends the run as host memory exhaustion
    // does: neither the catch nor the finally runs (no console.log at all), and the VM is
    // idle and usable (D3181). Without it the same script catches the InternalError.
    let src = "(function () { let a = [];\n\
               try { for (let i = 0; ; i++) a.push('x'.repeat(1000) + i); }\n\
               catch (e) { console.log('caught', String(e)); }\n\
               finally { console.log('finally'); } })();\n";
    for fatal in [true, false] {
        let mut vm = vm_with(VmOptions {
            memory_limit: 4 << 20,
            exec: ExecOptions {
                fatal_out_of_memory: fatal,
                ..Default::default()
            },
            ..Default::default()
        });
        let f = vm
            .compile(
                src.as_bytes(),
                &crate::compiler::CompileOptions::script("t.js"),
            )
            .unwrap();
        let this = Value::object(vm.global());
        vm.call(f, this, &[]).unwrap();
        let mut log = Vec::new();
        let st = loop {
            match vm.run(1 << 40) {
                Status::HostCall { call_id, .. } => {
                    let info = vm.host_call(call_id).unwrap();
                    let line: Vec<String> = info.args.iter().map(|&v| vm.display(v)).collect();
                    log.push(line.join(" "));
                    vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap()
                }
                st => break st,
            }
        };
        if fatal {
            assert_eq!(st, Status::HostOutOfMemory);
            assert!(log.is_empty(), "{log:?}");
            assert_eq!(vm.state(), RunState::Idle);
            vm.heap().verify().unwrap();
            // The abandoned call's data is garbage now.
            vm.collect().unwrap();
            let (_, out) = run_script(vm, "console.log('again', [1, 2].length);\n");
            assert_eq!(out.log, ["again 2"]);
        } else {
            assert!(matches!(st, Status::Done(_)), "{st:?}");
            assert_eq!(log, ["caught InternalError: out of memory", "finally"]);
        }
    }
}

#[cfg(feature = "embed")]
#[test]
fn reset_to_reuses_the_arena() {
    let (mut vm, _) = run_script(vm(), "globalThis.n = 1;\n");
    let image = vm.image().to_vec();
    // An arena much bigger than the image, so the reset has room for what a VM adds to
    // it (the base's size must not decide the test).
    let (mut vm, _) = run_script(vm, "n = 2; globalThis.big = Array(1 << 17).fill(n);\n");
    let cap = vm.heap().arena().capacity();
    assert!(cap > 2 * image.len(), "{cap}");
    vm.reset_to(&image).unwrap();
    assert_eq!(vm.bytes(), &image[..]);
    assert_eq!(vm.heap().arena().capacity(), cap);
    vm.heap().verify().unwrap();
    let (_, out) = run_script(vm, "console.log(n, typeof big);\n");
    assert_eq!(out.log, ["1 undefined"]);
}

/// A VM moves between threads (an embedder's async tasks): `Send` in every build, the
/// dump feature's caches included (D3183).
#[test]
fn vm_is_send() {
    fn send<T: Send>() {}
    send::<Vm>();
}

#[test]
fn a_refused_to_space_stops_the_run() {
    // A collection whose to-space the host refuses is not skipped: the run stops.
    let mut vm = vm_with(VmOptions {
        exec: ExecOptions {
            gc_stress: true,
            ..Default::default()
        },
        ..Default::default()
    });
    let f = vm
        .compile(
            b"let s = 0; for (let i = 0; i < 100; i++) s += i; console.log(s);",
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    vm.heap_mut().fail_next_gc_reserve = true;
    assert_eq!(vm.run(1 << 40), Status::HostOutOfMemory);
    // The embedder's own collection reports it too.
    vm.heap_mut().fail_next_gc_reserve = true;
    assert_eq!(vm.collect(), Err(HeapError::HostOutOfMemory));
    let (_, out) = run_script(vm, "console.log('fine');\n");
    assert_eq!(out.log, ["fine"]);
}

// ---- The canonical hash (D1730) --------------------------------------------------------------

/// 0083: the shape tree, including the budget's overflow into dictionaries, is a function
/// of the program, not of when collections ran (strong transitions, D1831): the canonical
/// hash is the same with no collections, with one after every slice of 997 or 5000
/// instructions, and across image restores.
#[test]
fn shapes_do_not_depend_on_collection_points() {
    let src = "globalThis.keep = [];\n\
               for (let i = 0; i < 400; i++) {\n\
                 const o = {}; o['f' + (i % 32)] = i;\n\
                 for (let j = 0; j < 12; j++) o['u' + i + '_' + j] = j;\n\
                 if (i % 5 == 0) keep.push(o);\n\
               }\n\
               for (let i = 0; i < 2000; i++) {\n\
                 const p = { a: i, b: i, ['c' + (i % 40)]: i };\n\
                 if (i % 9 == 0) { delete p.b; keep.push(p); }\n\
               }\n\
               globalThis.late = { late: 1, comer: 2 };\n";
    let run = |fuel: u64, collect: bool, suspend: bool| {
        let mut vm = vm();
        let f = vm
            .compile(
                src.as_bytes(),
                &crate::compiler::CompileOptions::script("t.js"),
            )
            .unwrap();
        let this = Value::object(vm.global());
        vm.call(f, this, &[]).unwrap();
        loop {
            match vm.run(fuel) {
                Status::Done(_) => break,
                Status::OutOfFuel => {
                    if collect {
                        vm.collect().unwrap();
                    }
                    if suspend {
                        let opts = vm.options();
                        vm = Vm::from_bytes(vm.image().to_vec(), opts).unwrap();
                    }
                }
                s => panic!("{s:?}"),
            }
        }
        vm.heap().verify().unwrap();
        vm
    };
    let mut plain = run(u64::MAX, false, false);
    // The budget was spent: the late key set is a dictionary.
    let key = plain.heap_mut().atom_str("late").unwrap();
    let late = plain
        .heap()
        .get_own_property(plain.global(), key)
        .and_then(|p| p.value())
        .and_then(|v| v.as_object())
        .unwrap();
    assert_ne!(
        plain.heap().cell_flags(late.offset()) & crate::heap::cells::flags::OBJ_DICTIONARY,
        0
    );
    let shapes = plain
        .heap()
        .arena()
        .get(0, crate::heap::cells::Roots::SHAPES);
    assert_eq!(shapes, crate::heap::shape::SHAPE_BUDGET);
    let h = plain.canonical_hash().unwrap();
    assert_eq!(run(997, true, false).canonical_hash().unwrap(), h);
    assert_eq!(run(5000, true, false).canonical_hash().unwrap(), h);
    assert_eq!(run(20_000, true, true).canonical_hash().unwrap(), h);
}

#[test]
fn canonical_hash_ignores_collection_points() {
    let src = "globalThis.keep = [];\n\
               for (let i = 0; i < 5000; i++) { const o = { i, s: 'x' + i }; if (i % 7 == 0) keep.push(o); }\n\
               const m = new Map(keep.map((o) => [o, o.i]));\n\
               globalThis.n = m.size;\n";
    let hash_at = |fuel: u64, suspend: bool| {
        let (mut vm, _) = finish_script(vm(), src, fuel, suspend);
        let before = vm.bytes().to_vec();
        let h = vm.canonical_hash().unwrap();
        assert!(before == vm.bytes(), "canonical_hash changed the VM");
        vm.collect().unwrap();
        assert_eq!(vm.canonical_hash().unwrap(), h, "a collection changed it");
        h
    };
    let h = hash_at(u64::MAX, false);
    assert_eq!(hash_at(997, true), h);
    assert_eq!(hash_at(1000, false), h);
    // A different state hashes differently.
    let (vm2, _) = finish_script(vm(), "globalThis.n = 1;\n", u64::MAX, false);
    assert_ne!(vm2.canonical_hash().unwrap(), h);
}

#[test]
fn a_failing_allocation_collects_and_retries() {
    failing_allocation_retries(&[2_000_000]);
}

/// Both limits (about 4 s): `cargo test -p djs-core --lib -- --ignored
/// a_failing_allocation_retries_at_both_limits`.
#[test]
#[ignore]
fn a_failing_allocation_retries_at_both_limits() {
    failing_allocation_retries(&[2_000_000, 2_500_000]);
}

fn failing_allocation_retries(limits: &[usize]) {
    // Garbage, then a big array near the cap: the allocation that fails collects and
    // runs again instead of throwing (D1700), at every slice and with restores.
    let src = include_str!("../../../../scripts/determinism/js/oom.js");
    let dots = ".".repeat(40);
    for &limit in limits {
        let plain = run_limited(src, limit, 1 << 40, false).unwrap();
        assert_eq!(plain.log, [dots.as_str()], "{limit}");
        let sliced = run_limited(src, limit, 7919, false).unwrap();
        let restored = run_limited(src, limit, 300_000, true).unwrap();
        assert_eq!(plain, sliced, "{limit}");
        assert_eq!(plain, restored, "{limit}");
    }
}

/// Shape statistics of scripts (0083, the limits): `DJS_SHAPE_STATS=a.js:b.js cargo test
/// -p djs-core --lib shape_stats -- --ignored --nocapture`. Per script: shapes created
/// (the base image's excluded), and the live ordinary objects in shape and dictionary mode
/// with their property counts.
#[test]
#[ignore]
fn shape_stats_report() {
    extern crate std;
    use crate::heap::cells::{Object, PropTable, PropValues, Roots, flags::OBJ_DICTIONARY};
    use crate::heap::{CellKind, ObjectClass};
    let base = vm();
    let base_shapes = base.heap().arena().get(0, Roots::SHAPES);
    let paths = std::env::var("DJS_SHAPE_STATS").unwrap_or_default();
    for path in paths.split(':').filter(|p| !p.is_empty()) {
        // Codemode bodies: an async function body.
        let src = std::fs::read_to_string(path).unwrap();
        let src = format!("globalThis.result = (async () => {{\n{src}\n}})();");
        let (mut vm, out) = finish_script(vm(), &src, 1 << 40, false);
        vm.collect().unwrap();
        let h = vm.heap();
        let a = h.arena();
        let shapes = a.get(0, Roots::SHAPES) - base_shapes;
        let (mut shaped, mut dicts, mut big) = (0u32, 0u32, 0u32);
        let mut counts = std::collections::BTreeMap::<u32, u32>::new();
        for c in h.cells() {
            if c.kind != CellKind::Object
                || ObjectClass::from_u32(a.get(c.offset, Object::CLASS))
                    != Some(ObjectClass::Ordinary)
            {
                continue;
            }
            let p = a.get(c.offset, Object::PROPS).0;
            let dict = h.cell_flags(c.offset) & OBJ_DICTIONARY != 0;
            let n = match p {
                0 => 0,
                p if dict => a.get(p, PropTable::COUNT),
                p => a.get(p, PropValues::LEN),
            };
            if dict {
                dicts += 1;
                if n > 32 {
                    big += 1;
                }
            } else {
                shaped += 1;
            }
            *counts.entry(n.min(40)).or_default() += 1;
        }
        let r: Vec<u32> = h
            .dict_reasons
            .iter()
            .zip(base.heap().dict_reasons)
            .map(|(a, b)| a - b)
            .collect();
        std::println!(
            "{path}: {shapes} shapes created; dictionaries made: {} > 32 keys, {} budget, {} delete, {} attributes, {} index keys, {} builtin; live ordinary objects at the end: {shaped} shape mode, {dicts} dictionaries ({big} with > 32 keys); by key count {counts:?}; result {}",
            r[0],
            r[1],
            r[2],
            r[3],
            r[4],
            r[5],
            out.result.chars().take(60).collect::<String>()
        );
    }
}

/// Bytes per object for some shapes of data (0083): run with
/// `cargo test -p djs-core --lib memory_per_object -- --ignored --nocapture`.
#[test]
#[ignore]
fn memory_per_object_report() {
    extern crate std;
    let n: u32 = 10_000;
    let cases = [
        (
            "literal {a,b,c,d}",
            "globalThis.keep = []; for (let i = 0; i < COUNT; i++) keep.push({ a: i, b: i, c: i, d: i });",
        ),
        (
            "JSON.parse 5-key records",
            "const s = JSON.stringify(Array.from({ length: COUNT }, (_, i) => ({ id: i, name: 'n' + (i % 10), ok: true, score: i / 2, tags: null }))); globalThis.keep = JSON.parse(s);",
        ),
        (
            "class instances (3 fields)",
            "class P { constructor(i) { this.x = i; this.y = i; this.z = i; } } globalThis.keep = []; for (let i = 0; i < COUNT; i++) keep.push(new P(i));",
        ),
        (
            "closures",
            "globalThis.keep = []; for (let i = 0; i < COUNT; i++) keep.push(() => i);",
        ),
        (
            "empty objects",
            "globalThis.keep = []; for (let i = 0; i < COUNT; i++) keep.push({});",
        ),
    ];
    for (name, src) in cases {
        let (mut vm, _) = run_script(vm(), "globalThis.keep = [];");
        vm.collect().unwrap();
        let before = vm.bytes().len();
        let (mut vm, _) = run_script(vm, &src.replace("COUNT", &format!("{n}")));
        vm.collect().unwrap();
        let after = vm.bytes().len();
        std::println!("{name}: {:.1} B/object", (after - before) as f64 / n as f64);
    }
}

// ---- Fuel debt (D1740) -------------------------------------------------------------------

#[test]
fn a_fuel_budget_stops_at_the_same_point_for_any_slice() {
    // Ordinary code and expensive leaf ops (long string searches, splits). A budget of
    // k * B fuel, given as one run or as B/s runs of s fuel, must stop at the same
    // instruction with the same state: the overshoot of each slice carries to the next
    // (before D1740 it was forgiven, so small slices got further).
    let src = "globalThis.s = 'xy'.repeat(20000);\n\
               globalThis.k = 0;\n\
               for (let i = 0; ; i++) {\n\
                 k += s.indexOf('yz', i % 7) + s.lastIndexOf('q') + s.split('y').length;\n\
                 const a = [5, 3, 9, 1, 7, i % 13];\n\
                 a.sort((p, q) => p - q);\n\
                 k += a[2];\n\
               }\n";
    budget_stops_at_the_same_point(src, &ALL_SCHEDULES, 8).unwrap();
}

/// Slices of the budget B (some with an image restore at every stop).
const ALL_SCHEDULES: [(u64, bool); 4] = [(30_030, false), (4620, true), (143, false), (13, false)];

/// Runs `src` (an endless loop) with a budget of k * B fuel for k = 1..`budgets`, given as
/// one run or as B/s runs of s fuel for each `(s, restore)` of `schedules`, and compares
/// the state (canonical hash and backtrace) after each budget.
fn budget_stops_at_the_same_point(
    src: &str,
    schedules: &[(u64, bool)],
    budgets: usize,
) -> Result<(), alloc::string::String> {
    const B: u64 = 60_060;
    let states = |slice: u64, suspend: bool| {
        let mut vm = vm();
        let f = vm
            .compile(
                src.as_bytes(),
                &crate::compiler::CompileOptions::script("t.js"),
            )
            .unwrap();
        let this = Value::object(vm.global());
        vm.call(f, this, &[]).unwrap();
        let mut states = Vec::new();
        for _ in 0..budgets {
            for _ in 0..B / slice {
                assert_eq!(vm.run(slice), Status::OutOfFuel);
                if suspend {
                    let opts = vm.options();
                    vm = Vm::from_bytes(vm.image().to_vec(), opts).unwrap();
                }
            }
            states.push((vm.canonical_hash().unwrap(), vm.backtrace()));
        }
        states
    };
    let reference = states(B, false);
    for &(slice, suspend) in schedules {
        if states(slice, suspend) != reference {
            return Err(alloc::format!("slice {slice}, suspend {suspend}"));
        }
    }
    Ok(())
}

/// The exceptions 0074 found: append buffers and JSON/Array.from depended on the
/// collection at the start of `run` (removed, D1710), and suspended regexps lost their
/// dispatch refund at the end of a slice (now a credit, D1712).
const BUDGET_CASES: [&str; 5] = [
    "globalThis.t = ''; const s = 'ab'.repeat(200); for (let i = 0; ; i++) { t += s.slice(i % 300, i % 300 + 50); }",
    "for (;;) { globalThis.j = JSON.stringify(Array.from({ length: 500 }, (_, i) => ({ i, s: 'x' + i }))); }",
    "globalThis.s = 'ab'.repeat(5000); for (;;) { globalThis.m = /abc|bab?x|q/.exec(s); }",
    "globalThis.s = 'ab'.repeat(5000); for (;;) { globalThis.m = /(?:ab){3}x/.exec(s); }",
    "globalThis.s = 'ab'.repeat(2000); for (;;) { globalThis.m = /(?:a|b)*c/.exec(s); }",
];

fn budget_cases(schedules: &[(u64, bool)], budgets: usize) {
    let mut errs = alloc::vec::Vec::new();
    for (n, src) in BUDGET_CASES.iter().enumerate() {
        if let Err(e) = budget_stops_at_the_same_point(src, schedules, budgets) {
            errs.push(alloc::format!("{n}: {e}"));
        }
    }
    assert!(errs.is_empty(), "{errs:?}");
}

/// A sample (6 s per gate in full): the slice that showed each failure (30030 for the
/// regexps, 143 for the collections) and a restoring schedule, for four budgets.
#[test]
fn budgets_with_collections_and_suspended_regexps() {
    budget_cases(&[(30_030, false), (4620, true), (143, false)], 4);
}

/// Every schedule and eight budgets (`cargo test -p djs-core --lib -- --ignored budgets`).
#[test]
#[ignore]
fn budgets_with_collections_and_suspended_regexps_all_schedules() {
    budget_cases(&ALL_SCHEDULES, 8);
}

#[test]
fn native_builtins_collect_and_retry() {
    // The same through native builtins (D1900): map's result array grows by doubling
    // while its callback makes garbage, so a growth reaches the cap between two
    // collections. Without the retry, rounds failed at these caps ("..E.E.").
    let src = include_str!("../../../../scripts/determinism/js/oom_native.js");
    let dots = ".".repeat(6);
    for limit in [4_800_000usize, 5_200_000] {
        let plain = run_limited(src, limit, 1 << 40, false).unwrap();
        assert_eq!(plain.log, [dots.as_str()], "{limit}");
        let sliced = run_limited(src, limit, 7919, false).unwrap();
        let restored = run_limited(src, limit, 300_000, true).unwrap();
        assert_eq!(plain, sliced, "{limit}");
        assert_eq!(plain, restored, "{limit}");
    }
}

#[test]
fn native_registers_survive_collect_and_retry() {
    // Typed locals of native builtins live in Rust variables (D2000) while their
    // allocations collect and retry at the cap (D1900): the checksums of every round must
    // match the run without a cap, however the run is sliced or restored.
    let src = include_str!("../../../../scripts/determinism/js/oom_native_regs.js");
    let free = run_limited(src, 1 << 30, 1 << 40, false).unwrap();
    assert!(!free.log[0].contains('E'), "{:?}", free.log);
    // At these caps rounds failed ("E") with the retry disabled.
    for limit in [2_700_000usize, 2_800_000] {
        let plain = run_limited(src, limit, 1 << 40, false).unwrap();
        assert_eq!(plain.log, free.log, "{limit}");
        let sliced = run_limited(src, limit, 7919, false).unwrap();
        let restored = run_limited(src, limit, 300_000, true).unwrap();
        assert_eq!(plain, sliced, "{limit}");
        assert_eq!(plain, restored, "{limit}");
    }
}

#[test]
fn fast_entries_decline_at_the_cap() {
    // @fast entries (D1910): slice and join of a large array after garbage. An entry
    // that reaches the cap declines and the builtin's own allocations collect and retry;
    // when entries threw instead, these caps printed `.EEEEEEE`.
    let src = include_str!("../../../../scripts/determinism/js/oom_fast.js");
    let dots = ".".repeat(8);
    for limit in [2_800_000usize, 3_200_000] {
        let plain = run_limited(src, limit, 1 << 40, false).unwrap();
        assert_eq!(plain.log, [dots.as_str()], "{limit}");
        let sliced = run_limited(src, limit, 7919, false).unwrap();
        let restored = run_limited(src, limit, 300_000, true).unwrap();
        assert_eq!(plain, sliced, "{limit}");
        assert_eq!(plain, restored, "{limit}");
    }
}

/// The stops of a run of `src` (a Script) with `opts` and slices of `fuel`, restoring the
/// image at every stop with `suspend`: a hash of the heap image at each stop and at the
/// end, and the console output (0093, D1930).
fn image_trace(src: &str, exec: ExecOptions, fuel: u64, suspend: bool) -> Vec<String> {
    let opts = VmOptions {
        exec,
        ..Default::default()
    };
    let mut vm = vm_with(opts);
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let hash = |b: &[u8]| {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &x in b {
            h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
        }
        h
    };
    let mut trace = Vec::new();
    loop {
        // Fine slices for the first stops, then coarser ones (every stop hashes the image).
        let fuel = if trace.len() > 600 {
            fuel.max(997)
        } else {
            fuel
        };
        let st = vm.run(fuel);
        trace.push(format!("{:?} {:x}", st, hash(vm.bytes())));
        match st {
            Status::OutOfFuel => {}
            Status::HostCall { call_id, .. } => {
                let info = vm.host_call(call_id).unwrap();
                let line: Vec<String> = info.args.iter().map(|&a| vm.display(a)).collect();
                trace.push(line.join(" "));
                vm.resolve_host_call(call_id, Ok(Value::UNDEFINED)).unwrap();
            }
            _ => return trace,
        }
        if suspend {
            let opts = vm.options();
            vm = Vm::from_bytes(vm.image().to_vec(), opts).unwrap();
        }
    }
}

/// The tier-0 loop (D1930) changes nothing observable: at every stop (fuel boundaries
/// inside it, with and without restoring the image), with collections wanted at its
/// back-edges (GC stress, and garbage that makes a collection pending), and with its
/// periodic exit forced at every backward jump, the heap images, the fuel at which the
/// run stops, and the output equal those of the run without it.
#[test]
fn hot_loop_is_invisible() {
    let src = "let sum = 0, s = '', o = { a: 1 }, xs = [];\n\
        function f(n) { let t = 0; for (let i = 0; i < n; i++) { t = (t + i * 3) | 0; if (t % 7 === 3) t--; } return t; }\n\
        for (let r = 0; r < 40; r++) {\n\
          for (let j = 0; j < 25; j++) { sum += j & 5; sum = sum ^ 1; }\n\
          s = 'x' + r; xs.push(s); o.a += f(r);\n\
          let k = 1.5; while (k < 100) { k = k * 2 + 0.25; }\n\
          sum += k > 50 ? 1 : 0; sum += (r === 3) + !s + (s !== 'x3');\n\
        }\n\
        let big = []; for (let i = 0; i < 3000; i++) { big = [big, 'garbage ' + i]; for (let q = 0; q < 3; q++) sum++; }\n\
        class P { constructor(x) { this.x = x; } get g() { return this.x + 1; } m(y) { return this.x * y; } }\n\
        const ps = []; for (let i = 0; i < 200; i++) ps.push(new P(i));\n\
        const add = (a, b) => a + b; let acc = 0;\n\
        for (const p of ps) { acc = add(acc, p.m(2)) + p.x; if (p.x % 50 === 0) acc += p.g; }\n\
        const arr = [1, 2, 3, 4, 5, 6, 7, 8]; for (let i = 0; i < 400; i++) { arr[i & 7] = arr[(i + 1) & 7] + 1; }\n\
        function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } acc += fib(12);\n\
        acc += ps.map((p) => p.x).filter((x) => x & 1).length + [3, 1, 2].sort((a, b) => a - b)[0];\n\
        console.log(acc, arr.join(), 'abc' === 'ab' + 'c', !'');\n\
        let cb = 0; const nums = []; for (let i = 0; i < 300; i++) nums.push((i * 37) % 101);\n\
        nums.forEach((x, i) => { cb += x * (i & 3); });\n\
        nums.forEach(function (x) { cb += this.k * x; }, { k: 2 });\n\
        nums.forEach(function (x, i, a, extra) { if (extra === undefined && a === nums) cb++; });\n\
        const words = nums.map((x) => 'w' + x).filter((w, i) => (i & 1) === 0);\n\
        cb += words.length + nums.reduce((s, x) => s + x, 0) + (nums.find((x) => x > 99) || 0);\n\
        const sorted = nums.slice().sort((a, b) => { const g = [a, b]; return g[0] - g[1]; });\n\
        cb += sorted[0] + sorted[299] + nums.some((x) => x === 100) + nums.every((x) => x >= 0);\n\
        try { nums.forEach((x) => { if (x === 50) throw new Error('stop ' + x); }); } catch (e) { cb += e.message.length; }\n\
        cb += [[1, 2], [3]].map((p) => p.map((x) => x * cb % 7).join('')).join('|').length;\n\
        console.log(cb, words[3], sorted[150]);\n\
        console.log(sum, o.a, xs.length, s);\n\
        class Q extends P { constructor(x) { super(x + 1); this.t = new.target === Q; } }\n\
        class R extends P {} function F(a) { this.a = a; } let qs = 0;\n\
        for (let i = 0; i < 120; i++) { const q = new Q(i), m = new Map(); m.set(i, q);\n\
          qs += q.x + q.t + m.size + new R(i).x + new F(i).a + new Set().size + new Date(i).getTime();\n\
          if (i % 10 === 0) qs += new Error('e' + i).message.length + (new Promise(() => {}) instanceof Promise); }\n\
        console.log(qs);\n";
    const HOT: u32 = crate::interp::run::HOT_BACKEDGES;
    let cases: &[(u64, bool, u32, bool)] = &[
        // (fuel slice, gc stress, backward jumps per run_hot, suspend through images)
        (1 << 40, false, HOT, false),
        (1, false, HOT, false),
        (3, false, HOT, true),
        (97, false, HOT, true),
        (1 << 40, false, 1, false),
        (13, false, 2, true),
        (1 << 40, true, HOT, false),
        (61, true, 1, true),
    ];
    // The 16 runs on threads of their own: one after another they took half a minute,
    // the slowest test of the gate (0183).
    extern crate std;
    let traces: Vec<(Vec<String>, Vec<String>)> = std::thread::scope(|s| {
        let runs: Vec<_> = cases
            .iter()
            .map(|&(fuel, gc_stress, backedges, suspend)| {
                let exec = move |hot_loop| ExecOptions {
                    gc_stress,
                    hot_loop,
                    hot_backedges: backedges,
                    ..Default::default()
                };
                let with = s.spawn(move || image_trace(src, exec(true), fuel, suspend));
                let without = s.spawn(move || image_trace(src, exec(false), fuel, suspend));
                (with, without)
            })
            .collect();
        runs.into_iter()
            .map(|(a, b)| (a.join().unwrap(), b.join().unwrap()))
            .collect()
    });
    for (&(fuel, gc_stress, backedges, _), (with, without)) in cases.iter().zip(traces) {
        assert!(with.len() > 2, "the program ran: {with:?}");
        assert_eq!(
            with, without,
            "fuel {fuel}, gc stress {gc_stress}, back-edges {backedges}"
        );
    }
}

/// 0120: a sealed VM's collections while scripts run are mostly minor (the sealed state is
/// old): the same output under suspension and GC stress, the same canonical hash under
/// suspension, and an old region the script's collections keep (minor) or shrink to its
/// live cells (full).
#[test]
fn sealed_vms_collect_minor_and_stay_deterministic() {
    use crate::heap::cells::Roots;
    let setup = "globalThis.wm = new WeakMap(); globalThis.m = new Map(); \
                 globalThis.held = { list: [] }; Array.prototype.extra = function () { return this.length; };";
    let src = r#"
        const keep = [];
        let sum = 0;
        for (let i = 0; i < 3000; i++) {
            const k = { i };
            wm.set(k, { v: i });              // old table, young keys
            if (i % 50 == 0) { m.set(k, i); keep.push(k); }
            if (i % 1000 == 0) held.list.push([i, "s" + i]);   // old object, young values
            sum += [i, i + 1].extra() + ("x" + i).length;
        }
        Object.prototype.late = { n: keep.length };   // an old prototype gets a young value
        let found = 0;
        for (const k of keep) found += m.get(k) === k.i && wm.get(k).v === k.i ? 1 : 0;
        console.log(sum, found, held.list.length, ({}).late.n, m.size);
    "#;
    let mut base = vm();
    eval(&mut base, setup);
    base.seal().unwrap();
    let sealed = base.heap().old_end();
    assert_eq!(sealed as usize, base.bytes().len());
    let image = base.bytes().to_vec();
    let mut results = Vec::new();
    for (fuel, suspend, gc_stress) in [(1 << 40, false, false), (1, true, false), (7, false, true)]
    {
        let opts = VmOptions {
            exec: ExecOptions {
                gc_stress,
                ..Default::default()
            },
            ..Default::default()
        };
        let vm = Vm::from_bytes(image.clone(), opts).unwrap();
        let (vm, out) = finish_script(vm, src, fuel, suspend);
        vm.heap().verify().unwrap();
        let a = vm.heap().arena();
        let minors = a.get(0, Roots::MINORS);
        let old = vm.heap().old_end();
        assert!(old <= sealed, "{old} {sealed}");
        if !gc_stress {
            // The run collected, at least once in a minor collection.
            assert!(minors > 0 || old < sealed, "{minors} {old}");
        }
        results.push((out, vm.canonical_hash().unwrap()));
    }
    assert_eq!(results[0].0.log, ["19890 60 3 60 60"]);
    assert_eq!(results[0], results[1], "suspension changed the run");
    // GC stress clears dead WeakMap entries at other times: its graph differs.
    assert_eq!(results[0].0, results[2].0, "gc stress changed the run");
}

#[test]
fn reserved_bases_start_with_room() {
    let base = base_image();
    let plain = Vm::from_base(&base, VmOptions::default()).unwrap();
    let big = Vm::from_base_reserved(&base, VmOptions::default(), 4 << 20).unwrap();
    assert!(plain.heap().arena().capacity() < 4 << 20);
    assert!(big.heap().arena().capacity() >= 8 << 20);
    assert_eq!(plain.bytes(), big.bytes());
}

#[test]
fn seal_puts_the_first_grown_cells_last() {
    // D2057: after a seal, the atom entries are the last cell, and a session's first atoms
    // and globals leave everything before the moved cells in place, through a full
    // collection too. A Map iterator alive across the seal still sees later additions.
    let mut v = vm();
    eval(
        &mut v,
        "globalThis.m = new Map([[1, 1]]); globalThis.it = m.keys(); it.next(); return 0",
    );
    // A VM from `Vm::new` has a 1024-slot stack, which a full collection would shrink
    // (D2308), moving what lies after it; a VM from the base starts with 256 slots.
    v.collect().unwrap();
    v.seal().unwrap();
    let a = v.heap().arena();
    let atoms: crate::heap::Ref = a.get(0, crate::heap::cells::Roots::ATOMS);
    let entries: crate::heap::Ref = a.get(atoms.0, crate::heap::cells::AtomTable::ENTRIES);
    assert_eq!(
        entries.0 + v.heap().cell_size(entries.0),
        a.len(),
        "entries last"
    );
    let sealed = v.bytes().to_vec();
    assert_eq!(
        eval(
            &mut v,
            "globalThis.fresh_name_1 = 1; m.set(2, 2); m.set(3, 3); return [...it].join()"
        ),
        "2,3"
    );
    v.collect().unwrap();
    // Nearly every word of the sealed image is where the seal put it. What moves is about
    // 4,100 words whatever the image's size, so the share drops as the image shrinks
    // (D2444 took it from 90.06% to 89.5%).
    let words = |b: &[u8]| b.chunks(8).map(|w| w.to_vec()).collect::<Vec<_>>();
    let (x, y) = (words(&sealed), words(v.bytes()));
    let same = x.iter().zip(&y).filter(|(a, b)| a == b).count();
    assert!(same * 100 > x.len() * 88, "{same} of {} words", x.len());
}

/// The prefix write barrier (D2065): armed, a write into the builtin prefix panics in
/// debug builds; a VM's own cells (globals, the stack, atoms) are above the prefix.
#[test]
#[cfg(debug_assertions)]
fn the_armed_barrier_traps_prefix_writes() {
    extern crate std;
    let mut base = Vm::new(VmOptions::default()).unwrap();
    base.prepare_base_image().unwrap();
    let base = base.bytes().to_vec();
    let armed = VmOptions {
        prefix_barrier: true,
        ..VmOptions::default()
    };
    // Ordinary work, a new global and new atoms: nothing below BASE_END is written.
    let mut vm = Vm::from_base(&base, armed).unwrap();
    assert!(vm.heap().prefix_barrier_armed());
    assert_eq!(
        eval(
            &mut vm,
            "globalThis.g = { fresh_atom_1: [1, 2].map(x => x + 1) }; let s = new Set([g]); \
             return g.fresh_atom_1.join() + Symbol.for('k').toString() + s.size"
        ),
        "2,3Symbol(k)1"
    );
    vm.collect().unwrap();
    vm.collect_canonical().unwrap();
    // Changing a builtin writes the prefix.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        eval(&mut vm, "Array.prototype.extra = 1; return 1")
    }));
    assert!(r.is_err(), "a write into the prefix must panic when armed");
    // Unarmed (the default), the same write is fine.
    let mut vm = Vm::from_base(&base, VmOptions::default()).unwrap();
    assert!(!vm.heap().prefix_barrier_armed());
    assert_eq!(
        eval(&mut vm, "Array.prototype.extra = 1; return [].extra"),
        "1"
    );
}

/// A base image as the build makes it (prefix and tail, D2063).
fn prefixed_base() -> Vec<u8> {
    static BASE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    extern crate std;
    BASE.get_or_init(|| {
        let mut v = Vm::new(VmOptions::default()).unwrap();
        v.prepare_base_image().unwrap();
        v.bytes().to_vec()
    })
    .clone()
}

#[test]
fn touching_builtins_in_any_order_leaves_the_same_image() {
    // D022: builtins are never created or written at run time, so two runs that use
    // different builtins in different orders, and keep nothing, leave identical images
    // and canonical hashes. Armed, the barrier also checks the prefix (D2065).
    let base = prefixed_base();
    let opts = VmOptions {
        prefix_barrier: true,
        ..VmOptions::default()
    };
    let run = |calls: &[&str]| {
        let mut vm = Vm::from_base(&base, opts).unwrap();
        for c in calls {
            let src = alloc::format!("{c}; return undefined");
            assert_eq!(eval(&mut vm, &src), "undefined");
        }
        vm.collect().unwrap();
        (vm.image().to_vec(), vm.canonical_hash().unwrap())
    };
    // The objects builtins create find their shapes in the base (D2067), and the engine's
    // queues are there too. (No identifiers, object literals with keys, group names or
    // classes: their names and shapes come from the script's source. Neither results
    // keyed by the data, as `getOwnPropertyDescriptors(Map.prototype)`.)
    let calls = [
        "'abc'.toUpperCase().split('')",
        "JSON.parse(JSON.stringify([1, [2]]))",
        "new Map([[1, 2]]).forEach(() => 0)",
        "[...new Set([1, 2]).values()].length",
        "Math.max(1, 2) + Number.parseFloat('1.5')",
        "/a+/.exec('caab')",
        "'caab'.match(/a/g)",
        "[...'aab'.matchAll(/(a)/dg)]",
        "Object.getOwnPropertyDescriptor(Math, 'PI')",
        "Object.getOwnPropertyDescriptor(Map.prototype, 'size')",
        "[1, 2].entries().next()",
        "[3, 1, 2].map(() => 0).sort()",
        "Object.keys([1]).concat([1])",
        "Array.from('ab').reverse()",
        "String(Symbol.iterator) + typeof Array.prototype.map",
        "await Promise.allSettled([Promise.resolve(1), Promise.reject(2)])",
        "await Promise.all([1, Promise.resolve(2)])",
        "Promise.withResolvers()",
        "Proxy.revocable({}, {}).revoke()",
        "new AggregateError([], 'x').errors",
        "(function* () { yield 1 })().next()",
        "String.raw`a${1}`",
    ];
    let mut reversed = calls;
    reversed.reverse();
    let (a, ha) = run(&calls);
    let (b, hb) = run(&reversed);
    let (c, hc) = run(&calls[..2]);
    assert_eq!(a, b, "the order of first use changes nothing");
    assert_eq!(ha, hb);
    assert_eq!(a, c, "neither does which builtins were used");
    assert_eq!(ha, hc);
}

/// D2430: the base image stores each flat string once, in the atom table's cell when it
/// is an atom: a builtin's `name` shares the cell of its key.
#[test]
fn base_strings_are_stored_once() {
    use crate::heap::CellKind;
    use crate::heap::cells::{Header, Str8};
    let base = prefixed_base();
    let mut vm = Vm::from_base(&base, VmOptions::default()).unwrap();
    let h = vm.heap();
    let a = h.arena();
    let mut seen = alloc::collections::BTreeMap::new();
    for c in h.cells() {
        if matches!(c.kind, CellKind::Str8 | CellKind::Str16) && c.offset < h.base_end() {
            let key = (
                c.kind as u8,
                a.get::<u32>(c.offset, Str8::LEN),
                a.slice(c.offset + 16, c.size - 16).to_vec(),
            );
            assert_eq!(a.get::<u16>(c.offset, Header::FLAGS), 0);
            if let Some(prev) = seen.insert(key, c.offset) {
                panic!("strings at {prev} and {} are equal", c.offset);
            }
        }
    }
    let h = vm.heap_mut();
    let map = h.atom_str("map").unwrap();
    let key = h.atom_string(map).unwrap();
    let array = h.atom_str("Array").unwrap();
    let prototype = h.atom_str("prototype").unwrap();
    let name = h.atom_str("name").unwrap();
    let g = vm.global();
    let h = vm.heap();
    let get = |o: crate::value::Obj, k| h.get_own_property(o, k).unwrap().value().unwrap();
    let proto = get(get(g, array).as_object().unwrap(), prototype);
    let f = get(proto.as_object().unwrap(), map);
    assert_eq!(get(f.as_object().unwrap(), name).as_string(), Some(key));
}

/// D2431: in a VM from the base, the base's atoms are in the builtin prefix and the VM's
/// own go to small arrays in the tail; interning finds both and a session image carries
/// only the VM's own.
#[test]
fn the_atom_table_has_two_levels() {
    use crate::heap::Ref;
    use crate::heap::cells::{AtomTable, Roots};
    let base = prefixed_base();
    let mut vm = Vm::from_base(&base, VmOptions::default()).unwrap();
    let h = vm.heap();
    let a = h.arena();
    let table: Ref = a.get(0, Roots::ATOMS);
    let count: u32 = a.get(table.0, AtomTable::BASE_COUNT);
    let field = |f| a.get::<Ref>(table.0, f).0;
    assert!(count > 400, "{count} base atoms");
    for f in [AtomTable::BASE_ENTRIES, AtomTable::BASE_INDEX] {
        assert!(field(f) != 0 && field(f) < h.base_end());
    }
    for f in [AtomTable::ENTRIES, AtomTable::INDEX] {
        assert!(field(f) >= h.base_end());
    }
    assert!(table.0 >= h.base_end());
    let before = h.atom_count();
    assert_eq!(before, count);
    let h = vm.heap_mut();
    let map = h.atom_str("map").unwrap();
    assert!(map.id() < count);
    let fresh: alloc::vec::Vec<_> = (0..50)
        .map(|i| h.atom_str(&alloc::format!("fresh_atom_{i}")).unwrap())
        .collect();
    assert_eq!(fresh[0].id(), count);
    assert_eq!(h.atom_count(), count + 50);
    // Found again (past a regrowth of the VM's arrays), with their strings.
    for (i, &f) in fresh.iter().enumerate() {
        assert_eq!(h.atom_str(&alloc::format!("fresh_atom_{i}")).unwrap(), f);
        assert_eq!(h.atom_name(f), alloc::format!("fresh_atom_{i}"));
    }
    assert_eq!(h.atom_str("map").unwrap(), map);
    assert_eq!(h.atom_name(map), "map");
    // The VM's own part of the image holds its atoms, not the base's 15 KB of entries.
    vm.collect().unwrap();
    let image = vm.image().to_vec();
    let own = image.len() - vm.heap().base_end() as usize;
    assert!(own < 16 * 1024, "{own} bytes");
    let mut back = Vm::from_bytes(image, VmOptions::default()).unwrap();
    let f = back.heap_mut().atom_str("fresh_atom_7").unwrap();
    assert_eq!(f, fresh[7]);
}

/// D2308: images hold nothing above the stack pointer, equal states give equal images
/// however the run was sliced, and the embedder's collections shrink a stack that grew.
#[test]
fn images_have_nothing_above_sp() {
    use crate::heap::cells::{Interp, Roots};
    let src = "function deep(n) { return n === 0 ? [1, 2, 3].map((x) => x * 2).length : 1 + deep(n - 1); }\n\
               let t = 0; for (let i = 0; i < 40; i++) t += deep(300) + JSON.stringify({ i }).length; t";
    // The dead slots above `sp` of an image, and its stack's capacity.
    let dead = |image: &[u8]| -> (u32, u32) {
        let h = Heap::from_bytes(image.to_vec(), crate::heap::DEFAULT_LIMIT).unwrap();
        let sp: u32 = h.arena().get(state::interp(&h), Interp::SP);
        let s: crate::heap::Ref = h.arena().get(0, Roots::STACK);
        let cap = h.value_array_capacity(s.0);
        let n = (sp..cap)
            .filter(|&k| h.value_array_get(s.0, k).to_bits() != 0)
            .count() as u32;
        (n, cap)
    };
    let start = |vm: &mut Vm| {
        let f = vm
            .compile(
                src.as_bytes(),
                &crate::compiler::CompileOptions::script("t.js"),
            )
            .unwrap();
        let this = Value::object(vm.global());
        vm.call(f, this, &[]).unwrap();
    };
    let mut b = Vm::new(VmOptions::default()).unwrap();
    b.prepare_base_image().unwrap();
    let base = b.bytes().to_vec();
    let vm = || Vm::from_base(&base, VmOptions::default()).unwrap();
    // Uninterrupted.
    let mut whole = vm();
    start(&mut whole);
    assert!(matches!(whole.run(u64::MAX), Status::Done(_)));
    assert!(
        dead(whole.bytes()).0 > 0,
        "the test must leave dead values (raw arena)"
    );
    assert_eq!(dead(whole.image()).0, 0);
    // Sliced, restored from its image at every stop, checked there.
    let mut sliced = vm();
    start(&mut sliced);
    let mut stops = 0;
    loop {
        match sliced.run(997) {
            Status::Done(_) => break,
            Status::OutOfFuel => {
                stops += 1;
                assert_eq!(dead(sliced.image()).0, 0);
                let o = sliced.options();
                sliced = Vm::from_bytes(sliced.image().to_vec(), o).unwrap();
            }
            s => panic!("{s:?}"),
        }
    }
    assert!(stops > 10, "{stops} stops");
    let (x, y) = (whole.image().to_vec(), sliced.image().to_vec());
    let diff: Vec<usize> = (0..x.len().min(y.len()))
        .filter(|&i| x[i] != y[i])
        .collect();
    assert!(
        x.len() == y.len() && diff.is_empty(),
        "{} vs {} bytes, {} differ, first {:?}",
        x.len(),
        y.len(),
        diff.len(),
        &diff[..diff.len().min(8)]
    );
    assert_eq!(
        whole.canonical_hash().unwrap(),
        sliced.canonical_hash().unwrap()
    );
    // The recursion grew the stack; the embedder's collection shrinks it again.
    let grown = dead(whole.image()).1;
    assert!(grown >= 1024, "{grown}");
    whole.collect().unwrap();
    assert_eq!(dead(whole.image()), (0, crate::heap::IMAGE_STACK));
    // And the VM runs on: a deeper recursion grows it again.
    let f = whole
        .compile(
            b"deep(900)",
            &crate::compiler::CompileOptions::script("u.js"),
        )
        .unwrap();
    let this = Value::object(whole.global());
    whole.call(f, this, &[]).unwrap();
    assert!(matches!(whole.run(u64::MAX), Status::Done(_)));
    assert!(dead(whole.image()).1 >= 2048);
}

/// D2433: the keyed hash comes from the seed. The same seed gives the same image, another
/// seed the same results in another layout, and reseeding mid-session keeps Maps, Sets
/// and atoms working, in insertion order.
#[test]
fn the_hash_key_follows_the_seed() {
    let base = prefixed_base();
    let script = "globalThis.m = new Map(); globalThis.o = {}; \
                  for (let i = 0; i < 200; i++) { m.set('k' + i, i); m.set(i * 1.5, -i); o['p' + i] = i; } \
                  globalThis.s = new Set(['x', 'y', 'z']); \
                  return [m.get('k7'), m.get(15), o.p9, [...s].join(''), [...m.keys()].slice(0, 4).join()].join()";
    let run = |seed: u64| {
        let opts = VmOptions {
            random_seed: seed,
            ..VmOptions::default()
        };
        let mut vm = Vm::from_base(&base, opts).unwrap();
        let out = eval(&mut vm, script);
        vm.collect().unwrap();
        (vm, out)
    };
    let (mut a, out_a) = run(7);
    let (b, out_b) = run(7);
    let (c, out_c) = run(8);
    assert_eq!(out_a, "7,-10,9,xyz,k0,0,k1,1.5");
    assert_eq!((&out_a, &out_b), (&out_c, &out_c));
    assert_eq!(a.bytes(), b.bytes(), "same seed, same image");
    assert_ne!(a.bytes(), c.bytes(), "another seed, another layout");
    assert_ne!(a.heap().hash_key(), c.heap().hash_key());
    a.set_random_seed(12345);
    assert_eq!(
        eval(
            &mut a,
            "return [m.get('k199'), m.get(298.5), o.p150, s.has('y'), [...m.keys()].slice(0, 4).join(), \
             ('p' + 42) in o].join()"
        ),
        "199,-199,150,true,k0,0,k1,1.5,true"
    );
    a.collect().unwrap();
    a.heap().verify().unwrap();
}

#[test]
fn outcome_while_idle() {
    // 0174: the entry call's value is readable when `run` stops `Idle` on a pending call.
    let mut vm = vm_with(VmOptions::default());
    let tool = vm.new_async_host_function(TOOL, "tool").unwrap();
    vm.define_global("tool", tool).unwrap();
    assert_eq!(vm.outcome(), None);
    let opts = crate::compiler::CompileOptions::script("t.js");
    let f = vm.compile(b"tool(1); 41 + 1", &opts).unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    assert_eq!(vm.run(u64::MAX), Status::Idle);
    assert_eq!(vm.outcome(), Some(Ok(Value::int(42))));
    let f = vm.compile(b"throw 7", &opts).unwrap();
    vm.call(f, this, &[]).unwrap();
    assert_eq!(vm.outcome(), None);
    // An exception is `Threw` at once, even with a call pending.
    assert_eq!(vm.run(u64::MAX), Status::Threw(Value::int(7)));
    assert_eq!(vm.outcome(), Some(Err(Value::int(7))));
}

/// `Vm::open_host_call` (D2890): a plain dump's `resume(id, name, args)` hook opens the
/// call under the dump's id, returns its promise, and moves the id counter past it.
#[test]
fn open_host_call_keeps_the_dump_s_id() {
    const RESUME: u32 = 21;
    let mut vm = vm_with(VmOptions::default());
    let tool = vm.new_async_host_function(TOOL, "tool").unwrap();
    vm.define_global("tool", tool).unwrap();
    let resume = vm.new_host_function(RESUME, "resume").unwrap();
    vm.define_global("resume", resume).unwrap();
    let src = "const p = resume(2, 'tool', ['a', 1]); const q = resume(7, 'tool', []);\n\
               let bad; try { resume(7, 'tool', []); } catch (e) { bad = e.message; }\n\
               const r = tool('new');\n\
               globalThis.out = []; p.then(v => out.push(v)); q.then(v => out.push(v));\n\
               bad";
    let f = vm
        .compile(
            src.as_bytes(),
            &crate::compiler::CompileOptions::script("t.js"),
        )
        .unwrap();
    let this = Value::object(vm.global());
    vm.call(f, this, &[]).unwrap();
    let status = loop {
        match vm.run(u64::MAX) {
            Status::HostCall { call_id, fn_id } => {
                assert_eq!(fn_id, RESUME);
                if let Err(m) = vm.open_host_call(call_id, TOOL) {
                    let e = vm.new_error(ErrorKind::Error, m).unwrap();
                    vm.resolve_host_call(call_id, Err(e)).unwrap();
                }
            }
            s => break s,
        }
    };
    assert_eq!(status, Status::Idle);
    assert_eq!(
        vm.display(vm.outcome().unwrap().unwrap()),
        "resume: the call is open already"
    );
    // The opened calls are the host's already; the new one comes after them (the
    // failed resume, a sync call, took 8).
    assert_eq!(vm.new_host_calls(), [9]);
    assert_eq!(vm.pending_host_calls(), [2, 7, 9]);
    let info = vm.host_call(2).unwrap();
    assert!(info.is_async && info.fn_id == TOOL);
    let args: Vec<String> = info.args.iter().map(|&a| vm.display(a)).collect();
    assert_eq!(args, ["a", "1"]);
    assert!(vm.host_call(7).unwrap().args.is_empty());
    let s = vm.heap_mut().alloc_str("A").unwrap();
    vm.resolve_host_call(2, Ok(Value::string(s))).unwrap();
    vm.resolve_host_call(7, Ok(Value::ZERO)).unwrap();
    assert_eq!(vm.run(u64::MAX), Status::Idle);
    // (Call 9 stays open: the VM is idle, and the Script's outcome is its value.)
    let f = vm
        .compile(
            b"out.join()",
            &crate::compiler::CompileOptions::script("u.js"),
        )
        .unwrap();
    vm.call(f, this, &[]).unwrap();
    assert_eq!(vm.run(u64::MAX), Status::Idle);
    assert_eq!(vm.display(vm.outcome().unwrap().unwrap()), "A,0");
    vm.heap().verify().unwrap();
}
