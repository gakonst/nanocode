use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::{Error, Match, PartialErrors, is_file_name, strip_prefix};
use crate::search::glob::{Candidate, GlobBuilder, GlobSet, GlobSetBuilder};

#[derive(Clone, Copy, Debug)]
struct Rule {
    whitelist: bool,
    only_dir: bool,
}

#[derive(Clone, Debug)]
pub struct Gitignore {
    set: GlobSet,
    root: PathBuf,
    rules: Vec<Rule>,
}

impl Gitignore {
    pub fn empty() -> Gitignore {
        Gitignore {
            set: GlobSet::empty(),
            root: PathBuf::new(),
            rules: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    pub fn num_ignores(&self) -> usize {
        self.rules.iter().filter(|rule| !rule.whitelist).count()
    }

    pub fn matched<P: AsRef<Path>>(&self, path: P, is_dir: bool) -> Match<()> {
        if self.is_empty() {
            return Match::None;
        }
        let path = self.strip(path.as_ref());
        let candidate = Candidate::new(path);
        let mut best: Option<usize> = None;
        self.set.visit_matches(&candidate, &mut |index| {
            let rule = self.rules[index];
            if (!rule.only_dir || is_dir) && best.is_none_or(|found| index > found) {
                best = Some(index);
            }
        });
        match best {
            None => Match::None,
            Some(index) if self.rules[index].whitelist => Match::Whitelist(()),
            Some(_) => Match::Ignore(()),
        }
    }

    fn strip<'a>(&self, path: &'a Path) -> &'a Path {
        let mut path = strip_prefix(Path::new("./"), path).unwrap_or(path);
        if self.root != Path::new(".")
            && !is_file_name(path)
            && let Some(rest) = strip_prefix(&self.root, path)
        {
            path = strip_prefix(Path::new("/"), rest).unwrap_or(rest);
        }
        path
    }
}

#[derive(Clone, Debug)]
pub struct GitignoreBuilder {
    builder: GlobSetBuilder,
    root: PathBuf,
    rules: Vec<Rule>,
    case_insensitive: bool,
    allow_unclosed_class: bool,
}

impl GitignoreBuilder {
    pub fn new<P: AsRef<Path>>(root: P) -> GitignoreBuilder {
        let root = root.as_ref();
        GitignoreBuilder {
            builder: GlobSetBuilder::new(),
            root: strip_prefix(Path::new("./"), root)
                .unwrap_or(root)
                .to_path_buf(),
            rules: Vec::new(),
            case_insensitive: false,
            allow_unclosed_class: true,
        }
    }

    pub fn build(&self) -> Gitignore {
        if self.rules.is_empty() {
            return Gitignore::empty();
        }
        Gitignore {
            set: self.builder.build(),
            root: self.root.clone(),
            rules: self.rules.clone(),
        }
    }

    pub fn case_insensitive(&mut self, yes: bool) -> &mut GitignoreBuilder {
        self.case_insensitive = yes;
        self
    }

    pub fn allow_unclosed_class(&mut self, yes: bool) -> &mut GitignoreBuilder {
        self.allow_unclosed_class = yes;
        self
    }

    pub fn add(&mut self, path: &Path) -> Option<Error> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(err) => return Some(Error::Io(err).with_path(path)),
        };
        let mut errors = PartialErrors::default();
        for (index, line) in BufReader::new(file).lines().enumerate() {
            let number = index as u64 + 1;
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    errors.push(Error::Io(err).tagged(path, number));
                    break;
                }
            };
            let line = if index == 0 {
                line.trim_start_matches('\u{feff}')
            } else {
                &line
            };
            if let Err(err) = self.add_line(line) {
                errors.push(err.tagged(path, number));
            }
        }
        errors.into_error()
    }

    pub fn add_line(&mut self, line: &str) -> Result<&mut GitignoreBuilder, Error> {
        if line.starts_with('#') {
            return Ok(self);
        }
        let mut line = if line.ends_with("\\ ") {
            line
        } else {
            line.trim_end()
        };
        if line.is_empty() {
            return Ok(self);
        }
        let original = line;
        let mut rule = Rule {
            whitelist: false,
            only_dir: false,
        };
        let mut is_absolute = false;
        if line.starts_with("\\!") || line.starts_with("\\#") {
            line = &line[1..];
        } else {
            if let Some(rest) = line.strip_prefix('!') {
                rule.whitelist = true;
                line = rest;
            }
            if let Some(rest) = line.strip_prefix('/') {
                line = rest;
                is_absolute = true;
            }
        }
        if let Some(rest) = line.strip_suffix('/') {
            rule.only_dir = true;
            line = rest.strip_suffix('\\').unwrap_or(rest);
        }
        let mut actual = line.to_string();
        let doublestar_prefix = actual.starts_with("**/") || actual == "**";
        if !is_absolute && !line.contains('/') && !doublestar_prefix {
            actual = format!("**/{actual}");
        }
        if actual.ends_with("/**") {
            actual.push_str("/*");
        }
        let glob = GlobBuilder::new(&actual)
            .literal_separator(true)
            .case_insensitive(self.case_insensitive)
            .backslash_escape(true)
            .allow_unclosed_class(self.allow_unclosed_class)
            .build()
            .map_err(|err| Error::Glob {
                glob: original.to_string(),
                err: err.kind().to_string(),
            })?;
        self.builder.add(glob);
        self.rules.push(rule);
        Ok(self)
    }
}
