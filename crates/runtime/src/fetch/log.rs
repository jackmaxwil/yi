use std::sync::{Mutex, MutexGuard, PoisonError};

use yi_types::fetch::FetchRecord;
use yi_types::url::{Durability, Url};

#[derive(Default)]
struct LogState {
    records: Vec<(String, FetchRecord)>,
    pins: Vec<(String, Url)>,
    session: Option<crate::goal::StoreHandle>,
}

#[derive(Default)]
pub struct FetchLog {
    state: Mutex<LogState>,
}

fn base_of(url: &Url) -> String {
    format!("{}://{}", url.scheme(), url.path())
}

impl FetchLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn attach_session(&self, session: yi_session::SharedSession) {
        self.attach_session_handle(std::sync::Arc::new(move || Some(session.clone())));
    }

    pub fn attach_session_handle(&self, handle: crate::goal::StoreHandle) {
        self.lock().session = Some(handle);
    }

    fn lock(&self) -> MutexGuard<'_, LogState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn record(&self, url: &Url, record: FetchRecord) {
        let session = self.lock().session.clone().and_then(|handle| handle());
        if let Some(session) = &session {
            let _log_write_never_fails_a_fetch =
                yi_session::lock_session(session).append_custom_record(&record);
        }
        self.remember(url, record);
    }

    /// Invariant: [`Self::record`] without the session row, for a read of the very listing
    /// that row would grow: a paged walk must not chase the entries it writes (D213).
    pub(crate) fn remember(&self, url: &Url, record: FetchRecord) {
        let key = base_of(url);
        let mut state = self.lock();
        match state.records.iter().position(|(base, _)| *base == key) {
            // Invariant: one row per address, so a daemon re-reading a file for a week logs
            // the latest hash, not a week of rows; every reader is a membership test.
            Some(at) => {
                if let Some(row) = state.records.get_mut(at) {
                    row.1 = record;
                }
            }
            None => state.records.push((key, record)),
        }
    }

    pub fn records(&self) -> Vec<FetchRecord> {
        self.lock()
            .records
            .iter()
            .map(|(_, record)| record.clone())
            .collect()
    }

    /// M3: what a delegation named against what the resolver served. Both sides are
    /// scheme-plus-path, so a citation with a fragment still names the supplied address.
    pub fn relevance(&self, supplied: &[Url]) -> Relevance {
        let state = self.lock();
        let served = state.records.iter().map(|(base, _)| base.clone());
        measure(served.collect(), supplied)
    }

    /// Invariant: identity is scheme plus path — a fragment is an expectation
    /// on the content, so a fetch without one still backs a cited span.
    pub fn backs(&self, citation: &Url) -> bool {
        let key = base_of(citation);
        self.lock().records.iter().any(|(base, _)| *base == key)
    }

    pub fn unbacked(&self, citations: &[Url]) -> Vec<Url> {
        citations
            .iter()
            .filter(|citation| !self.backs(citation))
            .cloned()
            .collect()
    }

    pub fn register_pin(&self, ephemeral: &Url, pinned: Url) -> Result<(), PinError> {
        if ephemeral.durability() != Durability::Ephemeral {
            return Err(PinError::SourceDurable {
                url: ephemeral.to_string(),
            });
        }
        if pinned.durability() != Durability::Durable {
            return Err(PinError::PinEphemeral {
                url: pinned.to_string(),
            });
        }
        self.lock().pins.push((ephemeral.to_string(), pinned));
        Ok(())
    }

    pub fn pin_of(&self, url: &Url) -> Option<Url> {
        let key = url.to_string();
        self.lock()
            .pins
            .iter()
            .rev()
            .find(|(source, _)| *source == key)
            .map(|(_, pinned)| pinned.clone())
    }

    /// Invariant: pins are minted by the host at record time, never by a model
    /// — an ephemeral URL either carries a registered pin or refuses the record.
    pub fn rewrite_terminal(&self, urls: &[Url]) -> Result<Vec<Url>, TerminalRecordError> {
        urls.iter()
            .map(|url| match url.durability() {
                Durability::Durable => Ok(url.clone()),
                Durability::Ephemeral => {
                    self.pin_of(url)
                        .ok_or_else(|| TerminalRecordError::EphemeralUnpinned {
                            url: url.to_string(),
                        })
                }
            })
            .collect()
    }
}

/// Over-supply is `unused`; under-supply is `unsupplied` — what nobody handed over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relevance {
    pub supplied: usize,
    pub referenced: usize,
    pub unused: Vec<String>,
    pub unsupplied: Vec<String>,
}

fn measure(mut served: Vec<String>, supplied: &[Url]) -> Relevance {
    served.sort_unstable();
    served.dedup();
    let named: Vec<String> = supplied.iter().map(base_of).collect();
    Relevance {
        supplied: named.len(),
        referenced: named.iter().filter(|base| served.contains(base)).count(),
        unused: supplied
            .iter()
            .filter(|url| !served.contains(&base_of(url)))
            .map(Url::to_string)
            .collect(),
        unsupplied: served
            .into_iter()
            .filter(|base| !named.contains(base))
            .collect(),
    }
}

/// A workspace file's text and hash exactly as `local://` serves them whole, so a blob named
/// by digest can be matched against the row a read of it left.
pub fn as_served(raw: &str) -> (String, String) {
    use yi_tools::hashline::normalize::{normalize_to_lf, strip_bom};
    let text = normalize_to_lf(strip_bom(raw).text);
    let hash = super::content_hash(&text);
    (text, hash)
}

/// The fetch rows a transcript carries: a child's fetches are durable custom entries, so
/// they outlive the child and answer "was this read, and at which hash" after it is gone.
pub fn rows_of(session: &yi_session::SharedSession) -> Vec<FetchRecord> {
    yi_session::lock_session(session).custom_records(yi_session::EntryOrder::NewestFirst, None)
}

/// M3 over a transcript rather than a live log: the owner measures its own supply after the
/// child is gone.
pub fn relevance_of(session: &yi_session::SharedSession, supplied: &[Url]) -> Relevance {
    let served = rows_of(session)
        .into_iter()
        .filter_map(|record| record.url.parse::<Url>().ok())
        .map(|url| base_of(&url))
        .collect();
    measure(served, supplied)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PinError {
    #[error("pin source {url} is durable already; only an ephemeral URL downgrades")]
    SourceDurable { url: String },
    #[error("pin target {url} is ephemeral; a pin must outlive its referent")]
    PinEphemeral { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TerminalRecordError {
    #[error("terminal record refuses ephemeral {url}: no pin was minted at reap")]
    EphemeralUnpinned { url: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn record_for(url: &Url) -> FetchRecord {
        FetchRecord {
            url: url.to_string(),
            hash: "deadbeef".to_owned(),
            served_by: "test".to_owned(),
        }
    }

    #[test]
    fn relevance_names_both_the_unused_and_the_unsupplied() -> TestResult {
        let log = FetchLog::new();
        let read: Url = "local://src/auth.rs".parse()?;
        let found: Url = "local://src/token.rs".parse()?;
        log.record(&read, record_for(&read));
        log.record(&found, record_for(&found));
        let handed: Url = "local://docs/auth.md".parse()?;
        let measured = log.relevance(&[read, handed.clone()]);
        assert_eq!(measured.supplied, 2);
        assert_eq!(measured.referenced, 1);
        assert_eq!(measured.unused, vec![handed.to_string()]);
        assert_eq!(measured.unsupplied, vec![found.to_string()]);
        Ok(())
    }

    #[test]
    fn re_reading_one_address_leaves_one_row() -> TestResult {
        let log = FetchLog::new();
        let url: Url = "local://src/auth.rs".parse()?;
        for _ in 0..64 {
            log.record(&url, record_for(&url));
        }
        let with_fragment: Url = "local://src/auth.rs#L1-2@9F3E".parse()?;
        log.record(&with_fragment, record_for(&with_fragment));
        assert_eq!(
            log.records().len(),
            1,
            "one address is one row however often it is read"
        );
        assert!(log.backs(&url), "the row still backs its citation");
        Ok(())
    }

    #[test]
    fn an_unbacked_citation_is_flagged() -> TestResult {
        let log = FetchLog::new();
        let fetched: Url = "local://src/auth.rs".parse()?;
        log.record(&fetched, record_for(&fetched));
        let cited_with_fragment: Url = "local://src/auth.rs#L1-2@9F3E".parse()?;
        let never_fetched: Url = "local://src/ghost.rs".parse()?;
        let flagged = log.unbacked(&[cited_with_fragment, never_fetched.clone()]);
        assert_eq!(flagged, vec![never_fetched]);
        Ok(())
    }

    #[test]
    fn an_ephemeral_url_is_refused_in_a_terminal_record() -> TestResult {
        let log = FetchLog::new();
        let live: Url = "agent://7f3a-auth/implement-refresh-flow".parse()?;
        let durable: Url = "local://docs/auth.md".parse()?;
        let error = log
            .rewrite_terminal(&[durable, live.clone()])
            .err()
            .ok_or("unpinned ephemeral must refuse the record")?;
        assert_eq!(
            error,
            TerminalRecordError::EphemeralUnpinned {
                url: live.to_string()
            }
        );
        Ok(())
    }

    #[test]
    fn a_pinned_ephemeral_is_rewritten_at_the_terminal_seam() -> TestResult {
        let log = FetchLog::new();
        let live: Url = "agent://7f3a-auth/implement-refresh-flow".parse()?;
        let pin: Url = "history://implement-refresh-flow".parse()?;
        log.register_pin(&live, pin.clone())?;
        let rewritten = log.rewrite_terminal(std::slice::from_ref(&live))?;
        assert_eq!(rewritten, vec![pin]);
        Ok(())
    }

    #[test]
    fn a_pin_never_points_at_an_ephemeral() -> TestResult {
        let log = FetchLog::new();
        let live: Url = "agent://p/t".parse()?;
        let also_live: Url = "kernel://main/x".parse()?;
        let durable: Url = "local://a.txt".parse()?;
        assert!(matches!(
            log.register_pin(&live, also_live),
            Err(PinError::PinEphemeral { .. })
        ));
        assert!(matches!(
            log.register_pin(&durable.clone(), durable),
            Err(PinError::SourceDurable { .. })
        ));
        Ok(())
    }
}
