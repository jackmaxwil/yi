use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::sandbox::Sandbox;
use crate::tool::CancelFlag;

/// A document is attacker-shaped parser input; past this size it is refused, never parsed.
const INPUT_CEILING: u64 = 64 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const STEM_CHARS: usize = 64;
/// `ocr="reject"` is the only mode ever passed: anydoc's hosted OCR uploads the document, and no
/// argument, setting or message leads there. Content markers name the format before the extension.
const CONVERT: &str = r#"import json, sys
source, out = sys.argv[1], sys.argv[2]
def say(**fields):
    print(json.dumps(fields))
try:
    import anydoc
except ImportError as error:
    say(missing=str(error))
    sys.exit()
with open(source, "rb") as handle:
    data = handle.read()
try:
    kind = anydoc.format_from_bytes(data) or anydoc.format_from_path(source)
    markdown = anydoc.to_markdown_bytes(data, kind, ocr="reject")
except anydoc.UnsupportedError:
    say(unsupported=True)
except anydoc.NeedsOcrError as error:
    pdf_type = "image-only"
    try:
        import pdf_inspector
        pdf_type = str(pdf_inspector.detect_pdf(source).pdf_type).replace("_", "-")
    except Exception:
        pass
    say(ocr=pdf_type, pages=len(error.pages), page_count=error.page_count)
except anydoc.ConvertError as error:
    say(error=f"{type(error).__name__}: {error}")
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(markdown)
    say(ok=True)
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Documents {
    pub python: PathBuf,
    pub formats: Vec<String>,
    pub home: PathBuf,
}

pub(crate) enum Converted {
    Markdown(PathBuf),
    NotADocument,
    Unavailable(String),
    Refused(String),
}

pub(crate) fn describe(base: &str, documents: Option<&Documents>) -> String {
    match documents {
        Some(documents) if !documents.formats.is_empty() => format!(
            "{base} A file that is not UTF-8 text is converted to Markdown when it is one of: {}. The Markdown is a read-only copy; edit and write refuse the original.",
            documents.formats.join(", ")
        ),
        _ => base.to_owned(),
    }
}

fn sidecar(home: &Path, source: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(source).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let canonical = source.canonicalize().ok()?;
    let key = format!("{}\0{modified}\0{}", canonical.display(), meta.len());
    let stem: String = source
        .file_stem()?
        .to_string_lossy()
        .chars()
        .take(STEM_CHARS)
        .collect();
    let low = xxhash_rust::xxh32::xxh32(key.as_bytes(), 0);
    let high = xxhash_rust::xxh32::xxh32(key.as_bytes(), 1);
    Some(
        home.join(".yi")
            .join("converted")
            .join(format!("{stem}-{high:08x}{low:08x}.md")),
    )
}

pub(crate) fn source_refusal(documents: Option<&Documents>, path: &Path) -> Option<String> {
    let copy = sidecar(&documents?.home, path)?;
    copy.is_file().then(|| {
        format!(
            "{} was read as a converted document and cannot be changed here: its Markdown copy {} is not written back. Change the original in the application that made it.",
            path.display(),
            copy.display()
        )
    })
}

pub(crate) fn convert(documents: &Documents, source: &Path, cancelled: &CancelFlag) -> Converted {
    let Some(copy) = sidecar(&documents.home, source) else {
        return Converted::NotADocument;
    };
    if copy.is_file() {
        return Converted::Markdown(copy);
    }
    if !documents.python.is_file() {
        return Converted::Unavailable(
            "[no document converter yet: the kernel venv is not built; the first ipython call builds it]"
                .to_owned(),
        );
    }
    let size = std::fs::metadata(source).map_or(0, |meta| meta.len());
    if size > INPUT_CEILING {
        return Converted::Refused(format!(
            "{size} bytes is past the {INPUT_CEILING}-byte ceiling for converting a document"
        ));
    }
    let Some(dir) = copy.parent() else {
        return Converted::NotADocument;
    };
    if let Err(error) = std::fs::create_dir_all(dir) {
        return Converted::Refused(format!("cannot create {}: {error}", dir.display()));
    }
    let staging = copy.with_extension(format!("{}.tmp", std::process::id()));
    let outcome = run(documents, dir, source, &staging, cancelled);
    let converted = match outcome {
        Ok(()) => std::fs::rename(&staging, &copy)
            .map(|()| Converted::Markdown(copy))
            .unwrap_or_else(|error| Converted::Refused(format!("cannot keep the copy: {error}"))),
        Err(converted) => converted,
    };
    let _staging_gone_or_never_written = std::fs::remove_file(&staging);
    converted
}

/// Seatbelt on macOS: no network and one writable root, since a parser escape needs neither.
fn run(
    documents: &Documents,
    dir: &Path,
    source: &Path,
    staging: &Path,
    cancelled: &CancelFlag,
) -> Result<(), Converted> {
    let python = documents.python.to_string_lossy().into_owned();
    let source_text = source.to_string_lossy().into_owned();
    let staging_text = staging.to_string_lossy().into_owned();
    let args = [
        "-I",
        "-c",
        CONVERT,
        source_text.as_str(),
        staging_text.as_str(),
    ];
    let mut command = if Sandbox::available() {
        let (program, wrapped) =
            Sandbox::for_workspace(dir, &documents.home, None).wrap(&python, &args);
        let mut command = crate::process::command(program);
        command.args(wrapped);
        command
    } else {
        let mut command = crate::process::command(&documents.python);
        command.args(args);
        command
    };
    command.current_dir(dir);
    let deadline = Instant::now() + TIMEOUT;
    let caller = Arc::clone(cancelled);
    let stop: CancelFlag = Arc::new(move || caller() || Instant::now() > deadline);
    let capture =
        crate::process::run_captured(command, None, &stop, 8 * 1024).map_err(Converted::Refused)?;
    if capture.cancelled {
        return Err(Converted::Refused(format!(
            "conversion stopped (cancelled, or past {}s)",
            TIMEOUT.as_secs()
        )));
    }
    let status = capture
        .stdout
        .lines()
        .last()
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .unwrap_or_default();
    let text = |key: &str| status.get(key).and_then(serde_json::Value::as_str);
    let count = |key: &str| status.get(key).and_then(serde_json::Value::as_u64);
    if status.get("ok").is_some() {
        return Ok(());
    }
    if status.get("unsupported").is_some() {
        return Err(Converted::NotADocument);
    }
    if let Some(missing) = text("missing") {
        return Err(Converted::Unavailable(format!(
            "[no document converter: the kernel venv lacks anydoc ({missing})]"
        )));
    }
    if let (Some(pdf_type), Some(pages), Some(total)) =
        (text("ocr"), count("pages"), count("page_count"))
    {
        return Err(Converted::Refused(format!(
            "a {pdf_type} PDF, {pages} of {total} pages without a text layer; it cannot be read as text"
        )));
    }
    let reason = text("error").map_or_else(
        || {
            let last = capture.stderr.trim().lines().last().unwrap_or("no output");
            format!("the converter failed: {last}")
        },
        str::to_owned,
    );
    Err(Converted::Refused(reason))
}
