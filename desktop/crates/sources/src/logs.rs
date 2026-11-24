//! Log lines gathered on this Mac: the server's worker log and the app's own log, each tailed,
//! cleaned, bounded and parsed into a row. Lines the model already holds are the pane's.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use serde_json::{Map, Value as Json};
use yd_core::feed::{FeedEvent, SourceCmd};
use yd_core::ids::{FeedId, Millis};
use yd_core::inbox::{Closed, FeedTx};
use yd_core::row::{Row, Tone, Value};
use yd_core::wire::client::Clock;

pub const FEED: FeedId = FeedId::new("logs");

const POLL: Duration = Duration::from_millis(250);
/// Bytes read from the end of a file the first time it is seen; older lines are not read.
const BACKLOG: u64 = 64 * 1024;
/// Bytes read from one file in one look; the rest waits for the next.
const CHUNK: u64 = 256 * 1024;
/// Bytes of an unfinished line held back; past it the line is taken as it stands.
const HELD: usize = 64 * 1024;
/// Characters of a line kept: another program's output has no bound of its own.
const LINE_CHARS: usize = 2_000;
const FIELDS: usize = 8;
const FIELD_CHARS: usize = 120;
/// Cells a line owns, which a parsed field may not take the name of.
const OWN: [&str; 6] = ["ts", "time", "level", "source", "message", "json"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn word(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }

    fn tone(self) -> Tone {
        match self {
            Level::Debug => Tone::Faint,
            Level::Info => Tone::Dim,
            Level::Warn => Tone::Warn,
            Level::Error => Tone::Bad,
        }
    }

    fn named(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "debug" | "trace" => Some(Level::Debug),
            "info" | "notice" => Some(Level::Info),
            "warn" | "warning" => Some(Level::Warn),
            "error" | "err" | "fatal" | "critical" => Some(Level::Error),
            _ => None,
        }
    }
}

/// Where the logs are: the worker's stderr beside the daemon's socket, and the app's own file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    pub worker: PathBuf,
    pub app: PathBuf,
}

/// The socket from `--socket <path>`, else `~/.yi/daemon.sock`; Yi names the worker's log by
/// swapping the socket's extension, so `daemon.sock` gives `daemon.worker.log`.
pub fn places(args: &[String], home: &Path) -> Places {
    let flagged = args
        .iter()
        .position(|arg| arg == "--socket")
        .and_then(|at| args.get(at.saturating_add(1)));
    let socket = flagged.map_or_else(|| home.join(".yi/daemon.sock"), PathBuf::from);
    Places {
        worker: socket.with_extension("worker.log"),
        app: home.join("Library/Logs/Yi Desktop/yi-desktop.log"),
    }
}

/// Unix milliseconds as the time of day in UTC, `HH:MM:SS.mmm`.
pub fn clock_text(at: Millis) -> String {
    let day = at.0 % 86_400_000;
    let (h, m, s, ms) = (
        day / 3_600_000,
        day / 60_000 % 60,
        day / 1000 % 60,
        day % 1000,
    );
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

/// Marks that read as text direction or hidden ink, which another program's line may carry.
fn hidden(c: char) -> bool {
    matches!(c, '\u{61c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// `text` with controls and direction marks gone (a tab becomes a space), cut to `limit`
/// characters; the second value is how many characters the cut took.
fn clean(text: &str, limit: usize) -> (String, usize) {
    let mut kept = text
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control() && !hidden(*c));
    let head: String = kept.by_ref().take(limit).collect();
    (head, kept.count())
}

fn bounded(text: &str, limit: usize) -> String {
    match clean(text, limit) {
        (head, 0) => head,
        (head, more) => format!("{head}… [{more} more characters]"),
    }
}

struct Parsed {
    level: Level,
    message: String,
    fields: Vec<(String, String)>,
    at: Option<Millis>,
}

fn scalar(value: &Json) -> Option<String> {
    match value {
        Json::String(text) => Some(text.clone()),
        Json::Number(number) => Some(number.to_string()),
        Json::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn field_name(key: &str) -> Option<String> {
    let name: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(32)
        .collect();
    let named = name.chars().next().is_some_and(|c| c.is_ascii_lowercase());
    (named && !OWN.contains(&name.as_str())).then_some(name)
}

/// A JSON record: its level and message, its time if it is a Unix millisecond count, and
/// every other scalar as a field.
fn record(map: &Map<String, Json>, line: &str) -> Parsed {
    let word = |names: &[&str]| names.iter().find_map(|name| map.get(*name)?.as_str());
    let level = word(&["level", "lvl", "severity"]).and_then(Level::named);
    let message = word(&["msg", "message"]).unwrap_or(line);
    let at = ["ts", "time"]
        .iter()
        .find_map(|name| map.get(*name)?.as_u64())
        .filter(|ms| *ms > 1_000_000_000_000)
        .map(Millis);
    let skip = ["level", "lvl", "severity", "msg", "message", "ts", "time"];
    let fields = map
        .iter()
        .filter(|(key, _)| !skip.contains(&key.as_str()))
        .filter_map(|(key, value)| Some((field_name(key)?, scalar(value)?)))
        .take(FIELDS)
        .collect();
    Parsed {
        level: level.unwrap_or(Level::Info),
        message: bounded(message, LINE_CHARS),
        fields,
        at,
    }
}

/// A plain line: a `warning:` style prefix names its level; `key=value` words are fields.
fn plain(line: &str) -> Parsed {
    let (prefix, rest) = line.split_once(": ").unwrap_or(("", line));
    let named = Level::named(prefix).filter(|_| !prefix.contains(' '));
    let (level, message) = match named {
        Some(level) => (level, rest),
        None => (Level::Info, line),
    };
    let pair = |word: &str| {
        let (key, value) = word.split_once('=')?;
        (!value.is_empty()).then(|| Some((field_name(key)?, bounded(value, FIELD_CHARS))))?
    };
    Parsed {
        level,
        message: bounded(message, LINE_CHARS),
        fields: message
            .split_whitespace()
            .filter_map(pair)
            .take(FIELDS)
            .collect(),
        at: None,
    }
}

/// One line as a row, tagged with its source; none for a line with nothing in it.
pub fn line_row(source: &str, raw: &str, read: Millis, seq: u64) -> Option<Row> {
    let (shown, _) = clean(raw, usize::MAX);
    if shown.trim().is_empty() {
        return None;
    }
    let json = serde_json::from_str::<Map<String, Json>>(shown.trim()).ok();
    let parsed = json
        .as_ref()
        .map_or_else(|| plain(&shown), |map| record(map, &shown));
    let at = parsed.at.unwrap_or(read);
    let kept = bounded(raw, LINE_CHARS);
    let row = Row::new(format!("{}-{seq}", at.0))
        .text("ts", at.0.to_string())
        .text("time", clock_text(at))
        .cell(
            "level",
            Value::Badge(parsed.level.word().to_owned(), parsed.level.tone()),
        )
        .text("source", source);
    // The raw record is the message itself unless parsing took something off it.
    let row = if kept == parsed.message {
        row
    } else {
        row.text("json", kept)
    };
    let row = row.text("message", parsed.message);
    Some(
        parsed
            .fields
            .into_iter()
            .fold(row, |row, (key, value)| row.text(&key, value)),
    )
}

/// One file followed from where it was last read.
struct Tail {
    path: PathBuf,
    source: &'static str,
    /// Where the next look starts; none until the file has been seen.
    at: Option<u64>,
    held: Vec<u8>,
}

impl Tail {
    fn new(path: PathBuf, source: &'static str) -> Self {
        Self {
            path,
            source,
            at: None,
            held: Vec::new(),
        }
    }

    /// The rows of what was appended since the last look. A file seen for the first time is
    /// read from its last [`BACKLOG`] bytes, and a row says so when that left lines out.
    fn look(&mut self, clock: &Clock, seq: &mut u64) -> Vec<Row> {
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        let Ok(len) = file.metadata().map(|meta| meta.len()) else {
            return Vec::new();
        };
        let first = self.at.is_none();
        let from = match self.at {
            Some(at) if at <= len => at,
            Some(_) => 0,
            None => len.saturating_sub(BACKLOG),
        };
        let mut data = Vec::new();
        let read = file
            .seek(SeekFrom::Start(from))
            .and_then(|_| file.take(CHUNK).read_to_end(&mut data));
        let Ok(read) = read else {
            return Vec::new();
        };
        if matches!(self.at, Some(at) if at > len) {
            self.held.clear();
        }
        self.at = Some(from.saturating_add(u64::try_from(read).unwrap_or(0)));
        self.held.extend_from_slice(&data);
        let at = clock();
        let mut rows = Vec::new();
        if first && from > 0 {
            rows.push(self.notice(len, at, seq));
            let cut = self.held.iter().position(|b| *b == b'\n');
            self.held
                .drain(..cut.map_or(self.held.len(), |cut| cut.saturating_add(1)));
        }
        let lines = self.lines();
        rows.extend(lines.iter().filter_map(|line| {
            *seq = seq.saturating_add(1);
            line_row(self.source, line, at, *seq)
        }));
        rows
    }

    /// The finished lines held, and the unfinished one kept for the next look.
    fn lines(&mut self) -> Vec<String> {
        let end = self.held.iter().rposition(|b| *b == b'\n');
        let whole = end.map_or(0, |end| end.saturating_add(1));
        let mut taken: Vec<u8> = self.held.drain(..whole).collect();
        if self.held.len() > HELD {
            taken.append(&mut self.held);
            taken.push(b'\n');
        }
        String::from_utf8_lossy(&taken)
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn notice(&self, len: u64, at: Millis, seq: &mut u64) -> Row {
        *seq = seq.saturating_add(1);
        let name = self
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let said = format!(
            "[…] older lines of {name} not read: the last {BACKLOG} of {len} bytes are shown (BACKLOG); the file holds the rest"
        );
        Row::new(format!("{}-{seq}", at.0))
            .text("ts", at.0.to_string())
            .text("time", clock_text(at))
            .cell(
                "level",
                Value::Badge(Level::Info.word().to_owned(), Level::Info.tone()),
            )
            .text("source", self.source)
            .text("message", bounded(&said, LINE_CHARS))
    }
}

/// Follows the worker log and the app's log until the app drops the ask channel.
fn serve(
    out: &FeedTx,
    cmds: &Receiver<SourceCmd>,
    places: Places,
    clock: &Clock,
    poll: Duration,
) -> Result<(), Closed> {
    let mut tails = [
        Tail::new(places.worker, "yi"),
        Tail::new(places.app, "desktop"),
    ];
    let mut seq = 0_u64;
    let mut first = true;
    loop {
        let rows: Vec<Row> = tails
            .iter_mut()
            .flat_map(|tail| tail.look(clock, &mut seq))
            .collect();
        if first {
            first = false;
            out.send(FeedEvent::Reset(rows))?;
        } else {
            rows.into_iter()
                .try_for_each(|row| out.send(FeedEvent::Upsert(row)))?;
        }
        if let Err(RecvTimeoutError::Disconnected) = cmds.recv_timeout(poll) {
            return Ok(());
        }
    }
}

pub fn run(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    serve(
        &out,
        &cmds,
        places(&args, &home),
        &crate::server::clock(),
        POLL,
    )
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::mpsc::channel;
    use std::time::Instant;

    use yd_core::feed::{FeedStatus, Table};
    use yd_core::inbox::inbox;
    use yd_core::model::Model;
    use yd_core::row::Rows;

    use super::*;

    type Check = Result<(), Box<dyn Error>>;

    const AT: Millis = Millis(1_728_000_000_000 + 17 * 3_600_000 + 58 * 60_000 + 1_101);

    fn folder(name: &str) -> Result<PathBuf, Box<dyn Error>> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/logs-tests")
            .join(format!("{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn clock() -> Clock {
        Arc::new(|| AT)
    }

    fn words<'a>(row: &'a Row, field: &str) -> &'a str {
        row.words(field).unwrap_or("")
    }

    #[test]
    fn a_worker_line_gets_the_level_its_prefix_names_and_the_time_it_was_read() -> Check {
        let line = "warning: models.classifier `english` is retired";
        let row = line_row("yi", line, AT, 1).ok_or("no row")?;
        assert_eq!(words(&row, "level"), "WARN");
        assert_eq!(
            row.get("level"),
            Some(&Value::Badge("WARN".into(), Tone::Warn))
        );
        assert_eq!(
            words(&row, "message"),
            "models.classifier `english` is retired"
        );
        assert_eq!(
            words(&row, "json"),
            line,
            "the raw record is the line as it was"
        );
        assert_eq!(
            (words(&row, "time"), words(&row, "source")),
            ("17:58:01.101", "yi")
        );
        Ok(())
    }

    #[test]
    fn a_json_record_keeps_its_level_message_and_other_fields() -> Check {
        let line = r#"{"level":"error","msg":"connection reset","peer":"198.51.100.14","pool":3,"nested":{"a":1}}"#;
        let row = line_row("yi", line, AT, 2).ok_or("no row")?;
        assert_eq!(words(&row, "level"), "ERROR");
        assert_eq!(words(&row, "message"), "connection reset");
        assert_eq!(
            (words(&row, "peer"), words(&row, "pool")),
            ("198.51.100.14", "3")
        );
        assert!(
            row.get("nested").is_none(),
            "an object is not a scalar field"
        );
        Ok(())
    }

    #[test]
    fn key_value_words_of_a_plain_line_become_fields() -> Check {
        let row =
            line_row("yi", "POST /upload 429 retry-after=41 user=u_193", AT, 3).ok_or("no row")?;
        assert_eq!(
            (words(&row, "retry_after"), words(&row, "user")),
            ("41", "u_193")
        );
        let own = line_row("yi", "level=debug json=x message=y", AT, 4).ok_or("no row")?;
        assert_eq!(
            words(&own, "level"),
            "INFO",
            "a field may not take a cell of the line"
        );
        Ok(())
    }

    /// Another program's text: escapes, direction marks, and a line past the cap whose cut
    /// falls inside a run of two-byte characters.
    #[test]
    fn hostile_text_is_cleaned_and_a_cut_says_how_much_it_took() -> Check {
        let line = "ok\u{1b}[31m \u{202e}evil\u{7}\ttab";
        let row = line_row("yi", line, AT, 5).ok_or("no row")?;
        assert_eq!(words(&row, "message"), "ok[31m evil tab");
        let long = "é".repeat(LINE_CHARS + 41);
        let row = line_row("yi", &long, AT, 6).ok_or("no row")?;
        let message = words(&row, "message");
        assert!(message.ends_with("… [41 more characters]"), "{message}");
        assert_eq!(message.chars().filter(|c| *c == 'é').count(), LINE_CHARS);
        assert!(
            line_row("yi", " \t ", AT, 7).is_none(),
            "a blank line is no line"
        );
        Ok(())
    }

    #[test]
    fn a_tail_reads_what_was_appended_holds_half_a_line_and_follows_a_truncation() -> Check {
        let path = folder("tail")?.join("daemon.worker.log");
        std::fs::write(&path, "first\nsecond\nthi")?;
        let mut tail = Tail::new(path.clone(), "yi");
        let mut seq = 0;
        let said = |rows: &[Row]| {
            rows.iter()
                .map(|r| words(r, "message").to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(said(&tail.look(&clock(), &mut seq)), ["first", "second"]);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"rd\nfourth\n")?;
        assert_eq!(said(&tail.look(&clock(), &mut seq)), ["third", "fourth"]);
        assert!(tail.look(&clock(), &mut seq).is_empty());
        std::fs::write(&path, "again\n")?;
        assert_eq!(
            said(&tail.look(&clock(), &mut seq)),
            ["again"],
            "a rotated file is read from its start"
        );
        Ok(())
    }

    #[test]
    fn a_file_past_the_backlog_is_cut_and_a_row_says_what_was_left_out() -> Check {
        let path = folder("backlog")?.join("daemon.worker.log");
        let body: String = (0..9_000)
            .map(|n| format!("line number {n:05}\n"))
            .collect();
        std::fs::write(&path, &body)?;
        let rows = Tail::new(path, "yi").look(&clock(), &mut 0);
        let first = rows.first().ok_or("no rows")?;
        let notice = words(first, "message");
        let total = body.len();
        assert!(
            notice.contains(&format!("last {BACKLOG} of {total} bytes")),
            "{notice}"
        );
        assert!(
            notice.contains("BACKLOG"),
            "the notice names its cap: {notice}"
        );
        let last = rows.last().ok_or("no rows")?;
        assert_eq!(words(last, "message"), "line number 08999");
        let cut = rows.get(1).ok_or("no second row")?;
        assert!(
            words(cut, "message").starts_with("line number "),
            "the line the cut split is dropped"
        );
        assert!(rows.len() < 9_000);
        Ok(())
    }

    #[test]
    fn the_worker_log_sits_beside_the_socket_the_app_was_given() {
        let home = Path::new("/Users/me");
        let flag = ["yi-desktop", "--socket", "/run/x/s.sock"].map(str::to_owned);
        assert_eq!(places(&flag, home).worker, Path::new("/run/x/s.worker.log"));
        assert_eq!(
            places(&[], home).worker,
            Path::new("/Users/me/.yi/daemon.worker.log")
        );
    }

    #[test]
    fn the_time_of_day_is_utc_with_milliseconds() {
        assert_eq!(clock_text(AT), "17:58:01.101");
        assert_eq!(clock_text(Millis(0)), "00:00:00.000");
    }

    #[test]
    fn the_source_sends_the_backlog_then_each_new_line_and_stops_with_its_channel() -> Check {
        let dir = folder("serve")?;
        let worker = dir.join("daemon.worker.log");
        std::fs::write(&worker, "worker: one\n")?;
        let (sender, inbox) = inbox(|| {});
        let (asks, cmds) = channel();
        let places = Places {
            worker: worker.clone(),
            app: dir.join("none.log"),
        };
        let out = sender.feed(FEED);
        let run = std::thread::spawn(move || {
            serve(&out, &cmds, places, &clock(), Duration::from_millis(10))
        });
        let mut model = Model::default();
        let table = |model: &Model| model.tables.get(FEED).map(Table::len).unwrap_or(0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while table(&model) < 1 && Instant::now() < deadline {
            inbox.drain(&mut model);
        }
        std::fs::OpenOptions::new()
            .append(true)
            .open(&worker)?
            .write_all(b"worker: two\n")?;
        while table(&model) < 2 && Instant::now() < deadline {
            inbox.drain(&mut model);
        }
        drop(asks);
        run.join().map_err(|_| "the source panicked")??;
        let held = model.tables.get(FEED).ok_or("no table")?;
        assert_eq!(held.status(), &FeedStatus::Live);
        assert_eq!(held.len(), 2);
        Ok(())
    }
}
