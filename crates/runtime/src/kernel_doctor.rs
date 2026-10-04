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
    match yi_kernel::bootstrap::existing_toolchain(home) {
        Some(toolchain) => Ok(toolchain.describe()),
        None => yi_kernel::uv_install::Release::pinned().map(|_| {
            let version = yi_kernel::uv_install::UV_VERSION;
            format!("no uv or python3 3.11+; the venv build fetches uv {version}")
        }),
    }
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
        connection_dir: None,
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

/// `name:port` of each listener in `lsof -nP +c 0 -iTCP -sTCP:LISTEN`, once per IP family. All are
/// reachable: Seatbelt's `localhost` is every address of this host, the LAN one included.
pub fn reachable_listeners(lsof: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for line in lsof.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (Some(name), Some(address)) = (fields.first(), fields.iter().rev().nth(1)) else {
            continue;
        };
        let port = address.rsplit(':').next().unwrap_or(address);
        let listener = format!("{}:{port}", name.replace("\\x20", " "));
        if !seen.contains(&listener) {
            seen.push(listener);
        }
    }
    seen
}

/// The `sandbox-listeners` row's text: `lsof` read for two seconds, its failure said in the row.
pub fn sandbox_listeners() -> String {
    if !cfg!(target_os = "macos") {
        return "no sandbox on this platform".to_owned();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let cancelled: yi_tools::CancelFlag =
        std::sync::Arc::new(move || std::time::Instant::now() >= deadline);
    let mut lsof = yi_tools::command("/usr/sbin/lsof");
    lsof.args(["-nP", "+c", "0", "-iTCP", "-sTCP:LISTEN"]);
    match yi_tools::run_captured(lsof, None, &cancelled, 1 << 20) {
        Ok(capture) if capture.cancelled || capture.truncated => {
            "listeners not listed: lsof was cut at 2 s or 1 MiB".to_owned()
        }
        Ok(capture) => listeners_detail(&reachable_listeners(&capture.stdout)),
        Err(error) => format!("listeners not listed: lsof did not run ({error})"),
    }
}

/// The row's text, its cap named where it cuts.
pub fn listeners_detail(listeners: &[String]) -> String {
    const SHOWN: usize = 8;
    let shown = listeners.iter().take(SHOWN).cloned().collect::<Vec<_>>();
    let cut = match listeners.len().checked_sub(SHOWN) {
        Some(more) if more > 0 => format!(
            "; [{SHOWN} of {} listeners shown, cap sandbox-listeners={SHOWN}; `lsof -nP -iTCP -sTCP:LISTEN` lists all]",
            listeners.len()
        ),
        _ => String::new(),
    };
    format!(
        "{} TCP listeners a contained process can reach: {}{cut}",
        listeners.len(),
        shown.join(", ")
    )
}
