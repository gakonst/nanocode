use std::path::Path;

use super::gitignore::{Gitignore, GitignoreBuilder};
use super::{Error, Match};

#[derive(Clone, Debug)]
pub struct Override(Gitignore);

impl Override {
    pub fn empty() -> Override {
        Override(Gitignore::empty())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn matched<P: AsRef<Path>>(&self, path: P, is_dir: bool) -> Match<()> {
        if self.is_empty() {
            return Match::None;
        }
        let found = self.0.matched(path, is_dir).invert();
        if found.is_none() && self.0.num_ignores() > 0 && !is_dir {
            return Match::Ignore(());
        }
        found
    }
}

#[derive(Clone, Debug)]
pub struct OverrideBuilder {
    builder: GitignoreBuilder,
}

impl OverrideBuilder {
    pub fn new<P: AsRef<Path>>(path: P) -> OverrideBuilder {
        let mut builder = GitignoreBuilder::new(path);
        builder.allow_unclosed_class(false);
        OverrideBuilder { builder }
    }

    pub fn build(&self) -> Override {
        Override(self.builder.build())
    }

    pub fn add(&mut self, glob: &str) -> Result<&mut OverrideBuilder, Error> {
        self.builder.add_line(glob)?;
        Ok(self)
    }

    pub fn case_insensitive(&mut self, yes: bool) -> &mut OverrideBuilder {
        self.builder.case_insensitive(yes);
        self
    }
}
