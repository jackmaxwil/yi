use std::error::Error;
use std::path::PathBuf;
use std::process::Output;

use crate::scratch::Scratch;
use crate::session_files::{frames_text, session_text};
use crate::tui_drive::cassette_lines;
use crate::yi_bin::yi_command;

type TestResult = Result<(), Box<dyn Error>>;

/// One headless drive of `keys` over the cassette `replies`, with `config` laid over the
/// scratch home's own; the scratch directory comes back holding `frames` and `sessions`.
fn drive(
    name: &str,
    config: Option<serde_json::Value>,
    replies: &[&str],
    keys: &str,
) -> Result<(Output, Scratch), Box<dyn Error>> {
    let dir = Scratch::new(name)?;
    let home = home_with(&dir, config)?;
    let cassette = dir.join("cassette.jsonl");
    let lines: String = (replies.iter())
        .map(|reply| cassette_lines(&[], reply))
        .collect();
    std::fs::write(&cassette, lines)?;
    let script = dir.join("script.keys");
    std::fs::write(&script, keys)?;
    let output = yi_command(&home)
        .args(["tui", "--headless", "--here", "--size", "120x30"])
        .args(["--model", "faux/faux-1", "--deadline", "60", "--faux"])
        .arg(&cassette)
        .arg("--session-dir")
        .arg(dir.join("sessions"))
        .arg("--keys")
        .arg(&script)
        .arg("--frames")
        .arg(dir.join("frames"))
        .current_dir(&dir)
        .output()?;
    Ok((output, dir))
}

/// The scratch home, its config extended by `config`'s keys.
fn home_with(dir: &Scratch, config: Option<serde_json::Value>) -> Result<PathBuf, Box<dyn Error>> {
    let home = dir.home()?;
    if let Some(serde_json::Value::Object(extra)) = config {
        let path = home.join(".yi/config.json");
        let mut held: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        let held_map = held
            .as_object_mut()
            .ok_or("the home's config is no object")?;
        held_map.extend(extra);
        std::fs::write(&path, held.to_string())?;
    }
    Ok(home)
}

fn rewriter(shape: serde_json::Value) -> Option<serde_json::Value> {
    Some(serde_json::json!({"models": {"rewriter": "faux/faux-1"}, "rewrite": shape}))
}

#[test]
fn enter_shows_the_rewrite_over_the_typed_words_and_a_second_enter_sends_both() -> TestResult {
    let keys = "wait-frame 20000 improve: off · short · [medium] · long\n\
                type fix the tset\nkey enter\n\
                wait-frame 20000 improved · enter sends · esc restores\n\
                wait-frame 20000 \u{2502}Fix the flaky test.\n\
                wait-frame 20000 \u{2502}> fix the tset\n\
                key enter\nwait-frame 20000 the main model answered\nwait-idle 20000\nquit\n";
    let replies = ["Fix the flaky test.", "the main model answered"];
    let shape = rewriter(serde_json::json!({}));
    let (output, dir) = drive("yi-rewrite-send", shape, &replies, keys)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "drive run must exit 0: {stderr}");
    let sent = r"Fix the flaky test.\n\nMy words, verbatim:\n> fix the tset";
    let session = session_text(&dir.join("sessions"))?;
    assert!(session.contains(sent), "not sent: {session}");
    Ok(())
}

#[test]
fn a_rewrite_that_cannot_run_says_why_and_the_draft_goes_as_typed() -> TestResult {
    let keys = "wait-frame 20000 improve: off\ntype fix the tset\nkey enter\n\
                wait-frame 20000 as typed · enter sends\n\
                key enter\nwait-frame 20000 the main model answered\nwait-idle 20000\nquit\n";
    let shape = serde_json::json!({"skill": "absent", "length": "long"});
    let replies = ["the main model answered"];
    let (output, dir) = drive("yi-rewrite-fail", rewriter(shape), &replies, keys)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "drive run must exit 0: {stderr}");
    let shown = frames_text(&dir.join("frames"))?;
    for needle in [
        "improve: off · short · medium · [long]",
        "no rewrite: rewrite.skill names `absent`, which no skill root holds",
        "enter sends your draft as typed",
    ] {
        assert!(shown.contains(needle), "{needle} missing from the frames");
    }
    let session = session_text(&dir.join("sessions"))?;
    assert!(session.contains("fix the tset") && !session.contains("My words, verbatim"));
    Ok(())
}

#[test]
fn with_no_rewriter_role_the_composer_draws_no_control() -> TestResult {
    let keys = "wait-frame 20000 faux-1\ntype fix the tset\nkey enter\n\
                wait-frame 20000 the main model answered\nwait-idle 20000\nquit\n";
    let replies = ["the main model answered"];
    let (output, dir) = drive("yi-rewrite-absent", None, &replies, keys)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "drive run must exit 0: {stderr}");
    assert!(!frames_text(&dir.join("frames"))?.contains("improve"));
    Ok(())
}

/// The console reaches its sessions over ACP: the control comes from the session's open result
/// and the rewrite from `_yi/rewrite`, with the faux model echoing the request it was asked.
#[test]
fn a_console_pane_shows_the_control_and_sends_the_rewrite_with_the_typed_words() -> TestResult {
    let dir = Scratch::new("yi-rewrite-console")?;
    let home = home_with(&dir, rewriter(serde_json::json!({})))?;
    let root = dir.join("repo");
    std::fs::create_dir_all(&root)?;
    let (mut daemon, socket) = crate::daemon::spawn_daemon_in(&dir, &home)?;
    let keys = dir.join("script.keys");
    let outcome = (|| -> TestResult {
        std::fs::write(
            &keys,
            "wait-frame 8000 !connecting\u{2026}\nkey alt-n\n\
             wait-frame 20000 improve: off \u{b7} short \u{b7} [medium] \u{b7} long\n\
             type fix the tset\nkey enter\n\
             wait-frame 30000 improved \u{b7} enter sends \u{b7} esc restores\n\
             wait-frame 20000 My words, verbatim:\n\
             wait-frame 20000 > fix the tset\n\
             key enter\nwait-idle 30000\nquit\n",
        )?;
        let output = yi_command(&home)
            .args(["console", "--headless", "--socket"])
            .arg(&socket)
            .arg("--cwd")
            .arg(&root)
            .arg("--keys")
            .arg(&keys)
            .arg("--frames")
            .arg(dir.join("frames"))
            .output()?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "console run must exit 0: {stderr}");
        Ok(())
    })();
    let _ = daemon.kill();
    let _ = daemon.wait();
    outcome?;
    let session = session_text(&dir.join("sessions"))?;
    let sent = r"My words, verbatim:\n> fix the tset";
    assert!(session.contains(sent), "not sent: {session}");
    Ok(())
}
