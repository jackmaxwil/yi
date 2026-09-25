use yi_types::message::Usage;

/// Usage-record cause naming a child-usage attribution (design P14).
pub const CHILD_USAGE_CAUSE: &str = "child_usage_attributed";

fn add_component(target: &mut i64, delta: i64) {
    *target = target.saturating_add(delta);
}

/// The child's billable usage folds into the parent's totals but its `total_tokens` is
/// preserved: child work affects cost, never the parent's context accounting.
pub fn attribute_child_usage(parent: &mut Usage, child: &Usage) {
    let parent_context_tokens = if parent.total_tokens != 0 {
        parent.total_tokens
    } else {
        parent
            .input
            .saturating_add(parent.output)
            .saturating_add(parent.cache_read)
            .saturating_add(parent.cache_write)
    };
    add_component(&mut parent.input, child.input);
    add_component(&mut parent.output, child.output);
    add_component(&mut parent.cache_read, child.cache_read);
    add_component(&mut parent.cache_write, child.cache_write);
    let add_cost = |target: &mut serde_json::Number, delta: &serde_json::Number| {
        let sum = target.as_f64().unwrap_or(0.0) + delta.as_f64().unwrap_or(0.0);
        if let Some(number) = serde_json::Number::from_f64(sum) {
            *target = number;
        }
    };
    add_cost(&mut parent.cost.input, &child.cost.input);
    add_cost(&mut parent.cost.output, &child.cost.output);
    add_cost(&mut parent.cost.cache_read, &child.cost.cache_read);
    add_cost(&mut parent.cost.cache_write, &child.cost.cache_write);
    add_cost(&mut parent.cost.total, &child.cost.total);
    parent.total_tokens = parent_context_tokens;
}
