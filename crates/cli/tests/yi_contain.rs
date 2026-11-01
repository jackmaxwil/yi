//! `yi __contain` from outside. Every failure must run nothing: exit 126 and no marker.
use std::error::Error;

type TestResult = Result<(), Box<dyn Error>>;

fn yi() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_yi"))
}

#[test]
fn a_wrapper_that_cannot_apply_runs_nothing() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-contain-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let marker = dir.join("ran");
    let touch = ["/bin/touch", marker.to_str().ok_or("not UTF-8")?];
    let cases: [Vec<&str>; 3] = [
        vec!["--nonsense", "--"],
        vec!["--read", "/no/such/root", "--"],
        vec!["--read", "/usr"],
    ];
    for flags in cases {
        let ran = yi().arg("__contain").args(&flags).args(touch).output()?;
        assert_eq!(ran.status.code(), Some(126), "{flags:?}: {ran:?}");
        assert!(!marker.exists(), "{flags:?} ran the command");
    }
    let bare = yi().args(["__contain", "--read", "/usr", "--"]).output()?;
    assert_eq!(
        bare.status.code(),
        Some(126),
        "no command names nothing to run"
    );
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
