//! A name the snapshot could not take is named again at resume: the skip list beside the index
//! reaches the restore marker, so a revived kernel says which variables did not come back and why.

use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use yi_kernel::snapshot::{build_restore_code, build_snapshot_code, parse_restore_result};

type TestResult = Result<(), Box<dyn Error>>;

/// pickle standing in for dill: the skip list is the contract, not the serializer's format.
const STUB_DILL: &str = "import pickle

settings = {}


def dump(value, buffer):
    pickle.dump(value, buffer)


def dumps(value):
    return pickle.dumps(value)


def load(fh):
    return pickle.load(fh)


def loads(blob):
    return pickle.loads(blob)
";

const NAMESPACE: &str = r#"class _IP:
    user_ns = {"small": 1, "big": bytearray(200)}
    user_ns_hidden = {}


def get_ipython():
    return _IP()
"#;

const RESTORE_NAMESPACE: &str = r#"class _IP:
    user_ns = {}
    user_ns_hidden = {}


def get_ipython():
    return _IP()
"#;

fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!("yi-snap-skips-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn python_step(dir: &Path, args: &[&OsStr]) -> Result<String, Box<dyn Error>> {
    let stub = dir.join("stub");
    fs::create_dir_all(&stub)?;
    fs::write(stub.join("dill.py"), STUB_DILL)?;
    #[expect(
        clippy::disallowed_methods,
        reason = "the generated helpers must run in a real python3 process"
    )]
    let output = Command::new("python3")
        .args(args)
        .env("PYTHONPATH", &stub)
        .output()?;
    let said = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "python step failed: {said}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(said)
}

fn run(dir: &Path, namespace: &str, code: &str) -> Result<String, Box<dyn Error>> {
    let helper_path = dir.join("helper.py");
    fs::write(&helper_path, code)?;
    let driver_path = dir.join("driver.py");
    let driver = format!(
        "import sys\n{namespace}\npath = sys.argv[1]\n\
         exec(compile(open(path).read(), path, \"exec\"), {{\"get_ipython\": get_ipython, \"__name__\": \"driven\"}})\n\
         print(\"_YI_TEST_RESTORE_FAILURES=\" + repr(get_ipython().user_ns.get(\"_yi_restore_failures\")))\n"
    );
    fs::write(&driver_path, driver)?;
    python_step(dir, &[driver_path.as_os_str(), helper_path.as_os_str()])
}

#[test]
fn a_name_the_snapshot_skipped_comes_back_named_at_resume() -> TestResult {
    let dir = scratch("resume")?;
    let state = dir.join("kernel-state.dill");
    let snapshot = build_snapshot_code(&state, 1 << 20, 64, false);
    let said = run(&dir, NAMESPACE, &snapshot)?;
    assert!(
        said.contains("\"saved\": [\"small\"]"),
        "the small name is saved and the oversized one is not: {said}"
    );
    assert!(
        said.contains("\"name\": \"big\"") && said.contains("size cap"),
        "the snapshot marker counts the name it left out: {said}"
    );
    let restore = build_restore_code(&state);
    let said = run(&dir, RESTORE_NAMESPACE, &restore)?;
    let result = parse_restore_result(&said, &state).ok_or("no restore marker line")?;
    assert_eq!(
        result.restored,
        vec!["small"],
        "only the saved name is restored: {said}"
    );
    let names: Vec<&str> = result
        .failed
        .iter()
        .map(|failed| failed.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["big"],
        "the name the snapshot skipped is reported as not restored: {said}"
    );
    assert!(
        result.failed[0].reason.contains("size cap"),
        "the reason travels with it: {}",
        result.failed[0].reason
    );
    assert!(
        said.contains("_YI_TEST_RESTORE_FAILURES=[{'name': 'big', 'reason': 'exceeds per-variable snapshot size cap'}]"),
        "the revived namespace keeps the continuation list: {said}"
    );
    assert!(
        !state.with_extension("skips.json").exists(),
        "the skip list rides the index, not a file beside it"
    );
    let skips = index_skips(&dir, &state)?;
    assert!(
        skips.contains("\"name\": \"big\"") && skips.contains("size cap"),
        "the index carries the skipped name and its reason: {skips}"
    );
    Ok(())
}

/// The `_skips` value the index holds, read back by the same stub serializer.
fn index_skips(dir: &Path, path: &Path) -> Result<String, Box<dyn Error>> {
    index_step(
        dir,
        "import dill, json, sys; print(json.dumps(dill.load(open(sys.argv[1], 'rb'))['_skips']))",
        path,
    )
}

fn index_step(dir: &Path, code: &str, path: &Path) -> Result<String, Box<dyn Error>> {
    python_step(dir, &[OsStr::new("-c"), OsStr::new(code), path.as_os_str()])
}

#[test]
fn a_skip_entry_of_the_wrong_shape_still_lets_the_marker_line_decode() -> TestResult {
    let dir = scratch("shape")?;
    let state = dir.join("kernel-state.dill");
    index_step(
        &dir,
        "import dill, sys; skip = [{'name': 'gone', 'reason': 3}, {'name': 7, 'reason': 'x'}, 'junk']; open(sys.argv[1], 'wb').write(dill.dumps({'small': dill.dumps(7), '_skips': skip}))",
        &state,
    )?;
    let restore = build_restore_code(&state);
    let said = run(&dir, RESTORE_NAMESPACE, &restore)?;
    let result = parse_restore_result(&said, &state).ok_or("the marker line must decode")?;
    assert_eq!(result.restored, vec!["small"], "{said}");
    let failed: Vec<(&str, &str)> = result
        .failed
        .iter()
        .map(|entry| (entry.name.as_str(), entry.reason.as_str()))
        .collect();
    assert_eq!(
        failed,
        vec![("gone", "")],
        "only a named skip rides the marker, with a reason the wire accepts: {said}"
    );
    Ok(())
}
