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
