//! The socket's edges: a dead server, a stopped one, a stuck one, and the timer.

use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;

use super::{Check, Plan, Rig, serve};
use crate::command::table::Target;
use crate::ids::ThreadId;
use crate::kind::Kind;
use crate::reduce::state::Link;
use crate::reduce::step::commands::{PROMPT, SETTINGS_READ};
use crate::reduce::step::tests::{address, rows};
use crate::wire::client::{OUT_QUEUE, Starter, Unsent};
use crate::wire::event::Down;
use crate::wire::frame::Refusal;
use crate::wire::request::Request;

fn down(rig: &mut Rig, why: &Down) -> Check {
    rig.until(
        "the client reports it",
        |model| matches!(&model.wire.link, Link::Down { why: found, .. } if found == why),
    )
}

/// A starter that counts its calls and, like `yi serve`, starts listening on the socket.
fn starter(calls: &Arc<AtomicUsize>) -> Result<Starter, Box<dyn std::error::Error>> {
    let calls = Arc::clone(calls);
    // Rows 0 and 1 of any recording: `initialize` and the server's answer.
    let hello: Vec<_> = rows("turn.jsonl")?.into_iter().take(2).collect();
    Ok(Box::new(move |socket: &Path| {
        calls.fetch_add(1, Ordering::Relaxed);
        let listener = UnixListener::bind(socket)?;
        let hello = hello.clone();
        std::thread::Builder::new().spawn(move || {
            let (_held, gate) = std::sync::mpsc::channel();
            drop(serve(&listener, &hello, Plan::default(), &gate));
        })?;
        Ok(())
    }))
}

#[test]
fn a_dead_socket_starts_the_server_and_connects_to_it() -> Check {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut rig = Rig::bare(Some(starter(&calls)?), None)?;
    rig.until(
        "the started server answers",
        |model| matches!(&model.wire.link, Link::Up { server, .. } if server == "0.1041.0"),
    )?;
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "one dead socket, one start"
    );
    rig.finish()
}

#[test]
fn a_server_stopped_on_purpose_is_never_started_again() -> Check {
    let calls = Arc::new(AtomicUsize::new(0));
    let socket = super::socket_path()?;
    // What `_yi/shutdown` leaves beside the socket.
    let marker = socket.with_extension("stopped");
    std::fs::write(&marker, b"")?;
    let mut rig = Rig::on(socket, Some(starter(&calls)?), None)?;
    // The report is made after a failed connect, where a start would have been asked for.
    down(&mut rig, &Down::Stopped)?;
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "the stop marker was ignored"
    );
    std::fs::remove_file(&marker)?;
    rig.until("the marker is gone and the server is started", |model| {
        matches!(model.wire.link, Link::Up { .. })
    })?;
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    rig.finish()
}

#[test]
fn a_server_that_speaks_another_protocol_is_never_taken_as_connected() -> Check {
    let mut hello: Vec<_> = rows("turn.jsonl")?.into_iter().take(2).collect();
    let answer = hello.get_mut(1).ok_or("the recording has no answer")?;
    answer.1["result"]["protocolVersion"] = json!(1);
    let mut rig = Rig::start(hello, Plan::default())?;
    rig.until("the client makes up its mind", |model| {
        !matches!(model.wire.link, Link::Starting)
    })?;
    let Link::Down {
        why: Down::Handshake(why),
        ..
    } = &rig.model.wire.link
    else {
        return Err(format!("the link is {:?}", rig.model.wire.link).into());
    };
    assert!(
        why.contains("protocol 1") && why.contains("protocol 2"),
        "the reason names neither side's version: {why}"
    );
    rig.finish()
}

#[test]
fn a_request_made_while_offline_comes_back_unsent() -> Check {
    let mut rig = Rig::bare(None, None)?;
    down(&mut rig, &Down::NoServer)?;
    let ticket = rig.model.wire.ticket();
    rig.run(SETTINGS_READ, Target::App, None)?;
    rig.until("the request comes back", |model| {
        model.wire.failed(&ticket).is_some()
    })?;
    let why = rig.model.wire.failed(&ticket).map(|failure| &failure.why);
    assert_eq!(why, Some(&Refusal::Unsent));
    rig.finish()
}

#[test]
fn a_prompt_the_socket_never_took_whole_goes_back_to_the_composer() -> Check {
    // Rows 0 and 1 are `initialize`; after them the script reads nothing more.
    let hello: Vec<_> = rows("turn.jsonl")?.into_iter().take(2).collect();
    let mut rig = Rig::start(hello, Plan::default())?;
    rig.until("the server answers", |model| {
        matches!(model.wire.link, Link::Up { .. })
    })?;
    // More words than a socket holds unread: the write is still under way when the server
    // hangs up, so the frame never reaches it whole.
    let words = "word ".repeat(1 << 18);
    let thread = ThreadId::new("01a1de97-bf6c-7514-9fae-b57db1eaa7e1");
    let ticket = rig.model.wire.ticket();
    rig.run(
        PROMPT,
        address(Kind::Thread, thread.as_str())?,
        Some(&words),
    )?;
    rig.go()?;
    rig.until("the prompt comes back", |model| {
        model.wire.failed(&ticket).is_some()
    })?;
    let why = rig.model.wire.failed(&ticket).map(|failure| &failure.why);
    assert_eq!(why, Some(&Refusal::Unsent));
    let composer = rig.model.wire.threads.get(&thread);
    let composer = composer.and_then(|thread| thread.prefill.as_deref());
    assert!(composer == Some(words.as_str()), "the words were dropped");
    rig.finish()
}

#[test]
fn a_stuck_server_fills_the_queue_and_the_next_request_is_refused() -> Check {
    let socket = super::socket_path()?;
    // A server that takes the connection and never answers `initialize`.
    let _listener = UnixListener::bind(&socket)?;
    let rig = Rig::on(socket.clone(), None, None)?;
    let request = || Request::call(7, "_yi/settings", json!({}));
    let client = rig.client()?;
    let queued = (0..OUT_QUEUE)
        .filter(|_| client.send(request()).is_ok())
        .count();
    assert_eq!(queued, OUT_QUEUE, "the queue holds its whole bound");
    assert_eq!(client.send(request()), Err(Unsent::Full { max: OUT_QUEUE }));
    drop(rig);
    std::fs::remove_file(&socket)?;
    Ok(())
}

#[test]
fn closing_stops_both_threads_while_a_connection_is_up() -> Check {
    let hello: Vec<_> = rows("turn.jsonl")?.into_iter().take(2).collect();
    let mut rig = Rig::start(hello, Plan::default())?;
    rig.until("the server answers", |model| {
        matches!(model.wire.link, Link::Up { .. })
    })?;
    // `finish` closes the client with a deadline and fails if its threads outlive it.
    rig.finish()
}

#[test]
fn the_timer_asks_for_the_thread_list_while_connected() -> Check {
    // Rows 0 and 1 are `initialize`; rows 27 and 28 are `session/list` and its answer.
    let recorded = rows("lists.jsonl")?;
    let script: Vec<_> = [0, 1, 27, 28]
        .iter()
        .filter_map(|row| recorded.get(*row).cloned())
        .collect();
    let every = Some(Duration::from_millis(20));
    let mut rig = Rig::polling(script, Plan::default(), every)?;
    rig.until("the polled list arrives", |model| {
        model.wire.threads.len() == 6
    })?;
    rig.finish()
}

#[test]
fn closing_writes_every_answer_queued_before_the_stop() -> Check {
    use std::io::{BufRead, BufReader, Write};

    // More than the socket holds unread, so the writer is still busy when the client closes.
    const ANSWERS: usize = 200;
    let filler = "c".repeat(64 << 10);
    let socket = super::socket_path()?;
    let listener = UnixListener::bind(&socket)?;
    let (go, gate) = std::sync::mpsc::channel::<()>();
    let (report, heard) = std::sync::mpsc::channel();
    std::thread::Builder::new().spawn(move || {
        let serve = || -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
            let (stream, _) = listener.accept()?;
            let mut writer = stream.try_clone()?;
            let mut lines = BufReader::new(stream).lines();
            let hello: serde_json::Value = serde_json::from_str(&lines.next().ok_or("no hello")??)?;
            let result = json!({"protocolVersion": 2, "agentInfo": {"name": "yi", "version": "0"}});
            let answer = json!({"jsonrpc": "2.0", "id": hello["id"], "result": result});
            writeln!(writer, "{answer}")?;
            gate.recv()?;
            Ok(lines
                .map_while(Result::ok)
                .filter(|line| line.contains("cancelled"))
                .count())
        };
        drop(report.send(serve().map_err(|error| error.to_string())));
    })?;
    let mut rig = Rig::on(socket.clone(), None, None)?;
    rig.until("the server answers", |model| {
        matches!(model.wire.link, Link::Up { .. })
    })?;
    let client = rig.client.take().ok_or("no client")?;
    for n in 0..ANSWERS {
        let outcome = json!({"outcome": {"outcome": "cancelled"}, "pad": filler});
        client.send(Request::answer(&json!(n), outcome))?;
    }
    go.send(())?;
    client.close(Duration::from_secs(10))?;
    let written = heard.recv_timeout(Duration::from_secs(10))??;
    std::fs::remove_file(&socket)?;
    assert_eq!(written, ANSWERS, "quitting answers every waiting approval");
    Ok(())
}
