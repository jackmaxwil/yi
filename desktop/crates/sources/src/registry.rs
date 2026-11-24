//! The feed table: every feed and the function that runs its source. A source is built by
//! filling in its own file; its row is already here.

use yd_core::feed::FeedDef;
use yd_core::ids::FeedId;

use crate::{browser, dap, docker, fetch, fs, git, launchd, logs, lsp, machines, pty};

pub const FEEDS: &[FeedDef] = &[
    FeedDef {
        id: fs::FEED,
        run: fs::run,
    },
    FeedDef {
        id: git::FEED,
        run: git::run,
    },
    FeedDef {
        id: pty::FEED,
        run: pty::run,
    },
    FeedDef {
        id: logs::FEED,
        run: logs::run,
    },
    FeedDef {
        id: browser::FEED,
        run: browser::run,
    },
    FeedDef {
        id: docker::FEED,
        run: docker::run,
    },
    FeedDef {
        id: fetch::FEED,
        run: fetch::run,
    },
    FeedDef {
        id: launchd::FEED,
        run: launchd::run,
    },
    FeedDef {
        id: machines::VMS,
        run: machines::vms,
    },
    FeedDef {
        id: machines::ASSETS,
        run: machines::assets,
    },
    FeedDef {
        id: lsp::client::FEED,
        run: lsp::client::run,
    },
    FeedDef {
        id: dap::client::FEED,
        run: dap::client::run,
    },
];

/// The row for `feed`, whichever instance of the source it names.
pub fn find(feed: FeedId) -> Option<&'static FeedDef> {
    FEEDS.iter().find(|def| def.id.source == feed.source)
}
