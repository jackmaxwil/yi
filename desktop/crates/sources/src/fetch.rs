//! Web addresses a plugin pane was granted, read with the system `curl`.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant};

use yd_core::command::tools::fetch::GET;
use yd_core::feed::{FeedEvent, FeedStatus, SourceCmd};
use yd_core::inbox::{Closed, FeedTx};
use yd_core::row::Row;

pub use yd_core::feed::FETCH as FEED;

/// Where `curl` may be; the first that exists is used, never a shell lookup.
const CURL: &[&str] = &["/usr/bin/curl", "/opt/homebrew/bin/curl"];
/// The longest one answer may take, curl's own limit; the wait for curl adds [`GRACE`].
const LIMIT: Duration = Duration::from_secs(5);
const GRACE: Duration = Duration::from_secs(1);
/// The most of a body a row keeps.
pub const MAX_BODY: usize = 256 << 10;

/// What one run of curl wrote: the body, and on stderr its errors then the status code.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ran {
    pub body: Vec<u8>,
    /// Bytes the server sent in all, the kept ones and those read past [`MAX_BODY`].
    pub sent: usize,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("only http and https addresses are fetched")]
    Scheme,
    #[error("no curl in {CURL:?}")]
    NoCurl,
    #[error("curl could not be started: {0}")]
    Start(String),
    #[error("curl ran past {LIMIT:?} and was stopped")]
    Timeout,
}

/// Answers `fetch.get` with the system curl until `cmds` closes.
pub fn run(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    serve(out, cmds, curl)
}

/// Answers each `fetch.get` with the row `get:<url>`, one at a time, through `runner`.
// ponytail: one request at a time; a pool when a pane asks many hosts at once.
pub fn serve(
    out: FeedTx,
    cmds: Receiver<SourceCmd>,
    runner: impl Fn(&[&str]) -> Result<Ran, FetchError>,
) -> Result<(), Closed> {
    out.send(FeedEvent::Status(FeedStatus::Live))?;
    for cmd in cmds {
        let SourceCmd::Run(run) = cmd else { continue };
        let Some(url) = run.text.filter(|_| run.command == GET) else {
            continue;
        };
        out.send(FeedEvent::Upsert(answer(&url, &runner)))?;
    }
    Ok(())
}

/// The row for `url`: its status and body, or why there is none.
pub fn answer(url: &str, runner: &impl Fn(&[&str]) -> Result<Ran, FetchError>) -> Row {
    let row = Row::new(["get:", url].concat()).text("url", url);
    let ran = web(url).and_then(|()| runner(&args(url)));
    let ran = match ran {
        Ok(ran) => ran,
        Err(why) => return row.text("status", "").text("error", why.to_string()),
    };
    let (said, code) = ran
        .stderr
        .trim_end()
        .rsplit_once('\n')
        .map_or(("", ran.stderr.trim_end()), |(said, code)| {
            (said.trim(), code.trim())
        });
    let (kept, cut) = cut(&ran.body);
    let mut error = said.to_owned();
    if cut {
        let note = format!(
            "kept {} of {} bytes",
            kept.len(),
            ran.sent.max(ran.body.len())
        );
        error = if error.is_empty() {
            note
        } else {
            [error, note].join("; ")
        };
    }
    let code = if code == "000" { "" } else { code };
    row.text("status", code)
        .text("body", kept)
        .text("error", error)
}

/// Refuses anything but `http://` and `https://`.
fn web(url: &str) -> Result<(), FetchError> {
    let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    match scheme.as_deref() {
        Some("http" | "https") if !url.chars().any(char::is_control) => Ok(()),
        _ => Err(FetchError::Scheme),
    }
}

/// curl's arguments: no redirects followed, only the web, its time and size limits.
fn args(url: &str) -> Vec<&str> {
    vec![
        "--silent",
        "--show-error",
        "--max-time",
        "5",
        "--max-filesize",
        "262144",
        "--proto",
        "=http,https",
        "--globoff",
        "--write-out",
        "%{stderr}\n%{http_code}",
        "--url",
        url,
    ]
}

/// The body as text, cut to [`MAX_BODY`] on a character boundary; true when it was cut.
fn cut(body: &[u8]) -> (String, bool) {
    let text = String::from_utf8_lossy(body);
    if text.len() <= MAX_BODY {
        return (text.into_owned(), false);
    }
    let mut end = MAX_BODY;
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (text.get(..end).unwrap_or_default().to_owned(), true)
}

/// Runs the system curl with `args`, killed with its group past its own limit.
fn curl(args: &[&str]) -> Result<Ran, FetchError> {
    let bin = CURL
        .iter()
        .find(|bin| Path::new(bin).exists())
        .ok_or(FetchError::NoCurl)?;
    spawned(Path::new(bin), args, LIMIT.saturating_add(GRACE))
}

/// Runs `bin`; past `wait` for both its outputs to close, its whole group is killed.
fn spawned(bin: &Path, args: &[&str], wait: Duration) -> Result<Ran, FetchError> {
    #[allow(clippy::disallowed_methods, reason = "the curl spawn")]
    let mut child = Command::new(bin)
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| FetchError::Start(e.to_string()))?;
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let (said, heard) = channel();
    thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.by_ref().take(4096).read_to_string(&mut text);
        }
        let _ = said.send(text);
    });
    let (read, body) = channel();
    thread::spawn(move || {
        let _ = read.send(stdout.map_or((Vec::new(), 0), read_capped));
    });
    let end = Instant::now().checked_add(wait);
    let left = || end.map_or(wait, |end| end.saturating_duration_since(Instant::now()));
    let ran = match (body.recv_timeout(left()), heard.recv_timeout(left())) {
        (Ok((body, sent)), Ok(stderr)) => Ok(Ran { body, sent, stderr }),
        (Err(RecvTimeoutError::Timeout), _) | (_, Err(RecvTimeoutError::Timeout)) => {
            Err(FetchError::Timeout)
        }
        (Err(RecvTimeoutError::Disconnected), _) | (_, Err(RecvTimeoutError::Disconnected)) => {
            Ok(Ran::default())
        }
    };
    stop(&mut child);
    ran
}

/// The first [`MAX_BODY`] + 1 bytes, and how many came in all.
fn read_capped(mut from: impl Read) -> (Vec<u8>, usize) {
    let (mut kept, mut sent, mut chunk) = (Vec::new(), 0usize, [0u8; 8192]);
    while let Ok(n) = from.read(&mut chunk) {
        if n == 0 {
            break;
        }
        sent = sent.saturating_add(n);
        let room = (MAX_BODY + 1).saturating_sub(kept.len());
        kept.extend(chunk.iter().take(n.min(room)));
    }
    (kept, sent)
}

/// Kills the child's whole group and reaps it.
fn stop(child: &mut std::process::Child) {
    let group = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw);
    if let Some(group) = group {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::Kill);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::error::Error;

    use yd_core::command::table::Run;
    use yd_core::inbox::inbox;
    use yd_core::model::Model;
    use yd_core::row::Row;

    use super::*;

    type Check = Result<(), Box<dyn Error>>;

    const URL: &str = "http://ci-ubuntu:9100/queue";

    fn recorded(body: &str, code: &str) -> impl Fn(&[&str]) -> Result<Ran, FetchError> + use<> {
        let (body, code) = (body.to_owned(), code.to_owned());
        move |_| {
            Ok(Ran {
                body: body.clone().into_bytes(),
                sent: body.len(),
                stderr: ["\n", &code].concat(),
            })
        }
    }

    fn served(
        urls: &[&str],
        runner: impl Fn(&[&str]) -> Result<Ran, FetchError> + Send + 'static,
    ) -> Result<Model, Box<dyn Error>> {
        let (tx, inbox) = inbox(|| {});
        let (cmds, heard) = channel();
        let out = tx.feed(FEED);
        let worker = thread::spawn(move || serve(out, heard, runner));
        for url in urls {
            let run = Run {
                text: Some((*url).to_owned()),
                ..Run::new(GET)
            };
            cmds.send(SourceCmd::Run(run))?;
        }
        drop(cmds);
        worker.join().map_err(|_| "the source thread panicked")??;
        let mut model = Model::default();
        inbox.drain(&mut model);
        Ok(model)
    }

    fn row(model: &Model, url: &str) -> Result<Row, Box<dyn Error>> {
        let table = model.tables.get(FEED).ok_or("no fetch table")?;
        let id = yd_core::ids::RowId::new(["get:", url].concat());
        Ok(table.find(&id).ok_or("no row for the url")?.clone())
    }

    #[test]
    fn a_get_upserts_the_status_and_body_under_its_url() -> Check {
        let model = served(&[URL], recorded("{\"waiting\":3}", "200"))?;
        let row = row(&model, URL)?;
        assert_eq!(row.words("status"), Some("200"));
        assert_eq!(row.words("body"), Some("{\"waiting\":3}"));
        assert_eq!(row.words("error"), Some(""));
        Ok(())
    }

    #[test]
    fn an_address_that_is_not_the_web_is_refused_and_curl_never_runs() -> Check {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = ran.clone();
        let runner = move |_: &[&str]| {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Ran::default())
        };
        let model = served(&["file:///etc/passwd", "gopher://x/"], runner)?;
        let row = row(&model, "file:///etc/passwd")?;
        assert_eq!(
            row.words("error"),
            Some("only http and https addresses are fetched")
        );
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn curl_is_asked_for_five_seconds_no_redirects_and_the_size_cap() -> Check {
        let asked = RefCell::new(Vec::new());
        let runner = |args: &[&str]| {
            asked.replace(args.iter().map(|a| (*a).to_owned()).collect());
            Ok(Ran::default())
        };
        answer(URL, &runner);
        let asked = asked.into_inner();
        let pair = |flag: &str| {
            let at = asked.iter().position(|a| a == flag)?;
            asked.get(at + 1).cloned()
        };
        assert_eq!(pair("--max-time").as_deref(), Some("5"));
        assert_eq!(pair("--max-filesize").as_deref(), Some("262144"));
        assert_eq!(pair("--url").as_deref(), Some(URL));
        assert!(!asked.iter().any(|a| a == "--location" || a == "-L"));
        assert!(
            asked.iter().any(|a| a == "--globoff"),
            "a [1-100000] in a url must not fan out"
        );
        Ok(())
    }

    #[test]
    fn a_child_that_holds_its_output_open_is_stopped_at_the_deadline() -> Check {
        let started = std::time::Instant::now();
        let ran = spawned(
            Path::new("/bin/sh"),
            &["-c", "exec sleep 30"],
            Duration::from_millis(300),
        );
        assert_eq!(ran, Err(FetchError::Timeout));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        Ok(())
    }

    #[test]
    fn a_body_past_the_cap_is_cut_and_the_row_says_kept_of_sent() -> Check {
        let big = "é".repeat(MAX_BODY);
        let model = served(&[URL], recorded(&big, "200"))?;
        let row = row(&model, URL)?;
        let body = row.words("body").ok_or("no body")?;
        assert!(body.len() <= MAX_BODY && body.chars().all(|c| c == 'é'));
        let said = format!("kept {} of {} bytes", body.len(), big.len());
        assert_eq!(row.words("error"), Some(said.as_str()));
        Ok(())
    }

    #[test]
    fn a_failed_run_keeps_curls_own_words_and_no_status() -> Check {
        let runner = |_: &[&str]| {
            Ok(Ran {
                stderr: "curl: (6) Could not resolve host: ci-ubuntu\n000".to_owned(),
                ..Ran::default()
            })
        };
        let row = answer(URL, &runner);
        assert_eq!(row.words("status"), Some(""));
        assert_eq!(
            row.words("error"),
            Some("curl: (6) Could not resolve host: ci-ubuntu")
        );
        Ok(())
    }
}
