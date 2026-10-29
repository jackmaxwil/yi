use std::error::Error;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext, WriteTool};

use crate::tools::{output_text, temp_dir};

type TestResult = Result<(), Box<dyn Error>>;

const MB: usize = 1024 * 1024;

fn write(path: &str, content: &str, context: &ToolContext) -> Result<(), String> {
    let mut args = Map::new();
    args.insert("path".to_owned(), json!(path));
    args.insert("content".to_owned(), Value::String(content.to_owned()));
    let output = WriteTool::default().execute(args, context);
    match output.is_error {
        true => Err(output_text(&output)),
        false => Ok(()),
    }
}

/// A watcher reading mid-write saw an empty or half file, since the write truncated first.
#[test]
fn readers_polling_during_rewrites_never_read_a_short_file() -> TestResult {
    let dir = temp_dir("write-atomic-readers")?;
    let context = ToolContext::new(dir.to_path_buf());
    let path = dir.join("big.txt");
    let bodies = ["a".repeat(MB), "b".repeat(MB)];
    fs::write(&path, &bodies[0])?;
    let done = Arc::new(AtomicBool::new(false));
    let short = Arc::new(AtomicUsize::new(0));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let (done, short, path) = (Arc::clone(&done), Arc::clone(&short), path.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    if fs::read(&path).is_ok_and(|bytes| bytes.len() != MB) {
                        short.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();
    let wrote = (0..200).try_for_each(|n| write("big.txt", &bodies[n % 2], &context));
    done.store(true, Ordering::Relaxed);
    for reader in readers {
        reader.join().map_err(|_| "a reader panicked")?;
    }
    wrote?;
    assert_eq!(
        short.load(Ordering::Relaxed),
        0,
        "reads that saw a short file"
    );
    Ok(())
}

#[test]
fn a_name_swapped_for_a_symlink_before_the_rename_is_refused() -> TestResult {
    let dir = temp_dir("write-atomic-swap")?;
    let (path, victim) = (dir.join("a.txt"), dir.join("victim.txt"));
    fs::write(&path, "judged\n")?;
    fs::write(&victim, "kept\n")?;
    let file = fs::OpenOptions::new().read(true).write(true).open(&path)?;
    let mut swap = || {
        let _ = fs::remove_file(&path);
        let _ = std::os::unix::fs::symlink(&victim, &path);
    };
    let landed = yi_tools::replace_after(&file, &path, b"new\n", &mut swap);
    assert!(landed.is_err(), "the swapped name was written");
    assert!(fs::symlink_metadata(&path)?.file_type().is_symlink());
    assert_eq!(fs::read_to_string(&victim)?, "kept\n");
    Ok(())
}

#[test]
fn a_hard_linked_files_other_name_sees_the_new_bytes() -> TestResult {
    let dir = temp_dir("write-atomic-link")?;
    let context = ToolContext::new(dir.to_path_buf());
    fs::write(dir.join("a.txt"), "old\n")?;
    fs::hard_link(dir.join("a.txt"), dir.join("b.txt"))?;
    write("a.txt", "new\n", &context)?;
    assert_eq!(fs::read_to_string(dir.join("b.txt"))?, "new\n");
    Ok(())
}

#[test]
fn a_rewritten_file_keeps_its_mode() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("write-atomic-mode")?;
    let context = ToolContext::new(dir.to_path_buf());
    fs::write(dir.join("run.sh"), "#!/bin/sh\n")?;
    fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o750))?;
    write("run.sh", "#!/bin/sh\necho hi\n", &context)?;
    let mode = fs::metadata(dir.join("run.sh"))?.permissions().mode() & 0o777;
    assert_eq!(mode, 0o750);
    Ok(())
}
