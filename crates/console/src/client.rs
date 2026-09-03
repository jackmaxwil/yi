//! Socket IO threads for the daemon connection, deliberately dumb: every protocol decision
//! lives in the synchronous app loop, where the drive harness exercises it deterministically.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A single wire frame is capped; past this the connection is torn down and
/// re-dialed rather than allocating without bound.
const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;
const READ_CHUNK: usize = 64 * 1024;
const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_CAP: Duration = Duration::from_secs(2);

/// Reader-side events into the app loop.
pub enum ClientEvent {
    Connected,
    Frame(Value),
    /// A non-JSON line, or one past the frame cap (which drops the link).
    BadFrame,
    Disconnected {
        reason: String,
    },
}

/// Outbound side handed to the app loop.
pub struct Outbound {
    lines: SyncSender<String>,
    shutdown: Arc<AtomicBool>,
}

impl Outbound {
    /// Queue one frame line. False means the writer is gone or the queue is
    /// full — the caller surfaces that in the status line.
    #[must_use]
    pub fn send(&self, frame: &Value) -> bool {
        match self.lines.try_send(frame.to_string()) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }

    /// Two-phase shutdown, phase one: stop reconnecting and unblock both IO
    /// threads.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

pub struct ClientThreads {
    pub reader: std::thread::JoinHandle<()>,
    pub writer: std::thread::JoinHandle<()>,
}

/// Spawn the IO threads; the bounded event receiver backpressures the
/// socket when the UI lags, never growing a queue.
pub fn spawn(socket: PathBuf) -> (Receiver<ClientEvent>, Outbound, ClientThreads) {
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel::<ClientEvent>(4096);
    let (line_tx, line_rx) = std::sync::mpsc::sync_channel::<String>(64);
    let (stream_tx, stream_rx) = std::sync::mpsc::channel::<UnixStream>();
    let shutdown = Arc::new(AtomicBool::new(false));

    let outbound = Outbound {
        lines: line_tx,
        shutdown: Arc::clone(&shutdown),
    };

    let reader_shutdown = Arc::clone(&shutdown);
    let reader = std::thread::spawn(move || {
        reader_loop(&socket, &event_tx, &stream_tx, &reader_shutdown);
    });

    let writer_shutdown = Arc::clone(&shutdown);
    let writer = std::thread::spawn(move || {
        writer_loop(&line_rx, &stream_rx, &writer_shutdown);
    });

    (event_rx, outbound, ClientThreads { reader, writer })
}

fn reader_loop(
    socket: &PathBuf,
    events: &SyncSender<ClientEvent>,
    streams: &std::sync::mpsc::Sender<UnixStream>,
    shutdown: &AtomicBool,
) {
    let mut backoff = BACKOFF_START;
    while !shutdown.load(Ordering::SeqCst) {
        let stream = match UnixStream::connect(socket) {
            Ok(stream) => stream,
            Err(error) => {
                if events
                    .send(ClientEvent::Disconnected {
                        reason: format!("connect: {error}"),
                    })
                    .is_err()
                {
                    return;
                }
                sleep_interruptible(backoff, shutdown);
                backoff = (backoff * 2).min(BACKOFF_CAP);
                continue;
            }
        };
        let write_half = match stream.try_clone() {
            Ok(clone) => clone,
            Err(error) => {
                let _ = events.send(ClientEvent::Disconnected {
                    reason: format!("clone: {error}"),
                });
                sleep_interruptible(backoff, shutdown);
                backoff = (backoff * 2).min(BACKOFF_CAP);
                continue;
            }
        };
        if streams.send(write_half).is_err() || events.send(ClientEvent::Connected).is_err() {
            return;
        }
        let connected_at = Instant::now();
        let reason = read_frames(&stream, events, shutdown);
        // Unblock a writer that still holds the dead stream.
        let _ = stream.shutdown(std::net::Shutdown::Both);
        if events.send(ClientEvent::Disconnected { reason }).is_err() {
            return;
        }
        // Incident: only the failed-connect path slept, so a daemon that accepted and closed
        // at once re-dialed in a spin. Only a link that outlived the cap resets the backoff.
        if connected_at.elapsed() > BACKOFF_CAP {
            backoff = BACKOFF_START;
        }
        sleep_interruptible(backoff, shutdown);
        backoff = (backoff * 2).min(BACKOFF_CAP);
    }
}

/// Bounded newline framing by hand: fill from the socket, split complete
/// lines, refuse a line past the cap. Returns the disconnect reason.
fn read_frames(
    mut stream: &UnixStream,
    events: &SyncSender<ClientEvent>,
    shutdown: &AtomicBool,
) -> String {
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; READ_CHUNK];
    loop {
        if shutdown.load(Ordering::SeqCst) {
            return "shutdown".to_owned();
        }
        let read = match stream.read(&mut chunk) {
            Ok(0) => return "daemon closed the socket".to_owned(),
            Ok(n) => n,
            Err(error) => return format!("read: {error}"),
        };
        let Some(bytes) = chunk.get(..read) else {
            return "read overrun".to_owned();
        };
        pending.extend_from_slice(bytes);
        loop {
            let Some(newline) = pending.iter().position(|byte| *byte == b'\n') else {
                break;
            };
            let line: Vec<u8> = pending.drain(..=newline).collect();
            let text = String::from_utf8_lossy(&line);
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            let event = match serde_json::from_str::<Value>(trimmed) {
                Ok(value) => ClientEvent::Frame(value),
                Err(_) => ClientEvent::BadFrame,
            };
            if events.send(event).is_err() {
                return "app gone".to_owned();
            }
        }
        if pending.len() > MAX_FRAME_BYTES {
            let _ = events.send(ClientEvent::BadFrame);
            return "frame exceeded 32 MiB cap".to_owned();
        }
    }
}

fn writer_loop(lines: &Receiver<String>, streams: &Receiver<UnixStream>, shutdown: &AtomicBool) {
    let mut current: Option<UnixStream> = None;
    loop {
        // A fresher stream always wins; drain without blocking.
        while let Ok(stream) = streams.try_recv() {
            current = Some(stream);
        }
        let line = match lines.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if shutdown.load(Ordering::SeqCst) {
                    if let Some(stream) = &current {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }
                    return;
                }
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(stream) = &current {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
                return;
            }
        };
        // A stream that arrived while blocked above must count for THIS
        // line — the handshake's initialize races the connect otherwise.
        while let Ok(stream) = streams.try_recv() {
            current = Some(stream);
        }
        // The app refuses sends while the link is down, so anything that
        // races a missing stream here is safe to drop.
        let Some(stream) = &mut current else {
            continue;
        };
        let mut payload = line.into_bytes();
        payload.push(b'\n');
        if stream.write_all(&payload).is_err() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            current = None;
        }
    }
}

fn sleep_interruptible(total: Duration, shutdown: &AtomicBool) {
    let mut remaining = total;
    let step = Duration::from_millis(50);
    while remaining > Duration::ZERO {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        let slice = remaining.min(step);
        std::thread::sleep(slice);
        remaining = remaining.saturating_sub(slice);
    }
}
