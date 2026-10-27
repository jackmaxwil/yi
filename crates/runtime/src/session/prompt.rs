use std::sync::Arc;

use super::{Shared, extensions_of};
use crate::ext::Host;

pub(super) fn assembled_prompt(
    shared: &Arc<Shared>,
    fallback: &str,
    read: fn(&Host) -> String,
) -> String {
    let Some(host) = extensions_of(shared) else {
        return fallback.to_owned();
    };
    let assembled = host.lock().map(|host| read(&host)).unwrap_or_default();
    if assembled.is_empty() {
        fallback.to_owned()
    } else {
        assembled
    }
}
