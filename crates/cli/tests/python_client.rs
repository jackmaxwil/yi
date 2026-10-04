//! `python/yi_client.py` against the real binary: a plain Python program fans out reader
//! children and gets their answers back with no model turn at the root.

use std::error::Error;
use std::process::Command;

use serde_json::Value;
use yi_runtime::faux::{faux_assistant_message, faux_text};
use yi_types::message::StopReason;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

#[expect(
    clippy::disallowed_methods,
    reason = "the client's contract is a separate process driving the spawned binary"
)]
fn run(program: &str, args: &[&str], dir: &std::path::Path) -> Result<String, Box<dyn Error>> {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("HOME", dir.join("home"))
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "{program} {args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(out.stdout)?)
}

const PROGRAM: &str = r#"
import json, sys
sys.path.insert(0, sys.argv[1])
from yi_client import Yi, YiError
yi_bin, repo, script, sessions = sys.argv[2:6]
with Yi(repo, model="faux/faux-1", yi=yi_bin, args=["--faux", script, "--session-dir", sessions]) as yi:
    answers = yi.ask_all(["first?", "second?", "third?"])
    big = yi.eval("'é' * 100000")
    try:
        yi.run("1/0")
        failed = None
    except YiError as error:
        failed = str(error)
print(json.dumps({"answers": answers, "big": len(big), "failed": failed}))
"#;

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_python_program_fans_out_readers_with_no_model_turn_at_the_root() -> TestResult {
    let dir = Scratch::new("yi-python-client")?;
    dir.home()?;
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo)?;
    std::fs::write(repo.join("README"), "probe\n")?;
    for args in [
        &["init", "-q"][..],
        &["add", "README"],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ],
    ] {
        run("git", args, &repo)?;
    }
    let replies = ["alpha", "beta", "gamma"]
        .iter()
        .map(|text| {
            serde_json::to_string(&faux_assistant_message(
                vec![faux_text(text)],
                StopReason::Stop,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let script = dir.join("script.jsonl");
    std::fs::write(&script, replies.join("\n"))?;
    let program = dir.join("program.py");
    std::fs::write(&program, PROGRAM)?;
    let client_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../python");
    let out = run(
        "python3",
        &[
            &program.display().to_string(),
            client_dir,
            env!("CARGO_BIN_EXE_yi"),
            &repo.display().to_string(),
            &script.display().to_string(),
            &dir.join("sessions").display().to_string(),
        ],
        &dir,
    )?;
    let report: Value = serde_json::from_str(out.trim())?;
    let mut answers: Vec<&str> = report["answers"]
        .as_array()
        .ok_or("no answers")?
        .iter()
        .map(|answer| answer["text"].as_str().unwrap_or_default())
        .collect();
    answers.sort_unstable();
    assert_eq!(
        answers,
        ["alpha", "beta", "gamma"],
        "three scripted replies feed three children only if the root took no model turn: {report}"
    );
    assert_eq!(
        report["big"], 100_000,
        "eval returns a value past the 64K output cap whole"
    );
    let failed = report["failed"]
        .as_str()
        .ok_or("a failing cell did not raise")?;
    assert!(failed.contains("ZeroDivisionError"), "{failed}");
    Ok(())
}
