use nanocode_macro_support as syntax;
use nanocode_macro_support::ToTokens;
use std::cell::RefCell;
use std::fmt::Display;
use std::thread;

#[derive(Default)]
pub struct Ctxt {
    errors: RefCell<Option<Vec<syntax::Error>>>,
}

impl Ctxt {
    pub const fn new() -> Self {
        Self {
            errors: RefCell::new(Some(Vec::new())),
        }
    }

    pub fn error_spanned_by<A: ToTokens, T: Display>(&self, obj: A, msg: T) {
        self.errors
            .borrow_mut()
            .as_mut()
            .unwrap()
            .push(syntax::Error::new_spanned(obj.into_token_stream(), msg));
    }

    pub fn syn_error(&self, err: syntax::Error) {
        self.errors.borrow_mut().as_mut().unwrap().push(err);
    }

    pub fn check(self) -> syntax::Result<()> {
        let mut errors = self.errors.borrow_mut().take().unwrap().into_iter();

        let mut combined = match errors.next() {
            Some(first) => first,
            None => return Ok(()),
        };

        for rest in errors {
            combined.combine(rest);
        }

        Err(combined)
    }
}

impl Drop for Ctxt {
    fn drop(&mut self) {
        if !thread::panicking() && self.errors.borrow().is_some() {
            panic!("forgot to check for errors");
        }
    }
}
