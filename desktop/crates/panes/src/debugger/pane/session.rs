//! The `state` row read as a session: what runs, in what state, and which controls apply.

use yd_core::feed::Table;
use yd_core::ids::RowId;
use yd_core::row::Row;

/// The word the source's `status` cell carries; a newer source's word is kept as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Idle,
    NotInstalled,
    Starting,
    Running,
    Paused,
    Exited,
    Stopped,
    Error,
    Other(String),
}

impl Status {
    fn of(word: &str) -> Status {
        match word {
            "idle" => Status::Idle,
            "not installed" => Status::NotInstalled,
            "starting" => Status::Starting,
            "running" => Status::Running,
            "paused" => Status::Paused,
            "exited" => Status::Exited,
            "stopped" => Status::Stopped,
            "error" => Status::Error,
            other => Status::Other(other.to_owned()),
        }
    }
}

/// The six controls of the header, in the order it draws them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Continue,
    StepOver,
    StepIn,
    StepOut,
    Restart,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub adapter: String,
    pub program: String,
    pub build: String,
    pub pid: String,
    pub threads: String,
    pub status: Status,
    pub reason: String,
    pub path: String,
    pub line: Option<u32>,
}

impl Session {
    /// The session the `debugger` table's `state` row describes; none before the source
    /// has said anything.
    pub fn of(table: Option<&Table>) -> Option<Session> {
        let row = table?.find(&RowId::new("state"))?;
        Some(Session::from(row))
    }

    pub fn paused(&self) -> bool {
        self.status == Status::Paused
    }

    /// Whether a program is, or was, under the debugger: there is something to look at.
    pub fn started(&self) -> bool {
        matches!(
            self.status,
            Status::Starting
                | Status::Running
                | Status::Paused
                | Status::Exited
                | Status::Stopped
                | Status::Error
        )
    }

    /// A move of the program needs it paused; stopping needs it alive; a restart needs a
    /// program the source remembers.
    pub fn can(&self, control: Control) -> bool {
        let alive = matches!(
            self.status,
            Status::Starting | Status::Running | Status::Paused
        );
        match control {
            Control::Continue | Control::StepOver | Control::StepIn | Control::StepOut => {
                self.paused()
            }
            Control::Stop => alive,
            Control::Restart => self.started() && !self.program.is_empty(),
        }
    }

    /// What the header says after the program: the build and pid in dim, `debug build · pid
    /// 48213 · `, then the state, `paused at breakpoint 2`.
    pub fn headline(&self) -> (String, String) {
        let reason = self.reason.trim();
        let state = match &self.status {
            Status::Idle => "not started".to_owned(),
            Status::NotInstalled => "debugger not installed".to_owned(),
            Status::Starting => "starting".to_owned(),
            Status::Running => "running".to_owned(),
            Status::Paused if reason.is_empty() => "paused".to_owned(),
            Status::Paused if reason == "step" => "paused after a step".to_owned(),
            Status::Paused => format!("paused at {reason}"),
            Status::Exited => "program exited".to_owned(),
            Status::Stopped => "stopped".to_owned(),
            Status::Error => "stopped by an error".to_owned(),
            Status::Other(word) => word.clone(),
        };
        let pid = (!self.pid.is_empty()).then(|| format!("pid {}", self.pid));
        let facts: Vec<String> = [Some(self.build.clone()), pid]
            .into_iter()
            .flatten()
            .filter(|fact| !fact.is_empty())
            .collect();
        let lead = facts.iter().map(|fact| format!("{fact} · ")).collect();
        (lead, state)
    }

    /// The pane meta of the tab bar: `pid 48213 · 5 threads`.
    pub fn meta(&self) -> String {
        let threads = match self.threads.as_str() {
            "" | "0" => None,
            "1" => Some("1 thread".to_owned()),
            many => Some(format!("{many} threads")),
        };
        let pid = (!self.pid.is_empty()).then(|| format!("pid {}", self.pid));
        let parts: Vec<String> = [pid, threads].into_iter().flatten().collect();
        parts.join(" · ")
    }
}

impl From<&Row> for Session {
    fn from(row: &Row) -> Session {
        let text = |field: &str| row.words(field).unwrap_or_default().to_owned();
        Session {
            adapter: text("adapter"),
            program: text("program"),
            build: text("build"),
            pid: text("pid"),
            threads: text("threads"),
            status: Status::of(row.words("status").unwrap_or_default()),
            reason: text("reason"),
            path: text("path"),
            line: row.words("line").and_then(|line| line.parse().ok()),
        }
    }
}
