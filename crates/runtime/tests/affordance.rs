use std::collections::HashSet;
use std::error::Error;
use std::path::Path;

use serde_json::{Map, Value, json};
use yi_runtime::affordance::{self, Facts};
use yi_types::graph::Graph;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn a_spawn_hands_back_the_exact_collect_and_watch_calls() {
    let line = affordance::next("rlm.run", &["child_state(running)"], "porter");
    assert!(line.starts_with(affordance::NEXT));
    assert!(line.contains("rlm.wait(120)"));
    assert!(line.contains("rlm.send('porter'"));
    assert!(
        !line.contains(".jsonl"),
        "F7 writes the transcript, but the spawn reply names its dir as data — \
         the hint stays the two calls the model makes during the run: {line}"
    );
    assert_eq!(line.lines().count(), 1, "at most two lines, one is plenty");
}

#[test]
fn listing_name_names_the_session_name_field() {
    let line = affordance::next("ipython", &["listing_name_missed"], "");
    assert!(line.starts_with(affordance::NEXT));
    assert!(line.contains("session_name"));
    assert!(line.contains("RLMSpawnHandle.name"));
}

#[test]
fn a_finished_child_says_how_to_take_its_answer_as_data() {
    let line = affordance::next("rlm.run", &["child_state(finished)"], "porter");
    assert!(line.contains("rlm.result('porter', schema="));
}

#[test]
fn compaction_names_where_the_full_history_lives() {
    let with_file = affordance::next("compact.run", &["session_on_disk"], "");
    assert!(with_file.starts_with(affordance::NEXT));
    assert!(with_file.contains("compact.recall(\"needle\")"));
    assert!(with_file.contains("rlm.fetch(\"history://<id>/<entry>\")"));
    assert!(with_file.contains("(#entry)"));
    assert_eq!(with_file.lines().count(), 1, "one next line");
    let without_file = affordance::next("compact.run", &["session_in_memory"], "");
    assert!(without_file.contains("compact.recall(\"needle\")"));
    assert!(
        !without_file.contains("rlm.fetch"),
        "no session file, no fetch URL half: {without_file}"
    );
}

/// The repair template carries the caller's own arguments, so the model reads
/// its own call back with the missing key filled in as a placeholder.
#[test]
fn the_repair_template_keeps_the_arguments_the_caller_gave() -> TestResult {
    let schema = json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "pattern": {"type": "string"},
            "context": {"type": "integer"}
        },
        "required": ["pattern"]
    });
    let mut arguments = Map::new();
    arguments.insert("path".to_owned(), json!("crates/runtime/src/lib.rs"));
    let line = affordance::call_template("grep", &schema, &arguments);
    let rendered = line
        .strip_prefix(&format!("{}call grep as ", affordance::NEXT))
        .ok_or("unexpected shape")?;
    let value: Value = serde_json::from_str(rendered)?;
    assert_eq!(value["path"], "crates/runtime/src/lib.rs");
    assert_eq!(value["pattern"], "<string>");
    assert!(
        value.get("context").is_none(),
        "an optional key the caller never used stays out of the template"
    );
    Ok(())
}

#[test]
fn an_affordance_is_appended_to_the_last_text_block() -> TestResult {
    let mut result = yi_types::event::ToolResult {
        content: vec![yi_types::message::Content::Text {
            text: "no output".to_owned(),
            text_signature: None,
        }],
        details: Value::Object(Map::new()),
        usage: None,
        added_tool_names: None,
        terminate: None,
    };
    affordance::append(&mut result, "next: try grep");
    let Some(yi_types::message::Content::Text { text, .. }) = result.content.first() else {
        return Err("expected a text block".into());
    };
    assert_eq!(text, "no output\nnext: try grep");
    Ok(())
}

const SPAWNED: &str = "next: await rlm.wait(120) blocks until this child reports; rlm.send('porter', 'line') steers it";
const COROUTINE_LEAK: &str = "next: an un-awaited coroutine ran nothing; rlm.run and handle.result are async, so write h = await rlm.run(...) then await h.result()";
const METHOD_AWAITED: &str = "next: rlm.run is a method, not a coroutine — call it: h = await rlm.run('…'), then r = await h.result()";
const LISTING_NAME: &str =
    "next: list_subagents() entries expose session_name; RLMSpawnHandle.name is the spawn handle";
const CHILD_FINISHED: &str = "next: await rlm.result('porter', schema=…) validates the answer host-side; the child stays addressable for follow-ups";
const COMPACTED_ON_DISK: &str = "next: the window holds a summary plus recent turns; compact.recall(\"needle\") then rlm.fetch(\"history://<id>/<entry>\") pulls what the summary cites as (#entry)";
const COMPACTED_IN_MEMORY: &str = "next: the window holds a summary plus recent turns; compact.recall(\"needle\") pulls entry ids the summary cites as (#entry)";

fn todo_lines(checklist: &str) -> Result<Vec<String>, Box<dyn Error>> {
    Ok(yi_runtime::todo::text::next_lines(
        &yi_runtime::todo::text::parse(checklist)?,
    ))
}

/// The golden strings are the bytes the producers rendered before the graph
/// existed; tool results are model-facing, so a moved byte is a changed prompt.
#[test]
fn every_line_rendered_today_renders_from_the_graph() -> TestResult {
    assert_eq!(
        affordance::next("rlm.run", &["child_state(running)"], "porter"),
        SPAWNED
    );
    assert_eq!(
        affordance::next("rlm.run", &["child_state(running)"], "it's {name}"),
        SPAWNED.replace("porter", "it's {name}")
    );
    assert_eq!(
        affordance::next("ipython", &["coroutine_unawaited"], ""),
        COROUTINE_LEAK
    );
    assert_eq!(
        affordance::next("ipython", &["method_awaited"], ""),
        METHOD_AWAITED
    );
    assert_eq!(
        affordance::next("ipython", &["listing_name_missed"], ""),
        LISTING_NAME
    );
    assert_eq!(
        affordance::next("rlm.run", &["child_state(finished)"], "porter"),
        CHILD_FINISHED
    );
    assert_eq!(
        affordance::next("compact.run", &["session_on_disk"], ""),
        COMPACTED_ON_DISK
    );
    assert_eq!(
        affordance::next("compact.run", &["session_in_memory"], ""),
        COMPACTED_IN_MEMORY
    );
    assert_eq!(
        affordance::call_template(
            "grep",
            &json!({"properties": {"pattern": {"type": "string"}}, "required": ["pattern"]}),
            &Map::new()
        ),
        "next: call grep as {\"pattern\":\"<string>\"}"
    );

    assert_eq!(
        todo_lines("- [>] t1 ship\n- [ ] t2 test\n- [ ] \"{name}\"\n- [ ] t4 late\n- [!] t5 held")?,
        [
            "next: done t1 evidence=`<command>` <output line> · block t1 on user · drop t1 <reason>",
            "next: start t2 · drop t2 <reason>",
            "next: start \"\\\"{name}\\\"\" · drop \"\\\"{name}\\\"\" <reason>",
        ]
    );
    assert_eq!(
        todo_lines("- [ ] a\n- [ ] b\n- [ ] c\n- [ ] d")?,
        [
            "next: start \"a\" · drop \"a\" <reason>",
            "next: start \"b\" · drop \"b\" <reason>",
            "next: start \"c\" · drop \"c\" <reason>",
        ]
    );
    assert_eq!(
        todo_lines("- [x] t1 shipped\n- [!] t2 held\n- [!] t3 held too")?,
        ["next: unblock t2 · drop t2 <reason>"]
    );
    assert_eq!(todo_lines("- [x] t1 shipped\n- [-] t2 cut")?, [""; 0]);
    Ok(())
}

const TODO_OPS: [&str; 10] = [
    "set", "init", "append", "start", "done", "drop", "block", "unblock", "rm", "view",
];

/// Every host request the runtime registers, read from the `register("…")` calls themselves.
fn registered_requests(dir: &Path, out: &mut HashSet<String>) -> TestResult {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            registered_requests(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path)?;
            for call in source.split(".register(").skip(1) {
                if let Some(name) = call.trim_start().strip_prefix('"') {
                    out.extend(name.split('"').next().map(str::to_owned));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn the_shipped_graph_passes_every_structural_check() -> TestResult {
    let mut verbs: HashSet<String> = yi_tools::builtin_tools()
        .iter()
        .map(|tool| tool.name().to_owned())
        .collect();
    verbs.extend(["ipython", "plan", yi_runtime::todo::tool::NAME].map(str::to_owned));
    verbs.extend(TODO_OPS.map(|op| format!("todo.{op}")));
    registered_requests(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut verbs,
    )?;
    assert!(
        verbs.contains("rlm.service"),
        "the scan reads multi-line calls"
    );
    let graph = affordance::shipped();
    assert_ne!(graph.version, 0, "an unparsable graph falls back to empty");
    let verbs = verbs.iter().map(String::as_str).collect();
    assert_eq!(graph.check(&verbs), Ok(()));
    Ok(())
}

fn small_graph() -> Result<Graph, serde_json::Error> {
    let edge = |from: &str, to: &str, guidance: &str, weight: u32| {
        json!({"from": from, "relation": "then", "to": to, "condition": "always",
               "guidance": guidance, "weight": weight})
    };
    let nodes = ["a", "b", "c", "d", "e"].map(|id| json!({"id": id, "kind": "tool"}));
    serde_json::from_value(json!({
        "version": 1,
        "nodes": nodes,
        "edges": [
            edge("a", "b", "light", 1),
            edge("a", "b", "heavy {name}", 9),
            edge("a", "c", "light", 1),
            edge("a", "c", "held", 5),
            edge("b", "d", "second hop", 7),
            edge("d", "e", "third hop", 99),
        ],
    }))
}

#[test]
fn an_unknown_last_call_renders_nothing() -> TestResult {
    let graph = small_graph()?;
    let facts = Facts {
        holds: &[],
        name: "",
        cap: 3,
    };
    for unknown in ["", "A", "a ", "a.b", "rlm", "todo.start.x"] {
        assert_eq!(
            affordance::render(&graph, unknown, &facts),
            [""; 0],
            "{unknown:?}"
        );
    }
    assert_eq!(
        affordance::next("rlm", &["child_state(running)"], "porter"),
        ""
    );
    Ok(())
}

#[test]
fn rendering_stops_at_two_hops_and_three_lines() -> TestResult {
    let mut graph = small_graph()?;
    let wide = Facts {
        holds: &[],
        name: "porter",
        cap: 9,
    };
    assert_eq!(
        affordance::render(&graph, "a", &wide),
        [
            "next: heavy porter",
            "next: second hop",
            "next: held",
            "next: light"
        ],
        "weight orders, one guidance text renders once, and the third hop never renders"
    );
    let todo = Facts { cap: 3, ..wide };
    assert_eq!(affordance::render(&graph, "a", &todo).len(), 3);
    for edge in &mut graph.edges {
        edge.condition = serde_json::from_value(json!("done_refused"))?;
    }
    assert_eq!(
        affordance::render(&graph, "a", &wide),
        [""; 0],
        "no fact, no line"
    );
    let refused = Facts {
        holds: &["done_refused"],
        ..wide
    };
    assert_eq!(affordance::render(&graph, "a", &refused).len(), 4);
    Ok(())
}
