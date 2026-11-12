use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::ops::Range;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ask {
    Check,
    Complete,
    Guesses,
    Correct,
}

impl Ask {
    fn op(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Complete => "complete",
            Self::Guesses => "guesses",
            Self::Correct => "correct",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Misspelled words as char ranges of the asked text.
    Spans(Vec<Range<usize>>),
    Words(Vec<String>),
}

/// Invariant: every call returns at once. Requests and replies cross bounded channels to two
/// threads that own the peer, so a stalled checker costs a missing answer, never a frame.
pub struct Speller {
    jobs: Option<SyncSender<String>>,
    replies: Receiver<Option<(u64, Value)>>,
    next: u64,
    asked: HashMap<u64, (Ask, String, Instant)>,
    known: HashMap<(Ask, String), Answer>,
    answered: u64,
}

const JOBS: usize = 64;
const KNOWN: usize = 4096;
const REPLY_BYTES: u64 = 1 << 20;
/// Incident: an answer that never came left the old helper blocking a key forever; past this
/// the peer is dead and spelling is absent.
pub const DEADLINE: Duration = Duration::from_secs(3);

/// NSSpellChecker over the Objective-C bridge every Mac ships, one JSON object a line. Requests
/// arrive ASCII-escaped, so a read that splits a chunk cannot split a character.
const SCRIPT: &str = r#"ObjC.import('AppKit');
var sc = $.NSSpellChecker.sharedSpellChecker, lang = sc.language, nil = $();
var input = $.NSFileHandle.fileHandleWithStandardInput, out = $.NSFileHandle.fileHandleWithStandardOutput;
function answer(op, text) {
  var s = $(text), n = text.length, found = [];
  if (op === 'check') {
    for (var at = 0; at < n;) {
      var r = sc.checkSpellingOfStringStartingAt(s, at), from = Number(r.location), len = Number(r.length);
      if (len === 0 || from < at || from >= n) break;
      found.push([from, len]); at = from + len;
    }
    return found;
  }
  var all = $.NSMakeRange(0, n);
  if (op === 'complete') return ObjC.deepUnwrap(sc.completionsForPartialWordRangeInStringLanguageInSpellDocumentWithTag(all, s, nil, 0)) || [];
  if (op === 'guesses') return ObjC.deepUnwrap(sc.guessesForWordRangeInStringLanguageInSpellDocumentWithTag(all, s, nil, 0)) || [];
  var fixed = ObjC.unwrap(sc.correctionForWordRangeInStringLanguageInSpellDocumentWithTag(all, s, lang, 0));
  return fixed ? [fixed] : [];
}
var held = '';
for (;;) {
  var data = input.availableData;
  if (Number(data.length) === 0) break;
  held += ObjC.unwrap($.NSString.alloc.initWithDataEncoding(data, $.NSASCIIStringEncoding));
  for (var nl; (nl = held.indexOf('\n')) >= 0;) {
    var line = held.slice(0, nl), reply;
    held = held.slice(nl + 1);
    try { var q = JSON.parse(line); reply = {id: q.id, out: answer(q.op, q.text)}; } catch (e) { reply = {id: -1, out: []}; }
    out.writeData($(JSON.stringify(reply) + '\n').dataUsingEncoding($.NSUTF8StringEncoding));
  }
}"#;

impl Speller {
    /// Apple's checker on a Mac; elsewhere there is none.
    pub fn apple() -> Option<Self> {
        cfg!(target_os = "macos").then(|| {
            let mut osascript = crate::process::command("/usr/bin/osascript");
            osascript.args(["-l", "JavaScript", "-e", SCRIPT]);
            Self::spawn(osascript)
        })
    }

    /// A checker speaking the line protocol over `peer`'s stdin and stdout.
    pub fn spawn(peer: Command) -> Self {
        let (jobs, queue) = mpsc::sync_channel(JOBS);
        let (answers, replies) = mpsc::sync_channel(JOBS);
        let _detached = std::thread::Builder::new()
            .name("yi-spell".to_owned())
            .spawn(move || serve(peer, &queue, answers));
        Self {
            jobs: Some(jobs),
            replies,
            next: 0,
            asked: HashMap::new(),
            known: HashMap::new(),
            answered: 0,
        }
    }

    /// The answer when it is known; otherwise the question goes out and `None` comes back.
    pub fn ask(&mut self, ask: Ask, text: &str, now: Instant) -> Option<&Answer> {
        let key = (ask, text.to_owned());
        if self.known.contains_key(&key) {
            return self.known.get(&key);
        }
        let out = self.asked.values().any(|(a, t, _)| *a == ask && t == text);
        if let Some(jobs) = self.jobs.as_ref().filter(|_| !out) {
            let id = self.next;
            let line = format!(
                "{{\"id\":{id},\"op\":\"{}\",\"text\":{}}}\n",
                ask.op(),
                ascii_json(text)
            );
            match jobs.try_send(line) {
                Ok(()) => {
                    self.next = id.saturating_add(1);
                    self.asked.insert(id, (ask, key.1, now));
                }
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => self.jobs = None,
            }
        }
        None
    }

    /// Takes in what has arrived. The count of answers ever taken in, so a caller that saw an
    /// older count knows to look again.
    pub fn poll(&mut self, now: Instant) -> u64 {
        while let Ok(reply) = self.replies.try_recv() {
            let Some((id, out)) = reply else {
                self.jobs = None;
                break;
            };
            let Some((ask, text, _)) = self.asked.remove(&id) else {
                continue;
            };
            if self.known.len() >= KNOWN {
                self.known.clear();
            }
            let answer = decode(ask, &text, &out);
            self.known.insert((ask, text), answer);
            self.answered = self.answered.saturating_add(1);
        }
        let late =
            (self.asked.values()).any(|(_, _, at)| now.saturating_duration_since(*at) > DEADLINE);
        if late || self.jobs.is_none() {
            self.jobs = None;
            self.asked.clear();
        }
        self.answered
    }

    pub fn waiting(&self) -> bool {
        !self.asked.is_empty()
    }

    /// False once the peer failed to start, died, desynced or missed [`DEADLINE`].
    pub fn alive(&self) -> bool {
        self.jobs.is_some()
    }
}

fn serve(mut peer: Command, queue: &Receiver<String>, answers: SyncSender<Option<(u64, Value)>>) {
    let child = peer
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        let _ = answers.send(None);
        return;
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = answers.send(None);
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    let _detached = std::thread::Builder::new()
        .name("yi-spell-read".to_owned())
        .spawn(move || read_replies(stdout, &answers));
    while let Ok(line) = queue.recv() {
        if stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .is_err()
        {
            break;
        }
    }
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
}

fn read_replies(stdout: std::process::ChildStdout, answers: &SyncSender<Option<(u64, Value)>>) {
    let mut lines = BufReader::new(stdout);
    loop {
        let mut line = Vec::new();
        let read = lines
            .by_ref()
            .take(REPLY_BYTES)
            .read_until(b'\n', &mut line);
        let reply = match read {
            Ok(n) if n > 0 && line.ends_with(b"\n") => serde_json::from_slice::<Value>(&line).ok(),
            _ => None,
        };
        let parsed = reply.and_then(|reply| {
            let id = reply.get("id").and_then(Value::as_u64)?;
            Some((id, reply.get("out").cloned().unwrap_or(Value::Null)))
        });
        let stop = parsed.is_none();
        if answers.send(parsed).is_err() || stop {
            return;
        }
    }
}

fn decode(ask: Ask, text: &str, out: &Value) -> Answer {
    let items = out.as_array().map(Vec::as_slice).unwrap_or_default();
    if ask != Ask::Check {
        return Answer::Words(
            items
                .iter()
                .filter_map(|word| word.as_str().map(str::to_owned))
                .collect(),
        );
    }
    let unit = |value: Option<&Value>| {
        value
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    };
    let spans = items.iter().filter_map(|pair| {
        let start = unit(pair.get(0))?;
        let end = start.checked_add(unit(pair.get(1))?)?;
        Some(char_at(text, start)..char_at(text, end))
    });
    Answer::Spans(spans.filter(|span| span.start < span.end).collect())
}

/// The char index of a UTF-16 offset, which is what an NSString range counts.
fn char_at(text: &str, utf16: usize) -> usize {
    let mut units = 0_usize;
    for (index, ch) in text.chars().enumerate() {
        if units >= utf16 {
            return index;
        }
        units = units.saturating_add(ch.len_utf16());
    }
    text.chars().count()
}

fn ascii_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    for ch in Value::String(text.to_owned()).to_string().chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            for unit in ch.encode_utf16(&mut [0; 2]) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}
