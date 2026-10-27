use std::error::Error;
use std::sync::{Arc, Mutex};

use yi_runtime::{PermissionBroker, PermissionMode};
use yi_types::permission::SessionPermissionRule;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn an_allow_write_grant_is_journaled_through_the_attached_rule_journal() -> TestResult {
    let scratch = Scratch::new("allow_write_journal")?;
    let broker = PermissionBroker::new(
        PermissionMode::Auto,
        scratch.to_path_buf(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(1).0,
    );
    let journaled: Arc<Mutex<Vec<SessionPermissionRule>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&journaled);
    broker.set_rule_journal(Arc::new(move |rule: SessionPermissionRule| {
        if let Ok(mut kept) = sink.lock() {
            kept.push(rule);
        }
    }));
    broker.grant_write(&scratch)?;
    let kept = journaled
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert!(
        !kept.is_empty(),
        "an `--allow-write` grant made through the attached rule journal was never journaled, \
         so `--continue` cannot restore it: journaled={kept:?}"
    );
    Ok(())
}
