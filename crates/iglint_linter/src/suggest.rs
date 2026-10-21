//! "Did you mean" suggestions for misspelled names.

/// Returns the candidate closest to `input` by edit distance, if it is close
/// enough to be a plausible typo. Comparison ignores ASCII case.
pub fn closest<'a>(input: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let input_lower = input.to_ascii_lowercase();
    let limit = (input.chars().count() / 3).max(1);
    let mut best: Option<(usize, &'a str)> = None;
    for candidate in candidates {
        let distance = levenshtein(&input_lower, &candidate.to_ascii_lowercase());
        if distance <= limit && best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// Edit distance counting insertions, deletions, substitutions and
/// transpositions of adjacent characters.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev2: Vec<usize> = vec![0; b.len() + 1];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut value = (prev[j] + 1)
                .min(current[j - 1] + 1)
                .min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                value = value.min(prev2[j - 2] + 1);
            }
            current[j] = value;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut current);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        assert_eq!(levenshtein("select", "select"), 0);
        assert_eq!(levenshtein("selcet", "select"), 1);
        assert_eq!(levenshtein("selct", "select"), 1);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn closest_candidate() {
        let keys = ["select", "extend-select", "ignore", "exclude"];
        assert_eq!(closest("selct", keys), Some("select"));
        assert_eq!(closest("IGNORE", keys), Some("ignore"));
        assert_eq!(closest("totally-different", keys), None);
    }
}
