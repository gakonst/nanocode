const SIMILARITY_MIN: f64 = 0.7;

pub(crate) fn jaro(left: &str, right: &str) -> f64 {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let search_range = (left.len().max(right.len()) / 2).saturating_sub(1);
    let mut left_flags = vec![false; left.len()];
    let mut right_flags = vec![false; right.len()];
    let mut matches = 0_usize;
    for (index, character) in left.iter().enumerate() {
        let start = index.saturating_sub(search_range);
        let end = right.len().min(index + search_range + 1);
        for position in start..end {
            if *character == right[position] && !right_flags[position] {
                left_flags[index] = true;
                right_flags[position] = true;
                matches += 1;
                break;
            }
        }
    }
    if matches == 0 {
        return 0.0;
    }
    let mut transpositions = 0_usize;
    let mut matched_right = right
        .iter()
        .zip(&right_flags)
        .filter(|(_, flag)| **flag)
        .map(|(character, _)| character);
    for (character, _) in left.iter().zip(&left_flags).filter(|(_, flag)| **flag) {
        let other = matched_right
            .next()
            .expect("both sides hold the same number of matched characters");
        if character != other {
            transpositions += 1;
        }
    }
    transpositions /= 2;
    ((matches as f64 / left.len() as f64)
        + (matches as f64 / right.len() as f64)
        + ((matches - transpositions) as f64 / matches as f64))
        / 3.0
}

pub(crate) fn did_you_mean<'a>(
    value: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut ranked: Vec<(f64, String)> = Vec::new();
    for candidate in candidates {
        let confidence = jaro(value, candidate);
        if confidence > SIMILARITY_MIN {
            let position = ranked.partition_point(|(other, _)| *other <= confidence);
            ranked.insert(position, (confidence, candidate.to_owned()));
        }
    }
    ranked.into_iter().map(|(_, candidate)| candidate).collect()
}
