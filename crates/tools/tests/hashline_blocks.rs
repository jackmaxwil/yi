use std::error::Error;
use std::fs;

use serde_json::{Map, json};
use yi_tools::hashline::tool::HashlineReadTool;
use yi_tools::{Tool, ToolContext};

use crate::scratch::Scratch;
use crate::tools::{output_text, temp_dir};

type TestResult = Result<(), Box<dyn Error>>;

fn find(dir: &Scratch, path: &str, needle: &str) -> String {
    let arguments = Map::from_iter([
        ("path".to_owned(), json!(path)),
        ("find".to_owned(), json!(needle)),
    ]);
    let output = HashlineReadTool::new(yi_tools::hashline::tool::shared_hashline_state())
        .execute(arguments, &ToolContext::new(dir.to_path_buf()));
    let text = output_text(&output);
    assert!(!output.is_error, "{text}");
    text
}

#[test]
fn a_multiline_python_signature_resolves_through_the_function_body() -> TestResult {
    let dir = temp_dir("hashline-python-signature")?;
    fs::write(
        dir.join("input.py"),
        include_str!("../../../python/yi_runtime/src/rlm/__init__.py"),
    )?;
    let output = find(&dir, "input.py", "async def result(");
    assert!(
        output.contains("[find: block at lines 287-354 of"),
        "{output}"
    );

    fs::write(dir.join("one.py"), "def a():\n")?;
    let one = find(&dir, "one.py", "def a():");
    assert!(one.contains("[find: no enclosing block"), "{one}");
    fs::write(
        dir.join("quoted.py"),
        "def f(\n    arg='abc)',\n):\n    return arg\n",
    )?;
    let quoted = find(&dir, "quoted.py", "def f(");
    assert!(quoted.contains("[find: block at lines 1-4 of"), "{quoted}");

    Ok(())
}

#[test]
fn rust_lifetimes_do_not_hide_braces_and_character_literals_stay_unchanged() -> TestResult {
    let dir = temp_dir("hashline-rust-lifetimes")?;
    fs::write(dir.join("leak.rs"), include_str!("../../ai/src/leak.rs"))?;
    let output = find(&dir, "leak.rs", "fn between<'a>");
    assert!(
        output.contains("[find: block at lines 93-101 of"),
        "{output}"
    );

    fs::write(
        dir.join("getter.rs"),
        "pub fn get<'a>(&'a self) -> &'a str {\n    self.value\n}\n",
    )?;
    let getter = find(&dir, "getter.rs", "pub fn get<'a>");
    assert!(getter.contains("[find: block at lines 1-3 of"), "{getter}");

    fs::write(
        dir.join("character.rs"),
        "fn a() {\n    let brace = '{';\n    let letter = 'x';\n}\n",
    )?;
    let character = find(&dir, "character.rs", "fn a()");
    assert!(
        character.contains("[find: block at lines 1-4 of"),
        "{character}"
    );

    fs::write(dir.join("one.rs"), "fn a() {}\n")?;
    let one = find(&dir, "one.rs", "fn a()");
    assert!(one.contains("[find: no enclosing block"), "{one}");
    Ok(())
}
