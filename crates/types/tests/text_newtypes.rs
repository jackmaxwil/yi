//! Every bounded text on the plan and todo wire refuses the same inputs with the same sentence
//! the model reads, whichever code generates the type.

use std::error::Error;

use yi_types::plan::ids::{
    GOAL_TEXT_MAX, GoalText, INLINE_NOTE_MAX_BYTES, INTENT_MAX_BYTES, InlineNote, Intent,
    NOTE_MAX_BYTES, Note, ProbeCommand, TODO_LABEL_MAX, TodoLabel,
};
use yi_types::plan::ledger::{EffectId, ID_MAX_BYTES, RequestId};
use yi_types::todo::{PHASE_NAME_MAX, PhaseName};

type TestResult = Result<(), Box<dyn Error>>;
type Make<'a> = &'a dyn Fn(&str) -> String;

fn verdict<T, E: std::fmt::Display>(made: Result<T, E>, text: impl Fn(&T) -> String) -> String {
    match made {
        Ok(value) => format!("ok {}", text(&value)),
        Err(error) => format!("err {error}"),
    }
}

/// A char cap counted on a non-ASCII string: one `é` is one char and two bytes.
#[test]
fn char_capped_lines_refuse_blank_long_and_multiline_text() -> TestResult {
    let at_cap = "é".repeat(TODO_LABEL_MAX);
    let over = "é".repeat(TODO_LABEL_MAX + 1);
    let label = |text: &str| verdict(TodoLabel::new(text), |label| label.as_str().to_owned());
    assert_eq!(label("  "), "err todo label is empty");
    assert_eq!(label(&at_cap), format!("ok {at_cap}"));
    assert_eq!(
        label(&over),
        format!("err todo label {over:?} exceeds {TODO_LABEL_MAX} chars")
    );
    assert_eq!(label("a\nb"), "err todo label \"a\\nb\" contains a newline");
    assert_eq!(label(" padded "), "ok  padded ");

    let goal = |text: &str| verdict(GoalText::new(text), |goal| goal.to_string());
    let over = "g".repeat(GOAL_TEXT_MAX + 1);
    assert_eq!(goal(""), "err goal text is empty");
    assert_eq!(
        goal(&"g".repeat(GOAL_TEXT_MAX)),
        format!("ok {}", "g".repeat(GOAL_TEXT_MAX))
    );
    assert_eq!(
        goal(&over),
        format!("err goal text {over:?} exceeds {GOAL_TEXT_MAX} chars")
    );
    assert_eq!(goal("a\rb"), "err goal text \"a\\rb\" contains a newline");

    let phase = |text: &str| verdict(PhaseName::new(text), |phase| phase.to_string());
    let over = "p".repeat(PHASE_NAME_MAX + 1);
    assert_eq!(phase(" Build "), "ok Build", "a phase name is trimmed");
    assert_eq!(phase(" "), "err todo label is empty");
    assert_eq!(
        phase(&format!(" {over} ")),
        format!("err todo label {over:?} exceeds {PHASE_NAME_MAX} chars")
    );

    let probe = |text: &str| verdict(ProbeCommand::new(text), |probe| probe.as_str().to_owned());
    assert_eq!(probe(" "), "err probe command is empty");
    assert_eq!(
        probe("a\nb"),
        "err probe command \"a\\nb\" contains a newline"
    );
    assert_eq!(
        probe(&"x".repeat(10_000)).len(),
        10_003,
        "a probe has no length cap"
    );
    Ok(())
}

#[test]
fn byte_capped_texts_refuse_blank_and_long_text_and_keep_newlines() -> TestResult {
    let cases: [(&str, usize, Make); 3] = [
        ("inline note", INLINE_NOTE_MAX_BYTES, &|text| {
            verdict(InlineNote::new(text), |note| note.as_str().to_owned())
        }),
        ("intent", INTENT_MAX_BYTES, &|text| {
            verdict(Intent::new(text), |intent| intent.as_str().to_owned())
        }),
        ("note", NOTE_MAX_BYTES, &|text| {
            verdict(Note::new(text), |note| note.as_str().to_owned())
        }),
    ];
    for (what, cap, make) in cases {
        let at_cap = "é".repeat(cap / 2);
        assert_eq!(make(&at_cap), format!("ok {at_cap}"), "{what}");
        let over = format!("{at_cap}x");
        assert!(make(&over).starts_with("err "), "{what}: {}", make(&over));
        assert!(
            make(&over).contains(&format!("{} bytes", cap + 1)),
            "{what}: {}",
            make(&over)
        );
        assert!(
            make("\t").starts_with("err ") && make("\t").contains("empty"),
            "{what}"
        );
        assert_eq!(make("a\nb"), "ok a\nb", "{what} keeps a newline");
    }
    Ok(())
}

#[test]
fn journal_ids_refuse_blank_long_and_spaced_text() -> TestResult {
    let request = |text: &str| verdict(RequestId::new(text), |id| id.to_string());
    let effect = |text: &str| verdict(EffectId::new(text), |id| id.to_string());
    assert_eq!(request("r-1"), "ok r-1");
    assert_eq!(request(""), "err request id is empty");
    assert_eq!(request("a b"), "err request id \"a b\" contains whitespace");
    let over = "e".repeat(ID_MAX_BYTES + 1);
    assert_eq!(
        effect(&over),
        format!(
            "err effect id of {} bytes exceeds {ID_MAX_BYTES}",
            ID_MAX_BYTES + 1
        )
    );
    assert!(RequestId::new("a")? < RequestId::new("b")?, "ids order");
    Ok(())
}
