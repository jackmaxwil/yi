use std::error::Error;

use yi_tools::hashline::blocks::{brace_block_resolver, indent_block_resolver};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn a_multiline_python_signature_resolves_through_the_function_body() -> TestResult {
    let text = include_str!("../../../python/yi_runtime/src/rlm/__init__.py");
    let span = indent_block_resolver(text, 287).ok_or("async def result did not resolve")?;
    assert_eq!((span.start, span.end), (287, 354));
    Ok(())
}

#[test]
fn rust_lifetimes_do_not_hide_braces_and_character_literals_stay_unchanged() -> TestResult {
    let text = include_str!("../../ai/src/leak.rs");
    let span = brace_block_resolver(text, 93).ok_or("fn between<'a> did not resolve")?;
    assert_eq!(span.start, 93);
    assert!(span.end > 93);

    let getter = "pub fn get<'a>(&'a self) -> &'a str {\n    self.value\n}\n";
    assert_eq!(
        brace_block_resolver(getter, 1).map(|span| span.end),
        Some(3)
    );
    let character = "fn a() {\n    let brace = '{';\n    let letter = 'x';\n}\n";
    assert_eq!(
        brace_block_resolver(character, 1).map(|span| span.end),
        Some(4)
    );
    Ok(())
}

#[test]
fn one_line_definitions_still_open_no_block() {
    assert_eq!(indent_block_resolver("def a():\n", 1), None);
    assert_eq!(brace_block_resolver("fn a() {}\n", 1), None);
}
