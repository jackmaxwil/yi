//! A todo written as text past the label cap keeps a label of its own: two long todos that share
//! their first 80 chars are two rows, not one the plan validator refuses as a duplicate.
use std::error::Error;

use yi_types::plan::doc::TODO_LABEL_MAX;
use yi_types::plan::doc::Todo;
use yi_types::todo::{PhaseName, TodoList, TodoPhase};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn two_long_todos_sharing_a_head_keep_distinct_labels() -> TestResult {
    let head = "read the record (architecture, design doc, git log, changelog, gate recipe, last";
    let texts = [
        format!("{head} merges and guardrails)"),
        format!("{head} merged pull requests)"),
    ];
    let items: Vec<Todo> = texts
        .iter()
        .map(|text| Todo::from_text(text))
        .collect::<Result<_, _>>()?;
    let labels: Vec<String> = items.iter().map(|item| item.label.to_string()).collect();
    assert_ne!(labels[0], labels[1], "{labels:?}");
    assert!(
        labels
            .iter()
            .all(|label| label.chars().count() <= TODO_LABEL_MAX),
        "{labels:?}"
    );
    assert!(
        items
            .iter()
            .all(|item| item.is_cut() && item.label.as_str().starts_with("read the record"))
    );
    let list = TodoList {
        phases: vec![TodoPhase {
            name: PhaseName::new("Tasks")?,
            items,
            extra: Default::default(),
        }],
        ..TodoList::default()
    };
    assert!(list.duplicates().is_empty(), "{:?}", list.duplicates());
    let again = Todo::from_text(&texts[0])?;
    assert_eq!(
        again.label.as_str(),
        labels[0],
        "the same text names the same row"
    );
    Ok(())
}

#[test]
fn a_todo_at_the_label_cap_is_its_own_label() -> TestResult {
    let at: String = "é".repeat(TODO_LABEL_MAX);
    let item = Todo::from_text(&at)?;
    assert_eq!((item.label.as_str(), item.is_cut()), (at.as_str(), false));
    Ok(())
}
