//! `yi doctor`'s two kernel rows: what would build the venv, and a real boot timed
//! (built under `--fix`, which is what the trial adapter runs at install).

use std::path::{Path, PathBuf};

use yi_kernel::client::{ExecuteOptions, KernelManager, KernelOptions};

use crate::kernel::rlm_bootstrap_code;

fn doctor_options(home: &Path) -> yi_kernel::bootstrap::BootstrapOptions {
    yi_kernel::bootstrap::BootstrapOptions {
        on_progress: Some(Box::new(|message: &str| eprintln!("{message}"))),
        home: home.to_path_buf(),
        runtime_source_dir: yi_kernel::bootstrap::default_runtime_source_dir(),
        skills_source_dir: yi_kernel::bootstrap::default_skills_source_dir(),
        toolchain: None,
        venv_dir: None,
    }
}

/// `yi doctor`'s `kernel-toolchain` row: what would build the venv, or why nothing can.
pub fn doctor_toolchain(home: &Path) -> Result<String, String> {
    if let Some(python) = std::env::var_os("YI_KERNEL_PYTHON") {
        return Ok(format!("YI_KERNEL_PYTHON {}", Path::new(&python).display()));
    }
    yi_kernel::bootstrap::find_toolchain(&doctor_options(home))
        .map(|toolchain| toolchain.describe())
}

/// `yi doctor`'s `kernel-boot` row: the venv (built under `fix`, else only reported), then a
/// real kernel booted and the runtime cell run, each timed. `Ok((fixed, detail))`.
pub fn doctor_boot(home: &Path, fix: bool) -> Result<(bool, String), String> {
    let options = doctor_options(home);
    let (python, fixed, build) = match yi_kernel::bootstrap::ready_kernel_python(&options) {
        Some(python) => (python, false, "cached".to_owned()),
        None if fix => {
            let started = std::time::Instant::now();
            let python = yi_kernel::bootstrap::ensure_kernel_python(&options)?;
            (
                python,
                true,
                format!("built {} ms", started.elapsed().as_millis()),
            )
        }
        None => return Err("venv not built; `yi doctor --fix` builds it".to_owned()),
    };
    let home = home.to_path_buf();
    let timings = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .map_err(|error| error.to_string())?
            .block_on(boot_once(python, home))
    })
    .join()
    .map_err(|_| "kernel boot thread panicked".to_owned())??;
    Ok((fixed, format!("{build} · {timings}")))
}

async fn boot_once(python: PathBuf, home: PathBuf) -> Result<String, String> {
    let started = std::time::Instant::now();
    let manager = KernelManager::new(KernelOptions {
        python: Some(python),
        cwd: None,
        env: Vec::new(),
        username: "yi".to_owned(),
        home,
        runtime_source_dir: yi_kernel::bootstrap::default_runtime_source_dir(),
        host: None,
        on_progress: None,
        snapshot: None,
        wrap: None,
    })?;
    manager.start().await?;
    let start_ms = started.elapsed().as_millis();
    let cell = std::time::Instant::now();
    let result = manager
        .execute(&rlm_bootstrap_code(&[]), ExecuteOptions::default())
        .await
        .map_err(|error| error.to_string());
    manager.dispose().await;
    let result = result?;
    if result.status != yi_types::kernel::ExecuteStatus::Ok {
        return Err(format!("runtime cell failed: {}", result.stderr));
    }
    Ok(format!(
        "start {start_ms} ms · bootstrap {} ms",
        cell.elapsed().as_millis()
    ))
}
