use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_runtime::kernel_tools::ipython_tool;
use yi_runtime::permission::PermissionBroker;
use yi_runtime::{HostRegistry, KernelService, KernelServiceOptions, PermissionMode};

type TestResult = Result<(), Box<dyn Error>>;

fn tool(cwd: &std::path::Path, sandbox: Option<yi_tools::Sandbox>) -> Arc<dyn yi_tools::Tool> {
    let service = KernelService::new(KernelServiceOptions {
        cwd: cwd.to_path_buf(),
        home: cwd.join("home"),
        session_dir: None,
        family_dir: None,
        host: Arc::new(HostRegistry::default()),
        on_restore: None,
        on_boot: None,
        sandbox,
        snapshot_key: None,
        per_session_state: false,
        cell_ceiling: None,
    });
    ipython_tool(Arc::new(service), None)
}

/// Whether an Auto broker with nobody to ask lets the cell run, judged as the tool seam judges it.
fn runs(tool: &Arc<dyn yi_tools::Tool>, cwd: &std::path::Path, code: &str) -> bool {
    let broker = PermissionBroker::new(
        PermissionMode::Auto,
        cwd.to_path_buf(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    );
    let mut args = Map::new();
    args.insert("code".to_owned(), Value::String(code.to_owned()));
    let (kind, irreversible) = (tool.kind(), tool.irreversible(&args));
    (broker.decide_call(tool.name(), kind, irreversible, "c1", &args, None)).allowed
}

/// Off macOS nothing contains the kernel, so a Python cell has the user's whole reach: auto asks
/// before it (headless, that is a refusal), while a shell cell keeps the bash verdict.
#[test]
fn auto_asks_before_an_uncontained_python_cell() -> TestResult {
    let cwd = PathBuf::from("/home/user/project");
    let uncontained = tool(&cwd, None);
    assert!(
        !runs(&uncontained, &cwd, "print(1)"),
        "an uncontained cell ran"
    );
    assert!(
        runs(&uncontained, &cwd, "%%bash\nls"),
        "a read-only shell cell asked"
    );
    assert!(
        !runs(&uncontained, &cwd, "%%bash\nrm -rf target"),
        "an rm cell ran"
    );
    Ok(())
}

/// Seatbelt is part of every supported macOS, so a missing one fails here instead of passing.
#[cfg(target_os = "macos")]
#[test]
fn auto_runs_a_contained_python_cell() -> TestResult {
    if !yi_tools::Sandbox::available() {
        return Err("Seatbelt is missing on macOS; no cell can be contained".into());
    }
    let cwd = PathBuf::from("/home/user/project");
    let home = cwd.join("home");
    let contained = tool(
        &cwd,
        Some(yi_tools::Sandbox::for_workspace(&cwd, &home, None)),
    );
    assert!(runs(&contained, &cwd, "print(1)"), "a contained cell asked");
    assert!(
        !runs(&contained, &cwd, "%%bash\nrm -rf target"),
        "an rm cell ran"
    );
    Ok(())
}
