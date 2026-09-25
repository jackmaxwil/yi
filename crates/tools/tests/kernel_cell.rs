use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_tools::{CancelFlag, IpythonTool, KernelBridge, KernelCellOutcome, Tool};

type TestResult = Result<(), Box<dyn Error>>;

/// The kernel is the one exec surface a static reader cannot follow: a `%%bash`
/// cell is a shell command and reads like one, a Python cell is allowed in auto
/// and gated by the phase-2 sandbox, and ask mode still stops both.
#[test]
fn a_shell_cell_is_read_as_a_shell_command() -> TestResult {
    for (code, allowed) in [
        ("%%bash\ncargo test", true),
        ("%%bash\nrm -rf target", false),
        ("!git status", true),
        ("!git reset --hard", false),
        ("total = sum(len(r) for r in results)", true),
    ] {
        let mut input = Map::new();
        input.insert("code".to_owned(), Value::String(code.to_owned()));
        let tool = IpythonTool {
            bridge: Arc::new(NoKernel),
        };
        assert_eq!(!tool.irreversible(&input), allowed, "{code:?}");
    }
    Ok(())
}

struct NoKernel;

impl KernelBridge for NoKernel {
    fn execute_cell(
        &self,
        _code: &str,
        _cancelled: &CancelFlag,
    ) -> Result<KernelCellOutcome, String> {
        Err("no kernel in this test".to_owned())
    }
}

/// Incident: `attach_image` promised the model the picture, which reached only `details`.
/// A type or size the provider refuses would sit in history and fail every later request.
/// Its bytes are stored once: the details copy doubled every session row and event.
#[test]
fn an_attached_image_is_in_the_models_view() -> TestResult {
    let result: yi_types::kernel::ExecuteResult = serde_json::from_value(serde_json::json!({
        "stdout": "attached", "stderr": "", "status": "ok", "durationMs": 1,
        "attachments": [{"mime_type": "image/png", "data": "iVBORw0KGgo="},
                        {"mime_type": "text/csv", "data": "YSxi"},
                        {"mime_type": "image/svg+xml", "data": "PHN2Zy8+"},
                        {"mime_type": "image/png", "data": "A".repeat(10_000_004)}],
    }))?;
    let outcome = KernelCellOutcome {
        result,
        kernel_restarted: false,
        notes: Vec::new(),
    };
    let stored = yi_tools::cell_output("x", outcome).result;
    let wire = serde_json::to_string(&stored)?;
    assert_eq!(wire.matches("iVBORw0KGgo=").count(), 1, "{wire}");
    assert_eq!(wire.matches("YSxi").count(), 1, "{wire}");
    let content = stored.content;
    let text: String = content
        .iter()
        .filter_map(|block| match block {
            yi_types::message::Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.contains("image/svg+xml"), "{text}");
    assert!(text.contains("10000004"), "{text}");
    assert!(!text.contains("text/csv"), "{text}");
    let images: Vec<_> = content
        .into_iter()
        .filter_map(|block| match block {
            yi_types::message::Content::Image { data, mime_type } => Some((data, mime_type)),
            _ => None,
        })
        .collect();
    assert_eq!(
        images,
        vec![("iVBORw0KGgo=".to_owned(), "image/png".to_owned())]
    );
    Ok(())
}

/// Incident: a self-imposed 350k-char cap sent a 1 MB screenshot to the model as a note, where
/// the provider takes up to 10 MB of base64 per image.
#[test]
fn an_image_up_to_the_providers_limit_reaches_the_model() -> TestResult {
    let data = "A".repeat(1_000_000);
    let result: yi_types::kernel::ExecuteResult = serde_json::from_value(serde_json::json!({
        "stdout": "", "stderr": "", "status": "ok", "durationMs": 1,
        "attachments": [{"mime_type": "image/jpeg", "data": data}],
    }))?;
    let outcome = KernelCellOutcome {
        result,
        kernel_restarted: false,
        notes: Vec::new(),
    };
    let images = yi_tools::cell_output("x", outcome)
        .result
        .content
        .into_iter()
        .filter(|block| matches!(block, yi_types::message::Content::Image { .. }))
        .count();
    assert_eq!(images, 1);
    Ok(())
}
