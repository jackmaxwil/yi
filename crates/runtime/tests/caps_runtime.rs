//! Six runtime cuts that shrink what the model reads each name the cap that cut and the call
//! that gets the rest: `[… kept of total what (CAP limit); the rest: next]`. Each is tried at its
//! limit, where nothing is cut, and one past it.

use crate::scratch::Scratch;
use crate::support;
use crate::todo_mirror::{memory_store, session, wired};

use std::error::Error;
use std::sync::Arc;

use serde_json::{Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::ProviderStream;
use yi_runtime::environment::files_line;
use yi_runtime::family::{MemberState, MemberView, children_line};
use yi_runtime::todo::{Op, Target, TodoStore};
use yi_types::kernel::{KernelRestoreResult, KernelSnapshotSkip};
use yi_types::message::{AgentMessage, Content, StopReason};
use yi_types::plan::doc::{Todo, TodoLabel};
use yi_types::todo::PhaseName;

type TestResult = Result<(), Box<dyn Error>>;

struct Row<'a> {
    kept: usize,
    total: usize,
    what: &'a str,
    cap: &'a str,
    limit: usize,
    next: &'a str,
}

/// The first `[… kept of total what (CAP limit); the rest: next]` row in `text`, as its parts.
fn row_in(text: &str) -> Option<Row<'_>> {
    let inner = text.split_once("[\u{2026} ")?.1.split_once(']')?.0;
    let (head, next) = inner.split_once("; the rest: ")?;
    let (kept, rest) = head.split_once(" of ")?;
    let (total, rest) = rest.split_once(' ')?;
    let (what, cap) = rest.rsplit_once(" (")?;
    let (cap, limit) = cap.strip_suffix(')')?.rsplit_once(' ')?;
    Some(Row {
        kept: kept.parse().ok()?,
        total: total.parse().ok()?,
        what,
        cap,
        limit: limit.parse().ok()?,
        next,
    })
}

fn row_of(text: &str) -> Result<Row<'_>, String> {
    row_in(text).ok_or_else(|| format!("no cut row in: {text}"))
}

#[tokio::test]
async fn find_models_says_how_many_matches_the_limit_left_out() -> TestResult {
    let root = Scratch::new("yi-caps-find-models")?;
    unsafe { std::env::set_var("HOME", root.home()?) };
    for variable in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"] {
        unsafe { std::env::remove_var(variable) };
    }
    unsafe { std::env::set_var("OPENROUTER_API_KEY", "key-or") };
    let family = support::family(
        root.to_path_buf(),
        root.to_path_buf(),
        support::memory_store("root"),
        None,
    );
    let all = family.host.find_models("", 10_000);
    let total = all["models"].as_array().ok_or("no models list")?.len();
    assert!(total > 2, "the openrouter registry holds {total} models");
    assert!(!all.contains_key("cut"), "{all:?}");
    let whole = family.host.find_models("", total);
    assert!(!whole.contains_key("cut"), "{whole:?}");

    let short = family.host.find_models("", total - 1);
    let kept = short["models"].as_array().ok_or("no models list")?.len();
    assert_eq!(kept, total - 1);
    let text = (short.get("cut").and_then(Value::as_str)).ok_or("a cut listing carries no row")?;
    let row = row_of(text)?;
    assert_eq!(
        (row.kept, row.total, row.what, row.cap, row.limit),
        (total - 1, total, "matching models", "limit", total - 1)
    );
    assert_eq!(
        row.next,
        format!("await rlm.find_models(\"\", limit={total})")
    );
    Ok(())
}

fn member(name: &str, state: MemberState, note: Option<&str>) -> MemberView {
    MemberView {
        name: name.to_owned(),
        state,
        note: note.map(str::to_owned),
        tools: 0,
        tokens: 0,
        cost: 0.0,
        idle_s: 0,
        worktree: None,
    }
}

#[test]
fn the_children_row_names_the_members_it_left_out() -> TestResult {
    let running = |count: usize| -> Vec<MemberView> {
        (0..count)
            .map(|at| member(&format!("w{at}"), MemberState::Running, None))
            .collect()
    };
    let mut views = running(8);
    views.push(member(
        "asker",
        MemberState::NeedsYou,
        Some("asked a question"),
    ));
    let line = children_line(&views).ok_or("no row")?;
    assert!(
        !line.contains('\u{2026}'),
        "eight names a state is the limit: {line}"
    );
    assert!(line.contains("w7"), "{line}");

    views.insert(8, member("w8", MemberState::Running, None));
    let line = children_line(&views).ok_or("no row")?;
    assert!(
        line.contains("9 running (w0, w1, w2, w3, w4, w5, w6, w7)"),
        "{line}"
    );
    assert!(!line.contains("w8"), "{line}");
    assert!(
        line.contains("1 needs you (asker: asked a question)"),
        "{line}"
    );
    let row = row_of(&line)?;
    assert_eq!(
        (row.kept, row.total, row.what, row.cap, row.limit, row.next),
        (
            9,
            10,
            "children named per state",
            "CHILDREN_SHOWN",
            8,
            "await rlm.status()"
        )
    );
    Ok(())
}

fn todo_store(
    count: usize,
    label: impl Fn(usize) -> String,
) -> Result<Arc<TodoStore>, Box<dyn Error>> {
    let session = memory_store();
    let store = TodoStore::new(Arc::new(move || Some(session.clone())), "main");
    let items = (0..count)
        .map(|at| Ok(Todo::from_text(&label(at))?))
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    store.apply(
        Op::Init {
            phases: vec![(PhaseName::new("Tasks")?, items)],
        },
        None,
    )?;
    Ok(store)
}

/// A list an older binary wrote, with every row running: the only way `done` finds several.
fn running_rows(count: usize) -> Result<Arc<TodoStore>, Box<dyn Error>> {
    let session = memory_store();
    let rows: Vec<Value> = (0..count)
        .map(|at| {
            let id = format!("t{}", at + 1);
            json!({"id": id, "label": format!("check {at:02}"), "state": "running", "by": "main", "attempt": 1, "refusals": 0})
        })
        .collect();
    let list =
        json!({"format": 2, "phases": [{"name": "Tasks", "items": rows}], "nextId": count + 1});
    let record = json!({"op": "append", "actor": "main", "at": 1_790_552_725_363_u64, "touched": 2, "list": list});
    yi_session::lock_session(&session).append_custom("main", "todo", Some(record))?;
    Ok(TodoStore::new(
        Arc::new(move || Some(session.clone())),
        "main",
    ))
}

fn refusal(store: &TodoStore, op: Op) -> Result<String, Box<dyn Error>> {
    Ok(store
        .apply(op, None)
        .err()
        .ok_or("the op was taken")?
        .to_string())
}

#[test]
fn todo_error_lists_stop_at_the_cap_and_name_the_view_call() -> TestResult {
    let view = "todo {\"op\": \"view\"}";
    let nobody = || {
        Ok::<_, Box<dyn Error>>(Op::Start {
            label: TodoLabel::new("zzzzzzzzzz")?,
        })
    };
    for (count, cut) in [(12, false), (13, true)] {
        let store = todo_store(count, |at| format!("rename the module {at:02}"))?;

        let text = refusal(&store, nobody()?)?;
        assert!(text.contains("the list holds: "), "{text}");
        assert_eq!(row_in(&text).is_some(), cut, "{count} labels: {text}");

        let prefix = Op::Start {
            label: TodoLabel::new("rename the module")?,
        };
        let text = refusal(&store, prefix)?;
        assert!(text.contains("is a prefix of more than one todo"), "{text}");
        assert_eq!(row_in(&text).is_some(), cut, "{count} candidates: {text}");

        let done = Op::Done {
            target: Target::All,
            evidence: Some("`true` exit 0".to_owned()),
        };
        let text = running_rows(count)?
            .aim(done)
            .err()
            .ok_or("done took several")?
            .to_string();
        assert!(text.contains("several are running"), "{text}");
        assert_eq!(row_in(&text).is_some(), cut, "{count} running: {text}");

        let phase = Op::Drop {
            target: Target::Phase(PhaseName::new("Nowhere")?),
            reason: "gone".to_owned(),
        };
        let text = refusal(&store, phase)?;
        assert!(text.contains("the list holds: \"Tasks\""), "{text}");
        assert!(row_in(&text).is_none(), "one phase is not a cut: {text}");

        if cut {
            let text = refusal(&store, nobody()?)?;
            let row = row_of(&text)?;
            assert_eq!(
                (row.kept, row.total, row.what, row.cap, row.limit, row.next),
                (12, 13, "todos", "LIST_ROWS", 12, view)
            );
            assert!(text.contains("rename the module 11"), "{text}");
            assert!(!text.contains("rename the module 12"), "{text}");
        }
    }
    Ok(())
}

#[test]
fn a_phase_list_stops_at_the_cap_and_names_the_view_call() -> TestResult {
    for (count, cut) in [(12, false), (13, true)] {
        let session = memory_store();
        let store = TodoStore::new(Arc::new(move || Some(session.clone())), "main");
        let phases = (0..count)
            .map(|at| {
                Ok((
                    PhaseName::new(format!("Phase {at:02}"))?,
                    vec![Todo::from_text(&format!("step {at:02}"))?],
                ))
            })
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
        store.apply(Op::Init { phases }, None)?;
        let gone = Op::Drop {
            target: Target::Phase(PhaseName::new("Nowhere")?),
            reason: "gone".to_owned(),
        };
        let text = refusal(&store, gone)?;
        assert!(text.contains("the list holds: \"Phase 00\""), "{text}");
        assert_eq!(row_in(&text).is_some(), cut, "{count} phases: {text}");
        if cut {
            let row = row_of(&text)?;
            assert_eq!(
                (row.kept, row.total, row.what, row.cap, row.limit),
                (12, 13, "phases", "LIST_ROWS", 12)
            );
            assert_eq!(row.next, "todo {\"op\": \"view\"}");
            assert!(
                text.contains("Phase 11") && !text.contains("Phase 12"),
                "{text}"
            );
        }
    }
    Ok(())
}

fn call(id: &str, tool: &str, args: Value) -> Content {
    faux_tool_call(id, tool, args.as_object().cloned().unwrap_or_default())
}

fn calls(batch: Vec<Content>) -> AgentMessage {
    faux_assistant_message(batch, StopReason::ToolUse)
}

/// A draft whose numbered rows, one newline each, total exactly `bytes`.
fn draft_of_rows_totalling(bytes: usize) -> String {
    let (mut lines, mut used) = (Vec::<String>::new(), 0usize);
    loop {
        let prefix =
            yi_tools::hashline::format::format_numbered_lines("", lines.len() as u64 + 1).len();
        let left = bytes - used;
        if left <= 200 {
            lines.push("z".repeat(left - 1 - prefix));
            return lines.join("\n");
        }
        lines.push("y".repeat(60));
        used += prefix + 60 + 1;
    }
}

/// What the model read back from an `answer` refused for an open todo.
async fn refused_echo(root: &Scratch, draft: &str) -> Result<String, Box<dyn Error>> {
    let script = vec![
        calls(vec![call(
            "c1",
            "todo",
            json!({"op": "init", "items": ["write the note"]}),
        )]),
        calls(vec![call(
            "c2",
            "answer",
            json!({"summary": "Wrote it", "text": draft}),
        )]),
        calls(vec![call(
            "c3",
            "todo",
            json!({"op": "done", "label": "write the note", "evidence": "`true` exit 0"}),
        )]),
        calls(vec![call("c4", "answer", json!({"summary": "Wrote it"}))]),
        faux_assistant_message(vec![faux_text("nobody asked")], StopReason::Stop),
    ];
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(script);
    let mut session = session(Arc::clone(&provider));
    session.attach_store(memory_store())?;
    wired(&mut session, root, provider);
    session.prompt("write the note")?;
    session.wait_idle().await;
    (session.messages().iter())
        .filter_map(|message| match message {
            AgentMessage::ToolResult {
                tool_name,
                is_error: true,
                content,
                ..
            } if tool_name == "answer" => Some(yi_types::message::join_text(content, "")),
            _ => None,
        })
        .next()
        .ok_or_else(|| "the answer was not refused".into())
}

#[tokio::test]
async fn a_refused_answer_echoes_a_bounded_view_naming_the_held_file() -> TestResult {
    let held = |root: &Scratch| {
        root.join("rlm/family")
            .join(yi_tools::SPILLS)
            .join("answers/draft.md")
    };

    let root = Scratch::new("yi-caps-answer-at")?;
    let draft = draft_of_rows_totalling(4096);
    let echo = refused_echo(&root, &draft).await?;
    assert!(
        row_in(&echo).is_none(),
        "rows at the cap pass whole: {echo}"
    );
    assert!(
        echo.ends_with(draft.lines().last().unwrap_or_default()),
        "{echo}"
    );

    let root = Scratch::new("yi-caps-answer-over")?;
    let draft = draft_of_rows_totalling(4097);
    let lines = draft.lines().count();
    let echo = refused_echo(&root, &draft).await?;
    let row = row_of(&echo)?;
    let file = held(&root);
    assert_eq!(
        (row.kept, row.total, row.what, row.cap, row.limit),
        (
            lines - 1,
            lines,
            "draft lines shown",
            "ANSWER_ECHO_BYTES",
            4096
        )
    );
    assert_eq!(row.next, format!("read {} offset {lines}", file.display()));
    assert!(
        !echo.contains(&"z".repeat(10)),
        "the last row stayed out: {echo}"
    );
    assert_eq!(
        std::fs::read_to_string(&file)?.trim_end(),
        draft,
        "the file holds every line"
    );

    let root = Scratch::new("yi-caps-answer-one-long-row")?;
    let draft = "w".repeat(5000);
    let echo = refused_echo(&root, &draft).await?;
    let row = row_of(&echo)?;
    assert_eq!(
        (row.kept, row.total, row.next),
        (
            0,
            1,
            format!("read {} offset 1", held(&root).display()).as_str()
        )
    );
    assert!(!echo.contains(&"w".repeat(100)), "{} B echoed", echo.len());
    let lines: Vec<&str> = echo.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with("[\u{2026} 0 of 1"));
    let above = at.and_then(|at| lines.get(at.checked_sub(1)?));
    assert!(
        above.is_some_and(|line| !line.is_empty()),
        "no blank row above the cut row: {echo}"
    );

    let root = Scratch::new("yi-caps-answer-report")?;
    let report = (0..600)
        .map(|at| format!("Row {at}: the wall-clock figure for run {at} was {at}.5 ms."))
        .collect::<Vec<_>>()
        .join("\n");
    let echo = refused_echo(&root, &report).await?;
    let row = row_of(&echo)?;
    assert_eq!((row.total, row.cap), (600, "ANSWER_ECHO_BYTES"));
    assert!(row.kept > 20 && row.kept < 100, "{} lines kept", row.kept);
    assert!(
        echo.len() < report.len() / 2,
        "{} of {} bytes echoed",
        echo.len(),
        report.len()
    );
    Ok(())
}

fn revived(restored: usize, failed: usize) -> KernelRestoreResult {
    KernelRestoreResult {
        restored: (0..restored).map(|at| format!("var{at:03}")).collect(),
        failed: (0..failed)
            .map(|at| KernelSnapshotSkip {
                name: format!("gone{at:03}"),
                reason: "PicklingError".to_owned(),
            })
            .collect(),
        path: String::new(),
    }
}

#[test]
fn the_restore_notice_lists_a_bounded_number_of_names() -> TestResult {
    let whole = yi_runtime::restore_notice_text(&revived(40, 0));
    assert!(row_in(&whole).is_none(), "{whole}");
    assert!(whole.contains("var039."), "{whole}");

    let over = yi_runtime::restore_notice_text(&revived(41, 0));
    let row = row_of(&over)?;
    assert_eq!(
        (row.kept, row.total, row.what, row.cap, row.limit, row.next),
        (40, 41, "restored names", "RESTORE_NAMES_SHOWN", 40, "%who")
    );
    assert!(over.contains("var039"), "{over}");
    assert!(!over.contains("var040"), "{over}");

    let failed = yi_runtime::restore_notice_text(&revived(1, 100));
    assert!(
        failed.contains("gone099."),
        "names to recreate stay whole: {failed}"
    );
    assert!(row_in(&failed).is_none(), "{failed}");
    Ok(())
}

fn add_notes(dir: &std::path::Path, range: std::ops::Range<usize>, hook: usize) -> TestResult {
    std::fs::create_dir_all(dir)?;
    for at in range {
        let hook = "h".repeat(hook);
        let body =
            format!("---\nname: n{at:03}\ndescription: {hook}\ntype: feedback\n---\nThe fact.\n");
        std::fs::write(dir.join(format!("n{at:03}.md")), body)?;
    }
    Ok(())
}

/// The memory block with `count` notes, then with one more: the limit and the limit plus one.
fn block_at(count: usize, hook: usize) -> Result<(String, String), Box<dyn Error>> {
    let (home, cwd) = (
        Scratch::new("yi-caps-mem-home")?,
        Scratch::new("yi-caps-mem-cwd")?,
    );
    let dir = yi_runtime::memory::repo_dir(&home, &cwd);
    add_notes(&dir, 0..count, hook)?;
    let at = yi_runtime::memory::block(&home, &cwd).0;
    add_notes(&dir, count..count + 1, hook)?;
    Ok((at, yi_runtime::memory::block(&home, &cwd).0))
}

#[test]
fn the_memory_index_names_what_it_did_not_load() -> TestResult {
    let (at, over) = block_at(200, 10)?;
    assert!(row_in(&at).is_none(), "200 short notes load whole: {at}");
    let row = row_of(&over)?;
    assert_eq!(
        (row.kept, row.total, row.what, row.cap, row.limit, row.next),
        (
            200,
            201,
            "notes loaded",
            "LOAD_LINES",
            200,
            "await memory.search(\"words\")"
        )
    );

    // An index line is 22 B and the hook, so a 227-char hook makes 250 B with its newline.
    let (at, over) = block_at(100, 227)?;
    assert!(
        row_in(&at).is_none(),
        "100 lines are 25,000 B, the limit: {at}"
    );
    let row = row_of(&over)?;
    assert_eq!(
        (row.kept, row.total, row.cap, row.limit),
        (100, 101, "LOAD_BYTES", 25_000),
        "the bytes cut here, so the bytes are named"
    );
    Ok(())
}

/// A directory of `visible` files and two dot-names.
fn files_in(visible: usize) -> Result<Scratch, Box<dyn Error>> {
    let dir = Scratch::new("yi-caps-files")?;
    for at in 0..visible {
        std::fs::write(dir.join(format!("f{at:02}.txt")), "x")?;
    }
    std::fs::write(dir.join(".git"), "x")?;
    std::fs::write(dir.join(".env"), "x")?;
    Ok(dir)
}

#[test]
fn the_files_line_names_the_cap_and_the_hidden_names_it_skipped() -> TestResult {
    let dir = files_in(20)?;
    let at = files_line(&dir).ok_or("no files line")?;
    assert!(row_in(&at).is_none(), "20 names are the limit: {at}");

    std::fs::write(dir.join("f20.txt"), "x")?;
    let over = files_line(&dir).ok_or("no files line")?;
    let row = row_of(&over)?;
    assert_eq!(
        (row.kept, row.total, row.what, row.cap, row.limit, row.next),
        (
            20,
            21,
            "top-level names, 2 hidden skipped",
            "FILES_SHOWN",
            20,
            "ls -A"
        )
    );
    assert!(
        over.contains("f19.txt") && !over.contains("f20.txt"),
        "{over}"
    );
    Ok(())
}
