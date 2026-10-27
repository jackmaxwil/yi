use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_tools::{CancelFlag, IpythonTool, KernelBridge, KernelCellOutcome, Tool, ToolContext};

use crate::tools::{output_text, read_tool, temp_dir, view_fixture};

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
            calls: None,
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
        _call_id: &str,
        _cancelled: &CancelFlag,
        _recovery_dir: Option<&std::path::Path>,
    ) -> Result<KernelCellOutcome, String> {
        Err("no kernel in this test".to_owned())
    }
}

/// Dies with every child card back on the timing guess: a child is placed under its spawning cell
/// only when the cell's own call id reaches the kernel, and an empty id records no cell.
#[test]
fn a_cell_hands_its_call_id_to_the_kernel() -> TestResult {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let tool = IpythonTool {
        bridge: Arc::new(Recording(Arc::clone(&seen))),
        calls: None,
    };
    let mut input = Map::new();
    input.insert("code".to_owned(), Value::String("1".to_owned()));
    let mut context = ToolContext::new(std::env::temp_dir());
    context.call_id = "toolu_01Ab".to_owned();
    tool.execute(input, &context);
    assert_eq!(*seen.lock().map_err(|e| e.to_string())?, ["toolu_01Ab"]);
    Ok(())
}

struct Recording(Arc<std::sync::Mutex<Vec<String>>>);

impl KernelBridge for Recording {
    fn execute_cell(
        &self,
        _code: &str,
        call_id: &str,
        _cancelled: &CancelFlag,
        _recovery_dir: Option<&std::path::Path>,
    ) -> Result<KernelCellOutcome, String> {
        self.0
            .lock()
            .map_err(|e| e.to_string())?
            .push(call_id.to_owned());
        Err("recorded".to_owned())
    }
}

/// Pillow 12.3's 1x1 RGB PNG.
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGM4+3g/AATwAnDE8Xs+AAAAAElFTkSuQmCC";

/// Incident: `attach_image` promised the model the picture, which reached only `details`.
/// A type or size the provider refuses would sit in history and fail every later request.
/// Its bytes are stored once: the details copy doubled every session row and event.
#[test]
fn an_attached_image_is_in_the_models_view() -> TestResult {
    let result: yi_types::kernel::ExecuteResult = serde_json::from_value(serde_json::json!({
        "stdout": "attached", "stderr": "", "status": "ok", "durationMs": 1,
        "attachments": [{"mime_type": "image/png", "data": PNG},
                        {"mime_type": "text/csv", "data": "YSxi"},
                        {"mime_type": "image/svg+xml", "data": "PHN2Zy8+"},
                        {"mime_type": "image/png", "data": "A".repeat(10_000_004)}],
    }))?;
    let outcome = KernelCellOutcome {
        result,
        kernel_restarted: false,
        notes: Vec::new(),
    };
    let stored = yi_tools::cell_output("x", outcome, None).result;
    let wire = serde_json::to_string(&stored)?;
    assert_eq!(wire.matches(PNG).count(), 1, "{wire}");
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
    assert!(
        text.contains("[image/png attachment of 10000004 base64 chars not sent to the model: the image is 10000004 base64 chars, over MAX_IMAGE_CHARS 10000000. If its source file is still on disk, run `print(await attach_image(path))` in ipython; it resizes to fit.]"),
        "{text}"
    );
    assert!(text.contains("the image is not png, jpeg, gif or webp. If its source file is still on disk, re-encode it"), "{text}");
    assert!(!text.contains("text/csv"), "{text}");
    let images: Vec<_> = content
        .into_iter()
        .filter_map(|block| match block {
            yi_types::message::Content::Image { data, mime_type } => Some((data, mime_type)),
            _ => None,
        })
        .collect();
    assert_eq!(images, vec![(PNG.to_owned(), "image/png".to_owned())]);
    Ok(())
}

/// Incident: a self-imposed 350k-char cap sent a 1 MB screenshot to the model as a note, where
/// the provider takes up to 10 MB of base64 per image. Pillow's PNG with a tEXt chunk, at the cap.
#[test]
fn an_image_up_to_the_providers_limit_reaches_the_model() -> TestResult {
    let data = format!(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAHJwjXRFWHRrZXkA{}YaWqugAAAAxJREFUeJxjOPt4PwAE8AJwxPF7PgAAAABJRU5ErkJggg==",
        "QUFB".repeat(2_499_971)
    );
    let result: yi_types::kernel::ExecuteResult = serde_json::from_value(serde_json::json!({
        "stdout": "", "stderr": "", "status": "ok", "durationMs": 1,
        "attachments": [{"mime_type": "image/png", "data": data}],
    }))?;
    let outcome = KernelCellOutcome {
        result,
        kernel_restarted: false,
        notes: Vec::new(),
    };
    let images = yi_tools::cell_output("x", outcome, None)
        .result
        .content
        .into_iter()
        .filter(|block| matches!(block, yi_types::message::Content::Image { .. }))
        .count();
    assert_eq!(images, 1);
    Ok(())
}

fn failed_cell(
    stdout: &str,
    stderr: &str,
    traceback: &[&str],
    notes: Vec<String>,
) -> serde_json::Result<KernelCellOutcome> {
    let last = traceback.last().copied().unwrap_or_default();
    let (ename, evalue) = last.split_once(": ").unwrap_or((last, ""));
    let result = serde_json::from_value(serde_json::json!({
        "stdout": stdout, "stderr": stderr, "status": "error", "durationMs": 1,
        "error": {"ename": ename, "evalue": evalue, "traceback": traceback},
    }))?;
    Ok(KernelCellOutcome {
        result,
        kernel_restarted: false,
        notes,
    })
}

/// A recursion traceback the kernel joined whole was 73,479 B of one repeated frame; the view
/// keeps its verdict line inline and tees the rest, which read opens.
#[test]
fn a_recursion_traceback_lands_under_the_ceiling_with_its_file() -> TestResult {
    let dir = temp_dir("cell-recursion")?;
    let raw = std::fs::read_to_string(view_fixture("view/recursion-traceback.txt"))?;
    let outcome = failed_cell("", "", &[&raw], Vec::new())?;
    let code = "def f(n):\n    return f(n + 1)\nf(0)";
    let text = output_text(&yi_tools::cell_output(code, outcome, Some(&dir)));
    assert!(text.len() <= yi_tools::VIEW_CEILING, "{} B", text.len());
    let last = "RecursionError: maximum recursion depth exceeded";
    assert!(text.lines().any(|line| line == last), "{text}");
    let file = text
        .lines()
        .last()
        .and_then(yi_tools::spill_path)
        .ok_or(text.clone())?
        .1;
    let input = Map::from_iter([("path".to_owned(), Value::String(file.to_owned()))]);
    let read = output_text(&read_tool().execute(input, &ToolContext::new(dir.to_path_buf())));
    assert!(read.contains("----> 4 f(0)"), "{read}");
    assert_eq!(std::fs::read_to_string(file)?, raw);
    Ok(())
}

/// A traceback under the reducer's floor reaches the model as the kernel joined it.
#[test]
fn a_short_traceback_reaches_the_model_byte_for_byte() -> TestResult {
    let dir = temp_dir("cell-keyerror")?;
    let raw = std::fs::read_to_string(view_fixture("view/pandas-keyerror-traceback.txt"))?;
    let outcome = failed_cell("", "", &[&raw], Vec::new())?;
    let text = output_text(&yi_tools::cell_output("df['total']", outcome, Some(&dir)));
    assert_eq!(text, raw);
    Ok(())
}

/// `%%bash` running a 3,000-test `cargo test` with one failure: the kernel kept the first 65,536
/// chars of stdout, so the verdict sat only in the spill; the cargo reducer reads it back inline.
#[test]
fn a_shell_cell_keeps_its_failing_test_and_summary_inline() -> TestResult {
    let dir = temp_dir("cell-cargo")?;
    let fixture = |name: &str| std::fs::read_to_string(view_fixture(name));
    let spill = dir.join("5b69550dd7781e9e92f754a050a0aad0.txt");
    std::fs::write(&spill, fixture("view/cargo-test-3000-cell-spill.txt")?)?;
    let pointer = format!("[full output: {}]", spill.display());
    let stderr = "    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.00s\n     Running unittests src/lib.rs (target/debug/deps/ledger-0d61fc207018dd0f)\nerror: test failed, to rerun pass `--lib`\n";
    let called = "CalledProcessError: Command 'b'cargo test --color never\\n'' returned non-zero exit status 101.";
    let traceback = ["-".repeat(75), "CalledProcessError                        Traceback (most recent call last)".to_owned(), "Cell In[3], line 1\n----> 1 get_ipython().run_cell_magic('bash', '', 'cargo test --color never\\n')\n".to_owned(), called.to_owned()];
    let traceback: Vec<&str> = traceback.iter().map(String::as_str).collect();
    let stdout = fixture("view/cargo-test-3000-cell-stdout.txt")?;
    let outcome = failed_cell(&stdout, stderr, &traceback, vec![pointer.clone()])?;
    let code = "%%bash\ncargo test --color never";
    let text = output_text(&yi_tools::cell_output(code, outcome, Some(&dir)));
    assert!(text.len() <= yi_tools::VIEW_CEILING, "{} B", text.len());
    for verdict in [
        "    tests::balance_1370",
        "test result: FAILED. 2999 passed; 1 failed",
        called,
    ] {
        assert!(
            text.lines().any(|line| line.starts_with(verdict)),
            "lost {verdict}: {text}"
        );
    }
    assert_eq!(text.matches(&pointer).count(), 1, "{text}");
    Ok(())
}

fn ran_cell(
    stdout: &str,
    stderr: &str,
    notes: Vec<String>,
) -> serde_json::Result<KernelCellOutcome> {
    let result = serde_json::from_value(serde_json::json!({
        "stdout": stdout, "stderr": stderr, "status": "ok", "durationMs": 1,
    }))?;
    Ok(KernelCellOutcome {
        result,
        kernel_restarted: false,
        notes,
    })
}

/// IPython's `!cmd` never raises on a non-zero exit, so a failing `!cargo test` arrives as an ok
/// cell; its merged streams come back CRLF from the pty, and the failing test still reads inline.
#[test]
fn a_bang_cargo_test_keeps_its_failing_test_inline() -> TestResult {
    let dir = temp_dir("cell-bang-cargo")?;
    let raw = std::fs::read_to_string(view_fixture("view/cargo-test-3000-cell-spill.txt"))?;
    let crlf = raw.replace('\n', "\r\n");
    let spill = dir.join("0c0b5a7f6d5e4f3a2b1c0d9e8f7a6b5c.txt");
    std::fs::write(&spill, &crlf)?;
    let head: String = crlf.chars().take(65_536).collect();
    let stdout = format!("{head}\n[... output truncated at 65536 chars ...]");
    let pointer = format!("[full output: {}]", spill.display());
    let outcome = ran_cell(&stdout, "", vec![pointer])?;
    let code = "!cargo test --color never";
    let text = output_text(&yi_tools::cell_output(code, outcome, Some(&dir)));
    assert!(text.len() <= yi_tools::VIEW_CEILING, "{} B", text.len());
    for verdict in [
        "    tests::balance_1370",
        "  left: 4110",
        "test result: FAILED.",
    ] {
        assert!(
            text.lines().any(|line| line.starts_with(verdict)),
            "lost {verdict}: {text}"
        );
    }
    Ok(())
}

/// A shell cell under the reducer's floor reads as the two streams always read, labels and all.
#[test]
fn a_small_shell_cell_reads_as_before() -> TestResult {
    let outcome = ran_cell("hi\n", "oops\n", Vec::new())?;
    let text = output_text(&yi_tools::cell_output(
        "%%bash\necho hi; echo oops >&2",
        outcome,
        None,
    ));
    assert_eq!(text, "hi\n\nstderr:\noops\n");
    Ok(())
}

/// What Python prints the model chose to print: 30 KB, past the view's ceiling, comes back whole,
/// also when a line of the cell opens with `!=`.
#[test]
fn a_python_print_is_read_as_printed() -> TestResult {
    let printed = std::fs::read_to_string(view_fixture("view/bash-cell-print.txt"))?;
    let continued = "mask = (\n    df.a\n    != df.b\n)\nprint(table)";
    for code in ["print(table)", continued] {
        let outcome = ran_cell(&printed, "", Vec::new())?;
        let text = output_text(&yi_tools::cell_output(code, outcome, None));
        assert!(
            text == printed,
            "{code:?}: {} of {} B",
            text.len(),
            printed.len()
        );
    }
    Ok(())
}

/// The kernel now hands a long shell cell over as its head, its own cut row and its tail. The
/// verdict reads inline either way, and with no file to name the kernel's row is still there,
/// so the first cut is not hidden behind the view's.
#[test]
fn a_shell_cell_reads_the_kernels_head_and_tail_capture() -> TestResult {
    let dir = temp_dir("cell-headtail")?;
    let fixture = |name: &str| std::fs::read_to_string(view_fixture(name));
    let stdout = fixture("view/cargo-test-3000-cell-stdout-headtail.txt")?;
    let spill = dir.join("7d1c2e3f4a5b6c7d8e9f0a1b2c3d4e5f.txt");
    std::fs::write(&spill, fixture("view/cargo-test-3000-cell-spill.txt")?)?;
    let pointer = format!("[full output: {}]", spill.display());
    let path: &std::path::Path = &dir;
    let called = "CalledProcessError: Command 'b'cargo test --color never\\n'' returned non-zero exit status 101.";
    for code in [
        "%%bash\ncargo test --color never",
        "!cargo test --color never",
    ] {
        for kept in [Some(path), None] {
            let notes = kept.map(|_| pointer.clone()).into_iter().collect();
            let outcome = match code.starts_with("%%bash") {
                true => failed_cell(&stdout, "", &[called], notes)?,
                false => ran_cell(&stdout, "", notes)?,
            };
            let text = output_text(&yi_tools::cell_output(code, outcome, kept));
            for verdict in ["    tests::balance_1370", "test result: FAILED."] {
                assert!(
                    text.lines().any(|line| line.starts_with(verdict)),
                    "{code:?} {kept:?}: lost {verdict}"
                );
            }
            let row = "[\u{2026} 65536 of 96614 chars kept, the middle cut";
            let named = match kept {
                Some(_) => text.matches(&pointer).count() == 1,
                None => text.lines().any(|line| line.starts_with(row)),
            };
            assert!(named, "{code:?} {kept:?}: the cut is not named: {text}");
        }
    }
    Ok(())
}
