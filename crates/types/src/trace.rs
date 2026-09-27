//! `YI_TRACE=<dir>`: every yi process appends Chrome trace events to `<dir>/<pid>.jsonl`, on
//! wall microseconds anchored once and advanced monotonically, so processes share a timeline.

use std::borrow::Cow;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde_json::{Map, Value, json};

struct Sink {
    file: Mutex<std::fs::File>,
    anchor: Instant,
    epoch_us: u64,
    pid: u32,
}

static SINK: OnceLock<Option<Sink>> = OnceLock::new();
static NEXT_TID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static TID: u64 = NEXT_TID.fetch_add(1, Ordering::Relaxed);
}

fn sink() -> Option<&'static Sink> {
    SINK.get_or_init(|| {
        let dir = std::env::var_os("YI_TRACE").filter(|dir| !dir.is_empty())?;
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).ok()?;
        let pid = std::process::id();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{pid}.jsonl")))
            .ok()?;
        #[allow(clippy::disallowed_methods)] // the wall anchor is the job: it aligns processes
        let epoch_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        Some(Sink {
            file: Mutex::new(file),
            anchor: Instant::now(),
            epoch_us,
            pid,
        })
    })
    .as_ref()
}

pub fn enabled() -> bool {
    sink().is_some()
}

/// Microseconds on the shared timeline; 0 when tracing is off.
pub fn now_us() -> u64 {
    sink().map_or(0, |sink| {
        let since = u64::try_from(sink.anchor.elapsed().as_micros()).unwrap_or(u64::MAX);
        sink.epoch_us.saturating_add(since)
    })
}

fn tid() -> u64 {
    TID.with(|tid| *tid)
}

fn emit(sink: &Sink, mut event: Value) {
    if let Some(map) = event.as_object_mut() {
        map.insert("pid".to_owned(), json!(sink.pid));
        map.entry("tid").or_insert_with(|| json!(tid()));
    }
    let mut line = event.to_string();
    line.push('\n');
    if let Ok(mut file) = sink.file.lock() {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Names this process on the timeline (`console`, `daemon`, `worker <root>`) and records
/// the moment it reached `main`.
pub fn init(process: &str) {
    let Some(sink) = sink() else { return };
    emit(
        sink,
        json!({"ph": "M", "name": "process_name", "args": {"name": format!("{process} [{}]", sink.pid)}}),
    );
    name_thread("main");
    instant("process.main", Map::new());
}

pub fn name_thread(name: &str) {
    let Some(sink) = sink() else { return };
    emit(
        sink,
        json!({"ph": "M", "name": "thread_name", "args": {"name": name}}),
    );
}

pub fn instant(name: &str, args: Map<String, Value>) {
    let Some(sink) = sink() else { return };
    emit(
        sink,
        json!({"ph": "i", "s": "t", "name": name, "ts": now_us(), "args": args}),
    );
}

/// A span measured by the caller, for work that starts in one event and ends in another
/// (a request sent now, answered frames later).
pub fn complete(name: &str, start_us: u64, args: Map<String, Value>) {
    let Some(sink) = sink() else { return };
    let end = now_us();
    emit(
        sink,
        json!({"ph": "X", "name": name, "ts": start_us, "dur": end.saturating_sub(start_us), "args": args}),
    );
}

/// A scoped span: recorded when dropped. Inert and allocation-free when tracing is off.
#[must_use]
pub struct Span {
    name: Cow<'static, str>,
    start: u64,
    args: Option<Map<String, Value>>,
}

pub fn span(name: impl Into<Cow<'static, str>>) -> Span {
    let on = enabled();
    Span {
        name: if on { name.into() } else { Cow::Borrowed("") },
        start: now_us(),
        args: on.then(Map::new),
    }
}

impl Span {
    pub fn arg(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.set(key, value);
        self
    }

    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        if let Some(args) = &mut self.args {
            args.insert(key.to_owned(), value.into());
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(args) = self.args.take() {
            complete(&self.name, self.start, args);
        }
    }
}
