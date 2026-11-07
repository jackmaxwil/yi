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
    let source = include_str!("../../../python/yi_runtime/src/rlm/__init__.py");
    let line_of = |needle: &str| source.lines().position(|line| line.starts_with(needle));
    let start = line_of("    async def result(").ok_or("no async def result(")? + 1;
    let close = line_of("    ) -> ").ok_or("no signature close")? + 1;
    fs::write(dir.join("input.py"), source)?;
    let output = find(&dir, "input.py", "async def result(");
    let header = output
        .lines()
        .find(|line| line.starts_with("[find: block at lines "))
        .ok_or_else(|| format!("no block header: {output}"))?;
    let span = header.trim_start_matches("[find: block at lines ");
    let (first, rest) = span.split_once('-').ok_or("no span")?;
    let last = rest.split_once(' ').ok_or("no end")?.0.parse::<usize>()?;
    assert_eq!(first.parse::<usize>()?, start, "{output}");
    assert!(last >= close, "the block ends at {last}, before {close}");

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
    let source = include_str!("../../ai/src/leak.rs");
    fs::write(dir.join("leak.rs"), source)?;
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with("fn between<'a>"))
        .ok_or("no fn between")?;
    let close = start
        + lines
            .iter()
            .skip(start)
            .position(|line| *line == "}")
            .ok_or("no close")?;
    let output = find(&dir, "leak.rs", "fn between<'a>");
    let span = format!("[find: block at lines {}-{} of", start + 1, close + 1);
    assert!(output.contains(&span), "{output}");

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

#[test]
fn non_rust_single_quoted_strings_do_not_change_block_depth() -> TestResult {
    let dir = temp_dir("hashline-non-rust-strings")?;
    for (name, quoted) in [
        ("closer.js", "const s = 'x)';"),
        ("opener.js", "const s = 'hello (world';"),
    ] {
        fs::write(
            dir.join(name),
            format!("function f() {{\n    {quoted}\n    return s;\n}}\n"),
        )?;
        let output = find(&dir, name, "function f()");
        assert!(
            output.contains("[find: block at lines 1-4 of"),
            "{name}: {output}"
        );
    }
    Ok(())
}

#[test]
fn non_rust_block_comment_apostrophes_do_not_hide_delimiters() -> TestResult {
    let dir = temp_dir("hashline-non-rust-block-comments")?;
    for (name, source) in [
        (
            "closer.js",
            "function f() {\n    if (ready) {\n        /* don't */ }\n    return ready;\n}\n",
        ),
        (
            "opener.js",
            "function f() {\n    /* user's */ return {\n        value: true\n    };\n}\n",
        ),
    ] {
        fs::write(dir.join(name), source)?;
        let output = find(&dir, name, "function f()");
        assert!(
            output.contains("[find: block at lines 1-5 of"),
            "{name}: {output}"
        );
    }
    Ok(())
}
