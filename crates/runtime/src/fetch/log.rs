use std::sync::{Mutex, MutexGuard, PoisonError};

use yi_types::fetch::{FETCH_ENTRY_TYPE, FetchRecord};
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
        if let Some(session) = &session
            && let Ok(payload) = serde_json::to_value(&record)
        {
            let _log_write_never_fails_a_fetch = yi_session::lock_session(session).append_custom(
                "main",
                FETCH_ENTRY_TYPE,
                Some(payload),
            );
        }
        self.lock().records.push((base_of(url), record));
    }

    pub fn records(&self) -> Vec<FetchRecord> {
        self.lock()
            .records
            .iter()
            .map(|(_, record)| record.clone())
            .collect()
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
