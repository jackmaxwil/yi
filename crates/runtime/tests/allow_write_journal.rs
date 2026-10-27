use std::error::Error;
use std::sync::Arc;

use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, PermissionBroker, PermissionMode, ProviderStream, SessionConfig};

use crate::scratch::Scratch;
use crate::support;

type TestResult = Result<(), Box<dyn Error>>;

fn ledgered(store: &yi_session::SharedSession) -> Result<usize, Box<dyn Error>> {
    let query = yi_session::EntryQuery {
        custom_type: Some(yi_types::permission::PERMISSION_RULE_ENTRY.to_owned()),
        ..yi_session::EntryQuery::default()
    };
    Ok(yi_session::lock_session(store).find_entries(&query)?.len())
}

/// `yi ask` grants `--allow-write` after the tools are wired and before the session store is
/// attached; the grant reached no ledger, so `--continue` without the flag lost it.
#[test]
fn an_allow_write_grant_made_before_the_store_attaches_is_journaled() -> TestResult {
    let scratch = Scratch::new("allow_write_journal")?;
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: support::faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::new(ProviderStream::new(None)),
    );
    let broker = Arc::new(PermissionBroker::new(
        PermissionMode::Auto,
        scratch.to_path_buf(),
        Vec::new(),
        None,
        session.events_sender(),
    ));
    session.use_tools(Vec::new(), scratch.to_path_buf(), Some(Arc::clone(&broker)));
    broker.grant_write(&scratch)?;
    let store = support::memory_store("allow-write");
    session.attach_store(Arc::clone(&store))?;
    assert_eq!(ledgered(&store)?, 1, "the grant reaches the session ledger");
    session.attach_store(Arc::clone(&store))?;
    assert_eq!(
        ledgered(&store)?,
        1,
        "a re-attach does not journal it twice"
    );
    Ok(())
}
