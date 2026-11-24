//! The replay player's own tests: a client that keeps to a recording passes, one that strays fails.
#![allow(dead_code, reason = "the module is shared with other test binaries")]

mod replay;

use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread::JoinHandle;

use replay::{Dir, Line, Mismatch, Played, Replay, play};
use serde_json::{Value, json};

type Check = Result<(), Box<dyn Error>>;
type Verdicts = (Result<Played, Mismatch>, Result<Vec<Value>, String>);

/// A client that sends what the recording sent under ids of its own, with `answer` for every ask.
fn client(
    lines: Vec<Line>,
    stream: UnixStream,
    answer: &'static str,
) -> JoinHandle<Result<Vec<Value>, String>> {
    std::thread::spawn(move || {
        let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(stream);
        let mut heard = Vec::new();
        for line in lines {
            let mut frame = line.frame;
            match line.dir {
                Dir::Recv => {
                    let mut raw = String::new();
                    reader.read_line(&mut raw).map_err(|e| e.to_string())?;
                    heard.push(serde_json::from_str(&raw).map_err(|e| e.to_string())?);
                }
                Dir::Send => {
                    if frame.contains_key("method") {
                        frame.insert("id".into(), json!(format!("own-{}", line.number)));
                    } else {
                        frame.insert(
                            "result".into(),
                            json!({"outcome": {"outcome": "selected", "optionId": answer}}),
                        );
                    }
                    writeln!(writer, "{}", Value::Object(frame)).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(heard)
    })
}

/// The player's verdict, and what the client heard (an error once the player hung up on it).
fn run(name: &str, answer: &'static str) -> Result<Verdicts, Box<dyn Error>> {
    let recording = Replay::fixture(name)?;
    let lines = recording
        .connections
        .first()
        .cloned()
        .ok_or("no connection")?;
    let (ours, theirs) = UnixStream::pair()?;
    let other = client(lines.clone(), theirs, answer);
    let played = play(&lines, ours);
    let heard = other.join().map_err(|_| "the client thread failed")?;
    Ok((played, heard))
}

#[test]
fn a_client_that_keeps_to_the_recording_passes() -> Check {
    let (played, heard) = run("approval-allow", "allow_once")?;
    assert_eq!(
        played?,
        Played {
            sent: 4,
            received: 47
        }
    );
    assert_eq!(
        heard?.first().and_then(|frame| frame.get("id")),
        Some(&json!("own-1"))
    );
    Ok(())
}

#[test]
fn a_different_answer_to_an_ask_fails_at_its_line() -> Check {
    let (played, _) = run("approval-allow", "reject_once")?;
    let error = played
        .err()
        .ok_or("a rejected ask matched an allowed one")?;
    assert!(matches!(error, Mismatch::Sent { line: 30, .. }), "{error}");
    assert!(error.to_string().contains("allow_once"), "{error}");
    Ok(())
}

#[test]
fn a_frame_past_the_recording_is_reported() -> Check {
    let lines = Replay::fixture("turn")?.connections.concat();
    let (ours, mut theirs) = UnixStream::pair()?;
    let other = client(lines.clone(), theirs.try_clone()?, "allow_once");
    let player = std::thread::spawn(move || play(&lines, ours));
    other.join().map_err(|_| "the client thread failed")??;
    writeln!(
        theirs,
        "{}",
        json!({"jsonrpc": "2.0", "id": 99, "method": "session/close"})
    )?;
    let error = player
        .join()
        .map_err(|_| "the player failed")?
        .err()
        .ok_or("an extra frame passed")?;
    assert!(matches!(error, Mismatch::Extra { .. }), "{error}");
    Ok(())
}

#[test]
fn a_resume_recording_plays_as_three_connections() -> Check {
    let recording = Replay::fixture("resume")?;
    assert_eq!(recording.connections.len(), 3);
    let socket = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("replay-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let server = recording.clone().serve(UnixListener::bind(&socket)?);
    for lines in recording.connections {
        client(lines, UnixStream::connect(&socket)?, "allow_once")
            .join()
            .map_err(|_| "the client thread failed")??;
    }
    let played = server.join().map_err(|_| "the player failed")??;
    std::fs::remove_file(&socket)?;
    assert_eq!(played.iter().map(|one| one.sent).sum::<usize>(), 9);
    Ok(())
}
