use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Instant;

use yi_runtime::spell::{Answer, Ask, Speller};
use yi_types::config::CompleteEngine;

use super::{AssistSettings, Engine};

/// One checker per process, shared by every chat pane. The lock covers channel and map work
/// only; the checker itself answers on its own threads.
static APPLE: LazyLock<Option<Mutex<Speller>>> = LazyLock::new(|| Speller::apple().map(Mutex::new));
static WORDS: LazyLock<Arc<OnceLock<Words>>> = LazyLock::new(Arc::default);
static LOADING: AtomicBool = AtomicBool::new(false);

struct Apple;

pub(super) fn apple(settings: &AssistSettings) -> Option<Box<dyn Engine>> {
    let used = settings.typos
        || settings.autocorrect
        || matches!(
            settings.complete,
            CompleteEngine::Auto | CompleteEngine::Apple
        );
    (used && cfg!(target_os = "macos")).then(|| Box::new(Apple) as Box<dyn Engine>)
}

fn with<T>(job: impl FnOnce(&mut Speller) -> Option<T>) -> Option<T> {
    let mut speller = APPLE.as_ref()?.lock().ok()?;
    job(&mut speller)
}

impl Engine for Apple {
    fn typos(&mut self, line: &str) -> Option<Vec<Range<usize>>> {
        with(
            |speller| match speller.ask(Ask::Check, line, Instant::now())? {
                Answer::Spans(spans) => Some(spans.clone()),
                Answer::Words(_) => None,
            },
        )
    }

    fn words(&mut self, ask: Ask, text: &str) -> Option<Vec<String>> {
        with(|speller| match speller.ask(ask, text, Instant::now())? {
            Answer::Words(words) => Some(words.clone()),
            Answer::Spans(_) => None,
        })
    }

    fn poll(&mut self) -> u64 {
        with(|speller| Some(speller.poll(Instant::now()))).unwrap_or_default()
    }

    fn waiting(&self) -> bool {
        with(|speller| Some(speller.waiting())).unwrap_or_default()
    }

    fn alive(&self) -> bool {
        with(|speller| Some(speller.alive())).unwrap_or_default()
    }
}

/// The user's own words, counted, from their recent prompts; ordered, so a prefix is a range of
/// the vocabulary rather than a scan of all of it.
#[derive(Debug, Default)]
pub struct Words(BTreeMap<String, u32>);

const SESSIONS: usize = 16;
/// Incident: a session's second line carries the whole system prompt, over 64 KiB, so a 64 KiB
/// head held no prompt at all.
const SESSION_BYTES: u64 = 1 << 20;

impl Words {
    pub fn from_sessions(sessions_dir: &Path) -> Self {
        let mut words = Self::default();
        for text in yi_runtime::history::recent_user_texts(sessions_dir, SESSIONS, SESSION_BYTES) {
            words.learn(&text);
        }
        words
    }

    pub fn learn(&mut self, text: &str) {
        let words = text.split(|ch: char| !ch.is_alphabetic());
        for word in words.filter(|word| word.chars().count() >= 2) {
            let count = self.0.entry(word.to_lowercase()).or_default();
            *count = count.saturating_add(1);
        }
    }

    /// The rest of the most used word that starts with `prefix`.
    pub fn suffix(&self, prefix: &str) -> Option<String> {
        let prefix = prefix.to_lowercase();
        let started = self.0.range(prefix.clone()..);
        let longer = (started.take_while(|(word, _)| word.starts_with(&prefix)))
            .filter(|(word, _)| word.len() > prefix.len());
        let best = longer.max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))?;
        best.0.get(prefix.len()..).map(str::to_owned)
    }
}

pub(super) fn words() -> Arc<OnceLock<Words>> {
    Arc::clone(&WORDS)
}

/// Reads the word list once, off the key thread; completion has no n-gram words until it lands.
pub(super) fn load_words(words: &Arc<OnceLock<Words>>, sessions_dir: &std::path::Path) {
    if !Arc::ptr_eq(words, &WORDS) || LOADING.swap(true, Ordering::Relaxed) {
        return;
    }
    let (cell, dir) = (Arc::clone(words), sessions_dir.to_path_buf());
    let _detached = std::thread::Builder::new()
        .name("yi-words".to_owned())
        .spawn(move || cell.set(Words::from_sessions(&dir)));
}
