//! Frames off the socket: every recorded one reads, and no line can be too long or too short.

use std::io::Cursor;

use serde_json::{Value, json};
use yi_types::entry::Entry;
use yi_types::event::AgentEvent;
use yi_types::image::MAX_IMAGE_CHARS;
use yi_types::message::AgentMessage;

use super::{Frame, Kept, MAX_FRAME, ReadError, Refusal, Update, read_frame};
use crate::reduce::step::tests::{Check, rows, update};
use crate::reduce::threads::Run;
use crate::wire::request::RequestId;

const RECORDINGS: [&str; 5] = [
    "turn.jsonl",
    "approval-allow.jsonl",
    "approval-reject.jsonl",
    "resume.jsonl",
    "lists.jsonl",
];

/// Why a decoded frame is one this build does not fully read, if it is.
fn unread(frame: &Frame) -> Option<String> {
    match frame {
        Frame::Other(raw) => Some(format!("a frame of no kind: {raw}")),
        Frame::Request { method, .. } => Some(format!("a call to {method}")),
        Frame::Ask { params, .. } => params.known().is_none().then(|| "an ask".to_owned()),
        Frame::Reply { .. } => None,
        Frame::Update { update, .. } => match update {
            Update::Other { kind, .. } => Some(format!("an update of kind {kind}")),
            Update::Event { event, .. } => event.known().is_none().then(|| "an event".to_owned()),
            Update::Replay(replay) => {
                let raw = replay.entries.iter().any(|entry| entry.known().is_none());
                (raw || !replay.unread.is_empty()).then(|| "a replay".to_owned())
            }
            Update::Committed { entries, .. } => entries
                .iter()
                .any(|entry| entry.known().is_none())
                .then(|| "a commit".to_owned()),
            _ => None,
        },
    }
}

#[test]
fn every_frame_a_real_server_sent_reads_as_a_kind_this_app_knows() -> Check {
    let mut read = 0;
    for name in RECORDINGS {
        for (row, (received, frame)) in rows(name)?.iter().enumerate() {
            if !received {
                continue;
            }
            let decoded = Frame::decode(frame.to_string().as_bytes())?;
            assert_eq!(unread(&decoded), None, "{name} row {row}");
            read += 1;
        }
    }
    assert_eq!(
        read, 221,
        "the recordings hold this many frames from the server"
    );
    Ok(())
}

#[test]
fn an_event_keeps_its_number_and_its_typed_body() -> Check {
    // Row 18 of `turn.jsonl`: the answer's first text delta, event 6.
    let (_, frame) = rows("turn.jsonl")?.into_iter().nth(18).ok_or("no row 18")?;
    let Frame::Update { thread, update } = Frame::decode(frame.to_string().as_bytes())? else {
        return Err("not an update".into());
    };
    assert_eq!(thread.as_str(), "01a1de97-bf6c-7514-9fae-b57db1eaa7e1");
    let Update::Event { seq, child, event } = update else {
        return Err("not an event".into());
    };
    assert_eq!((seq, child), (6, None));
    assert!(matches!(
        event.known(),
        Some(AgentEvent::MessageUpdate { .. })
    ));
    Ok(())
}

#[test]
fn a_reply_is_a_result_or_the_servers_refusal() -> Check {
    let recorded = rows("lists.jsonl")?;
    let frame = |row: usize| -> Result<Frame, Box<dyn std::error::Error>> {
        let (_, frame) = recorded.get(row).ok_or("no such row")?;
        Ok(Frame::decode(frame.to_string().as_bytes())?)
    };
    // Row 30 answers `_yi/pin`; row 44 refuses a setting that does not exist.
    assert_eq!(
        frame(30)?,
        Frame::Reply {
            id: RequestId::number(4),
            result: Ok(json!({"pinnedMs": 1_794_735_853_802_u64}))
        }
    );
    assert_eq!(
        frame(44)?,
        Frame::Reply {
            id: RequestId::number(11),
            result: Err(Refusal::Server {
                code: -32602,
                message: "no setting named no.such.key".to_owned()
            })
        }
    );
    Ok(())
}

#[test]
fn a_refusal_keeps_only_so_much_of_what_the_server_said() -> Check {
    let long = "x".repeat(4_000);
    let frame = json!({"jsonrpc": "2.0", "id": 1, "error": {"code": 1, "message": long}});
    let Frame::Reply {
        result: Err(Refusal::Server { message, .. }),
        ..
    } = Frame::decode(frame.to_string().as_bytes())?
    else {
        return Err("not a refusal".into());
    };
    assert_eq!(message.chars().count(), 512);
    Ok(())
}

#[test]
fn a_state_this_app_does_not_know_is_kept_as_written() -> Check {
    let update = |state: Value| {
        json!({"jsonrpc": "2.0", "method": "session/update",
            "params": {"sessionId": "s", "update": state}})
    };
    let paused = update(json!({"sessionUpdate": "state_update", "state": "paused"}));
    let Frame::Update {
        update: decoded, ..
    } = Frame::decode(paused.to_string().as_bytes())?
    else {
        return Err("not an update".into());
    };
    assert_eq!(decoded, Update::State(Run::Other("paused".to_owned())));
    Ok(())
}

#[test]
fn an_entry_this_app_cannot_read_does_not_cost_the_others() -> Check {
    // Row 59 of `resume.jsonl` replays twelve entries; one gets a role no build knows yet.
    let (_, mut frame) = rows("resume.jsonl")?.into_iter().nth(59).ok_or("no row")?;
    frame["params"]["update"]["entries"][3]["message"]["role"] = Value::from("narrator");
    let Frame::Update {
        update: Update::Replay(replay),
        ..
    } = Frame::decode(frame.to_string().as_bytes())?
    else {
        return Err("not a replay".into());
    };
    let raw: Vec<usize> = (0..replay.entries.len())
        .filter(|at| matches!(replay.entries.get(*at), Some(Kept::Raw(_))))
        .collect();
    assert_eq!((replay.entries.len(), raw), (12, vec![3]));
    Ok(())
}

fn frames(bytes: &[u8], max: u64) -> Vec<Result<Option<String>, String>> {
    let mut reader = Cursor::new(bytes);
    let mut out = Vec::new();
    loop {
        match read_frame(&mut reader, max) {
            Ok(Some(frame)) => out.push(Ok(Some(String::from_utf8_lossy(&frame).into_owned()))),
            Ok(None) => {
                out.push(Ok(None));
                return out;
            }
            Err(ReadError::TooLong { resynced: true, .. }) => {
                out.push(Err("too long, skipped".to_owned()));
            }
            Err(error) => {
                out.push(Err(error.to_string()));
                return out;
            }
        }
    }
}

#[test]
fn frames_are_lines_and_a_clean_end_is_no_frame() -> Check {
    assert_eq!(
        frames(b"{\"a\":1}\n{\"b\":2}\n", 64),
        [
            Ok(Some("{\"a\":1}".to_owned())),
            Ok(Some("{\"b\":2}".to_owned())),
            Ok(None)
        ]
    );
    Ok(())
}

#[test]
fn a_line_at_the_cap_reads_and_one_byte_more_is_skipped() -> Check {
    // Eight bytes with the newline: exactly the cap, then one over it.
    assert_eq!(
        frames(b"1234567\nab\n", 8),
        [
            Ok(Some("1234567".to_owned())),
            Ok(Some("ab".to_owned())),
            Ok(None)
        ]
    );
    assert_eq!(
        frames(b"12345678\nab\n", 8),
        [
            Err("too long, skipped".to_owned()),
            Ok(Some("ab".to_owned())),
            Ok(None)
        ],
        "the tail of a long line must not be read as a frame"
    );
    Ok(())
}

#[test]
fn a_prompt_of_two_images_as_large_as_yi_takes_is_a_frame_this_app_reads() -> Check {
    // Yi stores a prompt as one entry and never splits an entry between two frames.
    let image = || ("image/png".to_owned(), "A".repeat(MAX_IMAGE_CHARS));
    let entry = Entry::Message {
        id: "e1".to_owned(),
        message: AgentMessage::user_input_with_images("compare these", [image(), image()]),
        terminate: None,
        parent_id: None,
        seq: 1,
        timestamp: 1,
    };
    let entry = serde_json::to_value(&entry)?;
    let committed = json!({"sessionUpdate": "_yi/committed", "ancestors": [], "entry": entry});
    let line = format!("{}\n", update("t1", committed));
    let frame = read_frame(&mut Cursor::new(line.as_bytes()), MAX_FRAME)?.ok_or("no frame")?;
    assert_eq!(frame.len(), line.len().saturating_sub(1));
    let Frame::Update {
        update: Update::Committed { entries, .. },
        ..
    } = Frame::decode(&frame)?
    else {
        return Err("the frame is not a commit".into());
    };
    assert!(matches!(entries.as_slice(), [Kept::Known(_)]));
    Ok(())
}

#[test]
fn a_line_that_never_ends_closes_the_stream() -> Check {
    let endless = vec![b'x'; 100];
    assert_eq!(
        frames(&endless, 8),
        [Err("a frame over 8 bytes (resynced: false)".to_owned())],
        "a peer that sends no newline gets a bounded read and no second chance"
    );
    Ok(())
}

#[test]
fn the_search_for_a_long_lines_end_is_bounded_too() -> Check {
    // Past the cap of 8 the reader looks 16 more bytes for the newline, and no further.
    let within = [
        &[b'x'; 23][..],
        b"
ab
",
    ]
    .concat();
    assert_eq!(
        frames(&within, 8),
        [
            Err("too long, skipped".to_owned()),
            Ok(Some("ab".to_owned())),
            Ok(None)
        ]
    );
    let beyond = [
        &[b'x'; 24][..],
        b"
ab
",
    ]
    .concat();
    assert_eq!(
        frames(&beyond, 8),
        [Err("a frame over 8 bytes (resynced: false)".to_owned())],
        "a newline one byte past the bound must not be waited for"
    );
    Ok(())
}

#[test]
fn a_frame_cut_off_by_the_end_of_the_stream_is_an_error_not_a_frame() -> Check {
    assert_eq!(
        frames(b"{\"a\":1}\n{\"b\":", 64),
        [
            Ok(Some("{\"a\":1}".to_owned())),
            Err("the socket closed in the middle of a frame, 5 bytes in".to_owned())
        ]
    );
    Ok(())
}
