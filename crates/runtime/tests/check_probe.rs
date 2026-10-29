//! A check that cannot start is refused before it runs: a syntax error, a program that is not
//! on the PATH, or a path that is not an executable file.

use std::error::Error;

type TestResult = Result<(), Box<dyn Error>>;

fn refusal(command: &str) -> Result<String, Box<dyn Error>> {
    yi_runtime::goal::probe(command).ok_or_else(|| format!("`{command}` was not refused").into())
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
    assert_eq!(yi_runtime::goal::probe("cd . && true"), None);
    Ok(())
}

#[test]
fn an_assignment_prefix_does_not_hide_the_program() -> TestResult {
    assert_eq!(yi_runtime::goal::probe("FOO=1 true"), None);
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
        assert_eq!(yi_runtime::goal::probe(command), None, "{command}");
    }
    Ok(())
}

fn on_path(word: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .any(|dir| std::fs::metadata(dir.join(word)).is_ok_and(|meta| meta.is_file()))
    })
}
