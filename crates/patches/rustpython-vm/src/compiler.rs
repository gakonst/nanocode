#[cfg(all(not(feature = "compiler"), feature = "parser", feature = "codegen",))]
compile_error!("Use --features=compiler to enable both parser and codegen");

#[cfg(feature = "codegen")]
pub use rustpython_codegen::CompileOpts;

cfg_select! {
    feature = "compiler" => {
        pub use rustpython_compiler::*;
    }
    _ => {
        pub use ruff_python_parser as parser;

        pub use rustpython_compiler_core::Mode;
        pub use rustpython_compiler_core as core;
    }
}

#[cfg(not(feature = "compiler"))]
#[derive(Debug)]
pub enum CompileErrorType {
    #[cfg(feature = "codegen")]
    Codegen(super::codegen::error::CodegenErrorType),
    #[cfg(feature = "parser")]
    Parse(super::parser::ParseErrorType),
}

#[cfg(all(not(feature = "compiler"), feature = "codegen"))]
impl From<super::codegen::error::CodegenErrorType> for CompileErrorType {
    fn from(error: super::codegen::error::CodegenErrorType) -> Self {
        Self::Codegen(error)
    }
}

#[cfg(all(not(feature = "compiler"), feature = "parser"))]
impl From<super::parser::ParseErrorType> for CompileErrorType {
    fn from(error: super::parser::ParseErrorType) -> Self {
        Self::Parse(error)
    }
}

#[cfg(not(feature = "compiler"))]
impl core::fmt::Display for CompileErrorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            #[cfg(feature = "codegen")]
            Self::Codegen(ref error) => core::fmt::Display::fmt(error, f),
            #[cfg(feature = "parser")]
            Self::Parse(ref error) => core::fmt::Display::fmt(error, f),
        }
    }
}

#[cfg(not(feature = "compiler"))]
impl std::error::Error for CompileErrorType {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            #[cfg(feature = "codegen")]
            Self::Codegen(ref error) => std::error::Error::source(error),
            #[cfg(feature = "parser")]
            Self::Parse(ref error) => std::error::Error::source(error),
        }
    }
}

#[cfg(not(feature = "compiler"))]
#[derive(Debug)]
pub enum CompileError {
    #[cfg(feature = "codegen")]
    Codegen(super::codegen::error::CodegenError),
    #[cfg(feature = "parser")]
    Parse(super::parser::ParseError),
}

#[cfg(all(not(feature = "compiler"), feature = "codegen"))]
impl From<super::codegen::error::CodegenError> for CompileError {
    fn from(error: super::codegen::error::CodegenError) -> Self {
        Self::Codegen(error)
    }
}

#[cfg(all(not(feature = "compiler"), feature = "parser"))]
impl From<super::parser::ParseError> for CompileError {
    fn from(error: super::parser::ParseError) -> Self {
        Self::Parse(error)
    }
}

#[cfg(not(feature = "compiler"))]
impl core::fmt::Display for CompileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            #[cfg(feature = "codegen")]
            Self::Codegen(ref error) => core::fmt::Display::fmt(error, f),
            #[cfg(feature = "parser")]
            Self::Parse(ref error) => core::fmt::Display::fmt(error, f),
        }
    }
}

#[cfg(not(feature = "compiler"))]
impl std::error::Error for CompileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            #[cfg(feature = "codegen")]
            Self::Codegen(ref error) => std::error::Error::source(error),
            #[cfg(feature = "parser")]
            Self::Parse(ref error) => std::error::Error::source(error),
        }
    }
}

#[cfg(any(feature = "parser", feature = "codegen"))]
impl crate::convert::ToPyException for (CompileError, Option<&str>) {
    fn to_pyexception(&self, vm: &crate::VirtualMachine) -> crate::builtins::PyBaseExceptionRef {
        vm.new_syntax_error(&self.0, self.1)
    }
}

#[cfg(any(feature = "parser", feature = "codegen"))]
impl crate::convert::ToPyException for (CompileError, Option<&str>, bool) {
    fn to_pyexception(&self, vm: &crate::VirtualMachine) -> crate::builtins::PyBaseExceptionRef {
        vm.new_syntax_error_maybe_incomplete(&self.0, self.1, self.2)
    }
}
