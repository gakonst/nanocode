use super::{
    regex::Regex,
    syntaxes::{ATOMS, CONTEXTS, MATCHES, REGEXES, SCOPES, SYNTAXES},
};
use std::sync::OnceLock;

const ATOMS_PER_HALF: usize = 4;
const ATOM_BITS: u32 = 16;
const ATOMS_PER_SCOPE: u32 = 8;
const PLAIN_TEXT: &str = "Plain Text";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) struct Scope {
    a: u64,
    b: u64,
}

impl Scope {
    pub(super) const fn new(a: u64, b: u64) -> Self {
        Self { a, b }
    }

    pub(super) fn parse(text: &str) -> Self {
        let mut scope = Self::default();
        let atoms: Vec<&str> = text.trim_end_matches('.').split('.').collect();
        assert!(
            atoms.len() <= ATOMS_PER_SCOPE as usize,
            "a scope has at most eight atoms"
        );
        for (index, atom) in atoms.iter().enumerate() {
            let number = ATOMS
                .binary_search(atom)
                .unwrap_or_else(|_| panic!("the generated atoms include {atom}"))
                as u64
                + 1;
            if index < ATOMS_PER_HALF {
                scope.a |= number << ((ATOMS_PER_HALF - 1 - index) as u32 * ATOM_BITS);
            } else {
                scope.b |= number << ((2 * ATOMS_PER_HALF - 1 - index) as u32 * ATOM_BITS);
            }
        }
        scope
    }

    const fn missing_atoms(self) -> u32 {
        let trail = if self.b == 0 {
            self.a.trailing_zeros() + u64::BITS
        } else {
            self.b.trailing_zeros()
        };
        trail / ATOM_BITS
    }

    pub(super) const fn len(self) -> u32 {
        ATOMS_PER_SCOPE - self.missing_atoms()
    }

    pub(super) const fn is_prefix_of(self, other: Self) -> bool {
        let missing = self.missing_atoms();
        let half = ATOMS_PER_HALF as u32;
        let mask: (u64, u64) = if missing == ATOMS_PER_SCOPE {
            (0, 0)
        } else if missing == half {
            (u64::MAX, 0)
        } else if missing > half {
            (u64::MAX << ((missing - half) * ATOM_BITS), 0)
        } else {
            (u64::MAX, u64::MAX << (missing * ATOM_BITS))
        };
        (self.a ^ other.a) & mask.0 == 0 && (self.b ^ other.b) & mask.1 == 0
    }
}

pub(super) fn scope(index: u16) -> Scope {
    SCOPES[usize::from(index)]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ClearAmount {
    TopN(usize),
    All,
}

#[derive(Clone, Copy)]
pub(super) enum Pattern {
    Match(u32),
    Include(u32),
}

#[derive(Clone, Copy)]
pub(super) enum Operation {
    Push(&'static [u32]),
    Set(&'static [u32]),
    Pop,
    None,
}

pub(super) struct MatchPattern {
    pub(super) has_captures: bool,
    pub(super) regex: u32,
    pub(super) scope: &'static [u16],
    pub(super) captures: &'static [(usize, &'static [u16])],
    pub(super) operation: Operation,
    pub(super) with_prototype: Option<u32>,
}

pub(super) struct Context {
    pub(super) meta_scope: &'static [u16],
    pub(super) meta_content_scope: &'static [u16],
    pub(super) clear_scopes: Option<ClearAmount>,
    pub(super) prototype: Option<u32>,
    pub(super) uses_backrefs: bool,
    pub(super) patterns: &'static [Pattern],
}

pub struct Syntax {
    pub name: &'static str,
    pub(super) file_extensions: &'static [&'static str],
    pub(super) start: u32,
}

pub(super) const fn context(index: u32) -> &'static Context {
    &CONTEXTS[index as usize]
}

pub(super) const fn match_pattern(index: u32) -> &'static MatchPattern {
    &MATCHES[index as usize]
}

pub struct SyntaxSet {
    regexes: Box<[OnceLock<Regex>]>,
    patterns: Box<[OnceLock<Box<[u32]>>]>,
}

impl SyntaxSet {
    pub(super) fn load() -> Self {
        Self {
            regexes: REGEXES.iter().map(|_| OnceLock::new()).collect(),
            patterns: CONTEXTS.iter().map(|_| OnceLock::new()).collect(),
        }
    }

    pub fn find_syntax_by_name(&self, name: &str) -> Option<&'static Syntax> {
        SYNTAXES.iter().rev().find(|syntax| syntax.name == name)
    }

    pub fn find_syntax_by_extension(&self, extension: &str) -> Option<&'static Syntax> {
        SYNTAXES.iter().rev().find(|syntax| {
            syntax
                .file_extensions
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
    }

    pub fn find_syntax_by_token(&self, token: &str) -> Option<&'static Syntax> {
        self.find_syntax_by_extension(token).or_else(|| {
            SYNTAXES
                .iter()
                .rev()
                .find(|syntax| syntax.name.eq_ignore_ascii_case(token))
        })
    }

    pub fn find_syntax_plain_text(&self) -> &'static Syntax {
        self.find_syntax_by_name(PLAIN_TEXT)
            .expect("the generated syntaxes include plain text")
    }

    pub(super) fn regex(&self, index: u32) -> &Regex {
        self.regexes[index as usize].get_or_init(|| {
            Regex::new(REGEXES[index as usize]).expect("every generated syntax regex compiles")
        })
    }

    pub(super) fn patterns(&self, context: u32) -> &[u32] {
        self.patterns[context as usize].get_or_init(|| flatten(context))
    }
}

fn flatten(context: u32) -> Box<[u32]> {
    let mut matches = Vec::new();
    let mut stack: Vec<(u32, usize)> = vec![(context, 0)];
    while let Some(top) = stack.last_mut() {
        let patterns = CONTEXTS[top.0 as usize].patterns;
        let Some(&pattern) = patterns.get(top.1) else {
            stack.pop();
            continue;
        };
        top.1 += 1;
        match pattern {
            Pattern::Match(index) => matches.push(index),
            Pattern::Include(included) => {
                assert!(
                    stack.len() < CONTEXTS.len(),
                    "syntax includes do not form a cycle"
                );
                stack.push((included, 0));
            }
        }
    }
    matches.into_boxed_slice()
}
