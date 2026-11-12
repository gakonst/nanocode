use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;

const HEURISTIC_COST_MIN: u32 = 256;
const MAX_COST_MIN: u32 = 256;
const SNAKE_TOKENS_MIN: u32 = 20;
const HEURISTIC_SCORE_FACTOR: u32 = 4;
const EQUAL_LIMIT_MAX: u32 = 1024;
const PRUNE_WINDOW_LINES: usize = 100;
const PRUNE_UNMATCHED_FACTOR: u32 = 3;
const TAB_WIDTH_COLUMNS: u8 = 8;
const SLIDING_LINES_MAX: u32 = 100;
const BLANK_LINES_MAX: usize = 20;
const INDENT_MAX: u8 = 200;
const INDENT_BLANK: u8 = u8::MAX;
const START_OF_FILE_PENALTY: i32 = 1;
const END_OF_FILE_PENALTY: i32 = 21;
const TOTAL_BLANK_LINE_WEIGHT: i32 = -30;
const TRAILING_BLANK_LINES_WEIGHT: i32 = 6;
const RELATIVE_INDENT_PENALTY: i32 = -4;
const RELATIVE_INDENT_WITH_BLANK_PENALTY: i32 = 10;
const RELATIVE_OUTDENT_PENALTY: i32 = 24;
const RELATIVE_OUTDENT_WITH_BLANK_PENALTY: i32 = 17;
const RELATIVE_DEDENT_PENALTY: i32 = 23;
const RELATIVE_DEDENT_WITH_BLANK_PENALTY: i32 = 17;
const INDENT_WEIGHT: i32 = 60;

pub fn lines(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    text.split_inclusive(|&byte| byte == b'\n')
}

pub fn changed_lines<'text>(
    old: &'text [u8],
    new: &'text [u8],
) -> Vec<(Range<usize>, Range<usize>)> {
    let mut ids: HashMap<&'text [u8], u32> = HashMap::new();
    let mut indents = Vec::new();
    let mut intern = |line: &'text [u8]| -> u32 {
        let next = u32::try_from(ids.len()).expect("a text has fewer than 2^32 distinct lines");
        *ids.entry(line).or_insert_with(|| {
            indents.push(indent_of_line(line));
            next
        })
    };
    let before: Vec<u32> = lines(old).map(&mut intern).collect();
    let after: Vec<u32> = lines(new).map(&mut intern).collect();
    assert!(before.len() < i32::MAX as usize);
    assert!(after.len() < i32::MAX as usize);
    let mut removed = vec![false; before.len()];
    let mut added = vec![false; after.len()];
    diff(&before, &after, &mut removed, &mut added);
    postprocess(&mut removed, &added, &before, &indents);
    postprocess(&mut added, &removed, &after, &indents);
    hunks(&removed, &added)
}

fn indent_of_line(line: &[u8]) -> u8 {
    let mut indent: u8 = 0;
    for &byte in line {
        match byte {
            b' ' => indent = indent.saturating_add(1).min(INDENT_MAX),
            b'\t' => {
                indent = indent
                    .saturating_add(TAB_WIDTH_COLUMNS - indent % TAB_WIDTH_COLUMNS)
                    .min(INDENT_MAX)
            }
            b'\r' | b'\n' | b'\x0C' => {}
            _ => return indent,
        }
        if indent >= INDENT_MAX {
            return INDENT_MAX;
        }
    }
    INDENT_BLANK
}

fn common_prefix(first: &[u32], second: &[u32]) -> usize {
    first.iter().zip(second).take_while(|(a, b)| a == b).count()
}

fn common_suffix(first: &[u32], second: &[u32]) -> usize {
    first
        .iter()
        .rev()
        .zip(second.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
}

fn square_root_bound(value: usize) -> u32 {
    let bits = (usize::BITS - value.leading_zeros()).div_ceil(2);
    1u32.checked_shl(bits).unwrap_or(u32::MAX)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Occurrences {
    None,
    Some,
    Common,
}

fn count_tokens(tokens: &[u32]) -> Vec<u32> {
    let mut counts = Vec::new();
    for &token in tokens {
        let bucket = token as usize;
        if bucket >= counts.len() {
            counts.resize(bucket + 1, 0u32);
        }
        counts[bucket] += 1;
    }
    counts
}

fn should_prune_common_line(status: &[Occurrences], position: usize) -> bool {
    let mut unmatched_before = 0u32;
    let mut common_before = 1u32;
    let start = position.saturating_sub(PRUNE_WINDOW_LINES);
    for occurrences in status[start..position].iter().rev() {
        match occurrences {
            Occurrences::None => unmatched_before += 1,
            Occurrences::Common => common_before += 1,
            Occurrences::Some => break,
        }
    }
    if unmatched_before == 0 {
        return false;
    }
    let end = status.len().min(position + PRUNE_WINDOW_LINES + 1);
    let mut unmatched_after = 0u32;
    let mut common_after = 0u32;
    for occurrences in &status[position..end] {
        match occurrences {
            Occurrences::None => unmatched_after += 1,
            Occurrences::Common => common_after += 1,
            Occurrences::Some => break,
        }
    }
    if unmatched_after == 0 {
        return false;
    }
    unmatched_before + unmatched_after > PRUNE_UNMATCHED_FACTOR * (common_before + common_after)
}

struct Pruned {
    tokens: Vec<u32>,
    indices: Vec<u32>,
}

fn prune(tokens: &[u32], other_counts: &[u32], equal_limit: u32, changed: &mut [bool]) -> Pruned {
    let status: Vec<Occurrences> = tokens
        .iter()
        .map(
            |&token| match other_counts.get(token as usize).copied().unwrap_or(0) {
                0 => Occurrences::None,
                count if count >= equal_limit => Occurrences::Common,
                _ => Occurrences::Some,
            },
        )
        .collect();
    let mut pruned = Pruned {
        tokens: Vec::new(),
        indices: Vec::new(),
    };
    for (index, (&token, &occurrences)) in tokens.iter().zip(&status).enumerate() {
        let drop = match occurrences {
            Occurrences::None => true,
            Occurrences::Some => false,
            Occurrences::Common => should_prune_common_line(&status, index),
        };
        if drop {
            changed[index] = true;
        } else {
            pruned.tokens.push(token);
            pruned.indices.push(index as u32);
        }
    }
    pruned
}

fn diff(full_before: &[u32], full_after: &[u32], removed: &mut [bool], added: &mut [bool]) {
    let prefix = common_prefix(full_before, full_after);
    let suffix = common_suffix(&full_before[prefix..], &full_after[prefix..]);
    let before = &full_before[prefix..full_before.len() - suffix];
    let after = &full_after[prefix..full_after.len() - suffix];
    let removed_end = removed.len() - suffix;
    let removed = &mut removed[prefix..removed_end];
    let added_end = added.len() - suffix;
    let added = &mut added[prefix..added_end];
    if before.is_empty() {
        added.fill(true);
        return;
    }
    if after.is_empty() {
        removed.fill(true);
        return;
    }
    let equal_limit = |length: usize| square_root_bound(length).min(EQUAL_LIMIT_MAX);
    let counts_before = count_tokens(full_before);
    let counts_after = count_tokens(full_after);
    let first = prune(
        before,
        &counts_after,
        equal_limit(full_before.len()),
        removed,
    );
    let second = prune(after, &counts_before, equal_limit(full_after.len()), added);
    let mut myers = Myers::new(first.tokens.len(), second.tokens.len());
    let mut stack = vec![(0..first.tokens.len(), 0..second.tokens.len(), false)];
    while let Some((mut range1, mut range2, need_minimal)) = stack.pop() {
        let prefix = common_prefix(
            &first.tokens[range1.clone()],
            &second.tokens[range2.clone()],
        );
        range1.start += prefix;
        range2.start += prefix;
        let suffix = common_suffix(
            &first.tokens[range1.clone()],
            &second.tokens[range2.clone()],
        );
        range1.end -= suffix;
        range2.end -= suffix;
        if range1.is_empty() {
            for &index in &second.indices[range2] {
                added[index as usize] = true;
            }
            continue;
        }
        if range2.is_empty() {
            for &index in &first.indices[range1] {
                removed[index as usize] = true;
            }
            continue;
        }
        let file1 = &first.tokens[range1.clone()];
        let file2 = &second.tokens[range2.clone()];
        let split = myers.split(file1, file2, need_minimal);
        let middle1 = range1.start + split.index1 as usize;
        let middle2 = range2.start + split.index2 as usize;
        stack.push((
            middle1..range1.end,
            middle2..range2.end,
            split.minimized_high,
        ));
        stack.push((
            range1.start..middle1,
            range2.start..middle2,
            split.minimized_low,
        ));
    }
}

struct Split {
    index1: i32,
    index2: i32,
    minimized_low: bool,
    minimized_high: bool,
}

struct Myers {
    forward: Vec<i32>,
    backward: Vec<i32>,
    base: i32,
    max_cost: u32,
}

impl Myers {
    fn new(length1: usize, length2: usize) -> Myers {
        let diagonals = length1 + length2 + 3;
        Myers {
            forward: vec![0; diagonals],
            backward: vec![0; diagonals],
            base: length2 as i32 + 1,
            max_cost: square_root_bound(diagonals).max(MAX_COST_MIN),
        }
    }

    fn split(&mut self, file1: &[u32], file2: &[u32], need_minimal: bool) -> Split {
        let mut forward = Search::new(&mut self.forward, self.base, false, file1, file2);
        let mut backward = Search::new(&mut self.backward, self.base, true, file1, file2);
        let odd = file2.len().wrapping_sub(file1.len()) & 1 != 0;
        let mut cost = 0u32;
        loop {
            cost += 1;
            let mut found_snake = false;
            forward.next_d();
            if odd {
                let outcome = forward.run(file1, file2, |k, index1| {
                    backward.contains(k) && backward.x_at(k) <= index1
                });
                match outcome {
                    Some(Outcome::Snake) => found_snake = true,
                    Some(Outcome::Found { index1, index2 }) => {
                        return Split {
                            index1,
                            index2,
                            minimized_low: true,
                            minimized_high: true,
                        };
                    }
                    None => {}
                }
            } else {
                found_snake |= forward.run(file1, file2, |_, _| false).is_some();
            }
            backward.next_d();
            if !odd {
                let outcome = backward.run(file1, file2, |k, index1| {
                    forward.contains(k) && index1 <= forward.x_at(k)
                });
                match outcome {
                    Some(Outcome::Snake) => found_snake = true,
                    Some(Outcome::Found { index1, index2 }) => {
                        return Split {
                            index1,
                            index2,
                            minimized_low: true,
                            minimized_high: true,
                        };
                    }
                    None => {}
                }
            } else {
                found_snake |= backward.run(file1, file2, |_, _| false).is_some();
            }
            if need_minimal {
                continue;
            }
            if found_snake && cost > HEURISTIC_COST_MIN {
                if let Some((index1, index2)) = forward.found_snake(cost, file1, file2) {
                    return Split {
                        index1,
                        index2,
                        minimized_low: true,
                        minimized_high: false,
                    };
                }
                if let Some((index1, index2)) = backward.found_snake(cost, file1, file2) {
                    return Split {
                        index1,
                        index2,
                        minimized_low: false,
                        minimized_high: true,
                    };
                }
            }
            if cost >= self.max_cost {
                break;
            }
        }
        let (distance_forward, index1_forward) = forward.best_position(file1, file2);
        let (distance_backward, index1_backward) = backward.best_position(file1, file2);
        if distance_forward > file1.len() as isize + file2.len() as isize - distance_backward {
            Split {
                index1: index1_forward,
                index2: (distance_forward - index1_forward as isize) as i32,
                minimized_low: true,
                minimized_high: false,
            }
        } else {
            Split {
                index1: index1_backward,
                index2: (distance_backward - index1_backward as isize) as i32,
                minimized_low: false,
                minimized_high: true,
            }
        }
    }
}

enum Outcome {
    Snake,
    Found { index1: i32, index2: i32 },
}

struct Search<'a> {
    diagonals: &'a mut [i32],
    base: i32,
    backward: bool,
    k_min: i32,
    k_max: i32,
    d_min: i32,
    d_max: i32,
}

impl<'a> Search<'a> {
    fn new(
        diagonals: &'a mut [i32],
        base: i32,
        backward: bool,
        file1: &[u32],
        file2: &[u32],
    ) -> Search<'a> {
        let d_min = -(file2.len() as i32);
        let d_max = file1.len() as i32;
        let k_middle = if backward { d_min + d_max } else { 0 };
        let mut search = Search {
            diagonals,
            base,
            backward,
            k_min: k_middle,
            k_max: k_middle,
            d_min,
            d_max,
        };
        let initial = if backward { file1.len() as i32 } else { 0 };
        search.write(k_middle, initial);
        search
    }

    fn slot(&self, k: i32) -> usize {
        assert!((self.d_min - 1..=self.d_max + 1).contains(&k));
        (self.base + k) as usize
    }

    fn write(&mut self, k: i32, index1: i32) {
        let slot = self.slot(k);
        self.diagonals[slot] = index1;
    }

    fn x_at(&self, k: i32) -> i32 {
        self.diagonals[self.slot(k)]
    }

    fn contains(&self, k: i32) -> bool {
        (self.k_min..=self.k_max).contains(&k)
    }

    fn next_d(&mut self) {
        let initial = if self.backward { i32::MAX } else { i32::MIN };
        if self.k_min > self.d_min {
            self.k_min -= 1;
            self.write(self.k_min - 1, initial);
        } else {
            self.k_min += 1;
        }
        if self.k_max < self.d_max {
            self.k_max += 1;
            self.write(self.k_max + 1, initial);
        } else {
            self.k_max -= 1;
        }
    }

    fn run(
        &mut self,
        file1: &[u32],
        file2: &[u32],
        mut done: impl FnMut(i32, i32) -> bool,
    ) -> Option<Outcome> {
        let mut outcome = None;
        let mut k = self.k_max;
        while k >= self.k_min {
            let mut index1 = if self.backward {
                if self.x_at(k - 1) < self.x_at(k + 1) {
                    self.x_at(k - 1)
                } else {
                    self.x_at(k + 1) - 1
                }
            } else if self.x_at(k - 1) >= self.x_at(k + 1) {
                self.x_at(k - 1) + 1
            } else {
                self.x_at(k + 1)
            };
            let mut index2 = index1 - k;
            let run = if self.backward {
                if index1 > 0 && index2 > 0 {
                    common_suffix(&file1[..index1 as usize], &file2[..index2 as usize])
                } else {
                    0
                }
            } else if index1 < file1.len() as i32 && index2 < file2.len() as i32 {
                common_prefix(&file1[index1 as usize..], &file2[index2 as usize..])
            } else {
                0
            };
            if run > SNAKE_TOKENS_MIN as usize {
                outcome = Some(Outcome::Snake);
            }
            if self.backward {
                index1 -= run as i32;
                index2 -= run as i32;
            } else {
                index1 += run as i32;
                index2 += run as i32;
            }
            self.write(k, index1);
            if done(k, index1) {
                return Some(Outcome::Found { index1, index2 });
            }
            k -= 2;
        }
        outcome
    }

    fn best_position(&self, file1: &[u32], file2: &[u32]) -> (isize, i32) {
        let mut best_distance: isize = if self.backward { isize::MAX } else { -1 };
        let mut best_index1 = if self.backward { i32::MAX } else { -1 };
        let mut k = self.k_max;
        while k >= self.k_min {
            let mut index1 = self.x_at(k);
            if self.backward {
                index1 = index1.max(0);
            } else {
                index1 = index1.min(file1.len() as i32);
            }
            let mut index2 = index1 - k;
            if self.backward {
                if index2 < 0 {
                    index1 = k;
                    index2 = 0;
                }
            } else if index2 > file2.len() as i32 {
                index1 = file2.len() as i32 + k;
                index2 = file2.len() as i32;
            }
            let distance = index1 as isize + index2 as isize;
            if (self.backward && distance < best_distance)
                || (!self.backward && distance > best_distance)
            {
                best_distance = distance;
                best_index1 = index1;
            }
            k -= 2;
        }
        (best_distance, best_index1)
    }

    fn found_snake(&self, cost: u32, file1: &[u32], file2: &[u32]) -> Option<(i32, i32)> {
        let snake = SNAKE_TOKENS_MIN as i32;
        let length1 = file1.len() as i32;
        let length2 = file2.len() as i32;
        let mut best_score = 0usize;
        let mut best = (0, 0);
        let mut k = self.k_max;
        while k >= self.k_min {
            let index1 = self.x_at(k);
            let index2 = index1 - k;
            let inside = if self.backward {
                (0..length1 - snake).contains(&index1) && (0..length2 - snake).contains(&index2)
            } else {
                (snake..length1).contains(&index1) && (snake..length2).contains(&index2)
            };
            if inside {
                let distance = if self.backward {
                    (length1 - index1) as usize + (length2 - index2) as usize
                } else {
                    index1 as usize + index2 as usize
                };
                let score = distance + k.unsigned_abs() as usize;
                if score > (HEURISTIC_SCORE_FACTOR * cost) as usize && score > best_score {
                    let is_snake = if self.backward {
                        file1[index1 as usize..]
                            .iter()
                            .zip(&file2[index2 as usize..])
                            .take(SNAKE_TOKENS_MIN as usize)
                            .all(|(a, b)| a == b)
                    } else {
                        file1[..index1 as usize]
                            .iter()
                            .zip(&file2[..index2 as usize])
                            .rev()
                            .take(SNAKE_TOKENS_MIN as usize)
                            .all(|(a, b)| a == b)
                    };
                    if is_snake {
                        best = (index1, index2);
                        best_score = score;
                    }
                }
            }
            k -= 2;
        }
        (best_score > 0).then_some(best)
    }
}

struct Hunk {
    before: Range<u32>,
    after: Range<u32>,
}

fn find_next_change(changes: &[bool], position: u32) -> Option<u32> {
    changes[position as usize..]
        .iter()
        .position(|&changed| changed)
        .map(|offset| offset as u32)
}

fn find_hunk_end(changes: &[bool], position: u32) -> u32 {
    position
        + changes[position as usize..]
            .iter()
            .take_while(|&&changed| changed)
            .count() as u32
}

fn find_hunk_start(changes: &[bool], position: u32) -> u32 {
    position
        - changes[..position as usize]
            .iter()
            .rev()
            .take_while(|&&changed| changed)
            .count() as u32
}

impl Hunk {
    fn next_hunk(&mut self, other: &[bool], changed: &[bool]) -> bool {
        let Some(offset) = find_next_change(changed, self.after.end) else {
            return false;
        };
        let mut offset_before = 0;
        loop {
            let unchanged = find_next_change(other, self.before.end)
                .unwrap_or(other.len() as u32 - self.before.end);
            if offset_before + unchanged > offset {
                self.before.start = self.before.end + (offset - offset_before);
                self.before.end = self.before.start;
                break;
            }
            offset_before += unchanged;
            self.before.start = self.before.end + unchanged;
            self.before.end = find_hunk_end(other, self.before.end + unchanged);
            if offset_before == offset {
                break;
            }
        }
        self.after.start = self.after.end + offset;
        self.after.end = find_hunk_end(changed, self.after.start);
        true
    }
}

struct Postprocessor<'a> {
    changed: &'a mut [bool],
    other: &'a [bool],
    tokens: &'a [u32],
    indents: &'a [u8],
    hunk: Hunk,
}

fn postprocess(changed: &mut [bool], other: &[bool], tokens: &[u32], indents: &[u8]) {
    let mut processor = Postprocessor {
        changed,
        other,
        tokens,
        indents,
        hunk: Hunk {
            before: 0..0,
            after: 0..0,
        },
    };
    processor.run();
}

impl Postprocessor<'_> {
    fn run(&mut self) {
        while self.hunk.next_hunk(self.other, self.changed) {
            let mut earliest_end;
            let mut is_modification;
            loop {
                while self.slide_up() {}
                earliest_end = self.hunk.after.end;
                is_modification = self.hunk.before.start != self.hunk.before.end;
                let size_unexpanded = self.hunk.after.len();
                while self.slide_down() {
                    is_modification |= self.hunk.before.start != self.hunk.before.end;
                }
                if size_unexpanded == self.hunk.after.len() {
                    break;
                }
            }
            if self.hunk.after.end == earliest_end {
                continue;
            }
            if is_modification {
                while self.hunk.before.start == self.hunk.before.end {
                    assert!(self.slide_up());
                }
            } else {
                let slider_end = self.best_slider_end(earliest_end);
                for _ in slider_end..self.hunk.after.end {
                    assert!(self.slide_up());
                }
            }
        }
    }

    fn slide_down(&mut self) -> bool {
        let Some(&next) = self.tokens.get(self.hunk.after.end as usize) else {
            return false;
        };
        if self.tokens[self.hunk.after.start as usize] != next {
            return false;
        }
        self.changed[self.hunk.after.start as usize] = false;
        self.changed[self.hunk.after.end as usize] = true;
        self.hunk.after.start += 1;
        self.hunk.after.end = find_hunk_end(self.changed, self.hunk.after.end);
        self.hunk.before.start = self.hunk.before.end + 1;
        self.hunk.before.end = find_hunk_end(self.other, self.hunk.before.start);
        true
    }

    fn slide_up(&mut self) -> bool {
        if self.hunk.after.start == 0 {
            return false;
        }
        if self.tokens[self.hunk.after.start as usize - 1]
            != self.tokens[self.hunk.after.end as usize - 1]
        {
            return false;
        }
        self.changed[self.hunk.after.start as usize - 1] = true;
        self.changed[self.hunk.after.end as usize - 1] = false;
        self.hunk.after.end -= 1;
        self.hunk.after.start = find_hunk_start(self.changed, self.hunk.after.start - 1);
        self.hunk.before.end = self.hunk.before.start - 1;
        self.hunk.before.start = find_hunk_start(self.other, self.hunk.before.start - 1);
        true
    }

    fn best_slider_end(&self, earliest_end: u32) -> u32 {
        let hunk = self.hunk.after.clone();
        let mut top_slider_end = earliest_end;
        if top_slider_end < hunk.start - 1 {
            top_slider_end = hunk.start - 1;
        }
        if hunk.end > top_slider_end + SLIDING_LINES_MAX {
            top_slider_end = hunk.end - SLIDING_LINES_MAX;
        }
        let group_size = hunk.end - hunk.start;
        let mut best_score = self.score_range(top_slider_end - group_size, top_slider_end);
        let mut best_slider_end = top_slider_end;
        for slider_end in (top_slider_end + 1)..=hunk.end {
            let score = self.score_range(slider_end - group_size, slider_end);
            if score.is_improvement_over(best_score) {
                best_score = score;
                best_slider_end = slider_end;
            }
        }
        best_slider_end
    }

    fn indent(&self, token: u32) -> u8 {
        self.indents[token as usize]
    }

    fn score_range(&self, start: u32, end: u32) -> Score {
        let first = self.indents_at(start as usize).score();
        let second = self.indents_at(end as usize).score();
        Score {
            indent: first.indent + second.indent,
            penalty: first.penalty + second.penalty,
        }
    }

    fn indents_at(&self, index: usize) -> Indents {
        let nearest = |offset: usize, token: u32| {
            if offset == BLANK_LINES_MAX {
                Some((offset, 0))
            } else {
                let level = self.indent(token);
                (level != INDENT_BLANK).then_some((offset, level))
            }
        };
        let (leading_blanks, previous_indent) = self.tokens[..index]
            .iter()
            .rev()
            .enumerate()
            .find_map(|(offset, &token)| nearest(offset, token))
            .unwrap_or((index, INDENT_BLANK));
        let at_end = index == self.tokens.len();
        let (trailing_blanks, next_indent) = if at_end {
            (0, INDENT_BLANK)
        } else {
            self.tokens[index + 1..]
                .iter()
                .enumerate()
                .find_map(|(offset, &token)| nearest(offset, token))
                .unwrap_or((self.tokens.len() - index - 1, INDENT_BLANK))
        };
        Indents {
            indent: self
                .tokens
                .get(index)
                .map_or(INDENT_BLANK, |&token| self.indent(token)),
            previous_indent,
            next_indent,
            leading_blanks: leading_blanks as u8,
            trailing_blanks: trailing_blanks as u8,
            at_end,
        }
    }
}

struct Indents {
    indent: u8,
    previous_indent: u8,
    next_indent: u8,
    leading_blanks: u8,
    trailing_blanks: u8,
    at_end: bool,
}

#[derive(Clone, Copy)]
struct Score {
    indent: i32,
    penalty: i32,
}

impl Indents {
    fn score(&self) -> Score {
        let mut penalty = 0;
        if self.previous_indent == INDENT_BLANK && self.leading_blanks == 0 {
            penalty += START_OF_FILE_PENALTY;
        }
        if self.at_end {
            penalty += END_OF_FILE_PENALTY;
        }
        let trailing_blank_lines = if self.indent == INDENT_BLANK {
            i32::from(self.trailing_blanks) + 1
        } else {
            0
        };
        let total_blank_lines = trailing_blank_lines + i32::from(self.leading_blanks);
        penalty += TOTAL_BLANK_LINE_WEIGHT * total_blank_lines
            + trailing_blank_lines * TRAILING_BLANK_LINES_WEIGHT;
        let indent = if self.indent == INDENT_BLANK {
            self.next_indent
        } else {
            self.indent
        };
        if indent != INDENT_BLANK && self.previous_indent != INDENT_BLANK {
            let blank = total_blank_lines != 0;
            penalty += match indent.cmp(&self.previous_indent) {
                Ordering::Equal => 0,
                Ordering::Less if self.next_indent <= indent => {
                    if blank {
                        RELATIVE_DEDENT_WITH_BLANK_PENALTY
                    } else {
                        RELATIVE_DEDENT_PENALTY
                    }
                }
                Ordering::Less => {
                    if blank {
                        RELATIVE_OUTDENT_WITH_BLANK_PENALTY
                    } else {
                        RELATIVE_OUTDENT_PENALTY
                    }
                }
                Ordering::Greater => {
                    if blank {
                        RELATIVE_INDENT_WITH_BLANK_PENALTY
                    } else {
                        RELATIVE_INDENT_PENALTY
                    }
                }
            };
        }
        Score {
            indent: if indent == INDENT_BLANK {
                -1
            } else {
                i32::from(indent)
            },
            penalty,
        }
    }
}

impl Score {
    fn is_improvement_over(self, previous: Score) -> bool {
        let indent_score = match previous.indent.cmp(&self.indent) {
            Ordering::Less => INDENT_WEIGHT,
            Ordering::Greater => -INDENT_WEIGHT,
            Ordering::Equal => 0,
        };
        indent_score + self.penalty - previous.penalty <= 0
    }
}

fn hunks(removed: &[bool], added: &[bool]) -> Vec<(Range<usize>, Range<usize>)> {
    let take_run = |changes: &[bool], index: &mut usize| {
        let run = changes[*index..]
            .iter()
            .take_while(|&&changed| changed)
            .count();
        *index += run;
        if *index < changes.len() {
            *index += 1;
        }
        run
    };
    let mut changes = Vec::new();
    let (mut index_removed, mut index_added) = (0, 0);
    let (mut position_before, mut position_after) = (0, 0);
    loop {
        let removed_run = take_run(removed, &mut index_removed);
        let added_run = take_run(added, &mut index_added);
        if removed_run != 0 || added_run != 0 {
            changes.push((
                position_before..position_before + removed_run,
                position_after..position_after + added_run,
            ));
            position_before += removed_run + 1;
            position_after += added_run + 1;
        } else if index_removed == removed.len() && index_added == added.len() {
            return changes;
        } else {
            position_before += 1;
            position_after += 1;
        }
    }
}
