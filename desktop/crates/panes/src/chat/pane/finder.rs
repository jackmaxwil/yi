//! What the chat pane's keyed elements stand for: a prompt, an answer and a tool step.

use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::ids::PaneId;
use yd_core::kind::Kind;
use yd_core::reduce::threads::Thread;
use yd_ui::win::Hub;

use crate::chat::messages::{Block, line};
use crate::chat::tools::target;

/// The most characters of what was said that go with an attached message, and that name it.
const EXCERPT: usize = 240;
const NAME: usize = 48;

/// The message a key's index names, as the thing of `kind` its stored entry is. A message
/// the server has not stored yet has no id, so it is no thing until it has.
fn message(thread: &Thread, at: &str, kind: Kind) -> Option<Thing> {
    let message = thread.transcript.messages.get(at.parse::<usize>().ok()?)?;
    let entry = message.entry.as_ref()?;
    let id = format!("{}/{}", thread.id.as_str(), entry.as_str());
    let words = message.said()?
        .blocks
        .into_iter()
        .find_map(|block| match block {
            Block::Text(words) => Some(words),
            Block::Thinking(_) | Block::Call(_) => None,
        })?;
    Some(Thing {
        label: line(&words, NAME),
        address: Address::new(kind, id).ok()?,
        excerpt: Some(line(&words, EXCERPT)),
    })
}

/// A tool step, `<pane key>.step.<call id>` here and in the document: the call by its id.
fn step(thread: &Thread, key: &str) -> Option<Thing> {
    let (_, id) = key.rsplit_once(".step.")?;
    let call = thread.transcript.tools.get(id)?;
    let id = format!("{}/{}", thread.id.as_str(), call.id.as_str());
    let what = target(call).unwrap_or_default();
    let words = format!("{} {what}", call.name).trim().to_owned();
    Some(Thing {
        label: line(&words, NAME),
        address: Address::new(Kind::Step, id).ok()?,
        excerpt: Some(words),
    })
}

/// The chat pane's finder: a prompt's bubble is a message, an answer an answer and a tool
/// row a step, each of the thread the pane shows. Pick mode and attach reach them by it.
pub(super) fn thing_at(hub: &Hub, pane: PaneId, key: &str) -> Option<Thing> {
    let thread = super::shown(hub, pane)?;
    let (_, part) = key.split_once('.')?;
    let in_pane = part.strip_prefix("message.");
    let prompt = in_pane.and_then(|at| at.strip_suffix(".bubble"));
    let answer = in_pane.and_then(|at| at.strip_suffix(".answer"));
    let answer = answer.or_else(|| part.strip_prefix("block.answer."));
    match (prompt, answer) {
        (Some(at), _) => message(thread, at, Kind::Message),
        (None, Some(at)) => message(thread, at, Kind::Answer),
        (None, None) => step(thread, key),
    }
}
