#![cfg(target_os = "macos")]

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use yi_runtime::{HostRegistry, KernelService, KernelServiceOptions};
use yi_tools::{CancelFlag, KernelBridge, Sandbox};

type TestResult = Result<(), Box<dyn Error>>;

async fn cell(
    service: &Arc<KernelService>,
    code: String,
) -> Result<yi_tools::KernelCellOutcome, String> {
    let service = Arc::clone(service);
    tokio::task::spawn_blocking(move || {
        let cancelled: CancelFlag = Arc::new(|| false);
        KernelBridge::execute_cell(service.as_ref(), &code, &cancelled)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn workspace(tag: &str) -> Result<(PathBuf, PathBuf, PathBuf), Box<dyn Error>> {
    let root = std::env::temp_dir().join(format!("yi-kernel-sbx-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    let session = root.join("session");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&session)?;
    Ok((project, home, session))
}

fn service(cwd: PathBuf, home: PathBuf, sandbox: Sandbox) -> Arc<KernelService> {
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    Arc::new(KernelService::new(KernelServiceOptions {
        cwd,
        home,
        session_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        sandbox: Some(sandbox),
    }))
}

#[tokio::test]
async fn an_ipython_cell_cannot_write_outside_the_confined_roots() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (project, home, session) = workspace("write")?;
    let sandbox = Sandbox::for_workspace(&project, &home, Some(&session));
    let kernel = service(project.clone(), home.clone(), sandbox);
    let inside = cell(
        &kernel,
        "open('inside.txt','w').write('in')\nprint('ok')".to_owned(),
    )
    .await?;
    assert_eq!(inside.result.status, yi_types::kernel::ExecuteStatus::Ok);
    assert_eq!(std::fs::read_to_string(project.join("inside.txt"))?, "in");

    let escape = home.join(format!("yi-p7-escape-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&escape);
    let path = escape.display().to_string();
    let outside = cell(&kernel, format!("open(r'{path}','w').write('out')")).await?;
    assert_eq!(
        outside.result.status,
        yi_types::kernel::ExecuteStatus::Error
    );
    let blob = format!(
        "{}{}",
        outside.result.stderr,
        outside
            .result
            .error
            .as_ref()
            .map(|error| error.traceback.join("\n"))
            .unwrap_or_default()
    )
    .to_lowercase();
    assert!(
        blob.contains("permission") || blob.contains("not permitted") || blob.contains("errno"),
        "outside write must fail: {blob}"
    );
    assert!(
        !escape.is_file(),
        "the file must not exist after a contained write"
    );
    kernel.dispose().await;
    let _ = std::fs::remove_file(&escape);
    Ok(())
}

#[tokio::test]
async fn a_profile_change_restarts_the_kernel() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (project, home, session) = workspace("restart")?;
    let sandbox = Sandbox::for_workspace(&project, &home, Some(&session));
    let kernel = service(project.clone(), home.clone(), sandbox.clone());
    let first = cell(&kernel, "marker = 1\nprint(marker)".to_owned()).await?;
    assert_eq!(first.result.status, yi_types::kernel::ExecuteStatus::Ok);

    let extra = home.join(format!("yi-p7-extra-{}", std::process::id()));
    std::fs::create_dir_all(&extra)?;
    let mut next = sandbox;
    next.writable.push(extra.clone());
    kernel.set_sandbox(Some(next)).await;

    let second = cell(&kernel, "print(marker)".to_owned()).await?;
    assert_eq!(second.result.status, yi_types::kernel::ExecuteStatus::Error);
    let name = second
        .result
        .error
        .as_ref()
        .map(|error| error.ename.as_str())
        .unwrap_or("");
    assert_eq!(name, "NameError", "{name}");
    kernel.dispose().await;
    let _ = std::fs::remove_dir_all(&extra);
    Ok(())
}
