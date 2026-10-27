use std::error::Error;
use std::sync::{Arc, Mutex};

use yi_runtime::{PermissionBroker, PermissionMode};
use yi_types::permission::SessionPermissionRule;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn a_startup_grant_is_journaled_once_the_rule_journal_is_attached() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-allow-write-journal-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let broker = PermissionBroker::new(
        PermissionMode::Auto,
        dir.clone(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(1).0,
    );
    broker.grant_write(&dir)?;
    let journaled: Arc<Mutex<Vec<SessionPermissionRule>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&journaled);
    broker.set_rule_journal(Arc::new(move |rule: SessionPermissionRule| {
        if let Ok(mut kept) = sink.lock() {
            kept.push(rule);
        }
    }));
    let kept = journaled
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert!(
        !kept.is_empty(),
        "an --allow-write grant made before the rule journal is attached was never journaled, \
         so `--continue` cannot restore it: journaled={kept:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}
