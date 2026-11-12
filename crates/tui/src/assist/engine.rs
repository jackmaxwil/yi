use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use super::{ARMED, AUTO, CharSpan, EXTRA, Engine, FAKE, FORCE, LEDGER, Ledger, SETTINGS};
use crate::commands::SLASH_COMMANDS;
use yi_types::config::{AssistSettings, SMOL_REL};

static HELPER: Mutex<Option<Helper>> = Mutex::new(None);
static HELPER_DEAD: AtomicBool = AtomicBool::new(false);

pub(super) fn ngram_suffix(prefix: &str) -> Option<String> {
    let prefix_l = prefix.to_lowercase();
    let mut best: Option<(u32, String)> = None;
    let consider = |word: &str, count: u32, best: &mut Option<(u32, String)>| {
        if !word.starts_with(&prefix_l) || word.chars().count() <= prefix_l.chars().count() {
            return;
        }
        let suffix: String = word.chars().skip(prefix_l.chars().count()).collect();
        let replace = best.as_ref().is_none_or(|(held, _)| count > *held);
        if replace {
            *best = Some((count, suffix));
        }
    };
    EXTRA.with(|words| {
        for (word, count) in words.borrow().iter() {
            consider(word, *count, &mut best);
        }
    });
    if let Some(ledger) = guard(&LEDGER).as_ref() {
        for (word, count) in &ledger.words {
            consider(word, *count, &mut best);
        }
    }
    best.map(|(_, suffix)| suffix)
}

pub(super) fn add_words(words: &mut HashMap<String, u32>, text: &str) {
    let mut word = String::new();
    let flush = |word: &mut String, words: &mut HashMap<String, u32>| {
        if word.chars().count() >= 2 {
            *words.entry(word.to_lowercase()).or_insert(0) += 1;
        }
        word.clear();
    };
    for ch in text.chars() {
        if ch.is_alphabetic() {
            word.push(ch);
        } else {
            flush(&mut word, words);
        }
    }
    flush(&mut word, words);
}

pub(super) fn ledger_for_key(session_dir: &str) -> LedgerView {
    let path = if !session_dir.is_empty() {
        Some(PathBuf::from(session_dir))
    } else if ARMED.load(Ordering::Relaxed) {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".yi/sessions"))
    } else {
        None
    };
    let Some(path) = path else {
        return LedgerView::default();
    };
    ledger_for(&path)
}

#[derive(Default)]
pub(super) struct LedgerView {
    pub(super) slashes: HashMap<String, u32>,
}

pub(super) fn ledger_for(dir: &Path) -> LedgerView {
    let key = dir.display().to_string();
    let mut slot = guard(&LEDGER);
    let ready = slot.as_ref().is_some_and(|ledger| ledger.key == key);
    if !ready {
        *slot = Some(read_ledger(dir, &key));
    }
    LedgerView {
        slashes: slot
            .as_ref()
            .map(|ledger| ledger.slashes.clone())
            .unwrap_or_default(),
    }
}

fn read_ledger(dir: &Path, key: &str) -> Ledger {
    let mut words = HashMap::new();
    let mut slashes = HashMap::new();
    let mut files = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .take(32)
        .collect::<Vec<_>>();
    files.sort();
    for path in files {
        let bytes = std::fs::read(&path).unwrap_or_default();
        let kept: Vec<u8> = bytes.into_iter().take(65_536).collect();
        let text = String::from_utf8_lossy(&kept);
        for line in text.lines() {
            if line.contains("\"role\":\"user\"") || line.contains("\"role\": \"user\"") {
                add_words(&mut words, line);
            }
            for verb in SLASH_COMMANDS {
                let found = count_verb(line, verb);
                if found > 0 {
                    *slashes.entry((*verb).to_owned()).or_insert(0) += found;
                }
            }
        }
    }
    Ledger {
        key: key.to_owned(),
        words,
        slashes,
    }
}

fn count_verb(text: &str, verb: &str) -> u32 {
    let needle: Vec<char> = format!("/{verb}").chars().collect();
    let chars: Vec<char> = text.chars().collect();
    let mut count = 0;
    let mut index = 0;
    while index + needle.len() <= chars.len() {
        if chars[index..index + needle.len()] == needle[..] {
            let after = chars.get(index + needle.len()).copied();
            if after.is_none_or(|ch| !ch.is_ascii_alphanumeric()) {
                count += 1;
            }
            index += needle.len();
        } else {
            index += 1;
        }
    }
    count
}

pub(super) fn typos_on() -> bool {
    settings().typos
}

pub(super) fn autocorrect() -> bool {
    AUTO.with(|flag| flag.get())
        .unwrap_or(settings().autocorrect)
}

pub(super) fn fake() -> bool {
    FAKE.with(|flag| flag.get())
}

pub(super) fn settings() -> AssistSettings {
    *guard(&SETTINGS)
}

pub(super) fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

pub(super) struct Force;

impl Drop for Force {
    fn drop(&mut self) {
        FORCE.with(|flag| flag.set(false));
    }
}

pub(super) fn force_native() -> Force {
    FORCE.with(|flag| flag.set(true));
    Force
}

fn native_ok() -> bool {
    FORCE.with(|flag| flag.get()) || ARMED.load(Ordering::Relaxed)
}

pub(super) fn apple_typos(text: &str) -> Vec<CharSpan> {
    let mut spans = Vec::new();
    for line in text.split('\n') {
        for (start, end) in apple_ranges(line) {
            spans.push(CharSpan { start, end });
        }
    }
    spans
}

fn apple_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for line in ask("check", text).unwrap_or_default() {
        let mut parts = line.split_whitespace();
        let Some(start) = parts.next().and_then(|part| part.parse::<usize>().ok()) else {
            continue;
        };
        let Some(len) = parts.next().and_then(|part| part.parse::<usize>().ok()) else {
            continue;
        };
        let from = super::char_index(text, start);
        let to = super::char_index(text, start + len);
        if to > from {
            out.push((from, to));
        }
    }
    out
}

pub(super) fn apple_suffix(prefix: &str) -> Option<String> {
    let word = ask("complete", prefix)?.into_iter().next()?;
    let prefix_l = prefix.to_lowercase();
    let word_l = word.to_lowercase();
    if !word_l.starts_with(&prefix_l) || word_l.chars().count() <= prefix_l.chars().count() {
        return None;
    }
    Some(word.chars().skip(prefix.chars().count()).collect())
}

pub(super) fn apple_correct(word: &str) -> Option<String> {
    let fixed = ask("correct", word)?.into_iter().next()?;
    (fixed != word).then_some(fixed)
}

pub(super) fn apple_guesses(word: &str) -> Vec<String> {
    ask("guesses", word).unwrap_or_default()
}

pub(super) fn apple_engine(prefix: &str) -> Engine {
    if option_env!("SPELL_HELPER").is_none() || HELPER_DEAD.load(Ordering::Relaxed) {
        return Engine::Absent;
    }
    match apple_suffix(prefix) {
        Some(suffix) => Engine::Hit(suffix),
        None => Engine::Miss,
    }
}

pub(super) fn smol_engine(home: &Path, prefix: &str) -> Engine {
    let model = home.join(SMOL_REL);
    if !model.is_file() {
        return Engine::Absent;
    }
    let Some(runner) = runner() else {
        return Engine::Absent;
    };
    match smol_suffix(&runner, &model, prefix) {
        Some(suffix) => Engine::Hit(suffix),
        None => Engine::Miss,
    }
}

fn runner() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["llama-cli", "llama-completion", "llama"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[expect(
    clippy::disallowed_methods,
    reason = "the small model runs in an external llama program only after an explicit download"
)]
fn smol_suffix(runner: &Path, model: &Path, prefix: &str) -> Option<String> {
    let output = std::process::Command::new(runner)
        .args(["-m"])
        .arg(model)
        .args([
            "-p",
            prefix,
            "-n",
            "12",
            "--no-warmup",
            "-no-display-prompt",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|line| !line.trim().is_empty())?.trim();
    let rest = line
        .strip_prefix(prefix)
        .unwrap_or(line)
        .chars()
        .take_while(|ch| !ch.is_whitespace())
        .collect::<String>();
    (!rest.is_empty() && rest != prefix).then_some(rest)
}

struct Helper {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    lines: std::io::BufReader<std::process::ChildStdout>,
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ask(cmd: &str, text: &str) -> Option<Vec<String>> {
    if !native_ok() || fake() || HELPER_DEAD.load(Ordering::Relaxed) {
        return None;
    }
    let path = option_env!("SPELL_HELPER")?;
    let mut slot = guard(&HELPER);
    if slot.is_none() {
        *slot = spawn_helper(path);
        if slot.is_none() {
            HELPER_DEAD.store(true, Ordering::Relaxed);
            return None;
        }
    }
    let helper = slot.as_mut()?;
    let flat: String = text
        .chars()
        .map(|ch| if ch == '\n' { ' ' } else { ch })
        .collect();
    if writeln!(helper.stdin, "{cmd} {flat}").is_err() || helper.stdin.flush().is_err() {
        *slot = None;
        HELPER_DEAD.store(true, Ordering::Relaxed);
        return None;
    }
    let mut out = Vec::new();
    loop {
        let mut line = String::new();
        match helper.lines.read_line(&mut line) {
            Ok(0) | Err(_) => {
                *slot = None;
                HELPER_DEAD.store(true, Ordering::Relaxed);
                return None;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed == "." {
                    break;
                }
                if !trimmed.is_empty() {
                    out.push(trimmed.to_owned());
                }
            }
        }
    }
    Some(out)
}

#[expect(
    clippy::disallowed_methods,
    reason = "Apple spelling runs in a helper compiled beside this crate, started when the composer asks"
)]
fn spawn_helper(path: &str) -> Option<Helper> {
    let mut child = std::process::Command::new(path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    Some(Helper {
        child,
        stdin,
        lines: std::io::BufReader::new(stdout),
    })
}
