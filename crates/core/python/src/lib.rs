mod host;
mod imports;
mod psh;

use std::{collections::BTreeSet, io, rc::Rc, time::Duration};

use rustpython_vm::{
    Interpreter, PyObjectRef, PyResult, Settings, VirtualMachine,
    builtins::{PyBaseExceptionRef, PyDictRef},
    function::IntoFuncArgs,
    scope::Scope,
};

pub use psh::UTILITY;

const _: [&str; 2] = [include_str!("posix.py"), include_str!("boot.py")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub kind: Kind,
    pub size: u64,
    pub mode: u32,
    pub mtime_ns: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    Replace,
    Append,
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remove {
    File,
    Dir,
}

pub trait Files {
    fn stat(&self, path: &[u8]) -> io::Result<Stat>;
    fn read(&self, path: &[u8], limit: u64) -> io::Result<Vec<u8>>;
    fn write(&self, path: &[u8], data: &[u8], how: Write) -> io::Result<()>;
    fn list(&self, path: &[u8]) -> io::Result<Vec<Vec<u8>>>;
    fn mkdir(&self, path: &[u8], mode: u32) -> io::Result<()>;
    fn remove(&self, path: &[u8], what: Remove) -> io::Result<()>;
    fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub output: u64,
    pub file: u64,
    pub sleep: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    Output,
    Sleep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Package,
    Machine,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub module: String,
    pub why: Why,
    pub error: String,
}

impl std::fmt::Display for Missing {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lacks = match self.why {
            Why::Package => "Python packages, only the standard library",
            Why::Machine => "processes, sockets or native extensions for Python",
        };
        write!(
            formatter,
            "python3: {}: {} (this simulated shell has no {lacks})",
            self.module, self.error
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    Exited(i32),
    Needs(Vec<Missing>),
    Refused(Limit),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub written: BTreeSet<Vec<u8>>,
    pub removed: BTreeSet<Vec<u8>>,
    pub made: BTreeSet<Vec<u8>>,
    pub moved: Vec<(Vec<u8>, Vec<u8>)>,
}

pub struct Run {
    pub argv: Vec<String>,
    pub stdin: Vec<u8>,
    pub cwd: Vec<u8>,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub limits: Limits,
}

#[derive(Debug)]
pub struct Ran {
    pub ended: Ended,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub changes: Changes,
}

enum Program {
    Command(String),
    Module(String),
    Stdin,
    File(String),
    Version,
}

struct Invocation {
    program: Program,
    argv: Vec<String>,
}

fn invocation(args: &[String]) -> Result<Invocation, String> {
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let program = match arg.as_str() {
            "-c" => {
                let code = rest.next().ok_or("Argument expected for the -c option")?;
                Program::Command(code.clone())
            }
            "-m" => {
                let module = rest.next().ok_or("Argument expected for the -m option")?;
                Program::Module(module.clone())
            }
            "-" => Program::Stdin,
            "-V" | "--version" => Program::Version,
            "-W" | "-X" => {
                rest.next();
                continue;
            }
            "-u" | "-B" | "-E" | "-s" | "-S" | "-I" | "-O" | "-OO" | "-q" | "-b" | "-P" => continue,
            option if option.starts_with('-') => return Err(format!("Unknown option: {option}")),
            path => Program::File(path.to_owned()),
        };
        let first = match &program {
            Program::Command(_) => "-c",
            Program::Module(module) => module,
            Program::Stdin | Program::File(_) | Program::Version => arg,
        };
        let argv = std::iter::once(first.to_owned())
            .chain(rest.cloned())
            .collect();
        return Ok(Invocation { program, argv });
    }
    Ok(Invocation {
        program: Program::Stdin,
        argv: vec![String::new()],
    })
}

enum Prepared {
    Source(String, String),
    Module(String),
    Exited(i32),
}

pub struct Python {
    interpreter: Option<Interpreter>,
    boot: PyDictRef,
}

impl Drop for Python {
    fn drop(&mut self) {
        if let Some(interpreter) = self.interpreter.take() {
            interpreter.enter(|_| self.boot.clear());
            interpreter.finalize(None);
        }
    }
}

impl Python {
    pub fn new() -> io::Result<Self> {
        let mut seed = [0; 4];
        nanocode_platform::fill_random(&mut seed)?;
        let mut settings = Settings::default();
        settings.install_signal_handlers = false;
        settings.import_site = false;
        settings.user_site_directory = false;
        settings.write_bytecode = false;
        settings.ignore_environment = true;
        settings.isolated = true;
        settings.hash_seed = Some(u32::from_le_bytes(seed));
        let builder = Interpreter::builder(settings);
        let defs = rustpython_stdlib::stdlib_module_defs(&builder.ctx);
        let host = host::module_def(&builder.ctx);
        let interpreter = builder
            .add_native_modules(&defs)
            .add_native_module(host)
            .add_frozen_modules(rustpython_pylib::FROZEN_STDLIB)
            .build();
        let boot = interpreter.enter(|vm| vm.ctx.new_dict());
        Ok(Self {
            interpreter: Some(interpreter),
            boot,
        })
    }

    pub fn run(&self, run: Run, files: Rc<dyn Files>) -> Ran {
        let refused = |message: String| Ran {
            ended: Ended::Exited(2),
            stdout: Vec::new(),
            stderr: format!("python3: {message}\n").into_bytes(),
            changes: Changes::default(),
        };
        if !run.cwd.starts_with(b"/") {
            return refused("the working directory is not an absolute path".to_owned());
        }
        let invocation = match invocation(&run.argv) {
            Ok(invocation) => invocation,
            Err(message) => return refused(message),
        };
        let entered = host::enter(host::State::new(files, run));
        let ended = self
            .interpreter
            .as_ref()
            .expect("the interpreter lives as long as its owner")
            .enter(|vm| self.execute(vm, &invocation));
        let state = entered.leave();
        Ran {
            ended: state.refused.map_or(ended, Ended::Refused),
            stdout: state.stdout,
            stderr: state.stderr,
            changes: state.changes,
        }
    }

    fn execute(&self, vm: &VirtualMachine, invocation: &Invocation) -> Ended {
        let prepared = match self.prepare(vm, invocation) {
            Ok(prepared) => prepared,
            Err(error) => {
                let mut text = String::new();
                let _ = vm.write_exception(&mut text, &error);
                host::print(&text);
                return Ended::Exited(1);
            }
        };
        let (source, name) = match prepared {
            Prepared::Source(source, name) => (source, name),
            Prepared::Module(module) => {
                let missing = self.missing(vm, "_absent", vm.ctx.new_str(module.as_str()).into());
                if !missing.is_empty() {
                    return Ended::Needs(missing);
                }
                let result = vm
                    .new_scope_with_main()
                    .and_then(|_| vm.run_module(&module));
                return Ended::Exited(self.exit(vm, result.err()));
            }
            Prepared::Exited(code) => return Ended::Exited(code),
        };
        let names = imports::top_level(&source)
            .into_iter()
            .map(|name| vm.ctx.new_str(name).into())
            .collect();
        let missing = self.missing(vm, "_missing", vm.ctx.new_list(names).into());
        if !missing.is_empty() {
            return Ended::Needs(missing);
        }
        let result = vm.new_scope_with_main().and_then(|scope| {
            if let Program::File(path) = &invocation.program {
                let path = vm.ctx.new_str(path.as_str());
                scope.globals.set_item("__file__", path.into(), vm)?;
            }
            vm.run_string(scope, &source, &name)
        });
        Ended::Exited(self.exit(vm, result.err()))
    }

    fn prepare(&self, vm: &VirtualMachine, invocation: &Invocation) -> PyResult<Prepared> {
        if self.boot.is_empty() {
            let modules = vm.sys_module.get_attr("modules", vm)?;
            let posix = vm.new_module("posix", vm.ctx.new_dict(), None);
            modules.set_item("posix", posix.clone().into(), vm)?;
            #[cfg(target_family = "wasm")]
            modules.set_item("_datetime", vm.ctx.none(), vm)?;
            let code = rustpython_vm::py_compile!(file = "posix.py", module_name = "posix");
            let scope = Scope::with_builtins(None, posix.dict(), vm);
            vm.run_code_obj(vm.ctx.new_code(code), scope)?;
            let code = rustpython_vm::py_compile!(file = "boot.py", module_name = "python");
            let scope = Scope::with_builtins(None, self.boot.clone(), vm);
            vm.run_code_obj(vm.ctx.new_code(code), scope)?;
        }
        let first = match &invocation.program {
            Program::File(path) => path.rfind('/').map_or(".", |cut| &path[..cut.max(1)]),
            _ => "",
        };
        let argv: Vec<PyObjectRef> = invocation
            .argv
            .iter()
            .map(|arg| vm.ctx.new_str(arg.as_str()).into())
            .collect();
        let first = vm.ctx.new_str(first);
        self.call(vm, "_start", (vm.ctx.new_list(argv), first))?;
        let decoded = |source: Vec<u8>, name: &str| match String::from_utf8(source) {
            Ok(source) => Prepared::Source(source, name.to_owned()),
            Err(_) => {
                host::print(&format!("python3: {name} is not UTF-8\n"));
                Prepared::Exited(1)
            }
        };
        Ok(match &invocation.program {
            Program::Command(code) => Prepared::Source(code.clone(), "<string>".to_owned()),
            Program::Stdin => decoded(host::input(), "<stdin>"),
            Program::File(path) => match host::source(path) {
                Ok(source) => decoded(source, path),
                Err(error) => {
                    host::print(&format!(
                        "python3: can't open file {path:?}: {}\n",
                        host::text(&error)
                    ));
                    Prepared::Exited(2)
                }
            },
            Program::Module(module) => Prepared::Module(module.clone()),
            Program::Version => {
                let version = vm.sys_module.get_attr("version", vm)?.str(vm)?.to_string();
                let number = version.split(' ').next().unwrap_or_default();
                host::say(&format!("Python {number}\n"));
                Prepared::Exited(0)
            }
        })
    }

    fn call(&self, vm: &VirtualMachine, name: &str, args: impl IntoFuncArgs) -> PyResult {
        self.boot.get_item(name, vm)?.call(args, vm)
    }

    fn missing(&self, vm: &VirtualMachine, check: &str, of: PyObjectRef) -> Vec<Missing> {
        let found = self
            .call(vm, check, (of,))
            .and_then(|list| list.try_to_value::<Vec<String>>(vm));
        match found {
            Ok(found) => found
                .chunks_exact(3)
                .map(|found| Missing {
                    module: found[0].clone(),
                    why: if found[1] == "standard" {
                        Why::Machine
                    } else {
                        Why::Package
                    },
                    error: found[2].clone(),
                })
                .collect(),
            Err(error) => {
                vm.print_exception(&error);
                Vec::new()
            }
        }
    }

    fn exit(&self, vm: &VirtualMachine, error: Option<PyBaseExceptionRef>) -> i32 {
        let code = error.map_or(0, |error| vm.handle_exit_exception(error));
        if let Err(error) = self.call(vm, "_finish", ()) {
            vm.print_exception(&error);
        }
        code.cast_signed()
    }
}

pub fn run(run: Run, files: Rc<dyn Files>) -> io::Result<Ran> {
    Ok(Python::new()?.run(run, files))
}
