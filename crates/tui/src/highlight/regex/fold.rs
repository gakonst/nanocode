use super::{Class, Error, Node, class::CodePointSet, unicode};
use std::sync::OnceLock;

struct Group<const LENGTH: usize> {
    fold: [u32; LENGTH],
    unfolds: Vec<u32>,
}

struct Groups<const LENGTH: usize> {
    ordered: Vec<Group<LENGTH>>,
    by_fold: Vec<usize>,
}

struct Tables {
    one: Groups<1>,
    two: Groups<2>,
    three: Groups<3>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| Tables {
        one: groups::<1>(),
        two: groups::<2>(),
        three: groups::<3>(),
    })
}

fn groups<const LENGTH: usize>() -> Groups<LENGTH> {
    let mut pairs: Vec<([u32; LENGTH], u32)> = unicode::CASE_FOLDING
        .iter()
        .filter_map(|&(unfold, fold)| Some((<[u32; LENGTH]>::try_from(fold).ok()?, unfold)))
        .collect();
    pairs.sort_unstable();
    let mut groups: Vec<Group<LENGTH>> = Vec::new();
    for (fold, unfold) in pairs {
        match groups.last_mut() {
            Some(group) if group.fold == fold => group.unfolds.push(unfold),
            _ => groups.push(Group {
                fold,
                unfolds: vec![unfold],
            }),
        }
    }
    let turkic = |group: &Group<LENGTH>| {
        group
            .unfolds
            .iter()
            .any(|unfold| unicode::TURKIC_UNFOLDS.contains(unfold))
    };
    groups.sort_by(|left, right| {
        turkic(left)
            .cmp(&turkic(right))
            .then(left.fold.cmp(&right.fold))
    });
    for group in &groups {
        assert!(
            group.unfolds.windows(2).all(|pair| pair[0] < pair[1]),
            "case folding lists its unfolds in code point order"
        );
    }
    let mut by_fold: Vec<usize> = (0..groups.len()).collect();
    by_fold.sort_unstable_by_key(|&index| groups[index].fold);
    Groups {
        ordered: groups,
        by_fold,
    }
}

fn unfold_key(code: u32) -> Option<&'static [u32]> {
    unicode::CASE_FOLDING
        .binary_search_by_key(&code, |&(unfold, _)| unfold)
        .ok()
        .map(|index| unicode::CASE_FOLDING[index].1)
}

fn find<const LENGTH: usize>(groups: &Groups<LENGTH>, fold: [u32; LENGTH]) -> Option<&[u32]> {
    groups
        .by_fold
        .binary_search_by_key(&fold, |&index| groups.ordered[index].fold)
        .ok()
        .map(|position| groups.ordered[groups.by_fold[position]].unfolds.as_slice())
}

fn one_unfolds(code: u32) -> Option<&'static [u32]> {
    find(&tables().one, [code])
}

fn simple_fold(code: u32) -> u32 {
    match unfold_key(code) {
        Some([fold]) => *fold,
        _ => code,
    }
}

fn with_unfolds(code: u32) -> Vec<u32> {
    let mut codes = vec![code];
    if let Some(unfolds) = one_unfolds(code) {
        codes.extend_from_slice(unfolds);
    }
    codes
}

struct Item {
    consumed: usize,
    codes: Vec<u32>,
}

fn items_by_str(pattern: &[u32]) -> Vec<Item> {
    let tables = tables();
    let mut items = Vec::new();
    let original = pattern;
    let first_key = unfold_key(original[0]);
    let mut codes = [simple_fold(original[0]), 0, 0];
    if original.len() > 1 {
        codes[1] = simple_fold(original[1]);
        if original.len() > 2 {
            codes[2] = simple_fold(original[2]);
            if let Some(unfolds) = find(&tables.three, codes) {
                for &unfold in unfolds {
                    items.push(Item {
                        consumed: 3,
                        codes: vec![unfold],
                    });
                }
                let choices = codes.map(with_unfolds);
                for &first in &choices[0] {
                    for &second in &choices[1] {
                        for &third in &choices[2] {
                            if [first, second, third] == original[..3] {
                                continue;
                            }
                            items.push(Item {
                                consumed: 3,
                                codes: vec![first, second, third],
                            });
                        }
                    }
                }
                return items;
            }
        }
        if let Some(unfolds) = find(&tables.two, [codes[0], codes[1]]) {
            for &unfold in unfolds {
                items.push(Item {
                    consumed: 2,
                    codes: vec![unfold],
                });
            }
            let choices = [with_unfolds(codes[0]), with_unfolds(codes[1])];
            for &first in &choices[0] {
                for &second in &choices[1] {
                    if [first, second] == original[..2] {
                        continue;
                    }
                    items.push(Item {
                        consumed: 2,
                        codes: vec![first, second],
                    });
                }
            }
            return items;
        }
    }
    match first_key {
        Some(&[fold]) => {
            items.push(Item {
                consumed: 1,
                codes: vec![fold],
            });
            for &unfold in one_unfolds(fold).unwrap_or_default() {
                if unfold != original[0] {
                    items.push(Item {
                        consumed: 1,
                        codes: vec![unfold],
                    });
                }
            }
        }
        Some(&[first, second]) => {
            for &unfold in find(&tables.two, [first, second]).unwrap_or_default() {
                if unfold != original[0] {
                    items.push(Item {
                        consumed: 1,
                        codes: vec![unfold],
                    });
                }
            }
            let choices = [with_unfolds(first), with_unfolds(second)];
            for &a in &choices[0] {
                for &b in &choices[1] {
                    items.push(Item {
                        consumed: 1,
                        codes: vec![a, b],
                    });
                }
            }
        }
        Some(&[first, second, third]) => {
            for &unfold in find(&tables.three, [first, second, third]).unwrap_or_default() {
                if unfold != original[0] {
                    items.push(Item {
                        consumed: 1,
                        codes: vec![unfold],
                    });
                }
            }
            let choices = [
                with_unfolds(first),
                with_unfolds(second),
                with_unfolds(third),
            ];
            for &a in &choices[0] {
                for &b in &choices[1] {
                    for &c in &choices[2] {
                        items.push(Item {
                            consumed: 1,
                            codes: vec![a, b, c],
                        });
                    }
                }
            }
        }
        Some(_) => unreachable!("case folding maps to one to three code points"),
        None => {
            for &unfold in one_unfolds(original[0]).unwrap_or_default() {
                items.push(Item {
                    consumed: 1,
                    codes: vec![unfold],
                });
            }
        }
    }
    items
}

fn code_string(codes: &[u32]) -> String {
    codes
        .iter()
        .map(|&code| char::from_u32(code).expect("case folding yields scalar values"))
        .collect()
}

const fn plain(text: String) -> Node {
    Node::String {
        text,
        ignore_case: false,
        expanded: false,
    }
}

fn class_of(codes: impl IntoIterator<Item = u32>) -> Node {
    let mut set = CodePointSet::new();
    for code in codes {
        set.add(code);
    }
    Node::Class(Class {
        set,
        negated: false,
    })
}

pub(super) fn unravel(text: &str, in_look_behind: bool) -> Result<Node, Error> {
    let pattern: Vec<u32> = text.chars().map(u32::from).collect();
    let mut list: Vec<Node> = Vec::new();
    let mut current: Option<usize> = None;
    let mut position = 0;
    let add_string = |list: &mut Vec<Node>, current: &mut Option<usize>, codes: &[u32]| {
        if let Some(index) = *current
            && let Node::String { text, .. } = &mut list[index]
        {
            text.push_str(&code_string(codes));
            return;
        }
        list.push(plain(code_string(codes)));
        *current = Some(list.len() - 1);
    };
    while position < pattern.len() {
        let mut items = items_by_str(&pattern[position..]);
        let next;
        if items.is_empty() {
            next = position + 1;
            add_string(&mut list, &mut current, &pattern[position..next]);
        } else if in_look_behind {
            next = position + 1;
            if items[0].consumed != 1 {
                items = items_by_str(&pattern[position..next]);
            }
            let singles: Vec<u32> = items
                .iter()
                .filter(|item| item.consumed == 1 && item.codes.len() == 1)
                .map(|item| item.codes[0])
                .collect();
            if singles.is_empty() {
                add_string(&mut list, &mut current, &pattern[position..next]);
            } else {
                list.push(class_of(std::iter::once(pattern[position]).chain(singles)));
                current = None;
            }
        } else {
            let consumed = items[0].consumed;
            if items.iter().any(|item| item.consumed != consumed) {
                return Err(Error::Unsupported(
                    "case folding items of different lengths",
                ));
            }
            next = position + consumed;
            if items.iter().all(|item| item.codes.len() == 1) {
                list.push(class_of(
                    std::iter::once(pattern[position])
                        .chain(items.iter().map(|item| item.codes[0])),
                ));
            } else {
                let mut branches = vec![plain(code_string(&pattern[position..next]))];
                branches.extend(items.iter().map(|item| plain(code_string(&item.codes))));
                list.push(Node::Alternation(branches));
            }
            current = None;
        }
        position = next;
    }
    assert!(!list.is_empty(), "an ignore-case string is not empty");
    Ok(if list.len() == 1 {
        list.pop().expect("one node")
    } else {
        Node::Concat(list)
    })
}

fn expanded_sequence(fold: &[u32]) -> Node {
    let mut nodes: Vec<Node> = Vec::new();
    for &code in fold {
        match one_unfolds(code) {
            Some(unfolds) => nodes.push(class_of(
                unfolds.iter().copied().chain(std::iter::once(code)),
            )),
            None => {
                let character = char::from_u32(code).expect("case folding yields scalar values");
                if let Some(Node::String { text, .. }) = nodes.last_mut() {
                    text.push(character);
                } else {
                    nodes.push(Node::String {
                        text: character.to_string(),
                        ignore_case: false,
                        expanded: true,
                    });
                }
            }
        }
    }
    if nodes.len() == 1 {
        nodes.pop().expect("one node")
    } else {
        Node::Concat(nodes)
    }
}

fn close<const LENGTH: usize>(set: &mut CodePointSet, groups: &[Group<LENGTH>], with_fold: bool) {
    for group in groups {
        let members = || {
            group
                .unfolds
                .iter()
                .copied()
                .chain(with_fold.then_some(group.fold[0]))
        };
        if members().any(|code| set.contains(code)) {
            for code in members() {
                set.add(code);
            }
        }
    }
}

pub(super) fn fold_class(class: Class) -> Node {
    let tables = tables();
    let Class { mut set, negated } = class;
    close(&mut set, &tables.one.ordered, true);
    let mut branches = Vec::new();
    for group in &tables.two.ordered {
        let any = group.unfolds.iter().any(|&code| set.contains(code));
        if any && !negated {
            branches.push(expanded_sequence(&group.fold));
        }
    }
    close(&mut set, &tables.two.ordered, false);
    for group in &tables.three.ordered {
        let any = group.unfolds.iter().any(|&code| set.contains(code));
        if any && !negated {
            branches.push(expanded_sequence(&group.fold));
        }
    }
    close(&mut set, &tables.three.ordered, false);
    let class = Node::Class(Class { set, negated });
    if branches.is_empty() {
        class
    } else {
        branches.insert(0, class);
        Node::Alternation(branches)
    }
}
