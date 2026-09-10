use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_kernel::bootstrap::{
    BootstrapOptions, default_runtime_source_dir, default_skills_source_dir, ensure_kernel_python,
};
use yi_tools::{Converter, Documents, Tool, ToolContext, ToolOutput};
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
        header.starts_with("[brief.docx#"),
        "the header keeps the original's path, inside the working tree: {header}"
    );
    assert_eq!(
        lines.next(),
        Some("[brief.docx: docx converted to Markdown, read-only — edit and write refuse it]")
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
    assert_eq!(output.result.details["converted"]["from"], json!("docx"));
    assert_eq!(output.result.details["converted"]["cache"], json!("miss"));
    assert_eq!(output.result.details["sourceBytes"], json!(10748));

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
    let source = scratch.cwd.join("brief.rtf");
    std::fs::copy(Path::new(FIXTURES).join("brief.rtf"), &source)?;
    let read = tool(&tools, "read")?;

    let first = call(&read, &scratch.cwd, json!({"path": "brief.rtf"}))?;
    let second = call(&read, &scratch.cwd, json!({"path": "brief.rtf"}))?;
    assert_eq!(copies(&scratch.home), 1, "a second read converts nothing");
    assert_eq!(first.result.details["converted"]["cache"], json!("miss"));
    assert_eq!(second.result.details["converted"]["cache"], json!("hit"));

    let rtf = std::fs::read_to_string(&source)?;
    let (head, _) = rtf.rsplit_once('}').ok_or("no closing brace")?;
    std::fs::write(&source, format!("{head}\\par Addendum paragraph}}"))?;
    let output = call(&read, &scratch.cwd, json!({"path": "brief.rtf"}))?;
    assert!(
        text(&output).contains("Addendum paragraph"),
        "{}",
        text(&output)
    );
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
        refusal.contains("is a binary document") && refusal.contains("nothing writes text back"),
        "{refusal}"
    );
    let edit = call(
        &tool(&tools, "edit")?,
        &scratch.cwd,
        json!({"patch": "[brief.docx]\nPUT 1.=1:\n+# Annual brief\n"}),
    )?;
    assert!(edit.is_error, "{}", text(&edit));
    assert!(
        text(&edit).contains("is a binary document"),
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
            "scanned.pdf: no text layer on any of the 1 PDF page(s) read of 1 (image-only or vector art), so there is no text to show"
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
            "failed to read {}: a PNG image, not text; in ipython the bundled attach_image skill puts it in front of the model",
            path.display()
        )
    );
    let lock = scratch.cwd.join("~$brief.docx");
    std::fs::write(
        &lock,
        [0x08, 0x00, 0x00, 0x00, b'J', b'a', b'c', b'k', 0x00, 0x00],
    )?;
    let output = call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "~$brief.docx"}),
    )?;
    assert!(output.is_error);
    assert_eq!(
        text(&output),
        format!(
            "failed to read {}: a binary file (NUL bytes), and not a document the converter reads",
            lock.display()
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
    assert!(body.contains("[page 1]"), "page markers: {body}");
    assert!(body.contains("[page 2: no text layer]"), "{body}");
    assert!(body.contains("Zoë signs the lease"), "{body}");

    let only = text(&call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "mixed.pdf", "pages": "1"}),
    )?);
    assert!(
        only.contains("[mixed.pdf: pages 1 of the pdf converted"),
        "{only}"
    );
    assert!(!only.contains("page 2"), "{only}");
    for (pages, reason) in [
        ("9", "none of the pages asked for exist"),
        ("x", "not a 1-based list"),
    ] {
        let refused = call(
            &tool(&tools, "read")?,
            &scratch.cwd,
            json!({"path": "mixed.pdf", "pages": pages}),
        )?;
        assert!(
            refused.is_error && text(&refused).contains(reason),
            "{}",
            text(&refused)
        );
    }
    Ok(())
}

fn slow_converter(scratch: &Scratch, timeout_secs: u64) -> Result<Documents, Box<dyn Error>> {
    let script = scratch.home.join("slow-python");
    std::fs::write(&script, "#!/bin/sh\nsleep 30\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(Documents {
        timeout: std::time::Duration::from_secs(timeout_secs),
        ..Documents::fixed(
            scratch.home.clone(),
            Converter {
                python: script,
                formats: Vec::new(),
            },
        )
    })
}

#[test]
fn parallel_first_reads_of_one_document_all_succeed() -> TestResult {
    let (scratch, tools) = setup("parallel")?;
    let read = tool(&tools, "read")?;
    for round in 0..5 {
        let name = format!("brief{round}.docx");
        std::fs::copy(
            Path::new(FIXTURES).join("brief.docx"),
            scratch.cwd.join(&name),
        )?;
        let handles: Vec<_> = (0..3)
            .map(|offset| {
                let read = Arc::clone(&read);
                let cwd = scratch.cwd.clone();
                let name = name.clone();
                std::thread::spawn(move || {
                    call(&read, &cwd, json!({"path": name, "offset": offset + 1}))
                        .map(|out| (out.is_error, text(&out)))
                        .map_err(|error| error.to_string())
                })
            })
            .collect();
        for handle in handles {
            let (is_error, body) = handle.join().map_err(|_| "thread panicked")??;
            assert!(!is_error, "{body}");
            assert!(
                body.contains("Quarterly brief") || body.contains("Zoë"),
                "{body}"
            );
        }
    }
    assert_eq!(
        copies(&scratch.home),
        5,
        "one copy per document, however many readers raced"
    );
    Ok(())
}

#[test]
fn a_file_past_the_ceiling_is_refused_before_the_converter_runs() -> TestResult {
    let (scratch, tools) = setup("ceiling")?;
    let path = scratch.cwd.join("huge.docx");
    std::fs::write(&path, b"PK\x03\x04")?;
    std::fs::File::options()
        .write(true)
        .open(&path)?
        .set_len(yi_tools::document_ceiling() + 1)?;
    let output = call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "huge.docx"}),
    )?;
    assert!(output.is_error);
    assert!(
        text(&output).contains("past the 256 MiB ceiling"),
        "{}",
        text(&output)
    );
    assert_eq!(copies(&scratch.home), 0);
    Ok(())
}

#[test]
fn a_hanging_converter_is_stopped_at_the_timeout_and_on_cancel() -> TestResult {
    let (scratch, _) = setup("timeout")?;
    std::fs::copy(
        Path::new(FIXTURES).join("brief.docx"),
        scratch.cwd.join("brief.docx"),
    )?;
    let tools = yi_runtime::builtin_tools_with(false, Some(slow_converter(&scratch, 1)?));
    let started = std::time::Instant::now();
    let output = call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "brief.docx"}),
    )?;
    assert!(output.is_error);
    assert!(
        text(&output).contains("conversion stopped"),
        "{}",
        text(&output)
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the timeout, not the sleep, ended it"
    );

    let tools = yi_runtime::builtin_tools_with(false, Some(slow_converter(&scratch, 60)?));
    let input: Map<String, Value> = serde_json::from_value(json!({"path": "brief.docx"}))?;
    let mut context = ToolContext::new(scratch.cwd.clone());
    context.cancelled = Arc::new(|| true);
    let started = std::time::Instant::now();
    let output = tool(&tools, "read")?.execute(input, &context);
    assert!(
        output.is_error && text(&output).contains("conversion stopped"),
        "{}",
        text(&output)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(
        copies(&scratch.home),
        0,
        "a stopped conversion leaves no copy"
    );
    Ok(())
}

#[test]
fn a_document_with_a_unicode_name_converts() -> TestResult {
    let (scratch, tools) = setup("unicode")?;
    std::fs::copy(
        Path::new(FIXTURES).join("brief.docx"),
        scratch.cwd.join("résumé café.docx"),
    )?;
    let output = call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "résumé café.docx"}),
    )?;
    assert!(!output.is_error, "{}", text(&output));
    assert!(text(&output).contains("Quarterly brief"));
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn a_document_whose_name_is_not_utf8_converts() -> TestResult {
    use std::os::unix::ffi::OsStrExt;
    let (scratch, tools) = setup("osstr")?;
    let name = std::ffi::OsStr::from_bytes(b"na\xefve.docx");
    std::fs::copy(
        Path::new(FIXTURES).join("brief.docx"),
        scratch.cwd.join(name),
    )?;
    let input: Map<String, Value> = serde_json::from_value(json!({"path": "*.docx"}))?;
    let output = tool(&tools, "read")?.execute(input, &ToolContext::new(scratch.cwd.clone()));
    assert!(
        text(&output).contains("Quarterly brief"),
        "{}",
        text(&output)
    );
    Ok(())
}

#[test]
fn a_tampered_copy_is_converted_again() -> TestResult {
    let (scratch, tools) = setup("tamper")?;
    std::fs::copy(
        Path::new(FIXTURES).join("brief.docx"),
        scratch.cwd.join("brief.docx"),
    )?;
    let read = tool(&tools, "read")?;
    call(&read, &scratch.cwd, json!({"path": "brief.docx"}))?;
    let dir = scratch.home.join(".yi").join("converted");
    let copy = std::fs::read_dir(&dir)?.next().ok_or("no copy")??.path();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&copy)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o444, "the copy is read-only");
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o644))?;
    }
    std::fs::write(&copy, "# Forged\n")?;
    let output = call(&read, &scratch.cwd, json!({"path": "brief.docx"}))?;

    assert!(
        text(&output).contains("Quarterly brief") && !text(&output).contains("Forged"),
        "{}",
        text(&output)
    );
    assert!(
        std::fs::read_to_string(&copy)?.contains("Quarterly brief"),
        "the forged copy is replaced by a fresh conversion"
    );
    assert_eq!(copies(&scratch.home), 1);

    let refused = call(
        &tool(&tools, "write")?,
        &scratch.cwd,
        json!({"path": copy.to_string_lossy(), "content": "x"}),
    )?;
    assert!(
        refused.is_error && text(&refused).contains("read-only Markdown copy"),
        "{}",
        text(&refused)
    );
    Ok(())
}

#[test]
fn write_refuses_an_unread_document_and_allows_text_formats() -> TestResult {
    let (scratch, tools) = setup("unread")?;
    std::fs::copy(
        Path::new(FIXTURES).join("brief.docx"),
        scratch.cwd.join("never.docx"),
    )?;
    std::fs::copy(
        Path::new(FIXTURES).join("mixed.pdf"),
        scratch.cwd.join("never.pdf"),
    )?;
    std::fs::copy(
        Path::new(FIXTURES).join("brief.rtf"),
        scratch.cwd.join("brief.rtf"),
    )?;
    // A hand-written PDF: no NUL byte in it, so only the `%PDF-` marker can refuse the write.
    std::fs::write(
        scratch.cwd.join("tiny.pdf"),
        "%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n",
    )?;
    let write = tool(&tools, "write")?;
    for name in ["never.docx", "never.pdf", "tiny.pdf"] {
        let before = std::fs::read(scratch.cwd.join(name))?;
        let output = call(
            &write,
            &scratch.cwd,
            json!({"path": name, "content": "gone"}),
        )?;
        assert!(output.is_error, "{name}: {}", text(&output));
        assert_eq!(std::fs::read(scratch.cwd.join(name))?, before);
    }
    let output = call(
        &write,
        &scratch.cwd,
        json!({"path": "brief.rtf", "content": "{\\rtf1 plain}"}),
    )?;
    assert!(
        !output.is_error,
        "RTF is text and stays writable: {}",
        text(&output)
    );
    let output = call(
        &write,
        &scratch.cwd,
        json!({"path": "fresh.docx", "content": "not really a docx"}),
    )?;
    assert!(
        !output.is_error,
        "a new path is anyone's to write: {}",
        text(&output)
    );
    Ok(())
}

#[test]
fn text_that_is_not_utf8_is_decoded_rather_than_refused() -> TestResult {
    let (scratch, tools) = setup("decode")?;
    std::fs::write(scratch.cwd.join("latin.txt"), b"cr\xe8me br\xfbl\xe9e\n")?;
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend("naïve\n".encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(scratch.cwd.join("wide.txt"), utf16)?;
    let read = tool(&tools, "read")?;
    let latin = text(&call(&read, &scratch.cwd, json!({"path": "latin.txt"}))?);
    assert!(
        latin.contains("[decoded from Latin-1") && latin.contains("1:crème brûlée"),
        "{latin}"
    );
    let wide = text(&call(&read, &scratch.cwd, json!({"path": "wide.txt"}))?);
    assert!(
        wide.contains("[decoded from UTF-16") && wide.contains("1:naïve"),
        "{wide}"
    );
    assert_eq!(copies(&scratch.home), 0, "text never goes to the converter");
    Ok(())
}

#[test]
fn a_glob_reads_documents_whole_and_counts_their_markdown() -> TestResult {
    let (scratch, tools) = setup("glob")?;
    std::fs::copy(
        Path::new(FIXTURES).join("brief.docx"),
        scratch.cwd.join("a.docx"),
    )?;
    // 60 KB of ignored RTF destination: a big source whose Markdown is one line, so the budget
    // must be charged for the Markdown, not for the bytes on disk.
    let padded = format!(
        "{{\\rtf1\\ansi {{\\*\\junk {}}} padded rtf body}}",
        "A".repeat(60_000)
    );
    std::fs::write(scratch.cwd.join("b.rtf"), padded)?;
    let output = call(&tool(&tools, "read")?, &scratch.cwd, json!({"path": "*.*"}))?;
    let body = text(&output);
    assert!(
        body.contains("Quarterly brief") && body.contains("padded rtf body"),
        "{body}"
    );
    assert_eq!(output.result.details["whole"], json!(2), "{body}");
    Ok(())
}

#[test]
fn a_spreadsheet_loses_its_empty_rows_and_padding() -> TestResult {
    let (scratch, tools) = setup("sheet")?;
    std::fs::copy(
        Path::new(FIXTURES).join("roster.xlsx"),
        scratch.cwd.join("roster.xlsx"),
    )?;
    let body = text(&call(
        &tool(&tools, "read")?,
        &scratch.cwd,
        json!({"path": "roster.xlsx"}),
    )?);
    assert!(body.contains("[sheets: Roster 4x3, Notes 2x2]"), "{body}");
    assert!(
        body.contains("| Zoë | Enrolled 1.00 | A |"),
        "padding squeezed: {body}"
    );
    assert!(!body.contains("|  |  |  |"), "empty rows dropped: {body}");
    assert!(body.contains("| Café budget | €1 240 |"), "{body}");
    Ok(())
}

#[test]
fn markdown_gets_an_outline_when_capped_and_find_returns_its_section() -> TestResult {
    let (scratch, tools) = setup("markdown")?;
    let mut long =
        String::from("# Title\n\nintro\n\n## Setup\n\nneedle line\nmore setup\n\n## Usage\n\n");
    for line in 0..2100 {
        long.push_str(&format!("filler {line}\n"));
    }
    std::fs::write(scratch.cwd.join("guide.md"), long)?;
    let read = tool(&tools, "read")?;
    let capped = text(&call(&read, &scratch.cwd, json!({"path": "guide.md"}))?);
    assert!(capped.contains("[outline: 3 of 3 headings"), "{capped}");
    assert!(capped.contains("  5: ## Setup"), "{capped}");
    let found = text(&call(
        &read,
        &scratch.cwd,
        json!({"path": "guide.md", "find": "needle"}),
    )?);
    assert!(
        found.contains("[find: block at lines 5-9 of"),
        "the section, not a fixed window: {found}"
    );
    Ok(())
}
