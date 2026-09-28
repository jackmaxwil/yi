use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, json};
use std::sync::atomic::{AtomicBool, Ordering};

use yi_tools::{Converter, Documents, Tool, ToolContext, builtin_tools, builtin_tools_with};
use yi_types::message::Content;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn read_of(tools: Vec<Arc<dyn Tool>>) -> Result<Arc<dyn Tool>, Box<dyn Error>> {
    Ok(tools
        .into_iter()
        .find(|tool| tool.name() == "read")
        .ok_or("no read tool")?)
}

fn unbuilt(home: PathBuf, formats: &[&str]) -> Documents {
    Documents::fixed(
        home.clone(),
        Converter {
            python: home.join("no-venv").join("bin").join("python"),
            formats: formats.iter().map(|format| (*format).to_owned()).collect(),
        },
    )
}

#[test]
fn the_description_claims_only_the_recorded_formats() -> TestResult {
    let home = std::env::temp_dir();
    let base = read_of(builtin_tools())?.description().to_owned();
    let unrecorded = read_of(builtin_tools_with(false, Some(unbuilt(home.clone(), &[]))))?;
    assert_eq!(
        unrecorded.description(),
        base,
        "a venv that recorded no format adds no claim"
    );
    let recorded = read_of(builtin_tools_with(
        false,
        Some(unbuilt(home, &["docx", "pdf"])),
    ))?;
    assert!(
        recorded
            .description()
            .contains("converted to Markdown: docx, pdf."),
        "{}",
        recorded.description()
    );
    Ok(())
}

#[test]
fn an_unbuilt_venv_is_named_beside_the_plain_error() -> TestResult {
    let root = Scratch::new("yi-documents-unbuilt")?;
    let path = root.join("brief.docx");
    std::fs::write(&path, [b'P', b'K', 0x03, 0x04, 0xff, 0xfe, 0x00, 0x81])?;
    let read = read_of(builtin_tools_with(
        false,
        Some(unbuilt(root.to_path_buf(), &[])),
    ))?;
    let input: Map<String, serde_json::Value> =
        serde_json::from_value(json!({"path": "brief.docx"}))?;
    let output = read.execute(input, &ToolContext::new(root.to_path_buf()));
    let text: String = output
        .result
        .content
        .iter()
        .map(|content| match content {
            Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect();
    assert!(output.is_error);
    assert_eq!(
        text,
        format!(
            "failed to read {}: a binary file (NUL bytes), and not a document the converter reads\n[the kernel venv that converts documents is not built yet; it builds at session start or on the first ipython call — retry in a moment]",
            path.display()
        )
    );
    Ok(())
}

/// `pages=` on a docx was passed to a converter that only pages PDFs: the whole document came
/// back under a header naming the pages, and the paged size ceiling let a 1 GiB file through.
#[test]
fn pages_on_an_office_document_is_refused_like_plain_text() -> TestResult {
    let root = Scratch::new("yi-documents-pages-docx")?;
    std::fs::write(
        root.join("brief.docx"),
        [b'P', b'K', 0x03, 0x04, 0xff, 0xfe, 0x00, 0x81],
    )?;
    let read = read_of(builtin_tools_with(
        false,
        Some(unbuilt(root.to_path_buf(), &["docx", "pdf"])),
    ))?;
    let input: Map<String, serde_json::Value> =
        serde_json::from_value(json!({"path": "brief.docx", "pages": "1"}))?;
    let output = read.execute(input, &ToolContext::new(root.to_path_buf()));
    assert!(output.is_error);
    assert_eq!(output.result.details["errorKind"], json!("invalid_args"));
    let text: String = output
        .result
        .content
        .iter()
        .map(|content| match content {
            Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(text, "pages= applies to a PDF");
    Ok(())
}

/// Incident: the forge live lane's bash-marker read 0 cached tokens because the venv landed
/// between its first and second request and rewrote the tool table in front of the transcript.
#[test]
fn the_tool_table_holds_still_when_the_venv_lands_mid_session() -> TestResult {
    let built = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&built);
    let documents = Documents {
        converter: Arc::new(move || Converter {
            python: PathBuf::from("/nonexistent"),
            formats: if flag.load(Ordering::SeqCst) {
                vec!["docx".to_owned()]
            } else {
                Vec::new()
            },
        }),
        ..Documents::fixed(std::env::temp_dir(), Converter::default())
    };
    let table = |tools: &[Arc<dyn Tool>]| -> String {
        tools
            .iter()
            .map(|tool| {
                format!(
                    "{}\n{}\n{}\n",
                    tool.name(),
                    tool.description(),
                    tool.schema()
                )
            })
            .collect()
    };
    let session = builtin_tools_with(false, Some(documents.clone()));
    let before = table(&session);
    built.store(true, Ordering::SeqCst);
    assert_eq!(table(&session), before, "the venv landed mid-session");
    assert!(!before.contains("docx"));
    let next = read_of(builtin_tools_with(false, Some(documents)))?;
    assert!(next.description().contains("converted to Markdown: docx."));
    Ok(())
}

#[test]
fn bash_points_a_habitual_converter_at_read() -> TestResult {
    let tools = builtin_tools();
    let bash = tools
        .iter()
        .find(|tool| tool.name() == "bash")
        .ok_or("no bash tool")?;
    let input: Map<String, serde_json::Value> = serde_json::from_value(
        json!({"command": "pdftotext -layout nothing.pdf - 2>/dev/null; true"}),
    )?;
    let output = bash.execute(input, &ToolContext::new(std::env::temp_dir()));
    let text: String = output
        .result
        .content
        .iter()
        .map(|content| match content {
            Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect();
    assert!(
        text.contains("read <path> replaces pdftotext here"),
        "{text}"
    );
    Ok(())
}
