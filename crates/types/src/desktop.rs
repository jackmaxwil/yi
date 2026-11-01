use std::collections::BTreeMap;

use serde::Deserialize;

/// The part of agent-desktop's `refmap.json` Yi reads. Yi never writes this file, so unknown fields
/// are dropped rather than kept.
#[derive(Deserialize)]
struct RefMap {
    inner: BTreeMap<String, RefEntry>,
}

#[derive(Deserialize)]
struct RefEntry {
    source_app: Option<String>,
}

/// The app an unqualified ref such as `@e5` was snapshotted in; None for a record that does not
/// parse, a ref it does not hold, or an entry naming no app.
#[must_use]
pub fn source_app(refmap: &[u8], element: &str) -> Option<String> {
    let map: RefMap = serde_json::from_slice(refmap).ok()?;
    map.inner.get(element)?.source_app.clone()
}
