//! A debug adapter for LLDB or GDB, spoken to over stdio.

use std::io::{self, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, channel, sync_channel};
use std::thread;
use std::time::Duration;

use yd_core::feed::SourceCmd;
use yd_core::ids::CommandId;
use yd_core::inbox::{Closed, FeedTx};

use super::protocol::{Message, parse, read_frame};
use session::Debugger;

mod session;

pub use yd_core::command::tools::debug::{
    ATTACH, BREAK, CONTINUE, EVAL, EXPAND, LAUNCH, RESTART, STEP_IN, STEP_OUT, STEP_OVER, STOP,
    WATCH,
};
pub use yd_core::feed::DEBUGGER as FEED;

/// Who asked. An agent may read a paused program but never move it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    User,
    Agent,
}

/// One ask of this source and who made it.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub cmd: SourceCmd,
    pub from: Origin,
}

/// Where an adapter's program is found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locate<'a> {
    /// `xcrun -f <name>`, else `<name>` on `PATH`.
    Xcrun(&'a str),
    Path(&'a str),
    At(&'a str),
}

/// One debugger this source can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adapter<'a> {
    /// What the `state` row's `adapter` cell says.
    pub name: &'a str,
    /// The `adapterID` sent in `initialize`.
    pub id: &'a str,
    pub locate: Locate<'a>,
    pub args: &'a [&'a str],
}

/// The adapters tried in order; the first one installed runs.
pub const ADAPTERS: &[Adapter<'static>] = &[
    Adapter {
        name: "LLDB",
        id: "lldb-dap",
        locate: Locate::Xcrun("lldb-dap"),
        args: &[],
    },
    Adapter {
        name: "GDB",
        id: "gdb",
        locate: Locate::Path("gdb"),
        args: &["--interpreter=dap"],
    },
];

/// What a command does here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Launch,
    Attach,
    /// The request that moves a paused program: `continue`, `next`, `stepIn`, `stepOut`.
    Resume(&'static str),
    Restart,
    Stop,
    Break,
    Watch,
    Eval,
    Expand,
}

/// A command this source carries out. `user_only` ones change what the program runs, so no
/// ask an agent makes may send them; a watch or a console line can run code, so they are too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verb {
    pub id: CommandId,
    pub user_only: bool,
    action: Action,
}

const fn verb(id: CommandId, user_only: bool, action: Action) -> Verb {
    Verb {
        id,
        user_only,
        action,
    }
}

pub const COMMANDS: &[Verb] = &[
    verb(LAUNCH, true, Action::Launch),
    verb(ATTACH, true, Action::Attach),
    verb(CONTINUE, true, Action::Resume("continue")),
    verb(STEP_OVER, true, Action::Resume("next")),
    verb(STEP_IN, true, Action::Resume("stepIn")),
    verb(STEP_OUT, true, Action::Resume("stepOut")),
    verb(RESTART, true, Action::Restart),
    verb(STOP, true, Action::Stop),
    verb(BREAK, true, Action::Break),
    verb(WATCH, true, Action::Watch),
    verb(EVAL, true, Action::Eval),
    verb(EXPAND, false, Action::Expand),
];

/// Messages the session's queue holds before the adapter's reader waits.
const QUEUE: usize = 1024;
/// Requests queued for the adapter's stdin before the session gives it up as not reading.
const OUT_QUEUE: usize = 256;
/// How often a stopping adapter's deadline is looked at.
const TICK: Duration = Duration::from_millis(250);
/// The longest `xcrun -f` may take.
const XCRUN_TIMEOUT: Duration = Duration::from_secs(5);

/// What reaches the session's one thread.
enum Input {
    Ask(Ask),
    Gone,
    Adapter { generation: u32, message: Message },
    Ended { generation: u32, why: String },
}

/// A running adapter. Dropping it kills the adapter's whole group.
struct Link {
    child: Child,
    writer: SyncSender<Vec<u8>>,
}

impl Link {
    fn spawn(
        program: &Path,
        args: &[&str],
        generation: u32,
        inputs: SyncSender<Input>,
    ) -> io::Result<Self> {
        #[allow(clippy::disallowed_methods, reason = "the debug adapter spawn")]
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            kill(&mut child);
            return Err(io::Error::other("the adapter has no stdio"));
        };
        let (writer, frames) = sync_channel::<Vec<u8>>(OUT_QUEUE);
        thread::spawn(move || {
            for bytes in frames {
                if stdin
                    .write_all(&bytes)
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    return;
                }
            }
        });
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let input = match read_frame(&mut reader) {
                    Ok(bytes) => match parse(&bytes) {
                        Some(message) => Input::Adapter {
                            generation,
                            message,
                        },
                        None => continue,
                    },
                    Err(why) => {
                        let why = why.to_string();
                        let _ = inputs.send(Input::Ended { generation, why });
                        return;
                    }
                };
                if inputs.send(input).is_err() {
                    return;
                }
            }
        });
        Ok(Self { child, writer })
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        kill(&mut self.child);
    }
}

/// Kills `child`'s whole group and reaps it.
fn kill(child: &mut Child) {
    let group = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw);
    if let Some(group) = group {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::Kill);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Where an adapter's program is, if it is installed.
pub fn locate(locate: &Locate<'_>) -> Option<PathBuf> {
    match *locate {
        Locate::At(path) => Some(PathBuf::from(path)).filter(|p| runnable(p)),
        Locate::Path(name) => on_path(name),
        Locate::Xcrun(name) => xcrun(name).or_else(|| on_path(name)),
    }
}

fn runnable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|file| runnable(file))
}

fn xcrun(name: &str) -> Option<PathBuf> {
    let xcrun = Path::new("/usr/bin/xcrun");
    if !runnable(xcrun) {
        return None;
    }
    #[allow(clippy::disallowed_methods, reason = "the xcrun lookup")]
    let mut child = Command::new(xcrun)
        .args(["-f", name])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (done, got) = channel();
    thread::spawn(move || {
        let mut out = Vec::new();
        let read = stdout.take(4096).read_to_end(&mut out);
        let _ = done.send(read.map(|_| out));
    });
    let Ok(Ok(out)) = got.recv_timeout(XCRUN_TIMEOUT) else {
        kill(&mut child);
        return None;
    };
    let status = child.wait().ok()?;
    let found = PathBuf::from(String::from_utf8(out).ok()?.trim());
    (status.success() && runnable(&found)).then_some(found)
}

/// The registry's entry: every ask from the app's own commands is the user's.
pub fn run(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    let (tx, asks) = sync_channel(QUEUE);
    thread::spawn(move || {
        for cmd in cmds {
            let ask = Ask {
                cmd,
                from: Origin::User,
            };
            if tx.send(ask).is_err() {
                return;
            }
        }
    });
    serve(out, asks, ADAPTERS)
}

/// Runs the debugger until `asks` closes: the first adapter of `adapters` that is installed
/// drives each launch or attach, and every row of the feed comes from here.
pub fn serve(out: FeedTx, asks: Receiver<Ask>, adapters: &[Adapter<'_>]) -> Result<(), Closed> {
    let (tx, inputs) = sync_channel(QUEUE);
    let forward = tx.clone();
    thread::spawn(move || {
        for ask in asks {
            if forward.send(Input::Ask(ask)).is_err() {
                return;
            }
        }
        let _ = forward.send(Input::Gone);
    });
    let mut debugger = Debugger::new(out, adapters, tx);
    debugger.publish()?;
    loop {
        match inputs.recv_timeout(TICK) {
            Ok(Input::Ask(ask)) => debugger.ask(ask)?,
            Ok(Input::Adapter {
                generation,
                message,
            }) => debugger.message(generation, message)?,
            Ok(Input::Ended { generation, why }) => debugger.ended(generation, why)?,
            Ok(Input::Gone) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => debugger.tick()?,
        }
    }
}

#[cfg(test)]
mod tests;
