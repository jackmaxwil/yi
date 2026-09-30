use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Map, Value, json};
use tokio::sync::{Notify, mpsc, oneshot};
use yi_types::kernel::{
    ConnectionInfo, ExecuteResult, ExecuteStatus, JupyterMessage, KernelSentAgentMessage,
};
use zeromq::ZmqMessage;

use crate::bootstrap::{BootstrapOptions, ProgressFn, ensure_kernel_python};
use crate::connection::{has_resolved_ports, make_connection, random_hex, read_connection_info};
use crate::framing::{build_message, encode};
use crate::journal::record_orphan_process_state;
use crate::reduce::{CellState, parse_sent_agent_message};
use crate::{
    DEFAULT_MAX_OUTPUT_CHARS, HOST_REQUEST_DISPOSE_TIMEOUT_MS, KERNEL_ABORT_GRACE_MS,
    KERNEL_BUSY_AFTER_INTERRUPT_MESSAGE, KERNEL_BUSY_INTERRUPT_INTERVAL_MS,
    KERNEL_BUSY_REUSE_WAIT_MS, KERNEL_SHUTDOWN_TIMEOUT_MS, MAX_LATE_SENT_AGENT_MESSAGE_HANDLERS,
    PORTS_RESOLVE_TIMEOUT_MS, SNAPSHOT_DISPOSE_TIMEOUT_MS,
};

const STDERR_TAIL_CAP: usize = 16_384;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExecuteError {
    #[error("{KERNEL_BUSY_AFTER_INTERRUPT_MESSAGE}")]
    BusyAfterInterrupt,
    #[error("Kernel has been shut down")]
    ShutDown,
    #[error("{0}")]
    Failed(String),
}

#[derive(Clone, Default)]
pub struct AbortFlag(Arc<AbortInner>);

#[derive(Default)]
struct AbortInner {
    fired: AtomicBool,
    notify: Notify,
}

impl AbortFlag {
    pub fn fire(&self) {
        self.0.fired.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_fired(&self) -> bool {
        self.0.fired.load(Ordering::SeqCst)
    }

    pub async fn fired(&self) {
        loop {
            // Invariant: registered before the check, so a fire() between them still wakes it.
            let notified = self.0.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_fired() {
                return;
            }
            notified.await;
        }
    }
}

pub type HostReply = Result<Map<String, Value>, String>;
pub type HostFuture = Pin<Box<dyn Future<Output = HostReply> + Send>>;

/// Host-side dispatch for `host.request` comms (design §9.2). Returning `None`
/// means the type is not registered and errors rather than replying.
pub trait HostHandlers: Send + Sync {
    fn dispatch(&self, request_type: &str, payload: Map<String, Value>) -> Option<HostFuture>;

    /// A handle a kernel opened dies with the kernel, so whatever the host is
    /// holding for it is dropped here; a host holding nothing needs no body.
    fn retire(&self) {}
}

/// Where and how the kernel namespace is persisted (design §9). Only
/// sessions with an artifact directory get a revivable snapshot.
#[derive(Debug, Clone)]
pub struct KernelSnapshotConfig {
    pub path: PathBuf,
    pub manifest_path: PathBuf,
    pub max_bytes: Option<u64>,
    pub max_variable_bytes: Option<u64>,
    pub debounce_ms: Option<u64>,
}

pub struct KernelOptions {
    pub python: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub username: String,
    pub home: PathBuf,
    pub runtime_source_dir: PathBuf,
    pub host: Option<Arc<dyn HostHandlers>>,
    pub on_progress: Option<Arc<ProgressFn>>,
    pub snapshot: Option<KernelSnapshotConfig>,
    pub wrap: Option<(String, Vec<String>)>,
}

pub type StreamFn = dyn FnMut(&str, &str) + Send;
pub type LateAgentMessageFn = dyn Fn(KernelSentAgentMessage) + Send + Sync;

#[derive(Default)]
pub struct ExecuteOptions {
    pub abort: Option<AbortFlag>,
    pub on_stream: Option<Box<StreamFn>>,
    pub on_late_sent_agent_message: Option<Box<LateAgentMessageFn>>,
    pub max_output_chars: Option<usize>,
    pub internal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lifecycle {
    Idle,
    Starting,
    Running,
    Shutdown,
}

pub(crate) struct Active {
    pub(crate) cell: CellState,
    pub(crate) started: std::time::Instant,
    pub(crate) on_stream: Option<Box<StreamFn>>,
    pub(crate) on_late: Option<Box<LateAgentMessageFn>>,
    pub(crate) abort: Option<AbortFlag>,
    pub(crate) settled: bool,
    pub(crate) result_tx: Option<oneshot::Sender<Result<ExecuteResult, ExecuteError>>>,
}

pub(crate) type LateHandlers = VecDeque<(String, Arc<LateAgentMessageFn>)>;

pub(crate) struct Channels {
    shell_tx: mpsc::UnboundedSender<Vec<Vec<u8>>>,
    control_tx: mpsc::UnboundedSender<Vec<Vec<u8>>>,
    kill_tx: mpsc::UnboundedSender<()>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

pub(crate) struct Inner {
    pub(crate) python: Option<PathBuf>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) username: String,
    pub(crate) home: PathBuf,
    pub(crate) runtime_source_dir: PathBuf,
    pub(crate) host: Option<Arc<dyn HostHandlers>>,
    pub(crate) on_progress: Option<Arc<ProgressFn>>,
    pub(crate) session: String,
    pub(crate) state: Mutex<Lifecycle>,
    pub(crate) start_generation: AtomicU64,
    pub(crate) start_lock: tokio::sync::Mutex<()>,
    pub(crate) execution_queue: tokio::sync::Mutex<()>,
    pub(crate) active: Mutex<Option<Active>>,
    pub(crate) idle_notify: Notify,
    pub(crate) exited: Mutex<bool>,
    pub(crate) exit_notify: Notify,
    pub(crate) channels: Mutex<Option<Channels>>,
    pub(crate) connection: Mutex<Option<ConnectionInfo>>,
    pub(crate) temp_dir: Mutex<Option<PathBuf>>,
    pub(crate) pending_control: Mutex<HashMap<String, (String, oneshot::Sender<()>)>>,
    pub(crate) comm_targets: Mutex<HashMap<String, String>>,
    pub(crate) handled_host_comm_ids: Mutex<HashSet<String>>,
    pub(crate) late_handlers: Mutex<LateHandlers>,
    // Source of the most recently started cell, retained after it finishes so rlm.run spawns
    // from detached asyncio tasks can still attribute their spawning program.
    pub(crate) last_cell_code: Mutex<Option<String>>,
    pub(crate) kernel_stderr: Mutex<String>,
    pub(crate) in_flight_host: Mutex<Vec<(String, tokio::task::JoinHandle<()>)>>,
    pub(crate) child_pid: Mutex<Option<u32>>,
    pub(crate) snapshot: Option<KernelSnapshotConfig>,
    pub(crate) snapshot_timer: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(crate) checkpoints: crate::snapshot::Checkpoints,
    pub(crate) wrap: Option<(String, Vec<String>)>,
    /// A lock the kernel process holds for as long as it lives, a node slot (D285).
    pub(crate) held: Mutex<Option<std::fs::File>>,
}

pub struct KernelManager {
    pub(crate) inner: Arc<Inner>,
}

fn boot_gate() -> &'static Arc<tokio::sync::Semaphore> {
    static GATE: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    GATE.get_or_init(|| {
        // Above core count because boots are IO-bound (cold imports), but capped
        // so a fan-out can't thrash the FS past the port-resolve window.
        let cpus = std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(4);
        let permits = usize::min(16, usize::max(4, cpus.saturating_mul(2)));
        Arc::new(tokio::sync::Semaphore::new(permits))
    })
}

pub(crate) fn now_iso() -> String {
    #[expect(
        clippy::disallowed_methods,
        reason = "the Jupyter header date field is a timestamp; a clock read is the job"
    )]
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = since_epoch.as_secs();
    let millis = since_epoch.subsec_millis();
    let days = seconds / 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = (seconds % 86_400) / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Hinnant civil-from-days, shared with the runtime scheduler (§15.2 cron math).
pub fn civil_from_days(days: u64) -> (u64, u64, u64) {
    // Howard Hinnant's civil-from-days, unsigned since the epoch is 1970.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let adjusted_year = if month <= 2 { year + 1 } else { year };
    (adjusted_year, month, day)
}

fn spawn_kernel_process(
    inner: &Arc<Inner>,
    python: &std::path::Path,
    connection_path: &std::path::Path,
) -> Result<tokio::process::Child, String> {
    let mut command = match &inner.wrap {
        Some((program, prefix)) => {
            let mut command = tokio::process::Command::new(program);
            command.args(prefix).arg(python);
            command
        }
        None => tokio::process::Command::new(python),
    };
    command
        .args(["-m", "ipykernel_launcher", "-f"])
        .arg(connection_path)
        // Incident: every cell, Yi's own included, landed in the user's ~/.ipython history.
        .arg("--HistoryManager.enabled=False")
        // ipykernel's parent poller exits the kernel if this pid dies
        // (covers SIGKILL of the owner).
        .env("JPY_PARENT_PID", std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &inner.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &inner.env {
        command.env(key, value);
    }
    let child = command
        .spawn()
        .map_err(|error| format!("failed to spawn ipykernel: {error}"))?;
    let pid = child.id();
    if let (Some(pid), Ok(mut slot)) = (pid, inner.child_pid.lock()) {
        *slot = Some(pid);
    }
    if let Some(pid) = pid {
        record_orphan_process_state(pid, true, now_iso());
    }
    Ok(child)
}

pub(crate) fn zmq_message(frames: Vec<Vec<u8>>) -> Option<ZmqMessage> {
    let mut iter = frames.into_iter();
    let mut message = ZmqMessage::from(iter.next()?);
    for frame in iter {
        let tail = ZmqMessage::from(frame);
        let mut swapped = tail;
        swapped.prepend(&message);
        message = swapped;
    }
    Some(message)
}

pub(crate) fn frames_of(message: ZmqMessage) -> Vec<Vec<u8>> {
    message
        .into_vec()
        .into_iter()
        .map(|frame| frame.as_ref().to_vec())
        .collect()
}

impl Inner {
    pub(crate) fn state(&self) -> Lifecycle {
        self.state
            .lock()
            .map(|state| *state)
            .unwrap_or(Lifecycle::Shutdown)
    }

    pub(crate) fn set_state(&self, next: Lifecycle) {
        if let Ok(mut state) = self.state.lock() {
            *state = next;
        }
    }

    fn stale(&self, generation: u64) -> bool {
        self.start_generation.load(Ordering::SeqCst) != generation
    }

    pub(crate) fn diagnostic(&self, message: &str) {
        if let Ok(mut stderr) = self.kernel_stderr.lock() {
            stderr.push_str("[kernel] ");
            stderr.push_str(message);
            if !message.ends_with('\n') {
                stderr.push('\n');
            }
            trim_tail(&mut stderr);
        }
    }

    pub(crate) fn stderr_tail(&self) -> String {
        let tail = self
            .kernel_stderr
            .lock()
            .map(|stderr| {
                let chars: Vec<char> = stderr.chars().collect();
                let start = chars.len().saturating_sub(1_024);
                chars[start..].iter().collect::<String>()
            })
            .unwrap_or_default();
        if tail.is_empty() {
            "(empty)".to_owned()
        } else {
            tail
        }
    }

    pub(crate) fn build(
        &self,
        msg_type: &str,
        content: Map<String, Value>,
    ) -> Result<JupyterMessage, String> {
        Ok(build_message(
            msg_type,
            content,
            &self.session,
            &self.username,
            random_hex(16)?,
            now_iso(),
        ))
    }

    fn send_shell(&self, frames: Vec<Vec<u8>>) -> Result<(), String> {
        let channels = self.channels.lock().map_err(|_| "kernel state poisoned")?;
        let channels = channels.as_ref().ok_or("Kernel channel is not connected")?;
        channels
            .shell_tx
            .send(frames)
            .map_err(|_| "Kernel channel is not connected".to_owned())
    }

    pub(crate) fn send_control(&self, frames: Vec<Vec<u8>>) -> Result<(), String> {
        let channels = self.channels.lock().map_err(|_| "kernel state poisoned")?;
        let channels = channels.as_ref().ok_or("Kernel channel is not connected")?;
        channels
            .control_tx
            .send(frames)
            .map_err(|_| "Kernel channel is not connected".to_owned())
    }

    fn interrupt(&self) {
        let Some(connection) = self.connection.lock().ok().and_then(|conn| conn.clone()) else {
            return;
        };
        let Ok(message) = self.build("interrupt_request", Map::new()) else {
            return;
        };
        let _ = self.send_control(encode(&message, &connection.key));
    }

    pub(crate) fn resolve_active(
        &self,
        request_msg_id: Option<&str>,
        clear_active: bool,
        force_status: Option<ExecuteStatus>,
    ) {
        let mut resolved = None;
        if let Ok(mut active) = self.active.lock() {
            let matches = match (&*active, request_msg_id) {
                (Some(current), Some(id)) => current.cell.request_msg_id == id,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if !matches {
                return;
            }
            if let Some(current) = active.as_mut()
                && !current.settled
            {
                current.settled = true;
                let aborted = current.abort.as_ref().is_some_and(AbortFlag::is_fired)
                    || matches!(force_status, Some(ExecuteStatus::Aborted));
                if let Some(status) = force_status {
                    current.cell.status = status;
                }
                let duration =
                    u64::try_from(current.started.elapsed().as_millis()).unwrap_or(u64::MAX);
                let cell = std::mem::replace(
                    &mut current.cell,
                    CellState::new(String::new(), String::new(), 0, true),
                );
                let request_id = cell.request_msg_id.clone();
                if let Some(handler) = current.on_late.take() {
                    register_late_handler(&self.late_handlers, request_id, handler.into());
                }
                let result = crate::reduce::finish(cell, duration, aborted);
                if let Some(tx) = current.result_tx.take() {
                    resolved = Some((tx, Ok(result)));
                }
            }
            if clear_active {
                *active = None;
            }
        }
        if let Some((tx, result)) = resolved {
            let _ = tx.send(result);
        }
        if clear_active {
            self.idle_notify.notify_waiters();
        }
    }

    pub(crate) fn reject_active(&self, error: ExecuteError) {
        let mut sender = None;
        if let Ok(mut active) = self.active.lock()
            && let Some(mut current) = active.take()
        {
            sender = current.result_tx.take();
        }
        if let Some(tx) = sender {
            let _ = tx.send(Err(error));
        }
        self.idle_notify.notify_waiters();
    }

    pub(crate) fn clear_snapshot_timer(&self) {
        if let Ok(mut timer) = self.snapshot_timer.lock()
            && let Some(task) = timer.take()
        {
            task.abort();
        }
    }

    pub(crate) fn cleanup_resources(self: &Arc<Self>) {
        self.start_generation.fetch_add(1, Ordering::SeqCst);
        self.clear_snapshot_timer();
        if let Ok(mut handlers) = self.late_handlers.lock() {
            handlers.clear();
        }
        self.reject_active(ExecuteError::ShutDown);
        let channels = self.channels.lock().ok().and_then(|mut slot| slot.take());
        if let Some(channels) = channels {
            let _ = channels.kill_tx.send(());
            for task in channels.tasks {
                task.abort();
            }
        }
        if let Ok(mut pending) = self.pending_control.lock() {
            pending.clear();
        }
        if let Ok(mut connection) = self.connection.lock() {
            *connection = None;
        }
        let temp_dir = self.temp_dir.lock().ok().and_then(|mut slot| slot.take());
        if let Some(dir) = temp_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Incident: only shutdown and dispose removed the connection dir, so every manager dropped
/// without one left a `yi-kernel-*` dir in the temp dir; 1,393 had piled up by 2026-09-11.
impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(dir) = self.temp_dir.get_mut().ok().and_then(Option::take) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

pub(crate) fn trim_tail(text: &mut String) {
    if text.len() > STDERR_TAIL_CAP {
        let boundary = text
            .char_indices()
            .map(|(index, _)| index)
            .find(|index| *index >= text.len() - STDERR_TAIL_CAP)
            .unwrap_or(0);
        text.drain(..boundary);
    }
}

pub(crate) fn register_late_handler(
    handlers: &Mutex<LateHandlers>,
    request_id: String,
    handler: Arc<LateAgentMessageFn>,
) {
    if let Ok(mut handlers) = handlers.lock() {
        handlers.push_back((request_id, handler));
        while handlers.len() > MAX_LATE_SENT_AGENT_MESSAGE_HANDLERS {
            handlers.pop_front();
        }
    }
}

pub(crate) fn dispatch_late_agent_message(
    handlers: &Mutex<LateHandlers>,
    parent_id: Option<&str>,
    payload: Option<&Value>,
) -> bool {
    let (Some(parent_id), Some(payload)) = (parent_id, payload) else {
        return false;
    };
    let Some(message) = parse_sent_agent_message(payload) else {
        return false;
    };
    let handler = {
        let Ok(mut handlers) = handlers.lock() else {
            return false;
        };
        let Some(index) = handlers.iter().position(|(id, _)| id == parent_id) else {
            return false;
        };
        let Some((id, handler)) = handlers.remove(index) else {
            return false;
        };
        handlers.push_back((id, Arc::clone(&handler)));
        handler
    };
    handler(message);
    true
}

impl KernelManager {
    pub fn new(options: KernelOptions) -> Result<Self, String> {
        Ok(Self {
            inner: Arc::new(Inner {
                python: options.python,
                cwd: options.cwd,
                env: options.env,
                username: options.username,
                home: options.home,
                runtime_source_dir: options.runtime_source_dir,
                host: options.host,
                on_progress: options.on_progress,
                session: random_hex(16)?,
                state: Mutex::new(Lifecycle::Idle),
                start_generation: AtomicU64::new(0),
                start_lock: tokio::sync::Mutex::new(()),
                execution_queue: tokio::sync::Mutex::new(()),
                active: Mutex::new(None),
                idle_notify: Notify::new(),
                exited: Mutex::new(false),
                exit_notify: Notify::new(),
                channels: Mutex::new(None),
                connection: Mutex::new(None),
                temp_dir: Mutex::new(None),
                pending_control: Mutex::new(HashMap::new()),
                comm_targets: Mutex::new(HashMap::new()),
                handled_host_comm_ids: Mutex::new(HashSet::new()),
                late_handlers: Mutex::new(VecDeque::new()),
                last_cell_code: Mutex::new(None),
                kernel_stderr: Mutex::new(String::new()),
                in_flight_host: Mutex::new(Vec::new()),
                child_pid: Mutex::new(None),
                snapshot: options.snapshot,
                snapshot_timer: Mutex::new(None),
                checkpoints: crate::snapshot::Checkpoints::default(),
                wrap: options.wrap,
                held: Mutex::new(None),
            }),
        })
    }

    /// Invariant: dropped when the process is reaped, a crash included, or with the manager.
    pub fn hold_until_exit(&self, lock: Option<std::fs::File>) {
        if let Ok(mut held) = self.inner.held.lock() {
            *held = lock;
        }
    }

    pub fn is_running(&self) -> bool {
        self.inner.state() == Lifecycle::Running
    }

    pub fn wrap(&self) -> Option<&(String, Vec<String>)> {
        self.inner.wrap.as_ref()
    }

    pub fn snapshot_path(&self) -> Option<&std::path::Path> {
        self.inner
            .snapshot
            .as_ref()
            .map(|config| config.path.as_path())
    }

    pub async fn start(&self) -> Result<(), String> {
        let _guard = self.inner.start_lock.lock().await;
        match self.inner.state() {
            Lifecycle::Running => return Ok(()),
            Lifecycle::Shutdown => return Err("Kernel has been shut down".to_owned()),
            Lifecycle::Idle | Lifecycle::Starting => {}
        }
        // The boot gate wraps start only — never bootstrap-cell or restore
        // executes, which would pin a permit on a wedged kernel (design §9).
        let permit = boot_gate()
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "kernel boot gate closed".to_owned())?;
        let outcome = self.do_start().await;
        drop(permit);
        outcome
    }

    async fn do_start(&self) -> Result<(), String> {
        let _span = yi_types::trace::span("kernel.start");
        let inner = &self.inner;
        let generation = inner
            .start_generation
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        inner.set_state(Lifecycle::Starting);

        let python = match self.resolve_python(generation).await {
            Ok(python) => python,
            Err(error) => {
                if !inner.stale(generation) && inner.state() != Lifecycle::Shutdown {
                    inner.set_state(Lifecycle::Idle);
                }
                return Err(error);
            }
        };
        if inner.stale(generation) {
            return Err("Kernel start superseded".to_owned());
        }

        let connection = make_connection(&std::env::temp_dir())?;
        if let Ok(mut slot) = inner.temp_dir.lock() {
            *slot = Some(connection.temp_dir.clone());
        }
        let child = spawn_kernel_process(inner, &python, &connection.path)?;
        let child_tasks = crate::pump::spawn_child_tasks(inner, child);
        let mut tasks = vec![child_tasks.monitor, child_tasks.stderr];
        let kill_tx = child_tasks.kill_tx;

        let fail = |tasks: &mut Vec<tokio::task::JoinHandle<()>>, error: String| {
            let can_retry = inner.state() != Lifecycle::Shutdown;
            let _ = kill_tx.send(());
            for task in tasks.drain(..) {
                task.abort();
            }
            inner.cleanup_resources();
            inner.set_state(if can_retry {
                Lifecycle::Idle
            } else {
                Lifecycle::Shutdown
            });
            error
        };

        let info = match self.wait_for_resolved_connection(&connection.path).await {
            Ok(info) => info,
            Err(error) => {
                if inner.stale(generation) {
                    return Err(error);
                }
                return Err(fail(&mut tasks, error));
            }
        };
        if inner.stale(generation) {
            return Err("Kernel start superseded".to_owned());
        }
        if let Ok(mut slot) = inner.connection.lock() {
            *slot = Some(info.clone());
        }

        let (mut shell, mut iopub, control) = match crate::pump::connect_sockets(&info).await {
            Ok(sockets) => sockets,
            Err(error) => {
                if inner.stale(generation) {
                    return Err(error);
                }
                return Err(fail(&mut tasks, error));
            }
        };
        let (shell_tx, shell_rx) = mpsc::unbounded_channel::<Vec<Vec<u8>>>();
        let (control_tx, control_rx) = mpsc::unbounded_channel::<Vec<Vec<u8>>>();
        tasks.push(crate::pump::spawn_control_task(inner, control, control_rx));

        let closed = |error: &str| self.translate_socket_closure(error);
        let probed = crate::pump::probe_ready(inner, (&mut shell, &mut iopub), &info, &closed);
        if let Err(error) = probed.await {
            if inner.stale(generation) {
                return Err(error);
            }
            return Err(fail(&mut tasks, error));
        }
        if inner.stale(generation) {
            return Err("Kernel start superseded".to_owned());
        }
        tasks.push(crate::pump::spawn_iopub_task(inner, iopub));

        tasks.push(crate::pump::spawn_shell_task(shell, shell_rx));
        if let Ok(mut slot) = inner.channels.lock() {
            *slot = Some(Channels {
                shell_tx,
                control_tx,
                kill_tx,
                tasks,
            });
        }
        inner.set_state(Lifecycle::Running);
        Ok(())
    }

    async fn resolve_python(&self, _generation: u64) -> Result<PathBuf, String> {
        if let Some(python) = &self.inner.python {
            return Ok(python.clone());
        }
        let home = self.inner.home.clone();
        let runtime = self.inner.runtime_source_dir.clone();
        let progress = self.inner.on_progress.clone();
        tokio::task::spawn_blocking(move || {
            let options = BootstrapOptions {
                on_progress: progress.map(|callback| {
                    Box::new(move |message: &str| callback(message)) as Box<ProgressFn>
                }),
                home,
                runtime_source_dir: runtime,
                skills_source_dir: crate::bootstrap::default_skills_source_dir(),
                toolchain: None,
                venv_dir: None,
            };
            ensure_kernel_python(&options)
        })
        .await
        .map_err(|error| format!("bootstrap task failed: {error}"))?
    }

    async fn wait_for_resolved_connection(
        &self,
        path: &std::path::Path,
    ) -> Result<ConnectionInfo, String> {
        let _span = yi_types::trace::span("kernel.ports");
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(PORTS_RESOLVE_TIMEOUT_MS);
        let _span = yi_types::trace::span("kernel.wait_ports");
        let mut pace = yi_types::backoff::backoff(std::time::Duration::from_millis(25));
        while tokio::time::Instant::now() < deadline {
            if self.inner.state() == Lifecycle::Shutdown
                || self
                    .inner
                    .exited
                    .lock()
                    .map(|exited| *exited)
                    .unwrap_or(false)
            {
                return Err(format!(
                    "Kernel exited before resolving ports. stderr:\n{}",
                    self.inner.stderr_tail()
                ));
            }
            if let Some(info) = read_connection_info(path)
                && has_resolved_ports(&info)
            {
                return Ok(info);
            }
            tokio::time::sleep(pace()).await;
        }
        Err(format!(
            "Kernel did not resolve connection ports within {PORTS_RESOLVE_TIMEOUT_MS}ms. stderr tail:\n{}",
            self.inner.stderr_tail()
        ))
    }

    fn translate_socket_closure(&self, message: &str) -> String {
        let starting = self.inner.state() == Lifecycle::Starting;
        format!(
            "IPython kernel channel closed while {} (retriable): {message}. stderr tail:\n{}",
            if starting {
                "starting up"
            } else {
                "communicating"
            },
            self.inner.stderr_tail()
        )
    }

    pub async fn execute(
        &self,
        code: &str,
        options: ExecuteOptions,
    ) -> Result<ExecuteResult, ExecuteError> {
        if options.abort.as_ref().is_some_and(AbortFlag::is_fired) {
            return Ok(aborted_result());
        }
        self.start().await.map_err(ExecuteError::Failed)?;
        if self.inner.state() == Lifecycle::Shutdown {
            return Err(ExecuteError::ShutDown);
        }
        // Incident: a 5 s read waited out whole cells here; nothing is sent before this lock.
        let _queue = match options.abort.as_ref() {
            Some(abort) => tokio::select! {
                queue = self.inner.execution_queue.lock() => queue,
                () = abort.fired() => return Ok(aborted_result()),
            },
            None => self.inner.execution_queue.lock().await,
        };
        self.wait_for_active_to_clear_for_reuse(options.abort.as_ref())
            .await?;
        if options.abort.as_ref().is_some_and(AbortFlag::is_fired) {
            return Ok(aborted_result());
        }
        if self.inner.state() == Lifecycle::Shutdown {
            return Err(ExecuteError::ShutDown);
        }
        let internal = options.internal;
        let span = yi_types::trace::span("kernel.execute").arg("internal", internal);
        let result = self.execute_inner(code, options).await;
        drop(span);
        if !internal {
            self.after_user_cell(&result);
        }
        result
    }

    async fn wait_for_active_to_clear_for_reuse(
        &self,
        abort: Option<&AbortFlag>,
    ) -> Result<(), ExecuteError> {
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(KERNEL_BUSY_REUSE_WAIT_MS);
        loop {
            let busy = self
                .inner
                .active
                .lock()
                .map(|active| active.is_some())
                .unwrap_or(false);
            if !busy {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ExecuteError::BusyAfterInterrupt);
            }
            if self.inner.state() == Lifecycle::Shutdown {
                return Err(ExecuteError::ShutDown);
            }
            self.inner.interrupt();
            let wait = std::time::Duration::from_millis(KERNEL_BUSY_INTERRUPT_INTERVAL_MS);
            match abort {
                Some(abort) => {
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = self.inner.idle_notify.notified() => {}
                        _ = abort.fired() => return Ok(()),
                    }
                }
                None => {
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = self.inner.idle_notify.notified() => {}
                    }
                }
            }
        }
    }

    async fn execute_inner(
        &self,
        code: &str,
        options: ExecuteOptions,
    ) -> Result<ExecuteResult, ExecuteError> {
        let inner = &self.inner;
        let connection = inner
            .connection
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .ok_or(ExecuteError::ShutDown)?;
        let max_chars = options.max_output_chars.unwrap_or(DEFAULT_MAX_OUTPUT_CHARS);
        let content = json!({
            "code": code,
            "silent": false,
            "store_history": true,
            "user_expressions": {},
            "allow_stdin": false,
            // The kernel aborts every request queued behind a failed one; Yi's own cell, left
            // running under an abort, must not take the user's next cell with it.
            "stop_on_error": !options.internal,
        });
        let content = content.as_object().cloned().unwrap_or_default();
        let message = inner
            .build("execute_request", content)
            .map_err(ExecuteError::Failed)?;
        let request_id = message.header.msg_id.clone();

        let (result_tx, result_rx) = oneshot::channel();
        {
            let mut active = inner.active.lock().map_err(|_| ExecuteError::ShutDown)?;
            if active.is_some() {
                return Err(ExecuteError::Failed(
                    "Kernel already has an active execution".to_owned(),
                ));
            }
            *active = Some(Active {
                cell: CellState::new(
                    request_id.clone(),
                    code.to_owned(),
                    max_chars,
                    options.internal,
                ),
                started: std::time::Instant::now(),
                on_stream: options.on_stream,
                on_late: options.on_late_sent_agent_message,
                abort: options.abort.clone(),
                settled: false,
                result_tx: Some(result_tx),
            });
        }
        if !options.internal
            && let Ok(mut last) = inner.last_cell_code.lock()
        {
            *last = Some(code.to_owned());
        }

        let internal = options.internal;
        let abort_task = options.abort.clone().map(|abort| {
            let inner = Arc::clone(inner);
            let request_id = request_id.clone();
            tokio::spawn(async move {
                abort.fired().await;
                inner.interrupt();
                // 1 s grace, then force-Aborted; a user cell keeps the slot, since it may still
                // run and busy-reuse owns recovery (§9), while Yi's own cell gives it up.
                tokio::time::sleep(std::time::Duration::from_millis(KERNEL_ABORT_GRACE_MS)).await;
                inner.resolve_active(Some(&request_id), internal, Some(ExecuteStatus::Aborted));
            })
        });

        let send_result = inner.send_shell(encode(&message, &connection.key));
        if let Err(error) = send_result {
            if let Ok(mut active) = inner.active.lock()
                && active
                    .as_ref()
                    .is_some_and(|current| current.cell.request_msg_id == request_id)
            {
                *active = None;
            }
            if let Some(task) = abort_task {
                task.abort();
            }
            return Err(ExecuteError::Failed(error));
        }

        let outcome = result_rx
            .await
            .map_err(|_| ExecuteError::ShutDown)
            .and_then(|result| result);
        if let Some(task) = abort_task {
            task.abort();
        }
        outcome
    }

    pub fn interrupt(&self) {
        self.inner.interrupt();
    }

    /// Resolves true when this call performed the cleanup (false: a concurrent
    /// teardown won).
    pub async fn shutdown(&self) -> bool {
        let inner = &self.inner;
        if inner.state() == Lifecycle::Shutdown {
            inner.cleanup_resources();
            return true;
        }
        let generation = inner.start_generation.load(Ordering::SeqCst);
        inner.set_state(Lifecycle::Shutdown);

        let graceful = async {
            let connection = inner.connection.lock().ok().and_then(|slot| slot.clone())?;
            let content = json!({"restart": false})
                .as_object()
                .cloned()
                .unwrap_or_default();
            let message = inner.build("shutdown_request", content).ok()?;
            let request_id = message.header.msg_id.clone();
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Ok(mut pending) = inner.pending_control.lock() {
                pending.insert(request_id.clone(), ("shutdown_reply".to_owned(), reply_tx));
            }
            if inner
                .send_control(encode(&message, &connection.key))
                .is_err()
            {
                if let Ok(mut pending) = inner.pending_control.lock() {
                    pending.remove(&request_id);
                }
                return None;
            }
            Some((request_id, reply_rx))
        }
        .await;

        if let Some((request_id, reply_rx)) = graceful {
            let deadline = std::time::Duration::from_millis(KERNEL_SHUTDOWN_TIMEOUT_MS);
            let exit_wait = async {
                loop {
                    if inner.exited.lock().map(|exited| *exited).unwrap_or(true) {
                        return;
                    }
                    inner.exit_notify.notified().await;
                }
            };
            let _ = tokio::time::timeout(deadline, async {
                tokio::select! {
                    _ = reply_rx => {}
                    _ = exit_wait => {}
                }
            })
            .await;
            if let Ok(mut pending) = inner.pending_control.lock() {
                pending.remove(&request_id);
            }
        }

        // A superseded shutdown must not tear down a newer start's sockets.
        if inner.stale(generation) {
            return false;
        }
        inner.cleanup_resources();
        true
    }

    pub async fn dispose(&self) {
        let inner = &self.inner;
        // Captured before any await: teardowns and newer starts bump the counter.
        let generation = inner.start_generation.load(Ordering::SeqCst);
        // Final namespace flush while the kernel is live, bounded so a wedged kernel cannot
        // hang dispose; the debounced on-disk copy is the fallback past that bound.
        if inner.snapshot.is_some() && self.is_running() && inner.checkpoints.behind() {
            inner.clear_snapshot_timer();
            let deadline = std::time::Duration::from_millis(SNAPSHOT_DISPOSE_TIMEOUT_MS);
            let _ = tokio::time::timeout(deadline, self.snapshot_state()).await;
        }
        if inner.stale(generation) {
            // Superseded mid-flush: the newer owner already cleaned this kernel.
            return;
        }
        inner.set_state(Lifecycle::Shutdown);
        let in_flight: Vec<_> = inner
            .in_flight_host
            .lock()
            .map(|mut tasks| tasks.drain(..).map(|(_, task)| task).collect())
            .unwrap_or_default();
        if !in_flight.is_empty() {
            let deadline = std::time::Duration::from_millis(HOST_REQUEST_DISPOSE_TIMEOUT_MS);
            let _ = tokio::time::timeout(deadline, async {
                for task in in_flight {
                    let _ = task.await;
                }
            })
            .await;
        }
        if !inner.stale(generation) {
            inner.cleanup_resources();
        }
    }

    pub fn last_cell_code(&self) -> Option<String> {
        self.inner
            .last_cell_code
            .lock()
            .ok()
            .and_then(|last| last.clone())
    }
}

fn aborted_result() -> ExecuteResult {
    ExecuteResult {
        stdout: String::new(),
        stderr: String::new(),
        result: None,
        diffs: Vec::new(),
        attachments: Vec::new(),
        sent_agent_messages: Vec::new(),
        status: ExecuteStatus::Aborted,
        error: None,
        duration_ms: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{KernelManager, KernelOptions};
    use crate::scratch::Scratch;

    #[test]
    fn a_manager_dropped_without_shutdown_removes_its_connection_dir()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = Scratch::new("yi-kernel-drop")?;
        let connection = crate::connection::make_connection(&root)?;
        let manager = KernelManager::new(KernelOptions {
            python: None,
            cwd: None,
            env: Vec::new(),
            username: "yi".to_owned(),
            home: root.to_path_buf(),
            runtime_source_dir: root.to_path_buf(),
            host: None,
            on_progress: None,
            snapshot: None,
            wrap: None,
        })?;
        if let Ok(mut slot) = manager.inner.temp_dir.lock() {
            *slot = Some(connection.temp_dir.clone());
        }
        drop(manager);
        assert!(
            !connection.temp_dir.exists(),
            "connection dir survived the drop"
        );
        Ok(())
    }
}
