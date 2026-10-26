/// The common prefix length and bounded common suffix length of two sequences.
#[must_use]
pub fn changed_edges<T: PartialEq>(before: &[T], after: &[T]) -> (usize, usize) {
    let head = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    let rest = before.len().min(after.len()).saturating_sub(head);
    let tail = before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(rest);
    (head, tail)
}
