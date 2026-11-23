//! The client and the reducer together, against a script that plays a recorded server's side
//! on a real socket and checks what the client sends against the recording.

#[cfg(test)]
mod replays;
#[cfg(test)]
mod socket;

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender as Signal, TryRecvError, channel};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{Client, Clock, Config, Starter};
use crate::command::table::{Route, Run, Target};
use crate::ids::{CommandId, Millis};
use crate::inbox::{Inbox, inbox};
use crate::model::Model;
use crate::msg::{Effect, Msg, step};
use crate::wire::event::WireEvent;

pub(crate) type Check = Result<(), Box<dyn Error>>;

/// How long a test waits on the client, and the script on the test, before giving up.
const PATIENCE: Duration = Duration::from_secs(10);

/// How the script departs from playing every row in order.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Plan {
    /// Calls the app has no command for: the script neither waits for nor answers them.
    pub(crate) skip: &'static [&'static str],
    /// Rows after which the server hangs up.
    pub(crate) cut: &'static [usize],
    /// Rows before which the server waits for the test to say go.
    pub(crate) gate: &'static [usize],
    /// A row the server sends only half of before it hangs up.
    pub(crate) torn: Option<usize>,
    /// A row, and how many bytes of a line that is no JSON the server writes ahead of it.
    pub(crate) noise: Option<(usize, usize)>,
}

type Conn = (BufReader<UnixStream>, UnixStream);

fn connected(conn: &mut Option<Conn>, row: usize) -> Result<&mut Conn, String> {
    conn.as_mut()
        .ok_or_else(|| format!("row {row}: the script has no connection"))
}

/// Holds what the client sent against the recording's row. Ids are the sender's to choose.
fn check(row: usize, recorded: &Value, got: &Value) -> Result<(), String> {
    let same = match recorded["method"].as_str() {
        Some("initialize") => {
            got["method"] == "initialize" && got["params"]["protocolVersion"] == 2
        }
        Some(method) => got["method"] == method && got["params"] == recorded["params"],
        None => got["id"] == recorded["id"] && got["result"] == recorded["result"],
    };
    if same {
        return Ok(());
    }
    let wanted = serde_json::json!({"method": recorded["method"], "params": recorded["params"],
        "result": recorded["result"]});
    Err(format!(
        "row {row}: the recorded client sent {wanted}, this one sent {got}"
    ))
}

/// Plays the server's side of `rows`: reads what the recording's client sent, writes what
/// the recording's server sent, and accepts a new connection at every `initialize`.
fn serve(
    listener: &UnixListener,
    rows: &[(bool, Value)],
    plan: Plan,
    go: &Receiver<()>,
) -> Result<(), String> {
    let mut conn: Option<Conn> = None;
    let mut ids: HashMap<String, Value> = HashMap::new();
    let mut skipped: HashSet<String> = HashSet::new();
    for (row, (received, frame)) in rows.iter().enumerate() {
        if plan.gate.contains(&row) {
            go.recv_timeout(PATIENCE)
                .map_err(|_| format!("row {row}: the test never said go"))?;
        }
        let method = frame["method"].as_str();
        if *received {
            let mut frame = frame.clone();
            if method.is_none() {
                let key = frame["id"].to_string();
                if skipped.contains(&key) {
                    continue;
                }
                frame["id"] = ids
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| format!("row {row}: an answer to a call nobody made"))?;
            }
            let line = format!("{frame}\n");
            let torn = plan.torn == Some(row);
            let bytes = line.as_bytes();
            let part = if torn {
                bytes.get(..bytes.len() / 2).unwrap_or_default()
            } else {
                bytes
            };
            let (_, writer) = connected(&mut conn, row)?;
            if let Some((_, len)) = plan.noise.filter(|(before, _)| *before == row) {
                let mut noise = vec![b'x'; len];
                noise.push(b'\n');
                writer
                    .write_all(&noise)
                    .map_err(|error| format!("row {row}: {error}"))?;
            }
            writer
                .write_all(part)
                .map_err(|error| format!("row {row}: {error}"))?;
            if torn {
                conn = None;
            }
        } else {
            if method == Some("initialize") {
                let (stream, _) = listener.accept().map_err(|error| error.to_string())?;
                stream
                    .set_read_timeout(Some(PATIENCE))
                    .map_err(|error| error.to_string())?;
                let reader = stream.try_clone().map_err(|error| error.to_string())?;
                conn = Some((BufReader::new(reader), stream));
            }
            if method.is_some_and(|method| plan.skip.contains(&method)) {
                skipped.insert(frame["id"].to_string());
                continue;
            }
            let (reader, _) = connected(&mut conn, row)?;
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|error| format!("row {row}: waiting for the client: {error}"))?;
            let got: Value = serde_json::from_str(&line)
                .map_err(|error| format!("row {row}: the client sent {line:?}: {error}"))?;
            check(row, frame, &got)?;
            if method.is_some() {
                ids.insert(frame["id"].to_string(), got["id"].clone());
            }
        }
        if plan.cut.contains(&row) {
            conn = None;
        }
    }
    // The connection stays up until the test has looked at the model.
    match go.recv_timeout(PATIENCE) {
        Ok(()) | Err(RecvTimeoutError::Disconnected) => Ok(()),
        Err(RecvTimeoutError::Timeout) => Err("the test never finished".to_owned()),
    }
}

/// A socket path under this workspace's `target/`, short enough for a Unix socket.
pub(crate) fn socket_path() -> Result<PathBuf, Box<dyn Error>> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let exe = std::env::current_exe()?;
    let target = exe
        .ancestors()
        .nth(3)
        .ok_or("the test is not under target/")?;
    let dir = target.join("wc");
    std::fs::create_dir_all(&dir)?;
    let name = format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let socket = dir.join(name);
    // A failed run of an earlier process with this id may have left these two behind.
    for stale in [socket.clone(), socket.with_extension("stopped")] {
        if stale.exists() {
            std::fs::remove_file(&stale)?;
        }
    }
    Ok(socket)
}

/// A clock that ticks once each time it is read, so every stamp differs.
pub(crate) fn ticking() -> Clock {
    let now = AtomicU64::new(1_000);
    Arc::new(move || Millis(now.fetch_add(1, Ordering::Relaxed)))
}

/// The model, its inbox and a client on a socket, with the script behind it.
pub(crate) struct Rig {
    pub(crate) model: Model,
    inbox: Inbox,
    woken: Receiver<()>,
    client: Option<Client>,
    go: Signal<()>,
    verdict: Receiver<Result<(), String>>,
    socket: PathBuf,
}

impl Rig {
    /// A client alone: nothing listens unless `start` makes it.
    pub(crate) fn bare(
        start: Option<Starter>,
        poll: Option<Duration>,
    ) -> Result<Self, Box<dyn Error>> {
        let socket = socket_path()?;
        Self::on(socket, start, poll)
    }

    fn on(
        socket: PathBuf,
        start: Option<Starter>,
        poll: Option<Duration>,
    ) -> Result<Self, Box<dyn Error>> {
        let (wake, woken) = channel();
        // A send fails only once the rig is gone, when nobody is left to wake.
        let (sender, inbox) = inbox(move || {
            let _gone = wake.send(());
        });
        let config = Config {
            socket: socket.clone(),
            poll,
            start,
        };
        let client = Client::spawn(config, sender, ticking())?;
        let (go, _unused) = channel();
        let (_never, verdict) = channel();
        Ok(Self {
            model: Model::default(),
            inbox,
            woken,
            client: Some(client),
            go,
            verdict,
            socket,
        })
    }

    /// A client facing a script that plays `rows`.
    pub(crate) fn start(rows: Vec<(bool, Value)>, plan: Plan) -> Result<Self, Box<dyn Error>> {
        Self::polling(rows, plan, None)
    }

    pub(crate) fn polling(
        rows: Vec<(bool, Value)>,
        plan: Plan,
        poll: Option<Duration>,
    ) -> Result<Self, Box<dyn Error>> {
        let socket = socket_path()?;
        let listener = UnixListener::bind(&socket)?;
        let (go, gate) = channel();
        let (report, verdict) = channel();
        std::thread::Builder::new()
            .name("recorded-server".to_owned())
            .spawn(move || drop(report.send(serve(&listener, &rows, plan, &gate))))?;
        let mut rig = Self::on(socket, None, poll)?;
        rig.go = go;
        rig.verdict = verdict;
        Ok(rig)
    }

    fn deliver(&self, effects: Vec<Effect>) -> Check {
        let client = self.client.as_ref().ok_or("the client is closed")?;
        for effect in effects {
            if let Effect::Wire(request) = effect {
                client.send(request)?;
            }
        }
        Ok(())
    }

    /// Steps what arrived into the model and sends what it asks for, until `done` holds.
    pub(crate) fn until(&mut self, what: &str, done: impl Fn(&Model) -> bool) -> Check {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let drained = self.inbox.drain(&mut self.model);
            self.deliver(drained.effects)?;
            if done(&self.model) {
                return Ok(());
            }
            match self.verdict.try_recv() {
                Ok(Err(wrong)) => return Err(wrong.into()),
                Ok(Ok(())) | Err(TryRecvError::Empty | TryRecvError::Disconnected) => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("timed out waiting until {what}").into());
            }
            // A timeout here falls through to the deadline check above.
            let _timed_out = self.woken.recv_timeout(left.min(Duration::from_millis(50)));
        }
    }

    /// Runs a wire command as a view would and sends what it asks for.
    pub(crate) fn run(&mut self, command: CommandId, target: Target, text: Option<&str>) -> Check {
        let run = Run {
            text: text.map(str::to_owned),
            ..Run::new(command).on(target)
        };
        let effects = step(&mut self.model, Msg::Command(Route::Wire, run));
        self.deliver(effects)
    }

    /// What the client's timer does: one poll.
    pub(crate) fn poll(&mut self) -> Check {
        let effects = step(
            &mut self.model,
            Msg::Wire(WireEvent::Poll { at: Millis(0) }),
        );
        self.deliver(effects)
    }

    /// Lets the script past a gate.
    pub(crate) fn go(&self) -> Check {
        Ok(self.go.send(())?)
    }

    pub(crate) fn client(&self) -> Result<&Client, Box<dyn Error>> {
        Ok(self.client.as_ref().ok_or("the client is closed")?)
    }

    /// Ends the script, takes its verdict on what the client sent, and stops the client.
    pub(crate) fn finish(mut self) -> Check {
        let (hung_up, _) = channel();
        drop(std::mem::replace(&mut self.go, hung_up));
        let verdict = match self.verdict.recv_timeout(PATIENCE) {
            Ok(verdict) => verdict,
            Err(RecvTimeoutError::Disconnected) => Ok(()),
            Err(RecvTimeoutError::Timeout) => Err("the script never ended".to_owned()),
        };
        let client = self.client.take().ok_or("the client is closed")?;
        client.close(Duration::from_secs(5))?;
        if self.socket.exists() {
            std::fs::remove_file(&self.socket)?;
        }
        Ok(verdict?)
    }
}
