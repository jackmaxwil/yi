pub(super) fn hits_in(
    matcher: &regex::Regex,
    normalized: &str,
    multiline: bool,
    def: bool,
) -> Vec<usize> {
    let mut hits: Vec<usize> = if multiline {
        let newlines: Vec<usize> = normalized.match_indices('\n').map(|(at, _)| at).collect();
        matcher
            .find_iter(normalized)
            .map(|found| newlines.partition_point(|at| *at < found.start()))
            .collect()
    } else {
        normalized
            .split('\n')
            .enumerate()
            .filter(|(_, line)| matcher.is_match(line))
            .map(|(index, _)| index)
            .collect()
    };
    hits.dedup();
    if def {
        let lines: Vec<&str> = normalized.split('\n').collect();
        hits.retain(|index| {
            lines
                .get(*index)
                .is_some_and(|line| crate::orient::is_decl(line))
        });
    }
    hits
}
