use std::collections::BTreeMap;
use std::path::Path;

use super::default_types::DEFAULT_TYPES;
use super::{Error, Match, file_name};
use crate::search::glob::{Candidate, GlobBuilder, GlobSet, GlobSetBuilder};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileTypeDef {
    name: String,
    globs: Vec<String>,
}

impl FileTypeDef {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn globs(&self) -> &[String] {
        &self.globs
    }
}

#[derive(Clone, Debug)]
enum Selection {
    Select(String),
    Negate(String),
}

impl Selection {
    fn name(&self) -> &str {
        match self {
            Selection::Select(name) | Selection::Negate(name) => name,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Types {
    definitions: Vec<FileTypeDef>,
    negated: Vec<bool>,
    has_selected: bool,
    glob_to_selection: Vec<usize>,
    set: GlobSet,
}

impl Types {
    pub fn empty() -> Types {
        Types {
            definitions: Vec::new(),
            negated: Vec::new(),
            has_selected: false,
            glob_to_selection: Vec::new(),
            set: GlobSet::empty(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.negated.is_empty()
    }

    pub fn definitions(&self) -> &[FileTypeDef] {
        &self.definitions
    }

    pub fn matched<P: AsRef<Path>>(&self, path: P, is_dir: bool) -> Match<()> {
        if is_dir || self.set.is_empty() {
            return Match::None;
        }
        let Some(name) = file_name(path.as_ref()) else {
            return self.unmatched();
        };
        let candidate = Candidate::new(Path::new(name));
        let mut last: Option<usize> = None;
        self.set.visit_matches(&candidate, &mut |index| {
            if last.is_none_or(|found| index > found) {
                last = Some(index);
            }
        });
        match last {
            None => self.unmatched(),
            Some(index) if self.negated[self.glob_to_selection[index]] => Match::Ignore(()),
            Some(_) => Match::Whitelist(()),
        }
    }

    fn unmatched(&self) -> Match<()> {
        if self.has_selected {
            Match::Ignore(())
        } else {
            Match::None
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TypesBuilder {
    types: BTreeMap<String, Vec<String>>,
    selections: Vec<Selection>,
}

impl TypesBuilder {
    pub fn new() -> TypesBuilder {
        TypesBuilder::default()
    }

    pub fn build(&self) -> Result<Types, Error> {
        let mut negated = Vec::with_capacity(self.selections.len());
        let mut glob_to_selection = Vec::new();
        let mut set = GlobSetBuilder::new();
        for (index, selection) in self.selections.iter().enumerate() {
            let Some(globs) = self.types.get(selection.name()) else {
                return Err(Error::UnrecognizedFileType(selection.name().to_string()));
            };
            for glob in globs {
                let built = GlobBuilder::new(glob)
                    .literal_separator(true)
                    .build()
                    .map_err(|err| Error::Glob {
                        glob: glob.clone(),
                        err: err.kind().to_string(),
                    })?;
                set.add(built);
                glob_to_selection.push(index);
            }
            negated.push(matches!(selection, Selection::Negate(_)));
        }
        Ok(Types {
            definitions: self.definitions(),
            has_selected: negated.iter().any(|&negate| !negate),
            negated,
            glob_to_selection,
            set: set.build(),
        })
    }

    fn definitions(&self) -> Vec<FileTypeDef> {
        self.types
            .iter()
            .map(|(name, globs)| {
                let mut globs = globs.clone();
                globs.sort();
                FileTypeDef {
                    name: name.clone(),
                    globs,
                }
            })
            .collect()
    }

    pub fn select(&mut self, name: &str) -> &mut TypesBuilder {
        if name == "all" {
            let names: Vec<String> = self.types.keys().cloned().collect();
            self.selections
                .extend(names.into_iter().map(Selection::Select));
        } else {
            self.selections.push(Selection::Select(name.to_string()));
        }
        self
    }

    pub fn negate(&mut self, name: &str) -> &mut TypesBuilder {
        if name == "all" {
            let names: Vec<String> = self.types.keys().cloned().collect();
            self.selections
                .extend(names.into_iter().map(Selection::Negate));
        } else {
            self.selections.push(Selection::Negate(name.to_string()));
        }
        self
    }

    pub fn clear(&mut self, name: &str) -> &mut TypesBuilder {
        self.types.remove(name);
        self
    }

    pub fn add(&mut self, name: &str, glob: &str) -> Result<(), Error> {
        if name == "all" || !name.chars().all(char::is_alphanumeric) {
            return Err(Error::InvalidDefinition);
        }
        self.types
            .entry(name.to_string())
            .or_default()
            .push(glob.to_string());
        Ok(())
    }

    pub fn add_def(&mut self, definition: &str) -> Result<(), Error> {
        let parts: Vec<&str> = definition.split(':').collect();
        match parts[..] {
            [name, glob] => {
                if name.is_empty() || glob.is_empty() {
                    return Err(Error::InvalidDefinition);
                }
                self.add(name, glob)
            }
            [name, include, names] => {
                if name.is_empty() || include != "include" || names.is_empty() {
                    return Err(Error::InvalidDefinition);
                }
                if names
                    .split(',')
                    .any(|other| !self.types.contains_key(other))
                {
                    return Err(Error::InvalidDefinition);
                }
                for other in names.split(',') {
                    let globs = self.types[other].clone();
                    for glob in globs {
                        self.add(name, &glob)?;
                    }
                }
                Ok(())
            }
            _ => Err(Error::InvalidDefinition),
        }
    }

    pub fn add_defaults(&mut self) -> &mut TypesBuilder {
        for &(names, globs) in DEFAULT_TYPES {
            for name in names {
                for glob in globs {
                    self.add(name, glob)
                        .expect("a default type definition is valid");
                }
            }
        }
        self
    }
}
