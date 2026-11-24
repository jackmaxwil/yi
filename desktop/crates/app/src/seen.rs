//! Which feeds some window shows. The sources that read on a timer are told when the first
//! pane of theirs opens and when the last closes; a page's browser is closed with its last pane.

use std::collections::BTreeSet;

use yd_core::command::table::Run;
use yd_core::command::tools::remote;
use yd_core::feed::{ASSETS, CONTAINERS, DAEMONS, REMOTE, REPORT, SourceCmd, VMS, WEB};
use yd_core::ids::FeedId;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_panes::daemons::pane::labels;
use yd_panes::remote::pane::adapter;
use yd_ui::win::{Win, dispatch};

/// The feeds whose sources read on a timer, each with the pane kind that shows it.
const POLLED: &[(FeedId, PaneKind)] = &[
    (CONTAINERS, PaneKind::Containers),
    (DAEMONS, PaneKind::Daemons),
    (VMS, PaneKind::Vms),
    (ASSETS, PaneKind::Assets),
];
/// The feeds the `machines` source fills, told which adapter answers whenever one is shown.
const MACHINES: &[FeedId] = &[VMS, ASSETS, REMOTE];
/// The pages, each with the pane kind that shows it: Chrome runs only while one is open.
const PAGES: &[(FeedId, PaneKind)] = &[
    (WEB, PaneKind::Web),
    (REPORT, PaneKind::Report),
    (REMOTE, PaneKind::Remote),
];

/// What the sources were last told, kept in the hub so every window's poll agrees.
#[derive(Debug, Default)]
struct Told {
    seen: BTreeSet<FeedId>,
    labels: Vec<String>,
    adapter: String,
}

/// Sends the word when it changed: `Seen` to a polled source, `Close` to a page whose last pane
/// went, the launchd labels, and the machine adapter to each machines feed that is shown.
pub fn poll(win: &mut Win) {
    let asks = {
        let Ok(mut hub) = win.hub.try_borrow_mut() else {
            return;
        };
        let grid = &hub.model.grid;
        let open = |kind: PaneKind| {
            let mut panes = grid.panes.iter().filter(|pane| pane.kind == kind);
            panes.any(|pane| grid.window_of(pane.id).is_some())
        };
        let shown = POLLED.iter().chain(PAGES).filter(|(_, kind)| open(*kind));
        let seen: BTreeSet<FeedId> = shown.map(|(feed, _)| *feed).collect();
        let labels = labels(&hub.model);
        let named = adapter(&hub.model);
        let Some(told) = hub.panes.get_mut::<Told>(None) else {
            return;
        };
        // Invariant: a source starts on its first ask, so only a change is sent: a source no
        // pane ever showed is never started to be told it is unseen.
        let mut asks = Vec::new();
        if seen.contains(&DAEMONS) && told.labels != labels {
            asks.push((DAEMONS, SourceCmd::Labels(labels.clone())));
            told.labels = labels;
        }
        for feed in MACHINES {
            let (was, now) = (told.seen.contains(feed), seen.contains(feed));
            if now && (!was || told.adapter != named) {
                let ask = Run {
                    text: Some(named.clone()),
                    ..Run::new(remote::ADAPTER)
                };
                asks.push((*feed, SourceCmd::Run(ask)));
            }
        }
        told.adapter = named;
        for (feed, _) in POLLED {
            let (was, now) = (told.seen.contains(feed), seen.contains(feed));
            if was != now {
                asks.push((*feed, SourceCmd::Seen(now)));
            }
        }
        let gone = PAGES.iter().map(|(feed, _)| *feed);
        let gone = gone.filter(|feed| told.seen.contains(feed) && !seen.contains(feed));
        asks.extend(gone.map(|feed| (feed, SourceCmd::Close)));
        told.seen = seen;
        asks
    };
    for (feed, ask) in asks {
        dispatch(win, Msg::Source(feed, ask));
    }
}
