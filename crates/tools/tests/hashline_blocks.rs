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
        output.contains("[find: block at lines 319-380 of"),
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

#[test]
fn multiline_block_comments_do_not_close_the_enclosing_block() -> TestResult {
    let dir = temp_dir("hashline-multiline-block-comments")?;
    fs::write(
        dir.join("comment.js"),
        "function f() {\n    /* disabled {\n     * still disabled\n    } */\n    return 1;\n}\n",
    )?;
    let output = find(&dir, "comment.js", "function f()");
    assert!(output.contains("[find: block at lines 1-6 of"), "{output}");
    Ok(())
}

#[test]
fn comment_markers_inside_regex_classes_do_not_hide_delimiters() -> TestResult {
    let dir = temp_dir("hashline-regex-comment-markers")?;
    for (name, literal) in [("leading.js", "/[/*]/"), ("middle.js", "/a[/*]/")] {
        fs::write(
            dir.join(name),
            format!(
                "function f() {{\n    const pattern = {literal};\n    const value = total / count / 2;\n    return pattern.test(\"/*\");\n}}\n"
            ),
        )?;
        let output = find(&dir, name, "function f()");
        assert!(
            output.contains("[find: block at lines 1-5 of"),
            "{name}: {output}"
        );
    }
    Ok(())
}

#[test]
fn division_operators_and_continuations_do_not_hide_delimiters() -> TestResult {
    let dir = temp_dir("hashline-division-not-regex")?;
    for (name, source, span) in [
        (
            "calc.css",
            ".rule {\n    width: calc(100% / 3);\n    color: red;\n}\n",
            "[find: block at lines 1-4 of",
        ),
        (
            "increment.js",
            "function f() {\n    const value = i++ / 2;\n    return value;\n}\n",
            "[find: block at lines 1-4 of",
        ),
        (
            "continued.rs",
            "fn f() {\n    let value = (a\n        / b);\n    value\n}\n",
            "[find: block at lines 1-5 of",
        ),
    ] {
        fs::write(dir.join(name), source)?;
        let anchor = if name == "calc.css" { ".rule" } else { "f()" };
        let output = find(&dir, name, anchor);
        assert!(output.contains(span), "{name}: {output}");
    }
    Ok(())
}

#[test]
fn shell_globs_do_not_start_block_comments() -> TestResult {
    let dir = temp_dir("hashline-shell-glob-not-comment")?;
    for (name, shebang, span) in [
        ("run.sh", "", "1-4"),
        ("deploy", "#!/bin/sh\n", "2-5"),
        (".bashrc", "", "1-4"),
        ("Makefile", "", "1-4"),
        ("rules.mk", "", "1-4"),
    ] {
        fs::write(
            dir.join(name),
            format!("{shebang}run() {{\n    cp *.log /var/backups/*\n    echo done\n}}\n"),
        )?;
        let output = find(&dir, name, "run()");
        assert!(
            output.contains(&format!("[find: block at lines {span} of")),
            "{name}: {output}"
        );
    }
    Ok(())
}

#[test]
fn a_non_shell_shebang_keeps_c_style_block_comments() -> TestResult {
    let dir = temp_dir("hashline-non-shell-shebang")?;
    for (name, shebang) in [
        ("cli", "#!/usr/bin/env node\n"),
        ("script", "#!/usr/bin/env -S php -d strict\n"),
        ("daemon", "#!/opt/bin/ruby\n"),
    ] {
        fs::write(
            dir.join(name),
            format!("{shebang}function f() {{\n    /* }} */\n    return 1;\n}}\n"),
        )?;
        let output = find(&dir, name, "function f()");
        assert!(
            output.contains("[find: block at lines 2-5 of"),
            "{name}: {output}"
        );
    }
    Ok(())
}

#[test]
fn a_regex_literal_opening_a_continuation_line_does_not_hide_delimiters() -> TestResult {
    let dir = temp_dir("hashline-regex-continuation")?;
    for name in ["split.js", "split.mts", "split.cts"] {
        fs::write(
            dir.join(name),
            "function f() {\n    const pattern =\n        /[/*]/;\n    return pattern;\n}\n",
        )?;
        let output = find(&dir, name, "function f()");
        assert!(
            output.contains("[find: block at lines 1-5 of"),
            "{name}: {output}"
        );
    }
    Ok(())
}

#[test]
fn block_comments_stay_out_of_depth_for_brace_languages_beyond_the_named_ones() -> TestResult {
    let dir = temp_dir("hashline-unlisted-brace-languages")?;
    for name in [
        "main.tf",
        "view.mm",
        "kernel.cu",
        "schema.proto",
        "app.dart",
        "match.mts",
        "Jenkinsfile",
        ".eslintrc",
        ".babelrc",
        ".swcrc",
    ] {
        fs::write(dir.join(name), "resource {\n  /* } */\n  name = 1\n}\n")?;
        let output = find(&dir, name, "resource {");
        assert!(
            output.contains("[find: block at lines 1-4 of"),
            "{name}: {output}"
        );
    }
    Ok(())
}
