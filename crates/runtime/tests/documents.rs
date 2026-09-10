use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};
use yi_kernel::bootstrap::{
    BootstrapOptions, default_runtime_source_dir, default_skills_source_dir, ensure_kernel_python,
};
use yi_tools::{Documents, Tool, ToolContext, ToolOutput};
use yi_types::message::Content;

type TestResult = Result<(), Box<dyn Error>>;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/documents");

struct Scratch {
    cwd: PathBuf,
    home: PathBuf,
}

type Setup = (Scratch, Vec<Arc<dyn Tool>>);

/// The shared kernel venv every kernel test builds, with the converted copies kept under a
/// scratch home so no run reads another's cache.
fn setup(tag: &str) -> Result<Setup, Box<dyn Error>> {
    let real_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    ensure_kernel_python(&BootstrapOptions {
        on_progress: Some(Box::new(|message| eprintln!("{message}"))),
        home: real_home.clone(),
        runtime_source_dir: default_runtime_source_dir(),
        skills_source_dir: default_skills_source_dir(),
        toolchain: None,
        venv_dir: None,
    })?;
    let root = std::env::temp_dir().join(format!("yi-documents-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let scratch = Scratch {
        cwd: root.join("project"),
        home: root.join("home"),
    };
    std::fs::create_dir_all(&scratch.cwd)?;
    std::fs::create_dir_all(&scratch.home)?;
    let documents = Documents {
        home: scratch.home.clone(),
        ..yi_runtime::documents(&real_home)
    };
    Ok((
        scratch,
        yi_runtime::builtin_tools_with(false, Some(documents)),
    ))
}

fn tool(tools: &[Arc<dyn Tool>], name: &str) -> Result<Arc<dyn Tool>, Box<dyn Error>> {
    Ok(Arc::clone(
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .ok_or("no such tool")?,
    ))
}

fn call(tool: &Arc<dyn Tool>, cwd: &Path, input: Value) -> Result<ToolOutput, Box<dyn Error>> {
    let input: Map<String, Value> = serde_json::from_value(input)?;
    Ok(tool.execute(input, &ToolContext::new(cwd.to_path_buf())))
}

fn text(output: &ToolOutput) -> String {
    output
        .result
        .content
        .iter()
        .map(|content| match content {
            Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect()
}

fn copies(home: &Path) -> usize {
    std::fs::read_dir(home.join(".yi").join("converted")).map_or(0, Iterator::count)
}

#[test]
fn a_docx_reads_as_markdown_through_the_read_window() -> TestResult {
    let (scratch, tools) = setup("docx")?;
    std::fs::copy(
        Path::new(FIXTURES).join("brief.docx"),
        scratch.cwd.join("brief.docx"),
    )?;
    let read = tool(&tools, "read")?;

    let output = call(&read, &scratch.cwd, json!({"path": "brief.docx"}))?;
    let body = text(&output);
    assert!(!output.is_error, "{body}");
    let mut lines = body.lines();
    let header = lines.next().unwrap_or_default();
    assert!(
        header.contains("/.yi/converted/brief-") && header.contains(".md#"),
        "the header names the copy the anchors belong to: {header}"
    );
    assert_eq!(
        lines.next(),
        Some("[brief.docx converted to Markdown; editing this copy does not change brief.docx]")
    );
    assert!(body.contains("Quarterly brief"), "{body}");
    assert!(
        body.contains("€1 240 — naïve"),
        "non-ASCII survives: {body}"
    );
    assert!(
        body.contains("| Espresso machine | Zoë | ordered |"),
        "{body}"
    );
    assert_eq!(output.result.details["convertedFrom"], json!("docx"));

    let window = text(&call(
        &read,
        &scratch.cwd,
        json!({"path": "brief.docx", "limit": 1}),
    )?);
    assert!(
        window.contains("continue with offset=2"),
        "a converted document windows like any file: {window}"
    );
    Ok(())
}

#[test]
fn the_copy_is_reused_until_the_original_changes() -> TestResult {
    let (scratch, tools) = setup("cache")?;
    let source = scratch.cwd.join("brief.docx");
    std::fs::copy(Path::new(FIXTURES).join("brief.docx"), &source)?;
    let read = tool(&tools, "read")?;

    call(&read, &scratch.cwd, json!({"path": "brief.docx"}))?;
    call(&read, &scratch.cwd, json!({"path": "brief.docx"}))?;
    assert_eq!(copies(&scratch.home), 1, "a second read converts nothing");

    let later = std::fs::metadata(&source)?.modified()? + Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(&source)?
        .set_modified(later)?;
    let output = call(&read, &scratch.cwd, json!({"path": "brief.docx"}))?;
    assert!(!output.is_error, "{}", text(&output));
    assert_eq!(
        copies(&scratch.home),
        2,
        "a saved original is converted again"
    );
    Ok(())
}

#[test]
fn write_and_edit_refuse_a_converted_original() -> TestResult {
    let (scratch, tools) = setup("refuse")?;
    let source = scratch.cwd.join("brief.docx");
    std::fs::copy(Path::new(FIXTURES).join("brief.docx"), &source)?;
    let before = std::fs::read(&source)?;
    call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "brief.docx"}),
    )?;

    let write = call(
        &tool(&tools, "write")?,
        &scratch.cwd,
        json!({"path": "brief.docx", "content": "# Quarterly brief\n"}),
    )?;
    let refusal = text(&write);
    assert!(write.is_error, "{refusal}");
    assert!(
        refusal.contains("/.yi/converted/brief-"),
        "the refusal names the copy: {refusal}"
    );
    let edit = call(
        &tool(&tools, "edit")?,
        &scratch.cwd,
        json!({"patch": "[brief.docx]\nPUT 1.=1:\n+# Annual brief\n"}),
    )?;
    assert!(edit.is_error, "{}", text(&edit));
    assert!(
        text(&edit).contains("converted document"),
        "{}",
        text(&edit)
    );
    assert_eq!(std::fs::read(&source)?, before, "the document is untouched");
    Ok(())
}

#[test]
fn a_scanned_pdf_is_refused_with_its_reason() -> TestResult {
    let (scratch, tools) = setup("scanned")?;
    std::fs::copy(
        Path::new(FIXTURES).join("scanned.pdf"),
        scratch.cwd.join("scanned.pdf"),
    )?;
    let output = call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "scanned.pdf"}),
    )?;
    let message = text(&output);
    assert!(output.is_error, "{message}");
    assert!(
        message.ends_with(
            "scanned.pdf: a scanned PDF, 1 of 1 pages without a text layer; it cannot be read as text"
        ),
        "{message}"
    );
    assert_eq!(copies(&scratch.home), 0, "a refusal leaves no copy");
    Ok(())
}

#[test]
fn the_content_names_the_format_not_the_extension() -> TestResult {
    let (scratch, tools) = setup("sniff")?;
    std::fs::copy(
        Path::new(FIXTURES).join("brief.docx"),
        scratch.cwd.join("notes.txt"),
    )?;
    let output = call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "notes.txt"}),
    )?;
    assert!(
        text(&output).contains("| Printer toner | Łukasz | late |"),
        "{}",
        text(&output)
    );
    Ok(())
}

#[test]
fn a_binary_that_is_no_document_keeps_the_plain_error() -> TestResult {
    let (scratch, tools) = setup("binary")?;
    let path = scratch.cwd.join("logo.png");
    std::fs::write(
        &path,
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe],
    )?;
    let output = call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "logo.png"}),
    )?;
    assert!(output.is_error);
    assert_eq!(
        text(&output),
        format!(
            "failed to read {}: stream did not contain valid UTF-8",
            path.display()
        )
    );
    assert_eq!(copies(&scratch.home), 0);
    Ok(())
}

#[test]
fn an_rtf_file_converts_though_it_is_seven_bit_text() -> TestResult {
    let (scratch, tools) = setup("rtf")?;
    std::fs::copy(
        Path::new(FIXTURES).join("brief.rtf"),
        scratch.cwd.join("brief.rtf"),
    )?;
    let output = call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "brief.rtf"}),
    )?;
    let body = text(&output);
    assert!(
        body.lines()
            .nth(1)
            .is_some_and(|line| line.contains("converted to Markdown")),
        "{body}"
    );
    assert!(body.contains("Quarterly brief"), "{body}");
    assert!(body.contains("€1 240"), "{body}");
    assert!(
        !body.contains("\\rtf1"),
        "no RTF markup reaches the model: {body}"
    );
    Ok(())
}

#[test]
fn a_partly_scanned_pdf_reads_its_text_pages_and_names_the_rest() -> TestResult {
    let (scratch, tools) = setup("mixed")?;
    std::fs::copy(
        Path::new(FIXTURES).join("mixed.pdf"),
        scratch.cwd.join("mixed.pdf"),
    )?;
    let output = call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "mixed.pdf"}),
    )?;
    let body = text(&output);
    assert!(!output.is_error, "{body}");
    assert!(
        body.contains("[no text layer on page(s) 2 of 2; left out]"),
        "{body}"
    );
    assert!(body.contains("Zoë signs the lease"), "{body}");
    Ok(())
}
