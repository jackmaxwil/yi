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

/// Every ref a snapshot's `refmap.json` holds, paired with the app it was taken in, parsed once so a
/// command naming several refs reads and deserializes the file once.
#[derive(Default)]
pub struct Refs(BTreeMap<String, Option<String>>);

impl Refs {
    /// None for a record that does not parse.
    #[must_use]
    pub fn parse(refmap: &[u8]) -> Option<Self> {
        let map: RefMap = serde_json::from_slice(refmap).ok()?;
        Some(Self(
            map.inner
                .into_iter()
                .map(|(element, entry)| (element, entry.source_app))
                .collect(),
        ))
    }

    /// The app an unqualified ref such as `@e5` was snapshotted in; None for a ref this record does
    /// not hold or an entry naming no app.
    #[must_use]
    pub fn app(&self, element: &str) -> Option<&str> {
        self.0.get(element)?.as_deref()
    }
}
