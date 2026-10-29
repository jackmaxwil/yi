use yi_types::plan::doc::{TODO_LABEL_MAX, Todo};
use yi_types::todo::TodoList;

use super::{PREFIX_MIN, TodoError};

const LIST_ROWS: usize = super::LIST_ROWS;

/// Invariant: a label is resolved by normalised prefix, and ambiguity is refused rather than
/// guessed, because two rows that share a head are two different pieces of work.
pub(super) fn named(item: &Todo) -> String {
    match &item.id {
        Some(id) => format!("{id} {:?}", item.label.as_str()),
        None => format!("{:?}", item.label.as_str()),
    }
}

pub(super) fn rows(names: &[String], what: &str) -> String {
    let view = "todo {\"op\": \"view\"}";
    crate::listed::listed(names, what, "LIST_ROWS", LIST_ROWS, view)
}

pub(super) fn known_labels(list: &TodoList) -> String {
    let labels: Vec<String> = list.items().map(named).collect();
    if labels.is_empty() {
        "nothing".to_owned()
    } else {
        rows(&labels, "todos")
    }
}

pub(super) fn known_phases(list: &TodoList) -> String {
    let names: Vec<String> = list
        .phases
        .iter()
        .map(|phase| format!("{:?}", phase.name.as_str()))
        .collect();
    if names.is_empty() {
        "nothing".to_owned()
    } else {
        rows(&names, "phases")
    }
}
pub(super) fn normalized(text: &str) -> String {
    let stripped = text
        .trim()
        .trim_end_matches('…')
        .trim_end_matches("...")
        .trim();
    stripped
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('`', "")
        .to_lowercase()
}

/// Exact label; label or id with backticks stripped; an id and its own label (`t1 read the
/// spec`); the same label [`normalized`]; a unique case-insensitive prefix of [`PREFIX_MIN`].
pub(crate) fn locate(list: &TodoList, needle: &str) -> Result<usize, TodoError> {
    let items: Vec<&Todo> = list.items().collect();
    // A cut row is also named by the text it was cut from, whose head its label may not be.
    fn text(item: &Todo) -> &str {
        (item.note.as_ref().filter(|_| item.is_cut())).map_or(item.label.as_str(), |n| n.as_str())
    }
    if let Some(index) = items.iter().position(|item| item.label.as_str() == needle) {
        return Ok(index);
    }
    let bare = needle.trim().trim_matches('`').trim();
    if let Some(index) = items.iter().position(|item| {
        item.label.as_str() == bare
            || item
                .id
                .as_ref()
                .is_some_and(|id: &yi_types::todo::TodoId| id.as_str() == bare)
    }) {
        return Ok(index);
    }
    if let Some((id, rest)) = bare.split_once(' ')
        && let Some(index) = items.iter().position(|item| {
            let (label, rest) = (normalized(text(item)), normalized(rest));
            let same = !rest.is_empty() && (label.starts_with(&rest) || rest.starts_with(&label));
            same && item.id.as_ref().is_some_and(|own| own.as_str() == id)
        })
    {
        return Ok(index);
    }
    let wanted = normalized(needle);
    if !wanted.is_empty() {
        let same: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                [item.label.as_str(), text(item)]
                    .map(normalized)
                    .contains(&wanted)
            })
            .map(|(index, _)| index)
            .collect();
        if let [one] = same.as_slice() {
            return Ok(*one);
        }
    }
    // A whole text whose digest named no row (retyped in another case) still names it by head.
    let digest = |tail: &str| tail.len() == 8 && tail.bytes().all(|byte| byte.is_ascii_hexdigit());
    let long = bare.chars().count() > TODO_LABEL_MAX.saturating_sub(9);
    let tail = bare
        .rsplit_once(' ')
        .filter(|(_, tail)| long && digest(tail));
    let bare = tail.map_or(bare, |(head, _)| head);
    let prefix = bare.to_lowercase();
    let hits: Vec<usize> = if bare.chars().count() >= PREFIX_MIN {
        items
            .iter()
            .enumerate()
            .filter(|(_, item)| text(item).to_lowercase().starts_with(&prefix))
            .map(|(index, _)| index)
            .collect()
    } else {
        Vec::new()
    };
    match hits.as_slice() {
        [one] => Ok(*one),
        [] => Err(TodoError::NoSuchLabel {
            label: needle.to_owned(),
            known: known_labels(list),
        }),
        many => {
            let hits = many.iter().filter_map(|index| items.get(*index));
            let names: Vec<String> = hits.map(|item| named(item)).collect();
            Err(TodoError::Ambiguous {
                needle: needle.to_owned(),
                candidates: rows(&names, "todos"),
            })
        }
    }
}
