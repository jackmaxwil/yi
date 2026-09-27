//! Import probes of a kernel python, and the stamp a passing ready probe leaves in its venv.
use std::path::{Path, PathBuf};

const READY_STAMP_FILE: &str = ".ready-stamp";

/// The names `python` fails to import. A python that cannot answer fails every name.
pub(crate) fn failed_imports(python: &Path, names: &[&str]) -> Vec<String> {
    let every = || names.iter().map(|name| (*name).to_owned()).collect();
    let Ok(list) = serde_json::to_string(names) else {
        return every();
    };
    let code = format!(
        "import importlib, json\nfailed = []\nfor name in {list}:\n    try:\n        importlib.import_module(name)\n    except BaseException:\n        failed.append(name)\nprint(json.dumps(failed))"
    );
    crate::bootstrap::output(python, &["-c", &code])
        .ok()
        .and_then(|text| serde_json::from_str(text.lines().last()?.trim()).ok())
        .unwrap_or_else(every)
}

/// The interpreter the venv links to and each site-packages an install rewrites, by size
/// and mtime; `None` when one is unreadable, so the live probe runs.
fn fingerprint(python: &Path, venv: &Path) -> Option<String> {
    let mut paths = vec![std::fs::canonicalize(python).ok()?];
    let mut sites: Vec<PathBuf> = std::fs::read_dir(venv.join("lib"))
        .ok()?
        .map(|lib| lib.map(|lib| lib.path().join("site-packages")))
        .collect::<Result<_, _>>()
        .ok()?;
    sites.sort();
    paths.append(&mut sites);
    let check = crate::bootstrap::fnv1a(crate::bootstrap::RUNTIME_READY_CHECK.as_bytes());
    let mut stamp = format!("{check:016x}\n");
    for path in paths {
        let meta = std::fs::metadata(&path).ok()?;
        let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH);
        let nanos = modified.ok()?.as_nanos();
        stamp.push_str(&format!("{} {} {nanos}\n", path.display(), meta.len()));
    }
    Some(stamp)
}

pub(crate) fn stamped(python: &Path, venv: &Path) -> bool {
    fingerprint(python, venv).is_some_and(|now| {
        std::fs::read_to_string(venv.join(READY_STAMP_FILE)).is_ok_and(|then| then == now)
    })
}

pub(crate) fn stamp(python: &Path, venv: &Path) {
    if let Some(now) = fingerprint(python, venv) {
        let _best_effort = std::fs::write(venv.join(READY_STAMP_FILE), now);
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use crate::bootstrap::{kernel_python, kernel_ready, write_bootstrap_version};
    use crate::scratch::Scratch;
    use std::path::Path;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn counting_python(venv: &Path, log: &Path, reply: &str) -> TestResult {
        std::fs::create_dir_all(venv.join("bin"))?;
        let script = format!(
            "#!/bin/sh\necho run >> '{}'\necho '{reply}'\n",
            log.display()
        );
        std::fs::write(kernel_python(venv), script)?;
        let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
        std::fs::set_permissions(kernel_python(venv), mode)?;
        Ok(())
    }

    fn runs(log: &Path) -> usize {
        std::fs::read_to_string(log).map_or(0, |text| text.lines().count())
    }

    #[test]
    fn a_stamped_venv_skips_the_import_probe_until_site_packages_changes() -> TestResult {
        let root = Scratch::new("yi-kernel-stamp")?;
        let venv = root.join("kernel-venv-bbbb0000");
        let site = venv.join("lib").join("python3.13").join("site-packages");
        std::fs::create_dir_all(&site)?;
        let log = root.join("runs");
        counting_python(&venv, &log, "")?;
        write_bootstrap_version(&venv, "sha256:x", Vec::new())?;
        let python = kernel_python(&venv);
        assert!(kernel_ready(&python, &venv, "sha256:x"));
        assert_eq!(runs(&log), 1, "the first ready check proves the venv live");
        assert!(kernel_ready(&python, &venv, "sha256:x"));
        assert_eq!(runs(&log), 1, "a stamped venv spawned its python again");
        std::fs::create_dir(site.join("installed_later"))?;
        std::fs::File::open(&site)?.set_modified(std::time::SystemTime::UNIX_EPOCH)?;
        assert!(kernel_ready(&python, &venv, "sha256:x"));
        assert_eq!(runs(&log), 2, "an install into the venv must re-prove it");
        Ok(())
    }

    #[test]
    fn one_interpreter_answers_for_every_extra() -> TestResult {
        let root = Scratch::new("yi-kernel-extras")?;
        let log = root.join("runs");
        counting_python(&root, &log, r#"["numpy"]"#)?;
        let python = kernel_python(&root);
        assert_eq!(crate::bootstrap::missing_extra_packages(&python), ["numpy"]);
        assert_eq!(runs(&log), 1, "each extra spawned its own python");
        Ok(())
    }
}
