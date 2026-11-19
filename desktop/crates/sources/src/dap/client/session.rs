//! The debugger's state, stepped by what the user asks and by what the adapter says.

use std::collections::{BTreeSet, HashMap};
use std::mem;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yd_core::command::table::{Run, Target};
use yd_core::feed::{FeedEvent, FeedStatus, SourceCmd};
use yd_core::ids::RowId;
use yd_core::inbox::{Closed, FeedTx};
use yd_core::kind::Kind;
use yd_core::row::{Row, Value as Cell};

use super::super::protocol::{self, Message, body, frame, refusal, request};
use super::{Action, Adapter, Ask, COMMANDS, Input, Link, Origin, locate};
use rows::{Console, Role, clip, line_of, looked, mark_row, name_of};

mod rows;

/// Frames asked for at a stop, and variables kept from one answer.
const MAX_FRAMES: u64 = 64;
const MAX_VARS: usize = 1000;
/// How long a stopping adapter has to leave before it is killed.
const GRACE: Duration = Duration::from_secs(3);
/// How long the adapter may take to answer starting, and anything else.
#[cfg(not(test))]
const LIMITS: (Duration, Duration) = (Duration::from_secs(30), Duration::from_secs(10));
#[cfg(test)]
const LIMITS: (Duration, Duration) = (Duration::from_secs(5), Duration::from_secs(5));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Idle,
    NotInstalled,
    Starting,
    Running,
    Paused,
    Exited,
    Stopped,
    Error,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::NotInstalled => "not installed",
            Status::Starting => "starting",
            Status::Running => "running",
            Status::Paused => "paused",
            Status::Exited => "exited",
            Status::Stopped => "stopped",
            Status::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Goal {
    Launch(String),
    Attach(u32),
}

/// What an answer is for, kept by the request's `seq`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Initialize,
    Start,
    Breakpoints(String),
    Configured,
    Threads(Option<u64>),
    Stack,
    Scopes,
    Variables(Option<String>),
    Watch(u64),
    Eval,
    Resume,
    Disconnect,
}

impl Pending {
    fn limit(&self) -> Duration {
        match self {
            Pending::Initialize | Pending::Start => LIMITS.0,
            _ => LIMITS.1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mark {
    n: u64,
    path: String,
    line: u64,
    id: Option<u64>,
    hits: u64,
}

#[derive(Debug, Default)]
struct State {
    adapter: String,
    program: String,
    pid: Option<u64>,
    threads: usize,
    reason: String,
    path: String,
    line: Option<u64>,
}

pub(super) struct Debugger<'a> {
    out: FeedTx,
    adapters: &'a [Adapter<'a>],
    inputs: SyncSender<Input>,
    generation: u32,
    link: Option<Link>,
    seq: u64,
    /// Each request's purpose, command and when it went, by `seq`.
    pending: HashMap<u64, (Pending, &'static str, Instant)>,
    goal: Option<Goal>,
    status: Status,
    state: State,
    configured: bool,
    stopping: Option<Instant>,
    marks: Vec<Mark>,
    next_mark: u64,
    watches: Vec<(u64, String)>,
    console: Console,
    thread: Option<u64>,
    frame: Option<u64>,
    frames: usize,
    refs: HashMap<String, u64>,
    was: HashMap<String, String>,
    seen: HashMap<String, String>,
    shown: BTreeSet<String>,
    fresh: BTreeSet<String>,
    waiting: usize,
}

impl<'a> Debugger<'a> {
    pub(super) fn new(out: FeedTx, adapters: &'a [Adapter<'a>], inputs: SyncSender<Input>) -> Self {
        Self {
            out,
            adapters,
            inputs,
            generation: 0,
            link: None,
            seq: 0,
            pending: HashMap::new(),
            goal: None,
            status: Status::Idle,
            state: State::default(),
            configured: false,
            stopping: None,
            marks: Vec::new(),
            next_mark: 0,
            watches: Vec::new(),
            console: Console::default(),
            thread: None,
            frame: None,
            frames: 0,
            refs: HashMap::new(),
            was: HashMap::new(),
            seen: HashMap::new(),
            shown: BTreeSet::new(),
            fresh: BTreeSet::new(),
            waiting: 0,
        }
    }

    pub(super) fn publish(&self) -> Result<(), Closed> {
        let s = &self.state;
        let row = Row::new("state")
            .text("adapter", &s.adapter)
            .text("program", &s.program)
            .text("build", "")
            .text("pid", s.pid.map(|p| p.to_string()).unwrap_or_default())
            .text("threads", s.threads.to_string())
            .text("status", self.status.word())
            .text("reason", clip(&s.reason))
            .text("path", &s.path)
            .text("line", s.line.map(|l| l.to_string()).unwrap_or_default());
        self.out.send(FeedEvent::Upsert(row))
    }

    fn say(&mut self, text: &str, role: Role) -> Result<(), Closed> {
        self.console.push(&self.out, text, role)
    }

    pub(super) fn ask(&mut self, ask: Ask) -> Result<(), Closed> {
        let SourceCmd::Run(run) = ask.cmd else {
            return Ok(());
        };
        let Some(verb) = COMMANDS.iter().find(|v| v.id == run.command) else {
            return Ok(());
        };
        if ask.from == Origin::Agent && verb.user_only {
            let refused = format!(
                "{}: an agent may read the paused program but not run, step or change it",
                run.command.0
            );
            return self.say(&refused, Role::Error);
        }
        let text = run.text.clone().unwrap_or_default();
        match verb.action {
            Action::Launch => self.begin(Goal::Launch(text)),
            Action::Attach => match text.trim().parse::<u32>() {
                Ok(pid) => self.begin(Goal::Attach(pid)),
                Err(_) => self.say("attach needs a process id", Role::Error),
            },
            Action::Resume(command) => self.resume(command),
            Action::Restart => match self.goal.clone() {
                Some(goal) => self.begin(goal),
                None => self.say("nothing to restart", Role::Error),
            },
            Action::Stop => self.stop(),
            Action::Break => self.toggle(&run),
            Action::Watch => self.watch(text),
            Action::Eval => self.eval(text),
            Action::Expand => self.expand(&run),
        }
    }

    fn begin(&mut self, goal: Goal) -> Result<(), Closed> {
        self.end(Status::Stopped);
        self.clear_stop()?;
        self.state = State::default();
        self.state.program = match &goal {
            Goal::Launch(path) => name_of(path),
            Goal::Attach(pid) => format!("pid {pid}"),
        };
        self.goal = Some(goal);
        let found = self
            .adapters
            .iter()
            .find_map(|a| locate(&a.locate).map(|path| (*a, path)));
        let Some((adapter, path)) = found else {
            self.status = Status::NotInstalled;
            let looked: Vec<String> = self.adapters.iter().map(|a| looked(&a.locate)).collect();
            self.state.reason = format!("no debugger found: {}", looked.join("; "));
            return self.publish();
        };
        self.state.adapter = adapter.name.to_owned();
        self.generation = self.generation.wrapping_add(1);
        match Link::spawn(&path, adapter.args, self.generation, self.inputs.clone()) {
            Ok(link) => self.link = Some(link),
            Err(why) => {
                self.status = Status::Error;
                self.state.reason = format!("{} could not be started: {why}", path.display());
                return self.publish();
            }
        }
        self.status = Status::Starting;
        self.configured = false;
        for mark in &mut self.marks {
            mark.id = None;
        }
        self.publish()?;
        let args = json!({
            "clientID": "yi-desktop", "clientName": "Yi Desktop", "adapterID": adapter.id,
            "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path",
            "supportsVariableType": true,
        });
        self.send("initialize", args, Pending::Initialize)
    }

    /// Kills the adapter, if one runs, and leaves `status` for the reader.
    fn end(&mut self, status: Status) {
        if self.link.take().is_some() {
            self.status = status;
        }
        self.pending.clear();
        self.stopping = None;
        self.thread = None;
        self.frame = None;
    }

    fn send(&mut self, command: &'static str, args: Value, what: Pending) -> Result<(), Closed> {
        let Some(link) = &self.link else {
            return Ok(());
        };
        self.seq = self.seq.saturating_add(1);
        match link
            .writer
            .try_send(frame(&request(self.seq, command, args)))
        {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                return self.fail("the adapter stopped reading its input".to_owned());
            }
        }
        self.pending
            .insert(self.seq, (what, command, Instant::now()));
        Ok(())
    }

    fn fail(&mut self, why: String) -> Result<(), Closed> {
        self.end(Status::Error);
        self.status = Status::Error;
        self.state.reason = why.clone();
        self.publish()?;
        self.out
            .send(FeedEvent::Status(FeedStatus::Error(clip(&why))))
    }

    fn resume(&mut self, command: &'static str) -> Result<(), Closed> {
        let (Status::Paused, Some(thread)) = (self.status, self.thread) else {
            return self.say("the program is not paused", Role::Error);
        };
        self.send(command, json!({"threadId": thread}), Pending::Resume)?;
        self.status = Status::Running;
        self.state.reason.clear();
        self.publish()
    }

    fn stop(&mut self) -> Result<(), Closed> {
        if self.link.is_none() {
            return self.say("no program is being debugged", Role::Error);
        }
        let launched = matches!(self.goal, Some(Goal::Launch(_)));
        self.send(
            "disconnect",
            json!({"terminateDebuggee": launched}),
            Pending::Disconnect,
        )?;
        self.stopping = Instant::now().checked_add(GRACE);
        Ok(())
    }

    /// Kills a stopping adapter past its deadline; an answer past its limit ends the session,
    /// or for a watch or console line says so there. One late answer a tick.
    pub(super) fn tick(&mut self) -> Result<(), Closed> {
        let now = Instant::now();
        if self.stopping.is_some_and(|at| now >= at) {
            self.end(Status::Stopped);
            return self.publish();
        }
        let late = |s: &(Pending, &str, Instant)| {
            !matches!(s.0, Pending::Disconnect) && now.duration_since(s.2) >= s.0.limit()
        };
        let first = self
            .pending
            .iter()
            .filter(|(_, s)| late(s))
            .map(|(n, _)| *n)
            .min();
        let Some((what, command, _)) = first.and_then(|n| self.pending.remove(&n)) else {
            return Ok(());
        };
        let why = format!(
            "the adapter did not answer {} within {} s",
            command,
            what.limit().as_secs()
        );
        match what {
            Pending::Watch(_) | Pending::Eval => self.say(&why, Role::Error),
            _ => self.fail(why),
        }
    }

    fn toggle(&mut self, run: &Run) -> Result<(), Closed> {
        let place = match &run.target {
            Target::Thing(at) if matches!(at.kind(), Kind::Lines | Kind::File) => at
                .part_text()
                .and_then(line_of)
                .map(|l| (at.id().to_owned(), l)),
            Target::App | Target::Pane(_) | Target::Thing(_) => None,
        };
        let Some((path, line)) = place else {
            return self.say("a breakpoint needs a file and a line", Role::Error);
        };
        if let Some(at) = self
            .marks
            .iter()
            .position(|m| m.path == path && m.line == line)
        {
            let gone = self.marks.remove(at);
            self.out
                .send(FeedEvent::Remove(RowId::new(format!("bp:{}", gone.n))))?;
        } else {
            self.next_mark = self.next_mark.saturating_add(1);
            let mark = Mark {
                n: self.next_mark,
                path: path.clone(),
                line,
                id: None,
                hits: 0,
            };
            self.out.send(FeedEvent::Upsert(mark_row(&mark)))?;
            self.marks.push(mark);
        }
        if self.configured {
            self.set_breakpoints(&path)?;
        }
        Ok(())
    }

    fn set_breakpoints(&mut self, path: &str) -> Result<(), Closed> {
        let lines: Vec<Value> = self
            .marks
            .iter()
            .filter(|m| m.path == path)
            .map(|m| json!({"line": m.line}))
            .collect();
        let args = json!({"source": {"path": path}, "breakpoints": lines});
        self.send(
            "setBreakpoints",
            args,
            Pending::Breakpoints(path.to_owned()),
        )
    }

    fn watch(&mut self, expression: String) -> Result<(), Closed> {
        if expression.trim().is_empty() {
            return self.say("a watch needs an expression", Role::Error);
        }
        let n = u64::try_from(self.watches.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let row = Row::new(format!("watch:{n}"))
            .text("expression", clip(&expression))
            .text("value", "");
        self.out.send(FeedEvent::Upsert(row))?;
        self.watches.push((n, expression.clone()));
        if self.status == Status::Paused {
            self.evaluate(n, &expression)?;
        }
        Ok(())
    }

    fn evaluate(&mut self, n: u64, expression: &str) -> Result<(), Closed> {
        let args = json!({"expression": expression, "frameId": self.frame, "context": "watch"});
        self.send("evaluate", args, Pending::Watch(n))
    }

    fn eval(&mut self, text: String) -> Result<(), Closed> {
        self.say(&text, Role::Input)?;
        if self.link.is_none() {
            return self.say("no program is being debugged", Role::Error);
        }
        let args = json!({"expression": text, "frameId": self.frame, "context": "repl"});
        self.send("evaluate", args, Pending::Eval)
    }

    fn expand(&mut self, run: &Run) -> Result<(), Closed> {
        let Target::Thing(at) = &run.target else {
            return Ok(());
        };
        let Some(path) = at.id().strip_prefix("var:") else {
            return Ok(());
        };
        let Some(reference) = self.refs.get(path).copied() else {
            return Ok(());
        };
        let args = json!({"variablesReference": reference});
        self.send("variables", args, Pending::Variables(Some(path.to_owned())))
    }

    /// An adapter's output ended, or broke a bound: the session is over.
    pub(super) fn ended(&mut self, generation: u32, why: String) -> Result<(), Closed> {
        if generation != self.generation || self.link.is_none() {
            return Ok(());
        }
        if self.stopping.is_some() || self.status == Status::Exited {
            let status = if self.status == Status::Exited {
                Status::Exited
            } else {
                Status::Stopped
            };
            self.end(status);
            return self.publish();
        }
        self.fail(why)
    }

    pub(super) fn message(&mut self, generation: u32, message: Message) -> Result<(), Closed> {
        if generation != self.generation || self.link.is_none() {
            return Ok(());
        }
        match message {
            Message::Response {
                request_seq,
                success,
                message,
                body,
            } => match self.pending.remove(&request_seq) {
                Some((what, _, _)) => self.answer(what, success, message, &body),
                None => Ok(()),
            },
            Message::Event { event, body } => self.event(&event, &body),
            Message::Request { seq, command } => {
                self.seq = self.seq.saturating_add(1);
                if let Some(link) = &self.link {
                    let _ = link
                        .writer
                        .try_send(frame(&refusal(self.seq, seq, &command)));
                }
                Ok(())
            }
            Message::Other => Ok(()),
        }
    }

    fn answer(
        &mut self,
        pending: Pending,
        success: bool,
        message: Option<String>,
        reply: &Value,
    ) -> Result<(), Closed> {
        let refused = || message.clone().unwrap_or_else(|| "refused".to_owned());
        match pending {
            Pending::Initialize if !success => self.fail(format!("initialize: {}", refused())),
            Pending::Initialize => self.start(),
            Pending::Start if !success => self.fail(refused()),
            Pending::Breakpoints(path) => self.verified(&path, reply),
            Pending::Threads(want) => self.threads(want, reply),
            Pending::Stack => self.stack(reply),
            Pending::Scopes => self.scopes(reply),
            Pending::Variables(parent) => self.variables(parent, reply),
            Pending::Watch(n) => {
                let got: protocol::Evaluated = body(reply);
                let value = if success { got.result } else { refused() };
                self.watched(n, value, got.kind)
            }
            Pending::Eval if success => {
                let got: protocol::Evaluated = body(reply);
                for line in got.result.lines() {
                    self.say(line, Role::Output)?;
                }
                Ok(())
            }
            Pending::Eval | Pending::Resume if !success => self.say(&refused(), Role::Error),
            Pending::Start
            | Pending::Configured
            | Pending::Eval
            | Pending::Resume
            | Pending::Disconnect => Ok(()),
        }
    }

    fn start(&mut self) -> Result<(), Closed> {
        match self.goal.clone() {
            Some(Goal::Launch(program)) => {
                let args = json!({"program": program, "stopOnEntry": false});
                self.send("launch", args, Pending::Start)
            }
            Some(Goal::Attach(pid)) => self.send("attach", json!({"pid": pid}), Pending::Start),
            None => Ok(()),
        }
    }

    fn event(&mut self, event: &str, reply: &Value) -> Result<(), Closed> {
        match event {
            "initialized" => {
                self.configured = true;
                let paths: BTreeSet<String> = self.marks.iter().map(|m| m.path.clone()).collect();
                for path in paths {
                    self.set_breakpoints(&path)?;
                }
                self.send("configurationDone", json!({}), Pending::Configured)
            }
            "stopped" => {
                let stopped: protocol::Stopped = body(reply);
                self.status = Status::Paused;
                self.state.reason = stopped.reason;
                for mark in &mut self.marks {
                    if mark
                        .id
                        .is_some_and(|id| stopped.hit_breakpoint_ids.contains(&id))
                    {
                        mark.hits = mark.hits.saturating_add(1);
                        self.out.send(FeedEvent::Upsert(mark_row(mark)))?;
                    }
                }
                self.was = mem::take(&mut self.seen);
                self.publish()?;
                let thread = stopped.thread_id;
                self.send("threads", json!({}), Pending::Threads(thread))
            }
            "continued" => {
                self.status = Status::Running;
                self.publish()
            }
            "process" => {
                let process: protocol::Process = body(reply);
                self.state.pid = process.system_process_id;
                if self.state.program.is_empty() {
                    self.state.program = name_of(&process.name);
                }
                self.publish()
            }
            "output" => {
                let output: protocol::Output = body(reply);
                let role = match output.category.as_deref() {
                    Some("telemetry") => return Ok(()),
                    Some("stderr") => Role::Error,
                    _ => Role::Output,
                };
                for line in output.output.lines().filter(|l| !l.trim().is_empty()) {
                    self.say(line, role)?;
                }
                Ok(())
            }
            "exited" => {
                let exited: protocol::Exited = body(reply);
                self.status = Status::Exited;
                self.state.reason = exited
                    .exit_code
                    .map(|code| format!("exit code {code}"))
                    .unwrap_or_default();
                self.publish()
            }
            "terminated" => {
                if self.status != Status::Error {
                    self.status = Status::Exited;
                }
                self.publish()?;
                self.send("disconnect", json!({}), Pending::Disconnect)?;
                self.stopping = Instant::now().checked_add(GRACE);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn verified(&mut self, path: &str, reply: &Value) -> Result<(), Closed> {
        let got: protocol::Breakpoints = body(reply);
        let marks = self.marks.iter_mut().filter(|m| m.path == path);
        for (mark, set) in marks.zip(got.breakpoints) {
            mark.id = set.id;
        }
        Ok(())
    }

    fn threads(&mut self, want: Option<u64>, reply: &Value) -> Result<(), Closed> {
        let got: protocol::Threads = body(reply);
        self.state.threads = got.threads.len();
        let thread = want
            .filter(|w| got.threads.iter().any(|t| t.id == *w))
            .or_else(|| got.threads.first().map(|t| t.id));
        self.thread = thread;
        self.publish()?;
        let Some(thread) = thread else {
            return Ok(());
        };
        let args = json!({"threadId": thread, "startFrame": 0, "levels": MAX_FRAMES});
        self.send("stackTrace", args, Pending::Stack)
    }

    fn stack(&mut self, reply: &Value) -> Result<(), Closed> {
        let got: protocol::Stack = body(reply);
        for (n, frame) in got.stack_frames.iter().enumerate() {
            let path = frame.source.as_ref().and_then(protocol::Source::file);
            let at = match &path {
                Some(path) => format!("{}:{}", name_of(path), frame.line),
                None => "library".to_owned(),
            };
            let row = Row::new(format!("frame:{n}"))
                .text("frame", format!("#{n}"))
                .text("function", clip(&frame.name))
                .text("at", at)
                .cell("current", Cell::Flag(n == 0))
                .text("path", path.unwrap_or_default())
                .text("line", frame.line.to_string());
            self.out.send(FeedEvent::Upsert(row))?;
        }
        for n in got.stack_frames.len()..self.frames {
            self.out
                .send(FeedEvent::Remove(RowId::new(format!("frame:{n}"))))?;
        }
        self.frames = got.stack_frames.len();
        let top = got.stack_frames.first();
        self.frame = top.map(|f| f.id);
        self.state.path = top
            .and_then(|f| f.source.as_ref())
            .and_then(protocol::Source::file)
            .unwrap_or_default();
        self.state.line = top.map(|f| f.line);
        self.publish()?;
        match self.frame {
            Some(frame) => self.send("scopes", json!({"frameId": frame}), Pending::Scopes),
            None => self.settled(),
        }
    }

    /// The locals and arguments; an adapter that hints neither gives its first scope.
    fn scopes(&mut self, reply: &Value) -> Result<(), Closed> {
        let got: protocol::Scopes = body(reply);
        let hinted = |s: &protocol::Scope| {
            matches!(s.presentation_hint.as_deref(), Some("locals" | "arguments"))
        };
        let mut wanted: Vec<u64> = got
            .scopes
            .iter()
            .filter(|s| hinted(s))
            .map(|s| s.variables_reference)
            .collect();
        if wanted.is_empty() {
            wanted.extend(got.scopes.first().map(|s| s.variables_reference));
        }
        self.refs.clear();
        self.fresh.clear();
        self.waiting = wanted.len();
        if wanted.is_empty() {
            return self.settled();
        }
        for reference in wanted {
            let args = json!({"variablesReference": reference});
            self.send("variables", args, Pending::Variables(None))?;
        }
        Ok(())
    }

    fn variables(&mut self, parent: Option<String>, reply: &Value) -> Result<(), Closed> {
        let got: protocol::Variables = body(reply);
        for var in got.variables.into_iter().take(MAX_VARS) {
            let path = match &parent {
                Some(parent) => format!("{parent}.{}", var.name),
                None => var.name.clone(),
            };
            let id = format!("var:{path}");
            let was = self.was.get(&id).filter(|w| **w != var.value).cloned();
            let mut row = Row::new(id.clone())
                .text("name", clip(&var.name))
                .text("value", clip(&var.value))
                .text("type", clip(var.kind.as_deref().unwrap_or_default()))
                .cell("changed", Cell::Flag(was.is_some()))
                .cell("expandable", Cell::Flag(var.variables_reference > 0));
            if let Some(was) = was {
                row = row.text("was", clip(&was));
            }
            row.parent = parent.as_ref().map(|p| RowId::new(format!("var:{p}")));
            if var.variables_reference > 0 {
                self.refs.insert(path, var.variables_reference);
            }
            self.seen.insert(id.clone(), var.value);
            self.out.send(FeedEvent::Upsert(row))?;
            match parent {
                Some(_) => self.shown.insert(id),
                None => self.fresh.insert(id),
            };
        }
        if parent.is_none() {
            self.waiting = self.waiting.saturating_sub(1);
            if self.waiting == 0 {
                return self.settled();
            }
        }
        Ok(())
    }

    /// Every variable of this stop is in: drop the last stop's rows and re-evaluate watches.
    fn settled(&mut self) -> Result<(), Closed> {
        for gone in self.shown.difference(&self.fresh) {
            self.out.send(FeedEvent::Remove(RowId::new(gone.clone())))?;
        }
        self.shown = mem::take(&mut self.fresh);
        for (n, expression) in self.watches.clone() {
            self.evaluate(n, &expression)?;
        }
        Ok(())
    }

    fn watched(&mut self, n: u64, value: String, kind: Option<String>) -> Result<(), Closed> {
        let (expression, id) = match self.watches.iter().find(|(m, _)| *m == n) {
            Some((_, expression)) => (expression.clone(), format!("watch:{n}")),
            None => return Ok(()),
        };
        let was = self.was.get(&id).filter(|w| **w != value).cloned();
        let mut row = Row::new(id.clone())
            .text("expression", clip(&expression))
            .text("value", clip(&value))
            .text("type", clip(kind.as_deref().unwrap_or_default()))
            .cell("changed", Cell::Flag(was.is_some()));
        if let Some(was) = was {
            row = row.text("was", clip(&was));
        }
        self.seen.insert(id, value);
        self.out.send(FeedEvent::Upsert(row))
    }

    /// A new session starts with no stop: the old one's frames and variables go.
    fn clear_stop(&mut self) -> Result<(), Closed> {
        for gone in mem::take(&mut self.shown) {
            self.out.send(FeedEvent::Remove(RowId::new(gone)))?;
        }
        for n in 0..self.frames {
            self.out
                .send(FeedEvent::Remove(RowId::new(format!("frame:{n}"))))?;
        }
        self.frames = 0;
        self.refs.clear();
        self.was.clear();
        self.seen.clear();
        Ok(())
    }
}
