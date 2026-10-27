pub(crate) use yi_tools::cut_row;

pub(crate) fn first(items: &[String], limit: usize) -> String {
    let head: Vec<&str> = items.iter().take(limit).map(String::as_str).collect();
    head.join(", ")
}

pub(crate) fn listed(items: &[String], what: &str, cap: &str, limit: usize, next: &str) -> String {
    let head = first(items, limit);
    if items.len() <= limit {
        return head;
    }
    format!(
        "{head} {}",
        cut_row(limit, items.len(), what, cap, limit, next)
    )
}
