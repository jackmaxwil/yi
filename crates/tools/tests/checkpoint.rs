use std::fs;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FOREIGN_INDEX: &str = "yi-checkpoint-foreign-index";

#[test]
fn undo_restores_and_removes_non_ascii_paths() -> TestResult {
    let root = Scratch::new("yi-checkpoint-non-ascii")?;
    let project = root.join("project");
    fs::create_dir_all(&project)?;
    fs::write(project.join("naïve.txt"), "before\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&root.join("shadow"), &project)?;
    let turn_start = checkpoints.capture()?;
    fs::write(project.join("naïve.txt"), "after\n")?;
    fs::write(project.join("日本.txt"), "new\n")?;

    let mut changed = checkpoints.restore(&turn_start, None)?;
    changed.sort_by(|left, right| left.path.cmp(&right.path));
    let names: Vec<String> = changed
        .iter()
        .map(|change| change.path.display().to_string())
        .collect();
    assert_eq!(names, ["naïve.txt", "日本.txt"]);
    assert_eq!(fs::read_to_string(project.join("naïve.txt"))?, "before\n");
    assert!(
        !project.join("日本.txt").exists(),
        "the added file is removed"
    );
    Ok(())
}

#[test]
fn show_and_changed_return_every_byte_past_the_tool_output_cap() -> TestResult {
    let root = Scratch::new("yi-checkpoint-large")?;
    let project = root.join("project");
    fs::create_dir_all(&project)?;
    let body: String = (0..4_000).map(|line| format!("line {line:05}\n")).collect();
    fs::write(project.join("big.txt"), &body)?;
    let names: Vec<String> = (0..400)
        .map(|n| format!("{n:03}-{}.txt", "x".repeat(80)))
        .collect();
    for name in &names {
        fs::write(project.join(name), "before\n")?;
    }
    let checkpoints = yi_tools::Checkpoints::open(&root.join("shadow"), &project)?;
    let tree = checkpoints.capture()?;
    assert_eq!(checkpoints.show(&tree, "big.txt")?, body);
    for name in &names {
        fs::write(project.join(name), "after\n")?;
    }
    let mut changed: Vec<String> = checkpoints
        .changed(&tree)?
        .iter()
        .map(|change| change.path.display().to_string())
        .collect();
    changed.sort();
    assert_eq!(changed, names);
    Ok(())
}

#[test]
fn a_capture_leaves_the_callers_git_index_untouched() -> TestResult {
    let foreign = std::env::var_os("GIT_INDEX_FILE")
        .filter(|path| path.to_string_lossy().ends_with(FOREIGN_INDEX));
    let Some(index) = foreign else {
        let root = Scratch::new("yi-checkpoint-index-rerun")?;
        let test = crate::exact_path(
            module_path!(),
            "a_capture_leaves_the_callers_git_index_untouched",
        );
        let rerun = yi_tools::command(std::env::current_exe()?)
            .args(["--exact", &test])
            .env("GIT_INDEX_FILE", root.join(FOREIGN_INDEX))
            .output()?;
        let said = String::from_utf8_lossy(&rerun.stdout) + String::from_utf8_lossy(&rerun.stderr);
        assert!(
            rerun.status.success() && said.contains("1 passed"),
            "{said}"
        );
        return Ok(());
    };
    let root = Scratch::new("yi-checkpoint-index")?;
    let project = root.join("project");
    fs::create_dir_all(&project)?;
    fs::write(project.join("kept.txt"), "before\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&root.join("shadow"), &project)?;
    let tree = checkpoints.capture()?;
    fs::write(project.join("kept.txt"), "after\n")?;
    checkpoints.restore(&tree, None)?;
    assert_eq!(fs::read_to_string(project.join("kept.txt"))?, "before\n");
    assert!(
        !std::path::Path::new(&index).exists(),
        "the shadow git wrote the caller's GIT_INDEX_FILE"
    );
    Ok(())
}
