//! The one ask a request has in flight: a second ask for the same request waits, and the guard's
//! drop releases it whether the ask settled, failed or panicked.

use std::collections::BTreeSet;
use std::sync::Mutex;

use yi_permission::RequestId;

pub(super) struct InFlight<'a> {
    set: &'a Mutex<BTreeSet<RequestId>>,
    request: RequestId,
}

impl<'a> InFlight<'a> {
    pub(super) fn claim(set: &'a Mutex<BTreeSet<RequestId>>, request: RequestId) -> Option<Self> {
        let inserted = set
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(request);
        inserted.then(|| Self { set, request })
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.request);
    }
}
