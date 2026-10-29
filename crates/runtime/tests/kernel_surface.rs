//! The table named a typing alias as a function, and the pointer named `help(rlm)`, which
//! renders the whole package (45,631 bytes) instead of the one callable asked about.

use std::error::Error;
use std::path::PathBuf;

type TestResult = Result<(), Box<dyn Error>>;

/// The shipped table code, run against the shipped package in a plain python3: the same
/// string the kernel executes, without booting one.
#[expect(
    clippy::disallowed_methods,
    reason = "the contract under test is a real python process importing the shipped package"
)]
fn surface_table() -> Result<String, Box<dyn Error>> {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python/yi_runtime/src");
    let code = format!("import rlm\n{}", yi_runtime::kernel_bootstrap::SURFACE_CODE);
    let output = std::process::Command::new("python3")
        .args(["-c", &code])
        .env("PYTHONPATH", &src)
        .current_dir(std::env::temp_dir())
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "the surface table did not run: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// `rlm.HarnessScope` is a `Literal`, not a function: a row for it teaches a call that does
/// not exist, and a name listed twice hides the alias behind the real one it points at.
#[test]
fn the_table_lists_one_row_per_callable_and_no_typing_alias() -> TestResult {
    let table = surface_table()?;
    assert!(table.contains("\nrlm.bash(command)"), "{table}");
    assert!(
        !table.contains("HarnessScope"),
        "a typing alias is not a callable: {table}"
    );
    for row in table.lines() {
        let call = row.strip_prefix("await ").unwrap_or(row);
        let signature = call.strip_prefix("rlm.").unwrap_or_default();
        assert!(
            signature.contains('(') && !signature.starts_with('('),
            "not a signature row: {row}"
        );
    }
    assert_eq!(
        table
            .lines()
            .filter(|row| row.contains("rlm.get_harness_state("))
            .count(),
        1,
        "one row per callable: {table}"
    );
    Ok(())
}

/// `help(rlm)` renders the whole package; the note has to send the model to help on one name.
#[test]
fn the_note_points_at_help_on_one_name() -> TestResult {
    let note = yi_runtime::kernel_bootstrap::surface_note("rlm.bash(command)");
    let line = yi_runtime::kernel_bootstrap::surface_line();
    for text in [&note, &line] {
        assert!(text.contains("help(rlm.<name>)"), "{text}");
        assert!(
            !text.contains("help(rlm)"),
            "help(rlm) is the 45 KB dump: {text}"
        );
    }
    Ok(())
}
