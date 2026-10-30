//! A check that cannot start is refused before it runs: a syntax error, a program that is not
//! on the PATH, or a path that is not an executable file.

use std::error::Error;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn refusal(command: &str) -> Result<String, Box<dyn Error>> {
    yi_runtime::goal::probe(command, &std::env::temp_dir())
        .ok_or_else(|| format!("`{command}` was not refused").into())
}

#[test]
fn a_check_the_shell_cannot_parse_is_refused() -> TestResult {
    let said = refusal("if then fi")?;
    assert!(said.contains("cannot parse"), "{said}");
    assert!(said.contains("exit 2"), "{said}");
    assert!(said.contains("declare no check"), "{said}");
    Ok(())
}

#[test]
fn a_check_whose_program_is_absent_is_refused_by_name() -> TestResult {
    let said = refusal("jsut check")?;
    assert!(
        said.contains("`jsut` is not runnable (exit 127: not on the PATH)"),
        "{said}"
    );
    Ok(())
}

#[test]
fn a_directory_is_refused_as_not_runnable() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-probe-dir-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("build"))?;
    let said = refusal(&format!("{}/build", dir.display()))?;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        said.contains("(exit 126: a path that is not an executable file)"),
        "{said}"
    );
    Ok(())
}

#[test]
fn a_compound_command_is_probed_at_each_segment() -> TestResult {
    let said = refusal("cd . && yidog-not-a-program -q")?;
    assert!(said.contains("`yidog-not-a-program`"), "{said}");
    assert!(said.contains("first word of each segment"), "{said}");
    assert_eq!(
        yi_runtime::goal::probe("cd . && true", &std::env::temp_dir()),
        None
    );
    Ok(())
}

#[test]
fn an_assignment_prefix_does_not_hide_the_program() -> TestResult {
    assert_eq!(
        yi_runtime::goal::probe("FOO=1 true", &std::env::temp_dir()),
        None
    );
    let said = refusal("FOO=1 jsut check")?;
    assert!(said.contains("`jsut`"), "{said}");
    Ok(())
}

#[test]
fn the_life_sized_checks_pass_where_their_programs_exist() -> TestResult {
    for (command, program) in [
        ("just check", "just"),
        ("cd /app && python -m pytest tests/ -q", "python"),
    ] {
        if !on_path(program) {
            continue;
        }
        assert_eq!(
            yi_runtime::goal::probe(command, &std::env::temp_dir()),
            None,
            "{command}"
        );
    }
    Ok(())
}

#[test]
fn a_program_the_shell_expands_is_not_refused() {
    for command in [
        "$CARGO test",
        "\"$VENV/bin/python\" -m pytest",
        "`which true` -q",
    ] {
        assert_eq!(probe_here(command), None, "{command}");
    }
}

#[test]
fn a_relative_program_is_found_where_the_check_runs() -> TestResult {
    let scratch = Scratch::new("yi-probe-cwd")?;
    let dir = scratch.home()?;
    std::fs::write(dir.as_path().join("test.sh"), "#!/bin/sh\n")?;
    let mut mode = std::fs::metadata(dir.as_path().join("test.sh"))?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(dir.as_path().join("test.sh"), mode)?;
    assert_eq!(yi_runtime::goal::probe("./test.sh", dir.as_path()), None);
    let elsewhere = yi_runtime::goal::probe("./test.sh", &std::env::temp_dir());
    assert!(elsewhere.is_some_and(|said| said.contains("`./test.sh`")));
    Ok(())
}

fn probe_here(command: &str) -> Option<String> {
    yi_runtime::goal::probe(command, &std::env::temp_dir())
}

fn on_path(word: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .any(|dir| std::fs::metadata(dir.join(word)).is_ok_and(|meta| meta.is_file()))
    })
}
