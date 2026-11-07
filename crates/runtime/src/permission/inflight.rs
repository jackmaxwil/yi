//! The one ask a request has in flight: a second ask for the same request waits, and the guard's
//! drop releases it whether the ask settled, failed or panicked.

use std::{collections::BTreeSet, sync::Mutex};
use yi_permission::RequestId;

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
