use std::error::Error;

use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::{KeyCodeValue, SingleKey};
use yi_tui::plantree::{PlanTreeResult, PlanTreeView};
use yi_types::plan::doc::{
    AgentId, BlockedOn, Check, Delegation, GoalText, Plan, PlanId, PlanTier, RetryCount, SpawnSpec,
    Todo, TodoLabel, TodoState,
};

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn flat(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn key(code: KeyCodeValue) -> SingleKey {
    SingleKey {
        code,
        ctrl: false,
        alt: false,
        shift: false,
    }
}

fn todo(label: &str, state: TodoState, after: &[&str]) -> Result<Todo, Box<dyn Error>> {
    let mut after_labels = Vec::new();
    for label in after {
        after_labels.push(TodoLabel::new(*label)?);
    }
    Ok(Todo {
        label: TodoLabel::new(label)?,
        after: after_labels,
        state,
        delegation: None,
        subplan: None,
        retries: RetryCount(0),
        children: Vec::new(),
        note: None,
        attempt: yi_types::plan::doc::AttemptId::FIRST,
        refusals: 0,
        contract: None,
        contract_hash: None,
        extra: serde_json::Map::new(),
        cites: Default::default(),
        ask: None,
    })
}

fn delegated(label: &str) -> Result<Todo, Box<dyn Error>> {
    let mut todo = todo(label, TodoState::Pending, &[])?;
    todo.delegation = Some(Delegation {
        spec: SpawnSpec {
            role: Some("builder".to_owned()),
            model: None,
            effort: None,
            tools: Vec::new(),
            isolation: None,
            budget: None,
            wall: None,
            parent_close: None,
            extra: serde_json::Map::new(),
        },
        accept: Check::Command("just check".to_owned()),
        output: None,
        context: Vec::new(),
        note: None,
        extra: serde_json::Map::new(),
    });
    Ok(todo)
}

fn blocked(label: &str, on: BlockedOn, note: &str) -> Result<Todo, Box<dyn Error>> {
    todo(
        label,
        TodoState::Blocked {
            on,
            note: note.to_owned(),
        },
        &[],
    )
}

/// Every [`TodoState`] variant, every [`BlockedOn`] discriminant, a delegation
/// and a two-edge todo — the whole vocabulary one panel has to render.
fn plan() -> Result<Plan, Box<dyn Error>> {
    let todos = vec![
        todo("write the parser", TodoState::Pending, &[])?,
        todo(
            "run the suite",
            TodoState::Pending,
            &["write the parser", "wire the cli"],
        )?,
        todo(
            "wire the cli",
            TodoState::Running {
                by: AgentId::new("scout")?,
            },
            &[],
        )?,
        blocked(
            "await the child",
            BlockedOn::Child(AgentId::new("scout")?),
            "handing off",
        )?,
        blocked("ask the user", BlockedOn::User, "which database")?,
        blocked(
            "watch staging",
            BlockedOn::External { probe: None },
            "deploy pending",
        )?,
        blocked(
            "mystery",
            BlockedOn::Other("moon-phase".to_owned()),
            "unknown tag",
        )?,
        todo(
            "ship the docs",
            TodoState::Done {
                output: Some("plan://demo/ship-the-docs".parse()?),
                resolution: None,
            },
            &[],
        )?,
        todo(
            "port the mapper",
            TodoState::Failed {
                cause: "the fixture drifted".to_owned(),
                last: Some("agent://scout".parse()?),
            },
            &[],
        )?,
        todo("drop the flag", TodoState::Abandoned, &[])?,
        todo(
            "from the future",
            TodoState::Other("napping".to_owned()),
            &[],
        )?,
        delegated("hand off the port")?,
    ];
    Ok(Plan::opening(
        PlanId::new("demo")?,
        GoalText::new("prove the panel")?,
        PlanTier::Root,
        todos,
    ))
}

#[test]
fn panel_renders_every_state_glyph_and_the_dag_edges() -> TestResult {
    let view = PlanTreeView::new(&plan()?, &[]);
    let text = flat(&view.lines(100, &theme(), 40)).join("\n");
    for glyph in ['☐', '◆', '☑', '✗', '?'] {
        assert!(text.contains(glyph), "glyph {glyph} is missing:\n{text}");
    }
    assert!(
        text.contains("after: write the parser, wire the cli"),
        "the edges of a gated todo are readable:\n{text}"
    );
    for detail in [
        "by scout",
        "child scout",
        "user",
        "external",
        "moon-phase",
        "plan://demo/ship-the-docs",
        "the fixture drifted",
        "last: agent://scout",
        "→ builder",
        "just check",
        "napping",
    ] {
        assert!(text.contains(detail), "detail {detail} is missing:\n{text}");
    }
    assert!(
        text.contains("demo v1") && text.contains("prove the panel") && text.contains("active"),
        "the plan header names the plan, its version, goal, and state:\n{text}"
    );
    assert!(
        text.contains("↑↓ move · type to filter · esc close"),
        "the legend row is present:\n{text}"
    );
    Ok(())
}

#[test]
fn a_ready_pending_todo_is_accented_and_a_gated_one_is_not() -> TestResult {
    let theme = theme();
    let view = PlanTreeView::new(&plan()?, &[]);
    let lines = view.lines(100, &theme, 40);
    let accent_of = |needle: &str| {
        lines.iter().find_map(|line| {
            let span = line
                .spans
                .iter()
                .find(|span| span.content.as_ref() == needle)?;
            Some(span.style.fg == theme.accent_style().fg)
        })
    };
    assert_eq!(
        accent_of("write the parser"),
        Some(true),
        "a ready pending todo carries the accent"
    );
    assert_eq!(
        accent_of("run the suite"),
        Some(false),
        "a pending todo behind an uncleared edge does not"
    );
    Ok(())
}

#[test]
fn a_long_detail_truncates_inside_the_panel_instead_of_wrapping() -> TestResult {
    let mut plan = plan()?;
    plan.todos = vec![todo(
        "port the mapper",
        TodoState::Failed {
            cause: "the fixture drifted a very long way from what the mapper emits today"
                .to_owned(),
            last: None,
        },
        &[],
    )?];
    let view = PlanTreeView::new(&plan, &[]);
    for width in [30_usize, 44, 100] {
        let lines = view.lines(width, &theme(), 40);
        for line in flat(&lines) {
            assert!(
                UnicodeWidthStr::width(line.as_str()) <= width,
                "row {line:?} is wider than the {width}-column panel"
            );
        }
        assert_eq!(
            flat(&lines).len(),
            flat(&view.lines(100, &theme(), 40)).len(),
            "a narrow panel truncates its detail rather than wrapping onto new rows"
        );
    }
    Ok(())
}

#[test]
fn keys_move_filter_and_close_the_panel() -> TestResult {
    let mut view = PlanTreeView::new(&plan()?, &[]);
    let start = view.selected;
    assert!(matches!(
        view.handle_key(&key(KeyCodeValue::Down)),
        PlanTreeResult::Open
    ));
    assert_ne!(view.selected, start, "down moves the selection");
    let _ = view.handle_key(&key(KeyCodeValue::Up));
    assert_eq!(view.selected, start, "up returns to the same row");
    let all = flat(&view.lines(100, &theme(), 40)).len();
    for c in "mapper".chars() {
        let _ = view.handle_key(&key(KeyCodeValue::Char(c)));
    }
    let filtered = flat(&view.lines(100, &theme(), 40));
    assert!(
        filtered.len() < all,
        "typing filters the rows down: {filtered:?}"
    );
    assert!(
        filtered.iter().any(|line| line.contains("port the mapper")),
        "the matching todo survives the filter: {filtered:?}"
    );
    for _ in 0.."mapper".len() {
        let _ = view.handle_key(&key(KeyCodeValue::Backspace));
    }
    assert_eq!(
        flat(&view.lines(100, &theme(), 40)).len(),
        all,
        "backspace restores every row"
    );
    assert!(matches!(
        view.handle_key(&key(KeyCodeValue::Esc)),
        PlanTreeResult::Close
    ));
    Ok(())
}

#[test]
fn a_sub_plan_nests_under_the_todo_that_owns_it() -> TestResult {
    let mut root = plan()?;
    let owner = TodoLabel::new("hand off the port")?;
    let sub = Plan::opening(
        PlanId::new("demo.hand-off-the-port")?,
        GoalText::new("port the mapper")?,
        PlanTier::Sub {
            parent: yi_types::plan::doc::TodoAddr {
                plan: PlanId::new("demo")?,
                todo: owner.clone(),
            },
        },
        vec![todo("read the span", TodoState::Pending, &[])?],
    );
    if let Some(todo) = root.todos.iter_mut().find(|todo| todo.label == owner) {
        todo.subplan = Some(PlanId::new("demo.hand-off-the-port")?);
    }
    let view = PlanTreeView::new(&root, std::slice::from_ref(&sub));
    let lines = flat(&view.lines(100, &theme(), 60));
    let owner_row = lines
        .iter()
        .position(|line| line.contains("hand off the port"))
        .ok_or("the owning todo renders")?;
    let sub_row = lines
        .iter()
        .position(|line| line.contains("demo.hand-off-the-port"))
        .ok_or("the sub-plan header renders")?;
    assert!(
        sub_row > owner_row,
        "the sub-plan follows its owner: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("read the span")),
        "the sub-plan's own todos render: {lines:?}"
    );
    Ok(())
}

#[test]
fn children_render_under_their_parent_one_level_deeper() -> TestResult {
    let mut parent = todo("rebase", TodoState::Pending, &[])?;
    let mut child = todo(
        "remap",
        TodoState::Done {
            output: None,
            resolution: None,
        },
        &[],
    )?;
    child.children = vec![todo("rule one", TodoState::Pending, &[])?];
    parent.children = vec![child, todo("prompt", TodoState::Pending, &[])?];
    let plan = Plan::opening(
        PlanId::new("nested")?,
        GoalText::new("nest")?,
        PlanTier::Root,
        vec![parent],
    );
    let view = PlanTreeView::new(&plan, &[]);
    let lines = flat(&view.lines(100, &theme(), 40));
    let text = lines.join("\n");
    assert!(text.contains("remap"), "{text}");
    assert!(text.contains("rule one"), "{text}");
    let indent = |needle: &str| {
        lines
            .iter()
            .find(|line| line.contains(needle))
            .map(|line| line.find(needle).unwrap_or(0))
            .unwrap_or(0)
    };
    assert!(indent("rebase") < indent("remap"), "{text}");
    assert!(indent("remap") < indent("rule one"), "{text}");
    assert_eq!(indent("remap"), indent("prompt"), "{text}");
    Ok(())
}
