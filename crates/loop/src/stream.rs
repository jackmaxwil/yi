use tokio::sync::mpsc::Receiver;
use yi_types::event::AssistantMessageEvent;
use yi_types::model::{Effort, LlmContext, Model};

use crate::interrupt::InterruptSignal;

pub trait StreamFn: Send + Sync {
    fn stream(
        &self,
        model: &Model,
        context: &LlmContext,
        effort: Effort,
        signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent>;

    /// The TTL a loop request's history breakpoints carry; a stream with no ledger keeps five
    /// minutes (D315).
    fn cache_ttl(&self, _model: &Model) -> yi_types::model::Ttl {
        yi_types::model::Ttl::Min5
    }
    fn cache_tail(&self, _model: &Model) -> bool {
        true
    }
    /// Wall-clock ms a finished reply is stamped with, which reclaim ages the cache from; a
    /// stream with no clock leaves the reply's own time.
    fn now_ms(&self) -> Option<u64> {
        None
    }
}
