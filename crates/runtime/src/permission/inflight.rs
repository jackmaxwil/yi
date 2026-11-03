use std::{collections::BTreeSet, sync::Mutex};
use yi_permission::RequestId;

/// One asking request in flight, released on drop: a second ask for it joins the first.
pub(crate) struct InFlight<'a> {
    set: &'a Mutex<BTreeSet<RequestId>>,
    request: RequestId,
}

impl<'a> InFlight<'a> {
    pub(crate) fn claim(set: &'a Mutex<BTreeSet<RequestId>>, request: RequestId) -> Option<Self> {
        let mut guard = set
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let inserted = guard.insert(request);
        inserted.then(|| Self { set, request })
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        let mut guard = self
            .set
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.remove(&self.request);
    }
}
