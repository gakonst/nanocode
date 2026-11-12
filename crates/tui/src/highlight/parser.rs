use super::{
    grammar::{self, ClearAmount, Context, MatchPattern, Operation, Scope, Syntax, SyntaxSet},
    regex::{self, Regex, Region, Searcher},
};
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
};

const PUSH_DEPTH_MAX: usize = 100;
const SEARCH_CACHE_CAPACITY: usize = 128;
const DECIMAL_RADIX: u32 = 10;
const PATTERN_HASH_MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ScopeStackOp {
    Push(Scope),
    Pop(usize),
    Clear(ClearAmount),
    Restore,
    Noop,
}

#[derive(Debug)]
pub enum ParseError {
    MissingMainContext,
    Regex(regex::Error),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingMainContext => {
                write!(
                    formatter,
                    "the main syntax context was popped from the stack"
                )
            }
            Self::Regex(error) => write!(
                formatter,
                "a syntax regular expression with captured text failed: {error}"
            ),
        }
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StateLevel {
    context: u32,
    prototypes: Vec<u32>,
    captures: Option<(Region, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseState {
    stack: Vec<StateLevel>,
    first_line: bool,
    proto_starts: Vec<usize>,
}

struct RegexMatch {
    regions: Region,
    pattern: u32,
    from_with_prototype: bool,
    would_loop: bool,
}

#[derive(Default)]
struct PatternHasher(u64);

impl Hasher for PatternHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(PATTERN_HASH_MULTIPLIER);
        }
    }

    fn write_u32(&mut self, value: u32) {
        self.0 = u64::from(value).wrapping_mul(PATTERN_HASH_MULTIPLIER);
    }
}

type Chained<'a> = (bool, u32, Option<&'a (Region, String)>);

type SearchCache = HashMap<u32, Option<Region>, BuildHasherDefault<PatternHasher>>;

struct Line<'a> {
    text: &'a str,
    syntaxes: &'a SyntaxSet,
    cache: SearchCache,
    regions: Region,
    searcher: Searcher,
}

impl ParseState {
    pub fn new(syntax: &Syntax) -> Self {
        Self {
            stack: vec![StateLevel {
                context: syntax.start,
                prototypes: Vec::new(),
                captures: None,
            }],
            first_line: true,
            proto_starts: Vec::new(),
        }
    }

    pub(super) fn parse_line(
        &mut self,
        text: &str,
        syntaxes: &SyntaxSet,
    ) -> Result<Vec<(usize, ScopeStackOp)>, ParseError> {
        let Some(level) = self.stack.last() else {
            return Err(ParseError::MissingMainContext);
        };
        let mut ops = Vec::new();
        if self.first_line {
            let context = grammar::context(level.context);
            if let Some(&scope) = context.meta_content_scope.first() {
                ops.push((0, ScopeStackOp::Push(grammar::scope(scope))));
            }
            self.first_line = false;
        }
        let mut line = Line {
            text,
            syntaxes,
            cache: HashMap::with_capacity_and_hasher(
                SEARCH_CACHE_CAPACITY,
                BuildHasherDefault::default(),
            ),
            regions: Region::default(),
            searcher: Searcher::default(),
        };
        let mut start = 0;
        let mut non_consuming_push_at = (0, 0);
        while self.parse_next_token(&mut line, &mut start, &mut non_consuming_push_at, &mut ops)? {}
        Ok(ops)
    }

    fn parse_next_token(
        &mut self,
        line: &mut Line<'_>,
        start: &mut usize,
        non_consuming_push_at: &mut (usize, usize),
        ops: &mut Vec<(usize, ScopeStackOp)>,
    ) -> Result<bool, ParseError> {
        let check_pop_loop = {
            let (position, depth) = *non_consuming_push_at;
            position == *start && depth == self.stack.len()
        };
        while self
            .proto_starts
            .last()
            .is_some_and(|&proto_start| proto_start >= self.stack.len())
        {
            self.proto_starts.pop();
        }
        let Some(best) = self.find_best_match(line, *start, check_pop_loop)? else {
            return Ok(false);
        };
        if best.would_loop {
            let Some((offset, _)) = line.text[*start..].char_indices().nth(1) else {
                return Ok(false);
            };
            *start += offset;
            return Ok(true);
        }
        let (_, match_end) = best
            .regions
            .pos(0)
            .expect("a match has a whole-match region");
        let pattern = grammar::match_pattern(best.pattern);
        if match_end <= *start && matches!(pattern.operation, Operation::Push(_)) {
            *non_consuming_push_at = (match_end, self.stack.len() + 1);
        }
        *start = match_end;
        if best.from_with_prototype {
            self.proto_starts.push(self.stack.len());
        }
        let level_context = grammar::context(self.top()?.context);
        self.exec_pattern(line.text, &best, level_context, ops);
        Ok(true)
    }

    fn top(&self) -> Result<&StateLevel, ParseError> {
        self.stack.last().ok_or(ParseError::MissingMainContext)
    }

    fn find_best_match(
        &self,
        line: &mut Line<'_>,
        start: usize,
        check_pop_loop: bool,
    ) -> Result<Option<RegexMatch>, ParseError> {
        let current = self.top()?;
        let context = grammar::context(current.context);
        let proto_start = self.proto_starts.last().copied().unwrap_or(0);
        let mut chain: Vec<Chained<'_>> = Vec::new();
        for level in &self.stack[proto_start..] {
            for &prototype in &level.prototypes {
                chain.push((true, prototype, level.captures.as_ref()));
            }
        }
        if let Some(prototype) = context.prototype {
            chain.push((false, prototype, None));
        }
        chain.push((false, current.context, current.captures.as_ref()));
        let mut min_start = usize::MAX;
        let mut best: Option<RegexMatch> = None;
        let mut pop_would_loop = false;
        for (from_with_prototype, chained, captures) in chain {
            let syntaxes = line.syntaxes;
            for &index in syntaxes.patterns(chained) {
                let pattern = grammar::match_pattern(index);
                let Some(regions) = self.search(line, start, index, pattern, captures)? else {
                    continue;
                };
                let (match_start, match_end) =
                    regions.pos(0).expect("a match has a whole-match region");
                if match_start < min_start || (match_start == min_start && pop_would_loop) {
                    min_start = match_start;
                    let consuming = match_end > start;
                    pop_would_loop =
                        check_pop_loop && !consuming && matches!(pattern.operation, Operation::Pop);
                    let push_too_deep = matches!(pattern.operation, Operation::Push(_))
                        && self.stack.len() >= PUSH_DEPTH_MAX;
                    if push_too_deep {
                        return Ok(None);
                    }
                    best = Some(RegexMatch {
                        regions,
                        pattern: index,
                        from_with_prototype,
                        would_loop: pop_would_loop,
                    });
                    if match_start == start && !pop_would_loop {
                        return Ok(best);
                    }
                }
            }
        }
        Ok(best)
    }

    fn search(
        &self,
        line: &mut Line<'_>,
        start: usize,
        index: u32,
        pattern: &MatchPattern,
        captures: Option<&(Region, String)>,
    ) -> Result<Option<Region>, ParseError> {
        if let Some(cached) = line.cache.get(&index) {
            match cached {
                Some(region) => {
                    let (match_start, _) = region
                        .pos(0)
                        .expect("a cached match has a whole-match region");
                    if match_start >= start {
                        return Ok(Some(region.clone()));
                    }
                }
                None => return Ok(None),
            }
        }
        let substituted;
        let (regex, can_cache) = match (pattern.has_captures, captures) {
            (true, Some((region, text))) => {
                let source = substitute_backrefs(
                    super::syntaxes::REGEXES[pattern.regex as usize],
                    region,
                    text,
                );
                substituted = Regex::new(&source).map_err(ParseError::Regex)?;
                (&substituted, false)
            }
            _ => (line.syntaxes.regex(pattern.regex), true),
        };
        let matched = matches!(
            regex.search(line.text, start, &mut line.regions, &mut line.searcher),
            Ok(true)
        );
        if matched {
            let (match_start, match_end) = line
                .regions
                .pos(0)
                .expect("a match has a whole-match region");
            let does_something = match pattern.operation {
                Operation::None => match_start != match_end,
                Operation::Push(_) => self.stack.len() < PUSH_DEPTH_MAX,
                Operation::Set(_) | Operation::Pop => true,
            };
            if can_cache && does_something {
                line.cache.insert(index, Some(line.regions.clone()));
            }
            if does_something {
                return Ok(Some(line.regions.clone()));
            }
        } else if can_cache {
            line.cache.insert(index, None);
        }
        Ok(None)
    }

    fn exec_pattern(
        &mut self,
        text: &str,
        best: &RegexMatch,
        level_context: &'static Context,
        ops: &mut Vec<(usize, ScopeStackOp)>,
    ) {
        let (match_start, match_end) = best
            .regions
            .pos(0)
            .expect("a match has a whole-match region");
        let pattern = grammar::match_pattern(best.pattern);
        push_meta_ops(true, match_start, level_context, pattern.operation, ops);
        for &scope in pattern.scope {
            ops.push((match_start, ScopeStackOp::Push(grammar::scope(scope))));
        }
        if !pattern.captures.is_empty() {
            let mut map: Vec<((usize, i64), ScopeStackOp)> = Vec::new();
            for &(group, scopes) in pattern.captures {
                let Some((capture_start, capture_end)) = best.regions.pos(group) else {
                    continue;
                };
                if capture_start == capture_end {
                    continue;
                }
                let length =
                    i64::try_from(capture_end - capture_start).expect("line lengths fit i64");
                for &scope in scopes {
                    map.push((
                        (capture_start, -length),
                        ScopeStackOp::Push(grammar::scope(scope)),
                    ));
                }
                map.push(((capture_end, i64::MIN), ScopeStackOp::Pop(scopes.len())));
            }
            map.sort_by_key(|left| left.0);
            for ((position, _), op) in map {
                ops.push((position, op));
            }
        }
        if !pattern.scope.is_empty() {
            ops.push((match_end, ScopeStackOp::Pop(pattern.scope.len())));
        }
        push_meta_ops(false, match_end, level_context, pattern.operation, ops);
        self.perform_op(text, &best.regions, pattern);
    }

    fn perform_op(&mut self, text: &str, regions: &Region, pattern: &MatchPattern) {
        let (references, old_prototypes) = match pattern.operation {
            Operation::Push(references) => (references, None),
            Operation::Set(references) => {
                (references, self.stack.pop().map(|level| level.prototypes))
            }
            Operation::Pop => {
                self.stack.pop();
                return;
            }
            Operation::None => return,
        };
        for (index, &reference) in references.iter().enumerate() {
            let mut prototypes = if index == 0 {
                old_prototypes.clone().unwrap_or_default()
            } else {
                Vec::new()
            };
            if index == references.len() - 1
                && let Some(prototype) = pattern.with_prototype
            {
                prototypes.push(prototype);
            }
            let context = grammar::context(reference);
            let uses_backrefs = context.uses_backrefs
                || prototypes
                    .iter()
                    .any(|&prototype| grammar::context(prototype).uses_backrefs);
            let captures = uses_backrefs.then(|| (regions.clone(), text.to_owned()));
            self.stack.push(StateLevel {
                context: reference,
                prototypes,
                captures,
            });
        }
    }
}

fn push_meta_ops(
    initial: bool,
    index: usize,
    current: &Context,
    operation: Operation,
    ops: &mut Vec<(usize, ScopeStackOp)>,
) {
    match operation {
        Operation::Pop => {
            let scopes = if initial {
                current.meta_content_scope
            } else {
                current.meta_scope
            };
            if !scopes.is_empty() {
                ops.push((index, ScopeStackOp::Pop(scopes.len())));
            }
            if !initial && current.clear_scopes.is_some() {
                ops.push((index, ScopeStackOp::Restore));
            }
        }
        Operation::Push(references) | Operation::Set(references) => {
            let is_set = matches!(operation, Operation::Set(_));
            if initial {
                push_initial(index, current, references, is_set, ops);
            } else {
                push_final(index, current, references, is_set, ops);
            }
        }
        Operation::None => {}
    }
}

fn push_initial(
    index: usize,
    current: &Context,
    references: &[u32],
    is_set: bool,
    ops: &mut Vec<(usize, ScopeStackOp)>,
) {
    if is_set && current.clear_scopes.is_some() {
        ops.push((index, ScopeStackOp::Restore));
    }
    for &reference in references {
        let context = grammar::context(reference);
        if !is_set && let Some(amount) = context.clear_scopes {
            ops.push((index, ScopeStackOp::Clear(amount)));
        }
        for &scope in context.meta_scope {
            ops.push((index, ScopeStackOp::Push(grammar::scope(scope))));
        }
    }
}

fn push_final(
    index: usize,
    current: &Context,
    references: &[u32],
    is_set: bool,
    ops: &mut Vec<(usize, ScopeStackOp)>,
) {
    let repush = (is_set
        && (!current.meta_scope.is_empty() || !current.meta_content_scope.is_empty()))
        || references.iter().any(|&reference| {
            let context = grammar::context(reference);
            !context.meta_content_scope.is_empty() || (context.clear_scopes.is_some() && is_set)
        });
    if !repush {
        return;
    }
    let mut pop_count: usize = references
        .iter()
        .map(|&reference| grammar::context(reference).meta_scope.len())
        .sum();
    if is_set {
        pop_count += current.meta_content_scope.len() + current.meta_scope.len();
    }
    if pop_count > 0 {
        ops.push((index, ScopeStackOp::Pop(pop_count)));
    }
    for &reference in references {
        let context = grammar::context(reference);
        if is_set && let Some(amount) = context.clear_scopes {
            ops.push((index, ScopeStackOp::Clear(amount)));
        }
        for &scope in context.meta_scope.iter().chain(context.meta_content_scope) {
            ops.push((index, ScopeStackOp::Push(grammar::scope(scope))));
        }
    }
}

const fn is_meta_character(character: char) -> bool {
    matches!(
        character,
        '\\' | '.'
            | '+'
            | '*'
            | '?'
            | '('
            | ')'
            | '|'
            | '['
            | ']'
            | '{'
            | '}'
            | '^'
            | '$'
            | '#'
            | '&'
            | '-'
            | '~'
    )
}

fn substitute_backrefs(source: &str, region: &Region, text: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let mut last_was_escape = false;
    for character in source.chars() {
        if last_was_escape && let Some(digit) = character.to_digit(DECIMAL_RADIX) {
            if let Some((start, end)) = region.pos(digit as usize) {
                for captured in text[start..end].chars() {
                    if is_meta_character(captured) {
                        result.push('\\');
                    }
                    result.push(captured);
                }
            }
        } else if last_was_escape {
            result.push('\\');
            result.push(character);
        } else if character != '\\' {
            result.push(character);
        }
        last_was_escape = character == '\\' && !last_was_escape;
    }
    if last_was_escape {
        result.push('\\');
    }
    result
}
