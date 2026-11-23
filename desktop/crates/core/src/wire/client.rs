//! The socket: a reader thread with a bounded frame size and a writer thread, both on `UnixStream`.

#[cfg(test)]
mod tests;

use std::fmt;
use std::io::{self, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::thread::Builder;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::ids::Millis;
use crate::inbox::{Closed, Sender};
use crate::msg::Msg;
use crate::wire::event::{Down, WireEvent};
use crate::wire::frame::{Frame, MAX_FRAME, ReadError, read_frame};
use crate::wire::request::Request;

/// Requests waiting for the writer; one more is refused, since a full queue is a stuck socket.
pub const OUT_QUEUE: usize = 256;
/// How long the server has to answer `initialize`.
const HANDSHAKE: Duration = Duration::from_secs(10);
/// How long one write may block before the connection counts as stuck and is closed.
const WRITE: Duration = Duration::from_secs(10);
/// The wait before the first try to connect again, and the longest wait between tries.
const BACKOFF: (Duration, Duration) = (Duration::from_millis(100), Duration::from_secs(2));
/// How often a server that does not come up is started again.
const START_EVERY: Duration = Duration::from_secs(10);
/// The version of the protocol this client speaks, and the only one it takes.
const PROTOCOL: u64 = 2;
/// The id of the client's own `initialize`; the model's requests are numbers.
const HELLO: &str = "hello";

/// The app's clock, read on the client's threads to stamp what arrives.
pub type Clock = Arc<dyn Fn() -> Millis + Send + Sync>;
/// Starts `yi serve` on a socket. The app supplies it from where a process may be started.
pub type Starter = Box<dyn FnMut(&Path) -> io::Result<()> + Send>;

pub struct Config {
    /// The server's socket, `~/.yi/daemon.sock` unless the app says otherwise.
    pub socket: PathBuf,
    /// How often the thread list is asked for; none never asks.
    pub poll: Option<Duration>,
    /// Called when nothing listens on the socket, never while the stop marker is there.
    pub start: Option<Starter>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("socket", &self.socket)
            .field("poll", &self.poll)
            .field("start", &self.start.is_some())
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("the wire thread could not start")]
    Spawn(#[source] io::Error),
}

/// Why a request was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Unsent {
    #[error("the wire's queue of {max} requests is full")]
    Full { max: usize },
    #[error("the wire client has stopped")]
    Stopped,
}

/// The client did not stop within the time given; its threads end on their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the wire client did not stop within {0:?}")]
pub struct StillRunning(pub Duration);

enum Out {
    Send(Request),
    /// The reader of connection `link` has ended.
    Lost {
        link: u64,
        why: Down,
    },
    Stop,
}

/// The app's end of the connection to `yi serve`.
#[derive(Debug)]
pub struct Client {
    out: SyncSender<Out>,
    done: Receiver<()>,
    /// Set when the app lets go of the client; the thread reads it between requests.
    stop: Arc<AtomicBool>,
}

impl Client {
    /// Starts the client. It connects, starts the server if it may, says `initialize`,
    /// and from then on keeps the connection, telling the model of each change through `inbox`.
    pub fn spawn(config: Config, inbox: Sender, clock: Clock) -> Result<Self, ClientError> {
        let (out, queue) = sync_channel(OUT_QUEUE);
        let (finished, done) = sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let supervisor = Supervisor {
            config,
            inbox,
            clock,
            queue,
            lost: out.clone(),
            stop: Arc::clone(&stop),
            link: 0,
            started: None,
            said: None,
        };
        let run = move || {
            supervisor.run();
            // Dropping `finished` is the signal: `close` wakes when this thread is done.
            drop(finished);
        };
        let spawned = Builder::new().name("yi-wire".to_owned()).spawn(run);
        spawned.map_err(ClientError::Spawn)?;
        Ok(Self { out, done, stop })
    }

    /// Queues a request for the writer. It never waits, so the UI thread may call it.
    pub fn send(&self, request: Request) -> Result<(), Unsent> {
        self.out
            .try_send(Out::Send(request))
            .map_err(|error| match error {
                TrySendError::Full(_) => Unsent::Full { max: OUT_QUEUE },
                TrySendError::Disconnected(_) => Unsent::Stopped,
            })
    }

    /// Asks the thread to stop once it has written what was queued before, and waits up to
    /// `grace` for it; past that, `Drop` raises the flag.
    pub fn close(self, grace: Duration) -> Result<(), StillRunning> {
        // Invariant: the flag cuts the queue short, so only a stop a full queue refuses raises it.
        if self.out.try_send(Out::Stop).is_err() {
            self.stop.store(true, Ordering::Relaxed);
        }
        match self.done.recv_timeout(grace) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => Ok(()),
            Err(RecvTimeoutError::Timeout) => Err(StillRunning(grace)),
        }
    }

    /// Raises the stop flag and wakes the thread if it is idle. With a full queue it is
    /// not idle, and reads the flag when it comes back for the next request.
    fn halt(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.out.try_send(Out::Stop).ok();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Whether the client's thread goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Go,
    Stop,
}

struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    server: String,
    /// How long the server took to answer `initialize`.
    latency: Duration,
}

/// The client's own thread: it connects, writes, and watches over each connection's reader.
struct Supervisor {
    config: Config,
    inbox: Sender,
    clock: Clock,
    queue: Receiver<Out>,
    /// Handed to each reader, to say its connection is gone.
    lost: SyncSender<Out>,
    stop: Arc<AtomicBool>,
    /// Counts connections, so a late word from an old reader is told apart.
    link: u64,
    started: Option<Instant>,
    /// The last reason reported for being down, so a retry does not repeat it.
    said: Option<Down>,
}

/// Where the server leaves word that it was stopped on purpose.
fn stop_marker(socket: &Path) -> PathBuf {
    socket.with_extension("stopped")
}

fn write_line(writer: &mut UnixStream, mut line: String) -> io::Result<()> {
    line.push('\n');
    writer.write_all(line.as_bytes())
}

impl Supervisor {
    fn run(mut self) {
        let mut wait = BACKOFF.0;
        loop {
            let flow = match self.connect() {
                Ok(conn) => {
                    wait = BACKOFF.0;
                    self.serve(conn)
                }
                Err(why) => self.tell_down(why),
            };
            if flow == Flow::Stop || self.pause(wait) == Flow::Stop {
                return;
            }
            wait = wait.saturating_mul(2).min(BACKOFF.1);
        }
    }

    fn tell(&self, event: WireEvent) -> Result<(), Closed> {
        self.inbox.send(Msg::Wire(event))
    }

    fn now(&self) -> Millis {
        (self.clock)()
    }

    /// Reports why there is no connection, once per reason.
    fn tell_down(&mut self, why: Down) -> Flow {
        if self.said.as_ref() == Some(&why) {
            return Flow::Go;
        }
        self.said = Some(why.clone());
        let at = self.now();
        match self.tell(WireEvent::Down { at, why }) {
            Ok(()) => Flow::Go,
            Err(Closed) => Flow::Stop,
        }
    }

    fn connect(&mut self) -> Result<Conn, Down> {
        match UnixStream::connect(&self.config.socket) {
            Ok(stream) => handshake(stream),
            Err(_) => Err(self.start()),
        }
    }

    /// Asks the app to start the server, unless it was stopped on purpose or just asked.
    fn start(&mut self) -> Down {
        if stop_marker(&self.config.socket).exists() {
            return Down::Stopped;
        }
        let due = self.started.is_none_or(|at| at.elapsed() >= START_EVERY);
        let Some(start) = self.config.start.as_mut().filter(|_| due) else {
            return Down::NoServer;
        };
        self.started = Some(Instant::now());
        match start(&self.config.socket) {
            Ok(()) => Down::NoServer,
            Err(error) => Down::StartFailed(error.to_string()),
        }
    }

    /// Waits before the next try, turning away what is asked meanwhile: it cannot be sent.
    fn pause(&mut self, wait: Duration) -> Flow {
        let Some(deadline) = Instant::now().checked_add(wait) else {
            return Flow::Go;
        };
        while !self.stop.load(Ordering::Relaxed) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.queue.recv_timeout(left) {
                // A request while down is someone waiting: try the server now, not at the deadline.
                Ok(Out::Send(request)) => {
                    let Some(id) = request.id else {
                        return Flow::Go;
                    };
                    let at = self.now();
                    if self.tell(WireEvent::Unsent { at, id }).is_err() {
                        return Flow::Stop;
                    }
                    return Flow::Go;
                }
                Ok(Out::Lost { .. }) => {}
                Ok(Out::Stop) | Err(RecvTimeoutError::Disconnected) => return Flow::Stop,
                Err(RecvTimeoutError::Timeout) => return Flow::Go,
            }
        }
        Flow::Stop
    }

    /// Keeps one connection: writes what is asked and waits for word that the reader ended.
    fn serve(&mut self, conn: Conn) -> Flow {
        let Conn {
            reader,
            mut writer,
            server,
            latency,
        } = conn;
        self.link = self.link.saturating_add(1);
        (self.said, self.started) = (None, None);
        let at = self.now();
        let latency = Some(Millis(
            u64::try_from(latency.as_millis()).unwrap_or(u64::MAX),
        ));
        let up = WireEvent::Up {
            at,
            server,
            latency,
        };
        if self.tell(up).is_err() {
            return Flow::Stop;
        }
        let reading = {
            let (inbox, clock) = (self.inbox.clone(), Arc::clone(&self.clock));
            let (lost, link) = (self.lost.clone(), self.link);
            Builder::new()
                .name("yi-wire-read".to_owned())
                .spawn(move || read(reader, &inbox, &clock, &lost, link))
        };
        let end = match &reading {
            Ok(_) => self.write(&mut writer),
            Err(error) => Err(Down::Closed(error.to_string())),
        };
        // Closing the socket ends the reader's blocked read; then it is safe to wait for it.
        writer.shutdown(Shutdown::Both).ok();
        if let Ok(reading) = reading {
            reading.join().ok();
        }
        match end {
            Ok(()) => Flow::Stop,
            Err(why) => self.tell_down(why),
        }
    }

    /// Writes requests until the app stops the client (`Ok`) or the connection is lost.
    fn write(&mut self, writer: &mut UnixStream) -> Result<(), Down> {
        let mut poll = self.config.poll.map(|every| (Instant::now(), every));
        while !self.stop.load(Ordering::Relaxed) {
            let next = match poll {
                Some((due, _)) => {
                    let left = due.saturating_duration_since(Instant::now());
                    self.queue.recv_timeout(left)
                }
                None => self
                    .queue
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
            };
            match next {
                Ok(Out::Send(Request { id, line })) => {
                    if let Err(error) = write_line(writer, line) {
                        // A frame cut off mid-write is no frame: told unsent before the loss.
                        if let Some(id) = id {
                            let at = self.now();
                            self.tell(WireEvent::Unsent { at, id }).ok();
                        }
                        return Err(Down::Closed(error.to_string()));
                    }
                }
                Ok(Out::Lost { link, why }) if link == self.link => return Err(why),
                Ok(Out::Lost { .. }) => {}
                Ok(Out::Stop) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => {
                    let at = self.now();
                    if self.tell(WireEvent::Poll { at }).is_err() {
                        return Ok(());
                    }
                    // The next poll is a whole interval from now: a missed one is not made up.
                    poll = poll
                        .and_then(|(_, every)| Some((Instant::now().checked_add(every)?, every)));
                }
            }
        }
        Ok(())
    }
}

/// Says `initialize` and waits, within a deadline, for the server to answer as protocol 2.
fn handshake(stream: UnixStream) -> Result<Conn, Down> {
    let failed = |error: io::Error| Down::Handshake(error.to_string());
    let deadline = Instant::now().checked_add(HANDSHAKE);
    stream.set_write_timeout(Some(WRITE)).map_err(failed)?;
    let mut writer = stream.try_clone().map_err(failed)?;
    let info = json!({"name": "yi-desktop", "version": env!("CARGO_PKG_VERSION")});
    let params = json!({"protocolVersion": PROTOCOL, "clientInfo": info});
    let hello = json!({"jsonrpc": "2.0", "id": HELLO, "method": "initialize", "params": params});
    let asked = Instant::now();
    write_line(&mut writer, hello.to_string()).map_err(failed)?;
    let mut reader = BufReader::with_capacity(64 << 10, stream);
    let answer = loop {
        let left = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
        let Some(left) = left.filter(|left| !left.is_zero()) else {
            return Err(Down::Handshake("no answer in time".to_owned()));
        };
        reader
            .get_ref()
            .set_read_timeout(Some(left))
            .map_err(failed)?;
        let frame = match read_frame(&mut reader, MAX_FRAME) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Err(Down::Handshake("the server hung up".to_owned())),
            Err(ReadError::TooLong { resynced: true, .. }) => continue,
            Err(error) => return Err(Down::Handshake(reason(&error))),
        };
        // Nothing but the answer is expected this early; anything else is passed over.
        match serde_json::from_slice::<Value>(&frame) {
            Ok(answer) if answer.get("id").and_then(Value::as_str) == Some(HELLO) => break answer,
            Ok(_) | Err(_) => {}
        }
    };
    let latency = asked.elapsed();
    reader.get_ref().set_read_timeout(None).map_err(failed)?;
    let version = answer
        .pointer("/result/protocolVersion")
        .and_then(Value::as_u64);
    if version != Some(PROTOCOL) {
        let refusal = answer.pointer("/error/message").and_then(Value::as_str);
        let spoken =
            version.map_or_else(|| "no protocol".to_owned(), |it| format!("protocol {it}"));
        let why = refusal.map_or_else(
            || format!("the server speaks {spoken}, this app protocol {PROTOCOL}"),
            |refusal| refusal.chars().take(200).collect(),
        );
        return Err(Down::Handshake(why));
    }
    let server = answer
        .pointer("/result/info/version")
        .and_then(Value::as_str);
    Ok(Conn {
        reader,
        writer,
        server: server.unwrap_or_default().chars().take(64).collect(),
        latency,
    })
}

/// A read error's words, with its cause: `io::Error` carries the system's own text.
fn reason(error: &ReadError) -> String {
    match error {
        ReadError::Io(cause) => format!("{error}: {cause}"),
        ReadError::TooLong { .. } | ReadError::Truncated { .. } => error.to_string(),
    }
}

/// The reader thread: every frame the server sends, decoded, stamped and handed to the UI.
/// The inbox is bounded, so a UI that falls behind slows this thread, and through it the server.
fn read(
    mut reader: BufReader<UnixStream>,
    inbox: &Sender,
    clock: &Clock,
    lost: &SyncSender<Out>,
    link: u64,
) {
    let why = loop {
        let event = match read_frame(&mut reader, MAX_FRAME) {
            Ok(Some(line)) => match Frame::decode(&line) {
                Ok(frame) => WireEvent::Frame {
                    at: clock(),
                    frame: Box::new(frame),
                },
                Err(bad) => WireEvent::Bad {
                    at: clock(),
                    why: bad.to_string(),
                },
            },
            Ok(None) => break "the server hung up".to_owned(),
            Err(error @ ReadError::TooLong { resynced: true, .. }) => WireEvent::Bad {
                at: clock(),
                why: error.to_string(),
            },
            Err(error) => break reason(&error),
        };
        if inbox.send(Msg::Wire(event)).is_err() {
            return;
        }
    };
    let why = Down::Closed(why);
    // Invariant: never wait here, for the writer may be waiting for this thread to end. If
    // its queue is full it is writing, and on a socket shut down that fails and tells it.
    reader.get_ref().shutdown(Shutdown::Both).ok();
    lost.try_send(Out::Lost { link, why }).ok();
}
